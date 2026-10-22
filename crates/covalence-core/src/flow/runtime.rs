//! The flow runtime (flow spec §5 and §8.2–8.7): matches triggers, starts
//! runs under each flow's concurrency mode, runs their steps, and keeps
//! `flow.<slug>.*` in the state tree.
//!
//! Expressions are evaluated through an [`Evaluator`]. `state` triggers are
//! re-evaluated when a path they read changes, once per batch of changes.
//!
//! Every run is journaled: `trigger.matched` → `run.started` → `run.step`
//! (started, then done, failed or skipped) → `run.finished`. An action's
//! `action.requested` points its `cause` at its step's `run.step started`, so
//! `why` on any device action walks back through the run to its trigger.

use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::Duration as StdDuration;

use covalence_protocol::Value;
use covalence_protocol::state::PathSegment;
use serde::Serialize;
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::{Edge, Flow, FlowId, OnError, SetTarget, Step, StepKind, Trigger, WaitFor};
use crate::engine::{ActionRequest, Actions, CoreError};
use crate::expr::{Evaluation, Evaluator, ExprError, Reads, Scope, to_json, truthy};
use crate::gate::Verdict;
use crate::journal::{
    BlockReason, JournalError, JournalOp, NewEntry, Origin, Record, RunError, RunFinishedRecord,
    RunStartedRecord, RunStatus, RunStepRecord, Seq, StatePatchRecord, StepDetail, StepStatus,
    TriggerMatchedRecord,
};
use crate::state::{ConnId, Dirty, Meta, Quality, Source, StatePath};
use crate::{Shared, clock, schema};

/// The most runs a `queue` flow holds waiting (flow spec §8.5).
pub const QUEUE_LIMIT: usize = 16;
/// Finished runs kept in memory for [`Runtime::runs`].
pub const RECENT_RUNS: usize = 100;
/// The state root for flow status. Core roots use their name where a
/// connection uses its ID, so this is `{conn: "flow", path: [slug, ...]}`.
pub const STATE_ROOT: &str = "flow";
/// How far the loop guard follows `cause` before giving up.
const LOOP_GUARD_DEPTH: usize = 1_000;
/// How long shutdown waits for cancelled runs to finish.
const SHUTDOWN_GRACE: StdDuration = StdDuration::from_secs(5);
/// The state root `set` writes user variables under (state spec §1.1).
pub const VARS_ROOT: &str = "vars";

pub type RunId = String;

/// A run as `GET /runs` shows it (flow spec §9).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunInfo {
    pub run: RunId,
    pub flow: FlowId,
    pub status: RunPhase,
    /// The `trigger.matched` entry.
    pub matched: Seq,
    /// The `run.started` entry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started: Option<Seq>,
    /// The `run.finished` entry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished: Option<Seq>,
    /// Milliseconds since the epoch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// While running: the current step's position.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RunError>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RunPhase {
    Queued,
    Running,
    Ok,
    Failed,
    Cancelled,
}

impl From<RunStatus> for RunPhase {
    fn from(status: RunStatus) -> Self {
        match status {
            RunStatus::Ok => RunPhase::Ok,
            RunStatus::Failed => RunPhase::Failed,
            RunStatus::Cancelled => RunPhase::Cancelled,
        }
    }
}

/// What a trigger did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fired {
    /// The `trigger.matched` entry.
    pub matched: Seq,
    pub outcome: FireOutcome,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FireOutcome {
    Started(RunId),
    Queued(RunId),
    Blocked(BlockReason),
}

#[derive(Debug, thiserror::Error)]
pub enum FireError {
    #[error("no flow {0}")]
    UnknownFlow(FlowId),
    #[error("flow {0} is disabled")]
    Disabled(FlowId),
    #[error("flow {0} has no api trigger")]
    NoApiTrigger(FlowId),
    #[error(transparent)]
    Journal(#[from] JournalError),
}

#[derive(Debug, thiserror::Error)]
pub enum CancelError {
    #[error("no run {0}")]
    UnknownRun(RunId),
    #[error("run {0} has already finished")]
    Finished(RunId, RunPhase),
    #[error(transparent)]
    Journal(#[from] JournalError),
}

pub struct Runtime {
    inner: Arc<Inner>,
    matcher: JoinHandle<()>,
    watcher: JoinHandle<()>,
}

struct Inner {
    shared: Shared,
    actions: Arc<dyn Actions>,
    evaluator: Arc<dyn Evaluator>,
    /// Connection aliases, for expressions.
    aliases: RwLock<Arc<BTreeMap<String, ConnId>>>,
    book: Mutex<Book>,
    /// `state` triggers, by flow and trigger index.
    watches: Mutex<BTreeMap<(FlowId, usize), Watch>>,
}

/// A `state` trigger's last value and what it read (spec §8.2).
#[derive(Default)]
struct Watch {
    value: Option<Value>,
    reads: Reads,
    /// The last failure, so a repeating one is logged once.
    error: Option<String>,
}

#[derive(Default)]
struct Book {
    flows: BTreeMap<FlowId, Slot>,
    /// Runs that are queued or running.
    live: BTreeMap<RunId, Live>,
    /// Finished runs, oldest first.
    recent: VecDeque<RunInfo>,
}

struct Slot {
    flow: Arc<Flow>,
    /// Started runs that haven't finished, oldest first.
    running: Vec<RunId>,
    /// `queue` runs waiting to start.
    queue: VecDeque<RunId>,
}

struct Live {
    info: RunInfo,
    /// `trigger` in the run's expressions.
    trigger: Value,
    cancel: watch::Sender<bool>,
    /// Becomes `true` once the run has journaled `run.finished`.
    done: watch::Receiver<bool>,
    /// The task's half of `cancel` and `done`, until the run is started.
    pending: Option<(watch::Receiver<bool>, watch::Sender<bool>)>,
}

impl Live {
    fn new(info: RunInfo, trigger: Value) -> Self {
        let (cancel, cancel_rx) = watch::channel(false);
        let (done_tx, done) = watch::channel(false);
        Live {
            info,
            trigger,
            cancel,
            done,
            pending: Some((cancel_rx, done_tx)),
        }
    }
}

impl Runtime {
    /// Starts the runtime, its event-trigger matcher and its state-trigger
    /// watcher. Must be called inside a tokio runtime.
    pub fn start(shared: Shared, actions: Arc<dyn Actions>, evaluator: Arc<dyn Evaluator>) -> Self {
        let inner = Arc::new(Inner {
            shared,
            actions,
            evaluator,
            aliases: RwLock::new(Arc::new(BTreeMap::new())),
            book: Mutex::new(Book::default()),
            watches: Mutex::new(BTreeMap::new()),
        });
        let matcher = tokio::spawn(match_events(inner.clone()));
        let feed = inner.shared.state.subscribe();
        let watcher = tokio::spawn(watch_state(inner.clone(), feed));
        Runtime {
            inner,
            matcher,
            watcher,
        }
    }

    /// Sets the connection aliases expressions can read, by alias.
    pub fn set_aliases(&self, aliases: BTreeMap<String, ConnId>) {
        *self
            .inner
            .aliases
            .write()
            .unwrap_or_else(|e| e.into_inner()) = Arc::new(aliases);
    }

    /// Adds or replaces a flow. Runs already active keep the copy they started with.
    pub fn set_flow(&self, flow: Flow) {
        let inner = &self.inner;
        let mut book = inner.book();
        let old_slug = book.flows.get(&flow.id).map(|s| s.flow.slug.clone());
        if let Some(old) = old_slug.filter(|old| *old != flow.slug) {
            inner.delete_state(&old);
        }
        let slot = book
            .flows
            .entry(flow.id.clone())
            .and_modify(|slot| slot.flow = Arc::new(flow.clone()))
            .or_insert_with(|| Slot {
                flow: Arc::new(flow.clone()),
                running: Vec::new(),
                queue: VecDeque::new(),
            });
        let seq = inner.shared.journal.last_seq().unwrap_or(0);
        inner.publish_running(slot, seq);
        drop(book);
        inner.baseline(&flow);
    }

    /// Removes a flow, cancels its runs and deletes `flow.<slug>`.
    pub fn remove_flow(&self, id: &str) -> Result<(), JournalError> {
        let inner = &self.inner;
        let mut book = inner.book();
        let Some(slot) = book.flows.remove(id) else {
            return Ok(());
        };
        inner.watches().retain(|(flow, _), _| flow != id);
        inner.delete_state(&slot.flow.slug);
        for run in &slot.running {
            if let Some(live) = book.live.get(run) {
                live.cancel.send_replace(true);
            }
        }
        for run in slot.queue {
            inner.drop_queued(&mut book, &run)?;
        }
        Ok(())
    }

    /// Fires a flow's `api` trigger (`POST /flows/{id}/run`).
    pub fn fire_api(&self, id: &str, origin: Origin) -> Result<Fired, FireError> {
        let flow = self
            .inner
            .book()
            .flows
            .get(id)
            .map(|slot| slot.flow.clone())
            .ok_or_else(|| FireError::UnknownFlow(id.to_owned()))?;
        if !flow.enabled {
            return Err(FireError::Disabled(id.to_owned()));
        }
        let trigger = flow
            .triggers
            .iter()
            .position(|t| matches!(t, Trigger::Api {}))
            .ok_or_else(|| FireError::NoApiTrigger(id.to_owned()))?;
        let payload = Value::Map(vec![("args".into(), Value::Map(Vec::new()))]);
        let failed = self.inner.failed_condition(&flow, &payload);
        let mut book = self.inner.book();
        if !book.flows.contains_key(id) {
            return Err(FireError::UnknownFlow(id.to_owned()));
        }
        let fire = Fire {
            trigger,
            cause: None,
            origin,
            payload,
            failed_condition: failed,
        };
        Ok(self.inner.fire(&mut book, id, fire)?)
    }

    /// Cancels a run. A queued run is dropped at once; a running one stops at
    /// its next step boundary or wait, and the returned phase is `running`.
    pub fn cancel(&self, run: &str) -> Result<RunPhase, CancelError> {
        let inner = &self.inner;
        let mut book = inner.book();
        match book.live.get(run).map(|l| l.info.status) {
            Some(RunPhase::Queued) => {
                let flow = book.live[run].info.flow.clone();
                if let Some(slot) = book.flows.get_mut(&flow) {
                    slot.queue.retain(|r| r != run);
                }
                inner.drop_queued(&mut book, run)?;
                Ok(RunPhase::Cancelled)
            }
            Some(phase) => {
                book.live[run].cancel.send_replace(true);
                Ok(phase)
            }
            None => match book.recent.iter().find(|r| r.run == run) {
                Some(info) => Err(CancelError::Finished(run.to_owned(), info.status)),
                None => Err(CancelError::UnknownRun(run.to_owned())),
            },
        }
    }

    /// Runs newest first: running, then queued, then recently finished.
    /// With `flow`, only that flow's.
    pub fn runs(&self, flow: Option<&str>) -> Vec<RunInfo> {
        let book = self.inner.book();
        let wanted = |info: &&RunInfo| flow.is_none_or(|f| info.flow == f);
        // Run IDs are ULIDs, so reverse ID order is newest first.
        let live = |phase| {
            book.live
                .values()
                .rev()
                .map(|l| &l.info)
                .filter(move |i| i.status == phase)
        };
        live(RunPhase::Running)
            .chain(live(RunPhase::Queued))
            .chain(book.recent.iter().rev())
            .filter(wanted)
            .cloned()
            .collect()
    }

    pub fn run(&self, run: &str) -> Option<RunInfo> {
        let book = self.inner.book();
        book.live
            .get(run)
            .map(|l| l.info.clone())
            .or_else(|| book.recent.iter().find(|r| r.run == run).cloned())
    }

    /// Stops matching triggers, cancels every run and waits briefly for them to finish.
    pub async fn shutdown(&self) {
        self.matcher.abort();
        self.watcher.abort();
        let waiting: Vec<watch::Receiver<bool>> = {
            let book = self.inner.book();
            for live in book.live.values() {
                live.cancel.send_replace(true);
            }
            book.live.values().map(|l| l.done.clone()).collect()
        };
        let all = async {
            for mut done in waiting {
                let _ = done.wait_for(|d| *d).await;
            }
        };
        if tokio::time::timeout(SHUTDOWN_GRACE, all).await.is_err() {
            tracing::warn!("flow runs still active at shutdown");
        }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.matcher.abort();
        self.watcher.abort();
    }
}

/// Fires `event` and `gesture` triggers from plugin events and surface
/// gestures in the journal.
async fn match_events(inner: Arc<Inner>) {
    let mut feed = inner.shared.journal.subscribe();
    loop {
        let entry = match feed.recv().await {
            Ok(entry) => entry,
            Err(broadcast::error::RecvError::Lagged(missed)) => {
                tracing::warn!(missed, "flow trigger matcher fell behind the journal");
                continue;
            }
            Err(broadcast::error::RecvError::Closed) => return,
        };
        let (candidates, payload, origin) = match &entry.record {
            Record::Event(event) => {
                let Source::Conn(conn) = &event.source else {
                    continue;
                };
                let candidates = inner.candidates(|t| {
                    matches!(t, Trigger::Event { source, event: name, .. }
                        if &source.conn == conn && name == &event.event)
                });
                let mut payload = vec![
                    (
                        "source".into(),
                        Value::Map(vec![("conn".into(), conn.as_str().into())]),
                    ),
                    ("event".into(), event.event.as_str().into()),
                ];
                if let Some(p) = &event.payload {
                    payload.push(("payload".into(), p.clone()));
                }
                (candidates, Value::Map(payload), Origin::system())
            }
            Record::InputGesture(input) => {
                let Some(page) = &input.page else { continue };
                let candidates = inner.candidates(|t| {
                    matches!(t, Trigger::Gesture { control, gesture }
                        if &control.page == page
                            && control.control == input.control
                            && *gesture == input.gesture)
                });
                let mut payload = vec![
                    ("surface".into(), input.surface.as_str().into()),
                    ("page".into(), page.as_str().into()),
                    ("control".into(), input.control.as_str().into()),
                    ("gesture".into(), input.gesture.as_str().into()),
                ];
                if let Some(delta) = input.delta {
                    payload.push(("delta".into(), delta.into()));
                }
                (candidates, Value::Map(payload), entry.origin.clone())
            }
            _ => continue,
        };
        for (flow, trigger) in candidates {
            // A false filter journals nothing (spec §8.2).
            if let Trigger::Event {
                filter: Some(filter),
                ..
            } = &flow.triggers[trigger]
                && !inner.passes(&flow, filter, &payload, "filter")
            {
                continue;
            }
            let fire = Fire {
                trigger,
                cause: Some(entry.seq),
                origin: origin.clone(),
                payload: payload.clone(),
                failed_condition: inner.failed_condition(&flow, &payload),
            };
            let mut book = inner.book();
            if !book.flows.get(&flow.id).is_some_and(|s| s.flow.enabled) {
                continue;
            }
            if let Err(e) = inner.fire(&mut book, &flow.id, fire) {
                tracing::error!(flow = %flow.id, "cannot fire flow: {e}");
            }
        }
    }
}

/// Re-evaluates `state` triggers when what they read changes (spec §8.2).
async fn watch_state(inner: Arc<Inner>, mut feed: broadcast::Receiver<Arc<Dirty>>) {
    loop {
        // Everything that changed since the last look, as one batch.
        let mut dirty = match feed.recv().await {
            Ok(dirty) => Some((*dirty).clone()),
            Err(broadcast::error::RecvError::Lagged(_)) => None,
            Err(broadcast::error::RecvError::Closed) => return,
        };
        loop {
            match feed.try_recv() {
                Ok(more) => {
                    if let Some(d) = &mut dirty {
                        d.extend(more.iter().cloned());
                    }
                }
                Err(broadcast::error::TryRecvError::Lagged(_)) => dirty = None,
                Err(_) => break,
            }
        }
        // After a lag every state trigger is evaluated again.
        inner.check_watches(dirty.as_ref());
    }
}

/// What a run adds to an expression's scope.
struct RunScope<'a> {
    local: &'a BTreeMap<String, Value>,
    steps: &'a BTreeMap<String, Value>,
    cancel: &'a watch::Receiver<bool>,
}

/// How a trigger fired: what [`Inner::fire`] needs besides the flow.
struct Fire {
    trigger: usize,
    /// The entry that fired it: an `event` or a `state.patch`.
    cause: Option<Seq>,
    origin: Origin,
    /// `trigger` in expressions.
    payload: Value,
    /// The index of the first falsy condition, which blocks the run.
    failed_condition: Option<u32>,
}

impl Inner {
    fn book(&self) -> MutexGuard<'_, Book> {
        self.book.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Each enabled flow with a trigger that `matches`, and its index.
    fn candidates(&self, matches: impl Fn(&Trigger) -> bool) -> Vec<(Arc<Flow>, usize)> {
        self.book()
            .flows
            .values()
            .filter(|slot| slot.flow.enabled)
            .filter_map(|slot| {
                let index = slot.flow.triggers.iter().position(&matches)?;
                Some((slot.flow.clone(), index))
            })
            .collect()
    }

    fn watches(&self) -> MutexGuard<'_, BTreeMap<(FlowId, usize), Watch>> {
        self.watches.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Evaluates `source` with the current aliases. `run` is a run's scope.
    fn evaluate(
        &self,
        source: &str,
        trigger: Option<&Value>,
        run: Option<RunScope<'_>>,
    ) -> Evaluation {
        let aliases = self
            .aliases
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let mut scope = Scope::new(&self.shared.state, &aliases);
        scope.trigger = trigger;
        if let Some(run) = run {
            scope.local = run.local;
            scope.steps = run.steps;
            scope.cancel = Some(run.cancel);
        }
        self.evaluator.evaluate(source, &scope)
    }

    /// Whether a filter or condition is truthy. A failure counts as false
    /// and is logged (spec §8.2).
    fn passes(&self, flow: &Flow, source: &str, trigger: &Value, what: &str) -> bool {
        match self.evaluate(source, Some(trigger), None).value {
            Ok(value) => truthy(value.as_ref()),
            Err(e) => {
                tracing::warn!(flow = %flow.id, "{what} failed, counting it as false: {e}");
                false
            }
        }
    }

    /// The index of the first of the flow's conditions that is falsy.
    fn failed_condition(&self, flow: &Flow, trigger: &Value) -> Option<u32> {
        flow.conditions
            .iter()
            .position(|c| !self.passes(flow, c, trigger, "condition"))
            .map(|i| i as u32)
    }

    /// Sets the baseline of a flow's `state` triggers, which never fires
    /// (spec §8.2). Saving a flow resets it.
    fn baseline(&self, flow: &Flow) {
        let mut fresh = BTreeMap::new();
        if flow.enabled {
            for (i, trigger) in flow.triggers.iter().enumerate() {
                if let Trigger::State { when, .. } = trigger {
                    let (value, error) = self.watch_value(flow, when, None);
                    let eval_reads = value.1;
                    fresh.insert(
                        (flow.id.clone(), i),
                        Watch {
                            value: value.0,
                            reads: eval_reads,
                            error,
                        },
                    );
                }
            }
        }
        let mut watches = self.watches();
        watches.retain(|(id, _), _| id != &flow.id);
        watches.extend(fresh);
    }

    /// Evaluates a `state` trigger's `when`: its value (`None` on failure)
    /// and reads, and the failure, logged unless it repeats `last`.
    fn watch_value(
        &self,
        flow: &Flow,
        when: &str,
        last: Option<&str>,
    ) -> ((Option<Value>, Reads), Option<String>) {
        let Evaluation { value, reads } = self.evaluate(when, None, None);
        match value {
            Ok(value) => ((value, reads), None),
            Err(e) => {
                if last != Some(e.message.as_str()) {
                    tracing::warn!(flow = %flow.id, "state trigger failed, counting it as undefined: {e}");
                }
                ((None, reads), Some(e.message))
            }
        }
    }

    /// Re-evaluates the `state` triggers that read any of `dirty` (all of
    /// them for `None`) and fires those whose edge is crossed.
    fn check_watches(self: &Arc<Self>, dirty: Option<&Dirty>) {
        let due: Vec<((FlowId, usize), Option<String>)> = self
            .watches()
            .iter()
            .filter(|(_, w)| dirty.is_none_or(|d| w.reads.touched_by(d)))
            .map(|(k, w)| (k.clone(), w.error.clone()))
            .collect();
        for ((flow_id, index), last_error) in due {
            let Some(flow) = self.book().flows.get(&flow_id).map(|s| s.flow.clone()) else {
                continue;
            };
            let Some(Trigger::State { when, edge }) = flow.triggers.get(index) else {
                continue;
            };
            let ((value, reads), error) = self.watch_value(&flow, when, last_error.as_deref());
            let cause = dirty.and_then(|d| self.cause_of(&reads, d));
            let previous = {
                let mut watches = self.watches();
                let Some(watch) = watches.get_mut(&(flow_id.clone(), index)) else {
                    continue;
                };
                watch.reads = reads;
                watch.error = error;
                std::mem::replace(&mut watch.value, value.clone())
            };
            let fires = match edge {
                Edge::Rising => !truthy(previous.as_ref()) && truthy(value.as_ref()),
                Edge::Falling => truthy(previous.as_ref()) && !truthy(value.as_ref()),
                Edge::Change => previous != value,
            };
            if !fires {
                continue;
            }
            let mut payload = Vec::new();
            if let Some(v) = value {
                payload.push(("value".into(), v));
            }
            if let Some(p) = previous {
                payload.push(("previous".into(), p));
            }
            let payload = Value::Map(payload);
            let fire = Fire {
                trigger: index,
                cause,
                origin: Origin::system(),
                failed_condition: self.failed_condition(&flow, &payload),
                payload,
            };
            let mut book = self.book();
            if !book.flows.get(&flow_id).is_some_and(|s| s.flow.enabled) {
                continue;
            }
            if let Err(e) = self.fire(&mut book, &flow_id, fire) {
                tracing::error!(flow = %flow_id, "cannot fire flow: {e}");
            }
        }
    }

    /// The latest journal entry that set a changed leaf the reads cover.
    fn cause_of(&self, reads: &Reads, dirty: &Dirty) -> Option<Seq> {
        self.shared.state.read(|tree| {
            dirty
                .iter()
                .filter(|d| reads.paths.iter().any(|r| d.is_within(r) || r.is_within(d)))
                .filter_map(|d| tree.leaf(d).map(|leaf| leaf.meta.seq))
                .max()
        })
    }

    /// Journals `trigger.matched` and starts, queues or blocks a run: for a
    /// loop (spec §8.6), a falsy condition, or the flow's concurrency mode.
    fn fire(
        self: &Arc<Self>,
        book: &mut Book,
        flow_id: &str,
        fire: Fire,
    ) -> Result<Fired, JournalError> {
        let Fire {
            trigger,
            cause,
            origin,
            payload,
            failed_condition,
        } = fire;
        use super::Concurrency::*;
        let slot = book
            .flows
            .get(flow_id)
            .expect("callers check the flow exists");
        let flow = slot.flow.clone();
        let busy = !slot.running.is_empty() || !slot.queue.is_empty();
        enum Plan {
            Start,
            Queue,
            Block(BlockReason),
        }
        let plan = if cause.is_some_and(|seq| self.descends_from_run(flow_id, seq)) {
            Plan::Block(BlockReason::Loop)
        } else if failed_condition.is_some() {
            Plan::Block(BlockReason::Condition)
        } else {
            match flow.concurrency {
                Parallel | Restart => Plan::Start,
                Ignore if busy => Plan::Block(BlockReason::Ignore),
                Ignore => Plan::Start,
                Queue if !busy => Plan::Start,
                Queue if slot.queue.len() >= QUEUE_LIMIT => Plan::Block(BlockReason::QueueFull),
                Queue => Plan::Queue,
            }
        };
        let blocked = match plan {
            Plan::Block(reason) => Some(reason),
            _ => None,
        };
        let matched = self
            .shared
            .journal
            .append(
                NewEntry::new(
                    origin,
                    Record::TriggerMatched(TriggerMatchedRecord {
                        flow: flow_id.to_owned(),
                        trigger: trigger as u32,
                        blocked: blocked.map(|_| true),
                        reason: blocked,
                        condition: failed_condition
                            .filter(|_| blocked == Some(BlockReason::Condition)),
                    }),
                )
                .cause(cause),
            )?
            .seq;
        if let Some(reason) = blocked {
            return Ok(Fired {
                matched,
                outcome: FireOutcome::Blocked(reason),
            });
        }

        let run = ulid::Ulid::generate().to_string();
        let live = Live::new(
            RunInfo {
                run: run.clone(),
                flow: flow_id.to_owned(),
                status: RunPhase::Queued,
                matched,
                started: None,
                finished: None,
                started_at: None,
                duration_ms: None,
                position: None,
                error: None,
            },
            payload,
        );
        book.live.insert(run.clone(), live);
        let slot = book.flows.get_mut(flow_id).expect("checked above");
        if let Plan::Queue = plan {
            slot.queue.push_back(run.clone());
            return Ok(Fired {
                matched,
                outcome: FireOutcome::Queued(run),
            });
        }

        // `restart`: the new run starts once the active ones have finished.
        let mut wait_for = Vec::new();
        if flow.concurrency == Restart {
            for old in &slot.running {
                let old = &book.live[old];
                old.cancel.send_replace(true);
                wait_for.push(old.done.clone());
            }
        }
        self.start(book, &run, wait_for);
        Ok(Fired {
            matched,
            outcome: FireOutcome::Started(run),
        })
    }

    /// Whether the entry `seq`, or one it descends from through `cause`, is
    /// a run of `flow` (flow spec §8.6).
    fn descends_from_run(&self, flow: &str, seq: Seq) -> bool {
        let mut next = Some(seq);
        for _ in 0..LOOP_GUARD_DEPTH {
            let Some(seq) = next else { return false };
            let Ok(Some(entry)) = self.shared.journal.get(seq) else {
                return false;
            };
            if let Record::RunStarted(started) = &entry.record
                && started.flow == flow
            {
                return true;
            }
            next = entry.cause;
        }
        false
    }

    /// Moves a queued (or just-fired) run to running and spawns its task.
    fn start(self: &Arc<Self>, book: &mut Book, run: &str, wait_for: Vec<watch::Receiver<bool>>) {
        let live = book.live.get_mut(run).expect("runs are started while live");
        let (cancel, done) = live.pending.take().expect("a run is started once");
        let trigger = live.trigger.clone();
        live.info.status = RunPhase::Running;
        let flow_id = live.info.flow.clone();
        let matched = live.info.matched;
        let slot = book
            .flows
            .get_mut(&flow_id)
            .expect("runs start while their flow exists");
        slot.running.push(run.to_owned());
        let task = RunTask {
            inner: self.clone(),
            flow: slot.flow.clone(),
            run: run.to_owned(),
            matched,
            cancel,
            trigger,
            local: BTreeMap::new(),
            steps: BTreeMap::new(),
            started: 0,
        };
        tokio::spawn(async move {
            for mut old in wait_for {
                let _ = old.wait_for(|d| *d).await;
            }
            task.run().await;
            done.send_replace(true);
        });
    }

    /// Ends a queued run that never started: `run.finished cancelled`, caused
    /// by its `trigger.matched`.
    fn drop_queued(&self, book: &mut Book, run: &str) -> Result<(), JournalError> {
        let Some(mut live) = book.live.remove(run) else {
            return Ok(());
        };
        let entry = self.shared.journal.append(
            NewEntry::new(
                Origin::Flow {
                    flow: live.info.flow.clone(),
                    run: run.to_owned(),
                    step: None,
                },
                Record::RunFinished(RunFinishedRecord {
                    run: run.to_owned(),
                    status: RunStatus::Cancelled,
                    error: None,
                }),
            )
            .cause(live.info.matched)
            .run(run.to_owned()),
        )?;
        live.info.status = RunPhase::Cancelled;
        live.info.finished = Some(entry.seq);
        if let Some((_, done)) = live.pending.take() {
            done.send_replace(true);
        }
        push_recent(book, live.info);
        Ok(())
    }

    fn state_path(slug: &str, rest: &[&str]) -> StatePath {
        let mut path = vec![PathSegment::Key(slug.to_owned())];
        path.extend(rest.iter().map(|s| PathSegment::Key((*s).to_owned())));
        StatePath::new(STATE_ROOT, path)
    }

    fn meta(seq: Seq) -> Meta {
        Meta {
            ts: clock::epoch_ms(),
            seq,
            quality: Quality::Good,
            source: Source::Core,
        }
    }

    fn publish_running(&self, slot: &Slot, seq: Seq) {
        let path = Self::state_path(&slot.flow.slug, &["running"]);
        let count = Value::from(slot.running.len() as u64);
        self.shared
            .state
            .mutate(|tree| ((), tree.set(&path, &count, &Self::meta(seq))));
    }

    fn publish_last_run(&self, slug: &str, info: &RunInfo, seq: Seq) {
        let mut last = vec![
            ("status".into(), status_name(info.status).into()),
            ("startedAt".into(), info.started_at.unwrap_or(0).into()),
            ("durationMs".into(), info.duration_ms.unwrap_or(0).into()),
        ];
        if let Some(error) = &info.error {
            last.push((
                "error".into(),
                Value::Map(vec![
                    ("code".into(), error.code.as_str().into()),
                    ("message".into(), error.message.as_str().into()),
                ]),
            ));
        }
        let path = Self::state_path(slug, &["lastRun"]);
        self.shared
            .state
            .mutate(|tree| ((), tree.set(&path, &Value::Map(last), &Self::meta(seq))));
    }

    fn delete_state(&self, slug: &str) {
        let path = Self::state_path(slug, &[]);
        self.shared.state.mutate(|tree| ((), tree.delete(&path)));
    }
}

fn status_name(phase: RunPhase) -> &'static str {
    match phase {
        RunPhase::Queued => "queued",
        RunPhase::Running => "running",
        RunPhase::Ok => "ok",
        RunPhase::Failed => "failed",
        RunPhase::Cancelled => "cancelled",
    }
}

fn push_recent(book: &mut Book, info: RunInfo) {
    if book.recent.len() == RECENT_RUNS {
        book.recent.pop_front();
    }
    book.recent.push_back(info);
}

/// One run, on its own task.
struct RunTask {
    inner: Arc<Inner>,
    flow: Arc<Flow>,
    run: RunId,
    matched: Seq,
    cancel: watch::Receiver<bool>,
    /// `trigger` in expressions.
    trigger: Value,
    /// `local` in expressions: set by `set` steps.
    local: BTreeMap<String, Value>,
    /// `steps.<id>` in expressions, for the steps that have finished.
    steps: BTreeMap<String, Value>,
    /// The `run.started` entry, which every `run.step` points at.
    started: Seq,
}

/// How a list of steps ended.
enum End {
    Ok,
    Failed(RunError),
    Cancelled,
}

/// What a step that finished well leaves: its result, and for a `branch`
/// the arm it chose.
#[derive(Default)]
struct Done {
    result: Option<Value>,
    arm: Option<serde_json::Value>,
}

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

fn expr_error(e: ExprError) -> RunError {
    RunError::new(e.code.as_str(), e.message)
}

fn cancelled_error() -> RunError {
    RunError::new("cancelled", "the run was cancelled")
}

impl RunTask {
    async fn run(mut self) {
        let outcome = self.execute().await;
        let inner = self.inner.clone();
        let mut book = inner.book();
        let Some(mut live) = book.live.remove(&self.run) else {
            return;
        };
        match outcome {
            Ok((status, error, finished, duration_ms)) => {
                live.info.status = status.into();
                live.info.error = error;
                live.info.finished = Some(finished);
                live.info.duration_ms = duration_ms;
            }
            Err(e) => {
                tracing::error!(run = %self.run, "flow run stopped: cannot journal: {e}");
                live.info.status = RunPhase::Failed;
                live.info.error = Some(RunError::new("internal", e.to_string()));
            }
        }
        live.info.position = None;
        let seq = live.info.finished.unwrap_or(0);

        let slug = self.flow.slug.clone();
        let mut next = None;
        if let Some(slot) = book.flows.get_mut(&self.flow.id) {
            slot.running.retain(|r| *r != self.run);
            if slot.running.is_empty() {
                next = slot.queue.pop_front();
            }
            // A slug change since the run started already moved the state.
            if slot.flow.slug == slug {
                inner.publish_last_run(&slug, &live.info, seq);
            }
        }
        push_recent(&mut book, live.info);
        if let Some(next) = next {
            inner.start(&mut book, &next, Vec::new());
        }
        if let Some(slot) = book.flows.get(&self.flow.id) {
            inner.publish_running(slot, seq);
        }
    }

    fn origin(&self, step: Option<&str>) -> Origin {
        Origin::Flow {
            flow: self.flow.id.clone(),
            run: self.run.clone(),
            step: step.map(str::to_owned),
        }
    }

    fn append(&self, step: Option<&str>, record: Record, cause: Seq) -> Result<Seq, JournalError> {
        Ok(self
            .inner
            .shared
            .journal
            .append(
                NewEntry::new(self.origin(step), record)
                    .cause(cause)
                    .run(self.run.clone()),
            )?
            .seq)
    }

    fn cancelled(&self) -> bool {
        *self.cancel.borrow()
    }

    fn scope(&self) -> RunScope<'_> {
        RunScope {
            local: &self.local,
            steps: &self.steps,
            cancel: &self.cancel,
        }
    }

    /// Evaluates an expression in the run's scope.
    fn eval(&self, source: &str) -> Result<Option<Value>, RunError> {
        self.inner
            .evaluate(source, Some(&self.trigger), Some(self.scope()))
            .value
            .map_err(|e| {
                if *self.cancel.borrow() {
                    cancelled_error()
                } else {
                    expr_error(e)
                }
            })
    }

    /// Journals the run from `run.started` to `run.finished`. Returns how it
    /// ended, its error, the `run.finished` seq and its duration.
    async fn execute(
        &mut self,
    ) -> Result<(RunStatus, Option<RunError>, Seq, Option<u64>), JournalError> {
        if self.cancelled() {
            // Cancelled before it started (restart, delete): no run.started.
            let finished = self.append(
                None,
                Record::RunFinished(RunFinishedRecord {
                    run: self.run.clone(),
                    status: RunStatus::Cancelled,
                    error: None,
                }),
                self.matched,
            )?;
            return Ok((RunStatus::Cancelled, None, finished, None));
        }

        let started = self.append(
            None,
            Record::RunStarted(RunStartedRecord {
                flow: self.flow.id.clone(),
                run: self.run.clone(),
                mode: self.flow.concurrency,
                inputs: Default::default(),
            }),
            self.matched,
        )?;
        self.started = started;
        let started_at = clock::epoch_ms();
        let clock = Instant::now();
        {
            let mut book = self.inner.book();
            if let Some(live) = book.live.get_mut(&self.run) {
                live.info.started = Some(started);
                live.info.started_at = Some(started_at);
            }
            if let Some(slot) = book.flows.get(&self.flow.id) {
                self.inner.publish_running(slot, started);
            }
        }

        let flow = self.flow.clone();
        let (mut status, run_error) = match self.run_list(&flow.body, &[]).await? {
            End::Ok => (RunStatus::Ok, None),
            End::Failed(e) => (RunStatus::Failed, Some(e)),
            End::Cancelled => (RunStatus::Cancelled, None),
        };
        // A cancel that lands during the last step still counts.
        if status == RunStatus::Ok && self.cancelled() {
            status = RunStatus::Cancelled;
        }

        let finished = self.append(
            None,
            Record::RunFinished(RunFinishedRecord {
                run: self.run.clone(),
                status,
                error: run_error.clone(),
            }),
            started,
        )?;
        let duration_ms = clock.elapsed().as_millis() as u64;
        Ok((status, run_error, finished, Some(duration_ms)))
    }

    /// Runs a list of steps in order, journaling each. `prefix` is the
    /// position of the block they're in.
    fn run_list<'a>(
        &'a mut self,
        steps: &'a [Step],
        prefix: &'a [String],
    ) -> BoxFuture<'a, Result<End, JournalError>> {
        Box::pin(async move {
            for step in steps {
                if self.cancelled() {
                    return Ok(End::Cancelled);
                }
                let mut position = prefix.to_vec();
                position.push(step.id.clone());
                let run = self.run.clone();
                let record = |status, arm, error| {
                    Record::RunStep(RunStepRecord {
                        run: run.clone(),
                        step: step.id.clone(),
                        status,
                        detail: Some(StepDetail {
                            position: position.clone(),
                            arm,
                            error,
                        }),
                    })
                };
                if !step.is_enabled() {
                    let skipped = record(StepStatus::Skipped, None, None);
                    self.append(Some(&step.id), skipped, self.started)?;
                    continue;
                }
                let started = record(StepStatus::Started, None, None);
                let step_started = self.append(Some(&step.id), started, self.started)?;
                if let Some(live) = self.inner.book().live.get_mut(&self.run) {
                    live.info.position = Some(position.clone());
                }
                let started_at = clock::epoch_ms();
                let clock = Instant::now();

                let outcome = self.step(step, step_started, &position).await?;
                let mut summary = vec![
                    ("startedAt".into(), Value::from(started_at)),
                    (
                        "durationMs".into(),
                        Value::from(clock.elapsed().as_millis() as u64),
                    ),
                ];
                match outcome {
                    Ok(done) => {
                        summary.insert(0, ("status".into(), "done".into()));
                        if let Some(result) = done.result {
                            summary.push(("result".into(), result));
                        }
                        self.steps.insert(step.id.clone(), Value::Map(summary));
                        let done = record(StepStatus::Done, done.arm, None);
                        self.append(Some(&step.id), done, self.started)?;
                    }
                    Err(error) => {
                        summary.insert(0, ("status".into(), "failed".into()));
                        summary.push((
                            "error".into(),
                            Value::Map(vec![
                                ("code".into(), error.code.as_str().into()),
                                ("message".into(), error.message.as_str().into()),
                            ]),
                        ));
                        self.steps.insert(step.id.clone(), Value::Map(summary));
                        let failed = record(StepStatus::Failed, None, Some(error.clone()));
                        self.append(Some(&step.id), failed, self.started)?;
                        if error.code == "cancelled" {
                            return Ok(End::Cancelled);
                        }
                        if step.on_error.unwrap_or_default() == OnError::Fail {
                            return Ok(End::Failed(error));
                        }
                    }
                }
            }
            Ok(End::Ok)
        })
    }

    /// Runs one enabled step under its timeout. The inner `Err` is the
    /// step's error; journaling the step is the caller's.
    async fn step(
        &mut self,
        step: &Step,
        step_started: Seq,
        position: &[String],
    ) -> Result<Result<Done, RunError>, JournalError> {
        let timeout = step.timeout.as_ref().map(|t| t.get());
        let timed_out = || {
            RunError::new(
                "timeout",
                format!(
                    "step {} took longer than {}",
                    step.id,
                    step.timeout.as_ref().map_or("", |t| t.as_str())
                ),
            )
        };
        let id = step.id.as_str();
        let work = async {
            match &step.kind {
                StepKind::Action {
                    target,
                    action,
                    params,
                } => Ok(self
                    .action(id, &target.conn, action, params, step_started, timeout)
                    .await),
                StepKind::Wait { duration } => Ok(self.wait(duration).await),
                StepKind::WaitUntil { until } => Ok(self.wait_until(until).await),
                StepKind::Set { path, value } => self.set(id, path, value, step_started),
                StepKind::Branch { arms, otherwise } => {
                    self.branch(arms, otherwise.as_deref(), position).await
                }
            }
        };
        // An action enforces its own timeout, so its request keeps running.
        match timeout {
            Some(t) if !matches!(step.kind, StepKind::Action { .. }) => {
                match tokio::time::timeout(t, work).await {
                    Ok(outcome) => outcome,
                    Err(_) => Ok(Err(timed_out())),
                }
            }
            _ => work.await,
        }
    }

    async fn action(
        &mut self,
        step: &str,
        conn: &ConnId,
        action: &str,
        params: &serde_json::Map<String, serde_json::Value>,
        step_started: Seq,
        timeout: Option<StdDuration>,
    ) -> Result<Done, RunError> {
        // Expression parameters are evaluated now; an `undefined` one is left out.
        let mut computed = serde_json::Map::new();
        let mut any_expr = false;
        for (name, value) in params {
            match super::param_expr(value) {
                Some(source) => {
                    any_expr = true;
                    if let Some(v) = self.eval(source)? {
                        computed.insert(name.clone(), to_json(&v));
                    }
                }
                None => {
                    computed.insert(name.clone(), value.clone());
                }
            }
        }
        if any_expr && let Some(schema) = self.inner.actions.params_schema(conn, action).await {
            let computed = serde_json::Value::Object(computed.clone());
            let problems = schema::check(&schema, &computed);
            if !problems.is_empty() {
                let why: Vec<String> = problems.iter().map(|m| m.message.clone()).collect();
                return Err(RunError::new(
                    "invalidParams",
                    format!("{} (params: {computed})", why.join("; ")),
                ));
            }
        }

        let mut req = ActionRequest::new(
            self.origin(Some(step)),
            conn.clone(),
            action.to_owned(),
            covalence_protocol::to_value(&computed).expect("JSON encodes as MessagePack"),
        );
        req.cause = Some(step_started);
        req.run = Some(self.run.clone());
        req.deadline = timeout.map(|t| t.as_millis().min(u32::MAX as u128) as u32);
        // On its own task, so a timeout leaves the request running and its
        // late `action.result` is still journaled. A cancel waits for it
        // (flow spec §8.5).
        let actions = self.inner.actions.clone();
        let call = tokio::spawn(async move { actions.request_action(req).await });
        let outcome = match timeout {
            Some(t) => tokio::time::timeout(t, call).await.map_err(|_| {
                RunError::new("timeout", format!("step {step} took longer than {t:?}"))
            })?,
            None => call.await,
        };
        let outcome =
            outcome.map_err(|e| RunError::new("internal", format!("action task failed: {e}")))?;
        match outcome {
            Err(CoreError::UnknownConnection(conn)) => Err(RunError::new(
                "unavailable",
                format!("connection {conn} is not running"),
            )),
            Err(e) => Err(RunError::new("internal", e.to_string())),
            Ok(outcome) => match (outcome.verdict, outcome.result) {
                (Verdict::Suppress(reason), _) => Err(RunError::new(
                    "suppressed",
                    format!(
                        "the output gate suppressed the action: {}",
                        serde_json::to_value(reason)
                            .ok()
                            .and_then(|v| v.as_str().map(str::to_owned))
                            .unwrap_or_default()
                    ),
                )),
                (_, Some(Err(e))) => Err(RunError::new(
                    serde_json::to_value(e.code)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_owned))
                        .unwrap_or_else(|| "action_failed".into()),
                    e.message,
                )),
                (_, Some(Ok(result))) => Ok(Done { result, arm: None }),
                (_, None) => Ok(Done::default()),
            },
        }
    }

    /// Sleeps, or returns `cancelled` when the run is cancelled.
    async fn sleep(&mut self, duration: StdDuration) -> Result<(), RunError> {
        let cancel = &mut self.cancel;
        let interrupted = async {
            if cancel.wait_for(|c| *c).await.is_err() {
                std::future::pending::<()>().await;
            }
        };
        tokio::select! {
            () = tokio::time::sleep(duration) => Ok(()),
            () = interrupted => Err(cancelled_error()),
        }
    }

    async fn wait(&mut self, duration: &WaitFor) -> Result<Done, RunError> {
        let duration = match duration {
            WaitFor::Literal(d) => d.get(),
            WaitFor::Expr(source) => {
                let bad = |v: &dyn std::fmt::Debug| {
                    RunError::new(
                        "invalidDuration",
                        format!(
                            "{v:?} is not a duration: give a string like \"2s\" or milliseconds"
                        ),
                    )
                };
                match self.eval(source)? {
                    Some(Value::String(s)) => {
                        let text = s.as_str().unwrap_or_default();
                        text.parse::<super::Duration>()
                            .map_err(|_| bad(&text))?
                            .get()
                    }
                    Some(Value::Integer(i)) => match i.as_u64() {
                        Some(ms) => StdDuration::from_millis(ms),
                        None => return Err(bad(&i)),
                    },
                    Some(Value::F64(f)) if f.is_finite() && f >= 0.0 => {
                        StdDuration::from_secs_f64(f / 1000.0)
                    }
                    other => return Err(bad(&other)),
                }
            }
        };
        self.sleep(duration).await?;
        Ok(Done::default())
    }

    /// Done as soon as `until` is truthy: now, or after a change to what it
    /// reads (spec §8.3).
    async fn wait_until(&mut self, until: &str) -> Result<Done, RunError> {
        let mut feed = self.inner.shared.state.subscribe();
        loop {
            let Evaluation { value, reads } =
                self.inner
                    .evaluate(until, Some(&self.trigger), Some(self.scope()));
            let value = value.map_err(|e| {
                if self.cancelled() {
                    cancelled_error()
                } else {
                    expr_error(e)
                }
            })?;
            if truthy(value.as_ref()) {
                let mut result = Vec::new();
                if let Some(v) = value {
                    result.push(("value".into(), v));
                }
                return Ok(Done {
                    result: Some(Value::Map(result)),
                    arm: None,
                });
            }
            // Wait for a change it read, or a cancel.
            loop {
                let cancel = &mut self.cancel;
                let interrupted = async {
                    if cancel.wait_for(|c| *c).await.is_err() {
                        std::future::pending::<()>().await;
                    }
                };
                let changed = tokio::select! {
                    changed = feed.recv() => changed,
                    () = interrupted => return Err(cancelled_error()),
                };
                match changed {
                    Ok(dirty) if !reads.touched_by(&dirty) => continue,
                    Err(broadcast::error::RecvError::Closed) => {
                        return Err(RunError::new("internal", "state feed closed"));
                    }
                    // A change it read, or missed changes: evaluate again.
                    _ => break,
                }
            }
        }
    }

    /// Writes `local.<name>` or `vars.<name>`; `undefined` removes it.
    fn set(
        &mut self,
        step: &str,
        path: &str,
        value: &str,
        step_started: Seq,
    ) -> Result<Result<Done, RunError>, JournalError> {
        let Some((target, name)) = super::parse_set_path(path) else {
            return Ok(Err(RunError::new(
                "badPath",
                format!("{path:?} can't be set"),
            )));
        };
        let value = match self.eval(value) {
            Ok(v) => v,
            Err(e) => return Ok(Err(e)),
        };
        match target {
            SetTarget::Local => {
                match value {
                    Some(v) => self.local.insert(name.to_owned(), v),
                    None => self.local.remove(name),
                };
            }
            SetTarget::Vars => {
                let at = StatePath::new(VARS_ROOT, vec![PathSegment::Key(name.to_owned())]);
                let op = match &value {
                    Some(v) => JournalOp::Set {
                        path: at.clone(),
                        value: v.clone(),
                    },
                    None => JournalOp::Delete {
                        path: at.clone(),
                        delete: covalence_protocol::True,
                    },
                };
                let entry = NewEntry::new(
                    self.origin(Some(step)),
                    Record::StatePatch(StatePatchRecord {
                        ops: vec![op],
                        replaces: None,
                    }),
                )
                .cause(step_started)
                .run(self.run.clone());
                let journal = &self.inner.shared.journal;
                self.inner.shared.state.mutate(|tree| {
                    let seq = match journal.append(entry) {
                        Ok(e) => e.seq,
                        Err(e) => return (Err(e), Dirty::new()),
                    };
                    let dirty = match &value {
                        Some(v) => tree.set(&at, v, &Inner::meta(seq)),
                        None => tree.delete(&at),
                    };
                    (Ok(()), dirty)
                })?;
            }
        }
        Ok(Ok(Done::default()))
    }

    /// Runs the first arm whose `when` is truthy, or `else` (spec §8.3).
    async fn branch(
        &mut self,
        arms: &[super::Arm],
        otherwise: Option<&[Step]>,
        position: &[String],
    ) -> Result<Result<Done, RunError>, JournalError> {
        let mut chosen = None;
        for (i, arm) in arms.iter().enumerate() {
            match self.eval(&arm.when) {
                Ok(v) if truthy(v.as_ref()) => {
                    chosen = Some((serde_json::Value::from(i), arm.steps.as_slice()));
                    break;
                }
                Ok(_) => {}
                Err(e) => return Ok(Err(e)),
            }
        }
        let chosen = chosen.or_else(|| otherwise.map(|steps| ("else".into(), steps)));
        let Some((arm, steps)) = chosen else {
            return Ok(Ok(Done {
                result: None,
                arm: Some(serde_json::Value::Null),
            }));
        };
        Ok(match self.run_list(steps, position).await? {
            End::Ok => Ok(Done {
                result: None,
                arm: Some(arm),
            }),
            End::Failed(e) => Err(e),
            End::Cancelled => Err(cancelled_error()),
        })
    }
}

//! The flow runtime (flow spec §5 and §8.5–8.6): matches triggers, starts
//! runs under each flow's concurrency mode, runs their steps, and keeps
//! `flow.<slug>.*` in the state tree.
//!
//! Every run is journaled: `trigger.matched` → `run.started` → `run.step`
//! (started, then done, failed or skipped) → `run.finished`. An action's
//! `action.requested` points its `cause` at its step's `run.step started`, so
//! `why` on any device action walks back through the run to its trigger.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration as StdDuration;

use covalence_protocol::Value;
use covalence_protocol::state::PathSegment;
use serde::Serialize;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::{Flow, FlowId, OnError, Step, StepKind, Trigger};
use crate::engine::{ActionRequest, Actions, CoreError};
use crate::gate::Verdict;
use crate::journal::{
    BlockReason, Entry, JournalError, NewEntry, Origin, Record, RunError, RunFinishedRecord,
    RunStartedRecord, RunStatus, RunStepRecord, Seq, StepDetail, StepStatus, TriggerMatchedRecord,
};
use crate::state::{Meta, Quality, Source, StatePath};
use crate::{Shared, clock};

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
}

struct Inner {
    shared: Shared,
    actions: Arc<dyn Actions>,
    book: Mutex<Book>,
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
    cancel: watch::Sender<bool>,
    /// Becomes `true` once the run has journaled `run.finished`.
    done: watch::Receiver<bool>,
    /// The task's half of `cancel` and `done`, until the run is started.
    pending: Option<(watch::Receiver<bool>, watch::Sender<bool>)>,
}

impl Live {
    fn new(info: RunInfo) -> Self {
        let (cancel, cancel_rx) = watch::channel(false);
        let (done_tx, done) = watch::channel(false);
        Live {
            info,
            cancel,
            done,
            pending: Some((cancel_rx, done_tx)),
        }
    }
}

impl Runtime {
    /// Starts the runtime and its event-trigger matcher. Must be called inside
    /// a tokio runtime.
    pub fn start(shared: Shared, actions: Arc<dyn Actions>) -> Self {
        let inner = Arc::new(Inner {
            shared,
            actions,
            book: Mutex::new(Book::default()),
        });
        let matcher = tokio::spawn(match_events(inner.clone()));
        Runtime { inner, matcher }
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
    }

    /// Removes a flow, cancels its runs and deletes `flow.<slug>`.
    pub fn remove_flow(&self, id: &str) -> Result<(), JournalError> {
        let inner = &self.inner;
        let mut book = inner.book();
        let Some(slot) = book.flows.remove(id) else {
            return Ok(());
        };
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
        let mut book = self.inner.book();
        let slot = book
            .flows
            .get(id)
            .ok_or_else(|| FireError::UnknownFlow(id.to_owned()))?;
        if !slot.flow.enabled {
            return Err(FireError::Disabled(id.to_owned()));
        }
        let trigger = slot
            .flow
            .triggers
            .iter()
            .position(|t| matches!(t, Trigger::Api {}))
            .ok_or_else(|| FireError::NoApiTrigger(id.to_owned()))?;
        Ok(self.inner.fire(&mut book, id, trigger, None, origin)?)
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
    }
}

/// Fires `event` triggers from plugin events in the journal.
async fn match_events(inner: Arc<Inner>) {
    let mut feed = inner.shared.journal.subscribe();
    loop {
        let entry = match feed.recv().await {
            Ok(entry) => entry,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(missed)) => {
                tracing::warn!(missed, "flow trigger matcher fell behind the journal");
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
        };
        let Record::Event(event) = &entry.record else {
            continue;
        };
        let Source::Conn(conn) = &event.source else {
            continue;
        };
        let mut book = inner.book();
        let matches: Vec<(FlowId, usize)> = book
            .flows
            .values()
            .filter(|slot| slot.flow.enabled)
            .filter_map(|slot| {
                let index = slot.flow.triggers.iter().position(|t| {
                    matches!(t, Trigger::Event { source, event: name }
                        if &source.conn == conn && name == &event.event)
                })?;
                Some((slot.flow.id.clone(), index))
            })
            .collect();
        for (flow, trigger) in matches {
            if let Err(e) = inner.fire(&mut book, &flow, trigger, Some(&entry), Origin::system()) {
                tracing::error!(%flow, "cannot fire flow: {e}");
            }
        }
    }
}

impl Inner {
    fn book(&self) -> MutexGuard<'_, Book> {
        self.book.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Journals `trigger.matched` and starts, queues or blocks a run under
    /// the flow's concurrency mode. `event` is the entry that fired an event
    /// trigger; an `api` trigger has none.
    fn fire(
        self: &Arc<Self>,
        book: &mut Book,
        flow_id: &str,
        trigger: usize,
        event: Option<&Entry>,
        origin: Origin,
    ) -> Result<Fired, JournalError> {
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
        let plan = if event.is_some_and(|e| self.descends_from_run(flow_id, e)) {
            Plan::Block(BlockReason::Loop)
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
                    }),
                )
                .cause(event.map(|e| e.seq)),
            )?
            .seq;
        if let Some(reason) = blocked {
            return Ok(Fired {
                matched,
                outcome: FireOutcome::Blocked(reason),
            });
        }

        let run = ulid::Ulid::generate().to_string();
        let live = Live::new(RunInfo {
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
        });
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

    /// Whether `entry` descends, through `cause`, from a run of `flow`
    /// (flow spec §8.6).
    fn descends_from_run(&self, flow: &str, entry: &Entry) -> bool {
        let mut next = entry.cause;
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
        let mut status = RunStatus::Ok;
        let mut run_error = None;
        for step in &flow.body {
            if self.cancelled() {
                status = RunStatus::Cancelled;
                break;
            }
            let position = vec![step.id.clone()];
            let run = self.run.clone();
            let step_record = |status, error| {
                Record::RunStep(RunStepRecord {
                    run: run.clone(),
                    step: step.id.clone(),
                    status,
                    detail: Some(StepDetail {
                        position: position.clone(),
                        error,
                    }),
                })
            };
            if !step.is_enabled() {
                self.append(
                    Some(&step.id),
                    step_record(StepStatus::Skipped, None),
                    started,
                )?;
                continue;
            }
            let step_started = self.append(
                Some(&step.id),
                step_record(StepStatus::Started, None),
                started,
            )?;
            if let Some(live) = self.inner.book().live.get_mut(&self.run) {
                live.info.position = Some(position.clone());
            }

            match self.step(step, step_started).await {
                Ok(()) => {
                    self.append(Some(&step.id), step_record(StepStatus::Done, None), started)?;
                }
                Err(error) => {
                    self.append(
                        Some(&step.id),
                        step_record(StepStatus::Failed, Some(error.clone())),
                        started,
                    )?;
                    if error.code == "cancelled" {
                        status = RunStatus::Cancelled;
                        break;
                    }
                    if step.on_error.unwrap_or_default() == OnError::Fail {
                        status = RunStatus::Failed;
                        run_error = Some(error);
                        break;
                    }
                }
            }
        }
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

    /// Runs one enabled step. `Err` is the step's error; journaling is the caller's.
    async fn step(&mut self, step: &Step, step_started: Seq) -> Result<(), RunError> {
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
        match &step.kind {
            StepKind::Action {
                target,
                action,
                params,
            } => {
                let mut req = ActionRequest::new(
                    self.origin(Some(&step.id)),
                    target.conn.clone(),
                    action.clone(),
                    covalence_protocol::to_value(params).expect("JSON encodes as MessagePack"),
                );
                req.cause = Some(step_started);
                req.run = Some(self.run.clone());
                req.deadline = timeout.map(|t| t.as_millis().min(u32::MAX as u128) as u32);
                // On its own task, so a timeout leaves the request running
                // and its late `action.result` is still journaled. A cancel
                // waits for it (flow spec §8.5).
                let actions = self.inner.actions.clone();
                let call = tokio::spawn(async move { actions.request_action(req).await });
                let outcome = match timeout {
                    Some(t) => tokio::time::timeout(t, call)
                        .await
                        .map_err(|_| timed_out())?,
                    None => call.await,
                };
                let outcome = outcome
                    .map_err(|e| RunError::new("internal", format!("action task failed: {e}")))?;
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
                        _ => Ok(()),
                    },
                }
            }
            StepKind::Wait { duration } => {
                let wait = tokio::time::sleep(duration.get());
                let cancel = &mut self.cancel;
                let interrupted = async {
                    if cancel.wait_for(|c| *c).await.is_err() {
                        std::future::pending::<()>().await;
                    }
                };
                let waited = async {
                    tokio::select! {
                        () = wait => Ok(()),
                        () = interrupted => Err(RunError::new("cancelled", "the run was cancelled")),
                    }
                };
                match timeout {
                    Some(t) => tokio::time::timeout(t, waited)
                        .await
                        .map_err(|_| timed_out())?,
                    None => waited.await,
                }
            }
        }
    }
}

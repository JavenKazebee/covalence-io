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

use covalence_protocol::state::PathSegment;
use covalence_protocol::{Object, Value};
use serde::Serialize;
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::{
    Cond, Edge, EventSource, Flow, FlowId, OnError, SetTarget, Step, StepKind, StopStatus, Trigger,
};
use crate::compose::{self, ForEach, InstanceChain, Link};
use crate::core_conn;
use crate::dataflow::CellDef;
use crate::engine::{ActionRequest, Actions, CoreError};
use crate::expr::{Evaluation, Evaluator, ExprError, Reads, Scope, json_to_value, to_json, truthy};
use crate::gate::Verdict;
use crate::journal::{
    BlockReason, EventRecord, InstanceRecord, JournalError, JournalOp, NewEntry, Origin, Record,
    RunError, RunFinishedRecord, RunStartedRecord, RunStatus, RunStepRecord, Seq, StatePatchRecord,
    StepDetail, StepStatus, TriggerMatchedRecord,
};
use crate::select::{ControlView, Pattern, Target, TargetView};
use crate::source::{self, Eval, Source};
use crate::state::{self as st, ConnId, Dirty, Meta, Quality, StatePath, TurnFeed};
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
    /// What a `stop` step handed back (spec §2.3).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outputs: Option<serde_json::Value>,
    /// The instances it ran inside (composition spec §5).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<InstanceChain>,
    /// Their params when it started.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
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
    /// Inputs that don't fit the flow's `inputs` (spec §8.1).
    #[error("{}", .0.message)]
    BadInputs(RunError),
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
    /// Computed variables' slugs, which `set` can't write (dataflow §6).
    read_only: RwLock<std::collections::BTreeSet<String>>,
    book: Mutex<Book>,
    /// `state` triggers, by flow and trigger index.
    watches: Mutex<BTreeMap<(FlowId, usize), Watch>>,
    /// `state` triggers with `match`: each matching path's last values.
    patterns: Mutex<BTreeMap<(FlowId, usize), PatternWatch>>,
    /// Connections as target selectors see them, by ID.
    targets: RwLock<Arc<BTreeMap<ConnId, TargetView>>>,
    /// Event triggers by the root of the paths they can match; `None`
    /// for those that can match any.
    index: RwLock<EventIndex>,
    /// Where each placed flow runs, by its slot's ID.
    places: RwLock<BTreeMap<FlowId, Arc<Place>>>,
    /// `forEach` instances, by the instance's ID (composition spec §4).
    lists: Mutex<BTreeMap<FlowId, List>>,
    /// Keeps params' cells (composition spec §1), once the owner sets it.
    cells: RwLock<Option<Arc<dyn Cells>>>,
}

/// Where a placed flow runs (composition spec §1.1): its home in the state
/// tree, where its `params` are, and the instances it's inside.
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    /// `flow.<slug>`, or for an item of a `forEach` instance,
    /// `flow.<slug>.items[<key>]`, or for an owned flow,
    /// `<owner's home>.flows.<slug>` (composition spec §2).
    pub home: StatePath,
    /// `params` in its expressions: `<home>.params`, or an owned flow's
    /// owner's.
    pub params: StatePath,
    pub instance: Option<InstanceChain>,
}

impl Place {
    /// A flow's own home, outside any instance.
    pub fn of(flow: &Flow) -> Place {
        Place::at(StatePath::new(
            STATE_ROOT,
            vec![PathSegment::Key(flow.slug.clone())],
        ))
    }

    /// A home with its own `params`, outside any instance.
    fn at(home: StatePath) -> Place {
        Place {
            params: home.key("params"),
            home,
            instance: None,
        }
    }

    fn named(&self) -> Vec<(String, StatePath)> {
        vec![("params".to_owned(), self.params.clone())]
    }
}

/// The layer that owns a flow, as it's placed (composition spec §2).
#[derive(Clone, Debug, PartialEq)]
pub struct Owner {
    /// The layer's ID, which `{controls}` event sources imply.
    pub layer: crate::surface::LayerId,
    /// Its home, `layer.<slug>`: the flow's status is under
    /// `<home>.flows.<slug>`, and its `params` are `<home>.params`.
    pub home: StatePath,
}

impl Owner {
    pub fn of(layer: &crate::surface::Layer) -> Owner {
        Owner {
            layer: layer.id.clone(),
            home: StatePath::new("layer", vec![PathSegment::Key(layer.slug.clone())]),
        }
    }
}

/// Keeps cells for the runtime: the dataflow graph.
pub trait Cells: Send + Sync {
    /// Makes the cells whose IDs start with `prefix` exactly `cells`.
    fn replace(&self, prefix: &str, cells: Vec<CellDef>);
}

impl Cells for crate::dataflow::Dataflow {
    fn replace(&self, prefix: &str, cells: Vec<CellDef>) {
        crate::dataflow::Dataflow::replace(self, prefix, cells, None);
    }
}

/// A flow to run (composition spec §9): a plain flow, a template run by
/// itself with its params' defaults, or an instance with its template's
/// steps, its params' sources and its items.
#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    /// What runs: for an instance, its template's steps with its identity.
    pub flow: Flow,
    /// Each param's source: the instance's binding, or the default.
    pub bound: Vec<(String, Source<serde_json::Value>)>,
    pub for_each: Option<ForEach>,
    /// The layer that owns it.
    pub owner: Option<Owner>,
}

impl Placement {
    /// How `flow` runs, given its template if it's an instance. `None` for
    /// one that doesn't run by itself: an abstract template, or an instance
    /// whose template is missing.
    pub fn of(flow: &Flow, template: Option<&Flow>) -> Option<Placement> {
        match (&flow.template, template) {
            (Some(_), Some(t)) => Some(Placement {
                bound: compose::bound(&t.params, &flow.bindings),
                for_each: flow.for_each.clone(),
                owner: None,
                flow: flow.instance_of(t),
            }),
            (Some(_), None) => None,
            (None, _) if flow.is_abstract() => None,
            (None, _) => Some(Placement {
                bound: compose::bound(&flow.params, &BTreeMap::new()),
                for_each: None,
                owner: None,
                flow: flow.clone(),
            }),
        }
    }

    /// Runs it as `owner`'s (composition spec §2): its status under the
    /// layer's home, the layer's `params`, and its `{controls}` event
    /// sources on the layer's controls.
    pub fn owned_by(mut self, owner: Owner) -> Placement {
        for trigger in &mut self.flow.triggers {
            if let Trigger::Event { from, .. } = trigger
                && let EventSource::Owner { controls } = from
            {
                *from = EventSource::Layer {
                    layer: owner.layer.clone(),
                    controls: Some(controls.clone()),
                };
            }
        }
        self.owner = Some(owner);
        self
    }
}

/// A `forEach` instance's items as they're placed.
struct List {
    placement: Arc<Placement>,
    /// Each placed item, by key: its index and value.
    items: BTreeMap<PathSegment, compose::Item>,
    /// What the list read, so a change to it lists the items again.
    reads: std::collections::BTreeSet<StatePath>,
    /// How many items were past the limit, last time.
    dropped: usize,
}

/// Event triggers, by flow and index, under the root they can match.
type EventIndex = BTreeMap<Option<String>, Vec<(FlowId, usize)>>;

/// A pattern `state` trigger's view of each path it matched.
struct PatternWatch {
    pattern: Pattern,
    /// Per path: its value, and what the edge is judged on (`when`'s
    /// result, or the value).
    seen: BTreeMap<StatePath, (Option<Value>, Option<Value>)>,
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
    place: Arc<Place>,
    /// Started runs that haven't finished, oldest first.
    running: Vec<RunId>,
    /// `queue` runs waiting to start.
    queue: VecDeque<RunId>,
}

struct Live {
    info: RunInfo,
    /// `trigger` in the run's expressions.
    trigger: Value,
    /// `input` in the run's expressions, bound when it fired.
    input: Object,
    cancel: watch::Sender<bool>,
    /// Becomes `true` once the run has journaled `run.finished`.
    done: watch::Receiver<bool>,
    /// The task's half of `cancel` and `done`, until the run is started.
    pending: Option<(watch::Receiver<bool>, watch::Sender<bool>)>,
    /// Why it was cancelled, when that's more than "cancelled": its
    /// instance ended with its item (`removed`).
    reason: Option<RunError>,
}

impl Live {
    fn new(info: RunInfo, trigger: Value, input: Object) -> Self {
        let (cancel, cancel_rx) = watch::channel(false);
        let (done_tx, done) = watch::channel(false);
        Live {
            info,
            trigger,
            input,
            cancel,
            done,
            pending: Some((cancel_rx, done_tx)),
            reason: None,
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
            read_only: RwLock::new(std::collections::BTreeSet::new()),
            book: Mutex::new(Book::default()),
            watches: Mutex::new(BTreeMap::new()),
            patterns: Mutex::new(BTreeMap::new()),
            targets: RwLock::new(Arc::new(BTreeMap::new())),
            index: RwLock::new(BTreeMap::new()),
            places: RwLock::new(BTreeMap::new()),
            lists: Mutex::new(BTreeMap::new()),
            cells: RwLock::new(None),
        });
        // Subscribed before anything can be journaled, so `sys:startup`
        // reaches the matcher.
        let journal = inner.shared.journal.subscribe();
        let matcher = tokio::spawn(match_events(inner.clone(), journal));
        let feed = inner.shared.state.subscribe_turns();
        let watcher = tokio::spawn(watch_state(inner.clone(), feed));
        Runtime {
            inner,
            matcher,
            watcher,
        }
    }

    /// The computed variables, by slug: a `set` step on one fails with
    /// `readOnly` (dataflow §6).
    pub fn set_read_only(&self, slugs: std::collections::BTreeSet<String>) {
        *self
            .inner
            .read_only
            .write()
            .unwrap_or_else(|e| e.into_inner()) = slugs;
    }

    /// Sets the connection aliases expressions can read, by alias.
    pub fn set_aliases(&self, aliases: BTreeMap<String, ConnId>) {
        *self
            .inner
            .aliases
            .write()
            .unwrap_or_else(|e| e.into_inner()) = Arc::new(aliases);
    }

    /// Sets the connections target selectors pick from, with each one's plugin.
    pub fn set_targets(&self, targets: Vec<TargetView>) {
        *self
            .inner
            .targets
            .write()
            .unwrap_or_else(|e| e.into_inner()) =
            Arc::new(targets.into_iter().map(|t| (t.conn.clone(), t)).collect());
        // Lists of targets follow the connections.
        self.inner.sync_lists(None, true);
    }

    /// Sets what keeps params' cells: the dataflow graph.
    pub fn set_cells(&self, cells: Arc<dyn Cells>) {
        *self.inner.cells.write().unwrap_or_else(|e| e.into_inner()) = Some(cells);
    }

    /// Journals `sys:startup` (dataflow §3), once the flows are loaded, so
    /// flows can run at boot: `reason` is `boot` or `failover`.
    pub fn startup(&self, reason: &str) -> Result<Seq, JournalError> {
        let record = EventRecord::at(
            StatePath::root("sys"),
            "startup",
            Some(Value::Map(vec![("reason".into(), reason.into())])),
        );
        Ok(self
            .inner
            .shared
            .journal
            .append(NewEntry::new(Origin::system(), Record::Event(record)))?
            .seq)
    }

    /// Adds or replaces a plain flow, or a template that runs by itself
    /// with its params' defaults. Runs already active keep the copy they
    /// started with.
    pub fn set_flow(&self, flow: Flow) {
        match Placement::of(&flow, None) {
            Some(p) => self.place(p),
            // An abstract template only runs through its instances.
            None => {
                let _ = self.remove_flow(&flow.id);
            }
        }
    }

    /// Adds or replaces a flow as it runs (composition spec §9): its params
    /// become cells at `<home>.params`, and with `forEach`, it runs once per
    /// item, each at `flow.<slug>.items[<key>]`.
    pub fn place(&self, placement: Placement) {
        let inner = &self.inner;
        let id = placement.flow.id.clone();
        match placement.for_each.clone() {
            None => {
                inner.drop_list(&id);
                let place = match &placement.owner {
                    Some(owner) => Place {
                        home: owner.home.key("flows").key(&placement.flow.slug),
                        params: owner.home.key("params"),
                        instance: None,
                    },
                    None => Place {
                        instance: placement.flow.template.as_ref().map(|t| {
                            vec![Link {
                                template: t.clone(),
                                instance: id.clone(),
                                key: None,
                            }]
                        }),
                        ..Place::of(&placement.flow)
                    },
                };
                inner.put_cells(
                    &compose::param_prefix(&id, None),
                    &place.home,
                    &placement.bound,
                    &[],
                );
                inner.put_slot(placement.flow, place);
            }
            Some(_) => {
                // The instance itself doesn't run; its items do.
                if inner.book().flows.contains_key(&id) {
                    let _ = inner.remove_slot(&id, None);
                }
                inner.put_cells(
                    &compose::param_prefix(&id, None),
                    &Place::of(&placement.flow).home,
                    &[],
                    &[],
                );
                let mut lists = inner.lists();
                let list = lists.entry(id.clone()).or_insert_with(|| List {
                    placement: Arc::new(placement.clone()),
                    items: BTreeMap::new(),
                    reads: Default::default(),
                    dropped: 0,
                });
                let changed = *list.placement != placement;
                list.placement = Arc::new(placement);
                drop(lists);
                inner.sync_list(&id, changed);
            }
        }
    }

    /// A `forEach` instance's current items, in list order.
    pub fn items(&self, id: &str) -> Option<Vec<compose::Item>> {
        let lists = self.inner.lists();
        let mut items: Vec<compose::Item> = lists.get(id)?.items.values().cloned().collect();
        items.sort_by_key(|i| i.index);
        Some(items)
    }

    /// Removes a flow, cancels its runs and deletes `flow.<slug>`; for a
    /// `forEach` instance, every item's.
    pub fn remove_flow(&self, id: &str) -> Result<(), JournalError> {
        let inner = &self.inner;
        inner.drop_list(id);
        inner.put_cells(
            &compose::param_prefix(id, None),
            &StatePath::root(STATE_ROOT),
            &[],
            &[],
        );
        inner.remove_slot(id, None)
    }

    /// Fires a flow's `api` trigger (`POST /flows/{id}/run`), with `inputs`
    /// bound to the flow's declared ones.
    pub fn fire_api(
        &self,
        id: &str,
        origin: Origin,
        inputs: BTreeMap<String, serde_json::Value>,
    ) -> Result<Fired, FireError> {
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
        let input = self
            .inner
            .bind_inputs(&flow, inputs, true)
            .map_err(FireError::BadInputs)?;
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
            input,
            failed_condition: failed,
        };
        Ok(self.inner.fire(&mut book, id, fire)?)
    }

    /// Cancels a run. A queued run is dropped at once; a running one stops at
    /// its next step boundary or wait, and the returned phase is `running`.
    /// Cancels every active and queued run of a flow; how many there were.
    pub fn cancel_flow(&self, flow: &str) -> usize {
        let runs: Vec<RunId> = self
            .inner
            .book()
            .live
            .iter()
            .filter(|(_, l)| l.info.flow == flow)
            .map(|(id, _)| id.clone())
            .collect();
        runs.iter().filter(|run| self.cancel(run).is_ok()).count()
    }

    pub fn cancel(&self, run: &str) -> Result<RunPhase, CancelError> {
        self.inner.cancel_run(run)
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

/// Fires event triggers from `event` entries in the journal (dataflow §3.1).
async fn match_events(
    inner: Arc<Inner>,
    mut feed: broadcast::Receiver<Arc<crate::journal::Entry>>,
) {
    loop {
        let entry = match feed.recv().await {
            Ok(entry) => entry,
            Err(broadcast::error::RecvError::Lagged(missed)) => {
                tracing::warn!(missed, "flow trigger matcher fell behind the journal");
                continue;
            }
            Err(broadcast::error::RecvError::Closed) => return,
        };
        let Record::Event(event) = &entry.record else {
            continue;
        };
        // Control input keeps its user's origin; other events are the system's.
        let origin = if event.surface.is_some() {
            entry.origin.clone()
        } else {
            Origin::system()
        };
        for (flow, trigger, payload) in inner.event_matches(event, entry.instance.as_ref()) {
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
                failed_condition: inner.failed_condition(&flow, &payload),
                payload,
                input: inner.defaults(&flow),
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

/// The roots of the paths an event source can match, for the index; `None`
/// if it can match any.
fn source_roots(from: &EventSource) -> Option<Vec<String>> {
    use crate::select::{Selector, Wild};
    fn paths(s: &Selector<crate::select::Paths>) -> Option<Vec<String>> {
        match s {
            Selector::One(r) => Some(vec![r.state_path().conn]),
            Selector::List(rs) => Some(rs.iter().map(|r| r.state_path().conn).collect()),
            Selector::Form(f) => match Pattern::parse(&f.pattern).ok()?.root {
                // An alias names a connection only through the current aliases.
                Wild::Is(root) if crate::expr::RESERVED_ROOTS.iter().any(|(n, _)| *n == root) => {
                    Some(vec![root])
                }
                _ => None,
            },
            Selector::Any(items) => {
                let mut out = Vec::new();
                for item in items {
                    out.extend(paths(item)?);
                }
                Some(out)
            }
            Selector::All(items) => items.iter().find_map(paths),
            Selector::Expr(_) | Selector::Not(_) => None,
        }
    }
    match from {
        EventSource::Path { path } => paths(path),
        EventSource::Target { .. } => None,
        // Control input is at the surface; a helper's events are at its
        // cell under the layer (surfaces §4.3).
        EventSource::Layer { .. } => Some(vec![
            crate::surface::STATE_ROOT.to_owned(),
            "layer".to_owned(),
        ]),
        EventSource::Surface { .. } => Some(vec![crate::surface::STATE_ROOT.to_owned()]),
        // Placing an owned flow makes these layer sources; one that isn't
        // placed as owned matches nothing.
        EventSource::Owner { .. } => Some(Vec::new()),
    }
}

/// The name an action error gives in `data.code` (core actions).
fn named_code(e: &covalence_protocol::ProtocolError) -> Option<String> {
    let Some(Value::Map(fields)) = &e.data else {
        return None;
    };
    fields
        .iter()
        .find(|(k, _)| k.as_str() == Some("code"))
        .and_then(|(_, v)| v.as_str().map(str::to_owned))
}

/// Whether a value crossed a `state` trigger's edge.
fn crosses(edge: Edge, previous: Option<&Value>, now: Option<&Value>) -> bool {
    match edge {
        Edge::Rising => !truthy(previous) && truthy(now),
        Edge::Falling => truthy(previous) && !truthy(now),
        Edge::Change => previous != now,
    }
}

/// `{conn, path}` as text, with the connection's alias where it has one.
fn text_of(path: &StatePath, by_id: &BTreeMap<&ConnId, &String>) -> String {
    let root = by_id
        .get(&path.conn)
        .map_or(path.conn.as_str(), |a| a.as_str());
    crate::text_path::format(root, &path.path)
}

/// Re-evaluates `state` triggers when what they read changes (spec §8.2).
async fn watch_state(inner: Arc<Inner>, mut feed: TurnFeed) {
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
        // After a lag every state trigger, and every list, is evaluated again.
        inner.check_watches(dirty.as_ref());
        inner.sync_lists(dirty.as_ref(), false);
        feed.done();
    }
}

/// What a run adds to an expression's scope.
struct RunScope<'a> {
    /// `params`: the instance's, at its home.
    named: &'a [(String, StatePath)],
    input: &'a Value,
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
    /// `input` in expressions.
    input: Object,
    /// The index of the first falsy condition, which blocks the run.
    failed_condition: Option<u32>,
}

impl Inner {
    fn book(&self) -> MutexGuard<'_, Book> {
        self.book.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn pattern_watches(&self) -> MutexGuard<'_, BTreeMap<(FlowId, usize), PatternWatch>> {
        self.patterns.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn aliases(&self) -> Arc<BTreeMap<String, ConnId>> {
        self.aliases
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Rebuilds the event trigger index from the flows.
    fn reindex(&self, book: &Book) {
        let mut index = EventIndex::new();
        for slot in book.flows.values() {
            for (i, trigger) in slot.flow.triggers.iter().enumerate() {
                let Trigger::Event { from, .. } = trigger else {
                    continue;
                };
                match source_roots(from) {
                    Some(roots) => {
                        for root in roots {
                            index
                                .entry(Some(root))
                                .or_default()
                                .push((slot.flow.id.clone(), i));
                        }
                    }
                    None => index
                        .entry(None)
                        .or_default()
                        .push((slot.flow.id.clone(), i)),
                }
            }
        }
        *self.index.write().unwrap_or_else(|e| e.into_inner()) = index;
    }

    /// The enabled flows an event fires, each by its first matching
    /// trigger, with the trigger payload (dataflow §3.1).
    fn event_matches(
        &self,
        event: &EventRecord,
        instance: Option<&InstanceChain>,
    ) -> Vec<(Arc<Flow>, usize, Value)> {
        let keys: Vec<(FlowId, usize)> = {
            let index = self.index.read().unwrap_or_else(|e| e.into_inner());
            let root = index
                .get(&Some(event.path.conn.clone()))
                .into_iter()
                .flatten();
            let any = index.get(&None).into_iter().flatten();
            root.chain(any).cloned().collect()
        };
        let mut by_flow: BTreeMap<FlowId, Vec<usize>> = BTreeMap::new();
        for (flow, i) in keys {
            by_flow.entry(flow).or_default().push(i);
        }
        let aliases = self.aliases();
        let by_id: BTreeMap<&ConnId, &String> = aliases.iter().map(|(a, id)| (id, a)).collect();
        let mut out = Vec::new();
        for (flow_id, mut triggers) in by_flow {
            let Some(flow) = self
                .book()
                .flows
                .get(&flow_id)
                .filter(|s| s.flow.enabled)
                .map(|s| s.flow.clone())
            else {
                continue;
            };
            triggers.sort_unstable();
            for i in triggers {
                let Some(Trigger::Event {
                    from, event: names, ..
                }) = flow.triggers.get(i)
                else {
                    continue;
                };
                if !names.contains(&event.event) {
                    continue;
                }
                let Some((captures, source)) = self.source_matches(from, event, &aliases) else {
                    continue;
                };
                let mut payload: Vec<(Value, Value)> = vec![
                    ("path".into(), text_of(&event.path, &by_id).into()),
                    ("event".into(), event.event.as_str().into()),
                ];
                if let Some(p) = &event.payload {
                    payload.push(("payload".into(), p.clone()));
                }
                if let Some(c) = captures {
                    payload.push((
                        "captures".into(),
                        Value::Array(c.into_iter().map(Value::from).collect()),
                    ));
                }
                if let Some(t) = source {
                    payload.push((
                        "source".into(),
                        Value::Map(vec![("conn".into(), t.conn.as_str().into())]),
                    ));
                }
                if let Some(chain) = instance {
                    let json = serde_json::to_value(chain).unwrap_or_default();
                    payload.push(("instance".into(), json_to_value(&json)));
                }
                for (name, field) in [
                    ("surface", &event.surface),
                    ("layer", &event.layer),
                    ("control", &event.control),
                ] {
                    if let Some(v) = field {
                        payload.push((name.into(), v.as_str().into()));
                    }
                }
                out.push((flow.clone(), i, Value::Map(payload)));
                break;
            }
        }
        out
    }

    /// Whether `from` picks the event: `Some((captures, source))`.
    fn source_matches(
        &self,
        from: &EventSource,
        event: &EventRecord,
        aliases: &Arc<BTreeMap<String, ConnId>>,
    ) -> Option<(Option<Vec<String>>, Option<Target>)> {
        let scope = Scope::new(&self.shared.state, aliases);
        let mut eval = Eval::new(&scope, self.evaluator.as_ref());
        let view = event.conn.as_ref().and_then(|conn| {
            self.targets
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .get(conn)
                .cloned()
        });
        source_matches(from, event, aliases, &mut eval, view)
    }

    /// Journals a flow's own event at its home: `flow.<slug>` (flows spec
    /// §1.1), or an item's.
    fn flow_event(
        &self,
        origin: Origin,
        place: &Place,
        run: &str,
        name: &str,
        payload: Vec<(Value, Value)>,
        cause: Seq,
    ) -> Result<Seq, JournalError> {
        let mut fields: Vec<(Value, Value)> = vec![("run".into(), run.into())];
        fields.extend(payload);
        let record = EventRecord::at(place.home.clone(), name, Some(Value::Map(fields)));
        Ok(self
            .shared
            .journal
            .append(
                NewEntry::new(origin, Record::Event(record))
                    .cause(cause)
                    .run(run.to_owned())
                    .instance(place.instance.clone()),
            )?
            .seq)
    }

    /// `finished`, and `failed` when it failed, after a `run.finished`.
    fn flow_finished(
        &self,
        origin: Origin,
        place: &Place,
        run: &str,
        status: RunStatus,
        error: Option<&RunError>,
        finished: Seq,
    ) -> Result<(), JournalError> {
        let status_text = serde_json::to_value(status)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        self.flow_event(
            origin.clone(),
            place,
            run,
            "finished",
            vec![("status".into(), status_text.into())],
            finished,
        )?;
        if let Some(e) = error.filter(|_| status == RunStatus::Failed) {
            let error = Value::Map(vec![
                ("code".into(), e.code.as_str().into()),
                ("message".into(), e.message.as_str().into()),
            ]);
            self.flow_event(
                origin,
                place,
                run,
                "failed",
                vec![("error".into(), error)],
                finished,
            )?;
        }
        Ok(())
    }

    fn watches(&self) -> MutexGuard<'_, BTreeMap<(FlowId, usize), Watch>> {
        self.watches.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `params` for a flow's triggers and conditions: at its home.
    fn named(&self, flow: &str) -> Vec<(String, StatePath)> {
        self.places
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(flow)
            .map(|p| p.named())
            .unwrap_or_default()
    }

    /// Evaluates `source` with the current aliases, for `flow`. `run` is a
    /// run's scope.
    fn evaluate(
        &self,
        source: &str,
        flow: &str,
        trigger: Option<&Value>,
        run: Option<RunScope<'_>>,
    ) -> Evaluation {
        let aliases = self
            .aliases
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let named = match &run {
            Some(run) => run.named.to_vec(),
            None => self.named(flow),
        };
        let mut scope = Scope::new(&self.shared.state, &aliases);
        scope.trigger = trigger;
        scope.named = &named;
        if let Some(run) = run {
            scope.input = Some(run.input);
            scope.local = run.local;
            scope.steps = run.steps;
            scope.cancel = Some(run.cancel);
        }
        self.evaluator.evaluate(source, &scope)
    }

    /// Evaluates a source as [`Self::evaluate`] does an expression.
    fn evaluate_source<T: serde::Serialize>(
        &self,
        source: &Source<T>,
        flow: &str,
        trigger: Option<&Value>,
        run: Option<RunScope<'_>>,
    ) -> source::Outcome {
        let aliases = self
            .aliases
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let named = match &run {
            Some(run) => run.named.to_vec(),
            None => self.named(flow),
        };
        let mut scope = Scope::new(&self.shared.state, &aliases);
        scope.trigger = trigger;
        scope.named = &named;
        if let Some(run) = run {
            scope.input = Some(run.input);
            scope.local = run.local;
            scope.steps = run.steps;
            scope.cancel = Some(run.cancel);
        }
        source.evaluate(&scope, self.evaluator.as_ref())
    }

    /// Whether a filter or condition is truthy. A failure counts as false
    /// and is logged (spec §8.2).
    fn passes(&self, flow: &Flow, source: &str, trigger: &Value, what: &str) -> bool {
        match self.evaluate(source, &flow.id, Some(trigger), None).value {
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
        let mut fresh_patterns = BTreeMap::new();
        if flow.enabled {
            for (i, trigger) in flow.triggers.iter().enumerate() {
                match trigger {
                    Trigger::State {
                        pattern: Some(text),
                        when,
                        ..
                    } => {
                        let Ok(pattern) = Pattern::parse(text) else {
                            continue;
                        };
                        let seen = self.pattern_snapshot(flow, &pattern, when.as_ref());
                        fresh_patterns.insert((flow.id.clone(), i), PatternWatch { pattern, seen });
                    }
                    Trigger::State {
                        when: Some(when), ..
                    } => {
                        let ((value, reads), error) = self.watch_value(flow, when, None, None);
                        fresh.insert(
                            (flow.id.clone(), i),
                            Watch {
                                value,
                                reads,
                                error,
                            },
                        );
                    }
                    _ => {}
                }
            }
        }
        let mut watches = self.watches();
        watches.retain(|(id, _), _| id != &flow.id);
        watches.extend(fresh);
        drop(watches);
        let mut patterns = self.pattern_watches();
        patterns.retain(|(id, _), _| id != &flow.id);
        patterns.extend(fresh_patterns);
    }

    /// Evaluates a `state` trigger's `when`, with `trigger` in scope for a
    /// pattern trigger: its value (`None` on failure, after any fallback)
    /// and reads, and the failure, logged unless it repeats `last`.
    fn watch_value(
        &self,
        flow: &Flow,
        when: &Cond,
        trigger: Option<&Value>,
        last: Option<&str>,
    ) -> ((Option<Value>, Reads), Option<String>) {
        let outcome = self.evaluate_source(&when.0, &flow.id, trigger, None);
        let reads = Reads {
            paths: outcome.reads,
            text: Default::default(),
        };
        let as_value = |j: &serde_json::Value| covalence_protocol::to_value(j).ok();
        let fallback = when.0.fallback().and_then(as_value);
        match outcome.value {
            Ok(Some(v)) => ((as_value(&v), reads), None),
            Ok(None) => ((fallback, reads), None),
            Err(e) => {
                if last != Some(e.message.as_str()) {
                    tracing::warn!(flow = %flow.id, "state trigger failed, counting it as undefined: {e}");
                }
                ((fallback, reads), Some(e.message))
            }
        }
    }

    /// The paths a pattern matches at `paths`' nodes and their ancestors.
    fn pattern_hits<'p>(
        &self,
        pattern: &Pattern,
        paths: impl Iterator<Item = &'p StatePath>,
    ) -> BTreeMap<StatePath, Vec<String>> {
        let aliases = self.aliases();
        let mut out = BTreeMap::new();
        for leaf in paths {
            for k in 0..=leaf.path.len() {
                let node = StatePath::new(leaf.conn.clone(), leaf.path[..k].to_vec());
                if let Some(c) = pattern.captures(&node, &aliases) {
                    out.insert(node, c);
                }
            }
        }
        out
    }

    /// What a pattern trigger judges a path's edge on, and its payload.
    fn pattern_edge(
        &self,
        flow: &Flow,
        when: Option<&Cond>,
        path: &StatePath,
        captures: &[String],
        value: &Option<Value>,
    ) -> Option<Value> {
        let Some(when) = when else {
            return value.clone();
        };
        let aliases = self.aliases();
        let by_id: BTreeMap<&ConnId, &String> = aliases.iter().map(|(a, id)| (id, a)).collect();
        let mut trigger: Vec<(Value, Value)> = vec![
            ("path".into(), text_of(path, &by_id).into()),
            (
                "captures".into(),
                Value::Array(captures.iter().map(|c| c.as_str().into()).collect()),
            ),
        ];
        if let Some(v) = value {
            trigger.push(("value".into(), v.clone()));
        }
        self.watch_value(flow, when, Some(&Value::Map(trigger)), None)
            .0
            .0
    }

    /// Every path a pattern matches now, with its values: the baseline.
    fn pattern_snapshot(
        &self,
        flow: &Flow,
        pattern: &Pattern,
        when: Option<&Cond>,
    ) -> BTreeMap<StatePath, (Option<Value>, Option<Value>)> {
        use crate::select::Wild;
        let aliases = self.aliases();
        // A `*` root isn't snapshotted: its first changes compare with undefined.
        let root = match &pattern.root {
            Wild::Any => return BTreeMap::new(),
            Wild::Is(name) if crate::expr::RESERVED_ROOTS.iter().any(|(n, _)| n == name) => {
                name.clone()
            }
            Wild::Is(alias) => match aliases.get(alias) {
                Some(conn) => conn.clone(),
                None => return BTreeMap::new(),
            },
        };
        let leaves: Vec<StatePath> = self.shared.state.read(|tree| {
            tree.leaves_under(&StatePath::root(root.clone()))
                .map(|(p, _)| p.clone())
                .collect()
        });
        self.pattern_hits(pattern, leaves.iter())
            .into_iter()
            .map(|(path, captures)| {
                let value = self.shared.state.value(&path);
                let edge = self.pattern_edge(flow, when, &path, &captures, &value);
                (path, (value, edge))
            })
            .collect()
    }

    /// Fires pattern `state` triggers for the matching paths in `dirty`.
    /// After a lag (`None`) they take a new baseline instead.
    fn check_patterns(self: &Arc<Self>, dirty: Option<&Dirty>) {
        let keys: Vec<(FlowId, usize)> = self.pattern_watches().keys().cloned().collect();
        for (flow_id, index) in keys {
            let Some(flow) = self.book().flows.get(&flow_id).map(|s| s.flow.clone()) else {
                continue;
            };
            let Some(Trigger::State { when, edge, .. }) = flow.triggers.get(index) else {
                continue;
            };
            let Some(pattern) = self
                .pattern_watches()
                .get(&(flow_id.clone(), index))
                .map(|w| w.pattern.clone())
            else {
                continue;
            };
            let Some(dirty) = dirty else {
                let seen = self.pattern_snapshot(&flow, &pattern, when.as_ref());
                if let Some(w) = self.pattern_watches().get_mut(&(flow_id.clone(), index)) {
                    w.seen = seen;
                }
                continue;
            };
            let aliases = self.aliases();
            let by_id: BTreeMap<&ConnId, &String> = aliases.iter().map(|(a, id)| (id, a)).collect();
            for (path, captures) in self.pattern_hits(&pattern, dirty.iter()) {
                let value = self.shared.state.value(&path);
                let now = self.pattern_edge(&flow, when.as_ref(), &path, &captures, &value);
                let previous = {
                    let mut watches = self.pattern_watches();
                    let Some(w) = watches.get_mut(&(flow_id.clone(), index)) else {
                        break;
                    };
                    let previous = if value.is_none() && now.is_none() {
                        w.seen.remove(&path)
                    } else {
                        w.seen.insert(path.clone(), (value.clone(), now.clone()))
                    };
                    previous.unwrap_or((None, None))
                };
                if !crosses(*edge, previous.1.as_ref(), now.as_ref()) {
                    continue;
                }
                let cause = self.shared.state.read(|tree| {
                    dirty
                        .iter()
                        .filter(|d| d.is_within(&path))
                        .filter_map(|d| tree.leaf(d).map(|leaf| leaf.meta.seq))
                        .max()
                });
                let mut payload: Vec<(Value, Value)> = vec![
                    ("path".into(), text_of(&path, &by_id).into()),
                    (
                        "captures".into(),
                        Value::Array(captures.iter().map(|c| c.as_str().into()).collect()),
                    ),
                ];
                if let Some(v) = value {
                    payload.push(("value".into(), v));
                }
                if let Some(p) = previous.0 {
                    payload.push(("previous".into(), p));
                }
                payload.push(("ts".into(), clock::epoch_ms().into()));
                let payload = Value::Map(payload);
                let fire = Fire {
                    trigger: index,
                    cause,
                    origin: Origin::system(),
                    failed_condition: self.failed_condition(&flow, &payload),
                    payload,
                    input: self.defaults(&flow),
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
    }

    /// Re-evaluates the `state` triggers that read any of `dirty` (all of
    /// them for `None`) and fires those whose edge is crossed.
    fn check_watches(self: &Arc<Self>, dirty: Option<&Dirty>) {
        self.check_patterns(dirty);
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
            let Some(Trigger::State {
                when: Some(when),
                edge,
                pattern: None,
            }) = flow.triggers.get(index)
            else {
                continue;
            };
            let ((value, reads), error) =
                self.watch_value(&flow, when, None, last_error.as_deref());
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
            if !crosses(*edge, previous.as_ref(), value.as_ref()) {
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
                input: self.defaults(&flow),
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
            input,
            failed_condition,
        } = fire;
        use super::Concurrency::*;
        let slot = book
            .flows
            .get(flow_id)
            .expect("callers check the flow exists");
        let flow = slot.flow.clone();
        let instance = slot.place.instance.clone();
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
                .cause(cause)
                .instance(instance.clone()),
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
                outputs: None,
                instance,
                params: None,
            },
            payload,
            input,
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
        let input = Value::Map(
            live.input
                .iter()
                .map(|(k, v)| (Value::from(k.as_str()), v.clone()))
                .collect(),
        );
        let inputs = live.input.clone();
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
            place: slot.place.clone(),
            run: run.to_owned(),
            matched,
            cancel,
            trigger,
            input,
            inputs,
            local: BTreeMap::new(),
            steps: BTreeMap::new(),
            started: 0,
            named: slot.place.named(),
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
                    error: live.reason.clone(),
                    outputs: None,
                }),
            )
            .cause(live.info.matched)
            .run(run.to_owned())
            .instance(live.info.instance.clone()),
        )?;
        if let Some(slot) = book.flows.get(&live.info.flow) {
            self.flow_finished(
                entry.origin.clone(),
                &slot.place,
                run,
                RunStatus::Cancelled,
                None,
                entry.seq,
            )?;
        }
        live.info.status = RunPhase::Cancelled;
        live.info.error = live.reason.clone();
        live.info.finished = Some(entry.seq);
        if let Some((_, done)) = live.pending.take() {
            done.send_replace(true);
        }
        push_recent(book, live.info);
        Ok(())
    }

    fn state_path(home: &StatePath, rest: &[&str]) -> StatePath {
        rest.iter().fold(home.clone(), |p, k| p.key(k))
    }

    fn meta(seq: Seq) -> Meta {
        Meta {
            ts: clock::epoch_ms(),
            seq,
            quality: Quality::Good,
            source: st::Source::Core,
        }
    }

    fn publish_running(&self, slot: &Slot, seq: Seq) {
        let path = Self::state_path(&slot.place.home, &["running"]);
        let count = Value::from(slot.running.len() as u64);
        self.shared
            .state
            .mutate(|tree| ((), tree.set(&path, &count, &Self::meta(seq))));
    }

    fn publish_last_run(&self, home: &StatePath, info: &RunInfo, seq: Seq) {
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
        let path = Self::state_path(home, &["lastRun"]);
        self.shared
            .state
            .mutate(|tree| ((), tree.set(&path, &Value::Map(last), &Self::meta(seq))));
    }

    fn delete_state(&self, home: &StatePath) {
        let path = home.clone();
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

impl Inner {
    fn cancel_run(&self, run: &str) -> Result<RunPhase, CancelError> {
        let inner = self;
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

    /// Binds a run's inputs (spec §8.1): each given value checked against its
    /// `ParamDef`, and defaults for the rest. `strict` refuses unknown,
    /// missing and mistyped inputs; otherwise they're left out.
    fn bind_inputs(
        &self,
        flow: &Flow,
        mut given: BTreeMap<String, serde_json::Value>,
        strict: bool,
    ) -> Result<Object, RunError> {
        if strict && let Some(name) = given.keys().find(|k| !flow.inputs.contains_key(*k)) {
            return Err(RunError::new(
                "unknownInput",
                format!("flow {} has no input {name:?}", flow.name),
            ));
        }
        let mut out = Object::new();
        for (name, def) in &flow.inputs {
            let value = match (given.remove(name), &def.default) {
                (Some(v), _) => Some(v),
                (None, Some(default)) => {
                    match self.evaluate_source(default, &flow.id, None, None).value {
                        Ok(v) => v.or_else(|| default.fallback().cloned()),
                        Err(_) if !strict => default.fallback().cloned(),
                        Err(e) => {
                            return Err(RunError::new(
                                "invalidInputs",
                                format!("input {name:?}'s default failed: {}", e.message),
                            ));
                        }
                    }
                }
                (None, None) => None,
            };
            match value {
                Some(v) => {
                    let problems = schema::check(&def.schema(), &v);
                    if problems.is_empty() {
                        out.insert(name.clone(), json_to_value(&v));
                    } else if strict {
                        let why: Vec<String> = problems.into_iter().map(|m| m.message).collect();
                        return Err(RunError::new(
                            "invalidInputs",
                            format!("input {name:?}: {}", why.join("; ")),
                        ));
                    }
                }
                None if strict && def.needs_value() => {
                    return Err(RunError::new(
                        "missingInput",
                        format!("flow {} needs input {name:?}", flow.name),
                    ));
                }
                None => {}
            }
        }
        Ok(out)
    }

    /// The inputs a trigger that gives none binds: the defaults.
    fn defaults(&self, flow: &Flow) -> Object {
        self.bind_inputs(flow, BTreeMap::new(), false)
            .unwrap_or_default()
    }
}

/// Placing flows: slots, params' cells and `forEach` items (composition
/// spec §1, §4).
impl Inner {
    fn lists(&self) -> MutexGuard<'_, BTreeMap<FlowId, List>> {
        self.lists.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Adds or replaces the slot a flow runs in.
    fn put_slot(&self, flow: Flow, place: Place) {
        let place = Arc::new(place);
        let mut book = self.book();
        let old_home = book.flows.get(&flow.id).map(|s| s.place.home.clone());
        if let Some(old) = old_home.filter(|old| *old != place.home) {
            self.delete_state(&old);
        }
        self.places
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(flow.id.clone(), place.clone());
        let slot = book
            .flows
            .entry(flow.id.clone())
            .and_modify(|slot| {
                slot.flow = Arc::new(flow.clone());
                slot.place = place.clone();
            })
            .or_insert_with(|| Slot {
                flow: Arc::new(flow.clone()),
                place: place.clone(),
                running: Vec::new(),
                queue: VecDeque::new(),
            });
        let seq = self.shared.journal.last_seq().unwrap_or(0);
        self.publish_running(slot, seq);
        self.reindex(&book);
        drop(book);
        self.baseline(&flow);
    }

    /// Removes a slot, cancels its runs (with `reason`, if there is one)
    /// and deletes its home.
    fn remove_slot(&self, id: &str, reason: Option<RunError>) -> Result<(), JournalError> {
        let mut book = self.book();
        let Some(slot) = book.flows.remove(id) else {
            return Ok(());
        };
        self.places
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
        self.watches().retain(|(flow, _), _| flow != id);
        self.pattern_watches().retain(|(flow, _), _| flow != id);
        self.reindex(&book);
        self.delete_state(&slot.place.home);
        for run in &slot.running {
            if let Some(live) = book.live.get_mut(run) {
                live.reason = reason.clone();
                live.cancel.send_replace(true);
            }
        }
        for run in slot.queue {
            if let Some(live) = book.live.get_mut(&run) {
                live.reason = reason.clone();
            }
            self.drop_queued(&mut book, &run)?;
        }
        Ok(())
    }

    /// Makes an instance's params cells: `<home>.params.<name>`.
    fn put_cells(
        &self,
        prefix: &str,
        home: &StatePath,
        bound: &[(String, Source<serde_json::Value>)],
        values: &[(String, Value)],
    ) {
        let cells = self.cells.read().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(cells) = cells {
            cells.replace(prefix, compose::param_cells(prefix, home, bound, values));
        }
    }

    /// Ends a `forEach` instance's items, without journaling them as
    /// removed from the list: the instance itself went.
    fn drop_list(&self, id: &str) {
        let Some(list) = self.lists().remove(id) else {
            return;
        };
        for key in list.items.keys() {
            let _ = self.remove_slot(&item_id(id, key), None);
            self.put_cells(
                &compose::param_prefix(id, Some(key)),
                &StatePath::root(STATE_ROOT),
                &[],
                &[],
            );
        }
        self.delete_state(&Place::of(&list.placement.flow).home);
    }

    /// Lists the items of every `forEach` instance again: those whose
    /// reads `dirty` touched, those over targets when `targets` changed,
    /// or with `None`, all of them.
    fn sync_lists(&self, dirty: Option<&Dirty>, targets: bool) {
        let due: Vec<FlowId> = self
            .lists()
            .iter()
            .filter(|(_, list)| {
                let over_targets = matches!(
                    list.placement.for_each.as_ref().map(|f| &f.from),
                    Some(compose::ItemsFrom::Targets(_))
                );
                match dirty {
                    _ if targets && over_targets => true,
                    None => !targets,
                    Some(d) => d
                        .iter()
                        .any(|p| list.reads.iter().any(|r| p.is_within(r) || r.is_within(p))),
                }
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            self.sync_list(&id, false);
        }
    }

    /// Makes a `forEach` instance's items match its list: an added item
    /// gets a slot and params, a removed one ends, its runs cancelled with
    /// `removed`, both journaled (composition spec §4). `changed` is a new
    /// placement, which every item takes.
    fn sync_list(&self, id: &str, changed: bool) {
        let Some((placement, old)) = self
            .lists()
            .get(id)
            .map(|l| (l.placement.clone(), l.items.clone()))
        else {
            return;
        };
        let Some(for_each) = &placement.for_each else {
            return;
        };
        let aliases = self.aliases();
        let scope = Scope::new(&self.shared.state, &aliases);
        let targets: Vec<TargetView> = self
            .targets
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect();
        let found = match compose::expand(for_each, &scope, self.evaluator.as_ref(), &targets) {
            Ok(found) => found,
            Err(e) => {
                tracing::warn!(flow = %id, "forEach failed; its items stay as they were: {e}");
                return;
            }
        };
        let cause = self.latest_seq(&found.reads);
        let root = Place::of(&placement.flow).home;
        let template = placement.flow.template.clone().unwrap_or_default();
        let chain = |key: &PathSegment| {
            vec![Link {
                template: template.clone(),
                instance: id.to_owned(),
                key: Some(key.clone()),
            }]
        };
        let now: BTreeMap<PathSegment, compose::Item> = found
            .items
            .iter()
            .map(|i| (i.key.clone(), i.clone()))
            .collect();
        for key in old.keys().filter(|k| !now.contains_key(*k)) {
            let removed = RunError::new(
                "removed",
                format!("its item {} left the list", compose::key_text(key)),
            );
            if let Err(e) = self.remove_slot(&item_id(id, key), Some(removed)) {
                tracing::error!(flow = %id, "cannot end an item: {e}");
            }
            self.put_cells(&compose::param_prefix(id, Some(key)), &root, &[], &[]);
            self.journal_instance(
                Record::InstanceRemoved(InstanceRecord {
                    instance: chain(key),
                }),
                cause,
            );
        }
        for (key, item) in &now {
            let before = old.get(key);
            if before == Some(item) && !changed {
                continue;
            }
            let place = Place {
                instance: Some(chain(key)),
                ..Place::at(root.key("items").child(key.clone()))
            };
            self.put_cells(
                &compose::param_prefix(id, Some(key)),
                &place.home,
                &placement.bound,
                &item.values(),
            );
            let mut flow = placement.flow.clone();
            flow.id = item_id(id, key);
            self.put_slot(flow, place);
            if before.is_none() {
                self.journal_instance(
                    Record::InstanceAdded(InstanceRecord {
                        instance: chain(key),
                    }),
                    cause,
                );
            }
        }
        let was_dropped = {
            let mut lists = self.lists();
            let Some(list) = lists.get_mut(id) else {
                return;
            };
            list.items = now;
            list.reads = found.reads;
            std::mem::replace(&mut list.dropped, found.dropped)
        };
        let too_many = root.key("tooMany");
        if found.dropped > 0 {
            if was_dropped == 0 {
                tracing::warn!(
                    flow = %id,
                    "forEach has {} items past the limit of {}; they aren't instanced",
                    found.dropped,
                    compose::MAX_ITEMS
                );
            }
            let count = Value::from(found.dropped as u64);
            let meta = Self::meta(self.shared.journal.last_seq().unwrap_or(0));
            self.shared
                .state
                .mutate(|tree| ((), tree.set(&too_many, &count, &meta)));
        } else if was_dropped > 0 {
            self.shared
                .state
                .mutate(|tree| ((), tree.delete(&too_many)));
        }
    }

    /// The newest entry among the leaves at or below `reads`.
    fn latest_seq(&self, reads: &std::collections::BTreeSet<StatePath>) -> Option<Seq> {
        self.shared.state.read(|tree| {
            reads
                .iter()
                .flat_map(|r| tree.leaves_under(r).map(|(_, leaf)| leaf.meta.seq))
                .filter(|s| *s > 0)
                .max()
        })
    }

    fn journal_instance(&self, record: Record, cause: Option<Seq>) {
        let chain = match &record {
            Record::InstanceAdded(r) | Record::InstanceRemoved(r) => Some(r.instance.clone()),
            _ => None,
        };
        let entry = NewEntry::new(Origin::system(), record)
            .cause(cause)
            .instance(chain);
        if let Err(e) = self.shared.journal.append(entry) {
            tracing::error!("cannot journal an instance change: {e}");
        }
    }
}

/// A `forEach` item's slot: `<instance ID>[<key>]` (composition spec §6.4).
pub fn item_id(instance: &str, key: &PathSegment) -> FlowId {
    format!("{instance}[{}]", compose::key_text(key))
}

/// Cancels a called run if the `call` step waiting for it goes away: the
/// caller cancelled or timed out (spec §8.3).
struct CancelOnDrop {
    inner: Arc<Inner>,
    run: Option<RunId>,
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(run) = self.run.take() {
            let _ = self.inner.cancel_run(&run);
        }
    }
}

/// An object as a map value.
fn object_value(object: Object) -> Value {
    Value::Map(
        object
            .into_iter()
            .map(|(k, v)| (Value::from(k.as_str()), v))
            .collect(),
    )
}

/// One run, on its own task.
struct RunTask {
    inner: Arc<Inner>,
    flow: Arc<Flow>,
    /// Where it runs: its home and its instances, as they were when it started.
    place: Arc<Place>,
    run: RunId,
    matched: Seq,
    cancel: watch::Receiver<bool>,
    /// `trigger` in expressions.
    trigger: Value,
    /// `input` in expressions, and as `run.started` records it.
    input: Value,
    inputs: Object,
    /// `local` in expressions: set by `set` steps.
    local: BTreeMap<String, Value>,
    /// `steps.<id>` in expressions, for the steps that have finished.
    steps: BTreeMap<String, Value>,
    /// The `run.started` entry, which every `run.step` points at.
    started: Seq,
    /// `params` in expressions.
    named: Vec<(String, StatePath)>,
}

/// How a list of steps ended.
enum End {
    Ok,
    Failed(RunError),
    Cancelled,
    /// A `stop` step ended the run.
    Stopped(Stopped),
}

/// How a `stop` step ended its run (spec §2.3).
#[derive(Clone)]
struct Stopped {
    status: super::StopStatus,
    outputs: Object,
    error: Option<RunError>,
}

/// What a step that finished well leaves: its result, and for a `branch`
/// the arm it chose.
#[derive(Default)]
struct Done {
    result: Option<Value>,
    arm: Option<serde_json::Value>,
    /// A `stop` step, or a block one ran in: the run ends here.
    stop: Option<Stopped>,
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
            Ok((status, error, outputs, finished, duration_ms)) => {
                live.info.status = status.into();
                live.info.error = error;
                live.info.outputs = outputs
                    .filter(|o| !o.is_empty())
                    .map(|o| to_json(&object_value(o)));
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

        let mut next = None;
        if let Some(slot) = book.flows.get_mut(&self.flow.id) {
            slot.running.retain(|r| *r != self.run);
            if slot.running.is_empty() {
                next = slot.queue.pop_front();
            }
            // A slug change since the run started already moved the state.
            if slot.place.home == self.place.home {
                inner.publish_last_run(&self.place.home, &live.info, seq);
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
                    .run(self.run.clone())
                    .instance(self.place.instance.clone()),
            )?
            .seq)
    }

    fn cancelled(&self) -> bool {
        *self.cancel.borrow()
    }

    fn scope(&self) -> RunScope<'_> {
        RunScope {
            named: &self.named,
            input: &self.input,
            local: &self.local,
            steps: &self.steps,
            cancel: &self.cancel,
        }
    }

    /// The flow a param of kind `flow` holds: its ID, at `<home>.params`.
    fn flow_param(&self, param: &str) -> Result<FlowId, RunError> {
        match self.inner.shared.state.value(&self.place.params.key(param)) {
            Some(Value::String(id)) => Ok(id.as_str().unwrap_or_default().to_owned()),
            _ => Err(RunError::new(
                "unboundParam",
                format!("param {param:?} holds no flow"),
            )),
        }
    }

    /// Evaluates an expression in the run's scope.
    fn eval(&self, source: &str) -> Result<Option<Value>, RunError> {
        self.inner
            .evaluate(
                source,
                &self.flow.id,
                Some(&self.trigger),
                Some(self.scope()),
            )
            .value
            .map_err(|e| {
                if *self.cancel.borrow() {
                    cancelled_error()
                } else {
                    expr_error(e)
                }
            })
    }

    /// A source's own result in the run's scope, without its fallback.
    fn eval_raw<T: serde::Serialize>(
        &self,
        source: &Source<T>,
    ) -> Result<Option<serde_json::Value>, RunError> {
        self.inner
            .evaluate_source(
                source,
                &self.flow.id,
                Some(&self.trigger),
                Some(self.scope()),
            )
            .value
            .map_err(|e| {
                if *self.cancel.borrow() {
                    cancelled_error()
                } else {
                    expr_error(e)
                }
            })
    }

    /// A source in the run's scope, with its fallback when the result is
    /// `undefined` or fails.
    fn eval_source(
        &self,
        source: &Source<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, RunError> {
        match (self.eval_raw(source), source.fallback()) {
            (Ok(Some(v)), _) => Ok(Some(v)),
            (Err(e), _) if e.code == "cancelled" => Err(e),
            (_, Some(f)) => Ok(Some(f.clone())),
            (result, None) => result,
        }
    }

    /// Journals the run from `run.started` to `run.finished`. Returns how it
    /// ended, its error, the `run.finished` seq and its duration.
    #[allow(clippy::type_complexity)]
    async fn execute(
        &mut self,
    ) -> Result<
        (
            RunStatus,
            Option<RunError>,
            Option<Object>,
            Seq,
            Option<u64>,
        ),
        JournalError,
    > {
        if self.cancelled() {
            // Cancelled before it started (restart, delete): no run.started.
            let finished = self.append(
                None,
                Record::RunFinished(RunFinishedRecord {
                    run: self.run.clone(),
                    status: RunStatus::Cancelled,
                    error: None,
                    outputs: None,
                }),
                self.matched,
            )?;
            self.inner.flow_finished(
                self.origin(None),
                &self.place,
                &self.run,
                RunStatus::Cancelled,
                None,
                finished,
            )?;
            return Ok((RunStatus::Cancelled, None, None, finished, None));
        }

        // The params it starts with, recorded so `why` can say which camera
        // it was after they've moved on (composition spec §5).
        let params: Option<Object> = match self.inner.shared.state.value(&self.place.params) {
            Some(Value::Map(entries)) if !entries.is_empty() => Some(
                entries
                    .into_iter()
                    .filter_map(|(k, v)| Some((k.as_str()?.to_owned(), v)))
                    .collect(),
            ),
            _ => None,
        };
        let started = self.append(
            None,
            Record::RunStarted(RunStartedRecord {
                flow: self.flow.id.clone(),
                run: self.run.clone(),
                mode: self.flow.concurrency,
                inputs: self.inputs.clone(),
                params: params.clone(),
            }),
            self.matched,
        )?;
        self.started = started;
        self.inner.flow_event(
            self.origin(None),
            &self.place,
            &self.run,
            "started",
            Vec::new(),
            started,
        )?;
        let started_at = clock::epoch_ms();
        let clock = Instant::now();
        {
            let mut book = self.inner.book();
            if let Some(live) = book.live.get_mut(&self.run) {
                live.info.started = Some(started);
                live.info.started_at = Some(started_at);
                live.info.params = params.map(|p| to_json(&object_value(p)));
            }
            if let Some(slot) = book.flows.get(&self.flow.id) {
                self.inner.publish_running(slot, started);
            }
        }

        let flow = self.flow.clone();
        let (mut status, mut run_error, outputs) = match self.run_list(&flow.body, &[]).await? {
            End::Ok => (RunStatus::Ok, None, None),
            End::Failed(e) => (RunStatus::Failed, Some(e), None),
            End::Cancelled => (RunStatus::Cancelled, None, None),
            End::Stopped(Stopped {
                status: super::StopStatus::Ok,
                outputs,
                ..
            }) => (RunStatus::Ok, None, Some(outputs)),
            End::Stopped(Stopped { outputs, error, .. }) => (
                RunStatus::Failed,
                Some(
                    error.unwrap_or_else(|| RunError::new("stopped", "a stop step failed the run")),
                ),
                Some(outputs),
            ),
        };
        // A cancel that lands during the last step still counts.
        if status == RunStatus::Ok && self.cancelled() {
            status = RunStatus::Cancelled;
        }
        if status == RunStatus::Cancelled {
            run_error = self
                .inner
                .book()
                .live
                .get(&self.run)
                .and_then(|l| l.reason.clone());
        }

        let finished = self.append(
            None,
            Record::RunFinished(RunFinishedRecord {
                run: self.run.clone(),
                status,
                error: run_error.clone(),
                outputs: outputs.clone().filter(|o| !o.is_empty()),
            }),
            started,
        )?;
        self.inner.flow_finished(
            self.origin(None),
            &self.place,
            &self.run,
            status,
            run_error.as_ref(),
            finished,
        )?;
        let duration_ms = clock.elapsed().as_millis() as u64;
        Ok((status, run_error, outputs, finished, Some(duration_ms)))
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
                        let entry = record(StepStatus::Done, done.arm, None);
                        self.append(Some(&step.id), entry, self.started)?;
                        if let Some(stopped) = done.stop {
                            return Ok(End::Stopped(stopped));
                        }
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
                    constraint,
                    action,
                    params,
                } => match self.resolve_target(target, constraint.as_ref()) {
                    Ok(target) => Ok(self
                        .action(id, &target, action, params, step_started, timeout)
                        .await),
                    Err(e) => Ok(Err(e)),
                },
                StepKind::Wait { duration } => Ok(self.wait(duration).await),
                StepKind::WaitUntil { until } => Ok(self.wait_until(until).await),
                StepKind::Set { path, value } => self.set(id, path, value, step_started),
                StepKind::Branch { arms, otherwise } => {
                    self.branch(arms, otherwise.as_deref(), position).await
                }
                StepKind::Call { flow, inputs, wait } => {
                    let flow = match flow {
                        super::FlowRef::Id(id) => Ok(id.clone()),
                        super::FlowRef::Param { param } => self.flow_param(param),
                    };
                    match flow {
                        Ok(flow) => Ok(self
                            .call(id, &flow, inputs, wait.unwrap_or(true), step_started)
                            .await),
                        Err(e) => Ok(Err(e)),
                    }
                }
                StepKind::Stop {
                    status,
                    outputs,
                    error,
                } => Ok(self.stop(status.unwrap_or_default(), outputs, error.as_deref())),
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
        target: &super::ConnRef,
        action: &str,
        params: &BTreeMap<String, Source<serde_json::Value>>,
        step_started: Seq,
        timeout: Option<StdDuration>,
    ) -> Result<Done, RunError> {
        let conn = &target.conn;
        // Parameters that follow state are evaluated now; an `undefined` one
        // is left out.
        let mut computed = serde_json::Map::new();
        let mut any_dynamic = false;
        for (name, param) in params {
            let value = match param {
                Source::Literal(v) => Some(v.clone()),
                other => {
                    any_dynamic = true;
                    self.eval_source(other)?
                }
            };
            if let Some(v) = value {
                computed.insert(name.clone(), v);
            }
        }
        if any_dynamic && let Some(schema) = self.inner.actions.params_schema(conn, action).await {
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
        req.instance = self.place.instance.clone();
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
                // The spec's name for it, where the action gives one
                // (`unknownLayer`), else the protocol code.
                (_, Some(Err(e))) => Err(RunError::new(
                    named_code(&e).unwrap_or_else(|| {
                        serde_json::to_value(e.code)
                            .ok()
                            .and_then(|v| v.as_str().map(str::to_owned))
                            .unwrap_or_else(|| "action_failed".into())
                    }),
                    e.message,
                )),
                (_, Some(Ok(result))) => Ok(Done {
                    result,
                    ..Done::default()
                }),
                (_, None) => Ok(Done::default()),
            },
        }
    }

    /// An action step's target: its literal, or the connection its source
    /// gives now, which must be one of the constraint's plugin (plugin protocol spec
    /// §13.9). Anything else fails the step with `no_target` before a request is made.
    fn resolve_target(
        &self,
        target: &Source<super::ConnRef>,
        constraint: Option<&super::TargetConstraint>,
    ) -> Result<super::ConnRef, RunError> {
        if let Source::Literal(t) = target {
            return Ok(t.clone());
        }
        let no_target = |why: String| RunError::new("no_target", why);
        let value = match (self.eval_raw(target), target.fallback()) {
            (Ok(Some(v)), _) => v,
            (Err(e), _) if e.code == "cancelled" => return Err(e),
            (_, Some(f)) => serde_json::to_value(f).expect("a target encodes as JSON"),
            (Ok(None), None) => return Err(no_target("the target is undefined".into())),
            (Err(e), None) => return Err(e),
        };
        let named = serde_json::from_value::<Target>(value.clone())
            .map_err(|_| {
                no_target(format!(
                    "{value} isn't a target: a connection ID or {{conn}}"
                ))
            })?
            .conn;
        // In ID form, as state holds it; an alias, as an expression may write it.
        let conn = if named == core_conn::CORE {
            named
        } else {
            let targets = self
                .inner
                .targets
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            if targets.contains_key(&named) {
                named
            } else if let Some(id) = self.inner.aliases().get(&named) {
                id.clone()
            } else {
                return Err(no_target(format!("no connection {named:?}")));
            }
        };
        let plugin = if conn == core_conn::CORE {
            Some(core_conn::CORE.to_owned())
        } else {
            self.inner
                .targets
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .get(&conn)
                .map(|t| t.plugin.clone())
        };
        let wanted = constraint.map(|c| c.plugin.as_str());
        if wanted.is_none() || plugin.as_deref() != wanted {
            return Err(no_target(format!(
                "connection {conn:?} isn't one of plugin {}'s",
                wanted.unwrap_or("?")
            )));
        }
        Ok(super::ConnRef { conn })
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

    async fn wait(&mut self, duration: &Source<super::Duration>) -> Result<Done, RunError> {
        let duration = match duration {
            Source::Literal(d) => d.get(),
            other => {
                let bad = |v: &dyn std::fmt::Debug| {
                    RunError::new(
                        "invalidDuration",
                        format!(
                            "{v:?} is not a duration: give a string like \"2s\" or milliseconds"
                        ),
                    )
                };
                let evaluated = self.eval_raw(other).and_then(|v| match v {
                    Some(serde_json::Value::String(text)) => text
                        .parse::<super::Duration>()
                        .map(|d| d.get())
                        .map_err(|_| bad(&text)),
                    Some(serde_json::Value::Number(n)) => match (n.as_u64(), n.as_f64()) {
                        (Some(ms), _) => Ok(StdDuration::from_millis(ms)),
                        (None, Some(f)) if f.is_finite() && f >= 0.0 => {
                            Ok(StdDuration::from_secs_f64(f / 1000.0))
                        }
                        _ => Err(bad(&n)),
                    },
                    other => Err(bad(&other)),
                });
                match (evaluated, other.fallback()) {
                    (Ok(d), _) => d,
                    (Err(e), _) if e.code == "cancelled" => return Err(e),
                    (Err(_), Some(fallback)) => fallback.get(),
                    (Err(e), None) => return Err(e),
                }
            }
        };
        self.sleep(duration).await?;
        Ok(Done::default())
    }

    /// Done as soon as `until` is truthy: now, or after a change to what it
    /// reads (spec §8.3).
    async fn wait_until(&mut self, until: &str) -> Result<Done, RunError> {
        let mut feed = self.inner.shared.state.subscribe_turns();
        loop {
            let Evaluation { value, reads } = self.inner.evaluate(
                until,
                &self.flow.id,
                Some(&self.trigger),
                Some(self.scope()),
            );
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
                    ..Done::default()
                });
            }
            // Wait for a change it read, or a cancel.
            feed.done();
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
                    Ok(dirty) if !reads.touched_by(&dirty) => feed.done(),
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
                if read_lock(&self.inner.read_only).contains(name) {
                    return Ok(Err(RunError::new(
                        "readOnly",
                        format!("vars.{name} is computed; only its own value sets it"),
                    )));
                }
                let at = StatePath::new(VARS_ROOT, vec![PathSegment::Key(name.to_owned())]);
                let op = match &value {
                    Some(v) => JournalOp::Set {
                        path: at.clone(),
                        value: v.clone(),
                        reducer: None,
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
                        device_ts: None,
                    }),
                )
                .cause(step_started)
                .run(self.run.clone())
                .instance(self.place.instance.clone());
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
    /// A `call` step (spec §8.3): fires `flow`'s `call` trigger with the
    /// inputs bound, caused by this step. With `wait` it waits for that run
    /// and takes its outputs; a cancel or a timeout while it waits cancels it.
    async fn call(
        &mut self,
        step: &str,
        flow: &str,
        inputs: &BTreeMap<String, Source<serde_json::Value>>,
        wait: bool,
        step_started: Seq,
    ) -> Result<Done, RunError> {
        let mut given = BTreeMap::new();
        for (name, source) in inputs {
            if let Some(v) = self.eval_source(source)? {
                given.insert(name.clone(), v);
            }
        }
        let no_flow = || RunError::new("unknownFlow", format!("no flow {flow}"));
        let callee = self
            .inner
            .book()
            .flows
            .get(flow)
            .map(|s| s.flow.clone())
            .ok_or_else(no_flow)?;
        if !callee.enabled {
            return Err(RunError::new(
                "disabled",
                format!("flow {} is disabled", callee.name),
            ));
        }
        let trigger = callee
            .triggers
            .iter()
            .position(|t| matches!(t, Trigger::Call {}))
            .ok_or_else(|| {
                RunError::new(
                    "notCallable",
                    format!("flow {} has no call trigger", callee.name),
                )
            })?;
        let input = self.inner.bind_inputs(&callee, given, true)?;
        let payload = Value::Map(vec![(
            "caller".into(),
            Value::Map(vec![
                ("flow".into(), self.flow.id.as_str().into()),
                ("run".into(), self.run.as_str().into()),
            ]),
        )]);
        let failed_condition = self.inner.failed_condition(&callee, &payload);
        let fired = {
            let mut book = self.inner.book();
            if !book.flows.contains_key(flow) {
                return Err(no_flow());
            }
            let fire = Fire {
                trigger,
                cause: Some(step_started),
                origin: self.origin(Some(step)),
                payload,
                input,
                failed_condition,
            };
            self.inner
                .fire(&mut book, flow, fire)
                .map_err(|e| RunError::new("internal", e.to_string()))?
        };
        let run = match fired.outcome {
            FireOutcome::Started(run) | FireOutcome::Queued(run) => run,
            FireOutcome::Blocked(reason) => {
                let (code, why) = match reason {
                    BlockReason::Loop => ("loop", "it's already in this chain of calls"),
                    BlockReason::Ignore => ("busy", "it's running and ignores new triggers"),
                    BlockReason::QueueFull => ("queueFull", "its queue is full"),
                    BlockReason::Condition => ("condition", "one of its conditions is false"),
                };
                return Err(RunError::new(
                    code,
                    format!("flow {} didn't start: {why}", callee.name),
                ));
            }
        };
        if !wait {
            return Ok(Done {
                result: Some(Value::Map(vec![("run".into(), run.as_str().into())])),
                ..Done::default()
            });
        }

        let done = self.inner.book().live.get(&run).map(|l| l.done.clone());
        let mut guard = CancelOnDrop {
            inner: self.inner.clone(),
            run: Some(run.clone()),
        };
        if let Some(mut done) = done {
            let cancel = &mut self.cancel;
            let interrupted = async {
                if cancel.wait_for(|c| *c).await.is_err() {
                    std::future::pending::<()>().await;
                }
            };
            let finished = tokio::select! {
                _ = done.wait_for(|d| *d) => true,
                () = interrupted => false,
            };
            if !finished {
                drop(guard);
                let _ = done.wait_for(|d| *d).await;
                return Err(cancelled_error());
            }
        }
        guard.run = None;
        let info = self
            .inner
            .book()
            .recent
            .iter()
            .rev()
            .find(|r| r.run == run)
            .cloned()
            .ok_or_else(|| RunError::new("internal", format!("run {run} finished out of sight")))?;
        match info.status {
            RunPhase::Ok => Ok(Done {
                result: Some(json_to_value(
                    &info.outputs.unwrap_or_else(|| serde_json::json!({})),
                )),
                ..Done::default()
            }),
            RunPhase::Failed => {
                let e = info
                    .error
                    .unwrap_or_else(|| RunError::new("failed", "the called flow failed"));
                Err(RunError::new(
                    e.code,
                    format!("flow {} failed: {}", callee.name, e.message),
                ))
            }
            _ => Err(RunError::new(
                "callCancelled",
                format!("flow {} was cancelled", callee.name),
            )),
        }
    }

    /// A `stop` step (spec §2.3): its outputs, checked against the flow's
    /// `outputs`, and how the run ends.
    fn stop(
        &mut self,
        status: StopStatus,
        outputs: &BTreeMap<String, Source<serde_json::Value>>,
        error: Option<&str>,
    ) -> Result<Done, RunError> {
        let mut out = Object::new();
        for (name, source) in outputs {
            let Some(v) = self.eval_source(source)? else {
                continue;
            };
            let Some(schema) = self.flow.outputs.get(name) else {
                return Err(RunError::new(
                    "unknownOutput",
                    format!("the flow has no output {name:?}"),
                ));
            };
            let problems = schema::check(&serde_json::Value::Object(schema.clone()), &v);
            if !problems.is_empty() {
                let why: Vec<String> = problems.into_iter().map(|m| m.message).collect();
                return Err(RunError::new(
                    "invalidOutputs",
                    format!("output {name:?}: {}", why.join("; ")),
                ));
            }
            out.insert(name.clone(), json_to_value(&v));
        }
        let error = match (status, error) {
            (StopStatus::Ok, _) => None,
            (StopStatus::Failed, None) => {
                Some(RunError::new("stopped", "a stop step failed the run"))
            }
            (StopStatus::Failed, Some(message)) => {
                let text = match self.eval(message)? {
                    Some(Value::String(s)) => s.into_str().unwrap_or_default(),
                    Some(other) => to_json(&other).to_string(),
                    None => "a stop step failed the run".to_owned(),
                };
                Some(RunError::new("stopped", text))
            }
        };
        Ok(Done {
            stop: Some(Stopped {
                status,
                outputs: out,
                error,
            }),
            ..Done::default()
        })
    }

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
                arm: Some(serde_json::Value::Null),
                ..Done::default()
            }));
        };
        Ok(match self.run_list(steps, position).await? {
            End::Ok => Ok(Done {
                arm: Some(arm),
                ..Done::default()
            }),
            End::Failed(e) => Err(e),
            End::Cancelled => Err(cancelled_error()),
            End::Stopped(stopped) => Ok(Done {
                arm: Some(arm),
                stop: Some(stopped),
                ..Done::default()
            }),
        })
    }
}

/// Whether `from` (dataflow §3.1) picks the event: `Some((captures,
/// source))`. `view` is the event's connection, when known.
pub(crate) fn source_matches(
    from: &EventSource,
    event: &EventRecord,
    aliases: &BTreeMap<String, ConnId>,
    eval: &mut Eval<'_>,
    view: Option<TargetView>,
) -> Option<(Option<Vec<String>>, Option<Target>)> {
    let failed = |e: ExprError| {
        tracing::warn!("an event source's expression failed, matching nothing: {e}");
    };
    // A control selector, tried on the one control the event is from.
    // IDs, lists and combinators are implemented here; the other forms
    // are `unsupported` until a field needs them.
    let control_in = |controls: &Option<crate::select::Selector<crate::select::Controls>>,
                      eval: &mut Eval<'_>| {
        let Some(control) = &event.control else {
            return false;
        };
        let Some(selector) = controls else {
            return true;
        };
        let view = ControlView {
            id: control.clone(),
            at: None,
            inputs: Vec::new(),
            outputs: Vec::new(),
        };
        match selector.resolve(&[view], Some(eval)) {
            Ok(picked) => !picked.is_empty(),
            Err(e) => {
                failed(e);
                false
            }
        }
    };
    match from {
        EventSource::Path { path } => match path.matches(&event.path, aliases, Some(&mut *eval)) {
            Ok(Some(c)) => Some(((!c.is_empty()).then_some(c), None)),
            Ok(None) => None,
            Err(e) => {
                failed(e);
                None
            }
        },
        EventSource::Target { target } => {
            let conn = event.conn.as_ref()?;
            // Selector forms test one connection at a time, so the
            // event's own connection is the whole universe needed.
            let view = view.unwrap_or_else(|| TargetView {
                conn: conn.clone(),
                plugin: String::new(),
            });
            match target.resolve(&[view], Some(&mut *eval)) {
                Ok(picked) if !picked.is_empty() => Some((None, Some(Target::conn(conn.clone())))),
                Ok(_) => None,
                Err(e) => {
                    failed(e);
                    None
                }
            }
        }
        EventSource::Layer { layer, controls } => (event.layer.as_ref() == Some(layer)
            && control_in(controls, eval))
        .then_some((None, None)),
        EventSource::Surface { surface, controls } => (event.surface.as_ref() == Some(surface)
            && control_in(controls, eval))
        .then_some((None, None)),
        EventSource::Owner { .. } => None,
    }
}

fn read_lock<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|e| e.into_inner())
}

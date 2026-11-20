//! Cells and the graph (dataflow spec §5, state spec §7.3).
//!
//! A **cell** is a source with a home in the state tree: a variable's value
//! or a control's helper. The graph keeps every cell up to date within the
//! same turn as the change that moved it, so bindings, `state` triggers and
//! `waitUntil` only ever see settled state.
//!
//! **Where it runs.** The graph is the state tree's [`Settler`]: every
//! mutation of [`LiveState`] takes the turn, applies the writer's change,
//! then lets the graph evaluate whatever that change made due, in dependency
//! order, before the turn's dirty set is published. Nothing else writes
//! while it does. Events, wakes and edits start turns of their own through
//! [`LiveState::settle`]: events and wakes from the graph's thread, edits
//! from whoever makes them.
//!
//! **One turn** (§5.3):
//! 1. Cells are due when the turn's changes touch what they read, when an
//!    event their `on` names happened, when their wake is due, or when they
//!    were just added or edited.
//! 2. Due cells are evaluated, each before the cells that read it, and a
//!    cell whose value changes makes its readers due in the same turn.
//! 3. A cell that would be evaluated again in the same turn, and is on a
//!    cycle of reads, keeps the value it had before the turn, as does every
//!    cell on the cycle, and reports `cycle`.
//!
//! **Writing** (§5.1): a change is a `state.patch` with origin
//! `{core: "dataflow"}` and `cause` → the latest entry among the reads that
//! changed, or the event that stepped it. A reducer's patch carries its
//! state and wake (§5.2), and so does the leaf, so a cell resumes from the
//! tree after a restart or a failover. A cell with `journal: sampled`
//! changes state every time but journals at most once per interval, and
//! once more when it settles.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use covalence_protocol::Value;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as Json};
use tokio::sync::{broadcast, mpsc};

use crate::Shared;
use crate::engine::{ActionRequest, Actions};
use crate::expr::{Evaluator, Limits, QuickJs, Scope, json_to_value, to_json};
use crate::journal::{
    CoreOrigin, Entry, EventRecord, Journal, JournalOp, NewEntry, Origin, Record, Seq,
    StatePatchRecord,
};
use crate::select::Target;
use crate::source::{Eval, Ref, Source};
use crate::state::{
    ConnId, Dirty, LiveState, Meta, Quality, ReducerMeta, Settler, Source as Setter, StatePath,
};
use crate::transform::Transform;
use crate::transform::definition::{Definition, Sandbox, StepCtx, StepEvent, Stepped};
use crate::transform::reducer::{Native, Reducer, native};

/// An evaluation's time limit on the turn (state spec §7.3). An unoptimised
/// build spends more than that building a context, so it gets longer.
pub const EVAL_TIME: Duration = if cfg!(debug_assertions) {
    Duration::from_millis(50)
} else {
    Duration::from_millis(2)
};

/// How many times one cell may be evaluated in one turn before it counts
/// as a cycle, however the graph looks (dataflow §9, question 4).
pub const MAX_EVALS: usize = 16;

/// A cell's identity: its entity, never its path (dataflow §5.4).
/// `variable:<id>`, or `helper:<binding id>/<key>`.
pub type CellId = String;

/// What a cell is: where it lives, what gives its value, and where its
/// value goes.
#[derive(Clone, Debug, PartialEq)]
pub struct CellDef {
    pub id: CellId,
    pub home: StatePath,
    /// A stateless source, or a reducer.
    pub source: Source<Json>,
    pub journal: JournalMode,
    /// Sinks (dataflow §7).
    pub to: Vec<Sink>,
    /// A helper's control (surfaces §4.3): its events carry it, and it
    /// pauses `follow` while the control is touched.
    pub control: Option<ControlContext>,
    /// Names in scope that aren't state: a `forEach` item's `item`,
    /// `index` and `key`, for its params (composition §4).
    pub values: Vec<(String, Value)>,
    /// Names in scope that are state: `params` in a component's helpers,
    /// its instance's `<home>.params` (composition §2).
    pub named: Vec<(String, StatePath)>,
}

/// The control a helper belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlContext {
    /// The component's ID.
    pub component: String,
    pub control: String,
    /// The reducer has no `input` of its own: the control's `turn` and
    /// `change` events move it (bindings §5).
    pub input: bool,
    /// The surface of a helper with `scope: "surface"` (surfaces spec §11.4): its
    /// events carry it, and only input that arrived while `component` controlled
    /// the control steps it.
    pub surface: Option<String>,
}

impl CellDef {
    /// A fully journaled cell with no sinks.
    pub fn new(id: impl Into<CellId>, home: StatePath, source: Source<Json>) -> Self {
        CellDef {
            id: id.into(),
            home,
            source,
            journal: JournalMode::Full,
            to: Vec::new(),
            control: None,
            values: Vec::new(),
            named: Vec::new(),
        }
    }
}

/// `follow`'s pause after the last input, when `holdoff` doesn't say
/// (dataflow §4.2).
pub const DEFAULT_HOLDOFF: u64 = 250;

/// Where a cell's value goes (dataflow §7).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum Sink {
    /// A writable variable.
    Set { set: Ref },
    /// An action parameter.
    Action { action: ActionSink },
}

/// An action that receives a cell's value as one of its parameters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActionSink {
    pub target: Source<Target>,
    /// When the target isn't a literal (plugin protocol spec §13.9).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constraint: Option<Json>,
    pub action: String,
    /// The parameter that receives the value.
    pub param: String,
    /// The others.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, Source<Json>>,
    /// Default [`DEFAULT_RATE_LIMIT`]; the latest value wins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limit: Option<crate::flow::Duration>,
}

/// A sink's rate limit when it doesn't set one (dataflow §7).
pub const DEFAULT_RATE_LIMIT: Duration = Duration::from_millis(20);

/// How far back through `cause` a sink looks for its own action.
const ECHO_DEPTH: usize = 32;

/// How a cell's changes are journaled (dataflow §5.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JournalMode {
    #[default]
    Full,
    /// At most one entry per interval, and a last one when it settles.
    Sampled(#[serde(with = "millis")] Duration),
}

mod millis {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(d.as_millis() as u64)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        u64::deserialize(d).map(Duration::from_millis)
    }
}

/// Why a cell has no fresh value: what the editor shows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CellProblem {
    /// `cycle`, `exprError` (or another expression code), `unsupported`.
    pub code: String,
    pub message: String,
}

impl CellProblem {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        CellProblem {
            code: code.into(),
            message: message.into(),
        }
    }
}

/// Scheduler time in milliseconds (dataflow §4.2): reducers get it with
/// each step and never read a clock.
pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// The real clock: milliseconds since the graph started.
pub fn monotonic() -> Clock {
    let start = Instant::now();
    Arc::new(move || start.elapsed().as_millis() as u64)
}

// ---------------------------------------------------------------------------
// The handle

/// The dataflow graph: a handle on the settler and its thread. Dropping it
/// stops the thread and takes the settler off the state tree.
pub struct Dataflow {
    graph: Arc<Graph>,
    commands: mpsc::UnboundedSender<Command>,
}

enum Command {
    /// The wake times may have changed.
    Rearm,
    /// Reply once everything received so far is settled.
    Flush(std::sync::mpsc::Sender<()>),
}

impl Dataflow {
    /// Starts the graph on `shared`'s state, with the real clock.
    pub fn start(shared: &Shared) -> Self {
        Self::with_clock(shared, monotonic())
    }

    /// Starts the graph with its own clock, for tests.
    pub fn with_clock(shared: &Shared, clock: Clock) -> Self {
        let (fires, fired) = mpsc::unbounded_channel();
        let graph = Arc::new(Graph {
            state: Arc::downgrade(&shared.state),
            journal: shared.journal.clone(),
            clock: clock.clone(),
            inner: Mutex::new(Inner::new()),
            fires,
            actions: std::sync::RwLock::new(None),
        });
        shared
            .state
            .set_settler(Some(graph.clone() as Arc<dyn Settler>));
        let (commands, rx) = mpsc::unbounded_channel();
        let events = shared.journal.subscribe();
        let driver = Driver {
            graph: graph.clone(),
            clock,
        };
        std::thread::Builder::new()
            .name("dataflow".into())
            .spawn(move || driver.run(rx, events, fired))
            .expect("the dataflow thread starts");
        Dataflow { graph, commands }
    }

    fn state(&self) -> Option<Arc<LiveState>> {
        self.graph.state.upgrade()
    }

    /// Adds or replaces a cell, and settles a turn for it. A replaced cell
    /// keeps its value and state when its transform is the same `use` at
    /// the same version and its value still fits (dataflow §5.4); otherwise
    /// it starts again, with `cause` (the edit) as the reset's cause.
    pub fn put(&self, def: CellDef, cause: Option<Seq>) {
        self.graph.lock().put(def, cause);
        self.settle();
    }

    /// Removes a cell and its value.
    pub fn remove(&self, id: &str, cause: Option<Seq>) {
        self.graph.lock().remove(id, cause);
        self.settle();
    }

    /// Makes the cells whose IDs start with `prefix` exactly `cells`: a
    /// control binding's helpers, all at once.
    pub fn replace(&self, prefix: &str, cells: Vec<CellDef>, cause: Option<Seq>) {
        let mut inner = self.graph.lock();
        let keep: BTreeSet<CellId> = cells.iter().map(|c| c.id.clone()).collect();
        let gone: Vec<CellId> = inner
            .cells
            .keys()
            .filter(|id| id.starts_with(prefix) && !keep.contains(*id))
            .cloned()
            .collect();
        for id in gone {
            inner.remove(&id, cause);
        }
        for cell in cells {
            let same = inner.cells.get(&cell.id).is_some_and(|c| c.def == cell);
            if !same {
                inner.put(cell, cause);
            }
        }
        drop(inner);
        self.settle();
    }

    /// The connection aliases cells' expressions read, by alias.
    pub fn set_aliases(&self, aliases: BTreeMap<String, ConnId>) {
        let mut inner = self.graph.lock();
        inner.aliases = Arc::new(aliases);
        // Their reads may mean something else now.
        for cell in inner.cells.values_mut() {
            cell.quiet = true;
        }
        let ids: Vec<CellId> = inner.cells.keys().cloned().collect();
        inner.due.extend(ids);
        drop(inner);
        self.settle();
    }

    /// Leader or standby (dataflow §5.2). A standby applies cell patches
    /// as it receives them and never evaluates. On promotion, each reducer
    /// resumes from its leaf's metadata and each stateless cell is
    /// evaluated once against the replicated state.
    pub fn set_active(&self, active: bool) {
        let mut inner = self.graph.lock();
        if inner.active == active {
            return;
        }
        inner.active = active;
        if active {
            for cell in inner.cells.values_mut() {
                cell.run = None;
                cell.evaluated = false;
                cell.quiet = true;
            }
            let ids: Vec<CellId> = inner.cells.keys().cloned().collect();
            inner.due.extend(ids);
        }
        drop(inner);
        self.settle();
    }

    /// Who carries out action sinks' requests: the core.
    pub fn set_actions(&self, actions: Arc<dyn Actions>) {
        *self
            .graph
            .actions
            .write()
            .unwrap_or_else(|e| e.into_inner()) = Some(actions);
    }

    /// Each cell's current problem.
    pub fn problems(&self) -> BTreeMap<CellId, CellProblem> {
        self.graph
            .lock()
            .cells
            .iter()
            .filter_map(|(id, c)| c.problem.clone().map(|p| (id.clone(), p)))
            .collect()
    }

    /// The cells, by ID.
    pub fn cells(&self) -> Vec<CellDef> {
        self.graph
            .lock()
            .cells
            .values()
            .map(|c| c.def.clone())
            .collect()
    }

    fn settle(&self) {
        if let Some(state) = self.state() {
            state.settle();
        }
        let _ = self.commands.send(Command::Rearm);
    }

    /// Returns once every event journaled before the call has stepped what
    /// it steps. It spins rather than sleeps, so a paused test clock
    /// doesn't move while it waits.
    pub async fn flush(&self) {
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = self.commands.send(Command::Flush(tx));
        loop {
            match rx.try_recv() {
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    std::thread::yield_now();
                    tokio::task::yield_now().await;
                }
                _ => return,
            }
        }
    }

    /// Settles a turn now, for wakes that are due by the clock: what the
    /// graph's thread does on its own when they come due.
    pub fn tick(&self) {
        self.settle();
    }
}

impl Drop for Dataflow {
    fn drop(&mut self) {
        if let Some(state) = self.state() {
            state.set_settler(None);
        }
    }
}

// ---------------------------------------------------------------------------
// The thread: events and wakes

struct Driver {
    graph: Arc<Graph>,
    clock: Clock,
}

impl Driver {
    fn run(
        self,
        mut commands: mpsc::UnboundedReceiver<Command>,
        mut events: broadcast::Receiver<Arc<Entry>>,
        fired: mpsc::UnboundedReceiver<Fire>,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("the dataflow runtime starts");
        runtime.spawn(send_sinks(fired, Arc::downgrade(&self.graph)));
        runtime.block_on(async {
            loop {
                let wake = self.graph.lock().next_wake();
                let sleep = async {
                    match wake {
                        Some(at) => {
                            let wait = at.saturating_sub((self.clock)());
                            tokio::time::sleep(Duration::from_millis(wait)).await;
                        }
                        None => std::future::pending().await,
                    }
                };
                tokio::select! {
                    biased;
                    command = commands.recv() => match command {
                        Some(Command::Rearm) => {}
                        Some(Command::Flush(done)) => {
                            self.drain(&mut events);
                            self.turn();
                            let _ = done.send(());
                        }
                        None => return,
                    },
                    first = events.recv() => match first {
                        Ok(entry) => {
                            self.take(entry);
                            self.drain(&mut events);
                            self.turn();
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("dataflow fell behind the journal; {n} entries missed");
                        }
                        Err(broadcast::error::RecvError::Closed) => return,
                    },
                    () = sleep => self.turn(),
                }
            }
        });
    }

    fn drain(&self, events: &mut broadcast::Receiver<Arc<Entry>>) {
        loop {
            match events.try_recv() {
                Ok(entry) => self.take(entry),
                Err(broadcast::error::TryRecvError::Lagged(n)) => {
                    tracing::warn!("dataflow fell behind the journal; {n} entries missed");
                }
                Err(_) => return,
            }
        }
    }

    /// Queues an event for the next turn. The graph's own events were
    /// handled in the turn that made them.
    fn take(&self, entry: Arc<Entry>) {
        if matches!(entry.record, Record::Event(_)) && !is_dataflow(&entry.origin) {
            self.graph.lock().events.push_back(entry);
        }
    }

    fn turn(&self) {
        if let Some(state) = self.graph.state.upgrade() {
            state.settle();
        }
    }
}

/// Sends action sinks' requests, at most one per sink per rate limit, the
/// latest value winning while one waits (dataflow §7).
async fn send_sinks(mut fired: mpsc::UnboundedReceiver<Fire>, graph: Weak<Graph>) {
    use tokio::time::Instant as At;
    struct Slot {
        last: Option<At>,
        rate: Duration,
        pending: Option<ActionRequest>,
    }
    let send = |request: ActionRequest| {
        let actions = graph
            .upgrade()
            .and_then(|g| g.actions.read().unwrap_or_else(|e| e.into_inner()).clone());
        match actions {
            Some(actions) => {
                tokio::spawn(async move {
                    let what = format!("{}.{}", request.conn, request.action);
                    if let Err(e) = actions.request_action(request).await {
                        tracing::warn!(action = %what, "a sink's action failed: {e}");
                    }
                });
            }
            None => tracing::warn!(
                action = %request.action,
                "a sink fired, but nothing carries out actions here"
            ),
        }
    };
    let mut slots: BTreeMap<(CellId, usize), Slot> = BTreeMap::new();
    loop {
        let next = slots
            .values()
            .filter(|s| s.pending.is_some())
            .filter_map(|s| s.last.map(|l| l + s.rate))
            .min();
        let wait = async {
            match next {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            fire = fired.recv() => {
                let Some(fire) = fire else { return };
                let now = At::now();
                let slot = slots.entry(fire.key).or_insert(Slot {
                    last: None,
                    rate: fire.rate,
                    pending: None,
                });
                slot.rate = fire.rate;
                if slot.last.is_none_or(|l| now >= l + slot.rate) {
                    slot.last = Some(now);
                    slot.pending = None;
                    send(fire.request);
                } else {
                    slot.pending = Some(fire.request);
                }
            }
            () = wait => {
                let now = At::now();
                for slot in slots.values_mut() {
                    if slot.pending.is_some() && slot.last.is_none_or(|l| now >= l + slot.rate) {
                        slot.last = Some(now);
                        send(slot.pending.take().expect("checked"));
                    }
                }
            }
        }
    }
}

fn is_dataflow(origin: &Origin) -> bool {
    matches!(
        origin,
        Origin::Core {
            core: CoreOrigin::Dataflow
        }
    )
}

fn dataflow_origin() -> Origin {
    Origin::Core {
        core: CoreOrigin::Dataflow,
    }
}

// ---------------------------------------------------------------------------
// The graph

struct Graph {
    state: Weak<LiveState>,
    journal: Arc<Journal>,
    clock: Clock,
    inner: Mutex<Inner>,
    /// Action sinks' requests, to the task that rate-limits and sends them.
    fires: mpsc::UnboundedSender<Fire>,
    actions: std::sync::RwLock<Option<Arc<dyn Actions>>>,
}

/// One action sink's request.
struct Fire {
    /// The cell and the sink's index in its `to`.
    key: (CellId, usize),
    request: ActionRequest,
    rate: Duration,
}

impl Graph {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

struct Inner {
    cells: BTreeMap<CellId, Cell>,
    aliases: Arc<BTreeMap<String, ConnId>>,
    /// The turn's own runtime (state spec §7.3).
    evaluator: QuickJs,
    /// For reducers with no native implementation.
    sandbox: Sandbox,
    active: bool,
    /// Cells due at the next turn, whatever the turn's changes.
    due: BTreeSet<CellId>,
    /// Events for the next turn.
    events: VecDeque<Arc<Entry>>,
    /// Removed cells' homes, to delete at the next turn, with their cause.
    removed: Vec<(StatePath, Option<Seq>)>,
}

struct Cell {
    def: CellDef,
    /// What its last evaluation read: its source's reads, or a reducer's
    /// input, follow and options.
    reads: BTreeSet<StatePath>,
    /// A reducer's own state, once it has started.
    run: Option<Run>,
    /// Whether it has been evaluated since it was added or resumed.
    evaluated: bool,
    /// The cause to give its next write, when it isn't one of its reads:
    /// the edit that reset it.
    reset_cause: Option<Seq>,
    problem: Option<CellProblem>,
    /// Sampled journaling: when it last journaled, and whether a later value
    /// is waiting for its final entry.
    sampled_at: Option<u64>,
    unsampled: bool,
    /// Its next evaluation sends nothing to its sinks: it was just added,
    /// edited or resumed (dataflow §7).
    quiet: bool,
}

/// A running reducer (dataflow §5.2).
#[derive(Clone, Debug, PartialEq)]
struct Run {
    /// `toggle@1`.
    name: String,
    state: Option<Json>,
    /// Its value, which moves with it when its home does.
    value: Option<Json>,
    /// The input at the last step.
    input: Option<Json>,
    wake_at: Option<u64>,
    /// What `follow` gave when it was last applied (dataflow §4.3).
    followed: Option<Json>,
    /// Whether its control is touched, which pauses `follow`.
    touched: bool,
    /// When input last stepped it, for `holdoff`.
    last_input: Option<u64>,
    /// When a paused `follow` is due to be looked at again.
    follow_at: Option<u64>,
}

impl Inner {
    fn new() -> Self {
        Inner {
            cells: BTreeMap::new(),
            aliases: Arc::new(BTreeMap::new()),
            evaluator: QuickJs::new(Limits {
                time: EVAL_TIME,
                ..Limits::default()
            }),
            sandbox: Sandbox::new(),
            active: true,
            due: BTreeSet::new(),
            events: VecDeque::new(),
            removed: Vec::new(),
        }
    }

    fn put(&mut self, def: CellDef, cause: Option<Seq>) {
        let id = def.id.clone();
        let mut cell = Cell {
            def: def.clone(),
            reads: BTreeSet::new(),
            run: None,
            evaluated: false,
            reset_cause: cause,
            problem: None,
            sampled_at: None,
            unsampled: false,
            quiet: true,
        };
        if let Some(old) = self.cells.remove(&id) {
            if old.def.home != def.home {
                self.removed.push((old.def.home.clone(), cause));
            }
            let kept = match (&old.run, reducer_of(&def.source)) {
                (Some(run), Some(r)) if run.name == r.name() => Some(run.clone()),
                _ => None,
            };
            if let Some(run) = kept {
                // Kept state (dataflow §5.4): it continues, and its new
                // options apply from its next step.
                cell.run = Some(run);
                cell.evaluated = old.evaluated;
                cell.reset_cause = None;
                cell.sampled_at = old.sampled_at;
            }
        }
        self.cells.insert(id.clone(), cell);
        self.due.insert(id);
    }

    fn remove(&mut self, id: &str, cause: Option<Seq>) {
        if let Some(old) = self.cells.remove(id) {
            self.removed.push((old.def.home, cause));
        }
        self.due.remove(id);
    }

    /// The earliest pending wake: a reducer's, or a sample's last entry.
    fn next_wake(&self) -> Option<u64> {
        self.cells
            .values()
            .flat_map(|c| {
                let sample = match (c.def.journal, c.unsampled, c.sampled_at) {
                    (JournalMode::Sampled(every), true, Some(at)) => {
                        Some(at + every.as_millis() as u64)
                    }
                    _ => None,
                };
                let run = c.run.as_ref();
                run.and_then(|r| r.wake_at)
                    .into_iter()
                    .chain(run.and_then(|r| r.follow_at))
                    .chain(sample)
            })
            .min()
    }
}

/// The reducer a cell's source is, if it is one.
fn reducer_of(source: &Source<Json>) -> Option<&Reducer> {
    match source {
        Source::Use { transform, .. } => match &**transform {
            Transform::Reducer(r) => Some(r),
            _ => None,
        },
        _ => None,
    }
}

fn touches(reads: &BTreeSet<StatePath>, path: &StatePath) -> bool {
    reads.iter().any(|r| path.is_within(r) || r.is_within(path))
}

/// One turn's work, while the graph is locked.
struct Turn<'a> {
    inner: &'a mut Inner,
    state: Arc<LiveState>,
    journal: &'a Journal,
    now: u64,
    /// Everything changed so far this turn.
    dirty: &'a mut Dirty,
    /// Each cell's value and leaf metadata before the turn, the first time
    /// it's written, so a cycle can put them back.
    before: BTreeMap<CellId, (Option<Value>, Option<ReducerMeta>)>,
    evals: BTreeMap<CellId, usize>,
    /// Events for each cell to step on, in order, with their entry.
    events: BTreeMap<CellId, Vec<(Option<Seq>, StepEvent)>>,
    /// Whether the cell being evaluated sends to its sinks.
    quiet: bool,
    /// Paths `set` sinks wrote, whose readers are due.
    sunk: Vec<StatePath>,
    fires: mpsc::UnboundedSender<Fire>,
}

impl Settler for Graph {
    fn settle(&self, state: &LiveState, dirty: &mut Dirty) {
        let Some(state_arc) = self.state.upgrade() else {
            return;
        };
        debug_assert!(std::ptr::eq(state, &*state_arc));
        let mut inner = self.lock();
        let removed = std::mem::take(&mut inner.removed);
        for (home, cause) in removed {
            delete_home(&state_arc, &self.journal, &home, cause, dirty);
        }
        if !inner.active {
            // A standby only applies what it's sent.
            inner.events.clear();
            inner.due.clear();
            return;
        }
        let now = (self.clock)();
        let mut turn = Turn {
            inner: &mut inner,
            state: state_arc,
            journal: &self.journal,
            now,
            dirty,
            before: BTreeMap::new(),
            evals: BTreeMap::new(),
            events: BTreeMap::new(),
            quiet: true,
            sunk: Vec::new(),
            fires: self.fires.clone(),
        };
        turn.run();
    }
}

impl Turn<'_> {
    fn run(&mut self) {
        let mut due: BTreeSet<CellId> = std::mem::take(&mut self.inner.due);
        // The turn's changes so far.
        let changed: Vec<StatePath> = self.dirty.iter().cloned().collect();
        for (id, cell) in &self.inner.cells {
            if changed.iter().any(|p| touches(&cell.reads, p)) {
                due.insert(id.clone());
            }
            let woken = cell.run.as_ref().is_some_and(|r| {
                r.wake_at.is_some_and(|at| at <= self.now)
                    || r.follow_at.is_some_and(|at| at <= self.now)
            });
            let sample = match (cell.def.journal, cell.unsampled, cell.sampled_at) {
                (JournalMode::Sampled(every), true, Some(at)) => {
                    at + every.as_millis() as u64 <= self.now
                }
                _ => false,
            };
            if woken || sample {
                due.insert(id.clone());
            }
        }
        let events: Vec<Arc<Entry>> = self.inner.events.drain(..).collect();
        for entry in events {
            if let Record::Event(event) = &entry.record {
                self.route(Some(entry.seq), event, &mut due);
            }
        }
        while let Some(id) = self.next(&due) {
            due.remove(&id);
            let count = {
                let count = self.evals.entry(id.clone()).or_default();
                *count += 1;
                *count
            };
            if count > 1 && (count > MAX_EVALS || self.on_cycle(&id)) {
                self.cycle(&id, &mut due);
                continue;
            }
            let mut changed: Vec<StatePath> = self.evaluate(&id).into_iter().collect();
            changed.append(&mut self.sunk);
            // Their readers are due in the same turn.
            for path in changed {
                for (other, cell) in &self.inner.cells {
                    if *other != id && touches(&cell.reads, &path) {
                        due.insert(other.clone());
                    }
                }
            }
            // Events it gave step other cells in the same turn.
            due.extend(self.events.keys().cloned());
        }
    }

    /// The next due cell: one that reads no other due cell, if there is one.
    fn next(&self, due: &BTreeSet<CellId>) -> Option<CellId> {
        let homes: Vec<(&CellId, &StatePath)> = due
            .iter()
            .filter_map(|id| self.inner.cells.get(id).map(|c| (id, &c.def.home)))
            .collect();
        due.iter()
            .find(|id| {
                let Some(cell) = self.inner.cells.get(*id) else {
                    return true;
                };
                !homes
                    .iter()
                    .any(|(other, home)| other != id && touches(&cell.reads, home))
            })
            .or_else(|| due.iter().next())
            .cloned()
    }

    /// The cells `id` reads, by their homes.
    fn deps(&self, id: &str) -> Vec<CellId> {
        let Some(cell) = self.inner.cells.get(id) else {
            return Vec::new();
        };
        self.inner
            .cells
            .iter()
            .filter(|(_, other)| touches(&cell.reads, &other.def.home))
            .map(|(other, _)| other.clone())
            .collect()
    }

    /// The cells reachable from `id` through what they read.
    fn reachable(&self, id: &str) -> BTreeSet<CellId> {
        let mut seen = BTreeSet::new();
        let mut stack = self.deps(id);
        while let Some(next) = stack.pop() {
            if seen.insert(next.clone()) {
                stack.extend(self.deps(&next));
            }
        }
        seen
    }

    fn on_cycle(&self, id: &str) -> bool {
        self.reachable(id).contains(id)
    }

    /// Every cell on a cycle through `id` keeps its value from before the
    /// turn and reports `cycle` (dataflow §5.3).
    fn cycle(&mut self, id: &str, due: &mut BTreeSet<CellId>) {
        let from = self.reachable(id);
        let mut on: BTreeSet<CellId> = from
            .iter()
            .filter(|other| self.reachable(other).contains(id))
            .cloned()
            .collect();
        on.insert(id.to_owned());
        let names: Vec<String> = on.iter().cloned().collect();
        for cell_id in &on {
            due.remove(cell_id);
            if let Some((value, meta)) = self.before.remove(cell_id)
                && let Some(cell) = self.inner.cells.get(cell_id)
            {
                self.quiet = true;
                let home = cell.def.home.clone();
                let ops = vec![match &value {
                    Some(v) => JournalOp::Set {
                        path: home.clone(),
                        value: v.clone(),
                        reducer: meta.clone(),
                    },
                    None => JournalOp::Delete {
                        path: home.clone(),
                        delete: covalence_protocol::True,
                    },
                }];
                self.patch(&home, value.as_ref(), meta, ops, None, Quality::Good);
            }
            if let Some(cell) = self.inner.cells.get_mut(cell_id) {
                cell.problem = Some(CellProblem::new(
                    "cycle",
                    format!("its reads form a cycle: {}", names.join(" → ")),
                ));
                tracing::warn!(cell = %cell_id, "cell is on a cycle: {}", names.join(" → "));
            }
        }
    }

    /// Queues an event for every reducer whose `on` it matches.
    fn route(&mut self, seq: Option<Seq>, event: &EventRecord, due: &mut BTreeSet<CellId>) {
        let aliases = self.inner.aliases.clone();
        let scope = Scope::new(&self.state, &aliases);
        let mut matched = Vec::new();
        for (id, cell) in &self.inner.cells {
            let Some(r) = reducer_of(&cell.def.source) else {
                continue;
            };
            let Some(on) = &r.on else { continue };
            let Some(from) = &on.from else { continue };
            // A cell never steps on its own events.
            if event.path == cell.def.home {
                continue;
            }
            // Control input is at the control, `surface.<slug>.controls.<id>`;
            // a helper's events are at its cell, under the component or deeper.
            let at_surface =
                event.path.conn == crate::surface::STATE_ROOT && event.path.path.len() == 3;
            // A surface's own helper hears its component's input only.
            if at_surface
                && let Some(c) = &cell.def.control
                && c.surface.is_some()
                && event.component.as_ref() != Some(&c.component)
            {
                continue;
            }
            let control_input = at_surface && cell.def.control.as_ref().is_some_and(|c| c.input);
            let touch = at_surface
                && cell.def.control.is_some()
                && r.follow.is_some()
                && matches!(event.event.as_str(), "touch" | "untouch");
            let named = touch
                || match &on.event {
                    Some(names) => names.contains(&event.event),
                    None => {
                        r.definition().steps_on(&event.event)
                            || (control_input && matches!(event.event.as_str(), "turn" | "change"))
                    }
                };
            if !named {
                continue;
            }
            let mut eval = Eval::new(&scope, &self.inner.evaluator as &dyn Evaluator);
            if crate::flow::runtime::source_matches(from, event, &aliases, &mut eval, None)
                .is_none()
            {
                continue;
            }
            let payload = event.payload.as_ref().map(to_json);
            if let Some(filter) = &on.filter {
                let trigger = json_to_value(&serde_json::json!({
                    "event": event.event,
                    "payload": payload,
                }));
                let scope = Scope {
                    trigger: Some(&trigger),
                    ..Scope::new(&self.state, &aliases)
                };
                let result = self.inner.evaluator.evaluate(filter, &scope);
                if !matches!(result.value, Ok(ref v) if crate::expr::truthy(v.as_ref())) {
                    continue;
                }
            }
            matched.push((
                id.clone(),
                StepEvent {
                    name: event.event.clone(),
                    payload,
                },
            ));
        }
        for (id, step) in matched {
            self.events.entry(id.clone()).or_default().push((seq, step));
            due.insert(id);
        }
    }

    /// Evaluates one cell and writes what changed. Its home, if its value did.
    fn evaluate(&mut self, id: &str) -> Option<StatePath> {
        let cell = self.inner.cells.get_mut(id)?;
        self.quiet = cell.quiet || !cell.evaluated;
        cell.quiet = false;
        let def = cell.def.clone();
        let events = self.events.remove(id).unwrap_or_default();
        match reducer_of(&def.source) {
            Some(r) => self.reduce(id, &def, r, events),
            None => self.compute(id, &def),
        }
    }

    fn scope_reads(&self, reads: &BTreeSet<StatePath>) -> (Option<Seq>, Quality) {
        // `cause`: the latest entry among the reads that changed this turn
        // (dataflow §5.1); quality: stale if anything read is.
        let mut cause = None;
        let mut stale = false;
        self.state.read(|tree| {
            for r in reads {
                for (path, leaf) in tree.leaves_under(r) {
                    if leaf.meta.quality == Quality::Stale {
                        stale = true;
                    }
                    if self.dirty.contains(path) {
                        cause = cause.max(Some(leaf.meta.seq));
                    }
                }
            }
        });
        (cause, if stale { Quality::Stale } else { Quality::Good })
    }

    /// A stateless cell.
    fn compute(&mut self, id: &str, def: &CellDef) -> Option<StatePath> {
        let aliases = self.inner.aliases.clone();
        let scope = Scope {
            values: &def.values,
            named: &def.named,
            ..Scope::new(&self.state, &aliases)
        };
        let outcome = def.source.evaluate(&scope, &self.inner.evaluator);
        let (value, problem) = match (outcome.value, def.source.fallback()) {
            (Ok(Some(v)), _) => (Some(Some(v)), None),
            (Ok(None), Some(f)) | (Err(_), Some(f)) => (Some(Some(f.clone())), None),
            (Ok(None), None) => (Some(None), None),
            (Err(e), None) => (None, Some(CellProblem::new(e.code.as_str(), e.message))),
        };
        let cell = self.inner.cells.get_mut(id)?;
        cell.reads = outcome.reads;
        cell.problem = problem.clone();
        cell.evaluated = true;
        if let Some(p) = &problem {
            tracing::warn!(cell = %id, "cell failed: {}", p.message);
        }
        // A failure keeps the previous value.
        let value = value?;
        let reads = cell.reads.clone();
        let (cause, quality) = self.scope_reads(&reads);
        let cause = cause.or(self.inner.cells.get_mut(id)?.reset_cause.take());
        self.write(id, value.map(|v| json_to_value(&v)), None, cause, quality)
    }

    /// A reducer cell: started, resumed or stepped.
    fn reduce(
        &mut self,
        id: &str,
        def: &CellDef,
        r: &Reducer,
        events: Vec<(Option<Seq>, StepEvent)>,
    ) -> Option<StatePath> {
        let d = r.definition();
        let aliases = self.inner.aliases.clone();
        let scope = Scope {
            values: &def.values,
            named: &def.named,
            ..Scope::new(&self.state, &aliases)
        };
        let evaluator = &self.inner.evaluator;
        let mut eval = Eval::new(&scope, evaluator as &dyn Evaluator);
        let mut failed: Option<CellProblem> = None;
        let mut fail = |e: crate::expr::ExprError| {
            failed.get_or_insert(CellProblem::new(e.code.as_str(), e.message));
        };
        let input = match &r.input {
            Some(s) => eval.nested(s).unwrap_or_else(|e| {
                fail(e);
                None
            }),
            None => None,
        };
        let followed = match &r.follow {
            Some(s) => eval.nested(s).unwrap_or_else(|e| {
                fail(e);
                None
            }),
            None => None,
        };
        let mut options = Map::new();
        for (name, s) in &r.options {
            match eval.nested(s) {
                Ok(Some(v)) => {
                    options.insert(name.clone(), v);
                }
                Ok(None) => {}
                Err(e) => fail(e),
            }
        }
        let holdoff = match &r.holdoff {
            Some(s) => match eval.nested(s) {
                Ok(v) => millis(v.as_ref()).unwrap_or(DEFAULT_HOLDOFF),
                Err(e) => {
                    fail(e);
                    DEFAULT_HOLDOFF
                }
            },
            None => DEFAULT_HOLDOFF,
        };
        let reads = eval.reads;
        let options = d.with_defaults(&options);
        let current = self.state.read(|tree| {
            (
                tree.value(&def.home).as_ref().map(to_json),
                tree.reducer(&def.home).cloned(),
            )
        });

        let cell = self.inner.cells.get_mut(id)?;
        cell.reads = reads;
        cell.problem = failed.clone();
        if let Some(p) = &failed {
            tracing::warn!(cell = %id, "cell failed: {}", p.message);
            return None;
        }
        let first = cell.run.is_none();
        let reset_cause = cell.reset_cause.take();
        let mut run = match cell.run.take() {
            Some(run) => run,
            None => match resume(d, &current) {
                // Resumed from the leaf (dataflow §5.2): a replicated patch,
                // a restored variable, or the same cell before an edit.
                Some(run) => Run {
                    input: input.clone(),
                    followed: followed.clone(),
                    ..run
                },
                None => {
                    let init = match native(&d.id) {
                        Some(n) => Ok(n.init(&options, self.now)),
                        None => self.inner.sandbox.init(d, &options, self.now),
                    };
                    let (state, value) = match init {
                        Ok(sv) => sv,
                        Err(e) => {
                            let cell = self.inner.cells.get_mut(id)?;
                            cell.problem = Some(CellProblem::new(e.code.as_str(), e.message));
                            return None;
                        }
                    };
                    let mut run = Run {
                        name: d.name(),
                        state,
                        value: value.clone(),
                        input: input.clone(),
                        wake_at: None,
                        followed: None,
                        touched: false,
                        last_input: None,
                        follow_at: None,
                    };
                    // It starts where what it follows is.
                    if r.follow.is_some() && followed.is_some() {
                        let ctx = StepCtx {
                            followed: followed.clone(),
                            value: value.clone(),
                            options: options.clone(),
                            now: self.now,
                            ..StepCtx::default()
                        };
                        if let Ok(s) = follow(&self.inner.sandbox, d, run.state.as_ref(), &ctx) {
                            run.state = s.state;
                            run.value = s.value;
                        }
                        run.followed = followed.clone();
                    }
                    let value = run.value.clone();
                    let meta = meta_of(&run, value.as_ref());
                    let cell = self.inner.cells.get_mut(id)?;
                    cell.run = Some(run);
                    cell.evaluated = true;
                    return self.write(
                        id,
                        value.as_ref().map(json_to_value),
                        Some(meta),
                        reset_cause,
                        Quality::Good,
                    );
                }
            },
        };
        if first {
            // Resumed: nothing to step unless a wake is already past.
            let past = run.wake_at.is_some_and(|at| at <= self.now);
            let cell = self.inner.cells.get_mut(id)?;
            cell.evaluated = true;
            if !past && events.is_empty() {
                cell.run = Some(run);
                return None;
            }
        }

        // Steps: each event in order (a helper's control input moves its
        // input), then its own input's change, or a wake. Then `follow`.
        let control_input = def.control.as_ref().is_some_and(|c| c.input);
        let mut value = run.value.clone();
        let mut emitted: Vec<(Option<Seq>, crate::transform::definition::Emitted)> = Vec::new();
        let mut cause: Option<Seq> = None;
        let mut stepped_any = false;
        let mut steps: Vec<(Option<Seq>, Option<StepEvent>, Option<Json>)> = Vec::new();
        for (seq, e) in events {
            match e.name.as_str() {
                "touch" | "untouch" if def.control.is_some() && r.follow.is_some() => {
                    run.touched = e.name == "touch";
                    if !run.touched {
                        run.last_input = Some(self.now);
                        run.follow_at = Some(self.now + holdoff);
                    }
                    let named = r.on.as_ref().and_then(|on| on.event.as_ref());
                    if !named.is_some_and(|n| n.contains(&e.name)) && !d.steps_on(&e.name) {
                        continue;
                    }
                    steps.push((seq, Some(e), None));
                }
                "turn" | "change" if control_input && !d.steps_on(&e.name) => {
                    let payload = e.payload.as_ref();
                    let moved = if e.name == "turn" {
                        let delta = payload.and_then(|p| p.get("delta")).and_then(Json::as_f64);
                        let base = steps
                            .iter()
                            .rev()
                            .find_map(|(_, _, i)| i.clone())
                            .or_else(|| run.input.clone())
                            .and_then(|v| v.as_f64())
                            .unwrap_or(0.0);
                        delta.map(|d| crate::transform::reducer::num(base + d))
                    } else {
                        payload.and_then(|p| p.get("value")).cloned()
                    };
                    if let Some(moved) = moved {
                        steps.push((seq, None, Some(moved)));
                    }
                }
                _ => steps.push((seq, Some(e), None)),
            }
        }
        if r.input.is_some() && input != run.input {
            steps.push((None, None, input.clone()));
        }
        let woken = run.wake_at.is_some_and(|at| at <= self.now);
        if steps.is_empty() && woken {
            steps.push((None, None, None));
        }
        // Due for its sample's last entry, a paused `follow`, or an
        // option's change, which applies from its next step.
        let sample_flush = steps.is_empty();
        // A control's detents count from where they were when it started:
        // its first turn moves it.
        let mut current_input = match &run.input {
            None if control_input => Some(Json::from(0)),
            other => other.clone(),
        };
        for (seq, event, moved) in steps {
            let previous = current_input.clone();
            let is_input = moved.is_some();
            if let Some(m) = moved {
                current_input = Some(m);
            }
            let wake = event.is_none() && !is_input;
            if !wake {
                run.last_input = Some(self.now);
            }
            let ctx = StepCtx {
                event,
                input: current_input.clone(),
                previous: if is_input {
                    previous
                } else {
                    current_input.clone()
                },
                value: value.clone(),
                followed: followed.clone(),
                options: options.clone(),
                now: self.now,
            };
            let result = step(
                &self.inner.sandbox,
                d,
                native(&d.id),
                run.state.as_ref(),
                &ctx,
            );
            match result {
                Ok(Some(s)) => {
                    run.state = s.state;
                    value = s.value;
                    run.wake_at = s.wake_at;
                    emitted.extend(s.events.into_iter().map(|e| (seq, e)));
                    stepped_any = true;
                    cause = cause.max(seq);
                }
                Ok(None) => {
                    if wake {
                        // A wake that changed nothing ends.
                        run.wake_at = None;
                    }
                }
                Err(e) => {
                    let cell = self.inner.cells.get_mut(id)?;
                    tracing::warn!(cell = %id, "reducer failed; it keeps its state: {e}");
                    cell.problem = Some(CellProblem::new(e.code.as_str(), e.message));
                    break;
                }
            }
        }
        run.input = current_input;

        // `follow` (dataflow §4.3): paused while the control is touched and
        // for `holdoff` after the last input, then caught up.
        let mut followed_now = false;
        if r.follow.is_some() {
            let resume_at = run.last_input.map(|t| t + holdoff);
            let paused = run.touched || resume_at.is_some_and(|at| self.now < at);
            if paused {
                run.follow_at = if run.touched { None } else { resume_at };
            } else {
                run.follow_at = None;
                if followed != run.followed {
                    let ctx = StepCtx {
                        followed: followed.clone(),
                        value: value.clone(),
                        options: options.clone(),
                        now: self.now,
                        ..StepCtx::default()
                    };
                    match follow(&self.inner.sandbox, d, run.state.as_ref(), &ctx) {
                        Ok(s) => {
                            run.state = s.state;
                            value = s.value;
                            run.followed = followed.clone();
                            followed_now = true;
                        }
                        Err(e) => tracing::warn!(cell = %id, "follow failed: {e}"),
                    }
                }
            }
        }
        run.value = value.clone();
        let meta = meta_of(&run, value.as_ref());
        let cell = self.inner.cells.get_mut(id)?;
        cell.run = Some(run);
        if !stepped_any
            && !followed_now
            && !sample_flush
            && current.1.as_ref() == Some(&meta)
            && current.0 == value
        {
            return None;
        }
        // Following the device sends nothing back to it.
        if followed_now && !stepped_any {
            self.quiet = true;
        }
        let (read_cause, _) = {
            let reads = cell.reads.clone();
            self.scope_reads(&reads)
        };
        let cause = cause.or(read_cause).or(reset_cause);
        let home = self.write(
            id,
            value.as_ref().map(json_to_value),
            Some(meta),
            cause,
            Quality::Good,
        );
        // Events happen at the cell (dataflow §3), after its patch.
        let patch = self.journal.last_seq();
        for (seq, e) in emitted {
            let record = EventRecord {
                surface: def.control.as_ref().and_then(|c| c.surface.clone()),
                component: def.control.as_ref().map(|c| c.component.clone()),
                control: def.control.as_ref().map(|c| c.control.clone()),
                ..EventRecord::at(
                    def.home.clone(),
                    e.name.clone(),
                    e.payload.as_ref().map(json_to_value),
                )
            };
            let event_cause = if home.is_some() { patch } else { seq };
            let appended = self.journal.append(
                NewEntry::new(dataflow_origin(), Record::Event(record.clone())).cause(event_cause),
            );
            match appended {
                Ok(entry) => {
                    let mut due = BTreeSet::new();
                    self.route(Some(entry.seq), &record, &mut due);
                }
                Err(e) => tracing::warn!(cell = %id, "cannot journal a cell's event: {e}"),
            }
        }
        home
    }

    /// Writes a cell's value and metadata when they changed. Its home, if
    /// the value did.
    fn write(
        &mut self,
        id: &str,
        value: Option<Value>,
        meta: Option<ReducerMeta>,
        cause: Option<Seq>,
        quality: Quality,
    ) -> Option<StatePath> {
        let cell = self.inner.cells.get(id)?;
        let home = cell.def.home.clone();
        let (old_value, old_meta, old_quality) = self.state.read(|tree| {
            (
                tree.value(&home),
                tree.reducer(&home).cloned(),
                tree.leaf(&home).map(|l| l.meta.quality),
            )
        });
        let value_changed = old_value != value;
        let quality_changed = value.is_some() && old_quality.is_some_and(|q| q != quality);
        let sample_due = cell.unsampled
            && matches!(cell.def.journal, JournalMode::Sampled(every)
                if cell.sampled_at.is_none_or(|at| at + every.as_millis() as u64 <= self.now));
        if !value_changed && !quality_changed && old_meta == meta && !sample_due {
            return None;
        }
        self.before
            .entry(id.to_owned())
            .or_insert((old_value, old_meta));
        let ops = vec![match &value {
            Some(v) => JournalOp::Set {
                path: home.clone(),
                value: v.clone(),
                reducer: meta.clone(),
            },
            None => JournalOp::Delete {
                path: home.clone(),
                delete: covalence_protocol::True,
            },
        }];
        let journaled = match cell.def.journal {
            JournalMode::Full => true,
            JournalMode::Sampled(every) => cell
                .sampled_at
                .is_none_or(|at| at + every.as_millis() as u64 <= self.now),
        };
        let cell = self.inner.cells.get_mut(id)?;
        if journaled {
            cell.sampled_at = Some(self.now);
            cell.unsampled = false;
        } else {
            cell.unsampled = true;
        }
        let quality = if value.is_some() {
            quality
        } else {
            Quality::Unknown
        };
        let patch = if journaled {
            self.patch(&home, value.as_ref(), meta, ops, cause, quality)
        } else {
            // Applied to state, not journaled: the leaf points at what
            // moved it.
            let seq = cause.or(self.journal.last_seq()).unwrap_or(0);
            self.apply(&home, value.as_ref(), meta, seq, quality);
            Some(seq)
        };
        if value_changed
            && !self.quiet
            && let Some(v) = &value
        {
            self.sink(id, v, patch, cause);
        }
        value_changed.then_some(home)
    }

    /// Sends a cell's new value to its sinks (dataflow §7). `patch` is the
    /// cell's own entry; `cause` what moved the cell.
    fn sink(&mut self, id: &str, value: &Value, patch: Option<Seq>, cause: Option<Seq>) {
        let Some(cell) = self.inner.cells.get(id) else {
            return;
        };
        let home = cell.def.home.clone();
        let to = cell.def.to.clone();
        for (i, sink) in to.iter().enumerate() {
            match sink {
                Sink::Set { set } => {
                    // Checked on save: a `set` sink writes `vars.<name>`.
                    let Some(at) = set.state_path() else { continue };
                    let ops = vec![JournalOp::Set {
                        path: at.clone(),
                        value: value.clone(),
                        reducer: None,
                    }];
                    self.patch(&at, Some(value), None, ops, patch, Quality::Good);
                    self.sunk.push(at);
                }
                Sink::Action { action } => {
                    let aliases = self.inner.aliases.clone();
                    let scope = Scope::new(&self.state, &aliases);
                    let mut eval = Eval::new(&scope, &self.inner.evaluator as &dyn Evaluator);
                    let target = match action.target.literal() {
                        Some(t) => Some(t.clone()),
                        None => eval
                            .raw(&action.target)
                            .ok()
                            .flatten()
                            .or_else(|| {
                                action
                                    .target
                                    .fallback()
                                    .and_then(|f| serde_json::to_value(f).ok())
                            })
                            .and_then(|j| serde_json::from_value::<Target>(j).ok()),
                    };
                    let Some(target) = target else {
                        tracing::warn!(cell = %id, "a sink's target gave no connection; nothing sent");
                        continue;
                    };
                    let mut params: Vec<(Value, Value)> = Vec::new();
                    let mut failed = false;
                    for (name, s) in &action.params {
                        match eval.nested(s) {
                            Ok(Some(v)) => {
                                params.push((Value::from(name.as_str()), json_to_value(&v)))
                            }
                            Ok(None) => {}
                            Err(e) => {
                                tracing::warn!(cell = %id, param = %name, "a sink's parameter failed; nothing sent: {e}");
                                failed = true;
                            }
                        }
                    }
                    if failed || self.echoes(cause, &home, &target.conn, &action.action) {
                        continue;
                    }
                    params.push((Value::from(action.param.as_str()), value.clone()));
                    let mut request = ActionRequest::new(
                        dataflow_origin(),
                        target.conn.clone(),
                        action.action.clone(),
                        Value::Map(params),
                    );
                    request.cause = patch;
                    let rate = action
                        .rate_limit
                        .as_ref()
                        .map_or(DEFAULT_RATE_LIMIT, |d| d.get());
                    let _ = self.fires.send(Fire {
                        key: (id.to_owned(), i),
                        request,
                        rate,
                    });
                }
            }
        }
    }

    /// Whether a change descends, through `cause`, from this cell's own
    /// earlier action to `conn`'s `action` (dataflow §7): a mixer's volume
    /// coming back after the fader that set it.
    fn echoes(&self, cause: Option<Seq>, home: &StatePath, conn: &str, action: &str) -> bool {
        let mut next = cause;
        for _ in 0..ECHO_DEPTH {
            let Some(seq) = next else { return false };
            let Ok(Some(entry)) = self.journal.get(seq) else {
                return false;
            };
            if let Record::ActionRequested(r) = &entry.record
                && is_dataflow(&entry.origin)
                && r.conn == conn
                && r.action == action
                && let Some(Ok(Some(from))) = entry.cause.map(|c| self.journal.get(c))
                && let Record::StatePatch(p) = &from.record
                && p.ops.iter().any(|op| match op {
                    JournalOp::Set { path, .. } | JournalOp::Delete { path, .. } => path == home,
                })
            {
                return true;
            }
            next = entry.cause;
        }
        false
    }

    /// Journals a cell's patch and applies it.
    fn patch(
        &mut self,
        home: &StatePath,
        value: Option<&Value>,
        meta: Option<ReducerMeta>,
        ops: Vec<JournalOp>,
        cause: Option<Seq>,
        quality: Quality,
    ) -> Option<Seq> {
        let entry = NewEntry::new(
            dataflow_origin(),
            Record::StatePatch(StatePatchRecord {
                ops,
                replaces: None,
                device_ts: None,
            }),
        )
        .cause(cause);
        match self.journal.append(entry) {
            Ok(e) => {
                self.apply(home, value, meta, e.seq, quality);
                Some(e.seq)
            }
            Err(e) => {
                tracing::warn!(home = ?home, "cannot journal a cell's patch: {e}");
                None
            }
        }
    }

    fn apply(
        &mut self,
        home: &StatePath,
        value: Option<&Value>,
        meta: Option<ReducerMeta>,
        seq: Seq,
        quality: Quality,
    ) {
        let leaf = Meta {
            ts: crate::clock::epoch_ms(),
            seq,
            quality,
            source: Setter::Core,
        };
        let changed = self.state.write_in_turn(|tree| {
            let changed = match value {
                Some(v) => tree.set(home, v, &leaf),
                None => tree.delete(home),
            };
            tree.set_reducer(home, meta);
            changed
        });
        self.dirty.extend(changed);
    }
}

/// A removed cell's value goes too.
fn delete_home(
    state: &LiveState,
    journal: &Journal,
    home: &StatePath,
    cause: Option<Seq>,
    dirty: &mut Dirty,
) {
    if state.read(|tree| tree.value(home).is_none() && tree.reducer(home).is_none()) {
        return;
    }
    let entry = NewEntry::new(
        dataflow_origin(),
        Record::StatePatch(StatePatchRecord {
            ops: vec![JournalOp::Delete {
                path: home.clone(),
                delete: covalence_protocol::True,
            }],
            replaces: None,
            device_ts: None,
        }),
    )
    .cause(cause);
    if let Err(e) = journal.append(entry) {
        tracing::warn!(home = ?home, "cannot journal a cell's removal: {e}");
        return;
    }
    let changed = state.write_in_turn(|tree| {
        tree.set_reducer(home, None);
        tree.delete(home)
    });
    dirty.extend(changed);
}

/// A reducer's metadata: its state only when it isn't its value.
fn meta_of(run: &Run, value: Option<&Json>) -> ReducerMeta {
    ReducerMeta {
        transform: run.name.clone(),
        state: if run.state.as_ref() == value {
            None
        } else {
            Some(run.state.clone().unwrap_or(Json::Null))
        },
        wake_at: run.wake_at,
    }
}

/// A reducer resumed from its leaf, when the leaf's metadata is from the same
/// definition and its value still fits (dataflow §5.2, §5.4).
fn resume(d: &Definition, (value, meta): &(Option<Json>, Option<ReducerMeta>)) -> Option<Run> {
    let meta = meta.as_ref()?;
    if meta.transform != d.name() || !fits(value.as_ref(), &d.result) {
        return None;
    }
    Some(Run {
        name: meta.transform.clone(),
        state: meta.state.clone().or_else(|| value.clone()),
        value: value.clone(),
        input: None,
        wake_at: meta.wake_at,
        followed: None,
        touched: false,
        last_input: None,
        follow_at: None,
    })
}

/// Whether a value fits a definition's `result` type. Only `type` is
/// checked; `undefined` always fits.
pub fn fits(value: Option<&Json>, result: &Json) -> bool {
    let Some(value) = value else { return true };
    let types: Vec<&str> = match result.get("type") {
        Some(Json::String(t)) => vec![t.as_str()],
        Some(Json::Array(ts)) => ts.iter().filter_map(Json::as_str).collect(),
        _ => return true,
    };
    types.iter().any(|t| match *t {
        "boolean" => value.is_boolean(),
        "number" => value.is_number(),
        "string" => value.is_string(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => true,
    })
}

/// What following a value does: the definition's `follow`, or the value
/// as both state and value.
fn follow(
    sandbox: &Sandbox,
    d: &Definition,
    state: Option<&Json>,
    ctx: &StepCtx,
) -> Result<Stepped, crate::expr::ExprError> {
    let custom = match native(&d.id) {
        Some(n) => n.follow(state, ctx),
        None => sandbox.follow(d, state, ctx)?,
    };
    Ok(custom.unwrap_or_else(|| Stepped {
        state: ctx.followed.clone(),
        value: ctx.followed.clone(),
        ..Stepped::default()
    }))
}

/// A duration as milliseconds: a number, or text like `250ms`.
fn millis(v: Option<&Json>) -> Option<u64> {
    match v? {
        Json::Number(n) => n.as_f64().map(|x| x.max(0.0) as u64),
        Json::String(s) => s
            .parse::<crate::flow::Duration>()
            .ok()
            .map(|d| d.get().as_millis() as u64),
        _ => None,
    }
}

fn step(
    sandbox: &Sandbox,
    d: &Definition,
    native: Option<&dyn Native>,
    state: Option<&Json>,
    ctx: &StepCtx,
) -> Result<Option<Stepped>, crate::expr::ExprError> {
    match native {
        Some(n) => Ok(n.step(state, ctx)),
        None => sandbox.step(d, state, ctx),
    }
}

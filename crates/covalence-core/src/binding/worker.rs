//! The binding worker (bindings spec §3, ADR 0023): a thread of its own
//! that keeps the bindings of shown layers evaluated and draws what changes.
//!
//! One turn takes every state change waiting on the feed, re-evaluates the
//! properties whose reads they touch on layers some surface shows, and
//! redraws each control whose resolved look differs from what that surface
//! last got. A layer that comes into view is evaluated in full first.
//!
//! A `value` property drives a `level` output (ADR 0035). Each level sent is
//! written to `surface.<slug>.controls.<id>.requested.level`, with `cause` →
//! the latest change among what the property read.
//! `expr` bindings run on the worker's own QuickJS, so bindings and flows
//! never wait on each other.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tokio::sync::{broadcast, mpsc};

use super::{Binding, ControlBinding, Prop, PropValue, prop_value};
use crate::Shared;
use crate::expr::{Evaluator, Limits, QuickJs, Scope};
use crate::journal::{
    CoreOrigin, Journal, JournalOp, NewEntry, Origin, Record, Seq, StatePatchRecord,
};
use crate::state::{ConnId, Dirty, LiveState, Meta, Quality, Source, StatePath};
use crate::surface::Output;
use crate::surface::render::{Look, Renderer};
use crate::surface::{
    ControlDef, Draw, LayerId, OutputCap, OutputCapKind, SurfaceId, requested_level_path,
};

/// An `expr` binding's time limit (ADR 0023).
pub const EXPR_TIME: Duration = Duration::from_millis(10);

/// The last error of each binding property, by binding ID and the
/// property's name (`label`, `gestures.longAfter`): what the editor shows.
pub type Problems = BTreeMap<(String, String), String>;

/// A handle on the worker thread. Dropping it stops the thread.
pub struct Bindings {
    commands: mpsc::UnboundedSender<Command>,
    problems: Arc<Mutex<Problems>>,
}

enum Command {
    Set(Box<ControlBinding>),
    Remove(String),
    Aliases(Arc<BTreeMap<String, ConnId>>),
    /// A surface shows `layer` (or nothing) through `outputs`.
    Show {
        surface: SurfaceId,
        slug: String,
        layer: Option<LayerId>,
        controls: Vec<ControlDef>,
        outputs: mpsc::Sender<Output>,
    },
    Hide(SurfaceId),
    Flush(std::sync::mpsc::Sender<()>),
}

impl Bindings {
    /// Starts the worker on its own thread.
    pub fn start(shared: Shared) -> Self {
        let (commands, rx) = mpsc::unbounded_channel();
        let problems = Arc::new(Mutex::new(Problems::new()));
        let feed = shared.state.subscribe();
        let worker = Worker {
            state: shared.state,
            journal: shared.journal,
            evaluator: QuickJs::new(Limits {
                time: EXPR_TIME,
                ..Limits::default()
            }),
            aliases: Arc::new(BTreeMap::new()),
            bindings: BTreeMap::new(),
            by_control: BTreeMap::new(),
            shown: BTreeMap::new(),
            resolved: BTreeMap::new(),
            renderer: Renderer::default(),
            problems: problems.clone(),
        };
        std::thread::Builder::new()
            .name("bindings".into())
            .spawn(move || worker.run(rx, feed))
            .expect("the bindings thread starts");
        Bindings { commands, problems }
    }

    fn send(&self, command: Command) {
        // The thread only stops when this handle is dropped.
        let _ = self.commands.send(command);
    }

    /// Adds or replaces a binding.
    pub fn set(&self, binding: ControlBinding) {
        self.send(Command::Set(Box::new(binding)));
    }

    pub fn remove(&self, id: &str) {
        self.send(Command::Remove(id.to_owned()));
    }

    /// The connection aliases `expr` bindings read, by alias.
    pub fn set_aliases(&self, aliases: BTreeMap<String, ConnId>) {
        self.send(Command::Aliases(Arc::new(aliases)));
    }

    pub fn show(
        &self,
        surface: &str,
        slug: &str,
        layer: Option<LayerId>,
        controls: Vec<ControlDef>,
        outputs: mpsc::Sender<Output>,
    ) {
        self.send(Command::Show {
            surface: surface.to_owned(),
            slug: slug.to_owned(),
            layer,
            controls,
            outputs,
        });
    }

    pub fn hide(&self, surface: &str) {
        self.send(Command::Hide(surface.to_owned()));
    }

    /// The last error of each failing binding property.
    pub fn problems(&self) -> Problems {
        lock(&self.problems).clone()
    }

    /// Records, or clears, a property's error found outside the worker,
    /// such as a gesture setting read at a press. Logged when it changes.
    pub fn note(&self, binding: &str, prop: &str, error: Option<String>) {
        note(&self.problems, binding, prop, error);
    }

    /// Returns once the worker has handled every command sent before, and
    /// every state change published before. It spins rather than sleeps, so
    /// a paused test clock doesn't move while it waits.
    pub async fn flush(&self) {
        let (tx, rx) = std::sync::mpsc::channel();
        self.send(Command::Flush(tx));
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
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn note(problems: &Mutex<Problems>, id: &str, prop: &str, error: Option<String>) {
    let mut problems = lock(problems);
    let key = (id.to_owned(), prop.to_owned());
    match error {
        Some(message) => {
            if problems.get(&key) != Some(&message) {
                tracing::warn!(binding = %id, prop, "binding failed: {message}");
                problems.insert(key, message);
            }
        }
        None => {
            problems.remove(&key);
        }
    }
}

struct Worker {
    state: Arc<LiveState>,
    journal: Arc<Journal>,
    evaluator: QuickJs,
    aliases: Arc<BTreeMap<String, ConnId>>,
    bindings: BTreeMap<String, ControlBinding>,
    /// Binding IDs by `(layer, control)`.
    by_control: BTreeMap<(LayerId, String), String>,
    shown: BTreeMap<SurfaceId, Shown>,
    /// The bound controls of shown layers, evaluated.
    resolved: BTreeMap<(LayerId, String), Resolved>,
    renderer: Renderer,
    problems: Arc<Mutex<Problems>>,
}

struct Shown {
    slug: String,
    layer: Option<LayerId>,
    controls: Vec<ControlDef>,
    outputs: mpsc::Sender<Output>,
    /// What each control was last sent.
    drawn: BTreeMap<String, Look>,
    /// The level each `level` output was last sent.
    levels: BTreeMap<String, f64>,
}

#[derive(Default)]
struct Resolved {
    props: BTreeMap<Prop, Evaluated>,
    look: Look,
    /// The `value` property, for a `level` output: `None` sends nothing.
    level: Option<f64>,
}

impl Resolved {
    fn level(&self) -> Option<f64> {
        match self.props.get(&Prop::Value)?.value {
            Some(PropValue::Level(v)) => Some(v),
            _ => None,
        }
    }
}

/// One property, evaluated.
#[derive(Clone, Debug, PartialEq)]
pub struct Evaluated {
    /// Its value, or its fallback, or nothing: the control's default.
    pub value: Option<PropValue>,
    /// Why its own result wasn't used, if it failed.
    pub error: Option<String>,
    pub reads: BTreeSet<StatePath>,
}

/// Evaluates one property as the worker does: the binding's result if it
/// fits the property, else its fallback, else nothing.
pub fn evaluate(
    prop: Prop,
    b: &Binding,
    state: &Arc<LiveState>,
    aliases: &BTreeMap<String, ConnId>,
    evaluator: &dyn Evaluator,
) -> Evaluated {
    let outcome = b.evaluate(&Scope::new(state, aliases), evaluator);
    let (value, reads) = (outcome.value.map_err(|e| e.to_string()), outcome.reads);
    let (value, error) = match value {
        Ok(Some(v)) => match prop_value(prop, &v) {
            Ok(pv) => (Some(pv), None),
            Err(e) => (None, Some(e)),
        },
        Ok(None) => (None, None),
        Err(e) => (None, Some(e)),
    };
    let value = value.or_else(|| b.fallback().and_then(|f| prop_value(prop, f).ok()));
    Evaluated {
        value,
        error,
        reads,
    }
}

/// A control's look from the properties that draw its image. Stale if any
/// value they read is.
pub fn look(props: &BTreeMap<Prop, Evaluated>, state: &LiveState) -> Look {
    let image = || {
        props
            .iter()
            .filter(|(p, _)| p.output() == OutputCapKind::Image)
    };
    let mut look = Look::default();
    for (prop, evaluated) in image() {
        if let Some(value) = &evaluated.value {
            value.clone().apply(*prop, &mut look);
        }
    }
    look.stale = state.read(|tree| {
        image().flat_map(|(_, e)| &e.reads).any(|path| {
            tree.leaves_under(path)
                .any(|(_, leaf)| leaf.meta.quality == Quality::Stale)
        })
    });
    look
}

/// A binding evaluated once against live state, for the editor's preview.
#[derive(Clone, Debug, PartialEq)]
pub struct Preview {
    pub props: BTreeMap<Prop, Evaluated>,
    pub look: Look,
}

pub fn preview(
    binding: &ControlBinding,
    state: &Arc<LiveState>,
    aliases: &BTreeMap<String, ConnId>,
    evaluator: &dyn Evaluator,
) -> Preview {
    let props: BTreeMap<Prop, Evaluated> = binding
        .props
        .iter()
        .map(|(prop, b)| (*prop, evaluate(*prop, b, state, aliases, evaluator)))
        .collect();
    let look = look(&props, state);
    Preview { props, look }
}

impl Worker {
    fn run(
        mut self,
        mut commands: mpsc::UnboundedReceiver<Command>,
        mut feed: broadcast::Receiver<Arc<Dirty>>,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("the bindings runtime starts");
        runtime.block_on(async {
            loop {
                tokio::select! {
                    biased;
                    command = commands.recv() => match command {
                        Some(command) => self.command(command, &mut feed),
                        None => return,
                    },
                    first = feed.recv() => {
                        let dirty = match first {
                            Ok(d) => Some((*d).clone()),
                            Err(broadcast::error::RecvError::Lagged(_)) => None,
                            Err(broadcast::error::RecvError::Closed) => return,
                        };
                        self.turn(dirty, &mut feed);
                    }
                }
            }
        });
    }

    /// One turn: `first` and everything else waiting on the feed. After a
    /// lag (`None`), every shown binding is evaluated again.
    fn turn(&mut self, first: Option<Dirty>, feed: &mut broadcast::Receiver<Arc<Dirty>>) {
        let mut dirty = first;
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
        let keys: Vec<(LayerId, String)> = self.resolved.keys().cloned().collect();
        let mut changed = BTreeSet::new();
        for key in keys {
            if self.reevaluate(&key, dirty.as_ref()) {
                changed.insert(key);
            }
        }
        if !changed.is_empty() {
            let surfaces: Vec<SurfaceId> = self.shown.keys().cloned().collect();
            for surface in surfaces {
                self.draw(&surface);
            }
        }
    }

    fn command(&mut self, command: Command, feed: &mut broadcast::Receiver<Arc<Dirty>>) {
        match command {
            Command::Set(binding) => {
                self.forget(&binding.id);
                let key = (binding.layer.clone(), binding.control.clone());
                self.by_control.insert(key.clone(), binding.id.clone());
                self.bindings.insert(binding.id.clone(), *binding);
                if self.is_visible(&key.0) {
                    self.resolve(&key);
                    self.draw_all();
                }
            }
            Command::Remove(id) => {
                if let Some(key) = self.forget(&id) {
                    self.resolved.remove(&key);
                    if self.is_visible(&key.0) {
                        self.resolved.insert(key, Resolved::default());
                        self.draw_all();
                    }
                }
            }
            Command::Aliases(aliases) => {
                self.aliases = aliases;
                let keys: Vec<_> = self.resolved.keys().cloned().collect();
                for key in keys {
                    self.resolve(&key);
                }
                self.draw_all();
            }
            Command::Show {
                surface,
                slug,
                layer,
                controls,
                outputs,
            } => {
                // The same device keeps what it shows; a new connection
                // starts blank, as far as the core knows. Levels are sent
                // again under a new slug, so `requested` is written there.
                let (drawn, levels) = match self.shown.remove(&surface) {
                    Some(old) if old.outputs.same_channel(&outputs) => (
                        old.drawn,
                        if old.slug == slug {
                            old.levels
                        } else {
                            BTreeMap::new()
                        },
                    ),
                    _ => Default::default(),
                };
                self.shown.insert(
                    surface.clone(),
                    Shown {
                        slug,
                        layer,
                        controls,
                        outputs,
                        drawn,
                        levels,
                    },
                );
                self.track_visible();
                self.draw(&surface);
            }
            Command::Hide(surface) => {
                self.shown.remove(&surface);
                self.track_visible();
            }
            Command::Flush(done) => {
                self.turn(Some(Dirty::new()), feed);
                let _ = done.send(());
            }
        }
    }

    /// Removes a binding, returning where it was.
    fn forget(&mut self, id: &str) -> Option<(LayerId, String)> {
        let old = self.bindings.remove(id)?;
        let key = (old.layer, old.control);
        self.by_control.remove(&key);
        let mut problems = lock(&self.problems);
        problems.retain(|(binding, _), _| binding != id);
        Some(key)
    }

    fn is_visible(&self, layer: &str) -> bool {
        self.shown
            .values()
            .any(|s| s.layer.as_deref() == Some(layer))
    }

    /// Evaluates layers that came into view in full, and drops layers that
    /// nothing shows any more.
    fn track_visible(&mut self) {
        let visible: BTreeSet<LayerId> = self
            .shown
            .values()
            .filter_map(|s| s.layer.clone())
            .collect();
        self.resolved
            .retain(|(layer, _), _| visible.contains(layer));
        let fresh: Vec<(LayerId, String)> = self
            .by_control
            .keys()
            .filter(|(layer, _)| visible.contains(layer))
            .filter(|key| !self.resolved.contains_key(*key))
            .cloned()
            .collect();
        for key in fresh {
            self.resolve(&key);
        }
    }

    /// Evaluates every property of one control.
    fn resolve(&mut self, key: &(LayerId, String)) {
        self.resolved.insert(key.clone(), Resolved::default());
        self.reevaluate(key, None);
    }

    /// Re-evaluates the properties of one control whose reads `dirty`
    /// touches (all of them for `None`). Whether its look changed.
    fn reevaluate(&mut self, key: &(LayerId, String), dirty: Option<&Dirty>) -> bool {
        let Some(binding) = self
            .by_control
            .get(key)
            .and_then(|id| self.bindings.get(id))
            .cloned()
        else {
            return false;
        };
        let Some(mut resolved) = self.resolved.remove(key) else {
            return false;
        };
        let touched = |reads: &BTreeSet<StatePath>, d: &Dirty| {
            d.iter()
                .any(|d| reads.iter().any(|r| d.is_within(r) || r.is_within(d)))
        };
        let mut any = resolved.props.is_empty();
        for (prop, b) in &binding.props {
            let due = match (dirty, resolved.props.get(prop)) {
                (None, _) | (_, None) => true,
                (Some(d), Some(e)) => touched(&e.reads, d),
            };
            if due {
                let evaluated = self.evaluate(&binding.id, *prop, b);
                resolved.props.insert(*prop, evaluated);
                any = true;
            }
        }
        let changed = if any {
            let look = self.look(&resolved);
            let level = resolved.level();
            let changed = look != resolved.look || level != resolved.level;
            resolved.look = look;
            resolved.level = level;
            changed
        } else {
            false
        };
        self.resolved.insert(key.clone(), resolved);
        changed
    }

    /// One property. Errors are kept for the editor and logged when they change.
    fn evaluate(&mut self, id: &str, prop: Prop, b: &Binding) -> Evaluated {
        let evaluated = evaluate(prop, b, &self.state, &self.aliases, &self.evaluator);
        self.note(id, prop, evaluated.error.clone());
        evaluated
    }

    fn note(&self, id: &str, prop: Prop, error: Option<String>) {
        note(&self.problems, id, prop.as_str(), error);
    }

    fn look(&self, resolved: &Resolved) -> Look {
        look(&resolved.props, &self.state)
    }

    fn draw_all(&mut self) {
        let surfaces: Vec<SurfaceId> = self.shown.keys().cloned().collect();
        for surface in surfaces {
            self.draw(&surface);
        }
    }

    /// Sends a surface every control whose look or level differs from what
    /// it has.
    fn draw(&mut self, surface: &str) {
        let Some(shown) = self.shown.get_mut(surface) else {
            return;
        };
        let mut requested = Vec::new();
        for control in &shown.controls {
            let resolved = shown
                .layer
                .as_ref()
                .and_then(|layer| self.resolved.get(&(layer.clone(), control.id.clone())));
            let has_level = control
                .outputs
                .iter()
                .any(|o| matches!(o, OutputCap::Level { .. }));
            if has_level
                && let Some(resolved) = resolved
                && let Some(level) = resolved.level
                && shown.levels.get(&control.id) != Some(&level)
            {
                let output = Output::Level {
                    control: control.id.clone(),
                    value: level,
                };
                match shown.outputs.try_send(output) {
                    Ok(()) => {
                        shown.levels.insert(control.id.clone(), level);
                        let reads = resolved.props.get(&Prop::Value).map(|e| &e.reads);
                        requested.push((control.id.clone(), level, reads.cloned()));
                    }
                    // Tried again on the next turn, with the newest value.
                    Err(_) => {
                        tracing::warn!(%surface, control = %control.id, "surface isn't taking levels; dropped one");
                    }
                }
            }
            let Some(image) = control.image() else {
                continue;
            };
            let look = resolved.map(|r| r.look.clone()).unwrap_or_default();
            if shown.drawn.get(&control.id) == Some(&look) {
                continue;
            }
            let bytes =
                match self
                    .renderer
                    .render(&look, image.w, image.h, &image.format, image.rotation)
                {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        tracing::warn!(%surface, control = %control.id, "cannot draw: {e}");
                        continue;
                    }
                };
            let draw = Output::Image(Draw {
                control: control.id.clone(),
                format: image.format.clone(),
                image: bytes,
            });
            match shown.outputs.try_send(draw) {
                Ok(()) => {
                    shown.drawn.insert(control.id.clone(), look);
                }
                Err(_) => {
                    tracing::warn!(%surface, control = %control.id, "surface isn't taking images; dropped one");
                }
            }
        }
        let slug = shown.slug.clone();
        for (control, level, reads) in requested {
            self.requested(surface, &slug, &control, level, reads.as_ref());
        }
    }

    /// Writes a level sent as `requested.level`, with `cause` → the latest
    /// change among what the `value` property read.
    fn requested(
        &self,
        surface: &str,
        slug: &str,
        control: &str,
        level: f64,
        reads: Option<&BTreeSet<StatePath>>,
    ) {
        let cause: Option<Seq> = self.state.read(|tree| {
            reads?
                .iter()
                .flat_map(|path| tree.leaves_under(path))
                .map(|(_, leaf)| leaf.meta.seq)
                .max()
        });
        let path = requested_level_path(slug, control);
        let value: covalence_protocol::Value = level.into();
        let record = Record::StatePatch(StatePatchRecord {
            ops: vec![JournalOp::Set {
                path: path.clone(),
                value: value.clone(),
                reducer: None,
            }],
            replaces: None,
            device_ts: None,
        });
        let origin = Origin::Core {
            core: CoreOrigin::Dataflow,
        };
        let journal = &self.journal;
        let (written, _) = self.state.mutate_turn(|tree| {
            match journal.append(NewEntry::new(origin, record).cause(cause)) {
                Ok(entry) => {
                    let meta = Meta {
                        ts: crate::clock::epoch_ms(),
                        seq: entry.seq,
                        quality: Quality::Good,
                        source: Source::Core,
                    };
                    (Ok(()), tree.set(&path, &value, &meta))
                }
                Err(e) => (Err(e), Dirty::new()),
            }
        });
        if let Err(e) = written {
            tracing::error!(%surface, %control, "cannot journal a requested level: {e}");
        }
    }
}

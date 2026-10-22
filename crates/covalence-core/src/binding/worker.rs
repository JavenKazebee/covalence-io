//! The binding worker (bindings spec §3, ADR 0023): a thread of its own
//! that keeps the bindings of shown pages evaluated and draws what changes.
//!
//! One turn takes every state change waiting on the feed, re-evaluates the
//! properties whose reads they touch on pages some surface shows, and
//! redraws each control whose resolved look differs from what that surface
//! last got. A page that comes into view is evaluated in full first.
//! `expr` bindings run on the worker's own QuickJS, so bindings and flows
//! never wait on each other.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde_json::Value as Json;
use tokio::sync::{broadcast, mpsc};

use super::{Binding, ControlBinding, Prop, PropValue, prop_value};
use crate::expr::{Evaluator, Limits, QuickJs, Scope, to_json};
use crate::state::{ConnId, Dirty, LiveState, Quality, StatePath};
use crate::surface::render::{Look, Renderer};
use crate::surface::{ControlDef, Draw, PageId, SurfaceId};

/// An `expr` binding's time limit (ADR 0023).
pub const EXPR_TIME: Duration = Duration::from_millis(10);

/// The last error of each binding property, by binding ID: what the editor shows.
pub type Problems = BTreeMap<(String, Prop), String>;

/// A handle on the worker thread. Dropping it stops the thread.
pub struct Bindings {
    commands: mpsc::UnboundedSender<Command>,
    problems: Arc<Mutex<Problems>>,
}

enum Command {
    Set(ControlBinding),
    Remove(String),
    Aliases(Arc<BTreeMap<String, ConnId>>),
    /// A surface shows `page` (or nothing) through `draws`.
    Show {
        surface: SurfaceId,
        page: Option<PageId>,
        controls: Vec<ControlDef>,
        draws: mpsc::Sender<Draw>,
    },
    Hide(SurfaceId),
    Flush(std::sync::mpsc::Sender<()>),
}

impl Bindings {
    /// Starts the worker on its own thread.
    pub fn start(state: Arc<LiveState>) -> Self {
        let (commands, rx) = mpsc::unbounded_channel();
        let problems = Arc::new(Mutex::new(Problems::new()));
        let feed = state.subscribe();
        let worker = Worker {
            state,
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
        self.send(Command::Set(binding));
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
        page: Option<PageId>,
        controls: Vec<ControlDef>,
        draws: mpsc::Sender<Draw>,
    ) {
        self.send(Command::Show {
            surface: surface.to_owned(),
            page,
            controls,
            draws,
        });
    }

    pub fn hide(&self, surface: &str) {
        self.send(Command::Hide(surface.to_owned()));
    }

    /// The last error of each failing binding property.
    pub fn problems(&self) -> Problems {
        lock(&self.problems).clone()
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

struct Worker {
    state: Arc<LiveState>,
    evaluator: QuickJs,
    aliases: Arc<BTreeMap<String, ConnId>>,
    bindings: BTreeMap<String, ControlBinding>,
    /// Binding IDs by `(page, control)`.
    by_control: BTreeMap<(PageId, String), String>,
    shown: BTreeMap<SurfaceId, Shown>,
    /// The bound controls of shown pages, evaluated.
    resolved: BTreeMap<(PageId, String), Resolved>,
    renderer: Renderer,
    problems: Arc<Mutex<Problems>>,
}

struct Shown {
    page: Option<PageId>,
    controls: Vec<ControlDef>,
    draws: mpsc::Sender<Draw>,
    /// What each control was last sent.
    drawn: BTreeMap<String, Look>,
}

#[derive(Default)]
struct Resolved {
    props: BTreeMap<Prop, Evaluated>,
    look: Look,
}

struct Evaluated {
    value: Option<PropValue>,
    reads: BTreeSet<StatePath>,
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
        let keys: Vec<(PageId, String)> = self.resolved.keys().cloned().collect();
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
                let key = (binding.page.clone(), binding.control.clone());
                self.by_control.insert(key.clone(), binding.id.clone());
                self.bindings.insert(binding.id.clone(), binding);
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
                page,
                controls,
                draws,
            } => {
                // The same device keeps what it shows; a new connection
                // starts blank, as far as the core knows.
                let drawn = match self.shown.remove(&surface) {
                    Some(old) if old.draws.same_channel(&draws) => old.drawn,
                    _ => BTreeMap::new(),
                };
                self.shown.insert(
                    surface.clone(),
                    Shown {
                        page,
                        controls,
                        draws,
                        drawn,
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
    fn forget(&mut self, id: &str) -> Option<(PageId, String)> {
        let old = self.bindings.remove(id)?;
        let key = (old.page, old.control);
        self.by_control.remove(&key);
        let mut problems = lock(&self.problems);
        problems.retain(|(binding, _), _| binding != id);
        Some(key)
    }

    fn is_visible(&self, page: &str) -> bool {
        self.shown.values().any(|s| s.page.as_deref() == Some(page))
    }

    /// Evaluates pages that came into view in full, and drops pages that
    /// nothing shows any more.
    fn track_visible(&mut self) {
        let visible: BTreeSet<PageId> =
            self.shown.values().filter_map(|s| s.page.clone()).collect();
        self.resolved.retain(|(page, _), _| visible.contains(page));
        let fresh: Vec<(PageId, String)> = self
            .by_control
            .keys()
            .filter(|(page, _)| visible.contains(page))
            .filter(|key| !self.resolved.contains_key(*key))
            .cloned()
            .collect();
        for key in fresh {
            self.resolve(&key);
        }
    }

    /// Evaluates every property of one control.
    fn resolve(&mut self, key: &(PageId, String)) {
        self.resolved.insert(key.clone(), Resolved::default());
        self.reevaluate(key, None);
    }

    /// Re-evaluates the properties of one control whose reads `dirty`
    /// touches (all of them for `None`). Whether its look changed.
    fn reevaluate(&mut self, key: &(PageId, String), dirty: Option<&Dirty>) -> bool {
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
            let changed = look != resolved.look;
            resolved.look = look;
            changed
        } else {
            false
        };
        self.resolved.insert(key.clone(), resolved);
        changed
    }

    /// One property: its value, or its fallback, or nothing (the control's
    /// default). Errors are kept for the editor and logged when they change.
    fn evaluate(&mut self, id: &str, prop: Prop, b: &Binding) -> Evaluated {
        let (value, reads) = match b.evaluate(&self.state) {
            Some(outcome) => (outcome.value, outcome.reads),
            None => {
                let Binding::Expr { source, .. } = b else {
                    unreachable!("only expr bindings need the evaluator")
                };
                let scope = Scope::new(&self.state, &self.aliases);
                let evaluation = self.evaluator.evaluate(source, &scope);
                let value = evaluation
                    .value
                    .map(|v| v.as_ref().map(to_json))
                    .map_err(|e| e.to_string());
                (value, evaluation.reads.paths)
            }
        };
        let (value, error) = match value {
            Ok(Some(v)) => match prop_value(prop, &v) {
                Ok(pv) => (Some(pv), None),
                Err(e) => (None, Some(e)),
            },
            Ok(None) => (None, None),
            Err(e) => (None, Some(e)),
        };
        let value = value.or_else(|| b.fallback().and_then(|f: Json| prop_value(prop, &f).ok()));
        self.note(id, prop, error);
        Evaluated { value, reads }
    }

    fn note(&self, id: &str, prop: Prop, error: Option<String>) {
        let mut problems = lock(&self.problems);
        let key = (id.to_owned(), prop);
        match error {
            Some(message) => {
                if problems.get(&key) != Some(&message) {
                    tracing::warn!(binding = %id, prop = prop.as_str(), "binding failed: {message}");
                    problems.insert(key, message);
                }
            }
            None => {
                problems.remove(&key);
            }
        }
    }

    /// The control's look from its properties. Stale if any value it read is.
    fn look(&self, resolved: &Resolved) -> Look {
        let mut look = Look::default();
        for (prop, evaluated) in &resolved.props {
            if let Some(value) = &evaluated.value {
                value.clone().apply(*prop, &mut look);
            }
        }
        look.stale = self.state.read(|tree| {
            resolved.props.values().flat_map(|e| &e.reads).any(|path| {
                tree.leaves_under(path)
                    .any(|(_, leaf)| leaf.meta.quality == Quality::Stale)
            })
        });
        look
    }

    fn draw_all(&mut self) {
        let surfaces: Vec<SurfaceId> = self.shown.keys().cloned().collect();
        for surface in surfaces {
            self.draw(&surface);
        }
    }

    /// Sends a surface every control whose look differs from what it has.
    fn draw(&mut self, surface: &str) {
        let Some(shown) = self.shown.get_mut(surface) else {
            return;
        };
        for control in &shown.controls {
            let Some(image) = &control.image else {
                continue;
            };
            let look = shown
                .page
                .as_ref()
                .and_then(|page| self.resolved.get(&(page.clone(), control.id.clone())))
                .map(|r| r.look.clone())
                .unwrap_or_default();
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
            let draw = Draw {
                control: control.id.clone(),
                format: image.format.clone(),
                image: bytes,
            };
            match shown.draws.try_send(draw) {
                Ok(()) => {
                    shown.drawn.insert(control.id.clone(), look);
                }
                Err(_) => {
                    tracing::warn!(%surface, control = %control.id, "surface isn't taking images; dropped one");
                }
            }
        }
    }
}

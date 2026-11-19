//! Component resolution (surfaces spec §5.2): which component controls each control
//! of each attached surface.
//!
//! For each surface, a component is a candidate where it fits (§5.4) and its
//! `on` picks the surface, and active while its `when` holds. The active
//! components are ordered by `level` and `rank`, then by how recently each
//! became active, then oldest first by ID, and each control goes to the
//! highest active component that covers it.
//!
//! **Where it runs.** The [`Resolver`] is the state tree's resolver
//! ([`LiveState::set_resolver`]): it runs in every turn, after cells have
//! settled (dataflow §5.3), and re-resolves a surface when the turn's
//! changes touch what its components' fields read, or when components, the surface
//! or its controls changed. Its results are state, written in the same
//! turn, so bindings, triggers and `why` see them with the change that
//! caused them:
//! - `surface.<slug>.shown`: the active shown components' slugs, highest first.
//! - `surface.<slug>.controls.<id>.component`: the controlling component's slug, or
//!   `null`.
//!
//! Each is journaled as a `state.patch` with origin `{core: "dataflow"}` and
//! `cause` → the latest change among what the surface's component fields read.
//! The binding worker and the surface's input take the same results from
//! [`Resolver::resolution`].
//!
//! **Armed components** (§5.2) are candidates that aren't active but whose
//! `when` reads the surface's input state. They're listed so the binding
//! worker keeps them evaluated, and a held shift redraws without waiting
//! for its bindings.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use covalence_protocol::Value;
use serde_json::Value as Json;

use super::{Cascade, ComponentId, ControlDef, STATE_ROOT, SurfaceId, When, key, missing};
use crate::binding::{InputMode, Prop};
use crate::component::Component;
use crate::expr::{Limits, QuickJs, Scope};
use crate::journal::{CoreOrigin, Journal, JournalOp, NewEntry, Origin, Record, StatePatchRecord};
use crate::source::Eval;
use crate::state::{ConnId, Dirty, LiveState, Meta, Quality, Settler, Source, StatePath};

/// What resolution found for one surface.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Resolution {
    /// The active components, highest first.
    pub active: Vec<ComponentId>,
    /// The component that has each control's input, by control ID: the
    /// controlling component. A control no active component covers isn't listed.
    pub controls: BTreeMap<String, ComponentId>,
    /// Where each control's bound properties come from (surfaces spec
    /// §5.2.1). A property no component sets isn't listed: it's the default.
    pub look: BTreeMap<String, BTreeMap<Prop, ComponentId>>,
    /// Inactive components whose `when` reads the surface's input state.
    pub armed: BTreeSet<ComponentId>,
}

impl Resolution {
    /// The components whose bindings should be kept evaluated for the surface:
    /// those a control's look or input comes from, and armed ones.
    pub fn evaluated(&self) -> impl Iterator<Item = &ComponentId> {
        let shown: BTreeSet<&ComponentId> = self
            .controls
            .values()
            .chain(self.look.values().flat_map(BTreeMap::values))
            .collect();
        shown.into_iter().chain(self.armed.iter())
    }
}

/// What a component's binding on a control sets, for resolution: the
/// properties it binds and what it does with input.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bound {
    pub props: BTreeSet<Prop>,
    pub input: Option<InputMode>,
}

/// Bindings by `(component, control)`.
pub type BoundMap = BTreeMap<(ComponentId, String), Bound>;

/// Resolves components for every attached surface, in the state turn.
pub struct Resolver {
    state: Weak<LiveState>,
    journal: Arc<Journal>,
    inner: Mutex<Inner>,
    /// The latest resolution of each attached surface.
    published: Mutex<BTreeMap<SurfaceId, Arc<Resolution>>>,
    /// Components that became active on a surface, for whoever adds the cells
    /// they have there (surfaces spec §11.4).
    activated: Mutex<Vec<(SurfaceId, ComponentId)>>,
}

struct Inner {
    components: Arc<BTreeMap<ComponentId, Component>>,
    bound: Arc<BoundMap>,
    /// Every surface's ID: the universe of a component's `on`.
    universe: Vec<SurfaceId>,
    aliases: Arc<BTreeMap<String, ConnId>>,
    /// Evaluates component fields, with a binding's limit.
    evaluator: QuickJs,
    surfaces: BTreeMap<SurfaceId, Target>,
    /// Counts activations, so a component that becomes active goes on top of
    /// its level and rank.
    activations: u64,
    /// Each component field's last error, by component ID and field.
    problems: BTreeMap<(ComponentId, &'static str), String>,
}

/// One surface, as resolution sees it.
struct Target {
    slug: String,
    controls: Vec<ControlDef>,
    attached: bool,
    /// What its components' fields read when it was last resolved.
    reads: BTreeSet<StatePath>,
    /// Resolved at the next turn, whatever its changes.
    due: bool,
    /// Its active components, with when each became active (0: before anything
    /// was seen, so ties go to the oldest component).
    active: BTreeMap<ComponentId, u64>,
    /// Whether it has been resolved since it was added.
    resolved: bool,
}

impl Resolver {
    pub fn new(state: &Arc<LiveState>, journal: Arc<Journal>) -> Self {
        Resolver {
            state: Arc::downgrade(state),
            journal,
            inner: Mutex::new(Inner {
                components: Arc::new(BTreeMap::new()),
                bound: Arc::new(BTreeMap::new()),
                universe: Vec::new(),
                aliases: Arc::new(BTreeMap::new()),
                evaluator: QuickJs::new(Limits {
                    time: crate::binding::worker::EXPR_TIME,
                    ..Limits::default()
                }),
                surfaces: BTreeMap::new(),
                activations: 0,
                problems: BTreeMap::new(),
            }),
            published: Mutex::new(BTreeMap::new()),
            activated: Mutex::new(Vec::new()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        lock(&self.inner)
    }

    /// Replaces the components. Every surface is resolved at the next turn.
    pub fn set_components(&self, components: Arc<BTreeMap<ComponentId, Component>>) {
        let mut inner = self.lock();
        inner.components = components;
        inner.all_due();
    }

    /// Replaces what each binding sets (surfaces spec §5.2.1). Every
    /// surface is resolved at the next turn.
    pub fn set_bound(&self, bound: Arc<BoundMap>) {
        let mut inner = self.lock();
        inner.bound = bound;
        inner.all_due();
    }

    /// Every surface entity's ID, for `on`.
    pub fn set_universe(&self, mut ids: Vec<SurfaceId>) {
        ids.sort();
        let mut inner = self.lock();
        if inner.universe != ids {
            inner.universe = ids;
            inner.all_due();
        }
    }

    pub fn set_aliases(&self, aliases: Arc<BTreeMap<String, ConnId>>) {
        let mut inner = self.lock();
        inner.aliases = aliases;
        inner.all_due();
    }

    /// A surface attached, or was renamed while attached: it's resolved at
    /// the next turn, keeping the order its components became active in.
    pub fn attach(&self, surface: &str, slug: &str, controls: Vec<ControlDef>) {
        let mut inner = self.lock();
        let target = inner
            .surfaces
            .entry(surface.to_owned())
            .or_insert_with(|| Target {
                slug: slug.to_owned(),
                controls: Vec::new(),
                attached: true,
                reads: BTreeSet::new(),
                due: true,
                active: BTreeMap::new(),
                resolved: false,
            });
        target.slug = slug.to_owned();
        target.controls = controls;
        target.attached = true;
        target.due = true;
    }

    /// A surface detached: it isn't resolved until it attaches again.
    pub fn detach(&self, surface: &str) {
        if let Some(target) = self.lock().surfaces.get_mut(surface) {
            target.attached = false;
        }
        lock(&self.published).remove(surface);
    }

    /// A surface was deleted.
    pub fn remove(&self, surface: &str) {
        self.lock().surfaces.remove(surface);
        lock(&self.published).remove(surface);
    }

    /// A surface's latest resolution, while it's attached.
    pub fn resolution(&self, surface: &str) -> Option<Arc<Resolution>> {
        lock(&self.published).get(surface).cloned()
    }

    /// The component controlling a control now.
    pub fn controlling(&self, surface: &str, control: &str) -> Option<ComponentId> {
        lock(&self.published)
            .get(surface)?
            .controls
            .get(control)
            .cloned()
    }

    /// The components that became active on a surface since this was last
    /// called, once each.
    pub fn take_activated(&self) -> Vec<(SurfaceId, ComponentId)> {
        std::mem::take(&mut *lock(&self.activated))
    }

    /// Each failing component field's last error, by component ID and field.
    pub fn problems(&self) -> BTreeMap<(ComponentId, &'static str), String> {
        self.lock().problems.clone()
    }
}

impl Inner {
    fn all_due(&mut self) {
        for target in self.surfaces.values_mut() {
            target.due = true;
        }
    }

    fn note(&mut self, component: &str, field: &'static str, error: Option<String>) {
        let at = (component.to_owned(), field);
        match error {
            Some(message) => {
                if self.problems.get(&at) != Some(&message) {
                    tracing::warn!(%component, field, "a component field failed: {message}");
                    self.problems.insert(at, message);
                }
            }
            None => {
                self.problems.remove(&at);
            }
        }
    }
}

/// One candidate component, evaluated for a surface.
struct Candidate<'a> {
    component: &'a Component,
    active: bool,
    /// Its level's base plus its rank (surfaces spec §5.3).
    order: f64,
    /// Whether its `when` read the surface's input state.
    reads_input: bool,
}

impl Settler for Resolver {
    fn settle(&self, state: &LiveState, dirty: &mut Dirty) {
        let Some(state_arc) = self.state.upgrade() else {
            return;
        };
        debug_assert!(std::ptr::eq(state, &*state_arc));
        let mut inner = self.lock();
        let changed: Vec<StatePath> = dirty.iter().cloned().collect();
        let due: Vec<SurfaceId> = inner
            .surfaces
            .iter()
            .filter(|(_, t)| t.attached)
            .filter(|(_, t)| {
                t.due
                    || changed
                        .iter()
                        .any(|d| t.reads.iter().any(|r| d.is_within(r) || r.is_within(d)))
            })
            .map(|(id, _)| id.clone())
            .collect();
        for surface in due {
            self.resolve(&mut inner, &state_arc, &surface, dirty);
        }
    }
}

impl Resolver {
    fn resolve(&self, inner: &mut Inner, state: &Arc<LiveState>, surface: &str, dirty: &mut Dirty) {
        let components = inner.components.clone();
        let bound = inner.bound.clone();
        let aliases = inner.aliases.clone();
        let Some(target) = inner.surfaces.get(surface) else {
            return;
        };
        let slug = target.slug.clone();
        let controls = target.controls.clone();
        let self_path = StatePath::new(STATE_ROOT, vec![key(&slug)]);
        let input_path = StatePath::new(STATE_ROOT, vec![key(&slug), key("controls")]);
        let named = [("self".to_owned(), self_path)];
        let scope = Scope {
            named: &named,
            ..Scope::new(state, &aliases)
        };
        let mut reads = BTreeSet::new();
        let mut candidates = Vec::new();
        let mut problems: Vec<(String, &'static str, Option<String>)> = Vec::new();
        for component in components.values() {
            if !missing(component, &controls).is_empty() {
                continue;
            }
            if let Some(on) = &component.shows().on {
                let mut eval = Eval::new(&scope, &inner.evaluator);
                let picked = on.picks(surface, &inner.universe, Some(&mut eval));
                reads.extend(eval.reads);
                let error = picked.as_ref().err().map(ToString::to_string);
                problems.push((component.id.clone(), "on", error));
                if !picked.unwrap_or(false) {
                    continue;
                }
            }
            let (active, when_reads) = match component
                .shows()
                .when
                .as_ref()
                .unwrap_or(&When::default())
            {
                When::Nav(group) => {
                    let path =
                        StatePath::new(STATE_ROOT, vec![key(&slug), key("current"), key(group)]);
                    let current = state.value(&path);
                    let active = matches!(&current, Some(Value::String(s)) if s.as_str() == Some(component.slug.as_str()));
                    (active, BTreeSet::from([path]))
                }
                When::Always(_) => (true, BTreeSet::new()),
                When::While(source) => {
                    let mut eval = Eval::new(&scope, &inner.evaluator);
                    let value = eval.raw(source);
                    let value = match value {
                        Ok(Some(v)) => {
                            problems.push((component.id.clone(), "when", None));
                            Some(v)
                        }
                        Ok(None) => source.fallback().cloned(),
                        Err(e) => {
                            problems.push((component.id.clone(), "when", Some(e.to_string())));
                            source.fallback().cloned()
                        }
                    };
                    (crate::transform::truthy(value.as_ref()), eval.reads)
                }
            };
            let rank = match &component.shows().rank {
                None => 0.0,
                Some(source) => {
                    let mut eval = Eval::new(&scope, &inner.evaluator);
                    let value = eval.raw(source);
                    reads.extend(std::mem::take(&mut eval.reads));
                    let rank = match value {
                        Ok(Some(Json::Number(n))) => n.as_f64(),
                        Ok(Some(_)) => {
                            problems.push((
                                component.id.clone(),
                                "rank",
                                Some("a rank is a number".into()),
                            ));
                            None
                        }
                        Ok(None) => None,
                        Err(e) => {
                            problems.push((component.id.clone(), "rank", Some(e.to_string())));
                            None
                        }
                    };
                    rank.or_else(|| source.fallback().copied())
                        .filter(|r| r.is_finite())
                        .map_or(0.0, |r| r.clamp(0.0, 99.0))
                }
            };
            let reads_input = when_reads.iter().any(|r| r.is_within(&input_path));
            reads.extend(when_reads);
            candidates.push(Candidate {
                component,
                active,
                order: component.shows().level.base() + rank,
                reads_input,
            });
        }
        for (component, field, error) in problems {
            inner.note(&component, field, error);
        }

        // Activation order: a component that just became active goes on top.
        let mut activations = inner.activations;
        let target = inner.surfaces.get_mut(surface).expect("still there");
        let first = !target.resolved;
        let mut active = BTreeMap::new();
        let mut newly = Vec::new();
        for c in candidates.iter().filter(|c| c.active) {
            let at = match target.active.get(&c.component.id) {
                Some(&at) => at,
                None => {
                    newly.push(c.component.id.clone());
                    if first {
                        0
                    } else {
                        activations += 1;
                        activations
                    }
                }
            };
            active.insert(c.component.id.clone(), at);
        }
        target.active = active;
        target.reads = reads;
        target.due = false;
        target.resolved = true;
        let target = &*target;

        let mut order: Vec<&Candidate> = candidates.iter().filter(|c| c.active).collect();
        order.sort_by(|a, b| {
            b.order
                .total_cmp(&a.order)
                .then(target.active[&b.component.id].cmp(&target.active[&a.component.id]))
                .then(a.component.id.cmp(&b.component.id))
        });
        let ordered: Vec<&Component> = order.iter().map(|c| c.component).collect();
        let mut by_input = BTreeMap::new();
        let mut look = BTreeMap::new();
        for control in &controls {
            let (input, props) = cascade(&control.id, &ordered, &bound);
            if let Some(component) = input {
                by_input.insert(control.id.clone(), component);
            }
            if !props.is_empty() {
                look.insert(control.id.clone(), props);
            }
        }
        let resolution = Resolution {
            active: order.iter().map(|c| c.component.id.clone()).collect(),
            controls: by_input,
            look,
            armed: candidates
                .iter()
                .filter(|c| !c.active && c.reads_input)
                .map(|c| c.component.id.clone())
                .collect(),
        };

        // What changed, as state.
        let slug_of = |id: &ComponentId| -> Value {
            components
                .get(id)
                .map_or(Value::Nil, |l| l.slug.as_str().into())
        };
        let mut ops = vec![(
            StatePath::new(STATE_ROOT, vec![key(&slug), key("shown")]),
            Value::Array(resolution.active.iter().map(slug_of).collect()),
        )];
        for control in &controls {
            ops.push((
                StatePath::new(
                    STATE_ROOT,
                    vec![
                        key(&slug),
                        key("controls"),
                        key(&control.id),
                        key("component"),
                    ],
                ),
                resolution
                    .controls
                    .get(&control.id)
                    .map_or(Value::Nil, slug_of),
            ));
            let props = resolution.look.get(&control.id);
            ops.push((
                StatePath::new(
                    STATE_ROOT,
                    vec![key(&slug), key("controls"), key(&control.id), key("look")],
                ),
                Value::Map(
                    props
                        .into_iter()
                        .flatten()
                        .map(|(prop, component)| (prop.as_str().into(), slug_of(component)))
                        .collect(),
                ),
            ));
        }
        let (ops, cause) = state.read(|tree| {
            let ops: Vec<(StatePath, Value)> = ops
                .into_iter()
                .filter(|(path, value)| tree.value(path).as_ref() != Some(value))
                .collect();
            let cause = target
                .reads
                .iter()
                .flat_map(|path| tree.leaves_under(path))
                .map(|(_, leaf)| leaf.meta.seq)
                .max();
            (ops, cause)
        });
        if !ops.is_empty() {
            self.write(state, surface, ops, cause, dirty);
        }
        inner.activations = activations;
        lock(&self.published).insert(surface.to_owned(), Arc::new(resolution));
        lock(&self.activated).extend(newly.into_iter().map(|l| (surface.to_owned(), l)));
    }

    fn write(
        &self,
        state: &LiveState,
        surface: &str,
        ops: Vec<(StatePath, Value)>,
        cause: Option<crate::journal::Seq>,
        dirty: &mut Dirty,
    ) {
        let record = Record::StatePatch(StatePatchRecord {
            ops: ops
                .iter()
                .map(|(path, value)| JournalOp::Set {
                    path: path.clone(),
                    value: value.clone(),
                    reducer: None,
                })
                .collect(),
            replaces: None,
            device_ts: None,
        });
        let origin = Origin::Core {
            core: CoreOrigin::Dataflow,
        };
        let entry = match self
            .journal
            .append(NewEntry::new(origin, record).cause(cause))
        {
            Ok(entry) => entry,
            Err(e) => {
                tracing::error!(%surface, "cannot journal component resolution: {e}");
                return;
            }
        };
        let meta = Meta {
            ts: crate::clock::epoch_ms(),
            seq: entry.seq,
            quality: Quality::Good,
            source: Source::Core,
        };
        let changed = state.write_in_turn(|tree| {
            let mut changed = Dirty::new();
            for (path, value) in &ops {
                changed.extend(tree.set(path, value, &meta));
            }
            changed
        });
        dirty.extend(changed);
    }
}

/// Who has a control's input, and where each of its properties comes from
/// (surfaces spec §5.2.1), given the active components highest first.
///
/// Going down: a property comes from the first component that binds it or is a
/// `replace` component, which leaves what it doesn't bind blank. Input is one
/// more property: it goes to the first component whose binding claims or blocks
/// it, or the first `replace` component. A binding's input defaults to `claim`
/// on a `replace` component and `pass` on a `through` one.
pub fn cascade(
    control: &str,
    ordered: &[&Component],
    bound: &BoundMap,
) -> (Option<ComponentId>, BTreeMap<Prop, ComponentId>) {
    let mut input = None;
    let mut props = BTreeMap::new();
    let mut open: BTreeSet<Prop> = Prop::ALL.into_iter().collect();
    for component in ordered.iter().filter(|l| l.control(control).is_some()) {
        let binding = bound.get(&(component.id.clone(), control.to_owned()));
        let replace = component.shows().cascade == Cascade::Replace;
        for prop in binding.map(|b| &b.props).into_iter().flatten() {
            if open.remove(prop) {
                props.insert(*prop, component.id.clone());
            }
        }
        let mode = binding.and_then(|b| b.input).unwrap_or(if replace {
            InputMode::Claim
        } else {
            InputMode::Pass
        });
        if input.is_none() && (mode != InputMode::Pass || replace) {
            input = Some(component.id.clone());
        }
        if replace {
            // Nothing under a replace component shows.
            break;
        }
    }
    (input, props)
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

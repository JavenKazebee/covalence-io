//! Layer resolution (surfaces spec §5.2): which layer controls each control
//! of each attached surface.
//!
//! For each surface, a layer is a candidate where it fits (§5.4) and its
//! `on` picks the surface, and active while its `when` holds. The active
//! layers are ordered by `level` and `rank`, then by how recently each
//! became active, then oldest first by ID, and each control goes to the
//! highest active layer that covers it.
//!
//! **Where it runs.** The [`Resolver`] is the state tree's resolver
//! ([`LiveState::set_resolver`]): it runs in every turn, after cells have
//! settled (dataflow §5.3), and re-resolves a surface when the turn's
//! changes touch what its layers' fields read, or when layers, the surface
//! or its controls changed. Its results are state, written in the same
//! turn, so bindings, triggers and `why` see them with the change that
//! caused them:
//! - `surface.<slug>.layers`: the active layers' slugs, highest first.
//! - `surface.<slug>.controls.<id>.layer`: the controlling layer's slug, or
//!   `null`.
//!
//! Each is journaled as a `state.patch` with origin `{core: "dataflow"}` and
//! `cause` → the latest change among what the surface's layer fields read.
//! The binding worker and the surface's input take the same results from
//! [`Resolver::resolution`].
//!
//! **Armed layers** (§5.2) are candidates that aren't active but whose
//! `when` reads the surface's input state. They're listed so the binding
//! worker keeps them evaluated, and a held shift redraws without waiting
//! for its bindings.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use covalence_protocol::Value;
use serde_json::Value as Json;

use super::{ControlDef, Layer, LayerId, STATE_ROOT, SurfaceId, When, key, missing};
use crate::expr::{Limits, QuickJs, Scope};
use crate::journal::{CoreOrigin, Journal, JournalOp, NewEntry, Origin, Record, StatePatchRecord};
use crate::source::Eval;
use crate::state::{ConnId, Dirty, LiveState, Meta, Quality, Settler, Source, StatePath};

/// What resolution found for one surface.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Resolution {
    /// The active layers, highest first.
    pub active: Vec<LayerId>,
    /// The layer controlling each control, by control ID. A control no
    /// active layer covers isn't listed.
    pub controls: BTreeMap<String, LayerId>,
    /// Inactive layers whose `when` reads the surface's input state.
    pub armed: BTreeSet<LayerId>,
}

impl Resolution {
    /// The layers whose bindings should be kept evaluated for the surface:
    /// those controlling a control, and armed ones.
    pub fn evaluated(&self) -> impl Iterator<Item = &LayerId> {
        let controlling: BTreeSet<&LayerId> = self.controls.values().collect();
        controlling.into_iter().chain(self.armed.iter())
    }
}

/// Resolves layers for every attached surface, in the state turn.
pub struct Resolver {
    state: Weak<LiveState>,
    journal: Arc<Journal>,
    inner: Mutex<Inner>,
    /// The latest resolution of each attached surface.
    published: Mutex<BTreeMap<SurfaceId, Arc<Resolution>>>,
    /// Layers that became active on a surface, for whoever adds the cells
    /// they have there (surfaces spec §11.4).
    activated: Mutex<Vec<(SurfaceId, LayerId)>>,
}

struct Inner {
    layers: Arc<BTreeMap<LayerId, Layer>>,
    /// Every surface's ID: the universe of a layer's `on`.
    universe: Vec<SurfaceId>,
    aliases: Arc<BTreeMap<String, ConnId>>,
    /// Evaluates layer fields, with a binding's limit.
    evaluator: QuickJs,
    surfaces: BTreeMap<SurfaceId, Target>,
    /// Counts activations, so a layer that becomes active goes on top of
    /// its level and rank.
    activations: u64,
    /// Each layer field's last error, by layer ID and field.
    problems: BTreeMap<(LayerId, &'static str), String>,
}

/// One surface, as resolution sees it.
struct Target {
    slug: String,
    controls: Vec<ControlDef>,
    attached: bool,
    /// What its layers' fields read when it was last resolved.
    reads: BTreeSet<StatePath>,
    /// Resolved at the next turn, whatever its changes.
    due: bool,
    /// Its active layers, with when each became active (0: before anything
    /// was seen, so ties go to the oldest layer).
    active: BTreeMap<LayerId, u64>,
    /// Whether it has been resolved since it was added.
    resolved: bool,
}

impl Resolver {
    pub fn new(state: &Arc<LiveState>, journal: Arc<Journal>) -> Self {
        Resolver {
            state: Arc::downgrade(state),
            journal,
            inner: Mutex::new(Inner {
                layers: Arc::new(BTreeMap::new()),
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

    /// Replaces the layers. Every surface is resolved at the next turn.
    pub fn set_layers(&self, layers: Arc<BTreeMap<LayerId, Layer>>) {
        let mut inner = self.lock();
        inner.layers = layers;
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
    /// the next turn, keeping the order its layers became active in.
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

    /// The layer controlling a control now.
    pub fn controlling(&self, surface: &str, control: &str) -> Option<LayerId> {
        lock(&self.published)
            .get(surface)?
            .controls
            .get(control)
            .cloned()
    }

    /// The layers that became active on a surface since this was last
    /// called, once each.
    pub fn take_activated(&self) -> Vec<(SurfaceId, LayerId)> {
        std::mem::take(&mut *lock(&self.activated))
    }

    /// Each failing layer field's last error, by layer ID and field.
    pub fn problems(&self) -> BTreeMap<(LayerId, &'static str), String> {
        self.lock().problems.clone()
    }
}

impl Inner {
    fn all_due(&mut self) {
        for target in self.surfaces.values_mut() {
            target.due = true;
        }
    }

    fn note(&mut self, layer: &str, field: &'static str, error: Option<String>) {
        let at = (layer.to_owned(), field);
        match error {
            Some(message) => {
                if self.problems.get(&at) != Some(&message) {
                    tracing::warn!(%layer, field, "a layer field failed: {message}");
                    self.problems.insert(at, message);
                }
            }
            None => {
                self.problems.remove(&at);
            }
        }
    }
}

/// One candidate layer, evaluated for a surface.
struct Candidate<'a> {
    layer: &'a Layer,
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
        let layers = inner.layers.clone();
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
        for layer in layers.values() {
            if !missing(layer, &controls).is_empty() {
                continue;
            }
            if let Some(on) = &layer.on {
                let mut eval = Eval::new(&scope, &inner.evaluator);
                let picked = on.picks(surface, &inner.universe, Some(&mut eval));
                reads.extend(eval.reads);
                let error = picked.as_ref().err().map(ToString::to_string);
                problems.push((layer.id.clone(), "on", error));
                if !picked.unwrap_or(false) {
                    continue;
                }
            }
            let (active, when_reads) = match layer.when.as_ref().unwrap_or(&When::default()) {
                When::Nav(group) => {
                    let path =
                        StatePath::new(STATE_ROOT, vec![key(&slug), key("current"), key(group)]);
                    let current = state.value(&path);
                    let active = matches!(&current, Some(Value::String(s)) if s.as_str() == Some(layer.slug.as_str()));
                    (active, BTreeSet::from([path]))
                }
                When::Always(_) => (true, BTreeSet::new()),
                When::While(source) => {
                    let mut eval = Eval::new(&scope, &inner.evaluator);
                    let value = eval.raw(source);
                    let value = match value {
                        Ok(Some(v)) => {
                            problems.push((layer.id.clone(), "when", None));
                            Some(v)
                        }
                        Ok(None) => source.fallback().cloned(),
                        Err(e) => {
                            problems.push((layer.id.clone(), "when", Some(e.to_string())));
                            source.fallback().cloned()
                        }
                    };
                    (crate::transform::truthy(value.as_ref()), eval.reads)
                }
            };
            let rank = match &layer.rank {
                None => 0.0,
                Some(source) => {
                    let mut eval = Eval::new(&scope, &inner.evaluator);
                    let value = eval.raw(source);
                    reads.extend(std::mem::take(&mut eval.reads));
                    let rank = match value {
                        Ok(Some(Json::Number(n))) => n.as_f64(),
                        Ok(Some(_)) => {
                            problems.push((
                                layer.id.clone(),
                                "rank",
                                Some("a rank is a number".into()),
                            ));
                            None
                        }
                        Ok(None) => None,
                        Err(e) => {
                            problems.push((layer.id.clone(), "rank", Some(e.to_string())));
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
                layer,
                active,
                order: layer.level.base() + rank,
                reads_input,
            });
        }
        for (layer, field, error) in problems {
            inner.note(&layer, field, error);
        }

        // Activation order: a layer that just became active goes on top.
        let mut activations = inner.activations;
        let target = inner.surfaces.get_mut(surface).expect("still there");
        let first = !target.resolved;
        let mut active = BTreeMap::new();
        let mut newly = Vec::new();
        for c in candidates.iter().filter(|c| c.active) {
            let at = match target.active.get(&c.layer.id) {
                Some(&at) => at,
                None => {
                    newly.push(c.layer.id.clone());
                    if first {
                        0
                    } else {
                        activations += 1;
                        activations
                    }
                }
            };
            active.insert(c.layer.id.clone(), at);
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
                .then(target.active[&b.layer.id].cmp(&target.active[&a.layer.id]))
                .then(a.layer.id.cmp(&b.layer.id))
        });
        let resolution = Resolution {
            active: order.iter().map(|c| c.layer.id.clone()).collect(),
            controls: controls
                .iter()
                .filter_map(|control| {
                    let layer = order
                        .iter()
                        .find(|c| c.layer.control(&control.id).is_some())?;
                    Some((control.id.clone(), layer.layer.id.clone()))
                })
                .collect(),
            armed: candidates
                .iter()
                .filter(|c| !c.active && c.reads_input)
                .map(|c| c.layer.id.clone())
                .collect(),
        };

        // What changed, as state.
        let slug_of = |id: &LayerId| -> Value {
            layers
                .get(id)
                .map_or(Value::Nil, |l| l.slug.as_str().into())
        };
        let mut ops = vec![(
            StatePath::new(STATE_ROOT, vec![key(&slug), key("layers")]),
            Value::Array(resolution.active.iter().map(slug_of).collect()),
        )];
        for control in &controls {
            ops.push((
                StatePath::new(
                    STATE_ROOT,
                    vec![key(&slug), key("controls"), key(&control.id), key("layer")],
                ),
                resolution
                    .controls
                    .get(&control.id)
                    .map_or(Value::Nil, slug_of),
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
                tracing::error!(%surface, "cannot journal layer resolution: {e}");
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

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

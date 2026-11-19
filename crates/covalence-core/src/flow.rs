//! Flows: the implemented subset of the flow spec (docs/specs/flows.md §8).
//!
//! These types are the source of truth for storage and the HTTP API
//! (overview spec: project entities). Field names follow the spec, so the JSON stored in the
//! `entities` table is the JSON the API sends and the YAML export will use.
//! Anything outside the subset is rejected by [`Flow::parse`], never stored
//! and ignored.

use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration as StdDuration;

use serde::{Deserialize, Serialize};

use crate::compose::ForEach;
use crate::param::ParamDef;
use crate::select::{Controls, Paths, Selector, Targets};
use crate::source::Source;
use crate::state::ConnId;
use crate::surface::LayerId;

pub mod runtime;
mod validate;

pub use runtime::Runtime;
pub use validate::{
    Code, Context, Problem, Report, SetTarget, derive_slug, derive_slug_for, is_identifier,
    parse_set_path, validate,
};

/// Entity type for flows in the store.
pub const ENTITY: &str = "flow";

pub type FlowId = String;

/// JavaScript expression source (spec §8.7).
pub type Expr = String;

/// A flow: a plain one, a template (it declares `params`), or an instance
/// of a template (composition §9), which has only its own identity,
/// `enabled`, its params' bindings and `forEach`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Wire", into = "Wire")]
pub struct Flow {
    /// A ULID, assigned by the server. Empty in a create request.
    pub id: FlowId,
    pub name: String,
    /// For state paths: `flow.<slug>.running`. Derived from `name` when empty.
    pub slug: String,
    pub enabled: bool,
    /// An instance's template. Its triggers, conditions, concurrency,
    /// inputs, outputs and steps are the template's.
    pub template: Option<FlowId>,
    /// A template's params: `params.<name>` in its steps (composition §1).
    pub params: BTreeMap<String, ParamDef>,
    /// An instance's bindings of its template's params, as `params` on the wire.
    pub bindings: BTreeMap<String, Source<serde_json::Value>>,
    /// An instance's items: one flow per item (composition §4).
    pub for_each: Option<ForEach>,
    /// Any one starts the flow.
    pub triggers: Vec<Trigger>,
    /// Expressions that must all be truthy when a trigger matches (spec §8.2).
    pub conditions: Vec<Expr>,
    /// An instance's is its template's; it has none of its own.
    pub concurrency: Concurrency,
    pub failover: Failover,
    /// Bound each time the flow starts: by a `call` step, the API, or
    /// defaults (spec §8.1). `input.<name>` in expressions.
    pub inputs: BTreeMap<String, ParamDef>,
    /// What a `stop` step hands back to a caller: a state schema node each.
    pub outputs: BTreeMap<String, serde_json::Map<String, serde_json::Value>>,
    pub body: Vec<Step>,
    /// View-only metadata (spec §6). Stored as-is; the runtime never reads it.
    pub editor: Option<serde_json::Map<String, serde_json::Value>>,
}

impl Flow {
    /// An instance of a template.
    pub fn is_instance(&self) -> bool {
        self.template.is_some()
    }

    /// A template: it declares params, and isn't an instance.
    pub fn is_template(&self) -> bool {
        self.template.is_none() && !self.params.is_empty()
    }

    /// An abstract template: a required param with no default, so it only
    /// runs through instances (composition §1).
    pub fn is_abstract(&self) -> bool {
        self.template.is_none() && self.params.values().any(ParamDef::needs_value)
    }

    /// What an instance runs: the template's triggers, conditions,
    /// concurrency, inputs, outputs and steps, with the instance's identity
    /// and `enabled` (composition §9).
    pub fn instance_of(&self, template: &Flow) -> Flow {
        Flow {
            id: self.id.clone(),
            name: self.name.clone(),
            slug: self.slug.clone(),
            enabled: self.enabled,
            template: self.template.clone(),
            params: template.params.clone(),
            bindings: self.bindings.clone(),
            for_each: self.for_each.clone(),
            editor: self.editor.clone(),
            ..template.clone()
        }
    }
}

/// A flow as it's stored and sent. An instance's `params` are bindings,
/// and a template's are definitions, so both go through here.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Wire {
    #[serde(default)]
    id: FlowId,
    name: String,
    #[serde(default)]
    slug: String,
    enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    template: Option<FlowId>,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    params: serde_json::Map<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    for_each: Option<ForEach>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    triggers: Option<Vec<Trigger>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    conditions: Vec<Expr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    concurrency: Option<Concurrency>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    failover: Option<Failover>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    inputs: BTreeMap<String, ParamDef>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    outputs: BTreeMap<String, serde_json::Map<String, serde_json::Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    body: Option<Vec<Step>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    editor: Option<serde_json::Map<String, serde_json::Value>>,
}

/// Why a flow's shape is wrong, with where.
#[derive(Clone, Debug, PartialEq)]
pub struct ShapeError {
    pub path: covalence_protocol::state::Path,
    pub message: String,
}

impl fmt::Display for ShapeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl TryFrom<Wire> for Flow {
    type Error = ShapeError;

    fn try_from(w: Wire) -> Result<Self, ShapeError> {
        use covalence_protocol::state::PathSegment::Key;
        let at = |k: &str| vec![Key(k.to_owned())];
        let instance = w.template.is_some();
        if instance {
            let own = [
                ("triggers", w.triggers.is_some()),
                ("conditions", !w.conditions.is_empty()),
                ("concurrency", w.concurrency.is_some()),
                ("failover", w.failover.is_some()),
                ("inputs", !w.inputs.is_empty()),
                ("outputs", !w.outputs.is_empty()),
                ("body", w.body.is_some()),
            ];
            if let Some((field, _)) = own.iter().find(|(_, has)| *has) {
                return Err(ShapeError {
                    path: at(field),
                    message: format!(
                        "an instance takes its {field} from its template; it has only its identity, `enabled`, `params` and `forEach`"
                    ),
                });
            }
        } else if w.for_each.is_some() {
            return Err(ShapeError {
                path: at("forEach"),
                message: "only an instance of a template has `forEach`".into(),
            });
        }
        let mut params = BTreeMap::new();
        let mut bindings = BTreeMap::new();
        for (name, json) in w.params {
            let mut path = at("params");
            path.push(Key(name.clone()));
            let fail = |message: String| ShapeError {
                path: path.clone(),
                message,
            };
            if instance {
                bindings.insert(name, Source::from_json(json).map_err(fail)?);
            } else {
                let def: ParamDef =
                    serde_json::from_value(json).map_err(|e| fail(e.to_string()))?;
                params.insert(name, def);
            }
        }
        let concurrency = match (instance, w.concurrency) {
            (_, Some(c)) => c,
            // Never read: an instance runs with its template's.
            (true, None) => Concurrency::Parallel,
            (false, None) => {
                return Err(ShapeError {
                    path: Vec::new(),
                    message: "missing field `concurrency`".into(),
                });
            }
        };
        Ok(Flow {
            id: w.id,
            name: w.name,
            slug: w.slug,
            enabled: w.enabled,
            template: w.template,
            params,
            bindings,
            for_each: w.for_each,
            triggers: w.triggers.unwrap_or_default(),
            conditions: w.conditions,
            concurrency,
            failover: w.failover.unwrap_or_default(),
            inputs: w.inputs,
            outputs: w.outputs,
            body: w.body.unwrap_or_default(),
            editor: w.editor,
        })
    }
}

impl From<Flow> for Wire {
    fn from(f: Flow) -> Wire {
        let instance = f.template.is_some();
        let params = if instance {
            f.bindings
                .iter()
                .map(|(k, s)| (k.clone(), s.to_json().expect("a source encodes as JSON")))
                .collect()
        } else {
            f.params
                .iter()
                .map(|(k, d)| {
                    (
                        k.clone(),
                        serde_json::to_value(d).expect("a param encodes as JSON"),
                    )
                })
                .collect()
        };
        let own = |b: bool| (!instance).then_some(b);
        Wire {
            id: f.id,
            name: f.name,
            slug: f.slug,
            enabled: f.enabled,
            template: f.template,
            params,
            for_each: f.for_each,
            triggers: own(true).map(|_| f.triggers),
            conditions: f.conditions,
            concurrency: own(true).map(|_| f.concurrency),
            failover: own(true).map(|_| f.failover),
            inputs: f.inputs,
            outputs: f.outputs,
            body: own(true).map(|_| f.body),
            editor: f.editor,
        }
    }
}

/// What happens when a trigger fires while a run is active (spec §1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Concurrency {
    Parallel,
    Queue,
    Restart,
    Ignore,
}

/// What a standby node does with this flow's runs on failover (state spec §7.4).
/// Only `abort` is implemented.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Failover {
    #[default]
    Abort,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Trigger {
    /// `POST /flows/{id}/run`.
    Api {},
    /// A `call` step in another flow (spec §2.1): any flow may call one that has this.
    Call {},
    /// Events named `event` from the nodes `from` picks (dataflow §3.1).
    Event {
        from: EventSource,
        event: Names,
        /// Runs only when truthy, with `trigger` in scope.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<Expr>,
    },
    /// A value over state, on an edge (spec §8.2). With `match`, one value
    /// per path matching the pattern, and `when` is optional.
    State {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        when: Option<Cond>,
        #[serde(default, rename = "match", skip_serializing_if = "Option::is_none")]
        pattern: Option<String>,
        edge: Edge,
    },
}

/// Where an event trigger's events come from (dataflow §3.1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum EventSource {
    /// Any node or nodes; `{match}` for patterns.
    Path { path: Selector<Paths> },
    /// Connections, by ID, plugin or interface.
    Target { target: Selector<Targets> },
    /// The controls a layer controls, on whichever surface. Absent
    /// `controls` is every control of the layer.
    Layer {
        layer: LayerId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        controls: Option<Selector<Controls>>,
    },
    /// A surface's controls, whatever layer controls them.
    Surface {
        surface: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        controls: Option<Selector<Controls>>,
    },
}

/// One event name or several; `"*"` is any.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Names {
    One(String),
    Many(Vec<String>),
}

impl Names {
    pub fn contains(&self, name: &str) -> bool {
        let is = |n: &String| n == "*" || n == name;
        match self {
            Names::One(n) => is(n),
            Names::Many(ns) => ns.iter().any(is),
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &String> {
        match self {
            Names::One(n) => std::slice::from_ref(n).iter(),
            Names::Many(ns) => ns.iter(),
        }
    }
}

/// A condition: an expression's text, or any source (dataflow §1.1, "fields
/// that are always expressions").
#[derive(Clone, Debug, PartialEq)]
pub struct Cond(pub Source<serde_json::Value>);

impl Cond {
    pub fn expr(source: impl Into<String>) -> Self {
        Cond(Source::Expr {
            expr: source.into(),
            fallback: None,
        })
    }
}

impl Serialize for Cond {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match &self.0 {
            Source::Expr {
                expr,
                fallback: None,
            } => expr.serialize(serializer),
            other => other.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for Cond {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match serde_json::Value::deserialize(deserializer)? {
            serde_json::Value::String(expr) => Ok(Cond::expr(expr)),
            other => Source::from_json(other)
                .map(Cond)
                .map_err(serde::de::Error::custom),
        }
    }
}

/// Which change of a `state` trigger's value fires it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Edge {
    /// Falsy to truthy.
    Rising,
    /// Truthy to falsy.
    Falling,
    /// Any different value.
    Change,
}

/// A target: a connection, and nothing within it (plugin protocol spec §13.9). Which
/// device, output or surface behind the connection is the action's parameter.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnRef {
    pub conn: ConnId,
}

impl ConnRef {
    pub fn conn(conn: impl Into<ConnId>) -> Self {
        ConnRef { conn: conn.into() }
    }
}

/// What a target that follows state must be (plugin protocol spec §13.9): a connection
/// of this plugin.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetConstraint {
    pub plugin: String,
}

/// One step of a flow's body. On the wire the common fields and the
/// kind-specific ones sit side by side: `{id, kind: "wait", for: "1s"}`.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    /// Unique within the flow: `[a-z][A-Za-z0-9_]*`.
    pub id: String,
    pub name: Option<String>,
    /// `false` journals the step as skipped. Absent means enabled.
    pub enabled: Option<bool>,
    pub timeout: Option<Duration>,
    pub on_error: Option<OnError>,
    pub kind: StepKind,
}

impl Step {
    pub fn is_enabled(&self) -> bool {
        self.enabled != Some(false)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum StepKind {
    Action {
        /// A literal target, or one that follows state with a `constraint`;
        /// those run on `core` only.
        target: Source<ConnRef>,
        constraint: Option<TargetConstraint>,
        /// The action's ID in the plugin manifest, e.g. `route`.
        action: String,
        /// Each a source (dataflow §1), evaluated once when the step starts.
        params: BTreeMap<String, Source<serde_json::Value>>,
    },
    Wait {
        /// A duration, or a source giving a duration string or milliseconds.
        duration: Source<Duration>,
    },
    WaitUntil {
        until: Expr,
    },
    /// Writes `local.<name>` or `vars.<name>`; checked by validation.
    Set {
        path: String,
        value: Expr,
    },
    Branch {
        arms: Vec<Arm>,
        otherwise: Option<Vec<Step>>,
    },
    /// Starts another flow through its `call` trigger, and with `wait`
    /// (the default) waits for it and takes its outputs (spec §8.3).
    Call {
        flow: FlowRef,
        /// Each a source, evaluated once when the step starts.
        inputs: BTreeMap<String, Source<serde_json::Value>>,
        wait: Option<bool>,
    },
    /// Ends the run, `ok` unless `status` says otherwise, with outputs for a caller.
    Stop {
        status: Option<StopStatus>,
        outputs: BTreeMap<String, Source<serde_json::Value>>,
        /// A failed run's message.
        error: Option<Expr>,
    },
}

/// The flow a `call` step starts: by ID, or the one a param of kind
/// `flow` holds (composition §1.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum FlowRef {
    Id(FlowId),
    Param { param: String },
}

impl FlowRef {
    pub fn id(&self) -> Option<&str> {
        match self {
            FlowRef::Id(id) => Some(id),
            FlowRef::Param { .. } => None,
        }
    }
}

impl From<&str> for FlowRef {
    fn from(id: &str) -> Self {
        FlowRef::Id(id.to_owned())
    }
}

/// How a `stop` step ends its run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StopStatus {
    #[default]
    Ok,
    Failed,
}

impl StepKind {
    /// The steps nested in this one, in order.
    pub fn children(&self) -> impl Iterator<Item = &Step> {
        let (arms, otherwise): (&[Arm], &[Step]) = match self {
            StepKind::Branch { arms, otherwise } => (arms, otherwise.as_deref().unwrap_or(&[])),
            _ => (&[], &[]),
        };
        arms.iter().flat_map(|a| a.steps.iter()).chain(otherwise)
    }
}

/// One arm of a `branch`: its steps run when `when` is the first truthy one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Arm {
    pub when: Expr,
    pub steps: Vec<Step>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnError {
    /// The error moves up and fails the run.
    #[default]
    Fail,
    /// The error is recorded on the step and the run moves on.
    Continue,
}

// The wire form of a step. serde can't combine `flatten` with
// `deny_unknown_fields`, so the common fields are repeated per kind.
#[derive(Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum WireStep {
    Action {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        enabled: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout: Option<Duration>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on_error: Option<OnError>,
        target: Source<ConnRef>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        constraint: Option<TargetConstraint>,
        action: String,
        #[serde(default)]
        params: BTreeMap<String, Source<serde_json::Value>>,
    },
    Wait {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        enabled: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout: Option<Duration>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on_error: Option<OnError>,
        #[serde(rename = "for")]
        duration: Source<Duration>,
    },
    WaitUntil {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        enabled: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout: Option<Duration>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on_error: Option<OnError>,
        until: Expr,
    },
    Set {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        enabled: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout: Option<Duration>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on_error: Option<OnError>,
        path: String,
        value: Expr,
    },
    Branch {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        enabled: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout: Option<Duration>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on_error: Option<OnError>,
        arms: Vec<Arm>,
        #[serde(default, rename = "else", skip_serializing_if = "Option::is_none")]
        otherwise: Option<Vec<Step>>,
    },
    Call {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        enabled: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout: Option<Duration>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on_error: Option<OnError>,
        flow: FlowRef,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        inputs: BTreeMap<String, Source<serde_json::Value>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wait: Option<bool>,
    },
    Stop {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        enabled: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout: Option<Duration>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on_error: Option<OnError>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<StopStatus>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        outputs: BTreeMap<String, Source<serde_json::Value>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<Expr>,
    },
}

/// Moves a wire step's common fields into a [`Step`] around `kind`, or back.
macro_rules! wire_variants {
    ($($variant:ident { $($field:ident),* }),* $(,)?) => {
        impl From<WireStep> for Step {
            fn from(wire: WireStep) -> Self {
                match wire {
                    $(WireStep::$variant { id, name, enabled, timeout, on_error, $($field),* } => Step {
                        id,
                        name,
                        enabled,
                        timeout,
                        on_error,
                        kind: StepKind::$variant { $($field),* },
                    },)*
                }
            }
        }

        impl From<Step> for WireStep {
            fn from(step: Step) -> Self {
                let Step { id, name, enabled, timeout, on_error, kind } = step;
                match kind {
                    $(StepKind::$variant { $($field),* } => WireStep::$variant {
                        id,
                        name,
                        enabled,
                        timeout,
                        on_error,
                        $($field),*
                    },)*
                }
            }
        }
    };
}

wire_variants! {
    Action { target, constraint, action, params },
    Wait { duration },
    WaitUntil { until },
    Set { path, value },
    Branch { arms, otherwise },
    Call { flow, inputs, wait },
    Stop { status, outputs, error },
}

impl Serialize for Step {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        WireStep::from(self.clone()).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Step {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        WireStep::deserialize(deserializer).map(Step::from)
    }
}

/// A positive duration written as `500ms`, `2s` or `1m30s`: hours, minutes,
/// seconds and milliseconds, each at most once and in that order. Keeps the
/// text it was written as, so a flow round-trips unchanged.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Duration {
    value: StdDuration,
    text: String,
}

impl Duration {
    pub fn get(&self) -> StdDuration {
        self.value
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }
}

impl fmt::Debug for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Duration({:?})", self.text)
    }
}

impl std::str::FromStr for Duration {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let bad = || format!("{text:?} is not a duration: write it like 500ms, 2s or 1m30s");
        // Units in the order they must appear.
        const UNITS: [(&str, u64); 4] = [("h", 3_600_000), ("m", 60_000), ("s", 1_000), ("ms", 1)];
        let mut rest = text;
        let mut next_unit = 0;
        let mut millis: u64 = 0;
        while !rest.is_empty() {
            let digits = rest.find(|c: char| !c.is_ascii_digit()).ok_or_else(bad)?;
            if digits == 0 {
                return Err(bad());
            }
            let n: u64 = rest[..digits].parse().map_err(|_| bad())?;
            rest = &rest[digits..];
            let letters = rest
                .find(|c: char| c.is_ascii_digit())
                .unwrap_or(rest.len());
            let unit = &rest[..letters];
            rest = &rest[letters..];
            let index = UNITS[next_unit..]
                .iter()
                .position(|(u, _)| *u == unit)
                .ok_or_else(bad)?
                + next_unit;
            millis = n
                .checked_mul(UNITS[index].1)
                .and_then(|ms| millis.checked_add(ms))
                .ok_or_else(bad)?;
            next_unit = index + 1;
        }
        if millis == 0 {
            return Err(format!("{text:?} must be longer than zero"));
        }
        Ok(Duration {
            value: StdDuration::from_millis(millis),
            text: text.to_owned(),
        })
    }
}

impl TryFrom<String> for Duration {
    type Error = String;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        text.parse()
    }
}

impl From<Duration> for String {
    fn from(duration: Duration) -> Self {
        duration.text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_durations() {
        for (text, ms) in [
            ("500ms", 500),
            ("2s", 2_000),
            ("1m30s", 90_000),
            ("1h", 3_600_000),
            ("1h2m3s4ms", 3_723_004),
            ("90s", 90_000),
        ] {
            let d: Duration = text.parse().unwrap();
            assert_eq!(d.get(), StdDuration::from_millis(ms), "{text}");
            assert_eq!(d.as_str(), text);
        }
        for bad in [
            "",
            "0s",
            "0ms",
            "1",
            "s",
            "1x",
            "1s1m",
            "1s1s",
            "1.5s",
            "-1s",
            " 1s",
            "1 s",
            "vars.delay",
        ] {
            assert!(bad.parse::<Duration>().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn steps_keep_common_and_kind_fields_side_by_side() {
        let json = serde_json::json!({
            "id": "settle",
            "kind": "wait",
            "for": "1s",
            "timeout": "2s",
            "onError": "continue",
        });
        let step: Step = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(step.on_error, Some(OnError::Continue));
        assert!(
            matches!(&step.kind, StepKind::Wait { duration: Source::Literal(d) } if d.as_str() == "1s")
        );
        assert_eq!(serde_json::to_value(&step).unwrap(), json);
    }
}

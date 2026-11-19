//! Parsing and validating flows (flow spec §8.4). Errors block a save;
//! warnings don't.

use std::collections::{BTreeMap, BTreeSet};

use covalence_protocol::plugin::Manifest;
use covalence_protocol::state::{Path, PathSegment};
use serde::Serialize;
use serde_json::Value;

use super::{Cond, EventSource, Flow, Names, Step, StepKind, Trigger};
use crate::compose;
use crate::expr::{At, Evaluator};
use crate::journal::ControlEvent;
use crate::param::ParamDef;
use crate::select::{Controls, Issue, Pattern, Selector, Targets};
use crate::source::{Part, Ref, ScopeName, Source};
use crate::state::ConnId;
use crate::surface::{self, Layer, LayerId};
use crate::{core_conn, schema};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Code {
    // Errors.
    /// The JSON doesn't have the shape of a flow.
    Invalid,
    /// Part of the flow spec that isn't implemented yet.
    Unsupported,
    BadId,
    DuplicateId,
    BadSlug,
    DuplicateSlug,
    UnknownConnection,
    UnknownAction,
    /// A `core` target whose `key` isn't a surface or flow, or has none.
    UnknownTarget,
    UnknownEvent,
    BadParams,
    /// An expression that doesn't compile (spec §8.7).
    ExprSyntax,
    /// A `set` path that isn't `local.<name>` or `vars.<name>`.
    BadPath,
    /// A `branch` without arms.
    NoArms,
    /// A binding on a layer that doesn't exist.
    UnknownLayer,
    /// An event trigger, or a surface's start, naming a component that
    /// doesn't exist (surfaces spec §7).
    UnknownComponent,
    /// A `control` trigger or binding on a control its layer doesn't list.
    UnknownControl,
    /// A second binding for a control on the same layer, or a layer that
    /// lists a control twice.
    DuplicateControl,
    /// A binding on a control whose layer doesn't give it the output the
    /// binding drives.
    NoOutput,
    /// A binding value that doesn't fit its property: a colour that isn't one.
    BadValue,
    /// A `set` step or sink on a computed variable (dataflow §6).
    ReadOnly,
    /// Computed variables whose reads form a cycle (dataflow §5.3).
    Cycle,
    /// A layer field that reads layer resolution's results (surfaces spec §5.2).
    ResolutionCycle,
    /// A layer's `on` naming a surface that doesn't exist.
    UnknownSurface,
    /// A `call` step on a flow that doesn't exist.
    UnknownFlow,
    /// A `call` step on a flow with no `call` trigger.
    NotCallable,
    /// An input the called flow doesn't declare.
    UnknownInput,
    /// A required input with no default that nothing gives: a `call` step
    /// that leaves it out, or a trigger that can't give it.
    MissingInput,
    /// A `stop` output the flow doesn't declare.
    UnknownOutput,
    /// An instance of a template that doesn't exist (composition §5).
    UnknownTemplate,
    /// An instance whose template is itself an instance.
    TemplateIsInstance,
    /// An instance whose params don't fit its template: one required and
    /// unbound, or a binding of the wrong type. Saving a template lists the
    /// instances it would break.
    BadInstance,
    /// A binding or `{scope: "params"}` ref for a param that isn't declared.
    UnknownParam,
    /// An abstract template used directly: a required param with no default.
    UnboundParam,
    /// A component or flow param bound to anything but an ID (composition §1.2).
    TemplateNotLiteral,
    /// A template that reaches itself through its params (composition §3).
    RecursiveTemplate,
    /// Instances nested more than 16 deep.
    TooDeep,
    /// A `{scope}` ref to a name that isn't in scope there (dataflow §8).
    NotInScope,
    // Warnings.
    NoTriggers,
    /// An event trigger on a connection the flow also acts on (spec §8.6).
    SelfTrigger,
    /// The connection's plugin hasn't loaded, so its actions and events weren't checked.
    Unchecked,
    /// A `waitUntil` that may wait forever.
    NoTimeout,
    /// A `set` to a variable no `variable` entity declares (dataflow §6).
    UndeclaredVariable,
    /// Two layers that can be active at once, at the same level and rank,
    /// on the same control: recency decides (surfaces spec §5.2).
    AmbiguousOrder,
}

/// One problem, with its place in the flow as a segment array:
/// `["body", 1, "params", "input"]`. `at` is a position within an
/// expression's source.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Problem {
    pub path: Path,
    pub code: Code,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<At>,
}

impl Problem {
    pub(crate) fn new(path: Path, code: Code, message: impl Into<String>) -> Self {
        Problem {
            path,
            code,
            message: message.into(),
            at: None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Report {
    pub errors: Vec<Problem>,
    pub warnings: Vec<Problem>,
}

impl Report {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// What validation checks a flow against.
pub struct Context<'a> {
    /// Every connection, with its plugin's manifest once that has loaded.
    pub conns: &'a BTreeMap<ConnId, Option<&'a Manifest>>,
    /// Slugs taken by other flows.
    pub taken_slugs: &'a BTreeSet<String>,
    /// Every layer, by ID.
    pub layers: &'a BTreeMap<LayerId, Layer>,
    /// Every surface's ID.
    pub surfaces: &'a BTreeSet<String>,
    /// Every flow by ID, this one included as it's being saved: `call`
    /// steps are checked against the called flow's triggers and inputs.
    pub flows: &'a BTreeMap<String, Flow>,
    /// Compiles expressions.
    pub evaluator: &'a dyn Evaluator,
    /// Declared variables by slug, and whether each is computed.
    pub variables: &'a BTreeMap<String, bool>,
}

impl Context<'_> {
    /// A flow as it runs: an instance with its template's steps.
    pub fn effective(&self, id: &str) -> Option<Flow> {
        let flow = self.flows.get(id)?;
        match &flow.template {
            Some(t) => self.flows.get(t).map(|t| flow.instance_of(t)),
            None => Some(flow.clone()),
        }
    }
}

/// Calls `f` on every step, nested ones included.
fn each_step<'s>(steps: &'s [Step], f: &mut dyn FnMut(&'s Step)) {
    for step in steps {
        f(step);
        let children: Vec<&Step> = step.kind.children().collect();
        for child in children {
            each_step(std::slice::from_ref(child), f);
        }
    }
}

/// Why a `call` step giving `given` inputs can't call `callee`, if it can't.
fn call_problem(callee: &Flow, given: &BTreeSet<String>) -> Option<String> {
    if !callee
        .triggers
        .iter()
        .any(|t| matches!(t, Trigger::Call {}))
    {
        return Some("it has no `call` trigger".into());
    }
    if let Some(name) = given.iter().find(|n| !callee.inputs.contains_key(*n)) {
        return Some(format!("it has no input {name:?}"));
    }
    callee
        .inputs
        .iter()
        .find(|(n, d)| d.needs_value() && !given.contains(*n))
        .map(|(n, _)| format!("it needs input {n:?}"))
}

fn key(k: &str) -> PathSegment {
    PathSegment::Key(k.to_owned())
}

fn index(i: usize) -> PathSegment {
    PathSegment::Index(i as i64)
}

/// Builds a path from segments: `path![key("body"), index(1)]`.
macro_rules! path {
    ($($seg:expr),* $(,)?) => { vec![$($seg),*] };
}

/// The connections a target selector names by ID.
fn target_ids(target: &Selector<Targets>) -> Vec<ConnId> {
    let mut out = Vec::new();
    target.each_id(&mut Vec::new(), &mut |_, t| out.push(t.conn.clone()));
    out
}

/// What a source walk found to check.
enum Found {
    Expr(String),
    Conn(ConnId),
    Bad(String),
    Unsupported(String),
    Scope(ScopeName, Option<String>),
}

/// `base` followed by `more`.
fn join(base: &[PathSegment], more: &[PathSegment]) -> Path {
    base.iter().chain(more).cloned().collect()
}

impl Flow {
    /// Reads a flow from JSON. Parts of the spec that aren't implemented yet
    /// are reported as `unsupported`, with their place in the flow; any other
    /// shape problem is one `invalid` problem.
    pub fn parse(json: &Value) -> Result<Flow, Vec<Problem>> {
        let unsupported = find_unsupported(json);
        if !unsupported.is_empty() {
            return Err(unsupported);
        }
        let wire: super::Wire = serde_path_to_error::deserialize(json).map_err(|e| {
            let path = e
                .path()
                .iter()
                .filter_map(|seg| match seg {
                    serde_path_to_error::Segment::Seq { index: i } => Some(index(*i)),
                    serde_path_to_error::Segment::Map { key: k } => Some(key(k)),
                    _ => None,
                })
                .collect();
            vec![Problem::new(
                path,
                Code::Invalid,
                e.into_inner().to_string(),
            )]
        })?;
        Flow::try_from(wire).map_err(|e| vec![Problem::new(e.path, Code::Invalid, e.message)])
    }
}

const LATER_TRIGGERS: &[&str] = &["schedule", "osc"];
const LATER_STEPS: &[&str] = &[
    "script", "log", "break", "continue", "group", "loop", "parallel",
];

/// Spec features that a later milestone adds, so the message can say "not
/// yet" instead of "unknown field".
fn find_unsupported(json: &Value) -> Vec<Problem> {
    let mut out = Vec::new();
    let Some(flow) = json.as_object() else {
        return out;
    };
    let mut not_yet = |path: Path, what: &str| {
        out.push(Problem::new(
            path,
            Code::Unsupported,
            format!("{what} aren't supported yet"),
        ))
    };
    if let Some(policy @ ("resume" | "restart")) = flow.get("failover").and_then(Value::as_str) {
        not_yet(
            path![key("failover")],
            &format!("{policy:?} failover policies"),
        );
    }
    for (i, trigger) in array(flow.get("triggers")) {
        let at = |k: &str| path![key("triggers"), index(i), key(k)];
        match trigger.get("kind").and_then(Value::as_str) {
            Some(kind) if LATER_TRIGGERS.contains(&kind) => {
                not_yet(at("kind"), &format!("{kind:?} triggers"))
            }
            Some("event") => {
                if trigger.get("echo").is_some() {
                    not_yet(at("echo"), "echoes on event triggers");
                }
                let from = trigger.get("from");
                if from.is_some_and(|f| f.get("group").is_some()) {
                    not_yet(at("from"), "control group event sources");
                } else if from.is_some_and(|f| {
                    f.get("slot").is_some()
                        || (f.get("controls").is_some()
                            && f.get("component").is_none()
                            && f.get("surface").is_none())
                }) {
                    not_yet(at("from"), "component event sources");
                }
            }
            _ => {}
        }
    }
    unsupported_steps(flow.get("body"), &[key("body")], &mut not_yet);
    out
}

fn unsupported_steps(
    steps: Option<&Value>,
    base: &[PathSegment],
    not_yet: &mut dyn FnMut(Path, &str),
) {
    for (i, step) in array(steps) {
        let here = join(base, &[index(i)]);
        let at = |k: &str| join(&here, &[key(k)]);
        if step.get("retry").is_some() {
            not_yet(at("retry"), "retries");
        }
        match step.get("kind").and_then(Value::as_str) {
            Some(kind) if LATER_STEPS.contains(&kind) => {
                not_yet(at("kind"), &format!("{kind:?} steps"))
            }
            Some("branch") => {
                for (a, arm) in array(step.get("arms")) {
                    let arm_at = join(&here, &[key("arms"), index(a), key("steps")]);
                    unsupported_steps(arm.get("steps"), &arm_at, not_yet);
                }
                unsupported_steps(step.get("else"), &at("else"), not_yet);
            }
            _ => {}
        }
    }
}

fn array(value: Option<&Value>) -> impl Iterator<Item = (usize, &Value)> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
}

/// `[a-z][A-Za-z0-9_]*`, for step IDs and slugs.
pub fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// A `set` path: `local.<name>` or `vars.<name>`, with one segment
/// `[A-Za-z_][A-Za-z0-9_]*` (spec §8.3).
pub fn parse_set_path(path: &str) -> Option<(SetTarget, &str)> {
    let (root, name) = path.split_once('.')?;
    let target = match root {
        "local" => SetTarget::Local,
        "vars" => SetTarget::Vars,
        _ => return None,
    };
    let mut chars = name.chars();
    let ok = matches!(chars.next(), Some(c) if c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric());
    ok.then_some((target, name))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetTarget {
    Local,
    Vars,
}

/// Checks a parsed flow against the connections and manifests it refers to.
pub fn validate(flow: &Flow, ctx: &Context) -> Report {
    let mut v = Validation {
        ctx,
        report: Report::default(),
        seen: BTreeSet::new(),
        acted_on: BTreeSet::new(),
        outputs: &flow.outputs,
        params: &flow.params,
        placing: false,
    };

    if flow.name.trim().is_empty() {
        v.error(path![key("name")], Code::Invalid, "name is required".into());
    }
    if !is_identifier(&flow.slug) {
        v.error(
            path![key("slug")],
            Code::BadSlug,
            format!(
                "slug {:?} must start with a lowercase letter and use only letters, digits and _",
                flow.slug
            ),
        );
    } else if ctx.taken_slugs.contains(&flow.slug) {
        v.error(
            path![key("slug")],
            Code::DuplicateSlug,
            format!("another flow already has slug {:?}", flow.slug),
        );
    }

    if flow.is_instance() {
        v.instance(flow);
        return v.report;
    }
    v.template(flow);

    for (i, trigger) in flow.triggers.iter().enumerate() {
        let at = |k: &str| path![key("triggers"), index(i), key(k)];
        match trigger {
            Trigger::Api {} | Trigger::Call {} => {}
            Trigger::Event {
                from,
                event: names,
                filter,
            } => {
                v.event_source(from, names, &at("from"), &at("event"));
                if let Some(filter) = filter {
                    v.expr(at("filter"), filter);
                }
            }
            Trigger::State { when, pattern, .. } => {
                if let Some(text) = pattern
                    && let Err(e) = Pattern::parse(text)
                {
                    v.error(at("match"), Code::BadPath, e);
                }
                match when {
                    // A string is an expression, reported at `when` itself.
                    Some(Cond(Source::Expr {
                        expr,
                        fallback: None,
                    })) => v.expr(at("when"), expr),
                    Some(Cond(source)) => v.source(at("when"), source),
                    None if pattern.is_none() => v.error(
                        at("when"),
                        Code::Invalid,
                        "a state trigger needs `when`, or `match` with a path pattern".into(),
                    ),
                    None => {}
                }
            }
        }
    }
    for (i, condition) in flow.conditions.iter().enumerate() {
        v.expr(path![key("conditions"), index(i)], condition);
    }

    // Inputs and outputs (spec §8.1).
    for (name, def) in &flow.inputs {
        let at = path![key("inputs"), key(name)];
        if !is_identifier(name) {
            v.error(
                at.clone(),
                Code::BadId,
                format!("input {name:?} isn't a name"),
            );
        }
        if let Some(default) = &def.default {
            let at = join(&at, &[key("default")]);
            v.source(at.clone(), default);
            v.fits(at, default, &def.schema());
        }
    }
    for name in flow.outputs.keys() {
        if !is_identifier(name) {
            v.error(
                path![key("outputs"), key(name)],
                Code::BadId,
                format!("output {name:?} isn't a name"),
            );
        }
    }
    // Only `call` and `api` give inputs; any other trigger leaves them to defaults.
    let unfilled: Vec<&String> = flow
        .inputs
        .iter()
        .filter(|(_, d)| d.needs_value())
        .map(|(n, _)| n)
        .collect();
    if let Some(name) = unfilled.first() {
        for (i, trigger) in flow.triggers.iter().enumerate() {
            if matches!(trigger, Trigger::Event { .. } | Trigger::State { .. }) {
                v.error(
                    path![key("triggers"), index(i)],
                    Code::MissingInput,
                    format!(
                        "input {name:?} is required and has no default, so only `call` and `api` triggers can start this flow"
                    ),
                );
            }
        }
    }

    v.steps(&flow.body, &[key("body")]);

    let mut report = v.report;
    let acted_on = v.acted_on;
    if flow.triggers.is_empty() {
        report.warnings.push(Problem::new(
            path![key("triggers")],
            Code::NoTriggers,
            "the flow has no triggers, so nothing can start it",
        ));
    }
    for (i, trigger) in flow.triggers.iter().enumerate() {
        if let Trigger::Event {
            from: EventSource::Target { target },
            ..
        } = trigger
            && target_ids(target).iter().any(|c| acted_on.contains(c))
        {
            report.warnings.push(Problem::new(
                path![key("triggers"), index(i)],
                Code::SelfTrigger,
                "this trigger listens to a connection the flow also acts on, so the flow may start itself; runs it starts that way are blocked",
            ));
        }
    }
    report
}

struct Validation<'a> {
    ctx: &'a Context<'a>,
    report: Report,
    /// Step IDs so far, across the whole flow.
    seen: BTreeSet<String>,
    /// Connections the flow's action steps act on.
    acted_on: BTreeSet<ConnId>,
    /// What a `stop` step may hand back.
    outputs: &'a BTreeMap<String, serde_json::Map<String, Value>>,
    /// The flow's own params, which `{scope: "params"}` refs name.
    params: &'a BTreeMap<String, ParamDef>,
    /// Checking an instance's bindings, whose `{scope}` refs
    /// `compose::check_bindings` has checked where it's placed.
    placing: bool,
}

impl<'a> Validation<'a> {
    /// An instance (composition §9): its template exists and isn't an
    /// instance, its bindings fit, and its `forEach` reads what it can.
    fn instance(&mut self, flow: &Flow) {
        let id = flow.template.as_deref().unwrap_or_default();
        let at_template = path![key("template")];
        let Some(template) = self.ctx.flows.get(id) else {
            self.error(
                at_template,
                Code::UnknownTemplate,
                format!("no flow {id:?}"),
            );
            return;
        };
        if template.is_instance() {
            self.error(
                at_template,
                Code::TemplateIsInstance,
                format!(
                    "{:?} is itself an instance; use its template, {:?}",
                    template.name,
                    template.template.as_deref().unwrap_or_default()
                ),
            );
            return;
        }
        if template.id == flow.id {
            self.error(
                at_template,
                Code::RecursiveTemplate,
                "a flow can't be an instance of itself".into(),
            );
            return;
        }
        let for_each = flow.for_each.is_some();
        self.placing = true;
        for p in compose::check_bindings(&template.params, &flow.bindings, for_each) {
            self.report.errors.push(p);
        }
        for (name, binding) in &flow.bindings {
            let at = path![key("params"), key(name)];
            self.source(at.clone(), binding);
            if let Some(def) = template.params.get(name)
                && compose::ParamKind::of(def) == compose::ParamKind::Flow
                && let Some(Value::String(bound)) = binding.literal()
            {
                self.flow_param_fits(at, template, name, bound);
            }
        }
        if let Some(fe) = &flow.for_each {
            let at = path![key("forEach")];
            match &fe.from {
                compose::ItemsFrom::Items(items) => self.source(join(&at, &[key("items")]), items),
                compose::ItemsFrom::Targets(targets) => {
                    let conns = self.ctx.conns;
                    for (rel, issue) in targets.problems(&|c| conns.contains_key(c)) {
                        let (Issue::Unknown(m) | Issue::Bad(m)) = issue;
                        let path = join(&at, &[key("targets")])
                            .into_iter()
                            .chain(rel)
                            .collect();
                        self.error(path, Code::UnknownConnection, m);
                    }
                    for (rel, e) in targets.exprs() {
                        let path = join(&at, &[key("targets")])
                            .into_iter()
                            .chain(rel)
                            .collect();
                        self.expr(path, e);
                    }
                }
            }
            if let Some(k) = &fe.key {
                self.expr(join(&at, &[key("key")]), k);
            }
        }
    }

    /// A template's params: names, defaults that fit, and saving it
    /// doesn't break an instance (composition §5).
    fn template(&mut self, flow: &Flow) {
        for (name, def) in &flow.params {
            let at = path![key("params"), key(name)];
            if !is_identifier(name) {
                self.error(
                    at.clone(),
                    Code::BadId,
                    format!("param {name:?} isn't a name"),
                );
            }
            if let Some(default) = &def.default {
                let at = join(&at, &[key("default")]);
                match compose::ParamKind::of(def) {
                    compose::ParamKind::Value => {
                        self.source(at.clone(), default);
                        self.fits(at, default, &def.schema());
                    }
                    kind => match default.literal() {
                        Some(Value::String(bound)) if kind == compose::ParamKind::Flow => {
                            self.flow_param_fits(at, flow, name, bound)
                        }
                        Some(Value::String(_)) => {}
                        _ => self.error(
                            at,
                            Code::TemplateNotLiteral,
                            format!("{name:?} holds a template: its default is the template's ID"),
                        ),
                    },
                }
            }
        }
        let instances = self
            .ctx
            .flows
            .values()
            .filter(|f| f.template.as_deref() == Some(flow.id.as_str()) && !flow.id.is_empty());
        let broken = compose::broken_instances(
            &flow.params,
            instances.map(|f| (f.name.as_str(), &f.bindings, f.for_each.is_some())),
        );
        if !broken.is_empty() {
            let list: Vec<String> = broken
                .iter()
                .map(|(name, why)| format!("{name:?} ({})", why.join("; ")))
                .collect();
            self.error(
                path![key("params")],
                Code::BadInstance,
                format!(
                    "this change would break {} instance{}: {}",
                    broken.len(),
                    if broken.len() == 1 { "" } else { "s" },
                    list.join(", ")
                ),
            );
        }
    }

    /// The flow a `flow` param is bound to can be called by every `call`
    /// step that uses the param, with the inputs it gives (composition §1.2).
    fn flow_param_fits(&mut self, at: Path, template: &Flow, param: &str, bound: &str) {
        let Some(callee) = self.ctx.effective(bound) else {
            self.error(
                at,
                Code::BadInstance,
                format!("{param:?}: no flow {bound:?}"),
            );
            return;
        };
        if callee.for_each.is_some() || callee.is_abstract() {
            let why = if callee.is_abstract() {
                "a template with a required param, which only runs through its instances"
            } else {
                "a flow per item, so a call can't name one of them"
            };
            self.error(
                at,
                Code::BadInstance,
                format!("{param:?} is bound to {:?}: {why}", callee.name),
            );
            return;
        }
        let mut calls = Vec::new();
        each_step(&template.body, &mut |step| {
            if let StepKind::Call {
                flow: super::FlowRef::Param { param: p },
                inputs,
                ..
            } = &step.kind
                && p == param
            {
                calls.push((
                    step.id.clone(),
                    inputs.keys().cloned().collect::<BTreeSet<_>>(),
                ));
            }
        });
        for (step, given) in calls {
            let why = call_problem(&callee, &given);
            if let Some(why) = why {
                self.error(
                    at.clone(),
                    Code::BadInstance,
                    format!(
                        "{param:?} is bound to {:?}, which step {step:?} can't call: {why}",
                        callee.name
                    ),
                );
            }
        }
    }

    /// A `{scope}` ref in a flow's steps: `params.<name>` of its own params.
    fn scope_ref(&mut self, at: Path, scope: ScopeName, first: Option<String>) {
        if self.placing {
            return;
        }
        match scope {
            ScopeName::Params if self.params.is_empty() => self.error(
                at,
                Code::NotInScope,
                "this flow has no params; a flow that's placed with params declares them".into(),
            ),
            ScopeName::Params => match first {
                Some(name) if !self.params.contains_key(&name) => self.error(
                    at,
                    Code::UnknownParam,
                    format!("the flow has no param {name:?}"),
                ),
                _ => {}
            },
            other => self.error(
                at,
                Code::NotInScope,
                format!("{:?} isn't in scope in a flow's steps", other.as_str()),
            ),
        }
    }

    fn error(&mut self, path: Path, code: Code, message: String) {
        self.report.errors.push(Problem::new(path, code, message));
    }

    fn warn(&mut self, path: Path, code: Code, message: &str) {
        self.report.warnings.push(Problem::new(path, code, message));
    }

    /// A `core` action's literal `surface` or `flow` parameters name ones that
    /// exist (core actions spec §2). Ones that follow state are checked when the step runs.
    fn core_items(
        &mut self,
        action: &str,
        params: &BTreeMap<String, Source<Value>>,
        at: &[PathSegment],
    ) {
        for (name, item) in core_conn::items_of(action) {
            let Some(Value::String(id)) = params.get(&name).and_then(|p| p.literal()) else {
                continue;
            };
            let known = match item {
                core_conn::Item::Surface => self.ctx.surfaces.contains(id.as_str()),
                core_conn::Item::Flow => self.ctx.flows.contains_key(id.as_str()),
            };
            if !known {
                self.error(
                    join(at, &[key("params"), key(&name)]),
                    Code::UnknownTarget,
                    format!("no {} {id:?}", item.as_str()),
                );
            }
        }
    }

    /// Checks an event trigger's source and the event names it listens for.
    fn event_source(&mut self, from: &EventSource, names: &Names, at: &Path, at_event: &Path) {
        let mut problems: Vec<(Path, Issue, Code)> = Vec::new();
        match from {
            EventSource::Path { path } => {
                let conns = self.ctx.conns;
                for (rel, issue) in path.problems(&|c| conns.contains_key(c)) {
                    let code = match issue {
                        Issue::Unknown(_) => Code::UnknownConnection,
                        Issue::Bad(_) => Code::BadPath,
                    };
                    problems.push((
                        join(at, &[key("path")]).into_iter().chain(rel).collect(),
                        issue,
                        code,
                    ));
                }
                for (rel, e) in path.exprs() {
                    let at: Path = join(at, &[key("path")]).into_iter().chain(rel).collect();
                    self.expr(at, e);
                }
            }
            EventSource::Target { target } => {
                let conns = self.ctx.conns;
                for (rel, issue) in target.problems(&|c| conns.contains_key(c)) {
                    let code = match issue {
                        Issue::Unknown(_) => Code::UnknownConnection,
                        Issue::Bad(_) => Code::Unsupported,
                    };
                    problems.push((
                        join(at, &[key("target")]).into_iter().chain(rel).collect(),
                        issue,
                        code,
                    ));
                }
                for (rel, e) in target.exprs() {
                    let at: Path = join(at, &[key("target")]).into_iter().chain(rel).collect();
                    self.expr(at, e);
                }
                // Names are checked against each connection named by ID.
                let mut named = Vec::new();
                target.each_id(&mut join(at, &[key("target")]), &mut |p, t| {
                    named.push((p.clone(), t.conn.clone()));
                });
                for (p, conn) in named {
                    if !self.ctx.conns.contains_key(&conn) {
                        continue;
                    }
                    let Some(manifest) = self.manifest_of(&conn, p) else {
                        continue;
                    };
                    for name in names.iter().filter(|n| *n != "*") {
                        if !manifest.events.iter().any(|e| &e.id == name) {
                            let message = format!("plugin {} has no event {name:?}", manifest.id);
                            self.error(at_event.clone(), Code::UnknownEvent, message);
                        }
                    }
                }
            }
            EventSource::Component {
                component,
                controls,
            } => {
                let Some(def) = self.ctx.layers.get(component) else {
                    self.error(
                        join(at, &[key("component")]),
                        Code::UnknownComponent,
                        format!("no component {component:?}"),
                    );
                    return;
                };
                let listed: Vec<&crate::surface::ControlSpec> = match controls {
                    None => def.controls.iter().collect(),
                    Some(selector) => {
                        self.controls(selector, &join(at, &[key("controls")]), &|id| {
                            def.control(id).is_some()
                        });
                        let mut ids = Vec::new();
                        selector.each_id(&mut Vec::new(), &mut |_, id| ids.push(id.clone()));
                        ids.iter().filter_map(|id| def.control(id)).collect()
                    }
                };
                self.control_events(names, &listed, at_event);
            }
            EventSource::Surface { surface, controls } => {
                if !self.ctx.surfaces.contains(surface) {
                    self.error(
                        join(at, &[key("surface")]),
                        Code::Invalid,
                        format!("no surface {surface:?}"),
                    );
                }
                if let Some(selector) = controls {
                    // A surface's controls are known once it attaches.
                    self.controls(selector, &join(at, &[key("controls")]), &|_| true);
                }
                self.control_events(names, &[], at_event);
            }
        }
        for (path, issue, code) in problems {
            let (Issue::Unknown(message) | Issue::Bad(message)) = issue;
            self.error(path, code, message);
        }
    }

    /// A control selector in an event source: IDs, lists, `expr` and the
    /// combinators. Its other forms reach event sources later.
    fn controls(&mut self, selector: &Selector<Controls>, at: &Path, known: &dyn Fn(&str) -> bool) {
        for (rel, issue) in selector.problems(known) {
            let path: Path = at.iter().cloned().chain(rel).collect();
            match issue {
                Issue::Unknown(m) => self.error(path, Code::UnknownControl, m),
                Issue::Bad(m) => self.error(path, Code::Invalid, m),
            }
        }
        let mut forms = Vec::new();
        selector.each_form(&mut at.clone(), &mut |p, _| forms.push(p.clone()));
        for path in forms {
            self.error(
                path,
                Code::Unsupported,
                "regions, neighbours and capabilities aren't supported in event sources yet".into(),
            );
        }
        for (rel, e) in selector.exprs() {
            self.expr(at.iter().cloned().chain(rel).collect(), e);
        }
    }

    /// Event names for control input: each must be a control event, and
    /// each listed control must have the input that gives it.
    fn control_events(
        &mut self,
        names: &Names,
        listed: &[&crate::surface::ControlSpec],
        at: &Path,
    ) {
        for name in names.iter().filter(|n| *n != "*") {
            let Ok(event) = serde_json::from_value::<ControlEvent>(Value::String(name.clone()))
            else {
                self.error(
                    at.clone(),
                    Code::UnknownEvent,
                    format!("{name:?} isn't a control event"),
                );
                continue;
            };
            let needs = surface::input_for(event);
            for control in listed {
                if !control.inputs.contains(&needs) {
                    self.error(
                        at.clone(),
                        Code::Unsupported,
                        format!(
                            "{} gives no {name:?} events: that needs a {} input, and the layer doesn't list one for it",
                            control.id,
                            needs.as_str()
                        ),
                    );
                }
            }
        }
    }

    /// Checks the expressions, paths and transforms in a source.
    fn source<T>(&mut self, path: Path, source: &Source<T>) {
        let mut found = Vec::new();
        source.walk(&mut path.clone(), &mut |at, part| match part {
            Part::Expr(e) => found.push((at.clone(), Found::Expr(e.to_owned()))),
            Part::Ref(Ref::Conn { conn, .. }) => {
                found.push((at.clone(), Found::Conn(conn.clone())))
            }
            Part::Ref(Ref::Root { .. }) => {}
            Part::Ref(Ref::Scope { scope, path }) => {
                let first = match path.first() {
                    Some(PathSegment::Key(k)) => Some(k.clone()),
                    _ => None,
                };
                found.push((at.clone(), Found::Scope(*scope, first)))
            }
            Part::Transform(t) => {
                if t.is_reducer() {
                    found.push((
                        join(at, &[key("use")]),
                        Found::Unsupported(format!(
                            "a {} keeps a value of its own, so it needs a home: \
                             a variable's value or a helper",
                            t.id()
                        )),
                    ));
                }
                for (rel, message) in t.problems() {
                    found.push((join(at, &rel), Found::Bad(message)));
                }
            }
        });
        for (at, f) in found {
            match f {
                Found::Expr(e) => self.expr(at, &e),
                Found::Conn(conn) if !self.ctx.conns.contains_key(&conn) => self.error(
                    join(&at, &[key("conn")]),
                    Code::UnknownConnection,
                    format!("no connection {conn:?}"),
                ),
                Found::Conn(_) => {}
                Found::Bad(message) => self.error(at, Code::BadValue, message),
                Found::Unsupported(message) => self.error(at, Code::Unsupported, message),
                Found::Scope(name, first) => self.scope_ref(at, name, first),
            }
        }
    }

    fn expr(&mut self, path: Path, source: &str) {
        if let Err(e) = self.ctx.evaluator.check(source) {
            self.report.errors.push(Problem {
                path,
                code: Code::ExprSyntax,
                message: e.message,
                at: e.at,
            });
        }
    }

    /// The manifest of a connection, or None after reporting why it can't be checked.
    fn manifest_of(&mut self, conn: &ConnId, at: Path) -> Option<&'a Manifest> {
        let conns: &'a BTreeMap<ConnId, Option<&'a Manifest>> = self.ctx.conns;
        match conns.get(conn) {
            None => {
                self.error(
                    join(&at, &[key("conn")]),
                    Code::UnknownConnection,
                    format!("no connection {conn:?}"),
                );
                None
            }
            Some(None) => {
                self.warn(
                    at,
                    Code::Unchecked,
                    &format!("connection {conn:?}'s plugin hasn't loaded, so this wasn't checked"),
                );
                None
            }
            Some(Some(manifest)) => Some(*manifest),
        }
    }

    /// The manifest of a constraint's plugin, from any connection of it, or
    /// None after reporting why it can't be checked.
    fn manifest_of_plugin(&mut self, plugin: &str, at: Path) -> Option<&'a Manifest> {
        if plugin == core_conn::CORE {
            return Some(core_conn::manifest());
        }
        let conns: &'a BTreeMap<ConnId, Option<&'a Manifest>> = self.ctx.conns;
        if let Some(manifest) = conns.values().flatten().find(|m| m.id == plugin) {
            return Some(*manifest);
        }
        self.warn(
            join(&at, &[key("plugin")]),
            Code::Unchecked,
            &format!("no connection of plugin {plugin:?} has loaded, so this wasn't checked"),
        );
        None
    }

    fn steps(&mut self, steps: &[Step], base: &[PathSegment]) {
        for (i, step) in steps.iter().enumerate() {
            self.step(step, &join(base, &[index(i)]));
        }
    }

    fn step(&mut self, step: &Step, here: &[PathSegment]) {
        let at = |k: &str| join(here, &[key(k)]);
        if !is_identifier(&step.id) {
            self.error(
                at("id"),
                Code::BadId,
                format!(
                    "step ID {:?} must start with a lowercase letter and use only letters, digits and _",
                    step.id
                ),
            );
        } else if !self.seen.insert(step.id.clone()) {
            self.error(
                at("id"),
                Code::DuplicateId,
                format!("another step already has ID {:?}", step.id),
            );
        }
        match &step.kind {
            StepKind::Action {
                target,
                constraint,
                action,
                params,
            } => {
                for (name, param) in params {
                    self.source(join(here, &[key("params"), key(name)]), param);
                }
                let manifest: &Manifest = match target.literal() {
                    Some(t) if t.conn == core_conn::CORE => {
                        self.core_items(action, params, here);
                        core_conn::manifest()
                    }
                    Some(t) => {
                        self.acted_on.insert(t.conn.clone());
                        let Some(manifest) = self.manifest_of(&t.conn, at("target")) else {
                            return;
                        };
                        manifest
                    }
                    // It follows state: typed by its constraint (plugin protocol spec §13.9).
                    None => {
                        self.source(at("target"), target);
                        let Some(c) = constraint else {
                            self.error(
                                at("constraint"),
                                Code::Invalid,
                                "a target that follows state needs a constraint: {plugin}".into(),
                            );
                            return;
                        };
                        let Some(manifest) = self.manifest_of_plugin(&c.plugin, at("constraint"))
                        else {
                            return;
                        };
                        manifest
                    }
                };
                let Some(def) = manifest.actions.iter().find(|a| &a.id == action) else {
                    let message = format!("plugin {} has no action {action:?}", manifest.id);
                    self.error(at("action"), Code::UnknownAction, message);
                    return;
                };
                let schema =
                    serde_json::to_value(&def.params).expect("manifest values encode as JSON");
                // Parameters that follow state are checked when the step runs.
                let stored: serde_json::Map<String, Value> = params
                    .iter()
                    .map(|(k, v)| (k.clone(), v.to_json().expect("a source encodes as JSON")))
                    .collect();
                let dynamic = |path: &Path| {
                    matches!(path.first(), Some(PathSegment::Key(k))
                        if params.get(k).is_some_and(|p| p.literal().is_none()))
                };
                for mismatch in schema::check(&schema, &Value::Object(stored)) {
                    if dynamic(&mismatch.path) {
                        continue;
                    }
                    self.error(
                        join(&at("params"), &mismatch.path),
                        Code::BadParams,
                        mismatch.message,
                    );
                }
            }
            StepKind::Wait { duration } => self.source(at("for"), duration),
            StepKind::WaitUntil { until } => {
                self.expr(at("until"), until);
                if step.timeout.is_none() {
                    self.warn(
                        here.to_vec(),
                        Code::NoTimeout,
                        "this step has no timeout, so it may wait forever",
                    );
                }
            }
            StepKind::Set { path, value } => {
                match parse_set_path(path) {
                    None => self.error(
                        at("path"),
                        Code::BadPath,
                        format!("{path:?} must be local.<name> or vars.<name>"),
                    ),
                    Some((SetTarget::Vars, name)) => match self.ctx.variables.get(name) {
                        Some(true) => self.error(
                            at("path"),
                            Code::ReadOnly,
                            format!("vars.{name} is computed; only its own value sets it"),
                        ),
                        Some(false) => {}
                        None => self.warn(
                            at("path"),
                            Code::UndeclaredVariable,
                            &format!(
                                "no variable declares vars.{name}; declare it to give it a type"
                            ),
                        ),
                    },
                    Some((SetTarget::Local, _)) => {}
                }
                self.expr(at("value"), value);
            }
            StepKind::Branch { arms, otherwise } => {
                if arms.is_empty() {
                    self.error(
                        at("arms"),
                        Code::NoArms,
                        "a branch needs at least one arm".into(),
                    );
                }
                for (a, arm) in arms.iter().enumerate() {
                    let arm_at = join(here, &[key("arms"), index(a)]);
                    self.expr(join(&arm_at, &[key("when")]), &arm.when);
                    self.steps(&arm.steps, &join(&arm_at, &[key("steps")]));
                }
                if let Some(otherwise) = otherwise {
                    self.steps(otherwise, &at("else"));
                }
            }
            StepKind::Call { flow, inputs, .. } => {
                for (name, input) in inputs {
                    self.source(join(here, &[key("inputs"), key(name)]), input);
                }
                let flow = match flow {
                    super::FlowRef::Id(id) => id,
                    super::FlowRef::Param { param } => {
                        // The bound flow is checked where the param is bound.
                        match self.params.get(param) {
                            Some(def) if compose::ParamKind::of(def) == compose::ParamKind::Flow => {}
                            Some(_) => self.error(
                                at("flow"),
                                Code::Invalid,
                                format!("param {param:?} doesn't hold a flow: declare it with `kind: \"flow\"`"),
                            ),
                            None => self.error(
                                at("flow"),
                                Code::UnknownParam,
                                format!("the flow has no param {param:?}"),
                            ),
                        }
                        return;
                    }
                };
                let Some(callee) = self.ctx.effective(flow) else {
                    self.error(at("flow"), Code::UnknownFlow, format!("no flow {flow:?}"));
                    return;
                };
                if callee.for_each.is_some() {
                    self.error(
                        at("flow"),
                        Code::NotCallable,
                        format!(
                            "{:?} is a flow per item, so a call can't name one of them",
                            callee.name
                        ),
                    );
                    return;
                }
                if callee.is_abstract() {
                    self.error(
                        at("flow"),
                        Code::UnboundParam,
                        format!(
                            "{:?} has a required param with no default, so it only runs through its instances",
                            callee.name
                        ),
                    );
                    return;
                }
                let callee = &callee;
                if !callee
                    .triggers
                    .iter()
                    .any(|t| matches!(t, Trigger::Call {}))
                {
                    self.error(
                        at("flow"),
                        Code::NotCallable,
                        format!(
                            "flow {:?} has no `call` trigger, so it can't be called",
                            callee.name
                        ),
                    );
                }
                for (name, input) in inputs {
                    let at = join(here, &[key("inputs"), key(name)]);
                    match callee.inputs.get(name) {
                        Some(def) => self.fits(at, input, &def.schema()),
                        None => self.error(
                            at,
                            Code::UnknownInput,
                            format!("flow {:?} has no input {name:?}", callee.name),
                        ),
                    }
                }
                for (name, def) in &callee.inputs {
                    if def.needs_value() && !inputs.contains_key(name) {
                        self.error(
                            at("inputs"),
                            Code::MissingInput,
                            format!("flow {:?} needs input {name:?}", callee.name),
                        );
                    }
                }
            }
            StepKind::Stop { outputs, error, .. } => {
                for (name, output) in outputs {
                    let at = join(here, &[key("outputs"), key(name)]);
                    self.source(at.clone(), output);
                    match self.outputs.get(name) {
                        Some(schema) => self.fits(at, output, &Value::Object(schema.clone())),
                        None => self.error(
                            at,
                            Code::UnknownOutput,
                            format!("the flow has no output {name:?}"),
                        ),
                    }
                }
                if let Some(error) = error {
                    self.expr(at("error"), error);
                }
            }
        }
    }

    /// A literal value fits its schema. Sources are checked when they're evaluated.
    fn fits(&mut self, at: Path, value: &Source<Value>, schema: &Value) {
        let Some(literal) = value.literal() else {
            return;
        };
        for mismatch in schema::check(schema, literal) {
            self.error(join(&at, &mismatch.path), Code::BadValue, mismatch.message);
        }
    }
}

/// A slug from a flow's name: camelCase ASCII, starting with a lowercase
/// letter, with a number added if another flow has it.
/// `"Recall shot (cam 2)"` → `recallShotCam2`.
pub fn derive_slug(name: &str, taken: &BTreeSet<String>) -> String {
    derive_slug_for("flow", name, taken)
}

/// [`derive_slug`] for any entity: `prefix` starts a slug whose name
/// doesn't start with a letter.
pub fn derive_slug_for(prefix: &str, name: &str, taken: &BTreeSet<String>) -> String {
    let mut base = String::new();
    for word in name
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
    {
        // "ALL OFF" reads as two words, not as "aLLOFF".
        let word = if word.chars().any(|c| c.is_ascii_lowercase()) {
            word.to_owned()
        } else {
            word.to_ascii_lowercase()
        };
        let mut chars = word.chars();
        let first = chars.next().expect("words are not empty");
        base.push(if base.is_empty() {
            first.to_ascii_lowercase()
        } else {
            first.to_ascii_uppercase()
        });
        base.extend(chars);
    }
    if !base.starts_with(|c: char| c.is_ascii_lowercase()) {
        base.insert_str(0, prefix);
    }
    if !taken.contains(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}{n}"))
        .find(|s| !taken.contains(s))
        .expect("some suffix is free")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_slugs() {
        let none = BTreeSet::new();
        for (name, slug) in [
            ("Recall shot (cam 2)", "recallShotCam2"),
            ("pre-service", "preService"),
            ("2nd wash", "flow2ndWash"),
            ("ALL OFF", "allOff"),
            ("Caf\u{e9} more", "cafMore"),
            ("!!!", "flow"),
        ] {
            assert_eq!(derive_slug(name, &none), slug, "{name}");
        }
        let taken: BTreeSet<String> = ["preService".into(), "preService2".into()].into();
        assert_eq!(derive_slug("Pre service", &taken), "preService3");
        assert_eq!(derive_slug_for("page", "2nd floor", &none), "page2ndFloor");
    }

    #[test]
    fn identifiers() {
        for ok in ["a", "route1", "camLoop_2"] {
            assert!(is_identifier(ok), "{ok}");
        }
        for bad in ["", "1a", "Route", "_a", "a-b", "a.b"] {
            assert!(!is_identifier(bad), "{bad}");
        }
    }
}

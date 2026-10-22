//! Parsing and validating flows (flow spec §8.4). Errors block a save;
//! warnings don't.

use std::collections::{BTreeMap, BTreeSet};

use covalence_protocol::plugin::Manifest;
use covalence_protocol::state::{Path, PathSegment};
use serde::Serialize;
use serde_json::Value;

use super::{Flow, Step, StepKind, Trigger, WaitFor, param_expr};
use crate::expr::{At, Evaluator};
use crate::schema;
use crate::state::ConnId;
use crate::surface::{Layout, PageId};

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
    UnknownEvent,
    BadParams,
    /// An expression that doesn't compile (spec §8.7).
    ExprSyntax,
    /// A `set` path that isn't `local.<name>` or `vars.<name>`.
    BadPath,
    /// A `branch` without arms.
    NoArms,
    /// A `gesture` trigger or binding on a page that doesn't exist.
    UnknownPage,
    /// A `gesture` trigger or binding on a control outside its page's layout.
    UnknownControl,
    /// A second binding for a control on the same page.
    DuplicateControl,
    /// A binding value that doesn't fit its property: a colour that isn't one.
    BadValue,
    // Warnings.
    NoTriggers,
    /// An event trigger on a connection the flow also acts on (spec §8.6).
    SelfTrigger,
    /// The connection's plugin hasn't loaded, so its actions and events weren't checked.
    Unchecked,
    /// A `waitUntil` that may wait forever.
    NoTimeout,
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
    /// Every page's layout, by page ID.
    pub pages: &'a BTreeMap<PageId, Layout>,
    /// Compiles expressions.
    pub evaluator: &'a dyn Evaluator,
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
        serde_path_to_error::deserialize(json).map_err(|e| {
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
        })
    }
}

const LATER_TRIGGERS: &[&str] = &["schedule", "call", "osc", "startup"];
const LATER_STEPS: &[&str] = &[
    "call", "script", "log", "stop", "break", "continue", "group", "loop", "parallel",
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
    for (field, what) in [("inputs", "flow inputs"), ("outputs", "flow outputs")] {
        if flow.contains_key(field) {
            not_yet(path![key(field)], what);
        }
    }
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
            Some("gesture") if trigger.get("gesture").and_then(Value::as_str) == Some("turn") => {
                not_yet(at("gesture"), "\"turn\" gestures")
            }
            Some("event") => {
                let source = trigger.get("source");
                if source.is_some_and(|s| s.get("plugin").is_some() || s.get("interface").is_some())
                {
                    not_yet(at("source"), "plugin and interface event sources");
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
            Some("action") => {
                let target = step.get("target");
                if target.is_some_and(|t| t.get("expr").is_some()) {
                    not_yet(at("target"), "dynamic targets");
                } else if target.is_some_and(|t| t.get("key").is_some()) {
                    not_yet(at("target"), "targets narrowed with a key");
                }
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

    for (i, trigger) in flow.triggers.iter().enumerate() {
        let at = |k: &str| path![key("triggers"), index(i), key(k)];
        match trigger {
            Trigger::Api {} => {}
            Trigger::Event {
                source,
                event,
                filter,
            } => {
                if let Some(manifest) = v.manifest_of(&source.conn, at("source"))
                    && !manifest.events.iter().any(|e| &e.id == event)
                {
                    v.error(
                        at("event"),
                        Code::UnknownEvent,
                        format!("plugin {} has no event {event:?}", manifest.id),
                    );
                }
                if let Some(filter) = filter {
                    v.expr(at("filter"), filter);
                }
            }
            Trigger::State { when, .. } => v.expr(at("when"), when),
            Trigger::Gesture { control, .. } => match ctx.pages.get(&control.page) {
                None => v.error(
                    path![key("triggers"), index(i), key("control"), key("page")],
                    Code::UnknownPage,
                    format!("no page {:?}", control.page),
                ),
                Some(layout) if !layout.contains(&control.control) => v.error(
                    path![key("triggers"), index(i), key("control"), key("control")],
                    Code::UnknownControl,
                    format!(
                        "the page is {}×{}, so its controls are k0 to k{}, not {:?}",
                        layout.cols,
                        layout.rows,
                        (layout.cols * layout.rows).saturating_sub(1),
                        control.control
                    ),
                ),
                Some(_) => {}
            },
        }
    }
    for (i, condition) in flow.conditions.iter().enumerate() {
        v.expr(path![key("conditions"), index(i)], condition);
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
        if let Trigger::Event { source, .. } = trigger
            && acted_on.contains(&source.conn)
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
}

impl<'a> Validation<'a> {
    fn error(&mut self, path: Path, code: Code, message: String) {
        self.report.errors.push(Problem::new(path, code, message));
    }

    fn warn(&mut self, path: Path, code: Code, message: &str) {
        self.report.warnings.push(Problem::new(path, code, message));
    }

    /// Reports an expression that doesn't compile.
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
                action,
                params,
            } => {
                self.acted_on.insert(target.conn.clone());
                let mut literal = true;
                for (name, value) in params {
                    if let Some(e) = param_expr(value) {
                        literal = false;
                        self.expr(join(here, &[key("params"), key(name)]), e);
                    }
                }
                let Some(manifest) = self.manifest_of(&target.conn, at("target")) else {
                    return;
                };
                let Some(def) = manifest.actions.iter().find(|a| &a.id == action) else {
                    let message = format!("plugin {} has no action {action:?}", manifest.id);
                    self.error(at("action"), Code::UnknownAction, message);
                    return;
                };
                let schema =
                    serde_json::to_value(&def.params).expect("manifest values encode as JSON");
                // Expression parameters are checked when the step runs.
                let is_expr = |path: &Path| {
                    !literal
                        && matches!(path.first(), Some(PathSegment::Key(k))
                            if params.get(k).and_then(param_expr).is_some())
                };
                for mismatch in schema::check(&schema, &Value::Object(params.clone())) {
                    if is_expr(&mismatch.path) {
                        continue;
                    }
                    self.error(
                        join(&at("params"), &mismatch.path),
                        Code::BadParams,
                        mismatch.message,
                    );
                }
            }
            StepKind::Wait { duration } => {
                if let WaitFor::Expr(e) = duration {
                    self.expr(join(here, &[key("for"), key("expr")]), e);
                }
            }
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
                if parse_set_path(path).is_none() {
                    self.error(
                        at("path"),
                        Code::BadPath,
                        format!("{path:?} must be local.<name> or vars.<name>"),
                    );
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

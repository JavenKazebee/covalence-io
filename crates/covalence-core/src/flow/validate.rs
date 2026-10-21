//! Parsing and validating flows (flow spec §8.4). Errors block a save;
//! warnings don't.

use std::collections::{BTreeMap, BTreeSet};

use covalence_protocol::plugin::Manifest;
use covalence_protocol::state::{Path, PathSegment};
use serde::Serialize;
use serde_json::Value;

use super::{Flow, StepKind, Trigger};
use crate::schema;
use crate::state::ConnId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Code {
    // Errors.
    /// The JSON doesn't have the shape of a flow.
    Invalid,
    /// Part of the flow spec that M3 doesn't implement yet.
    Unsupported,
    BadId,
    DuplicateId,
    BadSlug,
    DuplicateSlug,
    UnknownConnection,
    UnknownAction,
    UnknownEvent,
    BadParams,
    // Warnings.
    NoTriggers,
    /// An event trigger on a connection the flow also acts on (spec §8.6).
    SelfTrigger,
    /// The connection's plugin hasn't loaded, so its actions and events weren't checked.
    Unchecked,
}

/// One problem, with its place in the flow as a segment array:
/// `["body", 1, "params", "input"]`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Problem {
    pub path: Path,
    pub code: Code,
    pub message: String,
}

impl Problem {
    fn new(path: Path, code: Code, message: impl Into<String>) -> Self {
        Problem {
            path,
            code,
            message: message.into(),
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

impl Flow {
    /// Reads a flow from JSON. Parts of the spec that M3 doesn't implement
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

const LATER_TRIGGERS: &[&str] = &["gesture", "state", "schedule", "call", "osc", "startup"];
const LATER_STEPS: &[&str] = &[
    "waitUntil",
    "call",
    "set",
    "script",
    "log",
    "stop",
    "break",
    "continue",
    "group",
    "branch",
    "loop",
    "parallel",
];

/// Spec features that a later milestone adds, so the message can say "not
/// yet" instead of "unknown field".
fn find_unsupported(json: &Value) -> Vec<Problem> {
    let mut out = Vec::new();
    let mut not_yet = |path: Path, what: &str| {
        out.push(Problem::new(
            path,
            Code::Unsupported,
            format!("{what} aren't supported yet"),
        ))
    };
    let Some(flow) = json.as_object() else {
        return out;
    };
    for (field, what) in [
        ("conditions", "conditions"),
        ("inputs", "flow inputs"),
        ("outputs", "flow outputs"),
    ] {
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
            Some("event") => {
                if trigger.get("filter").is_some() {
                    not_yet(at("filter"), "event filters");
                }
                let source = trigger.get("source");
                if source.is_some_and(|s| s.get("plugin").is_some() || s.get("interface").is_some())
                {
                    not_yet(at("source"), "plugin and interface event sources");
                }
            }
            _ => {}
        }
    }
    for (i, step) in array(flow.get("body")) {
        let at = |k: &str| path![key("body"), index(i), key(k)];
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
                if let Some(params) = step.get("params").and_then(Value::as_object) {
                    for (name, value) in params {
                        if is_expr(value) {
                            not_yet(
                                path![key("body"), index(i), key("params"), key(name)],
                                "expressions",
                            );
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn array(value: Option<&Value>) -> impl Iterator<Item = (usize, &Value)> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
}

/// `{expr: "..."}`: an expression where a literal parameter is expected (spec §7).
fn is_expr(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|o| o.len() == 1 && o.contains_key("expr"))
}

/// `[a-z][A-Za-z0-9_]*`, for step IDs and slugs.
fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// Checks a parsed flow against the connections and manifests it refers to.
pub fn validate(flow: &Flow, ctx: &Context) -> Report {
    let mut report = Report::default();
    let mut error = |path: Path, code: Code, message: String| {
        report.errors.push(Problem::new(path, code, message))
    };
    let mut warnings = Vec::new();

    if flow.name.trim().is_empty() {
        error(path![key("name")], Code::Invalid, "name is required".into());
    }
    if !is_identifier(&flow.slug) {
        error(
            path![key("slug")],
            Code::BadSlug,
            format!(
                "slug {:?} must start with a lowercase letter and use only letters, digits and _",
                flow.slug
            ),
        );
    } else if ctx.taken_slugs.contains(&flow.slug) {
        error(
            path![key("slug")],
            Code::DuplicateSlug,
            format!("another flow already has slug {:?}", flow.slug),
        );
    }

    // The manifest of a connection, or None after reporting why it can't be checked.
    let mut manifest_of =
        |conn: &ConnId, at: Path, error: &mut dyn FnMut(Path, Code, String)| match ctx
            .conns
            .get(conn)
        {
            None => {
                let mut at = at;
                at.push(key("conn"));
                error(
                    at,
                    Code::UnknownConnection,
                    format!("no connection {conn:?}"),
                );
                None
            }
            Some(None) => {
                warnings.push(Problem::new(
                    at,
                    Code::Unchecked,
                    format!("connection {conn:?}'s plugin hasn't loaded, so this wasn't checked"),
                ));
                None
            }
            Some(Some(manifest)) => Some(*manifest),
        };

    for (i, trigger) in flow.triggers.iter().enumerate() {
        let Trigger::Event { source, event } = trigger else {
            continue;
        };
        let at = path![key("triggers"), index(i), key("source")];
        if let Some(manifest) = manifest_of(&source.conn, at, &mut error)
            && !manifest.events.iter().any(|e| &e.id == event)
        {
            error(
                path![key("triggers"), index(i), key("event")],
                Code::UnknownEvent,
                format!("plugin {} has no event {event:?}", manifest.id),
            );
        }
    }

    let mut seen = BTreeSet::new();
    for (i, step) in flow.body.iter().enumerate() {
        let at = |k: &str| path![key("body"), index(i), key(k)];
        if !is_identifier(&step.id) {
            error(
                at("id"),
                Code::BadId,
                format!(
                    "step ID {:?} must start with a lowercase letter and use only letters, digits and _",
                    step.id
                ),
            );
        } else if !seen.insert(step.id.as_str()) {
            error(
                at("id"),
                Code::DuplicateId,
                format!("another step already has ID {:?}", step.id),
            );
        }
        let StepKind::Action {
            target,
            action,
            params,
        } = &step.kind
        else {
            continue;
        };
        let Some(manifest) = manifest_of(&target.conn, at("target"), &mut error) else {
            continue;
        };
        let Some(def) = manifest.actions.iter().find(|a| &a.id == action) else {
            error(
                at("action"),
                Code::UnknownAction,
                format!("plugin {} has no action {action:?}", manifest.id),
            );
            continue;
        };
        let schema = serde_json::to_value(&def.params).expect("manifest values encode as JSON");
        for mismatch in schema::check(&schema, &Value::Object(params.clone())) {
            let mut path = at("params");
            path.extend(mismatch.path);
            error(path, Code::BadParams, mismatch.message);
        }
    }

    if flow.triggers.is_empty() {
        warnings.push(Problem::new(
            path![key("triggers")],
            Code::NoTriggers,
            "the flow has no triggers, so nothing can start it",
        ));
    }
    let acted_on: BTreeSet<&ConnId> = flow
        .body
        .iter()
        .filter_map(|s| match &s.kind {
            StepKind::Action { target, .. } => Some(&target.conn),
            StepKind::Wait { .. } => None,
        })
        .collect();
    for (i, trigger) in flow.triggers.iter().enumerate() {
        if let Trigger::Event { source, .. } = trigger
            && acted_on.contains(&source.conn)
        {
            warnings.push(Problem::new(
                path![key("triggers"), index(i)],
                Code::SelfTrigger,
                "this trigger listens to a connection the flow also acts on, so the flow may start itself; runs it starts that way are blocked",
            ));
        }
    }

    report.warnings = warnings;
    report
}

/// A slug from a flow's name: camelCase ASCII, starting with a lowercase
/// letter, with a number added if another flow has it.
/// `"Recall shot (cam 2)"` → `recallShotCam2`.
pub fn derive_slug(name: &str, taken: &BTreeSet<String>) -> String {
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
        base.insert_str(0, "flow");
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

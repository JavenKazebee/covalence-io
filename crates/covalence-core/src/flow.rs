//! Flows: the implemented subset of the flow spec (docs/specs/flows.md §8).
//!
//! These types are the source of truth for storage and the HTTP API
//! (ADR 0020). Field names follow the spec, so the JSON stored in the
//! `entities` table is the JSON the API sends and the YAML export will use.
//! Anything outside the subset is rejected by [`Flow::parse`], never stored
//! and ignored.

use std::fmt;
use std::time::Duration as StdDuration;

use serde::{Deserialize, Serialize};

use crate::journal::Gesture;
use crate::state::ConnId;
use crate::surface::PageId;

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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Flow {
    /// A ULID, assigned by the server. Empty in a create request.
    #[serde(default)]
    pub id: FlowId,
    pub name: String,
    /// For state paths: `flow.<slug>.running`. Derived from `name` when empty.
    #[serde(default)]
    pub slug: String,
    pub enabled: bool,
    /// Any one starts the flow.
    #[serde(default)]
    pub triggers: Vec<Trigger>,
    /// Expressions that must all be truthy when a trigger matches (spec §8.2).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Expr>,
    pub concurrency: Concurrency,
    #[serde(default)]
    pub failover: Failover,
    #[serde(default)]
    pub body: Vec<Step>,
    /// View-only metadata (spec §6). Stored as-is; the runtime never reads it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editor: Option<serde_json::Map<String, serde_json::Value>>,
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

/// What a standby node does with this flow's runs on failover (ADR 0008).
/// M3 has only `abort`.
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
    /// A plugin event from one connection.
    Event {
        source: ConnRef,
        event: String,
        /// Runs only when truthy, with `trigger` in scope.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<Expr>,
    },
    /// An expression over state, on an edge (spec §8.2).
    State { when: Expr, edge: Edge },
    /// A gesture on a control of a page, on any surface showing it
    /// (surfaces spec §3.1).
    Gesture {
        control: ControlRef,
        gesture: Gesture,
    },
}

/// A control on a page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlRef {
    pub page: PageId,
    pub control: String,
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

/// A fixed target: one connection (spec §2.1 `TargetSpec`, without `key`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnRef {
    pub conn: ConnId,
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
        target: ConnRef,
        /// The action's ID in the plugin manifest, e.g. `route`.
        action: String,
        /// Literal values, or `{expr}` at the top level (spec §8.3).
        params: serde_json::Map<String, serde_json::Value>,
    },
    Wait {
        duration: WaitFor,
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

/// A `wait` duration: literal, or an expression giving a duration string or
/// milliseconds (spec §8.3).
#[derive(Clone, Debug, PartialEq)]
pub enum WaitFor {
    Literal(Duration),
    Expr(Expr),
}

impl Serialize for WaitFor {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            WaitFor::Literal(d) => d.serialize(serializer),
            WaitFor::Expr(e) => ExprValue { expr: e.clone() }.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for WaitFor {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire {
            Text(String),
            Expr(ExprValue),
        }
        match Wire::deserialize(deserializer).map_err(|_| {
            serde::de::Error::custom("expected a duration like \"2s\", or {\"expr\": ...}")
        })? {
            Wire::Text(text) => text
                .parse()
                .map(WaitFor::Literal)
                .map_err(serde::de::Error::custom),
            Wire::Expr(e) => Ok(WaitFor::Expr(e.expr)),
        }
    }
}

/// `{expr: "..."}`, where a value may be an expression.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExprValue {
    expr: Expr,
}

/// The expression in a parameter value shaped `{expr: "..."}`, if it is one.
pub fn param_expr(value: &serde_json::Value) -> Option<&str> {
    let object = value.as_object()?;
    match (object.len(), object.get("expr")) {
        (1, Some(serde_json::Value::String(e))) => Some(e),
        _ => None,
    }
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
        target: ConnRef,
        action: String,
        #[serde(default)]
        params: serde_json::Map<String, serde_json::Value>,
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
        duration: WaitFor,
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
    Action { target, action, params },
    Wait { duration },
    WaitUntil { until },
    Set { path, value },
    Branch { arms, otherwise },
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
            matches!(&step.kind, StepKind::Wait { duration: WaitFor::Literal(d) } if d.as_str() == "1s")
        );
        assert_eq!(serde_json::to_value(&step).unwrap(), json);
    }
}

//! Expressions (flow spec §8.7, ADR 0021): sandboxed JavaScript snippets that
//! compute a value from state and never change anything.
//!
//! The flow runtime talks to the [`Evaluator`] trait, so it can be tested
//! against a stub; [`QuickJs`] is the real engine. An evaluation reads state
//! lazily, through read-only proxies, and returns the paths it read, so
//! `state` triggers and `waitUntil` know when to evaluate again.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use covalence_protocol::Value;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::state::{ConnId, Dirty, LiveState, StatePath};

mod quickjs;

pub use quickjs::QuickJs;

/// The reserved roots an expression can read besides connection aliases, as
/// `(name in expressions, pseudo-connection the state is stored under)`.
pub const RESERVED_ROOTS: &[(&str, &str)] = &[
    ("vars", "vars"),
    ("sys", "sys"),
    ("flow", "flow"),
    ("surface", "surface"),
    ("layer", "layer"),
];

/// JavaScript's reserved words, which can't be names in an expression.
const KEYWORDS: &[&str] = &[
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "interface",
    "let",
    "new",
    "null",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
    "arguments",
];

/// Whether `name` can't be a connection alias, because an expression would
/// read something else by it: a JavaScript keyword, a built-in such as
/// `Math`, the standard library, `trigger`, `local`, `steps` or a reserved root.
pub fn is_taken_name(name: &str) -> bool {
    static GLOBALS: std::sync::OnceLock<BTreeSet<String>> = std::sync::OnceLock::new();
    KEYWORDS.contains(&name)
        || GLOBALS
            .get_or_init(|| quickjs::global_names().into_iter().collect())
            .contains(name)
}

/// Per-evaluation limits (ADR 0021).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub time: Duration,
    /// Bytes, for the whole engine.
    pub memory: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            time: Duration::from_millis(50),
            memory: 16 * 1024 * 1024,
        }
    }
}

/// Compiles and evaluates expressions.
pub trait Evaluator: Send + Sync {
    /// Compiles `source` without running it, for validation on save.
    fn check(&self, source: &str) -> Result<(), ExprError>;

    /// Evaluates `source` against `scope`.
    fn evaluate(&self, source: &str, scope: &Scope<'_>) -> Evaluation;
}

/// What an expression can see (flow spec §8.7).
pub struct Scope<'a> {
    pub state: &'a Arc<LiveState>,
    /// Connection aliases and the IDs their state is stored under.
    pub aliases: &'a BTreeMap<String, ConnId>,
    /// The trigger payload; `undefined` when absent.
    pub trigger: Option<&'a Value>,
    pub local: &'a BTreeMap<String, Value>,
    /// `steps.<id>` for the steps that have finished.
    pub steps: &'a BTreeMap<String, Value>,
    /// Stops the evaluation when it turns `true`.
    pub cancel: Option<&'a watch::Receiver<bool>>,
}

impl<'a> Scope<'a> {
    /// State and aliases only: no trigger, run or cancellation.
    pub fn new(state: &'a Arc<LiveState>, aliases: &'a BTreeMap<String, ConnId>) -> Self {
        static EMPTY: BTreeMap<String, Value> = BTreeMap::new();
        Scope {
            state,
            aliases,
            trigger: None,
            local: &EMPTY,
            steps: &EMPTY,
            cancel: None,
        }
    }
}

/// The result of one evaluation.
#[derive(Clone, Debug, PartialEq)]
pub struct Evaluation {
    /// `None` is `undefined`.
    pub value: Result<Option<Value>, ExprError>,
    pub reads: Reads,
}

/// The state an evaluation read (flow spec §8.7, "Dependencies").
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reads {
    /// In stored form. A path stands for itself and everything below it.
    pub paths: BTreeSet<StatePath>,
    /// The same reads as text (`vh.outputs[1].source`), for people.
    pub text: BTreeSet<String>,
}

impl Reads {
    /// Whether any changed leaf could change what was read: a leaf at or
    /// below a read path, or above it (a leaf replaced by a branch).
    pub fn touched_by(&self, dirty: &Dirty) -> bool {
        dirty
            .iter()
            .any(|d| self.paths.iter().any(|r| d.is_within(r) || r.is_within(d)))
    }
}

/// Why an expression failed (flow spec §8.7).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub struct ExprError {
    pub code: ExprCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<At>,
}

impl ExprError {
    fn new(code: ExprCode, message: impl Into<String>) -> Self {
        ExprError {
            code,
            message: message.into(),
            at: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExprCode {
    #[serde(rename = "exprSyntax")]
    Syntax,
    #[serde(rename = "exprError")]
    Error,
    #[serde(rename = "exprTimeout")]
    Timeout,
    #[serde(rename = "exprMemory")]
    Memory,
    /// The run was cancelled during the evaluation (flow spec §8.5).
    #[serde(rename = "cancelled")]
    Cancelled,
}

impl ExprCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ExprCode::Syntax => "exprSyntax",
            ExprCode::Error => "exprError",
            ExprCode::Timeout => "exprTimeout",
            ExprCode::Memory => "exprMemory",
            ExprCode::Cancelled => "cancelled",
        }
    }
}

/// A position in an expression's source, 1-based.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct At {
    pub line: u32,
    pub column: u32,
}

/// A value as JSON, as it goes into JavaScript. Binary and extension values have no JSON
/// form and arrive as `null`; so do NaN and the infinities.
pub fn to_json(value: &Value) -> serde_json::Value {
    use serde_json::Value as J;
    match value {
        Value::Nil | Value::Binary(_) | Value::Ext(..) => J::Null,
        Value::Boolean(b) => J::Bool(*b),
        Value::Integer(i) => match (i.as_i64(), i.as_u64()) {
            (Some(n), _) => n.into(),
            (None, Some(n)) => n.into(),
            (None, None) => J::Null,
        },
        Value::F32(f) => serde_json::Number::from_f64(f64::from(*f)).map_or(J::Null, J::Number),
        Value::F64(f) => serde_json::Number::from_f64(*f).map_or(J::Null, J::Number),
        Value::String(s) => J::String(s.as_str().map_or_else(|| s.to_string(), str::to_owned)),
        Value::Array(items) => J::Array(items.iter().map(to_json).collect()),
        Value::Map(entries) => J::Object(
            entries
                .iter()
                .map(|(k, v)| {
                    let key = match k {
                        Value::String(s) => s.as_str().map_or_else(|| s.to_string(), str::to_owned),
                        other => other.to_string(),
                    };
                    (key, to_json(v))
                })
                .collect(),
        ),
    }
}

/// Whether a result counts as true where an expression decides something:
/// JavaScript truthiness, with `None` as `undefined` (spec §8.7).
pub fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Nil) => false,
        Some(Value::Boolean(b)) => *b,
        Some(Value::Integer(i)) => i.as_i64() != Some(0),
        Some(Value::F32(f)) => *f != 0.0 && !f.is_nan(),
        Some(Value::F64(f)) => *f != 0.0 && !f.is_nan(),
        Some(Value::String(s)) => !s.as_bytes().is_empty(),
        Some(_) => true,
    }
}

#[cfg(test)]
mod tests;

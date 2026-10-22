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
pub const RESERVED_ROOTS: &[(&str, &str)] = &[("vars", "vars"), ("sys", "sys"), ("flow", "flow")];

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

#[cfg(test)]
mod tests;

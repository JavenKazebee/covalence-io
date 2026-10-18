//! State, events and logs (host → core). Spec §6.
//!
//! Paths are segment arrays relative to the connection root (ADR 0017).

use serde::{Deserialize, Serialize};

use crate::{Object, True, Value};

/// A path segment: an integer collection key, or a key.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PathSegment {
    Index(i64),
    Key(String),
}

pub type Path = Vec<PathSegment>;

/// A path and its value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PathValue {
    pub p: Path,
    pub v: Value,
}

/// `state.snapshot` params (ntf). Replaces the connection's whole subtree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StateSnapshot {
    pub values: Vec<PathValue>,
    pub complete: True,
}

/// `state.patch` params (ntf).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StatePatch {
    pub ops: Vec<PatchOp>,
    /// The `action.invoke` call that produced this change, if any. Unset for
    /// changes from device traffic outside an action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<String>,
    /// Unix epoch milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PatchOp {
    Delete(PathDelete),
    Set(PathValue),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PathDelete {
    pub p: Path,
    pub del: True,
}

/// `event.emit` params (ntf).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventEmit {
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
    /// The `action.invoke` call that produced this event, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<String>,
    /// Unix epoch milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts: Option<f64>,
}

/// `log` params (ntf). The envelope's `conn` is optional here.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Log {
    pub level: LogLevel,
    pub msg: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Object>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

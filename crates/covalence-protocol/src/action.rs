//! Actions. Spec §5.

use serde::{Deserialize, Serialize};

use crate::Value;

/// `action.invoke` params (core → host, req).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActionInvoke {
    /// ULID. Idempotency key; matches the journal's `action.requested.call`.
    pub call: String,
    /// Flow run ID, for logs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    /// Fencing term. The host rejects any term lower than the highest it has seen.
    pub term: u32,
    pub action: String,
    pub params: Value,
    /// Milliseconds. The host gives up after this long.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<u32>,
}

/// `action.invoke` result.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActionResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
}

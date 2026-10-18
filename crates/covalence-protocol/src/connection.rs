//! Connection lifecycle. Spec §3.3 and §3.4.
//!
//! The connection ID travels in the envelope's `conn`, not in these params.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::plugin::Permissions;
use crate::{Object, Value};

/// `conn.create` params (core → host, req).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConnCreate {
    pub plugin: String,
    pub version: String,
    pub alias: String,
    pub config: Object,
    pub secrets: BTreeMap<String, String>,
    /// Resolved from the manifest templates and the config. Not yet enforced (ADR 0015).
    pub permissions: Permissions,
    pub mode: ConnMode,
    pub isolate: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnMode {
    Live,
    Simulate,
}

/// `conn.destroy` params (core → host, req).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConnDestroy {
    pub reason: String,
}

/// `conn.status` params (host → core, ntf).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConnStatus {
    pub status: ConnStatusKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnStatusKind {
    Connecting,
    Ok,
    Warning,
    Error,
    Disconnected,
}

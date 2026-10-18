//! Plugin loading and the manifest. Spec §3.2 and §4.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{Object, Value};

/// `plugin.load` params (core → host, req).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginLoad {
    pub plugin: String,
    pub version: String,
    /// Path to the bundle, or a blob reference for an Orbital.
    pub bundle: String,
}

/// `plugin.load` result.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginLoaded {
    pub manifest: Manifest,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    /// Semver range of supported protocol versions.
    pub protocol: String,
    pub sdk: String,
    pub runtime: Runtime,
    pub license: String,
    pub authors: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    pub categories: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub config: ConfigDef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secrets: Option<BTreeMap<String, SecretDef>>,
    /// Declared, but not yet enforced (ADR 0015).
    pub permissions: Permissions,
    pub connections: ConnectionPolicy,
    pub actions: Vec<ActionDef>,
    pub events: Vec<EventDef>,
    /// State schema (state spec §1.2–1.4).
    pub state: Object,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presets: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surfaces: Option<Object>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub simulator: Option<SimulatorSupport>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Runtime {
    Deno,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConfigDef {
    /// JSON Schema that drives the connection form.
    pub schema: Object,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui: Option<Object>,
    /// Config schema version (ADR 0006).
    pub version: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SecretDef {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Permissions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub net: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub udp_broadcast: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<Vec<String>>,
}

/// Policy for standby nodes (ADR 0008).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionPolicy {
    Multiple,
    Single,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionDef {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema.
    pub params: Object,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Object>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotent: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancellable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_hint: Option<DurationHint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub learn: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DurationHint {
    Instant,
    Short,
    Long,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventDef {
    pub id: String,
    pub name: String,
    /// JSON Schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Object>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SimulatorSupport {
    pub state_level: bool,
    pub protocol_level: bool,
}

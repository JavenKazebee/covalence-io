//! Session lifecycle. Spec §3.1.

use serde::{Deserialize, Serialize};

/// `host.hello` params (host → core, req).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostHello {
    pub protocol: ProtocolRange,
    pub host: HostInfo,
    /// Orbital only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<NodeInfo>,
    pub capabilities: Vec<Capability>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProtocolRange {
    pub min: String,
    pub max: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HostInfo {
    pub kind: HostKind,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HostKind {
    DenoShared,
    DenoIsolated,
    Orbital,
    Wasm,
    External,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeInfo {
    pub id: String,
    pub name: String,
    pub public_key: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Capability {
    Workers,
    Permissions,
    Surfaces,
    Transports,
    Simulate,
    ComputerControl,
}

/// `host.hello` result.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostWelcome {
    /// The protocol version chosen for this session.
    pub protocol: String,
    pub session: String,
    /// Leadership term (ADR 0008).
    pub term: u32,
    pub core_id: String,
}

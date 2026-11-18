//! The core connection (core actions spec, core actions spec §6.1): everything Covalence
//! itself can do, as actions on a built-in connection `core`.
//!
//! Its actions are those of the interfaces it implements, kept in
//! `protocol/interfaces/`: `surface@1` on its surfaces and `flow@1` on its
//! flows, each item named by `key`. They run in-process: [`crate::Core`]
//! journals and gates them like any other action and hands them to the
//! [`CoreActions`] the server sets, which owns surfaces and flows.

use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;

use covalence_protocol::plugin::{ActionDef, Manifest};
use covalence_protocol::{ErrorCode, ProtocolError, Value};
use serde::Deserialize;

use crate::journal::{Origin, Seq};

/// The core connection's ID and alias. Reserved: no connection can take it.
pub const CORE: &str = "core";

/// An interface: a named set of actions (plugin protocol spec §13.9).
#[derive(Clone, Debug, Deserialize)]
pub struct Interface {
    pub id: String,
    pub name: String,
    pub actions: Vec<ActionDef>,
}

/// `surface@1` and `flow@1`, as `protocol/interfaces/` defines them.
pub fn interfaces() -> &'static [Interface] {
    static INTERFACES: OnceLock<Vec<Interface>> = OnceLock::new();
    INTERFACES.get_or_init(|| {
        [
            include_str!("../../../protocol/interfaces/surface@1.json"),
            include_str!("../../../protocol/interfaces/flow@1.json"),
        ]
        .iter()
        .map(|json| serde_json::from_str(json).expect("protocol/interfaces parse"))
        .collect()
    })
}

/// The interface an action of `core` belongs to, and so what its `key` names.
pub fn interface_of(action: &str) -> Option<&'static Interface> {
    interfaces()
        .iter()
        .find(|i| i.actions.iter().any(|a| a.id == action))
}

/// The core connection's manifest, served like a plugin's (core actions §1).
pub fn manifest() -> &'static Manifest {
    static MANIFEST: OnceLock<Manifest> = OnceLock::new();
    MANIFEST.get_or_init(|| {
        let actions: Vec<ActionDef> = interfaces()
            .iter()
            .flat_map(|i| i.actions.iter().cloned())
            .collect();
        serde_json::from_value(serde_json::json!({
            "id": CORE,
            "name": "Covalence",
            "version": env!("CARGO_PKG_VERSION"),
            "protocol": "^0.1",
            "sdk": "builtin",
            "runtime": "deno",
            "license": "MIT",
            "authors": ["Covalence IO contributors"],
            "categories": ["core"],
            "config": { "schema": { "type": "object" }, "version": 1 },
            "permissions": { "net": [], "serial": false, "udpBroadcast": false, "env": [] },
            "connections": "single",
            "actions": actions,
            "events": [],
            "state": {}
        }))
        .expect("the core manifest is a manifest")
    })
}

/// One `core` action, dispatched by the gate.
#[derive(Clone, Debug)]
pub struct CoreCall {
    /// Whose request it was: a run, the API, a user.
    pub origin: Origin,
    pub run: Option<String>,
    /// The `action.requested` entry: what the action's own changes are caused by.
    pub requested: Seq,
    /// The item: a surface ID or a flow ID.
    pub key: Option<String>,
    pub action: String,
    pub params: Value,
}

pub type CoreResult = Result<Option<Value>, ProtocolError>;

/// Carries out `core` actions: the server, which owns surfaces and flows.
pub trait CoreActions: Send + Sync + 'static {
    fn invoke(&self, call: CoreCall) -> Pin<Box<dyn Future<Output = CoreResult> + Send + '_>>;
}

/// An error with the spec's name for it (`unknownLayer`, `doesNotFit`) in
/// `data.code`, which a run's error takes as its code.
pub fn error(code: ErrorCode, name: &str, message: impl Into<String>) -> ProtocolError {
    let mut e = ProtocolError::new(code, message);
    e.data = Some(Value::Map(vec![("code".into(), name.into())]));
    e
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_has_every_interface_action_and_all_are_internal() {
        let ids: Vec<&str> = manifest().actions.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["navigate", "inject", "cancelRuns", "setEnabled"]);
        assert!(
            manifest()
                .actions
                .iter()
                .all(|a| a.effect() == covalence_protocol::plugin::Effect::Internal)
        );
        assert_eq!(interface_of("inject").unwrap().id, "surface@1");
        assert_eq!(interface_of("setEnabled").unwrap().id, "flow@1");
    }
}

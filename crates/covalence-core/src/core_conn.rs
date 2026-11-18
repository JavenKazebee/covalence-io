//! The core connection (core actions spec, core actions spec §6.1): everything Covalence
//! itself can do, as actions on a built-in connection `core`.
//!
//! A target is a connection and nothing within it (plugin protocol spec §13.9), so each
//! action names the surface or flow it acts on in a parameter, `surface` or `flow`, by ID
//! (core actions spec §2). They run in-process: [`crate::Core`] journals and gates them
//! like any other action and hands them to the [`CoreActions`] the server sets, which
//! owns surfaces and flows.

use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;

use covalence_protocol::plugin::{ActionDef, Manifest};
use covalence_protocol::{ErrorCode, ProtocolError, Value};

use crate::journal::{Origin, Seq};

/// The core connection's ID and alias. Reserved: no connection can take it.
pub const CORE: &str = "core";

/// What an action parameter names, by its schema's `format` (core actions spec §2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Surface,
    Flow,
}

impl Item {
    /// `surface` or `flow`: the parameter's name and its `format`.
    pub fn as_str(self) -> &'static str {
        match self {
            Item::Surface => "surface",
            Item::Flow => "flow",
        }
    }
}

/// The parameters of a `core` action that name a surface or a flow.
pub fn items_of(action: &str) -> Vec<(String, Item)> {
    let Some(def) = manifest().actions.iter().find(|a| a.id == action) else {
        return Vec::new();
    };
    let schema = serde_json::to_value(&def.params).expect("manifest values encode as JSON");
    let Some(props) = schema.get("properties").and_then(|p| p.as_object()) else {
        return Vec::new();
    };
    props
        .iter()
        .filter_map(|(name, prop)| {
            let item = match prop.get("format").and_then(|f| f.as_str()) {
                Some("surface") => Item::Surface,
                Some("flow") => Item::Flow,
                _ => return None,
            };
            Some((name.clone(), item))
        })
        .collect()
}

/// The core connection's manifest, served like a plugin's (core actions §1).
pub fn manifest() -> &'static Manifest {
    static MANIFEST: OnceLock<Manifest> = OnceLock::new();
    MANIFEST.get_or_init(|| {
        let actions: Vec<ActionDef> = serde_json::from_str(include_str!("core_conn/actions.json"))
            .expect("core's actions parse");
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
    fn the_manifest_has_every_action_and_all_are_internal() {
        let ids: Vec<&str> = manifest().actions.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["navigate", "inject", "cancelRuns", "setEnabled"]);
        assert!(
            manifest()
                .actions
                .iter()
                .all(|a| a.effect() == covalence_protocol::plugin::Effect::Internal)
        );
        assert_eq!(items_of("inject"), [("surface".to_owned(), Item::Surface)]);
        assert_eq!(items_of("setEnabled"), [("flow".to_owned(), Item::Flow)]);
    }
}

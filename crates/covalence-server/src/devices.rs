//! Devices: what's plugged in, which connection holds each, and connections
//! for devices no one holds (plugin protocol §7.1).
//!
//! | Method and path | Does |
//! |---|---|
//! | `GET /devices`  | `{available, devices, scanError?}`: each device with `heldBy`, its `handle` and an `offer` where there is one |
//!
//! A connection holds a device when its config names it, in a string field
//! of `format: "device"`, or when it has the device open. A device no
//! connection holds is offered to the plugins whose `devices` match it, and
//! gets a connection without asking when exactly one of them says
//! `autoCreate`.

use std::collections::BTreeMap;
use std::sync::{Arc, Weak};
use std::time::Duration;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use covalence_core::host::HostPhase;
use covalence_core::state::ConnId;
use covalence_devices::matching::{self, Claim, PluginDevices};
use covalence_protocol::connection::ConnMode;
use covalence_protocol::plugin::Manifest;
use covalence_protocol::{Object, Value};
use serde_json::{Value as JsonValue, json};
use tokio::sync::broadcast::error::RecvError;

use crate::{AddError, App, Connection, is_valid_alias};

/// How long an automatic connection waits for the plugin host.
const HOST_WAIT: Duration = Duration::from_secs(15);

pub(crate) fn routes() -> Router<Arc<App>> {
    Router::new().route("/devices", get(list))
}

/// The config property that names a connection's device: the first
/// top-level string property with `format: "device"`.
pub(crate) fn device_field(schema: &Object) -> Option<String> {
    let Some(Value::Map(properties)) = schema.get("properties") else {
        return None;
    };
    properties.iter().find_map(|(name, property)| {
        let Value::Map(fields) = property else {
            return None;
        };
        let format = fields
            .iter()
            .find(|(k, _)| k.as_str() == Some("format"))
            .and_then(|(_, v)| v.as_str());
        (format == Some("device")).then(|| name.as_str().map(str::to_owned))?
    })
}

/// The device a connection's config names, if its plugin has a device field.
fn named_device(conn: &Connection, manifest: &Manifest) -> Option<String> {
    let field = device_field(&manifest.config.schema)?;
    conn.config.get(&field)?.as_str().map(str::to_owned)
}

impl App {
    /// Every loaded plugin's manifest, by plugin ID.
    async fn manifests(&self) -> BTreeMap<String, Manifest> {
        let mut out = BTreeMap::new();
        for plugin in &self.plugins {
            if let Some(m) = self.manifest(&plugin.id).await {
                out.insert(plugin.id.clone(), m);
            }
        }
        out
    }

    /// Which connection holds each device: by its config, or by an open
    /// handle.
    fn holders(&self, manifests: &BTreeMap<String, Manifest>) -> BTreeMap<String, ConnId> {
        let mut out: BTreeMap<String, ConnId> = self
            .conns()
            .values()
            .filter_map(|c| Some((named_device(c, manifests.get(&c.plugin)?)?, c.id.clone())))
            .collect();
        if let Some(devices) = &self.devices {
            out.extend(devices.holders());
        }
        out
    }

    fn claims(&self, manifests: &BTreeMap<String, Manifest>) -> Vec<Claim> {
        let Some(devices) = &self.devices else {
            return Vec::new();
        };
        let plugins: Vec<PluginDevices<'_>> = manifests
            .iter()
            .filter(|(_, m)| !m.devices.is_empty())
            .map(|(id, m)| PluginDevices {
                plugin: id,
                matches: &m.devices,
            })
            .collect();
        let holders = self.holders(manifests);
        matching::claims(&plugins, &devices.list(), |d| holders.contains_key(d))
    }

    /// Creates a connection for each device that should get one without
    /// asking.
    async fn claim_devices(&self) {
        let manifests = self.manifests().await;
        for claim in self.claims(&manifests) {
            let Claim::AutoCreate { device, plugin } = claim else {
                continue;
            };
            let Some(field) = manifests
                .get(&plugin)
                .and_then(|m| device_field(&m.config.schema))
            else {
                tracing::warn!(
                    %plugin, %device,
                    "plugin asks for its devices' connections to be created, but its config has no field of format \"device\" to name one"
                );
                continue;
            };
            let alias = self.free_alias(&plugin);
            let mut config = serde_json::Map::new();
            config.insert(field, JsonValue::String(device.clone()));
            match self
                .add_connection(&plugin, alias, config, ConnMode::Live, HOST_WAIT)
                .await
            {
                Ok(conn) => {
                    tracing::info!(%device, alias = %conn.alias, "created a connection for a device")
                }
                // Saved, and retried with the host.
                Err(e @ AddError::HostRejected { .. }) => tracing::warn!(%device, "{e}"),
                Err(e) => {
                    tracing::error!(%device, %plugin, "cannot create a connection for a device: {e}")
                }
            }
        }
    }

    /// An alias for a new connection with `plugin`: the last part of its ID,
    /// numbered if that's taken.
    fn free_alias(&self, plugin: &str) -> String {
        let last = plugin.rsplit('.').next().unwrap_or(plugin);
        let mut base: String = last
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        if !is_valid_alias(&base) {
            base = format!("dev_{base}");
        }
        let conns = self.conns();
        let taken = |a: &str| conns.values().any(|c| c.alias == a);
        if !taken(&base) && is_valid_alias(&base) {
            return base;
        }
        (2..)
            .map(|n| format!("{base}{n}"))
            .find(|a| !taken(a) && is_valid_alias(a))
            .expect("some number is free")
    }
}

/// Claims devices whenever they change or the plugin host comes up.
pub(crate) fn spawn(app: &Arc<App>) {
    let Some(devices) = &app.devices else {
        return;
    };
    let mut changes = devices.subscribe();
    let mut status = app.core.host().status();
    let mut closing = app.closing.subscribe();
    let app: Weak<App> = Arc::downgrade(app);
    tokio::spawn(async move {
        loop {
            {
                let Some(app) = app.upgrade() else { return };
                if status.borrow().phase == HostPhase::Ready {
                    app.claim_devices().await;
                }
            }
            tokio::select! {
                change = changes.recv() => match change {
                    Ok(_) | Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => return,
                },
                changed = status.changed() => if changed.is_err() { return },
                _ = closing.changed() => return,
            }
        }
    });
}

async fn list(State(app): State<Arc<App>>) -> Json<JsonValue> {
    let Some(devices) = &app.devices else {
        return Json(json!({ "available": false, "devices": [] }));
    };
    let manifests = app.manifests().await;
    let holders = app.holders(&manifests);
    let offers: BTreeMap<String, Claim> = app
        .claims(&manifests)
        .into_iter()
        .map(|c| (c.device().to_owned(), c))
        .collect();
    let handles: BTreeMap<String, covalence_devices::HandleInfo> = devices
        .handles()
        .into_iter()
        .map(|h| (h.device.clone(), h))
        .collect();
    let list: Vec<JsonValue> = devices
        .list()
        .into_iter()
        .map(|d| {
            let mut out = serde_json::to_value(&d).expect("a device encodes as JSON");
            if let Some(holder) = holders.get(&d.id) {
                out["heldBy"] = json!(holder);
            }
            if let Some(h) = handles.get(&d.id) {
                out["handle"] = json!({
                    "handle": h.handle,
                    "state": h.state,
                    "error": h.error.as_ref().map(|e| e.report()),
                    "attached": h.attached,
                });
            }
            if let Some(claim) = offers.get(&d.id) {
                out["offer"] = json!(claim);
            }
            out
        })
        .collect();
    let mut out = json!({ "available": true, "devices": list });
    if let Some(e) = devices.scan_error() {
        out["scanError"] = json!(e.report());
    }
    Json(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_device_field() {
        let schema = crate::json_object_to_value(
            json!({
                "type": "object",
                "properties": {
                    "host": { "type": "string", "format": "hostname" },
                    "deck": { "type": "string", "format": "device" },
                }
            })
            .as_object()
            .unwrap(),
        );
        assert_eq!(device_field(&schema).as_deref(), Some("deck"));
        assert_eq!(device_field(&Object::new()), None);
    }
}

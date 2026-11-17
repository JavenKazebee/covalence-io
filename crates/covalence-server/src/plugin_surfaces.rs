//! Surfaces that plugins provide (plugin protocol §8, M7): a connection's
//! `surface.attach`, `surface.input` and `surface.detach` reach the core's
//! surface module the way the web surface's do, and its draws go back as
//! `surface.draw`, images as binary.
//!
//! - **Entities:** an attach is matched to a surface entity by `model` and
//!   `serial`, or by `model` and its provider when it has no serial. The
//!   first attach creates one, named as the plugin names it, with
//!   `provider` set to the connection; the provider follows the device to
//!   whichever connection attaches it next.
//! - **Keys:** `surface` in these notifications is the connection's own key
//!   for the device, not the entity's ID.
//! - **Detach:** a connection that stops (destroyed, or its plugin host
//!   lost) detaches its surfaces, which releases their input (surfaces
//!   §4.1). The device keeps showing what it showed until it's attached
//!   again.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, Weak};

use covalence_core::flow;
use covalence_core::host::PluginSurfaces;
use covalence_core::state::ConnId;
use covalence_core::surface::{self, Attach, ControlDef, Input, Surface, SurfaceId};
use covalence_protocol::Bytes;
use covalence_protocol::methods::core_to_host;
use covalence_protocol::surface::{ImageFormat, SurfaceAttach, SurfaceDraw, SurfaceInput};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::{App, read, write};

/// One attached surface of a connection.
struct Attached {
    surface: SurfaceId,
    /// Dropping it detaches the surface.
    input: mpsc::Sender<Input>,
    /// Sends the surface's draws to the connection.
    draws: JoinHandle<()>,
}

impl Drop for Attached {
    fn drop(&mut self) {
        self.draws.abort();
    }
}

pub(crate) struct AppSurfaces {
    app: Weak<App>,
    /// By connection and its key for the surface.
    attached: Mutex<BTreeMap<(ConnId, String), Attached>>,
}

impl AppSurfaces {
    pub(crate) fn new(app: Weak<App>) -> AppSurfaces {
        AppSurfaces {
            app,
            attached: Mutex::new(BTreeMap::new()),
        }
    }

    fn attached(&self) -> std::sync::MutexGuard<'_, BTreeMap<(ConnId, String), Attached>> {
        self.attached.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn try_attach(&self, conn: &str, attach: SurfaceAttach) -> Result<(), String> {
        let app = self.app.upgrade().ok_or("shutting down")?;
        let controls: Vec<ControlDef> = convert(&attach.controls)?;
        let entity = app.plugin_surface(conn, &attach)?;
        let key = (conn.to_owned(), attach.surface.clone());
        // An attach of a key it already has replaces it.
        self.attached().remove(&key);
        let link = app
            .surfaces()
            .attach(
                &entity.id,
                Attach {
                    model: attach.model,
                    serial: attach.serial,
                    name: attach.name,
                    controls,
                },
            )
            .map_err(|e| e.to_string())?;
        let mut draws = link.draws;
        let weak = self.app.clone();
        let (draw_conn, draw_key) = key.clone();
        let task = tokio::spawn(async move {
            while let Some(draw) = draws.recv().await {
                let Some(app) = weak.upgrade() else { return };
                let Some(format) = image_format(&draw.format) else {
                    tracing::warn!(format = %draw.format, "no protocol format for a drawn image");
                    continue;
                };
                app.core.host().notify::<core_to_host::SurfaceDraw>(
                    &draw_conn,
                    &SurfaceDraw {
                        surface: draw_key.clone(),
                        control: draw.control,
                        image: Bytes(draw.image),
                        format,
                    },
                );
            }
            // Attached again (by another connection or this one) or deleted:
            // whoever did that owns the surface now.
        });
        tracing::info!(%conn, surface = %entity.slug, "plugin surface attached");
        self.attached().insert(
            key,
            Attached {
                surface: entity.id,
                input: link.input,
                draws: task,
            },
        );
        Ok(())
    }
}

impl PluginSurfaces for AppSurfaces {
    fn attach(&self, conn: &str, attach: SurfaceAttach) {
        let key = attach.surface.clone();
        if let Err(e) = self.try_attach(conn, attach) {
            tracing::warn!(%conn, surface = %key, "cannot attach a plugin surface: {e}");
        }
    }

    fn input(&self, conn: &str, input: SurfaceInput) {
        let attached = self.attached();
        let Some(a) = attached.get(&(conn.to_owned(), input.surface.clone())) else {
            tracing::debug!(%conn, surface = %input.surface, "input for a surface that isn't attached");
            return;
        };
        let converted: Input = match convert_one(&input) {
            Ok(i) => i,
            Err(e) => {
                tracing::warn!(%conn, "bad surface.input: {e}");
                return;
            }
        };
        if a.input.try_send(converted).is_err() {
            tracing::warn!(%conn, surface = %a.surface, "surface input backlog full; dropped one");
        }
    }

    fn detach(&self, conn: &str, surface: &str) {
        self.attached()
            .remove(&(conn.to_owned(), surface.to_owned()));
    }

    fn stopped(&self, conn: &str) {
        self.attached().retain(|(c, _), _| c != conn);
    }
}

impl App {
    /// The entity an attach from `conn` is for, created if it's new.
    fn plugin_surface(&self, conn: &str, attach: &SurfaceAttach) -> Result<Surface, String> {
        let _saving = self.saving.lock().unwrap_or_else(|e| e.into_inner());
        let serial = attach.serial.as_deref().filter(|s| !s.is_empty());
        let found = read(&self.surface_entities)
            .values()
            .find(|s| {
                s.model == attach.model
                    && match serial {
                        Some(serial) => s.serial.as_deref() == Some(serial),
                        None => s.serial.is_none() && s.provider.as_deref() == Some(conn),
                    }
            })
            .cloned();
        let surface = match found {
            Some(s) if s.provider.as_deref() == Some(conn) => return Ok(s),
            // Its device moved to another connection.
            Some(s) => Surface {
                provider: Some(conn.to_owned()),
                ..s
            },
            None => {
                let taken: BTreeSet<String> = read(&self.surface_entities)
                    .values()
                    .map(|s| s.slug.clone())
                    .collect();
                let name = if attach.name.trim().is_empty() {
                    attach.model.clone()
                } else {
                    attach.name.clone()
                };
                Surface {
                    id: ulid::Ulid::generate().to_string(),
                    slug: flow::derive_slug_for("surface", &name, &taken),
                    name,
                    model: attach.model.clone(),
                    serial: serial.map(str::to_owned),
                    start_layer: None,
                    provider: Some(conn.to_owned()),
                }
            }
        };
        self.put_entity(surface::SURFACE_ENTITY, &surface.id, &surface)
            .map_err(|e| e.to_string())?;
        self.surfaces()
            .set_surface(surface.clone())
            .map_err(|e| e.to_string())?;
        write(&self.surface_entities).insert(surface.id.clone(), surface.clone());
        Ok(surface)
    }
}

/// The protocol's controls as the core's: the same JSON, with the core's
/// stricter checks.
fn convert<T: serde::Serialize, U: serde::de::DeserializeOwned>(from: &T) -> Result<U, String> {
    let json = serde_json::to_value(from).map_err(|e| e.to_string())?;
    serde_json::from_value(json).map_err(|e| e.to_string())
}

/// `surface.input` without `surface` and `echo`, which the core doesn't take
/// yet.
fn convert_one(input: &SurfaceInput) -> Result<Input, String> {
    let mut json = serde_json::to_value(input).map_err(|e| e.to_string())?;
    if let Some(fields) = json.as_object_mut() {
        fields.remove("surface");
        fields.remove("echo");
    }
    serde_json::from_value(json).map_err(|e| e.to_string())
}

fn image_format(format: &str) -> Option<ImageFormat> {
    match format {
        "jpeg" => Some(ImageFormat::Jpeg),
        "png" => Some(ImageFormat::Png),
        "bmp" => Some(ImageFormat::Bmp),
        "rgb565" => Some(ImageFormat::Rgb565),
        "rgb" => Some(ImageFormat::Rgb),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use covalence_protocol::surface::{InputCap, InputKind, OutputCap, Placement};

    #[test]
    fn protocol_controls_become_the_cores() {
        let protocol = vec![covalence_protocol::surface::ControlDef {
            id: "k0".into(),
            label: None,
            at: Placement {
                x: 0,
                y: 0,
                w: None,
                h: None,
            },
            inputs: vec![
                InputCap::Momentary { velocity: None },
                InputCap::Relative {},
            ],
            outputs: vec![OutputCap::Image {
                w: 72,
                h: 72,
                format: "jpeg".into(),
                rotation: Some(180),
            }],
        }];
        let core: Vec<ControlDef> = convert(&protocol).unwrap();
        assert_eq!(core[0].image().unwrap().rotation, 180);
        assert_eq!(core[0].inputs.len(), 2);

        let input = SurfaceInput {
            surface: "deck".into(),
            control: "k0".into(),
            kind: InputKind::Down,
            value: None,
            ts: 12.5,
            echo: Some(false),
        };
        let core = convert_one(&input).unwrap();
        assert_eq!((core.control.as_str(), core.ts), ("k0", 12.5));
    }
}

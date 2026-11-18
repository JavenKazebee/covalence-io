//! The `core` connection's actions (core actions spec §3), carried out by
//! the app, which owns surfaces and flows. The core journals and gates them
//! like any other action before they get here.

use std::future::Future;
use std::pin::Pin;
use std::sync::Weak;

use covalence_core::core_conn::{CoreActions, CoreCall, CoreResult, error};
use covalence_core::expr::to_json;
use covalence_core::journal::InputKind;
use covalence_core::surface::{InjectError, InputValue, ShowError};
use covalence_protocol::{ErrorCode, Value};
use serde::Deserialize;
use serde_json::Value as Json;

use crate::App;

pub(crate) struct AppCore(pub Weak<App>);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Navigate {
    surface: String,
    #[serde(default)]
    group: Option<String>,
    layer: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Inject {
    surface: String,
    control: String,
    input: InputKind,
    #[serde(default)]
    value: Option<InputValue>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetEnabled {
    flow: String,
    enabled: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CancelRuns {
    flow: String,
}

fn params<T: serde::de::DeserializeOwned>(
    json: Json,
) -> Result<T, covalence_protocol::ProtocolError> {
    serde_json::from_value(json)
        .map_err(|e| error(ErrorCode::InvalidParams, "invalidParams", e.to_string()))
}

impl CoreActions for AppCore {
    fn invoke(&self, call: CoreCall) -> Pin<Box<dyn Future<Output = CoreResult> + Send + '_>> {
        Box::pin(async move {
            let Some(app) = self.0.upgrade() else {
                return Err(error(
                    ErrorCode::NotConnected,
                    "unavailable",
                    "shutting down",
                ));
            };
            let json = to_json(&call.params);
            match call.action.as_str() {
                "navigate" => {
                    let p: Navigate = params(json)?;
                    let group = p
                        .group
                        .as_deref()
                        .unwrap_or(covalence_core::surface::MAIN_GROUP);
                    app.surfaces()
                        .navigate(&p.surface, group, p.layer.as_deref(), call.origin)
                        .map_err(|e| match e {
                            ShowError::BadGroup(_) => {
                                error(ErrorCode::InvalidParams, "invalid", e.to_string())
                            }
                            ShowError::UnknownSurface(_) => {
                                error(ErrorCode::NotFound, "unknownSurface", e.to_string())
                            }
                            ShowError::UnknownLayer(_) => {
                                error(ErrorCode::InvalidParams, "unknownLayer", e.to_string())
                            }
                            ShowError::DoesNotFit { .. } => {
                                error(ErrorCode::ActionFailed, "doesNotFit", e.to_string())
                            }
                            ShowError::Journal(_) => {
                                error(ErrorCode::Internal, "internal", e.to_string())
                            }
                        })?;
                    // Answers once the surface has been sent its outputs.
                    app.surfaces().flush().await;
                    let layer = p.layer.map_or(Value::Nil, Value::from);
                    Ok(Some(Value::Map(vec![("layer".into(), layer)])))
                }
                "inject" => {
                    let p: Inject = params(json)?;
                    app.surfaces()
                        .inject(
                            &p.surface,
                            &p.control,
                            p.input,
                            p.value,
                            call.origin,
                            Some(call.requested),
                        )
                        .await
                        .map_err(|e| match e {
                            InjectError::UnknownSurface(_) => {
                                error(ErrorCode::NotFound, "unknownSurface", e.to_string())
                            }
                            InjectError::NotAttached(_) => {
                                error(ErrorCode::NotConnected, "notAttached", e.to_string())
                            }
                            InjectError::UnknownControl(_) => {
                                error(ErrorCode::InvalidParams, "unknownControl", e.to_string())
                            }
                            InjectError::Unsupported(_) => {
                                error(ErrorCode::InvalidParams, "unsupported", e.to_string())
                            }
                        })?;
                    Ok(None)
                }
                "cancelRuns" => {
                    let p: CancelRuns = params(json)?;
                    if !app.flows().contains_key(&p.flow) {
                        return Err(error(
                            ErrorCode::NotFound,
                            "unknownFlow",
                            format!("no flow {}", p.flow),
                        ));
                    }
                    let cancelled = app.runtime().cancel_flow(&p.flow);
                    Ok(Some(Value::Map(vec![(
                        "cancelled".into(),
                        (cancelled as u64).into(),
                    )])))
                }
                "setEnabled" => {
                    let p: SetEnabled = params(json)?;
                    let Some(mut flow) = app.flows().get(&p.flow).cloned() else {
                        return Err(error(
                            ErrorCode::NotFound,
                            "unknownFlow",
                            format!("no flow {}", p.flow),
                        ));
                    };
                    if flow.enabled != p.enabled {
                        flow.enabled = p.enabled;
                        app.put(flow)
                            .map_err(|e| error(ErrorCode::Internal, "internal", e.to_string()))?;
                    }
                    Ok(None)
                }
                other => Err(error(
                    ErrorCode::InvalidParams,
                    "unknownAction",
                    format!("core has no action {other:?}"),
                )),
            }
        })
    }
}

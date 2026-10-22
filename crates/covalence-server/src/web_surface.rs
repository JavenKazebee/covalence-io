//! `GET /surface/{id}`: the web surface (surfaces spec §4). The browser's
//! virtual deck attaches to the core over this socket the way a hardware
//! deck will through the plugin host: plugin protocol §8 notifications, as
//! JSON text frames, with images as base64 PNG.
//!
//! The first message must be `surface.attach` with the surface's slug.
//! After that the browser sends `surface.input` and gets `surface.draw`.
//! Anything else closes the socket with a policy error and a reason, as
//! does the surface being attached again elsewhere or deleted. The socket
//! closing detaches the surface.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade, close_code};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use covalence_core::surface::{Attach, Draw, Input, Link, Surface};
use serde::Deserialize;
use serde_json::{Value as JsonValue, json};

use crate::App;
use crate::api::ApiError;

/// How long a new socket has to send its `surface.attach`.
const ATTACH_WAIT: Duration = Duration::from_secs(10);

pub async fn upgrade(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    ws: WebSocketUpgrade,
) -> Response {
    match app.surface_entities().remove(&id) {
        Some(surface) => ws.on_upgrade(move |socket| session(app, surface, socket)),
        None => ApiError::new(StatusCode::NOT_FOUND, format!("no surface {id:?}")).into_response(),
    }
}

/// A notification from the browser: `{method, params}`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Notification {
    method: String,
    params: serde_json::Map<String, JsonValue>,
}

/// Why a session ended, sent as the close frame.
struct Close(u16, String);

impl Close {
    fn policy(reason: impl Into<String>) -> Self {
        Close(close_code::POLICY, reason.into())
    }
}

async fn session(app: Arc<App>, surface: Surface, mut socket: WebSocket) {
    let close = match attach(&app, &surface, &mut socket).await {
        Ok(link) => run(&app, &surface, &mut socket, link).await,
        Err(close) => Some(close),
    };
    if let Some(Close(code, reason)) = close {
        let frame = CloseFrame {
            code,
            reason: reason.into(),
        };
        let _ = socket.send(Message::Close(Some(frame))).await;
    }
}

/// Waits for the browser's `surface.attach` and attaches the surface.
async fn attach(app: &App, surface: &Surface, socket: &mut WebSocket) -> Result<Link, Close> {
    let first = tokio::time::timeout(ATTACH_WAIT, next(socket))
        .await
        .map_err(|_| Close::policy("expected surface.attach"))??;
    let Some(Notification { method, params }) = first else {
        return Err(Close(close_code::NORMAL, String::new()));
    };
    if method != "surface.attach" {
        return Err(Close::policy(format!(
            "expected surface.attach, got {method}"
        )));
    }
    let attach: Attach = params_for(surface, params)?;
    app.surfaces()
        .attach(&surface.id, attach)
        .map_err(|e| Close::policy(format!("cannot attach: {e}")))
}

/// Carries input to the core and draws to the browser until either side
/// stops. `None` when the browser has gone.
async fn run(app: &App, surface: &Surface, socket: &mut WebSocket, link: Link) -> Option<Close> {
    let Link { input, mut draws } = link;
    let mut closing = app.closing.subscribe();
    loop {
        tokio::select! {
            notification = next(socket) => match notification {
                Ok(Some(Notification { method, params })) => match method.as_str() {
                    "surface.input" => {
                        let i: Input = match params_for(surface, params) {
                            Ok(i) => i,
                            Err(close) => return Some(close),
                        };
                        if input.send(i).await.is_err() {
                            return Some(Close(close_code::AWAY, "detached".into()));
                        }
                    }
                    "surface.detach" => return Some(Close(close_code::NORMAL, "detached".into())),
                    _ => return Some(Close::policy(format!("unexpected {method}"))),
                },
                Ok(None) => return None,
                Err(close) => return Some(close),
            },
            draw = draws.recv() => match draw {
                Some(draw) => {
                    if socket.send(draw_message(surface, draw)).await.is_err() {
                        return None;
                    }
                }
                None => {
                    // The core let go: attached again elsewhere, or deleted.
                    let reason = if app.surface_entities().contains_key(&surface.id) {
                        "attached elsewhere"
                    } else {
                        "surface deleted"
                    };
                    return Some(Close(close_code::NORMAL, reason.into()));
                }
            },
            changed = closing.changed() => {
                if changed.is_err() || *closing.borrow() {
                    return Some(Close(close_code::AWAY, "server shutting down".into()));
                }
            }
        }
    }
}

/// The next notification, `None` once the browser has closed the socket.
async fn next(socket: &mut WebSocket) -> Result<Option<Notification>, Close> {
    loop {
        match socket.recv().await {
            Some(Ok(Message::Text(text))) => {
                return serde_json::from_str(&text)
                    .map(Some)
                    .map_err(|e| Close::policy(format!("bad notification: {e}")));
            }
            Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return Ok(None),
            // Pings are answered by axum; binary frames mean nothing here.
            Some(Ok(_)) => {}
        }
    }
}

/// A notification's params without `surface`, which must be this surface's
/// slug.
fn params_for<T: serde::de::DeserializeOwned>(
    surface: &Surface,
    mut params: serde_json::Map<String, JsonValue>,
) -> Result<T, Close> {
    match params.remove("surface") {
        Some(JsonValue::String(slug)) if slug == surface.slug => {}
        other => {
            return Err(Close::policy(format!(
                "this socket is for surface {:?}, not {}",
                surface.slug,
                other.map_or_else(|| "none".into(), |v| v.to_string())
            )));
        }
    }
    serde_json::from_value(JsonValue::Object(params))
        .map_err(|e| Close::policy(format!("bad params: {e}")))
}

fn draw_message(surface: &Surface, draw: Draw) -> Message {
    let message = json!({
        "method": "surface.draw",
        "params": {
            "surface": surface.slug,
            "control": draw.control,
            "format": draw.format,
            "image": BASE64.encode(draw.image),
        },
    });
    Message::Text(message.to_string().into())
}

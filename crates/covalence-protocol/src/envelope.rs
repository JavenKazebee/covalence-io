//! The message envelope. Spec §2.1.

use serde::{Deserialize, Deserializer, Serialize};

use crate::Value;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum Message {
    #[serde(rename = "req")]
    Request(Request),
    #[serde(rename = "res")]
    Response(Response),
    #[serde(rename = "ntf")]
    Notification(Notification),
}

/// A request; the peer must answer with a [`Response`] carrying the same `id`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// Unique per direction per link.
    pub id: u32,
    /// Method name, for example `host.hello`.
    pub m: String,
    /// Connection this message concerns. Hosts route on it without decoding `p`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conn: Option<String>,
    pub p: Value,
}

/// A fire-and-forget notification.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Notification {
    pub m: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conn: Option<String>,
    pub p: Value,
}

/// A response. On the wire: `{id, ok: true, r}` or `{id, ok: false, e}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "WireResponse", into = "WireResponse")]
pub struct Response {
    pub id: u32,
    pub outcome: Result<Value, ProtocolError>,
}

#[derive(Serialize, Deserialize)]
struct WireResponse {
    id: u32,
    ok: bool,
    // `r: null` is a valid result, so a missing `r` and a null one must differ.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    r: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    e: Option<ProtocolError>,
}

fn present<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

impl TryFrom<WireResponse> for Response {
    type Error = &'static str;

    fn try_from(wire: WireResponse) -> Result<Self, Self::Error> {
        let outcome = match (wire.ok, wire.r, wire.e) {
            (true, Some(r), None) => Ok(r),
            (false, None, Some(e)) => Err(e),
            (true, _, _) => return Err("a response with `ok: true` needs `r` and no `e`"),
            (false, _, _) => return Err("a response with `ok: false` needs `e` and no `r`"),
        };
        Ok(Response {
            id: wire.id,
            outcome,
        })
    }
}

impl From<Response> for WireResponse {
    fn from(response: Response) -> Self {
        let (ok, r, e) = match response.outcome {
            Ok(r) => (true, Some(r), None),
            Err(e) => (false, None, Some(e)),
        };
        WireResponse {
            id: response.id,
            ok,
            r,
            e,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[error("{code:?}: {message}")]
pub struct ProtocolError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
}

impl ProtocolError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        ProtocolError {
            code,
            message: message.into(),
            data: None,
            retryable: None,
        }
    }
}

/// Spec §9.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidRequest,
    UnknownMethod,
    UnsupportedVersion,
    InvalidParams,
    NotFound,
    NotConnected,
    PermissionDenied,
    ActionFailed,
    Timeout,
    Cancelled,
    StaleTerm,
    RestartRequired,
    Overloaded,
    Internal,
}

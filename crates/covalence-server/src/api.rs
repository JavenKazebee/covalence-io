//! HTTP handlers. See the table in the crate docs.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use covalence_core::gate::Verdict;
use covalence_core::host::{HostPhase, HostStatus};
use covalence_core::journal::{Entry, Origin, Seq};
use covalence_core::state::{Leaf, Quality, StatePath};
use covalence_core::{ActionRequest, CoreError, text_path};
use covalence_protocol::connection::ConnMode;
use covalence_protocol::state::PathSegment;
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};

use crate::{
    App, CONNECTION, Connection, SaveError, SaveResult, apply_defaults, is_valid_alias,
    json_to_value, resolve_permissions,
};

/// How long `POST /connections` waits for the plugin host to come up.
const HOST_WAIT: Duration = Duration::from_secs(15);
const DEFAULT_JOURNAL_LIMIT: usize = 100;
const MAX_JOURNAL_LIMIT: usize = 1000;

pub fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/plugins", get(plugins))
        .route("/connections", get(connections).post(create_connection))
        .route("/state", get(state))
        .route("/actions", post(request_action))
        .route("/journal", get(journal))
        .route("/journal/{seq}/why", get(why))
        .route("/ws", get(crate::ws::upgrade))
        .merge(crate::flows::routes())
        .merge(crate::surfaces::routes())
        .with_state(app)
}

pub(crate) struct ApiError(StatusCode, String);

impl ApiError {
    pub(crate) fn new(status: StatusCode, message: impl Into<String>) -> Self {
        ApiError(status, message.into())
    }

    pub(crate) fn internal(e: impl std::fmt::Display) -> Self {
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

pub(crate) type ApiResult<T> = Result<T, ApiError>;

/// `{<kind>: entity, warnings}`, or `422 {errors, warnings}`.
pub(crate) fn saved<T: Serialize>(
    result: SaveResult<T>,
    kind: &str,
    status: StatusCode,
) -> Response {
    match result {
        Ok((entity, warnings)) => {
            (status, Json(json!({ kind: entity, "warnings": warnings }))).into_response()
        }
        Err(SaveError::Invalid { errors, warnings }) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "errors": errors, "warnings": warnings })),
        )
            .into_response(),
        Err(SaveError::NotFound) => {
            ApiError::new(StatusCode::NOT_FOUND, format!("no such {kind}")).into_response()
        }
        Err(SaveError::Store(e)) => ApiError::internal(e).into_response(),
        Err(SaveError::Journal(e)) => ApiError::internal(e).into_response(),
    }
}

async fn health(State(app): State<Arc<App>>) -> Json<JsonValue> {
    let status = app.core.host().status().borrow().clone();
    let journal = &app.core.shared().journal;
    Json(json!({
        "host": host_json(&status),
        "journal": { "lastSeq": journal.last_seq(), "term": journal.term() },
    }))
}

/// Host status as the API reports it.
pub(crate) fn host_json(status: &HostStatus) -> JsonValue {
    let phase = match status.phase {
        HostPhase::Starting => "starting",
        HostPhase::Ready => "ready",
        HostPhase::Down => "down",
        HostPhase::Stopped => "stopped",
    };
    json!({ "phase": phase, "generation": status.generation, "pid": status.pid })
}

async fn plugins(State(app): State<Arc<App>>) -> Json<JsonValue> {
    let mut out = Vec::new();
    for plugin in &app.plugins {
        let manifest = app.manifest(&plugin.id).await;
        out.push(json!({
            "id": plugin.id,
            "version": plugin.version,
            "bundle": plugin.bundle,
            "manifest": manifest,
        }));
    }
    Json(JsonValue::Array(out))
}

async fn connections(State(app): State<Arc<App>>) -> Json<Vec<Connection>> {
    Json(app.conns().values().cloned().collect())
}

#[derive(Deserialize)]
struct CreateConnection {
    plugin: String,
    alias: String,
    #[serde(default)]
    config: serde_json::Map<String, JsonValue>,
    #[serde(default = "simulate")]
    mode: ConnMode,
}

fn simulate() -> ConnMode {
    ConnMode::Simulate
}

async fn create_connection(
    State(app): State<Arc<App>>,
    Json(req): Json<CreateConnection>,
) -> ApiResult<(StatusCode, Json<Connection>)> {
    if !is_valid_alias(&req.alias) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            format!(
                "{:?} is not a valid alias: use letters, digits and _, not starting with a digit, and not a reserved root or a name expressions already use",
                req.alias
            ),
        ));
    }
    if app.conn_by_alias(&req.alias).is_some() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            format!("alias {:?} is taken", req.alias),
        ));
    }
    let Some(spec) = app.plugins.iter().find(|p| p.id == req.plugin) else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            format!("unknown plugin {:?}", req.plugin),
        ));
    };
    if !app.host_ready(HOST_WAIT).await {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "the plugin host is not ready",
        ));
    }
    let Some(manifest) = app.manifest(&spec.id).await else {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("plugin {} is not loaded", spec.id),
        ));
    };

    let mut config = req.config;
    apply_defaults(&manifest.config.schema, &mut config);
    let permissions = resolve_permissions(&manifest.permissions, &config)
        .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, e))?;
    let conn = Connection {
        id: ulid::Ulid::generate().to_string(),
        plugin: spec.id.clone(),
        version: spec.version.clone(),
        alias: req.alias,
        config,
        mode: req.mode,
        permissions,
    };

    let body = serde_json::to_string(&conn).map_err(ApiError::internal)?;
    app.store
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .put_entity(CONNECTION, &conn.id, &body)
        .map_err(ApiError::internal)?;
    app.conns
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .insert(conn.id.clone(), conn.clone());
    app.sync_aliases();

    // The connection stays registered even if the host rejects it, so it is
    // retried after a host restart; report the failure all the same.
    if let Err(e) = app
        .core
        .host()
        .create_connection(&conn.id, conn.create_params())
        .await
    {
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            format!(
                "connection {} saved, but the host rejected it: {e}",
                conn.alias
            ),
        ));
    }
    Ok((StatusCode::CREATED, Json(conn)))
}

#[derive(Deserialize)]
struct StateQuery {
    path: Option<String>,
}

/// A state leaf as the API and the WebSocket send it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StateLeaf {
    path: String,
    value: JsonValue,
    quality: Quality,
    ts: u64,
    seq: Seq,
}

impl StateLeaf {
    pub(crate) fn new(alias: &str, path: &[PathSegment], leaf: &Leaf) -> Self {
        StateLeaf {
            path: text_path::format(alias, path),
            value: serde_json::to_value(&leaf.value).unwrap_or(JsonValue::Null),
            quality: leaf.meta.quality,
            ts: leaf.meta.ts,
            seq: leaf.meta.seq,
        }
    }
}

async fn state(
    State(app): State<Arc<App>>,
    Query(query): Query<StateQuery>,
) -> ApiResult<Json<Vec<StateLeaf>>> {
    let state_roots = app.state_roots();
    let roots: Vec<(String, StatePath)> = match query.path {
        None => state_roots
            .iter()
            .map(|(key, alias)| (alias.clone(), StatePath::root(key.clone())))
            .collect(),
        Some(text) => {
            let (alias, path) = text_path::parse(&text)
                .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, e.to_string()))?;
            let (key, _) = state_roots
                .iter()
                .find(|(_, a)| **a == alias)
                .ok_or_else(|| {
                    ApiError::new(StatusCode::NOT_FOUND, format!("no connection {alias:?}"))
                })?;
            vec![(alias, StatePath::new(key.clone(), path))]
        }
    };
    let leaves = app.core.shared().state.read(|tree| {
        roots
            .iter()
            .flat_map(|(alias, root)| {
                tree.leaves_under(root)
                    .map(move |(path, leaf)| StateLeaf::new(alias, &path.path, leaf))
            })
            .collect()
    });
    Ok(Json(leaves))
}

#[derive(Deserialize)]
struct RequestAction {
    /// `<alias>.<action>`, for example `vh.route`.
    action: String,
    #[serde(default)]
    params: JsonValue,
    /// Milliseconds.
    deadline: Option<u32>,
}

async fn request_action(
    State(app): State<Arc<App>>,
    Json(req): Json<RequestAction>,
) -> ApiResult<Json<JsonValue>> {
    let bad = || {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            format!("expected <alias>.<action>, got {:?}", req.action),
        )
    };
    let (alias, path) = text_path::parse(&req.action).map_err(|_| bad())?;
    let [PathSegment::Key(action)] = path.as_slice() else {
        return Err(bad());
    };
    let conn = app
        .conn_by_alias(&alias)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, format!("no connection {alias:?}")))?;

    let origin = Origin::User {
        user: "api".into(),
        client: "http".into(),
    };
    let mut request =
        ActionRequest::new(origin, conn.id, action.clone(), json_to_value(&req.params));
    request.deadline = req.deadline;
    let outcome = app
        .core
        .request_action(request)
        .await
        .map_err(|e| match e {
            CoreError::UnknownConnection(_) => ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                format!("connection {alias:?} is not running"),
            ),
            e => ApiError::internal(e),
        })?;

    let verdict = match outcome.verdict {
        Verdict::Dispatch => json!("dispatched"),
        Verdict::Suppress(reason) => json!({ "suppressed": reason }),
    };
    let mut body = json!({
        "call": outcome.call,
        "requested": outcome.requested,
        "verdict": verdict,
        "resultSeq": outcome.result_seq,
    });
    match outcome.result {
        Some(Ok(result)) => {
            body["ok"] = json!(true);
            body["result"] = serde_json::to_value(&result).map_err(ApiError::internal)?;
        }
        Some(Err(error)) => {
            body["ok"] = json!(false);
            body["error"] = serde_json::to_value(&error).map_err(ApiError::internal)?;
        }
        None => {}
    }
    Ok(Json(body))
}

#[derive(Deserialize)]
struct JournalQuery {
    from: Option<Seq>,
    limit: Option<usize>,
}

async fn journal(
    State(app): State<Arc<App>>,
    Query(query): Query<JournalQuery>,
) -> ApiResult<Json<Vec<Entry>>> {
    let limit = query
        .limit
        .unwrap_or(DEFAULT_JOURNAL_LIMIT)
        .min(MAX_JOURNAL_LIMIT);
    let entries = app
        .core
        .shared()
        .journal
        .range(query.from.unwrap_or(1), limit)
        .map_err(ApiError::internal)?;
    Ok(Json(entries.iter().map(|e| Entry::clone(e)).collect()))
}

async fn why(State(app): State<Arc<App>>, Path(seq): Path<Seq>) -> ApiResult<Json<Vec<Entry>>> {
    let chain = app
        .core
        .shared()
        .journal
        .why(seq)
        .map_err(ApiError::internal)?;
    if chain.is_empty() {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            format!("no journal entry {seq}"),
        ));
    }
    Ok(Json(chain.iter().map(|e| Entry::clone(e)).collect()))
}

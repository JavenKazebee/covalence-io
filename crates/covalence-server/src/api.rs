//! HTTP handlers. See the table in the crate docs.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use covalence_core::core_conn;
use covalence_core::gate::Verdict;
use covalence_core::host::{HostPhase, HostStatus};
use covalence_core::journal::{Origin, Seq};
use covalence_core::state::{Leaf, Quality, StatePath};
use covalence_core::{ActionRequest, CoreError, text_path};
use covalence_protocol::connection::ConnMode;
use covalence_protocol::state::PathSegment;
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};

use crate::{AddError, App, Connection, SaveError, SaveResult, json_to_value};

/// How long `POST /connections` waits for the plugin host to come up.
const HOST_WAIT: Duration = Duration::from_secs(15);
const DEFAULT_JOURNAL_LIMIT: usize = 100;
const MAX_JOURNAL_LIMIT: usize = 1000;

pub fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/plugins", get(plugins))
        .route("/connections", get(connections).post(create_connection))
        .route("/connections/{id}/traffic", get(traffic))
        .route("/state", get(state))
        .route("/actions", post(request_action))
        .route("/journal", get(journal))
        .route("/journal/{seq}/why", get(why))
        .route("/cells/problems", get(cell_problems))
        .route("/ws", get(crate::ws::upgrade))
        .merge(crate::flows::routes())
        .merge(crate::batch::routes())
        .merge(crate::surfaces::routes())
        .merge(crate::devices::routes())
        .merge(crate::entity::routes::<covalence_core::variable::Variable>())
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
        Err(SaveError::Conflict(why)) => ApiError::new(StatusCode::CONFLICT, why).into_response(),
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
    // The core connection's manifest, served like a plugin's (core actions §1).
    out.push(json!({
        "id": core_conn::CORE,
        "version": env!("CARGO_PKG_VERSION"),
        "builtIn": true,
        "manifest": core_conn::manifest(),
    }));
    Json(JsonValue::Array(out))
}

/// The project's connections, and `core`, which always exists. A connection
/// whose plugin opens its own sockets says `directNetwork` (plugin protocol spec §13.6): its
/// traffic isn't seen by the core, so the UI marks it as not inspectable.
async fn connections(State(app): State<Arc<App>>) -> Json<JsonValue> {
    let conns: Vec<Connection> = app.conns().values().cloned().collect();
    let mut out = Vec::with_capacity(conns.len() + 1);
    for conn in conns {
        let direct = app
            .manifest(&conn.plugin)
            .await
            .is_some_and(|m| m.direct_network == Some(true));
        let mut json = serde_json::to_value(&conn).expect("a connection encodes as JSON");
        if direct {
            json["directNetwork"] = JsonValue::Bool(true);
        }
        out.push(json);
    }
    out.push(json!({
        "id": core_conn::CORE,
        "alias": core_conn::CORE,
        "plugin": core_conn::CORE,
        "version": env!("CARGO_PKG_VERSION"),
        "config": {},
        "mode": "live",
        "builtIn": true,
    }));
    Json(JsonValue::Array(out))
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
    match app
        .add_connection(&req.plugin, req.alias, req.config, req.mode, HOST_WAIT)
        .await
    {
        Ok(conn) => Ok((StatusCode::CREATED, Json(conn))),
        Err(e) => {
            let status = match &e {
                AddError::InvalidAlias(_) | AddError::Permissions(_) => StatusCode::BAD_REQUEST,
                AddError::AliasTaken(_) => StatusCode::CONFLICT,
                AddError::UnknownPlugin(_) => StatusCode::NOT_FOUND,
                AddError::HostNotReady | AddError::NotLoaded(_) => StatusCode::SERVICE_UNAVAILABLE,
                AddError::HostRejected { .. } => StatusCode::BAD_GATEWAY,
                AddError::Store(_) => StatusCode::INTERNAL_SERVER_ERROR,
            };
            Err(ApiError::new(status, e.to_string()))
        }
    }
}

/// A connection's transport handles and their recent traffic (spec §7).
async fn traffic(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
) -> ApiResult<Json<JsonValue>> {
    let conn = app
        .conns()
        .get(&id)
        .cloned()
        .or_else(|| app.conn_by_alias(&id));
    let Some(conn) = conn else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            format!("no connection {id:?}"),
        ));
    };
    let handles = app.core.host().io().traffic(&conn.id);
    Ok(Json(json!({
        "conn": conn.id,
        // Opens its own sockets: what it sends isn't seen here (plugin protocol spec §13.6).
        "inspectable": app.manifest(&conn.plugin).await.is_none_or(|m| m.direct_network != Some(true)),
        "handles": handles,
    })))
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
    let conn_id = if alias == core_conn::CORE {
        core_conn::CORE.to_owned()
    } else {
        app.conn_by_alias(&alias)
            .ok_or_else(|| {
                ApiError::new(StatusCode::NOT_FOUND, format!("no connection {alias:?}"))
            })?
            .id
    };

    let origin = Origin::User {
        user: "api".into(),
        client: "http".into(),
    };
    let mut request =
        ActionRequest::new(origin, conn_id, action.clone(), json_to_value(&req.params));
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
    #[serde(flatten)]
    filter: crate::flows::InstanceFilter,
}

/// The most entries a filtered `GET /journal` reads looking for matches.
const MAX_JOURNAL_SCAN: usize = 100_000;

/// Entries from `from` on. Read from the store, past the memory window,
/// they have gaps where derived entries were, and the answer says so with
/// `Covalence-Derived-Dropped: true` (state spec §2.4).
async fn journal(
    State(app): State<Arc<App>>,
    Query(query): Query<JournalQuery>,
) -> ApiResult<Response> {
    let limit = query
        .limit
        .unwrap_or(DEFAULT_JOURNAL_LIMIT)
        .min(MAX_JOURNAL_LIMIT);
    let journal = &app.core.shared().journal;
    let from = query.from.unwrap_or(1);
    let entries = if query.filter.is_empty() {
        journal.range(from, limit).map_err(ApiError::internal)?
    } else {
        // Filtered: read on until `limit` match or the journal ends.
        let mut out = Vec::new();
        let mut next = from;
        let mut scanned = 0;
        while out.len() < limit && scanned < MAX_JOURNAL_SCAN {
            let page = journal.range(next, 1_000).map_err(ApiError::internal)?;
            let Some(last) = page.last() else { break };
            next = last.seq + 1;
            scanned += page.len();
            out.extend(
                page.into_iter()
                    .filter(|e| query.filter.matches(e.instance.as_deref()))
                    .take(limit - out.len()),
            );
        }
        out
    };
    let body = Json(
        entries
            .iter()
            .map(|e| app.with_names(e))
            .collect::<Vec<_>>(),
    );
    Ok(if journal.derived_dropped(from) {
        ([("covalence-derived-dropped", "true")], body).into_response()
    } else {
        body.into_response()
    })
}

/// Each failing cell's problem (dataflow §5): a variable's `variable:<id>`,
/// a helper's `helper:<binding>/<key>`, with `code` and `message`.
async fn cell_problems(State(app): State<Arc<App>>) -> Json<Vec<serde_json::Value>> {
    Json(
        app.dataflow
            .problems()
            .into_iter()
            .map(|(cell, p)| json!({ "cell": cell, "code": p.code, "message": p.message }))
            .collect(),
    )
}

async fn why(
    State(app): State<Arc<App>>,
    Path(seq): Path<Seq>,
) -> ApiResult<Json<Vec<serde_json::Value>>> {
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
    Ok(Json(chain.iter().map(|e| app.with_names(e)).collect()))
}

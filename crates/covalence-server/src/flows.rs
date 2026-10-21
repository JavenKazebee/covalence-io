//! Flows and runs over HTTP (flow spec §9).

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use covalence_core::flow::Flow;
use covalence_core::flow::runtime::{CancelError, FireError, FireOutcome, RunInfo};
use covalence_core::journal::Origin;
use serde::Deserialize;
use serde_json::{Value as JsonValue, json};

use crate::api::{ApiError, ApiResult};
use crate::{App, SaveError, SaveResult};

pub(crate) fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/flows", get(list).post(create))
        .route("/flows/{id}", get(read).put(replace).delete(remove))
        .route("/flows/{id}/run", post(run))
        .route("/runs", get(runs))
        .route("/runs/{run}", get(one_run))
        .route("/runs/{run}/cancel", post(cancel))
}

async fn list(State(app): State<Arc<App>>) -> Json<Vec<Flow>> {
    Json(app.flows().into_values().collect())
}

async fn read(State(app): State<Arc<App>>, Path(id): Path<String>) -> ApiResult<Json<Flow>> {
    app.flows()
        .remove(&id)
        .map(Json)
        .ok_or_else(|| no_flow(&id))
}

async fn create(State(app): State<Arc<App>>, Json(body): Json<JsonValue>) -> Response {
    saved(app.save_flow(&body, None).await, StatusCode::CREATED)
}

async fn replace(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Json(body): Json<JsonValue>,
) -> Response {
    saved(app.save_flow(&body, Some(&id)).await, StatusCode::OK)
}

/// `{flow, warnings}`, or `422 {errors, warnings}`.
fn saved(result: SaveResult, status: StatusCode) -> Response {
    match result {
        Ok((flow, warnings)) => {
            (status, Json(json!({ "flow": flow, "warnings": warnings }))).into_response()
        }
        Err(SaveError::Invalid { errors, warnings }) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "errors": errors, "warnings": warnings })),
        )
            .into_response(),
        Err(SaveError::NotFound) => {
            ApiError::new(StatusCode::NOT_FOUND, "no such flow").into_response()
        }
        Err(SaveError::Store(e)) => ApiError::internal(e).into_response(),
    }
}

async fn remove(State(app): State<Arc<App>>, Path(id): Path<String>) -> ApiResult<StatusCode> {
    match app.delete_flow(&id).map_err(ApiError::internal)? {
        Some(_) => Ok(StatusCode::NO_CONTENT),
        None => Err(no_flow(&id)),
    }
}

async fn run(State(app): State<Arc<App>>, Path(id): Path<String>) -> ApiResult<Json<JsonValue>> {
    let origin = Origin::User {
        user: "api".into(),
        client: "http".into(),
    };
    let fired = app.runtime().fire_api(&id, origin).map_err(|e| match e {
        FireError::UnknownFlow(_) => no_flow(&id),
        e @ (FireError::Disabled(_) | FireError::NoApiTrigger(_)) => {
            ApiError::new(StatusCode::CONFLICT, e.to_string())
        }
        FireError::Journal(e) => ApiError::internal(e),
    })?;
    let mut body = json!({ "matched": fired.matched });
    match fired.outcome {
        FireOutcome::Started(run) => {
            body["status"] = json!("started");
            body["run"] = json!(run);
        }
        FireOutcome::Queued(run) => {
            body["status"] = json!("queued");
            body["run"] = json!(run);
        }
        FireOutcome::Blocked(reason) => {
            body["status"] = json!("blocked");
            body["reason"] = json!(reason);
        }
    }
    Ok(Json(body))
}

#[derive(Deserialize)]
struct RunsQuery {
    flow: Option<String>,
}

async fn runs(State(app): State<Arc<App>>, Query(query): Query<RunsQuery>) -> Json<Vec<RunInfo>> {
    Json(app.runtime().runs(query.flow.as_deref()))
}

async fn one_run(State(app): State<Arc<App>>, Path(run): Path<String>) -> ApiResult<Json<RunInfo>> {
    app.runtime()
        .run(&run)
        .map(Json)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, format!("no run {run}")))
}

async fn cancel(
    State(app): State<Arc<App>>,
    Path(run): Path<String>,
) -> ApiResult<(StatusCode, Json<JsonValue>)> {
    match app.runtime().cancel(&run) {
        Ok(status) => Ok((
            StatusCode::ACCEPTED,
            Json(json!({ "run": run, "status": status })),
        )),
        Err(CancelError::UnknownRun(_)) => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            format!("no run {run}"),
        )),
        Err(e @ CancelError::Finished(..)) => {
            Err(ApiError::new(StatusCode::CONFLICT, e.to_string()))
        }
        Err(CancelError::Journal(e)) => Err(ApiError::internal(e)),
    }
}

fn no_flow(id: &str) -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, format!("no flow {id:?}"))
}

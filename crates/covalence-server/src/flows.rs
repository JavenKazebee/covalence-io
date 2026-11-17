//! Flows and runs over HTTP (flow spec §9).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use covalence_core::expr;
use covalence_core::flow::runtime::{CancelError, FireError, FireOutcome, RunInfo};
use covalence_core::flow::{self, Flow};
use covalence_core::journal::Origin;
use covalence_core::state::ConnId;
use covalence_protocol::plugin::Manifest;
use serde::Deserialize;
use serde_json::{Value as JsonValue, json};

use crate::api::{ApiError, ApiResult};
use crate::entity::{self, EntityKind};
use crate::{App, SaveError, read};

pub(crate) fn routes() -> Router<Arc<App>> {
    Router::new()
        .merge(entity::routes::<Flow>())
        .route("/flows/{id}/run", post(run))
        .route("/runs", get(runs))
        .route("/runs/{run}", get(one_run))
        .route("/runs/{run}/cancel", post(cancel))
        .route("/expressions/evaluate", post(evaluate))
}

async fn run(State(app): State<Arc<App>>, Path(id): Path<String>) -> ApiResult<Json<JsonValue>> {
    let origin = Origin::User {
        user: "api".into(),
        client: "http".into(),
    };
    let fired = app.runtime().fire_api(&id, origin).map_err(|e| match e {
        FireError::UnknownFlow(_) => entity::not_found::<Flow>(&id),
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EvaluateBody {
    source: String,
    /// A sample `trigger` payload, for previewing filters.
    #[serde(default)]
    trigger: Option<JsonValue>,
}

/// The editor's live preview (flow spec §9): `{value?, error?, reads}`.
/// Never writes, runs actions or journals.
async fn evaluate(
    State(app): State<Arc<App>>,
    Json(body): Json<EvaluateBody>,
) -> ApiResult<Json<JsonValue>> {
    // Up to the time limit of work, off the async workers.
    let evaluation = tokio::task::spawn_blocking(move || {
        let trigger = body.trigger.as_ref().map(crate::json_to_value);
        app.evaluate(&body.source, trigger.as_ref())
    })
    .await
    .map_err(ApiError::internal)?;
    let mut out = serde_json::Map::new();
    match evaluation.value {
        Ok(Some(value)) => {
            out.insert("value".into(), expr::to_json(&value));
        }
        Ok(None) => {}
        Err(error) => {
            out.insert("error".into(), json!(error));
        }
    }
    out.insert("reads".into(), json!(evaluation.reads.text));
    Ok(Json(JsonValue::Object(out)))
}

impl EntityKind for Flow {
    const TYPE: &'static str = flow::ENTITY;
    const NAME: &'static str = "flow";
    const PATH: &'static str = "/flows";
    /// Each connection's plugin manifest, once it has loaded.
    type Context = BTreeMap<ConnId, Option<Manifest>>;

    fn parse(json: &JsonValue) -> Result<Flow, Vec<flow::Problem>> {
        Flow::parse(json)
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn id_mut(&mut self) -> &mut String {
        &mut self.id
    }

    fn all(app: &App) -> &RwLock<BTreeMap<String, Flow>> {
        &app.flows
    }

    /// Fetching manifests waits on the plugin host.
    async fn context(app: &App) -> Self::Context {
        let conns = app.conns().clone();
        let mut manifests: BTreeMap<String, Option<Manifest>> = BTreeMap::new();
        for conn in conns.values() {
            if !manifests.contains_key(&conn.plugin) {
                let manifest = app.manifest(&conn.plugin).await;
                manifests.insert(conn.plugin.clone(), manifest);
            }
        }
        conns
            .values()
            .map(|c| (c.id.clone(), manifests[&c.plugin].clone()))
            .collect()
    }

    /// Flow spec §8.4.
    fn check(&mut self, app: &App, manifests: &Self::Context) -> flow::Report {
        let by_conn: BTreeMap<ConnId, Option<&Manifest>> = manifests
            .iter()
            .map(|(id, m)| (id.clone(), m.as_ref()))
            .collect();
        let flows = read(&app.flows);
        let taken: BTreeSet<String> = flows
            .values()
            .filter(|f| f.id != self.id)
            .map(|f| f.slug.clone())
            .collect();
        if self.slug.is_empty() {
            self.slug = flow::derive_slug(&self.name, &taken);
        }
        let layers = app.surfaces.layers();
        let surfaces: BTreeSet<String> = read(&app.surface_entities).keys().cloned().collect();
        let mut flow_ids: BTreeSet<String> = flows.keys().cloned().collect();
        flow_ids.insert(self.id.clone());
        drop(flows);
        flow::validate(
            self,
            &flow::Context {
                conns: &by_conn,
                taken_slugs: &taken,
                layers: &layers,
                surfaces: &surfaces,
                flows: &flow_ids,
                evaluator: app.evaluator.as_ref(),
            },
        )
    }

    fn apply(&self, app: &App) -> Result<(), SaveError> {
        app.runtime.set_flow(self.clone());
        Ok(())
    }

    /// Cancels its runs. The flow is gone either way, so a journal error is
    /// only logged.
    fn unapply(id: &str, app: &App) -> Result<(), SaveError> {
        if let Err(e) = app.runtime.remove_flow(id) {
            tracing::error!(flow = %id, "cannot journal the end of a deleted flow's runs: {e}");
        }
        Ok(())
    }
}

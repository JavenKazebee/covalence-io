//! Flows and runs over HTTP (flow spec §9).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use covalence_core::component::Component;
use covalence_core::flow::runtime::{
    CancelError, FireError, FireOutcome, Owner, Placement, RunInfo, Runtime,
};
use covalence_core::flow::{self, Flow};
use covalence_core::journal::Origin;
use covalence_core::state::ConnId;
use covalence_core::surface::ComponentId;
use covalence_core::{compose, expr};
use covalence_protocol::plugin::Manifest;
use covalence_protocol::state::PathSegment;
use serde::Deserialize;
use serde_json::{Value as JsonValue, json};

use crate::api::{ApiError, ApiResult};
use crate::entity::{self, EntityKind};
use crate::{App, SaveError, read};

pub(crate) fn routes() -> Router<Arc<App>> {
    Router::new()
        .merge(entity::routes::<Flow>())
        .route("/flows/{id}/run", post(run))
        .route("/flows/{id}/detach", post(detach))
        .route("/runs", get(runs))
        .route("/runs/{run}", get(one_run))
        .route("/runs/{run}/cancel", post(cancel))
        .route("/expressions/evaluate", post(evaluate))
}

/// `POST /flows/{id}/run`'s body: the run's inputs (flow spec §9). An empty body is no inputs.
#[derive(Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RunRequest {
    #[serde(default)]
    inputs: BTreeMap<String, JsonValue>,
}

async fn run(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    body: Option<Json<RunRequest>>,
) -> ApiResult<Json<JsonValue>> {
    let origin = Origin::User {
        user: "api".into(),
        client: "http".into(),
    };
    let inputs = body.map(|Json(b)| b.inputs).unwrap_or_default();
    let fired = app
        .runtime()
        .fire_api(&id, origin, inputs)
        .map_err(|e| match e {
            FireError::UnknownFlow(_) => entity::not_found::<Flow>(&id),
            e @ (FireError::Disabled(_) | FireError::NoApiTrigger(_)) => {
                ApiError::new(StatusCode::CONFLICT, e.to_string())
            }
            FireError::BadInputs(e) => ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, e.message),
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

/// Filters on `GET /runs` and `GET /journal` (composition spec §5).
#[derive(Default, Deserialize)]
pub(crate) struct InstanceFilter {
    /// Inside this instance, at any depth.
    pub instance: Option<String>,
    /// That item of it, with `instance`: the key's text, as in brackets.
    pub key: Option<String>,
    /// Inside any instance of this template, at any depth.
    pub template: Option<String>,
}

impl InstanceFilter {
    pub fn is_empty(&self) -> bool {
        self.instance.is_none() && self.template.is_none()
    }

    pub fn matches(&self, chain: Option<&[compose::Link]>) -> bool {
        if self.is_empty() {
            return true;
        }
        let Some(chain) = chain else { return false };
        let key = self.key.as_deref().map(key_segment);
        self.instance
            .as_deref()
            .is_none_or(|i| compose::chain_has(chain, i, key.as_ref()))
            && self
                .template
                .as_deref()
                .is_none_or(|t| compose::chain_uses(chain, t))
    }
}

/// A key as a query string gives it: a whole number is an index.
fn key_segment(text: &str) -> PathSegment {
    match text.parse::<i64>() {
        Ok(i) => PathSegment::Index(i),
        Err(_) => PathSegment::Key(text.trim_matches('"').to_owned()),
    }
}

#[derive(Deserialize)]
struct RunsQuery {
    flow: Option<String>,
    #[serde(flatten)]
    filter: InstanceFilter,
}

async fn runs(State(app): State<Arc<App>>, Query(query): Query<RunsQuery>) -> Json<Vec<RunInfo>> {
    let runs = app.runtime().runs(query.flow.as_deref());
    Json(
        runs.into_iter()
            .filter(|r| query.filter.matches(r.instance.as_deref()))
            .collect(),
    )
}

/// Turns an instance into plain flows with no link back (composition
/// spec §5): its template's steps copied in, and its params as their own,
/// defaulting to what they were bound to. A `forEach` instance becomes one
/// flow per current item, with the item written in.
async fn detach(State(app): State<Arc<App>>, Path(id): Path<String>) -> ApiResult<Json<JsonValue>> {
    let _saving = app.saving.lock().unwrap_or_else(|e| e.into_inner());
    let flows = read(&app.flows).clone();
    let instance = flows
        .get(&id)
        .ok_or_else(|| entity::not_found::<Flow>(&id))?;
    let template = instance
        .template
        .as_ref()
        .and_then(|t| flows.get(t))
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::CONFLICT,
                format!("flow {:?} isn't an instance of a template", instance.name),
            )
        })?;
    let plain = |item: Option<&compose::Item>| Flow {
        template: None,
        for_each: None,
        bindings: BTreeMap::new(),
        params: compose::detached_params(&template.params, &instance.bindings, item),
        ..instance.instance_of(template)
    };
    let mut detached = Vec::new();
    match &instance.for_each {
        None => detached.push(plain(None)),
        Some(_) => {
            let mut taken: BTreeSet<String> = flows
                .values()
                .filter(|f| f.id != id)
                .map(|f| f.slug.clone())
                .collect();
            for item in app.runtime().items(&id).unwrap_or_default() {
                let key = compose::key_text(&item.key);
                let mut flow = plain(Some(&item));
                flow.id = ulid::Ulid::generate().to_string();
                flow.name = format!("{} {}", instance.name, key.trim_matches('"'));
                flow.slug = flow::derive_slug(&flow.name, &taken);
                taken.insert(flow.slug.clone());
                detached.push(flow);
            }
            // The instance goes; its items are plain flows now.
            app.delete_locked::<Flow>(&id).map_err(ApiError::internal)?;
        }
    }
    for flow in &detached {
        app.put(flow.clone()).map_err(ApiError::internal)?;
    }
    Ok(Json(json!({ "flows": detached })))
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

/// Makes the runtime run `flow` as it should (composition spec §9): an
/// instance with its template's steps, a template by itself unless it's
/// abstract, or a plain flow, owned by its component if it has one
/// (composition spec §2). `flows` is every flow, and `components` every component.
pub(crate) fn place(
    runtime: &Runtime,
    flow: &Flow,
    flows: &BTreeMap<String, Flow>,
    components: &BTreeMap<ComponentId, Component>,
) {
    let template = flow.template.as_ref().and_then(|t| flows.get(t));
    let placement = match &flow.owner {
        // An owned flow whose component is gone doesn't run.
        Some(owner) => components
            .get(owner)
            .and_then(|c| Some(Placement::of(flow, template)?.owned_by(owners(c, components)))),
        None => Placement::of(flow, template),
    };
    match placement {
        Some(p) => runtime.place(p),
        None => {
            if let Err(e) = runtime.remove_flow(&flow.id) {
                tracing::error!(flow = %flow.id, "cannot end a flow that no longer runs: {e}");
            }
        }
    }
}

/// Who runs a flow `component` owns (composition spec §2): the component
/// itself, unless it's an abstract template, each instance of it, and each
/// child of it on a shown component, at any depth (composition spec §8).
fn owners(component: &Component, components: &BTreeMap<ComponentId, Component>) -> Vec<Owner> {
    let itself = (!component.is_abstract()).then(|| Owner::of(component));
    let parts = components
        .values()
        .filter(|c| c.show.is_some() && !c.is_abstract())
        .flat_map(|c| {
            c.parts
                .iter()
                .filter(|p| p.template == component.id)
                .map(move |p| Owner::part(c, p))
        });
    itself
        .into_iter()
        .chain(
            components
                .values()
                .filter(|c| c.template.as_deref() == Some(component.id.as_str()))
                .map(Owner::of),
        )
        .chain(parts)
        .collect()
}

/// Places again every flow owned by one of `owners`, whose owners or homes
/// may have changed.
pub(crate) fn place_owned(
    runtime: &Runtime,
    owners: &BTreeSet<&str>,
    flows: &BTreeMap<String, Flow>,
    components: &BTreeMap<ComponentId, Component>,
) {
    for flow in flows
        .values()
        .filter(|f| f.owner.as_deref().is_some_and(|o| owners.contains(o)))
    {
        place(runtime, flow, flows, components);
    }
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
        // The app's copies, so a batch's owned flow sees the component it creates.
        let components = read(&app.components).clone();
        let surfaces: BTreeSet<String> = read(&app.surface_entities).keys().cloned().collect();
        let mut by_id: BTreeMap<String, Flow> = flows.clone();
        by_id.insert(self.id.clone(), self.clone());
        drop(flows);
        let variables: BTreeMap<String, bool> = read(&app.variables)
            .values()
            .map(|v| (v.slug.clone(), v.is_computed()))
            .collect();
        flow::validate(
            self,
            &flow::Context {
                conns: &by_conn,
                taken_slugs: &taken,
                components: &components,
                surfaces: &surfaces,
                flows: &by_id,
                evaluator: app.evaluator.as_ref(),
                variables: &variables,
            },
        )
    }

    /// An owned flow goes with its component (composition spec §2).
    fn refs(&self) -> Vec<entity::Ref> {
        self.owner
            .iter()
            .map(|component| {
                entity::Ref::new(
                    covalence_core::component::ENTITY,
                    component,
                    "owner",
                    entity::OnDelete::Cascade,
                )
            })
            .collect()
    }

    fn apply(&self, app: &App) -> Result<(), SaveError> {
        let flows = read(&app.flows).clone();
        let components = read(&app.components).clone();
        place(&app.runtime, self, &flows, &components);
        // A template's instances run its new steps and params.
        if !self.is_instance() {
            for instance in flows
                .values()
                .filter(|f| f.template.as_deref() == Some(self.id.as_str()))
            {
                match Placement::of(instance, Some(self)) {
                    Some(p) => app.runtime.place(p),
                    None => {
                        let _ = app.runtime.remove_flow(&instance.id);
                    }
                }
            }
        }
        Ok(())
    }

    /// A template with instances stays until they're detached or deleted
    /// (composition spec §5).
    fn deletable(app: &App, id: &str) -> Result<(), String> {
        let flows = read(&app.flows);
        let instances: Vec<String> = flows
            .values()
            .filter(|f| f.template.as_deref() == Some(id))
            .map(|f| format!("{:?}", f.name))
            .collect();
        if instances.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "it's the template of {}; detach or delete {} first",
                instances.join(", "),
                if instances.len() == 1 { "it" } else { "them" }
            ))
        }
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

impl App {
    /// A journal entry as the API sends it: each link of its instance chain
    /// with the instance's and the template's names as they are now
    /// (composition spec §5). The entry itself keeps only IDs.
    pub(crate) fn with_names(&self, entry: &covalence_core::journal::Entry) -> JsonValue {
        let mut json = serde_json::to_value(entry).unwrap_or_default();
        let Some(links) = json.get_mut("instance").and_then(JsonValue::as_array_mut) else {
            return json;
        };
        let flows = read(&self.flows);
        let components = read(&self.components);
        for link in links {
            for (field, named) in [("instance", "name"), ("template", "templateName")] {
                let id = link.get(field).and_then(JsonValue::as_str);
                // A child is named by its slug: `<container>/<slug>`
                // (composition spec §8).
                let name = id
                    .and_then(|id| flows.get(id).map(|f| f.name.clone()))
                    .or_else(|| id.and_then(|id| components.get(id).map(|c| c.name.clone())))
                    .or_else(|| {
                        id.and_then(|id| id.rsplit_once('/'))
                            .map(|(_, slug)| slug.to_owned())
                    });
                if let (Some(name), Some(obj)) = (name, link.as_object_mut()) {
                    obj.insert(named.into(), JsonValue::String(name));
                }
            }
        }
        json
    }
}

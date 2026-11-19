//! Surfaces, layers and control bindings over HTTP (surfaces spec §9,
//! bindings spec §2). The web surface's WebSocket is in `web_surface`.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::sync::{Arc, RwLock};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use covalence_core::binding::worker::{self, Evaluated};
use covalence_core::binding::{self, ControlBinding, PropValue};
use covalence_core::flow;
use covalence_core::journal::Origin;
use covalence_core::state::ConnId;
use covalence_core::surface::render::Rgb;
use covalence_core::surface::{
    self, ControlDef, Layer, LayerId, OutputCapKind, ShowError, Surface, SurfaceId, Surfaces,
};
use covalence_core::text_path;
use serde::Deserialize;
use serde_json::{Value as JsonValue, json};

use crate::api::{ApiError, ApiResult, saved};
use crate::entity::{self, EntityKind, OnDelete, Ref};
use crate::{App, SaveError, read};

/// The size of a preview's image when the request doesn't give one: the
/// virtual deck's keys.
const PREVIEW_SIZE: u32 = 144;

pub(crate) fn routes() -> Router<Arc<App>> {
    Router::new()
        .merge(entity::routes::<Surface>())
        .merge(entity::routes::<Layer>())
        .merge(entity::routes::<ControlBinding>())
        .route("/surfaces/{id}/current", post(navigate))
        .route("/surfaces/{id}/controls", get(surface_controls))
        .route("/surface/{id}", get(crate::web_surface::upgrade))
        .route("/layers/problems", get(layer_problems))
        .route("/control-bindings/problems", get(binding_problems))
        .route("/control-bindings/preview", post(preview))
}

impl App {
    /// Attached surfaces and what they show.
    pub fn surfaces(&self) -> &Surfaces {
        &self.surfaces
    }

    /// Every surface entity, by ID.
    pub fn surface_entities(&self) -> BTreeMap<SurfaceId, Surface> {
        read(&self.surface_entities).clone()
    }

    pub fn layers(&self) -> BTreeMap<LayerId, Layer> {
        read(&self.layers).clone()
    }

    pub fn control_bindings(&self) -> BTreeMap<String, ControlBinding> {
        read(&self.bindings).clone()
    }

    /// Evaluates a binding once against live state, as the binding worker
    /// would, and draws it. Blocks for up to the binding time limit.
    pub fn preview_binding(
        &self,
        binding: &ControlBinding,
        size: u32,
    ) -> (worker::Preview, Result<Vec<u8>, String>) {
        let aliases = crate::alias_map(&self.conns());
        let preview = worker::preview(
            binding,
            &self.core.shared().state,
            &aliases,
            &self.binding_evaluator,
        );
        let image = self
            .renderer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .render(&preview.look, size, size, "png", 0)
            .map_err(|e| e.to_string());
        (preview, image)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NavigateBody {
    /// `main` when absent.
    #[serde(default)]
    group: Option<String>,
    layer: Option<LayerId>,
}

/// Navigates one of a surface's navigation groups, `main` by default, to a
/// layer or to nothing (surfaces spec §5.3), and answers once the surface has been sent its controls'
/// outputs: `{surface, group, layer}`.
async fn navigate(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Json(body): Json<NavigateBody>,
) -> Response {
    let origin = Origin::User {
        user: "api".into(),
        client: "http".into(),
    };
    let problem = |status: StatusCode, code: &str, message: String| {
        (status, Json(json!({ "error": message, "code": code }))).into_response()
    };
    let group = body.group.as_deref().unwrap_or(surface::MAIN_GROUP);
    match app
        .surfaces()
        .navigate(&id, group, body.layer.as_deref(), origin)
    {
        Ok(()) => {
            app.surfaces().flush().await;
            Json(json!({ "surface": id, "group": group, "layer": body.layer })).into_response()
        }
        Err(e @ ShowError::BadGroup(_)) => {
            problem(StatusCode::UNPROCESSABLE_ENTITY, "invalid", e.to_string())
        }
        Err(e @ ShowError::UnknownSurface(_)) => {
            ApiError::new(StatusCode::NOT_FOUND, e.to_string()).into_response()
        }
        Err(e @ ShowError::UnknownLayer(_)) => problem(
            StatusCode::UNPROCESSABLE_ENTITY,
            "unknownLayer",
            e.to_string(),
        ),
        Err(ShowError::DoesNotFit { layer, missing }) => {
            let message = ShowError::DoesNotFit {
                layer,
                missing: missing.clone(),
            }
            .to_string();
            let body = json!({ "error": message, "code": "doesNotFit", "controls": missing });
            (StatusCode::CONFLICT, Json(body)).into_response()
        }
        Err(ShowError::Journal(e)) => ApiError::internal(e).into_response(),
    }
}

/// The controls a surface declared when it last attached, for "New layer".
async fn surface_controls(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<ControlDef>>> {
    if !app.surface_entities().contains_key(&id) {
        return Err(entity::not_found::<Surface>(&id));
    }
    app.surfaces().controls(&id).map(Json).ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            format!("surface {id:?} hasn't attached since the server started"),
        )
    })
}

/// Each layer field that failed when last evaluated (surfaces spec §5.2):
/// `[{layer, field, message}]`, `field` being `on`, `when` or `rank`.
async fn layer_problems(State(app): State<Arc<App>>) -> Json<Vec<JsonValue>> {
    Json(
        app.surfaces()
            .layer_problems()
            .into_iter()
            .map(|((layer, field), message)| {
                json!({ "layer": layer, "field": field, "message": message })
            })
            .collect(),
    )
}

async fn binding_problems(State(app): State<Arc<App>>) -> Json<Vec<JsonValue>> {
    Json(
        app.surfaces()
            .binding_problems()
            .into_iter()
            .map(|((binding, prop), message)| {
                json!({ "binding": binding, "prop": prop, "message": message })
            })
            .collect(),
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviewBody {
    /// A control binding, saved or not.
    binding: JsonValue,
    /// The image's width and height.
    #[serde(default)]
    size: Option<u32>,
}

/// The editor's live preview (bindings spec §4): each property's value,
/// error, compiled expression and reads, the look they make and the key
/// image. Never writes or journals.
async fn preview(State(app): State<Arc<App>>, Json(body): Json<PreviewBody>) -> Response {
    let binding = match ControlBinding::parse(&body.binding) {
        Ok(binding) => binding,
        Err(errors) => {
            return saved::<ControlBinding>(
                Err(SaveError::invalid(errors)),
                "binding",
                StatusCode::OK,
            );
        }
    };
    let size = body.size.unwrap_or(PREVIEW_SIZE);
    // Up to the time limit of work per `expr` property, off the async workers.
    let previewing = app.clone();
    let previewed = tokio::task::spawn_blocking(move || {
        let previewed = previewing.preview_binding(&binding, size);
        (binding, previewed)
    })
    .await;
    let (binding, (preview, image)) = match previewed {
        Ok(previewed) => previewed,
        Err(e) => return ApiError::internal(e).into_response(),
    };
    let image = match image {
        Ok(image) => image,
        Err(e) => return ApiError::new(StatusCode::BAD_REQUEST, e).into_response(),
    };

    let roots = app.state_roots();
    let props: serde_json::Map<String, JsonValue> = preview
        .props
        .iter()
        .map(|(prop, evaluated)| {
            let b = &binding.props[prop];
            (
                prop.as_str().to_owned(),
                prop_json(evaluated, b.to_expr(&roots), &roots),
            )
        })
        .collect();
    let look = &preview.look;
    Json(json!({
        "look": {
            "label": look.label,
            "textColor": hex(look.text_color),
            "background": hex(look.background),
            "stale": look.stale,
        },
        "props": props,
        "image": BASE64.encode(image),
    }))
    .into_response()
}

/// `{value?, error?, expr?, reads}` for one property.
fn prop_json(
    evaluated: &Evaluated,
    expr: Option<String>,
    roots: &BTreeMap<ConnId, String>,
) -> JsonValue {
    let mut out = serde_json::Map::new();
    match &evaluated.value {
        Some(PropValue::Text(text)) => {
            out.insert("value".into(), json!(text));
        }
        Some(PropValue::Color(color)) => {
            out.insert("value".into(), json!(hex(*color)));
        }
        Some(PropValue::Level(level)) => {
            out.insert("value".into(), json!(level));
        }
        None => {}
    }
    if let Some(error) = &evaluated.error {
        out.insert("error".into(), json!(error));
    }
    if let Some(expr) = expr {
        out.insert("expr".into(), json!(expr));
    }
    let reads: Vec<String> = evaluated
        .reads
        .iter()
        .filter_map(|r| Some(text_path::format(roots.get(&r.conn)?, &r.path)))
        .collect();
    out.insert("reads".into(), json!(reads));
    JsonValue::Object(out)
}

fn hex(Rgb(r, g, b): Rgb) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

impl EntityKind for Surface {
    const TYPE: &'static str = surface::SURFACE_ENTITY;
    const NAME: &'static str = "surface";
    const PATH: &'static str = "/surfaces";
    type Context = ();

    fn parse(json: &JsonValue) -> Result<Surface, Vec<flow::Problem>> {
        Surface::parse(json)
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn id_mut(&mut self) -> &mut String {
        &mut self.id
    }

    fn all(app: &App) -> &RwLock<BTreeMap<String, Surface>> {
        &app.surface_entities
    }

    fn context(_: &App) -> impl Future<Output = ()> + Send {
        std::future::ready(())
    }

    fn check(&mut self, app: &App, _: &()) -> flow::Report {
        let taken: BTreeSet<String> = read(&app.surface_entities)
            .values()
            .filter(|s| s.id != self.id)
            .map(|s| s.slug.clone())
            .collect();
        if self.slug.is_empty() {
            self.slug = flow::derive_slug_for("surface", &self.name, &taken);
        }
        surface::validate_surface(self, &taken, &app.surfaces.layers())
    }

    fn refs(&self) -> Vec<Ref> {
        self.start_layer
            .iter()
            .map(|layer| Ref::new(surface::LAYER_ENTITY, layer, "startLayer", OnDelete::Clear))
            .collect()
    }

    fn clear_ref(&mut self, field: &str) {
        if field == "startLayer" {
            self.start_layer = None;
        }
    }

    fn apply(&self, app: &App) -> Result<(), SaveError> {
        let renamed = read(&app.surface_entities)
            .get(&self.id)
            .is_some_and(|old| old.slug != self.slug);
        app.surfaces.set_surface(self.clone())?;
        if renamed {
            app.surface_renamed(self);
        }
        Ok(())
    }

    /// Detaches it, and forgets the layer it showed and its helpers.
    fn unapply(id: &str, app: &App) -> Result<(), SaveError> {
        app.delete_entity(crate::SHOWN_ENTITY, id)?;
        app.surfaces.remove_surface(id)?;
        app.forget_surface(id);
        Ok(())
    }

    fn settle(app: &App) -> impl Future<Output = ()> + Send {
        app.surfaces.flush()
    }
}

impl EntityKind for Layer {
    const TYPE: &'static str = surface::LAYER_ENTITY;
    const NAME: &'static str = "layer";
    const PATH: &'static str = "/layers";
    type Context = ();

    fn parse(json: &JsonValue) -> Result<Layer, Vec<flow::Problem>> {
        Layer::parse(json)
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn id_mut(&mut self) -> &mut String {
        &mut self.id
    }

    fn all(app: &App) -> &RwLock<BTreeMap<String, Layer>> {
        &app.layers
    }

    fn context(_: &App) -> impl Future<Output = ()> + Send {
        std::future::ready(())
    }

    fn check(&mut self, app: &App, _: &()) -> flow::Report {
        let taken: BTreeSet<String> = read(&app.layers)
            .values()
            .filter(|p| p.id != self.id)
            .map(|p| p.slug.clone())
            .collect();
        if self.slug.is_empty() {
            self.slug = flow::derive_slug_for("layer", &self.name, &taken);
        }
        let mut bound: BTreeMap<String, BTreeSet<OutputCapKind>> = BTreeMap::new();
        for b in read(&app.bindings).values().filter(|b| b.layer == self.id) {
            bound
                .entry(b.control.clone())
                .or_default()
                .extend(b.outputs());
        }
        let surfaces: BTreeSet<String> = read(&app.surface_entities).keys().cloned().collect();
        surface::validate_layer(
            self,
            &surface::LayerContext {
                taken_slugs: &taken,
                bound: &bound,
                surfaces: &surfaces,
                evaluator: &*app.evaluator,
                others: &read(&app.layers),
            },
        )
    }

    fn apply(&self, app: &App) -> Result<(), SaveError> {
        app.surfaces.set_layer(self.clone())?;
        // Helpers live under the layer's slug, which may have changed.
        let bindings: Vec<ControlBinding> = read(&app.bindings)
            .values()
            .filter(|b| b.layer == self.id)
            .cloned()
            .collect();
        for b in bindings {
            app.sync_helpers_on(&b, &self.slug);
        }
        // So do its flows' homes (composition spec §2).
        let flows = read(&app.flows).clone();
        let mut layers = read(&app.layers).clone();
        layers.insert(self.id.clone(), self.clone());
        for flow in flows
            .values()
            .filter(|f| f.owner.as_deref() == Some(self.id.as_str()))
        {
            crate::flows::place(&app.runtime, flow, &flows, &layers);
        }
        Ok(())
    }

    fn unapply(id: &str, app: &App) -> Result<(), SaveError> {
        app.surfaces.remove_layer(id)?;
        app.forget_layer(id);
        Ok(())
    }

    fn settle(app: &App) -> impl Future<Output = ()> + Send {
        app.surfaces.flush()
    }
}

/// Flows whose `control` triggers name a deleted layer keep them, and are
/// told when they're next saved, so flows declare no references to layers.
impl EntityKind for ControlBinding {
    const TYPE: &'static str = binding::ENTITY;
    const NAME: &'static str = "binding";
    const PATH: &'static str = "/control-bindings";
    type Context = ();

    fn parse(json: &JsonValue) -> Result<ControlBinding, Vec<flow::Problem>> {
        ControlBinding::parse(json)
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn id_mut(&mut self) -> &mut String {
        &mut self.id
    }

    fn all(app: &App) -> &RwLock<BTreeMap<String, ControlBinding>> {
        &app.bindings
    }

    fn context(_: &App) -> impl Future<Output = ()> + Send {
        std::future::ready(())
    }

    fn check(&mut self, app: &App, _: &()) -> flow::Report {
        let taken: BTreeSet<(LayerId, String)> = read(&app.bindings)
            .values()
            .filter(|b| b.id != self.id)
            .map(|b| (b.layer.clone(), b.control.clone()))
            .collect();
        let conns = app.conn_ids();
        binding::validate(
            self,
            &binding::Context {
                layers: &app.surfaces.layers(),
                conns: &conns,
                taken: &taken,
                evaluator: app.evaluator.as_ref(),
            },
        )
    }

    /// A binding goes with its layer.
    fn refs(&self) -> Vec<Ref> {
        vec![Ref::new(
            surface::LAYER_ENTITY,
            &self.layer,
            "layer",
            OnDelete::Cascade,
        )]
    }

    fn apply(&self, app: &App) -> Result<(), SaveError> {
        app.surfaces.set_binding(self.clone());
        app.sync_helpers(self);
        Ok(())
    }

    fn unapply(id: &str, app: &App) -> Result<(), SaveError> {
        app.surfaces.remove_binding(id);
        app.dataflow
            .replace(&binding::helper_cell_id(id, ""), Vec::new(), None);
        Ok(())
    }

    fn settle(app: &App) -> impl Future<Output = ()> + Send {
        app.surfaces.flush()
    }

    /// `?layer=<id>` lists one layer's bindings.
    fn listed(&self, query: &BTreeMap<String, String>) -> bool {
        query.get("layer").is_none_or(|layer| *layer == self.layer)
    }
}

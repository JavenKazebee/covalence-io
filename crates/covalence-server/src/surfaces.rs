//! Surfaces, pages and control bindings over HTTP (surfaces spec §5,
//! bindings spec §4). The web surface's WebSocket is in `web_surface`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use axum::extract::{Path, Query, State};
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
use covalence_core::surface::{self, Page, PageId, ShowError, Surface, SurfaceId, Surfaces};
use covalence_core::text_path;
use serde::Deserialize;
use serde_json::{Value as JsonValue, json};

use crate::api::{ApiError, ApiResult, saved};
use crate::{App, SaveError, SaveResult, assign_id, read, write};

/// The size of a preview's image when the request doesn't give one: the
/// virtual deck's keys.
const PREVIEW_SIZE: u32 = 144;

pub(crate) fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/surfaces", get(list_surfaces).post(create_surface))
        .route(
            "/surfaces/{id}",
            get(read_surface)
                .put(replace_surface)
                .delete(remove_surface),
        )
        .route("/surfaces/{id}/page", post(show_page))
        .route("/surface/{id}", get(crate::web_surface::upgrade))
        .route("/pages", get(list_pages).post(create_page))
        .route(
            "/pages/{id}",
            get(read_page).put(replace_page).delete(remove_page),
        )
        .route("/control-bindings", get(list_bindings).post(create_binding))
        .route("/control-bindings/problems", get(binding_problems))
        .route("/control-bindings/preview", post(preview))
        .route(
            "/control-bindings/{id}",
            get(read_binding)
                .put(replace_binding)
                .delete(remove_binding),
        )
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

    pub fn pages(&self) -> BTreeMap<PageId, Page> {
        read(&self.pages).clone()
    }

    pub fn control_bindings(&self) -> BTreeMap<String, ControlBinding> {
        read(&self.bindings).clone()
    }

    /// Parses, checks and stores a surface from the API. `id` is the surface
    /// being replaced, or `None` to create one.
    pub fn save_surface(&self, json: &JsonValue, id: Option<&str>) -> SaveResult<Surface> {
        let mut surface = Surface::parse(json).map_err(SaveError::invalid)?;
        assign_id(&mut surface.id, id, |id| {
            read(&self.surface_entities).contains_key(id)
        })?;
        let _saving = self.saving.lock().unwrap_or_else(|e| e.into_inner());
        let taken: BTreeSet<String> = read(&self.surface_entities)
            .values()
            .filter(|s| s.id != surface.id)
            .map(|s| s.slug.clone())
            .collect();
        if surface.slug.is_empty() {
            surface.slug = flow::derive_slug_for("surface", &surface.name, &taken);
        }
        let report = surface::validate_surface(&surface, &taken, &self.surfaces.layouts());
        if !report.is_ok() {
            return Err(SaveError::Invalid {
                errors: report.errors,
                warnings: report.warnings,
            });
        }
        self.put_entity(surface::SURFACE_ENTITY, &surface.id, &surface)?;
        self.surfaces.set_surface(surface.clone())?;
        write(&self.surface_entities).insert(surface.id.clone(), surface.clone());
        Ok((surface, report.warnings))
    }

    /// Removes a surface, detaching it, and returns it if there was one.
    pub fn delete_surface(&self, id: &str) -> Result<Option<Surface>, SaveError> {
        let _saving = self.saving.lock().unwrap_or_else(|e| e.into_inner());
        self.delete_entity(surface::SURFACE_ENTITY, id)?;
        // Forgotten before it's detached, so its socket can tell why it closed.
        let removed = write(&self.surface_entities).remove(id);
        self.surfaces.remove_surface(id)?;
        Ok(removed)
    }

    /// Parses, checks and stores a page from the API.
    pub fn save_page(&self, json: &JsonValue, id: Option<&str>) -> SaveResult<Page> {
        let mut page = Page::parse(json).map_err(SaveError::invalid)?;
        assign_id(&mut page.id, id, |id| read(&self.pages).contains_key(id))?;
        let _saving = self.saving.lock().unwrap_or_else(|e| e.into_inner());
        let taken: BTreeSet<String> = read(&self.pages)
            .values()
            .filter(|p| p.id != page.id)
            .map(|p| p.slug.clone())
            .collect();
        if page.slug.is_empty() {
            page.slug = flow::derive_slug_for("page", &page.name, &taken);
        }
        let bound: BTreeSet<String> = read(&self.bindings)
            .values()
            .filter(|b| b.page == page.id)
            .map(|b| b.control.clone())
            .collect();
        let report = surface::validate_page(&page, &taken, &bound);
        if !report.is_ok() {
            return Err(SaveError::Invalid {
                errors: report.errors,
                warnings: report.warnings,
            });
        }
        self.put_entity(surface::PAGE_ENTITY, &page.id, &page)?;
        self.surfaces.set_page(page.clone())?;
        write(&self.pages).insert(page.id.clone(), page.clone());
        Ok((page, report.warnings))
    }

    /// Removes a page, its bindings, and any surface's use of it as a start
    /// page. Flows whose `gesture` triggers name it keep them, and are told
    /// when they're next saved.
    pub fn delete_page(&self, id: &str) -> Result<Option<Page>, SaveError> {
        let _saving = self.saving.lock().unwrap_or_else(|e| e.into_inner());
        let bound: Vec<String> = read(&self.bindings)
            .values()
            .filter(|b| b.page == id)
            .map(|b| b.id.clone())
            .collect();
        for binding in bound {
            self.delete_entity(binding::ENTITY, &binding)?;
            self.surfaces.remove_binding(&binding);
            write(&self.bindings).remove(&binding);
        }
        let starting: Vec<Surface> = read(&self.surface_entities)
            .values()
            .filter(|s| s.start_page.as_deref() == Some(id))
            .cloned()
            .collect();
        for mut surface in starting {
            surface.start_page = None;
            self.put_entity(surface::SURFACE_ENTITY, &surface.id, &surface)?;
            self.surfaces.set_surface(surface.clone())?;
            write(&self.surface_entities).insert(surface.id.clone(), surface);
        }
        self.delete_entity(surface::PAGE_ENTITY, id)?;
        self.surfaces.remove_page(id)?;
        Ok(write(&self.pages).remove(id))
    }

    /// Parses, checks and stores a control binding from the API.
    pub fn save_binding(&self, json: &JsonValue, id: Option<&str>) -> SaveResult<ControlBinding> {
        let mut binding = ControlBinding::parse(json).map_err(SaveError::invalid)?;
        assign_id(&mut binding.id, id, |id| {
            read(&self.bindings).contains_key(id)
        })?;
        let _saving = self.saving.lock().unwrap_or_else(|e| e.into_inner());
        let taken: BTreeSet<(PageId, String)> = read(&self.bindings)
            .values()
            .filter(|b| b.id != binding.id)
            .map(|b| (b.page.clone(), b.control.clone()))
            .collect();
        let conns: BTreeSet<ConnId> = self.conns().keys().cloned().collect();
        let report = binding::validate(
            &binding,
            &binding::Context {
                pages: &self.surfaces.layouts(),
                conns: &conns,
                taken: &taken,
                evaluator: self.evaluator.as_ref(),
            },
        );
        if !report.is_ok() {
            return Err(SaveError::Invalid {
                errors: report.errors,
                warnings: report.warnings,
            });
        }
        self.put_entity(binding::ENTITY, &binding.id, &binding)?;
        self.surfaces.set_binding(binding.clone());
        write(&self.bindings).insert(binding.id.clone(), binding.clone());
        Ok((binding, report.warnings))
    }

    pub fn delete_binding(&self, id: &str) -> Result<Option<ControlBinding>, SaveError> {
        let _saving = self.saving.lock().unwrap_or_else(|e| e.into_inner());
        self.delete_entity(binding::ENTITY, id)?;
        self.surfaces.remove_binding(id);
        Ok(write(&self.bindings).remove(id))
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

async fn list_surfaces(State(app): State<Arc<App>>) -> Json<Vec<Surface>> {
    Json(app.surface_entities().into_values().collect())
}

async fn read_surface(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Surface>> {
    app.surface_entities()
        .remove(&id)
        .map(Json)
        .ok_or_else(|| not_found("surface", &id))
}

async fn create_surface(State(app): State<Arc<App>>, Json(body): Json<JsonValue>) -> Response {
    saved(
        app.save_surface(&body, None),
        "surface",
        StatusCode::CREATED,
    )
}

async fn replace_surface(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Json(body): Json<JsonValue>,
) -> Response {
    let result = app.save_surface(&body, Some(&id));
    app.surfaces().flush().await;
    saved(result, "surface", StatusCode::OK)
}

async fn remove_surface(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    match app.delete_surface(&id).map_err(ApiError::internal)? {
        Some(_) => Ok(StatusCode::NO_CONTENT),
        None => Err(not_found("surface", &id)),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ShowBody {
    page: Option<PageId>,
}

/// Shows a page on a surface (surfaces spec §5) and answers once the
/// surface has been sent its keys: `{surface, page}`.
async fn show_page(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Json(body): Json<ShowBody>,
) -> Response {
    let origin = Origin::User {
        user: "api".into(),
        client: "http".into(),
    };
    let problem = |status: StatusCode, code: &str, message: String| {
        (status, Json(json!({ "error": message, "code": code }))).into_response()
    };
    match app.surfaces().show(&id, body.page.as_deref(), origin) {
        Ok(()) => {
            app.surfaces().flush().await;
            Json(json!({ "surface": id, "page": body.page })).into_response()
        }
        Err(e @ ShowError::UnknownSurface(_)) => {
            ApiError::new(StatusCode::NOT_FOUND, e.to_string()).into_response()
        }
        Err(e @ ShowError::UnknownPage(_)) => problem(
            StatusCode::UNPROCESSABLE_ENTITY,
            "unknownPage",
            e.to_string(),
        ),
        Err(ShowError::DoesNotFit { page, missing }) => {
            let message = ShowError::DoesNotFit {
                page,
                missing: missing.clone(),
            }
            .to_string();
            let body = json!({ "error": message, "code": "doesNotFit", "controls": missing });
            (StatusCode::CONFLICT, Json(body)).into_response()
        }
        Err(ShowError::Journal(e)) => ApiError::internal(e).into_response(),
    }
}

async fn list_pages(State(app): State<Arc<App>>) -> Json<Vec<Page>> {
    Json(app.pages().into_values().collect())
}

async fn read_page(State(app): State<Arc<App>>, Path(id): Path<String>) -> ApiResult<Json<Page>> {
    app.pages()
        .remove(&id)
        .map(Json)
        .ok_or_else(|| not_found("page", &id))
}

async fn create_page(State(app): State<Arc<App>>, Json(body): Json<JsonValue>) -> Response {
    saved(app.save_page(&body, None), "page", StatusCode::CREATED)
}

async fn replace_page(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Json(body): Json<JsonValue>,
) -> Response {
    let result = app.save_page(&body, Some(&id));
    app.surfaces().flush().await;
    saved(result, "page", StatusCode::OK)
}

async fn remove_page(State(app): State<Arc<App>>, Path(id): Path<String>) -> ApiResult<StatusCode> {
    let removed = app.delete_page(&id).map_err(ApiError::internal)?;
    app.surfaces().flush().await;
    match removed {
        Some(_) => Ok(StatusCode::NO_CONTENT),
        None => Err(not_found("page", &id)),
    }
}

#[derive(Deserialize)]
struct BindingsQuery {
    page: Option<PageId>,
}

async fn list_bindings(
    State(app): State<Arc<App>>,
    Query(query): Query<BindingsQuery>,
) -> Json<Vec<ControlBinding>> {
    Json(
        app.control_bindings()
            .into_values()
            .filter(|b| query.page.as_ref().is_none_or(|p| *p == b.page))
            .collect(),
    )
}

async fn read_binding(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
) -> ApiResult<Json<ControlBinding>> {
    app.control_bindings()
        .remove(&id)
        .map(Json)
        .ok_or_else(|| not_found("binding", &id))
}

/// Answers once surfaces showing the binding's page have been redrawn.
async fn create_binding(State(app): State<Arc<App>>, Json(body): Json<JsonValue>) -> Response {
    let result = app.save_binding(&body, None);
    app.surfaces().flush().await;
    saved(result, "binding", StatusCode::CREATED)
}

async fn replace_binding(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Json(body): Json<JsonValue>,
) -> Response {
    let result = app.save_binding(&body, Some(&id));
    app.surfaces().flush().await;
    saved(result, "binding", StatusCode::OK)
}

async fn remove_binding(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let removed = app.delete_binding(&id).map_err(ApiError::internal)?;
    app.surfaces().flush().await;
    match removed {
        Some(_) => Ok(StatusCode::NO_CONTENT),
        None => Err(not_found("binding", &id)),
    }
}

/// `[{binding, prop, message}]`: each binding property that failed when it
/// was last evaluated, for the editor.
async fn binding_problems(State(app): State<Arc<App>>) -> Json<Vec<JsonValue>> {
    Json(
        app.surfaces()
            .binding_problems()
            .into_iter()
            .map(|((binding, prop), message)| {
                json!({ "binding": binding, "prop": prop.as_str(), "message": message })
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

fn not_found(kind: &str, id: &str) -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, format!("no {kind} {id:?}"))
}

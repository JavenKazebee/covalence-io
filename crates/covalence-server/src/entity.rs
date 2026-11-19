//! Project entity types, handled one way.
//!
//! Each type implements [`EntityKind`]: how to parse it, check it against
//! the project, apply it to the running core and take it out again, and
//! which other entities it refers to. The rest is shared:
//!
//! - [`App::save`] parses, gives the entity its ID, checks it under the
//!   saving lock, stores it and applies it.
//! - [`App::put`] stores and applies one that's already checked.
//! - [`App::delete`] deletes it, after deleting or clearing whatever refers
//!   to it, as each reference says ([`OnDelete`]).
//! - [`routes`] serves the five routes every type has: list and create on
//!   its path, and read, replace and delete on `<path>/{id}`. Answers are
//!   `{<name>, warnings}`, or `422 {errors, warnings}` (flow spec §9).
//!
//! A new type implements the trait and is added to [`KINDS`], so deletes
//! elsewhere reach it.

use std::collections::BTreeMap;
use std::future::Future;
use std::marker::PhantomData;
use std::sync::{Arc, RwLock};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::get;
use axum::{Json, Router};
use covalence_core::component::Component;
use covalence_core::flow::{Flow, Problem, Report};
use covalence_core::surface::Surface;
use covalence_core::variable::Variable;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value as JsonValue;

use crate::api::{ApiError, ApiResult, saved};
use crate::{App, SaveError, SaveResult, assign_id, read, write};

/// A project entity type the server stores, checks and serves.
pub trait EntityKind: Clone + Serialize + DeserializeOwned + Send + Sync + 'static {
    /// Its type in the store, and in references to it.
    const TYPE: &'static str;
    /// Its key in API answers (`{surface, warnings}`) and its name in errors.
    const NAME: &'static str;
    /// Its collection's path: `/surfaces`.
    const PATH: &'static str;

    /// What checking needs that has to be fetched before the saving lock is
    /// taken, such as plugin manifests.
    type Context: Send;

    fn parse(json: &JsonValue) -> Result<Self, Vec<Problem>>;
    fn id(&self) -> &str;
    fn id_mut(&mut self) -> &mut String;

    /// The app's copy of every entity of this type, by ID.
    fn all(app: &App) -> &RwLock<BTreeMap<String, Self>>;

    fn context(app: &App) -> impl Future<Output = Self::Context> + Send;

    /// Fills in what's derived (a slug from the name) and checks the entity
    /// against the project. Runs under the saving lock, so what it reads of
    /// other entities can't change before the entity is stored.
    fn check(&mut self, app: &App, ctx: &Self::Context) -> Report;

    /// The entities this one refers to, and what happens to it when one of
    /// them is deleted.
    fn refs(&self) -> Vec<Ref> {
        Vec::new()
    }

    /// Clears a reference named by a [`Ref`] with [`OnDelete::Clear`].
    fn clear_ref(&mut self, field: &str) {
        let _ = field;
    }

    /// Makes the running core match the entity, once it's stored.
    fn apply(&self, app: &App) -> Result<(), SaveError>;

    /// Takes the entity out of the running core, once it's deleted from the
    /// store and the app's copy.
    fn unapply(id: &str, app: &App) -> Result<(), SaveError>;

    /// Whether it can be deleted now: `Err` says why not.
    fn deletable(app: &App, id: &str) -> Result<(), String> {
        let _ = (app, id);
        Ok(())
    }

    /// Waits until the core has acted on a change, before the API answers.
    fn settle(app: &App) -> impl Future<Output = ()> + Send {
        let _ = app;
        std::future::ready(())
    }

    /// Whether it belongs in a list filtered by the query string.
    fn listed(&self, query: &BTreeMap<String, String>) -> bool {
        let _ = query;
        true
    }
}

/// A reference from one entity to another.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ref {
    /// The referenced entity's [`EntityKind::TYPE`].
    pub kind: &'static str,
    pub id: String,
    /// The field that holds it, for [`EntityKind::clear_ref`].
    pub field: &'static str,
    pub on_delete: OnDelete,
}

/// What happens to an entity when one it refers to is deleted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnDelete {
    /// It's deleted too.
    Cascade,
    /// The reference is cleared: a surface whose start component goes has none.
    Clear,
}

impl Ref {
    pub fn new(
        kind: &'static str,
        id: impl Into<String>,
        field: &'static str,
        on_delete: OnDelete,
    ) -> Ref {
        Ref {
            kind,
            id: id.into(),
            field,
            on_delete,
        }
    }
}

/// Every entity type, so a delete can find what refers to the entity.
const KINDS: &[&dyn Referrers] = &[
    &Of::<Surface>(PhantomData),
    &Of::<Component>(PhantomData),
    &Of::<Flow>(PhantomData),
    &Of::<Variable>(PhantomData),
];

/// One entity type, without its type parameter, so they fit in a list.
trait Referrers: Sync {
    /// Deletes or clears this type's references to an entity being deleted.
    /// The caller holds the saving lock.
    fn release(&self, app: &App, kind: &str, id: &str) -> Result<(), SaveError>;
}

struct Of<K>(PhantomData<fn() -> K>);

impl<K: EntityKind> Referrers for Of<K> {
    fn release(&self, app: &App, kind: &str, id: &str) -> Result<(), SaveError> {
        let referring: Vec<(K, Vec<Ref>)> = read(K::all(app))
            .values()
            .filter_map(|entity| {
                let refs: Vec<Ref> = entity
                    .refs()
                    .into_iter()
                    .filter(|r| r.kind == kind && r.id == id)
                    .collect();
                (!refs.is_empty()).then(|| (entity.clone(), refs))
            })
            .collect();
        for (mut entity, refs) in referring {
            if refs.iter().any(|r| r.on_delete == OnDelete::Cascade) {
                app.delete_locked::<K>(entity.id())?;
            } else {
                for r in &refs {
                    entity.clear_ref(r.field);
                }
                app.put(entity)?;
            }
        }
        Ok(())
    }
}

impl App {
    /// Parses, checks and stores an entity from the API, and applies it.
    /// `id` is the entity being replaced, or `None` to create one.
    pub async fn save<K: EntityKind>(&self, json: &JsonValue, id: Option<&str>) -> SaveResult<K> {
        let mut entity = K::parse(json).map_err(SaveError::invalid)?;
        assign_id(entity.id_mut(), id, |id| {
            read(K::all(self)).contains_key(id)
        })?;
        let ctx = K::context(self).await;
        let _saving = self.saving.lock().unwrap_or_else(|e| e.into_inner());
        let report = entity.check(self, &ctx);
        if !report.is_ok() {
            return Err(SaveError::Invalid {
                errors: report.errors,
                warnings: report.warnings,
            });
        }
        self.put(entity.clone())?;
        Ok((entity, report.warnings))
    }

    /// Stores an entity, replacing any with its ID, and applies it. The
    /// caller has checked it.
    pub fn put<K: EntityKind>(&self, entity: K) -> Result<(), SaveError> {
        self.put_entity(K::TYPE, entity.id(), &entity)?;
        entity.apply(self)?;
        write(K::all(self)).insert(entity.id().to_owned(), entity);
        Ok(())
    }

    /// Deletes an entity, and deletes or clears what refers to it. Returns
    /// it if there was one.
    pub fn delete<K: EntityKind>(&self, id: &str) -> Result<Option<K>, SaveError> {
        let _saving = self.saving.lock().unwrap_or_else(|e| e.into_inner());
        K::deletable(self, id).map_err(SaveError::Conflict)?;
        self.delete_locked::<K>(id)
    }

    pub(crate) fn delete_locked<K: EntityKind>(&self, id: &str) -> Result<Option<K>, SaveError> {
        if !read(K::all(self)).contains_key(id) {
            return Ok(None);
        }
        for kind in KINDS {
            kind.release(self, K::TYPE, id)?;
        }
        self.delete_entity(K::TYPE, id)?;
        // Forgotten before it's taken out of the core, so whatever closes
        // because of it can tell why.
        let removed = write(K::all(self)).remove(id);
        K::unapply(id, self)?;
        Ok(removed)
    }

    /// Every entity of a type, by ID.
    pub fn entities<K: EntityKind>(&self) -> BTreeMap<String, K> {
        read(K::all(self)).clone()
    }
}

/// The five routes of an entity type.
pub(crate) fn routes<K: EntityKind>() -> Router<Arc<App>> {
    Router::new()
        .route(K::PATH, get(list::<K>).post(create::<K>))
        .route(
            &format!("{}/{{id}}", K::PATH),
            get(one::<K>).put(replace::<K>).delete(remove::<K>),
        )
}

async fn list<K: EntityKind>(
    State(app): State<Arc<App>>,
    Query(query): Query<BTreeMap<String, String>>,
) -> Json<Vec<K>> {
    Json(
        read(K::all(&app))
            .values()
            .filter(|e| e.listed(&query))
            .cloned()
            .collect(),
    )
}

async fn one<K: EntityKind>(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
) -> ApiResult<Json<K>> {
    read(K::all(&app))
        .get(&id)
        .cloned()
        .map(Json)
        .ok_or_else(|| not_found::<K>(&id))
}

async fn create<K: EntityKind>(
    State(app): State<Arc<App>>,
    Json(body): Json<JsonValue>,
) -> Response {
    let result = app.save::<K>(&body, None).await;
    K::settle(&app).await;
    saved(result, K::NAME, StatusCode::CREATED)
}

async fn replace<K: EntityKind>(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Json(body): Json<JsonValue>,
) -> Response {
    let result = app.save::<K>(&body, Some(&id)).await;
    K::settle(&app).await;
    saved(result, K::NAME, StatusCode::OK)
}

async fn remove<K: EntityKind>(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let removed = app.delete::<K>(&id).map_err(|e| match e {
        SaveError::Conflict(why) => {
            ApiError::new(StatusCode::CONFLICT, format!("can't delete it: {why}"))
        }
        e => ApiError::internal(e),
    })?;
    K::settle(&app).await;
    match removed {
        Some(_) => Ok(StatusCode::NO_CONTENT),
        None => Err(not_found::<K>(&id)),
    }
}

pub(crate) fn not_found<K: EntityKind>(id: &str) -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, format!("no {} {id:?}", K::NAME))
}

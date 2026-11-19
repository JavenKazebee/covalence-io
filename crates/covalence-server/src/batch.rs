//! `POST /batch`: edits across entities, saved as one (state spec §7.6).
//!
//! The body is `{ops: [...]}`, each `{op: "create", type, body}`,
//! `{op: "update", type, id, body}` or `{op: "delete", type, id}`. Under the
//! saving lock every op is staged in the app's copies, then each created or
//! updated entity is checked against the project as it is with all of them,
//! so a template and its instances can change together. Any error puts the
//! copies back and answers `422 {errors, warnings}`, each problem's path
//! starting `["ops", <index>]`; nothing is stored or applied. Otherwise the
//! copies are put back and the ops are stored and applied in order, and the
//! answer is `{results: [{type, id, entity?}], warnings}`.
//!
//! A create may give its entity a ULID of its own as `body.id`, so later ops
//! in the batch can refer to it.
//!
//! Checks that read the running core rather than the app's copies (a flow's
//! layers, for one) see what's applied already, not the rest of the batch.

use std::any::Any;
use std::collections::BTreeMap;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use covalence_core::component::Component;
use covalence_core::flow::{Code, Flow, Problem};
use covalence_core::surface::Surface;
use covalence_core::variable::Variable;
use covalence_protocol::state::PathSegment;
use serde::Deserialize;
use serde_json::{Value as JsonValue, json};

use crate::api::ApiError;
use crate::entity::EntityKind;
use crate::{App, SaveError, assign_id, read, write};

pub(crate) fn routes() -> Router<Arc<App>> {
    Router::new().route("/batch", post(batch))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    ops: Vec<Op>,
}

#[derive(Clone, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", deny_unknown_fields)]
enum Op {
    Create {
        #[serde(rename = "type")]
        kind: String,
        body: JsonValue,
    },
    Update {
        #[serde(rename = "type")]
        kind: String,
        id: String,
        body: JsonValue,
    },
    Delete {
        #[serde(rename = "type")]
        kind: String,
        id: String,
    },
}

impl Op {
    fn kind(&self) -> &str {
        match self {
            Op::Create { kind, .. } | Op::Update { kind, .. } | Op::Delete { kind, .. } => kind,
        }
    }
}

type Context = Box<dyn Any + Send + Sync>;
type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// One entity type, as a batch handles it.
trait Batched: Sync {
    fn kind(&self) -> &'static str;
    fn context<'a>(&'a self, app: &'a App) -> BoxFuture<'a, Context>;
    /// The app's copy of every entity of the type.
    fn snapshot(&self, app: &App) -> Box<dyn Any>;
    fn restore(&self, app: &App, snapshot: &dyn Any);
    /// Parses an entity and puts it in the app's copy; its ID.
    fn stage_put(&self, app: &App, body: &JsonValue, id: Option<&str>)
    -> Result<String, SaveError>;
    fn stage_delete(&self, app: &App, id: &str) -> Result<(), SaveError>;
    /// Checks a staged entity against the staged project, filling in what's derived.
    fn check(&self, app: &App, id: &str, ctx: &Context) -> covalence_core::flow::Report;
    /// Whether a staged delete can go ahead, as the batch leaves things.
    fn deletable(&self, app: &App, id: &str) -> Result<(), String>;
    /// Stores and applies the entity `id` from `staged`, a snapshot.
    fn commit_put(&self, app: &App, staged: &dyn Any, id: &str) -> Result<JsonValue, SaveError>;
    fn commit_delete(&self, app: &App, id: &str) -> Result<(), SaveError>;
}

struct Of<K>(PhantomData<fn() -> K>);

impl<K: EntityKind> Batched for Of<K>
where
    K::Context: Sync,
{
    fn kind(&self) -> &'static str {
        K::TYPE
    }

    fn context<'a>(&'a self, app: &'a App) -> BoxFuture<'a, Context> {
        Box::pin(async move { Box::new(K::context(app).await) as Context })
    }

    fn snapshot(&self, app: &App) -> Box<dyn Any> {
        Box::new(read(K::all(app)).clone())
    }

    fn restore(&self, app: &App, snapshot: &dyn Any) {
        if let Some(all) = snapshot.downcast_ref::<BTreeMap<String, K>>() {
            *write(K::all(app)) = all.clone();
        }
    }

    fn stage_put(
        &self,
        app: &App,
        body: &JsonValue,
        id: Option<&str>,
    ) -> Result<String, SaveError> {
        let mut entity = K::parse(body).map_err(SaveError::invalid)?;
        match id {
            Some(_) => assign_id(entity.id_mut(), id, |i| read(K::all(app)).contains_key(i))?,
            // A create keeps an ID of its own, if it's a ULID nothing has.
            None => {
                let own = entity.id().to_owned();
                let usable =
                    ulid::Ulid::from_string(&own).is_ok() && !read(K::all(app)).contains_key(&own);
                if !usable {
                    if !own.is_empty() {
                        return Err(SaveError::invalid(vec![Problem {
                            path: vec![PathSegment::Key("id".into())],
                            code: Code::Invalid,
                            message: format!(
                                "id {own:?} must be a ULID that no {} has yet",
                                K::NAME
                            ),
                            at: None,
                        }]));
                    }
                    assign_id(entity.id_mut(), None, |_| false)?;
                }
            }
        }
        let id = entity.id().to_owned();
        write(K::all(app)).insert(id.clone(), entity);
        Ok(id)
    }

    fn stage_delete(&self, app: &App, id: &str) -> Result<(), SaveError> {
        write(K::all(app))
            .remove(id)
            .map(|_| ())
            .ok_or(SaveError::NotFound)
    }

    fn check(&self, app: &App, id: &str, ctx: &Context) -> covalence_core::flow::Report {
        let Some(mut entity) = read(K::all(app)).get(id).cloned() else {
            return Default::default();
        };
        let Some(ctx) = ctx.downcast_ref::<K::Context>() else {
            return Default::default();
        };
        let report = entity.check(app, ctx);
        write(K::all(app)).insert(id.to_owned(), entity);
        report
    }

    fn deletable(&self, app: &App, id: &str) -> Result<(), String> {
        K::deletable(app, id)
    }

    fn commit_put(&self, app: &App, staged: &dyn Any, id: &str) -> Result<JsonValue, SaveError> {
        let entity = staged
            .downcast_ref::<BTreeMap<String, K>>()
            .and_then(|all| all.get(id))
            .cloned()
            .ok_or(SaveError::NotFound)?;
        let json = serde_json::to_value(&entity).unwrap_or_default();
        app.put(entity)?;
        Ok(json)
    }

    fn commit_delete(&self, app: &App, id: &str) -> Result<(), SaveError> {
        app.delete_locked::<K>(id).map(|_| ())
    }
}

/// Every type a batch can change.
const KINDS: &[&dyn Batched] = &[
    &Of::<Surface>(PhantomData),
    &Of::<Component>(PhantomData),
    &Of::<Flow>(PhantomData),
    &Of::<Variable>(PhantomData),
];

fn kind_of(name: &str) -> Option<&'static dyn Batched> {
    KINDS.iter().copied().find(|k| k.kind() == name)
}

fn at_op(i: usize, mut problem: Problem) -> Problem {
    let mut path = vec![PathSegment::Key("ops".into()), PathSegment::Index(i as i64)];
    path.append(&mut problem.path);
    problem.path = path;
    problem
}

fn problem(i: usize, code: Code, message: String) -> Problem {
    at_op(
        i,
        Problem {
            path: Vec::new(),
            code,
            message,
            at: None,
        },
    )
}

async fn batch(State(app): State<Arc<App>>, Json(body): Json<Body>) -> Response {
    let mut errors = Vec::new();
    for (i, op) in body.ops.iter().enumerate() {
        if kind_of(op.kind()).is_none() {
            let types: Vec<&str> = KINDS.iter().map(|k| k.kind()).collect();
            errors.push(problem(
                i,
                Code::Invalid,
                format!(
                    "no entity type {:?}; a batch takes {}",
                    op.kind(),
                    types.join(", ")
                ),
            ));
        }
    }
    if !errors.is_empty() {
        return invalid(errors, Vec::new());
    }
    // What checking needs, fetched before the lock.
    let mut contexts: BTreeMap<&str, Context> = BTreeMap::new();
    for op in &body.ops {
        let kind = kind_of(op.kind()).expect("checked above");
        if !contexts.contains_key(kind.kind()) {
            contexts.insert(kind.kind(), kind.context(&app).await);
        }
    }
    let result = {
        let _saving = app.saving.lock().unwrap_or_else(|e| e.into_inner());
        run(&app, &body.ops, &contexts)
    };
    match result {
        Ok((results, warnings)) => (
            StatusCode::OK,
            Json(json!({ "results": results, "warnings": warnings })),
        )
            .into_response(),
        Err(Failed::Invalid(errors, warnings)) => invalid(errors, warnings),
        Err(Failed::Internal(e)) => ApiError::internal(e).into_response(),
    }
}

enum Failed {
    Invalid(Vec<Problem>, Vec<Problem>),
    Internal(SaveError),
}

/// Stages, checks and commits the ops. The caller holds the saving lock.
fn run(
    app: &App,
    ops: &[Op],
    contexts: &BTreeMap<&str, Context>,
) -> Result<(Vec<JsonValue>, Vec<Problem>), Failed> {
    let before: Vec<Box<dyn Any>> = KINDS.iter().map(|k| k.snapshot(app)).collect();
    let restore = |snapshots: &[Box<dyn Any>]| {
        for (k, s) in KINDS.iter().zip(snapshots) {
            k.restore(app, s.as_ref());
        }
    };
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    // Stage every op, so each check sees the whole batch.
    let mut ids = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        let kind = kind_of(op.kind()).expect("checked by the caller");
        let staged = match op {
            Op::Create { body, .. } => kind.stage_put(app, body, None),
            Op::Update { id, body, .. } => kind.stage_put(app, body, Some(id)),
            Op::Delete { id, .. } => kind.stage_delete(app, id).map(|_| id.clone()),
        };
        match staged {
            Ok(id) => ids.push(Some(id)),
            Err(SaveError::Invalid { errors: e, .. }) => {
                errors.extend(e.into_iter().map(|p| at_op(i, p)));
                ids.push(None);
            }
            Err(SaveError::NotFound) => {
                errors.push(problem(
                    i,
                    Code::Invalid,
                    format!("no {} with that id", op.kind()),
                ));
                ids.push(None);
            }
            Err(e) => {
                restore(&before);
                return Err(Failed::Internal(e));
            }
        }
    }
    if errors.is_empty() {
        for (i, (op, id)) in ops.iter().zip(&ids).enumerate() {
            let (kind, Some(id)) = (kind_of(op.kind()).expect("checked"), id) else {
                continue;
            };
            match op {
                Op::Delete { .. } => {
                    if let Err(why) = kind.deletable(app, id) {
                        errors.push(problem(i, Code::Invalid, format!("can't delete it: {why}")));
                    }
                }
                _ => {
                    let report = kind.check(app, id, &contexts[kind.kind()]);
                    errors.extend(report.errors.into_iter().map(|p| at_op(i, p)));
                    warnings.extend(report.warnings.into_iter().map(|p| at_op(i, p)));
                }
            }
        }
    }
    if !errors.is_empty() {
        restore(&before);
        return Err(Failed::Invalid(errors, warnings));
    }
    // The batch is good: put the copies back and do it for real, in order.
    let staged: Vec<Box<dyn Any>> = KINDS.iter().map(|k| k.snapshot(app)).collect();
    restore(&before);
    let mut results = Vec::new();
    for (op, id) in ops.iter().zip(ids) {
        let kind = kind_of(op.kind()).expect("checked");
        let id = id.expect("every op staged");
        let index = KINDS
            .iter()
            .position(|k| k.kind() == kind.kind())
            .expect("a known kind");
        let done = match op {
            Op::Delete { .. } => kind
                .commit_delete(app, &id)
                .map(|_| json!({ "type": kind.kind(), "id": id })),
            _ => kind
                .commit_put(app, staged[index].as_ref(), &id)
                .map(|entity| json!({ "type": kind.kind(), "id": id, "entity": entity })),
        };
        match done {
            Ok(result) => results.push(result),
            // Checked as a whole already; a store failure here is the
            // disk's, and what's done so far stays done.
            Err(e) => return Err(Failed::Internal(e)),
        }
    }
    Ok((results, warnings))
}

fn invalid(errors: Vec<Problem>, warnings: Vec<Problem>) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({ "errors": errors, "warnings": warnings })),
    )
        .into_response()
}

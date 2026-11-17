//! Variables over HTTP, and their life in the running core (dataflow §6).
//!
//! `/variables` has every entity type's five routes. A computed variable is
//! a cell in the dataflow graph; a writable one gets its `initial` value
//! when it has none. A persisted variable's value, and its reducer state,
//! go to the store's state cache whenever they change, and come back at
//! startup before any trigger is armed.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::sync::{Arc, RwLock, Weak};

use covalence_core::flow::{self, Report};
use covalence_core::journal::{JournalOp, NewEntry, Origin, Record, StatePatchRecord};
use covalence_core::state::{Meta, Quality, ReducerMeta, Source, StatePath};
use covalence_core::variable::{self, Variable};
use covalence_core::{Shared, clock, expr};
use covalence_store::Store;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use tokio::sync::broadcast;

use crate::entity::EntityKind;
use crate::{App, SaveError, read};

impl EntityKind for Variable {
    const TYPE: &'static str = variable::ENTITY;
    const NAME: &'static str = "variable";
    const PATH: &'static str = "/variables";
    type Context = ();

    fn parse(json: &JsonValue) -> Result<Variable, Vec<flow::Problem>> {
        Variable::parse(json)
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn id_mut(&mut self) -> &mut String {
        &mut self.id
    }

    fn all(app: &App) -> &RwLock<BTreeMap<String, Variable>> {
        &app.variables
    }

    fn context(_: &App) -> impl Future<Output = ()> + Send {
        std::future::ready(())
    }

    fn check(&mut self, app: &App, _: &()) -> Report {
        let others: BTreeMap<String, Variable> = read(&app.variables)
            .iter()
            .filter(|(id, _)| **id != self.id)
            .map(|(id, v)| (id.clone(), v.clone()))
            .collect();
        let taken: BTreeSet<String> = others.values().map(|v| v.slug.clone()).collect();
        if self.slug.is_empty() {
            self.slug = flow::derive_slug_for("var", &self.name, &taken);
        }
        let conns: BTreeSet<String> = app.conns().keys().cloned().collect();
        variable::validate(
            self,
            &variable::Context {
                taken_slugs: &taken,
                conns: &conns,
                variables: &others,
                evaluator: app.evaluator.as_ref(),
            },
        )
    }

    fn apply(&self, app: &App) -> Result<(), SaveError> {
        apply(app, self);
        let mut all = read(&app.variables).clone();
        all.insert(self.id.clone(), self.clone());
        app.runtime.set_read_only(read_only(&all));
        Ok(())
    }

    fn unapply(id: &str, app: &App) -> Result<(), SaveError> {
        app.dataflow
            .remove(&format!("{}:{id}", variable::ENTITY), None);
        uncache(app, id);
        app.runtime.set_read_only(read_only(&read(&app.variables)));
        Ok(())
    }

    fn settle(app: &App) -> impl Future<Output = ()> + Send {
        app.dataflow.flush()
    }
}

/// The computed variables' slugs, which `set` steps can't write.
pub(crate) fn read_only(vars: &BTreeMap<String, Variable>) -> BTreeSet<String> {
    vars.values()
        .filter(|v| v.is_computed())
        .map(|v| v.slug.clone())
        .collect()
}

/// Makes the core match a variable: its cell, or its initial value.
pub(crate) fn apply(app: &App, var: &Variable) {
    match var.cell() {
        Some(cell) => app.dataflow.put(cell, None),
        None => {
            app.dataflow.remove(&var.cell_id(), None);
            if let Some(initial) = &var.initial {
                write_initial(app.core.shared(), &var.home(), initial);
            }
        }
    }
    if var.persist {
        cache(app, var);
    } else {
        uncache(app, &var.id);
    }
}

/// Sets a writable variable to its initial value when it has none.
fn write_initial(shared: &Shared, home: &StatePath, initial: &JsonValue) {
    if shared.state.value(home).is_some() {
        return;
    }
    let value = expr::json_to_value(initial);
    patch(shared, home, value, None);
}

/// Journals and applies a core patch at `home`, with reducer metadata.
fn patch(
    shared: &Shared,
    home: &StatePath,
    value: covalence_protocol::Value,
    reducer: Option<ReducerMeta>,
) {
    let journal = &shared.journal;
    let entry = NewEntry::new(
        Origin::system(),
        Record::StatePatch(StatePatchRecord {
            ops: vec![JournalOp::Set {
                path: home.clone(),
                value: value.clone(),
                reducer: reducer.clone(),
            }],
            replaces: None,
            device_ts: None,
        }),
    );
    shared.state.mutate(|tree| {
        let seq = match journal.append(entry) {
            Ok(e) => e.seq,
            Err(e) => {
                tracing::error!(path = ?home, "cannot journal a variable's value: {e}");
                return ((), Default::default());
            }
        };
        let meta = Meta {
            ts: clock::epoch_ms(),
            seq,
            quality: Quality::Good,
            source: Source::Core,
        };
        let dirty = tree.set(home, &value, &meta);
        tree.set_reducer(home, reducer);
        ((), dirty)
    });
}

/// What the state cache keeps for one variable.
#[derive(Serialize, Deserialize)]
struct Cached {
    value: JsonValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reducer: Option<ReducerMeta>,
}

/// The cache key: the variable, not its path, so a rename keeps it.
fn cache_key(id: &str) -> String {
    format!("{}:{id}", variable::ENTITY)
}

/// Writes a persisted variable's current value to the state cache.
fn cache(app: &App, var: &Variable) {
    let home = var.home();
    let (value, reducer) = app.core.shared().state.read(|tree| {
        (
            tree.value(&home).as_ref().map(expr::to_json),
            tree.reducer(&home).cloned(),
        )
    });
    let store = app.store.lock().unwrap_or_else(|e| e.into_inner());
    let result = match value {
        Some(value) => {
            let body =
                serde_json::to_string(&Cached { value, reducer }).expect("a cached value encodes");
            store.put_cached(&cache_key(&var.id), &body)
        }
        None => store.delete_cached(&cache_key(&var.id)).map(drop),
    };
    if let Err(e) = result {
        tracing::error!(variable = %var.slug, "cannot keep a persisted variable: {e}");
    }
}

fn uncache(app: &App, id: &str) {
    let store = app.store.lock().unwrap_or_else(|e| e.into_inner());
    if let Err(e) = store.delete_cached(&cache_key(id)) {
        tracing::error!(variable = %id, "cannot forget a persisted variable: {e}");
    }
}

/// Puts persisted variables' cached values back in state, before their
/// cells are added (so reducers resume from them) and before any trigger is
/// armed. A writable variable with no cached value gets its initial value.
pub(crate) fn restore(shared: &Shared, store: &Store, vars: &BTreeMap<String, Variable>) {
    for var in vars.values() {
        let cached = if var.persist {
            match store.cached(&cache_key(&var.id)) {
                Ok(c) => c.and_then(|body| serde_json::from_str::<Cached>(&body).ok()),
                Err(e) => {
                    tracing::error!(variable = %var.slug, "cannot read a persisted variable: {e}");
                    None
                }
            }
        } else {
            None
        };
        match cached {
            Some(c) => patch(
                shared,
                &var.home(),
                expr::json_to_value(&c.value),
                c.reducer,
            ),
            None if !var.is_computed() => {
                if let Some(initial) = &var.initial {
                    write_initial(shared, &var.home(), initial);
                }
            }
            None => {}
        }
    }
}

/// Keeps persisted variables' cached values current, for as long as the app
/// runs.
pub(crate) fn spawn_persisting(app: &Arc<App>) {
    let mut feed = app.core.shared().state.subscribe();
    let app: Weak<App> = Arc::downgrade(app);
    tokio::spawn(async move {
        loop {
            let dirty = match feed.recv().await {
                Ok(d) => Some(d),
                // Behind: write them all again.
                Err(broadcast::error::RecvError::Lagged(_)) => None,
                Err(broadcast::error::RecvError::Closed) => return,
            };
            let Some(app) = app.upgrade() else { return };
            let persisted: Vec<Variable> = read(&app.variables)
                .values()
                .filter(|v| v.persist)
                .filter(|v| {
                    let home = v.home();
                    dirty
                        .as_ref()
                        .is_none_or(|d| d.iter().any(|p| p.is_within(&home)))
                })
                .cloned()
                .collect();
            for var in persisted {
                cache(&app, &var);
            }
        }
    });
}

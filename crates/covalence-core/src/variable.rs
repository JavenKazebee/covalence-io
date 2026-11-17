//! Variables (dataflow spec §6): the `variable` entity.
//!
//! A variable lives at `vars.<slug>`. Without `value` it's **writable**:
//! `set` steps write it, and `initial` is its value at load. With `value` it's
//! a **cell** (dataflow §5): computed by a stateless source or accumulated by
//! a reducer, read-only, and its `to` sinks keep other things in step.
//! `persist: true` keeps the value, and a reducer's state, across a restart.

use std::collections::{BTreeMap, BTreeSet};

use covalence_protocol::state::{Path, PathSegment};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::dataflow::{CellDef, JournalMode, Sink};
use crate::expr::Evaluator;
use crate::flow::{Code, Duration, Problem, Report};
use crate::source::{Part, Ref, Root, Source};
use crate::state::{ConnId, StatePath};
use crate::transform::Transform;

/// The entity type in the store.
pub const ENTITY: &str = "variable";

/// The state root variables live under.
pub const ROOT: &str = "vars";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Variable {
    /// A ULID, assigned by the server. Empty in a create request.
    #[serde(default)]
    pub id: String,
    pub name: String,
    /// `vars.<slug>`. Derived from `name` when empty.
    #[serde(default)]
    pub slug: String,
    /// A state schema node: `type`, `unit`, `kind` (state spec §1.2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<Json>,
    /// A writable variable's value at load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial: Option<Json>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub persist: bool,
    /// A computed or accumulated variable's source: it's read-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Source<Json>>,
    /// Sinks (dataflow §7).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub to: Vec<Sink>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journal: Option<Journaling>,
}

/// `"full"`, or `{sampled: "100ms"}` (dataflow §5.1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Journaling {
    Full(Full),
    Sampled { sampled: Duration },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Full {
    Full,
}

impl Variable {
    /// Reads a variable from JSON, or one `invalid` problem.
    pub fn parse(json: &Json) -> Result<Variable, Vec<Problem>> {
        crate::binding::deserialize(json, &[]).map_err(|p| vec![p])
    }

    /// `vars.<slug>`.
    pub fn home(&self) -> StatePath {
        home(&self.slug)
    }

    /// Whether it's a cell: computed or accumulated, so read-only.
    pub fn is_computed(&self) -> bool {
        self.value.is_some()
    }

    /// Its cell's identity: the entity, not the path (dataflow §5.4).
    pub fn cell_id(&self) -> String {
        format!("{ENTITY}:{}", self.id)
    }

    /// The cell a computed variable is.
    pub fn cell(&self) -> Option<CellDef> {
        let source = self.value.clone()?;
        Some(CellDef {
            journal: match &self.journal {
                Some(Journaling::Sampled { sampled }) => JournalMode::Sampled(sampled.get()),
                _ => JournalMode::Full,
            },
            to: self.to.clone(),
            ..CellDef::new(self.cell_id(), self.home(), source)
        })
    }
}

/// `vars.<slug>`.
pub fn home(slug: &str) -> StatePath {
    StatePath::new(ROOT, vec![PathSegment::Key(slug.to_owned())])
}

/// What checking a variable needs from the rest of the project.
pub struct Context<'a> {
    /// Slugs other variables have.
    pub taken_slugs: &'a BTreeSet<String>,
    /// Every connection's ID.
    pub conns: &'a BTreeSet<ConnId>,
    /// The other variables, by ID.
    pub variables: &'a BTreeMap<String, Variable>,
    pub evaluator: &'a dyn Evaluator,
}

fn key(k: &str) -> PathSegment {
    PathSegment::Key(k.to_owned())
}

fn join(base: &[PathSegment], more: &[PathSegment]) -> Path {
    base.iter().chain(more).cloned().collect()
}

/// Checks a variable (dataflow §6, §7).
pub fn validate(var: &Variable, ctx: &Context<'_>) -> Report {
    let mut report = Report {
        errors: crate::surface::named("variable", &var.name, &var.slug, ctx.taken_slugs),
        warnings: Vec::new(),
    };
    let mut error = |path: Path, code: Code, message: String| {
        report.errors.push(Problem::new(path, code, message));
    };
    if let Some(value) = &var.value {
        if var.initial.is_some() {
            error(
                vec![key("initial")],
                Code::Invalid,
                "a computed variable has no initial value: `value` gives it one".into(),
            );
        }
        source_problems(value, vec![key("value")], ctx, &mut error);
        if let Source::Use { transform, .. } = value
            && let Transform::Reducer(r) = &**transform
        {
            let steps = &r.definition().steps;
            if r.on.as_ref().is_some_and(|on| on.from.is_none()) {
                error(
                    vec![key("value"), key("on"), key("from")],
                    Code::Invalid,
                    "a variable's reducer names where its events come from".into(),
                );
            } else if steps.on.is_some() && steps.input.is_none() && r.on.is_none() {
                error(
                    vec![key("value"), key("on")],
                    Code::Invalid,
                    format!("a {} steps on events: give it `on`", r.id),
                );
            } else if steps.input.is_some() && steps.on.is_none() && r.input.is_none() {
                error(
                    vec![key("value"), key("input")],
                    Code::Invalid,
                    format!("a {} steps on its input: give it `input`", r.id),
                );
            }
        }
        if reaches_itself(var, ctx.variables) {
            error(
                vec![key("value")],
                Code::Cycle,
                format!("vars.{} reads itself through other variables", var.slug),
            );
        }
    } else if !var.to.is_empty() {
        error(
            vec![key("to")],
            Code::Unsupported,
            "sinks send a computed value; give the variable a `value`".into(),
        );
    }
    if let (Some(initial), Some(schema)) = (&var.initial, &var.schema)
        && !crate::dataflow::fits(Some(initial), schema)
    {
        error(
            vec![key("initial")],
            Code::BadValue,
            "the initial value doesn't fit the schema's type".into(),
        );
    }
    let mut warnings = Vec::new();
    for (i, sink) in var.to.iter().enumerate() {
        let at = vec![key("to"), PathSegment::Index(i as i64)];
        match sink {
            Sink::Set { set } => {
                let slug = match set {
                    Ref::Root {
                        root: Root::Vars,
                        path,
                    } => match path.as_slice() {
                        [PathSegment::Key(slug)] => Some(slug.clone()),
                        _ => None,
                    },
                    _ => None,
                };
                let at = join(&at, &[key("set")]);
                let Some(slug) = slug else {
                    error(at, Code::BadPath, "a `set` sink writes vars.<name>".into());
                    continue;
                };
                let target = ctx.variables.values().find(|v| v.slug == slug);
                if slug == var.slug {
                    error(
                        at,
                        Code::Cycle,
                        "a variable can't send its value to itself".into(),
                    );
                } else if target.is_some_and(Variable::is_computed) {
                    error(
                        at,
                        Code::ReadOnly,
                        format!("vars.{slug} is computed; only its own value sets it"),
                    );
                } else if target.is_none() {
                    warnings.push(Problem::new(
                        at,
                        Code::UndeclaredVariable,
                        format!("no variable declares vars.{slug}; declare it to give it a type"),
                    ));
                }
            }
            Sink::Action { action } => {
                let at = join(&at, &[key("action")]);
                if let Some(target) = action.target.literal()
                    && !ctx.conns.contains(&target.conn)
                {
                    error(
                        join(&at, &[key("target")]),
                        Code::UnknownConnection,
                        format!("no connection {:?}", target.conn),
                    );
                }
                if action.action.trim().is_empty() {
                    error(
                        join(&at, &[key("action")]),
                        Code::Invalid,
                        "name the action".into(),
                    );
                }
                if action.param.trim().is_empty() {
                    error(
                        join(&at, &[key("param")]),
                        Code::Invalid,
                        "name the parameter that receives the value".into(),
                    );
                }
                for (name, s) in &action.params {
                    source_problems(s, join(&at, &[key("params"), key(name)]), ctx, &mut error);
                }
            }
        }
    }
    report.warnings.extend(warnings);
    report
}

/// The problems inside a source: expressions that don't compile, unknown
/// connections, bad options, and reducers anywhere but at the top.
fn source_problems(
    source: &Source<Json>,
    base: Path,
    ctx: &Context<'_>,
    error: &mut dyn FnMut(Path, Code, String),
) {
    let mut here = base.clone();
    source.walk(&mut here, &mut |at, part| match part {
        Part::Expr(e) => {
            if let Err(problem) = ctx.evaluator.check(e) {
                error(at.clone(), Code::ExprSyntax, problem.message);
            }
        }
        Part::Ref(Ref::Conn { conn, .. }) if !ctx.conns.contains(conn) => error(
            join(at, &[key("conn")]),
            Code::UnknownConnection,
            format!("no connection {conn:?}"),
        ),
        Part::Ref(_) => {}
        Part::Transform(t) => {
            if t.is_reducer() && *at != base {
                error(
                    join(at, &[key("use")]),
                    Code::Unsupported,
                    format!(
                        "a {} keeps a value of its own, so it can only be the variable's whole value",
                        t.id()
                    ),
                );
            }
            for (rel, message) in t.problems() {
                error(join(at, &rel), Code::BadValue, message);
            }
        }
    });
}

/// The variables a source reads, by slug, as far as can be told without
/// running it: `{ref}`s to `vars`, and `vars.<name>` in expressions.
pub fn static_reads(source: &Source<Json>) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    source.walk(&mut Vec::new(), &mut |_, part| match part {
        Part::Ref(Ref::Root {
            root: Root::Vars,
            path,
        }) => {
            if let Some(PathSegment::Key(slug)) = path.first() {
                out.insert(slug.clone());
            }
        }
        Part::Expr(e) => {
            let mut rest = e;
            while let Some(i) = rest.find("vars.") {
                let before = rest[..i].chars().next_back();
                let after = &rest[i + 5..];
                let name: String = after
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                if !before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
                    && !name.is_empty()
                {
                    out.insert(name);
                }
                rest = after;
            }
        }
        _ => {}
    });
    out
}

/// Whether `var`'s value reads `var` itself, through computed variables.
fn reaches_itself(var: &Variable, others: &BTreeMap<String, Variable>) -> bool {
    let by_slug: BTreeMap<&str, &Variable> = others
        .values()
        .filter(|v| v.id != var.id)
        .chain(std::iter::once(var))
        .map(|v| (v.slug.as_str(), v))
        .collect();
    let reads = |v: &Variable| v.value.as_ref().map(static_reads).unwrap_or_default();
    let mut seen = BTreeSet::new();
    let mut stack: Vec<String> = reads(var).into_iter().collect();
    while let Some(slug) = stack.pop() {
        if slug == var.slug {
            return true;
        }
        if !seen.insert(slug.clone()) {
            continue;
        }
        if let Some(v) = by_slug.get(slug.as_str()) {
            stack.extend(reads(v));
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::expr::QuickJs;

    fn var(json: Json) -> Variable {
        Variable::parse(&json).unwrap()
    }

    fn check(v: &Variable, others: &[Variable]) -> (Vec<Code>, Vec<Code>) {
        let evaluator = QuickJs::default();
        let variables: BTreeMap<String, Variable> =
            others.iter().map(|v| (v.id.clone(), v.clone())).collect();
        let report = validate(
            v,
            &Context {
                taken_slugs: &BTreeSet::new(),
                conns: &["vh".to_owned()].into(),
                variables: &variables,
                evaluator: &evaluator,
            },
        );
        (
            report.errors.iter().map(|p| p.code).collect(),
            report.warnings.iter().map(|p| p.code).collect(),
        )
    }

    #[test]
    fn variables_round_trip() {
        for j in [
            json!({ "id": "1", "name": "Armed", "slug": "armed", "persist": true,
                    "value": { "use": "toggle", "on": { "from": { "path": { "match": "vh.key" } } } },
                    "to": [{ "set": { "root": "vars", "path": ["copy"] } },
                           { "action": { "target": "vh", "action": "arm", "param": "on", "rateLimit": "50ms" } }],
                    "journal": { "sampled": "100ms" } }),
            json!({ "id": "2", "name": "Cam", "slug": "cam", "initial": 1, "schema": { "type": "number" },
                    "journal": "full" }),
        ] {
            assert_eq!(serde_json::to_value(var(j.clone())).unwrap(), j);
        }
        assert!(Variable::parse(&json!({ "name": "x", "to": [{ "sett": {} }] })).is_err());
    }

    #[test]
    fn a_computed_variable_is_checked() {
        let live = var(json!({ "id": "1", "name": "Live", "slug": "isLive",
                               "value": { "expr": "vh.program == 3" } }));
        assert_eq!(check(&live, &[]), (vec![], vec![]));

        let bad = var(
            json!({ "id": "1", "name": "Bad", "slug": "bad", "initial": 1,
                              "value": { "expr": "vh.program ==" } }),
        );
        assert_eq!(check(&bad, &[]).0, vec![Code::Invalid, Code::ExprSyntax]);

        let toggle_without_on = var(json!({ "id": "1", "name": "T", "slug": "t",
                                            "value": { "use": "toggle" } }));
        assert_eq!(check(&toggle_without_on, &[]).0, vec![Code::Invalid]);

        let nested = var(json!({ "id": "1", "name": "N", "slug": "n",
            "value": { "use": "not", "input": { "use": "toggle", "on": { "from": { "path": { "match": "vh.key" } } } } } }));
        assert_eq!(check(&nested, &[]).0, vec![Code::Unsupported]);

        let writable_with_sinks = var(json!({ "id": "1", "name": "W", "slug": "w",
            "to": [{ "set": { "root": "vars", "path": ["x"] } }] }));
        assert_eq!(check(&writable_with_sinks, &[]).0, vec![Code::Unsupported]);
    }

    #[test]
    fn reads_that_form_a_cycle_are_an_error() {
        let a =
            var(json!({ "id": "a", "name": "A", "slug": "a", "value": { "expr": "vars.b + 1" } }));
        let b = var(json!({ "id": "b", "name": "B", "slug": "b",
                            "value": { "ref": { "root": "vars", "path": ["a"] } } }));
        assert_eq!(check(&a, &[]).0, vec![]);
        assert_eq!(check(&a, std::slice::from_ref(&b)).0, vec![Code::Cycle]);
        let me = var(
            json!({ "id": "c", "name": "C", "slug": "c", "value": { "expr": "myvars.c + vars.c" } }),
        );
        assert_eq!(check(&me, &[]).0, vec![Code::Cycle]);
        assert_eq!(
            static_reads(
                &Source::from_json(json!({ "expr": "myvars.x + vars.y_1 * vars.z" })).unwrap()
            ),
            ["y_1".to_owned(), "z".to_owned()].into()
        );
    }

    #[test]
    fn sinks_are_checked() {
        let computed = var(json!({ "id": "x", "name": "X", "slug": "x", "value": 1 }));
        let writable = var(json!({ "id": "y", "name": "Y", "slug": "y" }));
        let v = var(
            json!({ "id": "1", "name": "S", "slug": "s", "value": 2, "to": [
            { "set": { "root": "vars", "path": ["x"] } },
            { "set": { "root": "vars", "path": ["y"] } },
            { "set": { "root": "vars", "path": ["z"] } },
            { "set": { "root": "vars", "path": ["s"] } },
            { "set": { "root": "sys", "path": ["s"] } },
            { "action": { "target": "nope", "action": "go", "param": "" } }
        ] }),
        );
        assert_eq!(
            check(&v, &[computed, writable]),
            (
                vec![
                    Code::ReadOnly,
                    Code::Cycle,
                    Code::BadPath,
                    Code::UnknownConnection,
                    Code::Invalid
                ],
                vec![Code::UndeclaredVariable]
            )
        );
    }
}

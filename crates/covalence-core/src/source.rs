//! Sources (dataflow spec §1, ADR 0026): the one shape for a value that may
//! follow state.
//!
//! A [`Source<T>`] is a literal `T`, `{ref}` (one state path), `{expr}` (a
//! sandboxed expression) or `{use}` (a transform, [`crate::transform`]).
//! It's recognised by its shape (dataflow §1.1): an object with a `ref`,
//! `expr`, `use` or `value` key is a source, and any key its form doesn't
//! allow is an error rather than a literal. `{value}` escapes a literal that
//! would read as a source.
//!
//! One evaluator serves every position. [`Source::evaluate`] gives the
//! value and the state it read; the position applies the source's own
//! fallback, since what counts as a usable result depends on it (a label
//! takes text, a wait takes a duration). Sources nested in a transform
//! apply theirs as they're evaluated. Each source also compiles to an
//! expression ([`Source::to_expr`]), which gives the same result.

use std::collections::{BTreeMap, BTreeSet};

use covalence_protocol::state::{Path, PathSegment};
use serde::de::{DeserializeOwned, Error as _};
use serde::ser::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value as Json};

use crate::expr::{Evaluator, ExprError, Scope, to_json};
use crate::state::{ConnId, StatePath};
use crate::text_path;
use crate::transform::Transform;

/// The keys that make an object a source.
pub const RESERVED: [&str; 4] = ["ref", "expr", "use", "value"];

/// Where a value comes from (dataflow §1).
#[derive(Clone, Debug, PartialEq)]
pub enum Source<T> {
    /// A fixed value. Never evaluated, read-tracked or journaled.
    Literal(T),
    /// One state path.
    Ref { at: Ref, fallback: Option<T> },
    /// A sandboxed expression (ADR 0021).
    Expr { expr: String, fallback: Option<T> },
    /// A transform over other sources (dataflow §4).
    Use {
        transform: Box<Transform>,
        fallback: Option<T>,
    },
}

/// A state path in ID form: a connection's, or a reserved root's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Ref {
    Conn { conn: ConnId, path: Path },
    Root { root: Root, path: Path },
}

impl Ref {
    pub fn state_path(&self) -> StatePath {
        match self {
            Ref::Conn { conn, path } => StatePath::new(conn.clone(), path.clone()),
            Ref::Root { root, path } => StatePath::new(root.as_str(), path.clone()),
        }
    }

    /// The text form, `vh.outputs[1].source`, with a connection's alias.
    /// `None` for a connection with no alias.
    pub fn text(&self, aliases: &BTreeMap<ConnId, String>) -> Option<String> {
        match self {
            Ref::Conn { conn, path } => Some(text_path::format(aliases.get(conn)?, path)),
            Ref::Root { root, path } => Some(text_path::format(root.as_str(), path)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Root {
    Vars,
    Sys,
    Surface,
    Layer,
    Flow,
}

impl Root {
    pub fn as_str(self) -> &'static str {
        match self {
            Root::Vars => "vars",
            Root::Sys => "sys",
            Root::Surface => "surface",
            Root::Layer => "layer",
            Root::Flow => "flow",
        }
    }
}

impl<T> Source<T> {
    pub fn literal(&self) -> Option<&T> {
        match self {
            Source::Literal(v) => Some(v),
            _ => None,
        }
    }

    pub fn fallback(&self) -> Option<&T> {
        match self {
            Source::Literal(_) => None,
            Source::Ref { fallback, .. }
            | Source::Expr { fallback, .. }
            | Source::Use { fallback, .. } => fallback.as_ref(),
        }
    }
}

// ---------------------------------------------------------------------------
// Recognition (dataflow §1.1)

impl<T: DeserializeOwned> Source<T> {
    /// Reads a source from JSON, by its shape.
    pub fn from_json(json: Json) -> Result<Self, String> {
        let Json::Object(mut map) = json else {
            return typed(json).map(Source::Literal);
        };
        // `use` decides by itself: a transform's options may have any
        // name, `value` included (`compare`).
        let found: Vec<&str> = if map.contains_key("use") {
            vec!["use"]
        } else {
            RESERVED
                .into_iter()
                .filter(|k| map.contains_key(*k))
                .collect()
        };
        let form = match found.as_slice() {
            [] => return typed(Json::Object(map)).map(Source::Literal),
            [one] => *one,
            many => {
                return Err(format!(
                    "a source has one of `ref`, `expr`, `use` or `value`, not {}",
                    many.iter()
                        .map(|k| format!("`{k}`"))
                        .collect::<Vec<_>>()
                        .join(" and ")
                ));
            }
        };
        let fallback = match map.remove("fallback") {
            Some(_) if form == "value" => {
                return Err(not_allowed("fallback", form));
            }
            Some(f) => Some(typed(f).map_err(|e| format!("fallback: {e}"))?),
            None => None,
        };
        if form == "use" {
            let name = map.get("use").and_then(Json::as_str).unwrap_or_default();
            let (id, version) = crate::transform::reducer::split_use(name)?;
            let transform = if Transform::is_reducer_id(id) {
                Transform::Reducer(Box::new(crate::transform::Reducer::from_map(map)?))
            } else {
                if let Some(v) = version {
                    // Every stateless built-in is at version 1.
                    if v != 1 {
                        return Err(format!("there's no {id}@{v}; this core has {id}@1"));
                    }
                    map.insert("use".into(), Json::String(id.to_owned()));
                }
                // The transform's own options are checked by its type.
                typed::<Transform>(Json::Object(map))?
            };
            return Ok(Source::Use {
                transform: Box::new(transform),
                fallback,
            });
        }
        if let Some(extra) = map.keys().find(|k| k.as_str() != form) {
            return Err(not_allowed(extra, form));
        }
        let inner = map.remove(form).expect("the form's key is present");
        match form {
            "value" => typed(inner).map(Source::Literal),
            "ref" => Ok(Source::Ref {
                at: typed(inner).map_err(|e| format!("ref: {e}"))?,
                fallback,
            }),
            _ => match inner {
                Json::String(expr) => Ok(Source::Expr { expr, fallback }),
                _ => Err("`expr` must be the expression's text".into()),
            },
        }
    }
}

fn not_allowed(key: &str, form: &str) -> String {
    let takes = match form {
        "value" => "nothing else",
        _ => "only `fallback` beside it",
    };
    format!(
        "{key:?} isn't part of a `{form}` source, which takes {takes}. \
         An object with a `{form}` key that is meant as a value goes inside {{\"value\": …}}"
    )
}

/// Reads `json` as `U`, naming where inside it a problem is.
fn typed<U: DeserializeOwned>(json: Json) -> Result<U, String> {
    serde_path_to_error::deserialize(json).map_err(|e| {
        let path = e.path().to_string();
        let inner = e.into_inner();
        if path == "." || path.is_empty() {
            inner.to_string()
        } else {
            format!("{path}: {inner}")
        }
    })
}

impl<'de, T: DeserializeOwned> Deserialize<'de> for Source<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Source::from_json(Json::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

/// Whether a value would read as a source, so a literal needs `{value}`.
pub fn looks_like_source(json: &Json) -> bool {
    json.as_object()
        .is_some_and(|m| RESERVED.iter().any(|k| m.contains_key(*k)))
}

impl<T: Serialize> Source<T> {
    /// The stored form.
    pub fn to_json(&self) -> Result<Json, serde_json::Error> {
        let mut map = Map::new();
        let fallback = match self {
            Source::Literal(v) => {
                let json = serde_json::to_value(v)?;
                return Ok(if looks_like_source(&json) {
                    serde_json::json!({ "value": json })
                } else {
                    json
                });
            }
            Source::Ref { at, fallback } => {
                map.insert("ref".into(), serde_json::to_value(at)?);
                fallback
            }
            Source::Expr { expr, fallback } => {
                map.insert("expr".into(), Json::String(expr.clone()));
                fallback
            }
            Source::Use {
                transform,
                fallback,
            } => {
                let options = if let Transform::Reducer(r) = &**transform {
                    Json::Object(r.to_map()?)
                } else {
                    serde_json::to_value(transform)?
                };
                let Json::Object(options) = options else {
                    return Err(<serde_json::Error as serde::ser::Error>::custom(
                        "a transform is an object",
                    ));
                };
                map = options;
                fallback
            }
        };
        if let Some(f) = fallback {
            map.insert("fallback".into(), serde_json::to_value(f)?);
        }
        Ok(Json::Object(map))
    }
}

impl<T: Serialize> Serialize for Source<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_json()
            .map_err(S::Error::custom)?
            .serialize(serializer)
    }
}

// ---------------------------------------------------------------------------
// Evaluation (dataflow §1.2)

/// What a source gave: its value (`None` is `undefined`) and the state it read.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub value: Result<Option<Json>, ExprError>,
    pub reads: BTreeSet<StatePath>,
}

/// One evaluation in progress: where expressions run, and what's been read.
pub struct Eval<'a> {
    pub scope: &'a Scope<'a>,
    pub evaluator: &'a dyn Evaluator,
    pub reads: BTreeSet<StatePath>,
}

impl<'a> Eval<'a> {
    pub fn new(scope: &'a Scope<'a>, evaluator: &'a dyn Evaluator) -> Self {
        Eval {
            scope,
            evaluator,
            reads: BTreeSet::new(),
        }
    }

    /// A source's own result, without its fallback.
    pub fn raw<T: Serialize>(&mut self, source: &Source<T>) -> Result<Option<Json>, ExprError> {
        match source {
            Source::Literal(v) => Ok(Some(
                serde_json::to_value(v).expect("a literal encodes as JSON"),
            )),
            Source::Ref { at, .. } => {
                let path = at.state_path();
                let value = self.scope.state.value(&path).as_ref().map(to_json);
                self.reads.insert(path);
                Ok(value)
            }
            Source::Expr { expr, .. } => {
                let evaluation = self.evaluator.evaluate(expr, self.scope);
                self.reads.extend(evaluation.reads.paths);
                evaluation.value.map(|v| v.as_ref().map(to_json))
            }
            Source::Use { transform, .. } => transform.evaluate(self),
        }
    }

    /// A source nested in a transform: its result, or its fallback when
    /// that is `undefined` or fails.
    pub fn nested(&mut self, source: &Source<Json>) -> Result<Option<Json>, ExprError> {
        match (self.raw(source), source.fallback()) {
            (Ok(Some(v)), _) => Ok(Some(v)),
            (_, Some(f)) => Ok(Some(f.clone())),
            (result, None) => result,
        }
    }
}

impl<T: Serialize> Source<T> {
    /// Evaluates the source in `scope`. The caller applies [`Source::fallback`].
    pub fn evaluate(&self, scope: &Scope<'_>, evaluator: &dyn Evaluator) -> Outcome {
        let mut eval = Eval::new(scope, evaluator);
        let value = eval.raw(self);
        Outcome {
            value,
            reads: eval.reads,
        }
    }

    /// The expression this source stands for, without its own fallback
    /// (the position applies that), with connections by alias. `None` if a
    /// connection has no alias. An `expr` is its own source, as written.
    pub fn to_expr(&self, aliases: &BTreeMap<ConnId, String>) -> Option<String> {
        Some(match self {
            Source::Literal(v) => literal_expr(&serde_json::to_value(v).ok()?),
            Source::Ref { at, .. } => at.text(aliases)?,
            Source::Expr { expr, .. } => expr.clone(),
            Source::Use { transform, .. } => transform.to_expr(aliases)?,
        })
    }
}

/// A JSON value as a JavaScript literal. Objects are parenthesised, so they
/// don't read as a block.
pub(crate) fn literal_expr(v: &Json) -> String {
    match v {
        Json::Object(_) => format!("({v})"),
        _ => v.to_string(),
    }
}

/// A nested source as an operand of a compiled expression: with its
/// fallback applied, and in parentheses unless it's a literal or a path.
/// An `expr` written as a function body (with `return`) has no operand form;
/// its compiled expression is for reading, not running.
pub(crate) fn operand(source: &Source<Json>, aliases: &BTreeMap<ConnId, String>) -> Option<String> {
    let text = source.to_expr(aliases)?;
    let text = match source {
        Source::Literal(_) | Source::Ref { .. } => text,
        _ => format!("({text})"),
    };
    Some(match source.fallback() {
        None => text,
        Some(f) => {
            let f = literal_expr(f);
            format!(
                "(() => {{ try {{ const v = {text}; return v === undefined ? {f} : v; }} catch {{ return {f}; }} }})()"
            )
        }
    })
}

// ---------------------------------------------------------------------------
// Walking

/// A source's parts that validation checks, with where each is stored.
pub enum Part<'s> {
    Ref(&'s Ref),
    Expr(&'s str),
    Transform(&'s Transform),
}

impl<T> Source<T> {
    /// Calls `f` for this source and every source nested in it, depth first,
    /// with the stored path of each part: `…/ref`, `…/expr`, or the
    /// transform's own path.
    pub fn walk<'s>(&'s self, here: &mut Path, f: &mut dyn FnMut(&Path, Part<'s>)) {
        match self {
            Source::Literal(_) => {}
            Source::Ref { at, .. } => {
                here.push(PathSegment::Key("ref".into()));
                f(here, Part::Ref(at));
                here.pop();
            }
            Source::Expr { expr, .. } => {
                here.push(PathSegment::Key("expr".into()));
                f(here, Part::Expr(expr));
                here.pop();
            }
            Source::Use { transform, .. } => {
                f(here, Part::Transform(transform));
                transform.walk(here, f);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn parse(json: Json) -> Result<Source<Json>, String> {
        Source::from_json(json)
    }

    #[test]
    fn recognises_each_form() {
        assert_eq!(parse(json!(3)).unwrap(), Source::Literal(json!(3)));
        assert_eq!(
            parse(json!({ "a": 1 })).unwrap(),
            Source::Literal(json!({ "a": 1 }))
        );
        assert_eq!(
            parse(json!({ "value": { "ref": 1 } })).unwrap(),
            Source::Literal(json!({ "ref": 1 }))
        );
        assert!(matches!(
            parse(json!({ "ref": { "root": "vars", "path": ["a"] }, "fallback": 0 })).unwrap(),
            Source::Ref {
                fallback: Some(_),
                ..
            }
        ));
        assert!(matches!(
            parse(json!({ "expr": "1 + 1" })).unwrap(),
            Source::Expr { fallback: None, .. }
        ));
        assert!(matches!(
            parse(json!({ "use": "not", "input": true })).unwrap(),
            Source::Use { .. }
        ));
        // A transform's own option may share a reserved name.
        assert!(matches!(
            parse(json!({ "use": "compare", "input": 1, "op": "==", "value": 1 })).unwrap(),
            Source::Use { .. }
        ));
    }

    #[test]
    fn a_reserved_key_commits_the_object() {
        for bad in [
            json!({ "ref": { "root": "vars", "path": ["a"] }, "fallbak": 0 }),
            json!({ "expr": "1", "other": 2 }),
            json!({ "value": 1, "fallback": 2 }),
            json!({ "ref": { "root": "vars", "path": [] }, "expr": "1" }),
            json!({ "use": "not", "input": true, "expr": "1" }),
            json!({ "use": "not", "input": true, "extra": 1 }),
            json!({ "use": "nope" }),
            json!({ "expr": 3 }),
        ] {
            assert!(parse(bad.clone()).is_err(), "{bad} parsed");
        }
    }

    #[test]
    fn a_literal_that_looks_like_a_source_round_trips_escaped() {
        let s: Source<Json> = Source::Literal(json!({ "expr": "x" }));
        let stored = s.to_json().unwrap();
        assert_eq!(stored, json!({ "value": { "expr": "x" } }));
        assert_eq!(parse(stored).unwrap(), s);
    }

    #[test]
    fn literals_are_typed() {
        assert!(Source::<bool>::from_json(json!("yes")).is_err());
        assert_eq!(
            Source::<bool>::from_json(json!(true)).unwrap(),
            Source::Literal(true)
        );
        assert!(Source::<bool>::from_json(json!({ "expr": "x", "fallback": "no" })).is_err());
    }
}

//! Templates and instances (composition spec §1–§5): the one way to reuse
//! anything by parameters.
//!
//! A **template** declares `params` ([`ParamDef`]s); an **instance** names
//! its template and binds them, each a [`Source`] evaluated in the placer's
//! scope. What this module does is the same for every kind of template:
//!
//! - **Binding:** which source gives each param ([`bound`]), checked
//!   against the template's definitions ([`check_bindings`]), and each one
//!   made a cell at `<home>.params.<name>` ([`param_cells`]), so params
//!   follow state like any other cell and are read through it.
//! - **One instance per item** ([`ForEach`], [`expand`]): the items of a
//!   live list or a target selector, each with a key, at most
//!   [`MAX_ITEMS`].
//! - **Identity:** the [`InstanceChain`] every journal entry made inside an
//!   instance carries (§5).
//! - **Detach:** an instance's params as a plain entity's own
//!   ([`detached_params`]), with a `forEach` item written in.
//!
//! Each kind (flows today) puts the template's body together with an
//! instance's identity, and decides where its home is.

use std::collections::{BTreeMap, BTreeSet};

use covalence_protocol::Value;
use covalence_protocol::state::{Path, PathSegment};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::dataflow::CellDef;
use crate::expr::{Evaluator, ExprError, Scope, json_to_value};
use crate::flow::{Code, Problem};
use crate::param::ParamDef;
use crate::schema;
use crate::select::{Selector, TargetView, Targets};
use crate::source::{Eval, Part, Ref, ScopeName, Source};
use crate::state::StatePath;

/// At most this many items per `forEach` instance (composition §11.3).
pub const MAX_ITEMS: usize = 256;

/// Instances nest at most this deep (composition §3).
pub const MAX_DEPTH: usize = 16;

/// One instance an entry was made inside (composition §5): its template,
/// the instance entity, and the item's key for a `forEach` instance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    pub template: String,
    pub instance: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<PathSegment>,
}

/// The instances an entry was made inside, outermost first.
pub type InstanceChain = Vec<Link>;

/// Whether `chain` is inside `instance`, at any depth, and with `key`
/// that item of it.
pub fn chain_has(chain: &[Link], instance: &str, key: Option<&PathSegment>) -> bool {
    chain
        .iter()
        .any(|l| l.instance == instance && key.is_none_or(|k| l.key.as_ref() == Some(k)))
}

/// Whether `chain` has an instance of `template`, at any depth.
pub fn chain_uses(chain: &[Link], template: &str) -> bool {
    chain.iter().any(|l| l.template == template)
}

// ---------------------------------------------------------------------------
// Params

/// What a param holds (composition §1.2): a value, or a whole template.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamKind {
    Value,
    Component,
    Flow,
}

impl ParamKind {
    pub fn of(def: &ParamDef) -> ParamKind {
        match def.schema.get("kind").and_then(Json::as_str) {
            Some("component") => ParamKind::Component,
            Some("flow") => ParamKind::Flow,
            _ => ParamKind::Value,
        }
    }
}

/// The source each param takes: the instance's binding, or the template's
/// default. A param with neither is left out, and reads as `undefined`.
pub fn bound(
    defs: &BTreeMap<String, ParamDef>,
    bindings: &BTreeMap<String, Source<Json>>,
) -> Vec<(String, Source<Json>)> {
    defs.iter()
        .filter_map(|(name, def)| {
            let source = bindings.get(name).or(def.default.as_ref())?;
            Some((name.clone(), source.clone()))
        })
        .collect()
}

/// The cells an instance's params are, at `<home>.params.<name>`, with
/// `values` (a `forEach` item's) in scope. Their IDs start with `prefix`.
pub fn param_cells(
    prefix: &str,
    home: &StatePath,
    bound: &[(String, Source<Json>)],
    values: &[(String, Value)],
) -> Vec<CellDef> {
    bound
        .iter()
        .map(|(name, source)| CellDef {
            values: values.to_vec(),
            ..CellDef::new(
                format!("{prefix}{name}"),
                home.key("params").key(name),
                source.clone(),
            )
        })
        .collect()
}

/// The cell ID prefix of an instance's params: `param:<instance>/`, or
/// with a `forEach` item, `param:<instance>[<key>]/`.
pub fn param_prefix(instance: &str, key: Option<&PathSegment>) -> String {
    match key {
        None => format!("param:{instance}/"),
        Some(k) => format!("param:{instance}[{}]/", key_text(k)),
    }
}

/// A key as it's written in brackets: `"Cam A"` or `3`.
pub fn key_text(key: &PathSegment) -> String {
    match key {
        PathSegment::Key(k) => Json::String(k.clone()).to_string(),
        PathSegment::Index(i) => i.to_string(),
    }
}

/// Checks an instance's bindings against its template's params: each
/// names a declared param, each required one is bound, literals fit, a
/// component or flow param is bound to an ID, and `{scope}` refs name
/// what's in scope where it's placed (`item` with `forEach` only).
/// Problems are at `params.<name>`.
pub fn check_bindings(
    defs: &BTreeMap<String, ParamDef>,
    bindings: &BTreeMap<String, Source<Json>>,
    for_each: bool,
) -> Vec<Problem> {
    let mut out = Vec::new();
    let at = |name: &str| vec![key("params"), key(name)];
    for (name, source) in bindings {
        let Some(def) = defs.get(name) else {
            out.push(Problem::new(
                at(name),
                Code::UnknownParam,
                format!("the template has no param {name:?}"),
            ));
            continue;
        };
        match ParamKind::of(def) {
            ParamKind::Value => {
                if let Some(value) = source.literal() {
                    for m in schema::check(&def.schema(), value) {
                        out.push(Problem::new(
                            under(&at(name), &m.path),
                            Code::BadInstance,
                            m.message,
                        ));
                    }
                }
            }
            kind => {
                if !matches!(source.literal(), Some(Json::String(_))) {
                    let what = if kind == ParamKind::Flow {
                        "flow"
                    } else {
                        "component"
                    };
                    out.push(Problem::new(
                        at(name),
                        Code::TemplateNotLiteral,
                        format!(
                            "{name:?} holds a {what}: bind it to the {what}'s ID, which doesn't follow state"
                        ),
                    ));
                }
            }
        }
        source.walk(&mut at(name), &mut |path, part| {
            if let Part::Ref(Ref::Scope { scope, .. }) = part {
                let ok = *scope == ScopeName::Item && for_each;
                if !ok {
                    out.push(Problem::new(
                        path.clone(),
                        Code::NotInScope,
                        format!(
                            "{:?} isn't in scope where this instance is placed",
                            scope.as_str()
                        ),
                    ));
                }
            }
        });
    }
    for (name, def) in defs {
        if def.needs_value() && !bindings.contains_key(name) {
            out.push(Problem::new(
                at(name),
                Code::BadInstance,
                format!("{name:?} is required and has no default, so the instance must bind it"),
            ));
        }
    }
    out
}

/// What saving `defs` as a template would break: each instance, by ID,
/// with the reasons. Empty when every instance still fits (composition §5).
pub fn broken_instances<'a>(
    defs: &BTreeMap<String, ParamDef>,
    instances: impl IntoIterator<Item = (&'a str, &'a BTreeMap<String, Source<Json>>, bool)>,
) -> Vec<(String, Vec<String>)> {
    instances
        .into_iter()
        .filter_map(|(id, bindings, for_each)| {
            let problems: Vec<String> = check_bindings(defs, bindings, for_each)
                .into_iter()
                .map(|p| p.message)
                .collect();
            (!problems.is_empty()).then(|| (id.to_owned(), problems))
        })
        .collect()
}

/// Whether following `edges` (a template to the templates it uses,
/// through its body or its params' bindings) from `start` comes back to
/// it, or goes deeper than [`MAX_DEPTH`]. `Err` names the loop or the
/// path that's too deep.
pub fn check_nesting(
    start: &str,
    edges: &dyn Fn(&str) -> Vec<String>,
) -> Result<(), (Code, Vec<String>)> {
    fn walk(
        at: &str,
        start: &str,
        edges: &dyn Fn(&str) -> Vec<String>,
        trail: &mut Vec<String>,
    ) -> Result<(), (Code, Vec<String>)> {
        if trail.len() > MAX_DEPTH {
            return Err((Code::TooDeep, trail.clone()));
        }
        for next in edges(at) {
            if next == start {
                let mut cycle = trail.clone();
                cycle.push(next);
                return Err((Code::RecursiveTemplate, cycle));
            }
            if trail.contains(&next) {
                // A loop that doesn't pass through `start` is reported when
                // one of its own members is saved.
                continue;
            }
            trail.push(next.clone());
            walk(&next, start, edges, trail)?;
            trail.pop();
        }
        Ok(())
    }
    walk(start, start, edges, &mut vec![start.to_owned()])
}

// ---------------------------------------------------------------------------
// One instance per item (composition §4)

/// Where a `forEach` instance's items come from, and their keys:
/// `{items, key?}` or `{targets, key?}`.
#[derive(Clone, Debug, PartialEq)]
pub struct ForEach {
    pub from: ItemsFrom,
    /// Each item's identity, with `item` and `index` in scope. Default: the
    /// item itself, or a target's connection ID.
    pub key: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ItemsFrom {
    /// A live list.
    Items(Source<Json>),
    /// Every target the selector picks: `{plugin: "io.covalence.recsim"}`.
    Targets(Selector<Targets>),
}

impl Serialize for ForEach {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::Error;
        let mut map = serde_json::Map::new();
        match &self.from {
            ItemsFrom::Items(items) => {
                map.insert("items".into(), items.to_json().map_err(S::Error::custom)?);
            }
            ItemsFrom::Targets(targets) => {
                map.insert(
                    "targets".into(),
                    targets.to_json().map_err(S::Error::custom)?,
                );
            }
        }
        if let Some(key) = &self.key {
            map.insert("key".into(), Json::String(key.clone()));
        }
        Json::Object(map).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ForEach {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let Json::Object(mut map) = Json::deserialize(deserializer)? else {
            return Err(D::Error::custom("forEach is {items} or {targets}"));
        };
        let key = match map.remove("key") {
            None => None,
            Some(Json::String(k)) => Some(k),
            Some(_) => return Err(D::Error::custom("forEach's `key` is an expression's text")),
        };
        let from = match (map.remove("items"), map.remove("targets")) {
            (Some(items), None) => ItemsFrom::Items(
                Source::from_json(items).map_err(|e| D::Error::custom(format!("items: {e}")))?,
            ),
            (None, Some(targets)) => ItemsFrom::Targets(
                Selector::from_json(targets)
                    .map_err(|e| D::Error::custom(format!("targets: {e}")))?,
            ),
            _ => return Err(D::Error::custom("forEach has one of `items` or `targets`")),
        };
        if let Some(extra) = map.keys().next() {
            return Err(D::Error::custom(format!(
                "{extra:?} isn't part of forEach, which has `items` or `targets`, and `key`"
            )));
        }
        Ok(ForEach { from, key })
    }
}

/// One item of a `forEach` instance.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub key: PathSegment,
    pub index: usize,
    pub value: Json,
}

impl Item {
    /// `item`, `index` and `key`, as the item's params see them.
    pub fn values(&self) -> Vec<(String, Value)> {
        let key = match &self.key {
            PathSegment::Key(k) => Value::from(k.as_str()),
            PathSegment::Index(i) => Value::from(*i),
        };
        vec![
            ("item".to_owned(), json_to_value(&self.value)),
            ("index".to_owned(), Value::from(self.index as u64)),
            ("key".to_owned(), key),
        ]
    }
}

/// What [`expand`] found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Items {
    /// In list order, each key once, at most [`MAX_ITEMS`].
    pub items: Vec<Item>,
    /// Items past the limit, which aren't instanced (`tooMany`).
    pub dropped: usize,
    /// Items whose key repeats an earlier one's, which aren't instanced.
    pub duplicates: Vec<PathSegment>,
    /// The state the list and keys read.
    pub reads: BTreeSet<StatePath>,
}

/// The items of `for_each` now: the list's, or the targets'. A list that
/// isn't an array, or is `undefined`, has none.
pub fn expand(
    for_each: &ForEach,
    scope: &Scope<'_>,
    evaluator: &dyn Evaluator,
    targets: &[TargetView],
) -> Result<Items, ExprError> {
    let mut eval = Eval::new(scope, evaluator);
    let values: Vec<Json> = match &for_each.from {
        ItemsFrom::Items(items) => match eval.nested(items)? {
            Some(Json::Array(list)) => list,
            _ => Vec::new(),
        },
        ItemsFrom::Targets(selector) => selector
            .resolve(targets, Some(&mut eval))?
            .into_iter()
            .map(|t| serde_json::json!({ "conn": t.conn }))
            .collect(),
    };
    let mut reads = eval.reads;
    let mut out = Items::default();
    let mut seen = BTreeSet::new();
    for (index, value) in values.into_iter().enumerate() {
        let key = match &for_each.key {
            None => default_key(&value),
            Some(expr) => {
                let item = Item {
                    key: PathSegment::Index(index as i64),
                    index,
                    value: value.clone(),
                };
                let values = item.values();
                let scope = Scope {
                    values: &values,
                    ..Scope::new(scope.state, scope.aliases)
                };
                let evaluation = evaluator.evaluate(expr, &scope);
                reads.extend(evaluation.reads.paths);
                evaluation
                    .value?
                    .map(|v| default_key(&crate::expr::to_json(&v)))
                    .unwrap_or(PathSegment::Index(index as i64))
            }
        };
        if !seen.insert(key.clone()) {
            out.duplicates.push(key);
            continue;
        }
        if out.items.len() == MAX_ITEMS {
            out.dropped += 1;
            continue;
        }
        out.items.push(Item { key, index, value });
    }
    out.reads = reads;
    Ok(out)
}

/// An item's key when nothing says otherwise: a string or an integer as
/// itself, a target by its connection, anything else by its JSON text.
fn default_key(value: &Json) -> PathSegment {
    match value {
        Json::String(s) => PathSegment::Key(s.clone()),
        Json::Number(n) if n.is_i64() => PathSegment::Index(n.as_i64().unwrap_or_default()),
        Json::Object(m) if m.len() == 1 && m.get("conn").is_some_and(Json::is_string) => {
            PathSegment::Key(m["conn"].as_str().unwrap_or_default().to_owned())
        }
        other => PathSegment::Key(other.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Detach (composition §5)

/// An instance's params as a plain entity's own: each template param with
/// the instance's binding as its default, so the body reads them as it
/// did. With `item`, `item`, `index` and `key` are written in.
pub fn detached_params(
    defs: &BTreeMap<String, ParamDef>,
    bindings: &BTreeMap<String, Source<Json>>,
    item: Option<&Item>,
) -> BTreeMap<String, ParamDef> {
    defs.iter()
        .map(|(name, def)| {
            let default = bindings
                .get(name)
                .map(|s| match item {
                    Some(item) => written_in(s, item),
                    None => s.clone(),
                })
                .or_else(|| def.default.clone());
            let def = ParamDef {
                default,
                // It has a value now, whether the template required one or not.
                required: None,
                schema: def.schema.clone(),
            };
            (name.clone(), def)
        })
        .collect()
}

/// A binding with a `forEach` item written in: `{ref: {scope: "item"}}`
/// becomes the value it names, and an expression is wrapped in a function
/// that's given the item.
fn written_in(source: &Source<Json>, item: &Item) -> Source<Json> {
    let values = item.values();
    let constant = |v: &Value| crate::source::literal_expr(&crate::expr::to_json(v));
    let args = values
        .iter()
        .map(|(_, v)| constant(v))
        .collect::<Vec<_>>()
        .join(", ");
    match source {
        Source::Ref {
            at:
                Ref::Scope {
                    scope: ScopeName::Item,
                    path,
                },
            fallback,
        } => {
            let mut v = item.value.clone();
            for seg in path {
                v = match (seg, v) {
                    (PathSegment::Key(k), Json::Object(mut m)) => m.remove(k).unwrap_or(Json::Null),
                    (PathSegment::Index(i), Json::Array(a)) => {
                        a.get(*i as usize).cloned().unwrap_or(Json::Null)
                    }
                    _ => Json::Null,
                };
            }
            match (v, fallback) {
                (Json::Null, Some(f)) => Source::Literal(f.clone()),
                (v, _) => Source::Literal(v),
            }
        }
        Source::Expr { expr, fallback } => {
            let body_like = expr.contains("return") || expr.contains(';');
            let expr = if body_like {
                format!("(function (item, index, key) {{\n{expr}\n}})({args})")
            } else {
                format!("((item, index, key) => (\n{expr}\n))({args})")
            };
            Source::Expr {
                expr,
                fallback: fallback.clone(),
            }
        }
        other => other.clone(),
    }
}

fn key(k: &str) -> PathSegment {
    PathSegment::Key(k.to_owned())
}

/// A path under `base`.
pub fn under(base: &[PathSegment], more: &[PathSegment]) -> Path {
    base.iter().chain(more).cloned().collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::*;
    use crate::expr::QuickJs;
    use crate::state::LiveState;

    fn defs(json: Json) -> BTreeMap<String, ParamDef> {
        serde_json::from_value(json).unwrap()
    }

    fn bindings(json: Json) -> BTreeMap<String, Source<Json>> {
        serde_json::from_value(json).unwrap()
    }

    fn codes(problems: &[Problem]) -> Vec<Code> {
        problems.iter().map(|p| p.code).collect()
    }

    #[test]
    fn bindings_are_checked_against_the_template() {
        let d = defs(json!({
            "recorder": { "kind": "target", "required": true },
            "label": { "type": "string", "default": "REC" },
            "onTake": { "kind": "flow" },
        }));
        assert!(
            check_bindings(
                &d,
                &bindings(json!({ "recorder": { "conn": "r1" } })),
                false
            )
            .is_empty()
        );
        assert_eq!(
            codes(&check_bindings(&d, &bindings(json!({})), false)),
            [Code::BadInstance]
        );
        assert_eq!(
            codes(&check_bindings(
                &d,
                &bindings(json!({ "recorder": 1, "label": 3, "nope": 1 })),
                false
            )),
            [Code::BadInstance, Code::UnknownParam]
        );
        assert_eq!(
            codes(&check_bindings(
                &d,
                &bindings(json!({ "recorder": 1, "onTake": { "expr": "vars.f" } })),
                false
            )),
            [Code::TemplateNotLiteral]
        );
        let item = bindings(json!({ "recorder": { "ref": { "scope": "item", "path": [] } } }));
        assert!(check_bindings(&d, &item, true).is_empty());
        assert_eq!(codes(&check_bindings(&d, &item, false)), [Code::NotInScope]);
    }

    #[test]
    fn a_template_change_lists_the_instances_it_breaks() {
        let d = defs(
            json!({ "recorder": { "required": true }, "colour": { "type": "string", "required": true } }),
        );
        let a = bindings(json!({ "recorder": 1, "colour": "red" }));
        let b = bindings(json!({ "recorder": 2 }));
        let broken = broken_instances(&d, [("a", &a, false), ("b", &b, false)]);
        assert_eq!(broken.len(), 1);
        assert_eq!(broken[0].0, "b");
    }

    #[test]
    fn nesting_finds_loops_and_depth() {
        let g: BTreeMap<&str, Vec<&str>> =
            BTreeMap::from([("a", vec!["b"]), ("b", vec!["c"]), ("c", vec!["a"])]);
        let edges = |n: &str| {
            g.get(n)
                .map_or(Vec::new(), |v| v.iter().map(|s| s.to_string()).collect())
        };
        let (code, trail) = check_nesting("a", &edges).unwrap_err();
        assert_eq!(code, Code::RecursiveTemplate);
        assert_eq!(trail, ["a", "b", "c", "a"]);
        let chain: Vec<String> = (0..20).map(|i| i.to_string()).collect();
        let deep = |n: &str| {
            let i: usize = n.parse().unwrap_or(0);
            chain.get(i + 1).cloned().into_iter().collect()
        };
        assert_eq!(check_nesting("0", &deep).unwrap_err().0, Code::TooDeep);
    }

    #[test]
    fn items_have_keys_and_a_limit() {
        let state = Arc::new(LiveState::new());
        let aliases = BTreeMap::new();
        let scope = Scope::new(&state, &aliases);
        let js = QuickJs::default();
        let list: Vec<Json> = (0..300)
            .map(|i| json!({ "name": format!("cam{i}") }))
            .collect();
        let fe: ForEach = serde_json::from_value(json!({
            "items": { "value": list },
            "key": "item.name",
        }))
        .unwrap();
        let items = expand(&fe, &scope, &js, &[]).unwrap();
        assert_eq!(items.items.len(), MAX_ITEMS);
        assert_eq!(items.dropped, 300 - MAX_ITEMS);
        assert_eq!(items.items[2].key, PathSegment::Key("cam2".into()));

        let fe: ForEach = serde_json::from_value(json!({ "items": ["a", "b", "a", 7] })).unwrap();
        let items = expand(&fe, &scope, &js, &[]).unwrap();
        let keys: Vec<_> = items.items.iter().map(|i| i.key.clone()).collect();
        assert_eq!(
            keys,
            [
                PathSegment::Key("a".into()),
                PathSegment::Key("b".into()),
                PathSegment::Index(7)
            ]
        );
        assert_eq!(items.duplicates, [PathSegment::Key("a".into())]);

        let fe: ForEach =
            serde_json::from_value(json!({ "targets": { "plugin": "rec" } })).unwrap();
        let views = [
            TargetView {
                conn: "r2".into(),
                plugin: "rec".into(),
            },
            TargetView {
                conn: "x".into(),
                plugin: "other".into(),
            },
            TargetView {
                conn: "r1".into(),
                plugin: "rec".into(),
            },
        ];
        let items = expand(&fe, &scope, &js, &views).unwrap();
        assert_eq!(
            items
                .items
                .iter()
                .map(|i| i.key.clone())
                .collect::<Vec<_>>(),
            [PathSegment::Key("r1".into()), PathSegment::Key("r2".into())]
        );
        assert_eq!(items.items[0].value, json!({ "conn": "r1" }));
    }

    #[test]
    fn detaching_writes_the_item_in() {
        let d = defs(json!({
            "recorder": { "kind": "target", "required": true },
            "label": { "type": "string" },
            "speed": { "type": "number", "default": 2 },
        }));
        let b = bindings(json!({
            "recorder": { "ref": { "scope": "item", "path": [] } },
            "label": { "expr": "`Rec ${key}`" },
        }));
        let item = Item {
            key: PathSegment::Key("r1".into()),
            index: 0,
            value: json!({ "conn": "r1" }),
        };
        let params = detached_params(&d, &b, Some(&item));
        assert_eq!(
            params["recorder"].default,
            Some(Source::Literal(json!({ "conn": "r1" })))
        );
        assert_eq!(params["recorder"].required, None);
        assert_eq!(params["speed"].default, Some(Source::Literal(json!(2))));
        let Some(Source::Expr { expr, .. }) = &params["label"].default else {
            panic!("an expression stays one");
        };
        let state = Arc::new(LiveState::new());
        let aliases = BTreeMap::new();
        let v = QuickJs::default()
            .evaluate(expr, &Scope::new(&state, &aliases))
            .value
            .unwrap();
        assert_eq!(v, Some(Value::from("Rec r1")));
    }

    #[test]
    fn chains_match_by_instance_key_and_template() {
        let chain = vec![
            Link {
                template: "cams".into(),
                instance: "rig".into(),
                key: Some(PathSegment::Key("camB".into())),
            },
            Link {
                template: "tally".into(),
                instance: "k1".into(),
                key: None,
            },
        ];
        assert!(chain_has(&chain, "rig", None));
        assert!(chain_has(
            &chain,
            "rig",
            Some(&PathSegment::Key("camB".into()))
        ));
        assert!(!chain_has(
            &chain,
            "rig",
            Some(&PathSegment::Key("camA".into()))
        ));
        assert!(chain_has(&chain, "k1", None));
        assert!(chain_uses(&chain, "tally"));
        assert!(!chain_uses(&chain, "rig"));
    }
}

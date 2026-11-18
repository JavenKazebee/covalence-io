//! Selectors (dataflow spec §2, §10.1): the one shape for picking things.
//!
//! A [`Selector<D>`] is an ID, a list of IDs, `{expr}`, the combinators
//! `{any}`, `{all}` and `{not}`, or one of its domain's own forms. Three
//! domains are implemented:
//!
//! | Domain | ID | Forms | Resolves to |
//! |---|---|---|---|
//! | [`Controls`] | a control ID | `region`, `near`, `has` | control IDs, by `at`, row-major |
//! | [`Targets`] | a [`Target`] | `plugin`, `interface` | targets, by connection ID |
//! | [`Paths`] | a [`Ref`] | `match` | a match against a path, with captures |
//!
//! Controls and targets resolve within a universe the caller gives (a
//! surface's or a layer's controls, the project's connections), so `not`
//! means "the rest of it". Paths have no universe: a path selector is a
//! test applied to one path at a time, and `not` only narrows inside `all`.
//!
//! `{expr}` is evaluated through an [`Eval`], whose reads say when a
//! reactive selector has to resolve again. Without one, an `expr`
//! resolves to nothing. An ID that isn't in the universe resolves to
//! nothing; validation ([`Selector::problems`]) reports the ones written
//! outside an `expr`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Debug;

use covalence_protocol::state::{Path, PathSegment};
use serde::de::{DeserializeOwned, Error as _};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value as Json};

use crate::expr::{ExprError, RESERVED_ROOTS, to_json};
use crate::source::{Eval, Ref, Root};
use crate::state::{ConnId, StatePath};
use crate::surface::{InputCapKind, OutputCapKind, Placement};
use crate::text_path;

/// What a selector picks from.
pub trait Domain: Clone + Debug + PartialEq {
    type Id: Clone + Debug + PartialEq + Serialize + DeserializeOwned;
    type Form: Clone + Debug + PartialEq + Serialize + DeserializeOwned;
    /// The keys that make an object one of the domain's forms.
    const FORMS: &'static [&'static str];
}

/// Picks a set of things (dataflow §2).
#[derive(Clone, Debug, PartialEq)]
pub enum Selector<D: Domain> {
    One(D::Id),
    List(Vec<D::Id>),
    /// An ID, a list of IDs, or (for targets) targets, computed.
    Expr(String),
    /// Union.
    Any(Vec<Selector<D>>),
    /// Intersection.
    All(Vec<Selector<D>>),
    /// Everything in the universe except these.
    Not(Box<Selector<D>>),
    Form(D::Form),
}

const COMBINATORS: [&str; 4] = ["expr", "any", "all", "not"];

// ---------------------------------------------------------------------------
// Serde

impl<D: Domain> Selector<D> {
    pub fn from_json(json: Json) -> Result<Self, String> {
        match json {
            Json::Array(items) => items
                .into_iter()
                .enumerate()
                .map(|(i, v)| typed(v).map_err(|e| format!("[{i}]: {e}")))
                .collect::<Result<_, _>>()
                .map(Selector::List),
            Json::Object(map) => Self::from_object(map),
            other => typed(other).map(Selector::One),
        }
    }

    fn from_object(mut map: Map<String, Json>) -> Result<Self, String> {
        let combinator: Vec<&str> = COMBINATORS
            .into_iter()
            .filter(|k| map.contains_key(*k))
            .collect();
        if let [key] = combinator.as_slice() {
            if map.len() != 1 {
                let extra = map.keys().find(|k| k != key).expect("another key");
                return Err(format!("{extra:?} can't sit beside `{key}` in a selector"));
            }
            let inner = map.remove(*key).expect("present");
            let list = |inner: Json| match inner {
                Json::Array(items) => items
                    .into_iter()
                    .enumerate()
                    .map(|(i, v)| Self::from_json(v).map_err(|e| format!("{key}[{i}]: {e}")))
                    .collect::<Result<Vec<_>, _>>(),
                _ => Err(format!("`{key}` takes a list of selectors")),
            };
            return match *key {
                "expr" => match inner {
                    Json::String(e) => Ok(Selector::Expr(e)),
                    _ => Err("`expr` must be the expression's text".into()),
                },
                "any" => list(inner).map(Selector::Any),
                "all" => list(inner).map(Selector::All),
                _ => Self::from_json(inner)
                    .map(|s| Selector::Not(Box::new(s)))
                    .map_err(|e| format!("not: {e}")),
            };
        }
        if combinator.len() > 1 {
            return Err(format!(
                "a selector has one of `expr`, `any`, `all` or `not`, not {}",
                combinator.join(" and ")
            ));
        }
        if D::FORMS.iter().any(|k| map.contains_key(*k)) {
            return typed(Json::Object(map)).map(Selector::Form);
        }
        typed(Json::Object(map)).map(Selector::One)
    }

    pub fn to_json(&self) -> Result<Json, serde_json::Error> {
        let list = |items: &[Selector<D>]| -> Result<Json, serde_json::Error> {
            items
                .iter()
                .map(Selector::to_json)
                .collect::<Result<Vec<_>, _>>()
                .map(Json::Array)
        };
        Ok(match self {
            Selector::One(id) => serde_json::to_value(id)?,
            Selector::List(ids) => serde_json::to_value(ids)?,
            Selector::Expr(e) => serde_json::json!({ "expr": e }),
            Selector::Any(items) => serde_json::json!({ "any": list(items)? }),
            Selector::All(items) => serde_json::json!({ "all": list(items)? }),
            Selector::Not(inner) => serde_json::json!({ "not": inner.to_json()? }),
            Selector::Form(form) => serde_json::to_value(form)?,
        })
    }

    /// Calls `f` for every ID written in the selector (not computed by an
    /// `expr`), with its stored path below the selector.
    pub fn each_id<'s>(&'s self, here: &mut Path, f: &mut dyn FnMut(&Path, &'s D::Id)) {
        let mut nested = |key: &str, items: &'s [Selector<D>], here: &mut Path| {
            for (i, item) in items.iter().enumerate() {
                here.push(PathSegment::Key(key.into()));
                here.push(PathSegment::Index(i as i64));
                item.each_id(here, f);
                here.truncate(here.len() - 2);
            }
        };
        match self {
            Selector::One(id) => f(here, id),
            Selector::List(ids) => {
                for (i, id) in ids.iter().enumerate() {
                    here.push(PathSegment::Index(i as i64));
                    f(here, id);
                    here.pop();
                }
            }
            Selector::Any(items) => nested("any", items, here),
            Selector::All(items) => nested("all", items, here),
            Selector::Not(inner) => {
                here.push(PathSegment::Key("not".into()));
                inner.each_id(here, f);
                here.pop();
            }
            Selector::Expr(_) | Selector::Form(_) => {}
        }
    }

    /// Calls `f` for every domain form, with its stored path.
    pub fn each_form<'s>(&'s self, here: &mut Path, f: &mut dyn FnMut(&Path, &'s D::Form)) {
        match self {
            Selector::Form(form) => f(here, form),
            Selector::Any(items) | Selector::All(items) => {
                let key = if matches!(self, Selector::Any(_)) {
                    "any"
                } else {
                    "all"
                };
                for (i, item) in items.iter().enumerate() {
                    here.push(PathSegment::Key(key.into()));
                    here.push(PathSegment::Index(i as i64));
                    item.each_form(here, f);
                    here.truncate(here.len() - 2);
                }
            }
            Selector::Not(inner) => {
                here.push(PathSegment::Key("not".into()));
                inner.each_form(here, f);
                here.pop();
            }
            _ => {}
        }
    }

    /// The `expr`s in the selector, with their stored paths.
    pub fn exprs(&self) -> Vec<(Path, &str)> {
        let mut out = Vec::new();
        self.collect_exprs(&mut Vec::new(), &mut out);
        out
    }

    fn collect_exprs<'s>(&'s self, here: &mut Path, out: &mut Vec<(Path, &'s str)>) {
        match self {
            Selector::Expr(e) => {
                let mut at = here.clone();
                at.push(PathSegment::Key("expr".into()));
                out.push((at, e));
            }
            Selector::Any(items) | Selector::All(items) => {
                let key = if matches!(self, Selector::Any(_)) {
                    "any"
                } else {
                    "all"
                };
                for (i, item) in items.iter().enumerate() {
                    here.push(PathSegment::Key(key.into()));
                    here.push(PathSegment::Index(i as i64));
                    item.collect_exprs(here, out);
                    here.truncate(here.len() - 2);
                }
            }
            Selector::Not(inner) => {
                here.push(PathSegment::Key("not".into()));
                inner.collect_exprs(here, out);
                here.pop();
            }
            _ => {}
        }
    }
}

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

impl<'de, D: Domain> Deserialize<'de> for Selector<D> {
    fn deserialize<De: Deserializer<'de>>(deserializer: De) -> Result<Self, De::Error> {
        Selector::from_json(Json::deserialize(deserializer)?).map_err(De::Error::custom)
    }
}

impl<D: Domain> Serialize for Selector<D> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_json()
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}

// ---------------------------------------------------------------------------
// Resolution within a universe

/// Evaluates an `expr` for its result as JSON. Without an evaluator, nothing.
fn expr_value(expr: &str, eval: &mut Option<&mut Eval<'_>>) -> Result<Option<Json>, ExprError> {
    let Some(eval) = eval else {
        return Ok(None);
    };
    let evaluation = eval.evaluator.evaluate(expr, eval.scope);
    eval.reads.extend(evaluation.reads.paths);
    evaluation.value.map(|v| v.as_ref().map(to_json))
}

/// The items of an `expr`'s result: one value, or each item of a list.
fn items(value: Option<Json>) -> Vec<Json> {
    match value {
        None | Some(Json::Null) => Vec::new(),
        Some(Json::Array(items)) => items,
        Some(other) => vec![other],
    }
}

/// Resolves to positions in a universe of `n` items.
fn resolve_in<D: Domain>(
    selector: &Selector<D>,
    n: usize,
    index_of: &dyn Fn(&D::Id) -> Option<usize>,
    from_expr: &dyn Fn(&Json) -> Option<D::Id>,
    form: &dyn Fn(&D::Form) -> BTreeSet<usize>,
    eval: &mut Option<&mut Eval<'_>>,
) -> Result<BTreeSet<usize>, ExprError> {
    let again = |s: &Selector<D>, eval: &mut Option<&mut Eval<'_>>| {
        resolve_in(s, n, index_of, from_expr, form, eval)
    };
    Ok(match selector {
        Selector::One(id) => index_of(id).into_iter().collect(),
        Selector::List(ids) => ids.iter().filter_map(index_of).collect(),
        Selector::Expr(e) => items(expr_value(e, eval)?)
            .iter()
            .filter_map(from_expr)
            .filter_map(|id| index_of(&id))
            .collect(),
        Selector::Any(items) => {
            let mut out = BTreeSet::new();
            for item in items {
                out.extend(again(item, eval)?);
            }
            out
        }
        Selector::All(items) => {
            let mut out: Option<BTreeSet<usize>> = None;
            for item in items {
                let set = again(item, eval)?;
                out = Some(match out {
                    None => set,
                    Some(prev) => prev.intersection(&set).copied().collect(),
                });
            }
            // `all` of nothing is everything, as `[].every()` is true.
            out.unwrap_or_else(|| (0..n).collect())
        }
        Selector::Not(inner) => {
            let excluded = again(inner, eval)?;
            (0..n).filter(|i| !excluded.contains(i)).collect()
        }
        Selector::Form(f) => form(f),
    })
}

// ---------------------------------------------------------------------------
// Controls

#[derive(Clone, Debug, PartialEq)]
pub struct Controls;

/// A rectangle in grid cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Left,
    Right,
    Above,
    Below,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Has {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<InputCapKind>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<OutputCapKind>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum ControlForm {
    /// Every control whose `at` lies inside.
    Region { region: Rect },
    /// The nearest control in a direction, overlapping `of` across it.
    Near { near: Direction, of: String },
    /// Every control with all of these capabilities.
    Has { has: Has },
}

impl Domain for Controls {
    type Id = String;
    type Form = ControlForm;
    const FORMS: &'static [&'static str] = &["region", "near", "has"];
}

/// A control as a selector sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct ControlView {
    pub id: String,
    /// A layer's control may have no position; regions and neighbours skip it.
    pub at: Option<Placement>,
    pub inputs: Vec<InputCapKind>,
    pub outputs: Vec<OutputCapKind>,
}

impl From<&crate::surface::ControlDef> for ControlView {
    fn from(c: &crate::surface::ControlDef) -> Self {
        ControlView {
            id: c.id.clone(),
            at: Some(c.at),
            inputs: c.inputs.iter().map(|i| i.kind()).collect(),
            outputs: c.outputs.iter().map(|o| o.kind()).collect(),
        }
    }
}

impl From<&crate::surface::LayerControl> for ControlView {
    fn from(c: &crate::surface::LayerControl) -> Self {
        ControlView {
            id: c.id.clone(),
            at: c.at,
            inputs: c.inputs.clone(),
            outputs: c.outputs.clone(),
        }
    }
}

/// `(x, y, w, h)` with sizes defaulted.
fn cells(p: &Placement) -> (u64, u64, u64, u64) {
    (
        u64::from(p.x),
        u64::from(p.y),
        u64::from(p.w.unwrap_or(1)),
        u64::from(p.h.unwrap_or(1)),
    )
}

/// How far `c` is from `of` in `dir`, if it's that way and they overlap across it.
fn distance(dir: Direction, of: &Placement, c: &Placement) -> Option<u64> {
    let (ox, oy, ow, oh) = cells(of);
    let (cx, cy, cw, ch) = cells(c);
    let rows_overlap = cy < oy + oh && oy < cy + ch;
    let cols_overlap = cx < ox + ow && ox < cx + cw;
    match dir {
        Direction::Right if rows_overlap && cx >= ox + ow => Some(cx - (ox + ow)),
        Direction::Left if rows_overlap && cx + cw <= ox => Some(ox - (cx + cw)),
        Direction::Below if cols_overlap && cy >= oy + oh => Some(cy - (oy + oh)),
        Direction::Above if cols_overlap && cy + ch <= oy => Some(oy - (cy + ch)),
        _ => None,
    }
}

impl Selector<Controls> {
    /// The controls it picks, in the domain's order: by `at`, row-major,
    /// then controls with no position in the order given.
    pub fn resolve(
        &self,
        controls: &[ControlView],
        mut eval: Option<&mut Eval<'_>>,
    ) -> Result<Vec<String>, ExprError> {
        let index_of = |id: &String| controls.iter().position(|c| &c.id == id);
        let from_expr = |v: &Json| v.as_str().map(str::to_owned);
        let form = |f: &ControlForm| -> BTreeSet<usize> {
            match f {
                ControlForm::Region { region } => {
                    let (rx, ry, rw, rh) = (
                        u64::from(region.x),
                        u64::from(region.y),
                        u64::from(region.w),
                        u64::from(region.h),
                    );
                    (0..controls.len())
                        .filter(|&i| {
                            controls[i].at.as_ref().is_some_and(|p| {
                                let (x, y, w, h) = cells(p);
                                x >= rx && y >= ry && x + w <= rx + rw && y + h <= ry + rh
                            })
                        })
                        .collect()
                }
                ControlForm::Near { near, of } => {
                    let Some(origin) = controls.iter().find(|c| &c.id == of).and_then(|c| c.at)
                    else {
                        return BTreeSet::new();
                    };
                    (0..controls.len())
                        .filter(|&i| &controls[i].id != of)
                        .filter_map(|i| {
                            Some((distance(*near, &origin, controls[i].at.as_ref()?)?, i))
                        })
                        .min_by_key(|&(d, i)| (d, order_key(&controls[i], i)))
                        .map(|(_, i)| i)
                        .into_iter()
                        .collect()
                }
                ControlForm::Has { has } => (0..controls.len())
                    .filter(|&i| {
                        has.inputs.iter().all(|k| controls[i].inputs.contains(k))
                            && has.outputs.iter().all(|k| controls[i].outputs.contains(k))
                    })
                    .collect(),
            }
        };
        let picked = resolve_in(
            self,
            controls.len(),
            &index_of,
            &from_expr,
            &form,
            &mut eval,
        )?;
        let mut picked: Vec<usize> = picked.into_iter().collect();
        picked.sort_by_key(|&i| order_key(&controls[i], i));
        Ok(picked.into_iter().map(|i| controls[i].id.clone()).collect())
    }
}

/// Row-major by `at`; controls with no position after, in the order given.
fn order_key(c: &ControlView, i: usize) -> (bool, u32, u32, usize) {
    match c.at {
        Some(p) => (false, p.y, p.x, i),
        None => (true, 0, 0, i),
    }
}

// ---------------------------------------------------------------------------
// Targets

#[derive(Clone, Debug, PartialEq)]
pub struct Targets;

/// A connection, or an item within one (plugin protocol spec §13.9). A string is a connection ID.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Target {
    pub conn: ConnId,
    pub key: Option<String>,
}

impl Target {
    pub fn conn(conn: impl Into<ConnId>) -> Self {
        Target {
            conn: conn.into(),
            key: None,
        }
    }
}

impl<'de> Deserialize<'de> for Target {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire {
            Conn(ConnId),
            Item {
                conn: ConnId,
                #[serde(default)]
                key: Option<String>,
            },
        }
        // `deny_unknown_fields` can't be combined with `untagged`, so the
        // object form is checked by hand.
        let json = Json::deserialize(deserializer)?;
        if let Json::Object(map) = &json
            && let Some(extra) = map.keys().find(|k| *k != "conn" && *k != "key")
        {
            return Err(D::Error::custom(format!(
                "{extra:?} isn't part of a target, which is {{conn, key?}}"
            )));
        }
        match serde_json::from_value(json)
            .map_err(|_| D::Error::custom("a target is a connection ID or {conn, key?}"))?
        {
            Wire::Conn(conn) => Ok(Target::conn(conn)),
            Wire::Item { conn, key } => Ok(Target { conn, key }),
        }
    }
}

impl Serialize for Target {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match &self.key {
            None => self.conn.serialize(serializer),
            Some(key) => serde_json::json!({ "conn": self.conn, "key": key }).serialize(serializer),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum TargetForm {
    /// Every connection of a plugin, by plugin ID.
    Plugin { plugin: String },
    /// Every connection implementing an interface, such as `router@1`.
    Interface { interface: String },
}

impl Domain for Targets {
    type Id = Target;
    type Form = TargetForm;
    const FORMS: &'static [&'static str] = &["plugin", "interface"];
}

/// A connection as a selector sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct TargetView {
    pub conn: ConnId,
    pub plugin: String,
    /// The interfaces it implements, by name and major version: `router@1`.
    pub interfaces: Vec<String>,
}

impl Selector<Targets> {
    /// The connections it picks, by connection ID. Items within a
    /// connection (`key`) wait for plugin protocol spec §13.9's targets, so a target with a
    /// key resolves to nothing.
    pub fn resolve(
        &self,
        conns: &[TargetView],
        mut eval: Option<&mut Eval<'_>>,
    ) -> Result<Vec<Target>, ExprError> {
        let mut sorted: Vec<&TargetView> = conns.iter().collect();
        sorted.sort_by(|a, b| a.conn.cmp(&b.conn));
        let index_of = |t: &Target| {
            t.key
                .is_none()
                .then(|| sorted.iter().position(|c| c.conn == t.conn))
                .flatten()
        };
        let from_expr = |v: &Json| serde_json::from_value::<Target>(v.clone()).ok();
        let form = |f: &TargetForm| -> BTreeSet<usize> {
            (0..sorted.len())
                .filter(|&i| match f {
                    TargetForm::Plugin { plugin } => &sorted[i].plugin == plugin,
                    TargetForm::Interface { interface } => {
                        sorted[i].interfaces.iter().any(|x| x == interface)
                    }
                })
                .collect()
        };
        let picked = resolve_in(self, sorted.len(), &index_of, &from_expr, &form, &mut eval)?;
        Ok(picked
            .into_iter()
            .map(|i| Target::conn(sorted[i].conn.clone()))
            .collect())
    }
}

// ---------------------------------------------------------------------------
// Paths

#[derive(Clone, Debug, PartialEq)]
pub struct Paths;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathForm {
    /// A text path where `*` matches one segment: `surface.*.controls.*.pressed`.
    #[serde(rename = "match")]
    pub pattern: String,
}

/// A path a selector names: a [`Ref`] in ID form, or text for a reserved
/// root (`"sys"`, `"flow.preService"`). A connection's path is always
/// `{conn, path}`, so renaming an alias never breaks it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathRef(pub Ref);

impl PathRef {
    pub fn state_path(&self) -> StatePath {
        self.0.state_path()
    }
}

fn reserved_root(name: &str) -> Option<Root> {
    [
        ("vars", Root::Vars),
        ("sys", Root::Sys),
        ("surface", Root::Surface),
        ("layer", Root::Layer),
        ("flow", Root::Flow),
    ]
    .into_iter()
    .find(|(n, _)| *n == name)
    .map(|(_, r)| r)
}

impl<'de> Deserialize<'de> for PathRef {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match Json::deserialize(deserializer)? {
            Json::String(text) => {
                let (root, path) = text_path::parse(&text)
                    .map_err(|e| D::Error::custom(format!("{text:?} isn't a path: {}", e.why)))?;
                let root = reserved_root(&root).ok_or_else(|| {
                    D::Error::custom(format!(
                        "{root:?} isn't a reserved root: a connection's path is written {{conn, path}}"
                    ))
                })?;
                Ok(PathRef(Ref::Root { root, path }))
            }
            other => serde_json::from_value(other)
                .map(PathRef)
                .map_err(|_| D::Error::custom("a path is text such as \"sys\", or {conn, path}")),
        }
    }
}

impl Serialize for PathRef {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match &self.0 {
            Ref::Root { root, path } => {
                text_path::format(root.as_str(), path).serialize(serializer)
            }
            conn => conn.serialize(serializer),
        }
    }
}

impl Domain for Paths {
    type Id = PathRef;
    type Form = PathForm;
    const FORMS: &'static [&'static str] = &["match"];
}

/// A parsed [`PathForm`]: a root and segments, each fixed or `*`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pattern {
    pub root: Wild<String>,
    pub segments: Vec<Wild<PathSegment>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Wild<T> {
    Any,
    Is(T),
}

/// Stands for `*` while a pattern goes through the text path parser.
const STAR: &str = "__covalence_any__";

impl Pattern {
    pub fn parse(text: &str) -> Result<Pattern, String> {
        // `*` as a whole segment only: `a.*.b`, `*.b`, `a[*]`.
        let mut prepared = String::new();
        let mut chars = text.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            if c == '*' {
                let before = text[..i].chars().last();
                let after = text[i + 1..].chars().next();
                let starts = matches!(before, None | Some('.'));
                let in_brackets = before == Some('[') && after == Some(']');
                if in_brackets {
                    prepared.pop();
                    prepared.push('.');
                    prepared.push_str(STAR);
                    chars.next();
                    continue;
                }
                if !starts || !matches!(after, None | Some('.') | Some('[')) {
                    return Err(format!("`*` must be a whole segment, at {i}"));
                }
                prepared.push_str(STAR);
            } else {
                prepared.push(c);
            }
        }
        let (root, path) = text_path::parse(&prepared)
            .map_err(|e| format!("{text:?} isn't a path pattern: {}", e.why))?;
        let wild = |s: PathSegment| match s {
            PathSegment::Key(k) if k == STAR => Wild::Any,
            other => Wild::Is(other),
        };
        Ok(Pattern {
            root: if root == STAR {
                Wild::Any
            } else {
                Wild::Is(root)
            },
            segments: path.into_iter().map(wild).collect(),
        })
    }

    /// The segments `*` matched, if `path` matches. A root is a reserved
    /// root or a connection alias, which `aliases` maps to its ID; a `*`
    /// root captures the connection ID or reserved root.
    pub fn captures(
        &self,
        path: &StatePath,
        aliases: &BTreeMap<String, ConnId>,
    ) -> Option<Vec<String>> {
        if path.path.len() != self.segments.len() {
            return None;
        }
        let mut out = Vec::new();
        match &self.root {
            Wild::Any => out.push(path.conn.clone()),
            Wild::Is(name) => {
                let conn = if is_reserved_root(name) {
                    name
                } else {
                    aliases.get(name)?
                };
                if conn != &path.conn {
                    return None;
                }
            }
        }
        for (want, got) in self.segments.iter().zip(&path.path) {
            match want {
                Wild::Any => out.push(match got {
                    PathSegment::Key(k) => k.clone(),
                    PathSegment::Index(i) => i.to_string(),
                }),
                Wild::Is(seg) if seg == got => {}
                Wild::Is(_) => return None,
            }
        }
        Some(out)
    }
}

fn is_reserved_root(name: &str) -> bool {
    RESERVED_ROOTS.iter().any(|(n, _)| *n == name)
}

/// A text path or a [`Ref`] from an expression, in ID form.
fn ref_from(v: &Json, aliases: &BTreeMap<String, ConnId>) -> Option<Ref> {
    match v {
        Json::String(text) => {
            let (root, path) = text_path::parse(text).ok()?;
            Some(match reserved_root(&root) {
                Some(root) => Ref::Root { root, path },
                None => Ref::Conn {
                    conn: aliases.get(&root)?.clone(),
                    path,
                },
            })
        }
        other => serde_json::from_value(other.clone()).ok(),
    }
}

impl Selector<Paths> {
    /// Whether `path` is picked, and the segments the first matching
    /// pattern's `*`s captured (none for a path given by ID).
    pub fn matches(
        &self,
        path: &StatePath,
        aliases: &BTreeMap<String, ConnId>,
        mut eval: Option<&mut Eval<'_>>,
    ) -> Result<Option<Vec<String>>, ExprError> {
        self.test(path, aliases, &mut eval)
    }

    fn test(
        &self,
        path: &StatePath,
        aliases: &BTreeMap<String, ConnId>,
        eval: &mut Option<&mut Eval<'_>>,
    ) -> Result<Option<Vec<String>>, ExprError> {
        Ok(match self {
            Selector::One(r) => (&r.state_path() == path).then(Vec::new),
            Selector::List(refs) => refs.iter().any(|r| &r.state_path() == path).then(Vec::new),
            Selector::Expr(e) => items(expr_value(e, eval)?)
                .iter()
                .filter_map(|v| ref_from(v, aliases))
                .any(|r| &r.state_path() == path)
                .then(Vec::new),
            Selector::Form(form) => Pattern::parse(&form.pattern)
                .ok()
                .and_then(|p| p.captures(path, aliases)),
            Selector::Any(items) => {
                for item in items {
                    if let Some(c) = item.test(path, aliases, eval)? {
                        return Ok(Some(c));
                    }
                }
                None
            }
            Selector::All(items) => {
                let mut captures: Option<Vec<String>> = None;
                for item in items {
                    match item.test(path, aliases, eval)? {
                        None => return Ok(None),
                        Some(c) => {
                            if captures.as_ref().is_none_or(Vec::is_empty) {
                                captures = Some(c);
                            }
                        }
                    }
                }
                Some(captures.unwrap_or_default())
            }
            Selector::Not(inner) => match inner.test(path, aliases, eval)? {
                Some(_) => None,
                None => Some(Vec::new()),
            },
        })
    }

    /// Patterns written in the selector, for indexing: a path can only
    /// match if it matches one of them or one of its IDs. `None` if an
    /// `expr` or a bare `not` makes that impossible to know without
    /// evaluating.
    pub fn patterns(&self) -> Option<Vec<Pattern>> {
        match self {
            Selector::Form(form) => Pattern::parse(&form.pattern).ok().map(|p| vec![p]),
            Selector::One(_) | Selector::List(_) => Some(Vec::new()),
            Selector::Any(items) => {
                let mut out = Vec::new();
                for item in items {
                    out.extend(item.patterns()?);
                }
                Some(out)
            }
            // Every item has to match, so the first that narrows will do.
            Selector::All(items) => items.iter().find_map(Selector::patterns),
            Selector::Expr(_) | Selector::Not(_) => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Validation

/// A problem with a selector as written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Issue {
    /// An ID the universe doesn't have.
    Unknown(String),
    /// A form that can't work as written.
    Bad(String),
}

impl Selector<Controls> {
    /// IDs not among `known`, and `near` origins likewise. IDs computed by
    /// an `expr` aren't checked: they resolve to nothing at run time.
    pub fn problems(&self, known: &dyn Fn(&str) -> bool) -> Vec<(Path, Issue)> {
        let mut out = Vec::new();
        self.each_id(&mut Vec::new(), &mut |at, id| {
            if !known(id) {
                out.push((at.clone(), Issue::Unknown(format!("no control {id:?}"))));
            }
        });
        self.each_form(&mut Vec::new(), &mut |at, form| match form {
            ControlForm::Near { of, .. } if !known(of) => {
                let mut at = at.clone();
                at.push(PathSegment::Key("of".into()));
                out.push((at, Issue::Unknown(format!("no control {of:?}"))));
            }
            ControlForm::Region { region } if region.w == 0 || region.h == 0 => out.push((
                at.clone(),
                Issue::Bad("a region needs a width and a height".into()),
            )),
            _ => {}
        });
        out
    }
}

impl Selector<Targets> {
    pub fn problems(&self, known: &dyn Fn(&ConnId) -> bool) -> Vec<(Path, Issue)> {
        let mut out = Vec::new();
        self.each_id(&mut Vec::new(), &mut |at, t| {
            if t.key.is_some() {
                out.push((
                    at.clone(),
                    Issue::Bad("items within a connection aren't supported yet".into()),
                ));
            } else if !known(&t.conn) {
                out.push((
                    at.clone(),
                    Issue::Unknown(format!("no connection {:?}", t.conn)),
                ));
            }
        });
        out
    }
}

impl Selector<Paths> {
    /// Connections that don't exist, patterns that don't parse, and a
    /// `not` with nothing to narrow.
    pub fn problems(&self, known: &dyn Fn(&ConnId) -> bool) -> Vec<(Path, Issue)> {
        let mut out = Vec::new();
        self.each_id(&mut Vec::new(), &mut |at, r| {
            if let Ref::Conn { conn, .. } = &r.0
                && !known(conn)
            {
                let mut at = at.clone();
                at.push(PathSegment::Key("conn".into()));
                out.push((at, Issue::Unknown(format!("no connection {conn:?}"))));
            }
        });
        self.each_form(&mut Vec::new(), &mut |at, form| {
            if let Err(e) = Pattern::parse(&form.pattern) {
                let mut at = at.clone();
                at.push(PathSegment::Key("match".into()));
                out.push((at, Issue::Bad(e)));
            }
        });
        self.bare_nots(&mut Vec::new(), false, &mut out);
        out
    }

    fn bare_nots(&self, here: &mut Path, in_all: bool, out: &mut Vec<(Path, Issue)>) {
        match self {
            Selector::Not(inner) => {
                if !in_all {
                    out.push((
                        here.clone(),
                        Issue::Bad("paths have no universe: use `not` inside `all`".into()),
                    ));
                }
                here.push(PathSegment::Key("not".into()));
                inner.bare_nots(here, false, out);
                here.pop();
            }
            Selector::Any(items) | Selector::All(items) => {
                let all = matches!(self, Selector::All(_));
                let key = if all { "all" } else { "any" };
                for (i, item) in items.iter().enumerate() {
                    here.push(PathSegment::Key(key.into()));
                    here.push(PathSegment::Index(i as i64));
                    item.bare_nots(here, all, out);
                    here.truncate(here.len() - 2);
                }
            }
            _ => {}
        }
    }
}

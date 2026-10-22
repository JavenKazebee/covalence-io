//! Display bindings (bindings spec §1–§4, ADR 0023): how a control looks,
//! kept in step with state.
//!
//! A [`ControlBinding`] binds properties of one control on one page. Each
//! property's [`Binding`] is one of five kinds. `value`, `template`,
//! `rules` and `map` are evaluated natively ([`Binding::evaluate`]), reading
//! the state tree directly; they also compile to an expression
//! ([`Binding::to_expr`]), which the editor shows and which gives the same
//! result. `expr` bindings run on QuickJS. The worker in [`worker`] keeps
//! the bindings of shown pages evaluated and draws what changes.

use std::collections::{BTreeMap, BTreeSet};

use covalence_protocol::state::{Path, PathSegment};
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value as Json};

use crate::expr::{self, Evaluator};
use crate::flow::{Code, Problem, Report};
use crate::state::{ConnId, LiveState, StatePath};
use crate::surface::render::{Look, Rgb};
use crate::surface::{Layout, PageId};
use crate::text_path;

pub mod js;
pub mod worker;

/// Entity type in the store.
pub const ENTITY: &str = "control_binding";

/// The bindings of one control on one page (bindings spec §1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ControlBinding {
    /// A ULID, assigned by the server.
    #[serde(default)]
    pub id: String,
    pub page: PageId,
    pub control: String,
    pub props: BTreeMap<Prop, Binding>,
}

/// A property of a control. M5 has the three a key image needs. In the
/// order of their names, as JSON objects are written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Prop {
    Background,
    Label,
    TextColor,
}

impl Prop {
    pub const ALL: [Prop; 3] = [Prop::Background, Prop::Label, Prop::TextColor];

    pub fn as_str(self) -> &'static str {
        match self {
            Prop::Label => "label",
            Prop::TextColor => "textColor",
            Prop::Background => "background",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Binding {
    /// One value, formatted.
    Value {
        path: Ref,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        format: Option<Format>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fallback: Option<Json>,
    },
    /// Text with values in it.
    Template {
        parts: Vec<Part>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fallback: Option<String>,
    },
    /// The first rule whose condition holds, or `else`.
    Rules {
        rules: Vec<Rule>,
        #[serde(rename = "else")]
        otherwise: Json,
    },
    /// A number scaled to a range, or used to pick from a list.
    Map {
        path: Ref,
        from: [Number; 2],
        to: MapTo,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        curve: Option<Curve>,
        /// Absent means `true`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        clamp: Option<bool>,
    },
    /// Any expression.
    Expr {
        source: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fallback: Option<Json>,
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
    Page,
    Flow,
}

impl Root {
    pub fn as_str(self) -> &'static str {
        match self {
            Root::Vars => "vars",
            Root::Sys => "sys",
            Root::Surface => "surface",
            Root::Page => "page",
            Root::Flow => "flow",
        }
    }
}

/// How a value becomes text. `auto` is `String(v)` in M5; formatting by the
/// schema's `kind` comes later.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Format {
    Auto,
    Mmss,
    Hhmmss,
    /// 0–1 as a whole percentage.
    Percent,
    Fixed1,
    Fixed2,
    Upper,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Part {
    Text(String),
    Value {
        path: Ref,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        format: Option<Format>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub when: Condition,
    pub value: Json,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Condition {
    pub path: Ref,
    pub op: Op,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Json>,
}

/// `==` and `!=` are strict, as `===` and `!==`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Op {
    #[serde(rename = "==")]
    Eq,
    #[serde(rename = "!=")]
    Ne,
    #[serde(rename = ">")]
    Gt,
    #[serde(rename = ">=")]
    Ge,
    #[serde(rename = "<")]
    Lt,
    #[serde(rename = "<=")]
    Le,
    #[serde(rename = "truthy")]
    Truthy,
    #[serde(rename = "falsy")]
    Falsy,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MapTo {
    Range([Number; 2]),
    List(Vec<Json>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Curve {
    Linear,
    Log,
    Exp,
}

/// What a binding gave: its value (`None` is `undefined`) and the state it read.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub value: Result<Option<Json>, String>,
    pub reads: BTreeSet<StatePath>,
}

impl Binding {
    /// The fallback used when the result is `undefined` or an error.
    pub fn fallback(&self) -> Option<Json> {
        match self {
            Binding::Value { fallback, .. } | Binding::Expr { fallback, .. } => fallback.clone(),
            Binding::Template { fallback, .. } => fallback.clone().map(Json::String),
            Binding::Rules { .. } | Binding::Map { .. } => None,
        }
    }

    /// Evaluates a structured kind against state. `None` for `expr`, which
    /// needs an [`Evaluator`].
    pub fn evaluate(&self, state: &LiveState) -> Option<Outcome> {
        let mut reads = BTreeSet::new();
        let mut read = |r: &Ref| {
            let path = r.state_path();
            let value = state.value(&path).as_ref().map(expr::to_json);
            reads.insert(path);
            value
        };
        let value = match self {
            Binding::Expr { .. } => return None,
            Binding::Value { path, format, .. } => {
                read(path).map(|v| Json::String(format_value(Some(&v), *format)))
            }
            Binding::Template { parts, .. } => {
                let mut text = String::new();
                let mut missing = false;
                for part in parts {
                    match part {
                        Part::Text(t) => text.push_str(t),
                        Part::Value { path, format } => match read(path) {
                            Some(v) => text.push_str(&format_value(Some(&v), *format)),
                            None => missing = true,
                        },
                    }
                }
                (!missing).then_some(Json::String(text))
            }
            Binding::Rules { rules, otherwise } => {
                let mut chosen = None;
                for rule in rules {
                    let v = read(&rule.when.path);
                    if holds(rule.when.op, v.as_ref(), rule.when.value.as_ref()) {
                        chosen = Some(rule.value.clone());
                        break;
                    }
                }
                Some(chosen.unwrap_or_else(|| otherwise.clone()))
            }
            Binding::Map {
                path,
                from,
                to,
                curve,
                clamp,
            } => match read(path) {
                Some(Json::Number(n)) => map_number(
                    n.as_f64().unwrap_or(f64::NAN),
                    f64s(from),
                    to,
                    *curve,
                    *clamp,
                ),
                _ => None,
            },
        };
        Some(Outcome {
            value: Ok(value),
            reads,
        })
    }

    /// The expression this binding stands for (bindings spec §2), with
    /// connections by alias. `None` if a connection has no alias.
    pub fn to_expr(&self, aliases: &BTreeMap<ConnId, String>) -> Option<String> {
        let lit = |v: &Json| v.to_string();
        Some(match self {
            Binding::Expr { source, .. } => source.clone(),
            Binding::Value { path, format, .. } => format!(
                "((v) => v === undefined ? undefined : {})({})",
                format_expr("v", *format),
                path.text(aliases)?
            ),
            Binding::Template { parts, .. } => {
                let mut values = Vec::new();
                let mut text = String::from("`");
                for part in parts {
                    match part {
                        Part::Text(t) => text.push_str(&escape_template(t)),
                        Part::Value { path, format } => {
                            let name = format!("v{}", values.len());
                            text.push_str(&format!("${{{}}}", format_expr(&name, *format)));
                            values.push(path.text(aliases)?);
                        }
                    }
                }
                text.push('`');
                if values.is_empty() {
                    text
                } else {
                    let names: Vec<String> = (0..values.len()).map(|i| format!("v{i}")).collect();
                    let missing: Vec<String> =
                        names.iter().map(|n| format!("{n} === undefined")).collect();
                    format!(
                        "(({}) => {} ? undefined : {text})({})",
                        names.join(", "),
                        missing.join(" || "),
                        values.join(", ")
                    )
                }
            }
            Binding::Rules { rules, otherwise } => {
                let mut out = String::new();
                for rule in rules {
                    let path = format!("({})", rule.when.path.text(aliases)?);
                    let value = rule.when.value.as_ref().map_or("undefined".into(), lit);
                    let test = match rule.when.op {
                        Op::Eq => format!("{path} === {value}"),
                        Op::Ne => format!("{path} !== {value}"),
                        Op::Gt => format!("{path} > {value}"),
                        Op::Ge => format!("{path} >= {value}"),
                        Op::Lt => format!("{path} < {value}"),
                        Op::Le => format!("{path} <= {value}"),
                        Op::Truthy => path,
                        Op::Falsy => format!("!{path}"),
                    };
                    out.push_str(&format!("{test} ? {} : ", lit(&rule.value)));
                }
                out.push_str(&lit(otherwise));
                out
            }
            Binding::Map {
                path,
                from,
                to,
                curve,
                clamp,
            } => {
                let [a, b] = f64s(from);
                let num = |x: &f64| js::number_to_string(*x);
                let mut body = format!("let t = (v - {}) / ({} - {});", num(&a), num(&b), num(&a));
                if clamp.unwrap_or(true) {
                    body.push_str(" t = Math.min(Math.max(t, 0), 1);");
                }
                match curve.unwrap_or(Curve::Linear) {
                    Curve::Linear => {}
                    Curve::Log => body.push_str(" t = Math.log10(1 + 9 * t);"),
                    Curve::Exp => body.push_str(" t = (Math.pow(10, t) - 1) / 9;"),
                }
                match to {
                    MapTo::Range(range) => {
                        let [c, d] = f64s(range);
                        body.push_str(&format!(
                            " return {} + ({} - {}) * t;",
                            num(&c),
                            num(&d),
                            num(&c)
                        ))
                    }
                    MapTo::List(items) => body.push_str(&format!(
                        " const to = {}; return to[Math.min(Math.max(Math.floor(t * to.length), 0), to.length - 1)];",
                        Json::Array(items.clone())
                    )),
                }
                format!(
                    "((v) => {{ if (typeof v !== \"number\") return undefined; {body} }})({})",
                    path.text(aliases)?
                )
            }
        })
    }
}

/// A value as text, by `format` (absent is `auto`).
fn format_value(v: Option<&Json>, format: Option<Format>) -> String {
    match format.unwrap_or(Format::Auto) {
        Format::Auto => js::to_string(v),
        Format::Mmss => js::clock(v, false),
        Format::Hhmmss => js::clock(v, true),
        Format::Percent => format!(
            "{}%",
            js::number_to_string(js::round(js::to_number(v) * 100.0))
        ),
        Format::Fixed1 => js::to_fixed(js::to_number(v), 1),
        Format::Fixed2 => js::to_fixed(js::to_number(v), 2),
        Format::Upper => js::to_string(v).to_uppercase(),
    }
}

/// [`format_value`] as JavaScript, applied to the variable `v`.
fn format_expr(v: &str, format: Option<Format>) -> String {
    match format.unwrap_or(Format::Auto) {
        Format::Auto => format!("String({v})"),
        Format::Mmss => format!("mmss({v})"),
        Format::Hhmmss => format!("hhmmss({v})"),
        Format::Percent => format!("`${{Math.round(Number({v}) * 100)}}%`"),
        Format::Fixed1 => format!("Number({v}).toFixed(1)"),
        Format::Fixed2 => format!("Number({v}).toFixed(2)"),
        Format::Upper => format!("String({v}).toUpperCase()"),
    }
}

fn escape_template(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('`', "\\`")
        .replace("${", "\\${")
}

fn holds(op: Op, v: Option<&Json>, value: Option<&Json>) -> bool {
    use std::cmp::Ordering::*;
    let order = || js::compare(v, value);
    match op {
        Op::Eq => js::strict_equals(v, value),
        Op::Ne => !js::strict_equals(v, value),
        Op::Gt => order() == Some(Greater),
        Op::Ge => matches!(order(), Some(Greater | Equal)),
        Op::Lt => order() == Some(Less),
        Op::Le => matches!(order(), Some(Less | Equal)),
        Op::Truthy => json_truthy(v),
        Op::Falsy => !json_truthy(v),
    }
}

fn json_truthy(v: Option<&Json>) -> bool {
    match v {
        None | Some(Json::Null) => false,
        Some(Json::Bool(b)) => *b,
        Some(Json::Number(n)) => n.as_f64().is_some_and(|x| x != 0.0 && !x.is_nan()),
        Some(Json::String(s)) => !s.is_empty(),
        Some(Json::Array(_) | Json::Object(_)) => true,
    }
}

fn map_number(
    v: f64,
    [a, b]: [f64; 2],
    to: &MapTo,
    curve: Option<Curve>,
    clamp: Option<bool>,
) -> Option<Json> {
    let mut t = (v - a) / (b - a);
    if clamp.unwrap_or(true) {
        t = js::min(js::max(t, 0.0), 1.0);
    }
    match curve.unwrap_or(Curve::Linear) {
        Curve::Linear => {}
        Curve::Log => t = (1.0 + 9.0 * t).log10(),
        Curve::Exp => t = (10f64.powf(t) - 1.0) / 9.0,
    }
    match to {
        MapTo::Range(range) => {
            let [c, d] = f64s(range);
            number(c + (d - c) * t)
        }
        MapTo::List(items) => {
            let len = items.len() as f64;
            let i = js::min(js::max((t * len).floor(), 0.0), len - 1.0);
            // `to[NaN]` is `undefined`.
            (!i.is_nan()).then(|| items[i as usize].clone())
        }
    }
}

fn f64s(pair: &[Number; 2]) -> [f64; 2] {
    pair.each_ref().map(|n| n.as_f64().unwrap_or(f64::NAN))
}

/// A number as JSON, as it comes out of an expression: NaN and the
/// infinities have no JSON form, so they're `undefined`.
fn number(x: f64) -> Option<Json> {
    serde_json::Number::from_f64(x).map(Json::Number)
}

/// A result as a property's value: a label is text (numbers and booleans
/// are written as JavaScript writes them), a colour is `#rrggbb` or `#rgb`.
pub fn prop_value(prop: Prop, value: &Json) -> Result<PropValue, String> {
    match (prop, value) {
        (Prop::Label, Json::String(s)) => Ok(PropValue::Text(s.clone())),
        (Prop::Label, v @ (Json::Number(_) | Json::Bool(_))) => {
            Ok(PropValue::Text(js::to_string(Some(v))))
        }
        (Prop::Label, v) => Err(format!("a label must be text, not {v}")),
        (Prop::TextColor | Prop::Background, Json::String(s)) => Rgb::parse(s)
            .map(PropValue::Color)
            .ok_or_else(|| format!("{s:?} isn't a colour: write it #rrggbb")),
        (Prop::TextColor | Prop::Background, v) => {
            Err(format!("a colour must be text like \"#ff0000\", not {v}"))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PropValue {
    Text(String),
    Color(Rgb),
}

impl PropValue {
    pub fn apply(self, prop: Prop, look: &mut Look) {
        match (prop, self) {
            (Prop::Label, PropValue::Text(t)) => look.label = t,
            (Prop::TextColor, PropValue::Color(c)) => look.text_color = c,
            (Prop::Background, PropValue::Color(c)) => look.background = c,
            _ => {}
        }
    }
}

/// What validation checks a binding against.
pub struct Context<'a> {
    pub pages: &'a BTreeMap<PageId, Layout>,
    pub conns: &'a BTreeSet<ConnId>,
    /// `(page, control)` pairs bound by other bindings.
    pub taken: &'a BTreeSet<(PageId, String)>,
    pub evaluator: &'a dyn Evaluator,
}

fn key(k: &str) -> PathSegment {
    PathSegment::Key(k.to_owned())
}

fn index(i: usize) -> PathSegment {
    PathSegment::Index(i as i64)
}

fn at(base: &[PathSegment], more: &[PathSegment]) -> Path {
    base.iter().chain(more).cloned().collect()
}

impl ControlBinding {
    /// Reads a binding from JSON. Properties and refs that a later milestone
    /// adds are `unsupported`; any other shape problem is one `invalid`.
    pub fn parse(json: &Json) -> Result<ControlBinding, Vec<Problem>> {
        let mut unsupported = Vec::new();
        if let Some(props) = json.get("props").and_then(Json::as_object) {
            for (name, binding) in props {
                if !Prop::ALL.iter().any(|p| p.as_str() == name) {
                    unsupported.push(Problem::new(
                        vec![key("props"), key(name)],
                        Code::Unsupported,
                        format!("the {name:?} property isn't supported yet"),
                    ));
                }
                find_targets(
                    binding,
                    &mut vec![key("props"), key(name)],
                    &mut unsupported,
                );
            }
        }
        if !unsupported.is_empty() {
            return Err(unsupported);
        }
        // Each property on its own first, so a problem names its property.
        if let Some(props) = json.get("props").and_then(Json::as_object) {
            for (name, binding) in props {
                deserialize::<Binding>(binding, &[key("props"), key(name)]).map_err(|p| vec![p])?;
            }
        }
        deserialize(json, &[]).map_err(|p| vec![p])
    }
}

/// Reads `json`, or one `invalid` problem at its place under `base`.
fn deserialize<T: serde::de::DeserializeOwned>(
    json: &Json,
    base: &[PathSegment],
) -> Result<T, Problem> {
    serde_path_to_error::deserialize(json).map_err(|e| {
        let more: Path = e
            .path()
            .iter()
            .filter_map(|seg| match seg {
                serde_path_to_error::Segment::Seq { index: i } => Some(index(*i)),
                serde_path_to_error::Segment::Map { key: k } => Some(key(k)),
                _ => None,
            })
            .collect();
        Problem::new(at(base, &more), Code::Invalid, e.into_inner().to_string())
    })
}

/// Refs through a target (ADR 0019) wait for dynamic targets.
fn find_targets(json: &Json, here: &mut Path, out: &mut Vec<Problem>) {
    match json {
        Json::Object(map) => {
            if map.contains_key("target") && map.contains_key("path") {
                out.push(Problem::new(
                    here.clone(),
                    Code::Unsupported,
                    "paths through a target aren't supported yet",
                ));
                return;
            }
            for (k, v) in map {
                here.push(key(k));
                find_targets(v, here, out);
                here.pop();
            }
        }
        Json::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                here.push(index(i));
                find_targets(v, here, out);
                here.pop();
            }
        }
        _ => {}
    }
}

/// Checks a parsed binding against pages, connections and the property types.
pub fn validate(binding: &ControlBinding, ctx: &Context) -> Report {
    let mut v = Validation {
        ctx,
        errors: Vec::new(),
    };
    match ctx.pages.get(&binding.page) {
        None => v.error(
            vec![key("page")],
            Code::UnknownPage,
            format!("no page {:?}", binding.page),
        ),
        Some(layout) if !layout.contains(&binding.control) => v.error(
            vec![key("control")],
            Code::UnknownControl,
            format!(
                "the page is {}×{}, so its controls are k0 to k{}, not {:?}",
                layout.cols,
                layout.rows,
                (layout.cols * layout.rows).saturating_sub(1),
                binding.control
            ),
        ),
        Some(_)
            if ctx
                .taken
                .contains(&(binding.page.clone(), binding.control.clone())) =>
        {
            v.error(
                vec![key("control")],
                Code::DuplicateControl,
                format!("{} is already bound on this page", binding.control),
            )
        }
        Some(_) => {}
    }
    for (prop, b) in &binding.props {
        v.binding(*prop, b, &[key("props"), key(prop.as_str())]);
    }
    Report {
        errors: v.errors,
        warnings: Vec::new(),
    }
}

struct Validation<'a> {
    ctx: &'a Context<'a>,
    errors: Vec<Problem>,
}

impl Validation<'_> {
    fn error(&mut self, path: Path, code: Code, message: String) {
        self.errors.push(Problem::new(path, code, message));
    }

    fn binding(&mut self, prop: Prop, b: &Binding, base: &[PathSegment]) {
        match b {
            Binding::Value { path, fallback, .. } => {
                self.reference(path, at(base, &[key("path")]));
                if let Some(f) = fallback {
                    self.value(prop, f, at(base, &[key("fallback")]));
                }
            }
            Binding::Template { parts, fallback } => {
                for (i, part) in parts.iter().enumerate() {
                    if let Part::Value { path, .. } = part {
                        self.reference(path, at(base, &[key("parts"), index(i), key("path")]));
                    }
                }
                if prop != Prop::Label {
                    self.error(
                        at(base, &[key("kind")]),
                        Code::BadValue,
                        "a template makes text, so it can only be a label".into(),
                    );
                } else if let Some(f) = fallback {
                    self.value(prop, &Json::String(f.clone()), at(base, &[key("fallback")]));
                }
            }
            Binding::Rules { rules, otherwise } => {
                for (i, rule) in rules.iter().enumerate() {
                    let here = at(base, &[key("rules"), index(i)]);
                    self.reference(&rule.when.path, at(&here, &[key("when"), key("path")]));
                    self.value(prop, &rule.value, at(&here, &[key("value")]));
                }
                self.value(prop, otherwise, at(base, &[key("else")]));
            }
            Binding::Map { path, from, to, .. } => {
                self.reference(path, at(base, &[key("path")]));
                let [a, b] = f64s(from);
                if a == b {
                    self.error(
                        at(base, &[key("from")]),
                        Code::BadValue,
                        "from must be two different numbers".into(),
                    );
                }
                match to {
                    MapTo::List(items) if items.is_empty() => self.error(
                        at(base, &[key("to")]),
                        Code::BadValue,
                        "to must list at least one value".into(),
                    ),
                    MapTo::List(items) => {
                        for (i, item) in items.iter().enumerate() {
                            self.value(prop, item, at(base, &[key("to"), index(i)]));
                        }
                    }
                    MapTo::Range(_) if prop != Prop::Label => self.error(
                        at(base, &[key("to")]),
                        Code::BadValue,
                        "a colour can't be a number: map to a list of colours".into(),
                    ),
                    MapTo::Range(_) => {}
                }
            }
            Binding::Expr { source, fallback } => {
                if let Err(e) = self.ctx.evaluator.check(source) {
                    self.errors.push(Problem {
                        path: at(base, &[key("source")]),
                        code: Code::ExprSyntax,
                        message: e.message,
                        at: e.at,
                    });
                }
                if let Some(f) = fallback {
                    self.value(prop, f, at(base, &[key("fallback")]));
                }
            }
        }
    }

    fn reference(&mut self, r: &Ref, path: Path) {
        if let Ref::Conn { conn, .. } = r
            && !self.ctx.conns.contains(conn)
        {
            self.error(
                at(&path, &[key("conn")]),
                Code::UnknownConnection,
                format!("no connection {conn:?}"),
            );
        }
    }

    fn value(&mut self, prop: Prop, v: &Json, path: Path) {
        if let Err(message) = prop_value(prop, v) {
            self.error(path, Code::BadValue, message);
        }
    }
}

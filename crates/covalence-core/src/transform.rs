//! Stateless transforms (dataflow spec §4.1, §10.1): functions over
//! sources, used as `{use: <id>, …options}` wherever a source can be.
//!
//! `format`, `template`, `rules` and `map` are M5's binding kinds; `compare`,
//! `all`, `any` and `not` let conditions combine. Their options are sources,
//! so they chain. Each is evaluated natively ([`Transform::evaluate`]),
//! following the rules of bindings §2.1, and compiles to an expression
//! ([`Transform::to_expr`]) that gives the same result (bindings spec §9.2).

use std::collections::BTreeMap;

use covalence_protocol::state::{Path, PathSegment};
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value as Json};

use crate::expr::ExprError;
use crate::source::{Eval, Part, Source, literal_expr, operand};
use crate::state::ConnId;

pub mod conformance;
pub mod definition;
pub mod js;
pub mod reducer;

pub use reducer::Reducer;

/// A stateless transform, keyed by `use`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "use", rename_all = "camelCase", deny_unknown_fields)]
pub enum Transform {
    /// A value as text; `undefined` stays `undefined`.
    Format {
        input: Source<Json>,
        format: Format,
    },
    /// Text with values in it; `undefined` if any value is.
    Template {
        parts: Vec<Source<Json>>,
    },
    /// The value of the first rule whose `when` is truthy, or `else`.
    Rules {
        rules: Vec<Rule>,
        #[serde(rename = "else")]
        otherwise: Source<Json>,
    },
    /// A comparison, as a boolean.
    Compare {
        input: Source<Json>,
        op: Op,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<Source<Json>>,
    },
    /// Whether every item is truthy; `true` for none. Stops at the first falsy one.
    All {
        of: Vec<Source<Json>>,
    },
    /// Whether any item is truthy; `false` for none. Stops at the first truthy one.
    Any {
        of: Vec<Source<Json>>,
    },
    Not {
        input: Source<Json>,
    },
    /// A number scaled to a range, or used to pick from a list.
    Map {
        input: Source<Json>,
        from: [Number; 2],
        to: MapTo,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        curve: Option<Curve>,
        /// Absent means `true`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        clamp: Option<bool>,
    },
    /// A reducer (dataflow §4.2). Read and written by [`Source`]'s own
    /// serde, since its `use` is one of several names.
    #[serde(skip)]
    Reducer(Box<Reducer>),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub when: Source<Json>,
    pub value: Source<Json>,
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

/// What a transform gives, for checking it against where it's used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gives {
    Text,
    Boolean,
    /// A number, or an item of a list.
    Mapped,
    /// One of its rules' values.
    Chosen,
    /// A reducer's value.
    Reduced,
}

fn key(k: &str) -> PathSegment {
    PathSegment::Key(k.to_owned())
}

fn index(i: usize) -> PathSegment {
    PathSegment::Index(i as i64)
}

impl Transform {
    pub fn id(&self) -> &str {
        match self {
            Transform::Reducer(r) => &r.id,
            Transform::Format { .. } => "format",
            Transform::Template { .. } => "template",
            Transform::Rules { .. } => "rules",
            Transform::Compare { .. } => "compare",
            Transform::All { .. } => "all",
            Transform::Any { .. } => "any",
            Transform::Not { .. } => "not",
            Transform::Map { .. } => "map",
        }
    }

    pub fn gives(&self) -> Gives {
        match self {
            Transform::Format { .. } | Transform::Template { .. } => Gives::Text,
            Transform::Compare { .. }
            | Transform::All { .. }
            | Transform::Any { .. }
            | Transform::Not { .. } => Gives::Boolean,
            Transform::Map { .. } => Gives::Mapped,
            Transform::Rules { .. } => Gives::Chosen,
            Transform::Reducer(_) => Gives::Reduced,
        }
    }

    /// Whether it keeps a value of its own, and so needs a home (dataflow §4.2).
    pub fn is_reducer(&self) -> bool {
        matches!(self, Transform::Reducer(_))
    }

    /// Whether a `use` (without a version) names a reducer.
    pub fn is_reducer_id(id: &str) -> bool {
        Reducer::is_reducer(id)
    }

    /// Its option sources, each with its key path below the transform.
    pub fn sources(&self) -> Vec<(Vec<PathSegment>, &Source<Json>)> {
        fn items<'s>(
            name: &str,
            items: &'s [Source<Json>],
            out: &mut Vec<(Vec<PathSegment>, &'s Source<Json>)>,
        ) {
            for (i, s) in items.iter().enumerate() {
                out.push((vec![key(name), index(i)], s));
            }
        }
        let mut out: Vec<(Vec<PathSegment>, &Source<Json>)> = Vec::new();
        match self {
            Transform::Format { input, .. }
            | Transform::Not { input }
            | Transform::Map { input, .. } => out.push((vec![key("input")], input)),
            Transform::Template { parts } => items("parts", parts, &mut out),
            Transform::All { of } | Transform::Any { of } => items("of", of, &mut out),
            Transform::Compare { input, value, .. } => {
                out.push((vec![key("input")], input));
                if let Some(v) = value {
                    out.push((vec![key("value")], v));
                }
            }
            Transform::Rules { rules, otherwise } => {
                for (i, rule) in rules.iter().enumerate() {
                    out.push((vec![key("rules"), index(i), key("when")], &rule.when));
                    out.push((vec![key("rules"), index(i), key("value")], &rule.value));
                }
                out.push((vec![key("else")], otherwise));
            }
            Transform::Reducer(r) => {
                for (name, s) in [
                    ("input", &r.input),
                    ("follow", &r.follow),
                    ("holdoff", &r.holdoff),
                ] {
                    if let Some(s) = s {
                        out.push((vec![key(name)], s));
                    }
                }
                for (name, s) in &r.options {
                    out.push((vec![key(name)], s));
                }
            }
        }
        out
    }

    /// Walks the sources nested in this transform (see [`Source::walk`]).
    pub fn walk<'s>(&'s self, here: &mut Path, f: &mut dyn FnMut(&Path, Part<'s>)) {
        for (rel, source) in self.sources() {
            let depth = here.len();
            here.extend(rel);
            source.walk(here, f);
            here.truncate(depth);
        }
    }

    /// Problems with its own options, as `(path below it, message)`: the
    /// ones a type can't express.
    pub fn problems(&self) -> Vec<(Vec<PathSegment>, String)> {
        let mut out = Vec::new();
        if let Transform::Map { from, to, .. } = self {
            let [a, b] = f64s(from);
            if a == b {
                out.push((
                    vec![key("from")],
                    "from must be two different numbers".into(),
                ));
            }
            if matches!(to, MapTo::List(items) if items.is_empty()) {
                out.push((vec![key("to")], "to must list at least one value".into()));
            }
        }
        out
    }

    /// Evaluates natively.
    pub fn evaluate(&self, eval: &mut Eval<'_>) -> Result<Option<Json>, ExprError> {
        Ok(match self {
            Transform::Reducer(r) => {
                return Err(ExprError::new(
                    crate::expr::ExprCode::Error,
                    format!(
                        "a {} keeps a value of its own, so it can only be a cell's source: \
                         a variable's value or a helper",
                        r.id
                    ),
                ));
            }
            Transform::Format { input, format } => eval
                .nested(input)?
                .map(|v| Json::String(format_value(Some(&v), *format))),
            Transform::Template { parts } => {
                let mut text = String::new();
                let mut missing = false;
                // Every part is read, as the compiled expression does.
                for part in parts {
                    match eval.nested(part)? {
                        Some(v) => text.push_str(&js::to_string(Some(&v))),
                        None => missing = true,
                    }
                }
                (!missing).then_some(Json::String(text))
            }
            Transform::Rules { rules, otherwise } => {
                for rule in rules {
                    if truthy(eval.nested(&rule.when)?.as_ref()) {
                        return eval.nested(&rule.value);
                    }
                }
                eval.nested(otherwise)?
            }
            Transform::Compare { input, op, value } => {
                let v = eval.nested(input)?;
                let against = match value {
                    Some(s) => eval.nested(s)?,
                    None => None,
                };
                Some(Json::Bool(holds(*op, v.as_ref(), against.as_ref())))
            }
            Transform::All { of } => {
                for item in of {
                    if !truthy(eval.nested(item)?.as_ref()) {
                        return Ok(Some(Json::Bool(false)));
                    }
                }
                Some(Json::Bool(true))
            }
            Transform::Any { of } => {
                for item in of {
                    if truthy(eval.nested(item)?.as_ref()) {
                        return Ok(Some(Json::Bool(true)));
                    }
                }
                Some(Json::Bool(false))
            }
            Transform::Not { input } => Some(Json::Bool(!truthy(eval.nested(input)?.as_ref()))),
            Transform::Map {
                input,
                from,
                to,
                curve,
                clamp,
            } => match eval.nested(input)? {
                Some(Json::Number(n)) => map_number(
                    n.as_f64().unwrap_or(f64::NAN),
                    f64s(from),
                    to,
                    *curve,
                    *clamp,
                ),
                _ => None,
            },
        })
    }

    /// The expression this transform stands for (dataflow §4.1), with
    /// connections by alias. `None` if a connection has no alias.
    pub fn to_expr(&self, aliases: &BTreeMap<ConnId, String>) -> Option<String> {
        let op = |s: &Source<Json>| operand(s, aliases);
        Some(match self {
            // A reducer keeps state, which an expression can't.
            Transform::Reducer(_) => return None,
            Transform::Format { input, format } => format!(
                "((v) => v === undefined ? undefined : {})({})",
                format_expr("v", *format),
                op(input)?
            ),
            Transform::Template { parts } => {
                let mut values = Vec::new();
                let mut text = String::from("`");
                for part in parts {
                    match part {
                        Source::Literal(Json::String(t)) => text.push_str(&escape_template(t)),
                        other => {
                            text.push_str(&format!("${{String(v{})}}", values.len()));
                            values.push(op(other)?);
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
            Transform::Rules { rules, otherwise } => {
                let mut out = String::new();
                for rule in rules {
                    out.push_str(&format!("{} ? {} : ", op(&rule.when)?, op(&rule.value)?));
                }
                out.push_str(&op(otherwise)?);
                out
            }
            Transform::Compare {
                input,
                op: o,
                value,
            } => {
                let input = op(input)?;
                let value = match value {
                    Some(v) => op(v)?,
                    None => "undefined".into(),
                };
                match o {
                    Op::Eq => format!("{input} === {value}"),
                    Op::Ne => format!("{input} !== {value}"),
                    Op::Gt => format!("{input} > {value}"),
                    Op::Ge => format!("{input} >= {value}"),
                    Op::Lt => format!("{input} < {value}"),
                    Op::Le => format!("{input} <= {value}"),
                    Op::Truthy => format!("!!{input}"),
                    Op::Falsy => format!("!{input}"),
                }
            }
            Transform::All { of } | Transform::Any { of } => {
                let (join, empty) = match self {
                    Transform::All { .. } => (" && ", "true"),
                    _ => (" || ", "false"),
                };
                if of.is_empty() {
                    empty.into()
                } else {
                    let items: Option<Vec<String>> =
                        of.iter().map(|s| op(s).map(|t| format!("!!{t}"))).collect();
                    items?.join(join)
                }
            }
            Transform::Not { input } => format!("!{}", op(input)?),
            Transform::Map {
                input,
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
                        literal_expr(&Json::Array(items.clone()))
                    )),
                }
                format!(
                    "((v) => {{ if (typeof v !== \"number\") return undefined; {body} }})({})",
                    op(input)?
                )
            }
        })
    }
}

/// JavaScript truthiness, with `None` as `undefined`.
pub fn truthy(v: Option<&Json>) -> bool {
    match v {
        None | Some(Json::Null) => false,
        Some(Json::Bool(b)) => *b,
        Some(Json::Number(n)) => n.as_f64().is_some_and(|x| x != 0.0 && !x.is_nan()),
        Some(Json::String(s)) => !s.is_empty(),
        Some(Json::Array(_) | Json::Object(_)) => true,
    }
}

/// A value as text, by `format`.
fn format_value(v: Option<&Json>, format: Format) -> String {
    match format {
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
fn format_expr(v: &str, format: Format) -> String {
    match format {
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
        Op::Truthy => truthy(v),
        Op::Falsy => !truthy(v),
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

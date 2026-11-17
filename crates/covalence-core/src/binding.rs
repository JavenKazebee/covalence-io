//! Display bindings (bindings spec §1–§4, ADR 0023): how a control looks,
//! kept in step with state.
//!
//! A [`ControlBinding`] binds properties of one control on one layer. Each
//! property's binding is a [`Source`] (ADR 0026): a literal, a state path,
//! an expression or a stateless transform ([`crate::transform`]). Paths and
//! transforms are evaluated natively; expressions run on QuickJS. The worker
//! in [`worker`] keeps the bindings of shown layers evaluated and draws what
//! changes.

use std::collections::{BTreeMap, BTreeSet};

use covalence_protocol::state::{Path, PathSegment};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::expr::Evaluator;
use crate::flow::{Code, Problem, Report};
use crate::source::{Part, Ref, Source};
use crate::state::ConnId;
use crate::surface::render::{Look, Rgb};
use crate::surface::{Layer, LayerId, OutputCapKind};
use crate::transform::{MapTo, Transform};

pub mod worker;

/// Entity type in the store.
pub const ENTITY: &str = "control_binding";

/// The bindings of one control on one layer (bindings spec §1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ControlBinding {
    /// A ULID, assigned by the server.
    #[serde(default)]
    pub id: String,
    pub layer: LayerId,
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

    /// The output it drives (bindings spec §1).
    pub fn output(self) -> OutputCapKind {
        match self {
            Prop::Label | Prop::TextColor | Prop::Background => OutputCapKind::Image,
        }
    }
}

/// A property's binding: any source (bindings spec §2, dataflow §1).
pub type Binding = Source<Json>;

/// A result as a property's value: a label is text (numbers and booleans
/// are written as JavaScript writes them), a colour is `#rrggbb` or `#rgb`.
pub fn prop_value(prop: Prop, value: &Json) -> Result<PropValue, String> {
    match (prop, value) {
        (Prop::Label, Json::String(s)) => Ok(PropValue::Text(s.clone())),
        (Prop::Label, v @ (Json::Number(_) | Json::Bool(_))) => {
            Ok(PropValue::Text(crate::transform::js::to_string(Some(v))))
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
    pub layers: &'a BTreeMap<LayerId, Layer>,
    pub conns: &'a BTreeSet<ConnId>,
    /// `(layer, control)` pairs bound by other bindings.
    pub taken: &'a BTreeSet<(LayerId, String)>,
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
    /// The outputs its properties drive, which its layer must give the
    /// control (`noOutput`).
    pub fn outputs(&self) -> BTreeSet<OutputCapKind> {
        self.props.keys().map(|p| p.output()).collect()
    }

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
pub(crate) fn deserialize<T: serde::de::DeserializeOwned>(
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

/// Checks a parsed binding against layers, connections and the property
/// types.
pub fn validate(binding: &ControlBinding, ctx: &Context) -> Report {
    let mut v = Validation {
        ctx,
        errors: Vec::new(),
    };
    let control = match ctx.layers.get(&binding.layer) {
        None => {
            v.error(
                vec![key("layer")],
                Code::UnknownLayer,
                format!("no layer {:?}", binding.layer),
            );
            None
        }
        Some(layer) => match layer.control(&binding.control) {
            None => {
                v.error(
                    vec![key("control")],
                    Code::UnknownControl,
                    format!("the layer doesn't list a control {:?}", binding.control),
                );
                None
            }
            Some(control) => {
                if ctx
                    .taken
                    .contains(&(binding.layer.clone(), binding.control.clone()))
                {
                    v.error(
                        vec![key("control")],
                        Code::DuplicateControl,
                        format!("{} is already bound on this layer", binding.control),
                    );
                }
                Some(control)
            }
        },
    };
    if let Some(control) = control {
        for prop in binding.props.keys() {
            let output = prop.output();
            if !control.outputs.contains(&output) {
                v.error(
                    vec![key("props"), key(prop.as_str())],
                    Code::NoOutput,
                    format!(
                        "{} needs an {} output, and the layer doesn't give {} one",
                        prop.as_str(),
                        output.as_str(),
                        binding.control
                    ),
                );
            }
        }
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
        let mut found = Vec::new();
        let (conns, evaluator) = (self.ctx.conns, self.ctx.evaluator);
        b.walk(&mut base.to_vec(), &mut |path, part| match part {
            Part::Ref(Ref::Conn { conn, .. }) if !conns.contains(conn) => found.push(Problem::new(
                at(path, &[key("conn")]),
                Code::UnknownConnection,
                format!("no connection {conn:?}"),
            )),
            Part::Ref(_) => {}
            Part::Expr(source) => {
                if let Err(e) = evaluator.check(source) {
                    found.push(Problem {
                        path: path.clone(),
                        code: Code::ExprSyntax,
                        message: e.message,
                        at: e.at,
                    });
                }
            }
            Part::Transform(t) => {
                if t.is_reducer() {
                    found.push(Problem::new(
                        at(path, &[key("use")]),
                        Code::Unsupported,
                        format!(
                            "a {} keeps a value of its own, so it needs a home: \
                             use it as a helper or a variable's value, and bind that",
                            t.id()
                        ),
                    ));
                }
                for (rel, message) in t.problems() {
                    found.push(Problem::new(at(path, &rel), Code::BadValue, message));
                }
            }
        });
        self.errors.extend(found);
        self.result(prop, b, base);
    }

    /// Checks that what a source can give fits the property: its literals,
    /// its fallback, and what its transform makes.
    fn result(&mut self, prop: Prop, s: &Binding, base: &[PathSegment]) {
        if let Some(f) = s.fallback() {
            self.value(prop, f, at(base, &[key("fallback")]));
        }
        let transform = match s {
            Source::Literal(v) => return self.value(prop, v, base.to_vec()),
            Source::Ref { .. } | Source::Expr { .. } => return,
            Source::Use { transform, .. } => &**transform,
        };
        let not_label = |what: &str| {
            Problem::new(
                at(base, &[key("use")]),
                Code::BadValue,
                format!("a {} {what}, so it can only be a label", transform.id()),
            )
        };
        match transform {
            Transform::Format { .. } | Transform::Template { .. } if prop != Prop::Label => {
                self.errors.push(not_label("makes text"));
            }
            Transform::Compare { .. }
            | Transform::All { .. }
            | Transform::Any { .. }
            | Transform::Not { .. }
                if prop != Prop::Label =>
            {
                self.errors.push(not_label("gives true or false"));
            }
            Transform::Rules { rules, otherwise } => {
                for (i, rule) in rules.iter().enumerate() {
                    self.result(
                        prop,
                        &rule.value,
                        &at(base, &[key("rules"), index(i), key("value")]),
                    );
                }
                self.result(prop, otherwise, &at(base, &[key("else")]));
            }
            Transform::Map { to, .. } => match to {
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
            },
            _ => {}
        }
    }

    fn value(&mut self, prop: Prop, v: &Json, path: Path) {
        if let Err(message) = prop_value(prop, v) {
            self.error(path, Code::BadValue, message);
        }
    }
}

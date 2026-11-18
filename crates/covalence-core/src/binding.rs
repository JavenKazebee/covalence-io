//! Display bindings (bindings spec §1–§4, §9.2): how a control looks,
//! kept in step with state.
//!
//! A [`ControlBinding`] binds properties of one control on one layer. Each
//! property's binding is a [`Source`] (dataflow spec §10.1): a literal, a state path,
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
use crate::state::{ConnId, StatePath};
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
    #[serde(default)]
    pub props: BTreeMap<Prop, Binding>,
    /// Reducers on the control's input, by key (bindings §5).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub helpers: BTreeMap<String, Helper>,
    /// The control's gesture timing while this binding's layer controls it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gestures: Option<Gestures>,
}

/// Gesture timing for one control (bindings §1, surfaces spec §11.4). Each is a
/// duration (`400ms`) or a number of milliseconds, read when it's needed:
/// `longAfter` and `holdEvery` when the control goes down, `doubleWindow`
/// at the next press. A failure, or a value that isn't a duration, gives
/// that setting its default.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Gestures {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub double_window: Option<Binding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long_after: Option<Binding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold_every: Option<Binding>,
}

impl Gestures {
    /// Each setting, by its name.
    pub fn settings(&self) -> [(&'static str, Option<&Binding>); 3] {
        [
            ("doubleWindow", self.double_window.as_ref()),
            ("longAfter", self.long_after.as_ref()),
            ("holdEvery", self.hold_every.as_ref()),
        ]
    }
}

/// A gesture setting's value as a duration: a duration's text, or a
/// positive number of milliseconds.
pub fn gesture_duration(value: &Json) -> Result<std::time::Duration, String> {
    match value {
        Json::Number(n) => match n.as_f64() {
            Some(ms) if (1.0..=3_600_000.0).contains(&ms) => {
                Ok(std::time::Duration::from_millis(ms as u64))
            }
            _ => Err(format!("{n} isn't a duration: use 1 ms to an hour")),
        },
        Json::String(s) => s.parse::<crate::flow::Duration>().map(|d| d.get()),
        v => Err(format!(
            "a duration is text like \"400ms\" or a number of milliseconds, not {v}"
        )),
    }
}

/// A helper (bindings §5, surfaces §4.3): a reducer whose `on` and input
/// default to its control, and whose value is a cell under the layer.
#[derive(Clone, Debug, PartialEq)]
pub struct Helper {
    pub reducer: crate::transform::Reducer,
    /// The cell's name; default the helper's key.
    pub name: Option<String>,
    pub scope: HelperScope,
    pub to: Vec<crate::dataflow::Sink>,
    pub journal: Option<crate::variable::Journaling>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HelperScope {
    /// `layer.<slug>.controls.<id>.<name>`, shared by every surface.
    #[default]
    Layer,
    /// `surface.<slug>.controls.<id>.helpers.<name>`, one per surface.
    Surface,
}

impl<'de> Deserialize<'de> for Helper {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let Json::Object(mut map) = Json::deserialize(deserializer)? else {
            return Err(D::Error::custom("a helper is {use: \"toggle\", …}"));
        };
        let take = |map: &mut serde_json::Map<String, Json>, k: &str| map.remove(k);
        let name = match take(&mut map, "as") {
            None => None,
            Some(Json::String(s)) => Some(s),
            Some(_) => return Err(D::Error::custom("as: a helper's name is text")),
        };
        let scope = take(&mut map, "scope")
            .map(serde_json::from_value)
            .transpose()
            .map_err(|e| D::Error::custom(format!("scope: {e}")))?
            .unwrap_or_default();
        let to = match take(&mut map, "to") {
            None => Vec::new(),
            Some(Json::Array(items)) => serde_json::from_value(Json::Array(items))
                .map_err(|e| D::Error::custom(format!("to: {e}")))?,
            Some(one) => vec![
                serde_json::from_value(one).map_err(|e| D::Error::custom(format!("to: {e}")))?,
            ],
        };
        let journal = take(&mut map, "journal")
            .map(serde_json::from_value)
            .transpose()
            .map_err(|e| D::Error::custom(format!("journal: {e}")))?;
        if !map.contains_key("use") {
            return Err(D::Error::custom(
                "a helper names its reducer: {use: \"toggle\", …}",
            ));
        }
        let reducer =
            match Source::<Json>::from_json(Json::Object(map)).map_err(D::Error::custom)? {
                Source::Use { transform, .. } => match *transform {
                    crate::transform::Transform::Reducer(r) => *r,
                    other => {
                        return Err(D::Error::custom(format!(
                            "a helper is a reducer, and {} isn't one",
                            other.id()
                        )));
                    }
                },
                _ => unreachable!("an object with `use` is a transform"),
            };
        Ok(Helper {
            reducer,
            name,
            scope,
            to,
            journal,
        })
    }
}

impl Serialize for Helper {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::Error;
        let mut map = self.reducer.to_map().map_err(S::Error::custom)?;
        if let Some(n) = &self.name {
            map.insert("as".into(), Json::String(n.clone()));
        }
        if self.scope != HelperScope::Layer {
            map.insert(
                "scope".into(),
                serde_json::to_value(self.scope).map_err(S::Error::custom)?,
            );
        }
        if !self.to.is_empty() {
            map.insert(
                "to".into(),
                serde_json::to_value(&self.to).map_err(S::Error::custom)?,
            );
        }
        if let Some(j) = &self.journal {
            map.insert(
                "journal".into(),
                serde_json::to_value(j).map_err(S::Error::custom)?,
            );
        }
        Json::Object(map).serialize(serializer)
    }
}

/// A helper's cell identity: its binding and its key (dataflow §5.4).
pub fn helper_cell_id(binding: &str, key: &str) -> String {
    format!("helper:{binding}/{key}")
}

/// A surface-scoped helper's cell identity: one per surface (surfaces spec §11.4).
pub fn surface_helper_cell_id(binding: &str, key: &str, surface: &str) -> String {
    format!("helper:{binding}/{key}@{surface}")
}

/// A property of a control: the three a key image needs, and `value` for a
/// `level` output (surfaces spec §11.4). In the order of their names, as JSON objects
/// are written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Prop {
    Background,
    Label,
    TextColor,
    Value,
}

impl Prop {
    pub const ALL: [Prop; 4] = [Prop::Background, Prop::Label, Prop::TextColor, Prop::Value];

    pub fn as_str(self) -> &'static str {
        match self {
            Prop::Label => "label",
            Prop::TextColor => "textColor",
            Prop::Background => "background",
            Prop::Value => "value",
        }
    }

    /// The output it drives (bindings spec §1).
    pub fn output(self) -> OutputCapKind {
        match self {
            Prop::Label | Prop::TextColor | Prop::Background => OutputCapKind::Image,
            Prop::Value => OutputCapKind::Level,
        }
    }

    fn is_color(self) -> bool {
        matches!(self, Prop::TextColor | Prop::Background)
    }
}

/// A property's binding: any source (bindings spec §2, dataflow §1).
pub type Binding = Source<Json>;

/// A result as a property's value: a label is text (numbers and booleans
/// are written as JavaScript writes them), a colour is `#rrggbb` or `#rgb`,
/// and a `value` is a number, held to 0…1.
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
        (Prop::Value, Json::Number(n)) => match n.as_f64() {
            Some(n) if n.is_finite() => Ok(PropValue::Level(n.clamp(0.0, 1.0))),
            _ => Err(format!("{n} isn't a value from 0 to 1")),
        },
        (Prop::Value, v) => Err(format!("a value must be a number from 0 to 1, not {v}")),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum PropValue {
    Text(String),
    Color(Rgb),
    /// A `level` output's value, 0 to 1.
    Level(f64),
}

impl PropValue {
    pub fn apply(self, prop: Prop, look: &mut Look) {
        match (prop, self) {
            (Prop::Label, PropValue::Text(t)) => look.label = t,
            (Prop::TextColor, PropValue::Color(c)) => look.text_color = c,
            (Prop::Background, PropValue::Color(c)) => look.background = c,
            // A level isn't part of an image.
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
    /// Its layer-scoped helpers' cells, for a layer with this slug
    /// (surfaces §4.3): `layer.<slug>.controls.<id>.<name>`, whose `on`
    /// defaults to the control's events on the layer.
    pub fn helper_cells(&self, layer_slug: &str) -> Vec<crate::dataflow::CellDef> {
        use crate::flow::EventSource;
        use crate::select::Selector;
        self.helpers
            .iter()
            .filter(|(_, h)| h.scope == HelperScope::Layer)
            .map(|(k, h)| {
                let name = h.name.clone().unwrap_or_else(|| k.clone());
                let home = StatePath::new(
                    "layer",
                    vec![
                        key(layer_slug),
                        key("controls"),
                        key(&self.control),
                        key(&name),
                    ],
                );
                let from = EventSource::Layer {
                    layer: self.layer.clone(),
                    controls: Some(Selector::One(self.control.clone())),
                };
                self.helper_cell(helper_cell_id(&self.id, k), h, home, from, None)
            })
            .collect()
    }

    /// Whether any of its helpers has `scope: "surface"`.
    pub fn has_surface_helpers(&self) -> bool {
        self.helpers
            .values()
            .any(|h| h.scope == HelperScope::Surface)
    }

    /// Its surface-scoped helpers' cells on one surface (surfaces spec §11.4):
    /// `surface.<slug>.controls.<id>.helpers.<name>`, whose `on` defaults to
    /// the control's events on that surface. Only input that arrived while
    /// the binding's layer controlled the control steps them.
    pub fn surface_helper_cells(
        &self,
        surface: &str,
        surface_slug: &str,
    ) -> Vec<crate::dataflow::CellDef> {
        use crate::flow::EventSource;
        use crate::select::Selector;
        self.helpers
            .iter()
            .filter(|(_, h)| h.scope == HelperScope::Surface)
            .map(|(k, h)| {
                let name = h.name.clone().unwrap_or_else(|| k.clone());
                let home = StatePath::new(
                    crate::surface::STATE_ROOT,
                    vec![
                        key(surface_slug),
                        key("controls"),
                        key(&self.control),
                        key("helpers"),
                        key(&name),
                    ],
                );
                let from = EventSource::Surface {
                    surface: surface.to_owned(),
                    controls: Some(Selector::One(self.control.clone())),
                };
                self.helper_cell(
                    surface_helper_cell_id(&self.id, k, surface),
                    h,
                    home,
                    from,
                    Some(surface.to_owned()),
                )
            })
            .collect()
    }

    fn helper_cell(
        &self,
        id: String,
        h: &Helper,
        home: StatePath,
        from: crate::flow::EventSource,
        surface: Option<String>,
    ) -> crate::dataflow::CellDef {
        use crate::dataflow::{CellDef, ControlContext, JournalMode};
        use crate::transform::reducer::On;
        let mut reducer = h.reducer.clone();
        match &mut reducer.on {
            Some(on) if on.from.is_none() => on.from = Some(from),
            Some(_) => {}
            None => {
                reducer.on = Some(On {
                    from: Some(from),
                    event: None,
                    filter: None,
                })
            }
        }
        let input = reducer.input.is_none() && reducer.definition().steps.input.is_some();
        CellDef {
            journal: match &h.journal {
                Some(crate::variable::Journaling::Sampled { sampled }) => {
                    JournalMode::Sampled(sampled.get())
                }
                _ => JournalMode::Full,
            },
            to: h.to.clone(),
            control: Some(ControlContext {
                layer: self.layer.clone(),
                control: self.control.clone(),
                input,
                surface,
            }),
            ..CellDef::new(
                id,
                home,
                Source::Use {
                    transform: Box::new(crate::transform::Transform::Reducer(Box::new(reducer))),
                    fallback: None,
                },
            )
        }
    }

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

/// Refs through a target (plugin protocol spec §13.9) wait for dynamic targets.
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
    for (name, helper) in &binding.helpers {
        v.helper(helper, &[key("helpers"), key(name)]);
    }
    if let Some(gestures) = &binding.gestures {
        for (name, setting) in gestures.settings() {
            let Some(s) = setting else { continue };
            let base = [key("gestures"), key(name)];
            v.sources(s, &base, false);
            let literals = match s {
                Source::Literal(l) => vec![(l, base.to_vec())],
                _ => s
                    .fallback()
                    .map(|f| vec![(f, at(&base, &[key("fallback")]))])
                    .unwrap_or_default(),
            };
            for (l, path) in literals {
                if let Err(message) = gesture_duration(l) {
                    v.error(path, Code::BadValue, message);
                }
            }
        }
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
        self.sources(b, base, false);
        self.result(prop, b, base);
    }

    /// A helper (bindings §5): its reducer's sources and its sinks.
    fn helper(&mut self, helper: &Helper, base: &[PathSegment]) {
        let source = Source::Use {
            transform: Box::new(crate::transform::Transform::Reducer(Box::new(
                helper.reducer.clone(),
            ))),
            fallback: None,
        };
        self.sources(&source, base, true);
        for (i, sink) in helper.to.iter().enumerate() {
            let here = at(base, &[key("to"), index(i)]);
            match sink {
                crate::dataflow::Sink::Set { set } => {
                    let ok = matches!(set, Ref::Root { root: crate::source::Root::Vars, path }
                        if matches!(path.as_slice(), [PathSegment::Key(_)]));
                    if !ok {
                        self.error(
                            at(&here, &[key("set")]),
                            Code::BadPath,
                            "a `set` sink writes vars.<name>".into(),
                        );
                    }
                }
                crate::dataflow::Sink::Action { action } => {
                    if let Some(t) = action.target.literal()
                        && !self.ctx.conns.contains(&t.conn)
                    {
                        self.error(
                            at(&here, &[key("action"), key("target")]),
                            Code::UnknownConnection,
                            format!("no connection {:?}", t.conn),
                        );
                    }
                    for (name, s) in &action.params {
                        self.sources(
                            s,
                            &at(&here, &[key("action"), key("params"), key(name)]),
                            false,
                        );
                    }
                }
            }
        }
    }

    /// Checks the expressions, refs and transforms in a source. A reducer
    /// may only be the whole source, and only where `home` says it has one.
    fn sources(&mut self, b: &Binding, base: &[PathSegment], home: bool) {
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
                if t.is_reducer() && !(home && path.as_slice() == base) {
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
                MapTo::Range(_) if prop.is_color() => self.error(
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

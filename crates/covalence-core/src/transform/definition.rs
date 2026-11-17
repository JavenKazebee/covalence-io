//! Transform definitions (dataflow spec §4.4): pure JavaScript modules, run
//! in the core's sandbox, that specify what a transform does.
//!
//! Every built-in has one, in `transforms/`. A [`Definition`] is one loaded
//! and described; a [`Sandbox`] calls its functions under the bindings'
//! limits. Native implementations ([`super::reducer`], and
//! [`super::Transform::evaluate`] for the stateless ones) must agree with
//! the definition; [`super::conformance`] checks that they do.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as Json, json};

use crate::expr::{ExprCode, ExprError, Limits, QuickJs};

/// A definition's time limit per call: the bindings' (dataflow §4.4).
pub const CALL_TIME: Duration = Duration::from_millis(10);

/// The built-in definitions' sources, by ID.
pub const BUILTIN: &[(&str, &str)] = &[
    ("toggle", include_str!("../../transforms/toggle.js")),
    ("counter", include_str!("../../transforms/counter.js")),
    ("position", include_str!("../../transforms/position.js")),
    ("velocity", include_str!("../../transforms/velocity.js")),
    ("select", include_str!("../../transforms/select.js")),
    ("absolute", include_str!("../../transforms/absolute.js")),
    ("rate", include_str!("../../transforms/rate.js")),
    ("latch", include_str!("../../transforms/latch.js")),
    ("script", include_str!("../../transforms/script.js")),
    ("format", include_str!("../../transforms/format.js")),
    ("template", include_str!("../../transforms/template.js")),
    ("rules", include_str!("../../transforms/rules.js")),
    ("compare", include_str!("../../transforms/compare.js")),
    ("all", include_str!("../../transforms/all.js")),
    ("any", include_str!("../../transforms/any.js")),
    ("not", include_str!("../../transforms/not.js")),
    ("map", include_str!("../../transforms/map.js")),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Reducer,
    Stateless,
}

/// What steps a reducer, by default.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Steps {
    /// Event names; `"*"` is any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<Vec<String>>,
    /// The input's type, when its changes step it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,
}

/// A loaded definition: what it says about itself, and its source.
#[derive(Clone, Debug, PartialEq)]
pub struct Definition {
    pub id: String,
    pub version: u32,
    pub kind: Kind,
    /// Option name → its schema, with `default` where it has one.
    pub options: Map<String, Json>,
    pub steps: Steps,
    pub result: Json,
    /// Event name → its payload's schema.
    pub events: Map<String, Json>,
    pub source: Arc<str>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Described {
    id: String,
    version: u32,
    kind: Kind,
    #[serde(default)]
    options: Map<String, Json>,
    #[serde(default)]
    steps: Steps,
    #[serde(default)]
    result: Json,
    #[serde(default)]
    events: Map<String, Json>,
}

impl Definition {
    /// Loads a definition and reads what it says about itself.
    pub fn load(sandbox: &Sandbox, source: &str) -> Result<Self, ExprError> {
        let described = sandbox
            .js
            .call_definition(source, "", &json!([]))?
            .flatten()
            .ok_or_else(|| ExprError::new(ExprCode::Error, "the definition is empty"))?;
        let d: Described = serde_json::from_value(described)
            .map_err(|e| ExprError::new(ExprCode::Error, format!("the definition: {e}")))?;
        Ok(Definition {
            id: d.id,
            version: d.version,
            kind: d.kind,
            options: d.options,
            steps: d.steps,
            result: d.result,
            events: d.events,
            source: source.into(),
        })
    }

    /// `toggle@1`.
    pub fn name(&self) -> String {
        format!("{}@{}", self.id, self.version)
    }

    /// `options` with each missing option's `default`.
    pub fn with_defaults(&self, options: &Map<String, Json>) -> Map<String, Json> {
        let mut out = options.clone();
        for (name, schema) in &self.options {
            if let Some(d) = schema.get("default")
                && !out.contains_key(name)
            {
                out.insert(name.clone(), d.clone());
            }
        }
        out
    }

    /// Whether `name` steps the reducer by default.
    pub fn steps_on(&self, name: &str) -> bool {
        self.steps
            .on
            .as_ref()
            .is_some_and(|names| names.iter().any(|n| n == "*" || n == name))
    }
}

/// The built-in definitions, loaded once.
pub fn builtins() -> &'static BTreeMap<String, Definition> {
    static LOADED: OnceLock<BTreeMap<String, Definition>> = OnceLock::new();
    LOADED.get_or_init(|| {
        let sandbox = Sandbox::new();
        BUILTIN
            .iter()
            .map(|(id, source)| {
                let d = Definition::load(&sandbox, source)
                    .unwrap_or_else(|e| panic!("the built-in {id} definition loads: {e}"));
                assert_eq!(d.id, *id, "{id}.js defines {}", d.id);
                (d.id.clone(), d)
            })
            .collect()
    })
}

/// A built-in definition by ID.
pub fn builtin(id: &str) -> Option<&'static Definition> {
    builtins().get(id)
}

/// An event that steps a reducer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StepEvent {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Json>,
}

/// What a reducer's `step` sees besides its state. `None` is `undefined`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StepCtx {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<StepEvent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Json>,
    /// The input at the previous step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<Json>,
    /// The cell's current value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Json>,
    /// What the reducer's `follow` source gives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub followed: Option<Json>,
    /// With their defaults.
    pub options: Map<String, Json>,
    /// Scheduler time, in milliseconds.
    pub now: u64,
}

/// An event a step gives, at its cell.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Emitted {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Json>,
}

/// What a step gave: the new state, value and events, and when to be stepped
/// again. `None` fields are `undefined`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stepped {
    pub state: Option<Json>,
    pub value: Option<Json>,
    pub events: Vec<Emitted>,
    pub wake_at: Option<u64>,
}

impl Stepped {
    /// The state and value both.
    pub fn both(v: Json) -> Self {
        Stepped {
            state: Some(v.clone()),
            value: Some(v),
            ..Stepped::default()
        }
    }

    /// Reads what a definition's `step` returned.
    pub fn from_json(json: Json) -> Result<Self, ExprError> {
        let bad = |what: &str| {
            ExprError::new(
                ExprCode::Error,
                format!("a step returns {{state, value, events?, wakeAt?}} or undefined; {what}"),
            )
        };
        let Json::Object(mut map) = json else {
            return Err(bad("this isn't an object"));
        };
        let events = match map.remove("events") {
            None | Some(Json::Null) => Vec::new(),
            Some(Json::Array(items)) => items
                .into_iter()
                .map(|item| match item {
                    Json::String(name) => Ok(Emitted {
                        name,
                        payload: None,
                    }),
                    other => serde_json::from_value(other).map_err(|e| bad(&e.to_string())),
                })
                .collect::<Result<_, _>>()?,
            Some(_) => return Err(bad("`events` isn't a list")),
        };
        let wake_at = match map.remove("wakeAt") {
            None | Some(Json::Null) => None,
            Some(Json::Number(n)) => n.as_f64().map(|t| t.max(0.0) as u64),
            Some(_) => return Err(bad("`wakeAt` isn't a number")),
        };
        Ok(Stepped {
            state: map.remove("state"),
            value: map.remove("value"),
            events,
            wake_at,
        })
    }
}

/// Calls definitions under the bindings' limits. Each call gets a fresh
/// context, so a definition keeps nothing between calls.
pub struct Sandbox {
    js: QuickJs,
}

impl Default for Sandbox {
    fn default() -> Self {
        Self::new()
    }
}

impl Sandbox {
    pub fn new() -> Self {
        Sandbox {
            js: QuickJs::new(Limits {
                time: CALL_TIME,
                ..Limits::default()
            }),
        }
    }

    fn call(
        &self,
        d: &Definition,
        method: &str,
        args: Json,
    ) -> Result<Option<Option<Json>>, ExprError> {
        self.js.call_definition(&d.source, method, &args)
    }

    /// A reducer's initial state and value.
    pub fn init(
        &self,
        d: &Definition,
        options: &Map<String, Json>,
        now: u64,
    ) -> Result<(Option<Json>, Option<Json>), ExprError> {
        let state = self
            .call(d, "init", json!([{ "options": options, "now": now }]))?
            .ok_or_else(|| ExprError::new(ExprCode::Error, "a reducer has `init`"))?;
        let value = match self.call(
            d,
            "initValue",
            json!([{ "state": state, "options": options }]),
        )? {
            Some(value) => value,
            None => state.clone(),
        };
        Ok((state, value))
    }

    /// One step. `None` if nothing changed.
    pub fn step(
        &self,
        d: &Definition,
        state: Option<&Json>,
        ctx: &StepCtx,
    ) -> Result<Option<Stepped>, ExprError> {
        let ctx = serde_json::to_value(ctx).expect("a step context encodes");
        // An `undefined` state arrives as `null`.
        let args = json!([state.cloned().unwrap_or(Json::Null), ctx]);
        match self.call(d, "step", args)? {
            None => Err(ExprError::new(ExprCode::Error, "a reducer has `step`")),
            Some(None) => Ok(None),
            Some(Some(r)) => Stepped::from_json(r).map(Some),
        }
    }

    /// What following `ctx.followed` does. `None` if the definition leaves it
    /// to the core: the followed value as both state and value.
    pub fn follow(
        &self,
        d: &Definition,
        state: Option<&Json>,
        ctx: &StepCtx,
    ) -> Result<Option<Stepped>, ExprError> {
        let ctx = serde_json::to_value(ctx).expect("a step context encodes");
        let state = state.cloned().unwrap_or(Json::Null);
        match self.call(d, "follow", json!([state, ctx]))? {
            None => Ok(None),
            Some(None) => Ok(Some(Stepped::default())),
            Some(Some(r)) => Stepped::from_json(r).map(Some),
        }
    }

    /// A stateless transform's value, with its options evaluated.
    pub fn compute(
        &self,
        d: &Definition,
        options: &Map<String, Json>,
    ) -> Result<Option<Json>, ExprError> {
        self.call(d, "compute", json!([{ "options": options }]))?
            .ok_or_else(|| ExprError::new(ExprCode::Error, "a stateless transform has `compute`"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_loads_and_describes_itself() {
        let all = builtins();
        assert_eq!(all.len(), BUILTIN.len());
        let toggle = &all["toggle"];
        assert_eq!(toggle.kind, Kind::Reducer);
        assert_eq!(toggle.name(), "toggle@1");
        assert!(toggle.steps_on("press"));
        assert_eq!(all["map"].kind, Kind::Stateless);
        assert!(all["latch"].steps_on("anything"));
    }

    #[test]
    fn a_definition_runs_sandboxed() {
        let sandbox = Sandbox::new();
        let clock = "export default defineTransform({ id: 'x', version: 1, kind: 'reducer', \
                     init: () => Date.now(), step: () => undefined });";
        let d = Definition::load(&sandbox, clock).unwrap();
        let e = sandbox.init(&d, &Map::new(), 0).unwrap_err();
        assert!(e.message.contains("Date.now"), "{e}");

        let spin = "export default defineTransform({ id: 'x', version: 1, kind: 'reducer', \
                    init: () => { for (;;) {} }, step: () => undefined });";
        let d = Definition::load(&sandbox, spin).unwrap();
        assert_eq!(
            sandbox.init(&d, &Map::new(), 0).unwrap_err().code,
            ExprCode::Timeout
        );
    }

    #[test]
    fn toggle_flips_in_its_definition() {
        let sandbox = Sandbox::new();
        let d = builtin("toggle").unwrap();
        let options = d.with_defaults(&Map::new());
        let (state, value) = sandbox.init(d, &options, 0).unwrap();
        assert_eq!(
            (state.clone(), value),
            (Some(json!(false)), Some(json!(false)))
        );
        let ctx = StepCtx {
            event: Some(StepEvent {
                name: "press".into(),
                payload: None,
            }),
            options,
            ..StepCtx::default()
        };
        let stepped = sandbox.step(d, state.as_ref(), &ctx).unwrap().unwrap();
        assert_eq!(stepped.value, Some(json!(true)));
        assert_eq!(stepped.events[0].name, "on");
    }
}

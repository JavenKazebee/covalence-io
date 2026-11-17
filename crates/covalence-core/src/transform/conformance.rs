//! The conformance runner (dataflow spec §4.4): the same inputs through a
//! transform's definition and its native implementation, requiring equal
//! states, values and events at every step.
//!
//! A suite is JSON, one per transform, in `transforms/conformance/`:
//!
//! ```json
//! { "transform": "position",
//!   "cases": [
//!     { "name": "clamps at max", "options": { "max": 3 },
//!       "steps": [ { "input": 0 }, { "input": 5, "expect": 3 },
//!                  { "options": { "max": 2 }, "input": 6, "expect": 2 } ] } ] }
//! ```
//!
//! A reducer step may set `event` (a name, or `{name, payload}`), `input`,
//! `follow` (a followed value, applied through the definition's `follow`),
//! `options` (merged over the case's from this step on: options change
//! between steps, so a definition must accept state made under others) and
//! `now` (default 10 ms after the last). `expect` checks the value, and
//! `expectEvents` the event names. A step with none of `event`, `input` or
//! `follow` is a wake. A stateless case has `options`, already evaluated,
//! and `expect`.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Map, Value as Json};

use super::Transform;
use super::definition::{Definition, Kind, Sandbox, StepCtx, StepEvent, Stepped};
use super::reducer::Native;
use crate::expr::{Evaluator, QuickJs, Scope};
use crate::source::Eval;
use crate::state::LiveState;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Suite {
    pub transform: String,
    pub cases: Vec<Case>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub name: String,
    #[serde(default)]
    pub options: Map<String, Json>,
    #[serde(default)]
    pub steps: Vec<Step>,
    /// A stateless case's result; absent is `undefined`.
    #[serde(default)]
    pub expect: Option<Json>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Step {
    #[serde(default)]
    pub event: Option<Json>,
    #[serde(default)]
    pub input: Option<Json>,
    #[serde(default)]
    pub follow: Option<Json>,
    #[serde(default)]
    pub options: Option<Map<String, Json>>,
    #[serde(default)]
    pub now: Option<u64>,
    #[serde(default)]
    pub expect: Option<Json>,
    #[serde(default)]
    pub expect_events: Option<Vec<String>>,
}

/// One implementation's view of a reducer, for the runner.
trait Run {
    fn init(
        &self,
        options: &Map<String, Json>,
        now: u64,
    ) -> Result<(Option<Json>, Option<Json>), String>;
    fn step(&self, state: Option<&Json>, ctx: &StepCtx) -> Result<Option<Stepped>, String>;
    fn follow(&self, state: Option<&Json>, ctx: &StepCtx) -> Result<Option<Stepped>, String>;
}

struct ByDefinition<'a> {
    sandbox: &'a Sandbox,
    definition: &'a Definition,
}

impl Run for ByDefinition<'_> {
    fn init(
        &self,
        options: &Map<String, Json>,
        now: u64,
    ) -> Result<(Option<Json>, Option<Json>), String> {
        self.sandbox
            .init(self.definition, options, now)
            .map_err(|e| e.to_string())
    }
    fn step(&self, state: Option<&Json>, ctx: &StepCtx) -> Result<Option<Stepped>, String> {
        self.sandbox
            .step(self.definition, state, ctx)
            .map_err(|e| e.to_string())
    }
    fn follow(&self, state: Option<&Json>, ctx: &StepCtx) -> Result<Option<Stepped>, String> {
        self.sandbox
            .follow(self.definition, state, ctx)
            .map_err(|e| e.to_string())
    }
}

struct ByNative<'a>(&'a dyn Native);

impl Run for ByNative<'_> {
    fn init(
        &self,
        options: &Map<String, Json>,
        now: u64,
    ) -> Result<(Option<Json>, Option<Json>), String> {
        Ok(self.0.init(options, now))
    }
    fn step(&self, state: Option<&Json>, ctx: &StepCtx) -> Result<Option<Stepped>, String> {
        Ok(self.0.step(state, ctx))
    }
    fn follow(&self, state: Option<&Json>, ctx: &StepCtx) -> Result<Option<Stepped>, String> {
        Ok(self.0.follow(state, ctx))
    }
}

/// What a reducer has at one point of a case.
#[derive(Clone, Debug, PartialEq)]
pub struct Trace {
    pub state: Option<Json>,
    pub value: Option<Json>,
    pub events: Vec<(String, Option<Json>)>,
    pub wake_at: Option<u64>,
}

/// Runs a reducer case through one implementation: the trace after `init`
/// and after each step.
fn trace(run: &dyn Run, d: &Definition, case: &Case) -> Result<Vec<Trace>, String> {
    let mut options = case.options.clone();
    let mut now = 0u64;
    let (mut state, mut value) = run.init(&d.with_defaults(&options), now)?;
    let mut out = vec![Trace {
        state: state.clone(),
        value: value.clone(),
        events: Vec::new(),
        wake_at: None,
    }];
    let mut input: Option<Json> = None;
    let mut previous: Option<Json> = None;
    let mut followed: Option<Json> = None;
    for (i, step) in case.steps.iter().enumerate() {
        if let Some(o) = &step.options {
            options.extend(o.clone());
        }
        now = step.now.unwrap_or(now + 10);
        if let Some(v) = &step.input {
            input = Some(v.clone());
        }
        let event = match &step.event {
            None => None,
            Some(Json::String(name)) => Some(StepEvent {
                name: name.clone(),
                payload: None,
            }),
            Some(other) => Some(
                serde_json::from_value(other.clone())
                    .map_err(|e| format!("step {i}: event: {e}"))?,
            ),
        };
        let following = step.follow.is_some();
        if let Some(f) = &step.follow {
            followed = Some(f.clone());
        }
        let ctx = StepCtx {
            event,
            input: input.clone(),
            previous: previous.clone(),
            value: value.clone(),
            followed: followed.clone(),
            options: d.with_defaults(&options),
            now,
        };
        let stepped = if following {
            Some(
                run.follow(state.as_ref(), &ctx)?
                    .unwrap_or_else(|| Stepped::both(followed.clone().unwrap_or(Json::Null))),
            )
        } else {
            run.step(state.as_ref(), &ctx)?
        };
        if step.input.is_some() {
            previous = input.clone();
        }
        let mut t = Trace {
            state: state.clone(),
            value: value.clone(),
            events: Vec::new(),
            wake_at: None,
        };
        if let Some(s) = stepped {
            state = s.state.clone();
            value = s.value.clone();
            t = Trace {
                state: s.state,
                value: s.value,
                events: s.events.into_iter().map(|e| (e.name, e.payload)).collect(),
                wake_at: s.wake_at,
            };
        }
        out.push(t);
    }
    Ok(out)
}

/// Numbers compared by value: `2` and `2.0` are the same in JavaScript.
pub fn same(a: &Json, b: &Json) -> bool {
    match (a, b) {
        (Json::Number(x), Json::Number(y)) => x.as_f64() == y.as_f64(),
        (Json::Array(x), Json::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| same(a, b))
        }
        (Json::Object(x), Json::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        _ => a == b,
    }
}

fn same_opt(a: &Option<Json>, b: &Option<Json>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => same(a, b),
        _ => false,
    }
}

fn same_trace(a: &Trace, b: &Trace) -> bool {
    same_opt(&a.state, &b.state)
        && same_opt(&a.value, &b.value)
        && a.wake_at == b.wake_at
        && a.events.len() == b.events.len()
        && a.events
            .iter()
            .zip(&b.events)
            .all(|((n, p), (m, q))| n == m && same_opt(p, q))
}

/// Runs a reducer case through the definition, and through `native` when
/// given, and checks them against each other and the case's expectations.
pub fn run_reducer(
    sandbox: &Sandbox,
    d: &Definition,
    native: Option<&dyn Native>,
    case: &Case,
) -> Result<(), String> {
    let by_definition = trace(
        &ByDefinition {
            sandbox,
            definition: d,
        },
        d,
        case,
    )
    .map_err(|e| format!("{}: the definition failed: {e}", case.name))?;
    if let Some(native) = native {
        let by_native = trace(&ByNative(native), d, case)?;
        for (i, (a, b)) in by_definition.iter().zip(&by_native).enumerate() {
            if !same_trace(a, b) {
                let at = if i == 0 {
                    "init".into()
                } else {
                    format!("step {}", i - 1)
                };
                return Err(format!(
                    "{}: {at}: the definition gives {a:?}, the native implementation {b:?}",
                    case.name
                ));
            }
        }
    }
    for (i, step) in case.steps.iter().enumerate() {
        let t = &by_definition[i + 1];
        if let Some(want) = &step.expect
            && !same_opt(&t.value, &Some(want.clone()))
        {
            return Err(format!(
                "{}: step {i}: the value is {:?}, expected {want}",
                case.name, t.value
            ));
        }
        if let Some(want) = &step.expect_events {
            let got: Vec<&String> = t.events.iter().map(|(n, _)| n).collect();
            if got
                .iter()
                .map(|s| s.as_str())
                .ne(want.iter().map(String::as_str))
            {
                return Err(format!(
                    "{}: step {i}: the events are {got:?}, expected {want:?}",
                    case.name
                ));
            }
        }
    }
    Ok(())
}

/// Runs a stateless case through the definition and [`Transform::evaluate`].
pub fn run_stateless(sandbox: &Sandbox, d: &Definition, case: &Case) -> Result<(), String> {
    let options = d.with_defaults(&case.options);
    let by_definition = sandbox
        .compute(d, &options)
        .map_err(|e| format!("{}: the definition failed: {e}", case.name))?;
    let mut json = case.options.clone();
    json.insert("use".into(), Json::String(d.id.clone()));
    let transform: Transform = serde_json::from_value(Json::Object(json))
        .map_err(|e| format!("{}: not a {} transform: {e}", case.name, d.id))?;
    let state = Arc::new(LiveState::new());
    let aliases = BTreeMap::new();
    let scope = Scope::new(&state, &aliases);
    let js = QuickJs::default();
    let mut eval = Eval::new(&scope, &js as &dyn Evaluator);
    let native = transform
        .evaluate(&mut eval)
        .map_err(|e| format!("{}: the native transform failed: {e}", case.name))?;
    if !same_opt(&by_definition, &native) {
        return Err(format!(
            "{}: the definition gives {by_definition:?}, the native transform {native:?}",
            case.name
        ));
    }
    if !same_opt(&by_definition, &case.expect) {
        return Err(format!(
            "{}: the value is {by_definition:?}, expected {:?}",
            case.name, case.expect
        ));
    }
    Ok(())
}

/// Runs a whole suite against its built-in definition and native code.
pub fn run_suite(sandbox: &Sandbox, suite: &Suite) -> Result<(), String> {
    let d = super::definition::builtin(&suite.transform)
        .ok_or_else(|| format!("no built-in transform {:?}", suite.transform))?;
    for case in &suite.cases {
        match d.kind {
            Kind::Reducer => run_reducer(sandbox, d, super::reducer::native(&d.id), case)?,
            Kind::Stateless => run_stateless(sandbox, d, case)?,
        }
    }
    Ok(())
}

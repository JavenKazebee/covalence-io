//! Reducers (dataflow spec §4.2): transforms that keep a value of their own
//! and step on events or on their input changing.
//!
//! The definitions in `transforms/` are the specification. These are their
//! native implementations, for the turn's budget (state spec §7.3); each follows
//! its definition operation by operation, so floating point agrees, and the
//! conformance suite ([`super::conformance`]) holds them to it. `script` has
//! no native implementation: it runs in the sandbox.

use serde_json::{Map, Value as Json, json};

use super::definition::{Emitted, StepCtx, Stepped};

/// A reducer implemented in Rust.
pub trait Native: Send + Sync {
    /// The initial state and value.
    fn init(&self, options: &Map<String, Json>, now: u64) -> (Option<Json>, Option<Json>);

    /// One step; `None` if nothing changed.
    fn step(&self, state: Option<&Json>, ctx: &StepCtx) -> Option<Stepped>;

    /// What following a value does; `None` for the default (the followed
    /// value as both state and value).
    fn follow(&self, _state: Option<&Json>, _ctx: &StepCtx) -> Option<Stepped> {
        None
    }
}

/// The native implementation of a built-in reducer, if it has one.
pub fn native(id: &str) -> Option<&'static dyn Native> {
    Some(match id {
        "toggle" => &Toggle,
        "counter" => &Counter,
        "position" => &Position,
        "velocity" => &Velocity,
        "select" => &Select,
        "absolute" => &Absolute,
        "rate" => &Rate,
        "latch" => &Latch,
        _ => return None,
    })
}

/// A number as JSON, as it comes out of `JSON.stringify`: integral values
/// as integers.
pub fn num(x: f64) -> Json {
    if x.fract() == 0.0 && x.abs() < 9_007_199_254_740_992.0 {
        // `-0` prints as `0`.
        json!(x as i64)
    } else {
        serde_json::Number::from_f64(x).map_or(Json::Null, Json::Number)
    }
}

fn f(v: Option<&Json>) -> Option<f64> {
    v.and_then(Json::as_f64)
}

fn opt<'a>(ctx: &'a StepCtx, name: &str) -> Option<&'a Json> {
    ctx.options.get(name)
}

fn opt_f(ctx: &StepCtx, name: &str) -> Option<f64> {
    f(opt(ctx, name))
}

fn opt_bool(options: &Map<String, Json>, name: &str) -> bool {
    options.get(name) == Some(&Json::Bool(true))
}

fn change(value: f64) -> Emitted {
    Emitted {
        name: "change".into(),
        payload: Some(json!({ "value": num(value) })),
    }
}

fn named(name: &str) -> Emitted {
    Emitted {
        name: name.into(),
        payload: None,
    }
}

/// `Math.min(Math.max(x, lo), hi)`, with JavaScript's NaN.
fn clamp(x: f64, lo: f64, hi: f64) -> f64 {
    super::js::min(super::js::max(x, lo), hi)
}

/// Both numbers, or no step.
fn moved(ctx: &StepCtx) -> Option<(f64, f64)> {
    match (&ctx.input, &ctx.previous) {
        (Some(Json::Number(i)), Some(Json::Number(p))) => Some((i.as_f64()?, p.as_f64()?)),
        _ => None,
    }
}

fn same(state: Option<&Json>, next: f64) -> bool {
    f(state) == Some(next)
}

struct Toggle;

impl Native for Toggle {
    fn init(&self, options: &Map<String, Json>, _: u64) -> (Option<Json>, Option<Json>) {
        let v = json!(opt_bool(options, "initial"));
        (Some(v.clone()), Some(v))
    }

    fn step(&self, state: Option<&Json>, ctx: &StepCtx) -> Option<Stepped> {
        ctx.event.as_ref()?;
        let next = state != Some(&Json::Bool(true));
        Some(Stepped {
            events: vec![named(if next { "on" } else { "off" })],
            ..Stepped::both(json!(next))
        })
    }
}

struct Counter;

impl Native for Counter {
    fn init(&self, options: &Map<String, Json>, _: u64) -> (Option<Json>, Option<Json>) {
        let v = options.get("initial").cloned();
        (v.clone(), v)
    }

    fn step(&self, state: Option<&Json>, ctx: &StepCtx) -> Option<Stepped> {
        ctx.event.as_ref()?;
        let (min, max) = (opt_f(ctx, "min"), opt_f(ctx, "max"));
        let wrap = opt_bool(&ctx.options, "wrap");
        let from = f(state)
            .or_else(|| opt_f(ctx, "initial"))
            .unwrap_or(f64::NAN);
        let mut next = from + opt_f(ctx, "step").unwrap_or(f64::NAN);
        if let Some(max) = max
            && next > max
        {
            next = match min {
                Some(min) if wrap => min,
                _ => max,
            };
        }
        if let Some(min) = min
            && next < min
        {
            next = match max {
                Some(max) if wrap => max,
                _ => min,
            };
        }
        if same(state, next) {
            return None;
        }
        Some(Stepped {
            events: vec![change(next)],
            ..Stepped::both(num(next))
        })
    }
}

struct Position;

impl Native for Position {
    fn init(&self, options: &Map<String, Json>, _: u64) -> (Option<Json>, Option<Json>) {
        let v = match options.get("initial") {
            Some(Json::Number(n)) => Some(Json::Number(n.clone())),
            _ => options.get("min").cloned(),
        };
        (v.clone(), v)
    }

    fn step(&self, state: Option<&Json>, ctx: &StepCtx) -> Option<Stepped> {
        let (input, previous) = moved(ctx)?;
        let d = input - previous;
        if d == 0.0 {
            return None;
        }
        let g = |name| opt_f(ctx, name).unwrap_or(f64::NAN);
        let (min, max) = (g("min"), g("max"));
        let moved = d * (1.0 + g("accel") * (d.abs() - 1.0)) * g("step");
        let mut next = f(state).unwrap_or(min) + moved;
        let span = max - min;
        if opt_bool(&ctx.options, "wrap") && span > 0.0 {
            next = min + (((next - min) % span) + span) % span;
        } else {
            next = clamp(next, min, max);
        }
        if same(state, next) {
            return None;
        }
        Some(Stepped {
            events: vec![change(next)],
            ..Stepped::both(num(next))
        })
    }
}

struct Velocity;

impl Native for Velocity {
    fn init(&self, _: &Map<String, Json>, _: u64) -> (Option<Json>, Option<Json>) {
        (
            Some(json!({ "samples": [], "rate": 0, "at": 0 })),
            Some(json!(0)),
        )
    }

    fn step(&self, state: Option<&Json>, ctx: &StepCtx) -> Option<Stepped> {
        let now = ctx.now as f64;
        let g = |name| opt_f(ctx, name).unwrap_or(f64::NAN);
        let (window, decay, scale) = (g("window"), g("decay"), g("scale"));
        let s = state.filter(|s| s.get("samples").is_some_and(Json::is_array));
        let (old, rate0, at) = match s {
            Some(s) => (
                s["samples"].as_array().cloned().unwrap_or_default(),
                f(s.get("rate")).unwrap_or(f64::NAN),
                f(s.get("at")).unwrap_or(f64::NAN),
            ),
            None => (Vec::new(), 0.0, now),
        };
        let mut samples: Vec<Json> = old
            .into_iter()
            .filter(|sample| now - f(sample.get(0)).unwrap_or(f64::NAN) < window)
            .collect();
        if let Some((input, previous)) = moved(ctx)
            && input != previous
        {
            samples.push(json!([num(now), num(input - previous)]));
        }
        let mut sum = 0.0;
        for sample in &samples {
            sum += f(sample.get(1)).unwrap_or(f64::NAN);
        }
        let raw = if samples.is_empty() {
            0.0
        } else {
            ((sum * 1000.0) / window) * scale
        };
        let decayed = if decay > 0.0 {
            rate0 * 0.5f64.powf((now - at) / decay)
        } else {
            0.0
        };
        let mut rate = if raw.abs() >= decayed.abs() {
            raw
        } else {
            decayed
        };
        if rate.abs() < 0.001 {
            rate = 0.0;
        }
        let wake_at = (rate != 0.0 || !samples.is_empty()).then_some(ctx.now + 20);
        Some(Stepped {
            state: Some(json!({ "samples": samples, "rate": num(rate), "at": num(now) })),
            value: Some(num(rate)),
            events: Vec::new(),
            wake_at,
        })
    }
}

struct Select;

impl Select {
    fn size(options: &Map<String, Json>) -> f64 {
        match (options.get("items"), options.get("count")) {
            (Some(Json::Array(items)), _) => items.len() as f64,
            (_, Some(Json::Number(n))) => n.as_f64().map_or(0.0, |n| n.floor().max(0.0)),
            _ => 0.0,
        }
    }

    fn clamp(i: Option<&Json>, n: f64) -> f64 {
        clamp(f(i).unwrap_or(0.0), 0.0, n - 1.0)
    }

    fn at(options: &Map<String, Json>, i: f64) -> Option<Json> {
        match options.get("items") {
            Some(Json::Array(items)) => {
                (i >= 0.0 && i.fract() == 0.0).then(|| items.get(i as usize).cloned())?
            }
            _ => Some(num(i)),
        }
    }
}

impl Native for Select {
    fn init(&self, options: &Map<String, Json>, _: u64) -> (Option<Json>, Option<Json>) {
        let state = options.get("initial").cloned();
        let n = Self::size(options);
        let value = if n > 0.0 {
            Self::at(options, Self::clamp(state.as_ref(), n))
        } else {
            None
        };
        (state, value)
    }

    fn step(&self, state: Option<&Json>, ctx: &StepCtx) -> Option<Stepped> {
        let (input, previous) = moved(ctx)?;
        let n = Self::size(&ctx.options);
        if n == 0.0 {
            return None;
        }
        let from = Self::clamp(state, n);
        let mut next = from + super::js::round(input - previous);
        next = if opt_bool(&ctx.options, "wrap") {
            ((next % n) + n) % n
        } else {
            clamp(next, 0.0, n - 1.0)
        };
        if same(state, next) {
            return None;
        }
        let value = Self::at(&ctx.options, next);
        let mut payload = Map::new();
        payload.insert("index".into(), num(next));
        if let Some(v) = &value {
            payload.insert("value".into(), v.clone());
        }
        Some(Stepped {
            state: Some(num(next)),
            value,
            events: vec![Emitted {
                name: "select".into(),
                payload: Some(Json::Object(payload)),
            }],
            wake_at: None,
        })
    }
}

struct Absolute;

impl Absolute {
    fn shape(curve: Option<&Json>, x: f64) -> f64 {
        match curve.and_then(Json::as_str) {
            Some("log") => (1.0 + 9.0 * x).log10(),
            Some("exp") => (10f64.powf(x) - 1.0) / 9.0,
            _ => x,
        }
    }
}

impl Native for Absolute {
    fn init(&self, _: &Map<String, Json>, _: u64) -> (Option<Json>, Option<Json>) {
        (Some(json!({ "picked": false })), None)
    }

    fn step(&self, state: Option<&Json>, ctx: &StepCtx) -> Option<Stepped> {
        let input = match &ctx.input {
            Some(Json::Number(n)) => n.as_f64()?,
            _ => return None,
        };
        let curve = opt(ctx, "curve");
        let y = Self::shape(curve, clamp(input, 0.0, 1.0));
        let mut picked = state.and_then(|s| s.get("picked")) == Some(&Json::Bool(true));
        if opt_bool(&ctx.options, "pickup") && !picked {
            match &ctx.followed {
                Some(Json::Number(followed)) => {
                    let followed = followed.as_f64().unwrap_or(f64::NAN);
                    let p = match f(ctx.previous.as_ref()) {
                        Some(p) => Self::shape(curve, clamp(p, 0.0, 1.0)),
                        None => y,
                    };
                    picked = (y - followed).abs() <= 0.01 || (p - followed) * (y - followed) < 0.0;
                }
                _ => picked = true,
            }
            if !picked {
                return Some(Stepped {
                    state: Some(json!({ "picked": false })),
                    value: ctx.value.clone(),
                    events: Vec::new(),
                    wake_at: None,
                });
            }
        }
        let changed = f(ctx.value.as_ref()) != Some(y);
        Some(Stepped {
            state: Some(json!({ "picked": true })),
            value: Some(num(y)),
            events: if changed { vec![change(y)] } else { Vec::new() },
            wake_at: None,
        })
    }

    fn follow(&self, _: Option<&Json>, ctx: &StepCtx) -> Option<Stepped> {
        Some(Stepped {
            state: Some(json!({ "picked": false })),
            value: ctx.followed.clone(),
            events: Vec::new(),
            wake_at: None,
        })
    }
}

struct Rate;

impl Native for Rate {
    fn init(&self, _: &Map<String, Json>, _: u64) -> (Option<Json>, Option<Json>) {
        (Some(json!(0)), Some(json!(0)))
    }

    fn step(&self, state: Option<&Json>, ctx: &StepCtx) -> Option<Stepped> {
        let input = match &ctx.input {
            Some(Json::Number(n)) => n.as_f64()?,
            _ => return None,
        };
        let g = |name| opt_f(ctx, name).unwrap_or(f64::NAN);
        let (centre, deadband) = (g("centre"), g("deadband"));
        let d = input - centre;
        let sign = if d < 0.0 { -1.0 } else { 1.0 };
        let span = if d < 0.0 { centre } else { 1.0 - centre };
        let r = if d.abs() <= deadband {
            0.0
        } else if span - deadband <= 0.0 {
            sign
        } else {
            (sign * (d.abs() - deadband)) / (span - deadband)
        };
        let r = clamp(r, -1.0, 1.0);
        if same(state, r) {
            return None;
        }
        Some(Stepped::both(num(r)))
    }
}

struct Latch;

impl Latch {
    fn names(v: Option<&Json>) -> Vec<&str> {
        match v {
            Some(Json::Array(items)) => items.iter().filter_map(Json::as_str).collect(),
            Some(Json::String(s)) => vec![s.as_str()],
            _ => Vec::new(),
        }
    }
}

impl Native for Latch {
    fn init(&self, options: &Map<String, Json>, _: u64) -> (Option<Json>, Option<Json>) {
        let v = json!(opt_bool(options, "initial"));
        (Some(v.clone()), Some(v))
    }

    fn step(&self, state: Option<&Json>, ctx: &StepCtx) -> Option<Stepped> {
        let event = ctx.event.as_ref()?;
        let next = if Self::names(opt(ctx, "set")).contains(&event.name.as_str()) {
            true
        } else if Self::names(opt(ctx, "reset")).contains(&event.name.as_str()) {
            false
        } else {
            return None;
        };
        if state == Some(&Json::Bool(next)) {
            return None;
        }
        Some(Stepped {
            events: vec![named(if next { "on" } else { "off" })],
            ..Stepped::both(json!(next))
        })
    }
}

// ---------------------------------------------------------------------------
// A reducer as a source: `{use: "toggle", on?, input?, follow?, holdoff?, …}`

/// The built-in reducers' IDs.
pub const REDUCERS: &[&str] = &[
    "toggle", "counter", "position", "velocity", "select", "absolute", "rate", "latch", "script",
];

/// The keys every reducer takes besides its own options (dataflow §4.2).
const COMMON: &[&str] = &["on", "input", "follow", "holdoff"];

/// A reducer source. It only has a home as a cell's source (dataflow §4.2).
#[derive(Clone, Debug, PartialEq)]
pub struct Reducer {
    /// The `use`, without a version.
    pub id: String,
    /// `toggle@1`'s `1`.
    pub version: Option<u32>,
    pub on: Option<On>,
    pub input: Option<crate::source::Source<Json>>,
    pub follow: Option<crate::source::Source<Json>>,
    pub holdoff: Option<crate::source::Source<Json>>,
    /// The reducer's own options, by name.
    pub options: std::collections::BTreeMap<String, crate::source::Source<Json>>,
}

/// The events that step a reducer (dataflow §3.1).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct On {
    /// Absent on a control's helper: that control (bindings §5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<crate::flow::EventSource>,
    /// Absent: the definition's default names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<crate::flow::Names>,
    /// Steps only when truthy, with `trigger` in scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
}

/// `toggle@1` as `("toggle", Some(1))`.
pub fn split_use(name: &str) -> Result<(&str, Option<u32>), String> {
    match name.split_once('@') {
        None => Ok((name, None)),
        Some((id, v)) => v
            .parse()
            .map(|v| (id, Some(v)))
            .map_err(|_| format!("{name:?}: a version is a whole number, as in `toggle@1`")),
    }
}

impl Reducer {
    /// Whether `id` (without a version) names a reducer.
    pub fn is_reducer(id: &str) -> bool {
        REDUCERS.contains(&id)
    }

    /// Reads a reducer from a source's object, `use` included.
    pub fn from_map(mut map: Map<String, Json>) -> Result<Self, String> {
        let name = match map.remove("use") {
            Some(Json::String(s)) => s,
            _ => return Err("`use` names a transform".into()),
        };
        let (id, version) = split_use(&name)?;
        let definition = super::definition::builtin(id)
            .ok_or_else(|| format!("unknown variant `{id}`: no such transform"))?;
        if let Some(v) = version
            && v != definition.version
        {
            return Err(format!(
                "there's no {id}@{v}; this core has {}",
                definition.name()
            ));
        }
        let source = |key: &str, map: &mut Map<String, Json>| {
            map.remove(key)
                .map(|v| crate::source::Source::from_json(v).map_err(|e| format!("{key}: {e}")))
                .transpose()
        };
        let on = map
            .remove("on")
            .map(|v| serde_json::from_value::<On>(v).map_err(|e| format!("on: {e}")))
            .transpose()?;
        let input = source("input", &mut map)?;
        let follow = source("follow", &mut map)?;
        let holdoff = source("holdoff", &mut map)?;
        let mut options = std::collections::BTreeMap::new();
        for (key, value) in map {
            if key == "fallback" {
                continue;
            }
            if !definition.options.contains_key(&key) {
                let known: Vec<&str> = COMMON
                    .iter()
                    .copied()
                    .chain(definition.options.keys().map(String::as_str))
                    .collect();
                return Err(format!(
                    "unknown field `{key}`: {id} takes {}",
                    known.join(", ")
                ));
            }
            let s = crate::source::Source::from_json(value).map_err(|e| format!("{key}: {e}"))?;
            options.insert(key, s);
        }
        Ok(Reducer {
            id: id.to_owned(),
            version,
            on,
            input,
            follow,
            holdoff,
            options,
        })
    }

    /// The stored form, `use` included.
    pub fn to_map(&self) -> Result<Map<String, Json>, serde_json::Error> {
        let mut map = Map::new();
        let name = match self.version {
            Some(v) => format!("{}@{v}", self.id),
            None => self.id.clone(),
        };
        map.insert("use".into(), Json::String(name));
        if let Some(on) = &self.on {
            map.insert("on".into(), serde_json::to_value(on)?);
        }
        for (key, s) in [
            ("input", &self.input),
            ("follow", &self.follow),
            ("holdoff", &self.holdoff),
        ] {
            if let Some(s) = s {
                map.insert(key.into(), s.to_json()?);
            }
        }
        for (key, s) in &self.options {
            map.insert(key.clone(), s.to_json()?);
        }
        Ok(map)
    }

    /// Its definition.
    pub fn definition(&self) -> &'static super::definition::Definition {
        super::definition::builtin(&self.id).expect("a parsed reducer has a definition")
    }

    /// `toggle@1`: what the kept-state rule compares (dataflow §5.4).
    pub fn name(&self) -> String {
        self.definition().name()
    }
}

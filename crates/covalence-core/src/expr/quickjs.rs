//! The expression engine: QuickJS-ng through `rquickjs` (ADR 0004, ADR 0021).
//!
//! Every evaluation gets a fresh context, so nothing carries over between
//! evaluations. The context has no timers, `performance` or weak references,
//! and `prelude.js` removes the clock, randomness and `Promise`. State is
//! read lazily through `read` and `keys`, which record what was read.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use covalence_protocol::Value;
use covalence_protocol::state::PathSegment;
use rquickjs::context::EvalOptions;
use rquickjs::context::intrinsic;
use rquickjs::{Context, Ctx, Exception, Function, Runtime};
use tokio::sync::watch;

use super::{At, Evaluation, Evaluator, ExprCode, ExprError, Limits, RESERVED_ROOTS, Reads, Scope};
use crate::state::{LiveState, Probe, StatePath};
use crate::text_path;

const PRELUDE: &str = include_str!("prelude.js");

/// The file name errors in user code are reported against.
const FILE: &str = "expr";

/// Stack for one evaluation; deep recursion fails with `exprError`.
const STACK: usize = 256 * 1024;

/// The QuickJS evaluator. Evaluations run one at a time.
pub struct QuickJs {
    runtime: Mutex<Runtime>,
    guard: Arc<Mutex<Guard>>,
    limits: Limits,
}

/// What the interrupt handler checks during an evaluation.
#[derive(Default)]
struct Guard {
    deadline: Option<Instant>,
    cancel: Option<watch::Receiver<bool>>,
    /// Why the handler stopped the evaluation.
    tripped: Option<ExprCode>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl QuickJs {
    pub fn new(limits: Limits) -> Self {
        let runtime = Runtime::new().expect("QuickJS runtime");
        runtime.set_memory_limit(limits.memory);
        runtime.set_max_stack_size(STACK);
        let guard = Arc::new(Mutex::new(Guard::default()));
        let handler = guard.clone();
        runtime.set_interrupt_handler(Some(Box::new(move || {
            let mut g = lock(&handler);
            if g.cancel.as_ref().is_some_and(|c| *c.borrow()) {
                g.tripped = Some(ExprCode::Cancelled);
            } else if g.deadline.is_some_and(|d| Instant::now() >= d) {
                g.tripped = Some(ExprCode::Timeout);
            }
            g.tripped.is_some()
        })));
        QuickJs {
            runtime: Mutex::new(runtime),
            guard,
            limits,
        }
    }

    /// Runs `f` in a fresh context under the limits.
    fn run<R>(
        &self,
        cancel: Option<&watch::Receiver<bool>>,
        f: impl FnOnce(&Ctx<'_>) -> Result<R, ExprError>,
    ) -> Result<R, ExprError> {
        let runtime = lock(&self.runtime);
        *lock(&self.guard) = Guard {
            deadline: Some(Instant::now() + self.limits.time),
            cancel: cancel.cloned(),
            tripped: None,
        };
        let result = match context(&runtime) {
            Ok(context) => context.with(|ctx| f(&ctx)),
            Err(e) => Err(engine_error(&e)),
        };
        let tripped = std::mem::take(&mut *lock(&self.guard)).tripped;
        let result = match (result, tripped) {
            (Err(_), Some(code)) => Err(match code {
                ExprCode::Cancelled => ExprError::new(code, "cancelled"),
                _ => ExprError::new(
                    code,
                    format!("ran longer than {} ms", self.limits.time.as_millis()),
                ),
            }),
            (result, _) => result,
        };
        if matches!(&result, Err(e) if e.code == ExprCode::Memory) {
            runtime.run_gc();
        }
        result
    }
}

impl Default for QuickJs {
    fn default() -> Self {
        Self::new(Limits::default())
    }
}

impl Evaluator for QuickJs {
    fn check(&self, source: &str) -> Result<(), ExprError> {
        self.run(None, |ctx| compile(ctx, source).map(drop))
    }

    fn evaluate(&self, source: &str, scope: &Scope<'_>) -> Evaluation {
        let tracker = Rc::new(RefCell::new(Tracker::new(scope.state.clone())));
        let value = self.run(scope.cancel, |ctx| {
            let out = setup(ctx, scope, &tracker)?;
            let f = compile(ctx, source)?;
            let result: rquickjs::Value = f.call(()).map_err(|e| thrown(ctx, &e))?;
            let json: Option<String> = out.call((result,)).map_err(|e| thrown(ctx, &e))?;
            json.map(|j| from_json(&j)).transpose()
        });
        let reads = Rc::try_unwrap(tracker)
            .map(|t| t.into_inner().reads)
            .unwrap_or_else(|t| t.borrow().reads.clone());
        Evaluation { value, reads }
    }
}

/// A context with only what expressions need.
fn context(runtime: &Runtime) -> rquickjs::Result<Context> {
    Context::builder()
        .with::<intrinsic::Eval>()
        .with::<intrinsic::Date>()
        .with::<intrinsic::RegExpCompiler>()
        .with::<intrinsic::RegExp>()
        .with::<intrinsic::Json>()
        .with::<intrinsic::Proxy>()
        .with::<intrinsic::MapSet>()
        // Async functions need promises; the prelude hides the constructor
        // and rejects a returned promise, and pending jobs never run.
        .with::<intrinsic::Promise>()
        .build(runtime)
}

fn options(filename: &str) -> EvalOptions {
    let mut options = EvalOptions::default();
    options.global = true;
    options.strict = true;
    options.backtrace_barrier = true;
    options.filename = Some(filename.to_owned());
    options
}

/// Runs the prelude and installs the scope; returns `out`.
fn setup<'js>(
    ctx: &Ctx<'js>,
    scope: &Scope<'_>,
    tracker: &Rc<RefCell<Tracker>>,
) -> Result<Function<'js>, ExprError> {
    let fail = |e: rquickjs::Error| thrown(ctx, &e);
    let setup: Function = ctx
        .eval_with_options(PRELUDE, options("prelude"))
        .map_err(fail)?;

    let t = tracker.clone();
    let read = Function::new(ctx.clone(), move |id: u32, key: String| {
        t.borrow_mut().read(id, &key)
    })
    .map_err(fail)?;
    let t = tracker.clone();
    let keys = Function::new(ctx.clone(), move |id: u32| t.borrow_mut().keys(id)).map_err(fail)?;

    let mut roots = serde_json::Map::new();
    let mut t = tracker.borrow_mut();
    for (name, conn) in scope.aliases {
        let id = t.node(name, StatePath::root(conn.clone()));
        roots.insert(name.clone(), id.into());
    }
    for (name, conn) in RESERVED_ROOTS {
        let id = t.node(name, StatePath::root(*conn));
        roots.insert((*name).to_owned(), id.into());
    }
    drop(t);

    let map_json = |m: &BTreeMap<String, Value>| {
        serde_json::Value::Object(m.iter().map(|(k, v)| (k.clone(), to_json(v))).collect())
            .to_string()
    };
    setup
        .call((
            read,
            keys,
            serde_json::Value::Object(roots).to_string(),
            scope.trigger.map(|v| to_json(v).to_string()),
            map_json(scope.local),
            map_json(scope.steps),
        ))
        .map_err(fail)
}

/// Compiles `source` as an expression, or failing that as a function body
/// (flow spec §8.7), into a function that runs it. Nothing runs yet.
fn compile<'js>(ctx: &Ctx<'js>, source: &str) -> Result<Function<'js>, ExprError> {
    // The source starts on line 2 of both wrappers.
    let expression = format!("(function () {{ return (\n{source}\n); }})");
    let body = format!("(function () {{\n{source}\n}})");
    let first = match ctx.eval_with_options::<Function, _>(expression, options(FILE)) {
        Ok(f) => return Ok(f),
        Err(e) => within(thrown(ctx, &e), source),
    };
    if first.code != ExprCode::Syntax {
        return Err(first);
    }
    match ctx.eval_with_options::<Function, _>(body, options(FILE)) {
        Ok(f) => Ok(f),
        // Report whichever form the source looks like it was meant to be.
        Err(e) => {
            let second = within(thrown(ctx, &e), source);
            let body_like =
                source.contains("return") || source.contains(';') || source.contains('\n');
            Err(if body_like { second } else { first })
        }
    }
}

/// Moves a position past the end of `source`, in the wrapper's closing line,
/// to just after its last character: "unexpected end" errors land there.
fn within(mut e: ExprError, source: &str) -> ExprError {
    let lines: Vec<&str> = source.lines().collect();
    if let Some(at) = &mut e.at
        && at.line as usize > lines.len()
    {
        let last = lines.last().copied().unwrap_or("");
        *at = At {
            line: lines.len().max(1) as u32,
            column: last.chars().count() as u32 + 1,
        };
    }
    e
}

/// Turns a failed call into an error, taking the exception from `ctx`.
fn thrown(ctx: &Ctx<'_>, e: &rquickjs::Error) -> ExprError {
    match e {
        rquickjs::Error::Exception => {}
        rquickjs::Error::Allocation => return ExprError::new(ExprCode::Memory, "out of memory"),
        other => return engine_error(other),
    }
    let caught = ctx.catch();
    let Some(exception) = caught.as_exception() else {
        let shown = caught
            .as_string()
            .and_then(|s| s.to_string().ok())
            .unwrap_or_else(|| format!("a {} was thrown", caught.type_name()));
        return ExprError::new(ExprCode::Error, shown);
    };
    let name: String = exception.get("name").unwrap_or_else(|_| "Error".to_owned());
    let message = exception.message().unwrap_or_default();
    if message == "out of memory" {
        return ExprError::new(ExprCode::Memory, message);
    }
    ExprError {
        code: if name == "SyntaxError" && is_compile_error(exception) {
            ExprCode::Syntax
        } else {
            ExprCode::Error
        },
        message: format!("{name}: {message}"),
        at: stack_position(exception),
    }
}

/// A `SyntaxError` from the compiler, not one thrown at run time by
/// `JSON.parse` or `RegExp`: the compiler's has no call frames.
fn is_compile_error(exception: &Exception<'_>) -> bool {
    exception.stack().is_none_or(|stack| {
        !stack
            .lines()
            .any(|l| l.trim_start().starts_with("at ") && l.contains('('))
    })
}

/// The first position in user code, from the stack: `expr:<line>:<column>`,
/// moved back one line for the wrapper.
fn stack_position(exception: &Exception<'_>) -> Option<At> {
    let stack = exception.stack()?;
    let marker = format!("{FILE}:");
    let at = stack.find(&marker)? + marker.len();
    let mut parts = stack[at..].split(|c: char| !c.is_ascii_digit());
    let line: u32 = parts.next()?.parse().ok()?;
    let column: u32 = parts.next()?.parse().ok()?;
    (line >= 2).then(|| At {
        line: line - 1,
        column,
    })
}

fn engine_error(e: &rquickjs::Error) -> ExprError {
    ExprError::new(ExprCode::Error, format!("engine error: {e}"))
}

/// The state nodes handed to JavaScript, and what was read through them.
struct Tracker {
    state: Arc<LiveState>,
    /// By node ID: the root name it was reached from, and its path.
    nodes: Vec<(String, StatePath)>,
    reads: Reads,
}

impl Tracker {
    fn new(state: Arc<LiveState>) -> Self {
        Tracker {
            state,
            nodes: Vec::new(),
            reads: Reads::default(),
        }
    }

    fn node(&mut self, root: &str, path: StatePath) -> u32 {
        self.nodes.push((root.to_owned(), path));
        (self.nodes.len() - 1) as u32
    }

    fn record(&mut self, root: &str, path: &StatePath) {
        self.reads.text.insert(text_path::format(root, &path.path));
        self.reads.paths.insert(path.clone());
    }

    /// `"m"`, `"b<id>"` or `"l<json>"`; see `prelude.js`.
    fn read(&mut self, id: u32, key: &str) -> String {
        let Some((root, parent)) = self.nodes.get(id as usize).cloned() else {
            return "m".to_owned();
        };
        let candidates = segments(key);
        let found = self.state.read(|tree| {
            candidates.iter().find_map(|seg| {
                let mut path = parent.clone();
                path.path.push(seg.clone());
                match tree.probe(&path) {
                    Probe::Leaf(leaf) => Some((path, Some(to_json(&leaf.value).to_string()))),
                    Probe::Branch => Some((path, None)),
                    Probe::Missing => None,
                }
            })
        });
        match found {
            Some((path, Some(json))) => {
                self.record(&root, &path);
                format!("l{json}")
            }
            Some((path, None)) => format!("b{}", self.node(&root, path)),
            None => {
                for seg in candidates {
                    let mut path = parent.clone();
                    path.path.push(seg);
                    self.record(&root, &path);
                }
                "m".to_owned()
            }
        }
    }

    /// The keys below a node, as a JSON array. Reads the whole subtree.
    fn keys(&mut self, id: u32) -> String {
        let Some((root, path)) = self.nodes.get(id as usize).cloned() else {
            return "[]".to_owned();
        };
        self.record(&root, &path);
        let children = self.state.read(|tree| tree.children(&path));
        let keys: Vec<String> = children
            .into_iter()
            .map(|seg| match seg {
                PathSegment::Index(i) => i.to_string(),
                PathSegment::Key(k) => k,
            })
            .collect();
        serde_json::to_string(&keys).expect("strings always serialise")
    }
}

/// The path segments a JavaScript property key may stand for: `"1"` may be
/// the index 1 or the key "1", and the index is tried first.
fn segments(key: &str) -> Vec<PathSegment> {
    let mut out = Vec::with_capacity(2);
    if let Ok(i) = key.parse::<i64>()
        && i.to_string() == key
    {
        out.push(PathSegment::Index(i));
    }
    out.push(PathSegment::Key(key.to_owned()));
    out
}

/// A value going into JavaScript. Binary and extension values have no JSON
/// form and arrive as `null`; so do NaN and the infinities.
fn to_json(value: &Value) -> serde_json::Value {
    use serde_json::Value as J;
    match value {
        Value::Nil | Value::Binary(_) | Value::Ext(..) => J::Null,
        Value::Boolean(b) => J::Bool(*b),
        Value::Integer(i) => match (i.as_i64(), i.as_u64()) {
            (Some(n), _) => n.into(),
            (None, Some(n)) => n.into(),
            (None, None) => J::Null,
        },
        Value::F32(f) => serde_json::Number::from_f64(f64::from(*f)).map_or(J::Null, J::Number),
        Value::F64(f) => serde_json::Number::from_f64(*f).map_or(J::Null, J::Number),
        Value::String(s) => J::String(s.as_str().map_or_else(|| s.to_string(), str::to_owned)),
        Value::Array(items) => J::Array(items.iter().map(to_json).collect()),
        Value::Map(entries) => J::Object(
            entries
                .iter()
                .map(|(k, v)| {
                    let key = match k {
                        Value::String(s) => s.as_str().map_or_else(|| s.to_string(), str::to_owned),
                        other => other.to_string(),
                    };
                    (key, to_json(v))
                })
                .collect(),
        ),
    }
}

/// The largest integer JavaScript represents exactly, 2⁵³ − 1.
const SAFE: i64 = (1 << 53) - 1;

/// A value coming out of JavaScript, from the prelude's JSON: integral
/// numbers within ±2⁵³ become ints, other numbers floats.
fn from_json(json: &str) -> Result<Value, ExprError> {
    let parsed: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| ExprError::new(ExprCode::Error, format!("engine error: {e}")))?;
    Ok(convert(&parsed))
}

fn convert(json: &serde_json::Value) -> Value {
    use serde_json::Value as J;
    match json {
        J::Null => Value::Nil,
        J::Bool(b) => Value::Boolean(*b),
        J::Number(n) => match n.as_i64() {
            Some(i) if (-SAFE..=SAFE).contains(&i) => Value::from(i),
            _ => Value::F64(n.as_f64().unwrap_or(f64::NAN)),
        },
        J::String(s) => Value::from(s.as_str()),
        J::Array(items) => Value::Array(items.iter().map(convert).collect()),
        J::Object(entries) => Value::Map(
            entries
                .iter()
                .map(|(k, v)| (Value::from(k.as_str()), convert(v)))
                .collect(),
        ),
    }
}

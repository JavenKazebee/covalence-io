use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use covalence_protocol::Value;
use covalence_protocol::state::PathSegment;
use tokio::sync::watch;

use super::*;
use crate::state::{Dirty, LiveState, Meta, Quality, Source, StatePath};

const VH: &str = "01VH";

fn map(entries: Vec<(&str, Value)>) -> Value {
    Value::Map(entries.into_iter().map(|(k, v)| (k.into(), v)).collect())
}

fn seg(s: &str) -> PathSegment {
    match s.parse() {
        Ok(i) => PathSegment::Index(i),
        Err(_) => PathSegment::Key(s.to_owned()),
    }
}

fn sp(conn: &str, segs: &[&str]) -> StatePath {
    StatePath::new(conn, segs.iter().map(|s| seg(s)).collect())
}

/// `vh` with outputs 0 and 1, an "SDI 4" output and a list; `vars.n` is 1.
fn state() -> Arc<LiveState> {
    let live = Arc::new(LiveState::new());
    let meta = Meta {
        ts: 1,
        seq: 1,
        quality: Quality::Good,
        source: Source::Core,
    };
    live.mutate(|tree| {
        let mut dirty = Dirty::new();
        let outputs = Value::Map(vec![
            (Value::from(0), map(vec![("source", Value::from(2))])),
            (Value::from(1), map(vec![("source", Value::from(3))])),
            ("SDI 4".into(), map(vec![("source", Value::from(7))])),
        ]);
        dirty.extend(tree.set(&sp(VH, &["outputs"]), &outputs, &meta));
        let names = Value::Array(vec!["a".into(), "b".into()]);
        dirty.extend(tree.set(&sp(VH, &["names"]), &names, &meta));
        dirty.extend(tree.set(&sp("vars", &["n"]), &Value::from(1), &meta));
        ((), dirty)
    });
    live
}

fn aliases() -> BTreeMap<String, String> {
    BTreeMap::from([("vh".to_owned(), VH.to_owned())])
}

fn eval_in(engine: &QuickJs, source: &str, scope: &Scope<'_>) -> Evaluation {
    engine.evaluate(source, scope)
}

fn eval(source: &str) -> Evaluation {
    let live = state();
    let aliases = aliases();
    eval_in(&QuickJs::default(), source, &Scope::new(&live, &aliases))
}

fn value(source: &str) -> Option<Value> {
    match eval(source).value {
        Ok(v) => v,
        Err(e) => panic!("{source}: {e:?}"),
    }
}

fn error(source: &str) -> ExprError {
    match eval(source).value {
        Ok(v) => panic!("{source}: expected an error, got {v:?}"),
        Err(e) => e,
    }
}

#[test]
fn reads_state_through_aliases_and_reserved_roots() {
    assert_eq!(value("vh.outputs[1].source"), Some(Value::from(3)));
    assert_eq!(value(r#"vh.outputs["SDI 4"].source"#), Some(Value::from(7)));
    assert_eq!(
        value("vh.outputs[vars.n].source == 3"),
        Some(Value::from(true))
    );
    assert_eq!(value("vh.names[1]"), Some(Value::from("b")));
    assert_eq!(value("vh.names.length"), Some(Value::from(2)));
}

#[test]
fn missing_values_are_undefined() {
    assert_eq!(value("vh.outputs[9]"), None);
    assert_eq!(value("vh.outputs[9]?.source"), None);
    assert_eq!(value("vars.nothing === undefined"), Some(Value::from(true)));
    assert_eq!(
        value("vh.outputs[9]?.source ?? 'none'"),
        Some(Value::from("none"))
    );
}

#[test]
fn ints_and_floats_compare_as_numbers() {
    assert_eq!(
        value("vh.outputs[1].source == 3.0"),
        Some(Value::from(true))
    );
    assert_eq!(value("6 / 2"), Some(Value::from(3)));
    assert_eq!(value("1 / 4"), Some(Value::F64(0.25)));
    assert_eq!(value("2 ** 60"), Some(Value::F64(2f64.powi(60))));
}

#[test]
fn values_out_follow_the_spec() {
    assert_eq!(value("null"), Some(Value::Nil));
    assert_eq!(value("undefined"), None);
    assert_eq!(
        value("({a: 1, b: undefined, c: [1, undefined]})"),
        Some(Value::Map(vec![
            ("a".into(), Value::from(1)),
            ("c".into(), Value::Array(vec![Value::from(1), Value::Nil])),
        ]))
    );
    assert_eq!(value("new Date(1000)"), Some(Value::from(1000)));
    assert_eq!(
        value("vh.outputs[0]"),
        Some(map(vec![("source", Value::from(2))]))
    );
    for bad in ["NaN", "1 / 0", "() => 1", "Symbol('x')", "2n ** 64n"] {
        assert_eq!(error(bad).code, ExprCode::Error, "{bad}");
    }
    assert_eq!(
        error("const a = []; a.push(a); return a").code,
        ExprCode::Error
    );
}

#[test]
fn bodies_with_statements_work() {
    let source = r#"
        const names = { 2: "Stage", 3: "Lobby" };
        let label = names[vh.outputs[1].source] ?? "Unknown";
        for (const n of vh.names) label += n;
        return label.toUpperCase();
    "#;
    assert_eq!(value(source), Some(Value::from("LOBBYAB")));
    assert_eq!(
        value("if (vars.n) { return 'yes' }"),
        Some(Value::from("yes"))
    );
    assert_eq!(value("const x = 1"), None);
}

#[test]
fn scope_is_read_only() {
    for write in [
        "vars.n = 2",
        "vh.outputs[1].source = 4",
        "delete vh.outputs",
        "Object.defineProperty(vars, 'x', {value: 1})",
        "vh = 1",
        "steps.a = 1",
        "local.x = 1",
        "vh.names.push('c')",
    ] {
        let e = error(write);
        assert_eq!(e.code, ExprCode::Error, "{write}");
    }
    assert!(error("vars.n = 2").message.contains("read-only"));
}

#[test]
fn no_clock_randomness_or_async() {
    for banned in ["Date.now()", "new Date()", "Date()", "Math.random()"] {
        let e = error(banned);
        assert_eq!(e.code, ExprCode::Error, "{banned}");
        assert!(
            e.message.contains("isn't available"),
            "{banned}: {}",
            e.message
        );
    }
    assert_eq!(
        value("new Date(0).toISOString()"),
        Some(Value::from("1970-01-01T00:00:00.000Z"))
    );
    for missing in ["Promise", "performance", "setTimeout", "WeakRef"] {
        assert_eq!(
            value(&format!("typeof {missing}")),
            Some(Value::from("undefined")),
            "{missing}"
        );
    }
    assert_eq!(error("(async () => 1)()").code, ExprCode::Error);
    assert_eq!(error("await 1").code, ExprCode::Syntax);
}

#[test]
fn nothing_carries_between_evaluations() {
    let live = state();
    let aliases = aliases();
    let engine = QuickJs::default();
    let scope = Scope::new(&live, &aliases);
    let first = eval_in(
        &engine,
        "globalThis.count = 1; Object.prototype.x = 1; return 1",
        &scope,
    );
    assert!(first.value.is_ok());
    let second = eval_in(&engine, "[typeof count, typeof ({}).x]", &scope);
    assert_eq!(
        second.value,
        Ok(Some(Value::Array(vec![
            "undefined".into(),
            "undefined".into()
        ])))
    );
}

#[test]
fn trigger_local_and_steps_are_in_scope() {
    let live = state();
    let aliases = aliases();
    let trigger = map(vec![("payload", map(vec![("output", Value::from(1))]))]);
    let local = BTreeMap::from([("cam".to_owned(), Value::from("A"))]);
    let steps = BTreeMap::from([(
        "route1".to_owned(),
        map(vec![
            ("status", "done".into()),
            ("result", map(vec![("ok", true.into())])),
        ]),
    )]);
    let scope = Scope {
        trigger: Some(&trigger),
        local: &local,
        steps: &steps,
        ..Scope::new(&live, &aliases)
    };
    let e = QuickJs::default();
    let v = |s: &str| e.evaluate(s, &scope).value.unwrap();
    assert_eq!(v("trigger.payload.output"), Some(Value::from(1)));
    assert_eq!(
        v("local.cam + steps.route1.result.ok"),
        Some(Value::from("Atrue"))
    );
    assert_eq!(v("steps.route9?.result"), None);
    // Without a trigger it is undefined.
    assert_eq!(value("trigger"), None);
}

#[test]
fn records_what_it_reads() {
    let reads = eval("vh.outputs[vars.n].source + (vars.missing ?? 0)").reads;
    assert_eq!(
        reads.text,
        BTreeSet::from([
            "vars.n".to_owned(),
            "vars.missing".to_owned(),
            "vh.outputs[1].source".to_owned(),
        ])
    );
    let changed = |conn, segs: &[&str]| Dirty::from([sp(conn, segs)]);
    assert!(reads.touched_by(&changed(VH, &["outputs", "1", "source"])));
    assert!(reads.touched_by(&changed("vars", &["n"])));
    assert!(reads.touched_by(&changed("vars", &["missing", "deep"])));
    assert!(!reads.touched_by(&changed(VH, &["outputs", "0", "source"])));

    // Listing keys reads the whole node.
    let reads = eval("Object.keys(vh.outputs).length").reads;
    assert!(reads.touched_by(&changed(VH, &["outputs", "7", "source"])));
    assert_eq!(
        value("Object.keys(vh.outputs)"),
        Some(Value::Array(vec!["0".into(), "1".into(), "SDI 4".into()]))
    );
    assert_eq!(
        value("'SDI 4' in vh.outputs && !('9' in vh.outputs)"),
        Some(Value::from(true))
    );
}

#[test]
fn errors_point_into_the_source() {
    let e = error("1 +");
    assert_eq!(e.code, ExprCode::Syntax);
    assert_eq!(e.at.map(|a| a.line), Some(1), "{e:?}");

    let e = error("const a = 1;\nreturn a.b.c");
    assert_eq!(e.code, ExprCode::Error);
    assert!(e.message.starts_with("TypeError"), "{}", e.message);
    assert_eq!(e.at.map(|a| a.line), Some(2), "{e:?}");

    // A SyntaxError thrown while running is an ordinary error.
    assert_eq!(error("JSON.parse('{')").code, ExprCode::Error);
    assert_eq!(error("throw 'boom'").message, "boom");

    let engine = QuickJs::default();
    assert_eq!(engine.check("vh.outputs[1].source == 3"), Ok(()));
    assert_eq!(
        engine.check("const x = ;").map_err(|e| e.code),
        Err(ExprCode::Syntax)
    );
}

#[test]
fn runaway_code_is_stopped() {
    let started = Instant::now();
    let e = error("while (true) {}");
    assert_eq!(e.code, ExprCode::Timeout);
    assert!(started.elapsed() < Duration::from_millis(500));
    // Uncatchable.
    assert_eq!(
        error("try { while (true) {} } catch (e) { return 1 }").code,
        ExprCode::Timeout
    );

    assert_eq!(
        error("const a = []; while (true) a.push('x'.repeat(1 << 20) + a.length)").code,
        ExprCode::Memory
    );
    assert_eq!(error("const f = () => f(); f()").code, ExprCode::Error);
    // The engine is still usable afterwards.
    assert_eq!(value("1 + 1"), Some(Value::from(2)));
}

#[test]
fn memory_errors_leave_the_engine_usable() {
    let live = state();
    let aliases = aliases();
    let engine = QuickJs::default();
    let scope = Scope::new(&live, &aliases);
    let e = engine.evaluate("'x'.repeat(64 * 1024 * 1024)", &scope);
    assert_eq!(e.value.map_err(|e| e.code), Err(ExprCode::Memory));
    assert_eq!(
        engine.evaluate("2 + 2", &scope).value,
        Ok(Some(Value::from(4)))
    );
}

#[test]
fn cancellation_stops_an_evaluation() {
    let live = state();
    let aliases = aliases();
    let (tx, rx) = watch::channel(false);
    let scope = Scope {
        cancel: Some(&rx),
        ..Scope::new(&live, &aliases)
    };
    let engine = QuickJs::new(Limits {
        time: Duration::from_secs(10),
        ..Limits::default()
    });
    let started = Instant::now();
    std::thread::scope(|s| {
        s.spawn(|| {
            std::thread::sleep(Duration::from_millis(20));
            tx.send_replace(true);
        });
        let e = engine.evaluate("while (true) {}", &scope);
        assert_eq!(e.value.map_err(|e| e.code), Err(ExprCode::Cancelled));
    });
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn standard_library() {
    assert_eq!(value("mmss(125)"), Some(Value::from("02:05")));
    assert_eq!(value("mmss(-5.7)"), Some(Value::from("-00:05")));
    assert_eq!(value("hhmmss(3725)"), Some(Value::from("01:02:05")));
    assert_eq!(value("clamp(12, 0, 10)"), Some(Value::from(10)));
    assert_eq!(value("round(2.71828, 2)"), Some(Value::F64(2.72)));
    assert_eq!(value("pad(7, 3)"), Some(Value::from("007")));
    assert_eq!(value("lerp(0, 10, 0.25)"), Some(Value::F64(2.5)));
    assert_eq!(value("rgb(255, 128, 0)"), Some(Value::from("#ff8000")));
    assert_eq!(value("hsl(120, 100, 50)"), Some(Value::from("#00ff00")));
    assert_eq!(
        value("mix('#000000', '#ffffff', 0.5)"),
        Some(Value::from("#808080"))
    );
    assert_eq!(
        value("mix('#000', '#fff', 1)"),
        Some(Value::from("#ffffff"))
    );
}

#[test]
fn names_aliases_cannot_take() {
    for taken in [
        "var",
        "if",
        "Math",
        "JSON",
        "clamp",
        "mmss",
        "trigger",
        "steps",
        "local",
        "vars",
        "sys",
        "flow",
        "surface",
        "layer",
        "undefined",
    ] {
        assert!(is_taken_name(taken), "{taken}");
    }
    for free in ["vh", "atem", "cam1", "Stage"] {
        assert!(!is_taken_name(free), "{free}");
    }
}

#[test]
fn aliases_cannot_hide_globals() {
    let live = state();
    let aliases = BTreeMap::from([
        ("Math".to_owned(), VH.to_owned()),
        ("clamp".to_owned(), VH.to_owned()),
    ]);
    let e = QuickJs::default();
    let scope = Scope::new(&live, &aliases);
    assert_eq!(
        e.evaluate("Math.max(1, 2)", &scope).value,
        Ok(Some(Value::from(2)))
    );
    assert_eq!(
        e.evaluate("clamp(5, 0, 1)", &scope).value,
        Ok(Some(Value::from(1)))
    );
}

/// The cost of one evaluation, against the 16 ms re-render budget
/// (architecture). Run with
/// `cargo test --release -p covalence-core --lib expr::tests::bench -- --ignored --nocapture`.
#[test]
#[ignore]
fn bench() {
    let live = state();
    let aliases = aliases();
    let engine = QuickJs::default();
    let scope = Scope::new(&live, &aliases);
    let n = 2000;
    for (label, source) in [
        ("constant", "1"),
        ("one state read", "vh.outputs[1].source == 3"),
        ("dynamic read", "vh.outputs[vars.n].source"),
        (
            "body",
            "const m = {3: 'Lobby'}; return m[vh.outputs[1].source] ?? 'none'",
        ),
    ] {
        let started = Instant::now();
        for _ in 0..n {
            assert!(engine.evaluate(source, &scope).value.is_ok());
        }
        let each = started.elapsed() / n;
        eprintln!("{label:>16}: {each:?} per evaluation");
    }
}

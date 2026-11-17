//! Transform definitions (dataflow spec §4.4): every built-in's conformance
//! suite, run through its definition and its native implementation.

use std::time::{Duration, Instant};

use covalence_core::source::Source;
use covalence_core::transform::Transform;
use covalence_core::transform::conformance::{Suite, run_suite};
use covalence_core::transform::definition::{
    BUILTIN, Kind, Sandbox, StepCtx, StepEvent, builtin, builtins,
};
use covalence_core::transform::reducer::native;
use serde_json::{Map, Value as Json, json};

fn suites() -> Vec<(String, Suite)> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/transforms/conformance");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap();
        let suite: Suite =
            serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        out.push((path.display().to_string(), suite));
    }
    out
}

#[test]
fn every_builtin_passes_its_conformance_suite() {
    let sandbox = Sandbox::new();
    let suites = suites();
    for (id, _) in BUILTIN {
        assert!(
            suites.iter().any(|(_, s)| s.transform == *id),
            "{id} has no conformance suite"
        );
    }
    for (path, suite) in &suites {
        run_suite(&sandbox, suite).unwrap_or_else(|e| panic!("{path}: {e}"));
    }
}

#[test]
fn every_reducer_but_script_is_native() {
    for d in builtins().values().filter(|d| d.kind == Kind::Reducer) {
        assert_eq!(native(&d.id).is_some(), d.id != "script", "{}", d.id);
    }
}

#[test]
fn a_reducer_is_a_source_with_options_that_are_sources() {
    let s: Source<Json> = Source::from_json(json!({
        "use": "position@1",
        "input": { "ref": { "root": "surface", "path": ["deck", "controls", "e1", "detents"] } },
        "max": { "ref": { "root": "vars", "path": ["top"] } },
        "step": 2,
        "fallback": 0
    }))
    .unwrap();
    let Source::Use {
        transform,
        fallback,
    } = &s
    else {
        panic!("{s:?}")
    };
    assert_eq!(fallback, &Some(json!(0)));
    let Transform::Reducer(r) = &**transform else {
        panic!("{transform:?}")
    };
    assert_eq!((r.id.as_str(), r.version), ("position", Some(1)));
    assert!(r.input.is_some());
    assert_eq!(r.options.len(), 2);
    // It round-trips.
    assert_eq!(Source::from_json(s.to_json().unwrap()).unwrap(), s);

    let toggle = json!({
        "use": "toggle",
        "on": { "from": { "path": { "match": "surface.*.controls.k3" } }, "event": "press" }
    });
    let s: Source<Json> = Source::from_json(toggle.clone()).unwrap();
    assert_eq!(s.to_json().unwrap(), toggle);

    for bad in [
        json!({ "use": "toggle", "intial": true }),
        json!({ "use": "toggle@2" }),
        json!({ "use": "toggle@x" }),
        json!({ "use": "counter", "on": { "form": {} } }),
        json!({ "use": "not@2", "input": true }),
    ] {
        assert!(
            Source::<Json>::from_json(bad.clone()).is_err(),
            "{bad} parsed"
        );
    }
    // A stateless transform may name its version too.
    assert!(Source::<Json>::from_json(json!({ "use": "not@1", "input": true })).is_ok());
}

/// The benchmark that decides which reducers are native (M8 Phase 1): a
/// step through the sandbox against a native step. Printed, not asserted:
/// the turn's budget is checked end to end in M8's acceptance.
#[test]
fn native_steps_are_much_cheaper_than_sandboxed_ones() {
    let sandbox = Sandbox::new();
    let d = builtin("position").unwrap();
    let n = native("position").unwrap();
    let ctx = |i: i64| StepCtx {
        event: Some(StepEvent {
            name: "turn".into(),
            payload: None,
        }),
        input: Some(json!(i)),
        previous: Some(json!(i - 1)),
        options: d.with_defaults(&Map::new()),
        ..StepCtx::default()
    };
    let runs = 50;
    let start = Instant::now();
    for i in 0..runs {
        sandbox.step(d, Some(&json!(10)), &ctx(i)).unwrap();
    }
    let sandboxed = start.elapsed() / runs as u32;
    let start = Instant::now();
    for i in 0..runs {
        n.step(Some(&json!(10)), &ctx(i));
    }
    let natively = start.elapsed() / runs as u32;
    println!("position step: {sandboxed:?} sandboxed, {natively:?} native");
    assert!(natively < sandboxed);
    assert!(sandboxed < Duration::from_millis(50), "{sandboxed:?}");
}

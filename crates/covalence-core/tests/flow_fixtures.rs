//! The shared flow fixtures (`fixtures/flows/`, overview spec: project entities): valid flows parse,
//! validate cleanly and round-trip; the others report exactly the expected
//! problems.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use covalence_core::expr::QuickJs;
use covalence_core::flow::{self, Context, Flow};
use covalence_protocol::plugin::Manifest;
use serde_json::{Value, json};

mod common;

const VH: &str = "01JBZ7P0D3F5G7H9J1K3L5M7N9";
const VH2: &str = "01JBZ7P0D3F5G7H9J1K3L5M7NA";
const UNLOADED: &str = "01JBZ7P0D3F5G7H9J1K3L5M7NB";
const DECK_LAYER: &str = "01JBZ7P0D3F5G7H9J1K3L5M7PA";
const RECALL: &str = "01JC00000000000000000000CA";
const NOT_CALLABLE: &str = "01JC00000000000000000000CB";

/// A flow the fixtures call: a `call` trigger, a required `camera` and a
/// `speed` with a default, and a `preset` output.
fn recall_flow() -> Flow {
    Flow::parse(&json!({
        "id": RECALL,
        "name": "Recall",
        "slug": "recall",
        "enabled": true,
        "triggers": [{ "kind": "call" }],
        "concurrency": "parallel",
        "inputs": {
            "camera": { "type": "integer", "minimum": 1, "required": true },
            "speed": { "type": "number", "default": 1 },
        },
        "outputs": { "preset": { "type": "integer" } },
        "body": [],
    }))
    .unwrap()
}

/// The parts of the Videohub manifest that flows use.
fn videohub() -> Manifest {
    serde_json::from_value(json!({
        "id": "io.covalence.videohub",
        "name": "Blackmagic Videohub",
        "version": "0.1.0",
        "protocol": "^1",
        "sdk": "0.1.0",
        "runtime": "deno",
        "license": "MIT",
        "authors": [],
        "categories": [],
        "config": { "schema": { "type": "object" }, "version": 1 },
        "permissions": {},
        "connections": "multiple",
        "actions": [{
            "id": "route",
            "name": "Set route",
            "params": {
                "type": "object",
                "properties": {
                    "output": { "type": "integer", "minimum": 0 },
                    "input": { "type": "integer", "minimum": 0 },
                },
                "required": ["output", "input"],
                "additionalProperties": false,
            },
        }],
        "events": [{ "id": "routeChanged", "name": "Route changed" }],
        "state": {},
    }))
    .unwrap()
}

fn fixtures(dir: &str) -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/flows")
        .join(dir);
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no fixtures in {}", dir.display());
    files
}

fn read(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// `{path, code}` of each problem, as the fixtures list them.
fn summary(problems: &[flow::Problem]) -> Value {
    problems
        .iter()
        .map(|p| json!({ "path": p.path, "code": p.code }))
        .collect()
}

fn check(json: &Value) -> (Value, Value) {
    let manifest = videohub();
    let conns: BTreeMap<String, Option<&Manifest>> = [
        (VH.to_owned(), Some(&manifest)),
        (VH2.to_owned(), Some(&manifest)),
        (UNLOADED.to_owned(), None),
    ]
    .into();
    let taken: BTreeSet<String> = ["taken".to_owned()].into();
    let layers = [(
        DECK_LAYER.to_owned(),
        common::deck_layer(DECK_LAYER, "deck", 5, 3),
    )]
    .into();
    let evaluator = QuickJs::default();
    let mut not_callable = recall_flow();
    not_callable.triggers = vec![flow::Trigger::Api {}];
    let flows: BTreeMap<String, Flow> = [
        (RECALL.to_owned(), recall_flow()),
        (NOT_CALLABLE.to_owned(), not_callable),
    ]
    .into();
    // `lastPick` is writable and `isLive` computed (dataflow §6).
    let variables: BTreeMap<String, bool> =
        [("lastPick".to_owned(), false), ("isLive".to_owned(), true)].into();
    let ctx = Context {
        conns: &conns,
        taken_slugs: &taken,
        layers: &layers,
        surfaces: &BTreeSet::new(),
        flows: &flows,
        evaluator: &evaluator,
        variables: &variables,
    };
    match Flow::parse(json) {
        Err(errors) => (summary(&errors), json!([])),
        Ok(flow) => {
            let report = flow::validate(&flow, &ctx);
            (summary(&report.errors), summary(&report.warnings))
        }
    }
}

#[test]
fn valid_flows_validate_cleanly_and_round_trip() {
    for path in fixtures("") {
        let json = read(&path);
        let name = path.display();
        let flow = Flow::parse(&json).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_eq!(
            serde_json::to_value(&flow).unwrap(),
            json,
            "{name} round-trip"
        );
        assert_eq!(check(&json), (json!([]), json!([])), "{name}");
    }
}

#[test]
fn problem_flows_report_what_they_expect() {
    for path in fixtures("problems") {
        let fixture = read(&path);
        let (errors, warnings) = check(&fixture["flow"]);
        let name = path.display();
        assert_eq!(errors, fixture["errors"], "{name} errors");
        assert_eq!(warnings, fixture["warnings"], "{name} warnings");
    }
}

#[test]
fn problems_say_what_is_wrong() {
    let flow = Flow::parse(&json!({
        "name": "Messages",
        "slug": "messages",
        "enabled": true,
        "triggers": [{ "kind": "api" }],
        "concurrency": "parallel",
        "body": [{
            "id": "route1",
            "kind": "action",
            "target": { "conn": VH },
            "action": "route",
            "params": { "output": -1, "input": 0 },
        }],
    }))
    .unwrap();
    let manifest = videohub();
    let conns: BTreeMap<String, Option<&Manifest>> = [(VH.to_owned(), Some(&manifest))].into();
    let report = flow::validate(
        &flow,
        &Context {
            conns: &conns,
            taken_slugs: &BTreeSet::new(),
            layers: &BTreeMap::new(),
            surfaces: &BTreeSet::new(),
            flows: &BTreeMap::new(),
            evaluator: &QuickJs::default(),
            variables: &BTreeMap::new(),
        },
    );
    let messages: Vec<&str> = report.errors.iter().map(|p| p.message.as_str()).collect();
    assert_eq!(messages, ["output must be at least 0"]);

    let errors = Flow::parse(&json!({
        "name": "x", "enabled": true, "concurrency": "parallel",
        "body": [{ "id": "w", "kind": "wait", "for": "soon" }],
    }))
    .unwrap_err();
    assert!(errors[0].message.contains("not a duration"), "{errors:?}");
}

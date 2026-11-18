//! Selectors (dataflow spec §2): each domain's forms, the
//! combinators, stable order, `expr` that follows state, and validation.

use std::collections::BTreeMap;
use std::sync::Arc;

use covalence_core::expr::{QuickJs, Scope};
use covalence_core::select::{
    ControlView, Controls, Issue, Paths, Selector, Target, TargetView, Targets,
};
use covalence_core::source::Eval;
use covalence_core::state::{self, LiveState, Meta, Quality, StatePath};
use covalence_core::surface::{InputCapKind, OutputCapKind, Placement};
use covalence_protocol::Value;
use covalence_protocol::state::PathSegment;
use serde_json::{Value as Json, json};

fn key(k: &str) -> PathSegment {
    PathSegment::Key(k.into())
}

fn set(state: &LiveState, conn: &str, path: Vec<PathSegment>, v: Value) {
    let at = StatePath::new(conn, path);
    let meta = Meta {
        ts: 0,
        seq: 1,
        quality: Quality::Good,
        source: state::Source::Core,
    };
    state.mutate(|tree| ((), tree.set(&at, &v, &meta)));
}

fn controls(json: Json) -> Selector<Controls> {
    Selector::from_json(json).unwrap()
}

/// A 5×3 deck of keys, `k0`–`k14`, and two encoders below it.
fn deck() -> Vec<ControlView> {
    let mut out: Vec<ControlView> = (0..15)
        .map(|i| ControlView {
            id: format!("k{i}"),
            at: Some(Placement::at(i % 5, i / 5)),
            inputs: vec![InputCapKind::Momentary],
            outputs: vec![OutputCapKind::Image],
        })
        .collect();
    // Out of order, to show the result is ordered by position.
    for (id, x) in [("e1", 1), ("e0", 0)] {
        out.push(ControlView {
            id: id.into(),
            at: Some(Placement::at(x, 3)),
            inputs: vec![InputCapKind::Relative, InputCapKind::Momentary],
            outputs: vec![OutputCapKind::Level],
        });
    }
    out
}

fn picked(s: &Selector<Controls>) -> Vec<String> {
    s.resolve(&deck(), None).unwrap()
}

#[test]
fn control_forms_and_combinators() {
    let cases: Vec<(Json, Vec<&str>)> = vec![
        (json!("k3"), vec!["k3"]),
        (json!("k99"), vec![]),
        (json!(["k7", "k1"]), vec!["k1", "k7"]),
        (
            json!({ "region": { "x": 1, "y": 1, "w": 2, "h": 2 } }),
            vec!["k6", "k7", "k11", "k12"],
        ),
        (json!({ "near": "right", "of": "k6" }), vec!["k7"]),
        (json!({ "near": "above", "of": "k6" }), vec!["k1"]),
        (json!({ "near": "left", "of": "k0" }), vec![]),
        (json!({ "near": "below", "of": "k11" }), vec!["e1"]),
        (json!({ "near": "below", "of": "k12" }), vec![]),
        (
            json!({ "has": { "inputs": ["relative"] } }),
            vec!["e0", "e1"],
        ),
        (
            json!({ "has": { "inputs": ["momentary"], "outputs": ["level"] } }),
            vec!["e0", "e1"],
        ),
        (
            json!({ "all": [{ "region": { "x": 1, "y": 1, "w": 2, "h": 2 } }, { "not": "k6" }] }),
            vec!["k7", "k11", "k12"],
        ),
        (
            json!({ "any": ["k14", { "has": { "outputs": ["level"] } }] }),
            vec!["k14", "e0", "e1"],
        ),
        (
            json!({ "not": { "any": [{ "region": { "x": 0, "y": 0, "w": 5, "h": 2 } }, { "has": { "inputs": ["relative"] } }] } }),
            vec!["k10", "k11", "k12", "k13", "k14"],
        ),
    ];
    for (json, want) in cases {
        let s = controls(json.clone());
        assert_eq!(picked(&s), want, "{json}");
        assert_eq!(s.to_json().unwrap(), json, "{json} round-trips");
    }
    assert_eq!(
        picked(&controls(json!({ "all": [] }))).len(),
        deck().len(),
        "all of nothing is everything"
    );
}

#[test]
fn controls_without_a_position_come_last_and_skip_regions() {
    let mut controls_ = deck();
    controls_.insert(
        0,
        ControlView {
            id: "free".into(),
            at: None,
            inputs: vec![],
            outputs: vec![],
        },
    );
    let all = controls(json!({ "not": [] }))
        .resolve(&controls_, None)
        .unwrap();
    assert_eq!(all.first().map(String::as_str), Some("k0"));
    assert_eq!(all.last().map(String::as_str), Some("free"));
    let region = controls(json!({ "region": { "x": 0, "y": 0, "w": 99, "h": 99 } }))
        .resolve(&controls_, None)
        .unwrap();
    assert!(!region.contains(&"free".to_owned()));
}

#[test]
fn an_expr_follows_state() {
    let state = Arc::new(LiveState::new());
    let aliases = BTreeMap::new();
    let quickjs = QuickJs::default();
    let s = controls(json!({ "any": ["k14", { "expr": "vars.picked" }] }));
    let resolve = || {
        let scope = Scope::new(&state, &aliases);
        let mut eval = Eval::new(&scope, &quickjs);
        let ids = s.resolve(&deck(), Some(&mut eval)).unwrap();
        (ids, eval.reads)
    };
    let (ids, reads) = resolve();
    assert_eq!(ids, ["k14"], "undefined picks nothing");
    assert!(reads.contains(&StatePath::new("vars", vec![key("picked")])));

    set(
        &state,
        "vars",
        vec![key("picked")],
        Value::Array(vec!["k2".into(), "k0".into(), "nope".into()]),
    );
    assert_eq!(resolve().0, ["k0", "k2", "k14"]);
    set(&state, "vars", vec![key("picked")], "e1".into());
    assert_eq!(resolve().0, ["k14", "e1"]);
}

#[test]
fn control_problems() {
    let known = |id: &str| deck().iter().any(|c| c.id == id);
    let s = controls(json!({
        "any": ["k0", "k99", ["k1"], { "near": "left", "of": "x" }, { "region": { "x": 0, "y": 0, "w": 0, "h": 1 } }, { "expr": "'k77'" }]
    }));
    let problems: Vec<(Json, bool)> = s
        .problems(&known)
        .into_iter()
        .map(|(p, i)| {
            (
                serde_json::to_value(p).unwrap(),
                matches!(i, Issue::Unknown(_)),
            )
        })
        .collect();
    assert_eq!(
        problems,
        [
            (json!(["any", 1]), true),
            (json!(["any", 3, "of"]), true),
            (json!(["any", 4]), false),
        ]
    );
}

#[test]
fn misshapen_selectors_are_errors() {
    for bad in [
        json!({ "expr": "x", "any": [] }),
        json!({ "not": "k0", "extra": 1 }),
        json!({ "any": "k0" }),
        json!({ "region": { "x": 0, "y": 0, "w": 1 } }),
        json!({ "near": "up", "of": "k0" }),
        json!({ "region": { "x": 0, "y": 0, "w": 1, "h": 1 }, "extra": 1 }),
        json!({ "has": { "inputs": ["relative"] }, "near": "left" }),
        json!([{ "region": { "x": 0, "y": 0, "w": 1, "h": 1 } }]),
    ] {
        assert!(
            Selector::<Controls>::from_json(bad.clone()).is_err(),
            "{bad} parsed"
        );
    }
}

fn conns() -> Vec<TargetView> {
    vec![
        TargetView {
            conn: "C".into(),
            plugin: "videohub".into(),
        },
        TargetView {
            conn: "A".into(),
            plugin: "videohub".into(),
        },
        TargetView {
            conn: "B".into(),
            plugin: "atem".into(),
        },
    ]
}

#[test]
fn target_forms_by_connection_id() {
    let resolve = |json: Json| -> Vec<String> {
        Selector::<Targets>::from_json(json)
            .unwrap()
            .resolve(&conns(), None)
            .unwrap()
            .into_iter()
            .map(|t| t.conn)
            .collect()
    };
    assert_eq!(resolve(json!("B")), ["B"]);
    assert_eq!(resolve(json!({ "conn": "B" })), ["B"]);
    assert_eq!(resolve(json!({ "plugin": "videohub" })), ["A", "C"]);
    assert_eq!(resolve(json!({ "not": { "plugin": "videohub" } })), ["B"]);
    // A target is a connection; what's within it is a parameter (plugin protocol §13.9).
    assert!(Selector::<Targets>::from_json(json!({ "conn": "A", "key": "out1" })).is_err());

    let s = Selector::<Targets>::from_json(json!(["A", "Y", "Z"])).unwrap();
    let known = |c: &String| conns().iter().any(|v| &v.conn == c);
    let issues: Vec<Json> = s
        .problems(&known)
        .into_iter()
        .map(|(p, _)| serde_json::to_value(p).unwrap())
        .collect();
    assert_eq!(issues, [json!([1]), json!([2])]);
    assert_eq!(serde_json::to_value(Target::conn("A")).unwrap(), json!("A"));
}

fn paths(json: Json) -> Selector<Paths> {
    Selector::from_json(json).unwrap()
}

fn pressed(surface: &str, control: &str) -> StatePath {
    StatePath::new(
        "surface",
        vec![key(surface), key("controls"), key(control), key("pressed")],
    )
}

#[test]
fn path_patterns_capture_their_wildcards() {
    let aliases: BTreeMap<String, String> = [("vh".to_owned(), "01VH".to_owned())].into();
    let any_key = paths(json!({ "match": "surface.*.controls.*.pressed" }));
    let check = |s: &Selector<Paths>, p: &StatePath| s.matches(p, &aliases, None).unwrap();
    assert_eq!(
        check(&any_key, &pressed("deck", "k0")),
        Some(vec!["deck".into(), "k0".into()])
    );
    assert_eq!(
        check(
            &any_key,
            &StatePath::new("surface", vec![key("deck"), key("controls"), key("k0")])
        ),
        None,
        "a shorter path doesn't match"
    );

    let route = paths(json!({ "match": "vh.outputs[*].source" }));
    let out = StatePath::new(
        "01VH",
        vec![key("outputs"), PathSegment::Index(3), key("source")],
    );
    assert_eq!(check(&route, &out), Some(vec!["3".into()]));
    assert_eq!(
        check(&route, &StatePath::new("01XX", out.path.clone())),
        None
    );

    let any_root = paths(json!({ "match": "*.outputs[*].source" }));
    assert_eq!(
        check(&any_root, &out),
        Some(vec!["01VH".into(), "3".into()])
    );

    // Every key but one.
    let but_k0 = paths(json!({
        "all": [
            { "match": "surface.*.controls.*.pressed" },
            { "not": { "root": "surface", "path": ["deck", "controls", "k0", "pressed"] } }
        ]
    }));
    assert_eq!(check(&but_k0, &pressed("deck", "k0")), None);
    assert_eq!(
        check(&but_k0, &pressed("deck", "k1")),
        Some(vec!["deck".into(), "k1".into()])
    );
    assert_eq!(but_k0.patterns().map(|p| p.len()), Some(1));
}

#[test]
fn a_path_expr_gives_text_paths() {
    let state = Arc::new(LiveState::new());
    let aliases: BTreeMap<String, String> = [("vh".to_owned(), "01VH".to_owned())].into();
    let quickjs = QuickJs::default();
    let s = paths(json!({ "expr": "['vh.level', 'vars.x']" }));
    let scope = Scope::new(&state, &aliases);
    let mut eval = Eval::new(&scope, &quickjs);
    let level = StatePath::new("01VH", vec![key("level")]);
    assert_eq!(
        s.matches(&level, &aliases, Some(&mut eval)).unwrap(),
        Some(vec![])
    );
    let other = StatePath::new("01VH", vec![key("name")]);
    assert_eq!(s.matches(&other, &aliases, Some(&mut eval)).unwrap(), None);
    assert_eq!(s.patterns(), None, "an expr can't be indexed");
}

#[test]
fn path_problems() {
    let known = |c: &String| c == "01VH";
    let s = paths(json!({
        "any": [
            { "conn": "01XX", "path": ["a"] },
            { "match": "a.b*" },
            { "not": { "match": "vars.*" } }
        ]
    }));
    let issues: Vec<Json> = s
        .problems(&known)
        .into_iter()
        .map(|(p, _)| serde_json::to_value(p).unwrap())
        .collect();
    assert_eq!(
        issues,
        [
            json!(["any", 0, "conn"]),
            json!(["any", 1, "match"]),
            json!(["any", 2])
        ]
    );
}

//! Bindings (M5 Phase 2): the shared fixtures (`fixtures/bindings/`, ADR
//! 0020), and the rule from ADR 0023 that a structured binding evaluated
//! natively gives what its compiled expression gives on QuickJS.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use covalence_core::binding::{
    self, Binding, Condition, Context, ControlBinding, Curve, Format, MapTo, Op, Part, Ref, Root,
    Rule,
};
use covalence_core::expr::{Evaluator, QuickJs, Scope, to_json};
use covalence_core::flow::Problem;
use covalence_core::state::{LiveState, Meta, Quality, Source, StatePath};
use covalence_core::surface::Layout;
use covalence_protocol::Value;
use covalence_protocol::state::PathSegment;
use serde_json::{Value as Json, json};

const VH: &str = "01JBZ7P0D3F5G7H9J1K3L5M7N9";
const PAGE: &str = "01JBZ7P0D3F5G7H9J1K3L5M7PA";

fn fixtures(dir: &str) -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/bindings")
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

fn read(path: &Path) -> Json {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn summary(problems: &[Problem]) -> Json {
    problems
        .iter()
        .map(|p| json!({ "path": p.path, "code": p.code }))
        .collect()
}

fn check(json: &Json) -> Json {
    let pages = [(PAGE.to_owned(), Layout { cols: 5, rows: 3 })].into();
    let conns = [VH.to_owned()].into();
    let taken = [(PAGE.to_owned(), "k14".to_owned())].into();
    let evaluator = QuickJs::default();
    let ctx = Context {
        pages: &pages,
        conns: &conns,
        taken: &taken,
        evaluator: &evaluator,
    };
    match ControlBinding::parse(json) {
        Err(errors) => summary(&errors),
        Ok(b) => summary(&binding::validate(&b, &ctx).errors),
    }
}

#[test]
fn valid_bindings_validate_cleanly_and_round_trip() {
    for path in fixtures("") {
        let json = read(&path);
        let name = path.display();
        let b = ControlBinding::parse(&json).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_eq!(serde_json::to_value(&b).unwrap(), json, "{name} round-trip");
        assert_eq!(check(&json), json!([]), "{name}");
    }
}

#[test]
fn problem_bindings_report_what_they_expect() {
    for path in fixtures("problems") {
        let fixture = read(&path);
        assert_eq!(
            check(&fixture["binding"]),
            fixture["errors"],
            "{}",
            path.display()
        );
    }
}

/// JSON as a state value.
fn value(json: &Json) -> Value {
    match json {
        Json::Null => Value::Nil,
        Json::Bool(b) => Value::Boolean(*b),
        Json::Number(n) => match n.as_i64() {
            Some(i) => Value::from(i),
            None => Value::F64(n.as_f64().unwrap()),
        },
        Json::String(s) => Value::from(s.as_str()),
        Json::Array(items) => Value::Array(items.iter().map(value).collect()),
        Json::Object(map) => Value::Map(
            map.iter()
                .map(|(k, v)| (Value::from(k.as_str()), value(v)))
                .collect(),
        ),
    }
}

fn segments(path: &[&str]) -> Vec<PathSegment> {
    path.iter()
        .map(|s| match s.parse::<i64>() {
            Ok(i) => PathSegment::Index(i),
            Err(_) => PathSegment::Key((*s).to_owned()),
        })
        .collect()
}

fn set(state: &LiveState, conn: &str, path: &[&str], v: Option<&Json>) {
    let at = StatePath::new(conn, segments(path));
    let meta = Meta {
        ts: 0,
        seq: 1,
        quality: Quality::Good,
        source: Source::Core,
    };
    state.mutate(|tree| {
        let dirty = match v {
            Some(v) => tree.set(&at, &value(v), &meta),
            None => tree.delete(&at),
        };
        ((), dirty)
    });
}

/// The state the fixtures read.
fn fixture_state() -> Arc<LiveState> {
    let state = Arc::new(LiveState::new());
    for (path, v) in [
        ("outputs.1.source", json!(3)),
        ("timers.countdown.remaining", json!(299.7)),
        ("level", json!(0.42)),
    ] {
        set(&state, VH, &path.split('.').collect::<Vec<_>>(), Some(&v));
    }
    set(&state, "vars", &["preview"], Some(&json!(4)));
    set(&state, "vars", &["selectedCam"], Some(&json!(2)));
    state
}

/// A result, the same whichever way it was computed: numbers as `f64`,
/// and `undefined` and failure alike as `None` (an expression's NaN is an
/// error; a native one's is `undefined`).
fn normal(v: Option<Json>) -> Option<Json> {
    v.map(|v| match v {
        Json::Number(n) => json!(n.as_f64().unwrap()),
        other => other,
    })
}

/// Evaluates `b` natively and through its expression; panics if they differ.
fn both_ways(state: &Arc<LiveState>, b: &Binding, what: &str) -> Option<Json> {
    let aliases: BTreeMap<String, String> = [("vh".to_owned(), VH.to_owned())].into();
    let by_id: BTreeMap<String, String> = [(VH.to_owned(), "vh".to_owned())].into();
    let native = b.evaluate(state).expect("a structured kind");
    let source = b.to_expr(&by_id).unwrap();
    let evaluation = QuickJs::default().evaluate(&source, &Scope::new(state, &aliases));
    let native_value = normal(native.value.clone().unwrap());
    let expr_value = normal(
        evaluation
            .value
            .clone()
            .ok()
            .flatten()
            .as_ref()
            .map(to_json),
    );
    let close = match (&native_value, &expr_value) {
        (Some(Json::Number(a)), Some(Json::Number(b))) => {
            let (a, b) = (a.as_f64().unwrap(), b.as_f64().unwrap());
            (a - b).abs() <= 1e-12 * a.abs().max(1.0)
        }
        (a, b) => a == b,
    };
    assert!(
        close,
        "{what}: native {native_value:?}, expression {expr_value:?} ({:?})\n  {source}",
        evaluation.value
    );
    // A native binding reads whole values, so it re-evaluates at least
    // whenever its expression would.
    let expr_reads: BTreeSet<StatePath> = evaluation.reads.paths;
    assert!(
        expr_reads
            .iter()
            .all(|e| native.reads.iter().any(|n| e.is_within(n))),
        "{what}: native read {:?}, expression {expr_reads:?}",
        native.reads
    );
    native_value
}

#[test]
fn fixtures_give_the_same_result_both_ways() {
    let state = fixture_state();
    let mut results = BTreeMap::new();
    for path in fixtures("") {
        let b = ControlBinding::parse(&read(&path)).unwrap();
        for (prop, binding) in &b.props {
            if matches!(binding, Binding::Expr { .. }) {
                continue;
            }
            let what = format!("{} {}", path.display(), prop.as_str());
            results.insert(
                format!("{}.{}", b.control, prop.as_str()),
                both_ways(&state, binding, &what),
            );
        }
    }
    assert_eq!(
        results,
        BTreeMap::from([
            ("k0.background".into(), Some(json!("#e02020"))),
            ("k0.label".into(), Some(json!("Out 1: 3"))),
            ("k1.label".into(), Some(json!("04:59"))),
            ("k1.textColor".into(), Some(json!("#ffffff"))),
            ("k3.background".into(), Some(json!("#000000"))),
            (
                "k3.label".into(),
                Some(json!(-60.0 + 70.0 * (1.0f64 + 9.0 * 0.42).log10()))
            ),
        ])
    );
}

fn var(name: &str) -> Ref {
    Ref::Root {
        root: Root::Vars,
        path: vec![PathSegment::Key(name.into())],
    }
}

/// Values that show where JavaScript's rules are easy to get wrong.
fn samples() -> Vec<Option<Json>> {
    let mut out: Vec<Option<Json>> = [
        json!(0),
        json!(3),
        json!(-1.5),
        json!(2.5),
        json!(0.125),
        json!(1.005),
        json!(0.1),
        json!(299.7),
        json!(-61),
        json!(1e21),
        json!(1.5e-7),
        json!(3725),
        json!(""),
        json!("3"),
        json!(" 12 "),
        json!("0x1F"),
        json!("abc"),
        json!("Straße"),
        json!("10"),
        json!(true),
        json!(false),
        json!(null),
        json!([]),
        json!([1, null, "a"]),
        json!({ "a": 1 }),
    ]
    .into_iter()
    .map(Some)
    .collect();
    out.push(None);
    out
}

#[test]
fn every_format_matches_its_expression() {
    let state = Arc::new(LiveState::new());
    let formats = [
        None,
        Some(Format::Auto),
        Some(Format::Mmss),
        Some(Format::Hhmmss),
        Some(Format::Percent),
        Some(Format::Fixed1),
        Some(Format::Fixed2),
        Some(Format::Upper),
    ];
    for sample in samples() {
        set(&state, "vars", &["v"], sample.as_ref());
        for format in formats {
            let b = Binding::Value {
                path: var("v"),
                format,
                fallback: None,
            };
            both_ways(&state, &b, &format!("{sample:?} as {format:?}"));
            let t = Binding::Template {
                parts: vec![
                    Part::Text("a `${b}` \\ ".into()),
                    Part::Value {
                        path: var("v"),
                        format,
                    },
                ],
                fallback: None,
            };
            both_ways(
                &state,
                &t,
                &format!("{sample:?} in a template as {format:?}"),
            );
        }
    }
}

#[test]
fn every_condition_matches_its_expression() {
    let state = Arc::new(LiveState::new());
    let ops = [
        Op::Eq,
        Op::Ne,
        Op::Gt,
        Op::Ge,
        Op::Lt,
        Op::Le,
        Op::Truthy,
        Op::Falsy,
    ];
    let against = [
        None,
        Some(json!(3)),
        Some(json!("3")),
        Some(json!("abc")),
        Some(json!(null)),
        Some(json!(0)),
        Some(json!(true)),
        Some(json!("10")),
    ];
    for sample in samples() {
        set(&state, "vars", &["v"], sample.as_ref());
        for op in ops {
            for value in &against {
                let b = Binding::Rules {
                    rules: vec![Rule {
                        when: Condition {
                            path: var("v"),
                            op,
                            value: value.clone(),
                        },
                        value: json!("#ffffff"),
                    }],
                    otherwise: json!("#000000"),
                };
                both_ways(&state, &b, &format!("{sample:?} {op:?} {value:?}"));
            }
        }
    }
}

#[test]
fn every_map_matches_its_expression() {
    let state = Arc::new(LiveState::new());
    let n = |x: f64| serde_json::Number::from_f64(x).unwrap();
    let tos = [
        MapTo::Range([n(-60.0), n(10.0)]),
        MapTo::List(vec![json!("#00ff00"), json!("#ffff00"), json!("#ff0000")]),
        MapTo::List(vec![json!("#ffffff")]),
    ];
    let mut values: Vec<Option<Json>> = [-20.0, 0.0, 12.5, 33.3, 50.0, 99.999, 100.0, 140.0]
        .into_iter()
        .map(|x| Some(json!(x)))
        .collect();
    values.extend([Some(json!("50")), Some(json!(null)), None]);
    for sample in values {
        set(&state, "vars", &["v"], sample.as_ref());
        for to in &tos {
            for curve in [None, Some(Curve::Log), Some(Curve::Exp)] {
                for clamp in [None, Some(false)] {
                    let b = Binding::Map {
                        path: var("v"),
                        from: [n(0.0), n(100.0)],
                        to: to.clone(),
                        curve,
                        clamp,
                    };
                    both_ways(
                        &state,
                        &b,
                        &format!("{sample:?} {to:?} {curve:?} {clamp:?}"),
                    );
                }
            }
        }
    }
}

#[test]
fn compiled_expressions_read_like_the_spec() {
    let by_id: BTreeMap<String, String> = [(VH.to_owned(), "vh".to_owned())].into();
    let b = ControlBinding::parse(&read(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/bindings/tally.json"),
    ))
    .unwrap();
    let background = b.props[&binding::Prop::Background].to_expr(&by_id).unwrap();
    assert_eq!(
        background,
        r##"(vh.outputs[1].source) === 3 ? "#e02020" : (vars.preview) === 3 ? "#20a040" : "#333333""##
    );
    // A connection with no alias can't be written as an expression.
    assert_eq!(
        b.props[&binding::Prop::Label].to_expr(&BTreeMap::new()),
        None
    );
}

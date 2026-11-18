//! The binding worker: keys show their bindings, redraw when
//! what they read changes and only then, fall back when a binding has no
//! value, and dim when a value is stale.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use covalence_core::Shared;
use covalence_core::binding::ControlBinding;
use covalence_core::engine::{ActionOutcome, ActionRequest, Actions, CoreError};
use covalence_core::expr::QuickJs;
use covalence_core::flow::{Flow, Runtime};
use covalence_core::journal::{InputKind, Journal};
use covalence_core::state::{Meta, Quality, Source, StatePath};
use covalence_core::surface::gesture::Timing;
use covalence_core::surface::render::{Look, Renderer, Rgb};
use covalence_core::surface::{
    Attach, ControlDef, Draw, ImageDef, Input, InputCap, Link, Output, OutputCap, Placement,
    Surface, Surfaces,
};
use covalence_protocol::Value;
use covalence_protocol::state::PathSegment;
use serde_json::json;

mod common;

const VH: &str = "01JBZ7P0D3F5G7H9J1K3L5M7N9";
const DECK: &str = "01JC5URFACE000000000000001";
const OTHER: &str = "01JC5URFACE000000000000002";
const CAMS: &str = "01JC5PAGE00000000000000001";
const AUDIO: &str = "01JC5PAGE00000000000000002";

struct World {
    shared: Shared,
    surfaces: Surfaces,
}

fn world() -> World {
    let shared = Shared::new(Journal::in_memory(10_000));
    let surfaces = Surfaces::new(shared.clone(), Timing::default());
    for (id, slug) in [(DECK, "deck"), (OTHER, "other")] {
        surfaces
            .set_surface(Surface {
                id: id.into(),
                name: slug.into(),
                slug: slug.into(),
                model: "virtual.deck15".into(),
                serial: None,
                start_layer: Some(CAMS.into()),
                provider: None,
                virtual_layout: None,
            })
            .unwrap();
    }
    for (id, slug) in [(CAMS, "cams"), (AUDIO, "audio")] {
        surfaces
            .set_layer(common::deck_layer(id, slug, 5, 3))
            .unwrap();
    }
    surfaces.set_aliases([("vh".to_owned(), VH.to_owned())].into());
    World { shared, surfaces }
}

fn deck() -> Attach {
    Attach {
        model: "virtual.deck15".into(),
        serial: None,
        name: "Virtual deck".into(),
        controls: (0..15).map(|i| key(i, 5, 72)).collect(),
    }
}

/// Key `k<i>` of a deck `cols` wide, with `size`px images.
fn key(i: u32, cols: u32, size: u32) -> ControlDef {
    ControlDef {
        id: format!("k{i}"),
        label: None,
        at: Placement::at(i % cols, i / cols),
        inputs: vec![InputCap::Momentary { velocity: false }],
        outputs: vec![OutputCap::Image(ImageDef {
            w: size,
            h: size,
            format: "png".into(),
            rotation: 0,
        })],
    }
}

fn path(conn: &str, segments: &[&str]) -> StatePath {
    StatePath::new(
        conn,
        segments
            .iter()
            .map(|s| match s.parse::<i64>() {
                Ok(i) => PathSegment::Index(i),
                Err(_) => PathSegment::Key((*s).into()),
            })
            .collect(),
    )
}

fn set(shared: &Shared, at: StatePath, v: Value) {
    let meta = Meta {
        ts: 0,
        seq: 1,
        quality: Quality::Good,
        source: Source::Conn(VH.into()),
    };
    shared.state.mutate(|tree| ((), tree.set(&at, &v, &meta)));
}

fn binding(id: &str, page: &str, control: &str, props: serde_json::Value) -> ControlBinding {
    ControlBinding::parse(&json!({
        "id": id, "layer": page, "control": control, "props": props,
    }))
    .unwrap()
}

/// A ref into `vh`, with numeric segments as integers, as paths are stored.
fn source(segments: &[&str]) -> serde_json::Value {
    let path: Vec<serde_json::Value> = segments
        .iter()
        .map(|s| s.parse::<i64>().map_or_else(|_| json!(s), |i| json!(i)))
        .collect();
    json!({ "conn": VH, "path": path })
}

/// Draws waiting, by control: the last image each control got.
fn draws(link: &mut Link) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    while let Ok(output) = link.outputs.try_recv() {
        if let Output::Image(Draw { control, image, .. }) = output {
            out.insert(control, image);
        }
    }
    out
}

fn image(look: Look) -> Vec<u8> {
    Renderer::default().render(&look, 72, 72, "png", 0).unwrap()
}

fn label(text: &str) -> Look {
    Look {
        label: text.into(),
        ..Look::default()
    }
}

#[tokio::test]
async fn a_bound_key_shows_its_value_and_redraws_only_when_it_changes() {
    let w = world();
    set(&w.shared, path(VH, &["outputs", "1", "source"]), 3.into());
    w.surfaces.set_binding(binding(
        "b1",
        CAMS,
        "k0",
        json!({
            "label": { "use": "template", "parts": ["Out 1: ", { "ref": source(&["outputs", "1", "source"]) }] },
            "background": {
                "use": "rules",
                "rules": [{ "when": { "use": "compare", "input": { "ref": source(&["outputs", "1", "source"]) }, "op": "==", "value": 3 }, "value": "#e02020" }],
                "else": "#000000",
            },
        }),
    ));
    let mut link = w.surfaces.attach(DECK, deck()).unwrap();
    w.surfaces.flush().await;
    let first = draws(&mut link);
    assert_eq!(first.len(), 15, "every key is drawn once");
    let live = Look {
        label: "Out 1: 3".into(),
        background: Rgb(0xe0, 0x20, 0x20),
        ..Look::default()
    };
    assert_eq!(first["k0"], image(live));
    assert_eq!(first["k1"], image(Look::default()));

    // Something it doesn't read: nothing is drawn.
    set(&w.shared, path(VH, &["outputs", "2", "source"]), 7.into());
    w.surfaces.flush().await;
    assert!(draws(&mut link).is_empty());

    set(&w.shared, path(VH, &["outputs", "1", "source"]), 4.into());
    w.surfaces.flush().await;
    let next = draws(&mut link);
    assert_eq!(next.keys().collect::<Vec<_>>(), ["k0"]);
    assert_eq!(next["k0"], image(label("Out 1: 4")));

    // The same value again changes nothing.
    set(&w.shared, path(VH, &["outputs", "1", "source"]), 4.into());
    w.surfaces.flush().await;
    assert!(draws(&mut link).is_empty());
}

#[tokio::test]
async fn missing_values_fall_back_and_errors_are_reported() {
    let w = world();
    w.surfaces.set_binding(binding(
        "b1",
        CAMS,
        "k0",
        json!({
            "label": { "ref": source(&["name"]), "fallback": "No name" },
            "background": { "expr": "vh.level > 0.5 ? \"#00ff00\" : \"nope\"", "fallback": "#202020" },
        }),
    ));
    w.surfaces.set_binding(binding(
        "b2",
        CAMS,
        "k1",
        json!({ "label": { "expr": "vh.name.toUpperCase()" } }),
    ));
    let mut link = w.surfaces.attach(DECK, deck()).unwrap();
    w.surfaces.flush().await;
    let first = draws(&mut link);
    assert_eq!(
        first["k0"],
        image(Look {
            label: "No name".into(),
            background: Rgb(0x20, 0x20, 0x20),
            ..Look::default()
        }),
        "undefined, then a wrong type, both use the fallback"
    );
    assert_eq!(
        first["k1"],
        image(Look::default()),
        "no fallback: the default"
    );
    let problems = w.surfaces.binding_problems();
    assert!(
        problems[&("b1".to_owned(), "background".to_owned())].contains("isn't a colour"),
        "{problems:?}"
    );
    assert!(
        problems[&("b2".to_owned(), "label".to_owned())].contains("toUpperCase"),
        "{problems:?}"
    );
    assert!(
        !problems.contains_key(&("b1".to_owned(), "label".to_owned())),
        "undefined isn't an error"
    );

    set(&w.shared, path(VH, &["name"]), "SDI 4".into());
    set(&w.shared, path(VH, &["level"]), Value::F64(0.9));
    w.surfaces.flush().await;
    let next = draws(&mut link);
    assert_eq!(
        next["k0"],
        image(Look {
            label: "SDI 4".into(),
            background: Rgb(0, 0xff, 0),
            ..Look::default()
        })
    );
    assert_eq!(next["k1"], image(label("SDI 4")));
    assert!(
        w.surfaces.binding_problems().is_empty(),
        "fixed problems clear"
    );
}

#[tokio::test]
async fn a_stale_value_dims_its_key() {
    let w = world();
    set(&w.shared, path(VH, &["name"]), "CAM 1".into());
    w.surfaces.set_binding(binding(
        "b1",
        CAMS,
        "k0",
        json!({ "label": { "ref": source(&["name"]) } }),
    ));
    w.surfaces.set_binding(binding(
        "b2",
        CAMS,
        "k1",
        json!({ "label": { "expr": "\"fixed\"" } }),
    ));
    let mut link = w.surfaces.attach(DECK, deck()).unwrap();
    w.surfaces.flush().await;
    draws(&mut link);

    w.shared
        .state
        .mutate(|tree| ((), tree.set_quality(&StatePath::root(VH), Quality::Stale)));
    w.surfaces.flush().await;
    let next = draws(&mut link);
    assert_eq!(
        next.keys().collect::<Vec<_>>(),
        ["k0"],
        "only keys that read it"
    );
    assert_eq!(
        next["k0"],
        image(Look {
            stale: true,
            ..label("CAM 1")
        })
    );
}

#[tokio::test]
async fn only_shown_pages_are_evaluated() {
    let w = world();
    w.surfaces.set_binding(binding(
        "b1",
        AUDIO,
        "k0",
        json!({ "label": { "expr": "vh.missing.deeper" } }),
    ));
    let mut link = w.surfaces.attach(DECK, deck()).unwrap();
    w.surfaces.flush().await;
    assert!(
        w.surfaces.binding_problems().is_empty(),
        "the audio page isn't shown"
    );

    w.surfaces
        .show(DECK, Some(AUDIO), covalence_core::journal::Origin::system())
        .unwrap();
    w.surfaces.flush().await;
    assert!(
        w.surfaces
            .binding_problems()
            .contains_key(&("b1".to_owned(), "label".to_owned()))
    );
    draws(&mut link);
}

#[tokio::test]
async fn bindings_follow_dynamic_reads_and_edits() {
    let w = world();
    for (n, name) in [(1, "CAM 1"), (2, "CAM 2")] {
        set(
            &w.shared,
            path(VH, &["inputs", &n.to_string(), "name"]),
            name.into(),
        );
    }
    set(&w.shared, path("vars", &["cam"]), 1.into());
    let mut link = w.surfaces.attach(DECK, deck()).unwrap();
    w.surfaces.set_binding(binding(
        "b1",
        CAMS,
        "k4",
        json!({ "label": { "expr": "vh.inputs[vars.cam].name" } }),
    ));
    w.surfaces.flush().await;
    assert_eq!(
        draws(&mut link)["k4"],
        image(label("CAM 1")),
        "drawn when set"
    );

    set(&w.shared, path("vars", &["cam"]), 2.into());
    w.surfaces.flush().await;
    assert_eq!(draws(&mut link)["k4"], image(label("CAM 2")));
    // It now reads input 2, not input 1.
    set(
        &w.shared,
        path(VH, &["inputs", "1", "name"]),
        "Renamed".into(),
    );
    w.surfaces.flush().await;
    assert!(draws(&mut link).is_empty());

    w.surfaces.remove_binding("b1");
    w.surfaces.flush().await;
    assert_eq!(
        draws(&mut link)["k4"],
        image(Look::default()),
        "blank once removed"
    );
}

#[tokio::test]
async fn every_surface_showing_a_page_gets_its_keys() {
    let w = world();
    w.surfaces.set_binding(binding(
        "b1",
        CAMS,
        "k2",
        json!({ "label": { "expr": "String(vars.count ?? 0)" } }),
    ));
    let mut a = w.surfaces.attach(DECK, deck()).unwrap();
    let mut b = w.surfaces.attach(OTHER, deck()).unwrap();
    w.surfaces.flush().await;
    assert_eq!(draws(&mut a)["k2"], image(label("0")));
    assert_eq!(draws(&mut b)["k2"], image(label("0")));

    set(&w.shared, path("vars", &["count"]), 5.into());
    w.surfaces.flush().await;
    assert_eq!(draws(&mut a)["k2"], image(label("5")));
    assert_eq!(draws(&mut b)["k2"], image(label("5")));
}

/// State change → every key of a full page redrawn, with two `expr`
/// bindings per key reading what changed (the M5 budget is 16 ms). Run with
/// `cargo test --release -p covalence-core --test binding_worker bench -- --ignored --nocapture`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "a benchmark: run it in release"]
async fn bench_a_full_page_of_expr_bindings() {
    for (cols, rows, size) in [(5u32, 3u32, 72u32), (5, 3, 144), (8, 4, 96)] {
        let w = world();
        let page = format!("01JC5PAGEBENCH{cols}{rows}{size:0>9}");
        w.surfaces
            .set_layer(common::deck_layer(
                &page,
                &format!("bench{cols}{rows}"),
                cols,
                rows,
            ))
            .unwrap();
        let surface = format!("01JC5URFACEBENCH{cols}{rows}{size:0>7}");
        w.surfaces
            .set_surface(Surface {
                id: surface.clone(),
                name: "bench".into(),
                slug: format!("bench{cols}{rows}{size}"),
                model: "virtual".into(),
                serial: None,
                start_layer: Some(page.clone()),
                provider: None,
                virtual_layout: None,
            })
            .unwrap();
        let keys = cols * rows;
        for i in 0..keys {
            w.surfaces.set_binding(binding(
                &format!("b{i}"),
                &page,
                &format!("k{i}"),
                json!({
                    "label": { "expr": format!("`Cam {i}: ${{mmss(vars.n + {i})}}`") },
                    "background": { "expr": format!("vars.n % 2 ? hsl({i} * 20, 70, 40) : \"#202020\"") },
                }),
            ));
        }
        let mut attach = deck();
        attach.controls = (0..keys).map(|i| key(i, cols, size)).collect();
        set(&w.shared, path("vars", &["n"]), 0.into());
        let mut link = w.surfaces.attach(&surface, attach).unwrap();
        w.surfaces.flush().await;
        draws(&mut link);

        let mut times = Vec::new();
        for n in 1..=40 {
            let start = std::time::Instant::now();
            set(&w.shared, path("vars", &["n"]), n.into());
            let mut got = 0;
            while got < keys {
                link.outputs.recv().await.unwrap();
                got += 1;
            }
            times.push(start.elapsed());
        }
        times.sort();
        let median = times[times.len() / 2];
        let worst = times[times.len() - 1];
        println!(
            "{keys} keys at {size}px, 2 expr bindings each: median {median:?}, worst {worst:?}"
        );
    }
}

/// Notes when the flow runtime asks for its first action, and answers with
/// an error so the run ends there.
struct Timed(tokio::sync::mpsc::UnboundedSender<std::time::Instant>);

impl Actions for Timed {
    fn request_action(
        &self,
        req: ActionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ActionOutcome, CoreError>> + Send + '_>> {
        let _ = self.0.send(std::time::Instant::now());
        Box::pin(async move { Err(CoreError::UnknownConnection(req.conn)) })
    }
}

/// Key down → first action requested by its flow, while a full layer of
/// `expr` bindings redraws for a state change made just before (the M5
/// budget is 5 ms for a local surface). M6: the trigger is an event trigger,
/// once on the layer's key and once a `*` path pattern, matched among 200
/// other event triggers. Run with
/// `cargo test --release -p covalence-core --test binding_worker bench -- --ignored --nocapture`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "a benchmark: run it in release"]
async fn bench_key_down_to_first_action() {
    for (what, from) in [
        ("a layer's key", json!({ "layer": CAMS, "controls": "k0" })),
        (
            "a path pattern",
            json!({ "path": { "match": "surface.*.controls.k0" } }),
        ),
    ] {
        for others in [0, 200] {
            key_down_to_first_action(what, from.clone(), others).await;
        }
    }
}

async fn key_down_to_first_action(what: &str, from: serde_json::Value, others: usize) {
    let w = world();
    let (requested, mut requests) = tokio::sync::mpsc::unbounded_channel();
    let runtime = Runtime::start(
        w.shared.clone(),
        Arc::new(Timed(requested)),
        Arc::new(QuickJs::default()),
    );
    for i in 0..15 {
        w.surfaces.set_binding(binding(
            &format!("b{i}"),
            CAMS,
            &format!("k{i}"),
            json!({
                "label": { "expr": format!("`Cam {i}: ${{mmss(vars.n + {i})}}`") },
                "background": { "expr": format!("vars.n % 2 ? hsl({i} * 20, 70, 40) : \"#202020\"") },
            }),
        ));
    }
    // Others the matcher has to pass over: other keys, connections, flows.
    for i in 0..others {
        let from = match i % 4 {
            0 => json!({ "layer": CAMS, "controls": format!("k{}", 1 + i % 14) }),
            1 => json!({ "target": VH }),
            2 => json!({ "path": format!("flow.other{i}") }),
            _ => json!({ "path": { "match": format!("surface.*.controls.dial{i}") } }),
        };
        let flow: Flow = serde_json::from_value(json!({
            "id": format!("01JC5FLOWBENCH0000000{i:05}"),
            "name": format!("Other {i}"),
            "slug": format!("other{i}"),
            "enabled": true,
            "triggers": [{ "kind": "event", "from": from, "event": "press" }],
            "concurrency": "parallel",
            "body": [],
        }))
        .unwrap();
        runtime.set_flow(flow);
    }
    let flow: Flow = serde_json::from_value(json!({
        "id": "01JC5FLOWBENCH000000000001",
        "name": "Route on press",
        "slug": "routeOnPress",
        "enabled": true,
        "triggers": [{ "kind": "event", "from": from, "event": "press" }],
        "concurrency": "parallel",
        "body": [{
            "id": "route", "kind": "action", "target": { "conn": VH },
            "action": "route", "params": { "output": 1, "input": 2 },
        }],
    }))
    .unwrap();
    runtime.set_flow(flow);
    set(&w.shared, path("vars", &["n"]), 0.into());
    let mut link = w.surfaces.attach(DECK, deck()).unwrap();
    w.surfaces.flush().await;
    draws(&mut link);

    let key = |kind, ts| Input {
        control: "k0".into(),
        kind,
        value: None,
        ts,
        echo: false,
    };
    let mut times = Vec::new();
    for n in 1..=40 {
        set(&w.shared, path("vars", &["n"]), n.into());
        let start = std::time::Instant::now();
        let ts = f64::from(n) * 1_000.0;
        link.input.send(key(InputKind::Down, ts)).await.unwrap();
        let at = requests.recv().await.unwrap();
        times.push(at - start);
        link.input
            .send(key(InputKind::Up, ts + 50.0))
            .await
            .unwrap();
        w.surfaces.flush().await;
        draws(&mut link);
    }
    times.sort();
    println!(
        "key down to first action, on {what} among {others} other triggers, 15 keys of expr bindings redrawing: median {:?}, worst {:?}",
        times[times.len() / 2],
        times[times.len() - 1]
    );
    runtime.shutdown().await;
}

/// M8's first "done when": a latching key. A `toggle` helper on `k3`, its
/// background a `rules` source on the helper's value, and a flow that runs
/// on the helper's `on` event.
#[tokio::test]
async fn a_toggle_helper_latches_a_key_and_its_events_start_flows() {
    use covalence_core::dataflow::Dataflow;
    let w = world();
    let dataflow = Dataflow::start(&w.shared);
    let (requested, mut requests) = tokio::sync::mpsc::unbounded_channel();
    let runtime = Runtime::start(
        w.shared.clone(),
        Arc::new(Timed(requested)),
        Arc::new(QuickJs::default()),
    );
    let helper = json!({ "root": "layer", "path": ["cams", "controls", "k3", "latched"] });
    let b = ControlBinding::parse(&json!({
        "id": "b3", "layer": CAMS, "control": "k3",
        "helpers": { "latched": { "use": "toggle" } },
        "props": { "background": {
            "use": "rules",
            "rules": [{ "when": { "ref": helper }, "value": "#e02020" }],
            "else": "#000000",
        } },
    }))
    .unwrap();
    w.surfaces.set_binding(b.clone());
    dataflow.replace("helper:b3/", b.helper_cells("cams"), None);
    let flow: Flow = serde_json::from_value(json!({
        "id": "01JC5FLOWLATCH000000000001",
        "name": "On air", "slug": "onAir", "enabled": true,
        "triggers": [{ "kind": "event", "from": { "layer": CAMS, "controls": "k3" }, "event": "on" }],
        "concurrency": "parallel",
        "body": [{
            "id": "route", "kind": "action", "target": { "conn": VH },
            "action": "route", "params": { "output": 1, "input": 2 },
        }],
    }))
    .unwrap();
    runtime.set_flow(flow);

    let mut link = w.surfaces.attach(DECK, deck()).unwrap();
    w.surfaces.flush().await;
    let first = draws(&mut link);
    assert_eq!(first["k3"], image(Look::default()));

    let press = |kind, ts| Input {
        control: "k3".into(),
        kind,
        value: None,
        ts,
        echo: false,
    };
    let latched = path("layer", &["cams", "controls", "k3", "latched"]);
    for (n, (on, red)) in [(true, true), (false, false)].into_iter().enumerate() {
        let ts = 1_000.0 * (n as f64 + 1.0);
        link.input.send(press(InputKind::Down, ts)).await.unwrap();
        link.input
            .send(press(InputKind::Up, ts + 50.0))
            .await
            .unwrap();
        // Input reaches the surface's own task; wait for it to land.
        for _ in 0..400 {
            w.surfaces.flush().await;
            dataflow.flush().await;
            if w.shared.state.value(&latched) == Some(Value::from(on)) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert_eq!(w.shared.state.value(&latched), Some(Value::from(on)));
        w.surfaces.flush().await;
        let look = Look {
            background: if red {
                Rgb(0xe0, 0x20, 0x20)
            } else {
                Rgb(0, 0, 0)
            },
            ..Look::default()
        };
        assert_eq!(draws(&mut link)["k3"], image(look));
    }
    // The flow ran once: on `on`, not on `off`.
    tokio::time::timeout(std::time::Duration::from_secs(2), requests.recv())
        .await
        .expect("the flow ran on `on`")
        .unwrap();
    assert!(requests.try_recv().is_err());
    // The helper's events are at its cell, with its layer and control.
    let events: Vec<_> = w
        .shared
        .journal
        .range(1, usize::MAX)
        .unwrap()
        .into_iter()
        .filter_map(|e| match &e.record {
            covalence_core::journal::Record::Event(ev) if ev.path == latched => {
                Some((ev.event.clone(), ev.layer.clone(), ev.control.clone()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        events,
        [
            ("on".into(), Some(CAMS.into()), Some("k3".into())),
            ("off".into(), Some(CAMS.into()), Some("k3".into()))
        ]
    );
    runtime.shutdown().await;
}

/// Levels sent, in order: `(control, value)`.
fn levels(link: &mut Link) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    while let Ok(output) = link.outputs.try_recv() {
        if let Output::Level { control, value } = output {
            out.push((control, value));
        }
    }
    out
}

/// surfaces spec §11.4: a `value` binding drives a `level` output. Each level sent is
/// written to `requested.level`, with `cause` → what the binding read; an
/// unchanged or `undefined` value sends nothing.
#[tokio::test]
async fn a_value_binding_sets_a_level_and_records_what_was_sent() {
    use covalence_core::journal::Record;
    use covalence_core::surface::{VirtualLayout, virtual_controls};

    const MIX: &str = "01JC5PAGE00000000000000003";
    let w = world();
    let controls = virtual_controls(Some(&VirtualLayout {
        encoders: 2,
        faders: 2,
        motor: true,
    }));
    assert_eq!(
        controls
            .iter()
            .map(|c| c.id.as_str())
            .skip(15)
            .collect::<Vec<_>>(),
        ["e0", "e1", "f0", "f1"]
    );
    w.surfaces
        .set_layer(covalence_core::surface::Layer {
            id: MIX.into(),
            name: "mix".into(),
            slug: "mix".into(),
            controls: controls.iter().map(ControlDef::to_layer_control).collect(),
            ..Default::default()
        })
        .unwrap();
    set(&w.shared, path(VH, &["mixer", "ch1"]), 0.25.into());
    w.surfaces.set_binding(binding(
        "b1",
        MIX,
        "f0",
        json!({ "value": { "ref": source(&["mixer", "ch1"]) } }),
    ));
    // Out of range is held to 0…1.
    w.surfaces
        .set_binding(binding("b2", MIX, "e0", json!({ "value": 3 })));
    // Nothing to read: no level is sent.
    w.surfaces.set_binding(binding(
        "b3",
        MIX,
        "f1",
        json!({ "value": { "ref": source(&["mixer", "ch9"]) } }),
    ));

    let mut link = w
        .surfaces
        .attach(
            DECK,
            Attach {
                model: "virtual.deck15".into(),
                serial: None,
                name: "Virtual deck".into(),
                controls,
            },
        )
        .unwrap();
    w.surfaces
        .show(DECK, Some(MIX), covalence_core::journal::Origin::system())
        .unwrap();
    w.surfaces.flush().await;
    assert_eq!(
        levels(&mut link),
        [("e0".to_owned(), 1.0), ("f0".to_owned(), 0.25)]
    );
    let requested = |control: &str| {
        w.shared.state.value(&path(
            "surface",
            &["deck", "controls", control, "requested", "level"],
        ))
    };
    assert_eq!(requested("f0"), Some(0.25.into()));
    assert_eq!(requested("f1"), None);

    // `why` on the level reaches what the binding read (seq 1 here).
    let entries = w.shared.journal.range(1, 10_000).unwrap();
    let sent = entries
        .iter()
        .rev()
        .find(|e| matches!(&e.record, Record::StatePatch(p) if p.ops.iter().any(|op| format!("{op:?}").contains("\"f0\""))))
        .unwrap();
    assert_eq!(sent.cause, Some(1));

    set(&w.shared, path(VH, &["mixer", "ch1"]), 0.5.into());
    w.surfaces.flush().await;
    assert_eq!(levels(&mut link), [("f0".to_owned(), 0.5)]);
    assert_eq!(requested("f0"), Some(0.5.into()));

    // The same value again sends nothing.
    set(&w.shared, path(VH, &["mixer", "ch1"]), 0.5.into());
    w.surfaces.flush().await;
    assert!(levels(&mut link).is_empty());
}

#[test]
fn a_value_needs_a_level_output_and_a_number() {
    let layers: BTreeMap<String, covalence_core::surface::Layer> =
        [(CAMS.to_owned(), common::deck_layer(CAMS, "cams", 5, 3))].into();
    let conns = [VH.to_owned()].into();
    let ctx = covalence_core::binding::Context {
        layers: &layers,
        conns: &conns,
        taken: &Default::default(),
        evaluator: &covalence_core::expr::QuickJs::default(),
    };
    let codes = |props| {
        covalence_core::binding::validate(&binding("b", CAMS, "k0", props), &ctx)
            .errors
            .iter()
            .map(|p| serde_json::to_value(p).unwrap()["code"].clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(codes(json!({ "value": 0.5 })), [json!("noOutput")]);
    assert_eq!(
        codes(json!({ "value": "loud" })),
        [json!("noOutput"), json!("badValue")]
    );
}

/// surfaces spec §11.4: a helper with `scope: "surface"` has a cell on each surface,
/// stepped only by that surface's input while its layer controls the
/// control, and keeps its value while the surface shows another layer.
#[tokio::test]
async fn a_surface_scoped_helper_keeps_one_value_per_surface() {
    use covalence_core::dataflow::Dataflow;
    use covalence_core::journal::{Origin, Record};
    let w = world();
    let dataflow = Dataflow::start(&w.shared);
    let b = ControlBinding::parse(&json!({
        "id": "b3", "layer": CAMS, "control": "k3",
        "helpers": { "latched": { "use": "toggle", "scope": "surface" } },
    }))
    .unwrap();
    assert!(b.helper_cells("cams").is_empty());
    let mut cells = b.surface_helper_cells(DECK, "deck");
    cells.extend(b.surface_helper_cells(OTHER, "other"));
    assert_eq!(cells[0].id, format!("helper:b3/latched@{DECK}"));
    dataflow.replace("helper:b3/", cells, None);

    let deck_link = w.surfaces.attach(DECK, deck()).unwrap();
    let _other_link = w.surfaces.attach(OTHER, deck()).unwrap();
    let latched = |slug: &str| path("surface", &[slug, "controls", "k3", "helpers", "latched"]);
    let press = |ts: f64| {
        let input = deck_link.input.clone();
        async move {
            for (kind, at) in [(InputKind::Down, ts), (InputKind::Up, ts + 50.0)] {
                input
                    .send(Input {
                        control: "k3".into(),
                        kind,
                        value: None,
                        ts: at,
                        echo: false,
                    })
                    .await
                    .unwrap();
            }
        }
    };
    let settle = || async {
        for _ in 0..40 {
            w.surfaces.flush().await;
            dataflow.flush().await;
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    };

    press(1_000.0).await;
    settle().await;
    assert_eq!(w.shared.state.value(&latched("deck")), Some(true.into()));
    assert_eq!(w.shared.state.value(&latched("other")), Some(false.into()));

    // On another layer, the deck's k3 isn't this helper's.
    w.surfaces
        .show(DECK, Some(AUDIO), Origin::system())
        .unwrap();
    press(2_000.0).await;
    settle().await;
    assert_eq!(w.shared.state.value(&latched("deck")), Some(true.into()));

    // Back again, it kept its value.
    w.surfaces.show(DECK, Some(CAMS), Origin::system()).unwrap();
    press(3_000.0).await;
    settle().await;
    assert_eq!(w.shared.state.value(&latched("deck")), Some(false.into()));

    // Its events are at its cell, with its surface, layer and control.
    let events: Vec<_> = w
        .shared
        .journal
        .range(1, usize::MAX)
        .unwrap()
        .into_iter()
        .filter_map(|e| match &e.record {
            Record::Event(ev) if ev.path == latched("deck") => Some((
                ev.event.clone(),
                ev.surface.clone(),
                ev.layer.clone(),
                ev.control.clone(),
            )),
            _ => None,
        })
        .collect();
    let at = |name: &str| {
        (
            name.to_owned(),
            Some(DECK.to_owned()),
            Some(CAMS.to_owned()),
            Some("k3".to_owned()),
        )
    };
    assert_eq!(events, [at("on"), at("off")]);
}

#[test]
fn gesture_settings_must_be_durations() {
    let layers: BTreeMap<String, covalence_core::surface::Layer> =
        [(CAMS.to_owned(), common::deck_layer(CAMS, "cams", 5, 3))].into();
    let conns = [VH.to_owned()].into();
    let ctx = covalence_core::binding::Context {
        layers: &layers,
        conns: &conns,
        taken: &Default::default(),
        evaluator: &covalence_core::expr::QuickJs::default(),
    };
    let problems = |gestures| {
        let b = ControlBinding::parse(&json!({
            "id": "b", "layer": CAMS, "control": "k0", "gestures": gestures,
        }))
        .unwrap();
        covalence_core::binding::validate(&b, &ctx)
            .errors
            .iter()
            .map(|p| {
                let p = serde_json::to_value(p).unwrap();
                (p["path"].clone(), p["code"].clone())
            })
            .collect::<Vec<_>>()
    };
    assert!(problems(json!({ "longAfter": "800ms", "holdEvery": 100, "doubleWindow": { "expr": "vars.slow ? 600 : 300" } })).is_empty());
    assert_eq!(
        problems(json!({ "longAfter": "soon", "holdEvery": 0 })),
        [
            (json!(["gestures", "longAfter"]), json!("badValue")),
            (json!(["gestures", "holdEvery"]), json!("badValue")),
        ]
    );
    assert!(
        ControlBinding::parse(&json!({
            "id": "b", "layer": CAMS, "control": "k0", "gestures": { "tripleWindow": 9 },
        }))
        .is_err()
    );
}

/// A binding that blocks its control's input can't have helpers, which
/// would never step (surfaces spec §5.2.1).
#[test]
fn a_blocking_binding_has_no_helpers() {
    let layers: BTreeMap<String, covalence_core::surface::Layer> =
        [(CAMS.to_owned(), common::deck_layer(CAMS, "cams", 5, 3))].into();
    let conns = [VH.to_owned()].into();
    let ctx = covalence_core::binding::Context {
        layers: &layers,
        conns: &conns,
        taken: &Default::default(),
        evaluator: &covalence_core::expr::QuickJs::default(),
    };
    let codes = |extra: serde_json::Value| {
        let mut b = json!({ "id": "b", "layer": CAMS, "control": "k0" });
        b.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        covalence_core::binding::validate(&ControlBinding::parse(&b).unwrap(), &ctx)
            .errors
            .iter()
            .map(|p| serde_json::to_value(p).unwrap()["code"].clone())
            .collect::<Vec<_>>()
    };
    assert!(codes(json!({ "input": "block", "props": { "label": "Locked" } })).is_empty());
    assert!(codes(json!({ "input": "pass", "helpers": { "on": { "use": "toggle" } } })).is_empty());
    assert_eq!(
        codes(json!({ "input": "block", "helpers": { "on": { "use": "toggle" } } })),
        [json!("invalid")]
    );
}

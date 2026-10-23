//! The binding worker (M5 Phase 2): keys show their bindings, redraw when
//! what they read changes and only then, fall back when a binding has no
//! value, and dim when a value is stale.

use std::collections::BTreeMap;

use covalence_core::Shared;
use covalence_core::binding::{ControlBinding, Prop};
use covalence_core::journal::Journal;
use covalence_core::state::{Meta, Quality, Source, StatePath};
use covalence_core::surface::gesture::Timing;
use covalence_core::surface::render::{Look, Renderer, Rgb};
use covalence_core::surface::{
    Attach, ControlDef, Draw, ImageDef, InputCap, Layout, Link, OutputCap, Page, Placement,
    Surface, Surfaces,
};
use covalence_protocol::Value;
use covalence_protocol::state::PathSegment;
use serde_json::json;

const VH: &str = "01JBZ7P0D3F5G7H9J1K3L5M7N9";
const DECK: &str = "01JC5URFACE000000000000001";
const OTHER: &str = "01JC5URFACE000000000000002";
const CAMS: &str = "01JC5PAGE00000000000000001";
const AUDIO: &str = "01JC5PAGE00000000000000002";
const LAYOUT: Layout = Layout { cols: 5, rows: 3 };

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
                start_page: Some(CAMS.into()),
            })
            .unwrap();
    }
    for (id, slug) in [(CAMS, "cams"), (AUDIO, "audio")] {
        surfaces
            .set_page(Page {
                id: id.into(),
                name: slug.into(),
                slug: slug.into(),
                layout: LAYOUT,
            })
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
        controls: (0..LAYOUT.cols * LAYOUT.rows)
            .map(|i| key(i, LAYOUT.cols, 72))
            .collect(),
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
        "id": id, "page": page, "control": control, "props": props,
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
    while let Ok(Draw { control, image, .. }) = link.draws.try_recv() {
        out.insert(control, image);
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
            "label": { "kind": "template", "parts": ["Out 1: ", { "path": source(&["outputs", "1", "source"]) }] },
            "background": {
                "kind": "rules",
                "rules": [{ "when": { "path": source(&["outputs", "1", "source"]), "op": "==", "value": 3 }, "value": "#e02020" }],
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
            "label": { "kind": "value", "path": source(&["name"]), "fallback": "No name" },
            "background": { "kind": "expr", "source": "vh.level > 0.5 ? \"#00ff00\" : \"nope\"", "fallback": "#202020" },
        }),
    ));
    w.surfaces.set_binding(binding(
        "b2",
        CAMS,
        "k1",
        json!({ "label": { "kind": "expr", "source": "vh.name.toUpperCase()" } }),
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
        problems[&("b1".to_owned(), Prop::Background)].contains("isn't a colour"),
        "{problems:?}"
    );
    assert!(
        problems[&("b2".to_owned(), Prop::Label)].contains("toUpperCase"),
        "{problems:?}"
    );
    assert!(
        !problems.contains_key(&("b1".to_owned(), Prop::Label)),
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
        json!({ "label": { "kind": "value", "path": source(&["name"]) } }),
    ));
    w.surfaces.set_binding(binding(
        "b2",
        CAMS,
        "k1",
        json!({ "label": { "kind": "expr", "source": "\"fixed\"" } }),
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
        json!({ "label": { "kind": "expr", "source": "vh.missing.deeper" } }),
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
            .contains_key(&("b1".to_owned(), Prop::Label))
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
        json!({ "label": { "kind": "expr", "source": "vh.inputs[vars.cam].name" } }),
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
        json!({ "label": { "kind": "expr", "source": "String(vars.count ?? 0)" } }),
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
        let layout = Layout { cols, rows };
        let page = format!("01JC5PAGEBENCH{cols}{rows}{size:0>9}");
        w.surfaces
            .set_page(Page {
                id: page.clone(),
                name: "bench".into(),
                slug: format!("bench{cols}{rows}"),
                layout,
            })
            .unwrap();
        let surface = format!("01JC5URFACEBENCH{cols}{rows}{size:0>7}");
        w.surfaces
            .set_surface(Surface {
                id: surface.clone(),
                name: "bench".into(),
                slug: format!("bench{cols}{rows}{size}"),
                model: "virtual".into(),
                serial: None,
                start_page: Some(page.clone()),
            })
            .unwrap();
        let keys = cols * rows;
        for i in 0..keys {
            w.surfaces.set_binding(binding(
                &format!("b{i}"),
                &page,
                &format!("k{i}"),
                json!({
                    "label": { "kind": "expr", "source": format!("`Cam {i}: ${{mmss(vars.n + {i})}}`") },
                    "background": { "kind": "expr", "source": format!("vars.n % 2 ? hsl({i} * 20, 70, 40) : \"#202020\"") },
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
                link.draws.recv().await.unwrap();
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

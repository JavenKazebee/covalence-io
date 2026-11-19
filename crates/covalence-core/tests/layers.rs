//! Layer resolution (surfaces spec §5.2): which layer controls each
//! control, from `on`, `when`, `level` and `rank`, written to state in the
//! turn that changed it and drawn from there.

use std::collections::BTreeMap;
use std::time::Duration;

use covalence_core::Shared;
use covalence_core::binding::ControlBinding;
use covalence_core::journal::{InputKind, Journal, Origin, Record};
use covalence_core::state::StatePath;
use covalence_core::surface::gesture::Timing;
use covalence_core::surface::render::{Look, Renderer, Rgb};
use covalence_core::surface::{
    Attach, ControlDef, Draw, ImageDef, Input, InputCap, Layer, Link, Output, OutputCap, Placement,
    Surface, Surfaces,
};
use covalence_protocol::Value;
use covalence_protocol::state::PathSegment;
use serde_json::json;

mod common;

const DECK: &str = "01JC5URFACE000000000000001";
const OTHER: &str = "01JC5URFACE000000000000002";
const CAMS: &str = "01JC5PAGE00000000000000001";
const SHIFT: &str = "01JC5PAGE00000000000000002";
const MODE: &str = "01JC5PAGE00000000000000003";
const POP: &str = "01JC5PAGE00000000000000004";
const STRIP: &str = "01JC5PAGE00000000000000005";

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
                start_component: Some(CAMS.into()),
                provider: None,
                virtual_layout: None,
            })
            .unwrap();
    }
    surfaces
        .set_layer(common::deck_layer(CAMS, "cams", 5, 3))
        .unwrap();
    World { shared, surfaces }
}

/// A layer covering some of the deck's keys, with fields from `json`.
fn layer(id: &str, slug: &str, keys: &[u32], json: serde_json::Value) -> Layer {
    let full = common::deck_layer(id, slug, 5, 3);
    let mut l = serde_json::to_value(Layer {
        controls: full
            .controls
            .into_iter()
            .filter(|c| keys.iter().any(|k| c.id == format!("k{k}")))
            .collect(),
        ..full
    })
    .unwrap();
    l.as_object_mut()
        .unwrap()
        .extend(json.as_object().unwrap().clone());
    Layer::parse(&l).unwrap()
}

fn deck() -> Attach {
    Attach {
        model: "virtual.deck15".into(),
        serial: None,
        name: "Virtual deck".into(),
        controls: (0..15)
            .map(|i| ControlDef {
                id: format!("k{i}"),
                label: None,
                at: Placement::at(i % 5, i / 5),
                inputs: vec![InputCap::Momentary { velocity: false }],
                outputs: vec![OutputCap::Image(ImageDef {
                    w: 72,
                    h: 72,
                    format: "png".into(),
                    rotation: 0,
                })],
            })
            .collect(),
    }
}

fn path(segments: &[&str]) -> StatePath {
    StatePath::new(
        "surface",
        segments
            .iter()
            .map(|s| PathSegment::Key((*s).into()))
            .collect(),
    )
}

fn op_path(op: &covalence_core::journal::JournalOp) -> &StatePath {
    match op {
        covalence_core::journal::JournalOp::Set { path, .. }
        | covalence_core::journal::JournalOp::Delete { path, .. } => path,
    }
}

fn slugs(v: &[&str]) -> Value {
    Value::Array(v.iter().map(|s| Value::from(*s)).collect())
}

fn press(control: &str, kind: InputKind) -> Input {
    Input {
        control: control.into(),
        kind,
        value: None,
        ts: 1_000.0,
        echo: false,
    }
}

/// Waits until `path` holds `want`, the surface's task having applied its
/// input.
async fn until(w: &World, at: &StatePath, want: Value) {
    for _ in 0..400 {
        w.surfaces.flush().await;
        if w.shared.state.value(at).as_ref() == Some(&want) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(w.shared.state.value(at), Some(want), "at {at:?}");
}

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

fn label(id: &str, layer: &str, control: &str, text: &str) -> ControlBinding {
    ControlBinding::parse(&json!({
        "id": id, "layer": layer, "control": control,
        "props": { "label": text, "background": "#202020" },
    }))
    .unwrap()
}

fn look(text: &str) -> Look {
    Look {
        label: text.into(),
        background: Rgb(0x20, 0x20, 0x20),
        ..Look::default()
    }
}

/// The milestone's shift layer: `level: over`, active while `k14` is held,
/// over three keys. It's armed while inactive, takes the keys and their
/// input while held, and gives them back on release. Who controls a key is
/// state, with `cause` back to the press.
#[tokio::test]
async fn a_shift_layer_takes_three_keys_while_its_key_is_held() {
    let w = world();
    w.surfaces
        .set_layer(layer(
            SHIFT,
            "shift",
            &[2, 3, 4],
            json!({
                "level": "over",
                "when": { "while": { "expr": "self.controls.k14.pressed" } },
            }),
        ))
        .unwrap();
    w.surfaces.set_binding(label("b-cam2", CAMS, "k2", "Cam 2"));
    w.surfaces.set_binding(label("b-cam3", CAMS, "k3", "Cam 3"));
    w.surfaces
        .set_binding(label("b-shift3", SHIFT, "k3", "Wide"));

    let mut link = w.surfaces.attach(DECK, deck()).unwrap();
    w.surfaces.flush().await;
    assert_eq!(draws(&mut link)["k3"], image(look("Cam 3")));
    let layers = path(&["deck", "shown"]);
    let k3 = path(&["deck", "controls", "k3", "component"]);
    assert_eq!(w.shared.state.value(&layers), Some(slugs(&["cams"])));
    assert_eq!(w.shared.state.value(&k3), Some("cams".into()));
    let resolution = w.surfaces.resolution(DECK).unwrap();
    assert!(resolution.armed.contains(SHIFT), "shift is evaluated ahead");

    link.input
        .send(press("k14", InputKind::Down))
        .await
        .unwrap();
    until(&w, &k3, "shift".into()).await;
    assert_eq!(
        w.shared.state.value(&layers),
        Some(slugs(&["shift", "cams"]))
    );
    assert_eq!(
        w.shared
            .state
            .value(&path(&["deck", "controls", "k5", "component"])),
        Some("cams".into()),
        "keys shift doesn't cover stay on cams"
    );
    w.surfaces.flush().await;
    let drawn = draws(&mut link);
    assert_eq!(drawn["k3"], image(look("Wide")));
    assert_eq!(
        drawn["k2"],
        image(Look::default()),
        "a key the shift layer covers but doesn't bind is blank"
    );
    assert!(!drawn.contains_key("k5"));

    // `why` on k3's layer reaches the press.
    let entries = w.shared.journal.range(1, usize::MAX).unwrap();
    let resolved = entries
        .iter()
        .rev()
        .find(|e| match &e.record {
            Record::StatePatch(p) => p.ops.iter().any(|op| op_path(op) == &k3),
            _ => false,
        })
        .unwrap();
    assert!(matches!(resolved.origin, Origin::Core { .. }));
    let cause = entries
        .iter()
        .find(|e| Some(e.seq) == resolved.cause)
        .unwrap();
    assert!(
        matches!(&cause.record, Record::StatePatch(p) if p.ops.iter().any(|op| op_path(op) == &path(&["deck", "controls", "k14", "pressed"]))),
        "caused by {cause:?}"
    );

    // A press on k3 while shift is held is shift's.
    link.input.send(press("k3", InputKind::Down)).await.unwrap();
    link.input.send(press("k3", InputKind::Up)).await.unwrap();
    until(
        &w,
        &path(&["deck", "controls", "k3", "pressed"]),
        false.into(),
    )
    .await;
    let layer_of_press = w
        .shared
        .journal
        .range(1, usize::MAX)
        .unwrap()
        .into_iter()
        .find_map(|e| match &e.record {
            Record::Event(ev) if ev.control.as_deref() == Some("k3") && ev.event == "press" => {
                Some(ev.component.clone())
            }
            _ => None,
        });
    assert_eq!(layer_of_press, Some(Some(SHIFT.into())));

    link.input.send(press("k14", InputKind::Up)).await.unwrap();
    until(&w, &k3, "cams".into()).await;
    assert_eq!(w.shared.state.value(&layers), Some(slugs(&["cams"])));
    w.surfaces.flush().await;
    assert_eq!(draws(&mut link)["k3"], image(look("Cam 3")));
}

/// Level beats rank, rank beats recency, and recency beats age; a layer
/// that becomes active again goes back on top of its level and rank.
#[tokio::test]
async fn layers_are_ordered_by_level_then_rank_then_recency() {
    let w = world();
    let while_var = |name: &str| json!({ "while": { "ref": { "root": "vars", "path": [name] } } });
    w.surfaces
        .set_layer(layer(
            SHIFT,
            "a",
            &[0],
            json!({ "level": "over", "when": while_var("a") }),
        ))
        .unwrap();
    w.surfaces
        .set_layer(layer(
            MODE,
            "b",
            &[0],
            json!({ "level": "over", "when": while_var("b") }),
        ))
        .unwrap();
    w.surfaces
        .set_layer(layer(
            STRIP,
            "strip",
            &[0, 1],
            json!({ "level": "under", "rank": 50, "when": { "always": true } }),
        ))
        .unwrap();
    let _link = w.surfaces.attach(DECK, deck()).unwrap();
    let k0 = path(&["deck", "controls", "k0", "component"]);
    let layers = path(&["deck", "shown"]);
    let set = |name: &str, on: bool| {
        let at = StatePath::new("vars", vec![PathSegment::Key(name.into())]);
        let meta = covalence_core::state::Meta {
            ts: 0,
            seq: 1,
            quality: covalence_core::state::Quality::Good,
            source: covalence_core::state::Source::Core,
        };
        w.shared
            .state
            .mutate(|tree| ((), tree.set(&at, &Value::from(on), &meta)));
    };
    w.surfaces.flush().await;
    // An `under` strip loses to the page, whatever its rank.
    assert_eq!(
        w.shared.state.value(&layers),
        Some(slugs(&["cams", "strip"]))
    );
    assert_eq!(w.shared.state.value(&k0), Some("cams".into()));

    set("a", true);
    set("b", true);
    assert_eq!(
        w.shared.state.value(&k0),
        Some("b".into()),
        "the latest wins"
    );
    set("a", false);
    set("a", true);
    assert_eq!(w.shared.state.value(&k0), Some("a".into()), "on top again");
    assert_eq!(
        w.shared.state.value(&layers),
        Some(slugs(&["a", "b", "cams", "strip"]))
    );
    // A rank puts `b` above `a` however recent `a` is.
    w.surfaces
        .set_layer(layer(
            MODE,
            "b",
            &[0],
            json!({ "level": "over", "rank": 1, "when": while_var("b") }),
        ))
        .unwrap();
    assert_eq!(w.shared.state.value(&k0), Some("b".into()));
    // With nothing at `normal` covering it, the strip has k0.
    set("a", false);
    set("b", false);
    w.surfaces
        .set_layer(layer(CAMS, "cams", &[5, 6], json!({})))
        .unwrap();
    assert_eq!(w.shared.state.value(&k0), Some("strip".into()));
    assert_eq!(
        w.shared
            .state
            .value(&path(&["deck", "controls", "k9", "component"])),
        Some(Value::Nil),
        "a key no active layer covers is nobody's"
    );
}

/// A popover is opened by navigating its group to it, and closed by
/// navigating the group to nothing. `on` keeps a layer to its surfaces.
#[tokio::test]
async fn popovers_open_by_navigation_and_on_picks_surfaces() {
    let w = world();
    w.surfaces
        .set_layer(layer(
            POP,
            "pop",
            &[6, 7, 8],
            json!({ "level": "over", "when": { "nav": "popover" }, "on": [DECK] }),
        ))
        .unwrap();
    let _deck = w.surfaces.attach(DECK, deck()).unwrap();
    let _other = w.surfaces.attach(OTHER, deck()).unwrap();
    let k7 = path(&["deck", "controls", "k7", "component"]);
    w.surfaces.flush().await;
    assert_eq!(w.shared.state.value(&k7), Some("cams".into()));

    w.surfaces
        .navigate(DECK, "popover", Some(POP), Origin::system())
        .unwrap();
    assert_eq!(
        w.shared.state.value(&path(&["deck", "current", "popover"])),
        Some("pop".into())
    );
    assert_eq!(w.shared.state.value(&k7), Some("pop".into()));
    // Navigating the other deck's group to it shows nothing there: `on`.
    w.surfaces
        .navigate(OTHER, "popover", Some(POP), Origin::system())
        .unwrap();
    assert_eq!(
        w.shared
            .state
            .value(&path(&["other", "controls", "k7", "component"])),
        Some("cams".into())
    );

    w.surfaces
        .navigate(DECK, "popover", None, Origin::system())
        .unwrap();
    assert_eq!(w.shared.state.value(&k7), Some("cams".into()));
    assert!(
        w.surfaces
            .navigate(DECK, "pop over", None, Origin::system())
            .is_err()
    );
}

fn bound(id: &str, layer: &str, control: &str, json: serde_json::Value) -> ControlBinding {
    let mut b = json!({ "id": id, "layer": layer, "control": control });
    b.as_object_mut()
        .unwrap()
        .extend(json.as_object().unwrap().clone());
    ControlBinding::parse(&b).unwrap()
}

/// The milestone's `cascade: through` shift: it binds only `label` and
/// claims input, so held keys keep the base layer's background, and k3's
/// `look` names where each property comes from. A key it covers but doesn't
/// bind passes input down. A lock layer that binds a label and blocks input
/// leaves the look live and takes presses away from the page.
#[tokio::test]
async fn a_through_layer_sets_only_what_it_binds() {
    let w = world();
    w.surfaces
        .set_layer(layer(
            SHIFT,
            "shift",
            &[3, 4],
            json!({
                "level": "over", "cascade": "through",
                "when": { "while": { "expr": "self.controls.k14.pressed" } },
            }),
        ))
        .unwrap();
    w.surfaces
        .set_layer(layer(
            MODE,
            "lock",
            &[3, 4],
            json!({
                "level": "over", "rank": 50, "cascade": "through",
                "when": { "while": { "ref": { "root": "vars", "path": ["locked"] } } },
            }),
        ))
        .unwrap();
    let tally = json!({ "props": { "label": "Cam 3", "background": "#e02020" } });
    w.surfaces.set_binding(bound("b-cam3", CAMS, "k3", tally));
    w.surfaces.set_binding(bound(
        "b-shift3",
        SHIFT,
        "k3",
        json!({ "props": { "label": "Wide" }, "input": "claim" }),
    ));
    w.surfaces.set_binding(bound(
        "b-lock3",
        MODE,
        "k3",
        json!({ "props": { "label": "Locked" }, "input": "block" }),
    ));
    let red = |label: &str| Look {
        label: label.into(),
        background: Rgb(0xe0, 0x20, 0x20),
        ..Look::default()
    };

    let mut link = w.surfaces.attach(DECK, deck()).unwrap();
    w.surfaces.flush().await;
    assert_eq!(draws(&mut link)["k3"], image(red("Cam 3")));
    let at = |control: &str, leaf: &str| path(&["deck", "controls", control, leaf]);
    let look_of = |pairs: &[(&str, &str)]| {
        Value::Map(
            pairs
                .iter()
                .map(|(p, l)| (Value::from(*p), Value::from(*l)))
                .collect(),
        )
    };
    assert_eq!(
        w.shared.state.value(&at("k3", "look")),
        Some(look_of(&[("background", "cams"), ("label", "cams")]))
    );

    // Held: the label is Shift's, the background still Cameras'.
    link.input
        .send(press("k14", InputKind::Down))
        .await
        .unwrap();
    until(&w, &at("k3", "component"), "shift".into()).await;
    assert_eq!(
        w.shared.state.value(&at("k3", "look")),
        Some(look_of(&[("background", "cams"), ("label", "shift")]))
    );
    assert_eq!(
        w.shared.state.value(&at("k4", "component")),
        Some("cams".into()),
        "k4 is covered but not bound: its input passes down"
    );
    w.surfaces.flush().await;
    assert_eq!(draws(&mut link)["k3"], image(red("Wide")));
    link.input.send(press("k14", InputKind::Up)).await.unwrap();
    until(&w, &at("k3", "component"), "cams".into()).await;

    // Locked: presses are the lock layer's, so nothing on Cameras sees them,
    // and the key's look stays live.
    let meta = covalence_core::state::Meta {
        ts: 0,
        seq: 1,
        quality: covalence_core::state::Quality::Good,
        source: covalence_core::state::Source::Core,
    };
    let locked = StatePath::new("vars", vec![PathSegment::Key("locked".into())]);
    w.shared
        .state
        .mutate(|tree| ((), tree.set(&locked, &Value::from(true), &meta)));
    assert_eq!(
        w.shared.state.value(&at("k3", "component")),
        Some("lock".into())
    );
    w.surfaces.flush().await;
    assert_eq!(draws(&mut link)["k3"], image(red("Locked")));
    link.input.send(press("k3", InputKind::Down)).await.unwrap();
    link.input.send(press("k3", InputKind::Up)).await.unwrap();
    until(&w, &at("k3", "pressed"), false.into()).await;
    let layers_pressed: Vec<Option<String>> = w
        .shared
        .journal
        .range(1, usize::MAX)
        .unwrap()
        .into_iter()
        .filter_map(|e| match &e.record {
            Record::Event(ev) if ev.control.as_deref() == Some("k3") && ev.event == "press" => {
                Some(ev.component.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(layers_pressed, [Some(MODE.to_owned())]);
}

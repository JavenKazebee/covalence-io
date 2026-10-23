//! Surfaces in the core, driven by a simulated 15-key deck the way the web
//! surface drives them: attach with controls, send input with the host's
//! timestamps, receive key images. Input becomes the controls' input state
//! and events (surfaces spec §4). On a paused clock, so `long` and `hold`
//! timers run instantly.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use covalence_core::Shared;
use covalence_core::engine::{ActionOutcome, ActionRequest, Actions, CoreError};
use covalence_core::expr::QuickJs;
use covalence_core::flow::{Flow, Runtime};
use covalence_core::journal::{
    ControlEvent, Entry, InputEventRecord, InputKind, Journal, JournalOp, Origin, Record, Seq,
    StatePatchRecord,
};
use covalence_core::state::StatePath;
use covalence_core::surface::gesture::Timing;
use covalence_core::surface::{
    Attach, AttachError, ControlDef, Draw, ImageDef, Input, InputCap, InputCapKind, InputValue,
    Layer, LayerControl, Link, OutputCap, OutputCapKind, Placement, ShowError, Surface, Surfaces,
};
use covalence_protocol::Value;
use covalence_protocol::state::PathSegment;
use serde_json::json;
use tiny_skia::Pixmap;

const DECK: &str = "01JC5URFACE000000000000001";
const CAMS: &str = "01JC5PAGE00000000000000001";
const AUDIO: &str = "01JC5PAGE00000000000000002";
const WIDE: &str = "01JC5PAGE00000000000000003";

mod common;

/// Flows here only `set` variables, so nothing asks for an action.
struct NoActions;

impl Actions for NoActions {
    fn request_action(
        &self,
        req: ActionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ActionOutcome, CoreError>> + Send + '_>> {
        Box::pin(async move { Err(CoreError::UnknownConnection(req.conn)) })
    }
}

struct World {
    shared: Shared,
    surfaces: Surfaces,
}

fn world() -> World {
    let shared = Shared::new(Journal::in_memory(10_000));
    let surfaces = Surfaces::new(shared.clone(), Timing::default());
    surfaces
        .set_surface(Surface {
            id: DECK.into(),
            name: "FOH deck".into(),
            slug: "fohDeck".into(),
            model: "virtual.deck15".into(),
            serial: None,
            start_layer: Some(CAMS.into()),
        })
        .unwrap();
    for (id, slug, cols, rows) in [
        (CAMS, "cams", 5, 3),
        (AUDIO, "audio", 5, 3),
        (WIDE, "wide", 8, 4),
    ] {
        surfaces
            .set_layer(common::deck_layer(id, slug, cols, rows))
            .unwrap();
    }
    World { shared, surfaces }
}

/// A 5×3 deck of 72px keys, as the virtual deck declares it (at 144px).
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

/// A key, a dial that also presses, and a touch-sensitive fader.
fn mixer() -> Attach {
    let control = |id: &str, x, inputs| ControlDef {
        id: id.into(),
        label: None,
        at: Placement::at(x, 0),
        inputs,
        outputs: Vec::new(),
    };
    Attach {
        model: "test.mixer".into(),
        serial: None,
        name: "Mixer".into(),
        controls: vec![
            control("k0", 0, vec![InputCap::Momentary { velocity: true }]),
            control(
                "dial",
                1,
                vec![InputCap::Relative, InputCap::Momentary { velocity: false }],
            ),
            control(
                "fader",
                2,
                vec![InputCap::Absolute { axes: None }, InputCap::Touch],
            ),
        ],
    }
}

/// `surface.fohDeck.current.main`: the layer shown.
fn current(shared: &Shared) -> Option<Value> {
    let path = StatePath::new(
        "surface",
        ["fohDeck", "current", "main"]
            .into_iter()
            .map(|k| PathSegment::Key(k.into()))
            .collect(),
    );
    shared.state.value(&path)
}

fn state(shared: &Shared, leaf: &str) -> Option<Value> {
    let path = StatePath::new(
        "surface",
        vec![
            PathSegment::Key("fohDeck".into()),
            PathSegment::Key(leaf.into()),
        ],
    );
    shared.state.value(&path)
}

/// `surface.fohDeck.controls.<control>.<leaf>`.
fn control_path(control: &str, leaf: &str) -> StatePath {
    StatePath::new(
        "surface",
        ["fohDeck", "controls", control, leaf]
            .into_iter()
            .map(|k| PathSegment::Key(k.into()))
            .collect(),
    )
}

fn control_state(shared: &Shared, control: &str, leaf: &str) -> Option<Value> {
    shared.state.value(&control_path(control, leaf))
}

fn entries(shared: &Shared) -> Vec<Arc<Entry>> {
    shared.journal.range(1, 10_000).unwrap()
}

fn events(shared: &Shared) -> Vec<(Seq, InputEventRecord, Option<Seq>)> {
    entries(shared)
        .iter()
        .filter_map(|e| match &e.record {
            Record::InputEvent(g) => Some((e.seq, g.clone(), e.cause)),
            _ => None,
        })
        .collect()
}

fn names(shared: &Shared) -> Vec<String> {
    events(shared)
        .iter()
        .map(|(_, g, _)| match g.delta {
            Some(d) => format!("{}:{}{d}", g.control, g.event.as_str()),
            None => format!("{}:{}", g.control, g.event.as_str()),
        })
        .collect()
}

/// The `state.patch`es that wrote control input, after the attach's.
fn input_patches(shared: &Shared) -> Vec<(Seq, StatePatchRecord, Option<Seq>)> {
    entries(shared)
        .iter()
        .filter_map(|e| match &e.record {
            Record::StatePatch(p) if p.device_ts.is_some() => Some((e.seq, p.clone(), e.cause)),
            _ => None,
        })
        .collect()
}

async fn send(link: &Link, control: &str, kind: InputKind, ts: f64) {
    send_value(link, control, kind, None, ts).await;
}

async fn send_value(
    link: &Link,
    control: &str,
    kind: InputKind,
    value: Option<InputValue>,
    ts: f64,
) {
    link.input
        .send(Input {
            control: control.into(),
            kind,
            value,
            ts,
        })
        .await
        .unwrap();
}

/// A flow that sets `vars.<name>` when `when` crosses `edge`.
fn on_state(id: &str, when: &str, edge: &str, name: &str) -> Flow {
    serde_json::from_value(json!({
        "id": id,
        "name": name,
        "slug": name,
        "enabled": true,
        "triggers": [{ "kind": "state", "when": when, "edge": edge }],
        "concurrency": "parallel",
        "body": [{ "id": "note", "kind": "set", "path": format!("vars.{name}"), "value": "true" }],
    }))
    .unwrap()
}

fn var(shared: &Shared, name: &str) -> Option<Value> {
    shared
        .state
        .value(&StatePath::new("vars", vec![PathSegment::Key(name.into())]))
}

fn runtime(shared: &Shared) -> Runtime {
    Runtime::start(
        shared.clone(),
        Arc::new(NoActions),
        Arc::new(QuickJs::default()),
    )
}

/// Lets the surface's task, the flow runtime and the binding worker catch up.
async fn settle(surfaces: &Surfaces) {
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    tokio::time::sleep(Duration::from_millis(1)).await;
    surfaces.flush().await;
}

fn take_draws(link: &mut Link) -> Vec<Draw> {
    let mut out = Vec::new();
    while let Ok(draw) = link.draws.try_recv() {
        out.push(draw);
    }
    out
}

#[tokio::test(start_paused = true)]
async fn attaching_publishes_state_and_draws_every_key() {
    let w = world();
    let mut link = w.surfaces.attach(DECK, deck()).unwrap();
    assert_eq!(state(&w.shared, "connected"), Some(true.into()));
    assert_eq!(state(&w.shared, "model"), Some("virtual.deck15".into()));
    assert_eq!(current(&w.shared), Some("cams".into()));
    assert_eq!(w.surfaces.shown(DECK).as_deref(), Some(CAMS));

    let patch = entries(&w.shared).pop().unwrap();
    assert_eq!(
        patch.origin,
        Origin::Surface {
            surface: DECK.into(),
            orbital: None
        }
    );
    assert!(matches!(patch.record, Record::StatePatch(_)));
    // Every key starts released.
    assert_eq!(
        control_state(&w.shared, "k0", "pressed"),
        Some(false.into())
    );
    assert_eq!(control_state(&w.shared, "k0", "velocity"), None);

    w.surfaces.flush().await;
    let draws = take_draws(&mut link);
    assert_eq!(draws.len(), 15);
    assert_eq!(draws[0].control, "k0");
    let key = Pixmap::decode_png(&draws[0].image).unwrap();
    assert_eq!((key.width(), key.height()), (72, 72));
}

#[tokio::test(start_paused = true)]
async fn attaching_checks_the_surface_and_its_controls() {
    let w = world();
    assert!(matches!(
        w.surfaces.attach("nope", deck()),
        Err(AttachError::UnknownSurface(_))
    ));
    let refused = |change: &dyn Fn(&mut Attach), why: &str| {
        let mut attach = deck();
        change(&mut attach);
        match w.surfaces.attach(DECK, attach) {
            Err(AttachError::Invalid(message)) => {
                assert!(message.contains(why), "{message:?} doesn't say {why:?}")
            }
            Err(e) => panic!("{e}"),
            Ok(_) => panic!("attached with {why}"),
        }
    };
    refused(&|a| a.controls[3].id = "k2".into(), "appears twice");
    refused(&|a| a.controls[3].at = Placement::at(2, 0), "overlap");
    refused(
        &|a| {
            a.controls[14].at = Placement {
                w: Some(2),
                ..Placement::at(3, 2)
            }
        },
        "overlap",
    );
    refused(
        &|a| {
            a.controls[3].inputs.clear();
            a.controls[3].outputs.clear();
        },
        "no inputs or outputs",
    );
    refused(&|a| a.controls[3].at.w = Some(0), "no size");
    refused(
        &|a| {
            a.controls[3]
                .inputs
                .push(InputCap::Momentary { velocity: true })
        },
        "two momentary inputs",
    );
    refused(
        &|a| {
            if let OutputCap::Image(image) = &mut a.controls[3].outputs[0] {
                image.w = 0;
            }
        },
        "0×72 image",
    );
    assert!(!w.surfaces.is_attached(DECK));

    // `at` is required, and IDs are free-form.
    let mut json = serde_json::to_value(deck()).unwrap();
    json["controls"][0].as_object_mut().unwrap().remove("at");
    assert!(serde_json::from_value::<Attach>(json).is_err());
    let mut named = deck();
    named.controls[3].id = "pad.36".into();
    assert!(w.surfaces.attach(DECK, named).is_ok());
}

#[tokio::test(start_paused = true)]
async fn a_press_patches_input_state_then_journals_events() {
    let w = world();
    let link = w.surfaces.attach(DECK, deck()).unwrap();
    send(&link, "k3", InputKind::Down, 1000.0).await;
    settle(&w.surfaces).await;
    assert_eq!(control_state(&w.shared, "k3", "pressed"), Some(true.into()));
    send(&link, "k3", InputKind::Up, 1060.0).await;
    settle(&w.surfaces).await;
    assert_eq!(
        control_state(&w.shared, "k3", "pressed"),
        Some(false.into())
    );

    let patches = input_patches(&w.shared);
    assert_eq!(patches.len(), 2);
    assert_eq!(patches[0].1.device_ts, Some(1000.0));
    assert!(
        matches!(&patches[0].1.ops[..], [JournalOp::Set { path, value }]
        if *path == control_path("k3", "pressed") && *value == Value::from(true))
    );
    let down = w.shared.journal.get(patches[0].0).unwrap().unwrap();
    assert!(matches!(down.origin, Origin::Surface { .. }));

    let found = events(&w.shared);
    assert_eq!(names(&w.shared), ["k3:press", "k3:release"]);
    assert_eq!(found[0].1.layer.as_deref(), Some(CAMS));
    assert_eq!(found[0].1.surface, DECK);
    assert_eq!(
        found[0].2,
        Some(patches[0].0),
        "press is caused by the patch"
    );
    assert_eq!(
        found[1].2,
        Some(patches[1].0),
        "release is caused by the patch"
    );
}

#[tokio::test(start_paused = true)]
async fn holding_a_key_fires_long_and_hold_from_the_timer() {
    let w = world();
    let link = w.surfaces.attach(DECK, deck()).unwrap();
    send(&link, "k0", InputKind::Down, 0.0).await;
    settle(&w.surfaces).await;
    let down = events(&w.shared)[0].2;
    tokio::time::sleep(Duration::from_millis(700)).await;
    settle(&w.surfaces).await;
    assert_eq!(
        names(&w.shared),
        ["k0:press", "k0:long", "k0:hold1", "k0:hold2"]
    );
    for (_, _, cause) in events(&w.shared) {
        assert_eq!(cause, down, "timed events are caused by the press");
    }
}

#[tokio::test(start_paused = true)]
async fn a_release_carries_the_layer_of_its_press() {
    let w = world();
    let link = w.surfaces.attach(DECK, deck()).unwrap();
    send(&link, "k0", InputKind::Down, 0.0).await;
    settle(&w.surfaces).await;
    w.surfaces
        .show(DECK, Some(AUDIO), Origin::system())
        .unwrap();
    send(&link, "k0", InputKind::Up, 50.0).await;
    settle(&w.surfaces).await;
    let layers: Vec<Option<String>> = events(&w.shared).into_iter().map(|e| e.1.layer).collect();
    assert_eq!(layers, [Some(CAMS.into()), Some(CAMS.into())]);
}

#[tokio::test(start_paused = true)]
async fn input_from_an_unknown_control_is_ignored() {
    let w = world();
    let link = w.surfaces.attach(DECK, deck()).unwrap();
    send(&link, "k99", InputKind::Down, 0.0).await;
    settle(&w.surfaces).await;
    assert!(input_patches(&w.shared).is_empty());
    assert!(events(&w.shared).is_empty());
}

#[tokio::test(start_paused = true)]
async fn input_a_control_cant_give_is_dropped() {
    let w = world();
    let link = w.surfaces.attach(DECK, deck()).unwrap();
    send_value(&link, "k0", InputKind::Turn, Some(1.0.into()), 0.0).await;
    send_value(&link, "k0", InputKind::Value, Some(0.5.into()), 0.0).await;
    send_value(&link, "k0", InputKind::Touch, Some(1.0.into()), 0.0).await;
    // An `up` with nothing pressed changes nothing.
    send(&link, "k0", InputKind::Up, 0.0).await;
    settle(&w.surfaces).await;
    assert!(input_patches(&w.shared).is_empty());
    assert!(events(&w.shared).is_empty());
}

#[tokio::test(start_paused = true)]
async fn dials_faders_and_touch_have_state_and_events() {
    let w = world();
    let link = w.surfaces.attach(DECK, mixer()).unwrap();
    // No layer fits a mixer.
    assert_eq!(current(&w.shared), Some(Value::Nil));
    assert_eq!(control_state(&w.shared, "dial", "detents"), Some(0.into()));
    assert_eq!(
        control_state(&w.shared, "dial", "pressed"),
        Some(false.into())
    );
    assert_eq!(
        control_state(&w.shared, "fader", "touched"),
        Some(false.into())
    );
    assert_eq!(control_state(&w.shared, "fader", "value"), None);

    send_value(&link, "k0", InputKind::Down, Some(0.8.into()), 0.0).await;
    send_value(&link, "dial", InputKind::Turn, Some(3.0.into()), 10.0).await;
    send_value(&link, "dial", InputKind::Turn, Some((-1.0).into()), 20.0).await;
    // Not a whole number of detents.
    send_value(&link, "dial", InputKind::Turn, Some(0.5.into()), 25.0).await;
    send_value(&link, "fader", InputKind::Touch, Some(1.0.into()), 30.0).await;
    send_value(&link, "fader", InputKind::Value, Some(0.25.into()), 40.0).await;
    // The same value again changes nothing; out of range is clamped.
    send_value(&link, "fader", InputKind::Value, Some(0.25.into()), 45.0).await;
    send_value(&link, "fader", InputKind::Value, Some(1.5.into()), 50.0).await;
    settle(&w.surfaces).await;

    assert_eq!(control_state(&w.shared, "k0", "velocity"), Some(0.8.into()));
    assert_eq!(control_state(&w.shared, "dial", "detents"), Some(2.into()));
    assert_eq!(
        control_state(&w.shared, "fader", "touched"),
        Some(true.into())
    );
    assert_eq!(control_state(&w.shared, "fader", "value"), Some(1.0.into()));
    assert_eq!(
        names(&w.shared),
        [
            "k0:press",
            "dial:turn3",
            "dial:turn-1",
            "fader:touch",
            "fader:change",
            "fader:change"
        ]
    );
    assert_eq!(events(&w.shared)[4].1.value, Some(0.25.into()));

    drop(link);
    settle(&w.surfaces).await;
    assert_eq!(
        control_state(&w.shared, "k0", "pressed"),
        Some(false.into())
    );
    assert_eq!(
        control_state(&w.shared, "fader", "touched"),
        Some(false.into())
    );
    assert_eq!(names(&w.shared)[6..], ["fader:untouch", "k0:release"]);
}

#[tokio::test(start_paused = true)]
async fn detaching_disconnects_and_releases_held_keys() {
    let w = world();
    let link = w.surfaces.attach(DECK, deck()).unwrap();
    send(&link, "k2", InputKind::Down, 0.0).await;
    settle(&w.surfaces).await;
    drop(link);
    settle(&w.surfaces).await;

    assert!(!w.surfaces.is_attached(DECK));
    assert_eq!(state(&w.shared, "connected"), Some(false.into()));
    assert_eq!(
        current(&w.shared),
        Some("cams".into()),
        "kept while detached"
    );
    assert_eq!(
        control_state(&w.shared, "k2", "pressed"),
        Some(false.into())
    );
    let found = events(&w.shared);
    let (_, release, cause) = found.last().unwrap();
    assert_eq!(release.event, ControlEvent::Release);
    assert_eq!(release.layer.as_deref(), Some(CAMS));
    // release → the patch that released k2 → the patch that disconnected.
    let released = w.shared.journal.get(cause.unwrap()).unwrap().unwrap();
    let Record::StatePatch(patch) = &released.record else {
        panic!("release caused by {released:?}");
    };
    assert!(matches!(&patch.ops[..], [JournalOp::Set { path, value }]
        if *path == control_path("k2", "pressed") && *value == Value::from(false)));
    let disconnected = w
        .shared
        .journal
        .get(released.cause.unwrap())
        .unwrap()
        .unwrap();
    let Record::StatePatch(patch) = &disconnected.record else {
        panic!("release caused by {disconnected:?}");
    };
    assert!(matches!(&patch.ops[0], JournalOp::Set { value, .. } if *value == Value::from(false)));
}

#[tokio::test(start_paused = true)]
async fn attaching_again_resets_input_state() {
    let w = world();
    let first = w.surfaces.attach(DECK, mixer()).unwrap();
    send(&first, "k0", InputKind::Down, 0.0).await;
    send_value(&first, "dial", InputKind::Turn, Some(4.0.into()), 0.0).await;
    send_value(&first, "fader", InputKind::Value, Some(0.5.into()), 0.0).await;
    settle(&w.surfaces).await;
    let _second = w.surfaces.attach(DECK, mixer()).unwrap();
    settle(&w.surfaces).await;
    assert_eq!(
        control_state(&w.shared, "k0", "pressed"),
        Some(false.into())
    );
    assert_eq!(control_state(&w.shared, "dial", "detents"), Some(0.into()));
    assert_eq!(control_state(&w.shared, "fader", "value"), None);
    // A control the new attach doesn't have is gone.
    let _third = w.surfaces.attach(DECK, deck()).unwrap();
    assert_eq!(control_state(&w.shared, "dial", "detents"), None);
}

#[tokio::test(start_paused = true)]
async fn a_second_attach_replaces_the_first() {
    let w = world();
    let mut first = w.surfaces.attach(DECK, deck()).unwrap();
    w.surfaces.flush().await;
    take_draws(&mut first);
    let second = w.surfaces.attach(DECK, deck()).unwrap();
    settle(&w.surfaces).await;
    assert!(
        first.draws.recv().await.is_none(),
        "the first link is closed"
    );
    // The first transport hanging up later doesn't detach the second.
    drop(first);
    settle(&w.surfaces).await;
    assert_eq!(state(&w.shared, "connected"), Some(true.into()));
    send(&second, "k1", InputKind::Down, 0.0).await;
    settle(&w.surfaces).await;
    assert_eq!(names(&w.shared), ["k1:press"]);
}

#[tokio::test(start_paused = true)]
async fn showing_a_layer_checks_it_fits() {
    let w = world();
    let mut link = w.surfaces.attach(DECK, deck()).unwrap();
    w.surfaces.flush().await;
    take_draws(&mut link);
    let user = Origin::User {
        user: "u1".into(),
        client: "test".into(),
    };

    w.surfaces.show(DECK, Some(AUDIO), user.clone()).unwrap();
    assert_eq!(current(&w.shared), Some("audio".into()));
    assert_eq!(entries(&w.shared).last().unwrap().origin, user);
    w.surfaces.flush().await;
    // The audio layer has no bindings either, so nothing looks different.
    assert_eq!(take_draws(&mut link).len(), 0);

    match w.surfaces.show(DECK, Some(WIDE), user.clone()) {
        Err(ShowError::DoesNotFit { layer, missing }) => {
            assert_eq!(layer, WIDE);
            assert_eq!(missing.len(), 32 - 15);
            assert_eq!(missing[0], "k15");
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        w.surfaces.show(DECK, Some("nope"), user.clone()),
        Err(ShowError::UnknownLayer(_))
    ));
    assert_eq!(current(&w.shared), Some("audio".into()));

    w.surfaces.show(DECK, None, user).unwrap();
    assert_eq!(current(&w.shared), Some(Value::Nil));

    // Input with no layer shown is journaled, but can't match a trigger.
    send(&link, "k0", InputKind::Down, 0.0).await;
    settle(&w.surfaces).await;
    assert_eq!(events(&w.shared)[0].1.layer, None);
}

#[tokio::test(start_paused = true)]
async fn reattaching_shows_the_layer_it_showed_last() {
    let w = world();
    let link = w.surfaces.attach(DECK, deck()).unwrap();
    w.surfaces
        .show(DECK, Some(AUDIO), Origin::system())
        .unwrap();
    drop(link);
    settle(&w.surfaces).await;
    let _link = w.surfaces.attach(DECK, deck()).unwrap();
    assert_eq!(current(&w.shared), Some("audio".into()));
}

#[tokio::test(start_paused = true)]
async fn removing_a_layer_or_renaming_a_surface_updates_state() {
    let w = world();
    let _link = w.surfaces.attach(DECK, deck()).unwrap();
    w.surfaces.remove_layer(CAMS).unwrap();
    assert_eq!(current(&w.shared), Some(Value::Nil));

    w.surfaces
        .set_surface(Surface {
            id: DECK.into(),
            name: "FOH deck".into(),
            slug: "foh".into(),
            model: "virtual.deck15".into(),
            serial: None,
            start_layer: None,
        })
        .unwrap();
    assert_eq!(state(&w.shared, "connected"), None, "the old slug is gone");
    let moved = StatePath::new(
        "surface",
        vec![
            PathSegment::Key("foh".into()),
            PathSegment::Key("connected".into()),
        ],
    );
    assert_eq!(w.shared.state.value(&moved), Some(true.into()));

    w.surfaces.remove_surface(DECK).unwrap();
    assert_eq!(w.shared.state.value(&moved), None);
    assert!(!w.surfaces.is_attached(DECK));
}

#[tokio::test(start_paused = true)]
async fn a_control_trigger_runs_its_flow_and_why_reaches_the_input_patch() {
    let w = world();
    let runtime = Runtime::start(
        w.shared.clone(),
        Arc::new(NoActions),
        Arc::new(QuickJs::default()),
    );
    let flow: Flow = serde_json::from_value(json!({
        "id": "01JC5FLOW00000000000000001",
        "name": "Note the press",
        "slug": "notePress",
        "enabled": true,
        "triggers": [{
            "kind": "control",
            "control": { "layer": CAMS, "control": "k4" },
            "event": "press",
        }],
        "concurrency": "parallel",
        "body": [{
            "id": "note",
            "kind": "set",
            "path": "vars.pressed",
            "value": "`${trigger.control} on ${trigger.layer}`",
        }],
    }))
    .unwrap();
    runtime.set_flow(flow);

    let link = w.surfaces.attach(DECK, deck()).unwrap();
    // Another key, and a release, don't match.
    send(&link, "k3", InputKind::Down, 0.0).await;
    send(&link, "k3", InputKind::Up, 50.0).await;
    send(&link, "k4", InputKind::Down, 1000.0).await;
    settle(&w.surfaces).await;

    let pressed = StatePath::new("vars", vec![PathSegment::Key("pressed".into())]);
    let value = w.shared.state.value(&pressed);
    assert_eq!(value, Some(format!("k4 on {CAMS}").as_str().into()));

    let set_seq = w.shared.state.leaf(&pressed).unwrap().meta.seq;
    let kinds: Vec<&str> = w
        .shared
        .journal
        .why(set_seq)
        .unwrap()
        .iter()
        .map(|e| e.record.kind())
        .collect();
    assert_eq!(
        kinds,
        [
            "state.patch",
            "run.step",
            "run.started",
            "trigger.matched",
            "input.event",
            "state.patch"
        ]
    );
    let matched = entries(&w.shared)
        .into_iter()
        .filter(|e| matches!(e.record, Record::TriggerMatched(_)))
        .collect::<Vec<_>>();
    assert_eq!(matched.len(), 1, "only k4's press matched");
    assert!(matches!(matched[0].origin, Origin::Surface { .. }));
    runtime.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_control_trigger_only_matches_its_layer() {
    let w = world();
    let runtime = Runtime::start(
        w.shared.clone(),
        Arc::new(NoActions),
        Arc::new(QuickJs::default()),
    );
    let flow: Flow = serde_json::from_value(json!({
        "id": "01JC5FLOW00000000000000002",
        "name": "Audio only",
        "slug": "audioOnly",
        "enabled": true,
        "triggers": [{
            "kind": "control",
            "control": { "layer": AUDIO, "control": "k0" },
            "event": "double",
        }],
        "concurrency": "parallel",
        "body": [{ "id": "note", "kind": "set", "path": "vars.doubled", "value": "true" }],
    }))
    .unwrap();
    runtime.set_flow(flow);
    let link = w.surfaces.attach(DECK, deck()).unwrap();
    let doubled = StatePath::new("vars", vec![PathSegment::Key("doubled".into())]);

    for (ts, layer) in [(0.0, CAMS), (5000.0, AUDIO)] {
        w.surfaces
            .show(DECK, Some(layer), Origin::system())
            .unwrap();
        send(&link, "k0", InputKind::Down, ts).await;
        send(&link, "k0", InputKind::Up, ts + 50.0).await;
        send(&link, "k0", InputKind::Down, ts + 150.0).await;
        send(&link, "k0", InputKind::Up, ts + 200.0).await;
        settle(&w.surfaces).await;
        let expected = (layer == AUDIO).then(|| Value::from(true));
        assert_eq!(w.shared.state.value(&doubled), expected, "on {layer}");
    }
    runtime.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_tap_shorter_than_a_turn_still_gives_both_edges() {
    let w = world();
    let runtime = runtime(&w.shared);
    let pressed = "surface.fohDeck.controls.k0.pressed";
    runtime.set_flow(on_state(
        "01JC5FLOW00000000000000003",
        pressed,
        "rising",
        "rose",
    ));
    runtime.set_flow(on_state(
        "01JC5FLOW00000000000000004",
        pressed,
        "falling",
        "fell",
    ));
    let link = w.surfaces.attach(DECK, deck()).unwrap();
    settle(&w.surfaces).await;

    // Both arrive before anything else runs.
    send(&link, "k0", InputKind::Down, 0.0).await;
    send(&link, "k0", InputKind::Up, 5.0).await;
    settle(&w.surfaces).await;
    assert_eq!(var(&w.shared, "rose"), Some(true.into()));
    assert_eq!(var(&w.shared, "fell"), Some(true.into()));

    // The rising edge's cause is the press's patch.
    let rose = entries(&w.shared)
        .into_iter()
        .find(|e| matches!(&e.record, Record::TriggerMatched(t) if t.flow == "01JC5FLOW00000000000000003"))
        .unwrap();
    assert_eq!(rose.cause, Some(input_patches(&w.shared)[0].0));
    runtime.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_state_trigger_on_two_keys_is_a_chord() {
    let w = world();
    let runtime = runtime(&w.shared);
    runtime.set_flow(on_state(
        "01JC5FLOW00000000000000005",
        "surface.fohDeck.controls.k3.pressed && surface.fohDeck.controls.k4.pressed",
        "rising",
        "chord",
    ));
    let link = w.surfaces.attach(DECK, deck()).unwrap();
    settle(&w.surfaces).await;

    send(&link, "k3", InputKind::Down, 0.0).await;
    send(&link, "k3", InputKind::Up, 50.0).await;
    send(&link, "k4", InputKind::Down, 100.0).await;
    settle(&w.surfaces).await;
    assert_eq!(var(&w.shared, "chord"), None, "one at a time isn't a chord");

    send(&link, "k3", InputKind::Down, 150.0).await;
    settle(&w.surfaces).await;
    assert_eq!(var(&w.shared, "chord"), Some(true.into()));
    runtime.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_layer_fits_by_capability_not_by_size() {
    let w = world();
    let _link = w.surfaces.attach(DECK, mixer()).unwrap();
    let layer = |id: &str, slug: &str, controls: Vec<LayerControl>| Layer {
        id: id.into(),
        name: slug.into(),
        slug: slug.into(),
        controls,
    };
    let control = |id: &str, inputs: Vec<InputCapKind>, outputs: Vec<OutputCapKind>| LayerControl {
        id: id.into(),
        at: None,
        inputs,
        outputs,
    };
    // A dial and a key: fewer controls than the mixer has is fine.
    const DIALS: &str = "01JC5PAGE00000000000000004";
    w.surfaces
        .set_layer(layer(
            DIALS,
            "dials",
            vec![
                control("dial", vec![InputCapKind::Relative], vec![]),
                control("k0", vec![InputCapKind::Momentary], vec![]),
            ],
        ))
        .unwrap();
    w.surfaces
        .show(DECK, Some(DIALS), Origin::system())
        .unwrap();
    assert_eq!(current(&w.shared), Some("dials".into()));

    // A dial that turns, but the mixer's `k0` has no image.
    const IMAGES: &str = "01JC5PAGE00000000000000005";
    w.surfaces
        .set_layer(layer(
            IMAGES,
            "images",
            vec![
                control("dial", vec![InputCapKind::Relative], vec![]),
                control("k0", vec![], vec![OutputCapKind::Image]),
                control("k9", vec![], vec![]),
            ],
        ))
        .unwrap();
    match w.surfaces.show(DECK, Some(IMAGES), Origin::system()) {
        Err(ShowError::DoesNotFit { missing, .. }) => assert_eq!(missing, ["k0", "k9"]),
        other => panic!("{other:?}"),
    }
    assert_eq!(current(&w.shared), Some("dials".into()));

    // The surface's controls, as it attached, for "New layer".
    let controls = w.surfaces.controls(DECK).unwrap();
    assert_eq!(controls.len(), 3);
    assert_eq!(
        controls[1].to_layer_control(),
        LayerControl {
            id: "dial".into(),
            at: Some(Placement::at(1, 0)),
            inputs: vec![InputCapKind::Relative, InputCapKind::Momentary],
            outputs: vec![],
        }
    );
    assert_eq!(w.surfaces.controls("nope"), None);
}

#[tokio::test(start_paused = true)]
async fn a_control_trigger_on_a_dial_gets_the_turn() {
    let w = world();
    let runtime = runtime(&w.shared);
    const DIALS: &str = "01JC5PAGE00000000000000004";
    w.surfaces
        .set_layer(Layer {
            id: DIALS.into(),
            name: "Dials".into(),
            slug: "dials".into(),
            controls: vec![LayerControl {
                id: "dial".into(),
                at: None,
                inputs: vec![InputCapKind::Relative],
                outputs: vec![],
            }],
        })
        .unwrap();
    let flow: Flow = serde_json::from_value(json!({
        "id": "01JC5FLOW00000000000000006",
        "name": "Turned",
        "slug": "turned",
        "enabled": true,
        "triggers": [{
            "kind": "control",
            "control": { "layer": DIALS, "control": "dial" },
            "event": "turn",
        }],
        "concurrency": "queue",
        "body": [{ "id": "note", "kind": "set", "path": "vars.turned", "value": "`${trigger.event} ${trigger.delta}`" }],
    }))
    .unwrap();
    runtime.set_flow(flow);
    let link = w.surfaces.attach(DECK, mixer()).unwrap();
    w.surfaces
        .show(DECK, Some(DIALS), Origin::system())
        .unwrap();
    send_value(&link, "dial", InputKind::Turn, Some((-2.0).into()), 0.0).await;
    settle(&w.surfaces).await;
    assert_eq!(var(&w.shared, "turned"), Some("turn -2".into()));
    runtime.shutdown().await;
}

//! Surfaces in the core (M5 Phase 1), driven by a simulated 15-key deck the
//! way the web surface will drive them: attach with controls, send input
//! with the host's timestamps, receive key images. On a paused clock, so
//! `long` and `hold` timers run instantly.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use covalence_core::Shared;
use covalence_core::engine::{ActionOutcome, ActionRequest, Actions, CoreError};
use covalence_core::expr::QuickJs;
use covalence_core::flow::{Flow, Runtime};
use covalence_core::journal::{
    Entry, Gesture, InputGestureRecord, InputKind, InputRawRecord, Journal, JournalOp, Origin,
    Record, Seq,
};
use covalence_core::state::StatePath;
use covalence_core::surface::gesture::Timing;
use covalence_core::surface::{
    Attach, AttachError, ControlDef, ControlKind, Draw, ImageDef, Input, Layout, Link, Page,
    ShowError, Surface, Surfaces,
};
use covalence_protocol::Value;
use covalence_protocol::state::PathSegment;
use serde_json::json;
use tiny_skia::Pixmap;

const DECK: &str = "01JC5URFACE000000000000001";
const CAMS: &str = "01JC5PAGE00000000000000001";
const AUDIO: &str = "01JC5PAGE00000000000000002";
const WIDE: &str = "01JC5PAGE00000000000000003";
const DECK_LAYOUT: Layout = Layout { cols: 5, rows: 3 };

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
            start_page: Some(CAMS.into()),
        })
        .unwrap();
    for (id, slug, layout) in [
        (CAMS, "cams", DECK_LAYOUT),
        (AUDIO, "audio", DECK_LAYOUT),
        (WIDE, "wide", Layout { cols: 8, rows: 4 }),
    ] {
        surfaces
            .set_page(Page {
                id: id.into(),
                name: slug.into(),
                slug: slug.into(),
                layout,
            })
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
                kind: ControlKind::Button,
                x: i % 5,
                y: i / 5,
                w: None,
                h: None,
                image: Some(ImageDef {
                    w: 72,
                    h: 72,
                    format: "png".into(),
                    rotation: 0,
                }),
                pressable: None,
            })
            .collect(),
        layout: DECK_LAYOUT,
    }
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

fn entries(shared: &Shared) -> Vec<Arc<Entry>> {
    shared.journal.range(1, 10_000).unwrap()
}

fn gestures(shared: &Shared) -> Vec<(Seq, InputGestureRecord, Option<Seq>)> {
    entries(shared)
        .iter()
        .filter_map(|e| match &e.record {
            Record::InputGesture(g) => Some((e.seq, g.clone(), e.cause)),
            _ => None,
        })
        .collect()
}

fn names(shared: &Shared) -> Vec<String> {
    gestures(shared)
        .iter()
        .map(|(_, g, _)| match g.delta {
            Some(d) => format!("{}:{}{d}", g.control, g.gesture.as_str()),
            None => format!("{}:{}", g.control, g.gesture.as_str()),
        })
        .collect()
}

async fn send(link: &Link, control: &str, kind: InputKind, ts: f64) {
    link.input
        .send(Input {
            control: control.into(),
            kind,
            value: None,
            ts,
        })
        .await
        .unwrap();
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
    assert_eq!(state(&w.shared, "page"), Some("cams".into()));
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
    let mut bad = deck();
    bad.controls[3].id = "k15".into();
    assert!(matches!(
        w.surfaces.attach(DECK, bad),
        Err(AttachError::Invalid(_))
    ));
    let mut twice = deck();
    twice.controls[3].id = "k2".into();
    assert!(matches!(
        w.surfaces.attach(DECK, twice),
        Err(AttachError::Invalid(_))
    ));
    assert!(!w.surfaces.is_attached(DECK));
}

#[tokio::test(start_paused = true)]
async fn a_press_journals_raw_input_then_the_gesture() {
    let w = world();
    let link = w.surfaces.attach(DECK, deck()).unwrap();
    send(&link, "k3", InputKind::Down, 1000.0).await;
    send(&link, "k3", InputKind::Up, 1060.0).await;
    settle(&w.surfaces).await;

    let all = entries(&w.shared);
    let raw: Vec<(Seq, InputRawRecord)> = all
        .iter()
        .filter_map(|e| match &e.record {
            Record::InputRaw(r) => Some((e.seq, r.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(raw.len(), 2);
    assert_eq!(raw[0].1.kind, InputKind::Down);
    assert_eq!(raw[0].1.device_ts, 1000.0);

    let found = gestures(&w.shared);
    assert_eq!(names(&w.shared), ["k3:press", "k3:release"]);
    assert_eq!(found[0].1.page.as_deref(), Some(CAMS));
    assert_eq!(found[0].1.surface, DECK);
    assert_eq!(found[0].2, Some(raw[0].0), "press is caused by the down");
    assert_eq!(found[1].2, Some(raw[1].0), "release is caused by the up");
}

#[tokio::test(start_paused = true)]
async fn holding_a_key_fires_long_and_hold_from_the_timer() {
    let w = world();
    let link = w.surfaces.attach(DECK, deck()).unwrap();
    send(&link, "k0", InputKind::Down, 0.0).await;
    settle(&w.surfaces).await;
    let down = gestures(&w.shared)[0].2;
    tokio::time::sleep(Duration::from_millis(700)).await;
    settle(&w.surfaces).await;
    assert_eq!(
        names(&w.shared),
        ["k0:press", "k0:long", "k0:hold1", "k0:hold2"]
    );
    for (_, _, cause) in gestures(&w.shared) {
        assert_eq!(cause, down, "timed gestures are caused by the down");
    }
}

#[tokio::test(start_paused = true)]
async fn input_from_an_unknown_control_is_ignored() {
    let w = world();
    let link = w.surfaces.attach(DECK, deck()).unwrap();
    send(&link, "k99", InputKind::Down, 0.0).await;
    settle(&w.surfaces).await;
    assert!(
        !entries(&w.shared)
            .iter()
            .any(|e| matches!(e.record, Record::InputRaw(_)))
    );
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
        state(&w.shared, "page"),
        Some("cams".into()),
        "kept while detached"
    );
    let found = gestures(&w.shared);
    let (_, release, cause) = found.last().unwrap();
    assert_eq!(release.gesture, Gesture::Release);
    let cause = w.shared.journal.get(cause.unwrap()).unwrap().unwrap();
    let Record::StatePatch(patch) = &cause.record else {
        panic!("release caused by {cause:?}");
    };
    assert!(matches!(&patch.ops[0], JournalOp::Set { value, .. } if *value == Value::from(false)));
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
async fn showing_a_page_checks_its_layout() {
    let w = world();
    let mut link = w.surfaces.attach(DECK, deck()).unwrap();
    w.surfaces.flush().await;
    take_draws(&mut link);
    let user = Origin::User {
        user: "u1".into(),
        client: "test".into(),
    };

    w.surfaces.show(DECK, Some(AUDIO), user.clone()).unwrap();
    assert_eq!(state(&w.shared, "page"), Some("audio".into()));
    assert_eq!(entries(&w.shared).last().unwrap().origin, user);
    w.surfaces.flush().await;
    // The audio page has no bindings either, so nothing looks different.
    assert_eq!(take_draws(&mut link).len(), 0);

    assert!(matches!(
        w.surfaces.show(DECK, Some(WIDE), user.clone()),
        Err(ShowError::LayoutMismatch { .. })
    ));
    assert!(matches!(
        w.surfaces.show(DECK, Some("nope"), user.clone()),
        Err(ShowError::UnknownPage(_))
    ));
    assert_eq!(state(&w.shared, "page"), Some("audio".into()));

    w.surfaces.show(DECK, None, user).unwrap();
    assert_eq!(state(&w.shared, "page"), Some(Value::Nil));

    // Input with no page shown is journaled, but can't match a trigger.
    send(&link, "k0", InputKind::Down, 0.0).await;
    settle(&w.surfaces).await;
    assert_eq!(gestures(&w.shared)[0].1.page, None);
}

#[tokio::test(start_paused = true)]
async fn reattaching_shows_the_page_it_showed_last() {
    let w = world();
    let link = w.surfaces.attach(DECK, deck()).unwrap();
    w.surfaces
        .show(DECK, Some(AUDIO), Origin::system())
        .unwrap();
    drop(link);
    settle(&w.surfaces).await;
    let _link = w.surfaces.attach(DECK, deck()).unwrap();
    assert_eq!(state(&w.shared, "page"), Some("audio".into()));
}

#[tokio::test(start_paused = true)]
async fn removing_a_page_or_renaming_a_surface_updates_state() {
    let w = world();
    let _link = w.surfaces.attach(DECK, deck()).unwrap();
    w.surfaces.remove_page(CAMS).unwrap();
    assert_eq!(state(&w.shared, "page"), Some(Value::Nil));

    w.surfaces
        .set_surface(Surface {
            id: DECK.into(),
            name: "FOH deck".into(),
            slug: "foh".into(),
            model: "virtual.deck15".into(),
            serial: None,
            start_page: None,
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
async fn a_gesture_trigger_runs_its_flow_and_why_reaches_the_raw_input() {
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
            "kind": "gesture",
            "control": { "page": CAMS, "control": "k4" },
            "gesture": "press",
        }],
        "concurrency": "parallel",
        "body": [{
            "id": "note",
            "kind": "set",
            "path": "vars.pressed",
            "value": "`${trigger.control} on ${trigger.page}`",
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
            "input.gesture",
            "input.raw"
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
async fn a_gesture_trigger_only_matches_its_page() {
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
            "kind": "gesture",
            "control": { "page": AUDIO, "control": "k0" },
            "gesture": "double",
        }],
        "concurrency": "parallel",
        "body": [{ "id": "note", "kind": "set", "path": "vars.doubled", "value": "true" }],
    }))
    .unwrap();
    runtime.set_flow(flow);
    let link = w.surfaces.attach(DECK, deck()).unwrap();
    let doubled = StatePath::new("vars", vec![PathSegment::Key("doubled".into())]);

    for (ts, page) in [(0.0, CAMS), (5000.0, AUDIO)] {
        w.surfaces.show(DECK, Some(page), Origin::system()).unwrap();
        send(&link, "k0", InputKind::Down, ts).await;
        send(&link, "k0", InputKind::Up, ts + 50.0).await;
        send(&link, "k0", InputKind::Down, ts + 150.0).await;
        send(&link, "k0", InputKind::Up, ts + 200.0).await;
        settle(&w.surfaces).await;
        let expected = (page == AUDIO).then(|| Value::from(true));
        assert_eq!(w.shared.state.value(&doubled), expected, "on {page}");
    }
    runtime.shutdown().await;
}

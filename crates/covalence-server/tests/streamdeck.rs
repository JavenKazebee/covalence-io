//! The Stream Deck plugin, end to end, against a
//! simulated MK.2 at the level of its HID reports, the real Deno plugin host
//! and `@elgato-stream-deck/core`. Needs `deno` on the PATH, and the npm
//! package (fetched on first use).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::http::{Method, StatusCode};
use covalence_devices::fake::{Fake, Mk2};
use covalence_server::{App, Config, DeviceSource};
use serde_json::{Value, json};

mod common;
use common::Server;

const WAIT: Duration = Duration::from_secs(20);
const DECK: &str = "hid:0fd9:0080:CL12K1A00042";

fn start(data: &Path, fake: &Fake) -> Server {
    let app = App::start(Config {
        data_dir: data.to_owned(),
        root: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
        plugins: Config::default_plugins(),
        deno: "deno".into(),
        devices: DeviceSource::Backend(Arc::new(fake.clone())),
    })
    .unwrap();
    Server::with_app(app)
}

/// A component of the MK.2's 15 image keys, with labels on `k0` and `k7`.
async fn component(server: &Server) -> String {
    let controls: Vec<Value> = (0..15)
        .map(|k| {
            json!({
                "id": format!("k{k}"), "at": { "x": k % 5, "y": k / 5 },
                "inputs": ["momentary"], "outputs": ["image"],
            })
        })
        .collect();
    let (status, component) = server
        .post("/components", Server::page("Main", json!(controls)))
        .await;
    assert_eq!(status.as_u16(), 201, "{component}");
    let id = component["component"]["id"].as_str().unwrap().to_owned();
    for (control, label) in [("k0", "'Cam 1'"), ("k7", "'Cam 2'")] {
        let (status, binding) = server
            .try_bind(
                &id,
                json!({ "control": control, "props": { "label": { "expr": label } } }),
            )
            .await;
        assert_eq!(status.as_u16(), 200, "{binding}");
    }
    id
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stream_deck_mk2_becomes_a_surface_by_itself() {
    let data = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let deck = Mk2::plug(&fake, "/dev/hidraw0", "CL12K1A00042");
    let server = start(data.path(), &fake);

    // A connection, a surface, and the deck's details.
    let surface = server
        .eventually(WAIT, "a surface for the deck", || async {
            server
                .get("/surfaces")
                .await
                .as_array()
                .unwrap()
                .first()
                .cloned()
        })
        .await;
    assert_eq!(surface["model"], json!("elgato.original-mk2"));
    assert_eq!(surface["serial"], json!("CL12K1A00042"));
    assert_eq!(surface["name"], json!("Stream Deck MK.2"));
    let conns = server.get("/connections").await;
    let conn = conns
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["plugin"] == "io.covalence.streamdeck")
        .cloned()
        .expect("a connection for the deck");
    assert_eq!(conn["config"]["device"], json!(DECK));
    assert_eq!(conn["mode"], json!("live"));
    assert_eq!(surface["provider"], conn["id"]);
    let alias = conn["alias"].as_str().unwrap();
    server
        .eventually(WAIT, "the deck's details", || async {
            (server.value(&format!("{alias}.firmware")).await == Some(json!("1.01.000")))
                .then_some(())
        })
        .await;
    assert_eq!(
        server.value(&format!("{alias}.model")).await,
        Some(json!("Stream Deck MK.2"))
    );

    // It attached with 15 JPEG keys, turned 180°.
    let id = surface["id"].as_str().unwrap();
    let slug = surface["slug"].as_str().unwrap();
    let controls = server
        .eventually(WAIT, "the deck's controls", || async {
            let (status, controls) = server
                .call(Method::GET, &format!("/surfaces/{id}/controls"), None)
                .await;
            (status == StatusCode::OK).then_some(controls)
        })
        .await;
    let controls = controls.as_array().unwrap();
    assert_eq!(controls.len(), 15);
    assert_eq!(
        controls[7],
        json!({
            "id": "k7", "at": { "x": 2, "y": 1 },
            "inputs": [{ "kind": "momentary" }],
            "outputs": [{ "kind": "image", "w": 72, "h": 72, "format": "jpeg", "rotation": 180 }],
        })
    );

    // A shown component is drawn on the deck as JPEG, each key complete.
    let main = component(&server).await;
    let (status, body) = server
        .post(
            &format!("/surfaces/{id}/current"),
            json!({ "component": main }),
        )
        .await;
    assert_eq!(status.as_u16(), 200, "{body}");
    let images = server
        .eventually(WAIT, "every key drawn", || async {
            let images = deck.images();
            (images.len() == 15 && images[&7].len() >= 2).then_some(images)
        })
        .await;
    let k7 = images[&7].last().unwrap();
    assert_eq!(&k7[..2], [0xff, 0xd8], "a JPEG");
    assert_eq!(&k7[k7.len() - 2..], [0xff, 0xd9], "complete");
    assert_ne!(
        images[&7].last(),
        images[&3].last(),
        "k7 has a label, k3 doesn't"
    );

    // Pressing a key: control state and a journal entry with the deck's time.
    let pressed = format!("surface.{slug}.controls.k7.pressed");
    deck.press(7);
    server
        .eventually(WAIT, "k7 pressed", || async {
            (server.value(&pressed).await == Some(json!(true))).then_some(())
        })
        .await;
    // The press is journaled with when the core read the deck's report.
    let journal = server.get("/journal?limit=1000").await;
    let press = journal
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|e| e.to_string().contains("pressed") && e.to_string().contains("k7"))
        .cloned()
        .expect("the press in the journal");
    let device_ts = press
        .pointer("/data/deviceTs")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("no deviceTs in {press}"));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
        * 1000.0;
    assert!(
        (now - device_ts).abs() < 10_000.0,
        "{device_ts} is the core's clock"
    );
    deck.release(7);
    server
        .eventually(WAIT, "k7 released", || async {
            (server.value(&pressed).await == Some(json!(false))).then_some(())
        })
        .await;

    // Brightness is the plugin's one action.
    let (status, outcome) = server
        .post(
            "/actions",
            json!({ "action": format!("{alias}.brightness"), "params": { "percent": 40 } }),
        )
        .await;
    assert_eq!(
        (status.as_u16(), &outcome["ok"]),
        (200, &json!(true)),
        "{outcome}"
    );
    assert_eq!(deck.brightness(), Some(40));

    // Unplugged and back, in another port: the surface detaches, then
    // attaches and is redrawn.
    let connected = format!("surface.{slug}.connected");
    deck.unplug();
    server
        .eventually(WAIT, "the surface to detach", || async {
            (server.value(&connected).await == Some(json!(false))).then_some(())
        })
        .await;
    let deck = Mk2::plug(&fake, "/dev/hidraw3", "CL12K1A00042");
    server
        .eventually(WAIT, "the surface to attach again", || async {
            (server.value(&connected).await == Some(json!(true))).then_some(())
        })
        .await;
    server
        .eventually(WAIT, "k7 drawn again", || async {
            deck.images()
                .get(&7)
                .is_some_and(|i| !i.is_empty())
                .then_some(())
        })
        .await;
    assert_eq!(server.get("/surfaces").await.as_array().unwrap().len(), 1);
    server.app.clone().shutdown().await;
}

/// The deck's surface, once it's attached.
async fn attached(server: &Server, id: &str) -> Value {
    let slug = server.get(&format!("/surfaces/{id}")).await["slug"].clone();
    let connected = format!("surface.{}.connected", slug.as_str().unwrap());
    server
        .eventually(WAIT, "the deck to attach", || async {
            (server.value(&connected).await == Some(json!(true))).then_some(())
        })
        .await;
    server.get(&format!("/surfaces/{id}")).await
}

/// The Stream Deck connection, once the core has made it.
async fn deck_conn(server: &Server) -> Value {
    server
        .eventually(WAIT, "a connection for the deck", || async {
            server
                .get("/connections")
                .await
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["plugin"] == "io.covalence.streamdeck")
                .cloned()
        })
        .await
}

/// Waits until `deck` has the last image of every key.
async fn all_drawn(deck: &Mk2, since: usize) -> std::collections::BTreeMap<u8, Vec<u8>> {
    let deadline = std::time::Instant::now() + WAIT;
    loop {
        let images = deck.images();
        if images.len() == 15 && images.values().all(|i| i.len() > since) {
            return images
                .into_iter()
                .map(|(k, i)| (k, i.last().unwrap().clone()))
                .collect();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "not every key was drawn"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// The last `brightness` action the journal has, as its `action.requested`.
async fn brightness_requested(server: &Server) -> Option<Value> {
    server
        .get("/journal?limit=1000")
        .await
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|e| e["kind"] == "action.requested" && e.to_string().contains("brightness"))
        .cloned()
}

/// M7's "done when", against the simulated MK.2: a deck given a start component
/// ahead of time shows it; its keys run flows whose `why` goes back to the
/// key; unplugging releases held keys and replugging shows the same component;
/// and a plugin host restart doesn't close the deck or blank its keys.
#[tokio::test(flavor = "multi_thread")]
async fn a_stream_deck_runs_flows_and_rides_out_unplugs_and_host_restarts() {
    let data = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let server = start(data.path(), &fake);

    // A surface for the deck, made before it's plugged in, with a start component.
    let main = component(&server).await;
    let (status, made) = server
        .post(
            "/surfaces",
            json!({
                "name": "Desk", "model": "elgato.original-mk2",
                "serial": "CL12K1A00042", "startComponent": main,
            }),
        )
        .await;
    assert_eq!(status.as_u16(), 201, "{made}");
    let id = made["surface"]["id"].as_str().unwrap().to_owned();

    // Plugged in: the same surface attaches, showing its start component.
    let deck = Mk2::plug(&fake, "/dev/hidraw0", "CL12K1A00042");
    let conn = deck_conn(&server).await;
    let conn_id = conn["id"].as_str().unwrap().to_owned();
    let alias = conn["alias"].as_str().unwrap().to_owned();
    let surface = attached(&server, &id).await;
    let slug = surface["slug"].as_str().unwrap().to_owned();
    assert_eq!(surface["provider"], json!(conn_id));
    assert_eq!(server.get("/surfaces").await.as_array().unwrap().len(), 1);
    let current = format!("surface.{slug}.current.main");
    assert_eq!(server.value(&current).await, Some(json!("main")));
    all_drawn(&deck, 0).await;

    // `k0` on the component sets the deck's brightness, through a flow.
    let (status, flow) = server
        .post(
            "/flows",
            json!({
                "name": "Bright on k0", "enabled": true, "concurrency": "ignore",
                "triggers": [{ "kind": "event", "from": { "component": main, "controls": "k0" }, "event": "press" }],
                "body": [{
                    "id": "bright", "kind": "action", "target": { "conn": conn_id },
                    "action": "brightness", "params": { "percent": 60 },
                }],
            }),
        )
        .await;
    assert_eq!(status.as_u16(), 201, "{flow}");
    deck.press(0);
    let deadline = std::time::Instant::now() + WAIT;
    while deck.brightness() != Some(60) {
        assert!(std::time::Instant::now() < deadline, "no brightness");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    deck.release(0);

    // `why` on the action walks back through the trigger and the key's
    // event to its press, with the time the core read the deck's report.
    let requested = brightness_requested(&server).await.expect("the action");
    let chain = server
        .get(&format!("/journal/{}/why", requested["seq"]))
        .await;
    let chain = chain.as_array().unwrap();
    let kinds: Vec<&str> = chain.iter().map(|e| e["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds.first(), Some(&"action.requested"), "{kinds:?}");
    assert_eq!(
        &kinds[kinds.len() - 3..],
        ["trigger.matched", "event", "state.patch"],
        "{kinds:?}"
    );
    let press = chain.last().unwrap();
    let text = press.to_string();
    assert!(
        text.contains(&slug) && text.contains("k0") && text.contains("pressed"),
        "{press}"
    );
    assert!(
        press.pointer("/data/deviceTs").is_some_and(Value::is_f64),
        "{press}"
    );

    // Unplugged with a key held: the surface detaches and releases it.
    let pressed = format!("surface.{slug}.controls.k3.pressed");
    let connected = format!("surface.{slug}.connected");
    deck.press(3);
    server
        .eventually(WAIT, "k3 held", || async {
            (server.value(&pressed).await == Some(json!(true))).then_some(())
        })
        .await;
    deck.unplug();
    server
        .eventually(WAIT, "the surface to detach", || async {
            (server.value(&connected).await == Some(json!(false))).then_some(())
        })
        .await;
    assert_eq!(server.value(&pressed).await, Some(json!(false)), "released");

    // Back in another port: the same surface, the component it showed, redrawn.
    let deck = Mk2::plug(&fake, "/dev/hidraw3", "CL12K1A00042");
    attached(&server, &id).await;
    assert_eq!(server.get("/surfaces").await.as_array().unwrap().len(), 1);
    assert_eq!(server.value(&current).await, Some(json!("main")));
    let before = all_drawn(&deck, 0).await;
    let drawn_before: std::collections::BTreeMap<u8, usize> =
        deck.images().iter().map(|(k, i)| (*k, i.len())).collect();
    let features_before = fake.log(deck.path()).features_sent.len();

    // The plugin host is killed: the deck stays open, the surface detaches
    // and is back within 3 s, and nothing blanks or resets the keys.
    let pid = server.get("/health").await["host"]["pid"].as_u64().unwrap();
    let killed = std::time::Instant::now();
    std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status()
        .unwrap();
    server
        .eventually(WAIT, "the surface to detach", || async {
            (server.value(&connected).await == Some(json!(false))).then_some(())
        })
        .await;
    server
        .eventually(WAIT, "the surface to reattach", || async {
            (server.value(&connected).await == Some(json!(true))).then_some(())
        })
        .await;
    let back = killed.elapsed();
    assert!(back < Duration::from_secs(3), "reattached after {back:?}");
    assert_eq!(fake.log(deck.path()).opens, 1, "the deck was never closed");
    let resets: Vec<Vec<u8>> = fake.log(deck.path()).features_sent[features_before..]
        .iter()
        .filter(|f| f.len() >= 2 && f[0] == 0x03 && f[1] == 0x02)
        .cloned()
        .collect();
    assert!(resets.is_empty(), "reset to the logo: {resets:?}");
    // Redrawn with what each key had: never anything else.
    let after = all_drawn(&deck, 1).await;
    for (key, images) in deck.images() {
        for image in images.iter().skip(drawn_before[&key]) {
            assert_eq!(image, &before[&key], "k{key} drew something else");
        }
    }
    assert_eq!(after, before);
    assert_eq!(server.value(&current).await, Some(json!("main")));

    // And its keys still run the flow.
    let (status, outcome) = server
        .post(
            "/actions",
            json!({ "action": format!("{alias}.brightness"), "params": { "percent": 20 } }),
        )
        .await;
    assert_eq!(
        (status.as_u16(), &outcome["ok"]),
        (200, &json!(true)),
        "{outcome}"
    );
    deck.press(0);
    let deadline = std::time::Instant::now() + WAIT;
    while deck.brightness() != Some(60) {
        assert!(
            std::time::Instant::now() < deadline,
            "no brightness after the restart"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    deck.release(0);
    server.app.clone().shutdown().await;
}

/// M7's budgets with the plugin host in the path, against the simulated
/// MK.2: times are taken where the core's device layer meets the device.
///
/// - Key down → the action's feature report at the device: the key report
///   goes through the plugin to the core, which runs the flow and sends the
///   action back through the plugin to the device. That's more than "first
///   action dispatched", so it bounds it.
/// - State change → every key's image written to the device, 15 keys of 2
///   `expr` bindings each, rendered as the MK.2's 72 px JPEGs, as M5 timed
///   them without a plugin.
///
/// Run with
/// `cargo test --release -p covalence-server --test streamdeck bench -- --ignored --nocapture`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "a benchmark: run it in release"]
async fn bench_budgets_with_the_plugin_host_in_the_path() {
    use covalence_core::state::{Meta, Quality, Source, StatePath};
    use covalence_protocol::state::PathSegment;

    let data = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let deck = Mk2::plug(&fake, "/dev/hidraw0", "CL12K1A00042");
    let server = start(data.path(), &fake);
    let conn = deck_conn(&server).await;
    let conn_id = conn["id"].as_str().unwrap().to_owned();
    let surface = server
        .eventually(WAIT, "a surface for the deck", || async {
            server
                .get("/surfaces")
                .await
                .as_array()
                .unwrap()
                .first()
                .cloned()
        })
        .await;
    let id = surface["id"].as_str().unwrap().to_owned();
    attached(&server, &id).await;

    // A component of expr bindings on every key, as M5's bench.
    let keys: Vec<Value> = (0..15)
        .map(|k| {
            json!({
                "id": format!("k{k}"), "at": { "x": k % 5, "y": k / 5 },
                "inputs": ["momentary"], "outputs": ["image"],
            })
        })
        .collect();
    let (_, component) = server
        .post("/components", Server::page("Bench", json!(keys)))
        .await;
    let component = component["component"]["id"].as_str().unwrap().to_owned();
    for i in 0..15 {
        let (status, body) = server
            .try_bind(&component, json!({ "control": format!("k{i}"),
                    "props": {
                        "label": { "expr": format!("`Cam {i}: ${{mmss((vars.n ?? 0) + {i})}}`") },
                        "background": { "expr": format!("(vars.n ?? 0) % 2 ? hsl({i} * 20, 70, 40) : \"#202020\"") },
                    },
                }),
            )
            .await;
        assert_eq!(status.as_u16(), 200, "{body}");
    }
    let (status, flow) = server
        .post(
            "/flows",
            json!({
                "name": "Bright on k0", "enabled": true, "concurrency": "parallel",
                "triggers": [{ "kind": "event", "from": { "component": component, "controls": "k0" }, "event": "press" }],
                "body": [{
                    "id": "bright", "kind": "action", "target": { "conn": conn_id },
                    "action": "brightness", "params": { "percent": 60 },
                }],
            }),
        )
        .await;
    assert_eq!(status.as_u16(), 201, "{flow}");
    server
        .post(
            &format!("/surfaces/{id}/current"),
            json!({ "component": component }),
        )
        .await;
    all_drawn(&deck, 0).await;

    let n = StatePath::new("vars", vec![PathSegment::Key("n".into())]);
    let set_n = |v: i64| {
        let meta = Meta {
            ts: 0,
            seq: 1,
            quality: Quality::Good,
            source: Source::Conn("bench".into()),
        };
        server
            .app
            .core()
            .shared()
            .state
            .mutate(|tree| ((), tree.set(&n, &v.into(), &meta)));
    };
    // Times come from when the fake was written to, so polling it now and
    // then is as exact as spinning on it, and doesn't hold its lock.
    async fn poll_until(what: &str, f: impl Fn() -> bool) {
        let deadline = std::time::Instant::now() + WAIT;
        while !f() {
            assert!(std::time::Instant::now() < deadline, "timed out on {what}");
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }
    let stats = |mut times: Vec<Duration>| {
        times.sort();
        (times[times.len() / 2], times[times.len() - 1])
    };

    // State change → every key written.
    let mut redraws = Vec::new();
    let mut firsts = Vec::new();
    for round in 1..=40 {
        let counts: Vec<usize> = (0..15u8)
            .map(|k| deck.images().get(&k).map_or(0, Vec::len))
            .collect();
        let start = std::time::Instant::now();
        set_n(round);
        poll_until("the redraw", || {
            let images = deck.images();
            (0..15u8).all(|k| images.get(&k).map_or(0, Vec::len) > counts[k as usize])
        })
        .await;
        let written: Vec<std::time::Instant> = deck
            .timed_images()
            .values()
            .map(|i| i.last().unwrap().0)
            .collect();
        redraws.push(*written.iter().max().unwrap() - start);
        firsts.push(*written.iter().min().unwrap() - start);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let (median, worst) = stats(redraws);
    println!(
        "state change → 15 keys written to the MK.2 (72 px JPEG, 2 expr bindings each): median {median:?}, worst {worst:?}"
    );
    let (median, worst) = stats(firsts);
    println!("state change → the first of them written: median {median:?}, worst {worst:?}");

    // Key down → the action at the device.
    let mut presses = Vec::new();
    for _ in 0..40 {
        let before = fake.log(deck.path()).features_sent.len();
        let start = std::time::Instant::now();
        deck.press(0);
        poll_until("the action", || {
            fake.log(deck.path()).features_sent.len() > before
        })
        .await;
        presses.push(fake.log(deck.path()).feature_times[before] - start);
        deck.release(0);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let (median, worst) = stats(presses);
    println!(
        "key down → its flow's action at the MK.2, through the plugin both ways: median {median:?}, worst {worst:?}"
    );
    server.app.clone().shutdown().await;
}

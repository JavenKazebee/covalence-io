//! Milestone 7 Phase 4: the Stream Deck plugin, end to end, against a
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

/// A layer of the MK.2's 15 image keys, with labels on `k0` and `k7`.
async fn layer(server: &Server) -> String {
    let controls: Vec<Value> = (0..15)
        .map(|k| {
            json!({
                "id": format!("k{k}"), "at": { "x": k % 5, "y": k / 5 },
                "inputs": ["momentary"], "outputs": ["image"],
            })
        })
        .collect();
    let (status, layer) = server
        .post("/layers", json!({ "name": "Main", "controls": controls }))
        .await;
    assert_eq!(status.as_u16(), 201, "{layer}");
    let id = layer["layer"]["id"].as_str().unwrap().to_owned();
    for (control, label) in [("k0", "'Cam 1'"), ("k7", "'Cam 2'")] {
        let (status, binding) = server
            .post(
                "/control-bindings",
                json!({ "layer": id, "control": control, "props": { "label": { "expr": label } } }),
            )
            .await;
        assert_eq!(status.as_u16(), 201, "{binding}");
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

    // A shown layer is drawn on the deck as JPEG, each key complete.
    let main = layer(&server).await;
    let (status, body) = server
        .post(&format!("/surfaces/{id}/current"), json!({ "layer": main }))
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

//! Milestone 7 Phase 1: the device layer in the server, against pretend
//! devices and the real Deno plugin host. Needs `deno` on the PATH.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use covalence_devices::fake::{Fake, esp32, stream_deck_mk2};
use covalence_server::{App, Config, DeviceSource, PluginSpec};
use serde_json::{Value, json};

mod common;
use common::Server;

const WAIT: Duration = Duration::from_secs(15);
const DECK: &str = "hid:0fd9:0080:AL12";
const PLUGIN: &str = "io.covalence.test.deck";

fn start(data: &Path, fake: &Fake) -> Server {
    let mut plugins = Config::default_plugins();
    plugins.push(PluginSpec {
        id: PLUGIN.into(),
        version: "0.0.1".into(),
        bundle: "packages/plugin-host/testdata/device_plugin.ts".into(),
    });
    let app = App::start(Config {
        data_dir: data.to_owned(),
        root: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
        plugins,
        deno: "deno".into(),
        devices: DeviceSource::Backend(Arc::new(fake.clone())),
    })
    .unwrap();
    Server::with_app(app)
}

async fn device(server: &Server, id: &str) -> Option<Value> {
    let body = server.get("/devices").await;
    assert_eq!(body["available"], json!(true));
    body["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == id)
        .cloned()
}

async fn connections(server: &Server) -> Vec<Value> {
    let body = server.get("/connections").await;
    body.as_array()
        .unwrap()
        .iter()
        .filter(|c| c["plugin"] == PLUGIN)
        .cloned()
        .collect()
}

/// Polls `f` until it gives something, for up to `WAIT`.
async fn eventually<T, F: Future<Output = Option<T>>>(what: &str, f: impl Fn() -> F) -> T {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        if let Some(v) = f().await {
            return v;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_matching_device_gets_a_connection_and_others_are_offered() {
    let data = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    let server = start(data.path(), &fake);

    // A Stream Deck: its plugin says autoCreate.
    fake.plug(stream_deck_mk2("/dev/hidraw0", Some("AL12")));
    let conn = eventually("a connection for the deck", || async {
        connections(&server).await.into_iter().next()
    })
    .await;
    assert_eq!(conn["alias"], json!("deck"));
    assert_eq!(conn["config"]["device"], json!(DECK));
    assert_eq!(conn["mode"], json!("live"));
    let deck = device(&server, DECK).await.unwrap();
    assert_eq!(deck["heldBy"], conn["id"]);
    assert!(deck.get("offer").is_none(), "{deck}");
    assert_eq!(deck["product"], json!("Stream Deck MK.2"));

    // An ESP32: matched, but only offered.
    fake.plug(esp32("/dev/ttyACM0", Some("ESP1")));
    let esp = eventually("the ESP32", || async {
        device(&server, "serial:303a:1001:ESP1").await
    })
    .await;
    assert_eq!(
        esp["offer"],
        json!({ "kind": "offer", "device": "serial:303a:1001:ESP1", "plugins": [PLUGIN] })
    );
    assert!(esp.get("heldBy").is_none());

    // A device nothing matches is listed, with no offer.
    let mut other = esp32("/dev/ttyUSB0", Some("X"));
    other.vendor_id = Some(0x1a86);
    fake.plug(other);
    let other = eventually("the other device", || async {
        device(&server, "serial:1a86:1001:X").await
    })
    .await;
    assert!(other.get("offer").is_none());

    // Unplugged and plugged back in: still the one connection.
    fake.unplug("/dev/hidraw0");
    eventually("the deck to go", || async {
        device(&server, DECK).await.is_none().then_some(())
    })
    .await;
    fake.plug(stream_deck_mk2("/dev/hidraw3", Some("AL12")));
    eventually("the deck to return", || async {
        device(&server, DECK).await
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(connections(&server).await.len(), 1);

    // A second deck gets a connection of its own.
    fake.plug(stream_deck_mk2("/dev/hidraw4", Some("BX99")));
    let both = eventually("a second connection", || async {
        let conns = connections(&server).await;
        (conns.len() == 2).then_some(conns)
    })
    .await;
    let aliases: Vec<&str> = both.iter().map(|c| c["alias"].as_str().unwrap()).collect();
    assert!(aliases.contains(&"deck2"), "{aliases:?}");

    server.app.clone().shutdown().await;

    // After a restart, the stored connections hold their devices.
    let server = start(data.path(), &fake);
    let deck = eventually("the deck after a restart", || async {
        device(&server, DECK)
            .await
            .filter(|d| d.get("heldBy").is_some())
    })
    .await;
    assert_eq!(deck["heldBy"], conn["id"]);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(connections(&server).await.len(), 2);
    server.app.clone().shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn without_a_device_layer_the_list_is_empty() {
    let server = Server::start();
    let body = server.get("/devices").await;
    assert_eq!(body, json!({ "available": false, "devices": [] }));
}

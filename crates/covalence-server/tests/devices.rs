//! Milestone 7 Phase 1: the device layer in the server, against pretend
//! devices and the real Deno plugin host. Needs `deno` on the PATH.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use covalence_devices::fake::{Fake, esp32, test_deck};
use covalence_server::{App, Config, DeviceSource, PluginSpec};
use serde_json::{Value, json};

mod common;
use common::Server;

const WAIT: Duration = Duration::from_secs(15);
const DECK: &str = "hid:1209:0001:AL12";
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
    fake.plug(test_deck("/dev/hidraw0", Some("AL12")));
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
    assert_eq!(deck["product"], json!("Test deck"));

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
    fake.plug(test_deck("/dev/hidraw3", Some("AL12")));
    eventually("the deck to return", || async {
        device(&server, DECK).await
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(connections(&server).await.len(), 1);

    // A second deck gets a connection of its own.
    fake.plug(test_deck("/dev/hidraw4", Some("BX99")));
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

/// Runs an action on `deck` and returns its result.
async fn act(server: &Server, action: &str, params: Value) -> Value {
    let (status, outcome) = server
        .post(
            "/actions",
            json!({ "action": format!("deck.{action}"), "params": params }),
        )
        .await;
    assert_eq!(status.as_u16(), 200, "{outcome}");
    assert_eq!(outcome["ok"], json!(true), "{action}: {outcome}");
    outcome["result"].clone()
}

/// The value at `path`, once the connection its alias names exists.
async fn value(server: &Server, path: &str) -> Option<Value> {
    let alias = path.split('.').next().unwrap();
    let conns = server.get("/connections").await;
    if !conns
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["alias"] == alias)
    {
        return None;
    }
    server.value(path).await
}

/// A TCP server that answers each message with `echo: <message>`.
async fn echo_server() -> u16 {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                while let Ok(n) = socket.read(&mut buf).await {
                    if n == 0 {
                        return;
                    }
                    let reply = [b"echo: ", &buf[..n]].concat();
                    if socket.write_all(&reply).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    port
}

#[tokio::test(flavor = "multi_thread")]
async fn a_plugin_drives_its_device_and_sockets_through_the_core() {
    let data = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    fake.plug(test_deck("/dev/hidraw0", Some("AL12")));
    fake.set_feature("/dev/hidraw0", 0x06, &[0x06, 0x0c, 0xaa, 0xbb]);
    let server = start(data.path(), &fake);

    // The automatic connection opens its deck through `ctx.hid`.
    server
        .eventually(WAIT, "the deck's handle to open", || async {
            (value(&server, "deck.io").await == Some(json!("open"))).then_some(())
        })
        .await;

    // Input reports reach the plugin.
    fake.input("/dev/hidraw0", &[0x01, 0x00, 0x0f, 0x00, 0x01]);
    server
        .eventually(WAIT, "the input in state", || async {
            (value(&server, "deck.input").await == Some(json!("01000f0001"))).then_some(())
        })
        .await;

    // Output and feature reports reach the device.
    act(&server, "send", json!({ "hex": "0201ff" })).await;
    // Draws (`0x07`) go out too: the deck is a surface, blank until a layer
    // is shown.
    let output = |l: &covalence_devices::fake::Log| -> Vec<Vec<u8>> {
        l.writes.iter().filter(|w| w[0] != 0x07).cloned().collect()
    };
    let log = fake.wait_log("/dev/hidraw0", WAIT, |l| !output(l).is_empty());
    assert_eq!(output(&log), [vec![0x02, 0x01, 0xff]]);
    assert_eq!(
        act(&server, "feature", json!({ "reportId": 6, "length": 3 })).await,
        json!("060caa")
    );
    act(&server, "sendFeature", json!({ "hex": "030832" })).await;
    assert_eq!(
        fake.log("/dev/hidraw0").features_sent,
        [vec![0x03, 0x08, 0x32]]
    );
    assert_eq!(act(&server, "list", json!({})).await, json!([DECK]));

    // Unplugged and back: the plugin's handle closes and reopens.
    fake.unplug("/dev/hidraw0");
    server
        .eventually(WAIT, "the handle to close", || async {
            (value(&server, "deck.io").await == Some(json!("closed:unplugged"))).then_some(())
        })
        .await;
    fake.plug(test_deck("/dev/hidraw1", Some("AL12")));
    server
        .eventually(WAIT, "the handle to reopen", || async {
            (value(&server, "deck.io").await == Some(json!("open"))).then_some(())
        })
        .await;

    // TCP, as a `Deno.Conn` and as a `node:net` socket.
    let port = echo_server().await;
    for via in ["conn", "net"] {
        let reply = act(
            &server,
            "tcp",
            json!({ "port": port, "text": via, "via": via }),
        )
        .await;
        assert_eq!(reply, json!(format!("echo: {via}")));
    }
    let (status, refused) = server
        .post(
            "/actions",
            json!({ "action": "deck.tcp", "params": { "port": 1, "text": "x", "via": "conn" } }),
        )
        .await;
    assert_eq!(status.as_u16(), 200);
    assert_eq!(refused["ok"], json!(false), "{refused}");

    // The inspector saw all of it.
    let conn = connections(&server).await.remove(0);
    let traffic = server
        .get(&format!(
            "/connections/{}/traffic",
            conn["id"].as_str().unwrap()
        ))
        .await;
    assert_eq!(traffic["inspectable"], json!(true));
    let handles = traffic["handles"].as_array().unwrap();
    let deck = handles
        .iter()
        .find(|h| h["kind"] == "hid")
        .expect("the deck's handle");
    assert_eq!(deck["open"], json!(true));
    let hexes: Vec<&str> = deck["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["hex"].as_str())
        .collect();
    assert!(
        hexes.contains(&"01000f0001") && hexes.contains(&"0201ff"),
        "{deck}"
    );
    let sockets: Vec<&Value> = handles.iter().filter(|h| h["kind"] == "tcp").collect();
    // Two that talked, and the refused one, which says why.
    assert_eq!(sockets.len(), 3, "{traffic}");
    assert!(
        traffic.to_string().contains("Connection refused"),
        "{traffic}"
    );
    assert!(sockets.iter().all(|h| h["open"] == json!(false)));
    let said: Vec<&str> = sockets
        .iter()
        .flat_map(|h| h["entries"].as_array().unwrap())
        .filter(|e| e["dir"] == "out")
        .filter_map(|e| e["hex"].as_str())
        .collect();
    assert!(said.contains(&"636f6e6e"), "{traffic}"); // "conn"

    // Another connection can't open the deck.
    let (status, second) = server
        .post(
            "/connections",
            json!({ "plugin": PLUGIN, "alias": "other", "config": { "device": DECK }, "mode": "live" }),
        )
        .await;
    assert_eq!(status.as_u16(), 201, "{second}");
    server
        .eventually(WAIT, "the second connection to fail with busy", || async {
            let journal = server.get("/journal?limit=1000").await;
            journal
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e.to_string().contains("is held by connection"))
                .then_some(())
        })
        .await;

    server.app.clone().shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn devices_stay_open_while_the_plugin_host_restarts() {
    let data = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    fake.plug(test_deck("/dev/hidraw0", Some("AL12")));
    let server = start(data.path(), &fake);
    server
        .eventually(WAIT, "the deck's handle to open", || async {
            (value(&server, "deck.io").await == Some(json!("open"))).then_some(())
        })
        .await;
    let before = server.get("/devices").await["devices"][0]["handle"]["handle"].clone();

    let pid = server.get("/health").await["host"]["pid"].as_u64().unwrap();
    std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status()
        .unwrap();
    server
        .eventually(WAIT, "a new host", || async {
            let health = server.get("/health").await;
            (health["host"]["pid"].as_u64().is_some_and(|p| p != pid)
                && health["host"]["phase"] == "ready")
                .then_some(())
        })
        .await;
    fake.input("/dev/hidraw0", &[0x01, 0x07]);
    server
        .eventually(WAIT, "input after the restart", || async {
            (value(&server, "deck.input").await == Some(json!("0107"))).then_some(())
        })
        .await;
    let after = server.get("/devices").await["devices"][0]["handle"]["handle"].clone();
    assert_eq!(after, before, "the same handle");
    assert_eq!(
        fake.log("/dev/hidraw0").opens,
        1,
        "the device was never closed"
    );
    server.app.clone().shutdown().await;
}

/// A layer of the test deck's 15 image keys, with a label on `k3`.
async fn deck_layer(server: &Server) -> String {
    let controls: Vec<Value> = (0..15)
        .map(|k| {
            json!({
                "id": format!("k{k}"), "at": { "x": k % 5, "y": k / 5 },
                "inputs": ["momentary"], "outputs": ["image"],
            })
        })
        .collect();
    let (status, layer) = server
        .post("/layers", json!({ "name": "Cams", "controls": controls }))
        .await;
    assert_eq!(status.as_u16(), 201, "{layer}");
    let layer = layer["layer"]["id"].as_str().unwrap().to_owned();
    let (status, binding) = server
        .post(
            "/control-bindings",
            json!({ "layer": layer, "control": "k3", "props": { "label": { "expr": "'Hi'" } } }),
        )
        .await;
    assert_eq!(status.as_u16(), 201, "{binding}");
    layer
}

/// The JPEG images written to the fake deck's `key`, as the plugin writes
/// them: `[0x07, key, first 4 bytes]`.
fn drawn(fake: &Fake, path: &str, key: u8) -> usize {
    fake.log(path)
        .writes
        .iter()
        .filter(|w| w.len() == 6 && w[0] == 0x07 && w[1] == key && w[2..4] == [0xff, 0xd8])
        .count()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_plugin_provides_a_surface() {
    let data = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    fake.plug(test_deck("/dev/hidraw0", Some("AL12")));
    let server = start(data.path(), &fake);

    // The plugin attaches its deck: the core makes it a surface entity.
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
    let conn = connections(&server).await.remove(0);
    assert_eq!(surface["model"], json!("test.deck15"));
    assert_eq!(surface["serial"], json!(DECK));
    assert_eq!(surface["provider"], conn["id"]);
    let slug = surface["slug"].as_str().unwrap().to_owned();
    let id = surface["id"].as_str().unwrap().to_owned();
    let connected = format!("surface.{slug}.connected");
    server
        .eventually(WAIT, "the surface to attach", || async {
            (server.value(&connected).await == Some(json!(true))).then_some(())
        })
        .await;

    // Showing a layer draws its keys on the device, as JPEG.
    let layer = deck_layer(&server).await;
    let (status, shown) = server
        .post(
            &format!("/surfaces/{id}/current"),
            json!({ "layer": layer }),
        )
        .await;
    assert_eq!(status.as_u16(), 200, "{shown}");
    server
        .eventually(WAIT, "k3 drawn on the device", || async {
            (drawn(&fake, "/dev/hidraw0", 3) > 0).then_some(())
        })
        .await;

    // Key reports become control state.
    let pressed = format!("surface.{slug}.controls.k3.pressed");
    fake.input("/dev/hidraw0", &[0x01, 3, 1]);
    server
        .eventually(WAIT, "k3 pressed", || async {
            (server.value(&pressed).await == Some(json!(true))).then_some(())
        })
        .await;
    fake.input("/dev/hidraw0", &[0x01, 3, 0]);
    server
        .eventually(WAIT, "k3 released", || async {
            (server.value(&pressed).await == Some(json!(false))).then_some(())
        })
        .await;

    // The host goes: the surface detaches, releasing a held key, and
    // reattaches as the same entity, showing the same layer.
    fake.input("/dev/hidraw0", &[0x01, 3, 1]);
    server
        .eventually(WAIT, "k3 held", || async {
            (server.value(&pressed).await == Some(json!(true))).then_some(())
        })
        .await;
    let before = drawn(&fake, "/dev/hidraw0", 3);
    let pid = server.get("/health").await["host"]["pid"].as_u64().unwrap();
    std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status()
        .unwrap();
    server
        .eventually(WAIT, "the surface to detach", || async {
            (server.value(&connected).await == Some(json!(false))).then_some(())
        })
        .await;
    assert_eq!(server.value(&pressed).await, Some(json!(false)), "released");
    server
        .eventually(WAIT, "the surface to reattach", || async {
            (server.value(&connected).await == Some(json!(true))).then_some(())
        })
        .await;
    server
        .eventually(WAIT, "k3 drawn again", || async {
            (drawn(&fake, "/dev/hidraw0", 3) > before).then_some(())
        })
        .await;
    assert_eq!(server.get("/surfaces").await.as_array().unwrap().len(), 1);
    assert_eq!(
        server.value(&format!("surface.{slug}.current.main")).await,
        Some(json!("cams"))
    );
    server.app.clone().shutdown().await;
}

/// M7: a serial device picked in a connection's form is read and written
/// through `ctx.serial`, with its traffic seen.
#[tokio::test(flavor = "multi_thread")]
async fn a_serial_device_picked_for_a_connection_is_read_and_written() {
    const ESP: &str = "serial:303a:1001:ESP1";
    let data = tempfile::tempdir().unwrap();
    let fake = Fake::new();
    fake.plug(esp32("/dev/ttyACM0", Some("ESP1")));
    let server = start(data.path(), &fake);

    // The connection form's picker lists it for the plugin that matches it.
    let picker = eventually("the ESP32 in the picker", || async {
        let list = server.get(&format!("/devices?plugin={PLUGIN}")).await;
        list["devices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["id"] == ESP)
            .cloned()
    })
    .await;
    assert_eq!(picker["kind"], json!("serial"));
    assert!(picker.get("heldBy").is_none(), "{picker}");

    let (status, conn) = server
        .post(
            "/connections",
            json!({ "plugin": PLUGIN, "alias": "deck", "config": { "device": ESP }, "mode": "live" }),
        )
        .await;
    assert_eq!(status.as_u16(), 201, "{conn}");
    server
        .eventually(WAIT, "the port to open", || async {
            (value(&server, "deck.io").await == Some(json!("open"))).then_some(())
        })
        .await;
    assert_eq!(fake.log("/dev/ttyACM0").baud, Some(9600));
    assert_eq!(device(&server, ESP).await.unwrap()["heldBy"], conn["id"]);

    // Bytes both ways.
    fake.input("/dev/ttyACM0", b"hi\n");
    server
        .eventually(WAIT, "the line in state", || async {
            (value(&server, "deck.input").await == Some(json!("68690a"))).then_some(())
        })
        .await;
    act(&server, "send", json!({ "hex": "6f6b0a" })).await;
    let log = fake.wait_log("/dev/ttyACM0", WAIT, |l| !l.writes.is_empty());
    assert_eq!(log.writes, [b"ok\n".to_vec()]);

    // A serial port isn't a surface.
    assert_eq!(server.get("/surfaces").await, json!([]));

    // Its traffic is seen.
    let traffic = server
        .get(&format!(
            "/connections/{}/traffic",
            conn["id"].as_str().unwrap()
        ))
        .await;
    let port = traffic["handles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|h| h["kind"] == "serial")
        .unwrap_or_else(|| panic!("no serial handle in {traffic}"));
    assert_eq!(port["target"], json!(ESP));
    let entries = port["entries"].to_string();
    assert!(
        entries.contains("68690a") && entries.contains("6f6b0a"),
        "{entries}"
    );

    // Unplugged and back: the port closes and reopens.
    fake.unplug("/dev/ttyACM0");
    server
        .eventually(WAIT, "the port to close", || async {
            (value(&server, "deck.io").await == Some(json!("closed:unplugged"))).then_some(())
        })
        .await;
    fake.plug(esp32("/dev/ttyACM1", Some("ESP1")));
    server
        .eventually(WAIT, "the port to reopen", || async {
            (value(&server, "deck.io").await == Some(json!("open"))).then_some(())
        })
        .await;
    server.app.clone().shutdown().await;
}

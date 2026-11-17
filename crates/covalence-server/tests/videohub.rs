//! Milestone 7 Phase 5: the Videohub plugin live, on the core's `tcp`
//! transport, against a fake Videohub; and `directNetwork` on a connection.
//! Needs `deno` on the PATH.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::StatusCode;
use covalence_server::{App, Config, PluginSpec};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::broadcast;

mod common;
use common::Server;

const WAIT: Duration = Duration::from_secs(15);

fn start() -> Server {
    let data = tempfile::tempdir().unwrap();
    let mut plugins = Config::default_plugins();
    plugins.push(PluginSpec {
        id: "io.covalence.test.direct".into(),
        version: "0.0.1".into(),
        bundle: "packages/plugin-host/testdata/direct_plugin.ts".into(),
    });
    let app = App::start(Config {
        data_dir: data.path().to_owned(),
        root: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
        plugins,
        deno: "deno".into(),
        devices: Default::default(),
    })
    .unwrap();
    let mut server = Server::with_app(app);
    server._data = data;
    server
}

/// A 2×2 Videohub: sends its prelude, ACKs routes and echoes them to every
/// client, as the real one does.
struct FakeHub {
    port: u16,
    routes: Arc<Mutex<[u8; 2]>>,
    /// Each client listens here: blocks to send, or `None` to hang up.
    clients: broadcast::Sender<Option<String>>,
}

impl FakeHub {
    async fn start() -> FakeHub {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let routes = Arc::new(Mutex::new([0u8, 1]));
        let (clients, _) = broadcast::channel(64);
        let hub = FakeHub {
            port,
            routes: routes.clone(),
            clients: clients.clone(),
        };
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                tokio::spawn(serve(socket, routes.clone(), clients.clone()));
            }
        });
        hub
    }

    /// Hangs up on every client, as a reboot would.
    fn drop_clients(&self) {
        let _ = self.clients.send(None);
    }
}

fn prelude(routes: [u8; 2]) -> String {
    format!(
        "PROTOCOL PREAMBLE:\nVersion: 2.8\n\n\
         VIDEOHUB DEVICE:\nDevice present: true\nModel name: Fake Videohub 2x2\n\
         Video inputs: 2\nVideo outputs: 2\n\n\
         INPUT LABELS:\n0 Cam 1\n1 Cam 2\n\n\
         OUTPUT LABELS:\n0 Program\n1 Monitor\n\n\
         VIDEO OUTPUT ROUTING:\n0 {}\n1 {}\n\n\
         END PRELUDE:\n\n",
        routes[0], routes[1]
    )
}

async fn serve(
    socket: tokio::net::TcpStream,
    routes: Arc<Mutex<[u8; 2]>>,
    clients: broadcast::Sender<Option<String>>,
) {
    let mut others = clients.subscribe();
    let (reader, mut writer) = socket.into_split();
    let first = prelude(*routes.lock().unwrap());
    if writer.write_all(first.as_bytes()).await.is_err() {
        return;
    }
    let mut lines = BufReader::new(reader).lines();
    let mut block: Vec<String> = Vec::new();
    loop {
        tokio::select! {
            line = lines.next_line() => {
                let Ok(Some(line)) = line else { return };
                if !line.is_empty() {
                    block.push(line);
                    continue;
                }
                let lines_in = std::mem::take(&mut block);
                if lines_in.first().map(String::as_str) != Some("VIDEO OUTPUT ROUTING:") {
                    let _ = writer.write_all(b"NAK\n\n").await;
                    continue;
                }
                let mut echo = String::from("VIDEO OUTPUT ROUTING:\n");
                for route in &lines_in[1..] {
                    let (o, i) = route.split_once(' ').unwrap();
                    let (o, i): (usize, u8) = (o.parse().unwrap(), i.parse().unwrap());
                    routes.lock().unwrap()[o] = i;
                    echo.push_str(&format!("{o} {i}\n"));
                }
                echo.push('\n');
                let _ = writer.write_all(b"ACK\n\n").await;
                let _ = clients.send(Some(echo));
            }
            other = others.recv() => match other {
                Ok(Some(text)) => {
                    if writer.write_all(text.as_bytes()).await.is_err() {
                        return;
                    }
                }
                Ok(None) | Err(_) => return,
            },
        }
    }
}

/// The statuses journaled for `conn`, in order.
async fn statuses(server: &Server, conn: &str) -> Vec<String> {
    let journal = server.get("/journal?limit=10000").await;
    journal
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "conn.status" && e["data"]["conn"] == conn)
        .map(|e| e["data"]["status"].as_str().unwrap().to_owned())
        .collect()
}

async fn listed(server: &Server, alias: &str) -> Value {
    let conns = server.get("/connections").await;
    conns
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["alias"] == alias)
        .cloned()
        .unwrap_or_else(|| panic!("no connection {alias} in {conns}"))
}

/// The bytes the traffic view shows, for each entry going `dir`.
fn texts(traffic: &Value, dir: &str) -> Vec<String> {
    traffic["handles"][0]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["dir"] == dir)
        .filter_map(|e| e["hex"].as_str())
        .map(|h| {
            let bytes: Vec<u8> = (0..h.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap())
                .collect();
            String::from_utf8(bytes).unwrap()
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_videohub_talks_through_the_core_and_its_traffic_is_seen() {
    let hub = FakeHub::start().await;
    let server = start();
    let (status, conn) = server
        .post(
            "/connections",
            json!({
                "plugin": "io.covalence.videohub",
                "alias": "vh",
                "mode": "live",
                "config": { "host": "127.0.0.1", "port": hub.port },
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{conn}");
    let id = conn["id"].as_str().unwrap().to_owned();
    let model = server
        .eventually(WAIT, "the prelude", || server.value("vh.model"))
        .await;
    assert_eq!(model, json!("Fake Videohub 2x2"));
    assert_eq!(
        server.value("vh.outputs[1].name").await,
        Some(json!("Monitor"))
    );

    // A route goes out through the core and comes back as the device's echo.
    let (status, outcome) = server
        .post(
            "/actions",
            json!({ "action": "vh.route", "params": { "output": 1, "input": 0 } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{outcome}");
    assert_eq!(outcome["ok"], json!(true), "{outcome}");
    assert_eq!(*hub.routes.lock().unwrap(), [0, 0]);
    server
        .eventually(WAIT, "the echoed route", || async {
            (server.value("vh.outputs[1].source").await == Some(json!(0))).then_some(())
        })
        .await;

    // The core saw all of it: one socket, cut in lines.
    let listed_vh = listed(&server, "vh").await;
    assert_eq!(listed_vh.get("directNetwork"), None, "{listed_vh}");
    let traffic = server.get(&format!("/connections/{id}/traffic")).await;
    assert_eq!(traffic["inspectable"], json!(true));
    let handles = traffic["handles"].as_array().unwrap();
    assert_eq!(handles.len(), 1, "{traffic}");
    assert_eq!(handles[0]["kind"], json!("tcp"));
    assert_eq!(
        handles[0]["target"],
        json!(format!("127.0.0.1:{}", hub.port))
    );
    let read = texts(&traffic, "in");
    assert!(read.contains(&"END PRELUDE:\n".to_owned()), "{read:?}");
    assert!(read.contains(&"ACK\n".to_owned()), "{read:?}");
    assert_eq!(texts(&traffic, "out"), ["VIDEO OUTPUT ROUTING:\n1 0\n\n"]);

    // A reboot: the core reconnects the same socket, and the plugin goes
    // disconnected and back to ok with a fresh snapshot.
    *hub.routes.lock().unwrap() = [1, 1]; // changed while we were away
    hub.drop_clients();
    server
        .eventually(WAIT, "the reconnect", || async {
            let seen = statuses(&server, &id).await;
            let down = seen.iter().position(|s| s == "disconnected")?;
            seen[down..].contains(&"ok".to_owned()).then_some(())
        })
        .await;
    server
        .eventually(WAIT, "the fresh snapshot", || async {
            (server.value("vh.outputs[0].source").await == Some(json!(1))).then_some(())
        })
        .await;
    let traffic = server.get(&format!("/connections/{id}/traffic")).await;
    let handles = traffic["handles"].as_array().unwrap();
    assert_eq!(handles.len(), 1, "the same handle: {traffic}");
    let notes: Vec<&str> = handles[0]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["note"].as_str())
        .collect();
    assert!(
        notes.iter().any(|n| n.starts_with("reconnecting")),
        "{notes:?}"
    );

    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_direct_network_connection_is_marked_not_inspectable() {
    let server = start();
    let (status, conn) = server
        .post(
            "/connections",
            json!({ "plugin": "io.covalence.test.direct", "alias": "direct", "config": {} }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{conn}");
    let id = conn["id"].as_str().unwrap();
    assert_eq!(
        listed(&server, "direct").await["directNetwork"],
        json!(true)
    );
    let traffic = server.get(&format!("/connections/{id}/traffic")).await;
    assert_eq!(traffic["inspectable"], json!(false));
    assert_eq!(traffic["handles"], json!([]));
    server.stop().await;
}

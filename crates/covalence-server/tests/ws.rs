//! `GET /ws` against the real server, plugin host and Videohub simulator.
//! Needs `deno` on the PATH.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use covalence_server::{App, Config, router};
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tower::ServiceExt;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct Server {
    app: Arc<App>,
    url: String,
    serve: tokio::task::JoinHandle<()>,
    _data: tempfile::TempDir,
}

impl Server {
    async fn start() -> Self {
        let data = tempfile::tempdir().unwrap();
        let app = App::start(Config {
            data_dir: data.path().to_owned(),
            root: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
            plugins: Config::default_plugins(),
            deno: "deno".into(),
        })
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/ws", listener.local_addr().unwrap());
        let routes = router(app.clone());
        let serve = tokio::spawn(async move { axum::serve(listener, routes).await.unwrap() });
        Server {
            app,
            url,
            serve,
            _data: data,
        }
    }

    async fn post(&self, uri: &str, body: Value) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(Method::POST)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let response = router(self.app.clone()).oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn connect(&self) -> Socket {
        tokio_tungstenite::connect_async(&self.url).await.unwrap().0
    }
}

async fn send(ws: &mut Socket, message: Value) {
    ws.send(Message::text(message.to_string())).await.unwrap();
}

/// Reads messages until `want` returns `Some`, or panics after `timeout`.
async fn until<T>(
    ws: &mut Socket,
    timeout: Duration,
    what: &str,
    mut want: impl FnMut(&Value) -> Option<T>,
) -> T {
    let read = async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Text(text))) => {
                    let message: Value = serde_json::from_str(&text).unwrap();
                    if let Some(found) = want(&message) {
                        return found;
                    }
                }
                Some(Ok(_)) => {}
                other => panic!("socket ended waiting for {what}: {other:?}"),
            }
        }
    };
    tokio::time::timeout(timeout, read)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

/// The leaf at `path` in a `patch` or `snapshot`.
fn leaf<'a>(message: &'a Value, path: &str) -> Option<&'a Value> {
    let leaves = match message["type"].as_str()? {
        "patch" => &message["set"],
        "snapshot" => &message["leaves"],
        _ => return None,
    };
    leaves.as_array()?.iter().find(|l| l["path"] == path)
}

#[tokio::test(flavor = "multi_thread")]
async fn live_state_journal_and_host_status() {
    let server = Server::start().await;
    let mut ws = server.connect().await;

    // Host status comes first.
    let first = until(&mut ws, Duration::from_secs(2), "host status", |m| {
        Some(m.clone())
    })
    .await;
    assert_eq!(first["type"], "host", "{first}");

    // Subscribing before the connection exists is fine: nothing is there yet.
    send(
        &mut ws,
        json!({ "op": "subscribe", "paths": ["vh.outputs"] }),
    )
    .await;
    let snapshot = until(&mut ws, Duration::from_secs(2), "snapshot", |m| {
        (m["type"] == "snapshot").then(|| m.clone())
    })
    .await;
    assert_eq!(snapshot["paths"], json!(["vh.outputs"]));
    assert_eq!(snapshot["leaves"], json!([]));

    send(&mut ws, json!({ "op": "journal", "on": true })).await;
    until(&mut ws, Duration::from_secs(2), "journal ack", |m| {
        (m["type"] == "journal" && m["on"] == true).then_some(())
    })
    .await;

    // The simulator's initial state arrives as a patch.
    let (status, conn) = server
        .post(
            "/connections",
            json!({ "plugin": "io.covalence.videohub", "alias": "vh", "config": { "host": "10.0.0.12" } }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{conn}");
    until(&mut ws, Duration::from_secs(10), "initial outputs", |m| {
        leaf(m, "vh.outputs[1].source").map(|_| ())
    })
    .await;

    // A route shows up as a patch, and its result in the journal.
    let (status, outcome) = server
        .post(
            "/actions",
            json!({ "action": "vh.route", "params": { "output": 1, "input": 3 } }),
        )
        .await;
    assert_eq!(
        (status, &outcome["ok"]),
        (StatusCode::OK, &json!(true)),
        "{outcome}"
    );
    let result_seq = outcome["resultSeq"].clone();
    let (mut routed, mut journaled) = (false, false);
    until(&mut ws, Duration::from_secs(2), "the route", |m| {
        routed |= leaf(m, "vh.outputs[1].source").is_some_and(|l| l["value"] == 3);
        journaled |= m["type"] == "journal"
            && m["entry"]["seq"] == result_seq
            && m["entry"]["kind"] == "action.result";
        (routed && journaled).then_some(())
    })
    .await;

    // Only subscribed paths are sent.
    send(
        &mut ws,
        json!({ "op": "unsubscribe", "paths": ["vh.outputs"] }),
    )
    .await;
    send(
        &mut ws,
        json!({ "op": "subscribe", "paths": ["vh.inputs"] }),
    )
    .await;
    until(
        &mut ws,
        Duration::from_secs(2),
        "the inputs snapshot",
        |m| (m["type"] == "snapshot" && m["paths"] == json!(["vh.inputs"])).then_some(()),
    )
    .await;
    send(
        &mut ws,
        json!({ "op": "subscribe", "paths": ["vh.outputs"] }),
    )
    .await;
    until(
        &mut ws,
        Duration::from_secs(2),
        "the outputs snapshot",
        |m| (m["type"] == "snapshot" && m["paths"] == json!(["vh.outputs"])).then_some(()),
    )
    .await;

    // Killing the host: status goes down and values go stale.
    let pid = server.app.core().host().status().borrow().pid.unwrap();
    kill(pid);
    let (mut down, mut stale) = (false, false);
    until(&mut ws, Duration::from_secs(3), "down and stale", |m| {
        down |= m["type"] == "host" && m["host"]["phase"] != "ready";
        stale |= leaf(m, "vh.outputs[1].source").is_some_and(|l| l["quality"] == "stale");
        (down && stale).then_some(())
    })
    .await;
    // ... then it comes back with good values.
    until(&mut ws, Duration::from_secs(6), "resync", |m| {
        leaf(m, "vh.outputs[1].source")
            .is_some_and(|l| l["quality"] == "good")
            .then_some(())
    })
    .await;

    // Bad requests get an error and the session carries on.
    send(&mut ws, json!({ "op": "subscribe", "paths": ["vh..x"] })).await;
    until(&mut ws, Duration::from_secs(2), "an error", |m| {
        (m["type"] == "error").then_some(())
    })
    .await;
    ws.send(Message::text("not json")).await.unwrap();
    until(&mut ws, Duration::from_secs(2), "an error", |m| {
        (m["type"] == "error").then_some(())
    })
    .await;

    // Shutting down closes the socket, so the app isn't held open.
    server.serve.abort();
    let shutdown = tokio::spawn(server.app.clone().shutdown());
    drop(server.app);
    let closed = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "the socket stayed open at shutdown");
    shutdown.await.unwrap();
}

fn kill(pid: u32) {
    #[cfg(unix)]
    let status = std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status();
    #[cfg(windows)]
    let status = std::process::Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .status();
    assert!(status.unwrap().success(), "could not kill pid {pid}");
}

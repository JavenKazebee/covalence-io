//! Milestone 1 acceptance test: the real server, the real Deno plugin host
//! and the Videohub plugin in simulate mode. Needs `deno` on the PATH.
//!
//! Covers every "Done when" item in docs/milestones/m1-walking-skeleton.md.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use covalence_server::{App, Config, router};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

/// The restart budget from docs/architecture.md.
const RESTART_BUDGET: Duration = Duration::from_secs(3);

struct Server {
    app: Arc<App>,
    router: Router,
    _data: tempfile::TempDir,
}

impl Server {
    fn start() -> Self {
        let data = tempfile::tempdir().unwrap();
        let app = App::start(Config {
            data_dir: data.path().to_owned(),
            root: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
            plugins: Config::default_plugins(),
            deno: "deno".into(),
        })
        .unwrap();
        Server {
            router: router(app.clone()),
            app,
            _data: data,
        }
    }

    async fn call(&self, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
            .unwrap();
        let response = self.router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, json)
    }

    async fn get(&self, uri: &str) -> Value {
        let (status, body) = self.call(Method::GET, uri, None).await;
        assert_eq!(status, StatusCode::OK, "GET {uri}: {body}");
        body
    }

    async fn post(&self, uri: &str, body: Value) -> (StatusCode, Value) {
        self.call(Method::POST, uri, Some(body)).await
    }

    /// The value at a text path, if a leaf is there.
    async fn value(&self, path: &str) -> Option<Value> {
        let leaves = self.get(&format!("/state?path={}", encode(path))).await;
        leaves
            .as_array()?
            .iter()
            .find(|l| l["path"] == path)
            .map(|l| l["value"].clone())
    }

    /// Polls `check` until it returns `Some`, or panics after `timeout`.
    async fn eventually<T, F, Fut>(&self, timeout: Duration, what: &str, mut check: F) -> T
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Option<T>>,
    {
        let start = Instant::now();
        loop {
            if let Some(value) = check().await {
                return value;
            }
            assert!(start.elapsed() < timeout, "timed out waiting for {what}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn stop(self) {
        drop(self.router);
        self.app.shutdown().await;
    }
}

/// Percent-encodes the characters a text path can contain that a query can't.
fn encode(path: &str) -> String {
    path.replace('%', "%25")
        .replace('[', "%5B")
        .replace(']', "%5D")
        .replace('"', "%22")
        .replace(' ', "%20")
}

#[tokio::test(flavor = "multi_thread")]
async fn walking_skeleton() {
    let server = Server::start();

    // A Videohub connection in simulate mode.
    let (status, conn) = server
        .post(
            "/connections",
            json!({ "plugin": "io.covalence.videohub", "alias": "vh", "config": { "host": "10.0.0.12" } }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{conn}");
    assert_eq!(conn["mode"], "simulate");
    assert_eq!(conn["permissions"]["net"], json!(["10.0.0.12:9990"]));
    server
        .eventually(Duration::from_secs(10), "the simulator's snapshot", || {
            server.value("vh.outputs[1].source")
        })
        .await;

    // Done when 1: the action journals requested → dispatched → result.
    let (status, outcome) = server
        .post(
            "/actions",
            json!({ "action": "vh.route", "params": { "output": 1, "input": 3 } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{outcome}");
    assert_eq!(outcome["verdict"], "dispatched");
    assert_eq!(outcome["ok"], true, "{outcome}");
    let requested = outcome["requested"].as_u64().unwrap();
    let result_seq = outcome["resultSeq"].as_u64().unwrap();

    let journal = server
        .get(&format!("/journal?from={requested}&limit=1000"))
        .await;
    let kinds_caused_by_request: Vec<&str> = journal
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["seq"] == requested || e["cause"] == requested)
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds_caused_by_request,
        ["action.requested", "action.dispatched", "action.result"]
    );

    // Done when 2: the plugin's state.patch updates vh.outputs[1].source.
    server
        .eventually(
            Duration::from_secs(2),
            "vh.outputs[1].source = 3",
            || async {
                (server.value("vh.outputs[1].source").await == Some(json!(3))).then_some(())
            },
        )
        .await;

    // Done when 3: why walks back from the result to the API request.
    let chain = server.get(&format!("/journal/{result_seq}/why")).await;
    let chain = chain.as_array().unwrap();
    assert_eq!(chain.first().unwrap()["seq"], result_seq);
    let root = chain.last().unwrap();
    assert_eq!(root["kind"], "action.requested");
    assert_eq!(root["origin"], json!({ "user": "api", "client": "http" }));

    // Done when 4: kill the host; it restarts and state resyncs within budget.
    let health = server.get("/health").await;
    let generation = health["host"]["generation"].as_u64().unwrap();
    let pid = health["host"]["pid"].as_u64().unwrap();
    kill(pid);
    let killed = Instant::now();

    server
        .eventually(Duration::from_secs(2), "state to go stale", || async {
            let leaves = server.get("/state?path=vh").await;
            leaves
                .as_array()
                .unwrap()
                .iter()
                .any(|l| l["quality"] == "stale")
                .then_some(())
        })
        .await;
    server
        .eventually(
            RESTART_BUDGET * 2,
            "the host to restart and resync",
            || async {
                let health = server.get("/health").await;
                let restarted = health["host"]["phase"] == "ready"
                    && health["host"]["generation"].as_u64() > Some(generation);
                let leaves = server.get("/state?path=vh").await;
                let leaves = leaves.as_array().unwrap();
                let fresh = !leaves.is_empty() && leaves.iter().all(|l| l["quality"] == "good");
                (restarted && fresh).then_some(())
            },
        )
        .await;
    let restored = killed.elapsed();
    assert!(
        restored < RESTART_BUDGET,
        "host restart → connections restored took {restored:?}, budget {RESTART_BUDGET:?}"
    );

    // Actions work again after the restart.
    let (status, outcome) = server
        .post(
            "/actions",
            json!({ "action": "vh.route", "params": { "output": 0, "input": 2 } }),
        )
        .await;
    assert_eq!(
        (status, &outcome["ok"]),
        (StatusCode::OK, &json!(true)),
        "{outcome}"
    );

    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_bad_requests() {
    let server = Server::start();
    let conn =
        json!({ "plugin": "io.covalence.videohub", "alias": "vh", "config": { "host": "h" } });
    assert_eq!(
        server.post("/connections", conn.clone()).await.0,
        StatusCode::CREATED
    );
    assert_eq!(
        server.post("/connections", conn).await.0,
        StatusCode::CONFLICT
    );

    let cases = [
        (
            "/connections",
            json!({ "plugin": "io.covalence.videohub", "alias": "sys" }),
            StatusCode::BAD_REQUEST,
        ),
        (
            "/connections",
            json!({ "plugin": "io.example.nope", "alias": "x" }),
            StatusCode::NOT_FOUND,
        ),
        // The permission template needs config.host.
        (
            "/connections",
            json!({ "plugin": "io.covalence.videohub", "alias": "v2" }),
            StatusCode::BAD_REQUEST,
        ),
        (
            "/actions",
            json!({ "action": "nope.route" }),
            StatusCode::NOT_FOUND,
        ),
        (
            "/actions",
            json!({ "action": "vh" }),
            StatusCode::BAD_REQUEST,
        ),
        (
            "/actions",
            json!({ "action": "vh.route.x" }),
            StatusCode::BAD_REQUEST,
        ),
    ];
    for (uri, body, expected) in cases {
        let (status, response) = server.post(uri, body.clone()).await;
        assert_eq!(status, expected, "POST {uri} {body}: {response}");
    }
    assert_eq!(
        server
            .call(Method::GET, "/journal/999999/why", None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        server.call(Method::GET, "/state?path=vh..x", None).await.0,
        StatusCode::BAD_REQUEST
    );
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn connections_survive_a_server_restart() {
    let data = tempfile::tempdir().unwrap();
    let config = Config {
        data_dir: data.path().to_owned(),
        root: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
        plugins: Config::default_plugins(),
        deno: "deno".into(),
    };

    let app = App::start(config.clone()).unwrap();
    let first = Server {
        router: router(app.clone()),
        app,
        _data: tempfile::tempdir().unwrap(),
    };
    let body =
        json!({ "plugin": "io.covalence.videohub", "alias": "vh", "config": { "host": "h" } });
    assert_eq!(
        first.post("/connections", body).await.0,
        StatusCode::CREATED
    );
    let last_seq = first.get("/health").await["journal"]["lastSeq"]
        .as_u64()
        .unwrap();
    first.stop().await;

    let app = App::start(config).unwrap();
    let second = Server {
        router: router(app.clone()),
        app,
        _data: tempfile::tempdir().unwrap(),
    };
    let conns = second.get("/connections").await;
    assert_eq!(conns.as_array().unwrap().len(), 1);
    assert_eq!(conns[0]["alias"], "vh");
    // The stored connection is created again, and the journal carries on.
    second
        .eventually(Duration::from_secs(10), "vh state after restart", || {
            second.value("vh.outputs[0].source")
        })
        .await;
    let health = second.get("/health").await;
    assert!(health["journal"]["lastSeq"].as_u64().unwrap() > last_seq);
    second.stop().await;
}

fn kill(pid: u64) {
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

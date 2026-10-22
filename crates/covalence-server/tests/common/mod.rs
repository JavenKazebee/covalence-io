//! The real server, driven through its router, for the end-to-end tests.
//! Needs `deno` on the PATH.

#![allow(dead_code)] // Each test file uses a different part.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use covalence_server::{App, Config, router};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

pub struct Server {
    pub app: Arc<App>,
    pub router: Router,
    pub _data: tempfile::TempDir,
    /// Listeners started by `listen`, stopped by `stop`.
    serving: std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl Server {
    pub fn start() -> Self {
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
            serving: Default::default(),
        }
    }

    /// A server around an app started elsewhere, which owns its data.
    pub fn with_app(app: Arc<App>) -> Self {
        Server {
            router: router(app.clone()),
            app,
            _data: tempfile::tempdir().unwrap(),
            serving: Default::default(),
        }
    }

    /// Serves the router on a local port, for WebSocket clients. Returns
    /// `host:port`.
    pub async fn listen(&self) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let routes = self.router.clone();
        let task = tokio::spawn(async move { axum::serve(listener, routes).await.unwrap() });
        self.serving.lock().unwrap().push(task);
        addr
    }

    pub async fn call(
        &self,
        method: Method,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
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

    pub async fn get(&self, uri: &str) -> Value {
        let (status, body) = self.call(Method::GET, uri, None).await;
        assert_eq!(status, StatusCode::OK, "GET {uri}: {body}");
        body
    }

    pub async fn post(&self, uri: &str, body: Value) -> (StatusCode, Value) {
        self.call(Method::POST, uri, Some(body)).await
    }

    /// The value at a text path, if a leaf is there.
    pub async fn value(&self, path: &str) -> Option<Value> {
        let leaves = self.get(&format!("/state?path={}", encode(path))).await;
        leaves
            .as_array()?
            .iter()
            .find(|l| l["path"] == path)
            .map(|l| l["value"].clone())
    }

    /// Polls `check` until it returns `Some`, or panics after `timeout`.
    pub async fn eventually<T, F, Fut>(&self, timeout: Duration, what: &str, mut check: F) -> T
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

    pub async fn stop(self) {
        let serving = std::mem::take(&mut *self.serving.lock().unwrap());
        for task in serving {
            task.abort();
            let _ = task.await;
        }
        drop(self.router);
        self.app.shutdown().await;
    }
}

/// Percent-encodes the characters a text path can contain that a query can't.
pub fn encode(path: &str) -> String {
    path.replace('%', "%25")
        .replace('[', "%5B")
        .replace(']', "%5D")
        .replace('"', "%22")
        .replace(' ', "%20")
}

impl Server {
    pub async fn put(&self, uri: &str, body: Value) -> (StatusCode, Value) {
        self.call(Method::PUT, uri, Some(body)).await
    }

    pub async fn delete(&self, uri: &str) -> StatusCode {
        self.call(Method::DELETE, uri, None).await.0
    }
}

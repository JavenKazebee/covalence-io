//! Variables over HTTP (dataflow spec §6). Needs `deno`.

use std::path::PathBuf;
use std::time::Duration;

use axum::http::StatusCode;
use covalence_server::{App, Config};
use serde_json::{Value, json};

mod common;
use common::Server;

fn codes(body: &Value) -> Vec<String> {
    body["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["code"].as_str().unwrap().to_owned())
        .collect()
}

async fn created(server: &Server, uri: &str, body: Value) -> Value {
    let (status, out) = server.post(uri, body).await;
    assert_eq!(status, StatusCode::CREATED, "{out}");
    out
}

#[tokio::test]
async fn computed_variables_are_read_only_and_follow_what_they_read() {
    let server = Server::start();
    created(
        &server,
        "/variables",
        json!({ "name": "Mode", "initial": 1, "schema": { "type": "number" } }),
    )
    .await;
    created(
        &server,
        "/variables",
        json!({ "name": "Is live", "value": { "expr": "vars.mode == 3" } }),
    )
    .await;
    assert_eq!(server.value("vars.mode").await, Some(json!(1)));
    assert_eq!(server.value("vars.isLive").await, Some(json!(false)));
    let listed = server.get("/variables").await;
    assert_eq!(listed.as_array().unwrap().len(), 2);

    // A flow can't set a computed variable, and is warned about one that
    // isn't declared.
    let flow = |path: &str| {
        json!({ "name": format!("Set {path}"), "enabled": true, "triggers": [{ "kind": "api" }],
                "concurrency": "parallel",
                "body": [{ "id": "s", "kind": "set", "path": path, "value": "3" }] })
    };
    let (status, body) = server.post("/flows", flow("vars.isLive")).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(codes(&body), ["readOnly"]);
    let (_, body) = server.post("/flows", flow("vars.scratch")).await;
    assert_eq!(body["warnings"][0]["code"], "undeclaredVariable");

    // Setting what it reads moves it.
    let setter = created(&server, "/flows", flow("vars.mode")).await;
    let id = setter["flow"]["id"].as_str().unwrap().to_owned();
    server.post(&format!("/flows/{id}/run"), json!({})).await;
    server
        .eventually(Duration::from_secs(5), "vars.isLive", || async {
            (server.value("vars.isLive").await == Some(json!(true))).then_some(())
        })
        .await;

    // A cycle is refused.
    let (status, body) = server
        .post(
            "/variables",
            json!({ "name": "Loop", "slug": "mode2", "value": { "expr": "vars.isLive2" } }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, body) = server
        .post(
            "/variables",
            json!({ "name": "Loop back", "slug": "isLive2", "value": { "expr": "vars.mode2" } }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(codes(&body), ["cycle"]);
}

#[tokio::test]
async fn a_persisted_toggle_keeps_its_value_and_state_across_a_restart() {
    let data = tempfile::tempdir().unwrap();
    let start = || {
        App::start(Config {
            data_dir: data.path().to_owned(),
            root: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
            plugins: Config::default_plugins(),
            deno: "deno".into(),
            devices: Default::default(),
        })
        .unwrap()
    };
    let flow = json!({ "name": "Go", "enabled": true, "triggers": [{ "kind": "api" }],
                       "concurrency": "parallel",
                       "body": [{ "id": "s", "kind": "set", "path": "local.x", "value": "1" }] });
    // Latches on each run of any flow: no surface involved.
    let armed = json!({
        "name": "Armed",
        "persist": true,
        "value": { "use": "toggle",
                   "on": { "from": { "path": { "match": "flow.*" } }, "event": "started" } }
    });

    let server = Server::with_app(start());
    created(&server, "/variables", armed).await;
    let go = created(&server, "/flows", flow).await;
    let go = go["flow"]["id"].as_str().unwrap().to_owned();
    assert_eq!(server.value("vars.armed").await, Some(json!(false)));
    server.post(&format!("/flows/{go}/run"), json!({})).await;
    server
        .eventually(Duration::from_secs(5), "the toggle to flip", || async {
            (server.value("vars.armed").await == Some(json!(true))).then_some(())
        })
        .await;
    // Let the cache catch up, then restart.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let Server { app, router, .. } = server;
    drop(router);
    app.shutdown().await;

    let server = Server::with_app(start());
    assert_eq!(server.value("vars.armed").await, Some(json!(true)));
    // It carries on from there.
    server.post(&format!("/flows/{go}/run"), json!({})).await;
    server
        .eventually(
            Duration::from_secs(5),
            "the toggle to flip back",
            || async { (server.value("vars.armed").await == Some(json!(false))).then_some(()) },
        )
        .await;
    let Server { app, router, .. } = server;
    drop(router);
    app.shutdown().await;
}

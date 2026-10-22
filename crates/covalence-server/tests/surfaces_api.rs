//! Milestone 5 Phase 3: surfaces, pages and control bindings over HTTP, and
//! the web surface's WebSocket, against the real server, the real Deno
//! plugin host and two Videohubs in simulate mode. Needs `deno` on the PATH.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use axum::http::StatusCode;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use covalence_server::{App, Config};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

mod common;
use common::Server;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

const WAIT: Duration = Duration::from_secs(10);
const KEY: u32 = 144;

/// Creates a Videohub connection and waits for its simulator's state.
async fn videohub(server: &Server, alias: &str) -> String {
    let (status, conn) = server
        .post(
            "/connections",
            json!({ "plugin": "io.covalence.videohub", "alias": alias, "config": { "host": "h" } }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{conn}");
    let path = format!("{alias}.outputs[1].source");
    server.eventually(WAIT, &path, || server.value(&path)).await;
    conn["id"].as_str().unwrap().to_owned()
}

/// Creates an entity and returns it as stored.
async fn create(server: &Server, uri: &str, kind: &str, body: Value) -> Value {
    let (status, saved) = server.post(uri, body).await;
    assert_eq!(status, StatusCode::CREATED, "POST {uri}: {saved}");
    saved[kind].clone()
}

/// The `{path, code}` of each error in a `422`.
fn codes(body: &Value) -> Vec<(Value, String)> {
    body["errors"]
        .as_array()
        .unwrap_or_else(|| panic!("no errors in {body}"))
        .iter()
        .map(|e| (e["path"].clone(), e["code"].as_str().unwrap().to_owned()))
        .collect()
}

fn id(entity: &Value) -> &str {
    entity["id"].as_str().unwrap()
}

/// A 5 × 3 page.
fn deck_page(name: &str) -> Value {
    json!({ "name": name, "layout": { "cols": 5, "rows": 3 } })
}

/// The label of `control`, from a connection's value.
fn label_of(page: &str, control: &str, conn: &str) -> Value {
    json!({
        "page": page, "control": control,
        "props": { "label": { "kind": "value", "path": { "conn": conn, "path": ["outputs", 1, "source"] } } },
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn surfaces_pages_and_bindings_are_validated_and_stored() {
    let server = Server::start();

    // Pages: a slug from the name, and layouts that make sense.
    let cams = create(&server, "/pages", "page", deck_page("Cameras")).await;
    assert_eq!(cams["slug"], "cameras");
    let (status, body) = server
        .post(
            "/pages",
            json!({ "name": "Bad", "layout": { "cols": 0, "rows": 3 } }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(codes(&body), [(json!(["layout"]), "invalid".into())]);
    let (status, body) = server
        .post(
            "/pages",
            json!({ "name": "Cams", "slug": "cameras", "layout": { "cols": 5, "rows": 3 } }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(codes(&body), [(json!(["slug"]), "duplicateSlug".into())]);
    let (status, body) = server.post("/pages", json!({ "name": "x" })).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(codes(&body)[0].1, "invalid", "{body}");

    // Surfaces: a start page that exists.
    let deck = create(
        &server,
        "/surfaces",
        "surface",
        json!({ "name": "FOH deck", "model": "virtual.deck15", "startPage": id(&cams) }),
    )
    .await;
    assert_eq!(deck["slug"], "fohDeck");
    let (status, body) = server
        .post(
            "/surfaces",
            json!({ "name": "Other", "model": "virtual.deck15", "startPage": "nope" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(codes(&body), [(json!(["startPage"]), "unknownPage".into())]);

    // Bindings: on a page, inside its layout, one per control.
    let k14 = create(
        &server,
        "/control-bindings",
        "binding",
        json!({
            "page": id(&cams), "control": "k14",
            "props": { "label": { "kind": "expr", "source": "'Hi'" } },
        }),
    )
    .await;
    for (control, code) in [("k14", "duplicateControl"), ("k15", "unknownControl")] {
        let (status, body) = server
            .post(
                "/control-bindings",
                json!({ "page": id(&cams), "control": control, "props": {} }),
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{control}");
        assert_eq!(codes(&body), [(json!(["control"]), code.into())]);
    }
    let (status, body) = server
        .post(
            "/control-bindings",
            json!({
                "page": id(&cams), "control": "k0",
                "props": { "background": { "kind": "expr", "source": "1 +", "fallback": "#000" } },
            }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(codes(&body)[0].1, "exprSyntax", "{body}");

    let audio = create(&server, "/pages", "page", deck_page("Audio")).await;
    let on_audio = create(
        &server,
        "/control-bindings",
        "binding",
        json!({ "page": id(&audio), "control": "k0", "props": {} }),
    )
    .await;
    let listed = server
        .get(&format!("/control-bindings?page={}", id(&cams)))
        .await;
    assert_eq!(listed, json!([k14]));
    assert_eq!(
        server
            .get("/control-bindings")
            .await
            .as_array()
            .unwrap()
            .len(),
        2
    );

    // A page can't shrink past a bound control.
    let (status, body) = server
        .put(
            &format!("/pages/{}", id(&cams)),
            json!({ "name": "Cameras", "layout": { "cols": 2, "rows": 2 } }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(codes(&body), [(json!(["layout"]), "unknownControl".into())]);

    // Flows can now save `gesture` triggers, checked against pages.
    let gesture_flow = |page: &str| {
        json!({
            "name": "On press",
            "enabled": true, "concurrency": "ignore",
            "triggers": [{ "kind": "gesture", "control": { "page": page, "control": "k3" }, "gesture": "press" }],
            "body": [{ "id": "s", "kind": "set", "path": "vars.pressed", "value": "true" }],
        })
    };
    let (status, body) = server.post("/flows", gesture_flow(id(&cams))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, body) = server.post("/flows", gesture_flow("nope")).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(codes(&body)[0].1, "unknownPage", "{body}");

    // Showing a page: state, unknown pages, and nothing.
    let show = format!("/surfaces/{}/page", id(&deck));
    let (status, body) = server.post(&show, json!({ "page": id(&audio) })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        server.value("surface.fohDeck.page").await,
        Some(json!("audio"))
    );
    let (status, body) = server.post(&show, json!({ "page": "nope" })).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "unknownPage");
    let (status, _) = server.post(&show, json!({ "page": null })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        server.value("surface.fohDeck.page").await,
        Some(json!(null))
    );
    let (status, _) = server
        .post("/surfaces/nope/page", json!({ "page": null }))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Deleting a page deletes its bindings and clears start pages.
    assert_eq!(
        server.delete(&format!("/pages/{}", id(&cams))).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(server.get("/control-bindings").await, json!([on_audio]));
    let deck = server.get(&format!("/surfaces/{}", id(&deck))).await;
    assert!(deck.get("startPage").is_none(), "{deck}");
    assert_eq!(
        server.delete(&format!("/pages/{}", id(&cams))).await,
        StatusCode::NOT_FOUND
    );

    // Deleting a surface deletes its state.
    assert_eq!(
        server.delete(&format!("/surfaces/{}", id(&deck))).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(server.value("surface.fohDeck.page").await, None);
    assert_eq!(server.get("/surfaces").await, json!([]));

    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn surfaces_pages_and_bindings_survive_a_restart() {
    let data = tempfile::tempdir().unwrap();
    let start = || {
        App::start(Config {
            data_dir: data.path().to_owned(),
            root: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
            plugins: Config::default_plugins(),
            deno: "deno".into(),
        })
        .unwrap()
    };

    let app = start();
    let (page, _) = app.save_page(&deck_page("Cameras"), None).unwrap();
    let (surface, _) = app
        .save_surface(
            &json!({ "name": "FOH deck", "model": "virtual.deck15", "startPage": page.id }),
            None,
        )
        .unwrap();
    let (binding, _) = app
        .save_binding(
            &json!({ "page": page.id, "control": "k0", "props": { "label": { "kind": "expr", "source": "'Hi'" } } }),
            None,
        )
        .unwrap();
    app.shutdown().await;

    let app = start();
    assert_eq!(
        app.pages(),
        BTreeMap::from([(page.id.clone(), page.clone())])
    );
    assert_eq!(
        app.surface_entities(),
        BTreeMap::from([(surface.id.clone(), surface)])
    );
    assert_eq!(
        app.control_bindings(),
        BTreeMap::from([(binding.id.clone(), binding)])
    );
    // Pages reach the core, which validates against them.
    assert_eq!(app.surfaces().layouts()[&page.id], page.layout);
    app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn bindings_preview_with_their_look_and_image() {
    let server = Server::start();
    let vh = videohub(&server, "vh").await;
    let page = create(&server, "/pages", "page", deck_page("Cameras")).await;

    let preview =
        |binding: Value| server.post("/control-bindings/preview", json!({ "binding": binding }));
    let (status, body) = preview(json!({
        "page": id(&page), "control": "k0",
        "props": {
            "label": { "kind": "template", "parts": ["Out 1: ", { "path": { "conn": vh, "path": ["outputs", 1, "source"] } }] },
            "background": {
                "kind": "rules",
                "rules": [{ "when": { "path": { "conn": vh, "path": ["outputs", 1, "source"] }, "op": "==", "value": 1 }, "value": "#e02020" }],
                "else": "#333333",
            },
            "textColor": { "kind": "expr", "source": "nope()", "fallback": "#ffff00" },
        },
    }))
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["look"],
        json!({ "label": "Out 1: 1", "textColor": "#ffff00", "background": "#e02020", "stale": false })
    );
    let label = &body["props"]["label"];
    assert_eq!(label["value"], "Out 1: 1");
    assert_eq!(label["reads"], json!(["vh.outputs[1].source"]));
    assert!(
        label["expr"]
            .as_str()
            .unwrap()
            .contains("vh.outputs[1].source"),
        "the compiled expression uses the alias: {label}"
    );
    let text_color = &body["props"]["textColor"];
    assert_eq!(text_color["value"], "#ffff00", "the fallback");
    assert!(
        text_color["error"].as_str().unwrap().contains("nope"),
        "{text_color}"
    );
    assert_eq!(text_color["expr"], "nope()");
    let png = BASE64.decode(body["image"].as_str().unwrap()).unwrap();
    assert_eq!(&png[1..4], b"PNG");
    assert_eq!(png_size(&png), (KEY, KEY));

    let (status, body) = server
        .post(
            "/control-bindings/preview",
            json!({ "binding": { "page": id(&page), "control": "k0", "props": {} }, "size": 72 }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["look"]["label"], "");
    let png = BASE64.decode(body["image"].as_str().unwrap()).unwrap();
    assert_eq!(png_size(&png), (72, 72));

    // A binding that doesn't parse is a 422, as on save.
    let (status, body) = preview(
        json!({ "page": id(&page), "control": "k0", "props": { "label": { "kind": "nope" } } }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(codes(&body)[0].0, json!(["props", "label", "kind"]));

    server.stop().await;
}

/// A PNG's width and height, from its header.
fn png_size(png: &[u8]) -> (u32, u32) {
    let be = |at: usize| u32::from_be_bytes(png[at..at + 4].try_into().unwrap());
    (be(16), be(20))
}

/// The virtual deck's `surface.attach`.
fn attach(slug: &str) -> Value {
    let controls: Vec<Value> = (0..15)
        .map(|k| {
            json!({
                "id": format!("k{k}"), "kind": "button", "x": k % 5, "y": k / 5,
                "image": { "w": KEY, "h": KEY, "format": "png" },
            })
        })
        .collect();
    json!({
        "method": "surface.attach",
        "params": {
            "surface": slug, "model": "virtual.deck15", "name": "Virtual deck",
            "controls": controls, "layout": { "cols": 5, "rows": 3 },
        },
    })
}

fn input(slug: &str, control: &str, kind: &str, ts: f64) -> Value {
    json!({
        "method": "surface.input",
        "params": { "surface": slug, "control": control, "kind": kind, "ts": ts },
    })
}

async fn connect(addr: &str, surface: &str) -> Socket {
    let url = format!("ws://{addr}/surface/{surface}");
    tokio_tungstenite::connect_async(&url).await.unwrap().0
}

async fn send(ws: &mut Socket, message: Value) {
    ws.send(Message::text(message.to_string())).await.unwrap();
}

/// The next `surface.draw`: `(control, base64 image)`.
async fn next_draw(ws: &mut Socket, slug: &str) -> (String, String) {
    let read = async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Text(text))) => {
                    let message: Value = serde_json::from_str(&text).unwrap();
                    assert_eq!(message["method"], "surface.draw", "{message}");
                    let params = &message["params"];
                    assert_eq!(params["surface"], slug);
                    assert_eq!(params["format"], "png");
                    return (
                        params["control"].as_str().unwrap().to_owned(),
                        params["image"].as_str().unwrap().to_owned(),
                    );
                }
                Some(Ok(_)) => {}
                other => panic!("socket ended waiting for a draw: {other:?}"),
            }
        }
    };
    tokio::time::timeout(WAIT, read)
        .await
        .expect("timed out waiting for a draw")
}

/// Reads until the socket closes, and returns its close code and reason.
async fn closed(ws: &mut Socket) -> (CloseCode, String) {
    let read = async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Close(Some(frame)))) => {
                    return (frame.code, frame.reason.to_string());
                }
                Some(Ok(_)) => {}
                other => panic!("socket ended without a close frame: {other:?}"),
            }
        }
    };
    tokio::time::timeout(WAIT, read)
        .await
        .expect("timed out waiting for the socket to close")
}

/// A binding's previewed label and base64 image.
async fn previewed(server: &Server, binding: &Value) -> (Value, String) {
    let (status, body) = server
        .post("/control-bindings/preview", json!({ "binding": binding }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    (
        body["look"]["label"].clone(),
        body["image"].as_str().unwrap().to_owned(),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn a_web_surface_presses_routes_and_redraws() {
    let server = Server::start();
    let addr = server.listen().await;
    let vh = videohub(&server, "vh").await;
    let vh2 = videohub(&server, "vh2").await;

    let page = create(&server, "/pages", "page", deck_page("Cameras")).await;
    let deck = create(
        &server,
        "/surfaces",
        "surface",
        json!({ "name": "FOH deck", "model": "virtual.deck15", "startPage": id(&page) }),
    )
    .await;
    let k0 = label_of(id(&page), "k0", &vh);
    create(&server, "/control-bindings", "binding", k0.clone()).await;
    create(
        &server,
        "/control-bindings",
        "binding",
        label_of(id(&page), "k2", &vh2),
    )
    .await;
    let (status, body) = server
        .post(
            "/flows",
            json!({
                "name": "Route on press",
                "enabled": true, "concurrency": "ignore",
                "triggers": [{ "kind": "gesture", "control": { "page": id(&page), "control": "k1" }, "gesture": "press" }],
                "body": [{
                    "id": "route1", "kind": "action", "target": { "conn": vh },
                    "action": "route", "params": { "output": 1, "input": 3 },
                }],
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    // Attaching: state, then every key drawn, the bound ones as previewed.
    let mut ws = connect(&addr, id(&deck)).await;
    send(&mut ws, attach("fohDeck")).await;
    let mut drawn = BTreeMap::new();
    while drawn.len() < 15 {
        let (control, image) = next_draw(&mut ws, "fohDeck").await;
        drawn.insert(control, image);
    }
    assert_eq!(
        server.value("surface.fohDeck.connected").await,
        Some(json!(true))
    );
    assert_eq!(
        server.value("surface.fohDeck.page").await,
        Some(json!("cameras"))
    );
    assert_eq!(
        server.value("surface.fohDeck.model").await,
        Some(json!("virtual.deck15"))
    );
    let (label, image) = previewed(&server, &k0).await;
    assert_eq!(label, "1");
    assert_eq!(
        drawn["k0"], image,
        "the key is drawn as the preview draws it"
    );

    // Pressing k1 routes vh; k0 is redrawn with the new source.
    send(&mut ws, input("fohDeck", "k1", "down", 1000.0)).await;
    send(&mut ws, input("fohDeck", "k1", "up", 1080.0)).await;
    let (control, image) = next_draw(&mut ws, "fohDeck").await;
    assert_eq!(control, "k0");
    let (label, previewed_image) = previewed(&server, &k0).await;
    assert_eq!(label, "3");
    assert_eq!(image, previewed_image);

    // `why` on the route's state change walks back to the raw input.
    let leaves = server
        .get(&format!(
            "/state?path={}",
            common::encode("vh.outputs[1].source")
        ))
        .await;
    let patch = leaves[0]["seq"].as_u64().unwrap();
    let chain = server.get(&format!("/journal/{patch}/why")).await;
    let kinds: Vec<&str> = chain
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds.first(), Some(&"state.patch"), "{kinds:?}");
    let tail = &kinds[kinds.len() - 3..];
    assert_eq!(
        tail,
        ["trigger.matched", "input.gesture", "input.raw"],
        "{kinds:?}"
    );
    let raw = chain.as_array().unwrap().last().unwrap();
    assert_eq!(raw["data"]["control"], "k1", "{raw}");

    // State k0 doesn't read redraws only the key that reads it.
    let (status, body) = server
        .post(
            "/actions",
            json!({ "action": "vh2.route", "params": { "output": 1, "input": 5 } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (control, _) = next_draw(&mut ws, "fohDeck").await;
    assert_eq!(control, "k2", "vh2 changed, so k0 isn't redrawn");

    // Editing a binding while shown redraws its key before the API answers.
    let mut edited = k0.clone();
    edited["props"]["background"] = json!({ "kind": "expr", "source": "'#0000ff'" });
    let k0_id = server
        .get(&format!("/control-bindings?page={}", id(&page)))
        .await
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["control"] == "k0")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (status, body) = server
        .put(&format!("/control-bindings/{k0_id}"), edited)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (control, _) = next_draw(&mut ws, "fohDeck").await;
    assert_eq!(control, "k0");

    // Switching to a page with nothing bound blanks the bound keys.
    let empty = create(&server, "/pages", "page", deck_page("Empty")).await;
    let (status, body) = server
        .post(
            &format!("/surfaces/{}/page", id(&deck)),
            json!({ "page": id(&empty) }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let mut blanked = vec![
        next_draw(&mut ws, "fohDeck").await.0,
        next_draw(&mut ws, "fohDeck").await.0,
    ];
    blanked.sort();
    assert_eq!(blanked, ["k0", "k2"]);
    assert_eq!(
        server.value("surface.fohDeck.page").await,
        Some(json!("empty"))
    );

    // Closing the socket detaches the surface.
    ws.close(None).await.unwrap();
    server
        .eventually(WAIT, "the surface to detach", || async {
            (server.value("surface.fohDeck.connected").await == Some(json!(false))).then_some(())
        })
        .await;

    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_web_surface_must_attach_as_itself() {
    let server = Server::start();
    let addr = server.listen().await;
    let page = create(&server, "/pages", "page", deck_page("Cameras")).await;
    let deck = create(
        &server,
        "/surfaces",
        "surface",
        json!({ "name": "FOH deck", "model": "virtual.deck15", "startPage": id(&page) }),
    )
    .await;

    // An unknown surface is refused before the upgrade.
    let refused = tokio_tungstenite::connect_async(format!("ws://{addr}/surface/nope")).await;
    assert!(refused.is_err());

    // Another surface's slug, input before attaching, a bad layout.
    for (message, reason) in [
        (attach("other"), "not \"other\""),
        (
            input("fohDeck", "k0", "down", 1.0),
            "expected surface.attach",
        ),
        (json!({ "nope": 1 }), "bad notification"),
    ] {
        let mut ws = connect(&addr, id(&deck)).await;
        send(&mut ws, message).await;
        let (code, why) = closed(&mut ws).await;
        assert_eq!(code, CloseCode::Policy, "{why}");
        assert!(why.contains(reason), "{why:?} should say {reason:?}");
    }
    let mut bad = attach("fohDeck");
    bad["params"]["layout"] = json!({ "cols": 2, "rows": 2 });
    let mut ws = connect(&addr, id(&deck)).await;
    send(&mut ws, bad).await;
    let (code, why) = closed(&mut ws).await;
    assert_eq!(code, CloseCode::Policy);
    assert!(why.starts_with("cannot attach"), "{why}");

    // A second attach replaces the first, whose socket is closed.
    let mut first = connect(&addr, id(&deck)).await;
    send(&mut first, attach("fohDeck")).await;
    for _ in 0..15 {
        next_draw(&mut first, "fohDeck").await;
    }
    let mut second = connect(&addr, id(&deck)).await;
    send(&mut second, attach("fohDeck")).await;
    let (code, why) = closed(&mut first).await;
    assert_eq!(
        (code, why.as_str()),
        (CloseCode::Normal, "attached elsewhere")
    );
    for _ in 0..15 {
        next_draw(&mut second, "fohDeck").await;
    }

    // Input with another slug closes the socket; deleting the surface too.
    send(&mut second, input("other", "k0", "down", 1.0)).await;
    let (code, _) = closed(&mut second).await;
    assert_eq!(code, CloseCode::Policy);
    let mut third = connect(&addr, id(&deck)).await;
    send(&mut third, attach("fohDeck")).await;
    for _ in 0..15 {
        next_draw(&mut third, "fohDeck").await;
    }
    assert_eq!(
        server.delete(&format!("/surfaces/{}", id(&deck))).await,
        StatusCode::NO_CONTENT
    );
    let (code, why) = closed(&mut third).await;
    assert_eq!((code, why.as_str()), (CloseCode::Normal, "surface deleted"));

    server.stop().await;
}

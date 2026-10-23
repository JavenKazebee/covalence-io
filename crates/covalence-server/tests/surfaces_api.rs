//! Milestone 5 Phase 3: surfaces, layers and control bindings over HTTP, and
//! the web surface's WebSocket, against the real server, the real Deno
//! plugin host and two Videohubs in simulate mode. Needs `deno` on the PATH.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use axum::http::{Method, StatusCode};
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

/// A layer of the virtual deck's 15 keys.
fn deck_layer(name: &str) -> Value {
    json!({ "name": name, "controls": keys(15) })
}

/// `k0` … `k<n - 1>`, 5 to a row, each `momentary` with an `image`.
fn keys(n: u32) -> Vec<Value> {
    (0..n)
        .map(|k| {
            json!({
                "id": format!("k{k}"), "at": { "x": k % 5, "y": k / 5 },
                "inputs": ["momentary"], "outputs": ["image"],
            })
        })
        .collect()
}

/// The label of `control`, from a connection's value.
fn label_of(layer: &str, control: &str, conn: &str) -> Value {
    json!({
        "layer": layer, "control": control,
        "props": { "label": { "kind": "value", "path": { "conn": conn, "path": ["outputs", 1, "source"] } } },
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn surfaces_layers_and_bindings_are_validated_and_stored() {
    let server = Server::start();

    // Layers: a slug from the name, and each control listed once.
    let cams = create(&server, "/layers", "layer", deck_layer("Cameras")).await;
    assert_eq!(cams["slug"], "cameras");
    let mut twice = deck_layer("Bad");
    twice["controls"][1]["id"] = json!("k0");
    let (status, body) = server.post("/layers", twice).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        codes(&body),
        [(json!(["controls", 1, "id"]), "duplicateControl".into())]
    );
    let (status, body) = server
        .post(
            "/layers",
            json!({ "name": "Cams", "slug": "cameras", "controls": keys(15) }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(codes(&body), [(json!(["slug"]), "duplicateSlug".into())]);
    let (status, body) = server.post("/layers", json!({ "name": "x" })).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(codes(&body)[0].1, "invalid", "{body}");

    // Surfaces: a start layer that exists.
    let deck = create(
        &server,
        "/surfaces",
        "surface",
        json!({ "name": "FOH deck", "model": "virtual.deck15", "startLayer": id(&cams) }),
    )
    .await;
    assert_eq!(deck["slug"], "fohDeck");
    let (status, body) = server
        .post(
            "/surfaces",
            json!({ "name": "Other", "model": "virtual.deck15", "startLayer": "nope" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        codes(&body),
        [(json!(["startLayer"]), "unknownLayer".into())]
    );

    // Bindings: on a layer, on one of its controls, one per control.
    let k14 = create(
        &server,
        "/control-bindings",
        "binding",
        json!({
            "layer": id(&cams), "control": "k14",
            "props": { "label": { "kind": "expr", "source": "'Hi'" } },
        }),
    )
    .await;
    for (control, code) in [("k14", "duplicateControl"), ("k15", "unknownControl")] {
        let (status, body) = server
            .post(
                "/control-bindings",
                json!({ "layer": id(&cams), "control": control, "props": {} }),
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{control}");
        assert_eq!(codes(&body), [(json!(["control"]), code.into())]);
    }
    let (status, body) = server
        .post(
            "/control-bindings",
            json!({
                "layer": id(&cams), "control": "k0",
                "props": { "background": { "kind": "expr", "source": "1 +", "fallback": "#000" } },
            }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(codes(&body)[0].1, "exprSyntax", "{body}");

    let audio = create(&server, "/layers", "layer", deck_layer("Audio")).await;
    let on_audio = create(
        &server,
        "/control-bindings",
        "binding",
        json!({ "layer": id(&audio), "control": "k0", "props": {} }),
    )
    .await;
    let listed = server
        .get(&format!("/control-bindings?layer={}", id(&cams)))
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

    // A layer can't drop a bound control, or the output its binding drives.
    let (status, body) = server
        .put(
            &format!("/layers/{}", id(&cams)),
            json!({ "name": "Cameras", "controls": keys(4) }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        codes(&body),
        [(json!(["controls"]), "unknownControl".into())]
    );
    let mut dark = deck_layer("Cameras");
    dark["controls"][14]["outputs"] = json!([]);
    let (status, body) = server
        .put(&format!("/layers/{}", id(&cams)), dark.clone())
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(codes(&body), [(json!(["controls"]), "noOutput".into())]);
    // And a binding needs its control's output.
    dark["controls"][14]["outputs"] = json!(["image"]);
    dark["controls"][13]["outputs"] = json!([]);
    let (status, body) = server.put(&format!("/layers/{}", id(&cams)), dark).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = server
        .post(
            "/control-bindings",
            json!({
                "layer": id(&cams), "control": "k13",
                "props": { "label": { "kind": "expr", "source": "'Hi'" } },
            }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        codes(&body),
        [(json!(["props", "label"]), "noOutput".into())]
    );

    // Flows can now save `gesture` triggers, checked against layers.
    let gesture_flow = |layer: &str| {
        json!({
            "name": "On press",
            "enabled": true, "concurrency": "ignore",
            "triggers": [{ "kind": "gesture", "control": { "layer": layer, "control": "k3" }, "gesture": "press" }],
            "body": [{ "id": "s", "kind": "set", "path": "vars.pressed", "value": "true" }],
        })
    };
    let (status, body) = server.post("/flows", gesture_flow(id(&cams))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, body) = server.post("/flows", gesture_flow("nope")).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(codes(&body)[0].1, "unknownLayer", "{body}");

    // Navigating: state, unknown layers, other groups, and nothing.
    let show = format!("/surfaces/{}/current", id(&deck));
    let (status, body) = server.post(&show, json!({ "layer": id(&audio) })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        server.value("surface.fohDeck.current.main").await,
        Some(json!("audio"))
    );
    assert_eq!(body["group"], "main");
    let (status, body) = server.post(&show, json!({ "layer": "nope" })).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "unknownLayer");
    let (status, _) = server
        .post(&show, json!({ "group": "popover", "layer": null }))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    // It has never attached, so it has no controls to copy yet.
    let (status, _) = server
        .call(
            Method::GET,
            &format!("/surfaces/{}/controls", id(&deck)),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = server.post(&show, json!({ "layer": null })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        server.value("surface.fohDeck.current.main").await,
        Some(json!(null))
    );
    let (status, _) = server
        .post("/surfaces/nope/current", json!({ "layer": null }))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Deleting a layer deletes its bindings and clears start layers.
    assert_eq!(
        server.delete(&format!("/layers/{}", id(&cams))).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(server.get("/control-bindings").await, json!([on_audio]));
    let deck = server.get(&format!("/surfaces/{}", id(&deck))).await;
    assert!(deck.get("startLayer").is_none(), "{deck}");
    assert_eq!(
        server.delete(&format!("/layers/{}", id(&cams))).await,
        StatusCode::NOT_FOUND
    );

    // Deleting a surface deletes its state.
    assert_eq!(
        server.delete(&format!("/surfaces/{}", id(&deck))).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(server.value("surface.fohDeck.current.main").await, None);
    assert_eq!(server.get("/surfaces").await, json!([]));

    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn surfaces_layers_and_bindings_survive_a_restart() {
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
    let (layer, _) = app.save_layer(&deck_layer("Cameras"), None).unwrap();
    let (surface, _) = app
        .save_surface(
            &json!({ "name": "FOH deck", "model": "virtual.deck15", "startLayer": layer.id }),
            None,
        )
        .unwrap();
    let (binding, _) = app
        .save_binding(
            &json!({ "layer": layer.id, "control": "k0", "props": { "label": { "kind": "expr", "source": "'Hi'" } } }),
            None,
        )
        .unwrap();
    // A key held when the server stops.
    let attach: covalence_core::surface::Attach = {
        let mut params = attach("fohDeck")["params"].clone();
        params.as_object_mut().unwrap().remove("surface");
        serde_json::from_value(params).unwrap()
    };
    let link = app.surfaces().attach(&surface.id, attach).unwrap();
    link.input
        .send(covalence_core::surface::Input {
            control: "k0".into(),
            kind: covalence_core::journal::InputKind::Down,
            value: None,
            ts: 1.0,
        })
        .await
        .unwrap();
    let pressed = covalence_core::state::StatePath::new(
        "surface",
        ["fohDeck", "controls", "k0", "pressed"]
            .into_iter()
            .map(|k| covalence_protocol::state::PathSegment::Key(k.into()))
            .collect(),
    );
    let held = async {
        while app.core().shared().state.value(&pressed) != Some(true.into()) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    };
    tokio::time::timeout(WAIT, held).await.unwrap();
    app.shutdown().await;
    drop(link);

    let app = start();
    assert_eq!(
        app.core().shared().state.value(&pressed),
        None,
        "input state isn't restored"
    );
    assert_eq!(
        app.layers(),
        BTreeMap::from([(layer.id.clone(), layer.clone())])
    );
    assert_eq!(
        app.surface_entities(),
        BTreeMap::from([(surface.id.clone(), surface)])
    );
    assert_eq!(
        app.control_bindings(),
        BTreeMap::from([(binding.id.clone(), binding)])
    );
    // Layers reach the core, which validates against them.
    assert_eq!(app.surfaces().layers()[&layer.id], layer);
    app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn bindings_preview_with_their_look_and_image() {
    let server = Server::start();
    let vh = videohub(&server, "vh").await;
    let layer = create(&server, "/layers", "layer", deck_layer("Cameras")).await;

    let preview =
        |binding: Value| server.post("/control-bindings/preview", json!({ "binding": binding }));
    let (status, body) = preview(json!({
        "layer": id(&layer), "control": "k0",
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
            json!({ "binding": { "layer": id(&layer), "control": "k0", "props": {} }, "size": 72 }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["look"]["label"], "");
    let png = BASE64.decode(body["image"].as_str().unwrap()).unwrap();
    assert_eq!(png_size(&png), (72, 72));

    // A binding that doesn't parse is a 422, as on save.
    let (status, body) = preview(
        json!({ "layer": id(&layer), "control": "k0", "props": { "label": { "kind": "nope" } } }),
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
                "id": format!("k{k}"), "at": { "x": k % 5, "y": k / 5 },
                "inputs": [{ "kind": "momentary" }],
                "outputs": [{ "kind": "image", "w": KEY, "h": KEY, "format": "png" }],
            })
        })
        .collect();
    json!({
        "method": "surface.attach",
        "params": {
            "surface": slug, "model": "virtual.deck15", "name": "Virtual deck",
            "controls": controls,
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

    let layer = create(&server, "/layers", "layer", deck_layer("Cameras")).await;
    let deck = create(
        &server,
        "/surfaces",
        "surface",
        json!({ "name": "FOH deck", "model": "virtual.deck15", "startLayer": id(&layer) }),
    )
    .await;
    let k0 = label_of(id(&layer), "k0", &vh);
    create(&server, "/control-bindings", "binding", k0.clone()).await;
    create(
        &server,
        "/control-bindings",
        "binding",
        label_of(id(&layer), "k2", &vh2),
    )
    .await;
    let (status, body) = server
        .post(
            "/flows",
            json!({
                "name": "Route on press",
                "enabled": true, "concurrency": "ignore",
                "triggers": [{ "kind": "gesture", "control": { "layer": id(&layer), "control": "k1" }, "gesture": "press" }],
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
        server.value("surface.fohDeck.current.main").await,
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

    // `why` on the route's state change walks back to the input's patch.
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
        ["trigger.matched", "input.event", "state.patch"],
        "{kinds:?}"
    );
    let input = chain.as_array().unwrap().last().unwrap();
    assert_eq!(input["data"]["deviceTs"], json!(1000.0), "{input}");
    assert_eq!(
        input["data"]["ops"][0]["path"]["path"],
        json!(["fohDeck", "controls", "k1", "pressed"]),
        "{input}"
    );

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
        .get(&format!("/control-bindings?layer={}", id(&layer)))
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

    // Switching to a layer with nothing bound blanks the bound keys.
    let empty = create(&server, "/layers", "layer", deck_layer("Empty")).await;
    let (status, body) = server
        .post(
            &format!("/surfaces/{}/current", id(&deck)),
            json!({ "layer": id(&empty) }),
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
        server.value("surface.fohDeck.current.main").await,
        Some(json!("empty"))
    );

    // What the deck attached with, for "New layer".
    let controls = server
        .get(&format!("/surfaces/{}/controls", id(&deck)))
        .await;
    assert_eq!(controls.as_array().unwrap().len(), 15);
    assert_eq!(controls[7]["at"], json!({ "x": 2, "y": 1 }));
    assert_eq!(controls[7]["inputs"], json!([{ "kind": "momentary" }]));

    // A layer with keys the deck lacks doesn't fit.
    let wide = create(
        &server,
        "/layers",
        "layer",
        json!({ "name": "Wide", "controls": keys(16) }),
    )
    .await;
    let (status, body) = server
        .post(
            &format!("/surfaces/{}/current", id(&deck)),
            json!({ "layer": id(&wide) }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "doesNotFit");
    assert_eq!(body["controls"], json!(["k15"]));

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
    let layer = create(&server, "/layers", "layer", deck_layer("Cameras")).await;
    let deck = create(
        &server,
        "/surfaces",
        "surface",
        json!({ "name": "FOH deck", "model": "virtual.deck15", "startLayer": id(&layer) }),
    )
    .await;

    // An unknown surface is refused before the upgrade.
    let refused = tokio_tungstenite::connect_async(format!("ws://{addr}/surface/nope")).await;
    assert!(refused.is_err());

    // Another surface's slug, input before attaching, overlapping keys.
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
    bad["params"]["controls"][1]["at"] = json!({ "x": 0, "y": 0 });
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

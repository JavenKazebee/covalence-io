//! Surfaces and components over HTTP, and
//! the web surface's WebSocket, against the real server, the real Deno
//! plugin host and two Videohubs in simulate mode. Needs `deno` on the PATH.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use axum::http::{Method, StatusCode};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use covalence_core::component::Component;
use covalence_core::surface::Surface;
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

/// A page of the virtual deck's 15 keys.
fn deck_page(name: &str) -> Value {
    Server::page(name, json!(keys(15)))
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
fn label_of(control: &str, conn: &str) -> Value {
    json!({
        "control": control,
        "props": { "label": { "ref": { "conn": conn, "path": ["outputs", 1, "source"] } } },
    })
}

/// Puts a component back with `edit` made to it.
async fn edit(
    server: &Server,
    component: &str,
    edit: impl FnOnce(&mut Value),
) -> (StatusCode, Value) {
    let uri = format!("/components/{component}");
    let mut c = server.get(&uri).await;
    edit(&mut c);
    server.put(&uri, c).await
}

#[tokio::test(flavor = "multi_thread")]
async fn surfaces_components_and_bindings_are_validated_and_stored() {
    let server = Server::start();

    // Components: a slug from the name, and each control listed once.
    let cams = create(&server, "/components", "component", deck_page("Cameras")).await;
    assert_eq!(cams["slug"], "cameras");
    let mut twice = deck_page("Bad");
    twice["controls"][1]["id"] = json!("k0");
    let (status, body) = server.post("/components", twice).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        codes(&body),
        [(json!(["controls", 1, "id"]), "duplicateControl".into())]
    );
    let (status, body) = server
        .post(
            "/components",
            json!({ "name": "Cams", "slug": "cameras", "controls": keys(15) }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(codes(&body), [(json!(["slug"]), "duplicateSlug".into())]);
    let (status, body) = server.post("/components", json!({ "name": "x" })).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(codes(&body)[0].1, "invalid", "{body}");
    let (status, body) = server
        .post(
            "/components",
            json!({ "name": "Key", "controls": [{ "id": "key" }],
            "show": { "when": { "nav": "pop over" } } }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        codes(&body),
        [(json!(["show", "when", "nav"]), "invalid".into())]
    );
    // One that isn't shown is only used inside others.
    let key = create(
        &server,
        "/components",
        "component",
        json!({ "name": "Key", "controls": [{ "id": "key", "outputs": ["image"] }] }),
    )
    .await;

    // Surfaces: a start that exists and is shown.
    let deck = create(
        &server,
        "/surfaces",
        "surface",
        json!({ "name": "FOH deck", "model": "virtual.deck15", "startComponent": id(&cams) }),
    )
    .await;
    assert_eq!(deck["slug"], "fohDeck");
    for start in ["nope", id(&key)] {
        let (status, body) = server
            .post(
                "/surfaces",
                json!({ "name": "Other", "model": "virtual.deck15", "startComponent": start }),
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            codes(&body),
            [(json!(["startComponent"]), "unknownComponent".into())]
        );
    }

    // Bindings: in a component, on one of its controls, one per control.
    let k14 = server
        .bind(
            id(&cams),
            json!({ "control": "k14", "props": { "label": { "expr": "'Hi'" } } }),
        )
        .await;
    for (control, code) in [("k14", "duplicateControl"), ("k15", "unknownControl")] {
        let (status, body) = edit(&server, id(&cams), |c| {
            c["bindings"]
                .as_array_mut()
                .unwrap()
                .push(json!({ "id": "x", "control": control, "props": {} }))
        })
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{control}");
        assert_eq!(
            codes(&body),
            [(json!(["bindings", 1, "control"]), code.into())]
        );
    }
    let (status, body) = edit(&server, id(&cams), |c| {
        c["bindings"].as_array_mut().unwrap().push(k14.clone())
    })
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        codes(&body)[0],
        (json!(["bindings", 1, "id"]), "invalid".into())
    );
    let (status, body) = server
        .try_bind(
            id(&cams),
            json!({ "control": "k0", "props": { "background": { "expr": "1 +", "fallback": "#000" } } }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(codes(&body)[0].1, "exprSyntax", "{body}");

    let audio = create(&server, "/components", "component", deck_page("Audio")).await;
    server
        .bind(id(&audio), json!({ "control": "k0", "props": {} }))
        .await;
    let stored = server.get(&format!("/components/{}", id(&cams))).await;
    assert_eq!(stored["bindings"], json!([k14]));
    assert!(stored["bindings"][0].get("layer").is_none(), "{stored}");
    let shown = server.get("/components?shown=true").await;
    assert_eq!(shown.as_array().unwrap().len(), 2, "{shown}");

    // A component can't drop a bound control, or the output its binding drives.
    let (status, body) = edit(&server, id(&cams), |c| c["controls"] = json!(keys(4))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        codes(&body),
        [(json!(["bindings", 0, "control"]), "unknownControl".into())]
    );
    let (status, body) = edit(&server, id(&cams), |c| {
        c["controls"][14]["outputs"] = json!([])
    })
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        codes(&body),
        [(json!(["bindings", 0, "props", "label"]), "noOutput".into())]
    );

    // Flows save event triggers, checked against components.
    let control_flow = |component: &str| {
        json!({
            "name": "On press",
            "enabled": true, "concurrency": "ignore",
            "triggers": [{ "kind": "event", "from": { "component": component, "controls": "k3" }, "event": "press" }],
            "body": [{ "id": "s", "kind": "set", "path": "vars.pressed", "value": "true" }],
        })
    };
    let (status, body) = server.post("/flows", control_flow(id(&cams))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, body) = server.post("/flows", control_flow("nope")).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(codes(&body)[0].1, "unknownComponent", "{body}");

    // Navigating: state, unknown components, other groups, and nothing.
    let show = format!("/surfaces/{}/current", id(&deck));
    let (status, body) = server.post(&show, json!({ "component": id(&audio) })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        server.value("surface.fohDeck.current.main").await,
        Some(json!("audio"))
    );
    assert_eq!(body["group"], "main");
    for nope in ["nope", id(&key)] {
        let (status, body) = server.post(&show, json!({ "component": nope })).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["code"], "unknownComponent");
    }
    let (status, body) = server
        .post(
            &show,
            json!({ "group": "popover", "component": id(&audio) }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["group"], "popover");
    assert_eq!(
        server.value("surface.fohDeck.current.popover").await,
        Some(json!("audio"))
    );
    let (status, body) = server
        .post(&show, json!({ "group": "pop over", "component": null }))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "invalid");
    // It has never attached, so it has no controls to copy yet.
    let (status, _) = server
        .call(
            Method::GET,
            &format!("/surfaces/{}/controls", id(&deck)),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = server.post(&show, json!({ "component": null })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        server.value("surface.fohDeck.current.main").await,
        Some(json!(null))
    );
    let (status, _) = server
        .post("/surfaces/nope/current", json!({ "component": null }))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Deleting a component clears start components.
    assert_eq!(
        server.delete(&format!("/components/{}", id(&cams))).await,
        StatusCode::NO_CONTENT
    );
    let deck = server.get(&format!("/surfaces/{}", id(&deck))).await;
    assert!(deck.get("startComponent").is_none(), "{deck}");
    assert_eq!(
        server.delete(&format!("/components/{}", id(&cams))).await,
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
async fn surfaces_components_and_bindings_survive_a_restart() {
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

    let app = start();
    let mut cameras = deck_page("Cameras");
    cameras["bindings"] =
        json!([{ "id": "b", "control": "k0", "props": { "label": { "expr": "'Hi'" } } }]);
    let (component, _) = app.save::<Component>(&cameras, None).await.unwrap();
    let (surface, _) = app
        .save::<Surface>(
            &json!({ "name": "FOH deck", "model": "virtual.deck15", "startComponent": component.id }),
            None,
        )
        .await
        .unwrap();
    // What surfaces show is kept: one moved off its start component, and one with
    // no start component, shown something while detached.
    let (lights, _) = app
        .save::<Component>(&deck_page("Lights"), None)
        .await
        .unwrap();
    let (side, _) = app
        .save::<Surface>(
            &json!({ "name": "Side deck", "model": "virtual.deck15" }),
            None,
        )
        .await
        .unwrap();
    let system = covalence_core::journal::Origin::system;
    app.surfaces()
        .show(&surface.id, Some(&lights.id), system())
        .unwrap();
    app.surfaces()
        .show(&side.id, Some(&component.id), system())
        .unwrap();
    // A key held when the server stops.
    let link = app
        .surfaces()
        .attach(&surface.id, core_attach("fohDeck"))
        .unwrap();
    link.input
        .send(covalence_core::surface::Input {
            control: "k0".into(),
            kind: covalence_core::journal::InputKind::Down,
            value: None,
            ts: 1.0,
            echo: false,
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
    // Kept until the end: dropping it detaches, which the stopped app would
    // still journal, racing the next one's writes to the same database.
    let _link = link;

    let app = start();
    assert_eq!(
        app.core().shared().state.value(&pressed),
        None,
        "input state isn't restored"
    );
    assert_eq!(
        app.components(),
        BTreeMap::from([
            (component.id.clone(), component.clone()),
            (lights.id.clone(), lights.clone())
        ])
    );
    assert_eq!(
        app.surface_entities(),
        BTreeMap::from([
            (surface.id.clone(), surface.clone()),
            (side.id.clone(), side.clone())
        ])
    );
    // Each shows what it showed last, not its start component, and says so in
    // state before it attaches.
    assert_eq!(app.surfaces().shown(&surface.id), Some(lights.id.clone()));
    assert_eq!(app.surfaces().shown(&side.id), Some(component.id.clone()));
    let current = |slug: &str| {
        app.core()
            .shared()
            .state
            .value(&covalence_core::state::StatePath::new(
                "surface",
                [slug, "current", "main"]
                    .into_iter()
                    .map(|k| covalence_protocol::state::PathSegment::Key(k.into()))
                    .collect(),
            ))
    };
    assert_eq!(current("sideDeck"), Some("cameras".into()));
    let side_link = app
        .surfaces()
        .attach(&side.id, core_attach("sideDeck"))
        .unwrap();
    assert_eq!(app.surfaces().shown(&side.id), Some(component.id.clone()));
    assert_eq!(current("sideDeck"), Some("cameras".into()));
    drop(side_link);
    // Shown components reach the core, which resolves them.
    assert_eq!(app.surfaces().components()[&component.id], component);
    assert_eq!(app.components()[&component.id].bindings.len(), 1);
    app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn bindings_preview_with_their_look_and_image() {
    let server = Server::start();
    let vh = videohub(&server, "vh").await;

    let preview =
        |binding: Value| server.post("/components/preview", json!({ "binding": binding }));
    let (status, body) = preview(json!({
        "control": "k0",
        "props": {
            "label": { "use": "template", "parts": ["Out 1: ", { "ref": { "conn": vh, "path": ["outputs", 1, "source"] } }] },
            "background": {
                "use": "rules",
                "rules": [{ "when": { "use": "compare", "input": { "ref": { "conn": vh, "path": ["outputs", 1, "source"] } }, "op": "==", "value": 1 }, "value": "#e02020" }],
                "else": "#333333",
            },
            "textColor": { "expr": "nope()", "fallback": "#ffff00" },
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

    // A `map` picks from a list of colours: source 1 of 0–12 is the first.
    let (status, body) = preview(json!({
        "control": "k1",
        "props": {
            "label": { "ref": { "conn": vh, "path": ["outputs", 1, "source"] } },
            "background": {
                "use": "map", "input": { "ref": { "conn": vh, "path": ["outputs", 1, "source"] } },
                "from": [0, 12], "to": ["#00ff00", "#ffff00", "#ff0000"],
            },
        },
    }))
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["look"]["label"], "1");
    assert_eq!(body["look"]["background"], "#00ff00");
    assert!(body["props"]["background"]["expr"].is_string(), "{body}");

    let (status, body) = server
        .post(
            "/components/preview",
            json!({ "binding": { "control": "k0", "props": {} }, "size": 72 }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["look"]["label"], "");
    let png = BASE64.decode(body["image"].as_str().unwrap()).unwrap();
    assert_eq!(png_size(&png), (72, 72));

    // A binding that doesn't parse is a 422, as on save.
    let (status, body) =
        preview(json!({ "control": "k0", "props": { "label": { "use": "nope" } } })).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(codes(&body)[0].0, json!(["props", "label"]));

    server.stop().await;
}

/// A PNG's width and height, from its header.
fn png_size(png: &[u8]) -> (u32, u32) {
    let be = |at: usize| u32::from_be_bytes(png[at..at + 4].try_into().unwrap());
    (be(16), be(20))
}

/// The virtual deck's attach, as the core takes it.
fn core_attach(slug: &str) -> covalence_core::surface::Attach {
    let mut params = attach(slug)["params"].clone();
    params.as_object_mut().unwrap().remove("surface");
    serde_json::from_value(params).unwrap()
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
        .post("/components/preview", json!({ "binding": binding }))
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

    let component = create(&server, "/components", "component", deck_page("Cameras")).await;
    let deck = create(
        &server,
        "/surfaces",
        "surface",
        json!({ "name": "FOH deck", "model": "virtual.deck15", "startComponent": id(&component) }),
    )
    .await;
    let k0 = server.bind(id(&component), label_of("k0", &vh)).await;
    server.bind(id(&component), label_of("k2", &vh2)).await;
    let (status, body) = server
        .post(
            "/flows",
            json!({
                "name": "Route on press",
                "enabled": true, "concurrency": "ignore",
                "triggers": [{ "kind": "event", "from": { "component": id(&component), "controls": "k1" }, "event": "press" }],
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
        ["trigger.matched", "event", "state.patch"],
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
    edited["props"]["background"] = json!({ "expr": "'#0000ff'" });
    let (status, body) = server.try_bind(id(&component), edited).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (control, _) = next_draw(&mut ws, "fohDeck").await;
    assert_eq!(control, "k0");

    // Switching to a page with nothing bound blanks the bound keys.
    let empty = create(&server, "/components", "component", deck_page("Empty")).await;
    let (status, body) = server
        .post(
            &format!("/surfaces/{}/current", id(&deck)),
            json!({ "component": id(&empty) }),
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

    // What the deck attached with, for "New page".
    let controls = server
        .get(&format!("/surfaces/{}/controls", id(&deck)))
        .await;
    assert_eq!(controls.as_array().unwrap().len(), 15);
    assert_eq!(controls[7]["at"], json!({ "x": 2, "y": 1 }));
    assert_eq!(controls[7]["inputs"], json!([{ "kind": "momentary" }]));

    // A page with keys the deck lacks doesn't fit.
    let wide = create(
        &server,
        "/components",
        "component",
        Server::page("Wide", json!(keys(16))),
    )
    .await;
    let (status, body) = server
        .post(
            &format!("/surfaces/{}/current", id(&deck)),
            json!({ "component": id(&wide) }),
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
    let component = create(&server, "/components", "component", deck_page("Cameras")).await;
    let deck = create(
        &server,
        "/surfaces",
        "surface",
        json!({ "name": "FOH deck", "model": "virtual.deck15", "startComponent": id(&component) }),
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

/// Starts a flow through its `api` trigger and waits for the run to end.
async fn run_flow(server: &Server, flow: &Value) -> Value {
    let (status, body) = server
        .post(&format!("/flows/{}/run", id(flow)), json!({}))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let run = body["run"].as_str().unwrap().to_owned();
    server
        .eventually(WAIT, &format!("run {run} to finish"), || async {
            let info = server.get(&format!("/runs/{run}")).await;
            info.get("finished").is_some().then_some(info)
        })
        .await
}

/// Flows navigate the deck and press its keys with `core`
/// actions, on a fixed target and on one held in `vars`, in simulate mode;
/// and cancel and disable another flow.
#[tokio::test(flavor = "multi_thread")]
async fn flows_drive_the_deck_and_other_flows_through_core() {
    let server = Server::start();
    let addr = server.listen().await;
    // Rehearsal: device actions would be suppressed; core's are internal.
    server.app.core().set_simulate(true);

    let cams = create(&server, "/components", "component", deck_page("Cameras")).await;
    let audio = create(&server, "/components", "component", deck_page("Audio")).await;
    let deck = create(
        &server,
        "/surfaces",
        "surface",
        json!({ "name": "FOH deck", "model": "virtual.deck15", "startComponent": id(&cams) }),
    )
    .await;
    let mut ws = connect(&addr, id(&deck)).await;
    send(&mut ws, attach("fohDeck")).await;
    server
        .eventually(WAIT, "the deck to attach", || async {
            (server.value("surface.fohDeck.current.main").await == Some(json!("cameras")))
                .then_some(())
        })
        .await;

    // A press on the audio component's k3 notes itself.
    create(
        &server,
        "/flows",
        "flow",
        json!({
            "name": "Note audio press", "enabled": true, "concurrency": "parallel",
            "triggers": [{ "kind": "event", "from": { "component": id(&audio), "controls": "k3" }, "event": "press" }],
            "body": [{ "id": "note", "kind": "set", "path": "vars.audioPressed", "value": "trigger.control" }],
        }),
    )
    .await;
    let go = create(
        &server,
        "/flows",
        "flow",
        json!({
            "name": "Go to audio", "enabled": true, "concurrency": "ignore",
            "triggers": [{ "kind": "api" }],
            "body": [
                { "id": "pick", "kind": "set", "path": "vars.deck", "value": format!("'{}'", id(&deck)) },
                {
                    "id": "nav", "kind": "action", "target": { "conn": "core" }, "action": "navigate",
                    "params": { "surface": { "ref": { "root": "vars", "path": ["deck"] } }, "component": id(&audio) },
                },
                {
                    "id": "down", "kind": "action", "target": { "conn": "core" },
                    "action": "inject", "params": { "surface": id(&deck), "control": "k3", "input": "down" },
                },
                {
                    "id": "up", "kind": "action", "target": { "conn": "core" },
                    "action": "inject", "params": { "surface": id(&deck), "control": "k3", "input": "up" },
                },
            ],
        }),
    )
    .await;
    let info = run_flow(&server, &go).await;
    assert_eq!(info["status"], "ok", "{info}");
    assert_eq!(
        server.value("surface.fohDeck.current.main").await,
        Some(json!("audio"))
    );
    server
        .eventually(WAIT, "the injected press to run its flow", || async {
            (server.value("vars.audioPressed").await == Some(json!("k3"))).then_some(())
        })
        .await;

    // Each core action is journaled requested → dispatched → result, even
    // in simulate mode.
    let journal = server.get("/journal?from=1&limit=2000").await;
    let entries = journal.as_array().unwrap();
    let kinds_of = |call: &str| -> Vec<&str> {
        entries
            .iter()
            .filter(|e| e["data"]["call"] == call)
            .map(|e| e["kind"].as_str().unwrap())
            .collect()
    };
    let requested: Vec<&Value> = entries
        .iter()
        .filter(|e| e["kind"] == "action.requested" && e["data"]["conn"] == "core")
        .collect();
    assert_eq!(requested.len(), 3);
    for r in &requested {
        assert_eq!(r["data"]["params"]["surface"], id(&deck));
        assert_eq!(
            kinds_of(r["data"]["call"].as_str().unwrap()),
            ["action.requested", "action.dispatched", "action.result"]
        );
    }
    // The injected press is the run's: its patch is caused by the request.
    let press = entries
        .iter()
        .rfind(|e| {
            e["kind"] == "event" && e["data"]["control"] == "k3" && e["data"]["event"] == "press"
        })
        .unwrap();
    assert_eq!(press["origin"]["step"], "down", "{press}");
    let chain = server
        .get(&format!("/journal/{}/why", press["seq"].as_u64().unwrap()))
        .await;
    let kinds: Vec<&str> = chain
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds[..3],
        ["event", "state.patch", "action.requested"],
        "{kinds:?}"
    );

    // A flow cancels another's runs and disables it.
    let victim = create(
        &server,
        "/flows",
        "flow",
        json!({
            "name": "Long", "enabled": true, "concurrency": "parallel",
            "triggers": [{ "kind": "api" }],
            "body": [{ "id": "pause", "kind": "wait", "for": "1h" }],
        }),
    )
    .await;
    let (_, started) = server
        .post(&format!("/flows/{}/run", id(&victim)), json!({}))
        .await;
    let victim_run = started["run"].as_str().unwrap().to_owned();
    let stopper = create(
        &server,
        "/flows",
        "flow",
        json!({
            "name": "Stop it", "enabled": true, "concurrency": "ignore",
            "triggers": [{ "kind": "api" }],
            "body": [
                { "id": "cancel", "kind": "action", "target": { "conn": "core" }, "action": "cancelRuns", "params": { "flow": id(&victim) } },
                { "id": "off", "kind": "action", "target": { "conn": "core" }, "action": "setEnabled", "params": { "flow": id(&victim), "enabled": false } },
            ],
        }),
    )
    .await;
    let info = run_flow(&server, &stopper).await;
    assert_eq!(info["status"], "ok", "{info}");
    let victim_info = server.get(&format!("/runs/{victim_run}")).await;
    assert_eq!(victim_info["status"], "cancelled", "{victim_info}");
    let stored = server.get(&format!("/flows/{}", id(&victim))).await;
    assert_eq!(stored["enabled"], false, "{stored}");

    // A surface that doesn't exist is caught on save.
    let (status, body) = server
        .post(
            "/flows",
            json!({
                "name": "Bad", "enabled": true, "concurrency": "ignore",
                "triggers": [{ "kind": "api" }],
                "body": [{ "id": "nav", "kind": "action", "target": { "conn": "core" }, "action": "navigate", "params": { "surface": "nope", "component": null } }],
            }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        codes(&body),
        [(
            json!(["body", 0, "params", "surface"]),
            "unknownTarget".to_owned()
        )]
    );
    server.stop().await;
}

/// surfaces spec §11.4: a surface-scoped helper gets a cell on a surface the first time
/// it shows the helper's component, moves with the surface's slug, and goes when
/// the surface is deleted.
#[tokio::test(flavor = "multi_thread")]
async fn surface_scoped_helpers_live_on_the_surfaces_that_showed_their_component() {
    let server = Server::start();
    let cams = create(&server, "/components", "component", deck_page("Cams")).await;
    let deck = create(
        &server,
        "/surfaces",
        "surface",
        json!({ "name": "Deck", "model": "virtual.deck15" }),
    )
    .await;
    server
        .bind(
            id(&cams),
            json!({
                "control": "k3",
                "helpers": { "latched": { "use": "toggle", "scope": "surface" } },
            }),
        )
        .await;
    let latched = |slug: &str| format!("surface.{slug}.controls.k3.helpers.latched");
    assert_eq!(server.value(&latched("deck")).await, None);

    let (status, body) = server
        .post(
            &format!("/surfaces/{}/current", id(&deck)),
            json!({ "component": id(&cams) }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    server
        .eventually(WAIT, "the helper's cell", || async {
            server
                .value(&latched("deck"))
                .await
                .filter(|v| *v == json!(false))
        })
        .await;

    let (status, body) = server
        .put(
            &format!("/surfaces/{}", id(&deck)),
            json!({ "name": "Desk", "slug": "desk", "model": "virtual.deck15" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    server
        .eventually(WAIT, "the helper under the new slug", || async {
            server
                .value(&latched("desk"))
                .await
                .filter(|v| *v == json!(false))
        })
        .await;
    assert_eq!(server.value(&latched("deck")).await, None);

    assert_eq!(
        server.delete(&format!("/surfaces/{}", id(&deck))).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(server.value(&latched("desk")).await, None);
    server.stop().await;
}

/// A flow a component owns (composition spec §2) handles its keys with the
/// component implied, keeps its status under the component, follows its
/// slug, and is deleted with it.
#[tokio::test(flavor = "multi_thread")]
async fn a_component_owns_its_flows() {
    let server = Server::start();
    let addr = server.listen().await;
    let cams = create(&server, "/components", "component", deck_page("Cameras")).await;
    let deck = create(
        &server,
        "/surfaces",
        "surface",
        json!({ "name": "FOH deck", "model": "virtual.deck15", "startComponent": id(&cams) }),
    )
    .await;
    let mut ws = connect(&addr, id(&deck)).await;
    send(&mut ws, attach("fohDeck")).await;
    server
        .eventually(WAIT, "the deck to attach", || async {
            (server.value("surface.fohDeck.current.main").await == Some(json!("cameras")))
                .then_some(())
        })
        .await;

    // `{controls}` without an owner names nothing.
    let note = json!({
        "name": "Note press", "enabled": true, "concurrency": "parallel",
        "triggers": [{ "kind": "event", "from": { "controls": "k3" }, "event": "press" }],
        "body": [{ "id": "note", "kind": "set", "path": "vars.pressed", "value": "trigger.control" }],
    });
    let (status, body) = server.post("/flows", note.clone()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(codes(&body)[0].1, "notInScope");

    let mut owned = note;
    owned["owner"] = json!(id(&cams));
    let flow = create(&server, "/flows", "flow", owned).await;
    server
        .eventually(WAIT, "its status under the component", || async {
            server
                .value("component.cameras.flows.notePress.running")
                .await
        })
        .await;
    assert_eq!(server.value("flow.notePress.running").await, None);

    let press = create(
        &server,
        "/flows",
        "flow",
        json!({
            "name": "Press k3", "enabled": true, "concurrency": "ignore",
            "triggers": [{ "kind": "api" }],
            "body": [
                {
                    "id": "down", "kind": "action", "target": { "conn": "core" },
                    "action": "inject", "params": { "surface": id(&deck), "control": "k3", "input": "down" },
                },
                {
                    "id": "up", "kind": "action", "target": { "conn": "core" },
                    "action": "inject", "params": { "surface": id(&deck), "control": "k3", "input": "up" },
                },
            ],
        }),
    )
    .await;
    run_flow(&server, &press).await;
    server
        .eventually(WAIT, "the press to run the owned flow", || async {
            (server.value("vars.pressed").await == Some(json!("k3"))).then_some(())
        })
        .await;

    // Renaming the component moves the flow's home.
    let mut renamed = cams.clone();
    renamed["slug"] = json!("cams");
    let (status, body) = server
        .put(&format!("/components/{}", id(&cams)), renamed)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    server
        .eventually(WAIT, "its new home", || async {
            server.value("component.cams.flows.notePress.running").await
        })
        .await;
    assert_eq!(
        server
            .value("component.cameras.flows.notePress.running")
            .await,
        None
    );

    // Deleting the component deletes the flow.
    assert_eq!(
        server.delete(&format!("/components/{}", id(&cams))).await,
        StatusCode::NO_CONTENT
    );
    let (status, _) = server
        .call(Method::GET, &format!("/flows/{}", id(&flow)), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        server.value("component.cams.flows.notePress.running").await,
        None
    );

    // A component and its flow can be made together, the flow first.
    let component_id = "01JC0000000000000000000ABC";
    let (status, body) = server
        .post(
            "/batch",
            json!({ "ops": [
                { "op": "create", "type": "flow", "body": {
                    "name": "Owned", "enabled": true, "concurrency": "ignore", "owner": component_id,
                    "triggers": [{ "kind": "event", "from": { "controls": "k0" }, "event": "press" }],
                    "body": [],
                } },
                { "op": "create", "type": "component", "body": {
                    "id": component_id, "name": "Audio", "controls": keys(15), "show": {},
                } },
            ] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    server
        .eventually(WAIT, "the batch's flow under its component", || async {
            server.value("component.audio.flows.owned.running").await
        })
        .await;
}

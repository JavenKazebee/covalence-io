//! The dataflow milestone's "Done when" list (docs/milestones/m8-dataflow.md),
//! end to end: the real server and plugin host, a virtual deck with an
//! encoder and a motor fader over its WebSocket, and a simulated show
//! (plugins/showsim): a switcher's `program`, a mixer's `volume`, and a GPIO
//! contact. Needs `deno` on the PATH.
//!
//! Items covered by narrower tests are named there: the latching key
//! (core `binding_worker`), reducer conformance (core `transforms`), a cycle
//! between computed variables (core `dataflow`) and a persisted variable
//! across a restart (`variables_api`).

mod common;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use common::Server;
use covalence_core::surface::{VirtualLayout, virtual_controls};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

const WAIT: Duration = Duration::from_secs(10);
const SHOW: &str = "io.covalence.test.showsim@0.1.0=plugins/showsim/mod.ts";
/// The redraw budget (overview spec): a state change to a redrawn key.
const BUDGET: Duration = Duration::from_millis(16);

fn id(entity: &Value) -> &str {
    entity["id"].as_str().unwrap()
}

async fn create(server: &Server, uri: &str, kind: &str, body: Value) -> Value {
    let (status, saved) = server.post(uri, body).await;
    assert_eq!(status, StatusCode::CREATED, "POST {uri}: {saved}");
    saved[kind].clone()
}

/// A simulated show connection, once its state is there.
async fn show(server: &Server, alias: &str) -> String {
    let conn = create_conn(server, alias).await;
    let path = format!("{alias}.volume");
    server.eventually(WAIT, &path, || server.value(&path)).await;
    conn
}

async fn create_conn(server: &Server, alias: &str) -> String {
    let (status, conn) = server
        .post(
            "/connections",
            json!({ "plugin": "io.covalence.test.showsim", "alias": alias, "config": {} }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{conn}");
    conn["id"].as_str().unwrap().to_owned()
}

/// Runs `<alias>.<action>` as the API's user would: from elsewhere.
async fn act(server: &Server, alias: &str, action: &str, params: Value) {
    let (status, body) = server
        .post(
            "/actions",
            json!({ "action": format!("{alias}.{action}"), "params": params }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{action}: {body}");
    assert_eq!(body["ok"], true, "{action}: {body}");
}

/// The deck's controls: 15 keys, an encoder `e0` and a motor fader `f0`.
fn layout() -> VirtualLayout {
    serde_json::from_value(json!({ "encoders": 1, "faders": 1 })).unwrap()
}

/// A page of all the deck's controls, as "New page" copies them.
fn deck_page(name: &str) -> Value {
    let controls: Vec<Value> = virtual_controls(Some(&layout()))
        .iter()
        .map(|c| serde_json::to_value(c.to_control_spec()).unwrap())
        .collect();
    Server::page(name, json!(controls))
}

async fn deck(server: &Server, start: &str) -> Value {
    create(
        server,
        "/surfaces",
        "surface",
        json!({
            "name": "FOH deck", "model": "virtual.deck15", "startComponent": start,
            "virtual": { "encoders": 1, "faders": 1 },
        }),
    )
    .await
}

/// A web surface's client: what it was last sent per control.
struct Deck {
    ws: Socket,
    images: BTreeMap<String, String>,
    levels: BTreeMap<String, f64>,
}

impl Deck {
    async fn attach(server: &Server, addr: &str, surface: &str) -> Deck {
        let url = format!("ws://{addr}/surface/{surface}");
        // Small writes go at once, as a surface sends them (no Nagle).
        let ws = tokio_tungstenite::connect_async_with_config(&url, None, true)
            .await
            .unwrap()
            .0;
        let mut deck = Deck {
            ws,
            images: BTreeMap::new(),
            levels: BTreeMap::new(),
        };
        let controls = serde_json::to_value(virtual_controls(Some(&layout()))).unwrap();
        deck.send(json!({
            "method": "surface.attach",
            "params": { "surface": "fohDeck", "model": "virtual.deck15", "name": "Virtual deck", "controls": controls },
        }))
        .await;
        server
            .eventually(WAIT, "the deck attached", || async {
                (server.value("surface.fohDeck.connected").await == Some(json!(true))).then_some(())
            })
            .await;
        deck
    }

    async fn send(&mut self, message: Value) {
        self.ws
            .send(Message::text(message.to_string()))
            .await
            .unwrap();
    }

    async fn input(&mut self, control: &str, kind: &str, value: Option<Value>, echo: bool) {
        let mut params = json!({
            "surface": "fohDeck", "control": control, "kind": kind,
            "ts": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as f64,
        });
        if let Some(v) = value {
            params["value"] = v;
        }
        if echo {
            params["echo"] = json!(true);
        }
        self.send(json!({ "method": "surface.input", "params": params }))
            .await;
    }

    /// Reads outputs until `done` holds for what the deck has been sent.
    async fn until(&mut self, what: &str, done: impl Fn(&Deck) -> bool) {
        let read = async {
            while !done(self) {
                match self.ws.next().await {
                    Some(Ok(Message::Text(text))) => {
                        let m: Value = serde_json::from_str(&text).unwrap();
                        let p = &m["params"];
                        let control = p["control"].as_str().unwrap_or_default().to_owned();
                        match m["method"].as_str() {
                            Some("surface.draw") => {
                                self.images
                                    .insert(control, p["image"].as_str().unwrap().to_owned());
                            }
                            Some("surface.level") => {
                                self.levels.insert(control, p["value"].as_f64().unwrap());
                            }
                            _ => {}
                        }
                    }
                    Some(Ok(_)) => {}
                    other => panic!("the socket ended waiting for {what}: {other:?}"),
                }
            }
        };
        tokio::time::timeout(WAIT, read)
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
    }
}

/// A binding's image as the editor's preview draws it.
async fn image_of(server: &Server, binding: Value) -> String {
    let (status, body) = server
        .post("/components/preview", json!({ "binding": binding }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["image"].as_str().unwrap().to_owned()
}

fn labelled(control: &str, text: &str) -> Value {
    json!({ "control": control, "props": { "label": text } })
}

/// Entries of `kind` in the journal, oldest first.
async fn journal(server: &Server, kind: &str) -> Vec<Value> {
    server
        .get("/journal?from=1&limit=10000")
        .await
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == kind)
        .cloned()
        .collect()
}

/// `action.requested` entries the dataflow sent to `conn`.action.
async fn sent_by_dataflow(server: &Server, conn: &str, action: &str) -> Vec<Value> {
    journal(server, "action.requested")
        .await
        .into_iter()
        .filter(|e| {
            e["origin"] == json!({ "core": "dataflow" })
                && e["data"]["conn"] == conn
                && e["data"]["action"] == action
        })
        .collect()
}

/// "`vars.isLive` declared as `{expr: "atem.program == 3"}` (against a
/// simulated switcher) is a read-only cell. A binding and a `state` trigger
/// read it, and `why` on the trigger's action passes through the variable's
/// patch to the switcher's."
#[tokio::test(flavor = "multi_thread")]
async fn a_computed_variable_is_a_read_only_cell_that_why_passes_through() {
    let server = Server::start_with(&[SHOW]);
    let atem = show(&server, "atem").await;
    let mixer = show(&server, "mixer").await;
    create(
        &server,
        "/variables",
        "variable",
        json!({ "name": "Is live", "slug": "isLive", "value": { "expr": "atem.program == 3" } }),
    )
    .await;
    assert_eq!(server.value("vars.isLive").await, Some(json!(false)));

    // Read-only: a flow can't set it.
    let (status, body) = server
        .post(
            "/flows",
            json!({
                "name": "Set it", "enabled": true, "concurrency": "ignore", "triggers": [{ "kind": "api" }],
                "body": [{ "id": "s", "kind": "set", "path": "vars.isLive", "value": "true" }],
            }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["errors"][0]["code"], "readOnly", "{body}");

    // A binding reads it.
    let tally = json!({
        "control": "k0",
        "props": { "background": { "use": "rules",
            "rules": [{ "when": { "ref": { "root": "vars", "path": ["isLive"] } }, "value": "#e02020" }],
            "else": "#000000" } },
    });
    let dark = image_of(&server, tally.clone()).await;

    // A state trigger reads it.
    create(
        &server,
        "/flows",
        "flow",
        json!({
            "name": "Duck when live", "enabled": true, "concurrency": "ignore",
            "triggers": [{ "kind": "state", "when": "vars.isLive", "edge": "rising" }],
            "body": [{ "id": "duck", "kind": "action", "target": { "conn": mixer },
                       "action": "setVolume", "params": { "level": 0.2 } }],
        }),
    )
    .await;

    act(&server, "atem", "cut", json!({ "input": 3 })).await;
    server
        .eventually(WAIT, "the duck", || async {
            (server.value("mixer.volume").await == Some(json!(0.2))).then_some(())
        })
        .await;
    assert_eq!(server.value("vars.isLive").await, Some(json!(true)));
    assert_ne!(image_of(&server, tally).await, dark, "the tally went red");

    // why: the action, its run, the trigger, the variable's patch, the switcher's.
    let duck = journal(&server, "action.requested")
        .await
        .into_iter()
        .rfind(|e| e["data"]["action"] == "setVolume")
        .unwrap();
    let chain = server.get(&format!("/journal/{}/why", duck["seq"])).await;
    let steps: Vec<(String, Value)> = chain
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["kind"].as_str().unwrap().to_owned(), e["origin"].clone()))
        .collect();
    let at = |kind: &str, origin: Value| steps.iter().position(|s| s.0 == kind && s.1 == origin);
    let variable = at("state.patch", json!({ "core": "dataflow" })).expect("the variable's patch");
    let switcher = at("state.patch", json!({ "conn": atem })).expect("the switcher's patch");
    assert!(variable < switcher, "{steps:?}");
    assert_eq!(
        chain[variable]["data"]["ops"][0]["path"],
        json!({ "conn": "vars", "path": ["isLive"] })
    );
    server.stop().await;
}

/// "A GPIO-style contact from a test plugin latches the same way, through a
/// variable whose `value` is a `toggle` on the plugin's events. No surface
/// is involved."
#[tokio::test(flavor = "multi_thread")]
async fn a_contact_latches_through_a_variable() {
    let server = Server::start_with(&[SHOW]);
    let gpio = show(&server, "gpio").await;
    create(
        &server,
        "/variables",
        "variable",
        json!({ "name": "Armed", "value": {
            "use": "toggle", "on": { "from": { "target": gpio }, "event": "contact" } } }),
    )
    .await;
    assert_eq!(server.value("vars.armed").await, Some(json!(false)));
    for latched in [true, false, true] {
        act(&server, "gpio", "close", json!({})).await;
        server
            .eventually(WAIT, "the latch", || async {
                (server.value("vars.armed").await == Some(json!(latched))).then_some(())
            })
            .await;
    }
    server.stop().await;
}

/// "A virtual encoder with a `position` helper that `follow`s `vh`'s
/// simulated volume and has `to: {action: setVolume}`. Turning it sends
/// rate-limited actions with the latest value. When the volume changes from
/// elsewhere, the helper follows it and sends nothing back."
///
/// "The virtual fader in motor mode: `value` bound to `vh`'s simulated volume
/// moves it, and sends nothing back. Dragging it sends rate-limited actions,
/// and `requested.level` and `value` are both readable. A fader input the
/// host marks `echo: true` updates `value` and gives no `change`."
#[tokio::test(flavor = "multi_thread")]
async fn an_encoder_and_a_motor_fader_keep_in_step_with_a_volume() {
    let server = Server::start_with(&[SHOW]);
    let addr = server.listen().await;
    let vh = show(&server, "vh").await;
    let component = create(&server, "/components", "component", deck_page("Mix")).await;
    let surface = deck(&server, id(&component)).await;
    let volume = json!({ "ref": { "conn": vh, "path": ["volume"] } });
    let to = json!([{ "action": { "target": { "conn": vh }, "action": "setVolume", "param": "level", "rateLimit": "50ms" } }]);
    server.bind(id(&component), json!({ "control": "e0",
            "helpers": { "vol": { "use": "position", "max": 1, "step": 0.0625, "follow": volume, "to": to } },
        }),
    )
    .await;
    server
        .bind(
            id(&component),
            json!({ "control": "f0",
                "props": { "value": volume },
                "helpers": { "level": { "use": "absolute", "follow": volume, "to": to } },
            }),
        )
        .await;
    let mut deck = Deck::attach(&server, &addr, id(&surface)).await;

    // The motor fader goes to the volume, and sending it sends nothing back.
    deck.until("the fader's level", |d| d.levels.get("f0") == Some(&0.5))
        .await;
    // Written once it's been sent, so it may land just after the client has it.
    server
        .eventually(WAIT, "requested.level", || async {
            (server
                .value("surface.fohDeck.controls.f0.requested.level")
                .await
                == Some(json!(0.5)))
            .then_some(())
        })
        .await;
    let vol = "component.mix.controls.e0.vol";
    assert_eq!(server.value(vol).await, Some(json!(0.5)));

    // Turning the encoder sends the latest value, rate-limited: five
    // detents of 1/16 from 0.5.
    for _ in 0..5 {
        deck.input("e0", "turn", Some(json!(1)), false).await;
    }
    server
        .eventually(WAIT, "the turned volume", || async {
            (server.value("vh.volume").await == Some(json!(0.8125))).then_some(())
        })
        .await;
    let turned = sent_by_dataflow(&server, &vh, "setVolume").await;
    assert!(
        (1..5).contains(&turned.len()),
        "{} sends for five turns",
        turned.len()
    );
    assert_eq!(
        turned.last().unwrap()["data"]["params"]["level"],
        json!(0.8125)
    );
    // The fader follows the volume the encoder set, by its motor.
    deck.until("the fader at 0.8125", |d| {
        d.levels.get("f0") == Some(&0.8125)
    })
    .await;

    // The volume moving from elsewhere: both follow, after their hold-off,
    // and nothing is sent back.
    tokio::time::sleep(Duration::from_millis(300)).await;
    act(&server, "vh", "setVolume", json!({ "level": 0.25 })).await;
    server
        .eventually(WAIT, "the encoder following", || async {
            (server.value(vol).await == Some(json!(0.25))).then_some(())
        })
        .await;
    deck.until("the fader at 0.25", |d| d.levels.get("f0") == Some(&0.25))
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        sent_by_dataflow(&server, &vh, "setVolume").await.len(),
        turned.len(),
        "a change from elsewhere isn't sent back"
    );

    // The motor's own movement, marked as an echo, is state only.
    let changes = || async {
        journal(&server, "event")
            .await
            .into_iter()
            .filter(|e| e["data"]["control"] == "f0" && e["data"]["event"] == "change")
            .count()
    };
    let before = changes().await;
    deck.input("f0", "value", Some(json!(0.3)), true).await;
    server
        .eventually(WAIT, "the echoed value", || async {
            (server.value("surface.fohDeck.controls.f0.value").await == Some(json!(0.3)))
                .then_some(())
        })
        .await;
    assert_eq!(changes().await, before, "an echo gives no change");

    // Dragging it sends, rate-limited, with the latest value; both sides
    // of the fader are readable.
    tokio::time::sleep(Duration::from_millis(300)).await;
    deck.input("f0", "touch", Some(json!(1)), false).await;
    for v in [0.4, 0.5, 0.6, 0.7, 0.8] {
        deck.input("f0", "value", Some(json!(v)), false).await;
    }
    server
        .eventually(WAIT, "the dragged volume", || async {
            (server.value("vh.volume").await == Some(json!(0.8))).then_some(())
        })
        .await;
    let dragged = sent_by_dataflow(&server, &vh, "setVolume").await.len() - turned.len();
    assert!((1..5).contains(&dragged), "{dragged} sends for five moves");
    assert_eq!(
        server.value("surface.fohDeck.controls.f0.value").await,
        Some(json!(0.8))
    );
    assert!(
        server
            .value("surface.fohDeck.controls.f0.requested.level")
            .await
            .is_some()
    );
    deck.input("f0", "touch", Some(json!(0)), false).await;
    server.stop().await;
}

/// "A virtual encoder with a `select` helper over `core` surfaces' components
/// and `to: {action: navigate}` pages a deck."
#[tokio::test(flavor = "multi_thread")]
async fn an_encoder_pages_a_deck_through_navigate() {
    let server = Server::start();
    let addr = server.listen().await;
    let cams = create(&server, "/components", "component", deck_page("Cameras")).await;
    let audio = create(&server, "/components", "component", deck_page("Audio")).await;
    let lights = create(&server, "/components", "component", deck_page("Lights")).await;
    let surface = deck(&server, id(&cams)).await;
    let pages = json!([id(&cams), id(&audio), id(&lights)]);
    // The same encoder binding on each page, so it keeps paging.
    for component in [&cams, &audio, &lights] {
        server.bind(id(component), json!({ "control": "e0",
                "helpers": { "page": { "use": "select", "items": pages, "scope": "surface",
                    "to": [{ "action": { "target": "core", "action": "navigate", "param": "component",
                                         "params": { "surface": id(&surface) } } }] } },
            }),
        )
        .await;
    }
    let mut deck = Deck::attach(&server, &addr, id(&surface)).await;
    let current = "surface.fohDeck.current.main";
    for (turn, page) in [(1, "audio"), (1, "lights"), (-2, "cameras")] {
        deck.input("e0", "turn", Some(json!(turn)), false).await;
        server
            .eventually(WAIT, page, || async {
                (server.value(current).await == Some(json!(page))).then_some(())
            })
            .await;
    }
    server.stop().await;
}

/// "A shift layer (`level: over`, `when: {while: …k15.pressed}`) takes over
/// three keys while `k15` is held and redraws within 16 ms. A latching shift
/// reads a `toggle`." The virtual deck's last key is `k14`.
#[tokio::test(flavor = "multi_thread")]
async fn a_shift_layer_takes_three_keys_while_held_and_a_latching_one_on_a_toggle() {
    let server = Server::start();
    let addr = server.listen().await;
    let cams = create(&server, "/components", "component", deck_page("Cameras")).await;
    let three: Vec<Value> = deck_page("")["controls"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| ["k2", "k3", "k4"].contains(&c["id"].as_str().unwrap()))
        .cloned()
        .collect();
    let shift = create(
        &server,
        "/components",
        "component",
        json!({ "name": "Shift", "controls": three, "show": { "level": "over",
                "when": { "while": { "expr": "self.controls.k14.pressed" } } } }),
    )
    .await;
    let surface = deck(&server, id(&cams)).await;
    for (component, text) in [(&cams, "Cam"), (&shift, "Wide")] {
        for k in ["k2", "k3", "k4"] {
            server
                .bind(id(component), labelled(k, &format!("{text} {k}")))
                .await;
        }
    }
    let cam3 = image_of(&server, labelled("k3", "Cam k3")).await;
    let wide3 = image_of(&server, labelled("k3", "Wide k3")).await;
    let mut deck = Deck::attach(&server, &addr, id(&surface)).await;
    deck.until("Cam k3", |d| d.images.get("k3") == Some(&cam3))
        .await;

    // Held: within the budget, from the press to the redrawn key.
    let mut took = Vec::new();
    for _ in 0..5 {
        let pressed = Instant::now();
        deck.input("k14", "down", None, false).await;
        deck.until("Wide k3", |d| d.images.get("k3") == Some(&wide3))
            .await;
        took.push(pressed.elapsed());
        assert_eq!(
            server.value("surface.fohDeck.controls.k3.component").await,
            Some(json!("shift"))
        );
        deck.input("k14", "up", None, false).await;
        deck.until("Cam k3 again", |d| d.images.get("k3") == Some(&cam3))
            .await;
    }
    took.sort();
    let median = took[took.len() / 2];
    println!("shift: press to redrawn key {took:?}, median {median:?}");
    if !cfg!(debug_assertions) {
        assert!(median < BUDGET, "median {median:?} over {BUDGET:?}");
    }

    // Latching: the shift reads a toggle on k14 instead.
    let uri = format!("/components/{}", id(&shift));
    let mut latching = server.get(&uri).await;
    latching["show"] = json!({ "level": "over",
        "when": { "while": { "ref": { "root": "component", "path": ["cameras", "controls", "k14", "latched"] } } } });
    let (status, body) = server.put(&uri, latching).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    server
        .bind(
            id(&cams),
            json!({ "control": "k14", "helpers": { "latched": { "use": "toggle" } } }),
        )
        .await;
    for wide in [true, false] {
        deck.input("k14", "down", None, false).await;
        deck.input("k14", "up", None, false).await;
        let want = if wide { &wide3 } else { &cam3 };
        deck.until("the latched shift", |d| d.images.get("k3") == Some(want))
            .await;
    }
    server.stop().await;
}

/// "Budgets hold with cells between a change and a key: state change →
/// redrawn key in under 16 ms with a chain of three cells."
#[tokio::test(flavor = "multi_thread")]
async fn a_chain_of_three_cells_redraws_a_key_within_the_budget() {
    let server = Server::start_with(&[SHOW]);
    let addr = server.listen().await;
    show(&server, "atem").await;
    for (name, expr) in [
        ("a", "atem.program"),
        ("b", "vars.a + 1"),
        ("c", "vars.b + 1"),
    ] {
        create(
            &server,
            "/variables",
            "variable",
            json!({ "name": name, "value": { "expr": expr } }),
        )
        .await;
    }
    let component = create(&server, "/components", "component", deck_page("Cameras")).await;
    let label = || {
        json!({ "control": "k0",
                "props": { "label": { "use": "format", "format": "auto", "input": { "ref": { "root": "vars", "path": ["c"] } } } } })
    };
    server.bind(id(&component), label()).await;
    let surface = deck(&server, id(&component)).await;
    let mut deck = Deck::attach(&server, &addr, id(&surface)).await;

    let mut took = Vec::new();
    for input in 4..9 {
        // What the key will show: `c` is the program plus 2.
        let want = image_of(&server, labelled("k0", &(input + 2).to_string())).await;
        let cut = Instant::now();
        act(&server, "atem", "cut", json!({ "input": input })).await;
        deck.until("the redrawn key", |d| d.images.get("k0") == Some(&want))
            .await;
        took.push(cut.elapsed());
    }
    took.sort();
    let median = took[took.len() / 2];
    println!("three cells: action to redrawn key {took:?}, median {median:?}");
    if !cfg!(debug_assertions) {
        assert!(median < BUDGET, "median {median:?} over {BUDGET:?}");
    }
    server.stop().await;
}

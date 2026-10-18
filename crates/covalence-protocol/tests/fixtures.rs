//! Golden vectors shared with the TypeScript side (ADR 0016).
//!
//! `protocol/fixtures/encoded/rust.json` holds the frames this crate produces;
//! regenerate it with `COVALENCE_UPDATE_FIXTURES=1 cargo test`. The TypeScript
//! tests decode it, and this test decodes the frames TypeScript produced.

use std::collections::BTreeMap;
use std::path::PathBuf;

use covalence_protocol::framing::{Format, FrameDecoder, encode};
use covalence_protocol::methods::{Method, core_to_host, host_to_core};
use covalence_protocol::{Message, Value, from_value, to_value};
use pretty_assertions::assert_eq;
use serde::Deserialize;

#[derive(Deserialize)]
struct Fixture {
    name: String,
    dir: Option<String>,
    method: Option<String>,
    params: Option<String>,
    message: serde_json::Value,
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../protocol/fixtures")
}

fn load(file: &str) -> Vec<Fixture> {
    let text = std::fs::read_to_string(fixtures_dir().join(file)).unwrap();
    serde_json::from_str(&text).unwrap()
}

/// JSON with every number as f64, so `5` and `5.0` compare equal.
fn normalize(value: serde_json::Value) -> serde_json::Value {
    use serde_json::Value as J;
    match value {
        J::Number(n) => J::from(n.as_f64().unwrap()),
        J::Array(items) => J::Array(items.into_iter().map(normalize).collect()),
        J::Object(map) => J::Object(map.into_iter().map(|(k, v)| (k, normalize(v))).collect()),
        other => other,
    }
}

fn json(value: &impl serde::Serialize) -> serde_json::Value {
    normalize(serde_json::to_value(value).unwrap())
}

/// Decodes params with the method table for the fixture's direction, then
/// re-encodes them. Fields the Rust types lack would be dropped here.
fn roundtrip_params(dir: &str, message: &Message) -> Result<Value, String> {
    let err = |e: covalence_protocol::methods::MethodError| e.to_string();
    let enc = |e: covalence_protocol::ValueError| e.to_string();
    match (dir, message) {
        ("hostToCore", Message::Request(r)) => host_to_core::Request::decode(&r.m, &r.p)
            .map_err(err)?
            .params()
            .map_err(enc),
        ("coreToHost", Message::Request(r)) => core_to_host::Request::decode(&r.m, &r.p)
            .map_err(err)?
            .params()
            .map_err(enc),
        ("hostToCore", Message::Notification(n)) => host_to_core::Notification::decode(&n.m, &n.p)
            .map_err(err)?
            .params()
            .map_err(enc),
        other => panic!("no method table for {:?}", other.0),
    }
}

fn roundtrip_as<M: Method>(r: &Value) -> Result<Value, String> {
    let typed: M::Result = from_value(r).map_err(|e| e.to_string())?;
    to_value(&typed).map_err(|e| e.to_string())
}

fn roundtrip_result(dir: &str, method: &str, r: &Value) -> Result<Value, String> {
    match (dir, method) {
        ("hostToCore", "host.hello") => roundtrip_as::<host_to_core::HostHello>(r),
        ("hostToCore", "ping") => roundtrip_as::<host_to_core::Ping>(r),
        ("coreToHost", "plugin.load") => roundtrip_as::<core_to_host::PluginLoad>(r),
        ("coreToHost", "conn.create") => roundtrip_as::<core_to_host::ConnCreate>(r),
        ("coreToHost", "conn.destroy") => roundtrip_as::<core_to_host::ConnDestroy>(r),
        ("coreToHost", "conn.resync") => roundtrip_as::<core_to_host::ConnResync>(r),
        ("coreToHost", "action.invoke") => roundtrip_as::<core_to_host::ActionInvoke>(r),
        ("coreToHost", "ping") => roundtrip_as::<core_to_host::Ping>(r),
        other => panic!("no result type for {other:?}"),
    }
}

fn check(fixture: &Fixture) -> Result<Message, String> {
    let message: Message =
        serde_json::from_value(fixture.message.clone()).map_err(|e| e.to_string())?;
    if let Some(dir) = &fixture.dir {
        match &message {
            Message::Request(r) if fixture.params.is_some() => {
                assert_eq!(
                    json(&roundtrip_params(dir, &message)?),
                    json(&r.p),
                    "{}: params",
                    fixture.name
                );
            }
            Message::Notification(n) if fixture.params.is_some() => {
                assert_eq!(
                    json(&roundtrip_params(dir, &message)?),
                    json(&n.p),
                    "{}: params",
                    fixture.name
                );
            }
            Message::Response(res) => {
                if let (Some(method), Ok(r)) = (&fixture.method, &res.outcome) {
                    assert_eq!(
                        json(&roundtrip_result(dir, method, r)?),
                        json(r),
                        "{}: result",
                        fixture.name
                    );
                }
            }
            _ => {}
        }
    }
    Ok(message)
}

#[test]
fn valid_messages_round_trip() {
    for fixture in load("messages.json") {
        let message = check(&fixture).unwrap_or_else(|e| panic!("{}: {e}", fixture.name));
        assert_eq!(
            json(&message),
            normalize(fixture.message.clone()),
            "{}: envelope",
            fixture.name
        );

        for format in [Format::MsgPack, Format::Json] {
            let mut decoder = FrameDecoder::new(format);
            decoder.push(&encode(&message, format).unwrap());
            assert_eq!(
                decoder.next_message().unwrap(),
                Some(message.clone()),
                "{}: {format:?}",
                fixture.name
            );
        }
    }
}

#[test]
fn invalid_messages_are_rejected() {
    for fixture in load("invalid.json") {
        assert!(check(&fixture).is_err(), "{}: accepted", fixture.name);
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn rust_frames_are_current() {
    let frames: BTreeMap<String, String> = load("messages.json")
        .into_iter()
        .map(|f| {
            let message: Message = serde_json::from_value(f.message).unwrap();
            (f.name, hex(&encode(&message, Format::MsgPack).unwrap()))
        })
        .collect();
    let expected = serde_json::to_string_pretty(&frames).unwrap() + "\n";
    let path = fixtures_dir().join("encoded/rust.json");

    if std::env::var_os("COVALENCE_UPDATE_FIXTURES").is_some() {
        std::fs::write(&path, &expected).unwrap();
    }
    let actual = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        actual == expected,
        "{} is stale; rerun with COVALENCE_UPDATE_FIXTURES=1",
        path.display()
    );
}

#[test]
fn typescript_frames_decode() {
    let path = fixtures_dir().join("encoded/ts.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "{} is missing; run `deno test` with COVALENCE_UPDATE_FIXTURES=1",
            path.display()
        )
    });
    let frames: BTreeMap<String, String> = serde_json::from_str(&text).unwrap();

    let fixtures = load("messages.json");
    assert_eq!(frames.len(), fixtures.len(), "{} is stale", path.display());
    for fixture in fixtures {
        let frame = frames
            .get(&fixture.name)
            .unwrap_or_else(|| panic!("{}: no TypeScript frame", fixture.name));
        let mut decoder = FrameDecoder::new(Format::MsgPack);
        decoder.push(&unhex(frame));
        let message = decoder.next_message().unwrap().unwrap();
        assert_eq!(
            json(&message),
            normalize(fixture.message.clone()),
            "{}",
            fixture.name
        );

        // The TypeScript encoding must also decode into the typed params and results.
        let typed = Fixture {
            message: serde_json::to_value(&message).unwrap(),
            ..fixture
        };
        check(&typed).unwrap_or_else(|e| panic!("{}: {e}", typed.name));
    }
}

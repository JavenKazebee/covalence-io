//! A fake plugin host for testing the supervisor without Deno.
//!
//! Speaks the protocol on stdio and simulates a router plugin: each
//! connection has `outputs[n].source`, and the `route {output, input}` action
//! sets one, emitting `state.patch` and `event.emit routeChanged`. The `fail`
//! action fails. `conn.resync` logs "resync" and resends the snapshot.
//!
//! Environment:
//! - `FAKE_HOST_IGNORE_PINGS=1`: never answer `ping`, to test dead-host detection.

use std::collections::BTreeMap;
use std::io::{Read, Write};

use covalence_protocol::action::{ActionInvoke, ActionResult};
use covalence_protocol::connection::{ConnStatus, ConnStatusKind};
use covalence_protocol::framing::{Format, FrameDecoder, encode};
use covalence_protocol::methods::{Method, NotificationMethod, core_to_host, host_to_core};
use covalence_protocol::plugin::{
    ActionDef, ConfigDef, ConnectionPolicy, Manifest, Permissions, PluginLoaded, Runtime,
};
use covalence_protocol::session::{Capability, HostHello, HostInfo, HostKind, ProtocolRange};
use covalence_protocol::state::{
    EventEmit, Log, LogLevel, PatchOp, PathSegment, PathValue, StatePatch, StateSnapshot,
};
use covalence_protocol::{
    Empty, ErrorCode, Message, Notification, ProtocolError, Request, Response, True, Value,
    from_value, to_value,
};

const OUTPUTS: i64 = 4;

struct Host {
    format: Format,
    out: std::io::Stdout,
    /// conn → output → source
    conns: BTreeMap<String, BTreeMap<i64, i64>>,
    highest_term: u32,
    ignore_pings: bool,
}

fn main() {
    let format = Format::from_env();
    let mut host = Host {
        format,
        out: std::io::stdout(),
        conns: BTreeMap::new(),
        highest_term: 0,
        ignore_pings: std::env::var("FAKE_HOST_IGNORE_PINGS").as_deref() == Ok("1"),
    };
    let hello = HostHello {
        protocol: ProtocolRange {
            min: "0.1.0".into(),
            max: "0.1.x".into(),
        },
        host: HostInfo {
            kind: HostKind::External,
            version: "0.0.0".into(),
            runtime: Some("fake".into()),
        },
        node: None,
        capabilities: vec![Capability::Simulate],
    };
    host.send(Message::Request(Request {
        id: 1,
        m: host_to_core::HostHello::NAME.into(),
        conn: None,
        p: to_value(&hello).unwrap(),
    }));

    let mut decoder = FrameDecoder::new(format);
    let mut stdin = std::io::stdin().lock();
    let mut buf = vec![0; 64 * 1024];
    loop {
        let n = match stdin.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        decoder.push(&buf[..n]);
        while let Some(message) = decoder.next_message().expect("valid frame") {
            if let Message::Request(req) = message {
                host.handle(req);
            }
        }
    }
}

impl Host {
    fn send(&mut self, message: Message) {
        let frame = encode(&message, self.format).unwrap();
        // The core is gone if this fails; exit quietly.
        if self
            .out
            .write_all(&frame)
            .and_then(|_| self.out.flush())
            .is_err()
        {
            std::process::exit(0);
        }
    }

    fn notify<N: NotificationMethod>(&mut self, conn: Option<&str>, params: &N::Params) {
        self.send(Message::Notification(Notification {
            m: N::NAME.into(),
            conn: conn.map(str::to_owned),
            p: to_value(params).unwrap(),
        }));
    }

    fn handle(&mut self, req: Request) {
        let conn = req.conn.clone();
        let outcome = match core_to_host::Request::decode(&req.m, &req.p) {
            Err(e) => Err(ProtocolError::new(ErrorCode::UnknownMethod, e.to_string())),
            Ok(core_to_host::Request::Ping(_)) if self.ignore_pings => return,
            Ok(request) => self.dispatch(conn.as_deref(), request),
        };
        self.send(Message::Response(Response {
            id: req.id,
            outcome,
        }));
    }

    fn dispatch(
        &mut self,
        conn: Option<&str>,
        request: core_to_host::Request,
    ) -> Result<Value, ProtocolError> {
        use core_to_host::Request as R;
        let need_conn = || {
            conn.map(str::to_owned)
                .ok_or_else(|| ProtocolError::new(ErrorCode::InvalidRequest, "missing conn"))
        };
        match request {
            R::Ping(_) => ok(&Empty {}),
            R::PluginLoad(load) => ok(&PluginLoaded {
                manifest: manifest(&load.plugin, &load.version),
            }),
            R::ConnCreate(_) => {
                let conn = need_conn()?;
                self.conns
                    .insert(conn.clone(), (0..OUTPUTS).map(|o| (o, 0)).collect());
                self.status(&conn, ConnStatusKind::Connecting);
                self.snapshot(&conn);
                self.status(&conn, ConnStatusKind::Ok);
                ok(&Empty {})
            }
            R::ConnResync(_) => {
                let conn = need_conn()?;
                self.known(&conn)?;
                self.notify::<host_to_core::Log>(
                    Some(&conn),
                    &Log {
                        level: LogLevel::Info,
                        msg: "resync".into(),
                        fields: None,
                    },
                );
                self.snapshot(&conn);
                ok(&Empty {})
            }
            R::ConnDestroy(_) => {
                self.conns.remove(&need_conn()?);
                ok(&Empty {})
            }
            R::ActionInvoke(invoke) => {
                let conn = need_conn()?;
                self.invoke(&conn, invoke)
            }
        }
    }

    fn known(&self, conn: &str) -> Result<(), ProtocolError> {
        if self.conns.contains_key(conn) {
            Ok(())
        } else {
            Err(ProtocolError::new(
                ErrorCode::NotFound,
                "unknown connection",
            ))
        }
    }

    fn invoke(&mut self, conn: &str, invoke: ActionInvoke) -> Result<Value, ProtocolError> {
        self.known(conn)?;
        if invoke.term < self.highest_term {
            return Err(ProtocolError::new(ErrorCode::StaleTerm, "stale term"));
        }
        self.highest_term = invoke.term;
        match invoke.action.as_str() {
            "route" => {
                #[derive(serde::Deserialize)]
                struct Route {
                    output: i64,
                    input: i64,
                }
                let Route { output, input } = from_value(&invoke.params)
                    .map_err(|e| ProtocolError::new(ErrorCode::InvalidParams, e.to_string()))?;
                if !(0..OUTPUTS).contains(&output) {
                    return Err(ProtocolError::new(
                        ErrorCode::InvalidParams,
                        "no such output",
                    ));
                }
                self.conns.get_mut(conn).unwrap().insert(output, input);
                let patch = StatePatch {
                    ops: vec![PatchOp::Set(PathValue {
                        p: source_path(output),
                        v: Value::from(input),
                    })],
                    call: Some(invoke.call.clone()),
                    ts: None,
                };
                self.notify::<host_to_core::StatePatch>(Some(conn), &patch);
                let payload = Value::Map(vec![
                    ("output".into(), output.into()),
                    ("input".into(), input.into()),
                ]);
                self.notify::<host_to_core::EventEmit>(
                    Some(conn),
                    &EventEmit {
                        event: "routeChanged".into(),
                        payload: Some(payload),
                        call: Some(invoke.call.clone()),
                        ts: None,
                    },
                );
                ok(&ActionResult { result: None })
            }
            "fail" => Err(ProtocolError::new(ErrorCode::ActionFailed, "asked to fail")),
            other => Err(ProtocolError::new(
                ErrorCode::InvalidParams,
                format!("unknown action {other}"),
            )),
        }
    }

    fn status(&mut self, conn: &str, status: ConnStatusKind) {
        self.notify::<host_to_core::ConnStatus>(
            Some(conn),
            &ConnStatus {
                status,
                message: None,
                detail: None,
            },
        );
    }

    fn snapshot(&mut self, conn: &str) {
        let values = self.conns[conn]
            .iter()
            .map(|(&output, &source)| PathValue {
                p: source_path(output),
                v: Value::from(source),
            })
            .collect();
        self.notify::<host_to_core::StateSnapshot>(
            Some(conn),
            &StateSnapshot {
                values,
                complete: True,
            },
        );
    }
}

fn source_path(output: i64) -> Vec<PathSegment> {
    vec![
        PathSegment::Key("outputs".into()),
        PathSegment::Index(output),
        PathSegment::Key("source".into()),
    ]
}

fn ok<T: serde::Serialize>(value: &T) -> Result<Value, ProtocolError> {
    Ok(to_value(value).unwrap())
}

fn manifest(id: &str, version: &str) -> Manifest {
    Manifest {
        id: id.into(),
        name: "Fake router".into(),
        version: version.into(),
        protocol: "^0.1".into(),
        sdk: "0.0.0".into(),
        runtime: Runtime::Deno,
        license: "MIT".into(),
        authors: vec![],
        homepage: None,
        categories: vec!["video-routing".into()],
        icon: None,
        config: ConfigDef {
            schema: Default::default(),
            ui: None,
            version: 1,
        },
        secrets: None,
        permissions: Permissions::default(),
        connections: ConnectionPolicy::Multiple,
        actions: vec![ActionDef {
            id: "route".into(),
            name: "Set route".into(),
            description: None,
            params: Default::default(),
            result: None,
            idempotent: Some(true),
            cancellable: None,
            duration_hint: None,
            learn: None,
        }],
        events: vec![],
        state: Default::default(),
        presets: None,
        surfaces: None,
        simulator: None,
    }
}

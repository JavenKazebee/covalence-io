//! `GET /ws`: live state, journal entries and host status for UI clients.
//! The messages are in the state spec, §1.6.
//!
//! Each session subscribes to state path prefixes. Changes under them are
//! coalesced for [`COALESCE`] and sent as one `patch`, latest value winning;
//! a session that falls behind the core's change feed gets a fresh
//! `snapshot` instead of a gap. Journal entries are never coalesced.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use covalence_core::journal::Entry;
use covalence_core::state::{ConnId, Dirty, StatePath};
use covalence_core::text_path;
use covalence_protocol::state::Path;
use serde::Deserialize;
use serde_json::{Value as JsonValue, json};
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::Instant;

use crate::App;
use crate::api::{StateLeaf, host_json};

/// How long changes are gathered before a `patch` goes out: about one frame.
const COALESCE: Duration = Duration::from_millis(16);

pub async fn upgrade(State(app): State<Arc<App>>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| session(app, socket))
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
enum Request {
    Subscribe { paths: Vec<String> },
    Unsubscribe { paths: Vec<String> },
    Journal { on: bool },
}

/// A subscribed prefix. The alias need not exist yet: state appears once a
/// connection takes it.
#[derive(Clone)]
struct Sub {
    text: String,
    alias: String,
    path: Path,
}

struct Session {
    app: Arc<App>,
    subs: Vec<Sub>,
    journal: Option<broadcast::Receiver<Arc<Entry>>>,
    /// Changed leaves under a subscription, not yet sent.
    pending: Dirty,
    flush_at: Option<Instant>,
}

async fn session(app: Arc<App>, mut socket: WebSocket) {
    // Subscribe to changes before anything is read, so none fall in between.
    let mut dirty = app.core.shared().state.subscribe();
    let mut host = app.core.host().status();
    let mut closing = app.closing.subscribe();
    let mut s = Session {
        app,
        subs: Vec::new(),
        journal: None,
        pending: Dirty::new(),
        flush_at: None,
    };

    let status = host.borrow_and_update().clone();
    if send(
        &mut socket,
        json!({ "type": "host", "host": host_json(&status) }),
    )
    .await
    .is_err()
    {
        return;
    }

    loop {
        let out = tokio::select! {
            msg = socket.recv() => match msg {
                Some(Ok(Message::Text(text))) => Some(s.handle(&text)),
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                // Pings are answered by axum; binary frames mean nothing here.
                Some(Ok(_)) => None,
            },
            changed = dirty.recv() => match changed {
                Ok(paths) => {
                    s.collect(&paths);
                    None
                }
                Err(RecvError::Lagged(_)) => {
                    s.pending.clear();
                    s.flush_at = None;
                    (!s.subs.is_empty()).then(|| s.snapshot(s.subs.clone()))
                }
                Err(RecvError::Closed) => break,
            },
            _ = tokio::time::sleep_until(s.flush_at.unwrap_or_else(Instant::now)), if s.flush_at.is_some() => {
                s.flush_at = None;
                Some(s.patch())
            }
            changed = host.changed() => {
                if changed.is_err() {
                    break;
                }
                let status = host.borrow_and_update().clone();
                Some(json!({ "type": "host", "host": host_json(&status) }))
            }
            entry = recv_journal(&mut s.journal) => match entry {
                Ok(entry) => Some(json!({ "type": "journal", "entry": &*entry })),
                Err(RecvError::Lagged(missed)) => {
                    Some(json!({ "type": "lagged", "feed": "journal", "missed": missed }))
                }
                Err(RecvError::Closed) => {
                    s.journal = None;
                    None
                }
            },
            changed = closing.changed() => {
                if changed.is_err() || *closing.borrow() {
                    let _ = socket.send(Message::Close(None)).await;
                    break;
                }
                None
            }
        };
        if let Some(out) = out
            && send(&mut socket, out).await.is_err()
        {
            break;
        }
    }
}

impl Session {
    fn handle(&mut self, text: &str) -> JsonValue {
        let request: Request = match serde_json::from_str(text) {
            Ok(request) => request,
            Err(e) => return error(format!("bad request: {e}")),
        };
        match request {
            Request::Subscribe { paths } => {
                let mut added = Vec::new();
                for text in paths {
                    let (alias, path) = match text_path::parse(&text) {
                        Ok(parsed) => parsed,
                        Err(e) => return error(format!("bad path {text:?}: {e}")),
                    };
                    if !self.subs.iter().any(|s| s.text == text) {
                        added.push(Sub { text, alias, path });
                    }
                }
                self.subs.extend(added.iter().cloned());
                self.snapshot(added)
            }
            Request::Unsubscribe { paths } => {
                self.subs.retain(|s| !paths.contains(&s.text));
                json!({ "type": "unsubscribed", "paths": paths })
            }
            Request::Journal { on } => {
                self.journal = on.then(|| self.app.core.shared().journal.subscribe());
                json!({ "type": "journal", "on": on })
            }
        }
    }

    /// Queues the changed leaves this session subscribes to.
    fn collect(&mut self, paths: &Dirty) {
        if self.subs.is_empty() {
            return;
        }
        let aliases = self.aliases();
        for path in paths {
            let Some(alias) = aliases.get(&path.conn) else {
                continue;
            };
            let wanted = self
                .subs
                .iter()
                .any(|s| &s.alias == alias && path.path.starts_with(&s.path));
            if wanted {
                self.pending.insert(path.clone());
            }
        }
        if !self.pending.is_empty() && self.flush_at.is_none() {
            self.flush_at = Some(Instant::now() + COALESCE);
        }
    }

    /// Current values of the pending leaves; ones no longer there are deleted.
    fn patch(&mut self) -> JsonValue {
        let pending = std::mem::take(&mut self.pending);
        let aliases = self.aliases();
        let (set, deleted) = self.app.core.shared().state.read(|tree| {
            let mut set = Vec::new();
            let mut deleted = Vec::new();
            for path in &pending {
                let Some(alias) = aliases.get(&path.conn) else {
                    continue;
                };
                match tree.leaf(path) {
                    Some(leaf) => set.push(StateLeaf::new(alias, &path.path, leaf)),
                    None => deleted.push(text_path::format(alias, &path.path)),
                }
            }
            (set, deleted)
        });
        json!({ "type": "patch", "set": set, "deleted": deleted })
    }

    /// Every leaf under `subs`. The client replaces what it holds under them.
    fn snapshot(&self, subs: Vec<Sub>) -> JsonValue {
        let aliases = self.aliases();
        let roots: Vec<(&str, StatePath)> = subs
            .iter()
            .flat_map(|sub| {
                aliases
                    .iter()
                    .filter(|(_, alias)| **alias == sub.alias)
                    .map(|(conn, alias)| {
                        (
                            alias.as_str(),
                            StatePath::new(conn.clone(), sub.path.clone()),
                        )
                    })
            })
            .collect();
        let leaves = self.app.core.shared().state.read(|tree| {
            let mut seen = BTreeSet::new();
            let mut leaves = Vec::new();
            for (alias, root) in &roots {
                for (path, leaf) in tree.leaves_under(root) {
                    if seen.insert(path) {
                        leaves.push(StateLeaf::new(alias, &path.path, leaf));
                    }
                }
            }
            leaves
        });
        let paths: Vec<&str> = subs.iter().map(|s| s.text.as_str()).collect();
        json!({ "type": "snapshot", "paths": paths, "leaves": leaves })
    }

    fn aliases(&self) -> BTreeMap<ConnId, String> {
        self.app
            .conns()
            .iter()
            .map(|(id, c)| (id.clone(), c.alias.clone()))
            .collect()
    }
}

async fn recv_journal(
    journal: &mut Option<broadcast::Receiver<Arc<Entry>>>,
) -> Result<Arc<Entry>, RecvError> {
    match journal {
        Some(journal) => journal.recv().await,
        None => std::future::pending().await,
    }
}

fn error(message: String) -> JsonValue {
    json!({ "type": "error", "message": message })
}

async fn send(socket: &mut WebSocket, message: JsonValue) -> Result<(), axum::Error> {
    socket.send(Message::Text(message.to_string().into())).await
}

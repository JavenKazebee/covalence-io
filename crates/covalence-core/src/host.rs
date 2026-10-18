//! Host supervisor (architecture: Host Supervisor; protocol spec §2–§6).
//!
//! Spawns a host child process and speaks the protocol with it over stdio:
//!
//! - answers `host.hello` with `host.welcome`, then loads every registered
//!   plugin and creates every registered connection;
//! - pings every `ping_interval`; `max_missed_pings` unanswered pings mark
//!   the host dead;
//! - when the host exits or dies, marks every connection's subtree stale and
//!   restarts it with exponential backoff. After a restart, connections are
//!   created again and sent `conn.resync`;
//! - routes `state.snapshot`, `state.patch`, `event.emit`, `conn.status` and
//!   `log` into the state tree and the journal;
//! - sends `action.invoke` and correlates the response.
//!
//! The registry of plugins and connections is the desired state; it outlives
//! any one host process.

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use covalence_protocol::action::{ActionInvoke, ActionResult};
use covalence_protocol::connection::{ConnCreate, ConnDestroy, ConnMode, ConnStatusKind};
use covalence_protocol::framing::{self, Format, FrameDecoder, FrameError};
use covalence_protocol::methods::{Method, MethodError, core_to_host, host_to_core};
use covalence_protocol::plugin::{Manifest, PluginLoad};
use covalence_protocol::session::HostWelcome;
use covalence_protocol::state::{LogLevel, PatchOp, PathValue};
use covalence_protocol::{
    Empty, ErrorCode, Message, Notification, PROTOCOL_VERSION, ProtocolError, Request, Response,
    Value, from_value, to_value,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior};

use crate::clock;
use crate::journal::{
    ConnStatusRecord, EventRecord, JournalOp, LogRecord, NewEntry, Origin, Record, Seq,
    StatePatchRecord, StateQualityRecord,
};
use crate::state::{ConnId, Meta, Quality, Source, StatePath};
use crate::{Shared, journal::Journal};

#[derive(Clone, Debug)]
pub struct HostConfig {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub format: Format,
    pub ping_interval: Duration,
    pub max_missed_pings: u32,
    /// How long the host has to send `host.hello` after it starts.
    pub hello_timeout: Duration,
    /// For `plugin.load` and the connection lifecycle requests.
    pub request_timeout: Duration,
    /// For `action.invoke` without a deadline.
    pub action_timeout: Duration,
    pub backoff: Backoff,
    /// Sent in `host.welcome`.
    pub core_id: String,
}

impl HostConfig {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        HostConfig {
            program: program.into(),
            args: Vec::new(),
            env: Vec::new(),
            format: Format::from_env(),
            ping_interval: Duration::from_secs(2),
            max_missed_pings: 3,
            hello_timeout: Duration::from_secs(5),
            request_timeout: Duration::from_secs(10),
            action_timeout: Duration::from_secs(10),
            backoff: Backoff::default(),
            core_id: ulid::Ulid::generate().to_string(),
        }
    }
}

/// Restart delays: `initial`, doubling up to `max`. A host that stayed up for
/// `reset_after` starts again from `initial`.
#[derive(Clone, Debug)]
pub struct Backoff {
    pub initial: Duration,
    pub max: Duration,
    pub reset_after: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Backoff {
            initial: Duration::from_millis(100),
            max: Duration::from_secs(10),
            reset_after: Duration::from_secs(30),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostStatus {
    pub phase: HostPhase,
    /// Counts host processes started, from 1.
    pub generation: u64,
    pub pid: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostPhase {
    /// Spawned; handshake or setup in progress.
    Starting,
    /// Plugins loaded and connections created.
    Ready,
    /// Waiting to restart.
    Down,
    Stopped,
}

/// A supervised host. Dropping it stops the host process.
pub struct Host {
    inner: Arc<Inner>,
    task: Option<JoinHandle<()>>,
}

struct Inner {
    config: HostConfig,
    shared: Shared,
    registry: tokio::sync::Mutex<Registry>,
    /// Connection modes, readable without waiting for the registry.
    modes: Mutex<HashMap<ConnId, ConnMode>>,
    status: watch::Sender<HostStatus>,
    shutdown: watch::Sender<bool>,
}

#[derive(Default)]
struct Registry {
    plugins: Vec<PluginLoad>,
    manifests: HashMap<String, Manifest>,
    conns: BTreeMap<ConnId, ConnCreate>,
    /// Set once the current host is set up.
    link: Option<Arc<Link>>,
}

impl Host {
    /// Starts supervising a host. Must be called inside a tokio runtime.
    pub fn start(config: HostConfig, shared: Shared) -> Self {
        let (status, _) = watch::channel(HostStatus {
            phase: HostPhase::Starting,
            generation: 0,
            pid: None,
        });
        let (shutdown, shutdown_rx) = watch::channel(false);
        let inner = Arc::new(Inner {
            config,
            shared,
            registry: tokio::sync::Mutex::new(Registry::default()),
            modes: Mutex::new(HashMap::new()),
            status,
            shutdown,
        });
        let task = tokio::spawn(supervise(inner.clone(), shutdown_rx));
        Host {
            inner,
            task: Some(task),
        }
    }

    pub fn status(&self) -> watch::Receiver<HostStatus> {
        self.inner.status.subscribe()
    }

    /// Registers a plugin and loads it now if the host is up. Returns the
    /// manifest if it was loaded.
    pub async fn load_plugin(&self, load: PluginLoad) -> Result<Option<Manifest>, ProtocolError> {
        let mut reg = self.inner.registry.lock().await;
        reg.plugins.retain(|p| p.plugin != load.plugin);
        reg.plugins.push(load.clone());
        let Some(link) = reg.link.clone() else {
            return Ok(None);
        };
        let loaded = link
            .call::<core_to_host::PluginLoad>(None, &load, self.inner.config.request_timeout)
            .await?;
        reg.manifests
            .insert(load.plugin.clone(), loaded.manifest.clone());
        Ok(Some(loaded.manifest))
    }

    pub async fn manifest(&self, plugin: &str) -> Option<Manifest> {
        self.inner
            .registry
            .lock()
            .await
            .manifests
            .get(plugin)
            .cloned()
    }

    /// Registers a connection and creates it now if the host is up. It stays
    /// registered even if creating it fails, and is created again after a
    /// host restart.
    pub async fn create_connection(
        &self,
        id: &str,
        create: ConnCreate,
    ) -> Result<(), ProtocolError> {
        let mut reg = self.inner.registry.lock().await;
        reg.conns.insert(id.to_owned(), create.clone());
        self.inner.modes().insert(id.to_owned(), create.mode);
        let Some(link) = reg.link.clone() else {
            return Ok(());
        };
        link.call::<core_to_host::ConnCreate>(Some(id), &create, self.inner.config.request_timeout)
            .await?;
        Ok(())
    }

    pub async fn destroy_connection(&self, id: &str, reason: &str) -> Result<(), ProtocolError> {
        let mut reg = self.inner.registry.lock().await;
        reg.conns.remove(id);
        self.inner.modes().remove(id);
        let Some(link) = reg.link.clone() else {
            return Ok(());
        };
        let params = ConnDestroy {
            reason: reason.to_owned(),
        };
        link.call::<core_to_host::ConnDestroy>(
            Some(id),
            &params,
            self.inner.config.request_timeout,
        )
        .await?;
        Ok(())
    }

    pub fn conn_mode(&self, id: &str) -> Option<ConnMode> {
        self.inner.modes().get(id).copied()
    }

    /// Sends `action.invoke`. Only the output gate should call this.
    pub async fn invoke(
        &self,
        conn: &str,
        invoke: &ActionInvoke,
    ) -> Result<ActionResult, ProtocolError> {
        let link = self.inner.registry.lock().await.link.clone();
        let Some(link) = link else {
            return Err(not_connected("the plugin host is not running"));
        };
        let timeout = match invoke.deadline {
            // A little longer than the deadline, so the host's own timeout wins.
            Some(ms) => Duration::from_millis(u64::from(ms)) + Duration::from_millis(500),
            None => self.inner.config.action_timeout,
        };
        link.call::<core_to_host::ActionInvoke>(Some(conn), invoke, timeout)
            .await
    }

    /// Stops the host process and waits for the supervisor to finish.
    pub async fn shutdown(mut self) {
        self.inner.shutdown.send_replace(true);
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.inner.shutdown.send_replace(true);
    }
}

impl Inner {
    fn modes(&self) -> MutexGuard<'_, HashMap<ConnId, ConnMode>> {
        self.modes.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn set_status(&self, phase: HostPhase, generation: u64, pid: Option<u32>) {
        self.status.send_replace(HostStatus {
            phase,
            generation,
            pid,
        });
    }

    fn journal(&self) -> &Journal {
        &self.shared.journal
    }

    fn append(&self, entry: NewEntry) -> Option<Seq> {
        match self.journal().append(entry) {
            Ok(entry) => Some(entry.seq),
            Err(e) => {
                tracing::error!("journal append failed: {e}");
                None
            }
        }
    }

    fn log(&self, level: LogLevel, message: String) {
        self.append(NewEntry::new(
            Origin::system(),
            Record::Log(LogRecord {
                level,
                message,
                fields: None,
            }),
        ));
    }
}

// ---------------------------------------------------------------------------
// Supervision loop

enum SessionEnd {
    Shutdown,
    Failed(String),
}

async fn supervise(inner: Arc<Inner>, mut shutdown: watch::Receiver<bool>) {
    let backoff = inner.config.backoff.clone();
    let mut delay = backoff.initial;
    let mut generation = 0;
    loop {
        generation += 1;
        let started = Instant::now();
        let end = run_session(&inner, generation, &mut shutdown).await;
        let reason = match end {
            SessionEnd::Shutdown => "core shutting down".to_owned(),
            SessionEnd::Failed(reason) => reason,
        };
        on_host_down(&inner, &reason).await;
        if *shutdown.borrow() {
            inner.set_status(HostPhase::Stopped, generation, None);
            return;
        }
        if started.elapsed() >= backoff.reset_after {
            delay = backoff.initial;
        }
        inner.set_status(HostPhase::Down, generation, None);
        inner.log(
            LogLevel::Warn,
            format!("plugin host stopped ({reason}); restarting in {delay:?}"),
        );
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = shutdown.wait_for(|stop| *stop) => {
                inner.set_status(HostPhase::Stopped, generation, None);
                return;
            }
        }
        delay = (delay * 2).min(backoff.max);
    }
}

type Setup = Pin<Box<dyn Future<Output = ()> + Send>>;

enum Event {
    Shutdown,
    Exited(String),
    Incoming(Option<Message>),
    HelloTimeout,
    SetupDone,
    Tick,
}

async fn run_session(
    inner: &Arc<Inner>,
    generation: u64,
    shutdown: &mut watch::Receiver<bool>,
) -> SessionEnd {
    if *shutdown.borrow() {
        return SessionEnd::Shutdown;
    }
    let config = &inner.config;
    let mut child = match Command::new(&config.program)
        .args(&config.args)
        .envs(config.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            return SessionEnd::Failed(format!("cannot start {}: {e}", config.program.display()));
        }
    };
    let pid = child.id();
    inner.set_status(HostPhase::Starting, generation, pid);
    inner.log(
        LogLevel::Info,
        format!("plugin host started (generation {generation}, pid {pid:?})"),
    );

    let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        return SessionEnd::Failed("host stdio was not captured".into());
    };
    let (out_tx, out_rx) = mpsc::unbounded_channel();
    let (in_tx, mut incoming) = mpsc::unbounded_channel();
    let link = Arc::new(Link::new(out_tx));
    let writer = tokio::spawn(write_frames(stdin, out_rx, config.format));
    let reader = tokio::spawn(read_frames(stdout, link.clone(), in_tx, config.format));

    let mut hello_done = false;
    let hello_deadline = tokio::time::sleep(config.hello_timeout);
    tokio::pin!(hello_deadline);
    let mut setup: Option<Setup> = None;
    let mut ticker =
        tokio::time::interval_at(Instant::now() + config.ping_interval, config.ping_interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut ping: Option<(u32, oneshot::Receiver<Outcome>)> = None;
    let mut missed = 0;

    let end = loop {
        let event = tokio::select! {
            _ = shutdown.wait_for(|stop| *stop) => Event::Shutdown,
            status = child.wait() => Event::Exited(match status {
                Ok(status) => format!("host exited: {status}"),
                Err(e) => format!("cannot wait for host: {e}"),
            }),
            message = incoming.recv() => Event::Incoming(message),
            _ = &mut hello_deadline, if !hello_done => Event::HelloTimeout,
            _ = async {
                match setup.as_mut() {
                    Some(fut) => fut.await,
                    None => std::future::pending().await,
                }
            } => Event::SetupDone,
            _ = ticker.tick(), if hello_done => Event::Tick,
        };
        match event {
            Event::Shutdown => break SessionEnd::Shutdown,
            Event::Exited(reason) => break SessionEnd::Failed(reason),
            Event::Incoming(None) => break SessionEnd::Failed("host closed its stdout".into()),
            Event::Incoming(Some(Message::Request(req))) => {
                if req.m == host_to_core::HostHello::NAME {
                    if hello_done {
                        link.respond(req.id, Err(invalid_request("duplicate host.hello")));
                        continue;
                    }
                    match welcome(inner, &req) {
                        Ok(welcome) => {
                            link.respond(req.id, to_value(&welcome).map_err(internal));
                            hello_done = true;
                            setup = Some(Box::pin(set_up(
                                inner.clone(),
                                link.clone(),
                                generation > 1,
                            )));
                        }
                        Err(e) => {
                            link.respond(req.id, Err(e.clone()));
                            break SessionEnd::Failed(format!("handshake failed: {e}"));
                        }
                    }
                } else {
                    handle_request(&link, req, hello_done);
                }
            }
            Event::Incoming(Some(Message::Notification(ntf))) => {
                if hello_done {
                    route(inner, ntf);
                }
            }
            // Responses are completed by the reader.
            Event::Incoming(Some(Message::Response(_))) => {}
            Event::HelloTimeout => {
                break SessionEnd::Failed(format!(
                    "no host.hello within {:?}",
                    config.hello_timeout
                ));
            }
            Event::SetupDone => {
                setup = None;
                inner.set_status(HostPhase::Ready, generation, pid);
            }
            Event::Tick => {
                if let Some((id, mut rx)) = ping.take() {
                    match rx.try_recv() {
                        Ok(Ok(_)) => missed = 0,
                        _ => {
                            link.forget(id);
                            missed += 1;
                        }
                    }
                }
                if missed >= config.max_missed_pings {
                    break SessionEnd::Failed(format!("{missed} pings unanswered"));
                }
                match link.send_request(core_to_host::Ping::NAME, None, Value::Map(Vec::new())) {
                    Ok(pending) => ping = Some(pending),
                    Err(_) => break SessionEnd::Failed("host link closed".into()),
                }
            }
        }
    };

    // Dropping the setup future releases the registry if it holds it.
    drop(setup);
    link.close();
    let _ = child.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
    reader.abort();
    writer.abort();
    end
}

/// Builds `host.welcome`, or the error for an unusable `host.hello`.
fn welcome(inner: &Inner, req: &Request) -> Result<HostWelcome, ProtocolError> {
    let hello = match host_to_core::Request::decode(&req.m, &req.p) {
        Ok(host_to_core::Request::HostHello(hello)) => hello,
        Ok(_) => unreachable!("method checked by the caller"),
        Err(e) => return Err(ProtocolError::new(ErrorCode::InvalidParams, e.to_string())),
    };
    if !version_in_range(PROTOCOL_VERSION, &hello.protocol.min, &hello.protocol.max) {
        return Err(ProtocolError::new(
            ErrorCode::UnsupportedVersion,
            format!(
                "core speaks {PROTOCOL_VERSION}; host offers {}..{}",
                hello.protocol.min, hello.protocol.max
            ),
        ));
    }
    Ok(HostWelcome {
        protocol: PROTOCOL_VERSION.to_owned(),
        session: ulid::Ulid::generate().to_string(),
        term: inner.journal().term(),
        core_id: inner.config.core_id.clone(),
    })
}

/// `min <= version <= max`, comparing numeric components; `x` matches any.
fn version_in_range(version: &str, min: &str, max: &str) -> bool {
    fn parts(v: &str) -> Vec<Option<u64>> {
        v.split('.').map(|p| p.parse().ok()).collect()
    }
    let v: Vec<u64> = parts(version).into_iter().map(|p| p.unwrap_or(0)).collect();
    let cmp = |bound: &str| -> std::cmp::Ordering {
        for (i, b) in parts(bound).into_iter().enumerate() {
            let Some(b) = b else {
                return std::cmp::Ordering::Equal; // wildcard
            };
            match v.get(i).copied().unwrap_or(0).cmp(&b) {
                std::cmp::Ordering::Equal => continue,
                other => return other,
            }
        }
        std::cmp::Ordering::Equal
    };
    cmp(min).is_ge() && cmp(max).is_le()
}

fn handle_request(link: &Link, req: Request, hello_done: bool) {
    let outcome = match host_to_core::Request::decode(&req.m, &req.p) {
        Err(MethodError::Unknown(m)) => Err(ProtocolError::new(
            ErrorCode::UnknownMethod,
            format!("unknown method `{m}`"),
        )),
        Err(e) => Err(ProtocolError::new(ErrorCode::InvalidParams, e.to_string())),
        Ok(_) if !hello_done => Err(invalid_request("send host.hello first")),
        Ok(host_to_core::Request::Ping(_)) => Ok(Value::Map(Vec::new())),
        Ok(host_to_core::Request::HostHello(_)) => Err(invalid_request("duplicate host.hello")),
    };
    link.respond(req.id, outcome);
}

/// Loads plugins and creates connections on a fresh host. After a restart,
/// also asks each connection to resync.
async fn set_up(inner: Arc<Inner>, link: Arc<Link>, restarted: bool) {
    let timeout = inner.config.request_timeout;
    let mut reg = inner.registry.lock().await;
    for load in reg.plugins.clone() {
        match link
            .call::<core_to_host::PluginLoad>(None, &load, timeout)
            .await
        {
            Ok(loaded) => {
                reg.manifests.insert(load.plugin.clone(), loaded.manifest);
            }
            Err(e) => inner.log(
                LogLevel::Error,
                format!("cannot load plugin {}: {e}", load.plugin),
            ),
        }
    }
    for (id, create) in reg.conns.clone() {
        if let Err(e) = link
            .call::<core_to_host::ConnCreate>(Some(&id), &create, timeout)
            .await
        {
            inner.log(
                LogLevel::Error,
                format!("cannot create connection {id}: {e}"),
            );
            continue;
        }
        if restarted
            && let Err(e) = link
                .call::<core_to_host::ConnResync>(Some(&id), &Empty {}, timeout)
                .await
        {
            inner.log(
                LogLevel::Warn,
                format!("cannot resync connection {id}: {e}"),
            );
        }
    }
    reg.link = Some(link);
}

/// Marks every connection's state stale and journals why.
async fn on_host_down(inner: &Inner, reason: &str) {
    let mut reg = inner.registry.lock().await;
    if let Some(link) = reg.link.take() {
        link.close();
    }
    let conns: Vec<ConnId> = reg.conns.keys().cloned().collect();
    drop(reg);
    for conn in conns {
        let cause = inner.append(NewEntry::new(
            Origin::system(),
            Record::ConnStatus(ConnStatusRecord {
                conn: conn.clone(),
                status: ConnStatusKind::Disconnected,
                message: Some(format!("plugin host down: {reason}")),
            }),
        ));
        mark_stale(inner, &conn, cause);
    }
}

fn mark_stale(inner: &Inner, conn: &str, cause: Option<Seq>) {
    let subtree = StatePath::root(conn);
    inner.shared.state.mutate(|tree| {
        let dirty = tree.set_quality(&subtree, Quality::Stale);
        if !dirty.is_empty() {
            inner.append(
                NewEntry::new(
                    Origin::system(),
                    Record::StateQuality(StateQualityRecord {
                        subtree: subtree.clone(),
                        quality: Quality::Stale,
                    }),
                )
                .cause(cause),
            );
        }
        ((), dirty)
    });
}

// ---------------------------------------------------------------------------
// Routing host notifications into the state tree and journal

fn route(inner: &Inner, ntf: Notification) {
    let decoded = match host_to_core::Notification::decode(&ntf.m, &ntf.p) {
        Ok(decoded) => decoded,
        // Unknown notifications are ignored (spec §10).
        Err(MethodError::Unknown(_)) => return,
        Err(e) => {
            tracing::warn!("dropping notification: {e}");
            inner.log(LogLevel::Warn, format!("dropped invalid notification: {e}"));
            return;
        }
    };
    use host_to_core::Notification as N;
    // `log` is the only notification that may come without a connection.
    if let N::Log(log) = &decoded
        && ntf.conn.is_none()
    {
        inner.append(NewEntry::new(
            Origin::system(),
            Record::Log(LogRecord {
                level: log.level,
                message: log.msg.clone(),
                fields: log.fields.clone(),
            }),
        ));
        return;
    }
    let Some(conn) = ntf.conn else {
        inner.log(
            LogLevel::Warn,
            format!("dropped `{}` without a connection", ntf.m),
        );
        return;
    };
    if !inner.modes().contains_key(&conn) {
        inner.log(
            LogLevel::Warn,
            format!("dropped `{}` for unknown connection {conn}", ntf.m),
        );
        return;
    }
    let origin = Origin::conn(conn.clone());
    match decoded {
        N::ConnStatus(status) => {
            let seq = inner.append(NewEntry::new(
                origin,
                Record::ConnStatus(ConnStatusRecord {
                    conn: conn.clone(),
                    status: status.status,
                    message: status.message,
                }),
            ));
            if matches!(
                status.status,
                ConnStatusKind::Disconnected | ConnStatusKind::Error
            ) {
                mark_stale(inner, &conn, seq);
            }
        }
        N::StateSnapshot(snapshot) => {
            let ops = snapshot
                .values
                .iter()
                .map(|PathValue { p, v }| JournalOp::Set {
                    path: StatePath::new(conn.clone(), p.clone()),
                    value: v.clone(),
                })
                .collect();
            let record = StatePatchRecord {
                ops,
                replaces: Some(StatePath::root(conn.clone())),
            };
            apply_state(inner, &conn, origin, record, None, |tree, meta| {
                tree.replace_subtree(&conn, &snapshot.values, meta)
            });
        }
        N::StatePatch(patch) => {
            let ops = patch
                .ops
                .iter()
                .map(|op| match op {
                    PatchOp::Set(PathValue { p, v }) => JournalOp::Set {
                        path: StatePath::new(conn.clone(), p.clone()),
                        value: v.clone(),
                    },
                    PatchOp::Delete(d) => JournalOp::Delete {
                        path: StatePath::new(conn.clone(), d.p.clone()),
                        delete: covalence_protocol::True,
                    },
                })
                .collect();
            let record = StatePatchRecord {
                ops,
                replaces: None,
            };
            apply_state(inner, &conn, origin, record, patch.ts, |tree, meta| {
                tree.apply_patch(&conn, &patch.ops, meta)
            });
        }
        N::EventEmit(event) => {
            inner.append(NewEntry::new(
                origin,
                Record::Event(EventRecord {
                    source: Source::Conn(conn),
                    event: event.event,
                    payload: event.payload,
                }),
            ));
        }
        N::Log(log) => {
            inner.append(NewEntry::new(
                origin,
                Record::Log(LogRecord {
                    level: log.level,
                    message: log.msg,
                    fields: log.fields,
                }),
            ));
        }
    }
}

/// Journals a state change, then applies it with the entry's seq, under the
/// tree lock so the tree and the journal agree on order.
fn apply_state(
    inner: &Inner,
    conn: &str,
    origin: Origin,
    record: StatePatchRecord,
    ts: Option<f64>,
    apply: impl FnOnce(&mut crate::state::StateTree, &Meta) -> crate::state::Dirty,
) {
    inner.shared.state.mutate(|tree| {
        let Some(seq) = inner.append(NewEntry::new(origin, Record::StatePatch(record))) else {
            return ((), Default::default());
        };
        let meta = Meta {
            ts: ts.map_or_else(clock::epoch_ms, |ts| ts as u64),
            seq,
            quality: Quality::Good,
            source: Source::Conn(conn.to_owned()),
        };
        ((), apply(tree, &meta))
    });
}

// ---------------------------------------------------------------------------
// The link: request correlation and framed stdio

type Outcome = Result<Value, ProtocolError>;

struct Link {
    out: mpsc::UnboundedSender<Message>,
    pending: Mutex<Option<HashMap<u32, oneshot::Sender<Outcome>>>>,
    next_id: AtomicU32,
}

impl Link {
    fn new(out: mpsc::UnboundedSender<Message>) -> Self {
        Link {
            out,
            pending: Mutex::new(Some(HashMap::new())),
            next_id: AtomicU32::new(1),
        }
    }

    fn pending(&self) -> MutexGuard<'_, Option<HashMap<u32, oneshot::Sender<Outcome>>>> {
        self.pending.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn send_request(
        &self,
        method: &str,
        conn: Option<&str>,
        p: Value,
    ) -> Result<(u32, oneshot::Receiver<Outcome>), ProtocolError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        match self.pending().as_mut() {
            Some(pending) => pending.insert(id, tx),
            None => return Err(not_connected("host link closed")),
        };
        let message = Message::Request(Request {
            id,
            m: method.to_owned(),
            conn: conn.map(str::to_owned),
            p,
        });
        if self.out.send(message).is_err() {
            self.forget(id);
            return Err(not_connected("host link closed"));
        }
        Ok((id, rx))
    }

    async fn call<M: Method>(
        &self,
        conn: Option<&str>,
        params: &M::Params,
        timeout: Duration,
    ) -> Result<M::Result, ProtocolError> {
        let (id, rx) = self.send_request(M::NAME, conn, to_value(params).map_err(internal)?)?;
        let result = match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(outcome)) => outcome?,
            Ok(Err(_)) => return Err(not_connected("host link closed")),
            Err(_) => {
                self.forget(id);
                return Err(ProtocolError::new(
                    ErrorCode::Timeout,
                    format!("`{}` timed out after {timeout:?}", M::NAME),
                ));
            }
        };
        from_value(&result).map_err(|e| {
            ProtocolError::new(
                ErrorCode::Internal,
                format!("invalid `{}` result: {e}", M::NAME),
            )
        })
    }

    fn complete(&self, id: u32, outcome: Outcome) {
        let sender = self.pending().as_mut().and_then(|p| p.remove(&id));
        if let Some(sender) = sender {
            let _ = sender.send(outcome);
        }
    }

    fn forget(&self, id: u32) {
        if let Some(pending) = self.pending().as_mut() {
            pending.remove(&id);
        }
    }

    fn respond(&self, id: u32, outcome: Outcome) {
        let _ = self.out.send(Message::Response(Response { id, outcome }));
    }

    /// Fails every pending call and refuses new ones.
    fn close(&self) {
        self.pending().take();
    }
}

async fn write_frames(
    mut stdin: ChildStdin,
    mut rx: mpsc::UnboundedReceiver<Message>,
    format: Format,
) {
    while let Some(message) = rx.recv().await {
        let frame = match framing::encode(&message, format) {
            Ok(frame) => frame,
            Err(e) => {
                tracing::error!("cannot encode message for host: {e}");
                continue;
            }
        };
        if stdin.write_all(&frame).await.is_err() || stdin.flush().await.is_err() {
            return;
        }
    }
}

async fn read_frames(
    mut stdout: ChildStdout,
    link: Arc<Link>,
    incoming: mpsc::UnboundedSender<Message>,
    format: Format,
) {
    let mut decoder = FrameDecoder::new(format);
    let mut buf = vec![0; 64 * 1024];
    loop {
        let n = match stdout.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        decoder.push(&buf[..n]);
        loop {
            match decoder.next_message() {
                Ok(None) => break,
                Ok(Some(Message::Response(res))) => link.complete(res.id, res.outcome),
                Ok(Some(message)) => {
                    if incoming.send(message).is_err() {
                        return;
                    }
                }
                Err(FrameError::TooLarge(len)) => {
                    tracing::error!("host sent a {len}-byte frame; dropping the link");
                    return;
                }
                // The frame was consumed; the stream is still aligned.
                Err(e) => tracing::warn!("undecodable frame from host: {e}"),
            }
        }
    }
}

fn not_connected(message: &str) -> ProtocolError {
    ProtocolError::new(ErrorCode::NotConnected, message)
}

fn invalid_request(message: &str) -> ProtocolError {
    ProtocolError::new(ErrorCode::InvalidRequest, message)
}

fn internal(e: impl std::fmt::Display) -> ProtocolError {
    ProtocolError::new(ErrorCode::Internal, e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_ranges() {
        assert!(version_in_range("0.1.0", "0.1.0", "0.1.x"));
        assert!(version_in_range("0.1.0", "0.1.0", "0.1.0"));
        assert!(version_in_range("0.1.0", "0.0.1", "0.2.0"));
        assert!(!version_in_range("0.1.0", "0.2.0", "0.2.x"));
        assert!(!version_in_range("0.1.0", "0.0.1", "0.0.x"));
    }
}

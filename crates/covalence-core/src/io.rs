//! Host transports: the I/O the core performs for connections (plugin
//! protocol §7, §7.1, plugin protocol spec §13.6).
//!
//! - `hid` and `serial` go through the device layer (`covalence-devices`).
//!   Their handles outlive a plugin host: when the host goes, the device stays
//!   open and its input is dropped, and the restarted connection's `io.open`
//!   gets the same handle back.
//! - `tcp` is a tokio socket per handle, reconnecting with backoff when asked.
//!   It closes with the host.
//! - Every handle keeps its recent traffic in a ring, for the inspector
//!   (`GET /connections/{id}/traffic`). Handles' rings stay after they close,
//!   up to [`CLOSED_KEPT`] per connection.
//!
//! The connection is always the envelope's `conn`, and a connection can use
//! only its own handles. Permissions aren't checked yet (plugin protocol spec §13.5).

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::time::Duration;

use covalence_devices::{DeviceError, Devices, HandleEvent, OpenOptions, matching};
use covalence_protocol::io::{
    Backoff, DeviceInfo, DeviceList, DeviceMatch, FeatureOp, Framing, IoClose, IoData, IoFeature,
    IoFeatureResult, IoKind, IoOpen, IoOpened, IoState, IoStateKind, IoWrite,
};
use covalence_protocol::methods::{NotificationMethod, core_to_host};
use covalence_protocol::{Bytes, ErrorCode, Message, Notification, ProtocolError, to_value};
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::clock;
use crate::state::ConnId;

/// Entries each handle's traffic ring keeps.
pub const TRAFFIC_ENTRIES: usize = 500;
/// Bytes of each message the ring keeps.
pub const TRAFFIC_BYTES: usize = 512;
/// Closed handles' rings kept per connection.
pub const CLOSED_KEPT: usize = 8;
/// How long the first TCP connect may take before `io.open` answers.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_BACKOFF: Duration = Duration::from_millis(500);
const MAX_BACKOFF: Duration = Duration::from_secs(10);
/// A `length`-framed message larger than this closes the connection.
const MAX_FRAME: usize = 16 * 1024 * 1024;

/// Which way a traffic entry went.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Dir {
    /// From the device or peer.
    In,
    /// To it.
    Out,
    /// The handle opened, closed or failed.
    State,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TrafficEntry {
    /// Milliseconds since the epoch.
    pub ts: u64,
    pub dir: Dir,
    /// The first [`TRAFFIC_BYTES`], in hex.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hex: Option<String>,
    /// The whole message's length.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub len: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// One handle's traffic, as the inspector shows it.
#[derive(Clone, Debug, Serialize)]
pub struct HandleTraffic {
    pub handle: String,
    pub conn: ConnId,
    pub kind: IoKind,
    pub target: String,
    pub open: bool,
    pub entries: Vec<TrafficEntry>,
}

struct Ring {
    conn: ConnId,
    kind: IoKind,
    target: String,
    open: bool,
    /// When it closed, for pruning: the oldest closed rings go first.
    closed_at: u64,
    entries: VecDeque<TrafficEntry>,
}

impl Ring {
    fn push(&mut self, dir: Dir, data: Option<&[u8]>, note: Option<String>) {
        if self.entries.len() == TRAFFIC_ENTRIES {
            self.entries.pop_front();
        }
        self.entries.push_back(TrafficEntry {
            ts: clock::epoch_ms(),
            dir,
            hex: data.map(|d| hex(&d[..d.len().min(TRAFFIC_BYTES)])),
            len: data.map(<[u8]>::len),
            note,
        });
    }
}

/// When `at` was, in milliseconds since the Unix epoch.
fn epoch_ms_at(at: std::time::Instant) -> f64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    (now.saturating_sub(at.elapsed())).as_secs_f64() * 1000.0
}

fn hex(data: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(data.len() * 2);
    for b in data {
        out.push(DIGITS[(b >> 4) as usize] as char);
        out.push(DIGITS[(b & 15) as usize] as char);
    }
    out
}

enum Entry {
    Device {
        conn: ConnId,
        pump: Option<JoinHandle<()>>,
    },
    Tcp {
        conn: ConnId,
        commands: mpsc::UnboundedSender<TcpCommand>,
        task: JoinHandle<()>,
    },
}

impl Entry {
    fn conn(&self) -> &str {
        match self {
            Entry::Device { conn, .. } | Entry::Tcp { conn, .. } => conn,
        }
    }
}

enum TcpCommand {
    Write(Vec<u8>),
    Close,
}

/// Starts a handle's traffic once `io.open` has been answered, so its
/// `io.state` and `io.data` follow the response.
pub struct Start(Option<oneshot::Sender<()>>);

impl Start {
    pub fn go(mut self) {
        if let Some(tx) = self.0.take() {
            let _ = tx.send(());
        }
    }
}

/// The transports. One per host supervisor.
pub struct IoHub {
    me: Weak<IoHub>,
    devices: OnceLock<Devices>,
    /// The current host link's outbox, while there is a host.
    out: Mutex<Option<mpsc::UnboundedSender<Message>>>,
    handles: Mutex<BTreeMap<String, Entry>>,
    traffic: Mutex<BTreeMap<String, Ring>>,
    next_tcp: AtomicU64,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn error(code: ErrorCode, message: impl Into<String>) -> ProtocolError {
    ProtocolError::new(code, message)
}

/// How a device error reaches a plugin.
fn device_error(e: &DeviceError) -> ProtocolError {
    let code = match e {
        DeviceError::Busy { .. } => ErrorCode::Busy,
        DeviceError::NotFound(_) => ErrorCode::NotFound,
        DeviceError::NotOpen(_) | DeviceError::Unplugged => ErrorCode::NotConnected,
        DeviceError::PermissionDenied(_) => ErrorCode::PermissionDenied,
        DeviceError::Unsupported(_) => ErrorCode::InvalidParams,
        _ => ErrorCode::Internal,
    };
    error(code, e.to_string())
}

impl IoHub {
    pub fn new() -> Arc<IoHub> {
        Arc::new_cyclic(|me| IoHub {
            me: me.clone(),
            devices: OnceLock::new(),
            out: Mutex::new(None),
            handles: Mutex::new(BTreeMap::new()),
            traffic: Mutex::new(BTreeMap::new()),
            next_tcp: AtomicU64::new(1),
        })
    }

    /// Gives the hub the device layer. Only the first call counts.
    pub fn set_devices(&self, devices: Devices) {
        let _ = self.devices.set(devices);
    }

    pub fn devices(&self) -> Option<&Devices> {
        self.devices.get()
    }

    /// A host is up: its notifications go to `out`.
    pub fn attach(&self, out: mpsc::UnboundedSender<Message>) {
        *lock(&self.out) = Some(out);
    }

    /// The host went away. Devices stay open, dropping input; sockets close.
    pub fn host_down(&self) {
        *lock(&self.out) = None;
        let entries = std::mem::take(&mut *lock(&self.handles));
        let mut conns = std::collections::BTreeSet::new();
        for (handle, entry) in entries {
            match entry {
                Entry::Device { conn, pump } => {
                    if let Some(pump) = pump {
                        pump.abort();
                    }
                    self.record(
                        &handle,
                        Dir::State,
                        None,
                        Some("plugin host down; kept open".into()),
                    );
                    conns.insert(conn);
                }
                Entry::Tcp { commands, task, .. } => {
                    let _ = commands.send(TcpCommand::Close);
                    task.abort();
                    self.closed(&handle, "plugin host down");
                }
            }
        }
        if let Some(devices) = self.devices() {
            for conn in conns {
                devices.detach(&conn);
            }
        }
    }

    /// A connection was destroyed: its handles close, and devices are freed.
    pub fn release(&self, conn: &str) {
        let handles: Vec<String> = lock(&self.handles)
            .iter()
            .filter(|(_, e)| e.conn() == conn)
            .map(|(h, _)| h.clone())
            .collect();
        for handle in handles {
            self.close_handle(&handle);
        }
        if let Some(devices) = self.devices() {
            devices.release(conn);
        }
    }

    fn notify<M: NotificationMethod>(&self, conn: &str, params: &M::Params)
    where
        M::Params: Serialize,
    {
        let Ok(p) = to_value(params) else { return };
        if let Some(out) = lock(&self.out).as_ref() {
            let _ = out.send(Message::Notification(Notification {
                m: M::NAME.to_owned(),
                conn: Some(conn.to_owned()),
                p,
            }));
        }
    }

    fn record(&self, handle: &str, dir: Dir, data: Option<&[u8]>, note: Option<String>) {
        if let Some(ring) = lock(&self.traffic).get_mut(handle) {
            ring.push(dir, data, note);
        }
    }

    fn new_ring(&self, handle: &str, conn: &str, kind: IoKind, target: &str) {
        let mut traffic = lock(&self.traffic);
        let ring = traffic.entry(handle.to_owned()).or_insert_with(|| Ring {
            conn: conn.to_owned(),
            kind,
            target: target.to_owned(),
            open: true,
            closed_at: 0,
            entries: VecDeque::new(),
        });
        ring.open = true;
        // Prune this connection's oldest closed rings.
        let mut closed: Vec<(u64, String)> = traffic
            .iter()
            .filter(|(_, r)| r.conn == conn && !r.open)
            .map(|(h, r)| (r.closed_at, h.clone()))
            .collect();
        if closed.len() > CLOSED_KEPT {
            closed.sort();
            for (_, h) in &closed[..closed.len() - CLOSED_KEPT] {
                traffic.remove(h);
            }
        }
    }

    fn closed(&self, handle: &str, why: &str) {
        if let Some(ring) = lock(&self.traffic).get_mut(handle) {
            ring.push(Dir::State, None, Some(format!("closed: {why}")));
            ring.open = false;
            ring.closed_at = clock::epoch_ms();
        }
    }

    /// The traffic of `conn`'s handles, open ones first.
    pub fn traffic(&self, conn: &str) -> Vec<HandleTraffic> {
        let mut out: Vec<HandleTraffic> = lock(&self.traffic)
            .iter()
            .filter(|(_, r)| r.conn == conn)
            .map(|(h, r)| HandleTraffic {
                handle: h.clone(),
                conn: r.conn.clone(),
                kind: r.kind,
                target: r.target.clone(),
                open: r.open,
                entries: r.entries.iter().cloned().collect(),
            })
            .collect();
        out.sort_by_key(|t| !t.open);
        out
    }

    /// `device.list`: present devices `matches` covers.
    pub fn device_list(&self, matches: &[DeviceMatch], params: &DeviceList) -> Vec<DeviceInfo> {
        match self.devices() {
            Some(devices) => matching::matching(matches, params.kind, &devices.list()),
            None => Vec::new(),
        }
    }

    /// `io.open`. Call [`Start::go`] once the response is sent.
    pub async fn open(&self, conn: &str, open: IoOpen) -> Result<(IoOpened, Start), ProtocolError> {
        match open.kind {
            IoKind::Hid | IoKind::Serial => self.open_device(conn, open),
            IoKind::Tcp => self.open_tcp(conn, open).await,
            other => Err(error(
                ErrorCode::InvalidParams,
                format!("{other:?} transports aren't supported yet"),
            )),
        }
    }

    fn open_device(&self, conn: &str, open: IoOpen) -> Result<(IoOpened, Start), ProtocolError> {
        let devices = self
            .devices()
            .ok_or_else(|| error(ErrorCode::NotFound, "this core has no device layer"))?;
        let want = match open.kind {
            IoKind::Hid => covalence_devices::DeviceKind::Hid,
            _ => covalence_devices::DeviceKind::Serial,
        };
        if let Some(found) = devices.get(&open.target)
            && found.kind != want
        {
            return Err(error(
                ErrorCode::InvalidParams,
                format!(
                    "{} is a {:?} device, not {:?}",
                    open.target, found.kind, open.kind
                ),
            ));
        }
        let options = OpenOptions {
            baud: open.options.as_ref().and_then(|o| o.baud),
        };
        let mut opened = devices
            .open(conn, &open.target, options)
            .map_err(|e| device_error(&e))?;
        let handle = opened.handle.clone();
        self.new_ring(&handle, conn, open.kind, &open.target);
        self.record(
            &handle,
            Dir::State,
            None,
            Some(
                if opened.reattached {
                    "reopened by its connection"
                } else {
                    "opened"
                }
                .into(),
            ),
        );

        let (go, ready) = oneshot::channel();
        let me = self.me.clone();
        let (pump_conn, pump_handle) = (conn.to_owned(), handle.clone());
        let pump = tokio::spawn(async move {
            if ready.await.is_err() {
                return;
            }
            while let Some(event) = opened.events.recv().await {
                let Some(hub) = me.upgrade() else { return };
                hub.device_event(&pump_conn, &pump_handle, event);
            }
        });
        let previous = lock(&self.handles).insert(
            handle.clone(),
            Entry::Device {
                conn: conn.to_owned(),
                pump: Some(pump),
            },
        );
        if let Some(Entry::Device {
            pump: Some(old), ..
        }) = previous
        {
            old.abort();
        }
        Ok((IoOpened { handle }, Start(Some(go))))
    }

    fn device_event(&self, conn: &str, handle: &str, event: HandleEvent) {
        match event {
            HandleEvent::Data { data, at } => {
                self.record(handle, Dir::In, Some(&data), None);
                self.notify::<core_to_host::IoData>(
                    conn,
                    &IoData {
                        handle: handle.to_owned(),
                        data: Bytes(data),
                        ts: Some(epoch_ms_at(at)),
                    },
                );
            }
            HandleEvent::State { state, error } => {
                let note = match &error {
                    Some(e) => format!("{state:?}: {e}"),
                    None => format!("{state:?}"),
                };
                self.record(handle, Dir::State, None, Some(note.to_lowercase()));
                self.notify::<core_to_host::IoState>(
                    conn,
                    &IoState {
                        handle: handle.to_owned(),
                        state,
                        error: error.map(|e| e.code().to_owned()),
                    },
                );
            }
        }
    }

    async fn open_tcp(&self, conn: &str, open: IoOpen) -> Result<(IoOpened, Start), ProtocolError> {
        let target = open.target.clone();
        if target
            .rsplit_once(':')
            .is_none_or(|(_, p)| p.parse::<u16>().is_err())
        {
            return Err(error(
                ErrorCode::InvalidParams,
                format!("tcp target {target:?} isn't host:port"),
            ));
        }
        let options = open.options.unwrap_or_default();
        let handle = format!("t{}", self.next_tcp.fetch_add(1, Ordering::Relaxed));
        let (commands, rx) = mpsc::unbounded_channel();
        let (first, connected) = oneshot::channel();
        let (go, ready) = oneshot::channel();
        let tcp = Tcp {
            hub: self.me.clone(),
            conn: conn.to_owned(),
            handle: handle.clone(),
            target: target.clone(),
            reconnect: options.reconnect.unwrap_or(false),
            backoff: options.backoff.unwrap_or_default(),
            framing: options.framing.unwrap_or(Framing::Raw),
        };
        self.new_ring(&handle, conn, IoKind::Tcp, &target);
        let reconnect = tcp.reconnect;
        let task = tokio::spawn(tcp.run(rx, first, ready));
        let outcome = connected
            .await
            .unwrap_or_else(|_| Err("connect task ended".into()));
        match outcome {
            Err(e) if !reconnect => {
                task.abort();
                self.closed(&handle, &e);
                Err(error(
                    ErrorCode::NotConnected,
                    format!("cannot connect to {target}: {e}"),
                ))
            }
            _ => {
                lock(&self.handles).insert(
                    handle.clone(),
                    Entry::Tcp {
                        conn: conn.to_owned(),
                        commands,
                        task,
                    },
                );
                Ok((IoOpened { handle }, Start(Some(go))))
            }
        }
    }

    /// The handle, if `conn` owns it.
    fn owned<'a>(
        &self,
        handles: &'a BTreeMap<String, Entry>,
        conn: &str,
        handle: &str,
    ) -> Option<&'a Entry> {
        let entry = handles.get(handle)?;
        if entry.conn() != conn {
            tracing::warn!(%conn, %handle, "connection used another connection's handle");
            return None;
        }
        Some(entry)
    }

    /// `io.write`.
    pub fn write(&self, conn: &str, write: IoWrite) {
        let handles = lock(&self.handles);
        let Some(entry) = self.owned(&handles, conn, &write.handle) else {
            return;
        };
        let IoWrite { handle, data } = write;
        let result = match entry {
            Entry::Device { .. } => match self.devices() {
                Some(devices) => devices
                    .write(&handle, data.0.clone())
                    .map_err(|e| e.to_string()),
                None => Err("no device layer".into()),
            },
            Entry::Tcp { commands, .. } => commands
                .send(TcpCommand::Write(data.0.clone()))
                .map_err(|_| "closed".to_owned()),
        };
        drop(handles);
        match result {
            Ok(()) => self.record(&handle, Dir::Out, Some(&data.0), None),
            Err(e) => self.record(
                &handle,
                Dir::Out,
                Some(&data.0),
                Some(format!("dropped: {e}")),
            ),
        }
    }

    /// `io.feature`.
    pub async fn feature(
        &self,
        conn: &str,
        feature: IoFeature,
    ) -> Result<IoFeatureResult, ProtocolError> {
        {
            let handles = lock(&self.handles);
            match self.owned(&handles, conn, &feature.handle) {
                Some(Entry::Device { .. }) => {}
                Some(Entry::Tcp { .. }) => {
                    return Err(error(
                        ErrorCode::InvalidParams,
                        "feature reports are hid only",
                    ));
                }
                None => {
                    return Err(error(
                        ErrorCode::NotFound,
                        format!("no handle {}", feature.handle),
                    ));
                }
            }
        }
        let devices = self
            .devices()
            .ok_or_else(|| error(ErrorCode::NotFound, "no device layer"))?;
        match feature.op {
            FeatureOp::Get => {
                let report_id = feature
                    .report_id
                    .ok_or_else(|| error(ErrorCode::InvalidParams, "get needs reportId"))?;
                let length = feature.length.unwrap_or(64) as usize;
                let data = devices
                    .get_feature(&feature.handle, report_id, length)
                    .await
                    .map_err(|e| device_error(&e))?;
                self.record(
                    &feature.handle,
                    Dir::In,
                    Some(&data),
                    Some(format!("feature report {report_id}")),
                );
                Ok(IoFeatureResult {
                    data: Some(Bytes(data)),
                })
            }
            FeatureOp::Send => {
                let data = feature
                    .data
                    .ok_or_else(|| error(ErrorCode::InvalidParams, "send needs data"))?;
                devices
                    .send_feature(&feature.handle, data.0.clone())
                    .await
                    .map_err(|e| device_error(&e))?;
                self.record(
                    &feature.handle,
                    Dir::Out,
                    Some(&data.0),
                    Some("feature report".into()),
                );
                Ok(IoFeatureResult::default())
            }
        }
    }

    /// `io.close`.
    pub fn close(&self, conn: &str, close: IoClose) {
        let owned = self
            .owned(&lock(&self.handles), conn, &close.handle)
            .is_some();
        if owned {
            self.close_handle(&close.handle);
        }
    }

    fn close_handle(&self, handle: &str) {
        let Some(entry) = lock(&self.handles).remove(handle) else {
            return;
        };
        match entry {
            Entry::Device { pump, .. } => {
                if let Some(pump) = pump {
                    pump.abort();
                }
                if let Some(devices) = self.devices() {
                    devices.close(handle);
                }
            }
            Entry::Tcp { commands, .. } => {
                let _ = commands.send(TcpCommand::Close);
            }
        }
        self.closed(handle, "closed by its connection");
    }
}

/// A TCP handle's task.
struct Tcp {
    hub: Weak<IoHub>,
    conn: ConnId,
    handle: String,
    target: String,
    reconnect: bool,
    backoff: Backoff,
    framing: Framing,
}

enum Ended {
    /// Closed by its connection or the host: stop.
    Closed,
    /// Lost: reconnect if asked to.
    Lost(String),
}

impl Tcp {
    fn state(&self, state: IoStateKind, error: Option<String>) {
        let Some(hub) = self.hub.upgrade() else {
            return;
        };
        let note = match &error {
            Some(e) => format!("{state:?}: {e}").to_lowercase(),
            None => format!("{state:?}").to_lowercase(),
        };
        hub.record(&self.handle, Dir::State, None, Some(note));
        hub.notify::<core_to_host::IoState>(
            &self.conn,
            &IoState {
                handle: self.handle.clone(),
                state,
                error,
            },
        );
    }

    fn data(&self, data: Vec<u8>) {
        let Some(hub) = self.hub.upgrade() else {
            return;
        };
        hub.record(&self.handle, Dir::In, Some(&data), None);
        hub.notify::<core_to_host::IoData>(
            &self.conn,
            &IoData {
                handle: self.handle.clone(),
                data: Bytes(data),
                ts: Some(epoch_ms_at(std::time::Instant::now())),
            },
        );
    }

    async fn run(
        self,
        mut commands: mpsc::UnboundedReceiver<TcpCommand>,
        first: oneshot::Sender<Result<(), String>>,
        ready: oneshot::Receiver<()>,
    ) {
        let initial = self
            .backoff
            .initial_ms
            .map_or(DEFAULT_BACKOFF, Duration::from_millis);
        let max = self
            .backoff
            .max_ms
            .map_or(MAX_BACKOFF, Duration::from_millis);
        let mut delay = initial;
        let mut first = Some(first);
        let mut ready = Some(ready);
        loop {
            let attempt = tokio::time::timeout(
                CONNECT_TIMEOUT,
                tokio::net::TcpStream::connect(&self.target),
            )
            .await;
            let attempt = match attempt {
                Ok(Ok(stream)) => Ok(stream),
                Ok(Err(e)) => Err(e.to_string()),
                Err(_) => Err(format!("no answer within {CONNECT_TIMEOUT:?}")),
            };
            if let Some(first) = first.take() {
                let _ = first.send(attempt.as_ref().map(|_| ()).map_err(Clone::clone));
            }
            // `io.state` only once `io.open` is answered.
            if let Some(ready) = ready.take()
                && ready.await.is_err()
            {
                return;
            }
            let lost = match attempt {
                Ok(stream) => {
                    let _ = stream.set_nodelay(true);
                    delay = initial;
                    self.state(IoStateKind::Open, None);
                    match self.serve(stream, &mut commands).await {
                        Ended::Closed => return,
                        Ended::Lost(e) => e,
                    }
                }
                Err(e) => e,
            };
            if !self.reconnect {
                self.state(IoStateKind::Closed, Some(lost));
                return;
            }
            self.state(IoStateKind::Reconnecting, Some(lost));
            // Writes while away are dropped; a close still stops it.
            let sleep = tokio::time::sleep(delay);
            tokio::pin!(sleep);
            loop {
                tokio::select! {
                    _ = &mut sleep => break,
                    command = commands.recv() => match command {
                        Some(TcpCommand::Write(data)) => {
                            if let Some(hub) = self.hub.upgrade() {
                                hub.record(&self.handle, Dir::Out, Some(&data), Some("dropped: reconnecting".into()));
                            }
                        }
                        Some(TcpCommand::Close) | None => return,
                    },
                }
            }
            delay = (delay * 2).min(max);
        }
    }

    async fn serve(
        &self,
        stream: tokio::net::TcpStream,
        commands: &mut mpsc::UnboundedReceiver<TcpCommand>,
    ) -> Ended {
        let (mut reader, mut writer) = stream.into_split();
        let mut buf = vec![0u8; 64 * 1024];
        let mut pending: Vec<u8> = Vec::new();
        loop {
            tokio::select! {
                read = reader.read(&mut buf) => match read {
                    Ok(0) => return Ended::Lost("closed by the peer".into()),
                    Ok(n) => {
                        if let Err(e) = self.frame(&buf[..n], &mut pending) {
                            return Ended::Lost(e);
                        }
                    }
                    Err(e) => return Ended::Lost(e.to_string()),
                },
                command = commands.recv() => match command {
                    Some(TcpCommand::Write(data)) => {
                        if let Err(e) = writer.write_all(&data).await {
                            return Ended::Lost(e.to_string());
                        }
                    }
                    Some(TcpCommand::Close) | None => {
                        let _ = writer.shutdown().await;
                        return Ended::Closed;
                    }
                },
            }
        }
    }

    /// Cuts what was read into `io.data` messages by the handle's framing.
    fn frame(&self, read: &[u8], pending: &mut Vec<u8>) -> Result<(), String> {
        match self.framing {
            Framing::Raw => self.data(read.to_vec()),
            Framing::Line => {
                pending.extend_from_slice(read);
                while let Some(end) = pending.iter().position(|&b| b == b'\n') {
                    let line: Vec<u8> = pending.drain(..=end).collect();
                    self.data(line);
                }
            }
            Framing::Length => {
                pending.extend_from_slice(read);
                while pending.len() >= 4 {
                    let len = u32::from_be_bytes([pending[0], pending[1], pending[2], pending[3]])
                        as usize;
                    if len > MAX_FRAME {
                        return Err(format!("a {len}-byte frame is too large"));
                    }
                    if pending.len() < 4 + len {
                        break;
                    }
                    let frame: Vec<u8> = pending.drain(..4 + len).skip(4).collect();
                    self.data(frame);
                }
            }
        }
        Ok(())
    }
}

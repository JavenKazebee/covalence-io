//! The device layer: what's plugged in, and the handles connections hold.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak, mpsc as std_mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use covalence_protocol::io::{DeviceInfo, IoStateKind};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::backend::{Backend, OpenOptions, Port};
use crate::{DeviceError, id};

/// How often devices are enumerated even without a hotplug notification: for
/// what isn't USB, and in case one is missed.
pub const POLL: Duration = Duration::from_secs(1);
/// After a hotplug notification, enumerate again this much later: a USB
/// device's HID and serial nodes can lag its arrival.
pub const SETTLE: Duration = Duration::from_millis(300);
/// How long a device's I/O thread waits for input before it sends what's
/// queued. Bounds how long a write waits behind a read.
pub const READ_POLL: Duration = Duration::from_millis(2);
/// Input and state events a handle buffers for a slow reader. More are dropped.
pub const HANDLE_BUFFER: usize = 1024;

#[derive(Clone, Debug)]
pub struct Settings {
    /// Watch for hotplug and poll on a thread of the device layer's own.
    /// Without it, devices are enumerated only by [`Devices::scan`].
    pub watch: bool,
    pub poll: Duration,
    pub read_poll: Duration,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            watch: true,
            poll: POLL,
            read_poll: READ_POLL,
        }
    }
}

impl Settings {
    /// No thread: devices change only when the caller calls [`Devices::scan`].
    pub fn manual() -> Self {
        Settings {
            watch: false,
            ..Settings::default()
        }
    }
}

/// A device that came or went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceEvent {
    Added(DeviceInfo),
    Removed(DeviceInfo),
}

/// What a handle's holder hears.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HandleEvent {
    /// An input report, or serial bytes, with when they were read.
    Data { data: Vec<u8>, at: Instant },
    /// The handle opened, closed or failed. `Closed` with
    /// [`DeviceError::Unplugged`] opens again when the device returns.
    State {
        state: IoStateKind,
        error: Option<DeviceError>,
    },
}

/// An open (or waiting) handle, from [`Devices::open`].
#[derive(Debug)]
pub struct Opened {
    pub handle: String,
    /// The handle's current state first, then its input and changes.
    pub events: mpsc::Receiver<HandleEvent>,
    /// The holder already had this handle: the plugin host restarted, and
    /// the device stayed open.
    pub reattached: bool,
}

/// A handle as the API shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HandleInfo {
    pub handle: String,
    pub device: String,
    pub owner: String,
    pub state: IoStateKind,
    pub error: Option<DeviceError>,
    /// Whether anything is listening; data is dropped while nothing is.
    pub attached: bool,
}

/// The device layer. Cheap to clone; clones share it.
#[derive(Clone)]
pub struct Devices {
    inner: Arc<Inner>,
}

struct Inner {
    /// Itself, for the I/O threads.
    me: Weak<Inner>,
    backend: Arc<dyn Backend>,
    settings: Settings,
    state: Mutex<State>,
    events: broadcast::Sender<DeviceEvent>,
    /// Asks the watcher to enumerate now. `None` in manual mode.
    poke: Option<std_mpsc::Sender<()>>,
    next_handle: AtomicU64,
}

#[derive(Default)]
struct State {
    present: BTreeMap<String, DeviceInfo>,
    /// By handle.
    slots: BTreeMap<String, Slot>,
    /// The last enumeration's error, if it failed.
    scan_error: Option<DeviceError>,
}

impl State {
    fn slot_for_device(&mut self, device: &str) -> Option<&mut Slot> {
        self.slots.values_mut().find(|s| s.device == device)
    }
}

type Sink = Arc<Mutex<Option<mpsc::Sender<HandleEvent>>>>;

struct Slot {
    handle: String,
    owner: String,
    device: String,
    options: OpenOptions,
    state: IoStateKind,
    error: Option<DeviceError>,
    io: Option<Io>,
    /// Bumped whenever `io` changes, so a thread that's been replaced can't
    /// report on its successor.
    generation: u64,
    sink: Sink,
}

impl Slot {
    fn set_state(&mut self, state: IoStateKind, error: Option<DeviceError>) {
        self.state = state;
        self.error = error.clone();
        send(&self.sink, HandleEvent::State { state, error });
    }

    fn info(&self) -> HandleInfo {
        HandleInfo {
            handle: self.handle.clone(),
            device: self.device.clone(),
            owner: self.owner.clone(),
            state: self.state,
            error: self.error.clone(),
            attached: lock(&self.sink).as_ref().is_some_and(|s| !s.is_closed()),
        }
    }
}

fn send(sink: &Sink, event: HandleEvent) {
    let mut sink = lock(sink);
    if let Some(tx) = sink.as_ref() {
        match tx.try_send(event) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::debug!("device handle's reader is behind; dropping an event");
            }
            Err(mpsc::error::TrySendError::Closed(_)) => *sink = None,
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

enum Command {
    Write(Vec<u8>),
    GetFeature {
        report_id: u8,
        length: usize,
        reply: oneshot::Sender<Result<Vec<u8>, DeviceError>>,
    },
    SendFeature {
        data: Vec<u8>,
        reply: oneshot::Sender<Result<(), DeviceError>>,
    },
    Stop,
}

/// A device's I/O thread, which owns its port.
struct Io {
    commands: std_mpsc::Sender<Command>,
    thread: Option<JoinHandle<()>>,
}

impl Io {
    /// Stops the thread and waits for it to let go of the port.
    fn stop(mut self) {
        let _ = self.commands.send(Command::Stop);
        if let Some(thread) = self.thread.take()
            && thread.thread().id() != std::thread::current().id()
        {
            let _ = thread.join();
        }
    }
}

impl Devices {
    /// Starts the device layer over `backend`. With [`Settings::watch`], it
    /// enumerates on a thread of its own, at once and then on hotplug and
    /// every [`Settings::poll`].
    pub fn new(backend: Arc<dyn Backend>, settings: Settings) -> Devices {
        let (poke, pokes) = if settings.watch {
            let (tx, rx) = std_mpsc::channel();
            (Some(tx), Some(rx))
        } else {
            (None, None)
        };
        let inner = Arc::new_cyclic(|me| Inner {
            me: me.clone(),
            backend,
            settings,
            state: Mutex::new(State::default()),
            events: broadcast::channel(256).0,
            poke,
            next_handle: AtomicU64::new(1),
        });
        if let Some(pokes) = pokes {
            let weak = Arc::downgrade(&inner);
            let backend = inner.backend.clone();
            let poke = inner.poke.clone().expect("watching");
            let poll = inner.settings.poll;
            std::thread::Builder::new()
                .name("covalence-devices".into())
                .spawn(move || watch(weak, backend, poke, pokes, poll))
                .expect("can spawn the device watcher");
        }
        Devices { inner }
    }

    /// The system's devices, when the `system` feature is built.
    #[cfg(feature = "system")]
    pub fn system() -> Devices {
        Devices::new(Arc::new(crate::system::System::new()), Settings::default())
    }

    /// Enumerates now, and acts on what came and went.
    pub fn scan(&self) {
        self.inner.scan();
    }

    /// Devices present at the last enumeration, by ID.
    pub fn list(&self) -> Vec<DeviceInfo> {
        lock(&self.inner.state).present.values().cloned().collect()
    }

    pub fn get(&self, device: &str) -> Option<DeviceInfo> {
        lock(&self.inner.state).present.get(device).cloned()
    }

    /// Why the last enumeration failed, if it did.
    pub fn scan_error(&self) -> Option<DeviceError> {
        lock(&self.inner.state).scan_error.clone()
    }

    /// Devices coming and going, from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<DeviceEvent> {
        self.inner.events.subscribe()
    }

    /// Every handle.
    pub fn handles(&self) -> Vec<HandleInfo> {
        lock(&self.inner.state)
            .slots
            .values()
            .map(Slot::info)
            .collect()
    }

    /// Which owner holds each device that has a handle.
    pub fn holders(&self) -> BTreeMap<String, String> {
        lock(&self.inner.state)
            .slots
            .values()
            .map(|s| (s.device.clone(), s.owner.clone()))
            .collect()
    }

    /// Opens `device` for `owner`, a connection.
    ///
    /// - Another owner's device is refused with [`DeviceError::Busy`].
    /// - The owner's own device gives back the handle it had, still open,
    ///   with a new event stream: the plugin host restarted.
    /// - A device that isn't plugged in, or that the OS won't open, still
    ///   gets a handle. Its first event says why it isn't open, and it opens
    ///   when the device is plugged in (again).
    pub fn open(
        &self,
        owner: &str,
        device: &str,
        options: OpenOptions,
    ) -> Result<Opened, DeviceError> {
        let (tx, events) = mpsc::channel(HANDLE_BUFFER);
        let mut state = lock(&self.inner.state);
        if let Some(slot) = state.slot_for_device(device) {
            if slot.owner != owner {
                return Err(DeviceError::Busy {
                    device: device.to_owned(),
                    holder: slot.owner.clone(),
                });
            }
            *lock(&slot.sink) = Some(tx);
            let (s, e) = (slot.state, slot.error.clone());
            send(&slot.sink, HandleEvent::State { state: s, error: e });
            return Ok(Opened {
                handle: slot.handle.clone(),
                events,
                reattached: true,
            });
        }
        let handle = format!(
            "d{}",
            self.inner.next_handle.fetch_add(1, Ordering::Relaxed)
        );
        let mut slot = Slot {
            handle: handle.clone(),
            owner: owner.to_owned(),
            device: device.to_owned(),
            options,
            state: IoStateKind::Closed,
            error: Some(DeviceError::Unplugged),
            io: None,
            generation: 0,
            sink: Arc::new(Mutex::new(Some(tx))),
        };
        match state.present.get(device).cloned() {
            Some(info) => self.inner.start(&mut slot, &info),
            None => slot.set_state(IoStateKind::Closed, Some(DeviceError::Unplugged)),
        }
        state.slots.insert(handle.clone(), slot);
        Ok(Opened {
            handle,
            events,
            reattached: false,
        })
    }

    /// Queues an output report, or serial bytes. Writes are sent in order.
    pub fn write(&self, handle: &str, data: Vec<u8>) -> Result<(), DeviceError> {
        self.inner.command(handle, Command::Write(data))
    }

    /// HID only: reads a feature report.
    pub async fn get_feature(
        &self,
        handle: &str,
        report_id: u8,
        length: usize,
    ) -> Result<Vec<u8>, DeviceError> {
        let (reply, answer) = oneshot::channel();
        self.inner.command(
            handle,
            Command::GetFeature {
                report_id,
                length,
                reply,
            },
        )?;
        answer.await.unwrap_or(Err(DeviceError::Unplugged))
    }

    /// HID only: sends a feature report, its ID first.
    pub async fn send_feature(&self, handle: &str, data: Vec<u8>) -> Result<(), DeviceError> {
        let (reply, answer) = oneshot::channel();
        self.inner
            .command(handle, Command::SendFeature { data, reply })?;
        answer.await.unwrap_or(Err(DeviceError::Unplugged))
    }

    /// Lets go of a handle and its device.
    pub fn close(&self, handle: &str) {
        let io = {
            let mut state = lock(&self.inner.state);
            state.slots.remove(handle).and_then(|mut s| s.io.take())
        };
        if let Some(io) = io {
            io.stop();
        }
    }

    /// Lets go of every handle `owner` holds: the connection was deleted or
    /// disabled.
    pub fn release(&self, owner: &str) {
        let ios: Vec<Io> = {
            let mut state = lock(&self.inner.state);
            let handles: Vec<String> = state
                .slots
                .values()
                .filter(|s| s.owner == owner)
                .map(|s| s.handle.clone())
                .collect();
            handles
                .iter()
                .filter_map(|h| state.slots.remove(h).and_then(|mut s| s.io.take()))
                .collect()
        };
        for io in ios {
            io.stop();
        }
    }

    /// Stops sending `owner`'s handles' events, keeping its devices open:
    /// the plugin host went away. Data is dropped until it opens them again.
    pub fn detach(&self, owner: &str) {
        let state = lock(&self.inner.state);
        for slot in state.slots.values().filter(|s| s.owner == owner) {
            *lock(&slot.sink) = None;
        }
    }
}

impl Inner {
    fn scan(&self) {
        let found = self.backend.enumerate();
        let mut stopping = Vec::new();
        {
            let mut state = lock(&self.state);
            let mut found = match found {
                Ok(found) => {
                    state.scan_error = None;
                    found
                }
                Err(e) => {
                    if state.scan_error.as_ref() != Some(&e) {
                        tracing::warn!("cannot enumerate devices: {e}");
                    }
                    state.scan_error = Some(e);
                    return;
                }
            };
            id::assign_ids(&mut found);
            let found: BTreeMap<String, DeviceInfo> =
                found.into_iter().map(|d| (d.id.clone(), d)).collect();

            let removed: Vec<DeviceInfo> = state
                .present
                .values()
                .filter(|d| !found.contains_key(&d.id))
                .cloned()
                .collect();
            let added: Vec<DeviceInfo> = found
                .values()
                .filter(|d| !state.present.contains_key(&d.id))
                .cloned()
                .collect();
            state.present = found;

            for device in &removed {
                tracing::info!(device = %device.id, "device removed");
                if let Some(slot) = state.slot_for_device(&device.id) {
                    if let Some(io) = slot.io.take() {
                        slot.generation += 1;
                        stopping.push(io);
                    }
                    // Its I/O may have noticed first.
                    if slot.error != Some(DeviceError::Unplugged) {
                        slot.set_state(IoStateKind::Closed, Some(DeviceError::Unplugged));
                    }
                }
                let _ = self.events.send(DeviceEvent::Removed(device.clone()));
            }
            for device in &added {
                tracing::info!(device = %device.id, "device added");
                let _ = self.events.send(DeviceEvent::Added(device.clone()));
            }
            // Handles waiting for their device: newly plugged in, or present
            // after an I/O failure that looked like an unplug. One that failed
            // to open for another reason waits to be plugged in again.
            let present = state.present.clone();
            let newly: std::collections::BTreeSet<&str> =
                added.iter().map(|d| d.id.as_str()).collect();
            for slot in state.slots.values_mut() {
                let Some(info) = present.get(&slot.device) else {
                    continue;
                };
                let waiting = slot.io.is_none()
                    && (newly.contains(slot.device.as_str())
                        || slot.error == Some(DeviceError::Unplugged));
                if waiting {
                    self.start(slot, info);
                }
            }
        }
        for io in stopping {
            io.stop();
        }
    }

    /// Opens a slot's device and starts its I/O thread.
    fn start(&self, slot: &mut Slot, info: &DeviceInfo) {
        match self.backend.open(info, &slot.options) {
            Ok(port) => {
                slot.generation += 1;
                slot.io = Some(self.spawn_io(slot, port));
                tracing::info!(device = %slot.device, handle = %slot.handle, "device opened");
                slot.set_state(IoStateKind::Open, None);
            }
            Err(e) => {
                tracing::warn!(device = %slot.device, "cannot open device: {e}");
                let state = if e == DeviceError::Unplugged {
                    IoStateKind::Closed
                } else {
                    IoStateKind::Error
                };
                slot.set_state(state, Some(e));
            }
        }
    }

    fn spawn_io(&self, slot: &Slot, port: Box<dyn Port>) -> Io {
        let (commands, rx) = std_mpsc::channel();
        let me = self.me.clone();
        let handle = slot.handle.clone();
        let generation = slot.generation;
        let sink = slot.sink.clone();
        let read_poll = self.settings.read_poll;
        let thread = std::thread::Builder::new()
            .name(format!("covalence-device-{handle}"))
            .spawn(move || {
                let failed = run_io(port, rx, &sink, read_poll);
                if let (Some(e), Some(me)) = (failed, me.upgrade()) {
                    me.io_failed(&handle, generation, e);
                }
            })
            .expect("can spawn a device thread");
        Io {
            commands,
            thread: Some(thread),
        }
    }

    fn io_failed(&self, handle: &str, generation: u64, error: DeviceError) {
        {
            let mut state = lock(&self.state);
            let Some(slot) = state.slots.get_mut(handle) else {
                return;
            };
            if slot.generation != generation {
                return;
            }
            slot.io = None;
            slot.generation += 1;
            tracing::warn!(device = %slot.device, "device I/O failed: {error}");
            let kind = if error == DeviceError::Unplugged {
                IoStateKind::Closed
            } else {
                IoStateKind::Error
            };
            slot.set_state(kind, Some(error));
        }
        if let Some(poke) = &self.poke {
            let _ = poke.send(());
        }
    }

    fn command(&self, handle: &str, command: Command) -> Result<(), DeviceError> {
        let state = lock(&self.state);
        let io = state
            .slots
            .get(handle)
            .and_then(|s| s.io.as_ref())
            .ok_or_else(|| DeviceError::NotOpen(handle.to_owned()))?;
        io.commands
            .send(command)
            .map_err(|_| DeviceError::NotOpen(handle.to_owned()))
    }
}

/// Runs a device's I/O until it's stopped, or fails. The port is dropped
/// before this returns, so the device can be opened again at once.
fn run_io(
    mut port: Box<dyn Port>,
    commands: std_mpsc::Receiver<Command>,
    sink: &Sink,
    read_poll: Duration,
) -> Option<DeviceError> {
    loop {
        loop {
            match commands.try_recv() {
                Ok(Command::Write(data)) => {
                    if let Err(e) = port.write(&data) {
                        return Some(e);
                    }
                }
                Ok(Command::GetFeature {
                    report_id,
                    length,
                    reply,
                }) => {
                    let _ = reply.send(port.get_feature(report_id, length));
                }
                Ok(Command::SendFeature { data, reply }) => {
                    let _ = reply.send(port.send_feature(&data));
                }
                Ok(Command::Stop) | Err(std_mpsc::TryRecvError::Disconnected) => return None,
                Err(std_mpsc::TryRecvError::Empty) => break,
            }
        }
        match port.read(read_poll) {
            Ok(Some(data)) => send(
                sink,
                HandleEvent::Data {
                    data,
                    at: Instant::now(),
                },
            ),
            Ok(None) => {}
            Err(e) => return Some(e),
        }
    }
}

/// The watcher thread: enumerates at once, on every hotplug notification
/// and again [`SETTLE`] after it, and every `poll`.
fn watch(
    weak: Weak<Inner>,
    backend: Arc<dyn Backend>,
    poke: std_mpsc::Sender<()>,
    pokes: std_mpsc::Receiver<()>,
    poll: Duration,
) {
    let _guard = backend.watch(Box::new(move || {
        let _ = poke.send(());
    }));
    drop(backend);
    let mut settle: Option<Instant> = None;
    let mut first = true;
    loop {
        let wait = if first {
            Duration::ZERO
        } else {
            settle.map_or(poll, |at| at.saturating_duration_since(Instant::now()))
        };
        first = false;
        let woke = pokes.recv_timeout(wait);
        let Some(inner) = weak.upgrade() else {
            return;
        };
        match woke {
            Ok(()) => {
                while pokes.try_recv().is_ok() {}
                settle = Some(Instant::now() + SETTLE);
            }
            Err(std_mpsc::RecvTimeoutError::Timeout) => settle = None,
            Err(std_mpsc::RecvTimeoutError::Disconnected) => return,
        }
        inner.scan();
    }
}

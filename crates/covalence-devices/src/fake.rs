//! A backend of pretend devices, for tests: plug and unplug them, send input
//! reports, and see what was written to them.

use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use covalence_protocol::io::{DeviceInfo, DeviceKind};

use crate::DeviceError;
use crate::backend::{Backend, OpenOptions, Port};

/// Pretend devices. Clones share them.
#[derive(Clone, Default)]
pub struct Fake {
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    state: Mutex<FakeState>,
    /// Woken on input, unplug and close.
    changed: Condvar,
    watchers: Mutex<Vec<Arc<dyn Fn() + Send + Sync>>>,
}

#[derive(Default)]
struct FakeState {
    /// By path.
    devices: BTreeMap<String, FakeDevice>,
    enumerate_error: Option<DeviceError>,
}

struct FakeDevice {
    info: DeviceInfo,
    present: bool,
    /// Bumped on each plug, so a port from before an unplug sees it's gone.
    plugged: u64,
    open_error: Option<DeviceError>,
    /// How many ports are open: more than one means the layer opened twice.
    open: usize,
    input: Vec<Vec<u8>>,
    log: Log,
    /// Feature reports `get` answers, by report ID.
    features: BTreeMap<u8, Vec<u8>>,
}

/// What was done to a device, in order, across plugs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Log {
    pub writes: Vec<Vec<u8>>,
    pub features_sent: Vec<Vec<u8>>,
    pub opens: usize,
    pub baud: Option<u32>,
}

impl Fake {
    pub fn new() -> Fake {
        Fake::default()
    }

    fn lock(&self) -> MutexGuard<'_, FakeState> {
        self.shared.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn notify(&self) {
        self.shared.changed.notify_all();
        let watchers = self
            .shared
            .watchers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        for watcher in watchers {
            watcher();
        }
    }

    /// Plugs a device in at `info.path`, or plugs it back in with its log.
    pub fn plug(&self, info: DeviceInfo) {
        {
            let mut state = self.lock();
            let path = info.path.clone();
            match state.devices.get_mut(&path) {
                Some(d) => {
                    d.info = info;
                    d.present = true;
                    d.plugged += 1;
                }
                None => {
                    state.devices.insert(
                        path,
                        FakeDevice {
                            info,
                            present: true,
                            plugged: 1,
                            open_error: None,
                            open: 0,
                            input: Vec::new(),
                            log: Log::default(),
                            features: BTreeMap::new(),
                        },
                    );
                }
            }
        }
        self.notify();
    }

    /// Unplugs the device at `path`. Its log stays, for when it returns.
    pub fn unplug(&self, path: &str) {
        {
            let mut state = self.lock();
            if let Some(d) = state.devices.get_mut(path) {
                d.present = false;
                d.plugged += 1;
                d.input.clear();
            }
        }
        self.notify();
    }

    /// Sends an input report (or serial bytes) from the device at `path`.
    pub fn input(&self, path: &str, data: &[u8]) {
        if let Some(d) = self.lock().devices.get_mut(path) {
            d.input.push(data.to_vec());
        }
        self.shared.changed.notify_all();
    }

    /// Makes opening the device at `path` fail, until `None`.
    pub fn fail_open(&self, path: &str, error: Option<DeviceError>) {
        if let Some(d) = self.lock().devices.get_mut(path) {
            d.open_error = error;
        }
    }

    /// Makes enumeration fail, until `None`.
    pub fn fail_enumerate(&self, error: Option<DeviceError>) {
        self.lock().enumerate_error = error;
    }

    /// Sets what `get` of feature report `report_id` answers.
    pub fn set_feature(&self, path: &str, report_id: u8, data: &[u8]) {
        if let Some(d) = self.lock().devices.get_mut(path) {
            d.features.insert(report_id, data.to_vec());
        }
    }

    /// What was done to the device at `path`, plugged in or not.
    pub fn log(&self, path: &str) -> Log {
        self.lock()
            .devices
            .get(path)
            .map(|d| d.log.clone())
            .unwrap_or_default()
    }

    /// How many ports to the device at `path` are open now.
    pub fn open_ports(&self, path: &str) -> usize {
        self.lock().devices.get(path).map_or(0, |d| d.open)
    }

    /// Waits until `f` holds for the log of `path`, up to `timeout`.
    pub fn wait_log(&self, path: &str, timeout: Duration, f: impl Fn(&Log) -> bool) -> Log {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let log = self.log(path);
            if f(&log) || std::time::Instant::now() >= deadline {
                return log;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

impl Backend for Fake {
    fn enumerate(&self) -> Result<Vec<DeviceInfo>, DeviceError> {
        let state = self.lock();
        if let Some(e) = &state.enumerate_error {
            return Err(e.clone());
        }
        Ok(state
            .devices
            .values()
            .filter(|d| d.present)
            .map(|d| d.info.clone())
            .collect())
    }

    fn open(
        &self,
        device: &DeviceInfo,
        options: &OpenOptions,
    ) -> Result<Box<dyn Port>, DeviceError> {
        let mut state = self.lock();
        let d = state
            .devices
            .get_mut(&device.path)
            .filter(|d| d.present)
            .ok_or(DeviceError::Unplugged)?;
        if let Some(e) = &d.open_error {
            return Err(e.clone());
        }
        d.open += 1;
        d.log.opens += 1;
        if device.kind == DeviceKind::Serial {
            d.log.baud = Some(options.baud.unwrap_or(crate::backend::DEFAULT_BAUD));
        }
        Ok(Box::new(FakePort {
            fake: self.clone(),
            path: device.path.clone(),
            plugged: d.plugged,
            hid: device.kind == DeviceKind::Hid,
        }))
    }

    fn watch(&self, changed: Box<dyn Fn() + Send + Sync>) -> Option<Box<dyn Send>> {
        let changed: Arc<dyn Fn() + Send + Sync> = Arc::from(changed);
        self.shared
            .watchers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(changed);
        Some(Box::new(()))
    }
}

struct FakePort {
    fake: Fake,
    path: String,
    plugged: u64,
    hid: bool,
}

impl FakePort {
    /// The device, if it's the one this port opened.
    fn device<'a>(&self, state: &'a mut FakeState) -> Result<&'a mut FakeDevice, DeviceError> {
        match state.devices.get_mut(&self.path) {
            Some(d) if d.present && d.plugged == self.plugged => Ok(d),
            _ => Err(DeviceError::Unplugged),
        }
    }
}

impl Port for FakePort {
    fn read(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, DeviceError> {
        let mut state = self.fake.lock();
        let device = self.device(&mut state)?;
        if !device.input.is_empty() {
            return Ok(Some(device.input.remove(0)));
        }
        let (mut state, _) = self
            .fake
            .shared
            .changed
            .wait_timeout(state, timeout)
            .unwrap_or_else(|e| e.into_inner());
        let device = self.device(&mut state)?;
        Ok((!device.input.is_empty()).then(|| device.input.remove(0)))
    }

    fn write(&mut self, data: &[u8]) -> Result<(), DeviceError> {
        let mut state = self.fake.lock();
        self.device(&mut state)?.log.writes.push(data.to_vec());
        Ok(())
    }

    fn get_feature(&mut self, report_id: u8, length: usize) -> Result<Vec<u8>, DeviceError> {
        if !self.hid {
            return Err(DeviceError::Unsupported("feature reports on serial".into()));
        }
        let mut state = self.fake.lock();
        let device = self.device(&mut state)?;
        let mut data = device
            .features
            .get(&report_id)
            .cloned()
            .unwrap_or_else(|| vec![report_id]);
        data.truncate(length);
        Ok(data)
    }

    fn send_feature(&mut self, data: &[u8]) -> Result<(), DeviceError> {
        if !self.hid {
            return Err(DeviceError::Unsupported("feature reports on serial".into()));
        }
        let mut state = self.fake.lock();
        self.device(&mut state)?
            .log
            .features_sent
            .push(data.to_vec());
        Ok(())
    }
}

impl Drop for FakePort {
    fn drop(&mut self) {
        if let Some(d) = self.fake.lock().devices.get_mut(&self.path) {
            d.open = d.open.saturating_sub(1);
        }
    }
}

/// A Stream Deck MK.2's `DeviceInfo`, at `path`, with `serial`.
pub fn stream_deck_mk2(path: &str, serial: Option<&str>) -> DeviceInfo {
    DeviceInfo {
        id: String::new(),
        kind: DeviceKind::Hid,
        vendor_id: Some(0x0fd9),
        product_id: Some(0x0080),
        serial: serial.map(Into::into),
        manufacturer: Some("Elgato".into()),
        product: Some("Stream Deck MK.2".into()),
        path: path.into(),
        usage_page: Some(0x000c),
        usage: Some(0x0001),
    }
}

/// An ESP32 over USB CDC, at `path`.
pub fn esp32(path: &str, serial: Option<&str>) -> DeviceInfo {
    DeviceInfo {
        id: String::new(),
        kind: DeviceKind::Serial,
        vendor_id: Some(0x303a),
        product_id: Some(0x1001),
        serial: serial.map(Into::into),
        manufacturer: Some("Espressif".into()),
        product: Some("USB JTAG/serial debug unit".into()),
        path: path.into(),
        usage_page: None,
        usage: None,
    }
}

/// A test deck at `path`: the test plugin's device, on pid.codes' test
/// vendor ID, so it isn't a Stream Deck.
pub fn test_deck(path: &str, serial: Option<&str>) -> DeviceInfo {
    DeviceInfo {
        vendor_id: Some(0x1209),
        product_id: Some(0x0001),
        manufacturer: Some("Covalence".into()),
        product: Some("Test deck".into()),
        ..stream_deck_mk2(path, serial)
    }
}

/// A Stream Deck MK.2, at the level of its HID reports: what
/// `@elgato-stream-deck/core` reads and writes (M7 Phase 4).
///
/// - Feature report 6 answers the serial number, and 5 the firmware.
/// - [`Mk2::press`] and [`Mk2::release`] send key reports:
///   `[0x01, 0x00, 0x0f, 0x00, k0 … k14]`, 512 bytes.
/// - [`Mk2::images`] reassembles the JPEG written to each key from its
///   1024-byte image reports: `[0x02, 0x07, key, last, len (LE), part (LE),
///   …]`.
/// - [`Mk2::brightness`] is the last brightness set: `[0x03, 0x08, percent]`.
pub struct Mk2 {
    fake: Fake,
    path: String,
    keys: std::sync::Mutex<[bool; MK2_KEYS]>,
}

/// An MK.2's keys: 5 × 3.
pub const MK2_KEYS: usize = 15;

impl Mk2 {
    /// Plugs one in.
    pub fn plug(fake: &Fake, path: &str, serial: &str) -> Mk2 {
        fake.plug(stream_deck_mk2(path, Some(serial)));
        let mut serial_report = vec![0x06, serial.len() as u8];
        serial_report.extend_from_slice(serial.as_bytes());
        serial_report.resize(32, 0);
        fake.set_feature(path, 0x06, &serial_report);
        let firmware = b"1.01.000";
        let mut firmware_report = vec![0x05, 4 + firmware.len() as u8, 0, 0, 0, 0];
        firmware_report.extend_from_slice(firmware);
        firmware_report.resize(32, 0);
        fake.set_feature(path, 0x05, &firmware_report);
        Mk2 {
            fake: fake.clone(),
            path: path.to_owned(),
            keys: std::sync::Mutex::new([false; MK2_KEYS]),
        }
    }

    fn send_keys(&self, keys: &[bool; MK2_KEYS]) {
        let mut report = vec![0x01, 0x00, MK2_KEYS as u8, 0x00];
        report.extend(keys.iter().map(|&k| u8::from(k)));
        report.resize(512, 0);
        self.fake.input(&self.path, &report);
    }

    pub fn press(&self, key: usize) {
        let mut keys = self.keys.lock().unwrap_or_else(|e| e.into_inner());
        keys[key] = true;
        self.send_keys(&keys);
    }

    pub fn release(&self, key: usize) {
        let mut keys = self.keys.lock().unwrap_or_else(|e| e.into_inner());
        keys[key] = false;
        self.send_keys(&keys);
    }

    /// Every complete image written to each key, oldest first.
    pub fn images(&self) -> BTreeMap<u8, Vec<Vec<u8>>> {
        let mut done: BTreeMap<u8, Vec<Vec<u8>>> = BTreeMap::new();
        let mut partial: BTreeMap<u8, Vec<u8>> = BTreeMap::new();
        for w in self.fake.log(&self.path).writes {
            if w.len() < 8 || w[0] != 0x02 || w[1] != 0x07 {
                continue;
            }
            let (key, last) = (w[2], w[3] == 1);
            let len = usize::from(u16::from_le_bytes([w[4], w[5]]));
            let part = u16::from_le_bytes([w[6], w[7]]);
            let body = &w[8..(8 + len).min(w.len())];
            let image = partial.entry(key).or_default();
            if part == 0 {
                image.clear();
            }
            image.extend_from_slice(body);
            if last {
                done.entry(key)
                    .or_default()
                    .push(partial.remove(&key).unwrap_or_default());
            }
        }
        done
    }

    /// The last brightness set, in percent.
    pub fn brightness(&self) -> Option<u8> {
        self.fake
            .log(&self.path)
            .features_sent
            .iter()
            .rev()
            .find(|f| f.len() >= 3 && f[0] == 0x03 && f[1] == 0x08)
            .map(|f| f[2])
    }

    pub fn unplug(&self) {
        self.fake.unplug(&self.path);
    }

    pub fn path(&self) -> &str {
        &self.path
    }
}

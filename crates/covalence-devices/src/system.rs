//! The OS's devices: HID through `hidapi`, serial through `serialport`, and
//! USB arrivals and departures through `nusb`. None needs system libraries.

use std::ffi::CString;
use std::io::{Read, Write};
use std::sync::Mutex;
use std::time::Duration;

use covalence_protocol::io::{DeviceInfo, DeviceKind};
use hidapi::{HidApi, HidDevice, HidError};

use crate::DeviceError;
use crate::backend::{Backend, DEFAULT_BAUD, OpenOptions, Port};

/// Large enough for any input report: HID full-speed reports are at most 64
/// bytes, high-speed ones 1024, and Stream Decks' are 512.
const REPORT_BUFFER: usize = 4096;
const SERIAL_BUFFER: usize = 4096;

pub struct System {
    /// hidapi allows one context per process. `None` until first used, or
    /// if it can't start.
    hid: Mutex<Option<HidApi>>,
}

impl Default for System {
    fn default() -> Self {
        System::new()
    }
}

impl System {
    pub fn new() -> System {
        System {
            hid: Mutex::new(None),
        }
    }

    fn hid_devices(&self) -> Result<Vec<DeviceInfo>, DeviceError> {
        let mut hid = self.hid.lock().unwrap_or_else(|e| e.into_inner());
        match hid.as_mut() {
            Some(api) => api.refresh_devices().map_err(hid_error)?,
            None => *hid = Some(HidApi::new().map_err(hid_error)?),
        }
        let api = hid.as_ref().expect("started");
        Ok(api
            .device_list()
            .map(|d| DeviceInfo {
                id: String::new(),
                kind: DeviceKind::Hid,
                vendor_id: Some(d.vendor_id()),
                product_id: Some(d.product_id()),
                serial: d
                    .serial_number()
                    .map(Into::into)
                    .filter(|s: &String| !s.is_empty()),
                manufacturer: d.manufacturer_string().map(Into::into),
                product: d.product_string().map(Into::into),
                path: d.path().to_string_lossy().into_owned(),
                usage_page: Some(d.usage_page()),
                usage: Some(d.usage()),
            })
            .collect())
    }
}

fn serial_devices() -> Result<Vec<DeviceInfo>, DeviceError> {
    let ports = serialport::available_ports().map_err(serial_error)?;
    Ok(ports
        .into_iter()
        // macOS lists each port twice; `cu.` is the one to open.
        .filter(|p| !(cfg!(target_os = "macos") && p.port_name.starts_with("/dev/tty.")))
        .map(|p| {
            let usb = match p.port_type {
                serialport::SerialPortType::UsbPort(usb) => Some(usb),
                _ => None,
            };
            DeviceInfo {
                id: String::new(),
                kind: DeviceKind::Serial,
                vendor_id: usb.as_ref().map(|u| u.vid),
                product_id: usb.as_ref().map(|u| u.pid),
                serial: usb.as_ref().and_then(|u| u.serial_number.clone()),
                manufacturer: usb.as_ref().and_then(|u| u.manufacturer.clone()),
                product: usb.as_ref().and_then(|u| u.product.clone()),
                path: p.port_name,
                usage_page: None,
                usage: None,
            }
        })
        .collect())
}

impl Backend for System {
    fn enumerate(&self) -> Result<Vec<DeviceInfo>, DeviceError> {
        // One kind failing doesn't hide the other.
        let hid = self.hid_devices();
        let serial = serial_devices();
        match (hid, serial) {
            (Err(e), Err(_)) => Err(e),
            (hid, serial) => {
                for e in [hid.as_ref().err(), serial.as_ref().err()]
                    .into_iter()
                    .flatten()
                {
                    tracing::debug!("cannot enumerate some devices: {e}");
                }
                Ok(hid
                    .unwrap_or_default()
                    .into_iter()
                    .chain(serial.unwrap_or_default())
                    .collect())
            }
        }
    }

    fn open(
        &self,
        device: &DeviceInfo,
        options: &OpenOptions,
    ) -> Result<Box<dyn Port>, DeviceError> {
        match device.kind {
            DeviceKind::Hid => {
                let path = CString::new(device.path.clone())
                    .map_err(|_| DeviceError::NotFound(device.path.clone()))?;
                let hid = self.hid.lock().unwrap_or_else(|e| e.into_inner());
                let api = hid.as_ref().ok_or(DeviceError::Unplugged)?;
                let dev = api.open_path(&path).map_err(hid_error)?;
                Ok(Box::new(HidPort(dev)))
            }
            DeviceKind::Serial => {
                let port = serialport::new(&device.path, options.baud.unwrap_or(DEFAULT_BAUD))
                    .timeout(Duration::from_millis(2))
                    .open()
                    .map_err(serial_error)?;
                Ok(Box::new(SerialPort {
                    port,
                    timeout: Duration::from_millis(2),
                }))
            }
            DeviceKind::Midi => Err(DeviceError::Unsupported("midi".into())),
        }
    }

    fn watch(&self, changed: Box<dyn Fn() + Send + Sync>) -> Option<Box<dyn Send>> {
        let watch = match nusb::watch_devices() {
            Ok(watch) => watch,
            Err(e) => {
                tracing::warn!("cannot watch USB hotplug, polling only: {e}");
                return None;
            }
        };
        // The thread lives as long as the process: nusb's watch can't be
        // woken to stop, and the device layer lives as long as the server.
        std::thread::Builder::new()
            .name("covalence-usb-hotplug".into())
            .spawn(move || {
                use futures_lite::StreamExt;
                let mut watch = watch;
                futures_lite::future::block_on(async {
                    while watch.next().await.is_some() {
                        changed();
                    }
                });
            })
            .ok()?;
        Some(Box::new(()))
    }
}

struct HidPort(HidDevice);

impl Port for HidPort {
    fn read(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, DeviceError> {
        let mut buf = vec![0u8; REPORT_BUFFER];
        let ms = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
        match self.0.read_timeout(&mut buf, ms).map_err(hid_error)? {
            0 => Ok(None),
            n => {
                buf.truncate(n);
                Ok(Some(buf))
            }
        }
    }

    fn write(&mut self, data: &[u8]) -> Result<(), DeviceError> {
        self.0.write(data).map_err(hid_error)?;
        Ok(())
    }

    fn get_feature(&mut self, report_id: u8, length: usize) -> Result<Vec<u8>, DeviceError> {
        let mut buf = vec![0u8; length.max(1)];
        buf[0] = report_id;
        let n = self.0.get_feature_report(&mut buf).map_err(hid_error)?;
        buf.truncate(n);
        Ok(buf)
    }

    fn send_feature(&mut self, data: &[u8]) -> Result<(), DeviceError> {
        self.0.send_feature_report(data).map_err(hid_error)
    }
}

struct SerialPort {
    port: Box<dyn serialport::SerialPort>,
    timeout: Duration,
}

impl Port for SerialPort {
    fn read(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, DeviceError> {
        if timeout != self.timeout {
            self.port.set_timeout(timeout).map_err(serial_error)?;
            self.timeout = timeout;
        }
        let mut buf = vec![0u8; SERIAL_BUFFER];
        match self.port.read(&mut buf) {
            // A tty reads 0 bytes once the far end has gone.
            Ok(0) => Err(DeviceError::Unplugged),
            Ok(n) => {
                buf.truncate(n);
                Ok(Some(buf))
            }
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => Ok(None),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => Ok(None),
            Err(e) => Err(DeviceError::from_io(&e)),
        }
    }

    fn write(&mut self, data: &[u8]) -> Result<(), DeviceError> {
        self.port
            .write_all(data)
            .map_err(|e| DeviceError::from_io(&e))
    }
}

fn hid_error(e: HidError) -> DeviceError {
    match e {
        HidError::IoError { error } => DeviceError::from_io(&error),
        other => DeviceError::from_message(&other.to_string()),
    }
}

fn serial_error(e: serialport::Error) -> DeviceError {
    match e.kind {
        serialport::ErrorKind::Io(kind) => {
            DeviceError::from_io(&std::io::Error::new(kind, e.description))
        }
        serialport::ErrorKind::NoDevice => DeviceError::from_message(&e.description).or_unplugged(),
        _ => DeviceError::from_message(&e.description),
    }
}

impl DeviceError {
    /// serialport's `NoDevice` is either busy or gone; its text says which,
    /// when it says anything.
    fn or_unplugged(self) -> DeviceError {
        match self {
            DeviceError::Io(_) => DeviceError::Unplugged,
            other => other,
        }
    }
}

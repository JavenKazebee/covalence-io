//! What the device layer needs from the OS, so tests can stand in for it.

use std::time::Duration;

use covalence_protocol::io::DeviceInfo;

use crate::DeviceError;

/// How to open a device.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenOptions {
    /// `serial` only. Defaults to 115200.
    pub baud: Option<u32>,
}

pub const DEFAULT_BAUD: u32 = 115_200;

/// Enumerates, watches and opens devices. Every method may block briefly.
pub trait Backend: Send + Sync + 'static {
    /// Every device present now. `id` is left empty; the device layer
    /// assigns it.
    fn enumerate(&self) -> Result<Vec<DeviceInfo>, DeviceError>;

    /// Opens a device found by [`Backend::enumerate`].
    fn open(
        &self,
        device: &DeviceInfo,
        options: &OpenOptions,
    ) -> Result<Box<dyn Port>, DeviceError>;

    /// Calls `changed` whenever devices may have come or gone, until the
    /// returned guard is dropped. `None` if this backend can't tell, in
    /// which case the device layer only polls.
    fn watch(&self, changed: Box<dyn Fn() + Send + Sync>) -> Option<Box<dyn Send>>;
}

/// An open device. One thread owns it, so it needn't be `Sync`.
pub trait Port: Send {
    /// Waits up to `timeout` for an input report or a chunk of serial bytes.
    fn read(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, DeviceError>;

    /// An output report, its ID first; or bytes for serial.
    fn write(&mut self, data: &[u8]) -> Result<(), DeviceError>;

    /// HID only: reads feature report `report_id`, up to `length` bytes with
    /// the ID first.
    fn get_feature(&mut self, report_id: u8, length: usize) -> Result<Vec<u8>, DeviceError> {
        let _ = (report_id, length);
        Err(DeviceError::Unsupported("feature reports on this".into()))
    }

    /// HID only: sends a feature report, its ID first.
    fn send_feature(&mut self, data: &[u8]) -> Result<(), DeviceError> {
        let _ = data;
        Err(DeviceError::Unsupported("feature reports on this".into()))
    }
}

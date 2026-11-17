//! The core's device layer (plugin protocol §7.1, ADR 0025).
//!
//! It enumerates HID and serial devices, watches for hotplug, gives each
//! device a stable ID, matches devices to plugins' `devices`, and performs
//! their I/O for the connections that hold them. It knows nothing about any
//! device's protocol: that is the plugin's.
//!
//! - [`Devices`] is the layer: what's present, and the handles connections
//!   hold. One connection holds a device at a time; unplug and replug close
//!   and reopen its handle; and a handle stays open while its holder's plugin
//!   host restarts, dropping input until the holder opens it again.
//! - [`matching`] decides which plugins a device is for, and whether to
//!   create a connection for it without asking.
//! - [`Backend`] is what the layer needs from the OS. [`system::System`] is
//!   the real one; [`fake::Fake`] stands in for it in tests.

mod backend;
mod devices;
mod error;
pub mod fake;
pub mod id;
pub mod matching;
#[cfg(feature = "system")]
pub mod system;

pub use backend::{Backend, DEFAULT_BAUD, OpenOptions, Port};
pub use covalence_protocol::io::{DeviceInfo, DeviceKind, DeviceMatch, IoStateKind};
pub use devices::{
    DeviceEvent, Devices, HANDLE_BUFFER, HandleEvent, HandleInfo, Opened, POLL, READ_POLL, SETTLE,
    Settings,
};
pub use error::{DeviceError, ErrorReport};

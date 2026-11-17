//! Device errors, with codes the protocol carries and hints the UI shows
//! (plugin protocol §7.1).

use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DeviceError {
    /// No device with that ID is plugged in.
    #[error("no device {0}")]
    NotFound(String),
    /// Another connection holds the device.
    #[error("device {device} is held by connection {holder}")]
    Busy { device: String, holder: String },
    /// The device went away while it was open.
    #[error("the device was unplugged")]
    Unplugged,
    /// The OS refused access: on Linux, usually missing udev rules.
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    /// Something else holds the device exclusively: macOS keyboards and
    /// mice, or another app.
    #[error("exclusive access refused: {0}")]
    ExclusiveAccess(String),
    /// No driver lets the core open it: on Windows, a device that needs WinUSB.
    #[error("no driver: {0}")]
    NoDriver(String),
    /// This build can't open that kind of device.
    #[error("{0} devices are not supported")]
    Unsupported(String),
    /// The handle is unknown or isn't open now.
    #[error("handle {0} is not open")]
    NotOpen(String),
    #[error("{0}")]
    Io(String),
}

impl DeviceError {
    /// The code `io.state` and the API carry.
    pub fn code(&self) -> &'static str {
        match self {
            DeviceError::NotFound(_) => "not_found",
            DeviceError::Busy { .. } => "busy",
            DeviceError::Unplugged => "unplugged",
            DeviceError::PermissionDenied(_) => "permission_denied",
            DeviceError::ExclusiveAccess(_) => "exclusive_access",
            DeviceError::NoDriver(_) => "no_driver",
            DeviceError::Unsupported(_) => "unsupported",
            DeviceError::NotOpen(_) => "not_open",
            DeviceError::Io(_) => "io",
        }
    }

    /// What the user can do about it, on this platform, where there's
    /// something to say.
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            DeviceError::PermissionDenied(_) if cfg!(target_os = "linux") => Some(
                "Linux needs a udev rule to let Covalence open this device: add one for its vendor and product ID under /etc/udev/rules.d, then unplug it and plug it back in.",
            ),
            DeviceError::PermissionDenied(_) if cfg!(target_os = "macos") => Some(
                "macOS asks before an app reads input devices: allow Covalence under System Settings → Privacy & Security → Input Monitoring.",
            ),
            DeviceError::PermissionDenied(_) => {
                Some("The operating system refused access to this device.")
            }
            DeviceError::ExclusiveAccess(_) if cfg!(target_os = "macos") => Some(
                "Another app has this device open, or macOS keeps it for itself, as it does keyboards and mice. Quit apps that use it, such as the Stream Deck app.",
            ),
            DeviceError::ExclusiveAccess(_) => Some(
                "Another app has this device open. Quit apps that use it, such as the Stream Deck app.",
            ),
            DeviceError::NoDriver(_) if cfg!(target_os = "windows") => Some(
                "Windows has no driver that lets Covalence open this device. It may need WinUSB, which Zadig can install.",
            ),
            DeviceError::NoDriver(_) => Some("No driver lets Covalence open this device."),
            DeviceError::Unplugged => Some("Plug the device back in; it reopens by itself."),
            _ => None,
        }
    }

    /// Classifies an OS error from opening or using a device.
    pub fn from_io(error: &std::io::Error) -> DeviceError {
        match error.kind() {
            std::io::ErrorKind::PermissionDenied => {
                DeviceError::PermissionDenied(error.to_string())
            }
            std::io::ErrorKind::NotFound => DeviceError::Unplugged,
            std::io::ErrorKind::ResourceBusy => DeviceError::ExclusiveAccess(error.to_string()),
            _ => DeviceError::from_message(&error.to_string()),
        }
    }

    /// Classifies an error that only came as text, as hidapi's often do.
    pub fn from_message(message: &str) -> DeviceError {
        let lower = message.to_ascii_lowercase();
        let has = |needles: &[&str]| needles.iter().any(|n| lower.contains(n));
        // Before permissions: "exclusiveaccess" contains "eacces".
        if has(&[
            "exclusive",
            "kioreturnexclusiveaccess",
            "resource busy",
            "device or resource busy",
            "os error 16",
            "sharing violation",
        ]) {
            DeviceError::ExclusiveAccess(message.to_owned())
        } else if has(&[
            "permission denied",
            "os error 13",
            "eacces",
            "access is denied",
            "not permitted",
            "kioreturnnotpermitted",
            "kioreturnnotprivileged",
        ]) {
            DeviceError::PermissionDenied(message.to_owned())
        } else if has(&[
            "no such device",
            "no such file",
            "os error 19)",
            "kioreturnnodevice",
            "not connected",
            "disconnected",
        ]) {
            DeviceError::Unplugged
        } else if has(&["no driver", "driver not", "winusb"]) {
            DeviceError::NoDriver(message.to_owned())
        } else {
            DeviceError::Io(message.to_owned())
        }
    }

    /// How the API shows it: `{code, message, hint?}`.
    pub fn report(&self) -> ErrorReport {
        ErrorReport {
            code: self.code(),
            message: self.to_string(),
            hint: self.hint(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ErrorReport {
    pub code: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<&'static str>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_os_errors() {
        let denied = std::io::Error::from_raw_os_error(13);
        assert_eq!(DeviceError::from_io(&denied).code(), "permission_denied");
        assert!(DeviceError::from_io(&denied).hint().is_some());
        assert_eq!(
            DeviceError::from_message("hid_open_path: kIOReturnExclusiveAccess").code(),
            "exclusive_access"
        );
        assert_eq!(
            DeviceError::from_message("Failed to open /dev/hidraw3: No such device (os error 19)")
                .code(),
            "unplugged"
        );
        assert_eq!(DeviceError::from_message("weird").code(), "io");
    }
}

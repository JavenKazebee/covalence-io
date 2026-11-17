//! Stable device IDs (plugin protocol §7.1).
//!
//! An ID is built from the kind, vendor and product IDs and the serial number
//! where the device has one, so the same device gets the same ID on any port
//! and after a restart: `hid:0fd9:0080:AL12K1A01234`. A device without a
//! serial number is named by its path instead, which holds only while it
//! stays in the same port: `serial@/dev/ttyUSB0`.
//!
//! A device with several HID interfaces lists each under the same vendor,
//! product and serial. Those IDs gain the interface's usage page and usage,
//! `…:AL12K1A01234/ff00.0001`, and its path if that still isn't unique.

use std::collections::BTreeMap;

use covalence_protocol::io::{DeviceInfo, DeviceKind};

pub(crate) fn kind_name(kind: DeviceKind) -> &'static str {
    match kind {
        DeviceKind::Hid => "hid",
        DeviceKind::Serial => "serial",
        DeviceKind::Midi => "midi",
    }
}

/// The ID before any disambiguation.
pub fn base_id(device: &DeviceInfo) -> String {
    let kind = kind_name(device.kind);
    let serial = device
        .serial
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    match (device.vendor_id, device.product_id, serial) {
        (Some(vendor), Some(product), Some(serial)) => {
            format!("{kind}:{vendor:04x}:{product:04x}:{serial}")
        }
        _ => format!("{kind}@{}", device.path),
    }
}

/// Fills in `id` on each device, unique within the list.
pub fn assign_ids(devices: &mut [DeviceInfo]) {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for device in devices.iter_mut() {
        device.id = base_id(device);
        *counts.entry(device.id.clone()).or_default() += 1;
    }
    let mut widened: BTreeMap<String, usize> = BTreeMap::new();
    for device in devices.iter_mut() {
        if counts[&device.id] > 1 {
            device.id = format!(
                "{}/{:04x}.{:04x}",
                device.id,
                device.usage_page.unwrap_or(0),
                device.usage.unwrap_or(0)
            );
        }
        *widened.entry(device.id.clone()).or_default() += 1;
    }
    for device in devices.iter_mut() {
        if widened[&device.id] > 1 {
            device.id = format!("{}@{}", device.id, device.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hid(serial: Option<&str>, path: &str, usage_page: u16) -> DeviceInfo {
        DeviceInfo {
            id: String::new(),
            kind: DeviceKind::Hid,
            vendor_id: Some(0x0fd9),
            product_id: Some(0x0080),
            serial: serial.map(Into::into),
            manufacturer: None,
            product: Some("Stream Deck MK.2".into()),
            path: path.into(),
            usage_page: Some(usage_page),
            usage: Some(1),
        }
    }

    #[test]
    fn ids_use_the_serial_number_and_fall_back_to_the_path() {
        let mut devices = vec![
            hid(Some("AL12"), "/dev/hidraw0", 0xff00),
            hid(None, "/dev/hidraw1", 0xff00),
            hid(Some("  "), "/dev/hidraw2", 0xff00),
        ];
        assign_ids(&mut devices);
        let ids: Vec<&str> = devices.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(
            ids,
            ["hid:0fd9:0080:AL12", "hid@/dev/hidraw1", "hid@/dev/hidraw2"]
        );
    }

    #[test]
    fn interfaces_of_one_device_get_their_own_ids() {
        let mut devices = vec![
            hid(Some("AL12"), "/dev/hidraw0", 0xff00),
            hid(Some("AL12"), "/dev/hidraw1", 0x000c),
            hid(Some("AL12"), "/dev/hidraw2", 0x000c),
        ];
        assign_ids(&mut devices);
        let ids: Vec<&str> = devices.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "hid:0fd9:0080:AL12/ff00.0001",
                "hid:0fd9:0080:AL12/000c.0001@/dev/hidraw1",
                "hid:0fd9:0080:AL12/000c.0001@/dev/hidraw2",
            ]
        );
    }
}

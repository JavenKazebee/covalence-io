//! Host transports and devices: I/O the core performs for a connection
//! (spec §7, §7.1, ADR 0025). The connection is the envelope's `conn`.

use serde::{Deserialize, Serialize};

use crate::Bytes;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IoKind {
    Tcp,
    Udp,
    Ws,
    Serial,
    Hid,
    Midi,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Framing {
    Raw,
    Line,
    Length,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Backoff {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_ms: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IoOptions {
    /// Reopen after the far end closes or the device returns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reconnect: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backoff: Option<Backoff>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub framing: Option<Framing>,
    /// `serial` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baud: Option<u32>,
}

/// `io.open` params (host → core, req).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IoOpen {
    pub kind: IoKind,
    /// `host:port` for `tcp`; a device ID (§7.1) for `serial`, `hid` and `midi`.
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<IoOptions>,
}

/// `io.open` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IoOpened {
    pub handle: String,
}

/// `io.write` params (host → core, ntf). For `hid`, an output report whose
/// first byte is the report ID.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IoWrite {
    pub handle: String,
    pub data: Bytes,
}

/// `io.data` params (core → host, ntf). For `hid`, an input report.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IoData {
    pub handle: String,
    pub data: Bytes,
    /// When the core read it: milliseconds since the Unix epoch, on the
    /// core's clock. A surface's input `ts` (spec §8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FeatureOp {
    Get,
    Send,
}

/// `io.feature` params (host → core, req), `hid` only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IoFeature {
    pub handle: String,
    pub op: FeatureOp,
    /// `get`: which report, and how many bytes to read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_id: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<u32>,
    /// `send`: the report, its ID first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Bytes>,
}

/// `io.feature` result: `get` gives `data`; `send` gives nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IoFeatureResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Bytes>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IoStateKind {
    Open,
    Closed,
    Reconnecting,
    Error,
}

/// `io.state` params (core → host, ntf). A device handle closes with
/// `error: "unplugged"`, and opens again when the same device returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IoState {
    pub handle: String,
    pub state: IoStateKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `io.close` params (host → core, ntf).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IoClose {
    pub handle: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceKind {
    Hid,
    Serial,
    Midi,
}

/// One product ID, or several.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ProductIds {
    One(u16),
    Many(Vec<u16>),
}

impl ProductIds {
    pub fn contains(&self, id: u16) -> bool {
        match self {
            ProductIds::One(p) => *p == id,
            ProductIds::Many(ps) => ps.contains(&id),
        }
    }
}

/// A manifest's `devices` entry: which devices the plugin drives (§7.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceMatch {
    pub kind: DeviceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor_id: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product_id: Option<ProductIds>,
    /// `hid` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_page: Option<u16>,
    /// A glob over the product or MIDI port name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Create a connection without asking (surfaces).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_create: Option<bool>,
}

/// A device the core found (§7.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    /// Stable: from kind, vendor, product and serial number where there is
    /// one, else the port path.
    pub id: String,
    pub kind: DeviceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor_id: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product_id: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product: Option<String>,
    /// OS path or port name, for display.
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_page: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<u16>,
}

/// `device.list` params (host → core, req): devices this plugin's `devices`
/// match, for a picker. Result: `Vec<DeviceInfo>`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceList {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<DeviceKind>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Value, from_value, to_value};

    #[test]
    fn bytes_are_bin_in_messagepack_and_read_from_either_form() {
        let write = IoWrite {
            handle: "h".into(),
            data: Bytes(vec![2, 7, 255]),
        };
        let value = to_value(&write).unwrap();
        let Value::Map(fields) = &value else {
            panic!("{value:?}")
        };
        assert!(
            fields
                .iter()
                .any(|(k, v)| k.as_str() == Some("data") && *v == Value::Binary(vec![2, 7, 255]))
        );
        // As JSON writes them: an array of byte values.
        let from_json: IoWrite =
            serde_json::from_value(serde_json::json!({ "handle": "h", "data": [2, 7, 255] }))
                .unwrap();
        assert_eq!(from_json, write);
        assert_eq!(from_value::<IoWrite>(&value).unwrap(), write);
        assert_eq!(
            serde_json::to_value(&write).unwrap()["data"],
            serde_json::json!([2, 7, 255])
        );
    }
}

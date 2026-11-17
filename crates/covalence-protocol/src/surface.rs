//! Surfaces: two-way devices (spec §8, ADR 0024, ADR 0025). From the plugin
//! host, the envelope's `conn` names the connection that provides the
//! surface, and `surface` is the host's own key for it.

use serde::{Deserialize, Serialize};

use crate::Bytes;

/// Where a control is, in grid cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placement {
    pub x: u32,
    pub y: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub w: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub h: Option<u32>,
}

/// What a control can send.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum InputCap {
    Momentary {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        velocity: Option<bool>,
    },
    Relative {},
    Absolute {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        axes: Option<u8>,
    },
    Touch {},
}

/// What a control can show.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum OutputCap {
    Image {
        w: u32,
        h: u32,
        format: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rotation: Option<u16>,
    },
    Led {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rgb: Option<bool>,
    },
    Level {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        motor: Option<bool>,
    },
    Text {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        chars: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lines: Option<u32>,
    },
}

/// One control on a surface.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlDef {
    /// Unique on the surface: `k0`, `dial1`, `pad.36`.
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub at: Placement,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<InputCap>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<OutputCap>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceOutputs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brightness: Option<bool>,
}

/// `surface.attach` params (host → core, ntf).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceAttach {
    pub surface: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    pub name: String,
    pub controls: Vec<ControlDef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outputs: Option<SurfaceOutputs>,
}

/// `surface.detach` params (host → core, ntf).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceDetach {
    pub surface: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InputKind {
    Down,
    Up,
    Turn,
    Touch,
    Value,
}

/// An input's `value`: one number, or `[x, y]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum InputValue {
    One(f64),
    Many(Vec<f64>),
}

/// `surface.input` params (host → core, ntf).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SurfaceInput {
    pub surface: String,
    pub control: String,
    pub kind: InputKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<InputValue>,
    /// The host's timestamp, in milliseconds on its own clock.
    pub ts: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub echo: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    Jpeg,
    Bmp,
    Png,
    Rgb565,
    /// Raw 8-bit RGB, rows top to bottom, for devices whose driver packs
    /// images itself.
    Rgb,
}

/// `surface.draw` params (core → host, ntf), for `image` outputs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceDraw {
    pub surface: String,
    pub control: String,
    pub image: Bytes,
    pub format: ImageFormat,
}

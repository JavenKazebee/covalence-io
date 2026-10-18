//! Plugin protocol types and framing.
//!
//! The schema in `protocol/` is the source of truth. These types are written by
//! hand to mirror it (ADR 0016); the golden vectors in `protocol/fixtures/` keep
//! the two in step.

pub mod action;
pub mod connection;
pub mod envelope;
pub mod framing;
pub mod methods;
pub mod plugin;
pub mod session;
pub mod state;
mod value;

pub use envelope::{ErrorCode, Message, Notification, ProtocolError, Request, Response};
pub use value::{Empty, Object, True, Value, ValueError, from_value, to_value};

/// Protocol version this build speaks.
pub const PROTOCOL_VERSION: &str = "0.1.0";

//! Method tables: which methods exist, in which direction, with which params
//! and results.
//!
//! Each direction has a module. In it, every method has a marker type
//! implementing [`Method`] (requests) or [`NotificationMethod`], for typed
//! outgoing calls, and `Request` / `Notification` enums decode incoming ones.

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::{Empty, Value, ValueError, from_value, to_value};

/// A request method.
pub trait Method {
    const NAME: &'static str;
    type Params: Serialize + DeserializeOwned;
    type Result: Serialize + DeserializeOwned;
}

/// A notification method.
pub trait NotificationMethod {
    const NAME: &'static str;
    type Params: Serialize + DeserializeOwned;
}

#[derive(Debug, thiserror::Error)]
pub enum MethodError {
    #[error("unknown method `{0}`")]
    Unknown(String),
    #[error("invalid params for `{method}`: {source}")]
    InvalidParams {
        method: &'static str,
        #[source]
        source: ValueError,
    },
}

macro_rules! requests {
    ($(#[$doc:meta])* $enum:ident { $($name:literal => $variant:ident($params:ty) -> $result:ty,)* }) => {
        $(
            #[doc = concat!("The `", $name, "` request.")]
            pub struct $variant;

            impl Method for $variant {
                const NAME: &'static str = $name;
                type Params = $params;
                type Result = $result;
            }
        )*

        $(#[$doc])*
        #[derive(Clone, Debug, PartialEq)]
        pub enum $enum {
            $($variant($params),)*
        }

        dispatch!($enum { $($name => $variant,)* });
    };
}

macro_rules! notifications {
    ($(#[$doc:meta])* $enum:ident { $($name:literal => $variant:ident($params:ty),)* }) => {
        $(
            #[doc = concat!("The `", $name, "` notification.")]
            pub struct $variant;

            impl NotificationMethod for $variant {
                const NAME: &'static str = $name;
                type Params = $params;
            }
        )*

        $(#[$doc])*
        #[derive(Clone, Debug, PartialEq)]
        pub enum $enum {
            $($variant($params),)*
        }

        dispatch!($enum { $($name => $variant,)* });
    };
}

macro_rules! dispatch {
    ($enum:ident { $($name:literal => $variant:ident,)* }) => {
        impl $enum {
            /// Decodes params for method `m`.
            pub fn decode(m: &str, p: &Value) -> Result<Self, MethodError> {
                match m {
                    $($name => from_value(p)
                        .map(Self::$variant)
                        .map_err(|source| MethodError::InvalidParams { method: $name, source }),)*
                    other => Err(MethodError::Unknown(other.to_owned())),
                }
            }

            pub fn method(&self) -> &'static str {
                match self {
                    $(Self::$variant(_) => $name,)*
                }
            }

            /// Encodes the params for sending.
            pub fn params(&self) -> Result<Value, ValueError> {
                match self {
                    $(Self::$variant(p) => to_value(p),)*
                }
            }
        }
    };
}

/// Messages the host sends to the core.
pub mod host_to_core {
    use super::*;
    use crate::{connection, session, state};

    requests! {
        /// A request from a host, decoded.
        Request {
            "host.hello" => HostHello(session::HostHello) -> session::HostWelcome,
            "ping" => Ping(Empty) -> Empty,
        }
    }

    notifications! {
        /// A notification from a host, decoded.
        Notification {
            "conn.status" => ConnStatus(connection::ConnStatus),
            "state.snapshot" => StateSnapshot(state::StateSnapshot),
            "state.patch" => StatePatch(state::StatePatch),
            "event.emit" => EventEmit(state::EventEmit),
            "log" => Log(state::Log),
        }
    }
}

/// Messages the core sends to a host.
pub mod core_to_host {
    use super::*;
    use crate::{action, connection, plugin};

    requests! {
        /// A request from the core, decoded.
        Request {
            "plugin.load" => PluginLoad(plugin::PluginLoad) -> plugin::PluginLoaded,
            "conn.create" => ConnCreate(connection::ConnCreate) -> Empty,
            "conn.destroy" => ConnDestroy(connection::ConnDestroy) -> Empty,
            "conn.resync" => ConnResync(Empty) -> Empty,
            "action.invoke" => ActionInvoke(action::ActionInvoke) -> action::ActionResult,
            "ping" => Ping(Empty) -> Empty,
        }
    }
}

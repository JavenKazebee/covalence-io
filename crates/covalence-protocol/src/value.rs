use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// An untyped protocol value. MessagePack's data model, so binary survives.
pub type Value = rmpv::Value;

/// A JSON-style object with arbitrary values (`Record<unknown>` in the schema).
pub type Object = BTreeMap<String, Value>;

/// Params or result of a method that carries none.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Empty {}

/// The literal `true`, for fields the schema fixes to it (`complete: true`, `del: true`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct True;

impl Serialize for True {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bool(true)
    }
}

impl<'de> Deserialize<'de> for True {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match bool::deserialize(deserializer)? {
            true => Ok(True),
            false => Err(serde::de::Error::custom("expected `true`")),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ValueError {
    #[error(transparent)]
    Encode(#[from] rmp_serde::encode::Error),
    #[error(transparent)]
    Decode(#[from] rmp_serde::decode::Error),
    #[error(transparent)]
    Read(#[from] rmpv::decode::Error),
}

// Both conversions go through MessagePack bytes so that structs become maps
// and enums become strings, exactly as on the wire.

/// Converts a typed value into an untyped [`Value`].
pub fn to_value<T: Serialize>(value: &T) -> Result<Value, ValueError> {
    let bytes = rmp_serde::to_vec_named(value)?;
    Ok(rmpv::decode::read_value(&mut bytes.as_slice())?)
}

/// Converts an untyped [`Value`] into a typed value.
pub fn from_value<T: DeserializeOwned>(value: &Value) -> Result<T, ValueError> {
    let mut bytes = Vec::new();
    rmpv::encode::write_value(&mut bytes, value).expect("writing to a Vec cannot fail");
    Ok(rmp_serde::from_slice(&bytes)?)
}

/// Raw bytes (spec §7): `bin` in MessagePack, an array of byte values in
/// JSON. Reads either.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Bytes(pub Vec<u8>);

impl std::fmt::Debug for Bytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Bytes({} bytes)", self.0.len())
    }
}

impl From<Vec<u8>> for Bytes {
    fn from(bytes: Vec<u8>) -> Self {
        Bytes(bytes)
    }
}

impl Serialize for Bytes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(&self.0)
    }
}

impl<'de> Deserialize<'de> for Bytes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = Bytes;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("bytes, or an array of byte values")
            }

            fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<Bytes, E> {
                Ok(Bytes(v.to_vec()))
            }

            fn visit_byte_buf<E: serde::de::Error>(self, v: Vec<u8>) -> Result<Bytes, E> {
                Ok(Bytes(v))
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Bytes, A::Error> {
                let mut out = Vec::with_capacity(seq.size_hint().unwrap_or(0));
                while let Some(b) = seq.next_element::<u8>()? {
                    out.push(b);
                }
                Ok(Bytes(out))
            }
        }

        deserializer.deserialize_any(Visitor)
    }
}

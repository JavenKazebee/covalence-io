use std::collections::HashMap;
use std::fmt;

#[derive(Clone, Debug)]
pub enum DataType {
    Any,
    Bool,
    Float,
    String,
    List,
    Object,
    Trigger,
}

#[derive(Clone, Debug)]
pub enum Value {
    Null,
    Bool(bool),
    Float(f64),
    String(String),
    List(Vec<Value>),
    Object(std::collections::HashMap<String, Value>),
    Type(DataType),
}

impl Value {
    pub fn get_type(&self) -> DataType {
        match self {
            Value::Null => DataType::Any,
            Value::Bool(_) => DataType::Bool,
            Value::Float(_) => DataType::Float,
            Value::String(_) => DataType::String,
            Value::List(_) => DataType::List,
            Value::Object(_) => DataType::Object,
            Value::Type(_) => DataType::Any,
        }
    }

    pub fn as_datatype(&self) -> Option<DataType> {
        match self {
            Value::Type(t) => Some(t.clone()),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConversionError;

impl fmt::Display for ConversionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Conversion error")
    }
}

impl std::error::Error for ConversionError {}

impl TryFrom<&Value> for f64 {
    type Error = ConversionError;

    fn try_from(value: &Value) -> Result<Self, Self::Error> {
        match value {
            Value::Float(x) => Ok(*x),
            _ => Err(ConversionError),
        }
    }
}

impl TryFrom<&Value> for bool {
    type Error = ConversionError;

    fn try_from(value: &Value) -> Result<Self, Self::Error> {
        match value {
            Value::Bool(b) => Ok(*b),
            _ => Err(ConversionError),
        }
    }
}

impl TryFrom<&Value> for String {
    type Error = ConversionError;

    fn try_from(value: &Value) -> Result<Self, Self::Error> {
        match value {
            Value::String(s) => Ok(s.clone()),
            _ => Err(ConversionError),
        }
    }
}

impl TryFrom<&Value> for Vec<Value> {
    type Error = ConversionError;

    fn try_from(value: &Value) -> Result<Self, Self::Error> {
        match value {
            Value::List(l) => Ok(l.clone()),
            _ => Err(ConversionError),
        }
    }
}

impl TryFrom<&Value> for HashMap<String, Value> {
    type Error = ConversionError;

    fn try_from(value: &Value) -> Result<Self, Self::Error> {
        match value {
            Value::Object(m) => Ok(m.clone()),
            _ => Err(ConversionError),
        }
    }
}

impl TryFrom<&Value> for DataType {
    type Error = ConversionError;

    fn try_from(value: &Value) -> Result<Self, Self::Error> {
        match value {
            Value::Type(t) => Ok(t.clone()),
            _ => Err(ConversionError),
        }
    }
}

impl TryFrom<&Value> for () {
    type Error = ConversionError;

    fn try_from(value: &Value) -> Result<Self, Self::Error> {
        match value {
            Value::Null => Ok(()),
            _ => Err(ConversionError),
        }
    }
}

impl TryFrom<Value> for String {
    type Error = ConversionError;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::String(s) => Ok(s),
            _ => Err(ConversionError),
        }
    }
}

impl TryFrom<Value> for Vec<Value> {
    type Error = ConversionError;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::List(l) => Ok(l),
            _ => Err(ConversionError),
        }
    }
}

impl TryFrom<Value> for HashMap<String, Value> {
    type Error = ConversionError;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::Object(m) => Ok(m),
            _ => Err(ConversionError),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Message {
    pub seq: u64,
    pub source: String,
    pub payload: Event,
}

#[derive(Clone, Debug)]
pub enum Event {
    Signal {
        id: String,
        value: Value,
    },
    Command {
        target: String,
        name: String,
        params: HashMap<String, Value>,
    },
}

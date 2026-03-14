use std::collections::HashMap;

#[derive(Clone, Debug)]
pub enum Value {
    Null,
    Bool(bool),
    Float(f64),
    String(String),
    List(Vec<Value>),
    Object(std::collections::HashMap<String, Value>),
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
        }
    }
}

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

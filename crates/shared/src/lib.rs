use std::collections::HashMap;

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

    pub fn as_float(&self) -> Option<f64> {
        match self {
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_string(&self) -> Option<String> {
        match self {
            Value::String(s) => Some(s.clone()),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<Vec<Value>> {
        match self {
            Value::List(l) => Some(l.clone()),
            _ => None,
        }
    }
    
    
    
}

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

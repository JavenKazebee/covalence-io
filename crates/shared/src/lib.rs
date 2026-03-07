#[derive(Clone, Debug)]
pub enum Value {
    Bool(bool),
    Float(f64),
    String(String),
    List(Vec<Value>),
    Map(std::collections::HashMap<String, Value>),
}

#[derive(Clone, Debug)]
pub struct Message {
    pub seq: u64,
    pub source: String,
    pub payload: Event,
}

#[derive(Clone, Debug)]
pub enum Event {
    Input { source: String, id: String, value: Value },
    Action { target: String, command: String, params: Option<Value> },
}

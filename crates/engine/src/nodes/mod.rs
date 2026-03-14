use std::collections::HashMap;

use shared::{DataType, Value};
use uuid::Uuid;

pub mod registry;
pub mod manager;

pub struct Pin {
    pub name: &'static str,
    pub data_type: DataType,
}

pub struct Edge {
    pub from_pin: String,
    pub to_pin: String,
}

pub trait Node {
    fn id(&self) -> &'static str;
    fn inputs(&self) -> Vec<Pin>;
    fn outputs(&self) -> Vec<Pin>;
    fn execute(
        &self,
        inputs: &HashMap<String, Value>,
        config: &HashMap<String, Value>,
    ) -> HashMap<String, Value>;
}

pub struct NodeInstance {
    pub id: Uuid,
    pub node_type: String,
    pub config: HashMap<String, Value>,
    pub last_outputs: HashMap<String, Value>,    
}
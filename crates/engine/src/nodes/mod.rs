use std::collections::HashMap;

use shared::{DataType, Message, Value};
use tokio::sync::broadcast;
use uuid::Uuid;

pub mod impls;
pub mod manager;
pub mod registry;

pub struct Pin {
    pub name: &'static str,
    pub data_type: DataType,
}

pub struct Edge {
    pub from_pin: String,
    pub to_pin: String,
}

pub trait Node: Send + Sync {
    fn id(&self) -> &'static str;
    fn inputs(&self, instance: &NodeInstance) -> Vec<Pin>;
    fn outputs(&self, instance: &NodeInstance) -> Vec<Pin>;
    fn execute(
        &self,
        inputs: &HashMap<String, Value>,
        tx: &broadcast::Sender<Message>,
    ) -> NodeExecutionResult;

}

pub struct NodeInstance {
    pub id: Uuid,
    pub node_type: String,
    /// Default values for input pins. Connected pins override these at execution time.
    pub defaults: HashMap<String, Value>,
    pub last_outputs: HashMap<String, Value>,
    pub last_error: Option<String>,
}

pub struct NodeExecutionResult {
    pub outputs: HashMap<String, Value>,
    pub error: Option<String>,
}

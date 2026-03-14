use std::collections::HashMap;

use crate::nodes::Node;

pub struct NodeRegistry {
    nodes: HashMap<String, Box<dyn Node>>,
}

impl NodeRegistry {
    pub fn new() -> Self {
        Self { nodes: HashMap::new() }
    }

    pub fn register<T: Node + 'static>(&mut self, node: T) {
        self.nodes.insert(node.id().to_string(), Box::new(node));
    }

    pub fn get(&self, id: &str) -> Option<&dyn Node> {
        self.nodes.get(id).map(|node| node.as_ref())
    }
}
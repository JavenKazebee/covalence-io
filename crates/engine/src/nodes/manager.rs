use std::collections::HashMap;

use petgraph::stable_graph::{NodeIndex, StableGraph};
use uuid::Uuid;

use crate::nodes::{Edge, NodeInstance, registry::NodeRegistry};

pub struct NodeManager {
    pub graph: StableGraph<NodeInstance, Edge>,
    pub registry: NodeRegistry,
    index_map: HashMap<Uuid, NodeIndex>,
}

impl NodeManager {
    pub fn new(registry: NodeRegistry) -> Self {
        Self {
            graph: StableGraph::new(),
            registry,
            index_map: HashMap::new(),
        }
    }

    pub fn add_node() {

    }

    pub fn connect() {

    }

    pub fn run_from() {

    }

    pub fn run_all() {

    }

    fn execute_node() {
        
    }
}
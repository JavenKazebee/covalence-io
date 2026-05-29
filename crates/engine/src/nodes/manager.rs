use std::collections::{HashMap, HashSet};

use petgraph::{
    Direction,
    stable_graph::{NodeIndex, StableGraph},
    visit::{Bfs, Topo},
};
use shared::{Message, Value};
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::nodes::{Edge, NodeInstance, registry::NodeRegistry};

pub struct NodeManager {
    pub graph: StableGraph<NodeInstance, Edge>,
    pub registry: NodeRegistry,
    index_map: HashMap<Uuid, NodeIndex>,
    tx: broadcast::Sender<Message>,
}

impl NodeManager {
    pub fn new(registry: NodeRegistry, tx: broadcast::Sender<Message>) -> Self {
        Self {
            graph: StableGraph::new(),
            registry,
            index_map: HashMap::new(),
            tx,
        }
    }

    pub fn add_node(&mut self, node_type: &str, defaults: HashMap<String, Value>) -> Option<Uuid> {
        let id = Uuid::new_v4();
        let instance = NodeInstance {
            id,
            node_type: node_type.to_string(),
            defaults,
            last_outputs: HashMap::new(),
            last_error: None,
        };
        let index = self.graph.add_node(instance);
        self.index_map.insert(id, index);
        Some(id)
    }

    pub fn connect(&mut self, from_node: Uuid, from_pin: &str, to_node: Uuid, to_pin: &str) {
        if let (Some(&from_index), Some(&to_index)) =
            (self.index_map.get(&from_node), self.index_map.get(&to_node))
        {
            self.graph.add_edge(
                from_index,
                to_index,
                Edge {
                    from_pin: from_pin.to_string(),
                    to_pin: to_pin.to_string(),
                },
            );
        }
    }

    pub fn run_from(&mut self, node: Uuid) {
        let Some(&start_index) = self.index_map.get(&node) else {
            return;
        };

        let mut affected_indices = HashSet::new();
        let mut bfs = Bfs::new(&self.graph, start_index);
        while let Some(node_index) = bfs.next(&self.graph) {
            affected_indices.insert(node_index);
        }

        let mut topo = Topo::new(&self.graph);
        while let Some(node_index) = topo.next(&self.graph) {
            if affected_indices.contains(&node_index) {
                self.execute_node(node_index);
            }
        }
    }

    pub fn run_all(&mut self) {
        let mut topo = Topo::new(&self.graph);
        while let Some(node_index) = topo.next(&self.graph) {
            self.execute_node(node_index);
        }
    }

    fn execute_node(&mut self, index: NodeIndex) {
        // 1. Collect inputs from upstream neighbors
        let mut inputs = HashMap::new();
        let mut edges = self
            .graph
            .neighbors_directed(index, Direction::Incoming)
            .detach();

        while let Some((edge_index, source_index)) = edges.next(&self.graph) {
            let edge = &self.graph[edge_index];
            let source = &self.graph[source_index];

            if let Some(source_output) = source.last_outputs.get(&edge.from_pin) {
                inputs.insert(edge.to_pin.clone(), source_output.clone());
            }
        }

        // 2. Clone what we need before mutably borrowing the graph
        let node_type = self.graph[index].node_type.clone();
        let defaults = self.graph[index].defaults.clone();

        // 3. Fill unconnected pins from defaults (connected pins take precedence)
        for (key, val) in defaults {
            inputs.entry(key).or_insert(val);
        }

        // 4. Look up and execute
        if let Some(node) = self.registry.get(&node_type) {
            let result = node.execute(&inputs, &self.tx);
            self.graph[index].last_outputs = result.outputs;
            self.graph[index].last_error = result.error;
        }
    }
}

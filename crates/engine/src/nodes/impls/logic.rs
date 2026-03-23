use std::collections::HashMap;

use shared::{DataType, Value};

use crate::nodes::{Node, NodeInstance, Pin};

pub struct TypeConverter;

impl Node for TypeConverter {
    fn id(&self) -> &'static str { "logic/type_converter" }
    fn inputs(&self, instance: &NodeInstance) -> Vec<Pin> { vec![Pin { name: "in", data_type: DataType::Any }] }
    fn outputs(&self, instance: &NodeInstance) -> Vec<Pin> {
        // Dynamically determine the output pin type based on the config
        let target_type = instance.config.get("target_type")
            .and_then(|v| v.as_datatype())
            .unwrap_or(DataType::Any);

        vec![Pin { name: "out", data_type: target_type }]
     }
    fn execute(&self, inputs: &HashMap<String, Value>, config: &HashMap<String, Value>) -> HashMap<String, Value> {
        let mut out = HashMap::new();

        // Get what type we are trying to convert to
        let target_type = match config.get("target_type").and_then(|v| v.as_datatype()) {
            Some(t) => t,
            None => 
        }

    }
}
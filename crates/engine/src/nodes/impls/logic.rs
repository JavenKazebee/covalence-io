use std::collections::HashMap;

use shared::{DataType, Value};

use crate::nodes::{Node, NodeExecutionResult, NodeInstance, Pin};

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
    fn execute(&self, inputs: &HashMap<String, Value>, config: &HashMap<String, Value>) -> NodeExecutionResult {
        let mut out = HashMap::new();

        // Parse target_type for what we are converting to
        let target_type = match config.get("target_type").and_then(|v| v.as_datatype()) {
            Some(t) => t,
            None => return NodeExecutionResult {
                outputs: out,
                error: Some("target_type is missing".into()),
            }
        };

        let input_value = match inputs.get("in") {
            Some(v) => v,
            None => return NodeExecutionResult {
                outputs: out,
                error: Some("input is missing".into()),
            }
        };

        let converted = match target_type {
            DataType::Float => input_value.try_into().map(|f| Value::Float(f)),
            DataType::Bool => input_value.try_into().map(|b| Value::Bool(b)),
            DataType::String => input_value.try_into().map(|s| Value::String(s)),
            DataType::List => input_value.try_into().map(|l| Value::List(l)),
            DataType::Object => input_value.try_into().map(|o| Value::Object(o)),
            DataType::Trigger => input_value.try_into().map(|t| Value::Trigger(t)),
            DataType::Any => return NodeExecutionResult {
                outputs: out,
                error: Some("'Any' target_type is not supported".into()),
            }
        };


    }
}
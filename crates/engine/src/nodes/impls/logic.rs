use std::collections::HashMap;

use shared::{DataType, Value};

use crate::nodes::{Node, NodeExecutionResult, NodeInstance, Pin};

pub struct TypeConverter;

fn convert(input: &Value, target: &DataType) -> Result<Value, String> {
    match target {
        DataType::Float => match input {
            Value::Float(f) => Ok(Value::Float(*f)),
            Value::Bool(b) => Ok(Value::Float(if *b { 1.0 } else { 0.0 })),
            Value::String(s) => s
                .parse::<f64>()
                .map(Value::Float)
                .map_err(|_| format!("cannot parse {:?} as Float", s)),
            Value::Null => Ok(Value::Float(0.0)),
            other => Err(format!("cannot convert {:?} to Float", other.get_type())),
        },
        DataType::Bool => match input {
            Value::Bool(b) => Ok(Value::Bool(*b)),
            Value::Float(f) => Ok(Value::Bool(*f != 0.0)),
            Value::String(s) => Ok(Value::Bool(!s.is_empty())),
            Value::Null => Ok(Value::Bool(false)),
            other => Err(format!("cannot convert {:?} to Bool", other.get_type())),
        },
        DataType::String => match input {
            Value::String(s) => Ok(Value::String(s.clone())),
            Value::Float(f) => Ok(Value::String(f.to_string())),
            Value::Bool(b) => Ok(Value::String(b.to_string())),
            Value::Null => Ok(Value::String(String::new())),
            other => Err(format!("cannot convert {:?} to String", other.get_type())),
        },
        DataType::List => match input {
            Value::List(l) => Ok(Value::List(l.clone())),
            other => Ok(Value::List(vec![other.clone()])),
        },
        DataType::Object => match input {
            Value::Object(o) => Ok(Value::Object(o.clone())),
            other => Err(format!("cannot convert {:?} to Object", other.get_type())),
        },
        DataType::Trigger => Err("conversion to Trigger is not supported".to_string()),
        DataType::Any => Err("'Any' is not a valid target type".to_string()),
    }
}

impl Node for TypeConverter {
    fn id(&self) -> &'static str {
        "logic/type_converter"
    }

    fn inputs(&self, _instance: &NodeInstance) -> Vec<Pin> {
        vec![Pin {
            name: "in",
            data_type: DataType::Any,
        }]
    }

    fn outputs(&self, instance: &NodeInstance) -> Vec<Pin> {
        let target_type = instance
            .config
            .get("target_type")
            .and_then(|v| v.as_datatype())
            .unwrap_or(DataType::Any);
        vec![Pin {
            name: "out",
            data_type: target_type,
        }]
    }

    fn execute(
        &self,
        inputs: &HashMap<String, Value>,
        config: &HashMap<String, Value>,
    ) -> NodeExecutionResult {
        let mut out = HashMap::new();

        let target_type = match config.get("target_type").and_then(|v| v.as_datatype()) {
            Some(t) => t,
            None => {
                return NodeExecutionResult {
                    outputs: out,
                    error: Some("target_type config is missing or invalid".into()),
                };
            }
        };

        let input_value = match inputs.get("in") {
            Some(v) => v,
            None => {
                return NodeExecutionResult {
                    outputs: out,
                    error: Some("input pin 'in' is missing".into()),
                };
            }
        };

        match convert(input_value, &target_type) {
            Ok(converted) => {
                out.insert("out".to_string(), converted);
                NodeExecutionResult {
                    outputs: out,
                    error: None,
                }
            }
            Err(msg) => NodeExecutionResult {
                outputs: out,
                error: Some(msg),
            },
        }
    }
}

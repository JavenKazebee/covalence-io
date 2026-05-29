use std::collections::HashMap;

use shared::{DataType, Event, Message, Value};
use tokio::sync::broadcast;

use crate::nodes::{Node, NodeExecutionResult, NodeInstance, Pin};

/// Action node that sends start/stop/reset/set_elapsed commands to a TimerDriver instance.
///
/// Inputs (all have defaults; wire to override at runtime):
///   "trigger" (Any)    — any value arriving here fires the command
///   "target"  (String) — driver id to command (e.g. "show_clock")
///   "command" (String) — "start" | "stop" | "reset" | "set_elapsed"
///   "value"   (Float)  — ms value for "set_elapsed"
pub struct TimerControlNode;

impl Node for TimerControlNode {
    fn id(&self) -> &'static str {
        "timer/control"
    }

    fn inputs(&self, _instance: &NodeInstance) -> Vec<Pin> {
        vec![
            Pin {
                name: "trigger",
                data_type: DataType::Any,
            },
            Pin {
                name: "target",
                data_type: DataType::String,
            },
            Pin {
                name: "command",
                data_type: DataType::String,
            },
            Pin {
                name: "value",
                data_type: DataType::Float,
            },
        ]
    }

    fn outputs(&self, _instance: &NodeInstance) -> Vec<Pin> {
        vec![]
    }

    fn execute(
        &self,
        inputs: &HashMap<String, Value>,
        tx: &broadcast::Sender<Message>,
    ) -> NodeExecutionResult {
        let target = match inputs.get("target") {
            Some(Value::String(s)) => s.clone(),
            _ => {
                return NodeExecutionResult {
                    outputs: HashMap::new(),
                    error: Some("timer/control: missing or invalid 'target' input".into()),
                };
            }
        };

        let command = match inputs.get("command") {
            Some(Value::String(s)) => s.clone(),
            _ => {
                return NodeExecutionResult {
                    outputs: HashMap::new(),
                    error: Some("timer/control: missing or invalid 'command' input".into()),
                };
            }
        };

        let mut params = HashMap::new();
        if let Some(v) = inputs.get("value") {
            params.insert("ms".to_string(), v.clone());
        }

        let _ = tx.send(Message {
            seq: 0,
            source: "timer/control".to_string(),
            payload: Event::Command {
                target,
                name: command,
                params,
            },
        });

        NodeExecutionResult {
            outputs: HashMap::new(),
            error: None,
        }
    }
}

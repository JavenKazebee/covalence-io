use crate::drivers::{Driver, DriverContext};
use async_trait::async_trait;
use shared::{Event, Message, Value};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::broadcast;
use tracing::info;

pub struct VirtualKbDriver {
    id: String,
}

impl VirtualKbDriver {
    pub fn new(id: &str) -> Self {
        Self { id: id.to_string() }
    }
}

#[async_trait]
impl Driver for VirtualKbDriver {
    fn id(&self) -> &str {
        &self.id
    }

    async fn start(
        &self,
        context: DriverContext,
        mut rx: broadcast::Receiver<Message>,
    ) -> Result<(), String> {
        info!(
            driver = self.id,
            "Starting Virtual Keyboard Driver. Type command below:"
        );

        let in_context = context;
        tokio::spawn(async move {
            let stdin = tokio::io::stdin();
            let mut reader = BufReader::new(stdin).lines();

            while let Ok(Some(line)) = reader.next_line().await {
                let input = line.trim();
                if input.is_empty() {
                    continue;
                }

                if let Some((key, value)) = input.split_once(' ') {
                    let value = match value.parse::<f64>() {
                        Ok(v) => Value::Float(v),
                        Err(_) => Value::String(value.to_string()),
                    };

                    in_context.update(key, value);
                } else {
                    in_context.update(input, Value::Null);
                }
            }
        });

        while let Ok(msg) = rx.recv().await {
            if let Event::Command {
                target,
                name: command,
                params,
            } = msg.payload
            {
                if target == self.id || target == "all" {
                    println!(
                        "[from {}] Received command: {} ({:?})",
                        msg.source, command, params
                    );
                }
            }
        }

        Ok(())
    }
}

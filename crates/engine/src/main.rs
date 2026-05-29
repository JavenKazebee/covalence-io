mod drivers;
mod nodes;

use shared::Message;
use tokio::sync::broadcast;
use tokio::time::Duration;
use tracing::{Level, info};
use tracing_subscriber::FmtSubscriber;

use crate::drivers::DriverManager;
use crate::drivers::timer::{TimerDriver, TimerMode};
use crate::drivers::virtual_kb::VirtualKbDriver;
use crate::nodes::impls::logic::TypeConverter;
use crate::nodes::manager::NodeManager;
use crate::nodes::registry::NodeRegistry;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    info!("Engine starting...");

    // Global event bus
    let (tx, _rx) = broadcast::channel::<Message>(1024);

    // Driver manager
    let mut manager = DriverManager::new(tx.clone());
    manager.register(Box::new(VirtualKbDriver::new("keyboard_1")));
    manager.register(Box::new(TimerDriver::new(
        "show_clock",
        TimerMode::CountUp,
        true,
    )));
    manager.register(Box::new(TimerDriver::new(
        "break_timer",
        TimerMode::CountDown { from_ms: 600_000 },
        false,
    )));
    manager.register(Box::new(TimerDriver::new(
        "heartbeat",
        TimerMode::Interval { every_ms: 1_000 },
        true,
    )));

    let cache = manager.get_cache();

    // Collect driver-defined nodes before start_all() consumes the manager
    let driver_nodes = manager.collect_nodes();
    manager.start_all().await;

    // Node registry — built-in nodes + driver-contributed nodes
    let mut registry = NodeRegistry::new();
    registry.register(TypeConverter);
    for node in driver_nodes {
        registry.register_boxed(node);
    }

    let _node_manager = NodeManager::new(registry, tx.clone());

    // Print signals to the console
    let mut signal_rx = tx.subscribe();
    tokio::spawn(async move {
        while let Ok(msg) = signal_rx.recv().await {
            info!("Received signal: {} ({:?})", msg.source, msg.payload);
        }
    });

    // Print telemetry cache to the console
    let cache_monitor = cache.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            interval.tick().await;
            if !cache_monitor.is_empty() {
                println!("------ Telemetry Cache ------");
                for entry in cache_monitor.iter() {
                    println!("  {}: {:?}", entry.key(), entry.value());
                }
                println!("--------------------------------");
            }
        }
    });

    info!("All systems go. Press Ctrl+C to shut down...");
    tokio::signal::ctrl_c().await?;
    info!("Shutting down...");
    Ok(())
}

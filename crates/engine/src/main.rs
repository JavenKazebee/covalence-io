mod drivers;
use shared::Message;
use tokio::sync::broadcast;
use tokio::time::Duration;
use tracing::{Level, info};
use tracing_subscriber::FmtSubscriber;

use crate::drivers::DriverManager;
use crate::drivers::virtual_kb::VirtualKbDriver;

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
    let cache = manager.get_cache();
    manager.start_all().await;

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

    info!("All systems go.Press Ctrl+C to shut down...");
    tokio::signal::ctrl_c().await?;
    info!("Shutting down...");
    Ok(())
}

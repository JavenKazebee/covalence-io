use tokio::sync::broadcast;
use shared::{Event, Message, Value};
use tokio::time::{self, Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (tx, _rx) = broadcast::channel::<Message>(1024);

    println!("Engine started");

    let mut router = tx.subscribe();
    tokio::spawn(async move {
        while let Ok(msg) = router.recv().await {
            println!("Received message: {:?}", msg);
        }
    });

    let tx_heartbeat = tx.clone(); // Clone the transmitter for this task
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(2));
        let mut count = 0;

        loop {
            interval.tick().await; // Wait for the next 2-second mark
            count += 1;

            let heartbeat_msg = Message {
                seq: count,
                source: "internal_clock".into(),
                payload: Event::Input {
                    source: "system".into(),
                    id: "heartbeat".into(),
                    value: Value::String(format!("Tick {}", count)),
                },
            };

            // Send it to the bus!
            let _ = tx_heartbeat.send(heartbeat_msg);
        }
    });

    println!("All systems go.Press Ctrl+C to shut down...");
    tokio::signal::ctrl_c().await?;
    println!("Shutting down...");
    Ok(())
}
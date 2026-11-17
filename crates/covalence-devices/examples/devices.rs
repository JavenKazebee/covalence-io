//! Lists the devices the core would see, then prints them coming and going.
//! For checking a real device by hand: `cargo run -p covalence-devices --example devices`.

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let devices = covalence_devices::Devices::system();
    let mut changes = devices.subscribe();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    if let Some(e) = devices.scan_error() {
        eprintln!("cannot enumerate: {e}");
    }
    for d in devices.list() {
        println!("{:<48} {}  {}", d.id, d.path, d.product.unwrap_or_default());
    }
    println!("-- watching; ctrl-c to stop");
    while let Ok(change) = changes.recv().await {
        match change {
            covalence_devices::DeviceEvent::Added(d) => println!("+ {}  {}", d.id, d.path),
            covalence_devices::DeviceEvent::Removed(d) => println!("- {}  {}", d.id, d.path),
        }
    }
}

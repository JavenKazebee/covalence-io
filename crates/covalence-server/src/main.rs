//! Headless Covalence IO server.

fn main() {
    println!(
        "covalence-server {} (protocol {})",
        env!("CARGO_PKG_VERSION"),
        covalence_core::PROTOCOL_VERSION
    );
}

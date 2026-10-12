//! Verify per-process execution selection without building the editor.
use filmcraft_vision::{SeaRaft, execution::Device};
use std::sync::atomic::AtomicBool;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    for device in [Device::Cpu, Device::Auto] {
        let runtime = SeaRaft::start_with_device(&AtomicBool::new(false), device)?;
        println!("SEA-RAFT requested {} · actual {} · recorded request {}", device.as_str(), runtime.metadata["device"], runtime.metadata["executionRequest"]);
        if device == Device::Cpu && runtime.metadata["device"] != "cpu" {
            return Err("CPU preference was not honored".into());
        }
    }
    Ok(())
}

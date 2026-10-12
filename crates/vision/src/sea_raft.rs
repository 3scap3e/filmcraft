//! Rust connector to an isolated local SEA-RAFT runtime; never falls back to patch matching.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};

pub const MAX_SIZE: usize = 640;
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Vector {
    pub from: [f32; 2],
    pub to: [f32; 2],
    pub uncertainty_log_scale: f32,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Inference {
    pub width: usize,
    pub height: usize,
    pub vectors: Vec<Vector>,
    pub rejected_points: usize,
    pub mean_pixel_change: f32,
    pub changed_pixel_fraction: f32,
    pub mean_motion_pixels: f32,
    pub dense_pixel_count: usize,
}
impl Inference {
    fn validate(&self, width: usize, height: usize) -> Result<(), String> {
        if self.width != width
            || self.height != height
            || self.dense_pixel_count != width.saturating_mul(height)
            || self.vectors.len() > 2048
            || self.rejected_points > 2048
            || !self.mean_pixel_change.is_finite()
            || !(0.0..=255.0).contains(&self.mean_pixel_change)
            || !self.changed_pixel_fraction.is_finite()
            || !(0.0..=1.0).contains(&self.changed_pixel_fraction)
            || !self.mean_motion_pixels.is_finite()
            || !(0.0..=10000.0).contains(&self.mean_motion_pixels)
            || self.vectors.iter().any(|v| {
                !v.from.iter().all(|n| n.is_finite() && (0.0..=1.0).contains(n))
                    || !v.to.iter().all(|n| n.is_finite() && (-8.0..=9.0).contains(n))
                    || !v.uncertainty_log_scale.is_finite()
                    || !(0.0..=10.0).contains(&v.uncertainty_log_scale)
            })
        {
            return Err("SEA-RAFT returned invalid motion evidence".into());
        }
        Ok(())
    }
}
fn validate_input(width: usize, height: usize, first: &[u8], second: &[u8]) -> Result<(), String> {
    let bytes = width.checked_mul(height).and_then(|n| n.checked_mul(4)).ok_or("Frame dimensions overflow")?;
    if !(64..=MAX_SIZE).contains(&width) || !(64..=MAX_SIZE).contains(&height) || first.len() != bytes || second.len() != bytes {
        return Err("SEA-RAFT needs matching RGBA frames with dimensions between 64 and 640 pixels".into());
    }
    Ok(())
}
#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::*;
    use std::io::{BufRead, BufReader, BufWriter, Write};
    use std::path::PathBuf;
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc::{self, Receiver, SyncSender};
    use std::time::{Duration, Instant};
    struct Request {
        width: usize,
        height: usize,
        first: Vec<u8>,
        second: Vec<u8>,
    }
    pub struct SeaRaft {
        child: Child,
        requests: Option<SyncSender<Request>>,
        responses: Receiver<Result<Value, String>>,
        pub metadata: Value,
    }
    fn runtime_directory() -> Result<PathBuf, String> {
        if let Some(path) = std::env::var_os("FILMCRAFT_SEA_RAFT_DIR") {
            return Ok(PathBuf::from(path));
        }
        let home =
            std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).ok_or("Set FILMCRAFT_SEA_RAFT_DIR to your installed SEA-RAFT runtime")?;
        let home = PathBuf::from(home);
        Ok(if cfg!(target_os = "macos") { home.join("Library/Application Support/FilmCraft/sea-raft") } else { home.join(".cache/filmcraft/sea-raft") })
    }
    fn read_response(reader: &mut impl BufRead) -> Result<Value, String> {
        let mut line = Vec::new();
        loop {
            let buffer = reader.fill_buf().map_err(|e| format!("SEA-RAFT output: {e}"))?;
            if buffer.is_empty() {
                return Err("SEA-RAFT stopped unexpectedly. Check the local runtime installation.".into());
            }
            let end = buffer.iter().position(|b| *b == b'\n');
            let count = end.map_or(buffer.len(), |n| n + 1);
            if line.len().saturating_add(count) > 2_097_152 {
                return Err("SEA-RAFT response exceeded the size limit".into());
            }
            line.extend_from_slice(buffer.get(..count).ok_or("Invalid response buffer")?);
            reader.consume(count);
            if end.is_some() {
                break;
            }
        }
        let value: Value = serde_json::from_slice(&line).map_err(|e| format!("Invalid SEA-RAFT response: {e}"))?;
        if let Some(error) = value.get("error").and_then(Value::as_str) {
            return Err(format!("SEA-RAFT: {}", error.chars().take(2000).collect::<String>()));
        }
        Ok(value)
    }
    impl SeaRaft {
        pub fn start(cancel: &AtomicBool) -> Result<Self, String> {
            Self::start_with_device(cancel, crate::execution::Device::Auto)
        }
        pub fn start_with_device(cancel: &AtomicBool, device: crate::execution::Device) -> Result<Self, String> {
            let directory = runtime_directory()?;
            let python = if cfg!(target_os = "windows") { directory.join("venv-arm/Scripts/python.exe") } else { directory.join("venv-arm/bin/python") };
            let script = directory.join("worker.py");
            if !python.is_file() || !script.is_file() {
                return Err(format!(
                    "SEA-RAFT runtime is missing at {}. Install it or set FILMCRAFT_SEA_RAFT_DIR. No fallback analyzer is used.",
                    directory.display()
                ));
            }
            let mut child = Command::new(python)
                .env("FILMCRAFT_SEA_RAFT_DEVICE", device.as_str())
                .arg("-u")
                .arg(script)
                .current_dir(&directory)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| format!("Could not start SEA-RAFT: {e}"))?;
            let Some(stdin) = child.stdin.take() else {
                let _ = child.kill();
                let _ = child.wait();
                return Err("SEA-RAFT input pipe unavailable".into());
            };
            let Some(stdout) = child.stdout.take() else {
                let _ = child.kill();
                let _ = child.wait();
                return Err("SEA-RAFT output pipe unavailable".into());
            };
            let (requests, input) = mpsc::sync_channel::<Request>(1);
            let (output, responses) = mpsc::sync_channel(1);
            let spawn = std::thread::Builder::new().name("filmcraft-sea-raft-io".into()).spawn(move || {
                let failure = output.clone();
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<(), String> {
                    let mut writer = BufWriter::new(stdin);
                    let mut reader = BufReader::new(stdout);
                    let ready = read_response(&mut reader)?;
                    if output.send(Ok(ready)).is_err() {
                        return Ok(());
                    }
                    while let Ok(request) = input.recv() {
                        serde_json::to_writer(&mut writer, &serde_json::json!({"width":request.width,"height":request.height})).map_err(|e| e.to_string())?;
                        writer
                            .write_all(b"\n")
                            .and_then(|_| writer.write_all(&request.first))
                            .and_then(|_| writer.write_all(&request.second))
                            .and_then(|_| writer.flush())
                            .map_err(|e| format!("SEA-RAFT input: {e}"))?;
                        let value = read_response(&mut reader)?;
                        if output.send(Ok(value)).is_err() {
                            break;
                        }
                    }
                    Ok(())
                }))
                .unwrap_or_else(|_| Err("SEA-RAFT communication worker failed".into()));
                if let Err(error) = result {
                    let _ = failure.send(Err(error));
                }
            });
            if let Err(error) = spawn {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("Could not start SEA-RAFT communication: {error}"));
            }
            let mut runtime = Self { child, requests: Some(requests), responses, metadata: Value::Null };
            let ready = runtime.receive(cancel)?;
            if ready.get("ready").and_then(Value::as_bool) != Some(true)
                || ready.get("analyzer").and_then(Value::as_str) != Some("SEA-RAFT")
                || ready.get("model").and_then(Value::as_str) != Some("MemorySlices/Tartan-C-T-TSKH-spring540x960-M")
                || ready.get("checkpointRevision").and_then(Value::as_str) != Some("eb97ef34ba5d856c3fa2cdcd073150c057ac8b69")
            {
                return Err("Unexpected SEA-RAFT runtime or checkpoint".into());
            }
            if ready.get("sourceRevision").and_then(Value::as_str) != Some("e0882ac0cc336057eedecd4f793b9ca5b7be9dc9")
                || ready.get("checkpointSha256").and_then(Value::as_str) != Some("cb8cfbf14c5e0f6734b64add383708b7ff68cc6089a0007c67165d4761346102")
            {
                return Err("SEA-RAFT model provenance does not match the tested checkpoint".into());
            }
            runtime.metadata = ready;
            Ok(runtime)
        }
        fn receive(&mut self, cancel: &AtomicBool) -> Result<Value, String> {
            let started = Instant::now();
            loop {
                if cancel.load(Ordering::Relaxed) {
                    return Err("Motion analysis cancelled".into());
                }
                if started.elapsed() >= Duration::from_secs(90) {
                    return Err("SEA-RAFT timed out. Try a shorter range or check the runtime.".into());
                }
                match self.responses.recv_timeout(Duration::from_millis(50)) {
                    Ok(result) => return result,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => return Err("SEA-RAFT communication stopped".into()),
                }
            }
        }
        pub fn analyze(&mut self, width: usize, height: usize, first: &[u8], second: &[u8], cancel: &AtomicBool) -> Result<Inference, String> {
            validate_input(width, height, first, second)?;
            if cancel.load(Ordering::Relaxed) {
                return Err("Motion analysis cancelled".into());
            }
            self.requests
                .as_ref()
                .ok_or("SEA-RAFT is closed")?
                .try_send(Request { width, height, first: first.to_vec(), second: second.to_vec() })
                .map_err(|e| format!("SEA-RAFT request: {e}"))?;
            let result: Inference = serde_json::from_value(self.receive(cancel)?).map_err(|e| format!("Invalid SEA-RAFT motion result: {e}"))?;
            result.validate(width, height)?;
            Ok(result)
        }
    }
    impl Drop for SeaRaft {
        fn drop(&mut self) {
            self.requests.take();
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn rejects_oversized_or_broken_responses() {
            assert!(read_response(&mut std::io::Cursor::new(vec![b'x'; 2_097_153])).is_err());
            assert!(read_response(&mut std::io::Cursor::new(b"not json\n")).is_err());
            assert!(read_response(&mut std::io::Cursor::new(b"{\"error\":\"model missing\"}\n")).is_err());
        }
    }
}
#[cfg(not(target_arch = "wasm32"))]
pub use native::SeaRaft;
#[cfg(target_arch = "wasm32")]
pub struct SeaRaft {
    pub metadata: Value,
}
#[cfg(target_arch = "wasm32")]
impl SeaRaft {
    pub fn start_with_device(_: &AtomicBool, _: crate::execution::Device) -> Result<Self, String> {
        Err("SEA-RAFT requires the desktop app".into())
    }
    pub fn start(_: &AtomicBool) -> Result<Self, String> {
        Err("SEA-RAFT requires the desktop app".into())
    }
    pub fn analyze(&mut self, _: usize, _: usize, _: &[u8], _: &[u8], _: &AtomicBool) -> Result<Inference, String> {
        Err("SEA-RAFT requires the desktop app".into())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires installed external SEA-RAFT runtime"]
    fn installed_runtime_cancels_without_fallback() {
        let cancel = AtomicBool::new(false);
        let mut runtime = SeaRaft::start(&cancel).unwrap();
        assert_eq!(runtime.metadata["analyzer"], "SEA-RAFT");
        cancel.store(true, Ordering::Relaxed);
        let pixels = vec![0; 64 * 64 * 4];
        assert!(runtime.analyze(64, 64, &pixels, &pixels, &cancel).unwrap_err().contains("cancelled"));
    }
    #[test]
    fn rejects_hostile_frames_and_results() {
        for (w, h) in [(0, 0), (usize::MAX, 2), (641, 64), (64, 63), (64, 64)] {
            assert!(validate_input(w, h, &[], &[]).is_err());
        }
        let mut result = Inference {
            width: 64,
            height: 64,
            vectors: vec![],
            rejected_points: 0,
            mean_pixel_change: 0.0,
            changed_pixel_fraction: 0.0,
            mean_motion_pixels: 0.0,
            dense_pixel_count: 4096,
        };
        assert!(result.validate(64, 64).is_ok());
        result.changed_pixel_fraction = f32::NAN;
        assert!(result.validate(64, 64).is_err());
    }
}

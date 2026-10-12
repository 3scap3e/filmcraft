//! Bounded Rust connector to the isolated official BootsTAPIR runtime.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
pub const MAX_FRAMES: usize = 120;
pub const MAX_POINTS: usize = 64;
pub const SOURCE_REVISION: &str = "730cda1c730877cfedbe01bf87fb1cadb78a565d";
pub const CHECKPOINT_SHA256: &str = "8493c7a69e02c85b9382fbb3c7b8b539b36bc08ede744b9e99feb739a0129f4b";
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Query {
    pub frame_index: usize,
    pub position: [f64; 2],
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Observation {
    pub position: [f64; 2],
    pub visible: bool,
    pub visibility_score: f64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub query: Query,
    pub observations: Vec<Observation>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Inference {
    pub analyzer: String,
    pub frame_count: usize,
    pub tracks: Vec<Track>,
    pub runtime: Value,
}
pub fn validate_queries(queries: &[Query], frames: usize) -> Result<(), String> {
    if frames < 2
        || queries.is_empty()
        || queries.len() > MAX_POINTS
        || queries.iter().any(|q| q.frame_index >= frames || !q.position.iter().all(|v| v.is_finite() && (0.0..1.0).contains(v)))
    {
        return Err("Tracking needs at least two frames and 1–64 valid query points".into());
    }
    Ok(())
}
pub fn grid(frame: usize, side: usize, frames: usize) -> Result<Vec<Query>, String> {
    if !(2..=8).contains(&side) || frame >= frames || frames < 2 {
        return Err("Choose a 2–8 point grid side and a frame inside the selected range".into());
    }
    Ok((0..side)
        .flat_map(|y| (0..side).map(move |x| Query { frame_index: frame, position: [(x as f64 + 0.5) / side as f64, (y as f64 + 0.5) / side as f64] }))
        .collect())
}
impl Inference {
    fn validate(&self, queries: &[Query], frames: usize) -> Result<(), String> {
        if self.runtime.get("sourceRevision").and_then(Value::as_str) != Some(SOURCE_REVISION)
            || self.runtime.get("checkpointSha256").and_then(Value::as_str) != Some(CHECKPOINT_SHA256)
            || self.analyzer != "BootsTAPIR"
            || self.frame_count != frames
            || self.tracks.len() != queries.len()
            || self.tracks.iter().zip(queries).any(|(track, query)| {
                track.query.frame_index != query.frame_index
                    || track.query.position.iter().zip(query.position).any(|(a, b)| !a.is_finite() || (*a - b).abs() > 1e-12)
                    || track.observations.len() != frames
                    || track.observations.iter().any(|o| {
                        !o.position.iter().all(|v| v.is_finite() && (-8.0..=9.0).contains(v))
                            || !o.visibility_score.is_finite()
                            || !(0.0..=1.0).contains(&o.visibility_score)
                            || o.visible != (o.visibility_score > 0.5)
                    })
            })
        {
            return Err("BootsTAPIR returned invalid tracking evidence".into());
        }
        Ok(())
    }
}
pub fn infer(width: usize, height: usize, frames: Vec<Vec<u8>>, queries: Vec<Query>, cancel: &AtomicBool) -> Result<Inference, String> {
    infer_with_device(width, height, frames, queries, cancel, crate::execution::Device::Auto)
}
#[cfg(not(target_arch = "wasm32"))]
pub fn infer_with_device(
    width: usize,
    height: usize,
    frames: Vec<Vec<u8>>,
    queries: Vec<Query>,
    cancel: &AtomicBool,
    device: crate::execution::Device,
) -> Result<Inference, String> {
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    if device == crate::execution::Device::Gpu && cfg!(target_os = "macos") {
        return Err("BootsTAPIR Apple GPU predictions failed validation. Choose Auto or CPU.".into());
    }
    validate_queries(&queries, frames.len())?;
    if frames.len() > MAX_FRAMES {
        return Err("Internal tracking chunk exceeds 120 frames".into());
    }
    let bytes = width.checked_mul(height).and_then(|n| n.checked_mul(4)).ok_or("Tracking dimensions overflow")?;
    if !(64..=512).contains(&width)
        || !(64..=512).contains(&height)
        || bytes.checked_mul(frames.len()).is_none_or(|n| n > 128_000_000)
        || frames.iter().any(|f| f.len() != bytes)
    {
        return Err("Invalid or oversized tracking frames".into());
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("Point tracking cancelled".into());
    }
    let root = if let Some(path) = std::env::var_os("FILMCRAFT_BOOTSTAPIR_DIR") {
        std::path::PathBuf::from(path)
    } else {
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).ok_or("Set FILMCRAFT_BOOTSTAPIR_DIR to your installed runtime")?;
        let home = std::path::PathBuf::from(home);
        if cfg!(target_os = "macos") { home.join("Library/Application Support/FilmCraft/bootstapir") } else { home.join(".cache/filmcraft/bootstapir") }
    };
    let python = std::env::var_os("FILMCRAFT_BOOTSTAPIR_PYTHON")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| root.join(if cfg!(target_os = "windows") { "python.exe" } else { "python" }));
    if !python.exists() || !root.join("worker.py").exists() || !root.join("metadata.json").exists() {
        return Err("BootsTAPIR runtime not installed. See docs/reverse-engineering-point-tracking.md".into());
    }
    struct Process(std::process::Child);
    impl Drop for Process {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut process = Process(
        Command::new(python)
            .env("FILMCRAFT_BOOTSTAPIR_DEVICE", device.as_str())
            .arg(root.join("worker.py"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Could not start BootsTAPIR: {e}"))?,
    );
    let mut input = process.0.stdin.take().ok_or("Tracking input pipe unavailable")?;
    let output = process.0.stdout.take().ok_or("Tracking output pipe unavailable")?;
    let frame_count = frames.len();
    let header = serde_json::to_vec(&json!({"width":width,"height":height,"frameCount":frame_count,"queries":queries})).map_err(|e| e.to_string())?;
    let (tx, rx) = mpsc::sync_channel(2);
    let write_tx = tx.clone();
    std::thread::Builder::new()
        .name("bootstapir-input".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<(), String> {
                input.write_all(&header).and_then(|_| input.write_all(b"\n")).map_err(|e| e.to_string())?;
                for frame in frames {
                    input.write_all(&frame).map_err(|e| e.to_string())?;
                }
                Ok(())
            }))
            .unwrap_or_else(|_| Err("Tracking input worker failed".into()));
            if let Err(e) = result {
                let _ = write_tx.send(Err(e));
            }
        })
        .map_err(|e| e.to_string())?;
    std::thread::Builder::new()
        .name("bootstapir-output".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<Value, String> {
                let mut raw = Vec::new();
                output.take(4_194_305).read_to_end(&mut raw).map_err(|e| e.to_string())?;
                if raw.len() > 4_194_304 {
                    return Err("Tracking output exceeded 4 MB".into());
                }
                let value: Value = serde_json::from_slice(&raw).map_err(|e| format!("Invalid tracking response: {e}"))?;
                if let Some(error) = value.get("error").and_then(Value::as_str) {
                    return Err(error.to_owned());
                }
                Ok(value)
            }))
            .unwrap_or_else(|_| Err("Tracking output worker failed".into()));
            let _ = tx.send(result);
        })
        .map_err(|e| e.to_string())?;
    let started = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("Point tracking cancelled".into());
        }
        if started.elapsed() > Duration::from_secs(600) {
            return Err("BootsTAPIR timed out; use fewer frames or points".into());
        }
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(value) => {
                let inference: Inference = serde_json::from_value(value?).map_err(|e| e.to_string())?;
                inference.validate(&queries, frame_count)?;
                return Ok(inference);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err("Tracking worker disconnected".into()),
        }
    }
}
#[cfg(target_arch = "wasm32")]
pub fn infer_with_device(
    _width: usize,
    _height: usize,
    _frames: Vec<Vec<u8>>,
    _queries: Vec<Query>,
    _cancel: &AtomicBool,
    _device: crate::execution::Device,
) -> Result<Inference, String> {
    Err("Local BootsTAPIR requires the desktop app".into())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grids_and_queries_are_bounded_and_use_xy_coordinates() {
        let q = grid(3, 4, 10).unwrap();
        assert_eq!(q.len(), 16);
        assert_eq!(q[0], Query { frame_index: 3, position: [0.125, 0.125] });
        assert_eq!(q[1].position, [0.375, 0.125]);
        for (f, s, n) in [(0, 0, 10), (0, 9, 10), (10, 4, 10)] {
            assert!(grid(f, s, n).is_err());
        }
        assert!(validate_queries(&[Query { frame_index: 0, position: [f64::NAN, 0.5] }], 10).is_err());
        assert!(validate_queries(&[], 10).is_err());
    }
    #[test]
    fn malformed_tracks_cannot_become_valid_results() {
        let q = Query { frame_index: 0, position: [0.5, 0.5] };
        let mut i = Inference {
            analyzer: "BootsTAPIR".into(),
            frame_count: 2,
            tracks: vec![Track { query: q.clone(), observations: vec![Observation { position: [0.5, 0.5], visible: true, visibility_score: 0.9 }; 2] }],
            runtime: json!({"sourceRevision":SOURCE_REVISION,"checkpointSha256":CHECKPOINT_SHA256}),
        };
        assert!(i.validate(std::slice::from_ref(&q), 2).is_ok());
        i.tracks[0].observations[0].position[1] = f64::INFINITY;
        assert!(i.validate(&[q], 2).is_err());
    }
}

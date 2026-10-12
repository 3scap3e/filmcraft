//! Bounded connector to the official SAM 2.1 video predictor.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
pub const SOURCE_REVISION: &str = "2b90b9f5ceec907a1c18123530e92e794ad901a4";
// Published SHA256 of the official SAM 2.1 Tiny checkpoint.
pub const CHECKPOINT_SHA256: &str = "7402e0d864fa82708a20fbd15bc84245c2f26dff0eb43a4b5b93452deb34be69";
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Prompt {
    pub position: [f64; 2],
    pub foreground: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Object {
    pub name: String,
    pub frame_index: usize,
    pub points: Vec<Prompt>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mask {
    pub runs: Vec<[usize; 2]>,
}
impl Mask {
    /// 0 = background, 1 = mask interior, 2 = actual four-neighbour mask boundary.
    pub fn display_pixels(&self, size: [usize; 2]) -> Result<Vec<u8>, String> {
        self.measurements(size)?;
        let [w, h] = size;
        let total = w.checked_mul(h).ok_or("Mask dimensions overflow")?;
        let mut coverage = vec![0u8; total];
        for &[start, len] in &self.runs {
            let end = start.checked_add(len).ok_or("Mask run overflow")?;
            coverage.get_mut(start..end).ok_or("Mask run outside image")?.fill(1);
        }
        let mut display = coverage.clone();
        for (i, v) in coverage.iter().enumerate() {
            if *v == 0 {
                continue;
            }
            let x = i % w;
            let y = i / w;
            let boundary = x == 0
                || x + 1 == w
                || y == 0
                || y + 1 == h
                || coverage.get(i.saturating_sub(1)).copied().unwrap_or(0) == 0
                || coverage.get(i + 1).copied().unwrap_or(0) == 0
                || coverage.get(i.saturating_sub(w)).copied().unwrap_or(0) == 0
                || coverage.get(i + w).copied().unwrap_or(0) == 0;
            if boundary && let Some(pixel) = display.get_mut(i) {
                *pixel = 2;
            }
        }
        Ok(display)
    }
    pub fn measurements(&self, size: [usize; 2]) -> Result<(usize, Option<[f64; 2]>, Option<[f64; 4]>), String> {
        let [w, h] = size;
        let total = w.checked_mul(h).filter(|n| *n <= 102400 && *n > 0).ok_or("Invalid mask size")?;
        if self.runs.len() > total {
            return Err("Too many mask runs".into());
        }
        let mut last = 0;
        let mut area = 0;
        let mut sx = 0u64;
        let mut sy = 0u64;
        let mut bounds = [w, h, 0, 0];
        for &[start, len] in &self.runs {
            let end = start.checked_add(len).ok_or("Mask run overflow")?;
            if len == 0 || start < last || end > total {
                return Err("Invalid mask run".into());
            }
            last = end;
            let sums = |n: usize| {
                let rows = (n / w) as u64;
                let rest = (n % w) as u64;
                let width = w as u64;
                (rows * width * width.saturating_sub(1) / 2 + rest * rest.saturating_sub(1) / 2, width * rows * rows.saturating_sub(1) / 2 + rows * rest)
            };
            let (ex, ey) = sums(end);
            let (ax, ay) = sums(start);
            sx += ex - ax;
            sy += ey - ay;
            area += len;
            let first_y = start / w;
            let last_y = (end - 1) / w;
            let (left, right) = if first_y == last_y { (start % w, (end - 1) % w + 1) } else { (0, w) };
            bounds[0] = bounds[0].min(left);
            bounds[1] = bounds[1].min(first_y);
            bounds[2] = bounds[2].max(right);
            bounds[3] = bounds[3].max(last_y + 1);
        }
        Ok((
            area,
            (area > 0).then(|| [(sx as f64 / area as f64 + 0.5) / w as f64, (sy as f64 / area as f64 + 0.5) / h as f64]),
            (area > 0).then(|| [bounds[0] as f64 / w as f64, bounds[1] as f64 / h as f64, bounds[2] as f64 / w as f64, bounds[3] as f64 / h as f64]),
        ))
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub object: Object,
    pub masks: Vec<Option<Mask>>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Inference {
    pub analyzer: String,
    pub frame_count: usize,
    pub mask_size: [usize; 2],
    pub tracks: Vec<Track>,
    pub runtime: Value,
}
pub fn validate_objects(objects: &[Object], frames: usize) -> Result<(), String> {
    if frames == 0 || objects.is_empty() || objects.len() > 8 || frames.checked_mul(objects.len()).is_none_or(|n| n > 100000) {
        return Err("Choose 1–8 objects within the result memory budget".into());
    }
    for (i, o) in objects.iter().enumerate() {
        if o.name.trim().is_empty()
            || o.name.chars().count() > 80
            || objects.iter().take(i).any(|p| p.name == o.name)
            || o.frame_index >= frames
            || o.points.is_empty()
            || o.points.len() > 64
            || !o.points.iter().any(|p| p.foreground)
            || o.points.iter().any(|p| p.position.iter().any(|v| !v.is_finite() || !(0.0..1.0).contains(v)))
        {
            return Err("Each named object needs a reference frame, a foreground click and at most 64 valid clicks".into());
        }
    }
    Ok(())
}
impl Inference {
    pub fn validate(&self, objects: &[Object], frames: usize, size: [usize; 2]) -> Result<(), String> {
        validate_objects(objects, frames)?;
        if size.iter().any(|n| !(1..=320).contains(n)) {
            return Err("Invalid mask dimensions".into());
        }
        if self.analyzer != "SAM 2.1"
            || self.frame_count != frames
            || self.mask_size != size
            || self.tracks.len() != objects.len()
            || self.runtime.get("sourceRevision").and_then(Value::as_str) != Some(SOURCE_REVISION)
            || self.runtime.get("checkpointSha256").and_then(Value::as_str) != Some(CHECKPOINT_SHA256)
        {
            return Err("Invalid SAM model provenance or dimensions".into());
        }
        for (t, o) in self.tracks.iter().zip(objects) {
            if t.object.name != o.name
                || t.object.frame_index != o.frame_index
                || t.object.points.len() != o.points.len()
                || t.object
                    .points
                    .iter()
                    .zip(&o.points)
                    .any(|(a, b)| a.foreground != b.foreground || a.position.iter().zip(b.position).any(|(a, b)| !a.is_finite() || (*a - b).abs() > 1e-12))
                || t.masks.len() != frames
            {
                return Err("SAM result does not match object prompts or range".into());
            }
            for m in t.masks.iter().flatten() {
                m.measurements(size)?;
            }
        }
        Ok(())
    }
}
#[cfg(not(target_arch = "wasm32"))]
pub fn infer(
    width: usize,
    height: usize,
    frame_dir: std::path::PathBuf,
    frame_count: usize,
    objects: Vec<Object>,
    cancel: &AtomicBool,
    device: crate::execution::Device,
) -> Result<Inference, String> {
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    validate_objects(&objects, frame_count)?;
    if !(1..=320).contains(&width) || !(1..=320).contains(&height) {
        return Err("SAM input dimensions must be 1–320".into());
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("Point segmentation cancelled".into());
    }
    let root = if let Some(path) = std::env::var_os("FILMCRAFT_SAM2_DIR") {
        std::path::PathBuf::from(path)
    } else {
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).ok_or("Set FILMCRAFT_SAM2_DIR to your installed runtime")?;
        let home = std::path::PathBuf::from(home);
        if cfg!(target_os = "macos") { home.join("Library/Application Support/FilmCraft/sam2") } else { home.join(".cache/filmcraft/sam2") }
    };
    let python = std::env::var_os("FILMCRAFT_SAM2_PYTHON")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| root.join(if cfg!(target_os = "windows") { "python.exe" } else { "python" }));
    if !python.exists() || !root.join("worker.py").exists() || !root.join("metadata.json").exists() {
        return Err("SAM 2.1 runtime not installed. See docs/reverse-engineering-sam2.md".into());
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
            .env("FILMCRAFT_SAM2_DEVICE", device.as_str())
            .arg(root.join("worker.py"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Could not start SAM 2.1: {e}"))?,
    );
    let mut input = process.0.stdin.take().ok_or("Segmentation input pipe unavailable")?;
    let output = process.0.stdout.take().ok_or("Segmentation output pipe unavailable")?;
    let header = serde_json::to_vec(&json!({"width":width,"height":height,"frameCount":frame_count,"objects":objects,"frameDir":frame_dir}))
        .map_err(|e| e.to_string())?;
    let (tx, rx) = mpsc::sync_channel(2);
    let write_tx = tx.clone();
    std::thread::Builder::new()
        .name("sam2-input".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<(), String> {
                input.write_all(&header).and_then(|_| input.write_all(b"\n")).map_err(|e| e.to_string())?;
                Ok(())
            }))
            .unwrap_or_else(|_| Err("Segmentation input worker failed".into()));
            if let Err(e) = result {
                let _ = write_tx.send(Err(e));
            }
        })
        .map_err(|e| e.to_string())?;
    std::thread::Builder::new()
        .name("sam2-output".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<Value, String> {
                let mut raw = Vec::new();
                output.take(67_108_865).read_to_end(&mut raw).map_err(|e| e.to_string())?;
                if raw.len() > 67_108_864 {
                    return Err("Segmentation output exceeded 64 MB".into());
                }
                let value: Value = serde_json::from_slice(&raw).map_err(|e| format!("Invalid segmentation response: {e}"))?;
                if let Some(error) = value.get("error").and_then(Value::as_str) {
                    return Err(error.to_owned());
                }
                Ok(value)
            }))
            .unwrap_or_else(|_| Err("Segmentation output worker failed".into()));
            let _ = tx.send(result);
        })
        .map_err(|e| e.to_string())?;
    let started = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("Point segmentation cancelled".into());
        }
        if started.elapsed() > Duration::from_secs(3600) {
            return Err("SAM 2.1 timed out; use fewer frames or points".into());
        }
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(value) => {
                let mut inference: Inference = serde_json::from_value(value?).map_err(|e| e.to_string())?;
                inference.validate(&objects, frame_count, [width, height])?;
                for (track, object) in inference.tracks.iter_mut().zip(&objects) {
                    track.object = object.clone();
                }
                return Ok(inference);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err("Segmentation worker disconnected".into()),
        }
    }
}
#[cfg(target_arch = "wasm32")]
pub fn infer(
    _width: usize,
    _height: usize,
    _frame_dir: std::path::PathBuf,
    _frame_count: usize,
    _objects: Vec<Object>,
    _cancel: &AtomicBool,
    _device: crate::execution::Device,
) -> Result<Inference, String> {
    Err("Local SAM 2.1 requires the desktop app".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mask_geometry_and_hostile_runs() {
        let m = Mask { runs: vec![[5, 2], [9, 2]] };
        let (area, center, bounds) = m.measurements([4, 4]).unwrap();
        assert_eq!(area, 4);
        assert_eq!(center, Some([0.5, 0.5]));
        assert_eq!(bounds, Some([0.25, 0.25, 0.75, 0.75]));
        assert!(Mask { runs: vec![[usize::MAX, 2]] }.measurements([4, 4]).is_err());
        assert!(Mask { runs: vec![[5, 4], [7, 2]] }.measurements([4, 4]).is_err());
        assert_eq!(Mask { runs: vec![] }.measurements([4, 4]).unwrap().1, None);
    }
    #[test]
    fn full_and_cross_row_runs_match_pixel_geometry() {
        assert_eq!(Mask { runs: vec![[0, 16]] }.measurements([4, 4]).unwrap(), (16, Some([0.5, 0.5]), Some([0.0, 0.0, 1.0, 1.0])));
        let (area, center, bounds) = Mask { runs: vec![[3, 2]] }.measurements([4, 4]).unwrap();
        assert_eq!(area, 2);
        assert_eq!(center, Some([0.5, 0.25]));
        assert_eq!(bounds, Some([0.0, 0.0, 1.0, 0.5]));
    }
    #[test]
    fn hostile_inference_is_rejected() {
        let o = Object { name: "Sun".into(), frame_index: 0, points: vec![Prompt { position: [0.5, 0.5], foreground: true }] };
        let mut r = Inference {
            analyzer: "SAM 2.1".into(),
            frame_count: 1,
            mask_size: [4, 4],
            tracks: vec![Track { object: o.clone(), masks: vec![Some(Mask { runs: vec![[5, 2]] })] }],
            runtime: json!({"sourceRevision":SOURCE_REVISION,"checkpointSha256":CHECKPOINT_SHA256}),
        };
        assert!(r.validate(std::slice::from_ref(&o), 1, [4, 4]).is_ok());
        r.tracks[0].object.points[0].position[0] += 1e-16;
        assert!(r.validate(std::slice::from_ref(&o), 1, [4, 4]).is_ok());
        r.tracks[0].masks[0] = Some(Mask { runs: vec![[20, 1]] });
        assert!(r.validate(std::slice::from_ref(&o), 1, [4, 4]).is_err());
        assert!(r.validate(&[o], 1, [usize::MAX, 1]).is_err());
    }
    #[test]
    fn display_boundary_preserves_exact_mask_coverage() {
        let mask = Mask { runs: vec![[0, 9]] };
        let display = mask.display_pixels([3, 3]).unwrap();
        assert_eq!(display, vec![2, 2, 2, 2, 1, 2, 2, 2, 2]);
        assert_eq!(Mask { runs: vec![[4, 1]] }.display_pixels([3, 3]).unwrap(), vec![0, 0, 0, 0, 2, 0, 0, 0, 0]);
        assert!(Mask { runs: vec![[8, 2]] }.display_pixels([3, 3]).is_err());
    }
    #[test]
    fn prompts_are_bounded() {
        let mut o = Object { name: "Sun".into(), frame_index: 0, points: vec![Prompt { position: [0.5, 0.5], foreground: true }] };
        assert!(validate_objects(&[o.clone()], 1).is_ok());
        o.points[0].position[0] = f64::NAN;
        assert!(validate_objects(&[o], 1).is_err());
    }
}

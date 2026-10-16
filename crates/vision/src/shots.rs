//! Shot boundaries, held frames and black frames from small per-frame signatures. Pure Rust, no model.
use serde::{Deserialize, Serialize};

pub const SIGNATURE_WIDTH: usize = 16;
pub const SIGNATURE_HEIGHT: usize = 9;
pub const HISTOGRAM_BINS: usize = 16;
pub const MAX_FRAMES: usize = 36_000;
pub const MAX_SIDE: usize = 4096;
const WINDOW: usize = 8;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Signature {
    pub luma: Vec<u8>,
    pub histogram: Vec<u32>,
    pub pixels: u32,
    pub mean_luma: f32,
    pub luma_spread: f32,
    pub mean_saturation: f32,
    pub warm_bias: f32,
}

fn luma_of(r: u8, g: u8, b: u8) -> f32 {
    (0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b)) / 255.0
}

pub fn signature(width: usize, height: usize, rgba: &[u8]) -> Result<Signature, String> {
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return Err("Frame dimensions must be 1–4096 pixels".into());
    }
    let expected = width.checked_mul(height).and_then(|n| n.checked_mul(4)).ok_or("Frame size overflow")?;
    if rgba.len() != expected {
        return Err("Frame buffer does not match its dimensions".into());
    }
    let total = width * height;
    let stride = {
        let mut s = 1usize;
        while total / (s * s) > 65_536 {
            s += 1;
        }
        s
    };
    let mut histogram = vec![0u32; HISTOGRAM_BINS * 3];
    let (mut count, mut sum, mut sum_sq, mut sat, mut warm) = (0u32, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let mut y = 0;
    while y < height {
        let mut x = 0;
        while x < width {
            let i = (y * width + x) * 4;
            let (r, g, b) = (rgba.get(i).copied().unwrap_or(0), rgba.get(i + 1).copied().unwrap_or(0), rgba.get(i + 2).copied().unwrap_or(0));
            let bin = |v: u8| usize::from(v) * HISTOGRAM_BINS / 256;
            for (channel, value) in [r, g, b].into_iter().enumerate() {
                if let Some(slot) = histogram.get_mut(channel * HISTOGRAM_BINS + bin(value)) {
                    *slot = slot.saturating_add(1);
                }
            }
            let l = f64::from(luma_of(r, g, b));
            sum += l;
            sum_sq += l * l;
            let max = r.max(g).max(b);
            let min = r.min(g).min(b);
            if max > 0 {
                sat += f64::from(max - min) / f64::from(max);
            }
            warm += (f64::from(r) - f64::from(b)) / 255.0;
            count = count.saturating_add(1);
            x += stride;
        }
        y += stride;
    }
    let n = f64::from(count.max(1));
    let mean = sum / n;
    let variance = (sum_sq / n - mean * mean).max(0.0);
    let mut luma = Vec::with_capacity(SIGNATURE_WIDTH * SIGNATURE_HEIGHT);
    for cy in 0..SIGNATURE_HEIGHT {
        let y0 = cy * height / SIGNATURE_HEIGHT;
        let y1 = ((cy + 1) * height / SIGNATURE_HEIGHT).max(y0 + 1).min(height);
        let y0 = y0.min(y1 - 1);
        for cx in 0..SIGNATURE_WIDTH {
            let x0 = cx * width / SIGNATURE_WIDTH;
            let x1 = ((cx + 1) * width / SIGNATURE_WIDTH).max(x0 + 1).min(width);
            let x0 = x0.min(x1 - 1);
            let mut acc = 0.0f64;
            let mut cells = 0u32;
            for yy in y0..y1 {
                for xx in x0..x1 {
                    let i = (yy * width + xx) * 4;
                    let (r, g, b) = (rgba.get(i).copied().unwrap_or(0), rgba.get(i + 1).copied().unwrap_or(0), rgba.get(i + 2).copied().unwrap_or(0));
                    acc += f64::from(luma_of(r, g, b));
                    cells = cells.saturating_add(1);
                }
            }
            let value = (acc / f64::from(cells.max(1)) * 255.0).round().clamp(0.0, 255.0) as u8;
            luma.push(value);
        }
    }
    Ok(Signature {
        luma,
        histogram,
        pixels: count,
        mean_luma: mean as f32,
        luma_spread: variance.sqrt() as f32,
        mean_saturation: (sat / n) as f32,
        warm_bias: (warm / n) as f32,
    })
}

impl Signature {
    pub fn validate(&self) -> Result<(), String> {
        if self.luma.len() != SIGNATURE_WIDTH * SIGNATURE_HEIGHT
            || self.histogram.len() != HISTOGRAM_BINS * 3
            || self.pixels == 0
            || [self.mean_luma, self.luma_spread, self.mean_saturation].iter().any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
            || !self.warm_bias.is_finite()
            || !(-1.0..=1.0).contains(&self.warm_bias)
        {
            return Err("Invalid frame signature".into());
        }
        Ok(())
    }
    pub fn histogram_distance(&self, other: &Signature) -> f32 {
        let mut distance = 0.0f64;
        for channel in 0..3 {
            let a: u64 = self.histogram.iter().skip(channel * HISTOGRAM_BINS).take(HISTOGRAM_BINS).map(|v| u64::from(*v)).sum();
            let b: u64 = other.histogram.iter().skip(channel * HISTOGRAM_BINS).take(HISTOGRAM_BINS).map(|v| u64::from(*v)).sum();
            for bin in 0..HISTOGRAM_BINS {
                let x = f64::from(self.histogram.get(channel * HISTOGRAM_BINS + bin).copied().unwrap_or(0)) / a.max(1) as f64;
                let y = f64::from(other.histogram.get(channel * HISTOGRAM_BINS + bin).copied().unwrap_or(0)) / b.max(1) as f64;
                distance += (x - y).abs();
            }
        }
        (distance / 6.0).clamp(0.0, 1.0) as f32
    }
    pub fn pixel_difference(&self, other: &Signature) -> f32 {
        let total: u64 = self.luma.iter().zip(&other.luma).map(|(a, b)| u64::from(a.abs_diff(*b))).sum();
        (total as f64 / (self.luma.len().max(1) as f64 * 255.0)).clamp(0.0, 1.0) as f32
    }
    pub fn max_cell_difference(&self, other: &Signature) -> f32 {
        let largest = self.luma.iter().zip(&other.luma).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
        f32::from(largest) / 255.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Options {
    pub cut_threshold: f32,
    pub adaptive_ratio: f32,
    pub min_shot_frames: usize,
    pub black_luma: f32,
    pub duplicate_tolerance: f32,
}
impl Default for Options {
    fn default() -> Self {
        Self { cut_threshold: 0.3, adaptive_ratio: 3.0, min_shot_frames: 3, black_luma: 0.04, duplicate_tolerance: 0.004 }
    }
}
impl Options {
    pub fn validate(&self) -> Result<(), String> {
        if !self.cut_threshold.is_finite()
            || !(0.01..=1.0).contains(&self.cut_threshold)
            || !self.adaptive_ratio.is_finite()
            || !(1.0..=100.0).contains(&self.adaptive_ratio)
            || !(1..=1000).contains(&self.min_shot_frames)
            || !self.black_luma.is_finite()
            || !(0.0..=0.5).contains(&self.black_luma)
            || !self.duplicate_tolerance.is_finite()
            || !(0.0..=0.5).contains(&self.duplicate_tolerance)
        {
            return Err("Shot options: cut 0.01–1, ratio 1–100, minimum 1–1000 frames, black 0–0.5, duplicate 0–0.5".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameChange {
    pub frame_index: usize,
    pub timeline_tick: i64,
    pub histogram_distance: Option<f32>,
    pub pixel_difference: Option<f32>,
    #[serde(default)]
    pub max_cell_difference: Option<f32>,
    pub mean_luma: f32,
    pub luma_spread: f32,
    pub mean_saturation: f32,
    pub warm_bias: f32,
    pub black: bool,
    pub duplicate: bool,
}

pub fn changes(signatures: &[Signature], ticks: &[i64], options: &Options) -> Result<Vec<FrameChange>, String> {
    options.validate()?;
    if signatures.is_empty() || signatures.len() > MAX_FRAMES || signatures.len() != ticks.len() {
        return Err("Supply 1–36000 frame signatures with matching timeline ticks".into());
    }
    let mut out = Vec::new();
    out.try_reserve_exact(signatures.len()).map_err(|e| e.to_string())?;
    for (i, s) in signatures.iter().enumerate() {
        s.validate()?;
        let tick = *ticks.get(i).ok_or("Missing tick")?;
        if i > 0 && ticks.get(i - 1).is_some_and(|previous| *previous >= tick) {
            return Err("Timeline ticks must increase".into());
        }
        let previous = i.checked_sub(1).and_then(|p| signatures.get(p));
        let histogram_distance = previous.map(|p| p.histogram_distance(s));
        let pixel_difference = previous.map(|p| p.pixel_difference(s));
        let max_cell_difference = previous.map(|p| p.max_cell_difference(s));
        let black = s.mean_luma <= options.black_luma && s.luma_spread <= options.black_luma;
        let duplicate = match (histogram_distance, pixel_difference, max_cell_difference) {
            (Some(h), Some(p), Some(m)) => h <= options.duplicate_tolerance * 4.0 && p <= options.duplicate_tolerance && m <= options.duplicate_tolerance * 8.0,
            _ => false,
        };
        out.push(FrameChange {
            frame_index: i,
            timeline_tick: tick,
            histogram_distance,
            pixel_difference,
            max_cell_difference,
            mean_luma: s.mean_luma,
            luma_spread: s.luma_spread,
            mean_saturation: s.mean_saturation,
            warm_bias: s.warm_bias,
            black,
            duplicate,
        });
    }
    Ok(out)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Boundary {
    RangeStart,
    Cut,
    FromBlack,
    ToBlack,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Shot {
    pub index: usize,
    pub start_frame: usize,
    pub end_frame: usize,
    pub start_tick: i64,
    pub end_tick: i64,
    pub frame_count: usize,
    pub keyframe: usize,
    pub boundary: Boundary,
    pub boundary_score: Option<f32>,
    pub black: bool,
    pub mean_luma: f32,
    pub mean_saturation: f32,
    pub warm_bias: f32,
    pub duplicate_frames: usize,
    pub unique_frames: usize,
    pub hold_fraction: f32,
}

fn score(change: &FrameChange) -> Option<f32> {
    match (change.histogram_distance, change.pixel_difference) {
        (Some(h), Some(p)) => Some(h.max(p)),
        _ => None,
    }
}

fn local_median(changes: &[FrameChange], index: usize) -> f32 {
    let start = index.saturating_sub(WINDOW);
    let end = index.saturating_add(WINDOW).min(changes.len().saturating_sub(1));
    let mut values: Vec<f32> = (start..=end).filter(|i| *i != index).filter_map(|i| changes.get(i).and_then(score)).collect();
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    values.get(values.len() / 2).copied().unwrap_or(0.0)
}

pub fn detect(changes: &[FrameChange], options: &Options) -> Result<Vec<Shot>, String> {
    options.validate()?;
    if changes.is_empty() || changes.len() > MAX_FRAMES {
        return Err("Supply 1–36000 frame changes".into());
    }
    for (i, c) in changes.iter().enumerate() {
        if c.frame_index != i
            || [c.mean_luma, c.luma_spread, c.mean_saturation].iter().any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
            || [c.histogram_distance, c.pixel_difference, c.max_cell_difference].iter().flatten().any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
        {
            return Err("Invalid frame change record".into());
        }
    }
    let mut starts: Vec<(usize, Boundary, Option<f32>)> = vec![(0, Boundary::RangeStart, None)];
    for i in 1..changes.len() {
        let (Some(previous), Some(current)) = (changes.get(i - 1), changes.get(i)) else { continue };
        if previous.black != current.black {
            starts.push((i, if current.black { Boundary::ToBlack } else { Boundary::FromBlack }, score(current)));
            continue;
        }
        if let Some(s) = score(current)
            && s >= options.cut_threshold
            && s >= options.adaptive_ratio * local_median(changes, i)
        {
            starts.push((i, Boundary::Cut, Some(s)));
        }
    }
    let mut spans: Vec<(usize, usize, Boundary, Option<f32>)> = Vec::new();
    for (k, (start, boundary, boundary_score)) in starts.iter().enumerate() {
        let end = starts.get(k + 1).map(|next| next.0 - 1).unwrap_or(changes.len() - 1);
        spans.push((*start, end, *boundary, *boundary_score));
    }
    let mut merged: Vec<(usize, usize, Boundary, Option<f32>)> = Vec::new();
    for span in spans {
        let short = span.1 - span.0 + 1 < options.min_shot_frames;
        match merged.last_mut() {
            Some(previous) if short => previous.1 = span.1,
            Some(previous) if previous.1 - previous.0 + 1 < options.min_shot_frames => {
                previous.1 = span.1;
            }
            _ => merged.push(span),
        }
    }
    let mut shots = Vec::new();
    for (index, (start, end, boundary, boundary_score)) in merged.into_iter().enumerate() {
        let frames = changes.get(start..=end).ok_or("Shot span outside range")?;
        let count = frames.len();
        let n = count.max(1) as f32;
        let duplicate_frames = frames.iter().skip(1).filter(|c| c.duplicate).count();
        shots.push(Shot {
            index,
            start_frame: start,
            end_frame: end,
            start_tick: frames.first().map(|c| c.timeline_tick).ok_or("Empty shot")?,
            end_tick: frames.last().map(|c| c.timeline_tick).ok_or("Empty shot")?,
            frame_count: count,
            keyframe: start + count / 2,
            boundary,
            boundary_score,
            black: frames.iter().all(|c| c.black),
            mean_luma: frames.iter().map(|c| c.mean_luma).sum::<f32>() / n,
            mean_saturation: frames.iter().map(|c| c.mean_saturation).sum::<f32>() / n,
            warm_bias: frames.iter().map(|c| c.warm_bias).sum::<f32>() / n,
            duplicate_frames,
            unique_frames: count.saturating_sub(duplicate_frames),
            hold_fraction: duplicate_frames as f32 / count.saturating_sub(1).max(1) as f32,
        });
    }
    Ok(shots)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(w: usize, h: usize, color: [u8; 3]) -> Vec<u8> {
        let mut px = Vec::with_capacity(w * h * 4);
        for _ in 0..w * h {
            px.extend_from_slice(&[color[0], color[1], color[2], 255]);
        }
        px
    }
    fn noisy(w: usize, h: usize, color: [u8; 3], seed: u32) -> Vec<u8> {
        let mut px = frame(w, h, color);
        for (i, chunk) in px.chunks_mut(4).enumerate() {
            let mut x = (i as u32) ^ seed.wrapping_mul(0x9E37_79B9);
            x ^= x >> 16;
            x = x.wrapping_mul(0x7feb_352d);
            x ^= x >> 15;
            x = x.wrapping_mul(0x846c_a68b);
            x ^= x >> 16;
            let n = (x >> 24) as u8 / 8;
            chunk[0] = chunk[0].saturating_add(n);
            chunk[1] = chunk[1].saturating_add(n);
        }
        px
    }
    #[test]
    fn signature_rejects_hostile_dimensions() {
        assert!(signature(0, 4, &[]).is_err());
        assert!(signature(4, 4, &[0; 3]).is_err());
        assert!(signature(5000, 1, &[]).is_err());
        let s = signature(3, 2, &frame(3, 2, [255, 0, 0])).unwrap();
        assert_eq!(s.luma.len(), SIGNATURE_WIDTH * SIGNATURE_HEIGHT);
        assert!(s.warm_bias > 0.9);
    }
    #[test]
    fn cuts_black_runs_and_holds_are_detected() {
        let (w, h) = (64, 36);
        let mut frames = Vec::new();
        for i in 0..10 {
            frames.push(noisy(w, h, [20, 60, 120], i));
        }
        for _ in 0..3 {
            frames.push(frame(w, h, [0, 0, 0]));
        }
        for i in 0..12 {
            frames.push(noisy(w, h, [200, 150, 40], if i % 3 == 0 { i } else { i - i % 3 }));
        }
        let signatures: Vec<_> = frames.iter().map(|f| signature(w, h, f).unwrap()).collect();
        let ticks: Vec<i64> = (0..signatures.len() as i64).map(|i| i * 10).collect();
        let options = Options::default();
        let changes = changes(&signatures, &ticks, &options).unwrap();
        let shots = detect(&changes, &options).unwrap();
        assert_eq!(shots.len(), 3, "{shots:?}");
        assert_eq!(shots[0].boundary, Boundary::RangeStart);
        assert_eq!((shots[1].start_frame, shots[1].end_frame, shots[1].boundary), (10, 12, Boundary::ToBlack));
        assert!(shots[1].black);
        assert_eq!((shots[2].start_frame, shots[2].boundary), (13, Boundary::FromBlack));
        assert!(shots[2].warm_bias > shots[0].warm_bias);
        assert_eq!(shots[2].duplicate_frames, 8, "two held frames after every drawn frame");
        assert_eq!(shots[2].unique_frames, 4);
        assert_eq!(shots[0].duplicate_frames, 0);
        assert_eq!(shots[2].end_tick, 240);
    }
    #[test]
    fn hard_cut_without_black_uses_adaptive_threshold() {
        let (w, h) = (32, 18);
        let mut frames: Vec<Vec<u8>> = (0..8).map(|i| noisy(w, h, [30, 30, 30], i)).collect();
        frames.extend((0..8).map(|i| noisy(w, h, [220, 220, 220], i)));
        let signatures: Vec<_> = frames.iter().map(|f| signature(w, h, f).unwrap()).collect();
        let ticks: Vec<i64> = (0..16).collect();
        let changes = changes(&signatures, &ticks, &Options::default()).unwrap();
        let shots = detect(&changes, &Options::default()).unwrap();
        assert_eq!(shots.len(), 2);
        assert_eq!((shots[1].start_frame, shots[1].boundary), (8, Boundary::Cut));
        assert!(shots[1].boundary_score.unwrap() > 0.3);
        let short = Options { min_shot_frames: 9, ..Options::default() };
        assert_eq!(detect(&changes, &short).unwrap().len(), 1, "short shots merge into their predecessor");
    }
    #[test]
    fn hostile_inputs_are_errors() {
        let s = signature(4, 4, &frame(4, 4, [1, 2, 3])).unwrap();
        assert!(changes(std::slice::from_ref(&s), &[0, 1], &Options::default()).is_err());
        assert!(changes(&[s.clone(), s.clone()], &[1, 0], &Options::default()).is_err());
        assert!(changes(std::slice::from_ref(&s), &[0], &Options { cut_threshold: f32::NAN, ..Options::default() }).is_err());
        assert!(detect(&[], &Options::default()).is_err());
        let mut bad = changes(&[s], &[0], &Options::default()).unwrap();
        bad[0].mean_luma = 7.0;
        assert!(detect(&bad, &Options::default()).is_err());
        let mut broken = Signature { luma: vec![], histogram: vec![], pixels: 0, mean_luma: 0.0, luma_spread: 0.0, mean_saturation: 0.0, warm_bias: 0.0 };
        assert!(broken.validate().is_err());
        broken.luma = vec![0; SIGNATURE_WIDTH * SIGNATURE_HEIGHT];
        broken.histogram = vec![0; HISTOGRAM_BINS * 3];
        assert!(broken.validate().is_err());
    }
}

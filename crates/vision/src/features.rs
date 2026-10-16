//! Multi-scale Harris feature points with sub-pixel position and dominant orientation. Pure Rust, no model.
//! Written from the published method (Harris & Stephens 1988; orientation assignment after Lowe 2004).
use serde::{Deserialize, Serialize};

pub const MAX_SIDE: usize = 4096;
pub const MAX_POINTS: usize = 5000;
const HARRIS_K: f32 = 0.04;
const BASE_SIGMA: f32 = 1.5;
const DERIVATIVE_SIGMA: f32 = 0.8;
const INTEGRATION_SIGMA: f32 = 1.0;
const ORIENTATION_BINS: usize = 36;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Options {
    pub max_points: usize,
    pub min_distance: f32,
    pub threshold: f32,
    pub octaves: usize,
}
impl Default for Options {
    fn default() -> Self {
        Self { max_points: 500, min_distance: 8.0, threshold: 0.001, octaves: 4 }
    }
}
impl Options {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=MAX_POINTS).contains(&self.max_points)
            || !self.min_distance.is_finite()
            || !(0.0..=256.0).contains(&self.min_distance)
            || !self.threshold.is_finite()
            || !(0.0001..=1.0).contains(&self.threshold)
            || !(1..=6).contains(&self.octaves)
        {
            return Err("Feature options: 1–5000 points, distance 0–256 px, threshold 0.0001–1, octaves 1–6".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Feature {
    pub x: f32,
    pub y: f32,
    pub scale: f32,
    pub orientation: f32,
    pub response: f32,
    pub octave: usize,
}

pub(crate) struct Plane {
    pub(crate) w: usize,
    pub(crate) h: usize,
    pub(crate) px: Vec<f32>,
}
impl Plane {
    pub(crate) fn at(&self, x: usize, y: usize) -> f32 {
        self.px.get(y * self.w + x).copied().unwrap_or(0.0)
    }
    pub(crate) fn clamped(&self, x: isize, y: isize) -> f32 {
        let x = x.clamp(0, self.w as isize - 1) as usize;
        let y = y.clamp(0, self.h as isize - 1) as usize;
        self.at(x, y)
    }
    /// Bilinear sample; zero outside the image.
    pub(crate) fn sample(&self, x: f32, y: f32) -> f32 {
        if !x.is_finite() || !y.is_finite() || x < 0.0 || y < 0.0 || x > self.w as f32 - 1.0 || y > self.h as f32 - 1.0 {
            return 0.0;
        }
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let x1 = (x0 + 1).min(self.w - 1);
        let y1 = (y0 + 1).min(self.h - 1);
        self.at(x0, y0) * (1.0 - fx) * (1.0 - fy) + self.at(x1, y0) * fx * (1.0 - fy) + self.at(x0, y1) * (1.0 - fx) * fy + self.at(x1, y1) * fx * fy
    }
    pub(crate) fn gradients(&self) -> (Plane, Plane) {
        let (w, h) = (self.w, self.h);
        let mut gx = Plane { w, h, px: vec![0.0; w * h] };
        let mut gy = Plane { w, h, px: vec![0.0; w * h] };
        for y in 0..h {
            for x in 0..w {
                let dx = (self.clamped(x as isize + 1, y as isize) - self.clamped(x as isize - 1, y as isize)) * 0.5;
                let dy = (self.clamped(x as isize, y as isize + 1) - self.clamped(x as isize, y as isize - 1)) * 0.5;
                if let (Some(a), Some(b)) = (gx.px.get_mut(y * w + x), gy.px.get_mut(y * w + x)) {
                    *a = dx;
                    *b = dy;
                }
            }
        }
        (gx, gy)
    }
    pub(crate) fn blur(&self, sigma: f32) -> Plane {
        let radius = (sigma * 3.0).ceil().max(1.0) as isize;
        let kernel: Vec<f32> = (-radius..=radius).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
        let sum: f32 = kernel.iter().sum();
        let kernel: Vec<f32> = kernel.iter().map(|k| k / sum.max(1e-12)).collect();
        let mut horizontal = Plane { w: self.w, h: self.h, px: vec![0.0; self.w * self.h] };
        for y in 0..self.h {
            for x in 0..self.w {
                let mut acc = 0.0;
                for (k, weight) in kernel.iter().enumerate() {
                    acc += weight * self.clamped(x as isize + k as isize - radius, y as isize);
                }
                if let Some(slot) = horizontal.px.get_mut(y * self.w + x) {
                    *slot = acc;
                }
            }
        }
        let mut out = Plane { w: self.w, h: self.h, px: vec![0.0; self.w * self.h] };
        for y in 0..self.h {
            for x in 0..self.w {
                let mut acc = 0.0;
                for (k, weight) in kernel.iter().enumerate() {
                    acc += weight * horizontal.clamped(x as isize, y as isize + k as isize - radius);
                }
                if let Some(slot) = out.px.get_mut(y * self.w + x) {
                    *slot = acc;
                }
            }
        }
        out
    }
    fn half(&self) -> Option<Plane> {
        let (w, h) = (self.w / 2, self.h / 2);
        if w < 16 || h < 16 {
            return None;
        }
        let mut px = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                px.push((self.at(2 * x, 2 * y) + self.at(2 * x + 1, 2 * y) + self.at(2 * x, 2 * y + 1) + self.at(2 * x + 1, 2 * y + 1)) * 0.25);
            }
        }
        Some(Plane { w, h, px })
    }
}

/// Luma (0–1) of an RGBA8 frame, row-major, for the matching and camera steps.
pub fn luma(width: usize, height: usize, rgba: &[u8]) -> Result<Vec<f32>, String> {
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return Err("Frame dimensions must be 1–4096 pixels".into());
    }
    let expected = width.checked_mul(height).and_then(|n| n.checked_mul(4)).ok_or("Frame size overflow")?;
    if rgba.len() != expected {
        return Err("Frame buffer does not match its dimensions".into());
    }
    Ok(luma_plane(width, height, rgba).px)
}

pub(crate) fn luma_plane(width: usize, height: usize, rgba: &[u8]) -> Plane {
    let mut px = Vec::with_capacity(width * height);
    for chunk in rgba.chunks_exact(4).take(width * height) {
        px.push((0.2126 * f32::from(chunk[0]) + 0.7152 * f32::from(chunk[1]) + 0.0722 * f32::from(chunk[2])) / 255.0);
    }
    px.resize(width * height, 0.0);
    Plane { w: width, h: height, px }
}

struct Candidate {
    x: f32,
    y: f32,
    response: f32,
    octave: usize,
    orientation: f32,
}

fn parabola_offset(left: f32, centre: f32, right: f32) -> f32 {
    let denominator = left - 2.0 * centre + right;
    if denominator.abs() < 1e-12 {
        return 0.0;
    }
    (0.5 * (left - right) / denominator).clamp(-0.5, 0.5)
}

fn orientation(gx: &Plane, gy: &Plane, x: usize, y: usize, radius: isize) -> f32 {
    let mut bins = [0.0f32; ORIENTATION_BINS];
    let sigma = radius as f32 / 2.0;
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            let (xx, yy) = (x as isize + dx, y as isize + dy);
            if xx < 0 || yy < 0 || xx >= gx.w as isize || yy >= gx.h as isize {
                continue;
            }
            let (ix, iy) = (gx.at(xx as usize, yy as usize), gy.at(xx as usize, yy as usize));
            let magnitude = ix.hypot(iy);
            if magnitude <= 0.0 {
                continue;
            }
            let weight = (-((dx * dx + dy * dy) as f32) / (2.0 * sigma * sigma)).exp();
            let angle = iy.atan2(ix).rem_euclid(std::f32::consts::TAU);
            let bin = ((angle / std::f32::consts::TAU) * ORIENTATION_BINS as f32).floor() as usize % ORIENTATION_BINS;
            if let Some(slot) = bins.get_mut(bin) {
                *slot += magnitude * weight;
            }
        }
    }
    let best = bins.iter().enumerate().fold(0usize, |b, (i, v)| if *v > bins.get(b).copied().unwrap_or(0.0) { i } else { b });
    let left = bins.get((best + ORIENTATION_BINS - 1) % ORIENTATION_BINS).copied().unwrap_or(0.0);
    let centre = bins.get(best).copied().unwrap_or(0.0);
    let right = bins.get((best + 1) % ORIENTATION_BINS).copied().unwrap_or(0.0);
    let offset = parabola_offset(left, centre, right);
    ((best as f32 + 0.5 + offset) / ORIENTATION_BINS as f32 * std::f32::consts::TAU).rem_euclid(std::f32::consts::TAU)
}

fn harris_response(plane: &Plane) -> (Plane, Plane, Plane) {
    let smooth = plane.blur(DERIVATIVE_SIGMA);
    let (w, h) = (smooth.w, smooth.h);
    let mut gx = Plane { w, h, px: vec![0.0; w * h] };
    let mut gy = Plane { w, h, px: vec![0.0; w * h] };
    for y in 0..h {
        for x in 0..w {
            let dx = (smooth.clamped(x as isize + 1, y as isize) - smooth.clamped(x as isize - 1, y as isize)) * 0.5;
            let dy = (smooth.clamped(x as isize, y as isize + 1) - smooth.clamped(x as isize, y as isize - 1)) * 0.5;
            if let (Some(a), Some(b)) = (gx.px.get_mut(y * w + x), gy.px.get_mut(y * w + x)) {
                *a = dx;
                *b = dy;
            }
        }
    }
    let product = |a: &Plane, b: &Plane| Plane { w, h, px: a.px.iter().zip(&b.px).map(|(p, q)| p * q).collect() };
    let (xx, yy, xy) = (product(&gx, &gx).blur(INTEGRATION_SIGMA), product(&gy, &gy).blur(INTEGRATION_SIGMA), product(&gx, &gy).blur(INTEGRATION_SIGMA));
    let mut response = Plane { w, h, px: vec![0.0; w * h] };
    for i in 0..w * h {
        let (a, b, c) = (xx.px.get(i).copied().unwrap_or(0.0), yy.px.get(i).copied().unwrap_or(0.0), xy.px.get(i).copied().unwrap_or(0.0));
        if let Some(slot) = response.px.get_mut(i) {
            *slot = (a * b - c * c) - HARRIS_K * (a + b) * (a + b);
        }
    }
    (response, gx, gy)
}

fn is_local_maximum(response: &Plane, x: usize, y: usize) -> bool {
    let r = response.at(x, y);
    if r <= 0.0 {
        return false;
    }
    for dy in -1isize..=1 {
        for dx in -1isize..=1 {
            if (dx, dy) != (0, 0) && response.clamped(x as isize + dx, y as isize + dy) >= r {
                return false;
            }
        }
    }
    true
}

/// A coarse-octave peak sits several pixels inside its corner; the fine response says where the corner is.
fn refine_at_fine_scale(fine: &Plane, x: f32, y: f32, octave: usize) -> Option<(f32, f32)> {
    let radius = (1isize << octave.min(4)).max(1);
    let (cx, cy) = (x.round() as isize, y.round() as isize);
    let mut best: Option<(f32, usize, usize)> = None;
    for yy in (cy - radius).max(1)..=(cy + radius).min(fine.h as isize - 2) {
        for xx in (cx - radius).max(1)..=(cx + radius).min(fine.w as isize - 2) {
            let (xu, yu) = (xx as usize, yy as usize);
            if is_local_maximum(fine, xu, yu) {
                let r = fine.at(xu, yu);
                if best.is_none_or(|b| r > b.0) {
                    best = Some((r, xu, yu));
                }
            }
        }
    }
    let (_, xu, yu) = best?;
    let ox = parabola_offset(fine.at(xu - 1, yu), fine.at(xu, yu), fine.at(xu + 1, yu));
    let oy = parabola_offset(fine.at(xu, yu - 1), fine.at(xu, yu), fine.at(xu, yu + 1));
    Some((xu as f32 + ox, yu as f32 + oy))
}

fn octave_candidates(plane: &Plane, octave: usize, fine: &Plane, out: &mut Vec<Candidate>) {
    let (response, gx, gy) = harris_response(plane);
    let (w, h) = (response.w, response.h);
    let border = 3usize;
    let scale = (1usize << octave) as f32;
    for y in border..h.saturating_sub(border) {
        for x in border..w.saturating_sub(border) {
            if !is_local_maximum(&response, x, y) {
                continue;
            }
            let r = response.at(x, y);
            let ox = parabola_offset(response.at(x - 1, y), r, response.at(x + 1, y));
            let oy = parabola_offset(response.at(x, y - 1), r, response.at(x, y + 1));
            let (mut px, mut py) = ((x as f32 + ox + 0.5) * scale - 0.5, (y as f32 + oy + 0.5) * scale - 0.5);
            if octave > 0
                && let Some((fx, fy)) = refine_at_fine_scale(fine, px, py, octave)
            {
                px = fx;
                py = fy;
            }
            out.push(Candidate { x: px, y: py, response: r, octave, orientation: orientation(&gx, &gy, x, y, 4) });
        }
    }
}

pub fn detect(width: usize, height: usize, rgba: &[u8], options: &Options) -> Result<Vec<Feature>, String> {
    options.validate()?;
    if width < 16 || height < 16 || width > MAX_SIDE || height > MAX_SIDE {
        return Err("Frame dimensions must be 16–4096 pixels".into());
    }
    let expected = width.checked_mul(height).and_then(|n| n.checked_mul(4)).ok_or("Frame size overflow")?;
    if rgba.len() != expected {
        return Err("Frame buffer does not match its dimensions".into());
    }
    let mut candidates = Vec::new();
    let mut plane = luma_plane(width, height, rgba);
    let (fine, _, _) = harris_response(&plane);
    for octave in 0..options.octaves {
        octave_candidates(&plane, octave, &fine, &mut candidates);
        match plane.half() {
            Some(next) => plane = next,
            None => break,
        }
    }
    let peak = candidates.iter().map(|c| c.response).fold(0.0f32, f32::max);
    if peak <= 0.0 {
        return Ok(Vec::new());
    }
    let cutoff = peak * options.threshold;
    candidates.retain(|c| c.response >= cutoff && c.x.is_finite() && c.y.is_finite());
    candidates.sort_by(|a, b| b.response.partial_cmp(&a.response).unwrap_or(std::cmp::Ordering::Equal));
    let cell = options.min_distance.max(1.0);
    let mut accepted: Vec<Feature> = Vec::new();
    let mut grid: std::collections::HashMap<(i64, i64), Vec<usize>> = std::collections::HashMap::new();
    for c in candidates {
        if accepted.len() >= options.max_points {
            break;
        }
        let (gx, gy) = ((c.x / cell).floor() as i64, (c.y / cell).floor() as i64);
        let mut blocked = false;
        'cells: for ny in gy - 1..=gy + 1 {
            for nx in gx - 1..=gx + 1 {
                if let Some(indices) = grid.get(&(nx, ny)) {
                    for i in indices {
                        if let Some(f) = accepted.get(*i)
                            && (f.x - c.x).hypot(f.y - c.y) < options.min_distance
                        {
                            blocked = true;
                            break 'cells;
                        }
                    }
                }
            }
        }
        if blocked {
            continue;
        }
        grid.entry((gx, gy)).or_default().push(accepted.len());
        accepted.push(Feature {
            x: c.x,
            y: c.y,
            scale: BASE_SIGMA * (1usize << c.octave) as f32,
            orientation: c.orientation,
            response: c.response / peak,
            octave: c.octave,
        });
    }
    Ok(accepted)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn shapes(w: usize, h: usize) -> (Vec<u8>, Vec<(f32, f32)>) {
        let mut px = vec![0u8; w * h * 4];
        let mut corners = Vec::new();
        let mut fill = |x0: usize, y0: usize, x1: usize, y1: usize, v: u8| {
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = (y * w + x) * 4;
                    px[i] = v;
                    px[i + 1] = v;
                    px[i + 2] = v;
                    px[i + 3] = 255;
                }
            }
        };
        fill(0, 0, w, h, 40);
        for (x0, y0, x1, y1, v) in [(20, 20, 60, 50, 220), (80, 30, 110, 90, 150), (30, 70, 70, 110, 90)] {
            fill(x0, y0, x1, y1, v);
            corners.extend([
                (x0 as f32 - 0.5, y0 as f32 - 0.5),
                (x1 as f32 - 0.5, y0 as f32 - 0.5),
                (x0 as f32 - 0.5, y1 as f32 - 0.5),
                (x1 as f32 - 0.5, y1 as f32 - 0.5),
            ]);
        }
        (px, corners)
    }
    fn warp(src: &[u8], w: usize, h: usize, angle: f32, scale: f32) -> (Vec<u8>, usize, usize, impl Fn(f32, f32) -> (f32, f32)) {
        let (ow, oh) = ((w as f32 * scale * 1.6) as usize, (h as f32 * scale * 1.6) as usize);
        let (cx, cy, ocx, ocy) = (w as f32 / 2.0, h as f32 / 2.0, ow as f32 / 2.0, oh as f32 / 2.0);
        let (sin, cos) = angle.sin_cos();
        let forward = move |x: f32, y: f32| {
            let (dx, dy) = (x - cx, y - cy);
            (ocx + scale * (cos * dx - sin * dy), ocy + scale * (sin * dx + cos * dy))
        };
        let mut out = vec![0u8; ow * oh * 4];
        for y in 0..oh {
            for x in 0..ow {
                let (dx, dy) = ((x as f32 - ocx) / scale, (y as f32 - ocy) / scale);
                let sx = cx + cos * dx + sin * dy;
                let sy = cy - sin * dx + cos * dy;
                let v = if sx >= 0.0 && sy >= 0.0 && sx < w as f32 - 1.0 && sy < h as f32 - 1.0 {
                    let (x0, y0) = (sx.floor() as usize, sy.floor() as usize);
                    let (fx, fy) = (sx - x0 as f32, sy - y0 as f32);
                    let sample = |xx: usize, yy: usize| f32::from(src[(yy * w + xx) * 4]);
                    sample(x0, y0) * (1.0 - fx) * (1.0 - fy)
                        + sample(x0 + 1, y0) * fx * (1.0 - fy)
                        + sample(x0, y0 + 1) * (1.0 - fx) * fy
                        + sample(x0 + 1, y0 + 1) * fx * fy
                } else {
                    40.0
                };
                let i = (y * ow + x) * 4;
                out[i] = v.round() as u8;
                out[i + 1] = v.round() as u8;
                out[i + 2] = v.round() as u8;
                out[i + 3] = 255;
            }
        }
        (out, ow, oh, forward)
    }
    #[test]
    fn corners_of_rectangles_are_found_and_flat_areas_are_empty() {
        let (px, corners) = shapes(128, 128);
        let features = detect(128, 128, &px, &Options { max_points: 50, min_distance: 4.0, threshold: 0.002, octaves: 3 }).unwrap();
        assert!(features.len() >= 12, "{}", features.len());
        for c in &corners {
            let nearest = features.iter().map(|f| (f.x - c.0).hypot(f.y - c.1)).fold(f32::MAX, f32::min);
            assert!(nearest < 1.5, "corner {c:?} nearest {nearest}");
        }
        for f in &features {
            let nearest = corners.iter().map(|c| (f.x - c.0).hypot(f.y - c.1)).fold(f32::MAX, f32::min);
            assert!(nearest < 3.0, "feature at ({}, {}) is not near a corner", f.x, f.y);
            assert!((0.0..=1.0).contains(&f.response) && f.orientation.is_finite());
        }
    }
    #[test]
    fn points_survive_rotation_and_scale() {
        let (px, _) = shapes(128, 128);
        let options = Options { max_points: 50, min_distance: 4.0, threshold: 0.002, octaves: 3 };
        let base = detect(128, 128, &px, &options).unwrap();
        let (warped, ow, oh, forward) = warp(&px, 128, 128, 30f32.to_radians(), 1.5);
        let moved = detect(ow, oh, &warped, &options).unwrap();
        let found = base
            .iter()
            .filter(|f| {
                let (tx, ty) = forward(f.x, f.y);
                moved.iter().any(|m| (m.x - tx).hypot(m.y - ty) < 1.5)
            })
            .count();
        assert!(found as f32 >= base.len() as f32 * 0.85, "{found} of {}", base.len());
    }
    #[test]
    fn options_limit_count_and_spacing() {
        let (px, _) = shapes(128, 128);
        let few = detect(128, 128, &px, &Options { max_points: 3, ..Options::default() }).unwrap();
        assert_eq!(few.len(), 3);
        let spaced = detect(128, 128, &px, &Options { min_distance: 40.0, max_points: 50, ..Options::default() }).unwrap();
        for (i, a) in spaced.iter().enumerate() {
            for b in spaced.iter().skip(i + 1) {
                assert!((a.x - b.x).hypot(a.y - b.y) >= 40.0);
            }
        }
        let flat = detect(64, 64, &vec![128; 64 * 64 * 4], &Options::default()).unwrap();
        assert!(flat.is_empty());
    }
    #[test]
    fn hostile_inputs_are_errors() {
        assert!(detect(8, 8, &[0; 256], &Options::default()).is_err());
        assert!(detect(64, 64, &[0; 10], &Options::default()).is_err());
        assert!(detect(64, 64, &vec![0; 64 * 64 * 4], &Options { octaves: 0, ..Options::default() }).is_err());
        assert!(detect(64, 64, &vec![0; 64 * 64 * 4], &Options { threshold: f32::NAN, ..Options::default() }).is_err());
        assert!(detect(5000, 1, &[], &Options::default()).is_err());
    }
}

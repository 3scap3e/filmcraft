//! Rotation-aware gradient-histogram descriptors and ratio-tested matching between two frames.
//! Pure Rust, clean-room after Lowe 2004. Descriptors are working data and are never serialized.
use crate::features::{Feature, Plane, luma_plane};
use serde::{Deserialize, Serialize};

pub const DESCRIPTOR_LEN: usize = 128;
const GRID: usize = 4;
const SAMPLES: usize = 16;
const BINS: usize = 8;
const CLIP: f32 = 0.2;

#[derive(Clone, Debug, PartialEq)]
pub struct Descriptor(pub [f32; DESCRIPTOR_LEN]);

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MatchOptions {
    pub ratio: f32,
    pub cross_check: bool,
    pub max_distance_px: f32,
}
impl Default for MatchOptions {
    fn default() -> Self {
        Self { ratio: 0.75, cross_check: true, max_distance_px: 0.0 }
    }
}
impl MatchOptions {
    pub fn validate(&self) -> Result<(), String> {
        if !self.ratio.is_finite()
            || !(0.5..=0.95).contains(&self.ratio)
            || !self.max_distance_px.is_finite()
            || !(0.0..=4096.0).contains(&self.max_distance_px)
        {
            return Err("Match options: ratio 0.5–0.95, search window 0–4096 px".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Match {
    pub a: usize,
    pub b: usize,
    pub distance: f32,
    pub ratio: f32,
}

fn describe_one(gx: &Plane, gy: &Plane, f: &Feature) -> Descriptor {
    let mut hist = [0.0f32; DESCRIPTOR_LEN];
    let spacing = f.scale.max(1.0);
    let (sin, cos) = f.orientation.sin_cos();
    let window = SAMPLES as f32 * spacing * 0.5;
    let sigma2 = 2.0 * window * window;
    for j in 0..SAMPLES {
        for i in 0..SAMPLES {
            let u = (i as f32 - SAMPLES as f32 / 2.0 + 0.5) * spacing;
            let v = (j as f32 - SAMPLES as f32 / 2.0 + 0.5) * spacing;
            let x = f.x + cos * u - sin * v;
            let y = f.y + sin * u + cos * v;
            let (ix, iy) = (gx.sample(x, y), gy.sample(x, y));
            let magnitude = ix.hypot(iy);
            if magnitude <= 0.0 {
                continue;
            }
            let angle = (iy.atan2(ix) - f.orientation).rem_euclid(std::f32::consts::TAU);
            let weight = magnitude * (-(u * u + v * v) / sigma2).exp();
            let bin = angle / std::f32::consts::TAU * BINS as f32;
            let b0 = bin.floor() as usize % BINS;
            let b1 = (b0 + 1) % BINS;
            let t = bin - bin.floor();
            let cell = (j / (SAMPLES / GRID)) * GRID + i / (SAMPLES / GRID);
            if let Some(slot) = hist.get_mut(cell * BINS + b0) {
                *slot += weight * (1.0 - t);
            }
            if let Some(slot) = hist.get_mut(cell * BINS + b1) {
                *slot += weight * t;
            }
        }
    }
    let norm = hist.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 1e-12 {
        hist.iter_mut().for_each(|v| *v = (*v / norm).min(CLIP));
        let norm = hist.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 1e-12 {
            hist.iter_mut().for_each(|v| *v /= norm);
        }
    }
    Descriptor(hist)
}

/// One descriptor per feature of a frame, in the same order.
pub fn describe(width: usize, height: usize, rgba: &[u8], features: &[Feature]) -> Result<Vec<Descriptor>, String> {
    if width < 16 || height < 16 || width > crate::features::MAX_SIDE || height > crate::features::MAX_SIDE {
        return Err("Frame dimensions must be 16–4096 pixels".into());
    }
    let expected = width.checked_mul(height).and_then(|n| n.checked_mul(4)).ok_or("Frame size overflow")?;
    if rgba.len() != expected {
        return Err("Frame buffer does not match its dimensions".into());
    }
    if features.len() > crate::features::MAX_POINTS {
        return Err("Too many features to describe".into());
    }
    for f in features {
        if !f.x.is_finite() || !f.y.is_finite() || !f.orientation.is_finite() || !f.scale.is_finite() || f.scale <= 0.0 || f.scale > 1024.0 {
            return Err("Invalid feature".into());
        }
    }
    let (gx, gy) = luma_plane(width, height, rgba).blur(1.0).gradients();
    Ok(features.iter().map(|f| describe_one(&gx, &gy, f)).collect())
}

fn squared_distance(a: &Descriptor, b: &Descriptor) -> f32 {
    a.0.iter().zip(&b.0).map(|(x, y)| (x - y) * (x - y)).sum()
}

fn best_two(query: &Descriptor, query_pos: Option<&Feature>, pool: &[Descriptor], pool_pos: &[Feature], window: f32) -> Option<(usize, f32, f32)> {
    let mut best: Option<(usize, f32)> = None;
    let mut second = f32::MAX;
    for (j, candidate) in pool.iter().enumerate() {
        if window > 0.0
            && let (Some(q), Some(p)) = (query_pos, pool_pos.get(j))
            && (q.x - p.x).hypot(q.y - p.y) > window
        {
            continue;
        }
        let d = squared_distance(query, candidate);
        match best {
            Some((_, bd)) if d >= bd => second = second.min(d),
            Some((_, bd)) => {
                second = bd;
                best = Some((j, d));
            }
            None => best = Some((j, d)),
        }
    }
    best.map(|(j, d)| (j, d, second))
}

/// Matches from frame A features to frame B features: Lowe's ratio test on the two nearest
/// descriptors, optional cross-check (B's best must be the same A point), optional spatial window.
pub fn match_features(a: &[Descriptor], a_pos: &[Feature], b: &[Descriptor], b_pos: &[Feature], options: &MatchOptions) -> Result<Vec<Match>, String> {
    options.validate()?;
    if a.len() != a_pos.len() || b.len() != b_pos.len() || a.len() > crate::features::MAX_POINTS || b.len() > crate::features::MAX_POINTS {
        return Err("Descriptor and feature counts must agree and stay within limits".into());
    }
    let ratio2 = options.ratio * options.ratio;
    let mut out = Vec::new();
    for (i, query) in a.iter().enumerate() {
        let Some((j, d, second)) = best_two(query, a_pos.get(i), b, b_pos, options.max_distance_px) else { continue };
        let ratio = if second.is_finite() && second > 0.0 {
            (d / second).sqrt()
        } else if d <= 0.0 {
            0.0
        } else {
            1.0
        };
        if second.is_finite() && d > ratio2 * second {
            continue;
        }
        if options.cross_check
            && let Some(back) = b.get(j)
            && best_two(back, b_pos.get(j), a, a_pos, options.max_distance_px).is_none_or(|(k, _, _)| k != i)
        {
            continue;
        }
        out.push(Match { a: i, b: j, distance: d.sqrt(), ratio });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::{Options, detect};
    fn scene(w: usize, h: usize, seed: u32) -> Vec<u8> {
        let mut px = vec![0u8; w * h * 4];
        let mut state = seed.wrapping_mul(0x9E37_79B9).max(1);
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };
        let base = 40 + (next() % 60) as u8;
        px.chunks_exact_mut(4).for_each(|p| {
            let n = (next() % 9) as u8;
            p[0] = base + n;
            p[1] = base + n;
            p[2] = base + n;
            p[3] = 255;
        });
        for _ in 0..14 {
            let (x0, y0) = ((next() % (w as u32 - 24)) as usize + 4, (next() % (h as u32 - 24)) as usize + 4);
            let (bw, bh) = ((next() % 16) as usize + 6, (next() % 16) as usize + 6);
            let v = (next() % 160) as u8 + 60;
            let (fx, fy) = ((next() % 5) as f32 * 0.3 + 0.4, (next() % 5) as f32 * 0.3 + 0.4);
            for y in y0..(y0 + bh).min(h) {
                for x in x0..(x0 + bw).min(w) {
                    let i = (y * w + x) * 4;
                    let texture = ((x as f32 * fx).sin() * (y as f32 * fy).cos() * 40.0) as i32;
                    let value = (i32::from(v) + texture).clamp(30, 240) as u8;
                    px[i] = value;
                    px[i + 1] = value / 2 + 20;
                    px[i + 2] = 255 - value;
                }
            }
        }
        px
    }
    fn warp(src: &[u8], w: usize, h: usize, angle: f32, scale: f32) -> (Vec<u8>, usize, usize, impl Fn(f32, f32) -> (f32, f32)) {
        let (ow, oh) = ((w as f32 * scale * 1.5) as usize, (h as f32 * scale * 1.5) as usize);
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
                let i = (y * ow + x) * 4;
                for c in 0..3 {
                    let v = if sx >= 0.0 && sy >= 0.0 && sx < w as f32 - 1.0 && sy < h as f32 - 1.0 {
                        let (x0, y0) = (sx.floor() as usize, sy.floor() as usize);
                        let (fx, fy) = (sx - x0 as f32, sy - y0 as f32);
                        let s = |xx: usize, yy: usize| f32::from(src[(yy * w + xx) * 4 + c]);
                        s(x0, y0) * (1.0 - fx) * (1.0 - fy) + s(x0 + 1, y0) * fx * (1.0 - fy) + s(x0, y0 + 1) * (1.0 - fx) * fy + s(x0 + 1, y0 + 1) * fx * fy
                    } else {
                        f32::from(src[c])
                    };
                    out[i + c] = v.round() as u8;
                }
                out[i + 3] = 255;
            }
        }
        (out, ow, oh, forward)
    }
    #[test]
    fn matches_follow_rotation_and_scale_and_reject_unrelated_frames() {
        let (w, h) = (160, 120);
        let px = scene(w, h, 1);
        let options = Options { max_points: 200, min_distance: 4.0, threshold: 0.002, octaves: 3 };
        let a = detect(w, h, &px, &options).unwrap();
        let da = describe(w, h, &px, &a).unwrap();
        let (warped, ow, oh, forward) = warp(&px, w, h, 25f32.to_radians(), 1.15);
        let b = detect(ow, oh, &warped, &options).unwrap();
        let db = describe(ow, oh, &warped, &b).unwrap();
        let matches = match_features(&da, &a, &db, &b, &MatchOptions::default()).unwrap();
        assert!(matches.len() >= 20, "{} matches", matches.len());
        let correct = matches
            .iter()
            .filter(|m| {
                let (tx, ty) = forward(a[m.a].x, a[m.a].y);
                (b[m.b].x - tx).hypot(b[m.b].y - ty) < 2.0
            })
            .count();
        assert!(correct as f32 >= matches.len() as f32 * 0.9, "{correct} of {}", matches.len());
        let other = scene(w, h, 7);
        let c = detect(w, h, &other, &options).unwrap();
        let dc = describe(w, h, &other, &c).unwrap();
        let unrelated = match_features(&da, &a, &dc, &c, &MatchOptions::default()).unwrap();
        assert!(unrelated.len() as f32 <= (a.len().min(c.len()) as f32 * 0.05).max(2.0), "{} unrelated matches of {}", unrelated.len(), a.len());
    }
    #[test]
    fn descriptors_tolerate_brightness_changes_and_windows_limit_search() {
        let (w, h) = (160, 120);
        let px = scene(w, h, 3);
        let dim: Vec<u8> = px
            .chunks_exact(4)
            .flat_map(|p| [(f32::from(p[0]) * 0.6 + 20.0) as u8, (f32::from(p[1]) * 0.6 + 20.0) as u8, (f32::from(p[2]) * 0.6 + 20.0) as u8, 255])
            .collect();
        let options = Options { max_points: 100, min_distance: 6.0, threshold: 0.002, octaves: 2 };
        let a = detect(w, h, &px, &options).unwrap();
        let da = describe(w, h, &px, &a).unwrap();
        let db = describe(w, h, &dim, &a).unwrap();
        for (x, y) in da.iter().zip(&db) {
            assert!(squared_distance(x, y).sqrt() < 0.15);
        }
        let windowed = match_features(&da, &a, &db, &a, &MatchOptions { max_distance_px: 1.0, ..MatchOptions::default() }).unwrap();
        assert_eq!(windowed.len(), a.len());
        assert!(windowed.iter().all(|m| m.a == m.b));
    }
    #[test]
    fn hostile_inputs_are_errors() {
        assert!(describe(8, 8, &[0; 256], &[]).is_err());
        assert!(describe(64, 64, &[0; 10], &[]).is_err());
        let bad = Feature { x: f32::NAN, y: 0.0, scale: 1.5, orientation: 0.0, response: 1.0, octave: 0 };
        assert!(describe(64, 64, &vec![0; 64 * 64 * 4], &[bad]).is_err());
        let d = Descriptor([0.0; DESCRIPTOR_LEN]);
        assert!(match_features(std::slice::from_ref(&d), &[], &[], &[], &MatchOptions::default()).is_err());
        assert!(match_features(&[], &[], &[], &[], &MatchOptions { ratio: 2.0, ..MatchOptions::default() }).is_err());
        assert!(match_features(&[], &[], &[d], &[], &MatchOptions::default()).is_err());
    }
}

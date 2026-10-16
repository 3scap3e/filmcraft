//! Colour, tonality, palette and pixel-grid evidence for one rendered frame. Descriptive statistics, not style recognition.
use serde::{Deserialize, Serialize};

pub const MAX_PALETTE: usize = 16;
pub const MAX_SIDE: usize = 4096;
const SAMPLE_BUDGET: usize = 65_536;
const QUANT_BITS: u32 = 5;
const KMEANS_ITERATIONS: usize = 8;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaletteColor {
    pub rgb: [u8; 3],
    pub hex: String,
    pub share: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Look {
    pub analyzed_size: [usize; 2],
    pub mean_luma: f32,
    pub luma_spread: f32,
    pub dark_fraction: f32,
    pub bright_fraction: f32,
    pub mean_saturation: f32,
    pub warm_bias: f32,
    pub key: String,
    pub unique_colors: usize,
    pub palette: Vec<PaletteColor>,
    pub pixel_pitch: Option<usize>,
    pub letterbox: [f32; 4],
}

fn luma(c: [u8; 3]) -> f32 {
    (0.2126 * f32::from(c[0]) + 0.7152 * f32::from(c[1]) + 0.0722 * f32::from(c[2])) / 255.0
}

fn pixel(rgba: &[u8], width: usize, x: usize, y: usize) -> Option<[u8; 3]> {
    let i = y.checked_mul(width)?.checked_add(x)?.checked_mul(4)?;
    Some([*rgba.get(i)?, *rgba.get(i + 1)?, *rgba.get(i + 2)?])
}

fn hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

fn palette(bins: &[u32], sums: &[[u32; 3]], k: usize) -> Vec<PaletteColor> {
    let color_of = |bin: usize| -> [f32; 3] {
        let n = bins.get(bin).copied().unwrap_or(0).max(1) as f32;
        let s = sums.get(bin).copied().unwrap_or([0; 3]);
        [s[0] as f32 / n, s[1] as f32 / n, s[2] as f32 / n]
    };
    let mut populated: Vec<(usize, u32)> = bins.iter().enumerate().filter(|(_, n)| **n > 0).map(|(i, n)| (i, *n)).collect();
    if populated.is_empty() || k == 0 {
        return Vec::new();
    }
    populated.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let total: u64 = populated.iter().map(|(_, n)| u64::from(*n)).sum();
    let mut centers: Vec<[f32; 3]> = Vec::new();
    for (bin, _) in &populated {
        if centers.len() >= k {
            break;
        }
        let c = color_of(*bin);
        if centers.iter().all(|e| (0..3).map(|i| (e[i] - c[i]).abs()).fold(0.0f32, f32::max) >= 24.0) {
            centers.push(c);
        }
    }
    for (bin, _) in &populated {
        if centers.len() >= k {
            break;
        }
        let c = color_of(*bin);
        if !centers.contains(&c) {
            centers.push(c);
        }
    }
    let mut weights = vec![0u64; centers.len()];
    for _ in 0..KMEANS_ITERATIONS {
        let mut sums = vec![[0.0f64; 3]; centers.len()];
        weights.iter_mut().for_each(|w| *w = 0);
        for (bin, n) in &populated {
            let c = color_of(*bin);
            let nearest = centers
                .iter()
                .enumerate()
                .map(|(i, e)| (i, (0..3).map(|k| (e[k] - c[k]) * (e[k] - c[k])).sum::<f32>()))
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(i, _)| i);
            if let Some(i) = nearest
                && let (Some(sum), Some(weight)) = (sums.get_mut(i), weights.get_mut(i))
            {
                for k in 0..3 {
                    sum[k] += f64::from(c[k]) * f64::from(*n);
                }
                *weight = weight.saturating_add(u64::from(*n));
            }
        }
        for (i, center) in centers.iter_mut().enumerate() {
            if let (Some(sum), Some(weight)) = (sums.get(i), weights.get(i))
                && *weight > 0
            {
                for k in 0..3 {
                    center[k] = (sum[k] / *weight as f64) as f32;
                }
            }
        }
    }
    let mut out: Vec<PaletteColor> = centers
        .iter()
        .zip(&weights)
        .filter(|(_, w)| **w > 0)
        .map(|(c, w)| {
            let rgb = [c[0].round().clamp(0.0, 255.0) as u8, c[1].round().clamp(0.0, 255.0) as u8, c[2].round().clamp(0.0, 255.0) as u8];
            PaletteColor { rgb, hex: hex(rgb), share: (*w as f64 / total.max(1) as f64) as f32 }
        })
        .collect();
    out.sort_by(|a, b| b.share.partial_cmp(&a.share).unwrap_or(std::cmp::Ordering::Equal).then(a.hex.cmp(&b.hex)));
    out
}

fn run_lengths(rgba: &[u8], width: usize, height: usize, horizontal: bool) -> Vec<u32> {
    let mut counts = vec![0u32; 65];
    let lines = if horizontal { height } else { width };
    let length = if horizontal { width } else { height };
    let step = lines.div_ceil(64).max(1);
    let mut line = 0;
    while line < lines {
        let mut run = 1usize;
        for i in 1..length {
            let (a, b) = if horizontal {
                (pixel(rgba, width, i - 1, line), pixel(rgba, width, i, line))
            } else {
                (pixel(rgba, width, line, i - 1), pixel(rgba, width, line, i))
            };
            let same = match (a, b) {
                (Some(a), Some(b)) => (0..3).all(|k| a[k].abs_diff(b[k]) <= 6),
                _ => false,
            };
            if same {
                run += 1;
            } else {
                if let Some(slot) = counts.get_mut(run.min(64)) {
                    *slot = slot.saturating_add(1);
                }
                run = 1;
            }
        }
        if let Some(slot) = counts.get_mut(run.min(64)) {
            *slot = slot.saturating_add(1);
        }
        line += step;
    }
    counts
}

fn pitch_of(counts: &[u32], lines: usize) -> Option<usize> {
    let total: u64 = counts.iter().enumerate().filter(|(len, _)| (2..64).contains(len)).map(|(_, n)| u64::from(*n)).sum();
    if total < 64 || total < (lines.min(64) as u64).saturating_mul(4) {
        return None;
    }
    (2..=32).rev().find(|p| {
        let multiples: u64 = counts.iter().enumerate().filter(|(len, _)| (2..64).contains(len) && len % p == 0).map(|(_, n)| u64::from(*n)).sum();
        multiples as f64 / total as f64 >= 0.85
    })
}

pub fn analyze(width: usize, height: usize, rgba: &[u8], palette_size: usize) -> Result<Look, String> {
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return Err("Frame dimensions must be 1–4096 pixels".into());
    }
    if !(1..=MAX_PALETTE).contains(&palette_size) {
        return Err("Palette size must be 1–16 colours".into());
    }
    let expected = width.checked_mul(height).and_then(|n| n.checked_mul(4)).ok_or("Frame size overflow")?;
    if rgba.len() != expected {
        return Err("Frame buffer does not match its dimensions".into());
    }
    let total = width * height;
    let mut stride = 1usize;
    while total / (stride * stride) > SAMPLE_BUDGET {
        stride += 1;
    }
    let mut bins = vec![0u32; 1 << (3 * QUANT_BITS)];
    let mut sums = vec![[0u32; 3]; 1 << (3 * QUANT_BITS)];
    let mut coarse = vec![false; 4096];
    let (mut n, mut sum, mut sum_sq, mut dark, mut bright, mut sat, mut warm) = (0u64, 0.0f64, 0.0f64, 0u64, 0u64, 0.0f64, 0.0f64);
    let mut y = 0;
    while y < height {
        let mut x = 0;
        while x < width {
            let c = pixel(rgba, width, x, y).ok_or("Pixel outside buffer")?;
            let l = f64::from(luma(c));
            sum += l;
            sum_sq += l * l;
            if l < 0.1 {
                dark += 1;
            }
            if l > 0.9 {
                bright += 1;
            }
            let max = c[0].max(c[1]).max(c[2]);
            let min = c[0].min(c[1]).min(c[2]);
            if max > 0 {
                sat += f64::from(max - min) / f64::from(max);
            }
            warm += (f64::from(c[0]) - f64::from(c[2])) / 255.0;
            let q = |v: u8| usize::from(v >> (8 - QUANT_BITS));
            let index = (q(c[0]) << (2 * QUANT_BITS)) | (q(c[1]) << QUANT_BITS) | q(c[2]);
            if let (Some(bin), Some(sum)) = (bins.get_mut(index), sums.get_mut(index)) {
                *bin = bin.saturating_add(1);
                for k in 0..3 {
                    sum[k] = sum[k].saturating_add(u32::from(c[k]));
                }
            }
            if let Some(slot) = coarse.get_mut((usize::from(c[0] >> 4) << 8) | (usize::from(c[1] >> 4) << 4) | usize::from(c[2] >> 4)) {
                *slot = true;
            }
            n += 1;
            x += stride;
        }
        y += stride;
    }
    let count = n.max(1) as f64;
    let mean = sum / count;
    let variance = (sum_sq / count - mean * mean).max(0.0);
    let is_black = |c: [u8; 3]| luma(c) <= 0.06;
    let row_black = |yy: usize| (0..width).all(|xx| pixel(rgba, width, xx, yy).is_some_and(is_black));
    let column_black = |xx: usize| (0..height).all(|yy| pixel(rgba, width, xx, yy).is_some_and(is_black));
    let top = (0..height).take_while(|yy| row_black(*yy)).count();
    let letterbox = if top >= height {
        [0.0; 4]
    } else {
        let bottom = (0..height).rev().take_while(|yy| row_black(*yy)).count();
        let left = (0..width).take_while(|xx| column_black(*xx)).count();
        let right = (0..width).rev().take_while(|xx| column_black(*xx)).count();
        [top as f32 / height as f32, bottom as f32 / height as f32, left as f32 / width as f32, right as f32 / width as f32]
    };
    let horizontal = pitch_of(&run_lengths(rgba, width, height, true), height);
    let vertical = pitch_of(&run_lengths(rgba, width, height, false), width);
    let pixel_pitch = match (horizontal, vertical) {
        (Some(h), Some(v)) if h == v => Some(h),
        _ => None,
    };
    let key = if mean < 0.25 {
        "low-key"
    } else if mean > 0.65 {
        "high-key"
    } else {
        "mid-key"
    };
    Ok(Look {
        analyzed_size: [width, height],
        mean_luma: mean as f32,
        luma_spread: variance.sqrt() as f32,
        dark_fraction: (dark as f64 / count) as f32,
        bright_fraction: (bright as f64 / count) as f32,
        mean_saturation: (sat / count) as f32,
        warm_bias: (warm / count) as f32,
        key: key.into(),
        unique_colors: coarse.iter().filter(|b| **b).count(),
        palette: palette(&bins, &sums, palette_size),
        pixel_pitch,
        letterbox,
    })
}

impl Look {
    pub fn validate(&self) -> Result<(), String> {
        if self.analyzed_size.iter().any(|n| *n == 0 || *n > MAX_SIDE)
            || [self.mean_luma, self.luma_spread, self.dark_fraction, self.bright_fraction, self.mean_saturation]
                .iter()
                .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
            || !self.warm_bias.is_finite()
            || !(-1.0..=1.0).contains(&self.warm_bias)
            || self.palette.len() > MAX_PALETTE
            || self.palette.iter().any(|c| !c.share.is_finite() || !(0.0..=1.0).contains(&c.share) || c.hex.len() != 7)
            || self.letterbox.iter().any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
            || self.pixel_pitch.is_some_and(|p| !(2..=32).contains(&p))
            || self.unique_colors > 4096
        {
            return Err("Invalid look record".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn blocks(width: usize, height: usize, pitch: usize, colors: &[[u8; 3]]) -> Vec<u8> {
        let mut px = vec![0u8; width * height * 4];
        for y in 0..height {
            for x in 0..width {
                let cell = (x / pitch) * 7 + (y / pitch) * 3;
                let c = colors[cell % colors.len()];
                let i = (y * width + x) * 4;
                px[i..i + 3].copy_from_slice(&c);
                px[i + 3] = 255;
            }
        }
        px
    }
    #[test]
    fn pixel_art_grid_and_palette_are_recovered() {
        let colors = [[12, 20, 40], [200, 160, 60], [90, 30, 30], [30, 120, 90]];
        let px = blocks(128, 72, 4, &colors);
        let look = analyze(128, 72, &px, 6).unwrap();
        look.validate().unwrap();
        assert_eq!(look.pixel_pitch, Some(4));
        assert_eq!(look.unique_colors, 4);
        assert_eq!(look.palette.len(), 4, "{:?}", look.palette);
        assert!(look.palette.iter().all(|c| colors.iter().any(|k| (0..3).all(|i| c.rgb[i].abs_diff(k[i]) <= 1))));
        assert!(look.palette.iter().map(|c| c.share).sum::<f32>() > 0.99);
        assert_eq!(look.letterbox, [0.0; 4]);
        assert_eq!(look.palette[0].hex.len(), 7);
    }
    #[test]
    fn noise_has_no_pitch_and_letterbox_is_measured() {
        let (w, h) = (96, 64);
        let mut px = vec![0u8; w * h * 4];
        for y in 8..56 {
            for x in 0..w {
                let i = (y * w + x) * 4;
                let v = ((x * 31 + y * 17) % 97) as u8 + 60;
                px[i] = v;
                px[i + 1] = v / 2 + 40;
                px[i + 2] = 255 - v;
                px[i + 3] = 255;
            }
        }
        let look = analyze(w, h, &px, 8).unwrap();
        assert_eq!(look.pixel_pitch, None);
        assert!((look.letterbox[0] - 0.125).abs() < 0.001 && (look.letterbox[1] - 0.125).abs() < 0.001);
        assert_eq!(look.letterbox[2], 0.0);
        assert!(look.unique_colors >= 8);
        assert_eq!(look.key, "mid-key");
    }
    #[test]
    fn hostile_inputs_are_errors() {
        assert!(analyze(0, 1, &[], 4).is_err());
        assert!(analyze(2, 2, &[0; 8], 4).is_err());
        assert!(analyze(2, 2, &[0; 16], 0).is_err());
        assert!(analyze(2, 2, &[0; 16], 17).is_err());
        let black = analyze(2, 2, &[0; 16], 4).unwrap();
        assert_eq!(black.letterbox, [0.0; 4]);
        assert_eq!(black.palette.len(), 1);
    }
}

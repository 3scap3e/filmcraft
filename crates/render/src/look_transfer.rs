//! Look transfer: move the colours of one picture onto another at the pixel level, baked into a
//! 3D LUT the Lumetri Creative Look slot applies.
//!
//! Both pictures are summarised in **Oklab** (Björn Ottosson, 2020) by the mean and standard
//! deviation of each axis — lightness L, green–red a, blue–yellow b — over their pixels (several
//! frames pooled). The transfer is Reinhard's colour transfer (Reinhard, Ashikhmin, Gooch and
//! Shirley, "Color Transfer between Images", 2001) in that space: each axis of a pixel is
//! re-centred on the target mean and rescaled by the ratio of the standard deviations, so the
//! brightness, contrast, cast and colourfulness of the target land on the source while its
//! picture content stays. The ratio is bounded (×0.5 … ×2) so a flat source is not blown apart.
//! The transform is evaluated on a lattice of sRGB-encoded colours (Lumetri hands its Creative
//! Look LUT the sRGB-encoded SDR signal) and written as a `.cube` LUT, which is what "applying a
//! look" installs on the clip: a real per-colour transformation of the frame, editable later with
//! the ordinary Lumetri sliders on top.

use filmcraft_color::{Lut3d, linear_to_srgb, srgb_to_linear};
use serde::{Deserialize, Serialize};

use crate::Image;
use crate::color_match::{oklab, shrink};

/// Mean and standard deviation of a picture's Oklab axes (L, a, b) with the pixel weight that
/// produced them, so several pictures pool into one.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColorStats {
    pub mean: [f32; 3],
    pub std: [f32; 3],
    pub weight: f64,
}

/// Oklab → sRGB-primaries linear RGB (inverse of [`oklab`]).
pub fn oklab_to_linear(lab: [f32; 3]) -> [f32; 3] {
    let l_ = lab[0] + 0.396_337_78 * lab[1] + 0.215_803_76 * lab[2];
    let m_ = lab[0] - 0.105_561_346 * lab[1] - 0.063_854_17 * lab[2];
    let s_ = lab[0] - 0.089_484_18 * lab[1] - 1.291_485_5 * lab[2];
    let (l, m, s) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
    [
        4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_93 * s,
        -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s,
        -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s,
    ]
}

/// The Oklab statistics of a linear premultiplied image (downsampled to ≤ 96 px wide first).
pub fn stats(img: &Image) -> Option<ColorStats> {
    if img.w == 0 || img.h == 0 || img.px.len() != img.w * img.h * 4 {
        return None;
    }
    let small = shrink(img, 96);
    let mut sum = [0f64; 3];
    let mut sum_sq = [0f64; 3];
    let mut n = 0f64;
    for p in small.px.as_chunks::<4>().0 {
        if p[3] <= 1e-3 || !p.iter().all(|v| v.is_finite()) {
            continue;
        }
        let lab = oklab([p[0] / p[3], p[1] / p[3], p[2] / p[3]]);
        for k in 0..3 {
            sum[k] += f64::from(lab[k]);
            sum_sq[k] += f64::from(lab[k]) * f64::from(lab[k]);
        }
        n += 1.0;
    }
    if n < 1.0 {
        return None;
    }
    let mean = std::array::from_fn(|k| (sum[k] / n) as f32);
    let std = std::array::from_fn(|k| ((sum_sq[k] / n - (sum[k] / n).powi(2)).max(0.0)).sqrt() as f32);
    Some(ColorStats { mean, std, weight: n })
}

/// Pool several statistics into one (means weighted by pixel count, variances pooled).
pub fn merge(parts: &[ColorStats]) -> Option<ColorStats> {
    let total: f64 = parts.iter().map(|p| p.weight.max(0.0)).sum();
    if total <= 0.0 {
        return None;
    }
    let mean: [f32; 3] = std::array::from_fn(|k| (parts.iter().map(|p| f64::from(p.mean[k]) * p.weight.max(0.0)).sum::<f64>() / total) as f32);
    let std: [f32; 3] = std::array::from_fn(|k| {
        let var =
            parts.iter().map(|p| p.weight.max(0.0) * (f64::from(p.std[k]).powi(2) + (f64::from(p.mean[k]) - f64::from(mean[k])).powi(2))).sum::<f64>() / total;
        var.max(0.0).sqrt() as f32
    });
    Some(ColorStats { mean, std, weight: total })
}

fn valid(s: &ColorStats) -> bool {
    s.mean.iter().chain(&s.std).all(|v| v.is_finite()) && s.weight.is_finite() && s.weight > 0.0
}

/// Reinhard transfer of one linear RGB colour from the `from` statistics onto the `to`
/// statistics, blended by `strength` (0 = unchanged, 1 = full transfer).
pub fn transfer(c: [f32; 3], from: &ColorStats, to: &ColorStats, strength: f32) -> [f32; 3] {
    let lab = oklab(c);
    let mut out = [0f32; 3];
    for k in 0..3 {
        let ratio = (to.std[k] / from.std[k].max(1e-4)).clamp(0.5, 2.0);
        let moved = to.mean[k] + (lab[k] - from.mean[k]) * ratio;
        out[k] = lab[k] + (moved - lab[k]) * strength;
    }
    oklab_to_linear(out).map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 })
}

/// The transfer baked as a 3D LUT over sRGB-encoded input (what Lumetri's Creative Look applies).
pub fn transfer_lut(from: &ColorStats, to: &ColorStats, size: usize, strength: f32, title: &str) -> Result<Lut3d, String> {
    if !valid(from) || !valid(to) {
        return Err("Colour statistics must be finite and come from at least one pixel".into());
    }
    if !(2..=65).contains(&size) || !strength.is_finite() || !(0.0..=1.0).contains(&strength) {
        return Err("LUT size must be 2–65 and strength 0–1".into());
    }
    let mut lut = Lut3d::from_fn(size, |c| {
        let lin = c.map(srgb_to_linear);
        transfer(lin, from, to, strength).map(linear_to_srgb)
    });
    lut.title = title.chars().take(120).collect();
    Ok(lut)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(base: [f32; 3], spread: f32) -> Image {
        let (w, h) = (96, 54);
        let mut img = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let t = x as f32 / w as f32 - 0.5;
                let u = y as f32 / h as f32 - 0.5;
                let c = [(base[0] + spread * t).clamp(0.0, 1.0), (base[1] + spread * u * 0.6).clamp(0.0, 1.0), (base[2] - spread * t * 0.4).clamp(0.0, 1.0)];
                let i = (y * w + x) * 4;
                img.px[i..i + 4].copy_from_slice(&[c[0], c[1], c[2], 1.0]);
            }
        }
        img
    }

    fn apply_lut(img: &Image, lut: &Lut3d) -> Image {
        let mut out = img.clone();
        out.map_rgb(|c, _, _| lut.apply(c.map(linear_to_srgb)).map(srgb_to_linear));
        out
    }

    #[test]
    fn oklab_round_trips() {
        for c in [[0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [0.8, 0.1, 0.05], [0.02, 0.3, 0.6], [0.5, 0.5, 0.5]] {
            let back = oklab_to_linear(oklab(c));
            for k in 0..3 {
                assert!((back[k] - c[k]).abs() < 2e-3, "{c:?} → {back:?}");
            }
        }
    }

    #[test]
    fn stats_pool_and_a_transfer_lut_moves_the_source_onto_the_target() {
        let bright_warm = picture([0.7, 0.55, 0.35], 0.5);
        let dark_cool = picture([0.08, 0.12, 0.3], 0.15);
        let from = stats(&bright_warm).unwrap();
        let to = stats(&dark_cool).unwrap();
        assert!(from.mean[0] > to.mean[0], "the warm picture is lighter");
        assert!(from.mean[2] > to.mean[2], "the warm picture is yellower");
        let pooled = merge(&[from.clone(), from.clone()]).unwrap();
        assert!((pooled.mean[0] - from.mean[0]).abs() < 1e-5 && (pooled.std[0] - from.std[0]).abs() < 1e-4);
        assert!((pooled.weight - 2.0 * from.weight).abs() < 1e-6);
        let lut = transfer_lut(&from, &to, 17, 1.0, "test").unwrap();
        let moved = stats(&apply_lut(&bright_warm, &lut)).unwrap();
        for k in 0..3 {
            assert!((moved.mean[k] - to.mean[k]).abs() < 0.03, "axis {k}: {:?} wanted {:?}", moved.mean, to.mean);
        }
        assert!(moved.std[0] < from.std[0], "the flatter target lowers the lightness spread");
        let same = transfer_lut(&from, &from, 9, 1.0, "identity").unwrap();
        let kept = stats(&apply_lut(&bright_warm, &same)).unwrap();
        for k in 0..3 {
            assert!((kept.mean[k] - from.mean[k]).abs() < 0.01, "a transfer onto itself changes little: {kept:?}");
        }
        assert!(transfer_lut(&from, &to, 1, 1.0, "").is_err());
        assert!(transfer_lut(&from, &ColorStats::default(), 17, 1.0, "").is_err());
        assert!(stats(&Image::new(0, 0)).is_none());
    }
}

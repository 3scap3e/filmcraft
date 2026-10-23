//! The look of a shot as a reusable grade: tone (black and white points, contrast), colour
//! (saturation, temperature, palette with colour names), texture (grain, sharpness) and vignette,
//! measured on several frames and read in the words a colourist uses. Every word is a
//! classification of the number next to it under rules that travel with the result; nothing is
//! invented and no style is recognised.
use crate::look::Look;
use serde::{Deserialize, Serialize};

pub const MAX_SAMPLES: usize = 600;
const SAMPLE_BUDGET: usize = 65_536;
const CORNER_FRACTION: f32 = 1.0 / 6.0;
const CENTRE_FRACTION: f32 = 1.0 / 3.0;
const GRAIN_BLOCK: usize = 8;
const MAX_GRAIN_BLOCKS: usize = 512;

/// Tonal and textural measurements of one frame that [`Look`] does not carry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tone {
    /// Luma below which 1 % of the pixels lie.
    pub black_point: f32,
    /// Luma above which 1 % of the pixels lie.
    pub white_point: f32,
    /// `white_point - black_point`.
    pub contrast: f32,
    /// Mean luma of the four corners over the centre's (1 = none, lower = darker corners).
    pub vignette: f32,
    /// RMS of the fine luma noise in the flattest blocks (0–1).
    pub grain: f32,
    /// Mean absolute luma Laplacian (0–1): higher is sharper or busier.
    pub sharpness: f32,
}

fn luma_at(rgba: &[u8], width: usize, x: usize, y: usize) -> Option<f32> {
    let i = y.checked_mul(width)?.checked_add(x)?.checked_mul(4)?;
    let (r, g, b) = (*rgba.get(i)?, *rgba.get(i + 1)?, *rgba.get(i + 2)?);
    Some((0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b)) / 255.0)
}

fn region_mean(rgba: &[u8], width: usize, x0: usize, y0: usize, x1: usize, y1: usize, stride: usize) -> f32 {
    let (mut sum, mut n) = (0.0f64, 0u64);
    let mut y = y0;
    while y < y1 {
        let mut x = x0;
        while x < x1 {
            if let Some(l) = luma_at(rgba, width, x, y) {
                sum += f64::from(l);
                n += 1;
            }
            x += stride;
        }
        y += stride;
    }
    if n == 0 { 0.0 } else { (sum / n as f64) as f32 }
}

/// Measure the tone of one RGBA frame.
pub fn tone(width: usize, height: usize, rgba: &[u8]) -> Result<Tone, String> {
    if width == 0 || height == 0 || width > crate::look::MAX_SIDE || height > crate::look::MAX_SIDE {
        return Err("Frame dimensions must be 1–4096 pixels".into());
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
    let mut histogram = [0u64; 256];
    let (mut lap_sum, mut lap_n) = (0.0f64, 0u64);
    let mut y = 0;
    while y < height {
        let mut x = 0;
        while x < width {
            let l = luma_at(rgba, width, x, y).ok_or("Pixel outside buffer")?;
            let bin = (l * 255.0).round().clamp(0.0, 255.0) as usize;
            if let Some(slot) = histogram.get_mut(bin) {
                *slot += 1;
            }
            if x > 0
                && y > 0
                && x + 1 < width
                && y + 1 < height
                && let (Some(l_), Some(r), Some(u), Some(d)) =
                    (luma_at(rgba, width, x - 1, y), luma_at(rgba, width, x + 1, y), luma_at(rgba, width, x, y - 1), luma_at(rgba, width, x, y + 1))
            {
                lap_sum += f64::from((l_ + r + u + d - 4.0 * l).abs()) / 4.0;
                lap_n += 1;
            }
            x += stride;
        }
        y += stride;
    }
    let count: u64 = histogram.iter().sum();
    let percentile = |fraction: f64| -> f32 {
        let target = (count as f64 * fraction).floor() as u64;
        let mut seen = 0u64;
        for (bin, n) in histogram.iter().enumerate() {
            seen += n;
            if seen > target {
                return bin as f32 / 255.0;
            }
        }
        1.0
    };
    let black_point = percentile(0.01);
    let white_point = percentile(0.99).max(black_point);
    let cw = ((width as f32 * CORNER_FRACTION) as usize).max(1);
    let ch = ((height as f32 * CORNER_FRACTION) as usize).max(1);
    let corners = [(0, 0), (width - cw, 0), (0, height - ch), (width - cw, height - ch)];
    let corner_mean = corners.iter().map(|(x, y)| region_mean(rgba, width, *x, *y, x + cw, y + ch, stride)).sum::<f32>() / 4.0;
    let (mw, mh) = (((width as f32 * CENTRE_FRACTION) as usize).max(1), ((height as f32 * CENTRE_FRACTION) as usize).max(1));
    let (mx, my) = ((width - mw) / 2, (height - mh) / 2);
    let centre_mean = region_mean(rgba, width, mx, my, mx + mw, my + mh, stride);
    let vignette = if centre_mean > 1e-3 { (corner_mean / centre_mean).clamp(0.0, 2.0) } else { 1.0 };
    let grain = grain_of(rgba, width, height);
    let sharpness = if lap_n == 0 { 0.0 } else { (lap_sum / lap_n as f64).clamp(0.0, 1.0) as f32 };
    Ok(Tone { black_point, white_point, contrast: white_point - black_point, vignette, grain, sharpness })
}

/// RMS of the fine noise (pixel minus its 3×3 mean) in the flattest third of the sampled 8×8 blocks.
fn grain_of(rgba: &[u8], width: usize, height: usize) -> f32 {
    let (bw, bh) = (width / GRAIN_BLOCK, height / GRAIN_BLOCK);
    if bw < 2 || bh < 2 {
        return 0.0;
    }
    let total_blocks = bw * bh;
    let step = total_blocks.div_ceil(MAX_GRAIN_BLOCKS).max(1);
    let mut blocks: Vec<(f32, f32)> = Vec::new();
    let mut index = 0;
    while index < total_blocks {
        let (bx, by) = (index % bw, index / bw);
        let (x0, y0) = (bx * GRAIN_BLOCK, by * GRAIN_BLOCK);
        let (mut grad, mut hp, mut n) = (0.0f32, 0.0f32, 0u32);
        for y in y0.max(1)..(y0 + GRAIN_BLOCK).min(height - 1) {
            for x in x0.max(1)..(x0 + GRAIN_BLOCK).min(width - 1) {
                let mut sum = 0.0;
                let mut ok = true;
                for dy in -1i64..=1 {
                    for dx in -1i64..=1 {
                        match luma_at(rgba, width, (x as i64 + dx) as usize, (y as i64 + dy) as usize) {
                            Some(l) => sum += l,
                            None => ok = false,
                        }
                    }
                }
                if !ok {
                    continue;
                }
                let (Some(c), Some(r), Some(d)) = (luma_at(rgba, width, x, y), luma_at(rgba, width, x + 1, y), luma_at(rgba, width, x, y + 1)) else {
                    continue;
                };
                let residual = c - sum / 9.0;
                hp += residual * residual;
                grad += (r - c).abs() + (d - c).abs();
                n += 1;
            }
        }
        if n > 0 {
            blocks.push((grad / n as f32, hp / n as f32));
        }
        index += step;
    }
    if blocks.is_empty() {
        return 0.0;
    }
    blocks.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let keep = (blocks.len() / 3).max(1);
    let mean = blocks.iter().take(keep).map(|b| b.1).sum::<f32>() / keep as f32;
    mean.max(0.0).sqrt().clamp(0.0, 1.0)
}

/// One measured frame of a shot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameSample {
    pub frame: usize,
    pub look: Look,
    pub tone: Tone,
}

/// A dominant colour with the name a person would use for it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NamedColor {
    pub name: String,
    pub hex: String,
    pub rgb: [u8; 3],
    pub share: f32,
}

/// The thresholds the words of a [`Grade`] were derived with.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GradeRules {
    /// Mean luma below the first is low-key, above the second high-key.
    pub low_key_below: f32,
    pub high_key_above: f32,
    /// Contrast (white point − black point) below the first is flat, below the second normal, else punchy.
    pub contrast_flat_below: f32,
    pub contrast_normal_below: f32,
    /// Mean saturation below the first is desaturated, below the second natural, else vivid.
    pub saturation_desaturated_below: f32,
    pub saturation_natural_below: f32,
    /// Warm bias (mean R − B over 255) below −x is cool, above +x warm.
    pub temperature_neutral_within: f32,
    /// Corner/centre luma ratio below the first is a strong vignette, below the second a light one.
    pub vignette_strong_below: f32,
    pub vignette_light_below: f32,
    /// Grain RMS at or above which the picture is grainy.
    pub grainy_at_least: f32,
    /// Mean |Laplacian| below which the picture is soft.
    pub soft_below: f32,
    /// Coefficient of variation of the mean luma across frames at or above which the grade changes.
    pub changing_at_least: f32,
    /// Colour naming: saturation below this is grey, luma below `black_below` black, above `white_above` white.
    pub grey_saturation_below: f32,
    pub black_below: f32,
    pub white_above: f32,
}

const LOW_KEY: f32 = 0.25;
const HIGH_KEY: f32 = 0.65;
const CONTRAST_FLAT: f32 = 0.5;
const CONTRAST_NORMAL: f32 = 0.8;
const SAT_DESATURATED: f32 = 0.15;
const SAT_NATURAL: f32 = 0.35;
const TEMPERATURE_NEUTRAL: f32 = 0.05;
const VIGNETTE_STRONG: f32 = 0.55;
const VIGNETTE_LIGHT: f32 = 0.8;
const GRAINY: f32 = 0.02;
const SOFT: f32 = 0.01;
const CHANGING: f32 = 0.1;
const GREY_SAT: f32 = 0.12;
const BLACK: f32 = 0.12;
const WHITE: f32 = 0.88;

impl Default for GradeRules {
    fn default() -> Self {
        Self {
            low_key_below: LOW_KEY,
            high_key_above: HIGH_KEY,
            contrast_flat_below: CONTRAST_FLAT,
            contrast_normal_below: CONTRAST_NORMAL,
            saturation_desaturated_below: SAT_DESATURATED,
            saturation_natural_below: SAT_NATURAL,
            temperature_neutral_within: TEMPERATURE_NEUTRAL,
            vignette_strong_below: VIGNETTE_STRONG,
            vignette_light_below: VIGNETTE_LIGHT,
            grainy_at_least: GRAINY,
            soft_below: SOFT,
            changing_at_least: CHANGING,
            grey_saturation_below: GREY_SAT,
            black_below: BLACK,
            white_above: WHITE,
        }
    }
}

/// The grade of a shot: means over its sampled frames and the words they read as.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Grade {
    pub samples: usize,
    pub mean_luma: f32,
    pub black_point: f32,
    pub white_point: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub warm_bias: f32,
    pub vignette: f32,
    pub grain: f32,
    pub sharpness: f32,
    /// Relative spread of the mean luma across frames (standard deviation over mean).
    pub luma_variation: f32,
    pub palette: Vec<NamedColor>,
    pub key: String,
    pub contrast_class: String,
    pub saturation_class: String,
    pub temperature: String,
    pub vignette_class: String,
    pub texture: String,
    pub sharpness_class: String,
    pub stable: bool,
    pub summary: String,
    pub rules: GradeRules,
}

/// The name of a colour: black / white / greys by luma and saturation, else a hue band with a
/// dark / light prefix.
pub fn color_name(rgb: [u8; 3]) -> String {
    let (r, g, b) = (f32::from(rgb[0]) / 255.0, f32::from(rgb[1]) / 255.0, f32::from(rgb[2]) / 255.0);
    let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let sat = if max > 0.0 { (max - min) / max } else { 0.0 };
    if luma < BLACK && sat < 0.5 {
        return "black".into();
    }
    if luma > WHITE && sat < GREY_SAT {
        return "white".into();
    }
    if sat < GREY_SAT {
        return if luma < 0.35 {
            "dark grey".into()
        } else if luma > 0.65 {
            "light grey".into()
        } else {
            "grey".into()
        };
    }
    let d = max - min;
    let hue = if d <= 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / d) % 6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let hue = if hue < 0.0 { hue + 360.0 } else { hue };
    let base = match hue {
        h if !(15.0..345.0).contains(&h) => "red",
        h if h < 45.0 => {
            if luma < 0.4 {
                return "brown".into();
            }
            "orange"
        }
        h if h < 70.0 => "yellow",
        h if h < 165.0 => "green",
        h if h < 195.0 => "teal",
        h if h < 255.0 => "blue",
        h if h < 290.0 => "purple",
        _ => "pink",
    };
    let lightness = (max + min) / 2.0;
    if lightness < 0.4 {
        format!("dark {base}")
    } else if lightness > 0.7 {
        format!("light {base}")
    } else {
        base.into()
    }
}

/// Merge the palettes of several frames: colours within 24 of an existing entry (max channel
/// difference) pool their shares; the result is sorted by share and cut to `k`.
fn merge_palettes(samples: &[FrameSample], k: usize) -> Vec<NamedColor> {
    let mut pool: Vec<([f64; 3], f64)> = Vec::new();
    let n = samples.len().max(1) as f64;
    for s in samples {
        for c in &s.look.palette {
            let rgb = [f64::from(c.rgb[0]), f64::from(c.rgb[1]), f64::from(c.rgb[2])];
            let share = f64::from(c.share.max(0.0)) / n;
            match pool.iter_mut().find(|(e, _)| (0..3).map(|i| (e[i] - rgb[i]).abs()).fold(0.0f64, f64::max) < 24.0) {
                Some((e, w)) => {
                    let total = *w + share;
                    if total > 0.0 {
                        for i in 0..3 {
                            e[i] = (e[i] * *w + rgb[i] * share) / total;
                        }
                    }
                    *w = total;
                }
                None => pool.push((rgb, share)),
            }
        }
    }
    pool.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    pool.truncate(k);
    pool.into_iter()
        .map(|(c, w)| {
            let rgb = [c[0].round().clamp(0.0, 255.0) as u8, c[1].round().clamp(0.0, 255.0) as u8, c[2].round().clamp(0.0, 255.0) as u8];
            NamedColor { name: color_name(rgb), hex: format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]), rgb, share: w.clamp(0.0, 1.0) as f32 }
        })
        .collect()
}

fn mean(samples: &[FrameSample], f: impl Fn(&FrameSample) -> f32) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.iter().map(|s| f64::from(f(s))).sum::<f64>() as f32 / samples.len() as f32
}

/// Read the grade of a shot from its frame samples (1–600).
pub fn grade(samples: &[FrameSample], palette_size: usize) -> Result<Grade, String> {
    if samples.is_empty() || samples.len() > MAX_SAMPLES {
        return Err("Supply 1–600 frame samples".into());
    }
    for s in samples {
        s.look.validate()?;
        let t = &s.tone;
        if [t.black_point, t.white_point, t.contrast, t.grain, t.sharpness].iter().any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
            || !t.vignette.is_finite()
            || !(0.0..=2.0).contains(&t.vignette)
        {
            return Err("Invalid tone record".into());
        }
    }
    let rules = GradeRules::default();
    let mean_luma = mean(samples, |s| s.look.mean_luma);
    let variance = mean(samples, |s| (s.look.mean_luma - mean_luma) * (s.look.mean_luma - mean_luma));
    let luma_variation = if mean_luma > 1e-3 { variance.max(0.0).sqrt() / mean_luma } else { 0.0 };
    let black_point = mean(samples, |s| s.tone.black_point);
    let white_point = mean(samples, |s| s.tone.white_point);
    let contrast = (white_point - black_point).max(0.0);
    let saturation = mean(samples, |s| s.look.mean_saturation);
    let warm_bias = mean(samples, |s| s.look.warm_bias);
    let vignette = mean(samples, |s| s.tone.vignette);
    let grain = mean(samples, |s| s.tone.grain);
    let sharpness = mean(samples, |s| s.tone.sharpness);
    let key = if mean_luma < rules.low_key_below {
        "low-key"
    } else if mean_luma > rules.high_key_above {
        "high-key"
    } else {
        "mid-key"
    };
    let contrast_class = if contrast < rules.contrast_flat_below {
        "flat"
    } else if contrast < rules.contrast_normal_below {
        "normal"
    } else {
        "punchy"
    };
    let saturation_class = if saturation < rules.saturation_desaturated_below {
        "desaturated"
    } else if saturation < rules.saturation_natural_below {
        "natural"
    } else {
        "vivid"
    };
    let temperature = if warm_bias < -rules.temperature_neutral_within {
        "cool"
    } else if warm_bias > rules.temperature_neutral_within {
        "warm"
    } else {
        "neutral"
    };
    let vignette_class = if vignette < rules.vignette_strong_below {
        "strong vignette"
    } else if vignette < rules.vignette_light_below {
        "light vignette"
    } else {
        "no vignette"
    };
    let texture = if grain >= rules.grainy_at_least { "grainy" } else { "clean" };
    let sharpness_class = if sharpness < rules.soft_below { "soft" } else { "sharp" };
    let stable = luma_variation < rules.changing_at_least;
    let palette = merge_palettes(samples, palette_size.clamp(1, crate::look::MAX_PALETTE));
    let colours: Vec<String> = palette.iter().take(4).map(|c| format!("{} {:.0} %", c.name, c.share * 100.0)).collect();
    let summary = format!(
        "Look: {key}, {contrast_class} contrast, {saturation_class}, {temperature}; dominant colours {}; {vignette_class}, {texture}, {sharpness_class}; {} across {} sampled frame{}.",
        if colours.is_empty() { "none".to_string() } else { colours.join(", ") },
        if stable { "steady" } else { "changing" },
        samples.len(),
        if samples.len() == 1 { "" } else { "s" }
    );
    Ok(Grade {
        samples: samples.len(),
        mean_luma,
        black_point,
        white_point,
        contrast,
        saturation,
        warm_bias,
        vignette,
        grain,
        sharpness,
        luma_variation,
        palette,
        key: key.into(),
        contrast_class: contrast_class.into(),
        saturation_class: saturation_class.into(),
        temperature: temperature.into(),
        vignette_class: vignette_class.into(),
        texture: texture.into(),
        sharpness_class: sharpness_class.into(),
        stable,
        summary,
        rules,
    })
}

/// Lumetri Color parameters that reproduce a grade's tone and colour on neutral footage, as
/// (parameter id, value): a first-order translation of the measured numbers, not a match.
fn tone_chain(luma: f64, exposure: f64, blacks: f64, whites: f64) -> f64 {
    let v = luma * 2f64.powf(exposure);
    let b0 = -blacks / 100.0 * 0.15;
    let w0 = 1.0 - whites / 100.0 * 0.15;
    ((v - b0) / (w0 - b0).max(1e-3)).clamp(0.0, 1.0)
}

/// Lumetri Color parameters that move footage measured as `source` towards the `target` grade:
/// exposure, blacks and whites are searched so the source's black point, mean luma and white
/// point land on the target's through Lumetri's own tone chain; saturation is the ratio of the
/// two saturations, temperature the warmth difference, vignette the difference of the corner
/// ratios. A match of the measured numbers, not of the picture.
pub fn lumetri_match(target: &Grade, source: &Grade) -> Vec<(&'static str, f64)> {
    let want = [f64::from(target.black_point), f64::from(target.mean_luma), f64::from(target.white_point)];
    let have = [f64::from(source.black_point), f64::from(source.mean_luma), f64::from(source.white_point)];
    let weights = [1.0, 2.0, 1.0];
    let error =
        |e: f64, b: f64, w: f64| -> f64 { have.iter().zip(&want).zip(&weights).map(|((h, t), k)| k * (tone_chain(*h, e, b, w) - t).powi(2)).sum::<f64>() };
    let (mut e, mut b, mut w) = (0.0f64, 0.0f64, 0.0f64);
    let (mut se, mut sb, mut sw) = (1.0f64, 50.0f64, 50.0f64);
    let mut best = error(e, b, w);
    for _ in 0..48 {
        let mut improved = false;
        for (de, db, dw) in [(se, 0.0, 0.0), (-se, 0.0, 0.0), (0.0, sb, 0.0), (0.0, -sb, 0.0), (0.0, 0.0, sw), (0.0, 0.0, -sw)] {
            let (ne, nb, nw) = ((e + de).clamp(-3.0, 3.0), (b + db).clamp(-100.0, 100.0), (w + dw).clamp(-100.0, 100.0));
            let candidate = error(ne, nb, nw);
            if candidate + 1e-12 < best {
                best = candidate;
                (e, b, w) = (ne, nb, nw);
                improved = true;
            }
        }
        if !improved {
            se *= 0.5;
            sb *= 0.5;
            sw *= 0.5;
        }
    }
    let saturation = (f64::from(target.saturation) / f64::from(source.saturation).max(0.02) * 100.0).clamp(0.0, 200.0);
    let temperature = ((f64::from(target.warm_bias) - f64::from(source.warm_bias)) / (0.7 * f64::from(source.mean_luma).max(0.1)) * 100.0).clamp(-100.0, 100.0);
    let vignette = ((f64::from(target.vignette) - f64::from(source.vignette)) * 4.0).clamp(-5.0, 5.0);
    let round = |v: f64| (v * 100.0).round() / 100.0;
    vec![
        ("temperature", round(temperature)),
        ("exposure", round(e)),
        ("contrast", 0.0),
        ("blacks", round(b)),
        ("whites", round(w)),
        ("saturation", round(saturation)),
        ("vignette_amount", round(vignette)),
    ]
}

pub fn lumetri_params(g: &Grade) -> Vec<(&'static str, f64)> {
    let temperature = (f64::from(g.warm_bias) * 400.0).clamp(-100.0, 100.0);
    let exposure = ((f64::from(g.mean_luma) - 0.45) * 4.0).clamp(-3.0, 3.0);
    let contrast = ((f64::from(g.contrast) - 0.75) * 200.0).clamp(-100.0, 100.0);
    let blacks = (f64::from(g.black_point) * -300.0).clamp(-100.0, 100.0);
    let whites = ((f64::from(g.white_point) - 1.0) * 300.0).clamp(-100.0, 100.0);
    let saturation = (f64::from(g.saturation) / 0.25 * 100.0).clamp(0.0, 200.0);
    let vignette = ((f64::from(g.vignette) - 1.0) * 4.0).clamp(-5.0, 0.0);
    vec![
        ("temperature", temperature),
        ("exposure", exposure),
        ("contrast", contrast),
        ("blacks", blacks),
        ("whites", whites),
        ("saturation", saturation),
        ("vignette_amount", vignette),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(w: usize, h: usize, f: impl Fn(usize, usize) -> [u8; 3]) -> Vec<u8> {
        let mut px = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let c = f(x, y);
                let i = (y * w + x) * 4;
                px[i..i + 3].copy_from_slice(&c);
                px[i + 3] = 255;
            }
        }
        px
    }

    fn sample(frame_index: usize, w: usize, h: usize, px: &[u8]) -> FrameSample {
        FrameSample { frame: frame_index, look: crate::look::analyze(w, h, px, 6).unwrap(), tone: tone(w, h, px).unwrap() }
    }

    #[test]
    fn a_dark_warm_vignetted_frame_reads_as_low_key_warm_with_a_vignette() {
        let (w, h) = (192, 108);
        let px = frame(w, h, |x, y| {
            let dx = (x as f32 / w as f32 - 0.5) * 2.0;
            let dy = (y as f32 / h as f32 - 0.5) * 2.0;
            let fall = (1.0 - 0.9 * (dx * dx + dy * dy).sqrt().min(1.0)).max(0.0);
            [(120.0 * fall) as u8, (60.0 * fall) as u8, (30.0 * fall) as u8]
        });
        let t = tone(w, h, &px).unwrap();
        assert!(t.vignette < 0.8, "corners darker than the centre: {}", t.vignette);
        assert!(t.black_point < 0.1 && t.white_point < 0.5, "{t:?}");
        assert!(t.grain < 0.01, "a smooth gradient has no grain: {}", t.grain);
        let g = grade(&[sample(0, w, h, &px), sample(12, w, h, &px)], 6).unwrap();
        assert_eq!((g.key.as_str(), g.temperature.as_str(), g.contrast_class.as_str()), ("low-key", "warm", "flat"));
        assert!(g.vignette_class.contains("vignette") && g.vignette_class != "no vignette", "{}", g.vignette_class);
        assert!(g.stable && g.samples == 2);
        assert!(g.palette.iter().any(|c| c.name == "brown" || c.name.contains("red") || c.name == "black"), "{:?}", g.palette);
        assert!(g.summary.starts_with("Look: low-key, flat contrast,"), "{}", g.summary);
        assert!(g.summary.ends_with("steady across 2 sampled frames."), "{}", g.summary);
        let params = lumetri_params(&g);
        assert!(params.iter().any(|(id, v)| *id == "temperature" && *v > 0.0));
        assert!(params.iter().any(|(id, v)| *id == "vignette_amount" && *v < 0.0));
        let json = serde_json::to_value(&g).unwrap();
        assert_eq!(json["rules"]["lowKeyBelow"], 0.25);
        assert!(json["palette"][0]["name"].is_string());
    }

    #[test]
    fn lumetri_match_moves_the_source_tones_onto_the_target() {
        let (w, h) = (128, 72);
        let bright = frame(w, h, |x, y| {
            let v = (40 + (x * 180) / w + (y * 20) / h) as u8;
            [v.saturating_add(10), v, v.saturating_sub(10)]
        });
        let dark = frame(w, h, |x, y| {
            let v = (5 + (x * 90) / w + (y * 10) / h) as u8;
            [v.saturating_sub(5), v, v.saturating_add(25)]
        });
        let source = grade(&[sample(0, w, h, &bright)], 4).unwrap();
        let target = grade(&[sample(0, w, h, &dark)], 4).unwrap();
        let params = lumetri_match(&target, &source);
        let get = |id: &str| params.iter().find(|(k, _)| *k == id).map(|(_, v)| *v).unwrap();
        let after = |luma: f32| tone_chain(f64::from(luma), get("exposure"), get("blacks"), get("whites"));
        assert!(
            (after(source.mean_luma) - f64::from(target.mean_luma)).abs() < 0.04,
            "mean {} → {} wanted {}",
            source.mean_luma,
            after(source.mean_luma),
            target.mean_luma
        );
        assert!((after(source.white_point) - f64::from(target.white_point)).abs() < 0.08, "white {} wanted {}", after(source.white_point), target.white_point);
        assert!(get("exposure") < 0.0, "darker target lowers exposure: {params:?}");
        assert!(get("temperature") < 0.0, "a bluer target cools the temperature: {params:?}");
        assert!(get("contrast") == 0.0);
        let same = lumetri_match(&source, &source);
        let sget = |id: &str| same.iter().find(|(k, _)| *k == id).map(|(_, v)| *v).unwrap();
        assert!(sget("exposure").abs() < 0.05 && sget("blacks").abs() < 3.0 && sget("whites").abs() < 3.0, "{same:?}");
        assert!((sget("saturation") - 100.0).abs() < 0.5 && sget("temperature").abs() < 0.5 && sget("vignette_amount").abs() < 0.01);
    }

    #[test]
    fn a_flat_grey_card_and_a_noisy_card_differ_in_contrast_grain_and_names() {
        let (w, h) = (128, 128);
        let grey = frame(w, h, |_, _| [128, 128, 128]);
        let g = grade(&[sample(0, w, h, &grey)], 4).unwrap();
        assert_eq!(
            (g.contrast_class.as_str(), g.saturation_class.as_str(), g.temperature.as_str(), g.texture.as_str()),
            ("flat", "desaturated", "neutral", "clean")
        );
        assert_eq!(g.vignette_class, "no vignette");
        assert_eq!(g.palette[0].name, "grey");
        assert_eq!(g.sharpness_class, "soft");
        let noisy = frame(w, h, |x, y| {
            let v = 128i32 + (((x * 7919 + y * 104_729) % 61) as i32 - 30);
            let v = v.clamp(0, 255) as u8;
            [v, v, v]
        });
        let n = grade(&[sample(0, w, h, &noisy)], 4).unwrap();
        assert!(n.grain > g.grain && n.texture == "grainy", "{} vs {}", n.grain, g.grain);
        assert_eq!(n.sharpness_class, "sharp");
        let changing = grade(&[sample(0, w, h, &grey), sample(1, w, h, &frame(w, h, |_, _| [30, 30, 30]))], 4).unwrap();
        assert!(!changing.stable && changing.summary.contains("changing across 2 sampled frames"));
    }

    #[test]
    fn colour_names_follow_hue_luma_and_saturation() {
        assert_eq!(color_name([0, 0, 0]), "black");
        assert_eq!(color_name([250, 250, 250]), "white");
        assert_eq!(color_name([128, 128, 128]), "grey");
        assert_eq!(color_name([200, 30, 30]), "red");
        assert_eq!(color_name([40, 60, 160]), "dark blue");
        assert_eq!(color_name([120, 70, 20]), "brown");
        assert_eq!(color_name([240, 150, 40]), "orange");
        assert_eq!(color_name([40, 160, 170]), "teal");
        assert_eq!(color_name([190, 150, 240]), "light purple");
        assert_eq!(color_name([200, 190, 240]), "light blue");
        assert_eq!(color_name([60, 160, 60]), "green");
    }

    #[test]
    fn hostile_inputs_are_errors() {
        assert!(tone(0, 1, &[]).is_err());
        assert!(tone(2, 2, &[0; 8]).is_err());
        assert!(grade(&[], 4).is_err());
        let px = frame(16, 16, |_, _| [10, 20, 30]);
        let mut s = sample(0, 16, 16, &px);
        s.tone.vignette = f32::NAN;
        assert!(grade(&[s], 4).is_err());
        let t = tone(16, 16, &px).unwrap();
        assert!((0.0..=2.0).contains(&t.vignette) && t.grain == 0.0 || t.grain >= 0.0);
    }
}

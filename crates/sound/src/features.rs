//! Short-time features of a mono signal: one [`Hop`] every [`HOP`] samples from a Hann-windowed
//! [`FRAME`]-sample FFT.

use filmcraft_audio_dsp::fft::Fft;
use serde::{Deserialize, Serialize};

/// FFT frame length in samples (power of two).
pub const FRAME: usize = 2048;
/// Samples between two hops.
pub const HOP: usize = FRAME / 2;
/// Level reported for digital silence, dBFS.
pub const SILENCE_DB: f32 = -120.0;
/// Upper edge of the "low" band and lower edge of the speech band, Hz.
pub const LOW_HZ: f32 = 250.0;
/// Upper edge of the speech band and lower edge of the "high" band, Hz.
pub const HIGH_HZ: f32 = 4_000.0;

/// Features of one frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hop {
    /// RMS level of the frame, dBFS.
    pub rms_db: f32,
    /// Power-weighted mean frequency, Hz.
    pub centroid_hz: f32,
    /// Geometric over arithmetic mean of the power spectrum, 0 (one tone) to 1 (white noise).
    pub flatness: f32,
    /// Positive change of the normalized magnitude spectrum since the previous hop, 0–2.
    pub flux: f32,
    /// Zero crossings per sample, 0–1.
    pub zcr: f32,
    /// Power share below [`LOW_HZ`].
    pub low_share: f32,
    /// Power share between [`LOW_HZ`] and [`HIGH_HZ`].
    pub speech_share: f32,
    /// Power share above [`HIGH_HZ`].
    pub high_share: f32,
    /// Onset strength: positive change of the log-compressed magnitudes.
    pub onset: f32,
}

fn hann() -> Vec<f32> {
    (0..FRAME).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / FRAME as f32).cos()).collect()
}

fn finite(x: f32) -> f32 {
    if x.is_finite() { x.clamp(-1.0e6, 1.0e6) } else { 0.0 }
}

/// Features of every hop of `mono`. `progress(done, total)` returning false cancels.
pub fn extract(mono: &[f32], sample_rate: u32, progress: super::Progress) -> Result<Vec<Hop>, String> {
    if mono.is_empty() {
        return Err("the signal is empty".into());
    }
    let total = mono.len().div_ceil(HOP);
    let fft = Fft::new(FRAME);
    let window = hann();
    let bins = FRAME / 2;
    let hz_per_bin = sample_rate as f32 / FRAME as f32;
    let mut re = vec![0.0f32; FRAME];
    let mut im = vec![0.0f32; FRAME];
    let mut prev_mag = vec![0.0f32; bins];
    let mut prev_log = vec![0.0f32; bins];
    let mut mag = vec![0.0f32; bins];
    let mut log_mag = vec![0.0f32; bins];
    let mut hops = Vec::with_capacity(total);
    for h in 0..total {
        if !progress(h, total) {
            return Err("sound analysis cancelled".into());
        }
        let start = h * HOP;
        let mut sum_sq = 0.0f64;
        let mut crossings = 0usize;
        let mut last_sign = 0i8;
        let mut count = 0usize;
        for (i, (r, w)) in re.iter_mut().zip(&window).enumerate() {
            let x = mono.get(start + i).copied().map(finite).unwrap_or(0.0);
            if start + i < mono.len() {
                sum_sq += f64::from(x) * f64::from(x);
                count += 1;
                let sign = if x > 0.0 {
                    1
                } else if x < 0.0 {
                    -1
                } else {
                    0
                };
                if sign != 0 {
                    if last_sign != 0 && sign != last_sign {
                        crossings += 1;
                    }
                    last_sign = sign;
                }
            }
            *r = x * w;
        }
        im.iter_mut().for_each(|v| *v = 0.0);
        fft.forward(&mut re, &mut im);
        let mut total_power = 0.0f64;
        let mut weighted = 0.0f64;
        let mut log_sum = 0.0f64;
        let mut low = 0.0f64;
        let mut speech = 0.0f64;
        let mut high = 0.0f64;
        for k in 0..bins {
            let (r, i) = (re.get(k).copied().unwrap_or(0.0), im.get(k).copied().unwrap_or(0.0));
            let p = f64::from(r) * f64::from(r) + f64::from(i) * f64::from(i);
            let f = k as f32 * hz_per_bin;
            if k > 0 {
                total_power += p;
                weighted += p * f64::from(f);
                log_sum += (p + 1e-20).ln();
                if f < LOW_HZ {
                    low += p;
                } else if f < HIGH_HZ {
                    speech += p;
                } else {
                    high += p;
                }
            }
            if let Some(m) = mag.get_mut(k) {
                *m = p.sqrt() as f32;
            }
            if let Some(l) = log_mag.get_mut(k) {
                *l = (1.0 + 100.0 * p.sqrt() as f32 / FRAME as f32).ln();
            }
        }
        let n = (bins - 1).max(1) as f64;
        let mean_power = total_power / n;
        let flatness = if mean_power > 0.0 { ((log_sum / n).exp() / (mean_power + 1e-20)).clamp(0.0, 1.0) as f32 } else { 0.0 };
        let centroid = if total_power > 0.0 { (weighted / total_power) as f32 } else { 0.0 };
        let (low_share, speech_share, high_share) =
            if total_power > 0.0 { ((low / total_power) as f32, (speech / total_power) as f32, (high / total_power) as f32) } else { (0.0, 0.0, 0.0) };
        let mag_sum: f32 = mag.iter().sum();
        let mut flux = 0.0f32;
        let mut onset = 0.0f32;
        for k in 0..bins {
            let m = mag.get(k).copied().unwrap_or(0.0);
            let pm = prev_mag.get(k).copied().unwrap_or(0.0);
            if mag_sum > 0.0 {
                flux += (m / mag_sum - pm).max(0.0);
            }
            let l = log_mag.get(k).copied().unwrap_or(0.0);
            let pl = prev_log.get(k).copied().unwrap_or(0.0);
            onset += (l - pl).max(0.0);
        }
        for (p, m) in prev_mag.iter_mut().zip(&mag) {
            *p = if mag_sum > 0.0 { *m / mag_sum } else { 0.0 };
        }
        prev_log.copy_from_slice(&log_mag);
        let rms = if count > 0 { (sum_sq / count as f64).sqrt() } else { 0.0 };
        let rms_db = if rms > 0.0 { (20.0 * rms.log10()).max(f64::from(SILENCE_DB)) as f32 } else { SILENCE_DB };
        let zcr = if count > 1 { crossings as f32 / (count - 1) as f32 } else { 0.0 };
        hops.push(Hop { rms_db, centroid_hz: centroid, flatness, flux, zcr, low_share, speech_share, high_share, onset });
    }
    Ok(hops)
}

/// The 10th percentile of hop level over hops that are not digital silence, dBFS
/// ([`SILENCE_DB`] when everything is silent).
pub fn noise_floor(hops: &[Hop]) -> f64 {
    let mut levels: Vec<f32> = hops.iter().map(|h| h.rms_db).filter(|d| *d > -100.0).collect();
    if levels.is_empty() {
        return f64::from(SILENCE_DB);
    }
    levels.sort_by(f32::total_cmp);
    let i = (levels.len() / 10).min(levels.len() - 1);
    levels.get(i).copied().map_or(f64::from(SILENCE_DB), f64::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tone_is_flat_at_its_frequency_and_noise_is_not() {
        let sr = 16_000;
        let tone: Vec<f32> = (0..sr as usize).map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / sr as f32).sin() * 0.5).collect();
        let mut p = |_: usize, _: usize| true;
        let hops = extract(&tone, sr, &mut p).expect("hops");
        let mid = hops.get(hops.len() / 2).copied().expect("a hop");
        assert!((mid.centroid_hz - 1000.0).abs() < 60.0, "{mid:?}");
        assert!(mid.flatness < 0.05, "{mid:?}");
        assert!((mid.rms_db - (-9.0)).abs() < 1.0, "{mid:?}");
        assert!(mid.speech_share > 0.9, "{mid:?}");
        let noise = crate::synth::noise(sr as usize, 0.5, 1);
        let hops = extract(&noise, sr, &mut p).expect("hops");
        let mid = hops.get(hops.len() / 2).copied().expect("a hop");
        assert!(mid.flatness > 0.3, "{mid:?}");
        assert!(mid.zcr > 0.3, "{mid:?}");
        assert!(noise_floor(&hops) > -20.0);
        assert_eq!(noise_floor(&[]), f64::from(SILENCE_DB));
    }

    #[test]
    fn silence_and_non_finite_samples_do_not_break_the_features() {
        let mut p = |_: usize, _: usize| true;
        let hops = extract(&[0.0; 100], 48_000, &mut p).expect("hops");
        assert_eq!(hops.len(), 1);
        assert_eq!(hops[0].rms_db, SILENCE_DB);
        assert_eq!(hops[0].flatness, 0.0);
        let weird = vec![f32::NAN; 3000];
        let hops = extract(&weird, 48_000, &mut p).expect("hops");
        assert!(hops.iter().all(|h| h.rms_db == SILENCE_DB && h.onset == 0.0));
        assert!(extract(&[], 48_000, &mut p).is_err());
    }
}

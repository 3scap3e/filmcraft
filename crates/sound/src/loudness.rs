//! BS.1770 loudness of the mix through the audio-dsp meter: integrated, range, true peak and the
//! momentary curve sampled every 100 ms.

use filmcraft_audio_dsp::loudness::LoudnessMeter;
use serde::{Deserialize, Serialize};

/// Loudness of the whole signal.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Loudness {
    /// Gated integrated loudness, LUFS (None when nothing passed the gate).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integrated_lufs: Option<f64>,
    /// Loudness range, LU.
    pub range_lu: f64,
    /// True peak, dBTP (None for digital silence).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub true_peak_dbtp: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_momentary_lufs: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_short_term_lufs: Option<f64>,
    /// Seconds between two curve points.
    pub curve_seconds: f64,
    /// Momentary loudness after every 100 ms block, LUFS; −120 stands for −∞.
    pub momentary_curve: Vec<f32>,
}

const CURVE_FLOOR: f32 = -120.0;

fn finite(v: f64) -> Option<f64> {
    v.is_finite().then_some(v)
}

/// Measure `len` frames of a planar signal.
pub fn measure(channels: &[&[f32]], sample_rate: u32, len: usize) -> Loudness {
    let mut meter = LoudnessMeter::new(f64::from(sample_rate), channels.len().clamp(1, super::MAX_CHANNELS));
    let block = ((sample_rate as usize) / 10).max(1);
    let mut curve = Vec::with_capacity(len.div_ceil(block));
    let mut pos = 0usize;
    let mut scratch: Vec<Vec<f32>> = vec![Vec::with_capacity(block); channels.len()];
    while pos < len {
        let take = block.min(len - pos);
        for (s, c) in scratch.iter_mut().zip(channels) {
            s.clear();
            s.extend(c.iter().skip(pos).take(take).map(|x| if x.is_finite() { x.clamp(-64.0, 64.0) } else { 0.0 }));
        }
        let slices: Vec<&[f32]> = scratch.iter().map(Vec::as_slice).collect();
        meter.process(&slices);
        let m = meter.momentary();
        curve.push(if m.is_finite() { m.max(f64::from(CURVE_FLOOR)) as f32 } else { CURVE_FLOOR });
        pos += take;
    }
    Loudness {
        integrated_lufs: finite(meter.integrated()),
        range_lu: finite(meter.loudness_range()).unwrap_or(0.0),
        true_peak_dbtp: finite(meter.true_peak_dbtp()),
        max_momentary_lufs: finite(meter.max_momentary()),
        max_short_term_lufs: finite(meter.max_short_term()),
        curve_seconds: 0.1,
        momentary_curve: curve,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minus_20_dbfs_tone_reads_near_minus_23_lufs_and_silence_is_none() {
        let sr = 48_000;
        let tone: Vec<f32> = (0..sr as usize * 5).map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / sr as f32).sin() * 0.1).collect();
        let l = measure(&[&tone[..], &tone[..]], sr, tone.len());
        let i = l.integrated_lufs.expect("integrated");
        assert!((i - (-20.0)).abs() < 1.5, "{l:?}");
        assert_eq!(l.momentary_curve.len(), 50);
        assert!(l.true_peak_dbtp.is_some_and(|p| (p - (-20.0)).abs() < 1.0));
        let silence = vec![0.0f32; sr as usize];
        let l = measure(&[&silence[..]], sr, silence.len());
        assert!(l.integrated_lufs.is_none());
        assert!(l.true_peak_dbtp.is_none());
        assert!(l.momentary_curve.iter().all(|v| *v == CURVE_FLOOR));
    }
}

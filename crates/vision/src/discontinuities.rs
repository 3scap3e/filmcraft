//! Abrupt changes in sampled-flow summaries; candidate events, not shot detection.
use crate::motion_time::Sample;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub frame_index: usize,
    pub timeline_tick: i64,
    pub direction_change_pixels: Option<f64>,
    pub strength_change_pixels: Option<f64>,
    pub score_pixels: Option<f64>,
    pub flagged: bool,
}
pub fn detect(samples: &[Sample], threshold: f64) -> Result<Vec<Change>, String> {
    if !threshold.is_finite() || !(0.001..=100.0).contains(&threshold) || samples.len() > 299 {
        return Err("Threshold must be 0.001–100 analysis pixels; at most 299 comparisons".into());
    }
    for s in samples {
        if s.to_tick <= s.from_tick
            || s.sample_count > 2048
            || [s.mean_dx_pixels, s.mean_dy_pixels, s.mean_magnitude_pixels].iter().flatten().any(|v| !v.is_finite() || v.abs() > 30000.0)
            || s.mean_magnitude_pixels.is_some_and(|v| v < 0.0)
        {
            return Err("Invalid motion summary".into());
        }
    }
    let mut out = Vec::new();
    for (i, pair) in samples.windows(2).enumerate() {
        let a = &pair[0];
        let b = &pair[1];
        let values = if a.to_tick == b.from_tick && a.sample_count > 0 && b.sample_count > 0 {
            match (a.mean_dx_pixels, a.mean_dy_pixels, a.mean_magnitude_pixels, b.mean_dx_pixels, b.mean_dy_pixels, b.mean_magnitude_pixels) {
                (Some(ax), Some(ay), Some(am), Some(bx), Some(by), Some(bm)) => Some(((bx - ax).hypot(by - ay), (bm - am).abs())),
                _ => None,
            }
        } else {
            None
        };
        let direction = values.map(|v| v.0);
        let strength = values.map(|v| v.1);
        let score = values.map(|v| v.0.max(v.1));
        out.push(Change {
            frame_index: i + 2,
            timeline_tick: b.to_tick,
            direction_change_pixels: direction,
            strength_change_pixels: strength,
            score_pixels: score,
            flagged: score.is_some_and(|v| v >= threshold),
        });
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn s(t: i64, x: f64) -> Sample {
        Sample { from_tick: t, to_tick: t + 1, sample_count: 4, mean_dx_pixels: Some(x), mean_dy_pixels: Some(0.0), mean_magnitude_pixels: Some(x.abs()) }
    }
    #[test]
    fn steady_and_reversal() {
        let r = detect(&[s(0, 1.0), s(1, 1.0), s(2, -1.0)], 0.5).unwrap();
        assert!(!r[0].flagged);
        assert!(r[1].flagged);
        assert_eq!(r[1].direction_change_pixels, Some(2.0));
        assert_eq!(r[1].strength_change_pixels, Some(0.0));
        assert_eq!(r[1].frame_index, 3);
    }
    #[test]
    fn missing_and_gap_are_unavailable() {
        let mut a = s(0, 0.0);
        a.mean_dx_pixels = None;
        assert_eq!(detect(&[a, s(1, 2.0)], 0.5).unwrap()[0].score_pixels, None);
        assert_eq!(detect(&[s(0, 0.0), s(3, 2.0)], 0.5).unwrap()[0].score_pixels, None);
        assert!(detect(&[], f64::NAN).is_err());
    }
    #[test]
    fn strength_jump_and_threshold() {
        let mut b = s(1, 0.0);
        b.mean_magnitude_pixels = Some(3.0);
        let ss = [s(0, 0.0), b];
        assert!(detect(&ss, 2.0).unwrap()[0].flagged);
        assert!(!detect(&ss, 4.0).unwrap()[0].flagged);
    }
}

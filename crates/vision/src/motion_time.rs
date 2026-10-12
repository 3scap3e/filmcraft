//! Temporal summaries of sampled optical flow; never object or camera labels.
use crate::MotionPair;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sample {
    pub from_tick: i64,
    pub to_tick: i64,
    pub sample_count: usize,
    pub mean_dx_pixels: Option<f64>,
    pub mean_dy_pixels: Option<f64>,
    pub mean_magnitude_pixels: Option<f64>,
}
pub fn summarize(pair: &MotionPair) -> Result<Sample, String> {
    let [w, h] = pair.analyzed_size;
    if w == 0 || h == 0 || w > 640 || h > 640 || pair.vectors.len() > 2048 || pair.to_timeline_tick <= pair.from_timeline_tick {
        return Err("Invalid motion comparison".into());
    }
    let mut dx = 0.0;
    let mut dy = 0.0;
    let mut magnitude = 0.0;
    for v in &pair.vectors {
        if v.from.iter().chain(v.to.iter()).any(|v| !v.is_finite() || v.abs() > 16.0) {
            return Err("Invalid flow coordinates".into());
        }
        let x = (f64::from(v.to[0]) - f64::from(v.from[0])) * w as f64;
        let y = (f64::from(v.to[1]) - f64::from(v.from[1])) * h as f64;
        dx += x;
        dy += y;
        magnitude += x.hypot(y);
    }
    let n = pair.vectors.len();
    Ok(Sample {
        from_tick: pair.from_timeline_tick,
        to_tick: pair.to_timeline_tick,
        sample_count: n,
        mean_dx_pixels: (n > 0).then(|| dx / n as f64),
        mean_dy_pixels: (n > 0).then(|| dy / n as f64),
        mean_magnitude_pixels: (n > 0).then(|| magnitude / n as f64),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn pair() -> MotionPair {
        MotionPair {
            from_timeline_tick: 0,
            to_timeline_tick: 1,
            from_source_tick: 0,
            to_source_tick: 1,
            analyzed_size: [100, 100],
            vectors: vec![],
            rejected_points: 0,
            mean_pixel_change: 0.0,
            changed_pixel_fraction: 0.0,
            mean_motion_pixels: None,
            dense_pixel_count: None,
        }
    }
    #[test]
    fn empty_is_missing_not_zero() {
        let p = pair();
        assert_eq!(summarize(&p).unwrap().mean_dx_pixels, None);
    }
    #[test]
    fn opposite_motion_does_not_cancel_strength() {
        let mut p = pair();
        for x in [0.6, 0.4] {
            p.vectors.push(crate::MotionVector {
                from: [0.5, 0.5],
                to: [x, 0.5],
                mean_absolute_error: None,
                ambiguity_margin: None,
                uncertainty_log_scale: None,
            });
        }
        let s = summarize(&p).unwrap();
        assert!(s.mean_dx_pixels.unwrap().abs() < 0.00001);
        assert!((s.mean_magnitude_pixels.unwrap() - 10.0).abs() < 0.00001);
        p.vectors[0].to[0] = f32::NAN;
        assert!(summarize(&p).is_err());
    }
}

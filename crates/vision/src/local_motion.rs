//! Residual image motion: observed SEA-RAFT displacement minus a fitted similarity transform.
use crate::{MotionPair, global_motion::GlobalMotion};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResidualSample {
    pub source_sample_index: usize,
    pub source_position: [f64; 2],
    pub observed_position: [f64; 2],
    pub observed_displacement_pixels: [f64; 2],
    pub global_displacement_pixels: [f64; 2],
    pub residual_displacement_pixels: [f64; 2],
    pub uncertainty_log_scale: Option<f32>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum LocalMotion {
    Estimated {
        from_timeline_tick: i64,
        to_timeline_tick: i64,
        analyzed_size: [usize; 2],
        samples: Vec<ResidualSample>,
        mean_residual_pixels: f64,
        max_residual_pixels: f64,
    },
    Unavailable {
        reason: String,
    },
}
/// Do not invent a stationary camera when a global fit is unavailable.
pub fn estimate(pair: &MotionPair, global: &GlobalMotion) -> Result<LocalMotion, String> {
    let [w, h] = pair.analyzed_size;
    if w == 0 || h == 0 || w > 640 || h > 640 || pair.vectors.len() > 2048 {
        return Err("Invalid local-motion dimensions or sample count".into());
    }
    let fit = match global {
        GlobalMotion::Estimated { fit } => fit,
        GlobalMotion::InsufficientEvidence { reason, samples } => {
            return Ok(LocalMotion::Unavailable {
                reason: format!("Global fit unavailable: {reason} ({samples} valid samples). Residual motion cannot be calculated for this comparison."),
            });
        }
    };
    if fit.analyzed_size != pair.analyzed_size || !fit.transform.iter().all(|v| v.is_finite()) {
        return Err("Global transform does not match the analyzed image".into());
    }
    let [a, b, tx, ty] = fit.transform;
    let mut samples = Vec::with_capacity(pair.vectors.len());
    let (mut sum, mut maximum) = (0.0, 0.0f64);
    for (index, vector) in pair.vectors.iter().enumerate() {
        if !vector.from.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)) || !vector.to.iter().all(|v| v.is_finite() && (-8.0..=9.0).contains(v)) {
            return Err("Invalid SEA-RAFT sample coordinates".into());
        }
        let [x, y] = [f64::from(vector.from[0]) * w as f64, f64::from(vector.from[1]) * h as f64];
        let observed = [f64::from(vector.to[0]) * w as f64 - x, f64::from(vector.to[1]) * h as f64 - y];
        let predicted = [a * x - b * y + tx - x, b * x + a * y + ty - y];
        let residual = [observed[0] - predicted[0], observed[1] - predicted[1]];
        let magnitude = residual[0].hypot(residual[1]);
        if !magnitude.is_finite() {
            return Err("Nonfinite residual motion".into());
        }
        sum += magnitude;
        if !sum.is_finite() || vector.uncertainty_log_scale.is_some_and(|v| !v.is_finite()) {
            return Err("Nonfinite local-motion evidence".into());
        }
        maximum = maximum.max(magnitude);
        samples.push(ResidualSample {
            source_sample_index: index,
            source_position: vector.from.map(f64::from),
            observed_position: vector.to.map(f64::from),
            observed_displacement_pixels: observed,
            global_displacement_pixels: predicted,
            residual_displacement_pixels: residual,
            uncertainty_log_scale: vector.uncertainty_log_scale,
        });
    }
    if samples.is_empty() {
        return Ok(LocalMotion::Unavailable { reason: "No optical-flow samples available".into() });
    }
    Ok(LocalMotion::Estimated {
        from_timeline_tick: pair.from_timeline_tick,
        to_timeline_tick: pair.to_timeline_tick,
        analyzed_size: [w, h],
        mean_residual_pixels: sum / samples.len() as f64,
        max_residual_pixels: maximum,
        samples,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MotionVector, global_motion::Similarity};
    fn fixture(transform: [f64; 4], local: [f64; 2]) -> (MotionPair, GlobalMotion) {
        let [a, b, tx, ty] = transform;
        let x = 32.0;
        let y = 16.0;
        let mut pair = MotionPair {
            from_timeline_tick: 0,
            to_timeline_tick: 1,
            from_source_tick: 0,
            to_source_tick: 1,
            analyzed_size: [64, 64],
            vectors: vec![],
            rejected_points: 0,
            mean_pixel_change: 0.0,
            changed_pixel_fraction: 0.0,
            mean_motion_pixels: None,
            dense_pixel_count: None,
        };
        pair.vectors.push(MotionVector {
            from: [0.5, 0.25],
            to: [((a * x - b * y + tx + local[0]) / 64.0) as f32, ((b * x + a * y + ty + local[1]) / 64.0) as f32],
            mean_absolute_error: None,
            ambiguity_margin: None,
            uncertainty_log_scale: Some(0.3),
        });
        let global = GlobalMotion::Estimated {
            fit: Similarity {
                transform,
                analyzed_size: [64, 64],
                center_displacement_pixels: [0.0, 0.0],
                rotation_degrees: 0.0,
                scale: 1.0,
                rms_error_pixels: 0.0,
                support_fraction: 1.0,
                supporting_sample_indices: vec![0],
                supporting_samples: vec![],
                tested_samples: 1,
            },
        };
        (pair, global)
    }
    #[test]
    fn shared_translation_cancels_and_local_displacement_survives() {
        for local in [[0.0, 0.0], [2.0, -3.0]] {
            let (pair, global) = fixture([1.0, 0.0, 4.0, 2.0], local);
            let LocalMotion::Estimated { samples, .. } = estimate(&pair, &global).unwrap() else { panic!("missing residual") };
            assert_eq!(samples[0].residual_displacement_pixels, local);
            assert_eq!(samples[0].source_sample_index, 0);
            assert_eq!(samples[0].uncertainty_log_scale, Some(0.3));
        }
    }
    #[test]
    fn rotation_and_scale_are_removed_at_each_sample_position() {
        let (pair, global) = fixture([1.125, 0.25, -2.0, 3.0], [1.0, 2.0]);
        let LocalMotion::Estimated { samples, .. } = estimate(&pair, &global).unwrap() else { panic!("missing residual") };
        assert_eq!(samples[0].residual_displacement_pixels, [1.0, 2.0]);
    }
    #[test]
    fn missing_fit_and_hostile_inputs_do_not_produce_fake_measurements() {
        let (mut pair, mut global) = fixture([1.0, 0.0, 0.0, 0.0], [0.0, 0.0]);
        assert!(matches!(estimate(&pair, &GlobalMotion::InsufficientEvidence { reason: "test".into(), samples: 1 }).unwrap(), LocalMotion::Unavailable { .. }));
        pair.vectors[0].to[0] = f32::NAN;
        assert!(estimate(&pair, &global).is_err());
        pair.vectors[0].to[0] = 0.5;
        if let GlobalMotion::Estimated { fit } = &mut global {
            fit.transform[0] = f64::INFINITY;
        }
        assert!(estimate(&pair, &global).is_err());
        pair.analyzed_size = [0, 64];
        assert!(estimate(&pair, &global).is_err());
    }
}

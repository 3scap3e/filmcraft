//! Rust SEA-RAFT connector and serializable motion evidence. No project mutations.
use serde::{Deserialize, Serialize};

pub mod audit;
pub mod display;
pub mod execution;
pub mod global_motion;
pub mod local_motion;
pub mod motion_regions;
pub mod point_tracking;
pub mod point_tracking_chunks;
pub mod sea_raft;
pub use sea_raft::SeaRaft;
pub const SIZE: usize = sea_raft::MAX_SIZE;
/// Coordinates are normalized to the analyzed source image, not camera angles.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MotionVector {
    pub from: [f32; 2],
    pub to: [f32; 2],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mean_absolute_error: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ambiguity_margin: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uncertainty_log_scale: Option<f32>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MotionPair {
    pub from_timeline_tick: i64,
    pub to_timeline_tick: i64,
    pub from_source_tick: i64,
    pub to_source_tick: i64,
    pub analyzed_size: [usize; 2],
    pub vectors: Vec<MotionVector>,
    pub rejected_points: usize,
    #[serde(default)]
    pub mean_pixel_change: f32,
    #[serde(default)]
    pub changed_pixel_fraction: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mean_motion_pixels: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dense_pixel_count: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_vector_is_readable_but_model_output_has_no_patch_metrics() {
        let old: MotionVector = serde_json::from_str(r#"{"from":[0.5,0.5],"to":[0.5,0.5],"meanAbsoluteError":1.0,"ambiguityMargin":3.0}"#).unwrap();
        assert_eq!(old.mean_absolute_error, Some(1.0));
        let model = MotionVector { from: [0.5, 0.5], to: [0.6, 0.5], mean_absolute_error: None, ambiguity_margin: None, uncertainty_log_scale: Some(0.2) };
        let value = serde_json::to_value(model).unwrap();
        assert!(value.get("meanAbsoluteError").is_none());
        assert!(value.get("ambiguityMargin").is_none());
        assert!(value.get("uncertaintyLogScale").is_some());
    }
}

/// Atomically replace exactly one frame comparison after successful reanalysis.
pub fn replace_pair(pairs: &mut [MotionPair], updated: &[MotionPair], index: usize) -> Result<(), String> {
    if updated.len() != 1 {
        return Err("Single-frame reanalysis must return exactly one comparison".into());
    }
    let incoming = updated.first().ok_or("Missing reanalysis result")?;
    let target = pairs.get_mut(index).ok_or("Reanalysis frame is outside the original range")?;
    if (target.from_timeline_tick, target.to_timeline_tick, target.from_source_tick, target.to_source_tick)
        != (incoming.from_timeline_tick, incoming.to_timeline_tick, incoming.from_source_tick, incoming.to_source_tick)
    {
        return Err("Reanalysis timestamps differ from the original frame comparison".into());
    }
    *target = incoming.clone();
    Ok(())
}
#[cfg(test)]
mod replacement_tests {
    use super::*;
    fn pair(frame: i64) -> MotionPair {
        MotionPair {
            from_timeline_tick: frame,
            to_timeline_tick: frame + 1,
            from_source_tick: frame,
            to_source_tick: frame + 1,
            analyzed_size: [640, 360],
            vectors: vec![],
            rejected_points: 0,
            mean_pixel_change: 0.0,
            changed_pixel_fraction: 0.0,
            mean_motion_pixels: Some(0.0),
            dense_pixel_count: Some(640 * 360),
        }
    }
    #[test]
    fn reanalysis_changes_only_matching_pair_and_bad_results_are_atomic() {
        let mut original = vec![pair(0), pair(1), pair(2)];
        let before = original.clone();
        assert!(replace_pair(&mut original, &[pair(2)], 1).is_err());
        assert_eq!(original, before);
        assert!(replace_pair(&mut original, &[], 1).is_err());
        assert_eq!(original, before);
        assert!(replace_pair(&mut original, &[pair(1)], usize::MAX).is_err());
        assert_eq!(original, before);
        let mut updated = pair(1);
        updated.mean_motion_pixels = Some(0.2);
        replace_pair(&mut original, &[updated.clone()], 1).unwrap();
        assert_eq!(original[0], before[0]);
        assert_eq!(original[1], updated);
        assert_eq!(original[2], before[2]);
    }
}

pub mod motion_time;

pub mod discontinuities;

pub mod sam2;

pub mod brief;
pub mod camera2d;
pub mod camera_moves;
pub mod cinema;
pub mod features;
pub mod grade;
pub mod look;
pub mod matching;
pub mod motion;
pub mod shots;
pub mod trajectory;

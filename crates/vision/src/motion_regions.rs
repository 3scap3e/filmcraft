//! Connected groups of neighboring SEA-RAFT residual samples, not semantic segmentation.
use crate::local_motion::LocalMotion;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    pub cutoff_pixels: f64,
    pub neighbor_difference_pixels: f64,
    pub minimum_samples: usize,
}
impl Default for Settings {
    fn default() -> Self {
        Self { cutoff_pixels: 0.3, neighbor_difference_pixels: 0.5, minimum_samples: 3 }
    }
}
impl Settings {
    pub fn validate(self) -> Result<Self, String> {
        if !self.cutoff_pixels.is_finite()
            || !(0.0..=100.0).contains(&self.cutoff_pixels)
            || !self.neighbor_difference_pixels.is_finite()
            || !(0.01..=100.0).contains(&self.neighbor_difference_pixels)
            || !(1..=2048).contains(&self.minimum_samples)
        {
            return Err("Region settings require cutoff 0–100 px, neighbor difference 0.01–100 px, minimum 1–2048 samples".into());
        }
        Ok(self)
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Region {
    pub id: usize,
    pub source_sample_indices: Vec<usize>,
    pub observed_positions: Vec<[f64; 2]>,
    pub observed_bounds: [f64; 4],
    pub mean_residual_pixels: [f64; 2],
    pub mean_magnitude_pixels: f64,
    pub maximum_magnitude_pixels: f64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Regions {
    Estimated {
        from_timeline_tick: i64,
        to_timeline_tick: i64,
        analyzed_size: [usize; 2],
        regions: Vec<Region>,
        filtered_samples: usize,
        discarded_groups: usize,
    },
    Unavailable {
        reason: String,
    },
}
pub fn group(pair: &LocalMotion, settings: Settings) -> Result<Regions, String> {
    let settings = settings.validate()?;
    let LocalMotion::Estimated { from_timeline_tick, to_timeline_tick, analyzed_size, samples, .. } = pair else {
        let LocalMotion::Unavailable { reason } = pair else {
            return Err("Unsupported local motion".into());
        };
        return Ok(Regions::Unavailable { reason: reason.clone() });
    };
    let [w, h] = *analyzed_size;
    if w == 0 || h == 0 || w > 640 || h > 640 || samples.len() > 2048 {
        return Err("Invalid region evidence dimensions or count".into());
    }
    let mut cells = BTreeMap::new();
    let mut source_indices = BTreeSet::new();
    for (index, sample) in samples.iter().enumerate() {
        if !sample.source_position.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v))
            || !sample.observed_position.iter().all(|v| v.is_finite() && (-8.0..=9.0).contains(v))
            || !sample.residual_displacement_pixels.iter().all(|v| v.is_finite() && v.abs() <= 100_000.0)
            || sample.source_sample_index >= 2048
            || !source_indices.insert(sample.source_sample_index)
        {
            return Err("Invalid or duplicate residual sample".into());
        }
        let magnitude = sample.residual_displacement_pixels[0].hypot(sample.residual_displacement_pixels[1]);
        if magnitude <= settings.cutoff_pixels {
            continue;
        }
        let x = (sample.source_position[0] * w as f64 - 8.0) / 16.0;
        let y = (sample.source_position[1] * h as f64 - 8.0) / 16.0;
        if (x - x.round()).abs() > 0.01 || (y - y.round()).abs() > 0.01 {
            return Err("Motion regions require the original 16-pixel SEA-RAFT sample grid".into());
        }
        if cells.insert((x.round() as i32, y.round() as i32), index).is_some() {
            return Err("Duplicate source-grid position".into());
        }
    }
    let filtered_samples = samples.len().saturating_sub(cells.len());
    let mut visited = BTreeSet::new();
    let mut regions = Vec::new();
    let mut discarded_groups = 0;
    for (&cell, &index) in &cells {
        if !visited.insert(cell) {
            continue;
        }
        let mut queue = VecDeque::from([(cell, index)]);
        let mut members = Vec::new();
        while let Some(((x, y), index)) = queue.pop_front() {
            let sample = samples.get(index).ok_or("Missing region sample")?;
            members.push(index);
            for next in [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)] {
                if visited.contains(&next) {
                    continue;
                }
                let Some(&other) = cells.get(&next) else {
                    continue;
                };
                let target = samples.get(other).ok_or("Missing neighboring sample")?;
                let delta = (sample.residual_displacement_pixels[0] - target.residual_displacement_pixels[0])
                    .hypot(sample.residual_displacement_pixels[1] - target.residual_displacement_pixels[1]);
                if delta <= settings.neighbor_difference_pixels {
                    visited.insert(next);
                    queue.push_back((next, other));
                }
            }
        }
        if members.len() < settings.minimum_samples {
            discarded_groups += 1;
            continue;
        }
        let mut region = Region {
            id: regions.len() + 1,
            source_sample_indices: Vec::new(),
            observed_positions: Vec::new(),
            observed_bounds: [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY],
            mean_residual_pixels: [0.0, 0.0],
            mean_magnitude_pixels: 0.0,
            maximum_magnitude_pixels: 0.0,
        };
        for index in &members {
            let sample = samples.get(*index).ok_or("Missing region member")?;
            region.source_sample_indices.push(sample.source_sample_index);
            region.observed_positions.push(sample.observed_position);
            for k in 0..2 {
                let half = 8.0 / [w, h][k] as f64;
                region.observed_bounds[k] = region.observed_bounds[k].min(sample.observed_position[k] - half);
                region.observed_bounds[k + 2] = region.observed_bounds[k + 2].max(sample.observed_position[k] + half);
                region.mean_residual_pixels[k] += sample.residual_displacement_pixels[k];
            }
            let magnitude = sample.residual_displacement_pixels[0].hypot(sample.residual_displacement_pixels[1]);
            region.mean_magnitude_pixels += magnitude;
            region.maximum_magnitude_pixels = region.maximum_magnitude_pixels.max(magnitude);
        }
        for v in &mut region.mean_residual_pixels {
            *v /= members.len() as f64;
        }
        region.mean_magnitude_pixels /= members.len() as f64;
        regions.push(region);
    }
    Ok(Regions::Estimated {
        from_timeline_tick: *from_timeline_tick,
        to_timeline_tick: *to_timeline_tick,
        analyzed_size: *analyzed_size,
        regions,
        filtered_samples,
        discarded_groups,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_motion::ResidualSample;
    fn fixture() -> LocalMotion {
        let samples = (0..8)
            .map(|i| {
                let d = if i < 4 { [1.0, 0.0] } else { [-1.0, 0.0] };
                let p = [(8.0 + 16.0 * i as f64) / 128.0, 8.0 / 64.0];
                ResidualSample {
                    source_sample_index: i,
                    source_position: p,
                    observed_position: p,
                    observed_displacement_pixels: d,
                    global_displacement_pixels: [0.0, 0.0],
                    residual_displacement_pixels: d,
                    uncertainty_log_scale: None,
                }
            })
            .collect();
        LocalMotion::Estimated {
            from_timeline_tick: 0,
            to_timeline_tick: 1,
            analyzed_size: [128, 64],
            samples,
            mean_residual_pixels: 1.0,
            max_residual_pixels: 1.0,
        }
    }
    #[test]
    fn adjoining_opposite_motion_stays_in_two_groups_and_indices_survive() {
        let Regions::Estimated { regions, .. } = group(&fixture(), Settings::default()).unwrap() else { panic!("missing groups") };
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0].source_sample_indices, vec![0, 1, 2, 3]);
        assert_eq!(regions[1].source_sample_indices, vec![4, 5, 6, 7]);
        assert_eq!(regions[0].mean_residual_pixels, [1.0, 0.0]);
        assert_eq!(regions[1].mean_residual_pixels, [-1.0, 0.0]);
    }
    #[test]
    fn thresholds_and_minimum_size_change_groups_without_changing_input() {
        let input = fixture();
        let before = input.clone();
        let Regions::Estimated { regions, .. } = group(&input, Settings { cutoff_pixels: 1.0, ..Default::default() }).unwrap() else {
            panic!("missing result")
        };
        assert!(regions.is_empty());
        let Regions::Estimated { regions, discarded_groups, .. } = group(&input, Settings { minimum_samples: 5, ..Default::default() }).unwrap() else {
            panic!("missing result")
        };
        assert!(regions.is_empty());
        assert_eq!(discarded_groups, 2);
        let Regions::Estimated { regions, .. } = group(&input, Settings { neighbor_difference_pixels: 2.0, ..Default::default() }).unwrap() else {
            panic!("missing result")
        };
        assert_eq!(regions.len(), 1);
        assert_eq!(input, before);
    }
    #[test]
    fn unavailable_and_invalid_inputs_are_explicit() {
        assert_eq!(
            group(&LocalMotion::Unavailable { reason: "no fit".into() }, Settings::default()).unwrap(),
            Regions::Unavailable { reason: "no fit".into() }
        );
        assert!(group(&fixture(), Settings { cutoff_pixels: f64::NAN, ..Default::default() }).is_err());
        let mut input = fixture();
        if let LocalMotion::Estimated { samples, .. } = &mut input {
            samples[0].source_position[0] = f64::INFINITY;
        }
        assert!(group(&input, Settings::default()).is_err());
    }
}

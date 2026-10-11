//! User-controlled presentation of model evidence. Never changes predictions.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct MotionDisplay {
    pub enabled: bool,
    pub cutoff_pixels: f32,
    pub arrow_scale: f32,
    /// Display every Nth row and column of the model's exported 16-pixel grid.
    pub sample_stride: u32,
}
impl Default for MotionDisplay {
    fn default() -> Self {
        Self { enabled: true, cutoff_pixels: 0.0, arrow_scale: 1.0, sample_stride: 2 }
    }
}
impl MotionDisplay {
    pub fn validate(self) -> Result<Self, String> {
        if !self.cutoff_pixels.is_finite()
            || !(0.0..=100.0).contains(&self.cutoff_pixels)
            || !self.arrow_scale.is_finite()
            || !(0.1..=32.0).contains(&self.arrow_scale)
            || !(1..=8).contains(&self.sample_stride)
        {
            return Err("Display settings require cutoff 0–100 pixels, arrow scale 0.1–32, and grid stride 1–8".into());
        }
        Ok(self)
    }
    pub fn includes(self, from: [f32; 2], to: [f32; 2], size: [usize; 2]) -> bool {
        if self.validate().is_err()
            || !self.enabled
            || size.contains(&0)
            || !from.iter().all(|n| n.is_finite() && (0.0..=1.0).contains(n))
            || !to.iter().all(|n| n.is_finite() && (-8.0..=9.0).contains(n))
        {
            return false;
        }
        let magnitude = ((to[0] - from[0]) * size[0] as f32).hypot((to[1] - from[1]) * size[1] as f32);
        let column = ((from[0] * size[0] as f32 - 8.0) / 16.0).round().max(0.0) as u32;
        let row = ((from[1] * size[1] as f32 - 8.0) / 16.0).round().max(0.0) as u32;
        magnitude >= self.cutoff_pixels && column.is_multiple_of(self.sample_stride) && row.is_multiple_of(self.sample_stride)
    }
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MotionDisplays {
    pub all_frames: MotionDisplay,
    /// A frame is identified by its incoming frame-pair index, within this report.
    pub frame_overrides: BTreeMap<usize, MotionDisplay>,
}
impl MotionDisplays {
    pub fn effective(&self, index: usize) -> MotionDisplay {
        self.frame_overrides.get(&index).copied().unwrap_or(self.all_frames).validate().unwrap_or_default()
    }
    pub fn apply(&mut self, scope: &str, index: usize, pair_count: usize, settings: MotionDisplay) -> Result<(), String> {
        if pair_count == 0 || pair_count > 299 || index >= pair_count {
            return Err("Frame is outside this analysis".into());
        }
        let settings = settings.validate()?;
        match scope {
            "frame" => {
                self.frame_overrides.insert(index, settings);
            }
            "all" => {
                self.all_frames = settings;
                self.frame_overrides.clear();
            }
            "inherit" => {
                self.frame_overrides.remove(&index);
            }
            _ => return Err("Display scope must be frame, all, or inherit".into()),
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frame_overrides_inheritance_and_apply_all_are_separate() {
        let mut view = MotionDisplays::default();
        let custom = MotionDisplay { cutoff_pixels: 0.3, arrow_scale: 4.0, ..Default::default() };
        view.apply("frame", 1, 3, custom).unwrap();
        assert_eq!(view.effective(0), MotionDisplay::default());
        assert_eq!(view.effective(1), custom);
        view.apply("inherit", 1, 3, custom).unwrap();
        assert_eq!(view.effective(1), MotionDisplay::default());
        view.apply("frame", 2, 3, custom).unwrap();
        view.apply("all", 1, 3, custom).unwrap();
        assert!(view.frame_overrides.is_empty());
        assert_eq!(view.effective(0), custom);
        assert_eq!(view.effective(2), custom);
        assert_eq!(serde_json::from_value::<MotionDisplays>(serde_json::to_value(&view).unwrap()).unwrap(), view);
    }
    #[test]
    fn invalid_settings_do_not_mutate_preferences() {
        let mut view = MotionDisplays::default();
        let before = view.clone();
        for settings in [
            MotionDisplay { cutoff_pixels: f32::NAN, ..Default::default() },
            MotionDisplay { arrow_scale: f32::INFINITY, ..Default::default() },
            MotionDisplay { sample_stride: 0, ..Default::default() },
        ] {
            assert!(view.apply("all", 0, 3, settings).is_err());
            assert_eq!(view, before);
        }
        assert!(view.apply("frame", usize::MAX, 3, MotionDisplay::default()).is_err());
        assert!(view.apply("unknown", 0, 3, MotionDisplay::default()).is_err());
        assert_eq!(view, before);
    }
    #[test]
    fn cutoff_and_spatial_sampling_filter_only_display() {
        let settings = MotionDisplay { sample_stride: 2, cutoff_pixels: 0.5, ..Default::default() };
        assert!(settings.includes([0.08, 0.08], [0.09, 0.08], [100, 100]));
        assert!(!settings.includes([0.08, 0.08], [0.081, 0.08], [100, 100]));
        assert!(!settings.includes([0.24, 0.08], [0.25, 0.08], [100, 100]));
        assert!(!MotionDisplay { enabled: false, ..settings }.includes([0.08, 0.08], [0.09, 0.08], [100, 100]));
    }
}

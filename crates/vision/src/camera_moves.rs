//! Camera-language labels for fitted global image motion, under a static-scene assumption. Not physical pose.
use crate::global_motion::GlobalMotion;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Movement {
    Static,
    PanLeft,
    PanRight,
    TiltUp,
    TiltDown,
    ZoomIn,
    ZoomOut,
    RollClockwise,
    RollCounterclockwise,
    Mixed,
    Unavailable,
}
impl Movement {
    pub fn label(self) -> &'static str {
        match self {
            Self::Static => "static",
            Self::PanLeft => "pan left",
            Self::PanRight => "pan right",
            Self::TiltUp => "tilt up",
            Self::TiltDown => "tilt down",
            Self::ZoomIn => "zoom in",
            Self::ZoomOut => "zoom out",
            Self::RollClockwise => "roll clockwise",
            Self::RollCounterclockwise => "roll counterclockwise",
            Self::Mixed => "mixed movement",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Options {
    pub move_pixels: f64,
    pub zoom_fraction: f64,
    pub roll_degrees: f64,
}
impl Default for Options {
    fn default() -> Self {
        Self { move_pixels: 0.25, zoom_fraction: 0.002, roll_degrees: 0.1 }
    }
}
impl Options {
    pub fn validate(&self) -> Result<(), String> {
        if !self.move_pixels.is_finite()
            || !(0.001..=1000.0).contains(&self.move_pixels)
            || !self.zoom_fraction.is_finite()
            || !(0.00001..=1.0).contains(&self.zoom_fraction)
            || !self.roll_degrees.is_finite()
            || !(0.001..=90.0).contains(&self.roll_degrees)
        {
            return Err("Camera options: movement 0.001–1000 px, zoom 0.00001–1, roll 0.001–90°".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Classification {
    pub comparison_index: usize,
    pub movement: Movement,
    pub dx_pixels: Option<f64>,
    pub dy_pixels: Option<f64>,
    pub scale: Option<f64>,
    pub rotation_degrees: Option<f64>,
    pub zoom_pixels: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    pub movement: Movement,
    pub start_comparison: usize,
    pub end_comparison: usize,
    pub comparisons: usize,
    pub mean_dx_pixels: Option<f64>,
    pub mean_dy_pixels: Option<f64>,
    pub mean_scale: Option<f64>,
    pub mean_rotation_degrees: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Share {
    pub movement: Movement,
    pub comparisons: usize,
    pub fraction: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub options: Options,
    pub classifications: Vec<Classification>,
    pub segments: Vec<Segment>,
    pub shares: Vec<Share>,
    pub dominant: Movement,
}

fn classify_one(index: usize, pair: &GlobalMotion, options: &Options) -> Result<Classification, String> {
    let GlobalMotion::Estimated { fit } = pair else {
        return Ok(Classification {
            comparison_index: index,
            movement: Movement::Unavailable,
            dx_pixels: None,
            dy_pixels: None,
            scale: None,
            rotation_degrees: None,
            zoom_pixels: None,
        });
    };
    let [dx, dy] = fit.center_displacement_pixels;
    if !dx.is_finite() || !dy.is_finite() || !fit.scale.is_finite() || !fit.rotation_degrees.is_finite() || fit.scale <= 0.0 {
        return Err("Invalid global motion fit".into());
    }
    let [w, h] = fit.analyzed_size;
    if w == 0 || h == 0 || w > 8192 || h > 8192 {
        return Err("Invalid analysis dimensions".into());
    }
    let half_diagonal = ((w * w + h * h) as f64).sqrt() / 2.0;
    let zoom_pixels = (fit.scale - 1.0).abs() * half_diagonal;
    let translating = dx.hypot(dy) >= options.move_pixels;
    let zooming = (fit.scale - 1.0).abs() >= options.zoom_fraction;
    let rolling = fit.rotation_degrees.abs() >= options.roll_degrees;
    let movement = match (translating, zooming, rolling) {
        (false, false, false) => Movement::Static,
        (true, false, false) => {
            if dx.abs() >= dy.abs() {
                if dx > 0.0 { Movement::PanLeft } else { Movement::PanRight }
            } else if dy > 0.0 {
                Movement::TiltUp
            } else {
                Movement::TiltDown
            }
        }
        (false, true, false) => {
            if fit.scale > 1.0 {
                Movement::ZoomIn
            } else {
                Movement::ZoomOut
            }
        }
        (false, false, true) => {
            if fit.rotation_degrees > 0.0 {
                Movement::RollCounterclockwise
            } else {
                Movement::RollClockwise
            }
        }
        _ => Movement::Mixed,
    };
    Ok(Classification {
        comparison_index: index,
        movement,
        dx_pixels: Some(dx),
        dy_pixels: Some(dy),
        scale: Some(fit.scale),
        rotation_degrees: Some(fit.rotation_degrees),
        zoom_pixels: Some(zoom_pixels),
    })
}

fn mean(values: impl Iterator<Item = Option<f64>>) -> Option<f64> {
    let (mut sum, mut n) = (0.0, 0usize);
    for v in values.flatten() {
        sum += v;
        n += 1;
    }
    (n > 0).then(|| sum / n as f64)
}

pub fn classify(pairs: &[GlobalMotion], options: &Options) -> Result<Summary, String> {
    options.validate()?;
    if pairs.is_empty() || pairs.len() > 299 {
        return Err("Supply 1–299 global motion comparisons".into());
    }
    let classifications = pairs.iter().enumerate().map(|(i, p)| classify_one(i, p, options)).collect::<Result<Vec<_>, _>>()?;
    let mut segments: Vec<Segment> = Vec::new();
    for c in &classifications {
        match segments.last_mut() {
            Some(last) if last.movement == c.movement => {
                last.end_comparison = c.comparison_index;
                last.comparisons += 1;
            }
            _ => segments.push(Segment {
                movement: c.movement,
                start_comparison: c.comparison_index,
                end_comparison: c.comparison_index,
                comparisons: 1,
                mean_dx_pixels: None,
                mean_dy_pixels: None,
                mean_scale: None,
                mean_rotation_degrees: None,
            }),
        }
    }
    for s in &mut segments {
        let members = classifications.get(s.start_comparison..=s.end_comparison).ok_or("Segment outside classifications")?;
        s.mean_dx_pixels = mean(members.iter().map(|c| c.dx_pixels));
        s.mean_dy_pixels = mean(members.iter().map(|c| c.dy_pixels));
        s.mean_scale = mean(members.iter().map(|c| c.scale));
        s.mean_rotation_degrees = mean(members.iter().map(|c| c.rotation_degrees));
    }
    let mut shares: Vec<Share> = Vec::new();
    for c in &classifications {
        match shares.iter_mut().find(|s| s.movement == c.movement) {
            Some(s) => s.comparisons += 1,
            None => shares.push(Share { movement: c.movement, comparisons: 1, fraction: 0.0 }),
        }
    }
    for s in &mut shares {
        s.fraction = s.comparisons as f32 / classifications.len().max(1) as f32;
    }
    shares.sort_by(|a, b| b.comparisons.cmp(&a.comparisons).then(a.movement.cmp(&b.movement)));
    let dominant = shares.first().map(|s| s.movement).unwrap_or(Movement::Unavailable);
    Ok(Summary { options: *options, classifications, segments, shares, dominant })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::global_motion::Similarity;
    fn fit(dx: f64, dy: f64, scale: f64, rotation: f64) -> GlobalMotion {
        GlobalMotion::Estimated {
            fit: Similarity {
                transform: [scale, 0.0, dx, dy],
                analyzed_size: [640, 360],
                center_displacement_pixels: [dx, dy],
                rotation_degrees: rotation,
                scale,
                rms_error_pixels: 0.1,
                support_fraction: 0.9,
                supporting_sample_indices: vec![],
                supporting_samples: vec![],
                tested_samples: 100,
            },
        }
    }
    #[test]
    fn labels_follow_camera_convention_and_segments_merge() {
        let pairs = [
            fit(0.0, 0.0, 1.0, 0.0),
            fit(0.01, 0.02, 1.0, 0.0),
            fit(2.0, 0.1, 1.0, 0.0),
            fit(2.5, 0.0, 1.0, 0.0),
            fit(-3.0, 0.0, 1.0, 0.0),
            fit(0.0, 1.0, 1.0, 0.0),
            fit(0.0, -1.0, 1.0, 0.0),
            fit(0.0, 0.0, 1.01, 0.0),
            fit(0.0, 0.0, 0.99, 0.0),
            fit(0.0, 0.0, 1.0, 0.5),
            fit(0.0, 0.0, 1.0, -0.5),
            fit(2.0, 0.0, 1.01, 0.0),
            GlobalMotion::InsufficientEvidence { reason: "flat".into(), samples: 3 },
        ];
        let summary = classify(&pairs, &Options::default()).unwrap();
        let labels: Vec<Movement> = summary.classifications.iter().map(|c| c.movement).collect();
        assert_eq!(
            labels,
            vec![
                Movement::Static,
                Movement::Static,
                Movement::PanLeft,
                Movement::PanLeft,
                Movement::PanRight,
                Movement::TiltUp,
                Movement::TiltDown,
                Movement::ZoomIn,
                Movement::ZoomOut,
                Movement::RollCounterclockwise,
                Movement::RollClockwise,
                Movement::Mixed,
                Movement::Unavailable
            ]
        );
        assert_eq!(summary.segments.len(), 11);
        assert_eq!((summary.segments[1].start_comparison, summary.segments[1].end_comparison, summary.segments[1].comparisons), (2, 3, 2));
        assert!((summary.segments[1].mean_dx_pixels.unwrap() - 2.25).abs() < 1e-9);
        assert_eq!(summary.segments[10].mean_dx_pixels, None);
        assert_eq!(summary.dominant, Movement::Static);
        assert_eq!(summary.shares[0].comparisons, 2);
        assert!((summary.classifications[7].zoom_pixels.unwrap() - 0.01 * (640f64 * 640.0 + 360.0 * 360.0).sqrt() / 2.0).abs() < 1e-6);
    }
    #[test]
    fn thresholds_and_hostile_values_are_checked() {
        let pairs = [fit(0.2, 0.0, 1.0, 0.0)];
        assert_eq!(classify(&pairs, &Options::default()).unwrap().dominant, Movement::Static);
        assert_eq!(classify(&pairs, &Options { move_pixels: 0.1, ..Options::default() }).unwrap().dominant, Movement::PanLeft);
        assert!(classify(&pairs, &Options { move_pixels: f64::NAN, ..Options::default() }).is_err());
        assert!(classify(&[], &Options::default()).is_err());
        assert!(classify(&[fit(f64::INFINITY, 0.0, 1.0, 0.0)], &Options::default()).is_err());
        assert!(classify(&[fit(0.0, 0.0, 0.0, 0.0)], &Options::default()).is_err());
    }
}

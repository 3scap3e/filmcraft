//! Robust 2D similarity fitting of optical-flow samples. This is image motion, not camera pose.
use crate::MotionPair;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportingSample {
    pub index: usize,
    pub source_pixels: [f64; 2],
    pub target_pixels: [f64; 2],
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Similarity {
    /// x' = a*x - b*y + tx; y' = b*x + a*y + ty, in analysis-image pixels.
    pub transform: [f64; 4],
    pub analyzed_size: [usize; 2],
    pub center_displacement_pixels: [f64; 2],
    pub rotation_degrees: f64,
    pub scale: f64,
    pub rms_error_pixels: f64,
    pub support_fraction: f64,
    pub supporting_sample_indices: Vec<usize>,
    pub supporting_samples: Vec<SupportingSample>,
    pub tested_samples: usize,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum GlobalMotion {
    Estimated { fit: Similarity },
    InsufficientEvidence { reason: String, samples: usize },
}
#[derive(Clone, Copy)]
struct Point {
    from: [f64; 2],
    to: [f64; 2],
    index: usize,
}
fn fit(points: &[Point], indices: &[usize]) -> Option<[f64; 4]> {
    if indices.len() < 2 {
        return None;
    }
    let mut from = [0.0; 2];
    let mut to = [0.0; 2];
    for i in indices {
        let p = points.get(*i)?;
        for k in 0..2 {
            from[k] += p.from[k];
            to[k] += p.to[k];
        }
    }
    for k in 0..2 {
        from[k] /= indices.len() as f64;
        to[k] /= indices.len() as f64;
    }
    let (mut denom, mut dot, mut cross) = (0.0, 0.0, 0.0);
    for i in indices {
        let p = points.get(*i)?;
        let x = p.from[0] - from[0];
        let y = p.from[1] - from[1];
        let u = p.to[0] - to[0];
        let v = p.to[1] - to[1];
        denom += x * x + y * y;
        dot += x * u + y * v;
        cross += x * v - y * u;
    }
    if denom < 1.0 {
        return None;
    }
    let a = dot / denom;
    let b = cross / denom;
    let result = [a, b, to[0] - a * from[0] + b * from[1], to[1] - b * from[0] - a * from[1]];
    result.iter().all(|x| x.is_finite()).then_some(result)
}
fn residual(p: Point, m: [f64; 4]) -> f64 {
    (m[0] * p.from[0] - m[1] * p.from[1] + m[2] - p.to[0]).hypot(m[1] * p.from[0] + m[0] * p.from[1] + m[3] - p.to[1])
}
fn support(points: &[Point], m: [f64; 4], tolerance: f64) -> Vec<usize> {
    points.iter().enumerate().filter_map(|(i, p)| (residual(*p, m) <= tolerance).then_some(i)).collect()
}
pub fn estimate(pair: &MotionPair, tolerance: f64) -> Result<GlobalMotion, String> {
    if !tolerance.is_finite() || !(0.01..=10.0).contains(&tolerance) {
        return Err("Fit tolerance must be 0.01–10 analysis pixels".into());
    }
    let [w, h] = pair.analyzed_size;
    if w == 0 || h == 0 || w > 640 || h > 640 || pair.vectors.len() > 2048 {
        return Err("Invalid motion evidence dimensions or sample count".into());
    }
    let points: Vec<_> = pair
        .vectors
        .iter()
        .enumerate()
        .filter_map(|(index, v)| {
            (v.from.iter().all(|n| n.is_finite() && (0.0..=1.0).contains(n)) && v.to.iter().all(|n| n.is_finite() && (-8.0..=9.0).contains(n))).then_some(
                Point { from: [v.from[0] as f64 * w as f64, v.from[1] as f64 * h as f64], to: [v.to[0] as f64 * w as f64, v.to[1] as f64 * h as f64], index },
            )
        })
        .collect();
    let insufficient = |reason: &str| GlobalMotion::InsufficientEvidence { reason: reason.into(), samples: points.len() };
    if points.len() < 8 {
        return Ok(insufficient("At least eight finite, spatially distributed samples are needed"));
    }
    let mut best = Vec::new();
    let mut best_error = f64::INFINITY;
    // Fixed deterministic RANSAC budget. No random results, unbounded loops, or model training.
    for trial in 0..256usize {
        let i = (trial * 37 + trial / points.len()) % points.len();
        let j = (trial * 101 + points.len() / 2 + 1) % points.len();
        if i == j {
            continue;
        }
        if let Some(m) = fit(&points, &[i, j]) {
            let indices = support(&points, m, tolerance);
            let error = indices.iter().filter_map(|i| points.get(*i)).map(|p| residual(*p, m)).sum::<f64>();
            if indices.len() > best.len() || (indices.len() == best.len() && error < best_error) {
                best = indices;
                best_error = error;
            }
        }
    }
    for _ in 0..3 {
        if let Some(m) = fit(&points, &best) {
            best = support(&points, m, tolerance);
        }
    }
    if best.len() < 8 || best.len() * 2 < points.len() {
        return Ok(insufficient("No model has enough supporting samples"));
    }
    let Some(m) = fit(&points, &best) else {
        return Ok(insufficient("Supporting samples cannot constrain a transform"));
    };
    best = support(&points, m, tolerance);
    if best.len() < 8 || best.len() * 2 < points.len() {
        return Ok(insufficient("Final model has insufficient support"));
    }
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    let mut quadrants = [false; 4];
    let mut squared = 0.0;
    for i in &best {
        if let Some(p) = points.get(*i) {
            for k in 0..2 {
                lo[k] = lo[k].min(p.from[k]);
                hi[k] = hi[k].max(p.from[k]);
            }
            let q = usize::from(p.from[0] >= w as f64 / 2.0) + 2 * usize::from(p.from[1] >= h as f64 / 2.0);
            quadrants[q] = true;
            squared += residual(*p, m).powi(2);
        }
    }
    if hi[0] - lo[0] < w as f64 * 0.3 || hi[1] - lo[1] < h as f64 * 0.3 || quadrants.iter().filter(|q| **q).count() < 3 {
        return Ok(insufficient("Support is too concentrated to represent the whole image"));
    }
    let center = [w as f64 / 2.0, h as f64 / 2.0];
    Ok(GlobalMotion::Estimated {
        fit: Similarity {
            transform: m,
            analyzed_size: [w, h],
            center_displacement_pixels: [m[0] * center[0] - m[1] * center[1] + m[2] - center[0], m[1] * center[0] + m[0] * center[1] + m[3] - center[1]],
            rotation_degrees: m[1].atan2(m[0]).to_degrees(),
            scale: m[0].hypot(m[1]),
            rms_error_pixels: (squared / best.len() as f64).sqrt(),
            support_fraction: best.len() as f64 / points.len() as f64,
            supporting_sample_indices: best.iter().filter_map(|i| points.get(*i).map(|p| p.index)).collect(),
            supporting_samples: best
                .iter()
                .filter_map(|i| points.get(*i).map(|p| SupportingSample { index: p.index, source_pixels: p.from, target_pixels: p.to }))
                .collect(),
            tested_samples: points.len(),
        },
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::MotionVector;
    fn fixture(a: f64, b: f64, tx: f64, ty: f64, outliers: bool) -> MotionPair {
        let mut vectors = Vec::new();
        for y in 1..9 {
            for x in 1..9 {
                let from = [x as f32 / 10.0, y as f32 / 10.0];
                let px = from[0] as f64 * 640.0;
                let py = from[1] as f64 * 360.0;
                let (mut u, mut v) = (a * px - b * py + tx, b * px + a * py + ty);
                if outliers && vectors.len() % 4 == 0 {
                    u += 30.0;
                    v -= 20.0;
                }
                vectors.push(MotionVector {
                    from,
                    to: [(u / 640.0) as f32, (v / 360.0) as f32],
                    mean_absolute_error: None,
                    ambiguity_margin: None,
                    uncertainty_log_scale: Some(0.0),
                });
            }
        }
        MotionPair {
            from_timeline_tick: 0,
            to_timeline_tick: 1,
            from_source_tick: 0,
            to_source_tick: 1,
            analyzed_size: [640, 360],
            vectors,
            rejected_points: 0,
            mean_pixel_change: 0.0,
            changed_pixel_fraction: 0.0,
            mean_motion_pixels: None,
            dense_pixel_count: None,
        }
    }
    #[test]
    fn recovers_translation_rotation_scale_and_rejects_local_outliers() {
        for (a, b, tx, ty, outliers) in [(1.0, 0.0, 3.0, -2.0, false), (1.03 * 0.1f64.cos(), 1.03 * 0.1f64.sin(), 0.0, 0.0, false), (1.0, 0.0, 0.0, 0.0, true)]
        {
            let GlobalMotion::Estimated { fit } = estimate(&fixture(a, b, tx, ty, outliers), 0.1).unwrap() else {
                panic!("expected known transform");
            };
            for (got, want) in fit.transform.into_iter().zip([a, b, tx, ty]) {
                assert!((got - want).abs() < 0.001);
            }
            assert!(fit.rms_error_pixels < 0.001);
            if outliers {
                assert_eq!(fit.supporting_sample_indices.len(), 48);
            }
        }
    }
    #[test]
    fn weak_and_hostile_evidence_does_not_invent_global_motion() {
        let mut p = fixture(1.0, 0.0, 0.0, 0.0, false);
        p.vectors.truncate(3);
        assert!(matches!(estimate(&p, 0.5).unwrap(), GlobalMotion::InsufficientEvidence { .. }));
        assert!(estimate(&p, f64::NAN).is_err());
        p.analyzed_size = [0, 360];
        assert!(estimate(&p, 0.5).is_err());
        let mut p = fixture(1.0, 0.0, 0.0, 0.0, false);
        for v in &mut p.vectors {
            v.from = [0.5, 0.5];
        }
        assert!(matches!(estimate(&p, 0.5).unwrap(), GlobalMotion::InsufficientEvidence { .. }));
    }
}

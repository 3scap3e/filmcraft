//! Step E of the 2D camera: from a solved camera path to Motion parameters. A Motion transform
//! maps a source pixel by `subtract anchor → scale → rotate (degrees, clockwise on screen) →
//! translate to position`, which is the similarity `x' = a·x − b·y + tx, y' = b·x + a·y + ty`
//! with `a = s·cos θ`, `b = s·sin θ`. Composing the camera path with a clip's base Motion and
//! decomposing again gives one position, scale and rotation per frame.
use crate::camera2d::{apply, invert};
use serde::{Deserialize, Serialize};

pub const MAX_KEYFRAMES: usize = 36_000;

/// How the camera path is applied to a clip.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    /// The clip reproduces the camera move: `M_i = C_i ∘ M0`.
    Follow,
    /// The clip cancels the camera move: `M_i = C_i⁻¹ ∘ M0`.
    Stabilize,
}

/// Motion parameters of one frame: position in sequence pixels, scale in percent (both axes),
/// rotation in degrees clockwise on screen.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MotionKeyframe {
    pub frame: usize,
    pub position: [f64; 2],
    pub scale_percent: f64,
    pub rotation_degrees: f64,
}

fn finite4(m: [f64; 4]) -> bool {
    m.iter().all(|v| v.is_finite())
}

/// `outer ∘ inner`: apply `inner` first.
pub fn compose(outer: [f64; 4], inner: [f64; 4]) -> [f64; 4] {
    let t = apply(outer, [inner[2], inner[3]]);
    [outer[0] * inner[0] - outer[1] * inner[1], outer[0] * inner[1] + outer[1] * inner[0], t[0], t[1]]
}

/// The Motion similarity (source pixels → sequence pixels) of a position, scale, rotation and
/// anchor; `fit` is the extra factor of Scale to Frame Size (1 without it).
pub fn base_matrix(position: [f64; 2], scale_percent: f64, rotation_degrees: f64, anchor: [f64; 2], fit: f64) -> Result<[f64; 4], String> {
    if !position.iter().chain(anchor.iter()).all(|v| v.is_finite()) || !scale_percent.is_finite() || !rotation_degrees.is_finite() || !fit.is_finite() {
        return Err("Motion parameters must be finite".into());
    }
    if scale_percent <= 0.0 || fit <= 0.0 {
        return Err("Scale and fit must be positive".into());
    }
    let s = scale_percent / 100.0 * fit;
    let (sin, cos) = rotation_degrees.to_radians().sin_cos();
    let (a, b) = (s * cos, s * sin);
    Ok([a, b, position[0] - (a * anchor[0] - b * anchor[1]), position[1] - (b * anchor[0] + a * anchor[1])])
}

/// Position, scale percent and rotation degrees of a Motion similarity with the given anchor
/// and fit factor.
pub fn decompose(m: [f64; 4], anchor: [f64; 2], fit: f64) -> Result<([f64; 2], f64, f64), String> {
    if !finite4(m) || !anchor.iter().all(|v| v.is_finite()) || !fit.is_finite() || fit <= 0.0 {
        return Err("Transform, anchor and fit must be finite, fit positive".into());
    }
    let s = m[0].hypot(m[1]);
    if s < 1e-9 {
        return Err("Degenerate transform".into());
    }
    Ok((apply(m, anchor), 100.0 * s / fit, m[1].atan2(m[0]).to_degrees()))
}

/// A camera pose measured on frames analysed at `analysis_scale` (analysis width over sequence
/// width, at most 1) expressed in sequence pixels: rotation-scale unchanged, translation divided.
pub fn to_sequence(pose: [f64; 4], analysis_scale: f64) -> Result<[f64; 4], String> {
    if !finite4(pose) || !analysis_scale.is_finite() || analysis_scale <= 0.0 || analysis_scale > 1.0 {
        return Err("Pose must be finite and the analysis scale in (0, 1]".into());
    }
    Ok([pose[0], pose[1], pose[2] / analysis_scale, pose[3] / analysis_scale])
}

/// One Motion keyframe per pose of `path` (sequence pixels, frame 0 = identity), composed with the
/// clip's base Motion `base` (its similarity before the path is applied) and decomposed with its
/// `anchor` and `fit`. Rotations are unwrapped so consecutive frames never jump by 360°.
pub fn keyframes(path: &[[f64; 4]], base: [f64; 4], anchor: [f64; 2], fit: f64, mode: Mode) -> Result<Vec<MotionKeyframe>, String> {
    let bases = vec![base; path.len()];
    keyframes_over(path, &bases, anchor, fit, mode)
}

/// [`keyframes`] for a clip whose Motion already varies: `bases[i]` is the clip's Motion
/// similarity at frame `i` before the path is applied, so following adds the camera on top of the
/// existing animation and stabilising cancels the measured motion whatever produced it.
pub fn keyframes_over(path: &[[f64; 4]], bases: &[[f64; 4]], anchor: [f64; 2], fit: f64, mode: Mode) -> Result<Vec<MotionKeyframe>, String> {
    if path.len() > MAX_KEYFRAMES {
        return Err(format!("At most {MAX_KEYFRAMES} frames"));
    }
    if bases.len() != path.len() {
        return Err("One base Motion per frame is needed".into());
    }
    let mut out = Vec::new();
    out.try_reserve_exact(path.len()).map_err(|e| format!("Cannot allocate keyframes: {e}"))?;
    let mut previous_rotation: Option<f64> = None;
    for (frame, (pose, base)) in path.iter().zip(bases).enumerate() {
        if !finite4(*pose) {
            return Err(format!("Pose {frame} is not finite"));
        }
        if !finite4(*base) || base[0].hypot(base[1]) < 1e-9 {
            return Err(format!("Invalid base Motion at frame {frame}"));
        }
        let camera = match mode {
            Mode::Follow => *pose,
            Mode::Stabilize => invert(*pose).ok_or_else(|| format!("Pose {frame} cannot be inverted"))?,
        };
        let (position, scale_percent, mut rotation_degrees) = decompose(compose(camera, *base), anchor, fit)?;
        if let Some(p) = previous_rotation {
            while rotation_degrees - p > 180.0 {
                rotation_degrees -= 360.0;
            }
            while rotation_degrees - p < -180.0 {
                rotation_degrees += 360.0;
            }
        }
        previous_rotation = Some(rotation_degrees);
        out.push(MotionKeyframe { frame, position, scale_percent, rotation_degrees });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn base_matrix_and_decompose_round_trip_including_scale_to_frame() {
        let anchor = [320.0, 180.0];
        for (position, scale, rotation, fit) in
            [([960.0, 540.0], 100.0, 0.0, 1.0), ([900.0, 500.0], 120.0, 12.5, 1.0), ([960.0, 540.0], 100.0, -30.0, 3.0), ([10.0, 2000.0], 35.0, 179.0, 1.5)]
        {
            let m = base_matrix(position, scale, rotation, anchor, fit).unwrap();
            let (p, s, r) = decompose(m, anchor, fit).unwrap();
            assert!(close(p[0], position[0], 1e-9) && close(p[1], position[1], 1e-9), "{p:?}");
            assert!(close(s, scale, 1e-9) && close(r, rotation, 1e-9), "{s} {r}");
            let corner = apply(m, [0.0, 0.0]);
            let expected = {
                let s = scale / 100.0 * fit;
                let (sin, cos) = rotation.to_radians().sin_cos();
                [position[0] + s * (cos * -anchor[0] - sin * -anchor[1]), position[1] + s * (sin * -anchor[0] + cos * -anchor[1])]
            };
            assert!(close(corner[0], expected[0], 1e-9) && close(corner[1], expected[1], 1e-9));
        }
        assert!(base_matrix([f64::NAN, 0.0], 100.0, 0.0, anchor, 1.0).is_err());
        assert!(base_matrix([0.0, 0.0], 0.0, 0.0, anchor, 1.0).is_err());
        assert!(decompose([0.0, 0.0, 1.0, 1.0], anchor, 1.0).is_err());
        assert!(to_sequence([1.0, 0.0, 3.0, 6.0], 0.0).is_err());
        assert_eq!(to_sequence([1.0, 0.0, 3.0, 6.0], 1.0 / 3.0).unwrap(), [1.0, 0.0, 9.0, 18.0]);
    }

    #[test]
    fn follow_reproduces_the_camera_and_stabilize_cancels_it() {
        let anchor = [320.0, 180.0];
        let base = base_matrix([960.0, 540.0], 300.0, 0.0, anchor, 1.0).unwrap();
        let path: Vec<[f64; 4]> = (0..5)
            .map(|i| {
                let s = 1.0 + 0.01 * i as f64;
                let a = (0.5 * i as f64).to_radians();
                [s * a.cos(), s * a.sin(), 3.0 * i as f64, -2.0 * i as f64]
            })
            .collect();
        let follow = keyframes(&path, base, anchor, 1.0, Mode::Follow).unwrap();
        assert_eq!(follow.len(), 5);
        assert_eq!(follow[0].frame, 0);
        assert!(close(follow[0].position[0], 960.0, 1e-9) && close(follow[0].scale_percent, 300.0, 1e-9) && close(follow[0].rotation_degrees, 0.0, 1e-9));
        for (i, k) in follow.iter().enumerate() {
            let m = base_matrix(k.position, k.scale_percent, k.rotation_degrees, anchor, 1.0).unwrap();
            let expected = compose(path[i], base);
            for c in 0..4 {
                assert!(close(m[c], expected[c], 1e-7), "frame {i}: {m:?} vs {expected:?}");
            }
            assert!(close(k.scale_percent, 300.0 * (1.0 + 0.01 * i as f64), 1e-7));
            assert!(close(k.rotation_degrees, 0.5 * i as f64, 1e-7));
        }
        let stabilize = keyframes(&path, base, anchor, 1.0, Mode::Stabilize).unwrap();
        for (i, k) in stabilize.iter().enumerate() {
            let m = base_matrix(k.position, k.scale_percent, k.rotation_degrees, anchor, 1.0).unwrap();
            let back = compose(path[i], m);
            for c in 0..4 {
                assert!(close(back[c], base[c], 1e-7), "frame {i}: camera then stabilised clip must be the base: {back:?}");
            }
        }
        let wrapped: Vec<[f64; 4]> = [179.0f64, 181.0, 183.0]
            .iter()
            .map(|deg| {
                let (s, c) = deg.to_radians().sin_cos();
                [c, s, 0.0, 0.0]
            })
            .collect();
        let turned = keyframes(&wrapped, base, anchor, 1.0, Mode::Follow).unwrap();
        assert!(close(turned[1].rotation_degrees, 181.0, 1e-7) && close(turned[2].rotation_degrees, 183.0, 1e-7), "{turned:?}");
        let animated: Vec<[f64; 4]> = path.iter().map(|c| compose(*c, base)).collect();
        let cancelled = keyframes_over(&path, &animated, anchor, 1.0, Mode::Stabilize).unwrap();
        for (i, k) in cancelled.iter().enumerate() {
            let m = base_matrix(k.position, k.scale_percent, k.rotation_degrees, anchor, 1.0).unwrap();
            for c in 0..4 {
                assert!(close(m[c], base[c], 1e-7), "frame {i}: stabilising a clip animated by the same camera returns it to the base: {m:?}");
            }
        }
        assert!(keyframes_over(&path, &animated[..2], anchor, 1.0, Mode::Follow).is_err());
        assert!(keyframes(&[[0.0, 0.0, 0.0, 0.0]], base, anchor, 1.0, Mode::Stabilize).is_err());
        assert!(keyframes(&[[1.0, 0.0, f64::NAN, 0.0]], base, anchor, 1.0, Mode::Follow).is_err());
        assert!(keyframes(&path, [0.0, 0.0, 0.0, 0.0], anchor, 1.0, Mode::Follow).is_err());
    }
}

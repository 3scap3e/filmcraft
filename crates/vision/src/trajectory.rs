//! The camera path of a range: absolute 2D similarities per frame solved jointly from consecutive
//! measurements and long-range anchors, so drift does not grow with the frame count.
use crate::camera2d::apply;
use serde::{Deserialize, Serialize};

pub const MAX_FRAMES: usize = 36_000;
pub const MAX_EDGES: usize = 400_000;

/// One measured similarity mapping frame `from` coordinates onto frame `to`, with its weight.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Edge {
    pub from: usize,
    pub to: usize,
    pub transform: [f64; 4],
    pub weight: f64,
}

/// Absolute pose of a frame relative to the first frame of the range.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FramePose {
    pub frame: usize,
    pub transform: [f64; 4],
    pub displacement_px: [f64; 2],
    pub scale: f64,
    pub rotation_degrees: f64,
    pub residual_px: f64,
}

fn mul(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] * b[0] - a[1] * b[1], a[0] * b[1] + a[1] * b[0]]
}
fn conj(a: [f64; 2]) -> [f64; 2] {
    [a[0], -a[1]]
}

/// Minimise Σ wₑ |x_to − mₑ·x_from − cₑ|² over x₁…xₙ₋₁ with x₀ fixed, by Jacobi-preconditioned
/// conjugate gradient on the normal equations (the operator is applied edge by edge; nothing is
/// assembled). `x` holds the initial guess and receives the solution.
fn conjugate_gradient(frames: usize, edges: &[Edge], x: &mut [[f64; 2]], fixed: [f64; 2], constant: impl Fn(&Edge) -> [f64; 2]) {
    let n = frames;
    let mut diagonal = vec![0.0f64; n];
    for e in edges.iter().filter(|e| e.weight > 0.0) {
        if e.to != 0
            && let Some(d) = diagonal.get_mut(e.to)
        {
            *d += e.weight;
        }
        if e.from != 0
            && let Some(d) = diagonal.get_mut(e.from)
        {
            *d += e.weight * (e.transform[0] * e.transform[0] + e.transform[1] * e.transform[1]);
        }
    }
    let precondition = |r: &[[f64; 2]], out: &mut [[f64; 2]]| {
        for i in 0..n {
            if let (Some(o), Some(ri), Some(d)) = (out.get_mut(i), r.get(i), diagonal.get(i)) {
                let inv = if i == 0 || *d <= 0.0 { 0.0 } else { 1.0 / d };
                *o = [ri[0] * inv, ri[1] * inv];
            }
        }
    };
    let apply_operator = |v: &[[f64; 2]], out: &mut [[f64; 2]]| {
        out.iter_mut().for_each(|o| *o = [0.0, 0.0]);
        for e in edges.iter().filter(|e| e.weight > 0.0) {
            let m = [e.transform[0], e.transform[1]];
            let vf = if e.from == 0 { [0.0, 0.0] } else { v.get(e.from).copied().unwrap_or([0.0, 0.0]) };
            let vt = if e.to == 0 { [0.0, 0.0] } else { v.get(e.to).copied().unwrap_or([0.0, 0.0]) };
            let mf = mul(m, vf);
            let r = [vt[0] - mf[0], vt[1] - mf[1]];
            if e.to != 0
                && let Some(o) = out.get_mut(e.to)
            {
                o[0] += e.weight * r[0];
                o[1] += e.weight * r[1];
            }
            if e.from != 0
                && let Some(o) = out.get_mut(e.from)
            {
                let back = mul(conj(m), r);
                o[0] -= e.weight * back[0];
                o[1] -= e.weight * back[1];
            }
        }
    };
    let mut b = vec![[0.0f64; 2]; n];
    for e in edges.iter().filter(|e| e.weight > 0.0) {
        let m = [e.transform[0], e.transform[1]];
        let c = constant(e);
        let known_from = if e.from == 0 { mul(m, fixed) } else { [0.0, 0.0] };
        let known_to = if e.to == 0 { fixed } else { [0.0, 0.0] };
        let r0 = [known_to[0] - known_from[0] - c[0], known_to[1] - known_from[1] - c[1]];
        if e.to != 0
            && let Some(o) = b.get_mut(e.to)
        {
            o[0] -= e.weight * r0[0];
            o[1] -= e.weight * r0[1];
        }
        if e.from != 0
            && let Some(o) = b.get_mut(e.from)
        {
            let back = mul(conj(m), r0);
            o[0] += e.weight * back[0];
            o[1] += e.weight * back[1];
        }
    }
    if let Some(first) = x.get_mut(0) {
        *first = fixed;
    }
    let mut ax = vec![[0.0f64; 2]; n];
    apply_operator(x, &mut ax);
    let mut r: Vec<[f64; 2]> = (0..n).map(|i| if i == 0 { [0.0, 0.0] } else { [b[i][0] - ax[i][0], b[i][1] - ax[i][1]] }).collect();
    let mut zr = vec![[0.0f64; 2]; n];
    precondition(&r, &mut zr);
    let mut p = zr.clone();
    let dot = |a: &[[f64; 2]], c: &[[f64; 2]]| a.iter().zip(c).skip(1).map(|(u, v)| u[0] * v[0] + u[1] * v[1]).sum::<f64>();
    let mut rz = dot(&r, &zr);
    let scale = b.iter().skip(1).map(|v| v[0].abs() + v[1].abs()).fold(1.0f64, f64::max);
    let mut ap = vec![[0.0f64; 2]; n];
    for _ in 0..(20 * n).clamp(1, 400_000) {
        if dot(&r, &r).sqrt() <= 1e-12 * scale {
            break;
        }
        apply_operator(&p, &mut ap);
        let pap = dot(&p, &ap);
        if !pap.is_finite() || pap <= 1e-300 {
            break;
        }
        let alpha = rz / pap;
        for i in 1..n {
            if let (Some(xi), Some(pi), Some(ri), Some(api)) = (x.get_mut(i), p.get(i), r.get_mut(i), ap.get(i)) {
                xi[0] += alpha * pi[0];
                xi[1] += alpha * pi[1];
                ri[0] -= alpha * api[0];
                ri[1] -= alpha * api[1];
            }
        }
        precondition(&r, &mut zr);
        let next = dot(&r, &zr);
        if !next.is_finite() {
            break;
        }
        let beta = next / rz.max(1e-300);
        for i in 1..n {
            if let (Some(pi), Some(zi)) = (p.get_mut(i), zr.get(i)) {
                pi[0] = zi[0] + beta * pi[0];
                pi[1] = zi[1] + beta * pi[1];
            }
        }
        rz = next;
    }
}

fn divide(a: [f64; 2], b: [f64; 2]) -> Option<[f64; 2]> {
    let d = b[0] * b[0] + b[1] * b[1];
    (d > 1e-300).then(|| [(a[0] * b[0] + a[1] * b[1]) / d, (a[1] * b[0] - a[0] * b[1]) / d])
}

/// Weighted least squares of the camera path in two gauge-invariant stages, from the chained
/// initial guess, with frame 0 as the identity. First the rotation-scales in log form,
/// `log z_to − log z_from = log m`, so that contradictory evidence inside a stretch cannot shrink
/// or inflate the rest of the path (the plain linear form `z_to = m·z_from` is not scale
/// invariant: an inconsistent block lowers its residual by shrinking everything behind a weak
/// link). Then the translations `t_to = m′·t_from + m_t`, with `m′ = z_to / z_from` taken from
/// the solved rotation-scales so the second stage is consistent by construction.
pub fn solve(frames: usize, edges: &[Edge], center: [f64; 2]) -> Result<Vec<FramePose>, String> {
    if frames == 0 || frames > MAX_FRAMES || edges.len() > MAX_EDGES {
        return Err("Trajectory needs 1–36000 frames and at most 400000 edges".into());
    }
    for e in edges {
        if e.from >= frames || e.to >= frames || e.from == e.to || !e.weight.is_finite() || e.weight < 0.0 || e.transform.iter().any(|v| !v.is_finite()) {
            return Err("Invalid trajectory edge".into());
        }
        if e.transform[0].hypot(e.transform[1]) < 1e-6 {
            return Err("Degenerate edge transform".into());
        }
    }
    let log_of = |m: [f64; 2]| [m[0].hypot(m[1]).ln(), m[1].atan2(m[0])];
    let log_edges: Vec<Edge> = edges
        .iter()
        .map(|e| {
            let l = log_of([e.transform[0], e.transform[1]]);
            Edge { from: e.from, to: e.to, transform: [1.0, 0.0, l[0], l[1]], weight: e.weight }
        })
        .collect();
    let mut u = vec![[0.0f64; 2]; frames];
    for i in 1..frames {
        let chain = log_edges
            .iter()
            .filter(|e| e.to == i && e.from + 1 == i && e.weight > 0.0)
            .max_by(|a, b| a.weight.partial_cmp(&b.weight).unwrap_or(std::cmp::Ordering::Equal));
        let step = chain.map(|e| [e.transform[2], e.transform[3]]).unwrap_or([0.0, 0.0]);
        let previous = u.get(i - 1).copied().unwrap_or([0.0, 0.0]);
        if let Some(ui) = u.get_mut(i) {
            *ui = [previous[0] + step[0], previous[1] + step[1]];
        }
    }
    conjugate_gradient(frames, &log_edges, &mut u, [0.0, 0.0], |e| [e.transform[2], e.transform[3]]);
    let z: Vec<[f64; 2]> = u
        .iter()
        .map(|l| {
            let s = l[0].clamp(-20.0, 20.0).exp();
            [s * l[1].cos(), s * l[1].sin()]
        })
        .collect();
    let mut consistent: Vec<Edge> = Vec::with_capacity(edges.len());
    for e in edges {
        let (zf, zt) = (z.get(e.from).copied().unwrap_or([1.0, 0.0]), z.get(e.to).copied().unwrap_or([1.0, 0.0]));
        let m = divide(zt, zf).ok_or("Degenerate solved rotation-scale")?;
        consistent.push(Edge { from: e.from, to: e.to, transform: [m[0], m[1], e.transform[2], e.transform[3]], weight: e.weight });
    }
    let mut t = vec![[0.0f64, 0.0]; frames];
    for i in 1..frames {
        let chain = consistent
            .iter()
            .filter(|e| e.to == i && e.from + 1 == i && e.weight > 0.0)
            .max_by(|a, b| a.weight.partial_cmp(&b.weight).unwrap_or(std::cmp::Ordering::Equal));
        let (mz, mt) = chain.map(|e| ([e.transform[0], e.transform[1]], [e.transform[2], e.transform[3]])).unwrap_or(([1.0, 0.0], [0.0, 0.0]));
        let pt = t.get(i - 1).copied().unwrap_or([0.0, 0.0]);
        let moved = mul(mz, pt);
        if let Some(ti) = t.get_mut(i) {
            *ti = [moved[0] + mt[0], moved[1] + mt[1]];
        }
    }
    let mut incoming: Vec<Vec<usize>> = vec![Vec::new(); frames];
    for (k, e) in edges.iter().enumerate() {
        if e.weight > 0.0
            && let Some(v) = incoming.get_mut(e.to)
        {
            v.push(k);
        }
    }
    conjugate_gradient(frames, &consistent, &mut t, [0.0, 0.0], |e| [e.transform[2], e.transform[3]]);
    let mut poses = Vec::with_capacity(frames);
    for i in 0..frames {
        let (zi, ti) = (z.get(i).copied().unwrap_or([1.0, 0.0]), t.get(i).copied().unwrap_or([0.0, 0.0]));
        let m = [zi[0], zi[1], ti[0], ti[1]];
        let mut residual = 0.0f64;
        for k in incoming.get(i).map(Vec::as_slice).unwrap_or(&[]) {
            let Some(e) = edges.get(*k) else { continue };
            let zf = z.get(e.from).copied().unwrap_or([1.0, 0.0]);
            let tf = t.get(e.from).copied().unwrap_or([0.0, 0.0]);
            let from_pose = [zf[0], zf[1], tf[0], tf[1]];
            let predicted = apply(e.transform, apply(from_pose, center));
            let actual = apply(m, center);
            residual = residual.max((predicted[0] - actual[0]).hypot(predicted[1] - actual[1]));
        }
        let moved = apply(m, center);
        poses.push(FramePose {
            frame: i,
            transform: m,
            displacement_px: [moved[0] - center[0], moved[1] - center[1]],
            scale: zi[0].hypot(zi[1]),
            rotation_degrees: zi[1].atan2(zi[0]).to_degrees(),
            residual_px: residual,
        });
    }
    Ok(poses)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera2d::{CameraOptions, Fit, camera_between, invert};
    use crate::features::{Options, detect};
    use crate::matching::{MatchOptions, describe, match_features};

    fn compose(outer: [f64; 4], inner: [f64; 4]) -> [f64; 4] {
        let z = mul([outer[0], outer[1]], [inner[0], inner[1]]);
        let t = mul([outer[0], outer[1]], [inner[2], inner[3]]);
        [z[0], z[1], t[0] + outer[2], t[1] + outer[3]]
    }
    fn chain_edges(truth: &[[f64; 4]]) -> Vec<Edge> {
        (1..truth.len()).map(|i| Edge { from: i - 1, to: i, transform: compose(truth[i], invert(truth[i - 1]).unwrap()), weight: 1.0 }).collect()
    }

    #[test]
    fn anchors_remove_the_drift_of_a_noisy_chain() {
        let n = 61;
        let truth: Vec<[f64; 4]> = (0..n).map(|i| [1.0 + 0.001 * i as f64, 0.0005 * i as f64, 0.7 * i as f64, -0.3 * i as f64]).collect();
        let mut edges = chain_edges(&truth);
        for (k, e) in edges.iter_mut().enumerate() {
            e.transform[2] += 0.2;
            e.transform[3] += if k % 2 == 0 { 0.1 } else { -0.05 };
        }
        let chained = solve(n, &edges, [320.0, 180.0]).unwrap();
        let drift = (chained[n - 1].displacement_px[0] - (truth[n - 1][2] + (truth[n - 1][0] - 1.0) * 320.0 - truth[n - 1][1] * 180.0)).abs();
        assert!(drift > 5.0, "a biased chain must drift: {drift}");
        for i in (8..n).step_by(4) {
            edges.push(Edge { from: i - 8, to: i, transform: compose(truth[i], invert(truth[i - 8]).unwrap()), weight: 4.0 });
        }
        let solved = solve(n, &edges, [320.0, 180.0]).unwrap();
        let last = &solved[n - 1];
        let expected = apply(truth[n - 1], [320.0, 180.0]);
        let got = apply(last.transform, [320.0, 180.0]);
        assert!((got[0] - expected[0]).abs() < 0.6 && (got[1] - expected[1]).abs() < 0.6, "{got:?} vs {expected:?}");
        assert!((last.scale - truth[n - 1][0].hypot(truth[n - 1][1])).abs() < 0.002);
        assert!(solved.iter().all(|p| p.residual_px.is_finite()));
        assert_eq!(solved[0].transform, [1.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn contradictory_evidence_behind_a_weak_link_cannot_shrink_or_shift_the_path() {
        let n = 40;
        let mut edges: Vec<Edge> = (1..20).map(|i| Edge { from: i - 1, to: i, transform: [1.0, 0.0, 0.0, 0.0], weight: 100.0 }).collect();
        edges.push(Edge { from: 19, to: 20, transform: [1.0, 0.0, 0.0, 0.0], weight: 1.0 });
        for i in 21..n {
            edges.push(Edge { from: i - 1, to: i, transform: [0.98, 0.0, -0.4, -1.9], weight: 100.0 });
        }
        for i in [28usize, 36] {
            edges.push(Edge { from: i - 8, to: i, transform: [1.0, 0.0, 0.0, 0.0], weight: 100.0 });
        }
        let poses = solve(n, &edges, [320.0, 180.0]).unwrap();
        for p in &poses[..21] {
            assert!((p.scale - 1.0).abs() < 1e-3 && p.displacement_px[0].abs() < 0.3 && p.displacement_px[1].abs() < 0.3, "frame {} drifted: {p:?}", p.frame);
        }
        let last = &poses[n - 1];
        assert!(last.scale > 0.7 && last.scale < 1.0, "the contradictory block compromises instead of collapsing: {last:?}");
        assert!(poses.iter().all(|p| p.residual_px.is_finite()));
    }

    #[test]
    fn hostile_edges_are_errors() {
        assert!(solve(0, &[], [0.0, 0.0]).is_err());
        assert!(solve(2, &[Edge { from: 0, to: 5, transform: [1.0, 0.0, 0.0, 0.0], weight: 1.0 }], [0.0, 0.0]).is_err());
        assert!(solve(2, &[Edge { from: 0, to: 1, transform: [0.0, 0.0, 0.0, 0.0], weight: 1.0 }], [0.0, 0.0]).is_err());
        assert!(solve(2, &[Edge { from: 0, to: 1, transform: [1.0, 0.0, f64::NAN, 0.0], weight: 1.0 }], [0.0, 0.0]).is_err());
        let identity = solve(3, &[], [0.0, 0.0]).unwrap();
        assert!(identity.iter().all(|p| p.transform == [1.0, 0.0, 0.0, 0.0]));
    }

    fn textured(w: usize, h: usize) -> Vec<f32> {
        let mut seed = 0x1234_5678u32;
        let mut px = vec![0.0f32; w * h];
        let mut blobs = Vec::new();
        for _ in 0..60 {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let x = (seed % w as u32) as f32;
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let y = (seed % h as u32) as f32;
            let r = 3.0 + (seed % 7) as f32;
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let r2 = 3.0 + (seed % 9) as f32;
            blobs.push((x, y, r, r2, (seed % 3) as f32 * 0.35 + 0.1));
        }
        for y in 0..h {
            for x in 0..w {
                let mut v = 0.35 + 0.05 * ((x as f32 * 0.21).sin() + (y as f32 * 0.17).cos());
                for (bx, by, rw, rh, value) in &blobs {
                    if (x as f32 - bx).abs() < *rw && (y as f32 - by).abs() < *rh {
                        v = *value;
                    }
                }
                px[y * w + x] = v;
            }
        }
        px
    }
    fn render(base: &[f32], w: usize, h: usize, camera: [f64; 4], object_x: f32) -> Vec<u8> {
        let inv = invert(camera).unwrap();
        let mut out = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let s = apply(inv, [x as f64, y as f64]);
                let v = if s[0] >= 0.0 && s[1] >= 0.0 && s[0] < w as f64 - 1.0 && s[1] < h as f64 - 1.0 {
                    let (x0, y0) = (s[0].floor() as usize, s[1].floor() as usize);
                    let (fx, fy) = ((s[0] - x0 as f64) as f32, (s[1] - y0 as f64) as f32);
                    let at = |xx: usize, yy: usize| base[yy * w + xx];
                    at(x0, y0) * (1.0 - fx) * (1.0 - fy) + at(x0 + 1, y0) * fx * (1.0 - fy) + at(x0, y0 + 1) * (1.0 - fx) * fy + at(x0 + 1, y0 + 1) * fx * fy
                } else {
                    0.35
                };
                let inside = (x as f32 - object_x).abs() < 22.0 && (y as f32 - 70.0).abs() < 16.0;
                let v = if inside {
                    let (ox, oy) = ((x as f32 - object_x + 22.0) as u32, (y as f32 - 70.0 + 16.0) as u32);
                    let cell = ((ox / 5).wrapping_mul(2_654_435_761) ^ (oy / 5).wrapping_mul(40_503)) >> 13;
                    if cell % 3 == 0 {
                        0.95
                    } else if cell % 3 == 1 {
                        0.15
                    } else {
                        0.55
                    }
                } else {
                    v
                };
                let i = (y * w + x) * 4;
                let byte = (v * 255.0).round().clamp(0.0, 255.0) as u8;
                out[i] = byte;
                out[i + 1] = byte;
                out[i + 2] = byte;
                out[i + 3] = 255;
            }
        }
        out
    }

    #[test]
    fn full_pipeline_recovers_a_smooth_camera_and_isolates_a_moving_object() {
        let (w, h) = (200, 140);
        let base = textured(w, h);
        let n = 17;
        let truth: Vec<[f64; 4]> = (0..n)
            .map(|i| {
                let s = 1.0 + 0.003 * i as f64;
                let a = (0.002 * i as f64).to_radians();
                [s * a.cos(), s * a.sin(), 1.1 * i as f64, -0.4 * i as f64]
            })
            .collect();
        let detect_options = Options { max_points: 300, min_distance: 4.0, threshold: 0.002, octaves: 2 };
        let match_options = MatchOptions::default();
        let camera_options = CameraOptions::default();
        let center = [w as f64 / 2.0, h as f64 / 2.0];
        let mut memory: Vec<(Vec<crate::features::Feature>, Vec<crate::matching::Descriptor>, Vec<f32>)> = Vec::new();
        let mut edges = Vec::new();
        let mut regions_seen = 0;
        for i in 0..n {
            let frame = render(&base, w, h, truth[i], 150.0 - 3.0 * i as f32);
            let features = detect(w, h, &frame, &detect_options).unwrap();
            let descriptors = describe(w, h, &frame, &features).unwrap();
            let luma = crate::features::luma(w, h, &frame).unwrap();
            for (from, gap) in [(i.checked_sub(1), 1usize), (i.checked_sub(4), 4)] {
                let Some(from) = from else { continue };
                if gap == 4 && i % 4 != 0 {
                    continue;
                }
                let (pf, pd, pl) = &memory[from];
                let matches = match_features(pd, pf, &descriptors, &features, &match_options).unwrap();
                let pairs: Vec<([f64; 2], [f64; 2])> =
                    matches.iter().map(|m| ([f64::from(pf[m.a].x), f64::from(pf[m.a].y)], [f64::from(features[m.b].x), f64::from(features[m.b].y)])).collect();
                let mut fit = camera_between(&pairs, [w as f64, h as f64], &camera_options).unwrap();
                if gap == 1 && fit.regions.iter().any(|r| r.displacement_px[0] < -1.5) {
                    regions_seen += 1;
                }
                if let Fit::Estimated { fit: s } = &fit.camera {
                    let boxes: Vec<[f64; 4]> = fit.regions.iter().map(|r| r.bounds).collect();
                    let refined = crate::camera2d::refine_similarity(pl, &luma, w, h, s.transform, 15, &boxes).unwrap();
                    fit.camera = Fit::Estimated { fit: s.with_transform(refined, &pairs, center) };
                }
                if let Fit::Estimated { fit: s } = &fit.camera {
                    if std::env::var_os("FILMCRAFT_TRAJECTORY_DEBUG").is_some() {
                        let expected = compose(truth[i], invert(truth[from]).unwrap());
                        println!(
                            "edge {from}->{i}: scale {:.5} (truth {:.5}) rot {:.4} t ({:.3}, {:.3}) truth t ({:.3}, {:.3}) inliers {} of {} cells {} regions {}",
                            s.scale,
                            expected[0].hypot(expected[1]),
                            s.rotation_degrees,
                            s.transform[2],
                            s.transform[3],
                            expected[2],
                            expected[3],
                            s.inliers.len(),
                            s.tested,
                            fit.coverage_cells,
                            fit.regions.len()
                        );
                    }
                    edges.push(Edge { from, to: i, transform: s.transform, weight: s.inliers.len() as f64 });
                }
            }
            memory.push((features, descriptors, luma));
        }
        let poses = solve(n, &edges, center).unwrap();
        for (i, pose) in poses.iter().enumerate() {
            let expected = apply(truth[i], center);
            let got = apply(pose.transform, center);
            assert!((got[0] - expected[0]).abs() < 0.5 && (got[1] - expected[1]).abs() < 0.5, "frame {i}: {got:?} vs {expected:?}");
            assert!((pose.scale - truth[i][0].hypot(truth[i][1])).abs() < 0.003, "frame {i} scale {}", pose.scale);
        }
        assert!(regions_seen >= n / 2, "the moving checkerboard must show up as a region, seen {regions_seen}");
    }
}

//! Robust 2D similarity between two frames from matched points, and its photometric verification.
//! Image-plane motion only: no physical pose, no depth.
use serde::{Deserialize, Serialize};

pub const MAX_PAIRS: usize = 5000;
const HYPOTHESES: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FitOptions {
    pub tolerance_px: f64,
    pub min_inliers: usize,
    pub min_support: f64,
}
impl Default for FitOptions {
    fn default() -> Self {
        Self { tolerance_px: 1.5, min_inliers: 8, min_support: 0.25 }
    }
}
impl FitOptions {
    pub fn validate(&self) -> Result<(), String> {
        if !self.tolerance_px.is_finite()
            || !(0.01..=100.0).contains(&self.tolerance_px)
            || !(3..=MAX_PAIRS).contains(&self.min_inliers)
            || !self.min_support.is_finite()
            || !(0.0..=1.0).contains(&self.min_support)
        {
            return Err("Fit options: tolerance 0.01–100 px, at least 3 inliers, support 0–1".into());
        }
        Ok(())
    }
}

/// `x' = a·x − b·y + tx`, `y' = b·x + a·y + ty` in analysis pixels, mapping frame A positions to frame B.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Similarity {
    pub transform: [f64; 4],
    pub center_displacement_px: [f64; 2],
    pub scale: f64,
    pub rotation_degrees: f64,
    pub rms_error_px: f64,
    pub inliers: Vec<usize>,
    pub tested: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum Fit {
    Estimated { fit: Similarity },
    Insufficient { reason: String, pairs: usize },
}

fn solve(pairs: &[([f64; 2], [f64; 2])], indices: &[usize]) -> Option<[f64; 4]> {
    if indices.len() < 2 {
        return None;
    }
    let n = indices.len() as f64;
    let (mut mx, mut my, mut ux, mut uy) = (0.0, 0.0, 0.0, 0.0);
    for i in indices {
        let p = pairs.get(*i)?;
        mx += p.0[0];
        my += p.0[1];
        ux += p.1[0];
        uy += p.1[1];
    }
    let (mx, my, ux, uy) = (mx / n, my / n, ux / n, uy / n);
    let (mut denom, mut dot, mut cross) = (0.0, 0.0, 0.0);
    for i in indices {
        let p = pairs.get(*i)?;
        let (x, y) = (p.0[0] - mx, p.0[1] - my);
        let (u, v) = (p.1[0] - ux, p.1[1] - uy);
        denom += x * x + y * y;
        dot += x * u + y * v;
        cross += x * v - y * u;
    }
    if denom < 1e-9 {
        return None;
    }
    let a = dot / denom;
    let b = cross / denom;
    let m = [a, b, ux - a * mx + b * my, uy - b * mx - a * my];
    m.iter().all(|v| v.is_finite()).then_some(m)
}

pub fn apply(m: [f64; 4], p: [f64; 2]) -> [f64; 2] {
    [m[0] * p[0] - m[1] * p[1] + m[2], m[1] * p[0] + m[0] * p[1] + m[3]]
}

pub fn invert(m: [f64; 4]) -> Option<[f64; 4]> {
    let s2 = m[0] * m[0] + m[1] * m[1];
    if s2 < 1e-12 || !s2.is_finite() {
        return None;
    }
    let (a, b) = (m[0] / s2, -m[1] / s2);
    Some([a, b, -(a * m[2] - b * m[3]), -(b * m[2] + a * m[3])])
}

fn residual(m: [f64; 4], p: &([f64; 2], [f64; 2])) -> f64 {
    let q = apply(m, p.0);
    (q[0] - p.1[0]).hypot(q[1] - p.1[1])
}

/// Deterministic RANSAC over two-pair hypotheses, then least squares on the support set.
pub fn fit_similarity(pairs: &[([f64; 2], [f64; 2])], center: [f64; 2], options: &FitOptions) -> Result<Fit, String> {
    options.validate()?;
    if pairs.len() > MAX_PAIRS {
        return Err("Too many pairs".into());
    }
    if pairs.iter().any(|p| p.0.iter().chain(p.1.iter()).any(|v| !v.is_finite() || v.abs() > 1.0e6)) {
        return Err("Invalid pair coordinates".into());
    }
    let n = pairs.len();
    if n < options.min_inliers.max(2) {
        return Ok(Fit::Insufficient { reason: format!("{n} pairs; at least {} needed", options.min_inliers.max(2)), pairs: n });
    }
    let mut state = 0x9E37_79B9u32 ^ (n as u32).wrapping_mul(2_654_435_761);
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    let mut best: Option<(usize, [f64; 4])> = None;
    for _ in 0..HYPOTHESES.min(n * (n - 1)) {
        let i = (next() as usize) % n;
        let mut j = (next() as usize) % n;
        if j == i {
            j = (j + 1) % n;
        }
        let Some(m) = solve(pairs, &[i, j]) else { continue };
        let support = pairs.iter().filter(|p| residual(m, p) <= options.tolerance_px).count();
        if best.is_none_or(|b| support > b.0) {
            best = Some((support, m));
        }
    }
    let Some((_, mut m)) = best else {
        return Ok(Fit::Insufficient { reason: "No consistent pair of matches".into(), pairs: n });
    };
    for _ in 0..3 {
        let support: Vec<usize> = (0..n).filter(|i| pairs.get(*i).is_some_and(|p| residual(m, p) <= options.tolerance_px)).collect();
        match solve(pairs, &support) {
            Some(refined) => m = refined,
            None => break,
        }
    }
    let inliers: Vec<usize> = (0..n).filter(|i| pairs.get(*i).is_some_and(|p| residual(m, p) <= options.tolerance_px)).collect();
    let support = inliers.len() as f64 / n as f64;
    if inliers.len() < options.min_inliers || support < options.min_support {
        return Ok(Fit::Insufficient { reason: format!("{} of {n} matches support the best model ({:.0}%)", inliers.len(), support * 100.0), pairs: n });
    }
    let rms = (inliers.iter().filter_map(|i| pairs.get(*i)).map(|p| residual(m, p).powi(2)).sum::<f64>() / inliers.len().max(1) as f64).sqrt();
    let moved = apply(m, center);
    Ok(Fit::Estimated {
        fit: Similarity {
            transform: m,
            center_displacement_px: [moved[0] - center[0], moved[1] - center[1]],
            scale: m[0].hypot(m[1]),
            rotation_degrees: m[1].atan2(m[0]).to_degrees(),
            rms_error_px: rms,
            inliers,
            tested: n,
        },
    })
}

pub const GRID: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CameraOptions {
    pub fit: FitOptions,
    pub min_coverage_cells: usize,
    pub min_span: f64,
    pub max_regions: usize,
    pub static_px: f64,
}
impl Default for CameraOptions {
    fn default() -> Self {
        Self { fit: FitOptions::default(), min_coverage_cells: 6, min_span: 0.5, max_regions: 3, static_px: 0.15 }
    }
}
impl CameraOptions {
    pub fn validate(&self) -> Result<(), String> {
        self.fit.validate()?;
        if !(1..=GRID * GRID).contains(&self.min_coverage_cells)
            || !self.min_span.is_finite()
            || !(0.0..=1.0).contains(&self.min_span)
            || self.max_regions > 16
            || !self.static_px.is_finite()
            || !(0.0..=100.0).contains(&self.static_px)
        {
            return Err("Camera options: coverage 1–16 cells, span 0–1, up to 16 regions, static threshold 0–100 px".into());
        }
        Ok(())
    }
}

/// A group of matches that move together but not with the camera: an object, text or a window scrolling.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MovingRegion {
    pub bounds: [f64; 4],
    pub count: usize,
    pub displacement_px: [f64; 2],
    pub scale: f64,
    pub rotation_degrees: f64,
    pub inliers: Vec<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CameraState {
    Static,
    Moving,
    Unverified,
    NoEvidence,
}
impl CameraState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Static => "static",
            Self::Moving => "moving",
            Self::Unverified => "not verified",
            Self::NoEvidence => "no camera evidence",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraFit {
    pub camera: Fit,
    pub coverage_cells: usize,
    pub span: [f64; 2],
    pub regions: Vec<MovingRegion>,
}

fn coverage(pairs: &[([f64; 2], [f64; 2])], indices: &[usize], size: [f64; 2]) -> (usize, [f64; 2]) {
    let mut cells = [false; GRID * GRID];
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for i in indices {
        let Some(p) = pairs.get(*i) else { continue };
        let cx = ((p.0[0] / size[0].max(1.0)) * GRID as f64).floor().clamp(0.0, GRID as f64 - 1.0) as usize;
        let cy = ((p.0[1] / size[1].max(1.0)) * GRID as f64).floor().clamp(0.0, GRID as f64 - 1.0) as usize;
        if let Some(c) = cells.get_mut(cy * GRID + cx) {
            *c = true;
        }
        x0 = x0.min(p.0[0]);
        y0 = y0.min(p.0[1]);
        x1 = x1.max(p.0[0]);
        y1 = y1.max(p.0[1]);
    }
    let span = if indices.is_empty() { [0.0, 0.0] } else { [((x1 - x0) / size[0].max(1.0)).clamp(0.0, 1.0), ((y1 - y0) / size[1].max(1.0)).clamp(0.0, 1.0)] };
    (cells.iter().filter(|c| **c).count(), span)
}

struct Rng(u32);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }
}

/// RANSAC restricted to `candidates`; `score` ranks a support set. Returns the refined model and its support.
fn ransac(pairs: &[([f64; 2], [f64; 2])], candidates: &[usize], tolerance: f64, score: impl Fn(&[usize]) -> (usize, usize)) -> Option<([f64; 4], Vec<usize>)> {
    let n = candidates.len();
    if n < 2 {
        return None;
    }
    let mut rng = Rng(0x9E37_79B9 ^ (n as u32).wrapping_mul(2_654_435_761));
    let mut best: Option<((usize, usize), [f64; 4])> = None;
    for _ in 0..HYPOTHESES.min(n * (n - 1)) {
        let i = *candidates.get(rng.next() as usize % n)?;
        let mut j = *candidates.get(rng.next() as usize % n)?;
        if j == i {
            j = *candidates.get((rng.next() as usize + 1) % n)?;
            if j == i {
                continue;
            }
        }
        let Some(m) = solve(pairs, &[i, j]) else { continue };
        let support: Vec<usize> = candidates.iter().copied().filter(|k| pairs.get(*k).is_some_and(|p| residual(m, p) <= tolerance)).collect();
        let s = score(&support);
        if best.is_none_or(|b| s > b.0) {
            best = Some((s, m));
        }
    }
    let (_, mut m) = best?;
    for _ in 0..3 {
        let support: Vec<usize> = candidates.iter().copied().filter(|k| pairs.get(*k).is_some_and(|p| residual(m, p) <= tolerance)).collect();
        match solve(pairs, &support) {
            Some(refined) => m = refined,
            None => break,
        }
    }
    let support: Vec<usize> = candidates.iter().copied().filter(|k| pairs.get(*k).is_some_and(|p| residual(m, p) <= tolerance)).collect();
    Some((m, support))
}

fn similarity_of(pairs: &[([f64; 2], [f64; 2])], m: [f64; 4], inliers: Vec<usize>, tested: usize, center: [f64; 2]) -> Similarity {
    let rms = (inliers.iter().filter_map(|i| pairs.get(*i)).map(|p| residual(m, p).powi(2)).sum::<f64>() / inliers.len().max(1) as f64).sqrt();
    let moved = apply(m, center);
    Similarity {
        transform: m,
        center_displacement_px: [moved[0] - center[0], moved[1] - center[1]],
        scale: m[0].hypot(m[1]),
        rotation_degrees: m[1].atan2(m[0]).to_degrees(),
        rms_error_px: rms,
        inliers,
        tested,
    }
}

fn bounds(pairs: &[([f64; 2], [f64; 2])], members: &[usize]) -> [f64; 4] {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for k in members {
        if let Some(p) = pairs.get(*k) {
            x0 = x0.min(p.0[0]);
            y0 = y0.min(p.0[1]);
            x1 = x1.max(p.0[0]);
            y1 = y1.max(p.0[1]);
        }
    }
    if members.is_empty() { [0.0, 0.0, 0.0, 0.0] } else { [x0, y0, x1, y1] }
}

/// The camera between two frames is chosen among the consistent groups of matches (consensus
/// size, largest first). An object moves inside the picture while the background surrounds it, so
/// a group whose extent encloses every other group is the camera, whatever its size. Otherwise the
/// best-spread group (grid coverage and span) is the camera when it spreads enough, or when it is
/// the largest group and holds at least half of all matches. The other groups are moving regions.
pub fn camera_between(pairs: &[([f64; 2], [f64; 2])], size: [f64; 2], options: &CameraOptions) -> Result<CameraFit, String> {
    options.validate()?;
    if pairs.len() > MAX_PAIRS {
        return Err("Too many pairs".into());
    }
    if size.iter().any(|v| !v.is_finite() || *v < 1.0 || *v > 16384.0) {
        return Err("Invalid frame size".into());
    }
    if pairs.iter().any(|p| p.0.iter().chain(p.1.iter()).any(|v| !v.is_finite() || v.abs() > 1.0e6)) {
        return Err("Invalid pair coordinates".into());
    }
    let center = [size[0] / 2.0, size[1] / 2.0];
    let n = pairs.len();
    let all: Vec<usize> = (0..n).collect();
    let min = options.fit.min_inliers.max(2);
    if n < min {
        return Ok(CameraFit {
            camera: Fit::Insufficient { reason: format!("{n} pairs; at least {min} needed"), pairs: n },
            coverage_cells: 0,
            span: [0.0, 0.0],
            regions: Vec::new(),
        });
    }
    let tolerance = options.fit.tolerance_px;
    // Consistent groups by consensus size, largest first; the camera is chosen among them by coverage.
    let mut groups: Vec<([f64; 4], Vec<usize>, usize, [f64; 2])> = Vec::new();
    let mut rest = all.clone();
    for _ in 0..options.max_regions.saturating_add(1) {
        if rest.len() < options.fit.min_inliers.max(2) {
            break;
        }
        let Some((m, members)) = ransac(pairs, &rest, tolerance, |support| (support.len(), 0)) else { break };
        if members.len() < options.fit.min_inliers {
            break;
        }
        rest.retain(|i| !members.contains(i));
        let (cells, span) = coverage(pairs, &members, size);
        groups.push((m, members, cells, span));
    }
    if groups.is_empty() {
        return Ok(CameraFit {
            camera: Fit::Insufficient { reason: format!("No group of at least {} consistent matches among {n}", options.fit.min_inliers), pairs: n },
            coverage_cells: 0,
            span: [0.0, 0.0],
            regions: Vec::new(),
        });
    }
    let boxes: Vec<[f64; 4]> = groups.iter().map(|g| bounds(pairs, &g.1)).collect();
    let margin = [size[0] * 0.02, size[1] * 0.02];
    let encloses = |outer: &[f64; 4], inner: &[f64; 4]| {
        outer[0] <= inner[0] + margin[0] && outer[1] <= inner[1] + margin[1] && outer[2] >= inner[2] - margin[0] && outer[3] >= inner[3] - margin[1]
    };
    let outer: Option<usize> = (groups.len() > 1)
        .then(|| {
            (0..groups.len()).find(|i| {
                (0..groups.len()).filter(|j| j != i).all(|j| matches!((boxes.get(*i), boxes.get(j)), (Some(a), Some(b)) if encloses(a, b) && !encloses(b, a)))
            })
        })
        .flatten();
    let coverage_score = |g: &([f64; 4], Vec<usize>, usize, [f64; 2])| g.2 * 1000 + ((g.3[0] + g.3[1]) * 100.0) as usize;
    let best = outer.unwrap_or_else(|| (0..groups.len()).max_by_key(|i| groups.get(*i).map(coverage_score).unwrap_or(0)).unwrap_or(0));
    let candidate = groups.get(best).ok_or("Camera candidate missing")?;
    let (cells, span) = (candidate.2, candidate.3);
    let spread = cells >= options.min_coverage_cells && span[0] >= options.min_span && span[1] >= options.min_span;
    let dominant = best == 0 && candidate.1.len() * 2 >= n && candidate.1.len() >= options.fit.min_inliers * 2;
    let accepted = outer.is_some() || spread || dominant;
    let camera = if accepted {
        Fit::Estimated { fit: similarity_of(pairs, candidate.0, candidate.1.clone(), n, center) }
    } else {
        Fit::Insufficient {
            reason: format!(
                "Best-spread group covers {cells} of {} cells and {:.0}%×{:.0}% of the frame with {} of {n} matches while another group has {}; a camera needs {} cells and {:.0}% span, a group enclosing the others, or one group with half of the matches",
                GRID * GRID,
                span[0] * 100.0,
                span[1] * 100.0,
                candidate.1.len(),
                groups.iter().enumerate().filter(|(i, _)| *i != best).map(|(_, g)| g.1.len()).max().unwrap_or(0),
                options.min_coverage_cells,
                options.min_span * 100.0
            ),
            pairs: n,
        }
    };
    let mut regions = Vec::new();
    for (i, (rm, members, _, _)) in groups.iter().enumerate() {
        if accepted && i == best {
            continue;
        }
        if regions.len() >= options.max_regions {
            break;
        }
        let [x0, y0, x1, y1] = boxes.get(i).copied().unwrap_or([0.0; 4]);
        let (mut dx, mut dy) = (0.0, 0.0);
        for k in members {
            if let Some(p) = pairs.get(*k) {
                dx += p.1[0] - p.0[0];
                dy += p.1[1] - p.0[1];
            }
        }
        let count = members.len().max(1) as f64;
        regions.push(MovingRegion {
            bounds: [x0 / size[0], y0 / size[1], x1 / size[0], y1 / size[1]].map(|v| v.clamp(0.0, 1.0)),
            count: members.len(),
            displacement_px: [dx / count, dy / count],
            scale: rm[0].hypot(rm[1]),
            rotation_degrees: rm[1].atan2(rm[0]).to_degrees(),
            inliers: members.clone(),
        });
    }
    Ok(CameraFit { camera, coverage_cells: cells, span, regions })
}

fn solve4(mut a: [[f64; 4]; 4], mut b: [f64; 4]) -> Option<[f64; 4]> {
    for col in 0..4 {
        let pivot = (col..4).max_by(|i, j| a[*i][col].abs().partial_cmp(&a[*j][col].abs()).unwrap_or(std::cmp::Ordering::Equal))?;
        if a[pivot][col].abs() < 1e-18 {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        for row in 0..4 {
            if row == col {
                continue;
            }
            let f = a[row][col] / a[col][col];
            for k in col..4 {
                a[row][k] -= f * a[col][k];
            }
            b[row] -= f * b[col];
        }
    }
    let x = [b[0] / a[0][0], b[1] / a[1][1], b[2] / a[2][2], b[3] / a[3][3]];
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// Refine a similarity A→B on the pixels themselves (Gauss–Newton on the luma planes, sampled on a
/// grid), starting from a feature-based estimate. Removes the sub-pixel bias of point detectors
/// for small motions. Returns the input unchanged when the refinement does not converge or when
/// the warp and the excluded regions leave too few pixels to compare (that is a property of the
/// pair, not an error: the photometric check afterwards decides whether the fit stands).
pub fn refine_similarity(
    a: &[f32],
    b: &[f32],
    width: usize,
    height: usize,
    initial: [f64; 4],
    iterations: usize,
    exclude: &[[f64; 4]],
) -> Result<[f64; 4], String> {
    let total = width.checked_mul(height).ok_or("Frame size overflow")?;
    if width < 8 || height < 8 || a.len() != total || b.len() != total || iterations > 100 || exclude.len() > 64 {
        return Err("Luma planes must match the frame dimensions; at most 100 iterations and 64 exclusions".into());
    }
    if initial.iter().any(|v| !v.is_finite()) || initial[0].hypot(initial[1]) < 1e-6 {
        return Err("Invalid initial transform".into());
    }
    if exclude.iter().any(|r| r.iter().any(|v| !v.is_finite())) {
        return Err("Invalid exclusion box".into());
    }
    let mut inverse = invert(initial).ok_or("Degenerate initial transform")?;
    let stride = ((total / 65_536).max(1) as f64).sqrt().ceil().max(1.0) as usize;
    let margin = 6.0 + initial[2].abs().max(initial[3].abs());
    let excluded = |p: [f64; 2]| {
        exclude.iter().any(|r| {
            p[0] >= r[0] * width as f64 - margin
                && p[0] <= r[2] * width as f64 + margin
                && p[1] >= r[1] * height as f64 - margin
                && p[1] <= r[3] * height as f64 + margin
        })
    };
    let sigma = 0.04f64;
    let grad = |x: f64, y: f64| -> Option<(f32, f32, f32)> {
        let v = sample(a, width, height, x, y)?;
        let gx = (sample(a, width, height, x + 1.0, y)? - sample(a, width, height, x - 1.0, y)?) * 0.5;
        let gy = (sample(a, width, height, x, y + 1.0)? - sample(a, width, height, x, y - 1.0)?) * 0.5;
        Some((v, gx, gy))
    };
    let cost = |n: [f64; 4]| -> Option<f64> {
        let (mut sum, mut count) = (0.0f64, 0usize);
        let mut y = 0;
        while y < height {
            let mut x = 0;
            while x < width {
                let q = [x as f64, y as f64];
                let p = apply(n, q);
                if excluded(q) || excluded(p) {
                    x += stride;
                    continue;
                }
                if let (Some(sa), Some(sb)) = (sample(a, width, height, p[0], p[1]), b.get(y * width + x)) {
                    let r = f64::from(sa - sb);
                    sum += r * r / (1.0 + (r / sigma).powi(2));
                    count += 1;
                }
                x += stride;
            }
            y += stride;
        }
        (count * 8 >= (width / stride + 1) * (height / stride + 1)).then(|| sum / count.max(1) as f64)
    };
    let Some(mut best_cost) = cost(inverse) else { return Ok(initial) };
    for _ in 0..iterations {
        let mut jtj = [[0.0f64; 4]; 4];
        let mut jtr = [0.0f64; 4];
        let mut y = 1;
        while y + 1 < height {
            let mut x = 1;
            while x + 1 < width {
                let q = [x as f64, y as f64];
                let p = apply(inverse, q);
                if excluded(q) || excluded(p) {
                    x += stride;
                    continue;
                }
                if let (Some((va, gx, gy)), Some(vb)) = (grad(p[0], p[1]), b.get(y * width + x)) {
                    let (gx, gy) = (f64::from(gx), f64::from(gy));
                    let j = [gx * q[0] + gy * q[1], -gx * q[1] + gy * q[0], gx, gy];
                    let r = f64::from(va - vb);
                    let weight = 1.0 / (1.0 + (r / sigma).powi(2));
                    for i in 0..4 {
                        jtr[i] += weight * j[i] * r;
                        for k in 0..4 {
                            jtj[i][k] += weight * j[i] * j[k];
                        }
                    }
                }
                x += stride;
            }
            y += stride;
        }
        for i in 0..4 {
            jtj[i][i] += 1e-9;
        }
        let Some(delta) = solve4(jtj, [-jtr[0], -jtr[1], -jtr[2], -jtr[3]]) else { break };
        let candidate = [inverse[0] + delta[0], inverse[1] + delta[1], inverse[2] + delta[2], inverse[3] + delta[3]];
        if candidate[0].hypot(candidate[1]) < 1e-6 {
            break;
        }
        match cost(candidate) {
            Some(c) if c < best_cost => {
                best_cost = c;
                inverse = candidate;
                if delta.iter().all(|d| d.abs() < 1e-7) {
                    break;
                }
            }
            _ => break,
        }
    }
    Ok(invert(inverse).unwrap_or(initial))
}

impl Similarity {
    /// The same support with another transform (after refinement on the pixels).
    pub fn with_transform(&self, m: [f64; 4], pairs: &[([f64; 2], [f64; 2])], center: [f64; 2]) -> Similarity {
        let mut s = similarity_of(pairs, m, self.inliers.clone(), self.tested, center);
        if pairs.is_empty() {
            s.rms_error_px = self.rms_error_px;
        }
        s
    }
}

/// The camera state of a comparison from its fit and the photometric check: *static* when the fit
/// is within the static tolerance, *moving* when warping lowers the photometric error, *static*
/// again when the motion is negligible (≤ 1 px, 0.5 % scale, 0.1°) and standing still explains the
/// pair at least as well, *unverified* otherwise.
pub fn classify(fit: &CameraFit, before: Option<f32>, after: Option<f32>, static_px: f64) -> CameraState {
    match &fit.camera {
        Fit::Insufficient { .. } => CameraState::NoEvidence,
        Fit::Estimated { fit: s } => {
            let still = s.center_displacement_px[0].hypot(s.center_displacement_px[1]) <= static_px
                && (s.scale - 1.0).abs() <= 0.001
                && s.rotation_degrees.abs() <= 0.02;
            let negligible =
                s.center_displacement_px[0].hypot(s.center_displacement_px[1]) <= 1.0 && (s.scale - 1.0).abs() <= 0.005 && s.rotation_degrees.abs() <= 0.1;
            match (before, after) {
                _ if still => CameraState::Static,
                (Some(b), Some(a)) if a <= b => CameraState::Moving,
                (Some(_), Some(_)) if negligible => CameraState::Static,
                _ => CameraState::Unverified,
            }
        }
    }
}

fn sample(luma: &[f32], w: usize, h: usize, x: f64, y: f64) -> Option<f32> {
    if !x.is_finite() || !y.is_finite() || x < 0.0 || y < 0.0 || x > w as f64 - 1.0 || y > h as f64 - 1.0 {
        return None;
    }
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (fx, fy) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
    let x1 = (x0 + 1).min(w - 1);
    let y1 = (y0 + 1).min(h - 1);
    let at = |xx: usize, yy: usize| luma.get(yy * w + xx).copied().unwrap_or(0.0);
    Some(at(x0, y0) * (1.0 - fx) * (1.0 - fy) + at(x1, y0) * fx * (1.0 - fy) + at(x0, y1) * (1.0 - fx) * fy + at(x1, y1) * fx * fy)
}

/// Mean absolute luma difference (0–1) between frame B and frame A warped by `transform`
/// (identity when `None`), over the pixels of B whose source lands inside A. `None` when fewer
/// than a quarter of the pixels overlap.
pub fn photometric_error(a: &[f32], b: &[f32], width: usize, height: usize, transform: Option<[f64; 4]>) -> Result<Option<f32>, String> {
    let total = width.checked_mul(height).ok_or("Frame size overflow")?;
    if width == 0 || height == 0 || a.len() != total || b.len() != total {
        return Err("Luma planes must match the frame dimensions".into());
    }
    let inverse = match transform {
        Some(m) => Some(invert(m).ok_or("Degenerate transform")?),
        None => None,
    };
    let stride = ((total / 65_536).max(1) as f64).sqrt().ceil().max(1.0) as usize;
    let (mut sum, mut count, mut visited) = (0.0f64, 0usize, 0usize);
    let mut y = 0;
    while y < height {
        let mut x = 0;
        while x < width {
            visited += 1;
            let source = match inverse {
                Some(inv) => apply(inv, [x as f64, y as f64]),
                None => [x as f64, y as f64],
            };
            if let (Some(sa), Some(sb)) = (sample(a, width, height, source[0], source[1]), b.get(y * width + x)) {
                sum += f64::from((sa - sb).abs());
                count += 1;
            }
            x += stride;
        }
        y += stride;
    }
    if count * 4 < visited {
        return Ok(None);
    }
    Ok(Some((sum / count.max(1) as f64) as f32))
}

/// Grey RGBA picture of |B − warp(A)| amplified ×4, black where the warp leaves the frame.
pub fn difference_image(a: &[f32], b: &[f32], width: usize, height: usize, transform: Option<[f64; 4]>) -> Result<Vec<u8>, String> {
    let total = width.checked_mul(height).ok_or("Frame size overflow")?;
    if width == 0 || height == 0 || a.len() != total || b.len() != total || total > 16_777_216 {
        return Err("Luma planes must match the frame dimensions".into());
    }
    let inverse = match transform {
        Some(m) => Some(invert(m).ok_or("Degenerate transform")?),
        None => None,
    };
    let mut out = vec![0u8; total * 4];
    for y in 0..height {
        for x in 0..width {
            let source = match inverse {
                Some(inv) => apply(inv, [x as f64, y as f64]),
                None => [x as f64, y as f64],
            };
            let value = match (sample(a, width, height, source[0], source[1]), b.get(y * width + x)) {
                (Some(sa), Some(sb)) => ((sa - sb).abs() * 4.0 * 255.0).clamp(0.0, 255.0) as u8,
                _ => 0,
            };
            let i = (y * width + x) * 4;
            if let Some(px) = out.get_mut(i..i + 4) {
                px[0] = value;
                px[1] = value;
                px[2] = value;
                px[3] = 255;
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pairs_from(m: [f64; 4], n: usize, outliers: usize) -> Vec<([f64; 2], [f64; 2])> {
        let mut out = Vec::new();
        for i in 0..n {
            let p = [((i * 37) % 300) as f64 + 10.0, ((i * 91) % 200) as f64 + 10.0];
            let mut q = apply(m, p);
            if i < outliers {
                q[0] += 25.0 + i as f64;
                q[1] -= 17.0;
            }
            out.push((p, q));
        }
        out
    }
    #[test]
    fn recovers_translation_rotation_and_scale_with_outliers() {
        let angle = 5f64.to_radians();
        let s = 1.03;
        let m = [s * angle.cos(), s * angle.sin(), 12.5, -4.25];
        let pairs = pairs_from(m, 120, 30);
        let Fit::Estimated { fit } = fit_similarity(&pairs, [160.0, 100.0], &FitOptions::default()).unwrap() else { panic!("expected a fit") };
        for k in 0..4 {
            assert!((fit.transform[k] - m[k]).abs() < 1e-6, "{:?} vs {:?}", fit.transform, m);
        }
        assert_eq!(fit.inliers.len(), 90);
        assert!((fit.scale - s).abs() < 1e-6 && (fit.rotation_degrees - 5.0).abs() < 1e-6 && fit.rms_error_px < 1e-6);
        let inv = invert(m).unwrap();
        let back = apply(inv, apply(m, [33.0, 44.0]));
        assert!((back[0] - 33.0).abs() < 1e-9 && (back[1] - 44.0).abs() < 1e-9);
    }
    #[test]
    fn insufficient_evidence_is_explicit() {
        let pairs = pairs_from([1.0, 0.0, 0.0, 0.0], 6, 0);
        assert!(matches!(fit_similarity(&pairs, [0.0, 0.0], &FitOptions::default()).unwrap(), Fit::Insufficient { .. }));
        let random: Vec<([f64; 2], [f64; 2])> = (0..40).map(|i| ([i as f64, (i * 7 % 13) as f64], [(i * 11 % 17) as f64, (i * 3 % 29) as f64 * 5.0])).collect();
        assert!(matches!(fit_similarity(&random, [0.0, 0.0], &FitOptions::default()).unwrap(), Fit::Insufficient { .. }));
        assert!(fit_similarity(&[([f64::NAN, 0.0], [0.0, 0.0])], [0.0, 0.0], &FitOptions::default()).is_err());
        assert!(fit_similarity(&[], [0.0, 0.0], &FitOptions { tolerance_px: 0.0, ..FitOptions::default() }).is_err());
    }
    #[test]
    fn camera_is_the_spread_group_not_the_largest_and_a_lone_group_is_accepted() {
        let size = [640.0, 360.0];
        let mut pairs: Vec<([f64; 2], [f64; 2])> = Vec::new();
        for i in 0..60 {
            let p = [((i * 97) % 600) as f64 + 20.0, ((i * 53) % 320) as f64 + 20.0];
            pairs.push((p, p));
        }
        for i in 0..90 {
            let p = [200.0 + ((i * 31) % 240) as f64, 110.0 + ((i * 17) % 140) as f64];
            pairs.push((p, [p[0], p[1] - 40.0]));
        }
        let fit = camera_between(&pairs, size, &CameraOptions::default()).unwrap();
        let Fit::Estimated { fit: camera } = &fit.camera else { panic!("the spread static group is the camera: {:?}", fit.camera) };
        assert!(camera.center_displacement_px[0].abs() < 1e-6 && camera.center_displacement_px[1].abs() < 1e-6);
        assert_eq!(camera.inliers.len(), 60);
        assert_eq!(fit.regions.len(), 1);
        assert_eq!(fit.regions[0].count, 90);
        assert!((fit.regions[0].displacement_px[1] + 40.0).abs() < 1e-6);
        assert!(fit.regions[0].bounds[1] > 0.25 && fit.regions[0].bounds[3] < 0.75, "{:?}", fit.regions[0].bounds);
        let band: Vec<([f64; 2], [f64; 2])> = (0..40)
            .map(|i| {
                let p = [((i * 61) % 600) as f64 + 20.0, 150.0 + ((i * 7) % 60) as f64];
                (p, [p[0] + 2.0, p[1]])
            })
            .collect();
        let lone = camera_between(&band, size, &CameraOptions::default()).unwrap();
        let Fit::Estimated { fit: camera } = &lone.camera else { panic!("a single dominant group is accepted: {:?}", lone.camera) };
        assert!((camera.center_displacement_px[0] - 2.0).abs() < 1e-6);
        assert!(lone.span[1] < 0.5, "coverage is reported even when weak: {:?}", lone.span);
        assert_eq!(classify(&lone, Some(0.1), Some(0.05), 0.15), CameraState::Moving);
        assert_eq!(classify(&lone, Some(0.1), Some(0.2), 0.15), CameraState::Unverified);
        assert_eq!(classify(&fit, Some(0.1), Some(0.2), 0.15), CameraState::Static);
        let tiny: Vec<([f64; 2], [f64; 2])> = (0..40)
            .map(|i| {
                let p = [((i * 61) % 600) as f64 + 20.0, 150.0 + ((i * 7) % 60) as f64];
                (p, [p[0] + 0.3, p[1]])
            })
            .collect();
        let tiny = camera_between(&tiny, size, &CameraOptions::default()).unwrap();
        assert_eq!(classify(&tiny, Some(0.1047), Some(0.1048), 0.15), CameraState::Static, "negligible motion that explains nothing better is static");
        assert_eq!(classify(&tiny, Some(0.1), Some(0.05), 0.15), CameraState::Moving);
        assert_eq!(classify(&tiny, None, None, 0.15), CameraState::Unverified);
    }

    #[test]
    fn the_group_enclosing_the_others_is_the_camera_even_when_it_is_sparse() {
        let size = [640.0, 360.0];
        let mut pairs: Vec<([f64; 2], [f64; 2])> = Vec::new();
        for i in 0..30 {
            let p = if i % 2 == 0 {
                [50.0 + ((i * 37) % 540) as f64, if i % 4 == 0 { 30.0 } else { 340.0 }]
            } else {
                [if i % 4 == 1 { 50.0 } else { 620.0 }, 30.0 + ((i * 53) % 310) as f64]
            };
            pairs.push((p, p));
        }
        for i in 0..120 {
            let p = [60.0 + ((i * 31) % 480) as f64, 100.0 + ((i * 17) % 150) as f64];
            pairs.push((p, [p[0], p[1] - 40.0]));
        }
        let fit = camera_between(&pairs, size, &CameraOptions::default()).unwrap();
        let Fit::Estimated { fit: camera } = &fit.camera else { panic!("the enclosing frame of static points is the camera: {:?}", fit.camera) };
        assert_eq!(camera.inliers.len(), 30);
        assert!(camera.center_displacement_px[1].abs() < 1e-6);
        assert_eq!(fit.regions.len(), 1);
        assert_eq!(fit.regions[0].count, 120);
        let majority: Vec<([f64; 2], [f64; 2])> =
            (0..40).map(|i| ([200.0 + (i * 5) as f64, 170.0 + (i % 3) as f64], [205.0 + (i * 5) as f64, 170.0 + (i % 3) as f64])).collect();
        let fit = camera_between(&majority, size, &CameraOptions::default()).unwrap();
        assert!(matches!(fit.camera, Fit::Estimated { .. }), "one group with every match is accepted: {:?}", fit.camera);
        let block = |x0: f64, y0: f64, count: usize, d: [f64; 2]| {
            (0..count).map(move |i| {
                let p = [x0 + (i % 4) as f64 * 30.0, y0 + (i / 4) as f64 * 30.0];
                (p, [p[0] + d[0], p[1] + d[1]])
            })
        };
        let split: Vec<([f64; 2], [f64; 2])> =
            block(40.0, 40.0, 12, [5.0, 0.0]).chain(block(280.0, 160.0, 10, [0.0, 20.0])).chain(block(480.0, 250.0, 10, [-15.0, 3.0])).collect();
        let fit = camera_between(&split, size, &CameraOptions::default()).unwrap();
        assert!(matches!(fit.camera, Fit::Insufficient { .. }), "a small concentrated group without a majority is not the camera: {:?}", fit.camera);
    }
    #[test]
    fn pixel_refinement_recovers_a_small_motion_from_a_coarse_guess() {
        let (w, h) = (96, 64);
        let mut seed = 7u32;
        let base: Vec<f32> = (0..w * h)
            .map(|i| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let noise = (seed % 100) as f32 / 400.0;
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                0.4 + 0.2 * (x * 0.25).sin() * (y * 0.19).cos() + noise
            })
            .collect();
        let smooth: Vec<f32> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let mut acc = 0.0;
                let mut n = 0.0;
                for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        let (xx, yy) = (x as i32 + dx, y as i32 + dy);
                        if xx >= 0 && yy >= 0 && (xx as usize) < w && (yy as usize) < h {
                            acc += base[yy as usize * w + xx as usize];
                            n += 1.0;
                        }
                    }
                }
                acc / n
            })
            .collect();
        let truth = [1.002 * 0.3f64.to_radians().cos(), 1.002 * 0.3f64.to_radians().sin(), 0.7, -0.4];
        let inv = invert(truth).unwrap();
        let moved: Vec<f32> = (0..w * h)
            .map(|i| {
                let p = apply(inv, [(i % w) as f64, (i / w) as f64]);
                sample(&smooth, w, h, p[0], p[1]).unwrap_or(0.4)
            })
            .collect();
        let guess = [1.0, 0.0, 0.2, 0.1];
        let refined = refine_similarity(&smooth, &moved, w, h, guess, 20, &[]).unwrap();
        for k in 0..4 {
            assert!((refined[k] - truth[k]).abs() < 0.02, "{refined:?} vs {truth:?}");
        }
        let centre = apply(refined, [48.0, 32.0]);
        let expected = apply(truth, [48.0, 32.0]);
        assert!((centre[0] - expected[0]).abs() < 0.05 && (centre[1] - expected[1]).abs() < 0.05, "{centre:?} vs {expected:?}");
        let masked = refine_similarity(&smooth, &moved, w, h, guess, 20, &[[0.3, 0.3, 0.6, 0.6]]).unwrap();
        assert!((masked[2] - truth[2]).abs() < 0.05 && (masked[3] - truth[3]).abs() < 0.05, "{masked:?}");
        assert!(refine_similarity(&smooth, &moved[..10], w, h, guess, 5, &[]).is_err());
        let far = [1.0, 0.0, 1000.0, 1000.0];
        assert_eq!(refine_similarity(&smooth, &moved, w, h, far, 5, &[]).unwrap(), far, "no overlap keeps the fit instead of failing the comparison");
        assert_eq!(refine_similarity(&smooth, &moved, w, h, guess, 5, &[[0.0, 0.0, 1.0, 1.0]]).unwrap(), guess, "regions covering the frame keep the fit");
        assert!(refine_similarity(&smooth, &moved, w, h, [0.0, 0.0, 0.0, 0.0], 5, &[]).is_err());
        assert!(refine_similarity(&smooth, &moved, w, h, guess, 5, &[[f64::NAN, 0.0, 1.0, 1.0]]).is_err());
    }
    #[test]
    fn photometric_error_drops_when_the_warp_is_right() {
        let (w, h) = (64, 48);
        let pattern = |x: f64, y: f64| (((x * 0.3).sin() + (y * 0.2).cos()) * 0.25 + 0.5) as f32;
        let a: Vec<f32> = (0..w * h).map(|i| pattern((i % w) as f64, (i / w) as f64)).collect();
        let shift = [1.0, 0.0, 3.0, -2.0];
        let inv = invert(shift).unwrap();
        let b: Vec<f32> = (0..w * h)
            .map(|i| {
                let s = apply(inv, [(i % w) as f64, (i / w) as f64]);
                pattern(s[0], s[1])
            })
            .collect();
        let before = photometric_error(&a, &b, w, h, None).unwrap().unwrap();
        let after = photometric_error(&a, &b, w, h, Some(shift)).unwrap().unwrap();
        assert!(after < before * 0.2, "before {before} after {after}");
        let diff = difference_image(&a, &b, w, h, Some(shift)).unwrap();
        assert_eq!(diff.len(), w * h * 4);
        assert!(photometric_error(&a, &b, w, h, Some([1.0, 0.0, 1000.0, 0.0])).unwrap().is_none());
        assert!(photometric_error(&a, &b[..10], w, h, None).is_err());
        assert!(difference_image(&a, &b, w, h, Some([0.0, 0.0, 0.0, 0.0])).is_err());
    }
}

//! 2D camera, steps A–D: feature points on every rendered frame of the range, matches between
//! consecutive frames and to anchor frames, a camera similarity per comparison (shared motion,
//! moving regions separated), pixel refinement, photometric verification, and the jointly solved
//! camera path of the range. Pure Rust; no model, no project edits.
use super::{panel_state::ReverseEngineeringCapture, reverse_engineering};
use crate::FilmcraftApp;
use filmcraft_vision::{
    camera2d::{self, CameraFit, CameraOptions, CameraState, Fit, FitOptions, MovingRegion},
    cinema,
    features::{self, Feature, Options},
    matching::{self, Match, MatchOptions},
    motion,
    trajectory::{self, Edge, FramePose},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

pub const MAX_RANGE_FRAMES: usize = 36_000;
pub const MAX_TOTAL_POINTS: usize = 2_000_000;
pub const MIN_WIDTH: usize = 160;
pub const MAX_WIDTH: usize = 4096;
const DISPLAY_CACHE: usize = 8;
const VIEW_POINTS: u8 = 0;
const VIEW_MATCHES: u8 = 1;
const VIEW_DIFFERENCE: u8 = 2;
const REFINE_ITERATIONS: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AnchorOptions {
    pub gap: usize,
    pub step: usize,
    pub long_gap: usize,
}
impl Default for AnchorOptions {
    fn default() -> Self {
        Self { gap: 8, step: 4, long_gap: 24 }
    }
}
impl AnchorOptions {
    pub fn validate(&self) -> Result<(), String> {
        if !(2..=64).contains(&self.gap)
            || !(1..=64).contains(&self.step)
            || self.step > self.gap
            || self.long_gap > 256
            || (self.long_gap != 0 && self.long_gap <= self.gap)
        {
            return Err(
                "Anchor options: gap 2–64 frames, step 1–64 frames and not larger than the gap, long gap 0 (off) or larger than the gap up to 256".into()
            );
        }
        Ok(())
    }
    fn memory(&self) -> usize {
        self.gap.max(self.long_gap).saturating_add(1)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameFeatures {
    pub frame_index: usize,
    pub timeline_tick: i64,
    pub analyzed_size: [usize; 2],
    pub mean_luma: f32,
    pub features: Vec<Feature>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comparison {
    pub from_frame: usize,
    pub to_frame: usize,
    pub matches: Vec<Match>,
    pub fit: Fit,
    pub coverage_cells: usize,
    pub span: [f64; 2],
    pub regions: Vec<MovingRegion>,
    pub state: CameraState,
    pub photometric_before: Option<f32>,
    pub photometric_after: Option<f32>,
    pub verified: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeatureReport {
    pub schema_version: u32,
    pub analyzer: String,
    pub captures: Vec<ReverseEngineeringCapture>,
    pub project_revision: u64,
    pub completed_at_unix_ms: u64,
    pub elapsed_ms: u64,
    pub source_size: [u32; 2],
    pub analysis_width: usize,
    pub options: Options,
    pub match_options: MatchOptions,
    pub camera_options: CameraOptions,
    pub anchor_options: AnchorOptions,
    pub total_points: usize,
    pub frames: Vec<FrameFeatures>,
    pub comparisons: Vec<Comparison>,
    pub anchors: Vec<Comparison>,
    pub trajectory: Vec<FramePose>,
    pub limitations: Vec<String>,
}
#[derive(Clone)]
struct Job {
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicUsize>,
    total: usize,
    result: Arc<Mutex<Option<Result<FeatureReport, String>>>>,
}
#[derive(Clone)]
struct DisplayJob {
    key: (u64, usize, u8),
    result: Arc<Mutex<Option<Result<(usize, usize, Vec<u8>), String>>>>,
}
#[derive(Clone, Default)]
struct DisplayCache {
    order: Vec<(u64, usize, u8)>,
}
struct Analysed {
    luma: Vec<f32>,
    features: Vec<Feature>,
    descriptors: Vec<matching::Descriptor>,
}
fn job_id() -> egui::Id {
    egui::Id::new("reverse-camera2d-job")
}
fn display_job_id() -> egui::Id {
    egui::Id::new("reverse-camera2d-display-job")
}
fn display_cache_id() -> egui::Id {
    egui::Id::new("reverse-camera2d-display-cache")
}
fn texture_id(key: (u64, usize, u8)) -> egui::Id {
    egui::Id::new(("reverse-camera2d-texture", key))
}
fn view_code(view: &str) -> u8 {
    match view {
        "matches" => VIEW_MATCHES,
        "difference" => VIEW_DIFFERENCE,
        _ => VIEW_POINTS,
    }
}
pub(super) fn is_running(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<Job>(job_id())).is_some()
}
pub(super) fn current(app: &FilmcraftApp) -> bool {
    let s = &app.ui.panels.reverse_engineering;
    s.camera2d_report.as_ref().is_some_and(|r| {
        r.project_revision == app.session.revision
            && r.options == s.camera2d_options
            && r.match_options == s.camera2d_match
            && r.camera_options == s.camera2d_camera
            && r.anchor_options == s.camera2d_anchors
            && r.analysis_width == s.camera2d_width
            && r.captures == s.camera2d_captures
            && r.frames.len() == r.captures.len()
            && r.trajectory.len() == r.frames.len()
            && r.comparisons.len() + 1 == r.frames.len().max(1)
            && reverse_engineering::captures_are_current(app, &r.captures)
    })
}
pub(super) fn prepare(app: &mut FilmcraftApp) -> Result<(), String> {
    let captures = match reverse_engineering::range_captures(app, MAX_RANGE_FRAMES) {
        Ok(captures) => captures,
        Err(error) => {
            app.ui.panels.reverse_engineering.camera2d_captures.clear();
            return Err(error);
        }
    };
    let s = &mut app.ui.panels.reverse_engineering;
    if s.camera2d_captures != captures {
        s.camera2d_frame = captures.len() / 2;
        s.camera2d_captures = captures;
    }
    Ok(())
}
fn timestamp() -> Result<u64, String> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())
        .and_then(|d| u64::try_from(d.as_millis()).map_err(|e| e.to_string()))
}
fn mean_luma(luma: &[f32]) -> f32 {
    if luma.is_empty() {
        return 0.0;
    }
    (luma.iter().map(|v| f64::from(*v)).sum::<f64>() / luma.len() as f64) as f32
}
fn fit_transform(fit: &Fit) -> Option<[f64; 4]> {
    match fit {
        Fit::Estimated { fit } => Some(fit.transform),
        Fit::Insufficient { .. } => None,
    }
}
fn fit_inliers(fit: &Fit) -> usize {
    match fit {
        Fit::Estimated { fit } => fit.inliers.len(),
        Fit::Insufficient { .. } => 0,
    }
}
/// One comparison between two analysed frames: match, camera fit with regions, pixel refinement,
/// photometric verification and state.
fn compare(
    from: &Analysed,
    to: &Analysed,
    from_index: usize,
    to_index: usize,
    size: [usize; 2],
    match_options: &MatchOptions,
    camera_options: &CameraOptions,
) -> Result<Comparison, String> {
    let [w, h] = size;
    let matches = matching::match_features(&from.descriptors, &from.features, &to.descriptors, &to.features, match_options)?;
    let pairs: Vec<([f64; 2], [f64; 2])> = matches
        .iter()
        .filter_map(|m| {
            let a = from.features.get(m.a)?;
            let b = to.features.get(m.b)?;
            Some(([f64::from(a.x), f64::from(a.y)], [f64::from(b.x), f64::from(b.y)]))
        })
        .collect();
    let mut camera: CameraFit = camera2d::camera_between(&pairs, [w as f64, h as f64], camera_options)?;
    if let Fit::Estimated { fit } = &camera.camera {
        let boxes: Vec<[f64; 4]> = camera.regions.iter().map(|r| r.bounds).collect();
        let refined = camera2d::refine_similarity(&from.luma, &to.luma, w, h, fit.transform, REFINE_ITERATIONS, &boxes)?;
        camera.camera = Fit::Estimated { fit: fit.with_transform(refined, &pairs, [w as f64 / 2.0, h as f64 / 2.0]) };
    }
    let photometric_before = camera2d::photometric_error(&from.luma, &to.luma, w, h, None)?;
    let photometric_after = match fit_transform(&camera.camera) {
        Some(m) => camera2d::photometric_error(&from.luma, &to.luma, w, h, Some(m))?,
        None => None,
    };
    let state = camera2d::classify(&camera, photometric_before, photometric_after, camera_options.static_px);
    let verified = matches!(state, CameraState::Static | CameraState::Moving);
    Ok(Comparison {
        from_frame: from_index,
        to_frame: to_index,
        matches,
        fit: camera.camera,
        coverage_cells: camera.coverage_cells,
        span: camera.span,
        regions: camera.regions,
        state,
        photometric_before,
        photometric_after,
        verified,
    })
}
fn evidence_weight(c: &Comparison) -> f64 {
    (fit_inliers(&c.fit) as f64 * c.coverage_cells as f64 / (camera2d::GRID * camera2d::GRID) as f64).max(1.0)
}
fn edge_of(c: &Comparison) -> Edge {
    match (c.state, fit_transform(&c.fit)) {
        (CameraState::Moving, Some(m)) => Edge { from: c.from_frame, to: c.to_frame, transform: m, weight: evidence_weight(c) },
        (CameraState::Static, _) => Edge { from: c.from_frame, to: c.to_frame, transform: [1.0, 0.0, 0.0, 0.0], weight: evidence_weight(c) },
        _ => Edge { from: c.from_frame, to: c.to_frame, transform: [1.0, 0.0, 0.0, 0.0], weight: 1.0 },
    }
}
pub(super) fn poll(app: &mut FilmcraftApp, ctx: &egui::Context) {
    if let Some(job) = ctx.data(|d| d.get_temp::<DisplayJob>(display_job_id())) {
        let result = job.result.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
        if let Some(result) = result {
            ctx.data_mut(|d| d.remove::<DisplayJob>(display_job_id()));
            match result {
                Ok((w, h, rgba)) if rgba.len() == w.saturating_mul(h).saturating_mul(4) && w > 0 && h > 0 => {
                    let handle = ctx.load_texture(
                        format!("reverse-camera2d-{}-{}-{}", job.key.0, job.key.1, job.key.2),
                        egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba),
                        egui::TextureOptions::LINEAR,
                    );
                    let mut cache = ctx.data(|d| d.get_temp::<DisplayCache>(display_cache_id())).unwrap_or_default();
                    cache.order.retain(|k| *k != job.key);
                    cache.order.push(job.key);
                    while cache.order.len() > DISPLAY_CACHE {
                        let old = cache.order.remove(0);
                        ctx.data_mut(|d| d.remove::<egui::TextureHandle>(texture_id(old)));
                    }
                    ctx.data_mut(|d| {
                        d.insert_temp(texture_id(job.key), handle);
                        d.insert_temp(display_cache_id(), cache);
                    });
                }
                Ok(_) => app.ui.status = "The analysed frame could not be displayed (unexpected image size)".into(),
                Err(e) => app.ui.status = e,
            }
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }
    let Some(job) = ctx.data(|d| d.get_temp::<Job>(job_id())) else {
        return;
    };
    let result = job.result.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
    if let Some(result) = result {
        ctx.data_mut(|d| d.remove::<Job>(job_id()));
        match result {
            Ok(report) if !job.cancel.load(Ordering::Relaxed) => {
                let moving = report.comparisons.iter().filter(|c| c.state == CameraState::Moving).count();
                let still = report.comparisons.iter().filter(|c| c.state == CameraState::Static).count();
                app.ui.status = format!(
                    "{} points on {} frames · {still} static, {moving} moving, {} other comparisons · {} anchors · {:.1} s",
                    report.total_points,
                    report.frames.len(),
                    report.comparisons.len().saturating_sub(still + moving),
                    report.anchors.len(),
                    report.elapsed_ms as f64 / 1000.0
                );
                app.ui.panels.reverse_engineering.camera2d_report = Some(report);
            }
            Ok(_) => app.ui.status = "2D camera analysis cancelled; previous result preserved".into(),
            Err(e) => {
                app.ui.status = e.clone();
                ctx.data_mut(|d| d.insert_temp(egui::Id::new("reverse-tools-error"), e));
            }
        }
    } else {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}
fn start(app: &mut FilmcraftApp, ctx: &egui::Context) -> Result<Value, String> {
    if is_running(ctx) {
        return Err("2D camera analysis is already running".into());
    }
    if cfg!(target_arch = "wasm32") {
        return Err("2D camera analysis currently requires the desktop app.".into());
    }
    prepare(app)?;
    let s = &app.ui.panels.reverse_engineering;
    let captures = s.camera2d_captures.clone();
    let first = captures.first().ok_or("Select a range with at least one frame")?;
    let options = s.camera2d_options;
    options.validate()?;
    let match_options = s.camera2d_match;
    match_options.validate()?;
    let camera_options = s.camera2d_camera;
    camera_options.validate()?;
    let anchor_options = s.camera2d_anchors;
    anchor_options.validate()?;
    let analysis_width = s.camera2d_width;
    if !(MIN_WIDTH..=MAX_WIDTH).contains(&analysis_width) {
        return Err("Analysis width must be 160–4096 pixels".into());
    }
    if captures.len().checked_mul(options.max_points).is_none_or(|n| n > MAX_TOTAL_POINTS) {
        return Err(format!("Up to {MAX_TOTAL_POINTS} points per result: lower max points or shorten the range"));
    }
    let (width, height) = app.session.project.source_size(first.item).ok_or("No video dimensions")?;
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 67_108_864 {
        return Err("Invalid or oversized source dimensions".into());
    }
    let revision = app.session.revision;
    let project = app.session.project.clone();
    let provider = app.session.media.full_res_provider(project.clone(), app.session.services.clone());
    let job =
        Job { cancel: Arc::new(AtomicBool::new(false)), progress: Arc::new(AtomicUsize::new(0)), total: captures.len(), result: Arc::new(Mutex::new(None)) };
    let worker = job.clone();
    let repaint = ctx.clone();
    std::thread::Builder::new()
        .name("filmcraft-camera2d".into())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<FeatureReport, String> {
                let started = std::time::Instant::now();
                let scale = (analysis_width as f32 / width as f32).min(1.0);
                let mut frames = Vec::new();
                frames.try_reserve_exact(captures.len()).map_err(|e| format!("Cannot allocate feature results: {e}"))?;
                let mut comparisons = Vec::new();
                comparisons.try_reserve_exact(captures.len().saturating_sub(1)).map_err(|e| format!("Cannot allocate comparison results: {e}"))?;
                let mut anchors = Vec::new();
                let mut total_points = 0usize;
                let mut memory: std::collections::VecDeque<(usize, Analysed)> = std::collections::VecDeque::new();
                let mut size: Option<[usize; 2]> = None;
                for (i, capture) in captures.iter().enumerate() {
                    if worker.cancel.load(Ordering::Relaxed) {
                        return Err("2D camera analysis cancelled".into());
                    }
                    if started.elapsed().as_secs() > 3600 {
                        return Err("2D camera analysis timed out; choose a shorter range".into());
                    }
                    let image = filmcraft_render::render_item(&project, capture.item, capture.media_time, scale, &provider)
                        .ok_or_else(|| format!("Frame {} could not be rendered. Check the media and retry.", i + 1))?;
                    let rgba = image.to_rgba8();
                    let (w, h) = (image.w, image.h);
                    if size.is_some_and(|s| s != [w, h]) {
                        return Err("Rendered frame dimensions changed during analysis".into());
                    }
                    size = Some([w, h]);
                    let luma = features::luma(w, h, &rgba)?;
                    let detected = features::detect(w, h, &rgba, &options)?;
                    total_points = total_points.saturating_add(detected.len());
                    if total_points > MAX_TOTAL_POINTS {
                        return Err(format!("More than {MAX_TOTAL_POINTS} points: lower max points or shorten the range"));
                    }
                    let descriptors = matching::describe(w, h, &rgba, &detected)?;
                    let current = Analysed { luma, features: detected, descriptors };
                    if let Some((_, previous)) = memory.back() {
                        comparisons.push(compare(previous, &current, i - 1, i, [w, h], &match_options, &camera_options)?);
                    }
                    if i % anchor_options.step == 0 {
                        for gap in [anchor_options.gap, anchor_options.long_gap] {
                            if gap == 0 || i < gap {
                                continue;
                            }
                            if let Some((from_index, anchor)) = memory.iter().find(|(k, _)| *k + gap == i) {
                                anchors.push(compare(anchor, &current, *from_index, i, [w, h], &match_options, &camera_options)?);
                            }
                        }
                    }
                    frames.push(FrameFeatures {
                        frame_index: i,
                        timeline_tick: capture.timeline_time.0,
                        analyzed_size: [w, h],
                        mean_luma: mean_luma(&current.luma),
                        features: current.features.clone(),
                    });
                    memory.push_back((i, current));
                    while memory.len() > anchor_options.memory() {
                        memory.pop_front();
                    }
                    worker.progress.store(i + 1, Ordering::Relaxed);
                    repaint.request_repaint();
                }
                let mut edges: Vec<Edge> = comparisons.iter().map(edge_of).collect();
                edges.extend(anchors.iter().filter(|a| a.state == CameraState::Moving || a.state == CameraState::Static).map(edge_of));
                let [w, h] = size.ok_or("No frames analysed")?;
                let path = trajectory::solve(frames.len(), &edges, [w as f64 / 2.0, h as f64 / 2.0])?;
                Ok(FeatureReport {
                    schema_version: 4,
                    analyzer: "filmcraft-vision/camera2d-trajectory-v1".into(),
                    captures,
                    project_revision: revision,
                    completed_at_unix_ms: timestamp()?,
                    elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                    source_size: [width, height],
                    analysis_width,
                    options,
                    match_options,
                    camera_options,
                    anchor_options,
                    total_points,
                    frames,
                    comparisons,
                    anchors,
                    trajectory: path,
                    limitations: vec![
                        "Points: multi-scale Harris corners with sub-pixel refinement and a dominant gradient orientation on every rendered frame; positions are in pixels of the analysed image, not the source.".into(),
                        "Matches: rotation-aware gradient-histogram descriptors, Lowe ratio test and cross-check, between consecutive frames and from each anchor frame to the frame `gap` later. Repeated texture, flat areas and black frames produce few or no matches.".into(),
                        "Camera per comparison: the 2D similarity (translation, uniform scale, rotation) shared by matches spread over the frame (grid coverage and span, not the largest group), refined on the pixels with moving regions excluded; what moves differently is grouped into regions. Image-plane motion under a static-scene assumption, not physical pose, depth or lens.".into(),
                        "States: static (no significant motion), moving (warping lowers the photometric error), not verified (standing still explains the frames at least as well) and no camera evidence (matches too few or too concentrated). Only static and moving comparisons feed the path; the rest count as standing still with low weight.".into(),
                        "Path: absolute similarities per frame solved jointly from consecutive comparisons and anchors by weighted least squares; residual per frame is the largest disagreement between a measurement and the solved path. A subject covering most of the frame, or a pan over a featureless background, leaves the path unsupported there.".into(),
                    ],
                })
            }))
            .unwrap_or_else(|_| Err("The 2D camera worker failed; previous result preserved".into()));
            *worker.result.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(outcome);
            repaint.request_repaint();
        })
        .map_err(|e| format!("Could not start 2D camera analysis: {e}"))?;
    ctx.data_mut(|d| d.insert_temp(job_id(), job));
    Ok(json!({"started":true,"frames":app.ui.panels.reverse_engineering.camera2d_captures.len()}))
}
fn request_display(app: &FilmcraftApp, ctx: &egui::Context, report: &FeatureReport, frame: usize, view: u8) {
    let key = (report.completed_at_unix_ms, frame, view);
    if ctx.data(|d| d.get_temp::<egui::TextureHandle>(texture_id(key))).is_some() || ctx.data(|d| d.get_temp::<DisplayJob>(display_job_id())).is_some() {
        return;
    }
    let Some(capture) = report.captures.get(frame).cloned() else {
        return;
    };
    let previous = if view == VIEW_DIFFERENCE { frame.checked_sub(1).and_then(|p| report.captures.get(p).cloned()) } else { None };
    let transform =
        if view == VIEW_DIFFERENCE { frame.checked_sub(1).and_then(|p| report.comparisons.get(p)).and_then(|c| fit_transform(&c.fit)) } else { None };
    let Some((width, _)) = app.session.project.source_size(capture.item) else {
        return;
    };
    let scale = (report.analysis_width as f32 / width.max(1) as f32).min(1.0);
    let project = app.session.project.clone();
    let provider = app.session.media.full_res_provider(project.clone(), app.session.services.clone());
    let job = DisplayJob { key, result: Arc::new(Mutex::new(None)) };
    let worker = job.clone();
    let repaint = ctx.clone();
    let spawned = std::thread::Builder::new().name("filmcraft-camera2d-display".into()).spawn(move || {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<(usize, usize, Vec<u8>), String> {
            let image = filmcraft_render::render_item(&project, capture.item, capture.media_time, scale, &provider)
                .ok_or("The frame could not be rendered for display")?;
            let rgba = image.to_rgba8();
            match previous {
                Some(previous) => {
                    let before = filmcraft_render::render_item(&project, previous.item, previous.media_time, scale, &provider)
                        .ok_or("The previous frame could not be rendered")?;
                    if before.w != image.w || before.h != image.h {
                        return Err("Frame dimensions differ between the two frames".into());
                    }
                    let a = features::luma(before.w, before.h, &before.to_rgba8())?;
                    let b = features::luma(image.w, image.h, &rgba)?;
                    Ok((image.w, image.h, camera2d::difference_image(&a, &b, image.w, image.h, transform)?))
                }
                None => Ok((image.w, image.h, rgba)),
            }
        }))
        .unwrap_or_else(|_| Err("The display renderer failed".into()));
        *worker.result.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(outcome);
        repaint.request_repaint();
    });
    if spawned.is_ok() {
        ctx.data_mut(|d| d.insert_temp(display_job_id(), job));
    }
}
pub(super) fn command(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str, params: &Value) -> Result<Value, String> {
    if id == "reverseEngineering.cancelFeatures" {
        if let Some(job) = ctx.data(|d| d.get_temp::<Job>(job_id())) {
            job.cancel.store(true, Ordering::Relaxed);
        }
        return Ok(json!({"cancelled":true}));
    }
    if id == "reverseEngineering.setFeatureView" {
        let view = params.get("view").and_then(Value::as_str).ok_or("Supply view: points, matches or difference")?;
        if !matches!(view, "points" | "matches" | "difference") {
            return Err("View must be points, matches or difference".into());
        }
        app.ui.panels.reverse_engineering.camera2d_view = view.into();
        ctx.request_repaint();
        return Ok(json!({"view":view}));
    }
    if id == "reverseEngineering.featureFrame" {
        prepare(app)?;
        let index = params.get("frameIndex").and_then(Value::as_u64).and_then(|v| usize::try_from(v).ok()).ok_or("Supply frameIndex")?;
        let capture = app.ui.panels.reverse_engineering.camera2d_captures.get(index).cloned().ok_or("Frame outside the range")?;
        app.ui.panels.reverse_engineering.camera2d_frame = index;
        app.ui.panels.reverse_engineering.capture = Some(capture.clone());
        app.stop();
        app.session.set_playhead(capture.timeline_time);
        reverse_engineering::reset_decode_timer(ctx);
        return Ok(json!({"frameIndex":index}));
    }
    if is_running(ctx) {
        return Err("Wait for the 2D camera analysis to finish or cancel it first".into());
    }
    if id == "reverseEngineering.setFeatureOptions" {
        let s = &app.ui.panels.reverse_engineering;
        let number = |key: &str, current: f32| -> Result<f32, String> {
            match params.get(key) {
                Some(v) => v.as_f64().map(|v| v as f32).filter(|v| v.is_finite()).ok_or_else(|| format!("{key} must be a finite number")),
                None => Ok(current),
            }
        };
        let wide = |key: &str, current: f64| -> Result<f64, String> {
            match params.get(key) {
                Some(v) => v.as_f64().filter(|v| v.is_finite()).ok_or_else(|| format!("{key} must be a finite number")),
                None => Ok(current),
            }
        };
        let integer = |key: &str, current: usize| -> Result<usize, String> {
            match params.get(key) {
                Some(v) => v.as_u64().and_then(|v| usize::try_from(v).ok()).ok_or_else(|| format!("{key} must be a non-negative integer")),
                None => Ok(current),
            }
        };
        let options = Options {
            max_points: integer("maxPoints", s.camera2d_options.max_points)?,
            min_distance: number("minDistance", s.camera2d_options.min_distance)?,
            threshold: number("threshold", s.camera2d_options.threshold)?,
            octaves: integer("octaves", s.camera2d_options.octaves)?,
        };
        options.validate()?;
        let match_options = MatchOptions {
            ratio: number("ratio", s.camera2d_match.ratio)?,
            cross_check: match params.get("crossCheck") {
                Some(v) => v.as_bool().ok_or("crossCheck must be a boolean")?,
                None => s.camera2d_match.cross_check,
            },
            max_distance_px: number("maxDistancePx", s.camera2d_match.max_distance_px)?,
        };
        match_options.validate()?;
        let camera_options = CameraOptions {
            fit: FitOptions {
                tolerance_px: wide("tolerancePx", s.camera2d_camera.fit.tolerance_px)?,
                min_inliers: integer("minInliers", s.camera2d_camera.fit.min_inliers)?,
                min_support: wide("minSupport", s.camera2d_camera.fit.min_support)?,
            },
            min_coverage_cells: integer("minCoverageCells", s.camera2d_camera.min_coverage_cells)?,
            min_span: wide("minSpan", s.camera2d_camera.min_span)?,
            max_regions: integer("maxRegions", s.camera2d_camera.max_regions)?,
            static_px: wide("staticPx", s.camera2d_camera.static_px)?,
        };
        camera_options.validate()?;
        let anchor_options = AnchorOptions {
            gap: integer("anchorGap", s.camera2d_anchors.gap)?,
            step: integer("anchorStep", s.camera2d_anchors.step)?,
            long_gap: integer("anchorLongGap", s.camera2d_anchors.long_gap)?,
        };
        anchor_options.validate()?;
        let width = integer("analysisWidth", s.camera2d_width)?;
        if !(MIN_WIDTH..=MAX_WIDTH).contains(&width) {
            return Err("analysisWidth must be 160–4096".into());
        }
        let state = &mut app.ui.panels.reverse_engineering;
        state.camera2d_options = options;
        state.camera2d_match = match_options;
        state.camera2d_camera = camera_options;
        state.camera2d_anchors = anchor_options;
        state.camera2d_width = width;
        return Ok(
            json!({"options":options,"matchOptions":match_options,"cameraOptions":camera_options,"anchorOptions":anchor_options,"analysisWidth":width,"appliesTo":"nextDetection"}),
        );
    }
    if id == "reverseEngineering.runFeatures" {
        return start(app, ctx);
    }
    if id == "reverseEngineering.exportCamera" {
        return export_camera(app, params);
    }
    if id == "reverseEngineering.applyCamera" {
        return apply_camera(app, params);
    }
    if id == "reverseEngineering.exportComponent" {
        return export_component(app, params);
    }
    if id == "reverseEngineering.copyDescription" {
        return copy_description(app, ctx, params);
    }
    Err(format!("Unknown 2D camera command: {id}"))
}

/// The complete description of the camera (the camera file: measured path in sequence pixels,
/// Motion keyframes, beats with their numbers and the rules behind the labels) for the current
/// analysis or for an applied component (`{"id"}`), copied to the clipboard as JSON and
/// returned. FilmCraft writes no prompt: whoever reads this decides how to say it.
fn copy_description(app: &mut FilmcraftApp, ctx: &egui::Context, params: &Value) -> Result<Value, String> {
    let (file, component) = match params.get("id").and_then(Value::as_u64) {
        Some(id) => {
            let sequence = app.session.active_sequence().ok_or("Open a sequence")?;
            let component = sequence.components.iter().find(|c| c.id.0 == id).ok_or("No such component in the active sequence")?.clone();
            (camera_file_from_component(&component)?, Some(component))
        }
        None => {
            if !current(app) {
                return Err("Analyse the range first; the result must be current".into());
            }
            let report = app.ui.panels.reverse_engineering.camera2d_report.clone().ok_or("No 2D camera result")?;
            (camera_file(app, &report)?, None)
        }
    };
    let mut description = serde_json::to_value(&file).map_err(|e| e.to_string())?;
    if let (Some(object), Some(component)) = (description.as_object_mut(), component) {
        object.insert("component".into(), serde_json::to_value(&component).map_err(|e| e.to_string())?);
    }
    let text = serde_json::to_string_pretty(&description).map_err(|e| e.to_string())?;
    ctx.copy_text(text);
    app.ui.status = format!("Camera description copied ({} frames)", file.path.len());
    Ok(description)
}

/// A camera file rebuilt from an applied component's record: the samples hold centre
/// displacement, scale and rotation, from which the similarity about the source centre follows.
fn camera_file_from_component(c: &filmcraft_project::AppliedComponent) -> Result<CameraFile, String> {
    let (width, height) = (c.source.width.max(1), c.source.height.max(1));
    let centre = [f64::from(width) / 2.0, f64::from(height) / 2.0];
    let mut path = Vec::new();
    path.try_reserve_exact(c.samples.len()).map_err(|e| format!("Cannot allocate the camera path: {e}"))?;
    for (i, s) in c.samples.iter().enumerate() {
        let (sin, cos) = s.rotation.to_radians().sin_cos();
        let (a, b) = (s.scale * cos, s.scale * sin);
        let moved = [centre[0] + s.dx, centre[1] + s.dy];
        let transform = [a, b, moved[0] - (a * centre[0] - b * centre[1]), moved[1] - (b * centre[0] + a * centre[1])];
        path.push(CameraFrame {
            frame: i,
            tick: s.tick,
            seconds: s.tick as f64 / filmcraft_time::TICKS_PER_SECOND as f64,
            transform,
            displacement_px: [s.dx, s.dy],
            scale: s.scale,
            rotation_degrees: s.rotation,
            residual_px: s.residual,
            state: match s.state.as_deref() {
                Some("static") => Some(CameraState::Static),
                Some("moving") => Some(CameraState::Moving),
                Some("unverified") => Some(CameraState::Unverified),
                Some("noEvidence") => Some(CameraState::NoEvidence),
                _ => None,
            },
        });
    }
    let base = motion::base_matrix(centre, 100.0, 0.0, centre, 1.0)?;
    let poses: Vec<[f64; 4]> = path.iter().map(|f| f.transform).collect();
    let keyframes = motion::keyframes(&poses, base, centre, 1.0, motion::Mode::Follow)?;
    Ok(CameraFile {
        format: CAMERA_FORMAT.into(),
        version: 1,
        source: CameraSource {
            sequence: c.source.sequence.clone(),
            width,
            height,
            frame_rate: c.source.frame_rate,
            start_frame: c.source.start_frame,
            end_frame: c.source.end_frame,
            analysis_width: 0,
            analyzer: format!("component:{}", c.name),
            completed_at_unix_ms: 0,
        },
        cinematography: cinematography_of(&path, width, height, c.source.frame_rate),
        path,
        motion: keyframes,
    })
}
fn export_component(app: &mut FilmcraftApp, params: &Value) -> Result<Value, String> {
    let id = params.get("id").and_then(Value::as_u64).ok_or("Supply the component id")?;
    let sequence = app.session.active_sequence().ok_or("Open a sequence")?;
    let component = sequence.components.iter().find(|c| c.id.0 == id).ok_or("No such component in the active sequence")?.clone();
    if component.samples.is_empty() {
        return Err("The component carries no path samples to export".into());
    }
    let file = camera_file_from_component(&component)?;
    let suggested = format!("{}.camera2d.json", component.name.replace(['/', '\\', ':'], "-"));
    let path = if let Some(path) = params.get("path") {
        Some(path.as_str().filter(|s| !s.trim().is_empty()).ok_or("Supply a nonempty export path")?.to_owned())
    } else {
        app.hooks.pick_save_as.as_mut().ok_or("No save dialog; supply path")?("2D camera", &["json"], &suggested)
    };
    let Some(path) = path else {
        return Ok(Value::Null);
    };
    let bytes = serde_json::to_vec_pretty(&file).map_err(|e| e.to_string())?;
    std::fs::write(&path, bytes).map_err(|e| format!("Could not export the camera: {e}"))?;
    app.ui.status = format!("Exported component {} ({} frames) to {path}", component.name, file.path.len());
    Ok(json!({"path":path,"frames":file.path.len(),"id":id}))
}
/// The components applied in the active sequence, with go / remove / export.
fn applied_components(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    let Some(sequence) = app.session.active_sequence().cloned() else { return };
    if sequence.components.is_empty() {
        return;
    }
    ui.separator();
    ui.label(format!("Applied components in this sequence ({}):", sequence.components.len()));
    let focused = app.ui.panels.reverse_engineering.focused_component;
    let rate = sequence.settings.frame_rate;
    for c in &sequence.components {
        let is_focused = focused == Some(c.id.0);
        ui.horizontal_wrapped(|ui| {
            let frames = rate.frame_at(c.end() - filmcraft_time::Tick(1)).saturating_sub(rate.frame_at(c.start)).saturating_add(1);
            let (lo, hi) = c.scale_range();
            let label = format!(
                "{} · {} · clip {} · {:.2} s ({frames} frames) · max {:.1} px · zoom {:.3}–{:.3} · {} changes",
                c.name,
                c.mode.label(),
                c.clip.0,
                c.duration.seconds(),
                c.max_displacement(),
                lo,
                hi,
                c.changes().len()
            );
            let response = ui.selectable_label(is_focused, &label);
            app.auto.add(&format!("reverseEngineering.camera2d.component.{}", c.id.0), response.rect, &label);
            if response.clicked() {
                app.ui.panels.reverse_engineering.focused_component = Some(c.id.0);
                if let Err(e) = app.session.execute("timeline.select", json!({"clips":[c.clip.0]})) {
                    app.ui.status = e.to_string();
                }
                if let Err(e) = app.session.execute("playhead.set", json!({"time":c.start.0})) {
                    app.ui.status = e.to_string();
                }
            }
            let response = ui.button("Remove");
            app.auto.add(
                &format!("reverseEngineering.camera2d.component.{}.remove", c.id.0),
                response.rect,
                "Remove this component and the keyframes it wrote",
            );
            if response.clicked() {
                match app.session.execute("components.remove", json!({"id":c.id.0})) {
                    Ok(_) => app.ui.status = format!("Removed component {}", c.name),
                    Err(e) => app.ui.status = e.to_string(),
                }
            }
            let response = ui.button("Export…");
            app.auto.add(&format!("reverseEngineering.camera2d.component.{}.export", c.id.0), response.rect, "Export this component as a camera file");
            if response.clicked() {
                invoke(app, ui.ctx(), "reverseEngineering.exportComponent", json!({"id":c.id.0}));
            }
        });
    }
}

/// Step E: the solved path as a reusable camera component. `path[]` holds one pose per frame in
/// sequence pixels (frame 0 = identity), `motion[]` the equivalent Motion keyframes for a clip
/// centred at 100 % in the source sequence; `tick` is the exact offset from the first frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraFrame {
    pub frame: usize,
    pub tick: i64,
    pub seconds: f64,
    pub transform: [f64; 4],
    pub displacement_px: [f64; 2],
    pub scale: f64,
    pub rotation_degrees: f64,
    pub residual_px: f64,
    pub state: Option<CameraState>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraSource {
    pub sequence: String,
    pub width: u32,
    pub height: u32,
    pub frame_rate: f64,
    pub start_frame: i64,
    pub end_frame: i64,
    pub analysis_width: usize,
    pub analyzer: String,
    pub completed_at_unix_ms: u64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraFile {
    pub format: String,
    pub version: u32,
    pub source: CameraSource,
    pub path: Vec<CameraFrame>,
    pub motion: Vec<motion::MotionKeyframe>,
    /// The camera in cinema terms and as a prompt, derived from `path` and the evidence states.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cinematography: Option<cinema::Cinematography>,
}
pub const CAMERA_FORMAT: &str = "filmcraft.camera2d";

/// The cinematography of a camera path given in sequence pixels (`path` as the file stores it).
fn cinematography_of(path: &[CameraFrame], width: u32, height: u32, fps: f64) -> Option<cinema::Cinematography> {
    if path.len() < 2 {
        return None;
    }
    let states: Vec<CameraState> = path.iter().skip(1).map(|f| f.state.unwrap_or(CameraState::Unverified)).collect();
    let trajectory: Vec<FramePose> = path
        .iter()
        .map(|f| FramePose {
            frame: f.frame,
            transform: f.transform,
            displacement_px: f.displacement_px,
            scale: f.scale,
            rotation_degrees: f.rotation_degrees,
            residual_px: f.residual_px,
        })
        .collect();
    cinema::cinematography(&states, &trajectory, cinema::Frame { width, height, fps }, 1.0).ok()
}
const MAX_CAMERA_FILE_BYTES: u64 = 256 * 1024 * 1024;

/// The current report as a camera component, with the path in sequence pixels.
fn camera_file(app: &FilmcraftApp, report: &FeatureReport) -> Result<CameraFile, String> {
    let first = report.captures.first().ok_or("The analysis has no frames")?;
    let [width, height] = report.source_size;
    let k = (report.analysis_width as f64 / f64::from(width.max(1))).min(1.0);
    let rate = app.session.active_sequence().map(|q| q.settings.frame_rate).ok_or("No active sequence")?;
    let state = &app.ui.panels.reverse_engineering;
    let mut path = Vec::new();
    path.try_reserve_exact(report.trajectory.len()).map_err(|e| format!("Cannot allocate the camera path: {e}"))?;
    for (i, pose) in report.trajectory.iter().enumerate() {
        let capture = report.captures.get(i).ok_or("Trajectory longer than the captured range")?;
        let tick = capture.timeline_time.0.checked_sub(first.timeline_time.0).ok_or("Capture times overflow")?;
        path.push(CameraFrame {
            frame: i,
            tick,
            seconds: tick as f64 / filmcraft_time::TICKS_PER_SECOND as f64,
            transform: motion::to_sequence(pose.transform, k)?,
            displacement_px: [pose.displacement_px[0] / k, pose.displacement_px[1] / k],
            scale: pose.scale,
            rotation_degrees: pose.rotation_degrees,
            residual_px: pose.residual_px / k,
            state: i.checked_sub(1).and_then(|p| report.comparisons.get(p)).map(|c| c.state),
        });
    }
    let centre = [f64::from(width) / 2.0, f64::from(height) / 2.0];
    let base = motion::base_matrix(centre, 100.0, 0.0, centre, 1.0)?;
    let poses: Vec<[f64; 4]> = path.iter().map(|f| f.transform).collect();
    let keyframes = motion::keyframes(&poses, base, centre, 1.0, motion::Mode::Follow)?;
    Ok(CameraFile {
        format: CAMERA_FORMAT.into(),
        version: 1,
        source: CameraSource {
            sequence: first.name.clone(),
            width,
            height,
            frame_rate: rate.as_f64(),
            start_frame: state.start_frame,
            end_frame: state.end_frame,
            analysis_width: report.analysis_width,
            analyzer: report.analyzer.clone(),
            completed_at_unix_ms: report.completed_at_unix_ms,
        },
        cinematography: cinematography_of(&path, width, height, rate.as_f64()),
        path,
        motion: keyframes,
    })
}
fn load_camera_file(path: &str) -> Result<CameraFile, String> {
    let size = std::fs::metadata(path).map_err(|e| format!("Cannot read the camera file: {e}"))?.len();
    if size > MAX_CAMERA_FILE_BYTES {
        return Err("The camera file is larger than 256 MB".into());
    }
    let bytes = std::fs::read(path).map_err(|e| format!("Cannot read the camera file: {e}"))?;
    let file: CameraFile = serde_json::from_slice(&bytes).map_err(|e| format!("Not a camera file: {e}"))?;
    if file.format != CAMERA_FORMAT || file.version != 1 {
        return Err(format!("Expected a {CAMERA_FORMAT} version 1 file"));
    }
    if file.path.is_empty() || file.path.len() > motion::MAX_KEYFRAMES {
        return Err(format!("A camera file holds 1–{} frames", motion::MAX_KEYFRAMES));
    }
    let mut previous = -1i64;
    for frame in &file.path {
        if !frame.transform.iter().all(|v| v.is_finite()) || frame.transform[0].hypot(frame.transform[1]) < 1e-9 {
            return Err(format!("Frame {} has an invalid transform", frame.frame));
        }
        if frame.tick <= previous {
            return Err("Frame ticks must increase".into());
        }
        previous = frame.tick;
    }
    Ok(file)
}
fn export_camera(app: &mut FilmcraftApp, params: &Value) -> Result<Value, String> {
    if !current(app) {
        return Err("Analyse the range first; the result must be current".into());
    }
    let report = app.ui.panels.reverse_engineering.camera2d_report.clone().ok_or("No 2D camera result")?;
    let file = camera_file(app, &report)?;
    let path = if let Some(path) = params.get("path") {
        Some(path.as_str().filter(|s| !s.trim().is_empty()).ok_or("Supply a nonempty export path")?.to_owned())
    } else {
        app.hooks.pick_save_as.as_mut().ok_or("No save dialog; supply path")?("2D camera", &["json"], "camera.camera2d.json")
    };
    let Some(path) = path else {
        return Ok(Value::Null);
    };
    let bytes = serde_json::to_vec_pretty(&file).map_err(|e| e.to_string())?;
    std::fs::write(&path, bytes).map_err(|e| format!("Could not export the camera: {e}"))?;
    app.ui.status = format!("Exported camera ({} frames) to {path}", file.path.len());
    Ok(json!({"path":path,"frames":file.path.len()}))
}
fn selected_video_clips(app: &FilmcraftApp) -> Result<Vec<filmcraft_project::ClipId>, String> {
    let sequence = app.session.active_sequence().ok_or("Open a sequence")?;
    let clips: Vec<_> = app
        .session
        .state
        .selection
        .iter()
        .copied()
        .filter(|id| sequence.find_item(*id).and_then(|(track, _)| sequence.track(track)).is_some_and(|t| t.kind == filmcraft_project::TrackKind::Video))
        .collect();
    if clips.is_empty() {
        return Err("Select the video clips to apply the camera to".into());
    }
    Ok(clips)
}
/// Apply a camera path (the current result or a camera file) to clips as Motion keyframes:
/// `{"clips":[id]?,"mode":"follow"|"stabilize","path":file?,"file":true? (open dialog),
/// "start":ticks? (where frame 0 lands; default the clip start, or the analysed range when
/// "atTimeline":true)}`. Each clip is one undo step.
fn apply_camera(app: &mut FilmcraftApp, params: &Value) -> Result<Value, String> {
    let mode = match params.get("mode").and_then(Value::as_str).unwrap_or("follow") {
        "follow" => motion::Mode::Follow,
        "stabilize" => motion::Mode::Stabilize,
        _ => return Err("mode must be follow or stabilize".into()),
    };
    let file_path = match params.get("path") {
        Some(path) => Some(path.as_str().filter(|s| !s.trim().is_empty()).ok_or("Supply a nonempty camera file path")?.to_owned()),
        None if params.get("file").and_then(Value::as_bool) == Some(true) => {
            match app.hooks.pick_open_file.as_mut().ok_or("No open dialog; supply path")?("2D camera", &["json"]) {
                Some(path) => Some(path),
                None => return Ok(Value::Null),
            }
        }
        None => None,
    };
    let (file, range_start, analysed_sequence, name): (CameraFile, Option<filmcraft_time::Tick>, Option<filmcraft_project::ItemId>, String) = match &file_path {
        Some(path) => {
            let file = load_camera_file(path)?;
            let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.clone());
            (file, None, None, name)
        }
        None => {
            if !current(app) {
                return Err("Analyse the range first; the result must be current (or apply a camera file)".into());
            }
            let report = app.ui.panels.reverse_engineering.camera2d_report.clone().ok_or("No 2D camera result")?;
            let file = camera_file(app, &report)?;
            let first = report.captures.first().map(|c| (c.timeline_time, c.item));
            let name = format!("2D camera · {} {}–{}", file.source.sequence, file.source.start_frame, file.source.end_frame);
            (file, first.map(|f| f.0), first.map(|f| f.1), name)
        }
    };
    let poses: Vec<[f64; 4]> = file.path.iter().map(|f| f.transform).collect();
    let ticks: Vec<i64> = file.path.iter().map(|f| f.tick).collect();
    let clips = match params.get("clips").and_then(Value::as_array) {
        Some(list) => list.iter().map(|v| v.as_u64().map(filmcraft_project::ClipId).ok_or("clips must be ids")).collect::<Result<Vec<_>, _>>()?,
        None => selected_video_clips(app)?,
    };
    let explicit_start = params.get("start").and_then(Value::as_i64).map(filmcraft_time::Tick);
    let at_timeline = params.get("atTimeline").and_then(Value::as_bool).unwrap_or(false);
    if at_timeline && range_start.is_none() {
        return Err("atTimeline needs the current analysis, not a camera file".into());
    }
    let sequence = app.session.active_sequence().ok_or("Open a sequence")?.clone();
    let (sw, sh) = (f64::from(sequence.settings.width), f64::from(sequence.settings.height));
    let seq_par = filmcraft_project::sane_par(sequence.settings.par);
    let mut applied = Vec::new();
    for clip in &clips {
        let (track, item) = sequence.find_item(*clip).ok_or_else(|| format!("Clip {} is not in the active sequence", clip.0))?;
        if sequence.track(track).is_none_or(|t| t.kind != filmcraft_project::TrackKind::Video) {
            return Err(format!("Clip {} is not a video clip", clip.0));
        }
        let src = app.session.project.source_size(item.item).ok_or_else(|| format!("Clip {} has no picture size", clip.0))?;
        let src_par = app.session.project.source_par(item.item).map_or(seq_par, filmcraft_project::sane_par);
        if filmcraft_project::par_ratio(src_par) != filmcraft_project::par_ratio(seq_par) {
            return Err(format!("Clip {}: the clip and the sequence must share the pixel aspect ratio", clip.0));
        }
        let start = explicit_start.or(if at_timeline { range_start } else { None }).unwrap_or(item.start);
        let last = (item.end() - filmcraft_time::Tick(1)).max(item.start);
        let mut bases = Vec::new();
        bases.try_reserve_exact(poses.len()).map_err(|e| format!("Cannot allocate Motion bases: {e}"))?;
        for tick in &ticks {
            let time = filmcraft_time::Tick(start.0.saturating_add(*tick)).clamp(item.start, last);
            let m = filmcraft_render::motion_matrix(&sequence, item, src, app.session.project.source_par(item.item), item.effect_time_at(time));
            if (m.c + m.b).abs() > 1e-6 * (1.0 + m.a.abs()) || (m.d - m.a).abs() > 1e-6 * (1.0 + m.a.abs()) {
                return Err(format!("Clip {}: Motion with non-uniform scale is not a similarity; set Uniform Scale first", clip.0));
            }
            bases.push([m.a, m.b, m.e, m.f]);
        }
        let fit = if item.scale_to_frame { (sw / f64::from(src.0.max(1))).min(sh / f64::from(src.1.max(1))) } else { 1.0 };
        let anchor = item
            .effect("motion")
            .and_then(|m| m.param("anchor"))
            .and_then(|p| p.value.as_vec2())
            .filter(|a| a.x.is_finite() && a.y.is_finite())
            .map_or([f64::from(src.0) / 2.0, f64::from(src.1) / 2.0], |a| [a.x, a.y]);
        let keyframes = motion::keyframes_over(&poses, &bases, anchor, fit, mode)?;
        let mut position = Vec::new();
        let mut scale = Vec::new();
        let mut rotation = Vec::new();
        let mut samples = Vec::new();
        let mut first_time: Option<i64> = None;
        let mut last_time: i64 = 0;
        for ((k, tick), frame) in keyframes.iter().zip(&ticks).zip(&file.path) {
            let time = start.0.checked_add(*tick).ok_or("Keyframe time overflows")?;
            if time < item.start.0 || time >= item.end().0 {
                continue;
            }
            let first = *first_time.get_or_insert(time);
            last_time = time;
            position.push(json!({"time":time,"value":[k.position[0],k.position[1]]}));
            scale.push(json!({"time":time,"value":k.scale_percent}));
            rotation.push(json!({"time":time,"value":k.rotation_degrees}));
            let centre = [f64::from(file.source.width.max(1)) / 2.0, f64::from(file.source.height.max(1)) / 2.0];
            let moved = camera2d::apply(frame.transform, centre);
            samples.push(json!({
                "tick":time - first,
                "dx":moved[0] - centre[0],
                "dy":moved[1] - centre[1],
                "scale":frame.transform[0].hypot(frame.transform[1]),
                "rotation":frame.transform[1].atan2(frame.transform[0]).to_degrees(),
                "residual":frame.residual_px,
                "state":frame.state,
            }));
        }
        let Some(first_time) = first_time else {
            return Err(format!("Clip {}: no frame of the camera path falls inside the clip", clip.0));
        };
        let written = position.len();
        let duration = last_time
            .saturating_sub(first_time)
            .saturating_add(sequence.settings.frame_rate.frame_duration().0)
            .min(item.end().0.saturating_sub(first_time))
            .max(1);
        let result = app
            .session
            .execute(
                "components.add",
                json!({
                    "clip":clip.0,
                    "kind":"camera2d",
                    "name":name,
                    "mode":match mode { motion::Mode::Follow => "follow", motion::Mode::Stabilize => "stabilize" },
                    "start":first_time,
                    "duration":duration,
                    "effect":"motion",
                    "keyframes":{"position":position,"scale":scale,"rotation":rotation},
                    "set":{"uniform_scale":true},
                    "source":{"sequence":file.source.sequence,"width":file.source.width,"height":file.source.height,"frameRate":file.source.frame_rate,
                        "startFrame":file.source.start_frame,"endFrame":file.source.end_frame,"file":file_path},
                    "samples":samples,
                    "summary":file.cinematography.as_ref().map(|c| c.summary.clone()).unwrap_or_default(),
                }),
            )
            .map_err(|e| e.to_string())?;
        app.ui.panels.reverse_engineering.focused_component = result.get("id").and_then(Value::as_u64);
        applied.push(json!({"clip":clip.0,"keyframes":written,"start":first_time,"component":result.get("id").cloned().unwrap_or(Value::Null)}));
    }
    if analysed_sequence.is_some_and(|id| app.session.state.active_sequence != Some(id))
        && let Some(report) = app.ui.panels.reverse_engineering.camera2d_report.as_mut()
    {
        report.project_revision = app.session.revision;
    }
    app.ui.status = format!(
        "{} camera applied to {} clip{} ({} frames)",
        match mode {
            motion::Mode::Follow => "Follow",
            motion::Mode::Stabilize => "Stabilizing",
        },
        applied.len(),
        if applied.len() == 1 { "" } else { "s" },
        poses.len()
    );
    Ok(json!({"mode":mode,"clips":applied,"frames":poses.len()}))
}
fn invoke(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str, params: Value) {
    if let Err(e) = crate::menus::invoke(app, ctx, id, params) {
        app.ui.status = e.clone();
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("reverse-tools-error"), e));
    } else {
        ctx.data_mut(|d| d.remove::<String>(egui::Id::new("reverse-tools-error")));
    }
}
fn option_value(v: Option<f32>) -> String {
    v.map(|v| format!("{v:.4}")).unwrap_or_else(|| "n/a".into())
}
fn state_color(state: CameraState) -> egui::Color32 {
    match state {
        CameraState::Static => egui::Color32::from_rgb(120, 170, 220),
        CameraState::Moving => egui::Color32::from_rgb(90, 200, 110),
        CameraState::Unverified => egui::Color32::from_rgb(200, 160, 60),
        CameraState::NoEvidence => egui::Color32::from_rgb(160, 50, 50),
    }
}
/// The 18 analysis parameters, under *Advanced*: changing any of them marks the result stale.
fn advanced_options(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    let mut options = app.ui.panels.reverse_engineering.camera2d_options;
    let mut match_options = app.ui.panels.reverse_engineering.camera2d_match;
    let mut camera = app.ui.panels.reverse_engineering.camera2d_camera;
    let mut anchors = app.ui.panels.reverse_engineering.camera2d_anchors;
    let mut width = app.ui.panels.reverse_engineering.camera2d_width;
    let mut changed = false;
    ui.label("Points");
    ui.horizontal_wrapped(|ui| {
        ui.label("Max");
        let r = ui.add(egui::DragValue::new(&mut options.max_points).range(1..=features::MAX_POINTS));
        app.auto.add("reverseEngineering.camera2d.maxPoints", r.rect, "Maximum number of points kept per frame");
        changed |= r.changed();
        ui.label("Min distance");
        let r = ui.add(egui::DragValue::new(&mut options.min_distance).range(0.0..=256.0).speed(0.5).suffix(" px"));
        app.auto.add("reverseEngineering.camera2d.minDistance", r.rect, "Minimum spacing between kept points, analysis pixels");
        changed |= r.changed();
        ui.label("Threshold");
        let r = ui.add(egui::DragValue::new(&mut options.threshold).range(0.0001..=1.0).speed(0.0005).fixed_decimals(4));
        app.auto.add("reverseEngineering.camera2d.threshold", r.rect, "Minimum response relative to the strongest point of the frame");
        changed |= r.changed();
        ui.label("Octaves");
        let r = ui.add(egui::DragValue::new(&mut options.octaves).range(1..=6));
        app.auto.add("reverseEngineering.camera2d.octaves", r.rect, "Scale levels searched");
        changed |= r.changed();
        ui.label("Width");
        let r = ui.add(egui::DragValue::new(&mut width).range(MIN_WIDTH..=MAX_WIDTH).speed(16));
        app.auto.add("reverseEngineering.camera2d.analysisWidth", r.rect, "Width of the rendered frames handed to the detector");
        changed |= r.changed();
    });
    ui.label("Matches");
    ui.horizontal_wrapped(|ui| {
        ui.label("Ratio");
        let r = ui.add(egui::DragValue::new(&mut match_options.ratio).range(0.5..=0.95).speed(0.01).fixed_decimals(2));
        app.auto.add("reverseEngineering.camera2d.ratio", r.rect, "Best descriptor distance must be below this fraction of the second best");
        changed |= r.changed();
        let r = ui.checkbox(&mut match_options.cross_check, "Cross-check");
        app.auto.add("reverseEngineering.camera2d.crossCheck", r.rect, "Keep a match only when it is mutual");
        changed |= r.changed();
        ui.label("Window");
        let r = ui.add(egui::DragValue::new(&mut match_options.max_distance_px).range(0.0..=4096.0).speed(1.0).suffix(" px"));
        app.auto.add("reverseEngineering.camera2d.maxDistancePx", r.rect, "Spatial search window; 0 = unlimited");
        changed |= r.changed();
    });
    ui.label("Camera");
    ui.horizontal_wrapped(|ui| {
        ui.label("Tolerance");
        let r = ui.add(egui::DragValue::new(&mut camera.fit.tolerance_px).range(0.01..=100.0).speed(0.05).suffix(" px"));
        app.auto.add("reverseEngineering.camera2d.tolerancePx", r.rect, "Maximum residual of a supporting match");
        changed |= r.changed();
        ui.label("Min inliers");
        let r = ui.add(egui::DragValue::new(&mut camera.fit.min_inliers).range(3..=camera2d::MAX_PAIRS));
        app.auto.add("reverseEngineering.camera2d.minInliers", r.rect, "Supporting matches needed for an estimate");
        changed |= r.changed();
        ui.label("Min support");
        let r = ui.add(egui::DragValue::new(&mut camera.fit.min_support).range(0.0..=1.0).speed(0.01).fixed_decimals(2));
        app.auto.add("reverseEngineering.camera2d.minSupport", r.rect, "Fraction of matches that must support the estimate");
        changed |= r.changed();
        ui.label("Coverage");
        let r = ui.add(egui::DragValue::new(&mut camera.min_coverage_cells).range(1..=camera2d::GRID * camera2d::GRID).suffix(" cells"));
        app.auto.add("reverseEngineering.camera2d.minCoverageCells", r.rect, "Grid cells (of 16) the camera support must occupy");
        changed |= r.changed();
        ui.label("Span");
        let r = ui.add(egui::DragValue::new(&mut camera.min_span).range(0.0..=1.0).speed(0.01).fixed_decimals(2));
        app.auto.add("reverseEngineering.camera2d.minSpan", r.rect, "Fraction of width and height the camera support must span");
        changed |= r.changed();
        ui.label("Static ≤");
        let r = ui.add(egui::DragValue::new(&mut camera.static_px).range(0.0..=100.0).speed(0.01).suffix(" px"));
        app.auto.add("reverseEngineering.camera2d.staticPx", r.rect, "Centre displacement at or below this counts as static");
        changed |= r.changed();
        ui.label("Regions");
        let r = ui.add(egui::DragValue::new(&mut camera.max_regions).range(0..=16));
        app.auto.add("reverseEngineering.camera2d.maxRegions", r.rect, "Moving regions separated per comparison");
        changed |= r.changed();
    });
    ui.label("Anchors");
    ui.horizontal_wrapped(|ui| {
        ui.label("Gap");
        let r = ui.add(egui::DragValue::new(&mut anchors.gap).range(2..=64).suffix(" frames"));
        app.auto.add("reverseEngineering.camera2d.anchorGap", r.rect, "Distance between an anchor frame and the frame it is compared with");
        changed |= r.changed();
        ui.label("Every");
        let r = ui.add(egui::DragValue::new(&mut anchors.step).range(1..=64).suffix(" frames"));
        app.auto.add("reverseEngineering.camera2d.anchorStep", r.rect, "How often an anchor comparison is made");
        changed |= r.changed();
        ui.label("Long gap");
        let r = ui.add(egui::DragValue::new(&mut anchors.long_gap).range(0..=256).suffix(" frames"));
        app.auto.add("reverseEngineering.camera2d.anchorLongGap", r.rect, "Second anchor distance to bridge short gaps without evidence; 0 = off");
        changed |= r.changed();
    });
    if changed {
        invoke(
            app,
            ui.ctx(),
            "reverseEngineering.setFeatureOptions",
            json!({"maxPoints":options.max_points,"minDistance":options.min_distance,"threshold":options.threshold,"octaves":options.octaves,"analysisWidth":width,
                "ratio":match_options.ratio,"crossCheck":match_options.cross_check,"maxDistancePx":match_options.max_distance_px,
                "tolerancePx":camera.fit.tolerance_px,"minInliers":camera.fit.min_inliers,"minSupport":camera.fit.min_support,
                "minCoverageCells":camera.min_coverage_cells,"minSpan":camera.min_span,"maxRegions":camera.max_regions,"staticPx":camera.static_px,
                "anchorGap":anchors.gap,"anchorStep":anchors.step,"anchorLongGap":anchors.long_gap}),
        );
    }
    let response = ui.button("Reset to defaults");
    app.auto.add("reverseEngineering.camera2d.resetOptions", response.rect, "Put every analysis parameter back to its default");
    if response.clicked() {
        let (o, m, c, a) = (Options::default(), MatchOptions::default(), CameraOptions::default(), AnchorOptions::default());
        invoke(
            app,
            ui.ctx(),
            "reverseEngineering.setFeatureOptions",
            json!({"maxPoints":o.max_points,"minDistance":o.min_distance,"threshold":o.threshold,"octaves":o.octaves,"analysisWidth":640,
                "ratio":m.ratio,"crossCheck":m.cross_check,"maxDistancePx":m.max_distance_px,
                "tolerancePx":c.fit.tolerance_px,"minInliers":c.fit.min_inliers,"minSupport":c.fit.min_support,
                "minCoverageCells":c.min_coverage_cells,"minSpan":c.min_span,"maxRegions":c.max_regions,"staticPx":c.static_px,
                "anchorGap":a.gap,"anchorStep":a.step,"anchorLongGap":a.long_gap}),
        );
    }
}

/// What the analysis found, in words: the phases of the camera path and how trustworthy they are.
#[derive(Clone, Debug, PartialEq)]
pub struct Verdict {
    pub headline: String,
    pub quality: String,
    /// A short note on the structure behind the headline ("3 beats"); empty without a path.
    pub prompt: String,
}

fn seconds_of(frame: usize, rate: filmcraft_time::FrameRate) -> f64 {
    (rate.tick_of(frame as i64) - rate.tick_of(0)).seconds()
}

/// Build the verdict from the per-comparison states and the solved path. `px_scale` converts
/// analysis pixels to source pixels.
pub fn verdict(states: &[CameraState], trajectory: &[FramePose], rate: filmcraft_time::FrameRate, px_scale: f64, size: [u32; 2]) -> Verdict {
    if states.is_empty() || trajectory.len() != states.len() + 1 {
        return Verdict { headline: "Not analysed yet".into(), quality: String::new(), prompt: String::new() };
    }
    let n = states.len();
    let end_s = seconds_of(n, rate);
    let frame = cinema::Frame { width: size[0], height: size[1], fps: rate.as_f64() };
    let (headline, prompt) = match cinema::cinematography(states, trajectory, frame, px_scale) {
        Ok(c) => (c.summary, format!("{} beats", c.beats.len())),
        Err(e) => (format!("Camera path could not be described: {e}"), String::new()),
    };
    let solid = states.iter().filter(|s| matches!(s, CameraState::Static | CameraState::Moving)).count();
    let gaps = states.iter().filter(|s| **s == CameraState::NoEvidence).count();
    let doubtful: Vec<usize> = trajectory.iter().enumerate().filter(|(_, p)| p.residual_px > 2.0).map(|(i, _)| i).collect();
    let mut quality = format!("{:.0} % of frames with solid evidence", 100.0 * solid as f64 / n as f64);
    if doubtful.is_empty() {
        quality.push_str(" · no doubtful frames");
    } else {
        let times: Vec<String> = doubtful.iter().take(3).map(|i| format!("{:.1} s", seconds_of(*i, rate))).collect();
        quality.push_str(&format!(
            " · {} doubtful frame{} ({}{})",
            doubtful.len(),
            if doubtful.len() == 1 { "" } else { "s" },
            times.join(", "),
            if doubtful.len() > 3 { ", …" } else { "" }
        ));
    }
    if gaps > 0 {
        quality.push_str(&format!(" · {gaps} frame{} without evidence", if gaps == 1 { "" } else { "s" }));
    }
    quality.push_str(&format!(" · {end_s:.1} s analysed"));
    Verdict { headline, quality, prompt }
}

fn badge(ui: &mut egui::Ui, text: &str, fill: egui::Color32) -> egui::Response {
    egui::Frame::new()
        .fill(fill)
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).color(egui::Color32::WHITE).size(11.0));
        })
        .response
}

/// Header shared by the tool pages: back, title, status badge, context on the right.
fn header(app: &mut FilmcraftApp, ui: &mut egui::Ui, title: &str, status: (&str, egui::Color32), context: &str) {
    let busy = is_running(ui.ctx());
    ui.horizontal(|ui| {
        let response = ui.add_enabled(!busy, egui::Button::new("← Tools"));
        app.auto.add("reverseEngineering.tool.back", response.rect, "Back to tool menu");
        if response.clicked() {
            invoke(app, ui.ctx(), "reverseEngineering.chooseTool", json!({"tool":"menu"}));
        }
        ui.heading(title);
        let response = badge(ui, status.0, status.1);
        app.auto.add("reverseEngineering.status", response.rect, status.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(context).color(app.tokens.text_faint));
        });
    });
}

/// The 2D camera page: header, verdict, viewer with graphs, actions, Details and Advanced.
pub(super) fn page(app: &mut FilmcraftApp, ui: &mut egui::Ui, bounds: egui::Rect) {
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(bounds));
    child.set_clip_rect(bounds.intersect(ui.clip_rect()));
    child.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
    egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt("reverse-camera2d-page").show(&mut child, |ui| {
        ui.set_width(bounds.width().max(1.0));
        page_body(app, ui, bounds);
    });
}

fn page_body(app: &mut FilmcraftApp, ui: &mut egui::Ui, bounds: egui::Rect) {
    let running = ui.ctx().data(|d| d.get_temp::<Job>(job_id()));
    let prepared = prepare(app);
    let is_current = prepared.is_ok() && current(app);
    let saved =
        app.ui.panels.reverse_engineering.saved_results.iter().any(|r| r.tool() == "camera2d" && reverse_engineering::captures_are_current(app, r.captures()));
    let has_report = app.ui.panels.reverse_engineering.camera2d_report.is_some();
    let status = if running.is_some() {
        ("Analysing…", egui::Color32::from_rgb(90, 110, 170))
    } else if is_current && saved {
        ("Analysed · saved", egui::Color32::from_rgb(40, 130, 90))
    } else if is_current {
        ("Analysed", egui::Color32::from_rgb(40, 130, 90))
    } else if has_report {
        ("Stale · analyse again", egui::Color32::from_rgb(170, 120, 40))
    } else {
        ("Not analysed", egui::Color32::from_gray(90))
    };
    let rate = app.session.active_sequence().map(|q| q.settings.frame_rate).unwrap_or_default();
    let capture_count = app.ui.panels.reverse_engineering.camera2d_captures.len();
    let context = {
        let state = &app.ui.panels.reverse_engineering;
        match state.camera2d_captures.first() {
            Some(first) => format!("{} · frames {}–{} · {capture_count} frames · {:.3} fps", first.name, state.start_frame, state.end_frame, rate.as_f64()),
            None => String::new(),
        }
    };
    header(app, ui, "2D camera", status, &context);
    if let Some(job) = running {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(format!("Analysing · frame {} / {}", job.progress.load(Ordering::Relaxed), job.total));
            let response = ui.button("Cancel");
            app.auto.add("reverseEngineering.cancelFeatures", response.rect, "Cancel 2D camera analysis");
            if response.clicked() {
                invoke(app, ui.ctx(), "reverseEngineering.cancelFeatures", json!({}));
            }
        });
        return;
    }
    if let Err(e) = &prepared {
        ui.label(e);
    }
    if let Some(error) = ui.ctx().data(|d| d.get_temp::<String>(egui::Id::new("reverse-tools-error"))) {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
    }
    let report = app.ui.panels.reverse_engineering.camera2d_report.clone().filter(|_| is_current);
    let verdict = match &report {
        Some(r) => {
            let states: Vec<CameraState> = r.comparisons.iter().map(|c| c.state).collect();
            let px_scale = f64::from(r.source_size[0].max(1)) / r.analysis_width.max(1) as f64;
            verdict(&states, &r.trajectory, rate, px_scale, r.source_size)
        }
        None if has_report => Verdict {
            headline: "Options, range or project changed: analyse again to see the camera.".into(),
            quality: "The previous result is kept hidden until then.".into(),
            prompt: String::new(),
        },
        None => Verdict {
            headline: "Not analysed yet.".into(),
            quality: format!("Analyse renders the {capture_count} frames of the range, finds the camera motion and solves its path."),
            prompt: String::new(),
        },
    };
    egui::Frame::group(ui.style()).inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let response = ui.label(egui::RichText::new(&verdict.headline).strong().size(15.0));
        app.auto.add("reverseEngineering.verdict", response.rect, &verdict.headline);
        let response = ui.label(egui::RichText::new(&verdict.quality).color(app.tokens.text_dim));
        app.auto.add("reverseEngineering.quality", response.rect, &verdict.quality);
        if !verdict.prompt.is_empty() {
            ui.horizontal_wrapped(|ui| {
                let response = ui.button("Copy description");
                app.auto.add(
                    "reverseEngineering.copyDescription",
                    response.rect,
                    "Copy the complete camera description as JSON: measured path, Motion keyframes, beats and the rules behind their labels",
                );
                if response.clicked() {
                    invoke(app, ui.ctx(), "reverseEngineering.copyDescription", json!({}));
                }
                ui.label(
                    egui::RichText::new(format!("{} · the words above are labels; the numbers and their rules travel in the description", verdict.prompt))
                        .color(app.tokens.text_faint)
                        .size(11.0),
                );
            });
        }
    });
    ui.add_space(6.0);
    let count = app.ui.panels.reverse_engineering.camera2d_captures.len();
    let frame = app.ui.panels.reverse_engineering.camera2d_frame.min(count.saturating_sub(1));
    actions_bar(app, ui, is_current, count, report.is_some());
    ui.add_space(6.0);
    let wide = bounds.width() >= 700.0;
    let viewer_h = (bounds.height() * 0.42).clamp(150.0, 420.0);
    if count > 0 {
        if wide {
            let gap = 12.0;
            let left_w = ((bounds.width() - gap) * 0.62).max(1.0);
            let right_w = (bounds.width() - gap - left_w).max(1.0);
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(left_w);
                    viewer_block(app, ui, egui::vec2(left_w, viewer_h), report.is_some());
                });
                ui.add_space(gap);
                ui.vertical(|ui| {
                    ui.set_width(right_w);
                    graphs_block(app, ui, report.as_ref(), frame);
                });
            });
        } else {
            viewer_block(app, ui, egui::vec2(bounds.width().max(1.0), viewer_h), report.is_some());
            graphs_block(app, ui, report.as_ref(), frame);
        }
    }
    ui.add_space(6.0);
    applied_components(app, ui);
    if let Some(r) = &report
        && app.ui.panels.reverse_engineering.details_open
    {
        egui::Frame::group(ui.style()).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            details(app, ui, r, frame);
        });
    }
    if app.ui.panels.reverse_engineering.advanced_open {
        egui::Frame::group(ui.style()).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            advanced_options(app, ui);
        });
    }
}

fn viewer_block(app: &mut FilmcraftApp, ui: &mut egui::Ui, size: egui::Vec2, has_report: bool) {
    if has_report {
        ui.horizontal_wrapped(|ui| {
            for (id, label) in [("points", "Points"), ("matches", "Matches & regions"), ("difference", "Difference")] {
                let response = ui.selectable_label(app.ui.panels.reverse_engineering.camera2d_view == id, label);
                app.auto.add(&format!("reverseEngineering.camera2d.view.{id}"), response.rect, label);
                if response.clicked() {
                    invoke(app, ui.ctx(), "reverseEngineering.setFeatureView", json!({"view":id}));
                }
            }
        });
    }
    result(app, ui, Some(size));
    navigation(app, ui);
}

fn graphs_block(app: &mut FilmcraftApp, ui: &mut egui::Ui, report: Option<&FeatureReport>, frame: usize) {
    let Some(report) = report else {
        ui.label(egui::RichText::new("Camera path and evidence appear here after the analysis.").color(app.tokens.text_faint));
        return;
    };
    ui.label(egui::RichText::new("Camera path").color(app.tokens.text_dim));
    path_graph(app, ui, report, frame);
    ui.label(egui::RichText::new("Evidence per frame").color(app.tokens.text_dim));
    graph(app, ui, report, frame);
    ui.add_space(4.0);
    let px_scale = f64::from(report.source_size[0].max(1)) / report.analysis_width.max(1) as f64;
    let mut lines: Vec<String> = Vec::new();
    match frame.checked_sub(1).and_then(|p| report.comparisons.get(p)) {
        Some(c) => {
            let state = match c.state {
                CameraState::Static => "Camera still",
                CameraState::Moving => "Camera moving",
                CameraState::Unverified => "Motion not verified",
                CameraState::NoEvidence => "No camera evidence",
            };
            match &c.fit {
                Fit::Estimated { fit } => lines.push(format!(
                    "{state} · Δ {:.1}, {:.1} px · zoom {:.3} · {} of {} points agree",
                    fit.center_displacement_px[0] * px_scale,
                    fit.center_displacement_px[1] * px_scale,
                    fit.scale,
                    fit.inliers.len(),
                    fit.tested
                )),
                Fit::Insufficient { pairs, .. } => lines.push(format!("{state} · {pairs} matches, too few or too concentrated")),
            }
            if !c.regions.is_empty() {
                lines.push(format!("{} moving region{} set apart", c.regions.len(), if c.regions.len() == 1 { "" } else { "s" }));
            }
        }
        None => lines.push("First frame of the range: the reference".into()),
    }
    if let Some(p) = report.trajectory.get(frame) {
        lines.push(format!(
            "Path so far: Δ {:.1}, {:.1} px · zoom {:.3} · roll {:.2}°{}",
            p.displacement_px[0] * px_scale,
            p.displacement_px[1] * px_scale,
            p.scale,
            p.rotation_degrees,
            if p.residual_px > 2.0 { " · doubtful frame" } else { "" }
        ));
    }
    egui::Frame::group(ui.style()).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(egui::RichText::new(format!("This frame · {}", frame + 1)).color(app.tokens.text_dim));
        let text = lines.join("\n");
        let response = ui.label(&text);
        app.auto.add("reverseEngineering.camera2d.thisFrame", response.rect, &text);
    });
}

fn actions_bar(app: &mut FilmcraftApp, ui: &mut egui::Ui, is_current: bool, count: usize, has_report: bool) {
    let selected = selected_video_clips(app).map(|c| c.len()).unwrap_or(0);
    ui.horizontal_wrapped(|ui| {
        if has_report {
            let open = app.ui.panels.reverse_engineering.details_open;
            let response = ui.selectable_label(open, if open { "▾ Details" } else { "▸ Details" });
            app.auto.add("reverseEngineering.details", response.rect, "Every number behind the verdict");
            if response.clicked() {
                app.ui.panels.reverse_engineering.details_open = !open;
            }
        }
        let open = app.ui.panels.reverse_engineering.advanced_open;
        let response = ui.selectable_label(open, if open { "▾ Advanced" } else { "▸ Advanced" });
        app.auto.add("reverseEngineering.advanced", response.rect, "Analysis parameters; the defaults suit most footage");
        if response.clicked() {
            app.ui.panels.reverse_engineering.advanced_open = !open;
        }
        ui.separator();
        let response = ui
            .add_enabled(count > 0, egui::Button::new(if is_current { format!("Analyse again ({count} frames)") } else { format!("Analyse {count} frames") }));
        app.auto.add("reverseEngineering.runFeatures", response.rect, "Render every frame of the range, detect, match, fit the camera and solve the path");
        if response.clicked() {
            invoke(app, ui.ctx(), "reverseEngineering.runFeatures", json!({}));
        }
        if is_current && selected > 0 {
            let response = ui.button(format!("Apply to {selected} selected clip{}", if selected == 1 { "" } else { "s" }));
            app.auto.add(
                "reverseEngineering.camera2d.apply",
                response.rect,
                "Write the camera path as Motion keyframes on the selected clips, frame 1 at each clip's start",
            );
            if response.clicked() {
                invoke(app, ui.ctx(), "reverseEngineering.applyCamera", json!({"mode":"follow"}));
            }
            let response = ui.button("Stabilize selected");
            app.auto.add(
                "reverseEngineering.camera2d.stabilize",
                response.rect,
                "Write the inverse camera path as Motion keyframes at the analysed times, cancelling the camera motion",
            );
            if response.clicked() {
                invoke(app, ui.ctx(), "reverseEngineering.applyCamera", json!({"mode":"stabilize","atTimeline":true}));
            }
        }
        if is_current {
            let response = ui.button("Export camera…");
            app.auto.add("reverseEngineering.camera2d.exportCamera", response.rect, "Save the camera path and its Motion keyframes as a reusable camera file");
            if response.clicked() {
                invoke(app, ui.ctx(), "reverseEngineering.exportCamera", json!({}));
            }
        }
        if selected > 0 {
            let response = ui.button("Apply camera file…");
            app.auto.add("reverseEngineering.camera2d.applyFile", response.rect, "Apply a saved camera file to the selected clips");
            if response.clicked() {
                invoke(app, ui.ctx(), "reverseEngineering.applyCamera", json!({"mode":"follow","file":true}));
            }
        } else {
            ui.label(egui::RichText::new("Select video clips in the timeline to apply a camera.").color(app.tokens.text_faint));
        }
    });
    if is_current {
        super::reverse_tools::result_actions(app, ui, "camera2d", false);
    }
}

/// Every number behind the verdict, for the selected frame.
fn details(app: &mut FilmcraftApp, ui: &mut egui::Ui, report: &FeatureReport, frame: usize) {
    let frames = report.frames.len().max(1);
    let count_state = |s: CameraState| report.comparisons.iter().filter(|c| c.state == s).count();
    let summary = format!(
        "{} points on {} frames · {:.0} per frame · comparisons: {} static, {} moving, {} not verified, {} without camera evidence · {} anchors · analysed at {} px wide (source {}×{}) · {:.1} s",
        report.total_points,
        report.frames.len(),
        report.total_points as f64 / frames as f64,
        count_state(CameraState::Static),
        count_state(CameraState::Moving),
        count_state(CameraState::Unverified),
        count_state(CameraState::NoEvidence),
        report.anchors.len(),
        report.analysis_width,
        report.source_size[0],
        report.source_size[1],
        report.elapsed_ms as f64 / 1000.0
    );
    let response = ui.label(&summary);
    app.auto.add("reverseEngineering.camera2d.summary", response.rect, &summary);
    if let Some(f) = report.frames.get(frame) {
        ui.label(format!("Selected frame {}: {} points · mean luma {:.3}", frame + 1, f.features.len(), f.mean_luma));
    }
    if let Some(p) = report.trajectory.get(frame) {
        let label = format!(
            "Camera path at this frame (relative to frame 1): Δx {:.2} px · Δy {:.2} px · scale {:.4} · rotation {:.3}° · residual {:.2} px",
            p.displacement_px[0], p.displacement_px[1], p.scale, p.rotation_degrees, p.residual_px
        );
        let response = ui.label(&label);
        app.auto.add("reverseEngineering.camera2d.pose", response.rect, &label);
    }
    match frame.checked_sub(1).and_then(|p| report.comparisons.get(p)) {
        Some(c) => {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!(
                    "Comparison {} → {}: {} matches · {} of 16 cells · span {:.0}%×{:.0}% ·",
                    c.from_frame + 1,
                    c.to_frame + 1,
                    c.matches.len(),
                    c.coverage_cells,
                    c.span[0] * 100.0,
                    c.span[1] * 100.0
                ));
                ui.colored_label(state_color(c.state), c.state.label());
            });
            match &c.fit {
                Fit::Estimated { fit } => {
                    ui.label(format!(
                        "Camera similarity: Δx {:.3} px · Δy {:.3} px · scale {:.5} · rotation {:.3}° · RMS {:.3} px · {} / {} inliers",
                        fit.center_displacement_px[0],
                        fit.center_displacement_px[1],
                        fit.scale,
                        fit.rotation_degrees,
                        fit.rms_error_px,
                        fit.inliers.len(),
                        fit.tested
                    ));
                }
                Fit::Insufficient { reason, pairs } => {
                    ui.colored_label(egui::Color32::LIGHT_RED, format!("No camera estimate: {reason} ({pairs} pairs)"));
                }
            }
            ui.label(format!("Photometric error: {} before → {} after", option_value(c.photometric_before), option_value(c.photometric_after)));
            for (i, r) in c.regions.iter().enumerate() {
                ui.label(format!(
                    "Moving region {}: {} matches · box {:.0}–{:.0}% × {:.0}–{:.0}% · Δ({:.2}, {:.2}) px · scale {:.4} · rotation {:.2}°",
                    i + 1,
                    r.count,
                    r.bounds[0] * 100.0,
                    r.bounds[2] * 100.0,
                    r.bounds[1] * 100.0,
                    r.bounds[3] * 100.0,
                    r.displacement_px[0],
                    r.displacement_px[1],
                    r.scale,
                    r.rotation_degrees
                ));
            }
        }
        None => {
            ui.label("First frame of the range: no comparison. Select a later frame.");
        }
    }
    ui.label("Points: circle = scale, tick = orientation. Matches: green supports the camera, red does not; cyan boxes are moving regions. Difference: previous frame warped by the camera minus this frame.");
}
fn navigation(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    let count = app.ui.panels.reverse_engineering.camera2d_captures.len();
    if count == 0 {
        return;
    }
    let frame = app.ui.panels.reverse_engineering.camera2d_frame.min(count - 1);
    ui.horizontal_wrapped(|ui| {
        for (label, index, enabled) in [
            ("First", 0, frame > 0),
            ("Previous", frame.saturating_sub(1), frame > 0),
            ("Next", (frame + 1).min(count - 1), frame + 1 < count),
            ("Last", count - 1, frame + 1 < count),
        ] {
            let response = ui.add_enabled(enabled, egui::Button::new(label));
            app.auto.add(&format!("reverseEngineering.camera2d.{label}"), response.rect, label);
            if response.clicked() {
                invoke(app, ui.ctx(), "reverseEngineering.featureFrame", json!({"frameIndex":index}));
            }
        }
        let mut jump = frame + 1;
        let response = ui.add(egui::DragValue::new(&mut jump).range(1..=count));
        app.auto.add("reverseEngineering.camera2d.jump", response.rect, "Jump to range frame");
        if response.changed() {
            invoke(app, ui.ctx(), "reverseEngineering.featureFrame", json!({"frameIndex":jump.saturating_sub(1)}));
        }
        ui.label(format!("of {count}"));
    });
    if let Some(capture) = app.ui.panels.reverse_engineering.camera2d_captures.get(frame).cloned() {
        let label = format!("Frame {} / {} · {}", frame + 1, count, reverse_engineering::timecode(app, &capture));
        let response = ui.label(egui::RichText::new(label).color(app.tokens.text_dim));
        app.auto.add("reverseEngineering.timecode", response.rect, "Exact timestamp of the displayed frame");
    }
}
fn graph(app: &mut FilmcraftApp, ui: &mut egui::Ui, report: &FeatureReport, selected: usize) {
    let n = report.frames.len();
    if n == 0 {
        return;
    }
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width().max(1.0), 44.0), egui::Sense::click());
    app.auto.add("reverseEngineering.camera2d.graph", rect, "Points per frame (top) and camera state per comparison (bottom); click to jump");
    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect, 2.0, egui::Color32::from_gray(25));
    let half = rect.height() / 2.0;
    let peak = report.frames.iter().map(|f| f.features.len()).max().unwrap_or(0).max(1) as f32;
    let inlier_peak = report.comparisons.iter().map(|c| fit_inliers(&c.fit)).max().unwrap_or(0).max(1) as f32;
    let width = rect.width() / n as f32;
    for (i, f) in report.frames.iter().enumerate() {
        let h = (half - 2.0) * f.features.len() as f32 / peak;
        let x = rect.left() + width * i as f32;
        let color = if f.features.is_empty() { egui::Color32::from_rgb(160, 50, 50) } else { egui::Color32::from_rgb(220, 190, 60) };
        painter.rect_filled(egui::Rect::from_min_max(egui::pos2(x, rect.top() + half - h), egui::pos2(x + width.max(1.0), rect.top() + half)), 0.0, color);
        if let Some(c) = i.checked_sub(1).and_then(|p| report.comparisons.get(p)) {
            let h = ((half - 2.0) * fit_inliers(&c.fit) as f32 / inlier_peak).max(1.0);
            painter.rect_filled(
                egui::Rect::from_min_max(egui::pos2(x, rect.bottom() - h), egui::pos2(x + width.max(1.0), rect.bottom())),
                0.0,
                state_color(c.state),
            );
        }
    }
    let x = rect.left() + width * selected as f32 + width / 2.0;
    painter.line_segment([egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())], egui::Stroke::new(1.0, egui::Color32::WHITE));
    if let Some(pos) = response.hover_pos() {
        let i = (((pos.x - rect.left()) / rect.width().max(1.0)) * n as f32).floor().max(0.0) as usize;
        let i = i.min(n - 1);
        if let Some(f) = report.frames.get(i) {
            let comparison = i.checked_sub(1).and_then(|p| report.comparisons.get(p)).map(|c| match &c.fit {
                Fit::Estimated { fit } => format!(
                    "\n{} · {} matches · {} inliers · Δ({:.2}, {:.2}) px · scale {:.4} · rot {:.3}° · photometric {} → {} · {} regions",
                    c.state.label(),
                    c.matches.len(),
                    fit.inliers.len(),
                    fit.center_displacement_px[0],
                    fit.center_displacement_px[1],
                    fit.scale,
                    fit.rotation_degrees,
                    option_value(c.photometric_before),
                    option_value(c.photometric_after),
                    c.regions.len()
                ),
                Fit::Insufficient { reason, .. } => format!("\n{} · {} matches · {reason}", c.state.label(), c.matches.len()),
            });
            response.clone().on_hover_text(format!(
                "Frame {} · {} points · mean luma {:.3}{}",
                i + 1,
                f.features.len(),
                f.mean_luma,
                comparison.unwrap_or_default()
            ));
            if response.clicked() {
                invoke(app, ui.ctx(), "reverseEngineering.featureFrame", json!({"frameIndex":i}));
            }
        }
    }
    ui.label(
        egui::RichText::new(format!("points per frame (0–{}) above · blue still, green moving, amber not verified, red no evidence below", peak as usize))
            .color(app.tokens.text_faint)
            .size(11.0),
    );
    let _ = inlier_peak;
}
fn path_graph(app: &mut FilmcraftApp, ui: &mut egui::Ui, report: &FeatureReport, selected: usize) {
    let n = report.trajectory.len();
    if n == 0 {
        return;
    }
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width().max(1.0), 56.0), egui::Sense::click());
    app.auto.add("reverseEngineering.camera2d.path", rect, "Solved camera path: Δx, Δy (pixels) and zoom along the range; click to jump");
    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect, 2.0, egui::Color32::from_gray(25));
    let bound = report.trajectory.iter().map(|p| p.displacement_px[0].abs().max(p.displacement_px[1].abs())).fold(1.0f64, f64::max);
    let zoom_bound = report.trajectory.iter().map(|p| (p.scale - 1.0).abs()).fold(0.01f64, f64::max);
    let x_of = |i: usize| rect.left() + rect.width() * i as f32 / n.saturating_sub(1).max(1) as f32;
    painter.line_segment(
        [egui::pos2(rect.left(), rect.center().y), egui::pos2(rect.right(), rect.center().y)],
        egui::Stroke::new(1.0, egui::Color32::from_gray(70)),
    );
    for (kind, color) in
        [(0usize, egui::Color32::from_rgb(90, 160, 255)), (1, egui::Color32::from_rgb(120, 220, 140)), (2, egui::Color32::from_rgb(255, 190, 70))]
    {
        let mut previous = None;
        for (i, p) in report.trajectory.iter().enumerate() {
            let v = match kind {
                0 => p.displacement_px[0] / bound,
                1 => p.displacement_px[1] / bound,
                _ => (p.scale - 1.0) / zoom_bound,
            };
            let pos = egui::pos2(x_of(i), rect.center().y - (v as f32) * rect.height() * 0.45);
            if let Some(last) = previous {
                painter.line_segment([last, pos], egui::Stroke::new(1.5, color));
            }
            previous = Some(pos);
        }
    }
    let x = x_of(selected.min(n - 1));
    painter.line_segment([egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())], egui::Stroke::new(1.0, egui::Color32::WHITE));
    if let Some(pos) = response.hover_pos() {
        let i = (((pos.x - rect.left()) / rect.width().max(1.0)) * n.saturating_sub(1) as f32).round().max(0.0) as usize;
        if let Some(p) = report.trajectory.get(i.min(n - 1)) {
            response.clone().on_hover_text(format!(
                "Frame {} · Δx {:.2} px · Δy {:.2} px · scale {:.4} · rotation {:.3}° · residual {:.2} px",
                i.min(n - 1) + 1,
                p.displacement_px[0],
                p.displacement_px[1],
                p.scale,
                p.rotation_degrees,
                p.residual_px
            ));
            if response.clicked() {
                invoke(app, ui.ctx(), "reverseEngineering.featureFrame", json!({"frameIndex":i.min(n - 1)}));
            }
        }
    }
    ui.label(egui::RichText::new(format!("blue Δx, green Δy (±{bound:.1} px) · amber zoom (±{:.3})", zoom_bound)).color(app.tokens.text_faint).size(11.0));
}
/// The analysed frame with its overlay and the frame strip; `max_size` bounds the picture (the
/// page gives it a fixed box, since it lives inside a scroll area).
pub(super) fn result(app: &mut FilmcraftApp, ui: &mut egui::Ui, max_size: Option<egui::Vec2>) {
    let captures = app.ui.panels.reverse_engineering.camera2d_captures.clone();
    let frame = app.ui.panels.reverse_engineering.camera2d_frame.min(captures.len().saturating_sub(1));
    let report = app.ui.panels.reverse_engineering.camera2d_report.clone().filter(|_| current(app));
    let view = view_code(&app.ui.panels.reverse_engineering.camera2d_view);
    let view = if view == VIEW_DIFFERENCE && frame == 0 { VIEW_POINTS } else { view };
    let reserve = 104.0;
    let texture_view = if view == VIEW_DIFFERENCE { VIEW_DIFFERENCE } else { VIEW_POINTS };
    let handle = report.as_ref().and_then(|r| {
        request_display(app, ui.ctx(), r, frame, texture_view);
        ui.ctx().data(|d| d.get_temp::<egui::TextureHandle>(texture_id((r.completed_at_unix_ms, frame, texture_view))))
    });
    match (report.as_ref().and_then(|r| r.frames.get(frame)), handle) {
        (Some(f), Some(handle)) => {
            let [w, h] = f.analyzed_size;
            let available = max_size.unwrap_or_else(|| egui::vec2(ui.available_width().max(1.0), (ui.available_height() - reserve).max(1.0)));
            let scale = (available.x / w.max(1) as f32).min(available.y / h.max(1) as f32);
            let size = egui::vec2(w as f32 * scale, h as f32 * scale);
            let response = ui.add(egui::Image::new((handle.id(), size)).sense(egui::Sense::hover()));
            app.auto.add("reverseEngineering.frame", response.rect, "Analysed frame with the selected overlay");
            if let Some(r) = report.as_ref() {
                match view {
                    VIEW_MATCHES => overlay_matches(ui, r, frame, response.rect),
                    VIEW_DIFFERENCE => {}
                    _ => overlay_points(ui, f, response.rect),
                }
                if view == VIEW_POINTS
                    && let Some(pos) = response.hover_pos()
                {
                    let px = ((pos.x - response.rect.left()) / scale, (pos.y - response.rect.top()) / scale);
                    let nearest = f
                        .features
                        .iter()
                        .enumerate()
                        .map(|(i, p)| (i, (p.x - px.0).hypot(p.y - px.1)))
                        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
                    if let Some((i, distance)) = nearest
                        && distance * scale <= 8.0
                        && let Some(p) = f.features.get(i)
                    {
                        response.on_hover_text(format!(
                            "Point {i}: x {:.2} y {:.2} analysis px (source ≈ {:.1}, {:.1}) · scale {:.1} · orientation {:.1}° · response {:.3} · octave {}",
                            p.x,
                            p.y,
                            p.x * r.source_size[0] as f32 / w.max(1) as f32,
                            p.y * r.source_size[1] as f32 / h.max(1) as f32,
                            p.scale,
                            p.orientation.to_degrees(),
                            p.response,
                            p.octave
                        ));
                    }
                }
            }
        }
        _ => {
            if let Some(capture) = captures.get(frame) {
                match max_size {
                    Some(size) => {
                        ui.allocate_ui(size, |ui| reverse_engineering::selected_picture(app, ui, capture, 0.0));
                    }
                    None => reverse_engineering::selected_picture(app, ui, capture, reserve),
                }
            }
            if report.is_some() {
                ui.label("Rendering the analysed frame…");
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
    }
    ui.add_space(8.0);
    reverse_engineering::frame_strip(app, ui, &captures, frame, false);
}
fn overlay_points(ui: &egui::Ui, frame: &FrameFeatures, rect: egui::Rect) {
    let [w, h] = frame.analyzed_size;
    let painter = ui.painter().with_clip_rect(rect);
    let sx = rect.width() / w.max(1) as f32;
    let sy = rect.height() / h.max(1) as f32;
    for f in frame.features.iter().take(features::MAX_POINTS) {
        if !f.x.is_finite() || !f.y.is_finite() {
            continue;
        }
        let centre = egui::pos2(rect.left() + f.x * sx, rect.top() + f.y * sy);
        let radius = (f.scale * 2.0 * sx).clamp(2.0, 40.0);
        let brightness = (0.35 + 0.65 * f.response.clamp(0.0, 1.0)) * 255.0;
        let color = egui::Color32::from_rgb(brightness as u8, (brightness * 0.85) as u8, 40);
        painter.circle_stroke(centre, radius, egui::Stroke::new(1.5, color));
        let (sin, cos) = f.orientation.sin_cos();
        painter.line_segment([centre, centre + egui::vec2(cos * radius, sin * radius)], egui::Stroke::new(1.5, color));
    }
}
fn overlay_matches(ui: &egui::Ui, report: &FeatureReport, frame: usize, rect: egui::Rect) {
    let Some(current) = report.frames.get(frame) else { return };
    let Some(previous) = frame.checked_sub(1).and_then(|p| report.frames.get(p)) else { return };
    let Some(comparison) = frame.checked_sub(1).and_then(|p| report.comparisons.get(p)) else { return };
    let [w, h] = current.analyzed_size;
    let painter = ui.painter().with_clip_rect(rect);
    let sx = rect.width() / w.max(1) as f32;
    let sy = rect.height() / h.max(1) as f32;
    let inliers: std::collections::HashSet<usize> = match &comparison.fit {
        Fit::Estimated { fit } => fit.inliers.iter().copied().collect(),
        Fit::Insufficient { .. } => std::collections::HashSet::new(),
    };
    for (i, m) in comparison.matches.iter().enumerate().take(camera2d::MAX_PAIRS) {
        let (Some(a), Some(b)) = (previous.features.get(m.a), current.features.get(m.b)) else { continue };
        if ![a.x, a.y, b.x, b.y].iter().all(|v| v.is_finite()) {
            continue;
        }
        let from = egui::pos2(rect.left() + a.x * sx, rect.top() + a.y * sy);
        let to = egui::pos2(rect.left() + b.x * sx, rect.top() + b.y * sy);
        let color = if inliers.contains(&i) { egui::Color32::from_rgb(90, 220, 110) } else { egui::Color32::from_rgb(230, 70, 70) };
        painter.line_segment([from, to], egui::Stroke::new(1.5, color));
        painter.circle_filled(to, 2.0, color);
    }
    for (k, r) in comparison.regions.iter().enumerate().take(16) {
        let bounds = egui::Rect::from_min_max(
            egui::pos2(rect.left() + r.bounds[0] as f32 * rect.width(), rect.top() + r.bounds[1] as f32 * rect.height()),
            egui::pos2(rect.left() + r.bounds[2] as f32 * rect.width(), rect.top() + r.bounds[3] as f32 * rect.height()),
        );
        painter.rect_stroke(bounds, 2.0, egui::Stroke::new(2.0, egui::Color32::from_rgb(80, 220, 230)), egui::StrokeKind::Outside);
        painter.text(
            bounds.left_top() + egui::vec2(3.0, 2.0),
            egui::Align2::LEFT_TOP,
            format!("region {} · Δ({:.1}, {:.1})", k + 1, r.displacement_px[0], r.displacement_px[1]),
            egui::FontId::proportional(11.0),
            egui::Color32::from_rgb(80, 220, 230),
        );
    }
}

#[cfg(test)]
mod verdict_tests {
    use super::*;
    use filmcraft_time::FrameRate;

    fn pose(frame: usize, dx: f64, scale: f64, residual: f64) -> FramePose {
        FramePose { frame, transform: [scale, 0.0, dx, 0.0], displacement_px: [dx, 0.0], scale, rotation_degrees: 0.0, residual_px: residual }
    }

    #[test]
    fn verdict_names_still_stretches_pans_shakes_gaps_and_doubtful_frames() {
        let rate = FrameRate::FPS_24;
        let mut states = vec![CameraState::Static; 24];
        states.extend(std::iter::repeat_n(CameraState::Moving, 24));
        states.extend(std::iter::repeat_n(CameraState::Static, 24));
        let mut trajectory: Vec<FramePose> = (0..=24).map(|i| pose(i, 0.0, 1.0, 0.0)).collect();
        trajectory.extend((25..=48).map(|i| pose(i, (i - 24) as f64 * 2.0, 1.0, if i == 30 { 3.0 } else { 0.0 })));
        trajectory.extend((49..=72).map(|i| pose(i, 48.0, 1.0, 0.0)));
        let size = [1920, 1080];
        let v = verdict(&states, &trajectory, rate, 3.0, size);
        assert_eq!(
            v.headline,
            "Camera: static shot for 1.0 s (0.0–1.0 s); then slow pan or truck right across 8 % of the frame over 1.0 s (1.0–2.0 s); then static shot for 1.0 s (2.0–3.0 s)."
        );
        assert!(v.quality.starts_with("100 % of frames with solid evidence · 1 doubtful frame (1.2 s)"), "{}", v.quality);
        assert!(v.quality.ends_with("3.0 s analysed"), "{}", v.quality);

        let mut states = vec![CameraState::Static; 24];
        states.extend(std::iter::repeat_n(CameraState::Moving, 6));
        states.extend(std::iter::repeat_n(CameraState::Static, 24));
        let trajectory: Vec<FramePose> = (0..=54).map(|i| pose(i, if (25..=30).contains(&i) { 0.3 } else { 0.0 }, 1.0, 0.0)).collect();
        let v = verdict(&states, &trajectory, rate, 1.0, size);
        assert!(v.headline.contains("then handheld shake for 0.2 s"), "{}", v.headline);

        let mut states = vec![CameraState::Moving; 48];
        states.extend(std::iter::repeat_n(CameraState::NoEvidence, 12));
        let trajectory: Vec<FramePose> = (0..=60).map(|i| pose(i, 0.0, 1.0 + 0.005 * i.min(48) as f64, 0.0)).collect();
        let v = verdict(&states, &trajectory, rate, 1.0, size);
        assert!(v.headline.starts_with("Camera: steady zoom or dolly in by 24 % over 2.0 s (0.0–2.0 s); then no camera evidence for 0.5 s"), "{}", v.headline);
        assert!(v.quality.contains("80 % of frames with solid evidence") && v.quality.contains("12 frames without evidence"), "{}", v.quality);

        let states = [CameraState::Static, CameraState::Unverified, CameraState::Static];
        let trajectory: Vec<FramePose> = (0..=3).map(|i| pose(i, 0.0, 1.0, 0.0)).collect();
        assert_eq!(verdict(&states, &trajectory, rate, 1.0, size).headline, "Camera: static shot for 0.1 s (0.0–0.1 s).");
        assert_eq!(verdict(&[], &[], rate, 1.0, size).headline, "Not analysed yet");
        assert_eq!(verdict(&states, &trajectory[..2], rate, 1.0, size).headline, "Not analysed yet");
    }
}

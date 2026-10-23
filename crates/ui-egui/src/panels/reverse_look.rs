//! Look: the grade of every shot in the selected range, measured on several rendered frames and
//! read in a colourist's words (key, contrast, saturation, temperature, palette, vignette, grain,
//! sharpness, stability), exportable as a `filmcraft.look` file and applicable to clips as a
//! Lumetri Color grade recorded as a component. Pure Rust, no model.
//!
//! Commands: `reverseEngineering.runLook`, `reverseEngineering.cancelLook`,
//! `reverseEngineering.setLookOptions {"paletteSize"?,"maxSamples"?}`, `reverseEngineering.lookShot {"index"}`,
//! `reverseEngineering.exportLook {"path"?}`, `reverseEngineering.applyLook {"clips"?,"shot"?,"path"?|"file"?}`,
//! `reverseEngineering.chooseLookFile {"path"?}` (opens the chooser), `reverseEngineering.copyLookDescription {}`.
use super::{panel_state::ReverseEngineeringCapture, reverse_engineering};
use crate::FilmcraftApp;
use filmcraft_render::look_transfer::{self, ColorStats};
use filmcraft_vision::{
    grade::{self, FrameSample, Grade},
    look,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

pub const MAX_RANGE_FRAMES: usize = filmcraft_vision::shots::MAX_FRAMES;
pub const LOOK_FORMAT: &str = "filmcraft.look";
pub const MAX_SAMPLES: usize = 240;
const ANALYSIS_SIDE: f32 = 320.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LookShot {
    pub index: usize,
    pub start_frame: usize,
    pub end_frame: usize,
    pub start_tick: i64,
    pub end_tick: i64,
    pub keyframe: usize,
    pub black: bool,
    pub samples: Vec<FrameSample>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grade: Option<Grade>,
    /// Oklab mean and spread of the shot's sampled frames: the reference the look transfer moves
    /// other footage onto.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<ColorStats>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LookReport {
    pub schema_version: u32,
    pub analyzer: String,
    pub captures: Vec<ReverseEngineeringCapture>,
    pub project_revision: u64,
    pub completed_at_unix_ms: u64,
    pub source_size: [u32; 2],
    pub analysis_size: [usize; 2],
    pub palette_size: usize,
    pub max_samples: usize,
    /// The shots came from a current Shots & look result (else the whole range is one shot).
    pub from_shots: bool,
    pub shots: Vec<LookShot>,
    pub overall: Grade,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<ColorStats>,
    pub limitations: Vec<String>,
}

/// The exported `filmcraft.look` file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LookFile {
    pub format: String,
    pub version: u32,
    pub source: LookSource,
    pub overall: Grade,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<ColorStats>,
    pub shots: Vec<LookFileShot>,
    /// Lumetri Color parameters that reproduce the overall grade on neutral footage.
    pub lumetri: serde_json::Map<String, Value>,
    pub summary: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LookSource {
    pub sequence: String,
    pub width: u32,
    pub height: u32,
    pub frame_rate: f64,
    pub start_frame: i64,
    pub end_frame: i64,
    pub analyzer: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LookFileShot {
    pub index: usize,
    pub start_frame: usize,
    pub end_frame: usize,
    pub seconds: f64,
    pub black: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grade: Option<Grade>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<ColorStats>,
}

#[derive(Clone)]
struct Job {
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicUsize>,
    total: usize,
    result: Arc<Mutex<Option<Result<LookReport, String>>>>,
}

fn job_id() -> egui::Id {
    egui::Id::new("reverse-look-job")
}

fn after_shots_id() -> egui::Id {
    egui::Id::new("reverse-look-after-shots")
}

pub(super) fn awaiting_shots(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(after_shots_id())).unwrap_or(false)
}

pub(super) fn is_running(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<Job>(job_id())).is_some() || awaiting_shots(ctx)
}

pub(super) fn current(app: &FilmcraftApp) -> bool {
    let s = &app.ui.panels.reverse_engineering;
    s.look_report.as_ref().is_some_and(|r| {
        r.project_revision == app.session.revision
            && r.palette_size == s.look_palette_size
            && r.max_samples == s.look_max_samples
            && !r.shots.is_empty()
            && reverse_engineering::captures_are_current(app, &r.captures)
    })
}

fn limitations() -> Vec<String> {
    vec![
        "The grade is measured on rendered frames at 320 pixels on the longest side: means and percentiles of luma, saturation and a red-minus-blue warmth, a k-means palette with names from hue, lightness and saturation bands, a corner-over-centre luma ratio for the vignette, fine noise in the flattest blocks for grain and the luma Laplacian for sharpness.".into(),
        "Every word (low-key, punchy, vivid, warm, grainy…) is a classification of the number next to it under the thresholds in `rules`; it is not a recognised style, LUT or camera profile.".into(),
        "Applying measures the destination clip's own frames and bakes a colour-transfer LUT (Reinhard transfer in Oklab: mean and spread of lightness and the two colour axes) into Lumetri's Creative Look slot, so every colour of the clip moves towards the reference frames; it carries tone, cast and colourfulness, not objects or textures.".into(),
        "Black shots carry no grade; shots come from the Shots & look result when it is current for the same range, else the range is one shot.".into(),
    ]
}

fn timestamp() -> Result<u64, String> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())
        .and_then(|d| u64::try_from(d.as_millis()).map_err(|e| e.to_string()))
}

/// The shots to grade: from a current Shots & look result over the same captures, else one.
fn shot_plan(app: &FilmcraftApp, captures: &[ReverseEngineeringCapture]) -> (bool, Vec<(usize, usize, usize, bool)>) {
    let last = captures.len().saturating_sub(1);
    if super::reverse_shots::current(app)
        && let Some(report) = app.ui.panels.reverse_engineering.shots.as_ref()
        && report.captures == captures
        && !report.shots.is_empty()
    {
        return (true, report.shots.iter().map(|s| (s.start_frame.min(last), s.end_frame.min(last), s.keyframe.min(last), s.black)).collect());
    }
    (false, vec![(0, last, last / 2, false)])
}

/// Evenly spaced frame indices of a shot, `n` of them (the keyframe alone when n = 1).
fn sample_frames(start: usize, end: usize, keyframe: usize, n: usize) -> Vec<usize> {
    let len = end.saturating_sub(start) + 1;
    let n = n.clamp(1, len);
    if n == 1 {
        return vec![keyframe.clamp(start, end)];
    }
    (0..n).map(|j| start + (len - 1) * j / (n - 1)).collect()
}

pub(super) fn poll(app: &mut FilmcraftApp, ctx: &egui::Context) {
    if awaiting_shots(ctx) {
        if super::reverse_shots::is_running(ctx) {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            return;
        }
        ctx.data_mut(|d| d.remove::<bool>(after_shots_id()));
        if super::reverse_shots::current(app) {
            if let Err(e) = start(app, ctx) {
                app.ui.status = e.clone();
                ctx.data_mut(|d| d.insert_temp(egui::Id::new("reverse-tools-error"), e));
            }
        } else if ctx.data(|d| d.get_temp::<String>(egui::Id::new("reverse-tools-error"))).is_none() {
            app.ui.status = "Shot detection did not finish, so the look was not analysed".into();
        }
        return;
    }
    let Some(job) = ctx.data(|d| d.get_temp::<Job>(job_id())) else {
        return;
    };
    let result = job.result.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
    if let Some(result) = result {
        ctx.data_mut(|d| d.remove::<Job>(job_id()));
        match result {
            Ok(report) if !job.cancel.load(Ordering::Relaxed) => {
                let graded = report.shots.iter().filter(|s| s.grade.is_some()).count();
                app.ui.status = format!("{} · {graded} shot{} graded", report.overall.summary, if graded == 1 { "" } else { "s" });
                let selected = app.ui.panels.reverse_engineering.look_selected.min(report.shots.len().saturating_sub(1));
                app.ui.panels.reverse_engineering.look_report = Some(report);
                let _ = select(app, ctx, selected);
            }
            Ok(_) => app.ui.status = "Look analysis cancelled; previous result preserved".into(),
            Err(e) => {
                app.ui.status = e.clone();
                ctx.data_mut(|d| d.insert_temp(egui::Id::new("reverse-tools-error"), e));
            }
        }
    } else {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}

fn select(app: &mut FilmcraftApp, ctx: &egui::Context, index: usize) -> Result<Value, String> {
    let report = app.ui.panels.reverse_engineering.look_report.as_ref().ok_or("Analyse the look first")?;
    let shot = report.shots.get(index).ok_or("Shot index outside the result")?;
    let capture = report.captures.get(shot.keyframe).cloned().ok_or("Shot keyframe outside the range")?;
    let start = report.captures.get(shot.start_frame).map(|c| c.timeline_time).ok_or("Shot start outside the range")?;
    let (start_frame, end_frame) = (shot.start_frame, shot.end_frame);
    app.ui.panels.reverse_engineering.look_selected = index;
    app.ui.panels.reverse_engineering.capture = Some(capture);
    app.stop();
    app.session.set_playhead(start);
    reverse_engineering::reset_decode_timer(ctx);
    Ok(json!({"index":index,"startFrame":start_frame,"endFrame":end_frame}))
}

fn start(app: &mut FilmcraftApp, ctx: &egui::Context) -> Result<Value, String> {
    if is_running(ctx) {
        return Err("Look analysis is already running".into());
    }
    if cfg!(target_arch = "wasm32") {
        return Err("Look analysis currently requires the desktop app.".into());
    }
    let captures = reverse_engineering::range_captures(app, MAX_RANGE_FRAMES)?;
    let first = captures.first().ok_or("No frames selected")?;
    let (width, height) = app.session.project.source_size(first.item).ok_or("No video dimensions")?;
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 67_108_864 {
        return Err("Invalid or oversized source dimensions".into());
    }
    let state = &app.ui.panels.reverse_engineering;
    let palette_size = state.look_palette_size;
    if !(1..=look::MAX_PALETTE).contains(&palette_size) {
        return Err("Palette size must be 1–16 colours".into());
    }
    let max_samples = state.look_max_samples;
    if !(1..=MAX_SAMPLES).contains(&max_samples) {
        return Err(format!("Sampled frames must be 1–{MAX_SAMPLES}"));
    }
    if !super::reverse_shots::current(app) {
        if !super::reverse_shots::is_running(ctx) {
            super::reverse_shots::start(app, ctx, None)?;
        }
        ctx.data_mut(|d| d.insert_temp(after_shots_id(), true));
        app.ui.status = "Detecting shots first; the look follows one grade per shot".into();
        return Ok(json!({"started":true,"detectingShots":true}));
    }
    let (from_shots, plan) = shot_plan(app, &captures);
    let total_frames = captures.len().max(1);
    let mut planned: Vec<(usize, usize, usize, bool, Vec<usize>)> = Vec::new();
    for (start, end, keyframe, black) in plan {
        let len = end.saturating_sub(start) + 1;
        let frames = if black { Vec::new() } else { sample_frames(start, end, keyframe, (max_samples * len).div_ceil(total_frames).clamp(1, len)) };
        planned.push((start, end, keyframe, black, frames));
    }
    let total: usize = planned.iter().map(|p| p.4.len()).sum();
    if total == 0 {
        return Err("The range holds only black frames; nothing to grade".into());
    }
    let revision = app.session.revision;
    let project = app.session.project.clone();
    let provider = app.session.media.provider(project.clone(), app.session.services.clone());
    let job = Job { cancel: Arc::new(AtomicBool::new(false)), progress: Arc::new(AtomicUsize::new(0)), total, result: Arc::new(Mutex::new(None)) };
    let worker = job.clone();
    let repaint = ctx.clone();
    std::thread::Builder::new()
        .name("filmcraft-look".into())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<LookReport, String> {
                let started = std::time::Instant::now();
                let scale = (ANALYSIS_SIDE / width.max(height) as f32).min(1.0);
                let mut analysis_size = [0usize; 2];
                let mut done = 0usize;
                let mut shots = Vec::new();
                let mut all: Vec<FrameSample> = Vec::new();
                let mut all_color: Vec<ColorStats> = Vec::new();
                for (index, (start, end, keyframe, black, frames)) in planned.into_iter().enumerate() {
                    let mut samples = Vec::new();
                    let mut colors: Vec<ColorStats> = Vec::new();
                    for frame in frames {
                        if worker.cancel.load(Ordering::Relaxed) {
                            return Err("Look analysis cancelled".into());
                        }
                        if started.elapsed().as_secs() > 1800 {
                            return Err("Look analysis timed out; choose a shorter range or fewer samples".into());
                        }
                        let capture = captures.get(frame).ok_or("Sample outside the range")?;
                        let image = filmcraft_render::render_item(&project, capture.item, capture.media_time, scale, &provider)
                            .ok_or("A timeline frame could not be rendered. Check the media and retry.")?;
                        if let Some(stats) = look_transfer::stats(&image) {
                            colors.push(stats);
                        }
                        let (w, h, rgba) = (image.w, image.h, image.to_rgba8());
                        analysis_size = [w, h];
                        let sample = FrameSample { frame, look: look::analyze(w, h, &rgba, palette_size)?, tone: grade::tone(w, h, &rgba)? };
                        all.push(sample.clone());
                        samples.push(sample);
                        done += 1;
                        worker.progress.store(done, Ordering::Relaxed);
                        repaint.request_repaint();
                    }
                    let grade = if black || samples.is_empty() { None } else { Some(grade::grade(&samples, palette_size)?) };
                    let color = if black { None } else { look_transfer::merge(&colors) };
                    if let Some(c) = &color {
                        all_color.push(c.clone());
                    }
                    let start_tick = captures.get(start).map(|c| c.timeline_time.0).ok_or("Shot start outside the range")?;
                    let end_tick = captures.get(end).map(|c| c.timeline_time.0).ok_or("Shot end outside the range")?;
                    shots.push(LookShot { index, start_frame: start, end_frame: end, start_tick, end_tick, keyframe, black, samples, grade, color });
                }
                all.truncate(grade::MAX_SAMPLES);
                let overall = grade::grade(&all, palette_size)?;
                let color = look_transfer::merge(&all_color);
                Ok(LookReport {
                    schema_version: 1,
                    analyzer: "filmcraft-vision/look-grade-v1".into(),
                    captures,
                    project_revision: revision,
                    completed_at_unix_ms: timestamp()?,
                    source_size: [width, height],
                    analysis_size,
                    palette_size,
                    max_samples,
                    from_shots,
                    shots,
                    overall,
                    color,
                    limitations: limitations(),
                })
            }))
            .unwrap_or_else(|_| Err("The look worker failed; previous results preserved".into()));
            *worker.result.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(outcome);
            repaint.request_repaint();
        })
        .map_err(|e| format!("Could not start the look analysis: {e}"))?;
    ctx.data_mut(|d| d.insert_temp(job_id(), job));
    Ok(json!({"started":true,"samples":total}))
}

fn frame_rate(app: &FilmcraftApp) -> f64 {
    app.session.active_sequence().map(|q| q.settings.frame_rate.as_f64()).unwrap_or(0.0)
}

fn sequence_name(app: &FilmcraftApp) -> String {
    app.session.state.active_sequence.and_then(|id| app.session.project.item(id)).map(|i| i.name.clone()).unwrap_or_default()
}

fn look_file(app: &FilmcraftApp, report: &LookReport) -> Result<LookFile, String> {
    app.session.active_sequence().ok_or("Open a sequence")?;
    let state = &app.ui.panels.reverse_engineering;
    let fps = frame_rate(app);
    let shots = report
        .shots
        .iter()
        .map(|s| LookFileShot {
            index: s.index,
            start_frame: s.start_frame,
            end_frame: s.end_frame,
            seconds: if fps > 0.0 { (s.end_frame - s.start_frame + 1) as f64 / fps } else { 0.0 },
            black: s.black,
            grade: s.grade.clone(),
            color: s.color.clone(),
        })
        .collect();
    let lumetri = grade::lumetri_params(&report.overall).into_iter().map(|(k, v)| (k.to_string(), json!(v))).collect();
    Ok(LookFile {
        format: LOOK_FORMAT.into(),
        version: 1,
        source: LookSource {
            sequence: sequence_name(app),
            width: report.source_size[0],
            height: report.source_size[1],
            frame_rate: fps,
            start_frame: state.start_frame,
            end_frame: state.end_frame,
            analyzer: report.analyzer.clone(),
        },
        overall: report.overall.clone(),
        color: report.color.clone(),
        shots,
        lumetri,
        summary: report.overall.summary.clone(),
    })
}

fn current_report(app: &FilmcraftApp) -> Result<LookReport, String> {
    if !current(app) {
        return Err("Analyse the look first; the result must be current".into());
    }
    app.ui.panels.reverse_engineering.look_report.clone().ok_or_else(|| "No look result".into())
}

fn export_look(app: &mut FilmcraftApp, params: &Value) -> Result<Value, String> {
    let report = current_report(app)?;
    let file = look_file(app, &report)?;
    let path = if let Some(path) = params.get("path") {
        Some(path.as_str().filter(|s| !s.trim().is_empty()).ok_or("Supply a nonempty export path")?.to_owned())
    } else {
        app.hooks.pick_save_as.as_mut().ok_or("No save dialog; supply path")?("Look", &["json"], "look.look.json")
    };
    let Some(path) = path else {
        return Ok(Value::Null);
    };
    let bytes = serde_json::to_vec_pretty(&file).map_err(|e| e.to_string())?;
    std::fs::write(&path, bytes).map_err(|e| format!("Could not export the look: {e}"))?;
    app.ui.status = format!("Exported look ({} shots) to {path}", file.shots.len());
    Ok(json!({"path":path,"shots":file.shots.len()}))
}

fn copy_description(app: &mut FilmcraftApp, ctx: &egui::Context) -> Result<Value, String> {
    let report = current_report(app)?;
    let file = look_file(app, &report)?;
    let description = serde_json::to_value(&file).map_err(|e| e.to_string())?;
    ctx.copy_text(serde_json::to_string_pretty(&description).map_err(|e| e.to_string())?);
    app.ui.status = format!("Look description copied ({} shots)", file.shots.len());
    Ok(description)
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
        return Err("Select the video clips to apply the look to".into());
    }
    Ok(clips)
}

const MAX_LOOK_FILE_BYTES: u64 = 16 * 1024 * 1024;

fn validate_grade(grade: &Grade) -> Result<(), String> {
    let numbers = [
        grade.mean_luma,
        grade.black_point,
        grade.white_point,
        grade.contrast,
        grade.saturation,
        grade.warm_bias,
        grade.vignette,
        grade.grain,
        grade.sharpness,
        grade.luma_variation,
    ];
    if numbers.iter().any(|v| !v.is_finite() || !(-10.0..=10.0).contains(v)) || grade.palette.len() > look::MAX_PALETTE || grade.summary.len() > 4096 {
        return Err("The look file holds an invalid grade".into());
    }
    Ok(())
}

fn load_look_file(path: &str) -> Result<LookFile, String> {
    let size = std::fs::metadata(path).map_err(|e| format!("Cannot read the look file: {e}"))?.len();
    if size > MAX_LOOK_FILE_BYTES {
        return Err("The look file is larger than 16 MB".into());
    }
    let bytes = std::fs::read(path).map_err(|e| format!("Cannot read the look file: {e}"))?;
    let file: LookFile = serde_json::from_slice(&bytes).map_err(|e| format!("Not a look file: {e}"))?;
    if file.format != LOOK_FORMAT || file.version != 1 {
        return Err(format!("Expected a {LOOK_FORMAT} version 1 file"));
    }
    if file.shots.len() > 10_000 {
        return Err("The look file holds too many shots".into());
    }
    validate_grade(&file.overall)?;
    for grade in file.shots.iter().filter_map(|s| s.grade.as_ref()) {
        validate_grade(grade)?;
    }
    Ok(file)
}

/// Apply a grade to clips as Lumetri Color parameters recorded as a `look` component:
/// `{"clips":[id]?,"shot":index? (default: the overall grade),"path"?|"file":true}`. With a path
/// or `file`, the grade comes from a saved look file instead of the current result.
fn apply_look(app: &mut FilmcraftApp, params: &Value) -> Result<Value, String> {
    let file_path = match params.get("path") {
        Some(path) => Some(path.as_str().filter(|s| !s.trim().is_empty()).ok_or("Supply a nonempty look file path")?.to_owned()),
        None if params.get("file").and_then(Value::as_bool) == Some(true) => {
            match app.hooks.pick_open_file.as_mut().ok_or("No open dialog; supply path")?("Look", &["json"]) {
                Some(path) => Some(path),
                None => return Ok(Value::Null),
            }
        }
        None => None,
    };
    let shot_index = params.get("shot").and_then(Value::as_u64).map(|i| i as usize);
    let strength = match params.get("strength") {
        Some(v) => v.as_f64().filter(|s| s.is_finite() && (0.0..=1.0).contains(s)).ok_or("strength must be 0–1")? as f32,
        None => 1.0,
    };
    let (grade, target, name, source, from_report) = match &file_path {
        Some(path) => {
            let file = load_look_file(path)?;
            let label = std::path::Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
            let (grade, target, name) = match shot_index {
                Some(i) => {
                    let shot = file.shots.get(i).ok_or("No such shot in the look file")?;
                    (shot.grade.clone().ok_or("That shot is black and has no grade")?, shot.color.clone(), format!("Look · {label} · shot {}", i + 1))
                }
                None => (file.overall.clone(), file.color.clone(), format!("Look · {label}")),
            };
            let s = &file.source;
            let source = json!({"sequence":s.sequence,"width":s.width,"height":s.height,"frameRate":s.frame_rate,"startFrame":s.start_frame,"endFrame":s.end_frame,"file":path});
            (grade, target, name, source, false)
        }
        None => {
            let report = current_report(app)?;
            let (grade, mut target, name, frames) = match shot_index {
                Some(i) => {
                    let shot = report.shots.get(i).ok_or("No such shot")?;
                    let frames: Vec<usize> = shot.samples.iter().map(|s| s.frame).collect();
                    (shot.grade.clone().ok_or("That shot is black and has no grade")?, shot.color.clone(), format!("Look · shot {}", i + 1), frames)
                }
                None => {
                    let frames: Vec<usize> = report.shots.iter().filter(|s| !s.black).flat_map(|s| s.samples.iter().map(|x| x.frame)).collect();
                    (report.overall.clone(), report.color.clone(), "Look · whole range".to_string(), frames)
                }
            };
            if target.is_none() {
                target = reference_stats(app, &report.captures, &frames);
            }
            let state_range = (app.ui.panels.reverse_engineering.start_frame, app.ui.panels.reverse_engineering.end_frame);
            let source = json!({"sequence":sequence_name(app),"width":report.source_size[0],"height":report.source_size[1],"frameRate":frame_rate(app),
                "startFrame":state_range.0,"endFrame":state_range.1});
            (grade, target, name, source, true)
        }
    };
    let target = target.ok_or(
        "This look carries no colour statistics (it was analysed or exported before colour transfer existed). Analyse the look again, or re-export the look file, and apply it then.",
    )?;
    let to_range = params.get("range").is_some()
        || params.get("target").and_then(Value::as_str) == Some("range")
        || (params.get("clips").is_none() && selected_video_clips(app).is_err());
    let spans: Vec<(filmcraft_time::Tick, filmcraft_time::Tick)> = if to_range {
        vec![apply_range(app, params)?]
    } else {
        let clips = match params.get("clips").and_then(Value::as_array) {
            Some(list) => list.iter().map(|v| v.as_u64().map(filmcraft_project::ClipId).ok_or("clips must be ids")).collect::<Result<Vec<_>, _>>()?,
            None => selected_video_clips(app)?,
        };
        let sequence = app.session.active_sequence().ok_or("Open a sequence")?;
        let mut spans = Vec::new();
        for clip in &clips {
            let (track, item) = sequence.find_item(*clip).ok_or_else(|| format!("Clip {} is not in the active sequence", clip.0))?;
            if sequence.track(track).is_none_or(|t| t.kind != filmcraft_project::TrackKind::Video) {
                return Err(format!("Clip {} is not a video clip", clip.0));
            }
            spans.push((item.start, item.end()));
        }
        spans
    };
    let mut applied = Vec::new();
    let mut last_lut = Value::Null;
    for (start, end) in &spans {
        let result = app
            .session
            .execute(
                "components.applyLookRange",
                json!({"start":start.0,"end":end.0,"color":target,"name":name,"summary":grade.summary,"strength":strength,"source":source}),
            )
            .map_err(|e| e.to_string())?;
        app.ui.panels.reverse_engineering.focused_component = result.get("component").and_then(Value::as_u64);
        last_lut = result.get("lut").cloned().unwrap_or(Value::Null);
        applied.push(json!({
            "clip":result.get("clip").cloned().unwrap_or(Value::Null),
            "component":result.get("component").cloned().unwrap_or(Value::Null),
            "track":result.get("track").cloned().unwrap_or(Value::Null),
            "start":start.0,
            "end":end.0,
        }));
    }
    if from_report && let Some(r) = app.ui.panels.reverse_engineering.look_report.as_mut() {
        r.project_revision = app.session.revision;
    }
    app.ui.status = format!(
        "Look placed on {} adjustment layer{} as a colour-transfer LUT; drag, trim or cut the layer like any clip",
        applied.len(),
        if applied.len() == 1 { "" } else { "s" }
    );
    Ok(json!({"clips":applied,"adjustmentLayer":true,"lumetri":{"look_lut":last_lut},"transferred":applied.len()}))
}

/// The time range a look lands on when no clip is the target: an explicit `range`, else the
/// sequence's In and Out points, else the analysed range.
fn apply_range(app: &FilmcraftApp, params: &Value) -> Result<(filmcraft_time::Tick, filmcraft_time::Tick), String> {
    let sequence = app.session.active_sequence().ok_or("Open a sequence")?;
    let frame = sequence.settings.frame_rate.frame_duration();
    if let Some(range) = params.get("range") {
        let start = range.get("start").and_then(Value::as_i64).ok_or("range.start must be integer ticks")?;
        let end = range.get("end").and_then(Value::as_i64).ok_or("range.end must be integer ticks")?;
        if end <= start {
            return Err("The range must end after it starts".into());
        }
        return Ok((filmcraft_time::Tick(start), filmcraft_time::Tick(end)));
    }
    if let (Some(i), Some(o)) = (sequence.mark_in, sequence.mark_out) {
        return Ok((i, o + frame));
    }
    let captures = reverse_engineering::range_captures(app, MAX_RANGE_FRAMES)?;
    let (Some(first), Some(last)) = (captures.first(), captures.last()) else {
        return Err("Set In and Out points, or select clips, to say where the look goes".into());
    };
    Ok((first.timeline_time, last.timeline_time + frame))
}

/// Where a look applied without clips would land, for the chooser.
fn range_description(app: &FilmcraftApp) -> String {
    match apply_range(app, &json!({})) {
        Ok((start, end)) => {
            let rate = app.session.active_sequence().map(|q| q.settings.frame_rate).unwrap_or_default();
            let (a, b) = (rate.frame_at(start), rate.frame_at(end).saturating_sub(1));
            let from_marks = app.session.active_sequence().is_some_and(|q| q.mark_in.is_some() && q.mark_out.is_some());
            format!("frames {}–{} ({})", a.saturating_add(1), b.saturating_add(1), if from_marks { "In → Out" } else { "the analysed range" })
        }
        Err(e) => e,
    }
}

/// Colour statistics of reference frames of the analysed range, rendered now (for a result made
/// before the statistics were recorded).
fn reference_stats(app: &FilmcraftApp, captures: &[ReverseEngineeringCapture], frames: &[usize]) -> Option<ColorStats> {
    let first = captures.first()?;
    let (width, height) = app.session.project.source_size(first.item)?;
    if width == 0 || height == 0 {
        return None;
    }
    let project = app.session.project.clone();
    let provider = app.session.media.provider(project.clone(), app.session.services.clone());
    let scale = (ANALYSIS_SIDE / width.max(height) as f32).min(1.0);
    let step = (frames.len() / 8).max(1);
    let mut parts = Vec::new();
    for frame in frames.iter().step_by(step).take(8) {
        let capture = captures.get(*frame)?;
        let image = filmcraft_render::render_item(&project, capture.item, capture.media_time, scale, &provider)?;
        if let Some(stats) = look_transfer::stats(&image) {
            parts.push(stats);
        }
    }
    look_transfer::merge(&parts)
}

#[derive(Clone)]
struct PendingLookFile {
    path: String,
    file: LookFile,
    to_range: bool,
}

fn pending_file_id() -> egui::Id {
    egui::Id::new("reverse-look-pending-file")
}

/// Open a look file and keep it for the chooser: `{"path"?}` (a dialog without one).
fn choose_look_file(app: &mut FilmcraftApp, ctx: &egui::Context, params: &Value) -> Result<Value, String> {
    let path = match params.get("path") {
        Some(path) => path.as_str().filter(|s| !s.trim().is_empty()).ok_or("Supply a nonempty look file path")?.to_owned(),
        None => match app.hooks.pick_open_file.as_mut().ok_or("No open dialog; supply path")?("Look", &["json"]) {
            Some(path) => path,
            None => return Ok(Value::Null),
        },
    };
    let file = load_look_file(&path)?;
    let graded = file.shots.iter().filter(|s| s.grade.is_some()).count();
    let to_range = selected_video_clips(app).is_err();
    ctx.data_mut(|d| d.insert_temp(pending_file_id(), PendingLookFile { path: path.clone(), file, to_range }));
    ctx.request_repaint();
    Ok(json!({"path":path,"shots":graded,"toRange":to_range}))
}

fn file_chooser(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let Some(pending) = ctx.data(|d| d.get_temp::<PendingLookFile>(pending_file_id())) else {
        return;
    };
    let clips = selected_video_clips(app).map(|c| c.len()).unwrap_or(0);
    let label = std::path::Path::new(&pending.path).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| pending.path.clone());
    let range_text = range_description(app);
    let range_ok = apply_range(app, &json!({})).is_ok();
    let mut to_range = pending.to_range || clips == 0;
    let mut close = false;
    let mut apply: Option<Option<usize>> = None;
    egui::Modal::new(pending_file_id()).show(ctx, |ui| {
        ui.set_width(560.0);
        ui.heading(format!("Apply a look from {label}"));
        ui.label(
            egui::RichText::new(format!(
                "{} · {}×{} · frames {}–{}",
                pending.file.source.sequence,
                pending.file.source.width,
                pending.file.source.height,
                pending.file.source.start_frame.saturating_add(1),
                pending.file.source.end_frame.saturating_add(1),
            ))
            .color(app.tokens.text_dim),
        );
        ui.add_space(4.0);
        ui.label(egui::RichText::new("Put it on").strong());
        ui.horizontal_wrapped(|ui| {
            let r = ui.add_enabled(clips > 0, egui::RadioButton::new(!to_range, format!("the {clips} selected clip{}", if clips == 1 { "" } else { "s" })));
            app.auto.add("reverseEngineering.look.file.toClips", r.rect, "Apply to the selected clips");
            if r.clicked() {
                to_range = false;
            }
            let r = ui.add_enabled(range_ok, egui::RadioButton::new(to_range, format!("an adjustment layer over {range_text}")));
            app.auto.add("reverseEngineering.look.file.toRange", r.rect, "Apply over a time range as an adjustment layer you can drag and trim");
            if r.clicked() {
                to_range = true;
            }
        });
        if to_range {
            ui.label(
                egui::RichText::new(
                    "Set In and Out points in the timeline to choose another range. The layer is an ordinary clip: move it, trim it, stack several.",
                )
                .color(app.tokens.text_faint)
                .size(11.0),
            );
        }
        let can_apply = if to_range { range_ok } else { clips > 0 };
        ui.add_space(6.0);
        let mut row = |app: &mut FilmcraftApp, ui: &mut egui::Ui, id: &str, title: String, grade: &Grade, choice: Option<usize>| {
            egui::Frame::group(ui.style()).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(&title).strong());
                        swatches(app, ui, grade, &format!("{id}.palette"));
                        ui.label(egui::RichText::new(&grade.summary).color(app.tokens.text_dim).size(11.0));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let response = ui.add_enabled(can_apply, egui::Button::new("Apply"));
                        app.auto.add(id, response.rect, &title);
                        if response.clicked() {
                            apply = Some(choice);
                        }
                    });
                });
            });
        };
        let graded: Vec<&LookFileShot> = pending.file.shots.iter().filter(|s| s.grade.is_some()).collect();
        if graded.len() > 1 {
            row(app, ui, "reverseEngineering.look.file.whole", "Whole range (all shots averaged)".to_string(), &pending.file.overall, None);
        }
        for shot in graded {
            if let Some(grade) = &shot.grade {
                let title = format!("Shot {} · frames {}–{} · {:.1} s", shot.index + 1, shot.start_frame + 1, shot.end_frame + 1, shot.seconds);
                row(app, ui, &format!("reverseEngineering.look.file.shot.{}", shot.index), title, grade, Some(shot.index));
            }
        }
        ui.add_space(6.0);
        let response = ui.button("Cancel");
        app.auto.add("reverseEngineering.look.file.cancel", response.rect, "Close without applying");
        if response.clicked() {
            close = true;
        }
    });
    if to_range != pending.to_range {
        ctx.data_mut(|d| d.insert_temp(pending_file_id(), PendingLookFile { to_range, ..pending.clone() }));
    }
    if let Some(choice) = apply {
        let mut params = json!({"path":pending.path});
        if let Some(map) = params.as_object_mut() {
            if let Some(index) = choice {
                map.insert("shot".into(), json!(index));
            }
            if to_range {
                map.insert("target".into(), json!("range"));
            }
        }
        match crate::menus::invoke(app, ctx, "reverseEngineering.applyLook", params) {
            Ok(_) => close = true,
            Err(e) => {
                app.ui.status = e.clone();
                ctx.data_mut(|d| d.insert_temp(egui::Id::new("reverse-tools-error"), e));
            }
        }
    }
    if close {
        ctx.data_mut(|d| d.remove::<PendingLookFile>(pending_file_id()));
    }
}

pub(super) fn command(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str, params: &Value) -> Result<Value, String> {
    if id == "reverseEngineering.cancelLook" {
        if awaiting_shots(ctx) {
            ctx.data_mut(|d| d.remove::<bool>(after_shots_id()));
            super::reverse_shots::cancel(ctx);
        }
        if let Some(job) = ctx.data(|d| d.get_temp::<Job>(job_id())) {
            job.cancel.store(true, Ordering::Relaxed);
        }
        return Ok(json!({"cancelled":true}));
    }
    if is_running(ctx) {
        return Err("Wait for the look analysis to finish or cancel it first".into());
    }
    match id {
        "reverseEngineering.runLook" => start(app, ctx),
        "reverseEngineering.lookShot" => {
            let index = params.get("index").and_then(Value::as_u64).and_then(|v| usize::try_from(v).ok()).ok_or("Supply a shot index")?;
            if !current(app) {
                return Err("Analyse the look for the current range before selecting a shot".into());
            }
            select(app, ctx, index)
        }
        "reverseEngineering.setLookOptions" => {
            let state = &mut app.ui.panels.reverse_engineering;
            if let Some(v) = params.get("paletteSize") {
                state.look_palette_size =
                    v.as_u64().and_then(|v| usize::try_from(v).ok()).filter(|n| (1..=look::MAX_PALETTE).contains(n)).ok_or("paletteSize must be 1–16")?;
            }
            if let Some(v) = params.get("maxSamples") {
                state.look_max_samples =
                    v.as_u64().and_then(|v| usize::try_from(v).ok()).filter(|n| (1..=MAX_SAMPLES).contains(n)).ok_or("maxSamples must be 1–240")?;
            }
            Ok(json!({"paletteSize":state.look_palette_size,"maxSamples":state.look_max_samples}))
        }
        "reverseEngineering.exportLook" => export_look(app, params),
        "reverseEngineering.applyLook" => apply_look(app, params),
        "reverseEngineering.chooseLookFile" => choose_look_file(app, ctx, params),
        "reverseEngineering.copyLookDescription" => copy_description(app, ctx),
        _ => Err(format!("Unknown look command: {id}")),
    }
}

fn invoke(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str, params: Value) {
    if let Err(e) = crate::menus::invoke(app, ctx, id, params) {
        app.ui.status = e.clone();
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("reverse-tools-error"), e));
    } else {
        ctx.data_mut(|d| d.remove::<String>(egui::Id::new("reverse-tools-error")));
    }
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

fn swatches(app: &mut FilmcraftApp, ui: &mut egui::Ui, grade: &Grade, prefix: &str) {
    ui.horizontal_wrapped(|ui| {
        for (i, c) in grade.palette.iter().enumerate() {
            let (rect, response) = ui.allocate_exact_size(egui::vec2(24.0, 20.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 3.0, egui::Color32::from_rgb(c.rgb[0], c.rgb[1], c.rgb[2]));
            ui.painter().rect_stroke(rect, 3.0, egui::Stroke::new(1.0, egui::Color32::from_gray(90)), egui::StrokeKind::Inside);
            response.on_hover_text(format!("{} · {} · {:.1}%", c.name, c.hex, c.share * 100.0));
            app.auto.add(&format!("{prefix}.palette.{i}"), rect, &c.hex);
        }
    });
}

fn numbers(ui: &mut egui::Ui, g: &Grade) {
    ui.label(format!(
        "Luma {:.2} ({}) · black {:.2} · white {:.2} · contrast {:.2} ({}) · variation {:.0} %",
        g.mean_luma,
        g.key,
        g.black_point,
        g.white_point,
        g.contrast,
        g.contrast_class,
        g.luma_variation * 100.0
    ));
    ui.label(format!(
        "Saturation {:.2} ({}) · warm bias {:+.2} ({}) · vignette ratio {:.2} ({}) · grain {:.3} ({}) · sharpness {:.3} ({}) · {} samples",
        g.saturation,
        g.saturation_class,
        g.warm_bias,
        g.temperature,
        g.vignette,
        g.vignette_class,
        g.grain,
        g.texture,
        g.sharpness,
        g.sharpness_class,
        g.samples
    ));
}

/// The Look page: header, verdict, actions, shots and the selected shot's keyframe.
pub(super) fn page(app: &mut FilmcraftApp, ui: &mut egui::Ui, bounds: egui::Rect) {
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(bounds));
    child.set_clip_rect(bounds.intersect(ui.clip_rect()));
    child.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
    egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt("reverse-look-page").show(&mut child, |ui| {
        ui.set_width(bounds.width().max(1.0));
        page_body(app, ui, bounds);
    });
    file_chooser(app, ui.ctx());
}

fn page_body(app: &mut FilmcraftApp, ui: &mut egui::Ui, bounds: egui::Rect) {
    let running = ui.ctx().data(|d| d.get_temp::<Job>(job_id()));
    let awaiting = awaiting_shots(ui.ctx());
    let shots_current = super::reverse_shots::current(app);
    let captures = reverse_engineering::range_captures(app, MAX_RANGE_FRAMES);
    let count = captures.as_ref().map(|c| c.len()).unwrap_or(0);
    let is_current = current(app);
    let saved =
        app.ui.panels.reverse_engineering.saved_results.iter().any(|r| r.tool() == "look" && reverse_engineering::captures_are_current(app, r.captures()));
    let has_report = app.ui.panels.reverse_engineering.look_report.is_some();
    let status = if awaiting {
        ("Detecting shots…", egui::Color32::from_rgb(90, 110, 170))
    } else if running.is_some() {
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
    let fps = frame_rate(app);
    let context = {
        let state = &app.ui.panels.reverse_engineering;
        match captures.as_ref().ok().and_then(|c| c.first()) {
            Some(first) => format!("{} · frames {}–{} · {count} frames · {fps:.3} fps", first.name, state.start_frame, state.end_frame),
            None => String::new(),
        }
    };
    let busy = running.is_some() || awaiting;
    ui.horizontal(|ui| {
        let response = ui.add_enabled(!busy, egui::Button::new("← Tools"));
        app.auto.add("reverseEngineering.tool.back", response.rect, "Back to tool menu");
        if response.clicked() {
            invoke(app, ui.ctx(), "reverseEngineering.chooseTool", json!({"tool":"menu"}));
        }
        ui.heading("Look");
        let response = badge(ui, status.0, status.1);
        app.auto.add("reverseEngineering.status", response.rect, status.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(&context).color(app.tokens.text_faint));
        });
    });
    if awaiting {
        ui.horizontal(|ui| {
            ui.spinner();
            let text = match super::reverse_shots::progress(ui.ctx()) {
                Some((0, done, total)) => format!("Detecting shots first · reading frame {done} / {total}"),
                Some((_, done, _)) => format!("Detecting shots first · shot keyframe {done}"),
                None => "Detecting shots first…".to_string(),
            };
            ui.label(text);
            let response = ui.button("Cancel");
            app.auto.add("reverseEngineering.cancelLook", response.rect, "Cancel the shot detection and the look analysis");
            if response.clicked() {
                invoke(app, ui.ctx(), "reverseEngineering.cancelLook", json!({}));
            }
        });
        ui.label(egui::RichText::new("The look is graded shot by shot, so the range's shots are detected first.").color(app.tokens.text_faint));
        return;
    }
    if let Some(job) = running {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(format!("Grading · frame {} / {}", job.progress.load(Ordering::Relaxed), job.total));
            let response = ui.button("Cancel");
            app.auto.add("reverseEngineering.cancelLook", response.rect, "Cancel look analysis");
            if response.clicked() {
                invoke(app, ui.ctx(), "reverseEngineering.cancelLook", json!({}));
            }
        });
        return;
    }
    if let Err(e) = &captures {
        ui.label(e);
    }
    if let Some(error) = ui.ctx().data(|d| d.get_temp::<String>(egui::Id::new("reverse-tools-error"))) {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
    }
    let report = app.ui.panels.reverse_engineering.look_report.clone().filter(|_| is_current);
    let (headline, quality) = match &report {
        Some(r) => (
            r.overall.summary.clone(),
            format!(
                "{} shot{} graded{} · {} frames sampled at {}×{}",
                r.shots.len(),
                if r.shots.len() == 1 { "" } else { "s" },
                if r.from_shots { "" } else { " (shots not detected: the whole range as one)" },
                r.overall.samples,
                r.analysis_size[0],
                r.analysis_size[1]
            ),
        ),
        None if has_report => {
            ("Options, range or project changed: analyse again to see the look.".to_string(), "The previous result is kept hidden until then.".to_string())
        }
        None => (
            "Not analysed yet.".to_string(),
            "Analyse finds the shots of the range and grades each one: tone, colour, palette, vignette, grain and sharpness.".to_string(),
        ),
    };
    egui::Frame::group(ui.style()).inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let response = ui.label(egui::RichText::new(&headline).strong().size(15.0));
        app.auto.add("reverseEngineering.verdict", response.rect, &headline);
        let response = ui.label(egui::RichText::new(&quality).color(app.tokens.text_dim));
        app.auto.add("reverseEngineering.quality", response.rect, &quality);
        if let Some(r) = &report {
            swatches(app, ui, &r.overall, "reverseEngineering.look.overall");
            ui.horizontal_wrapped(|ui| {
                let response = ui.button("Copy description");
                app.auto.add(
                    "reverseEngineering.copyLookDescription",
                    response.rect,
                    "Copy the complete look description as JSON: measurements per shot, the rules behind the words and the Lumetri translation",
                );
                if response.clicked() {
                    invoke(app, ui.ctx(), "reverseEngineering.copyLookDescription", json!({}));
                }
                ui.label(
                    egui::RichText::new("the words above are labels; the numbers and their rules travel in the description")
                        .color(app.tokens.text_faint)
                        .size(11.0),
                );
            });
        }
    });
    ui.add_space(6.0);
    let selected_clips = selected_video_clips(app).map(|c| c.len()).unwrap_or(0);
    ui.horizontal_wrapped(|ui| {
        if report.is_some() {
            let open = app.ui.panels.reverse_engineering.details_open;
            let response = ui.selectable_label(open, if open { "▾ Details" } else { "▸ Details" });
            app.auto.add("reverseEngineering.details", response.rect, "Every number behind the words");
            if response.clicked() {
                app.ui.panels.reverse_engineering.details_open = !open;
            }
        }
        let open = app.ui.panels.reverse_engineering.advanced_open;
        let response = ui.selectable_label(open, if open { "▾ Advanced" } else { "▸ Advanced" });
        app.auto.add("reverseEngineering.advanced", response.rect, "Palette size and how many frames are sampled");
        if response.clicked() {
            app.ui.panels.reverse_engineering.advanced_open = !open;
        }
        ui.separator();
        let label = if is_current { "Analyse again" } else { "Analyse" };
        let response = ui.add_enabled(count > 0, egui::Button::new(egui::RichText::new(label).strong()));
        app.auto.add(
            "reverseEngineering.runLook",
            response.rect,
            if shots_current {
                "Grade every shot of the range: samples its frames and reads tone, colour, palette, vignette, grain and sharpness"
            } else {
                "Detect the shots of the range, then grade every one of them"
            },
        );
        if response.clicked() {
            invoke(app, ui.ctx(), "reverseEngineering.runLook", json!({}));
        }
        if is_current {
            let label = if selected_clips > 0 {
                format!("Apply to {selected_clips} selected clip{}", if selected_clips == 1 { "" } else { "s" })
            } else {
                "Apply over In → Out".to_string()
            };
            let response = ui.button(label);
            app.auto.add(
                "reverseEngineering.look.apply",
                response.rect,
                if selected_clips > 0 {
                    "Transfer the whole-range look onto the selected clips as a colour-transfer LUT, recorded as a component"
                } else {
                    "Put the whole-range look on an adjustment layer over In → Out (or the analysed range)"
                },
            );
            if response.clicked() {
                invoke(app, ui.ctx(), "reverseEngineering.applyLook", json!({}));
            }
        }
        if is_current {
            let response = ui.button("Export look…");
            app.auto.add(
                "reverseEngineering.look.export",
                response.rect,
                "Save the grade of every shot, the rules and the Lumetri translation as a reusable look file",
            );
            if response.clicked() {
                invoke(app, ui.ctx(), "reverseEngineering.exportLook", json!({}));
            }
        }
        let response = ui.button("Apply look file…");
        app.auto.add(
            "reverseEngineering.look.applyFile",
            response.rect,
            "Open a saved look file and choose which of its looks to apply, to the selected clips or over a time range as an adjustment layer",
        );
        if response.clicked() {
            invoke(app, ui.ctx(), "reverseEngineering.chooseLookFile", json!({}));
        }
        if selected_clips == 0 {
            ui.label(
                egui::RichText::new("No clips selected: looks go on an adjustment layer over In → Out (or the analysed range).").color(app.tokens.text_faint),
            );
        }
    });
    if is_current {
        super::reverse_tools::result_actions(app, ui, "look", false);
    }
    if app.ui.panels.reverse_engineering.advanced_open {
        let mut palette = app.ui.panels.reverse_engineering.look_palette_size;
        let mut samples = app.ui.panels.reverse_engineering.look_max_samples;
        ui.horizontal_wrapped(|ui| {
            ui.label("Palette colours");
            let r = ui.add(egui::DragValue::new(&mut palette).range(1..=look::MAX_PALETTE));
            app.auto.add("reverseEngineering.look.paletteSize", r.rect, "Colours in each palette");
            ui.label("Sampled frames");
            let s = ui.add(egui::DragValue::new(&mut samples).range(1..=MAX_SAMPLES));
            app.auto.add("reverseEngineering.look.maxSamples", s.rect, "Frames rendered over the whole range, spread across the shots");
            if r.changed() || s.changed() {
                invoke(app, ui.ctx(), "reverseEngineering.setLookOptions", json!({"paletteSize":palette,"maxSamples":samples}));
            }
        });
    }
    let Some(report) = report else { return };
    ui.add_space(6.0);
    let selected = app.ui.panels.reverse_engineering.look_selected.min(report.shots.len().saturating_sub(1));
    let details = app.ui.panels.reverse_engineering.details_open;
    let wide = bounds.width() >= 700.0;
    let list = |app: &mut FilmcraftApp, ui: &mut egui::Ui| {
        for shot in &report.shots {
            let title = match &shot.grade {
                Some(g) => format!(
                    "Shot {} · frames {}–{} · {}, {} contrast, {}, {}",
                    shot.index + 1,
                    shot.start_frame + 1,
                    shot.end_frame + 1,
                    g.key,
                    g.contrast_class,
                    g.saturation_class,
                    g.temperature
                ),
                None => format!("Shot {} · frames {}–{} · black", shot.index + 1, shot.start_frame + 1, shot.end_frame + 1),
            };
            let response = ui.selectable_label(selected == shot.index, &title);
            app.auto.add(&format!("reverseEngineering.look.shot.{}", shot.index), response.rect, &title);
            if response.clicked() {
                invoke(app, ui.ctx(), "reverseEngineering.lookShot", json!({"index":shot.index}));
            }
            if let Some(g) = &shot.grade {
                swatches(app, ui, g, &format!("reverseEngineering.look.shot.{}", shot.index));
                if selected == shot.index {
                    ui.label(egui::RichText::new(&g.summary).color(app.tokens.text_dim).size(11.5));
                    if details {
                        numbers(ui, g);
                    }
                }
            }
        }
    };
    let keyframe = report.shots.get(selected).and_then(|s| report.captures.get(s.keyframe)).cloned();
    if wide {
        let gap = 12.0;
        let left_w = ((bounds.width() - gap) * 0.55).max(1.0);
        let right_w = (bounds.width() - gap - left_w).max(1.0);
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(left_w);
                list(app, ui);
            });
            ui.add_space(gap);
            ui.vertical(|ui| {
                ui.set_width(right_w);
                if let Some(k) = &keyframe {
                    reverse_engineering::picture(app, ui, k, right_w.min(480.0) as u32, "reverseEngineering.look.keyframe", false);
                    let caption = format!("Shot {} keyframe · {}", selected + 1, reverse_engineering::timecode(app, k));
                    ui.label(egui::RichText::new(caption).color(app.tokens.text_faint).size(11.0));
                }
            });
        });
    } else {
        if let Some(k) = &keyframe {
            reverse_engineering::picture(app, ui, k, bounds.width().min(480.0) as u32, "reverseEngineering.look.keyframe", false);
        }
        list(app, ui);
    }
    if details {
        ui.separator();
        ui.label(egui::RichText::new("Whole range").strong());
        numbers(ui, &report.overall);
        for l in &report.limitations {
            ui.label(egui::RichText::new(l).color(app.tokens.text_faint).size(11.0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_frames_spread_evenly_and_keep_the_keyframe_when_alone() {
        assert_eq!(sample_frames(10, 19, 14, 1), vec![14]);
        assert_eq!(sample_frames(10, 19, 14, 4), vec![10, 13, 16, 19]);
        assert_eq!(sample_frames(5, 5, 5, 9), vec![5]);
        assert_eq!(sample_frames(0, 2, 1, 3), vec![0, 1, 2]);
        assert_eq!(sample_frames(0, 9, 30, 1), vec![9], "an out-of-range keyframe is clamped");
    }
}

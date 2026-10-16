//! Shot structure and per-shot look from rendered timeline frames. Pure Rust; no model and no project edits.
use super::{panel_state::ReverseEngineeringCapture, reverse_engineering};
use crate::FilmcraftApp;
use filmcraft_vision::{
    look::{self, Look},
    shots::{self, FrameChange, Options, Shot, Signature},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

pub const MAX_RANGE_FRAMES: usize = shots::MAX_FRAMES;
const SIGNATURE_SIDE: f32 = 160.0;
const LOOK_SIDE: f32 = 320.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShotReport {
    pub schema_version: u32,
    pub analyzer: String,
    pub captures: Vec<ReverseEngineeringCapture>,
    pub project_revision: u64,
    pub completed_at_unix_ms: u64,
    pub ticks_per_second: i64,
    pub timeline_frame_rate: Value,
    pub source_size: [u32; 2],
    pub signature_size: [usize; 2],
    pub options: Options,
    pub palette_size: usize,
    pub changes: Vec<FrameChange>,
    pub shots: Vec<Shot>,
    pub looks: Vec<Option<Look>>,
    pub limitations: Vec<String>,
}
#[derive(Clone)]
struct Job {
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicUsize>,
    total: usize,
    stage: Arc<AtomicUsize>,
    result: Arc<Mutex<Option<Result<ShotReport, String>>>>,
}
fn job_id() -> egui::Id {
    egui::Id::new("reverse-shots-job")
}
pub(super) fn is_running(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<Job>(job_id())).is_some()
}
pub(super) fn current(app: &FilmcraftApp) -> bool {
    let s = &app.ui.panels.reverse_engineering;
    s.shots.as_ref().is_some_and(|r| {
        r.project_revision == app.session.revision
            && r.options == s.shot_options
            && r.palette_size == s.shot_palette_size
            && r.shots.len() == r.looks.len()
            && reverse_engineering::captures_are_current(app, &r.captures)
    })
}
fn limitations() -> Vec<String> {
    vec![
        "Cuts come from histogram and downscaled-luma differences between consecutive rendered frames with an adaptive threshold; gradual dissolves, flashes, fast motion and strobing can be missed or over-split.".into(),
        "Black runs split shots; a black shot is a fade, gap or slate, not a scene description. Held (duplicate) frames are measured against the previous frame only.".into(),
        "Look statistics describe one keyframe per shot at reduced resolution. Palette is a deterministic k-means of sampled pixels; pixel pitch and letterbox are heuristics, not style recognition.".into(),
        "No object, text, audio or camera-pose interpretation is produced by this tool.".into(),
    ]
}
fn timestamp() -> Result<u64, String> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())
        .and_then(|d| u64::try_from(d.as_millis()).map_err(|e| e.to_string()))
}
pub(super) fn poll(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let Some(job) = ctx.data(|d| d.get_temp::<Job>(job_id())) else {
        return;
    };
    let result = job.result.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
    if let Some(result) = result {
        ctx.data_mut(|d| d.remove::<Job>(job_id()));
        match result {
            Ok(report) if !job.cancel.load(Ordering::Relaxed) => {
                let selected = app.ui.panels.reverse_engineering.shot_selected.min(report.shots.len().saturating_sub(1));
                app.ui.panels.reverse_engineering.shots = Some(report);
                let _ = select(app, ctx, selected);
                app.ui.status = "Shot structure ready; inspect, adjust thresholds or save the result.".into();
            }
            Ok(_) => app.ui.status = "Shot detection cancelled; previous results preserved".into(),
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
    let report = app.ui.panels.reverse_engineering.shots.as_ref().ok_or("Detect shots first")?;
    let shot = report.shots.get(index).ok_or("Shot index outside the result")?;
    let capture = report.captures.get(shot.keyframe).cloned().ok_or("Shot keyframe outside the range")?;
    let start = report.captures.get(shot.start_frame).map(|c| c.timeline_time).ok_or("Shot start outside the range")?;
    let (start_frame, end_frame) = (shot.start_frame, shot.end_frame);
    app.ui.panels.reverse_engineering.shot_selected = index;
    app.ui.panels.reverse_engineering.capture = Some(capture);
    app.stop();
    app.session.set_playhead(start);
    reverse_engineering::reset_decode_timer(ctx);
    Ok(json!({"index":index,"startFrame":start_frame,"endFrame":end_frame}))
}
fn start(app: &mut FilmcraftApp, ctx: &egui::Context, base: Option<ShotReport>) -> Result<Value, String> {
    if is_running(ctx) {
        return Err("Shot detection is already running".into());
    }
    if cfg!(target_arch = "wasm32") {
        return Err("Shot detection currently requires the desktop app.".into());
    }
    let captures = reverse_engineering::range_captures(app, MAX_RANGE_FRAMES)?;
    if let Some(base) = base.as_ref()
        && (base.captures != captures || base.changes.len() != captures.len())
    {
        return Err("Previous shot evidence does not match the selected range".into());
    }
    let first = captures.first().ok_or("No frames selected")?;
    let (width, height) = app.session.project.source_size(first.item).ok_or("No video dimensions")?;
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 67_108_864 {
        return Err("Invalid or oversized source dimensions".into());
    }
    let state = &app.ui.panels.reverse_engineering;
    let options = state.shot_options;
    options.validate()?;
    let palette_size = state.shot_palette_size;
    if !(1..=look::MAX_PALETTE).contains(&palette_size) {
        return Err("Palette size must be 1–16 colours".into());
    }
    let rate = app.session.active_sequence().ok_or("No active sequence")?.settings.frame_rate;
    let revision = app.session.revision;
    let project = app.session.project.clone();
    let provider = app.session.media.provider(project.clone(), app.session.services.clone());
    let timeline_frame_rate = serde_json::to_value(rate).map_err(|e| e.to_string())?;
    let job = Job {
        cancel: Arc::new(AtomicBool::new(false)),
        progress: Arc::new(AtomicUsize::new(0)),
        total: captures.len(),
        stage: Arc::new(AtomicUsize::new(0)),
        result: Arc::new(Mutex::new(None)),
    };
    let worker = job.clone();
    let repaint = ctx.clone();
    std::thread::Builder::new()
        .name("filmcraft-shots".into())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<ShotReport, String> {
                let started = std::time::Instant::now();
                let cancelled = || worker.cancel.load(Ordering::Relaxed);
                let render = |capture: &ReverseEngineeringCapture, side: f32| -> Result<(usize, usize, Vec<u8>), String> {
                    let scale = (side / width.max(height) as f32).min(1.0);
                    let image = filmcraft_render::render_item(&project, capture.item, capture.media_time, scale, &provider)
                        .ok_or("A timeline frame could not be rendered. Check the media and retry.")?;
                    Ok((image.w, image.h, image.to_rgba8()))
                };
                let (changes, signature_size) = match base.as_ref() {
                    Some(base) => (base.changes.clone(), base.signature_size),
                    None => {
                        let mut signatures: Vec<Signature> = Vec::new();
                        signatures.try_reserve_exact(captures.len()).map_err(|e| format!("Cannot allocate shot evidence: {e}"))?;
                        let mut size = None;
                        for (i, capture) in captures.iter().enumerate() {
                            if cancelled() {
                                return Err("Shot detection cancelled".into());
                            }
                            if started.elapsed().as_secs() > 3600 {
                                return Err("Shot detection timed out; choose a shorter range".into());
                            }
                            let (w, h, rgba) = render(capture, SIGNATURE_SIDE)?;
                            if size.is_some_and(|s| s != [w, h]) {
                                return Err("Rendered frame dimensions changed during analysis".into());
                            }
                            size = Some([w, h]);
                            signatures.push(shots::signature(w, h, &rgba)?);
                            worker.progress.store(i + 1, Ordering::Relaxed);
                            repaint.request_repaint();
                        }
                        let ticks: Vec<i64> = captures.iter().map(|c| c.timeline_time.0).collect();
                        (shots::changes(&signatures, &ticks, &options)?, size.ok_or("No frames rendered")?)
                    }
                };
                let detected = shots::detect(&changes, &options)?;
                let mut looks: Vec<Option<Look>> = Vec::new();
                looks.try_reserve_exact(detected.len()).map_err(|e| format!("Cannot allocate look results: {e}"))?;
                worker.stage.store(1, Ordering::Relaxed);
                worker.progress.store(0, Ordering::Relaxed);
                for (i, shot) in detected.iter().enumerate() {
                    if cancelled() {
                        return Err("Shot detection cancelled".into());
                    }
                    let reused = base
                        .as_ref()
                        .filter(|b| b.palette_size == palette_size)
                        .and_then(|b| b.shots.iter().position(|s| s.keyframe == shot.keyframe && s.black == shot.black).and_then(|p| b.looks.get(p).cloned()))
                        .flatten();
                    let look = if shot.black {
                        None
                    } else if let Some(look) = reused {
                        Some(look)
                    } else {
                        let capture = captures.get(shot.keyframe).ok_or("Keyframe outside the range")?;
                        let (w, h, rgba) = render(capture, LOOK_SIDE)?;
                        Some(look::analyze(w, h, &rgba, palette_size)?)
                    };
                    looks.push(look);
                    worker.progress.store(i + 1, Ordering::Relaxed);
                    repaint.request_repaint();
                }
                Ok(ShotReport {
                    schema_version: 1,
                    analyzer: "filmcraft-vision/shots-and-look-v1".into(),
                    captures,
                    project_revision: revision,
                    completed_at_unix_ms: timestamp()?,
                    ticks_per_second: filmcraft_time::TICKS_PER_SECOND,
                    timeline_frame_rate,
                    source_size: [width, height],
                    signature_size,
                    options,
                    palette_size,
                    changes,
                    shots: detected,
                    looks,
                    limitations: limitations(),
                })
            }))
            .unwrap_or_else(|_| Err("The shot worker failed; previous results preserved".into()));
            *worker.result.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(outcome);
            repaint.request_repaint();
        })
        .map_err(|e| format!("Could not start shot detection: {e}"))?;
    ctx.data_mut(|d| d.insert_temp(job_id(), job));
    Ok(json!({"started":true}))
}
pub(super) fn command(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str, params: &Value) -> Result<Value, String> {
    if id == "reverseEngineering.cancelShots" {
        if let Some(job) = ctx.data(|d| d.get_temp::<Job>(job_id())) {
            job.cancel.store(true, Ordering::Relaxed);
        }
        return Ok(json!({"cancelled":true}));
    }
    if is_running(ctx) {
        return Err("Wait for shot detection to finish or cancel it first".into());
    }
    if id == "reverseEngineering.shotSelect" {
        let index = params.get("index").and_then(Value::as_u64).and_then(|v| usize::try_from(v).ok()).ok_or("Supply a shot index")?;
        if !current(app) {
            return Err("Detect shots for the current range before selecting one".into());
        }
        return select(app, ctx, index);
    }
    if id == "reverseEngineering.setShotOptions" {
        let s = &app.ui.panels.reverse_engineering;
        let number = |key: &str, current: f32| -> Result<f32, String> {
            match params.get(key) {
                Some(v) => v.as_f64().map(|v| v as f32).filter(|v| v.is_finite()).ok_or_else(|| format!("{key} must be a finite number")),
                None => Ok(current),
            }
        };
        let options = Options {
            cut_threshold: number("cutThreshold", s.shot_options.cut_threshold)?,
            adaptive_ratio: number("adaptiveRatio", s.shot_options.adaptive_ratio)?,
            min_shot_frames: match params.get("minShotFrames") {
                Some(v) => v.as_u64().and_then(|v| usize::try_from(v).ok()).ok_or("minShotFrames must be an integer")?,
                None => s.shot_options.min_shot_frames,
            },
            black_luma: number("blackLuma", s.shot_options.black_luma)?,
            duplicate_tolerance: number("duplicateTolerance", s.shot_options.duplicate_tolerance)?,
        };
        options.validate()?;
        let palette_size = match params.get("paletteSize") {
            Some(v) => v.as_u64().and_then(|v| usize::try_from(v).ok()).filter(|n| (1..=look::MAX_PALETTE).contains(n)).ok_or("paletteSize must be 1–16")?,
            None => s.shot_palette_size,
        };
        let state = &mut app.ui.panels.reverse_engineering;
        state.shot_options = options;
        state.shot_palette_size = palette_size;
        let existing = state.shots.clone();
        let reusable = existing.filter(|r| r.project_revision == app.session.revision && reverse_engineering::captures_are_current(app, &r.captures));
        if let Some(base) = reusable {
            start(app, ctx, Some(base))?;
            return Ok(json!({"options":options,"paletteSize":palette_size,"redetecting":true}));
        }
        return Ok(json!({"options":options,"paletteSize":palette_size,"redetecting":false}));
    }
    if id == "reverseEngineering.runShots" {
        return start(app, ctx, None);
    }
    Err(format!("Unknown shot command: {id}"))
}
fn invoke(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str, params: Value) {
    if let Err(e) = crate::menus::invoke(app, ctx, id, params) {
        app.ui.status = e.clone();
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("reverse-tools-error"), e));
    } else {
        ctx.data_mut(|d| d.remove::<String>(egui::Id::new("reverse-tools-error")));
    }
}
fn frame_rate(report: &ShotReport) -> f64 {
    let num = report.timeline_frame_rate.get("num").and_then(Value::as_f64).unwrap_or(0.0);
    let den = report.timeline_frame_rate.get("den").and_then(Value::as_f64).unwrap_or(1.0);
    if den > 0.0 && num > 0.0 { num / den } else { 0.0 }
}
fn seconds(report: &ShotReport, shot: &Shot) -> f64 {
    shot.frame_count as f64 / frame_rate(report).max(0.001)
}
pub(super) fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    ui.heading("Shots & look");
    ui.label("Cuts, black runs, held frames and one keyframe look per shot. Pure Rust, no model download.");
    if let Some(job) = ui.ctx().data(|d| d.get_temp::<Job>(job_id())) {
        ui.spinner();
        let done = job.progress.load(Ordering::Relaxed);
        ui.label(if job.stage.load(Ordering::Relaxed) == 0 {
            format!("Reading frames · {done} / {}", job.total)
        } else {
            format!("Analyzing shot keyframes · {done}")
        });
        let response = ui.button("Cancel detection");
        app.auto.add("reverseEngineering.cancelShots", response.rect, "Cancel shot detection");
        if response.clicked() {
            invoke(app, ui.ctx(), "reverseEngineering.cancelShots", json!({}));
        }
        return;
    }
    let mut options = app.ui.panels.reverse_engineering.shot_options;
    let mut palette = app.ui.panels.reverse_engineering.shot_palette_size;
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        ui.label("Cut threshold");
        let r = ui.add(egui::DragValue::new(&mut options.cut_threshold).range(0.01..=1.0).speed(0.01));
        app.auto.add("reverseEngineering.shots.cutThreshold", r.rect, "Minimum frame change score for a cut");
        changed |= r.changed();
        ui.label("× local median");
        let r = ui.add(egui::DragValue::new(&mut options.adaptive_ratio).range(1.0..=100.0).speed(0.1));
        app.auto.add("reverseEngineering.shots.adaptiveRatio", r.rect, "Cut score must exceed this multiple of nearby changes");
        changed |= r.changed();
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("Minimum shot frames");
        let r = ui.add(egui::DragValue::new(&mut options.min_shot_frames).range(1..=1000));
        app.auto.add("reverseEngineering.shots.minShotFrames", r.rect, "Shorter shots merge into the previous shot");
        changed |= r.changed();
        ui.label("Black luma");
        let r = ui.add(egui::DragValue::new(&mut options.black_luma).range(0.0..=0.5).speed(0.005));
        app.auto.add("reverseEngineering.shots.blackLuma", r.rect, "Mean luma at or below this is a black frame");
        changed |= r.changed();
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("Held-frame tolerance");
        let r = ui.add(egui::DragValue::new(&mut options.duplicate_tolerance).range(0.0..=0.5).speed(0.001));
        app.auto.add("reverseEngineering.shots.duplicateTolerance", r.rect, "Frame change at or below this counts as a held frame");
        changed |= r.changed();
        ui.label("Palette colours");
        let r = ui.add(egui::DragValue::new(&mut palette).range(1..=look::MAX_PALETTE));
        app.auto.add("reverseEngineering.shots.paletteSize", r.rect, "Colours in each shot palette");
        changed |= r.changed();
    });
    if changed {
        invoke(
            app,
            ui.ctx(),
            "reverseEngineering.setShotOptions",
            json!({"cutThreshold":options.cut_threshold,"adaptiveRatio":options.adaptive_ratio,"minShotFrames":options.min_shot_frames,"blackLuma":options.black_luma,"duplicateTolerance":options.duplicate_tolerance,"paletteSize":palette}),
        );
    }
    let is_current = current(app);
    let response = ui.button(if is_current { "Detect again" } else { "Detect shots" });
    app.auto.add("reverseEngineering.runShots", response.rect, "Render every frame of the range and detect shots");
    if response.clicked() {
        invoke(app, ui.ctx(), "reverseEngineering.runShots", json!({}));
    }
    if !is_current && app.ui.panels.reverse_engineering.shots.is_some() {
        ui.label("Range or project changed. Detect again; previous results are hidden.");
        return;
    }
    let Some(report) = app.ui.panels.reverse_engineering.shots.clone().filter(|_| is_current) else {
        return;
    };
    let black = report.shots.iter().filter(|s| s.black).count();
    let visible: Vec<&Shot> = report.shots.iter().filter(|s| !s.black).collect();
    let mean = if visible.is_empty() { 0.0 } else { visible.iter().map(|s| seconds(&report, s)).sum::<f64>() / visible.len() as f64 };
    let summary = format!(
        "{} shots · {} boundaries · {black} black · average shot {mean:.2} s · {} frames read",
        report.shots.len(),
        report.shots.len().saturating_sub(1),
        report.changes.len()
    );
    let response = ui.label(&summary);
    app.auto.add("reverseEngineering.shots.summary", response.rect, &summary);
    let selected = app.ui.panels.reverse_engineering.shot_selected.min(report.shots.len().saturating_sub(1));
    let Some(shot) = report.shots.get(selected) else {
        return;
    };
    ui.separator();
    ui.label(format!(
        "Shot {} of {} · frames {}–{} ({} frames, {:.2} s) · {}{}",
        selected + 1,
        report.shots.len(),
        shot.start_frame + 1,
        shot.end_frame + 1,
        shot.frame_count,
        seconds(&report, shot),
        format!("{:?}", shot.boundary).to_lowercase(),
        shot.boundary_score.map(|s| format!(" · change {s:.2}")).unwrap_or_default()
    ));
    if shot.black {
        ui.label("Black frames: fade, gap or slate.");
    } else if shot.frame_count > 1 {
        let effective = frame_rate(&report) * shot.unique_frames as f64 / shot.frame_count as f64;
        ui.label(format!(
            "Cadence: {} unique of {} frames · held {:.0}% · about {:.1} fps of new drawings",
            shot.unique_frames,
            shot.frame_count,
            shot.hold_fraction * 100.0,
            effective
        ));
    }
    if let Some(Some(look)) = report.looks.get(selected) {
        ui.label(format!(
            "{} · luma {:.2} · contrast {:.2} · {:.0}% near black · {:.0}% near white",
            look.key,
            look.mean_luma,
            look.luma_spread,
            look.dark_fraction * 100.0,
            look.bright_fraction * 100.0
        ));
        ui.label(format!(
            "Saturation {:.2} · warm bias {:+.2} · {} distinct colours{}",
            look.mean_saturation,
            look.warm_bias,
            look.unique_colors,
            look.pixel_pitch.map(|p| format!(" · pixel grid ≈ {p} px at {}×{}", look.analyzed_size[0], look.analyzed_size[1])).unwrap_or_default()
        ));
        if look.letterbox.iter().any(|v| *v > 0.01) {
            ui.label(format!(
                "Black bars: top {:.0}% · bottom {:.0}% · left {:.0}% · right {:.0}%",
                look.letterbox[0] * 100.0,
                look.letterbox[1] * 100.0,
                look.letterbox[2] * 100.0,
                look.letterbox[3] * 100.0
            ));
        }
        ui.horizontal_wrapped(|ui| {
            for (i, c) in look.palette.iter().enumerate() {
                let (rect, response) = ui.allocate_exact_size(egui::vec2(26.0, 22.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 3.0, egui::Color32::from_rgb(c.rgb[0], c.rgb[1], c.rgb[2]));
                ui.painter().rect_stroke(rect, 3.0, egui::Stroke::new(1.0, egui::Color32::from_gray(90)), egui::StrokeKind::Inside);
                response.on_hover_text(format!("{} · {:.1}%", c.hex, c.share * 100.0));
                app.auto.add(&format!("reverseEngineering.shots.palette.{i}"), rect, &c.hex);
            }
        });
        let hexes: Vec<String> = look.palette.iter().map(|c| format!("{} {:.0}%", c.hex, c.share * 100.0)).collect();
        ui.label(hexes.join(" · "));
    }
}
pub(super) fn controls(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    let Some(report) = app.ui.panels.reverse_engineering.shots.clone().filter(|_| current(app)) else {
        return;
    };
    let count = report.shots.len();
    if count == 0 {
        return;
    }
    let selected = app.ui.panels.reverse_engineering.shot_selected.min(count - 1);
    ui.horizontal_wrapped(|ui| {
        for (label, index, enabled) in [
            ("First", 0, selected > 0),
            ("Previous", selected.saturating_sub(1), selected > 0),
            ("Next", (selected + 1).min(count - 1), selected + 1 < count),
            ("Last", count - 1, selected + 1 < count),
        ] {
            let response = ui.add_enabled(enabled, egui::Button::new(label));
            app.auto.add(&format!("reverseEngineering.shots.{label}"), response.rect, label);
            if response.clicked() {
                invoke(app, ui.ctx(), "reverseEngineering.shotSelect", json!({"index":index}));
            }
        }
        let mut jump = selected + 1;
        let response = ui.add(egui::DragValue::new(&mut jump).range(1..=count));
        app.auto.add("reverseEngineering.shots.jump", response.rect, "Jump to shot");
        if response.changed() {
            invoke(app, ui.ctx(), "reverseEngineering.shotSelect", json!({"index":jump.saturating_sub(1)}));
        }
        ui.label(format!("of {count} shots"));
    });
    if let Some(shot) = report.shots.get(selected)
        && let Some(capture) = report.captures.get(shot.keyframe)
    {
        let label = format!("Keyframe {} of range · {}", shot.keyframe + 1, reverse_engineering::timecode(app, capture));
        let response = ui.label(label);
        app.auto.add("reverseEngineering.timecode", response.rect, "Exact timestamp of the displayed keyframe");
    }
    shot_bar(app, ui, &report, selected);
}
fn shot_color(index: usize, black: bool) -> egui::Color32 {
    if black {
        return egui::Color32::from_gray(40);
    }
    let hue = (index as f32 * 0.618_034) % 1.0;
    let [r, g, b] = egui::ecolor::Hsva::new(hue, 0.55, 0.75, 1.0).to_rgb();
    egui::Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}
fn shot_bar(app: &mut FilmcraftApp, ui: &mut egui::Ui, report: &ShotReport, selected: usize) {
    let total = report.changes.len().max(1) as f32;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width().max(1.0), 24.0), egui::Sense::click());
    app.auto.add("reverseEngineering.shots.bar", rect, "Shot structure; click a block to select its shot");
    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect, 2.0, egui::Color32::from_gray(25));
    let x_of = |frame: usize| rect.left() + rect.width() * frame as f32 / total;
    let mut hovered = None;
    for (i, shot) in report.shots.iter().enumerate() {
        let block = egui::Rect::from_min_max(
            egui::pos2(x_of(shot.start_frame), rect.top()),
            egui::pos2(x_of(shot.end_frame + 1).max(x_of(shot.start_frame) + 1.0), rect.bottom()),
        );
        painter.rect_filled(block.shrink2(egui::vec2(0.5, 2.0)), 2.0, shot_color(i, shot.black));
        if i == selected {
            painter.rect_stroke(block, 2.0, egui::Stroke::new(2.0, egui::Color32::WHITE), egui::StrokeKind::Inside);
        }
        if response.hover_pos().is_some_and(|p| block.contains(p)) {
            hovered = Some(i);
        }
    }
    if let Some(i) = hovered
        && let Some(shot) = report.shots.get(i)
    {
        response.clone().on_hover_text(format!("Shot {} · frames {}–{} · {:.2} s", i + 1, shot.start_frame + 1, shot.end_frame + 1, seconds(report, shot)));
        if response.clicked() {
            invoke(app, ui.ctx(), "reverseEngineering.shotSelect", json!({"index":i}));
        }
    }
}
pub(super) fn result(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    let Some(report) = app.ui.panels.reverse_engineering.shots.clone().filter(|_| current(app)) else {
        return;
    };
    let selected = app.ui.panels.reverse_engineering.shot_selected.min(report.shots.len().saturating_sub(1));
    let Some(capture) = report.shots.get(selected).and_then(|s| report.captures.get(s.keyframe)).cloned() else {
        return;
    };
    reverse_engineering::selected_picture(app, ui, &capture, 104.0);
    ui.add_space(8.0);
    let cell_width = 96.0;
    let count = report.shots.len();
    egui::ScrollArea::horizontal().auto_shrink([false, true]).id_salt("reverse-shots-strip").max_height(92.0).show_viewport(ui, |ui, viewport| {
        ui.set_min_width(count as f32 * cell_width);
        let first = ((viewport.min.x.max(0.0) / cell_width).floor() as usize).min(count);
        let last = first.saturating_add((viewport.width().max(0.0) / cell_width).ceil() as usize).saturating_add(2).min(count);
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.horizontal(|ui| {
            if first > 0 {
                ui.allocate_space(egui::vec2((first as f32 * cell_width - 8.0).max(0.0), 76.0));
            }
            for index in first..last {
                let Some(shot) = report.shots.get(index) else { continue };
                let Some(keyframe) = report.captures.get(shot.keyframe) else { continue };
                ui.vertical(|ui| {
                    ui.set_width(88.0);
                    let response = ui.add(egui::Button::new(format!("{} · {:.1}s", index + 1, seconds(&report, shot))).selected(selected == index));
                    app.auto.add(&format!("reverseEngineering.shots.shot.{index}"), response.rect, "Select shot");
                    let clicked = reverse_engineering::picture(app, ui, keyframe, 96, &format!("reverseEngineering.shots.thumbnail.{index}"), true);
                    if response.clicked() || clicked {
                        invoke(app, ui.ctx(), "reverseEngineering.shotSelect", json!({"index":index}));
                    }
                });
            }
        });
    });
}

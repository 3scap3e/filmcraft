//! Rust orchestration for the isolated local SEA-RAFT inference runtime. No project mutations.
use super::panel_state::ReverseEngineeringCapture;
use crate::FilmcraftApp;
use filmcraft_vision::display::{MotionDisplay, MotionDisplays};
use filmcraft_vision::{MotionPair, MotionVector, SeaRaft};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MotionReport {
    pub schema_version: u32,
    pub analyzer: String,
    pub settings: Value,
    pub timeline_frame_rate: Value,
    pub source_frame_rate: Value,
    pub source_size: [u32; 2],
    pub ticks_per_second: i64,
    pub captures: Vec<ReverseEngineeringCapture>,
    pub pairs: Vec<MotionPair>,
    pub limitations: Vec<String>,
    #[serde(default)]
    pub display: MotionDisplays,
    #[serde(default)]
    pub audit: filmcraft_vision::audit::AnalysisAudit,
}
#[derive(Clone)]
struct Job {
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicUsize>,
    total: usize,
    result: Arc<Mutex<Option<Result<MotionReport, String>>>>,
    started: web_time::Instant,
    replacement: Option<(Arc<MotionReport>, usize)>,
}
fn job_id() -> egui::Id {
    egui::Id::new("reverse-motion-job")
}
fn frame_id() -> egui::Id {
    egui::Id::new("reverse-motion-frame")
}
pub(super) fn is_running(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<Job>(job_id())).is_some()
}
fn index_id() -> egui::Id {
    egui::Id::new("reverse-motion-index")
}
fn sample_id() -> egui::Id {
    egui::Id::new("reverse-motion-selected-sample")
}
fn error_id() -> egui::Id {
    egui::Id::new("reverse-motion-error")
}

pub fn start(app: &mut FilmcraftApp, ctx: &egui::Context, captures: Vec<ReverseEngineeringCapture>) -> Result<Value, String> {
    start_job(app, ctx, captures, None)
}
fn start_job(
    app: &mut FilmcraftApp,
    ctx: &egui::Context,
    captures: Vec<ReverseEngineeringCapture>,
    replacement: Option<(MotionReport, usize)>,
) -> Result<Value, String> {
    if !(2..=300).contains(&captures.len()) {
        return Err("Motion analysis needs 2–300 frames.".into());
    }
    if ctx.data(|d| d.get_temp::<Job>(job_id())).is_some() {
        return Err("Cancel the current motion analysis before starting another.".into());
    }
    if super::reverse_tools::is_running(ctx) {
        return Err("Cancel global fitting before starting optical-flow analysis".into());
    }
    if cfg!(target_arch = "wasm32") {
        return Err("Local motion analysis currently requires the desktop app.".into());
    }
    let first = captures.first().ok_or("No frames selected")?;
    let source = app.session.project.item(first.item).ok_or("Source is no longer available")?;
    let (width, height) = app.session.project.source_size(first.item).ok_or("No video dimensions")?;
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 67_108_864 {
        return Err("Invalid or oversized source dimensions".into());
    }
    let device = app.ui.panels.reverse_engineering.flow_device;
    let rate = app.session.active_sequence().ok_or("No active sequence")?.settings.frame_rate;
    let report = MotionReport {
        schema_version: 5,
        analyzer: "filmcraft-vision/sea-raft-M".into(),
        settings: json!({"model":"SEA-RAFT M","maximumAnalysisDimension":640,"inference":"dense optical flow","exportGridSpacing":16,"vectorExport":"samples of dense model flow","refinementIterations":4,"timelineFrameStep":1,"coordinateSpace":"normalized rendered timeline image","appearanceChangeThreshold":2,"timelineProjectRevision":app.session.revision}),
        timeline_frame_rate: serde_json::to_value(rate).map_err(|e| e.to_string())?,
        source_frame_rate: serde_json::to_value(source.frame_rate()).map_err(|e| e.to_string())?,
        source_size: [width, height],
        ticks_per_second: filmcraft_time::TICKS_PER_SECOND,
        captures: captures.clone(),
        pairs: Vec::new(),
        display: app.ui.panels.reverse_engineering.motion.as_ref().filter(|old| old.captures==captures).map(|old| old.display.clone()).unwrap_or_default(),
        audit: app.ui.panels.reverse_engineering.motion.as_ref().filter(|old|old.captures==captures).map(|old|old.audit.clone()).unwrap_or_default(),
        limitations: vec![
            "Measures image motion, not physical camera angles or camera/subject separation.".into(),
            "Independent frame-pair vectors, not persistent object identities.".into(),
            "Learned flow is an estimate; cuts, occlusion, flat regions and changing illumination can produce errors. JSON vectors sample dense predictions on a 16-pixel grid; uncertainty is a model log-scale estimate, not a calibrated confidence probability."
                .into(),
            "Rendered timeline frames, including compositing, effects, clip boundaries and empty gaps. Flow across a cut is not continuous physical motion.".into(),
        ],
    };
    let job = Job {
        cancel: Arc::new(AtomicBool::new(false)),
        progress: Arc::new(AtomicUsize::new(0)),
        total: report.captures.len(),
        result: Arc::new(Mutex::new(None)),
        started: web_time::Instant::now(),
        replacement: replacement.map(|(report, index)| (Arc::new(report), index)),
    };
    let project = app.session.project.clone();
    let provider = app.session.media.provider(project.clone(), app.session.services.clone());
    let worker = job.clone();
    let repaint = ctx.clone();
    std::thread::Builder::new()
        .name("filmcraft-motion".into())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<MotionReport, String> {
                let mut report = report;
                let mut runtime = SeaRaft::start_with_device(&worker.cancel, device)?;
                report.settings["runtime"] = runtime.metadata.clone();
                let mut previous: Option<(usize, usize, Vec<u8>)> = None;
                for (index, capture) in report.captures.iter().enumerate() {
                    if worker.cancel.load(Ordering::Relaxed) {
                        return Err("Motion analysis cancelled".into());
                    }
                    if worker.started.elapsed().as_secs() >= 600 {
                        return Err("Motion analysis timed out. Try a shorter range.".into());
                    }
                    let (item, time) = match project.resolve_media(capture.item) {
                        Some((parent, _, Some(range))) => (parent, capture.media_time + range.start),
                        _ => (capture.item, capture.media_time),
                    };
                    let image = filmcraft_render::render_item(
                        &project,
                        item,
                        time,
                        (filmcraft_vision::sea_raft::MAX_SIZE as f32 / width.max(height) as f32).min(1.0),
                        &provider,
                    )
                    .ok_or("A source frame could not be decoded. Check the media and retry.")?;
                    let rgba = image.to_rgba8();
                    if let Some(previous) = previous.as_ref() {
                        let from = report.captures.get(index.saturating_sub(1)).ok_or("Missing previous timestamp")?;
                        if previous.0 != image.w || previous.1 != image.h {
                            return Err("Source frame dimensions changed during analysis".into());
                        }
                        let evidence = runtime.analyze(image.w, image.h, &previous.2, &rgba, &worker.cancel)?;
                        let vectors = evidence
                            .vectors
                            .into_iter()
                            .map(|v| MotionVector {
                                from: v.from,
                                to: v.to,
                                uncertainty_log_scale: Some(v.uncertainty_log_scale),
                                mean_absolute_error: None,
                                ambiguity_margin: None,
                            })
                            .collect();
                        report.pairs.push(MotionPair {
                            from_timeline_tick: from.timeline_time.0,
                            to_timeline_tick: capture.timeline_time.0,
                            from_source_tick: from.media_time.0,
                            to_source_tick: capture.media_time.0,
                            analyzed_size: [image.w, image.h],
                            vectors,
                            rejected_points: evidence.rejected_points,
                            mean_pixel_change: evidence.mean_pixel_change,
                            changed_pixel_fraction: evidence.changed_pixel_fraction,
                            mean_motion_pixels: Some(evidence.mean_motion_pixels),
                            dense_pixel_count: Some(evidence.dense_pixel_count),
                        });
                    }
                    previous = Some((image.w, image.h, rgba));
                    worker.progress.store(index.saturating_add(1), Ordering::Relaxed);
                    repaint.request_repaint();
                }
                Ok(report)
            }))
            .unwrap_or_else(|_| Err("The motion worker failed. The project is unchanged; try a shorter range.".into()));
            *worker.result.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(outcome);
            repaint.request_repaint();
        })
        .map_err(|e| format!("Could not start motion analysis: {e}"))?;
    ctx.data_mut(|d| {
        d.insert_temp(job_id(), job);
        d.remove::<String>(error_id());
    });
    Ok(json!({"started":true}))
}

pub fn poll(app: &mut FilmcraftApp, ctx: &egui::Context) {
    super::reverse_tracking::poll(app, ctx);
    super::reverse_tools::poll(app, ctx);
    let Some(job) = ctx.data(|d| d.get_temp::<Job>(job_id())) else {
        return;
    };
    let result = job.result.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
    if let Some(result) = result {
        ctx.data_mut(|d| d.remove::<Job>(job_id()));
        if job.cancel.load(Ordering::Relaxed) {
            app.ui.status = "Motion analysis cancelled".into();
            return;
        }
        match result {
            Ok(mut report) => {
                let selected_frame = if let Some((original, index)) = job.replacement.clone() {
                    let mut original = original.as_ref().clone();
                    if let Err(error) = filmcraft_vision::replace_pair(&mut original.pairs, &report.pairs, index) {
                        app.ui.status = error.clone();
                        ctx.data_mut(|d| d.insert_temp(error_id(), error));
                        return;
                    }
                    report = original;
                    index.saturating_add(1)
                } else {
                    0
                };
                let timestamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|_| "System clock is before the Unix epoch".to_owned())
                    .and_then(|d| u64::try_from(d.as_millis()).map_err(|_| "Completion timestamp overflow".to_owned()));
                let completion = timestamp.and_then(|time| report.audit.complete(job.replacement.as_ref().map(|(_, index)| *index), report.pairs.len(), time));
                if let Err(error) = completion {
                    app.ui.status = error.clone();
                    ctx.data_mut(|d| d.insert_temp(error_id(), error));
                    return;
                }
                report.schema_version = 5;
                app.ui.panels.reverse_engineering.capture = report.captures.get(selected_frame).cloned();
                app.ui.panels.reverse_engineering.motion = Some(report);
                app.ui.panels.reverse_engineering.global_motion = None;
                app.ui.panels.reverse_engineering.local_motion = None;
                app.ui.panels.reverse_engineering.motion_regions = None;
                ctx.data_mut(|d| {
                    d.insert_temp(index_id(), selected_frame.saturating_sub(1));
                    d.insert_temp(frame_id(), selected_frame);
                    d.remove::<(usize, usize)>(sample_id());
                });
                super::reverse_engineering::reset_decode_timer(ctx);
                if matches!(app.ui.panels.reverse_engineering.active_tool.as_str(), "globalMotion" | "localMotion" | "motionRegions")
                    && super::reverse_engineering::motion_is_current(app)
                {
                    if let Err(error) = super::reverse_tools::command(app, ctx, "reverseEngineering.runGlobal", &json!({})) {
                        app.ui.status = error.clone();
                        ctx.data_mut(|d| d.insert_temp(error_id(), error));
                        return;
                    }
                }
                app.ui.status = if selected_frame > 0 {
                    format!(
                        "Frame {} reanalysis complete · revision {}",
                        selected_frame + 1,
                        app.ui.panels.reverse_engineering.motion.as_ref().map(|r| r.audit.revision).unwrap_or(0)
                    )
                } else {
                    "Full-range analysis complete".into()
                };
            }
            Err(error) => {
                app.ui.status = error.clone();
                ctx.data_mut(|d| d.insert_temp(error_id(), error));
            }
        }
    } else {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}

pub fn command(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str, params: &Value) -> Result<Value, String> {
    if id == "reverseEngineering.cancelMotion" {
        if let Some(job) = ctx.data(|d| d.get_temp::<Job>(job_id())) {
            job.cancel.store(true, Ordering::Relaxed);
        }
        return Ok(json!({"cancelled":true}));
    }
    if is_running(ctx) {
        return Err("Wait for analysis to finish or cancel it first.".into());
    }
    if id == "reverseEngineering.setMotionDisplay" && app.ui.panels.reverse_engineering.active_tool == "localMotion" {
        return super::reverse_local::set_display(app, params);
    }
    if !super::reverse_engineering::motion_is_current(app) {
        return Err("Range changed. Analyze the selected range before inspecting or editing results.".into());
    }
    if id == "reverseEngineering.reanalyzeFrame" {
        let frame = ctx.data(|d| d.get_temp::<usize>(frame_id())).unwrap_or(0);
        let index = frame.checked_sub(1).ok_or("The first frame is a reference image; select a later frame to reanalyze its incoming motion.")?;
        let original = app.ui.panels.reverse_engineering.motion.clone().ok_or("Analyze a range first")?;
        let captures = original.captures.get(index..=index.saturating_add(1)).ok_or("Frame outside analysis")?.to_vec();
        return start_job(app, ctx, captures, Some((original, index)));
    }
    if id == "reverseEngineering.motionFrame" {
        let frame = params.get("frameIndex").and_then(Value::as_u64).and_then(|n| usize::try_from(n).ok()).ok_or("Supply a frame index")?;
        let capture = app.ui.panels.reverse_engineering.motion.as_ref().and_then(|r| r.captures.get(frame)).cloned().ok_or("Frame outside analyzed range")?;
        app.stop();
        app.session.set_playhead(capture.timeline_time);
        app.ui.panels.reverse_engineering.capture = Some(capture);
        ctx.data_mut(|d| {
            d.insert_temp(frame_id(), frame);
            d.insert_temp(index_id(), frame.saturating_sub(1));
            d.remove::<(usize, usize)>(sample_id());
        });
        super::reverse_engineering::reset_decode_timer(ctx);
        return Ok(json!({"frameIndex":frame}));
    }
    if id == "reverseEngineering.setMotionDisplay" {
        let report = app.ui.panels.reverse_engineering.motion.as_mut().ok_or("Analyze a range first")?;
        if report.analyzer != "filmcraft-vision/sea-raft-M" {
            return Err("Reanalyze this legacy report with SEA-RAFT before adjusting its display".into());
        }
        let index = if let Some(value) = params.get("index") {
            value.as_u64().and_then(|v| usize::try_from(v).ok()).ok_or("Supply an integer frame-pair index")?
        } else {
            ctx.data(|d| d.get_temp::<usize>(index_id())).unwrap_or(0)
        };
        let scope = params.get("scope").and_then(Value::as_str).ok_or("Supply display scope: frame, all, or inherit")?;
        let settings: MotionDisplay = if scope == "inherit" {
            report.display.effective(index)
        } else {
            serde_json::from_value(params.get("display").cloned().ok_or("Supply display settings")?).map_err(|e| e.to_string())?
        };
        report.display.apply(scope, index, report.pairs.len(), settings)?;
        ctx.request_repaint();
        return Ok(json!({"scope":scope,"index":index,"display":report.display.effective(index)}));
    }
    let report = app.ui.panels.reverse_engineering.motion.as_ref().ok_or("Analyze a range first")?;
    if id == "reverseEngineering.inspectMotionVector" {
        let index = params.get("pairIndex").and_then(Value::as_u64).and_then(|v| usize::try_from(v).ok()).ok_or("Supply a frame-pair index")?;
        let sample = params.get("vectorIndex").and_then(Value::as_u64).and_then(|v| usize::try_from(v).ok()).ok_or("Supply a model-sample index")?;
        let vector = report.pairs.get(index).and_then(|p| p.vectors.get(sample)).ok_or("Sample is outside this analysis")?;
        ctx.data_mut(|d| d.insert_temp(sample_id(), (index, sample)));
        return serde_json::to_value(vector).map_err(|e| e.to_string());
    }
    if id == "reverseEngineering.motionPair" {
        let index = params.get("index").and_then(Value::as_u64).and_then(|v| usize::try_from(v).ok()).ok_or("Supply an integer pair index")?;
        report.pairs.get(index).ok_or("Pair index is outside this analysis")?;
        let capture = report.captures.get(index.saturating_add(1)).cloned().ok_or("Missing analyzed frame")?;
        app.session.set_playhead(capture.timeline_time);
        app.stop();
        app.ui.panels.reverse_engineering.capture = Some(capture);
        ctx.data_mut(|d| {
            d.insert_temp(index_id(), index);
            d.insert_temp(frame_id(), index.saturating_add(1));
        });
        super::reverse_engineering::reset_decode_timer(ctx);
        return Ok(json!({"index":index}));
    }
    if id != "reverseEngineering.exportMotion" {
        return Err("Unknown motion command".into());
    }
    if report.analyzer != "filmcraft-vision/sea-raft-M" {
        return Err("This is a legacy patch-tracker result. Analyze the range again with SEA-RAFT before exporting.".into());
    }
    let mut exported = report.clone();
    exported.schema_version = 5;
    let bytes = serde_json::to_vec_pretty(&exported).map_err(|e| e.to_string())?;
    let path = if let Some(value) = params.get("path") {
        Some(value.as_str().filter(|s| !s.trim().is_empty()).ok_or("Export path must be nonempty text")?.to_owned())
    } else {
        let pick = app.hooks.pick_save_as.as_mut().ok_or("No save dialog available. Supply a path.")?;
        pick("Motion analysis", &["json"], "motion-analysis.json")
    };
    let Some(path) = path else {
        return Ok(Value::Null);
    };
    std::fs::write(&path, bytes).map_err(|e| format!("Could not export motion analysis: {e}"))?;
    app.ui.status = format!("Exported {path}");
    Ok(json!({"path":path}))
}
fn invoke(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str, params: Value) {
    if let Err(error) = crate::menus::invoke(app, ctx, id, params) {
        app.ui.status = error.clone();
        ctx.data_mut(|d| d.insert_temp(error_id(), error));
    }
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    ui.separator();
    ui.heading("SEA-RAFT optical flow");
    if let Some(job) = ui.ctx().data(|d| d.get_temp::<Job>(job_id())) {
        let progress = job.progress.load(Ordering::Relaxed);
        ui.label(format!("Analyzing · {progress} / {} source frames", job.total));
        let response = ui.button("Cancel analysis");
        app.auto.add("reverseEngineering.cancelMotion", response.rect, "Cancel analysis");
        if response.clicked() {
            invoke(app, ui.ctx(), "reverseEngineering.cancelMotion", json!({}));
        }
        return;
    }
    super::reverse_tools::device_controls(app, ui, false);
    let current = super::reverse_engineering::motion_is_current(app);
    if !current && app.ui.panels.reverse_engineering.motion.is_some() {
        ui.label("Range changed — analyze again. Previous results are hidden.");
    }
    let response = ui.button(if current { "Reanalyze entire range" } else { "Retry analysis" });
    app.auto.add("reverseEngineering.analyzeMotion", response.rect, "Analyze every selected frame");
    if response.clicked() {
        invoke(app, ui.ctx(), "reverseEngineering.analyzeMotion", json!({}));
    }
    if current {
        ui.label("Inspect results · select a thumbnail or jump to a frame below");
        if ui.ctx().data(|d| d.get_temp::<usize>(frame_id())).unwrap_or(0) > 0 {
            display_controls(app, ui);
            ui.label("Overlay edits update immediately; they do not change SEA-RAFT predictions.");
            let response = ui.button("Reanalyze selected frame’s motion");
            app.auto.add("reverseEngineering.reanalyzeFrame", response.rect, "Rerun SEA-RAFT for the previous and selected frame only");
            if response.clicked() {
                invoke(app, ui.ctx(), "reverseEngineering.reanalyzeFrame", json!({}));
            }
        } else {
            ui.label("First frame: reference image. Select a later frame to inspect motion from its preceding frame.");
        }
    }
    if let Some(error) = ui.ctx().data(|d| d.get_temp::<String>(error_id())) {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
    }
}

/// Fixed inspection area: navigation and its preview stay visible together.
pub fn show_result_controls(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    if !super::reverse_tools::results_visible(app, ui.ctx()) {
        return;
    }
    let Some(report) = app.ui.panels.reverse_engineering.motion.as_ref() else {
        return;
    };
    if app.ui.panels.reverse_engineering.active_tool == "opticalFlow"
        && let Some(run) = report.audit.completed_runs.last()
    {
        let seconds = (run.completed_at_unix_ms / 1000) % 86400;
        let scope = run.frame_number.map(|n| format!("frame {n}")).unwrap_or_else(|| "entire range".into());
        ui.label(format!("Completed {scope} · {:02}:{:02}:{:02} UTC · revision {}", seconds / 3600, (seconds / 60) % 60, seconds % 60, run.revision));
    }
    let captures = report.captures.clone();
    let count = captures.len();
    if count == 0 {
        return;
    }
    let frame = ui.ctx().data(|d| d.get_temp::<usize>(frame_id())).unwrap_or(0).min(count - 1);
    ui.horizontal_wrapped(|ui| {
        for (label, target, enabled, id) in [
            ("First", 0, frame > 0, "reverseEngineering.motionFirst"),
            ("Previous", frame.saturating_sub(1), frame > 0, "reverseEngineering.motionPrevious"),
            ("Next", frame.saturating_add(1), frame + 1 < count, "reverseEngineering.motionNext"),
            ("Last", count - 1, frame + 1 < count, "reverseEngineering.motionLast"),
        ] {
            let response = ui.add_enabled(enabled, egui::Button::new(label));
            app.auto.add(id, response.rect, label);
            if response.clicked() {
                invoke(app, ui.ctx(), "reverseEngineering.motionFrame", json!({"frameIndex":target}));
            }
        }
        let response = ui.button("Export this tool");
        app.auto.add("reverseEngineering.exportMotion", response.rect, "Export model results and separate display settings");
        if response.clicked() {
            invoke(app, ui.ctx(), "reverseEngineering.exportTool", json!({}));
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("Jump to frame in range:");
        let draft_id = egui::Id::new("reverse-motion-jump");
        let mut jump = ui.ctx().data(|d| d.get_temp::<usize>(draft_id)).unwrap_or(frame + 1);
        let response = ui.add(egui::DragValue::new(&mut jump).range(1..=count));
        app.auto.add("reverseEngineering.motionJump.input", response.rect, "1-based frame number within analyzed range");
        ui.ctx().data_mut(|d| d.insert_temp(draft_id, jump));
        let response = ui.button("Go");
        app.auto.add("reverseEngineering.motionJump.go", response.rect, "Jump to this analyzed frame");
        if response.clicked() {
            invoke(app, ui.ctx(), "reverseEngineering.motionFrame", json!({"frameIndex":jump.saturating_sub(1)}));
        }
        ui.label(format!("of {count}"));
    });
    let frame = ui.ctx().data(|d| d.get_temp::<usize>(frame_id())).unwrap_or(0).min(count - 1);
    if let Some(capture) = captures.get(frame) {
        let label = format!("Frame {} / {} · {}", frame + 1, count, super::reverse_engineering::timecode(app, capture));
        let response = ui.label(label);
        app.auto.add("reverseEngineering.timecode", response.rect, "Exact timestamp of the displayed analyzed frame");
        if frame > 0 {
            if let Some(pair) = app.ui.panels.reverse_engineering.motion.as_ref().and_then(|r| r.pairs.get(frame - 1)) {
                if app.ui.panels.reverse_engineering.active_tool == "globalMotion" {
                    super::reverse_tools::caption(app, ui, frame - 1);
                } else if app.ui.panels.reverse_engineering.active_tool == "motionRegions" {
                    super::reverse_regions::caption(app, ui, frame - 1);
                } else if app.ui.panels.reverse_engineering.active_tool == "localMotion" {
                    super::reverse_local::caption(app, ui, frame - 1);
                } else {
                    ui.label(format!("Motion: frame {} → {} · {} SEA-RAFT samples", frame, frame + 1, pair.vectors.len()));
                }
                if app.ui.panels.reverse_engineering.active_tool == "opticalFlow"
                    && let Some((selected_pair, sample)) = ui.ctx().data(|d| d.get_temp::<(usize, usize)>(sample_id()))
                    && selected_pair == frame - 1
                    && let Some(vector) = pair.vectors.get(sample)
                {
                    let dx = (vector.to[0] - vector.from[0]) * pair.analyzed_size[0] as f32;
                    let dy = (vector.to[1] - vector.from[1]) * pair.analyzed_size[1] as f32;
                    ui.label(format!(
                        "Sample {sample}: Δx {dx:.4}px · Δy {dy:.4}px · magnitude {:.4}px · uncertainty log-scale {:?}",
                        dx.hypot(dy),
                        vector.uncertainty_log_scale
                    ));
                }
            }
        }
    }
}

pub fn show_result(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    if !super::reverse_tools::results_visible(app, ui.ctx()) {
        return;
    }
    let Some(report) = app.ui.panels.reverse_engineering.motion.as_ref() else {
        return;
    };
    let captures = report.captures.clone();
    let frame = ui.ctx().data(|d| d.get_temp::<usize>(frame_id())).unwrap_or(0).min(captures.len().saturating_sub(1));
    if let Some(capture) = captures.get(frame) {
        super::reverse_engineering::selected_picture(app, ui, capture, 104.0);
        ui.add_space(8.0);
        super::reverse_engineering::frame_strip(app, ui, &captures, frame, false);
    }
}

pub fn overlay(app: &mut FilmcraftApp, ui: &egui::Ui, capture: &ReverseEngineeringCapture, rect: egui::Rect) {
    let tool = app.ui.panels.reverse_engineering.active_tool.as_str();
    if tool == "motionOverTime" {
        return;
    }
    if tool == "globalMotion" {
        let frame = ui.ctx().data(|d| d.get_temp::<usize>(frame_id())).unwrap_or(0);
        if frame > 0 {
            super::reverse_tools::overlay(app, ui, frame - 1, rect);
        }
        return;
    }
    if tool == "motionRegions" {
        let frame = ui.ctx().data(|d| d.get_temp::<usize>(frame_id())).unwrap_or(0);
        if frame > 0 {
            super::reverse_regions::overlay(app, ui, frame - 1, rect);
        }
        return;
    }
    if tool == "localMotion" {
        let frame = ui.ctx().data(|d| d.get_temp::<usize>(frame_id())).unwrap_or(0);
        if frame > 0 {
            super::reverse_local::overlay(app, ui, frame - 1, rect);
        }
        return;
    }
    if tool != "opticalFlow" {
        return;
    }

    if ui.ctx().data(|d| d.get_temp::<usize>(frame_id())).unwrap_or(0) == 0 || !super::reverse_engineering::motion_is_current(app) || is_running(ui.ctx()) {
        return;
    }
    let Some(report) = app.ui.panels.reverse_engineering.motion.as_ref() else {
        return;
    };
    if report.analyzer != "filmcraft-vision/sea-raft-M" {
        return;
    }
    let index = ui.ctx().data(|d| d.get_temp::<usize>(index_id())).unwrap_or(0);
    if report.captures.get(index.saturating_add(1)) != Some(capture) {
        return;
    }
    let Some(pair) = report.pairs.get(index) else {
        return;
    };
    let settings = report.display.effective(index);
    if !settings.enabled {
        return;
    }
    let response = ui.interact(rect, egui::Id::new("reverse-motion-overlay"), egui::Sense::click());
    let pointer = response.hover_pos();
    let selected = ui.ctx().data(|d| d.get_temp::<(usize, usize)>(sample_id()));
    let painter = ui.painter().with_clip_rect(rect);
    let mut nearest: Option<(usize, f32)> = None;
    for (sample, vector) in pair.vectors.iter().take(2048).enumerate() {
        if !settings.includes(vector.from, vector.to, pair.analyzed_size) {
            continue;
        }
        let position = |p: [f32; 2]| rect.min + egui::vec2(p[0] * rect.width(), p[1] * rect.height());
        let from = position(vector.from);
        let displacement = (position(vector.to) - from) * settings.arrow_scale;
        if displacement.length_sq() > 0.0 {
            painter.arrow(from, displacement, egui::Stroke::new(1.0, egui::Color32::from_rgb(130, 225, 235)));
        }
        if selected == Some((index, sample)) {
            painter.circle_stroke(from, 3.0, egui::Stroke::new(1.0, egui::Color32::WHITE));
        }
        if let Some(pointer) = pointer {
            let distance = pointer.distance(from);
            if distance <= 10.0 && nearest.is_none_or(|(_, previous)| distance < previous) {
                nearest = Some((sample, distance));
            }
        }
    }
    if let Some((sample, _)) = nearest
        && let Some(vector) = pair.vectors.get(sample)
    {
        let dx = (vector.to[0] - vector.from[0]) * pair.analyzed_size[0] as f32;
        let dy = (vector.to[1] - vector.from[1]) * pair.analyzed_size[1] as f32;
        response.clone().on_hover_text(format!("SEA-RAFT sample {sample}\nΔx {dx:.4}px · Δy {dy:.4}px\nClick to inspect the measurement"));
        if response.clicked() {
            invoke(app, ui.ctx(), "reverseEngineering.inspectMotionVector", json!({"pairIndex":index,"vectorIndex":sample}));
        }
    }
}

pub(super) fn display_controls(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    let index = ui.ctx().data(|d| d.get_temp::<usize>(index_id())).unwrap_or(0);
    let local = app.ui.panels.reverse_engineering.active_tool == "localMotion";
    let displays = if local {
        let Some(report) = app.ui.panels.reverse_engineering.local_motion.as_ref().filter(|r| index < r.pairs.len()) else {
            return;
        };
        &report.display
    } else {
        let Some(report) = app.ui.panels.reverse_engineering.motion.as_ref().filter(|r| r.analyzer == "filmcraft-vision/sea-raft-M" && index < r.pairs.len())
        else {
            return;
        };
        &report.display
    };
    let custom = displays.frame_overrides.contains_key(&index);
    let mut settings = displays.effective(index);
    let before = settings;
    ui.separator();
    ui.label(format!("{} overlay · {} settings", if local { "Comparison" } else { "SEA-RAFT" }, if custom { "this frame’s" } else { "shared" }));
    let response = ui.checkbox(&mut settings.enabled, if local { "Show comparison arrows" } else { "Show SEA-RAFT flow" });
    app.auto.add("reverseEngineering.motionDisplay.enabled", response.rect, if local { "Show comparison arrows" } else { "Show SEA-RAFT flow" });
    ui.horizontal_wrapped(|ui| {
        ui.label("Cutoff (analysis px)");
        let response = ui.add(egui::DragValue::new(&mut settings.cutoff_pixels).range(0.0..=100.0).speed(0.01));
        app.auto.add("reverseEngineering.motionDisplay.cutoff", response.rect, "Display cutoff in analysis pixels");
        ui.label("Arrow scale ×");
        let response = ui.add(egui::DragValue::new(&mut settings.arrow_scale).range(0.1..=32.0).speed(0.1));
        app.auto.add("reverseEngineering.motionDisplay.scale", response.rect, "Arrow display scale");
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("Display grid stride");
        let response = ui.add(egui::DragValue::new(&mut settings.sample_stride).range(1..=8));
        app.auto.add("reverseEngineering.motionDisplay.stride", response.rect, "Display every Nth model grid row and column");
        ui.label(format!("{} px spacing", settings.sample_stride * 16));
    });
    if settings != before {
        invoke(app, ui.ctx(), "reverseEngineering.setMotionDisplay", json!({"scope":"frame","index":index,"display":settings}));
    }
    ui.horizontal_wrapped(|ui| {
        let response = ui.button("Apply to all");
        app.auto.add("reverseEngineering.motionDisplay.applyAll", response.rect, "Apply these settings to all frames and clear frame overrides");
        if response.clicked() {
            invoke(app, ui.ctx(), "reverseEngineering.setMotionDisplay", json!({"scope":"all","index":index,"display":settings}));
        }
        let response = ui.add_enabled(custom || settings != before, egui::Button::new("Use all-frame settings"));
        app.auto.add("reverseEngineering.motionDisplay.inherit", response.rect, "Remove this frame override");
        if response.clicked() {
            invoke(app, ui.ctx(), "reverseEngineering.setMotionDisplay", json!({"scope":"inherit","index":index}));
        }
    });
    ui.label("Edits affect motion into the selected frame. Apply to all clears frame overrides. Measurements stay unchanged.");
}

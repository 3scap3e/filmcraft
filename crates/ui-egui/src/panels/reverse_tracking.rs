//! Manual/grid query selection and BootsTAPIR trajectory inspection, isolated from project edits.
use super::{panel_state::ReverseEngineeringCapture, reverse_engineering};
use crate::FilmcraftApp;
use filmcraft_vision::point_tracking::{self, Query};
use filmcraft_vision::point_tracking_chunks;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackingReport {
    pub schema_version: u32,
    pub analyzer: String,
    pub captures: Vec<ReverseEngineeringCapture>,
    pub project_revision: u64,
    pub selection_mode: String,
    pub queries: Vec<Query>,
    pub source_size: [u32; 2],
    pub inference: point_tracking_chunks::Result,
    pub limitations: Vec<String>,
}
#[derive(Clone)]
struct Job {
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicUsize>,
    total: usize,
    result: Arc<Mutex<Option<Result<TrackingReport, String>>>>,
}
fn job_id() -> egui::Id {
    egui::Id::new("reverse-point-tracking-job")
}
pub(super) fn is_running(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<Job>(job_id())).is_some()
}
pub(super) fn current(app: &FilmcraftApp) -> bool {
    let s = &app.ui.panels.reverse_engineering;
    s.tracking_report.as_ref().is_some_and(|r| {
        r.project_revision == app.session.revision
            && r.captures == s.tracking_captures
            && reverse_engineering::captures_are_current(app, &r.captures)
            && r.queries == s.tracking_queries
    })
}
pub(super) fn prepare(app: &mut FilmcraftApp) -> Result<(), String> {
    let captures = match reverse_engineering::range_captures(app, usize::MAX) {
        Ok(captures) => captures,
        Err(error) => {
            app.ui.panels.reverse_engineering.tracking_captures.clear();
            return Err(error);
        }
    };
    let state = &mut app.ui.panels.reverse_engineering;
    if state.tracking_captures != captures {
        state.tracking_captures = captures;
        state.tracking_queries.clear();
        state.tracking_frame = 0;
    }
    Ok(())
}
pub(super) fn poll(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let Some(job) = ctx.data(|d| d.get_temp::<Job>(job_id())) else {
        return;
    };
    let result = job.result.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
    if let Some(result) = result {
        ctx.data_mut(|d| d.remove::<Job>(job_id()));
        if job.cancel.load(Ordering::Relaxed) {
            app.ui.status = "Point tracking cancelled; previous results preserved".into();
            return;
        }
        match result {
            Ok(report) => {
                app.ui.panels.reverse_engineering.tracking_report = Some(report);
                app.ui.status = "BootsTAPIR tracking complete. Inspect trajectories and save.".into();
            }
            Err(e) => {
                app.ui.status = e.clone();
                ctx.data_mut(|d| d.insert_temp(egui::Id::new("reverse-tools-error"), e));
            }
        }
    } else {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}
pub(super) fn command(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str, params: &Value) -> Result<Value, String> {
    if id == "reverseEngineering.cancelTracking" {
        if let Some(job) = ctx.data(|d| d.get_temp::<Job>(job_id())) {
            job.cancel.store(true, Ordering::Relaxed);
        }
        return Ok(json!({"cancelled":true}));
    }
    if is_running(ctx) || super::reverse_motion::is_running(ctx) || super::reverse_tools::is_running(ctx) {
        return Err("Cancel the current analysis before changing point tracking".into());
    }
    prepare(app)?;
    if id == "reverseEngineering.trackingFrame" {
        let index = params.get("frameIndex").and_then(Value::as_u64).and_then(|v| usize::try_from(v).ok()).ok_or("Supply frame index")?;
        let capture = app.ui.panels.reverse_engineering.tracking_captures.get(index).ok_or("Frame outside tracking range")?.clone();
        app.ui.panels.reverse_engineering.tracking_frame = index;
        app.stop();
        app.session.set_playhead(capture.timeline_time);
        ctx.request_repaint();
        return Ok(json!({"frameIndex":index}));
    }
    if id == "reverseEngineering.setTrackingMode" {
        let mode = params.get("mode").and_then(Value::as_str).ok_or("Choose manual or grid selection")?;
        if !matches!(mode, "manual" | "grid") {
            return Err("Selection mode must be manual or grid".into());
        }
        let side = params
            .get("gridSide")
            .and_then(Value::as_u64)
            .and_then(|v| usize::try_from(v).ok())
            .unwrap_or(app.ui.panels.reverse_engineering.tracking_grid_side);
        if !(2..=8).contains(&side) {
            return Err("Grid side must be 2–8".into());
        }
        let s = &mut app.ui.panels.reverse_engineering;
        s.tracking_grid_side = side;
        s.tracking_grid = mode == "grid";
        s.tracking_queries = if s.tracking_grid { point_tracking::grid(s.tracking_frame, side, s.tracking_captures.len())? } else { Vec::new() };
        ctx.request_repaint();
        return Ok(json!({"mode":mode}));
    }
    if id == "reverseEngineering.clearTrackingPoints" {
        app.ui.panels.reverse_engineering.tracking_queries.clear();
        return Ok(json!({"cleared":true}));
    }
    if id == "reverseEngineering.addTrackingPoint" {
        let position: [f64; 2] = serde_json::from_value(params.get("position").cloned().ok_or("Supply normalized XY position")?).map_err(|e| e.to_string())?;
        let s = &mut app.ui.panels.reverse_engineering;
        if s.tracking_grid || s.tracking_queries.len() >= point_tracking::MAX_POINTS {
            return Err("Use manual mode and at most 64 points".into());
        }
        let q = Query { frame_index: s.tracking_frame, position };
        point_tracking::validate_queries(std::slice::from_ref(&q), s.tracking_captures.len())?;
        s.tracking_queries.push(q);
        ctx.request_repaint();
        return Ok(json!({"points":s.tracking_queries.len()}));
    }
    if id != "reverseEngineering.runTracking" {
        return Err("Unknown point-tracking command".into());
    }
    let s = &app.ui.panels.reverse_engineering;
    point_tracking::validate_queries(&s.tracking_queries, s.tracking_captures.len())?;
    let captures = s.tracking_captures.clone();
    let queries = s.tracking_queries.clone();
    let mode = if s.tracking_grid { "grid" } else { "manual" };
    let first = captures.first().ok_or("No tracking frames")?;
    let size = app.session.project.source_size(first.item).ok_or("No video dimensions")?;
    if size.0 == 0 || size.1 == 0 || u64::from(size.0) * u64::from(size.1) > 67_108_864 {
        return Err("Invalid tracking source dimensions".into());
    }
    let device = s.tracking_device;
    let revision = app.session.revision;
    let project = app.session.project.clone();
    let provider = app.session.media.provider(project.clone(), app.session.services.clone());
    let job =
        Job { cancel: Arc::new(AtomicBool::new(false)), progress: Arc::new(AtomicUsize::new(0)), total: captures.len(), result: Arc::new(Mutex::new(None)) };
    let worker = job.clone();
    let repaint = ctx.clone();
    std::thread::Builder::new().name("filmcraft-point-tracking".into()).spawn(move || {
        let result=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||->Result<TrackingReport,String>{
            let inference=point_tracking_chunks::track(captures.len(),queries.clone(),&worker.cancel,device,|index| {
                if worker.cancel.load(Ordering::Relaxed) {return Err("Point tracking cancelled".into());}
                let capture=captures.get(index).ok_or("Tracking frame outside full range")?;
                let image=filmcraft_render::render_item(&project,capture.item,capture.media_time,(320.0/size.0.max(size.1) as f32).min(1.0),&provider).ok_or("Tracking frame could not render")?;
                Ok((image.w,image.h,image.to_rgba8()))
            },|done,total| {
                let normalized=if total==0 {0} else {(done as u128*captures.len() as u128/total as u128) as usize};
                worker.progress.store(normalized,Ordering::Relaxed);repaint.request_repaint();
            })?;
            Ok(TrackingReport {schema_version:2,analyzer:"filmcraft-vision/bootstapir-chunked-v2".into(),captures,project_revision:revision,
                selection_mode:mode.into(),queries,source_size:[size.0,size.1],inference,
                limitations:vec!["Tracks supplied points through rendered timeline frames; grid selection does not identify objects.".into(),
                    "Uses the official BootsTAPIR checkpoint at 256x256 inference resolution; coordinates are normalized XY, queries include a range-relative frame index.".into(),
                    "Visibility is the model's score and threshold decision, not calibrated confidence. Occluded points are not drawn as visible tracks.".into(),
                    "Point tracking can fail at cuts, occlusion and visually ambiguous features. Paths are model predictions, not ground truth.".into(),
                    "Inference uses at most 120 frames per overlapping window. Continuations require visible in-bounds points; missing continuation predictions are null. Re-seeding predictions can accumulate drift.".into()]})
        })).unwrap_or_else(|_|Err("Point-tracking worker failed; previous results preserved".into()));
        *worker.result.lock().unwrap_or_else(std::sync::PoisonError::into_inner)=Some(result);repaint.request_repaint();
    }).map_err(|e|e.to_string())?;
    ctx.data_mut(|d| d.insert_temp(job_id(), job));
    Ok(json!({"started":true}))
}
fn invoke(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str, params: Value) {
    if let Err(error) = crate::menus::invoke(app, ctx, id, params) {
        app.ui.status = error.clone();
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("reverse-tools-error"), error));
    } else {
        ctx.data_mut(|d| d.remove::<String>(egui::Id::new("reverse-tools-error")));
    }
}
pub(super) fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    ui.heading("Point tracking · BootsTAPIR");
    if let Some(job) = ui.ctx().data(|d| d.get_temp::<Job>(job_id())) {
        let done = job.progress.load(Ordering::Relaxed);
        ui.label(format!("Processing full range in chunks · {done} / {} frames", job.total));
        let response = ui.button("Cancel tracking");
        app.auto.add("reverseEngineering.cancelTracking", response.rect, "Cancel point tracking");
        if response.clicked() {
            invoke(app, ui.ctx(), "reverseEngineering.cancelTracking", json!({}));
        }
        return;
    }
    if let Err(error) = prepare(app) {
        ui.label(error);
        return;
    }
    super::reverse_tools::device_controls(app, ui, true);
    ui.horizontal_wrapped(|ui| {
        for (grid, label, mode) in [(false, "Manual points", "manual"), (true, "Automatic grid", "grid")] {
            let response = ui.selectable_label(app.ui.panels.reverse_engineering.tracking_grid == grid, label);
            app.auto.add(&format!("reverseEngineering.trackingMode.{mode}"), response.rect, label);
            if response.clicked() {
                invoke(app, ui.ctx(), "reverseEngineering.setTrackingMode", json!({"mode":mode}));
            }
        }
    });
    if app.ui.panels.reverse_engineering.tracking_grid {
        let mut side = app.ui.panels.reverse_engineering.tracking_grid_side;
        ui.horizontal_wrapped(|ui| {
            ui.label("Grid side");
            let response = ui.add(egui::DragValue::new(&mut side).range(2..=8));
            app.auto.add("reverseEngineering.tracking.gridSide", response.rect, "Grid dimensions, at most 64 points");
            if response.changed() {
                invoke(app, ui.ctx(), "reverseEngineering.setTrackingMode", json!({"mode":"grid","gridSide":side}));
            }
        });
        let response = ui.button("Seed grid on displayed frame");
        app.auto.add("reverseEngineering.tracking.seedGrid", response.rect, "Choose the displayed frame as grid reference");
        if response.clicked() {
            invoke(app, ui.ctx(), "reverseEngineering.setTrackingMode", json!({"mode":"grid"}));
        }
    } else {
        ui.label("Choose a visible frame below and click on the image to add points. For the sun, choose several edge points.");
    }
    ui.horizontal_wrapped(|ui| {
        ui.label(format!("{} query points", app.ui.panels.reverse_engineering.tracking_queries.len()));
        let response = ui.button("Clear points");
        app.auto.add("reverseEngineering.clearTrackingPoints", response.rect, "Clear query points");
        if response.clicked() {
            invoke(app, ui.ctx(), "reverseEngineering.clearTrackingPoints", json!({}));
        }
        let response = ui.add_enabled(!app.ui.panels.reverse_engineering.tracking_queries.is_empty(), egui::Button::new("Track points"));
        app.auto.add("reverseEngineering.runTracking", response.rect, "Run BootsTAPIR on the chosen points");
        if response.clicked() {
            invoke(app, ui.ctx(), "reverseEngineering.runTracking", json!({}));
        }
    });
}
pub(super) fn show_result_controls(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    let count = app.ui.panels.reverse_engineering.tracking_captures.len();
    if count == 0 {
        return;
    }
    let frame = app.ui.panels.reverse_engineering.tracking_frame.min(count - 1);
    let busy = is_running(ui.ctx());
    ui.add_enabled_ui(!busy, |ui| {
        ui.horizontal_wrapped(|ui| {
            for (label, index) in [("First", 0), ("Previous", frame.saturating_sub(1)), ("Next", frame.saturating_add(1).min(count - 1)), ("Last", count - 1)] {
                let response = ui.button(label);
                app.auto.add(&format!("reverseEngineering.tracking.{label}"), response.rect, label);
                if response.clicked() {
                    invoke(app, ui.ctx(), "reverseEngineering.trackingFrame", json!({"frameIndex":index}));
                }
            }
            let mut jump = frame + 1;
            let response = ui.add(egui::DragValue::new(&mut jump).range(1..=count));
            app.auto.add("reverseEngineering.tracking.jump", response.rect, "Jump to range frame");
            if response.changed() {
                invoke(app, ui.ctx(), "reverseEngineering.trackingFrame", json!({"frameIndex":jump-1}));
            }
        });
    });
    let frame = app.ui.panels.reverse_engineering.tracking_frame.min(count - 1);
    if let Some(capture) = app.ui.panels.reverse_engineering.tracking_captures.get(frame).cloned() {
        ui.label(format!("Frame {} / {} · {}", frame + 1, count, reverse_engineering::timecode(app, &capture)));
        if current(app) {
            if let Some(report) = app.ui.panels.reverse_engineering.tracking_report.as_ref() {
                let visible = report.inference.tracks.iter().filter(|t| t.observations.get(frame).and_then(Option::as_ref).is_some_and(|o| o.visible)).count();
                let missing = report.inference.tracks.iter().filter(|t| t.observations.get(frame).is_none_or(Option::is_none)).count();
                ui.label(format!("{visible} / {} points visible · {missing} missing continuations", report.inference.tracks.len()));
            }
        }
    }
}
pub(super) fn show_result(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    let captures = app.ui.panels.reverse_engineering.tracking_captures.clone();
    let frame = app.ui.panels.reverse_engineering.tracking_frame.min(captures.len().saturating_sub(1));
    if let Some(capture) = captures.get(frame) {
        reverse_engineering::selected_picture(app, ui, capture, 104.0);
        ui.add_space(8.0);
        reverse_engineering::frame_strip(app, ui, &captures, frame, true);
    }
}

pub(super) fn overlay(app: &mut FilmcraftApp, ui: &egui::Ui, rect: egui::Rect) {
    let frame = app.ui.panels.reverse_engineering.tracking_frame;
    let painter = ui.painter().with_clip_rect(rect);
    let position = |p: [f64; 2]| rect.min + egui::vec2(p[0] as f32 * rect.width(), p[1] as f32 * rect.height());
    if current(app) {
        if let Some(report) = app.ui.panels.reverse_engineering.tracking_report.as_ref() {
            for (i, track) in report.inference.tracks.iter().take(64).enumerate() {
                let color = if i % 2 == 0 { egui::Color32::from_rgb(100, 220, 235) } else { egui::Color32::from_rgb(245, 210, 100) };
                let start = frame.saturating_sub(30);
                let mut previous = None;
                for t in start..=frame {
                    let Some(o) = track
                        .observations
                        .get(t)
                        .and_then(Option::as_ref)
                        .filter(|o| o.visible && o.position.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)))
                    else {
                        previous = None;
                        continue;
                    };
                    let p = position(o.position);
                    if let Some(prev) = previous {
                        painter.line_segment([prev, p], egui::Stroke::new(1.5, color));
                    }
                    previous = Some(p);
                }
                if let Some(o) = track
                    .observations
                    .get(frame)
                    .and_then(Option::as_ref)
                    .filter(|o| o.visible && o.position.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)))
                {
                    let point = position(o.position);
                    painter.circle_filled(point, 3.0, color);
                    let response = ui.interact(
                        egui::Rect::from_center_size(point, egui::vec2(14.0, 14.0)),
                        egui::Id::new(("reverse-tracking-measurement", frame, i)),
                        egui::Sense::hover(),
                    );
                    let reference =
                        track.observations.get(track.query.frame_index).and_then(Option::as_ref).map(|r| r.position).unwrap_or(track.query.position);
                    response.on_hover_text(format!(
                        "Point {} · normalized x {:.5}, y {:.5}\nFrom reference: Δx {:.3}px · Δy {:.3}px at {}×{}\nModel visibility score {:.4}",
                        i + 1,
                        o.position[0],
                        o.position[1],
                        (o.position[0] - reference[0]) * report.source_size[0] as f64,
                        (o.position[1] - reference[1]) * report.source_size[1] as f64,
                        report.source_size[0],
                        report.source_size[1],
                        o.visibility_score
                    ));
                }
            }
        }
    } else {
        for q in app.ui.panels.reverse_engineering.tracking_queries.iter().take(64).filter(|q| q.frame_index == frame) {
            painter.circle_stroke(position(q.position), 4.0, egui::Stroke::new(1.5, egui::Color32::YELLOW));
        }
    }
    if !app.ui.panels.reverse_engineering.tracking_grid && !is_running(ui.ctx()) {
        let response = ui.interact(rect, egui::Id::new("reverse-tracking-point-pick"), egui::Sense::click());
        if response.clicked()
            && let Some(p) = response.interact_pointer_pos()
        {
            let x = ((p.x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 0.999999);
            let y = ((p.y - rect.top()) / rect.height().max(1.0)).clamp(0.0, 0.999999);
            invoke(app, ui.ctx(), "reverseEngineering.addTrackingPoint", json!({"position":[x,y]}));
        }
    }
}

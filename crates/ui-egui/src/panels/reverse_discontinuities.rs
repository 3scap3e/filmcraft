//! Inspect abrupt changes in sampled optical-flow motion.
use super::{panel_state::ReverseEngineeringCapture, reverse_engineering};
use crate::FilmcraftApp;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub schema_version: u32,
    pub analyzer: String,
    pub captures: Vec<ReverseEngineeringCapture>,
    pub source_motion_revision: u64,
    pub source_completed_at_unix_ms: u64,
    pub ticks_per_second: i64,
    pub threshold_pixels: f64,
    pub changes: Vec<filmcraft_vision::discontinuities::Change>,
    pub limitations: Vec<String>,
}
pub(super) fn current(app: &FilmcraftApp) -> bool {
    if !reverse_engineering::motion_is_current(app) {
        return false;
    }
    let s = &app.ui.panels.reverse_engineering;
    match (&s.discontinuities, &s.motion) {
        (Some(r), Some(f)) => {
            r.captures == f.captures
                && r.source_motion_revision == f.audit.revision
                && r.source_completed_at_unix_ms == f.audit.completed_runs.last().map(|r| r.completed_at_unix_ms).unwrap_or(0)
        }
        _ => false,
    }
}
pub(super) fn build(app: &mut FilmcraftApp, threshold: f64) -> Result<(), String> {
    let f = app.ui.panels.reverse_engineering.motion.as_ref().ok_or("Analyze optical flow first")?;
    let samples = f.pairs.iter().map(filmcraft_vision::motion_time::summarize).collect::<Result<Vec<_>, _>>()?;
    let changes = filmcraft_vision::discontinuities::detect(&samples, threshold)?;
    app.ui.panels.reverse_engineering.discontinuities=Some(Report{schema_version:1,analyzer:"filmcraft-vision/motion-discontinuities-v1".into(),captures:f.captures.clone(),source_motion_revision:f.audit.revision,source_completed_at_unix_ms:f.audit.completed_runs.last().map(|r|r.completed_at_unix_ms).unwrap_or(0),ticks_per_second:f.ticks_per_second,threshold_pixels:threshold,changes,limitations:vec!["Compares adjacent sampled SEA-RAFT summaries. Score is max(direction-mean change magnitude, absolute mean-strength change), in analysis pixels per comparison.".into(),"Events are motion-change candidates, not confirmed cuts, scene boundaries, objects or camera movements. Fast motion, transitions and model errors can all cause peaks.".into(),"Whole-image means can hide opposing or small local motion. First two frames provide no change score; missing/non-contiguous comparisons are unavailable.".into()]});
    Ok(())
}
pub(super) fn options(app: &mut FilmcraftApp, params: &Value) -> Result<Value, String> {
    if !reverse_engineering::motion_is_current(app) {
        return Err("Analyze optical flow first".into());
    }
    let threshold = params.get("thresholdPixels").and_then(Value::as_f64).ok_or("Supply thresholdPixels")?;
    build(app, threshold)?;
    Ok(json!({"thresholdPixels":threshold}))
}
pub(super) fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    ui.heading("Motion discontinuities");
    if super::reverse_motion::is_running(ui.ctx()) {
        super::reverse_motion::show(app, ui);
        return;
    }
    if !current(app) && reverse_engineering::motion_is_current(app) {
        let threshold = app.ui.panels.reverse_engineering.discontinuities.as_ref().map(|r| r.threshold_pixels).unwrap_or(0.1);
        if let Err(e) = build(app, threshold) {
            ui.colored_label(egui::Color32::LIGHT_RED, e);
            return;
        }
    }
    ui.label("Abrupt changes in whole-image motion. Inspect each candidate before interpreting it.");
    let Some(r) = app.ui.panels.reverse_engineering.discontinuities.as_ref().filter(|_| current(app)) else {
        return;
    };
    let mut threshold = r.threshold_pixels;
    let events = r.changes.iter().filter(|c| c.flagged).cloned().collect::<Vec<_>>();
    let unavailable = r.changes.iter().filter(|c| c.score_pixels.is_none()).count();
    ui.horizontal_wrapped(|ui| {
        ui.label("Change threshold (analysis px)");
        let response = ui.add(egui::DragValue::new(&mut threshold).range(0.001..=100.0).speed(0.01));
        app.auto.add("reverseEngineering.discontinuities.threshold", response.rect, "Minimum motion change to flag");
        if response.changed() {
            if let Err(e) = crate::menus::invoke(app, ui.ctx(), "reverseEngineering.setDiscontinuityOptions", json!({"thresholdPixels":threshold})) {
                app.ui.status = e;
            }
        }
    });
    ui.label(format!("{} candidates · {unavailable} unavailable comparisons", events.len()));
    ui.label("Lower the threshold to include smaller changes. Updates immediately without running the model again.");
    ui.horizontal_wrapped(|ui| {
        for event in events {
            let response = ui.button(format!("Frame {} · {:.3}px", event.frame_index + 1, event.score_pixels.unwrap_or(0.0)));
            app.auto.add(&format!("reverseEngineering.discontinuities.event.{}", event.frame_index), response.rect, "Inspect motion-change candidate");
            if response.clicked() {
                if let Err(e) = crate::menus::invoke(app, ui.ctx(), "reverseEngineering.motionFrame", json!({"frameIndex":event.frame_index})) {
                    app.ui.status = e;
                }
            }
        }
    });
}
pub(super) fn graph(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    let Some(r) = app.ui.panels.reverse_engineering.discontinuities.as_ref().filter(|_| current(app)) else {
        return;
    };
    let changes = r.changes.clone();
    let threshold = r.threshold_pixels;
    let ticks = r.ticks_per_second.max(1) as f64;
    if changes.is_empty() {
        ui.label("At least three frames are needed to compare motion changes.");
        return;
    }
    ui.label("Motion change · yellow: flagged candidate");
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width().max(1.0), 120.0), egui::Sense::click());
    let bound = changes.iter().filter_map(|c| c.score_pixels).fold(threshold, f64::max).max(0.001) * 1.1;
    let p = ui.painter().with_clip_rect(rect);
    let x = |i: usize| rect.left() + rect.width() * (i as f32 + 0.5) / changes.len() as f32;
    let yy = rect.bottom() - (threshold / bound) as f32 * rect.height();
    p.line_segment([egui::pos2(rect.left(), yy), egui::pos2(rect.right(), yy)], egui::Stroke::new(1.0, egui::Color32::GRAY));
    for (i, c) in changes.iter().enumerate() {
        if let Some(score) = c.score_pixels {
            p.line_segment(
                [egui::pos2(x(i), rect.bottom()), egui::pos2(x(i), rect.bottom() - (score / bound) as f32 * rect.height())],
                egui::Stroke::new(2.0, if c.flagged { egui::Color32::YELLOW } else { egui::Color32::LIGHT_BLUE }),
            );
        }
    }
    let frame = ui.ctx().data(|d| d.get_temp::<usize>(egui::Id::new("reverse-motion-frame"))).unwrap_or(0);
    if let Some(i) = changes.iter().position(|c| c.frame_index == frame) {
        p.line_segment([egui::pos2(x(i), rect.top()), egui::pos2(x(i), rect.bottom())], egui::Stroke::new(1.0, egui::Color32::WHITE));
        let c = &changes[i];
        ui.label(format!("Selected change: direction {:?} · strength {:?} px", c.direction_change_pixels, c.strength_change_pixels));
    } else {
        ui.label("First two frames have no motion-change score.");
    }
    ui.label(format!("Scale 0–{bound:.3}px · threshold {threshold:.3}px · click to inspect"));
    if let Some(pos) = response.hover_pos() {
        let i = (((pos.x - rect.left()) / rect.width().max(1.0)) * changes.len() as f32).floor() as usize;
        let c = &changes[i.min(changes.len() - 1)];
        response.clone().on_hover_text(format!(
            "Frame {} · timeline {:.3}s · score {:?} px",
            c.frame_index + 1,
            c.timeline_tick as f64 / ticks,
            c.score_pixels
        ));
        if response.clicked() {
            if let Err(e) = crate::menus::invoke(app, ui.ctx(), "reverseEngineering.motionFrame", json!({"frameIndex":c.frame_index})) {
                app.ui.status = e;
            }
        }
    }
}

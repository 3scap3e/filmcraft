//! Motion-over-time graph and export provenance.
use super::{panel_state::ReverseEngineeringCapture, reverse_engineering};
use crate::FilmcraftApp;
use serde::{Deserialize, Serialize};
use serde_json::json;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeReport {
    pub schema_version: u32,
    pub analyzer: String,
    pub captures: Vec<ReverseEngineeringCapture>,
    pub source_motion_revision: u64,
    pub source_completed_at_unix_ms: u64,
    pub ticks_per_second: i64,
    pub samples: Vec<filmcraft_vision::motion_time::Sample>,
    pub limitations: Vec<String>,
}
pub(super) fn current(app: &FilmcraftApp) -> bool {
    if !reverse_engineering::motion_is_current(app) {
        return false;
    }
    let s = &app.ui.panels.reverse_engineering;
    match (&s.motion_time, &s.motion) {
        (Some(r), Some(f)) => {
            r.captures == f.captures
                && r.source_motion_revision == f.audit.revision
                && r.source_completed_at_unix_ms == f.audit.completed_runs.last().map(|r| r.completed_at_unix_ms).unwrap_or(0)
        }
        _ => false,
    }
}
pub(super) fn build(app: &mut FilmcraftApp) -> Result<(), String> {
    let f = app.ui.panels.reverse_engineering.motion.as_ref().ok_or("Analyze optical flow first")?;
    let samples = f.pairs.iter().map(filmcraft_vision::motion_time::summarize).collect::<Result<Vec<_>, _>>()?;
    app.ui.panels.reverse_engineering.motion_time = Some(TimeReport {
        schema_version: 1,
        analyzer: "filmcraft-vision/motion-over-time-v1".into(),
        captures: f.captures.clone(),
        source_motion_revision: f.audit.revision,
        source_completed_at_unix_ms: f.audit.completed_runs.last().map(|r| r.completed_at_unix_ms).unwrap_or(0),
        ticks_per_second: f.ticks_per_second,
        samples,
        limitations: vec![
            "Means of sampled SEA-RAFT vectors in analysis pixels per comparison. Positive X is right; positive Y is down.".into(),
            "Overall strength is mean vector magnitude, not magnitude of mean direction. Opposing directions can cancel the directional mean.".into(),
            "Missing samples are null and break plotted lines. No object identification or physical camera interpretation.".into(),
        ],
    });
    Ok(())
}
pub(super) fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    ui.heading("Motion over time");
    if super::reverse_motion::is_running(ui.ctx()) {
        super::reverse_motion::show(app, ui);
        return;
    }
    if !current(app) && reverse_engineering::motion_is_current(app) {
        if let Err(e) = build(app) {
            ui.colored_label(egui::Color32::LIGHT_RED, e);
            return;
        }
    }
    ui.label("Sampled image motion · analysis pixels per frame comparison");
    ui.label("X: right (+), left (−). Y: down (+), up (−). Strength: average movement regardless of direction.");
    if !current(app) {
        let b = ui.button("Analyze motion");
        if b.clicked() {
            let _ = crate::menus::invoke(app, ui.ctx(), "reverseEngineering.analyzeMotion", json!({}));
        }
    }
}
pub(super) fn graph(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    let Some(r) = app.ui.panels.reverse_engineering.motion_time.as_ref().filter(|_| current(app)) else {
        return;
    };
    let samples = r.samples.clone();
    if samples.is_empty() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        for (label, c) in [("X", egui::Color32::LIGHT_BLUE), ("Y", egui::Color32::LIGHT_GREEN), ("Strength", egui::Color32::YELLOW)] {
            ui.colored_label(c, label);
        }
    });
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width().max(1.0), 130.0), egui::Sense::click());
    let bound = samples.iter().flat_map(|s| [s.mean_dx_pixels, s.mean_dy_pixels, s.mean_magnitude_pixels]).flatten().fold(0.01f64, |a, b| a.max(b.abs()));
    let p = ui.painter().with_clip_rect(rect);
    p.line_segment([egui::pos2(rect.left(), rect.center().y), egui::pos2(rect.right(), rect.center().y)], egui::Stroke::new(1.0, egui::Color32::GRAY));
    let x = |i: usize| rect.left() + rect.width() * i as f32 / samples.len().saturating_sub(1).max(1) as f32;
    for (kind, color) in [(0, egui::Color32::LIGHT_BLUE), (1, egui::Color32::LIGHT_GREEN), (2, egui::Color32::YELLOW)] {
        let mut previous = None;
        for (i, s) in samples.iter().enumerate() {
            let v = match kind {
                0 => s.mean_dx_pixels,
                1 => s.mean_dy_pixels,
                _ => s.mean_magnitude_pixels,
            };
            if let Some(v) = v {
                let pos = egui::pos2(x(i), rect.center().y - (v / bound) as f32 * rect.height() * 0.45);
                if let Some(last) = previous {
                    p.line_segment([last, pos], egui::Stroke::new(1.5, color));
                }
                previous = Some(pos);
            } else {
                previous = None;
            }
        }
    }
    let frame = ui.ctx().data(|d| d.get_temp::<usize>(egui::Id::new("reverse-motion-frame"))).unwrap_or(0);
    if frame > 0 {
        let xx = x((frame - 1).min(samples.len() - 1));
        p.line_segment([egui::pos2(xx, rect.top()), egui::pos2(xx, rect.bottom())], egui::Stroke::new(1.0, egui::Color32::WHITE));
    }
    ui.label(format!("Vertical scale ±{bound:.3} px · click to inspect a frame"));
    if let (Some(first), Some(last)) = (samples.first(), samples.last()) {
        let ticks = r.ticks_per_second.max(1) as f64;
        ui.label(format!("Timeline {:.3}s → {:.3}s", first.to_tick as f64 / ticks, last.to_tick as f64 / ticks));
    }
    if frame > 0 {
        if let Some(sample) = samples.get(frame - 1) {
            let value = |v: Option<f64>| v.map(|v| format!("{v:.4}")).unwrap_or_else(|| "unavailable".into());
            ui.label(format!(
                "Selected: X {} · Y {} · strength {} px",
                value(sample.mean_dx_pixels),
                value(sample.mean_dy_pixels),
                value(sample.mean_magnitude_pixels)
            ));
        }
    }

    if let Some(pos) = response.hover_pos() {
        let i = (((pos.x - rect.left()) / rect.width().max(1.0)) * (samples.len() - 1) as f32).round() as usize;
        let i = i.min(samples.len() - 1);
        let s = &samples[i];
        response.clone().on_hover_text(format!(
            "Frame {} · {} samples\nX {:?} · Y {:?} · strength {:?} px",
            i + 2,
            s.sample_count,
            s.mean_dx_pixels,
            s.mean_dy_pixels,
            s.mean_magnitude_pixels
        ));
        if response.clicked() {
            if let Err(e) = crate::menus::invoke(app, ui.ctx(), "reverseEngineering.motionFrame", json!({"frameIndex":i+1})) {
                app.ui.status = e;
            }
        }
    }
}

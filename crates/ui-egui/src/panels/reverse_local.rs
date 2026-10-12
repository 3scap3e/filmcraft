//! Local residual-motion presentation and serializable evidence.
use super::{panel_state::ReverseEngineeringCapture, reverse_tools};
use crate::FilmcraftApp;
use filmcraft_vision::{
    display::{MotionDisplay, MotionDisplays},
    local_motion::{LocalMotion, estimate},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalReport {
    pub schema_version: u32,
    pub analyzer: String,
    pub source_motion_revision: u64,
    pub source_flow_completed_at_unix_ms: u64,
    pub source_global_completed_at_unix_ms: u64,
    pub residual_tolerance_pixels: f64,
    pub captures: Vec<ReverseEngineeringCapture>,
    pub pairs: Vec<LocalMotion>,
    pub global_transforms: Vec<Option<[f64; 4]>>,
    pub display: MotionDisplays,
    pub limitations: Vec<String>,
}
pub(super) fn current(app: &FilmcraftApp) -> bool {
    if !reverse_tools::current_global(app) {
        return false;
    }
    let s = &app.ui.panels.reverse_engineering;
    match (&s.local_motion, &s.global_motion) {
        (Some(local), Some(global)) => {
            local.captures == global.captures
                && local.source_motion_revision == global.source_motion_revision
                && local.source_flow_completed_at_unix_ms == global.source_completed_at_unix_ms
                && local.source_global_completed_at_unix_ms == global.completed_at_unix_ms
                && local.residual_tolerance_pixels == global.residual_tolerance_pixels
        }
        _ => false,
    }
}
pub(super) fn build(app: &mut FilmcraftApp) -> Result<Value, String> {
    if !reverse_tools::current_global(app) {
        return Err("Local motion needs a current global fit".into());
    }
    let s = &app.ui.panels.reverse_engineering;
    let flow = s.motion.as_ref().ok_or("Missing optical flow")?;
    let global = s.global_motion.as_ref().ok_or("Missing global fit")?;
    if flow.pairs.len() != global.pairs.len() || flow.pairs.is_empty() || flow.pairs.len() > 299 {
        return Err("Global fit does not match the optical-flow comparisons".into());
    }
    let pairs = flow.pairs.iter().zip(&global.pairs).map(|(flow, global)| estimate(flow, global)).collect::<Result<Vec<_>, _>>()?;
    let display = s.local_motion.as_ref().filter(|old| old.captures == flow.captures).map(|old| old.display.clone()).unwrap_or_default();
    let report=LocalReport {schema_version:1,analyzer:"filmcraft-vision/local-residual-motion-v1".into(),
        source_motion_revision:global.source_motion_revision,source_flow_completed_at_unix_ms:global.source_completed_at_unix_ms,
        source_global_completed_at_unix_ms:global.completed_at_unix_ms,residual_tolerance_pixels:global.residual_tolerance_pixels,
        captures:flow.captures.clone(),pairs,global_transforms:global.pairs.iter().map(|pair|match pair {
            filmcraft_vision::global_motion::GlobalMotion::Estimated {fit} => Some(fit.transform),
            filmcraft_vision::global_motion::GlobalMotion::InsufficientEvidence {..} => None,
        }).collect(),display,
        limitations:vec!["Residual = observed SEA-RAFT displacement minus fitted global displacement at the same source position, in analysis pixels.".into(),
            "Residual motion is not object identification or proof of independent subject motion. Parallax, occlusion, cuts and model errors also produce residuals.".into(),
            "No residual is produced when the global fit has insufficient evidence. Samples retain their original SEA-RAFT indices.".into(),
            "Arrows start at the observed position in the current frame. Arrow scale and cutoff affect presentation only.".into()]};
    app.ui.panels.reverse_engineering.local_motion = Some(report);
    app.ui.status = "Local motion ready. Inspect residuals and save the result.".into();
    Ok(json!({"computed":true}))
}
pub(super) fn set_display(app: &mut FilmcraftApp, params: &Value) -> Result<Value, String> {
    if !current(app) {
        return Err("Analyze local motion for the current range first".into());
    }
    let index = params.get("index").and_then(Value::as_u64).and_then(|v| usize::try_from(v).ok()).ok_or("Supply a frame-pair index")?;
    let scope = params.get("scope").and_then(Value::as_str).ok_or("Supply display scope")?;
    let report = app.ui.panels.reverse_engineering.local_motion.as_mut().ok_or("Missing local motion")?;
    let settings: MotionDisplay = if scope == "inherit" {
        report.display.effective(index)
    } else {
        serde_json::from_value(params.get("display").cloned().ok_or("Supply display settings")?).map_err(|e| e.to_string())?
    };
    report.display.apply(scope, index, report.pairs.len(), settings)?;
    Ok(json!({"scope":scope,"index":index,"display":report.display.effective(index)}))
}
pub(super) fn caption(app: &FilmcraftApp, ui: &mut egui::Ui, index: usize) {
    let Some(pair) = app.ui.panels.reverse_engineering.local_motion.as_ref().and_then(|r| r.pairs.get(index)) else {
        return;
    };
    match pair {
        LocalMotion::Estimated { samples, mean_residual_pixels, max_residual_pixels, .. } => {
            let mean_observed = if samples.is_empty() {
                0.0
            } else {
                samples.iter().map(|s| s.observed_displacement_pixels[0].hypot(s.observed_displacement_pixels[1])).sum::<f64>() / samples.len() as f64
            };
            ui.label(if app.ui.panels.reverse_engineering.local_show_raw {
                "Showing raw SEA-RAFT displacement"
            } else {
                "Showing residual displacement after global subtraction"
            });
            ui.label(format!(
                "{} samples · raw mean {:.4}px · residual mean {:.4}px · residual max {:.4}px",
                samples.len(),
                mean_observed,
                mean_residual_pixels,
                max_residual_pixels
            ));
        }
        LocalMotion::Unavailable { reason } => {
            ui.label(reason);
        }
    }
}
pub(super) fn overlay(app: &FilmcraftApp, ui: &egui::Ui, index: usize, rect: egui::Rect) {
    if !current(app) {
        return;
    }
    let Some(report) = app.ui.panels.reverse_engineering.local_motion.as_ref() else {
        return;
    };
    let Some(LocalMotion::Estimated { samples, analyzed_size, .. }) = report.pairs.get(index) else {
        return;
    };
    let [w, h] = *analyzed_size;
    if w == 0 || h == 0 || w > 640 || h > 640 {
        return;
    }
    let settings = report.display.effective(index);
    let painter = ui.painter().with_clip_rect(rect);
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let mut nearest: Option<(&filmcraft_vision::local_motion::ResidualSample, f32)> = None;
    for sample in samples.iter().take(2048) {
        let from = sample.source_position.map(|v| v as f32);
        let [dx, dy] = if app.ui.panels.reverse_engineering.local_show_raw { sample.observed_displacement_pixels } else { sample.residual_displacement_pixels };
        let grid_settings = MotionDisplay { cutoff_pixels: 0.0, ..settings };
        if !grid_settings.includes(from, from, *analyzed_size)
            || !dx.is_finite()
            || !dy.is_finite()
            || dx.hypot(dy) < f64::from(settings.cutoff_pixels)
            || !sample.observed_position.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v))
        {
            continue;
        }
        let position = rect.min + egui::vec2(sample.observed_position[0] as f32 * rect.width(), sample.observed_position[1] as f32 * rect.height());
        let displacement = egui::vec2((dx / w as f64) as f32 * rect.width(), (dy / h as f64) as f32 * rect.height()) * settings.arrow_scale;
        if !displacement.is_finite() {
            continue;
        }
        if displacement.length_sq() > 0.0 {
            painter.arrow(position, displacement, egui::Stroke::new(1.0, egui::Color32::from_rgb(130, 225, 235)));
        }
        if let Some(pointer) = pointer.filter(|p| rect.contains(*p)) {
            let distance = pointer.distance(position);
            if distance <= 10.0 && nearest.as_ref().is_none_or(|(_, previous)| distance < *previous) {
                nearest = Some((sample, distance));
            }
        }
    }
    if let Some((sample, _)) = nearest {
        let response = ui.interact(rect, egui::Id::new("reverse-local-measurement"), egui::Sense::hover());
        response.on_hover_text(format!(
            "SEA-RAFT sample {}\nObserved Δx {:.4}px · Δy {:.4}px\nGlobal Δx {:.4}px · Δy {:.4}px\nResidual Δx {:.4}px · Δy {:.4}px",
            sample.source_sample_index,
            sample.observed_displacement_pixels[0],
            sample.observed_displacement_pixels[1],
            sample.global_displacement_pixels[0],
            sample.global_displacement_pixels[1],
            sample.residual_displacement_pixels[0],
            sample.residual_displacement_pixels[1]
        ));
    }
}

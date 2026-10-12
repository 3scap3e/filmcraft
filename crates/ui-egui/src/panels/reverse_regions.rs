//! Motion-region tool; groups measured residual samples without identifying objects.
use super::{panel_state::ReverseEngineeringCapture, reverse_local};
use crate::FilmcraftApp;
use filmcraft_vision::motion_regions::{Regions, Settings, group};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegionReport {
    pub schema_version: u32,
    pub analyzer: String,
    pub source_motion_revision: u64,
    pub source_flow_completed_at_unix_ms: u64,
    pub source_global_completed_at_unix_ms: u64,
    pub residual_tolerance_pixels: f64,
    pub captures: Vec<ReverseEngineeringCapture>,
    pub all_frames: Settings,
    pub frame_overrides: BTreeMap<usize, Settings>,
    pub pairs: Vec<Regions>,
    pub limitations: Vec<String>,
}
pub(super) fn current(app: &FilmcraftApp) -> bool {
    if !reverse_local::current(app) {
        return false;
    }
    let s = &app.ui.panels.reverse_engineering;
    match (&s.motion_regions, &s.local_motion) {
        (Some(regions), Some(local)) => {
            regions.captures == local.captures
                && regions.source_motion_revision == local.source_motion_revision
                && regions.source_flow_completed_at_unix_ms == local.source_flow_completed_at_unix_ms
                && regions.source_global_completed_at_unix_ms == local.source_global_completed_at_unix_ms
                && regions.residual_tolerance_pixels == local.residual_tolerance_pixels
        }
        _ => false,
    }
}
pub(super) fn build(app: &mut FilmcraftApp) -> Result<Value, String> {
    if !reverse_local::current(app) {
        return Err("Prepare local residual motion first".into());
    }
    let s = &app.ui.panels.reverse_engineering;
    let local = s.local_motion.as_ref().ok_or("Missing residual motion")?;
    if local.pairs.is_empty() || local.pairs.len() > 299 {
        return Err("Invalid region range".into());
    }
    let previous = s.motion_regions.as_ref().filter(|r| r.captures == local.captures);
    let all_frames = previous.map(|r| r.all_frames).unwrap_or_default().validate()?;
    let overrides = previous.map(|r| r.frame_overrides.clone()).unwrap_or_default();
    if overrides.len() > 299 || overrides.keys().any(|i| *i >= local.pairs.len()) {
        return Err("Invalid region settings overrides".into());
    }
    let pairs = local.pairs.iter().enumerate().map(|(i, p)| group(p, overrides.get(&i).copied().unwrap_or(all_frames))).collect::<Result<Vec<_>, _>>()?;
    let report=RegionReport {schema_version:1,analyzer:"filmcraft-vision/residual-regions-v1".into(),source_motion_revision:local.source_motion_revision,
        source_flow_completed_at_unix_ms:local.source_flow_completed_at_unix_ms,source_global_completed_at_unix_ms:local.source_global_completed_at_unix_ms,
        residual_tolerance_pixels:local.residual_tolerance_pixels,captures:local.captures.clone(),all_frames,frame_overrides:overrides,pairs,
        limitations:vec!["Groups above-cutoff residual samples using four neighboring source-grid cells and the difference between their residual vectors.".into(),
            "Groups are connected transitively: neighboring vectors must agree, but a group may contain gradual changes in direction or speed.".into(),
            "Filled cells and bounds show sample coverage, not object contours or segmentation masks. IDs are local to each frame comparison, not persistent tracks.".into(),
            "Motion groups can include model errors, parallax, illumination and cuts. Unavailable residual comparisons remain unavailable.".into()]};
    app.ui.panels.reverse_engineering.motion_regions = Some(report);
    app.ui.status = "Motion regions ready. Inspect sample groups and adjust thresholds.".into();
    Ok(json!({"computed":true}))
}
pub(super) fn set_options(app: &mut FilmcraftApp, params: &Value) -> Result<Value, String> {
    if !current(app) {
        return Err("Analyze regions for the current range first".into());
    }
    let index = params.get("index").and_then(Value::as_u64).and_then(|v| usize::try_from(v).ok()).ok_or("Supply a frame comparison index")?;
    let scope = params.get("scope").and_then(Value::as_str).ok_or("Supply scope: frame, all, inherit")?;
    let s = &app.ui.panels.reverse_engineering;
    let old = s.motion_regions.as_ref().ok_or("Missing regions")?;
    let local = s.local_motion.as_ref().ok_or("Missing residuals")?;
    if index >= local.pairs.len() || local.pairs.len() > 299 {
        return Err("Frame outside region range".into());
    }
    let settings = if scope == "inherit" {
        old.all_frames
    } else {
        serde_json::from_value::<Settings>(params.get("settings").cloned().ok_or("Supply region settings")?).map_err(|e| e.to_string())?.validate()?
    };
    let mut report = old.clone();
    match scope {
        "all" => {
            report.all_frames = settings;
            report.frame_overrides.clear();
            report.pairs = local.pairs.iter().map(|p| group(p, settings)).collect::<Result<Vec<_>, _>>()?;
        }
        "frame" | "inherit" => {
            let pair = local.pairs.get(index).ok_or("Missing residual comparison")?;
            let updated = group(pair, settings)?;
            let slot = report.pairs.get_mut(index).ok_or("Missing region comparison")?;
            *slot = updated;
            if scope == "frame" {
                report.frame_overrides.insert(index, settings);
            } else {
                report.frame_overrides.remove(&index);
            }
        }
        _ => return Err("Region scope must be frame, all or inherit".into()),
    }
    app.ui.panels.reverse_engineering.motion_regions = Some(report);
    Ok(json!({"updated":true,"scope":scope,"index":index}))
}
fn invoke(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str, params: Value) {
    if let Err(error) = crate::menus::invoke(app, ctx, id, params) {
        app.ui.status = error.clone();
        ctx.data_mut(|data| data.insert_temp(egui::Id::new("reverse-tools-error"), error));
    }
}
pub(super) fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    ui.heading("Motion regions");
    ui.label("Neighboring residual samples with similar movement. These are not identified objects.");
    if super::reverse_motion::is_running(ui.ctx()) {
        super::reverse_motion::show(app, ui);
        return;
    }
    if super::reverse_tools::is_running(ui.ctx()) {
        ui.label("Preparing the global fit for residual grouping…");
        let response = ui.button("Cancel preparation");
        app.auto.add("reverseEngineering.cancelGlobal", response.rect, "Cancel region prerequisite fitting");
        if response.clicked() {
            invoke(app, ui.ctx(), "reverseEngineering.cancelGlobal", json!({}));
        }
        return;
    }
    let response = ui.button(if current(app) { "Regroup range" } else { "Analyze motion regions" });
    app.auto.add("reverseEngineering.runRegions", response.rect, "Group residual motion across the selected timeline range");
    if response.clicked() {
        invoke(app, ui.ctx(), "reverseEngineering.runRegions", json!({}));
    }
    if !current(app) {
        return;
    }
    let frame = ui.ctx().data(|d| d.get_temp::<usize>(egui::Id::new("reverse-motion-frame"))).unwrap_or(0);
    if frame == 0 {
        ui.label("Select frame 2 or later to inspect regions.");
        return;
    }
    let index = frame - 1;
    let Some(report) = app.ui.panels.reverse_engineering.motion_regions.as_ref() else {
        return;
    };
    let mut settings = report.frame_overrides.get(&index).copied().unwrap_or(report.all_frames);
    let before = settings;
    let custom = report.frame_overrides.contains_key(&index);
    ui.horizontal_wrapped(|ui| {
        ui.label("Residual cutoff (px)");
        let response = ui.add(egui::DragValue::new(&mut settings.cutoff_pixels).range(0.0..=100.0).speed(0.01));
        app.auto.add("reverseEngineering.regions.cutoff", response.rect, "Exclude residual samples at or below this magnitude in analysis pixels");
        ui.label("Neighbor difference (px)");
        let response = ui.add(egui::DragValue::new(&mut settings.neighbor_difference_pixels).range(0.01..=100.0).speed(0.01));
        app.auto.add("reverseEngineering.regions.difference", response.rect, "Maximum difference between neighboring residual vectors");
        ui.label("Minimum samples");
        let response = ui.add(egui::DragValue::new(&mut settings.minimum_samples).range(1..=2048));
        app.auto.add("reverseEngineering.regions.minimum", response.rect, "Minimum number of connected samples per group");
    });
    if settings != before {
        invoke(app, ui.ctx(), "reverseEngineering.setRegionOptions", json!({"scope":"frame","index":index,"settings":settings}));
    }
    ui.horizontal_wrapped(|ui| {
        let response = ui.button("Apply to all");
        app.auto.add("reverseEngineering.regions.applyAll", response.rect, "Regroup all comparisons with these settings and clear overrides");
        if response.clicked() {
            invoke(app, ui.ctx(), "reverseEngineering.setRegionOptions", json!({"scope":"all","index":index,"settings":settings}));
        }
        let response = ui.add_enabled(custom, egui::Button::new("Use all-frame settings"));
        app.auto.add("reverseEngineering.regions.inherit", response.rect, "Restore shared settings for this comparison");
        if response.clicked() {
            invoke(app, ui.ctx(), "reverseEngineering.setRegionOptions", json!({"scope":"inherit","index":index}));
        }
    });
    ui.label("Changes regroup samples immediately; SEA-RAFT does not rerun. Colored cells show sampled coverage.");
}
pub(super) fn caption(app: &FilmcraftApp, ui: &mut egui::Ui, index: usize) {
    let Some(pair) = app.ui.panels.reverse_engineering.motion_regions.as_ref().and_then(|r| r.pairs.get(index)) else {
        return;
    };
    match pair {
        Regions::Estimated { regions, filtered_samples, discarded_groups, .. } => {
            ui.label(format!("{} motion groups · {filtered_samples} samples below cutoff · {discarded_groups} small groups discarded", regions.len()));
        }
        Regions::Unavailable { reason } => {
            ui.label(reason);
        }
    }
}
pub(super) fn overlay(app: &FilmcraftApp, ui: &egui::Ui, index: usize, rect: egui::Rect) {
    if !current(app) {
        return;
    }
    let Some(Regions::Estimated { regions, analyzed_size, .. }) = app.ui.panels.reverse_engineering.motion_regions.as_ref().and_then(|r| r.pairs.get(index))
    else {
        return;
    };
    let [w, h] = *analyzed_size;
    if w == 0 || h == 0 || w > 640 || h > 640 {
        return;
    }
    let painter = ui.painter().with_clip_rect(rect);
    let colors = [
        egui::Color32::from_rgb(100, 210, 235),
        egui::Color32::from_rgb(240, 195, 80),
        egui::Color32::from_rgb(225, 125, 190),
        egui::Color32::from_rgb(125, 220, 150),
    ];
    let position = |p: [f64; 2]| rect.min + egui::vec2(p[0] as f32 * rect.width(), p[1] as f32 * rect.height());
    let mut remaining = 2048usize;
    for region in regions.iter().take(2048) {
        let color = colors[region.id.saturating_sub(1) % colors.len()];
        for p in region.observed_positions.iter().take(remaining) {
            if p.iter().all(|v| v.is_finite() && (-8.0..=9.0).contains(v)) {
                let cell = egui::Rect::from_center_size(position(*p), egui::vec2(16.0 / w as f32 * rect.width(), 16.0 / h as f32 * rect.height()));
                painter.rect_filled(cell, 0.0, color.gamma_multiply(0.20));
            }
        }
        remaining = remaining.saturating_sub(region.observed_positions.len());
        let [x0, y0, x1, y1] = region.observed_bounds;
        if ![x0, y0, x1, y1].iter().all(|v| v.is_finite() && (-9.0..=10.0).contains(v)) || x1 < x0 || y1 < y0 {
            continue;
        }
        let bounds = egui::Rect::from_min_max(position([x0, y0]), position([x1, y1])).intersect(rect);
        if !bounds.is_positive() {
            continue;
        }
        painter.rect_stroke(bounds, 0.0, egui::Stroke::new(1.0, color), egui::StrokeKind::Inside);
        painter.text(bounds.min, egui::Align2::LEFT_TOP, format!("R{}", region.id), egui::FontId::proportional(12.0), color);
        let response = ui.interact(bounds, egui::Id::new(("reverse-region", index, region.id)), egui::Sense::hover());
        response.on_hover_text(format!(
            "Region {} · {} samples\nMean residual Δx {:.4}px · Δy {:.4}px\nMean magnitude {:.4}px · max {:.4}px\nSample coverage, not an object boundary",
            region.id,
            region.source_sample_indices.len(),
            region.mean_residual_pixels[0],
            region.mean_residual_pixels[1],
            region.mean_magnitude_pixels,
            region.maximum_magnitude_pixels
        ));
    }
}

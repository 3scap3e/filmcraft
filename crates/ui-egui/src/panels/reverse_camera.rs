//! Camera-language labels for the fitted global image motion. Static-scene assumption, not physical pose.
use super::{panel_state::ReverseEngineeringCapture, reverse_tools};
use crate::FilmcraftApp;
use filmcraft_vision::camera_moves::{self, Movement, Options, Summary};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraReport {
    pub schema_version: u32,
    pub analyzer: String,
    pub captures: Vec<ReverseEngineeringCapture>,
    pub source_motion_revision: u64,
    pub source_flow_completed_at_unix_ms: u64,
    pub source_global_completed_at_unix_ms: u64,
    pub residual_tolerance_pixels: f64,
    pub ticks_per_second: i64,
    pub summary: Summary,
    pub limitations: Vec<String>,
}
pub(super) fn current(app: &FilmcraftApp) -> bool {
    if !reverse_tools::current_global(app) {
        return false;
    }
    let s = &app.ui.panels.reverse_engineering;
    match (&s.camera_moves, &s.global_motion) {
        (Some(camera), Some(global)) => {
            camera.captures == global.captures
                && camera.source_motion_revision == global.source_motion_revision
                && camera.source_flow_completed_at_unix_ms == global.source_completed_at_unix_ms
                && camera.source_global_completed_at_unix_ms == global.completed_at_unix_ms
                && camera.residual_tolerance_pixels == global.residual_tolerance_pixels
                && camera.summary.options == s.camera_options
        }
        _ => false,
    }
}
pub(super) fn build(app: &mut FilmcraftApp) -> Result<Value, String> {
    if !reverse_tools::current_global(app) {
        return Err("Camera moves need a current global image-motion fit".into());
    }
    let s = &app.ui.panels.reverse_engineering;
    let flow = s.motion.as_ref().ok_or("Missing optical flow")?;
    let global = s.global_motion.as_ref().ok_or("Missing global fit")?;
    let summary = camera_moves::classify(&global.pairs, &s.camera_options)?;
    let report = CameraReport {
        schema_version: 1,
        analyzer: "filmcraft-vision/camera-moves-v1".into(),
        captures: global.captures.clone(),
        source_motion_revision: global.source_motion_revision,
        source_flow_completed_at_unix_ms: global.source_completed_at_unix_ms,
        source_global_completed_at_unix_ms: global.completed_at_unix_ms,
        residual_tolerance_pixels: global.residual_tolerance_pixels,
        ticks_per_second: flow.ticks_per_second,
        summary,
        limitations: vec![
            "Labels interpret the fitted 2D similarity of image motion as camera movement under a static-scene assumption: content moving right is labelled pan left, content growing is zoom in.".into(),
            "A large moving subject, parallax, cuts, flat regions or flow errors can produce wrong labels. No physical pose, focal length or dolly/pan separation is recovered.".into(),
            "Thresholds are presentation decisions in analysis pixels, scale fraction and degrees; changing them reclassifies the same fitted numbers.".into(),
        ],
    };
    app.ui.panels.reverse_engineering.camera_moves = Some(report);
    app.ui.status = "Camera moves classified; inspect segments and save the result.".into();
    Ok(json!({"computed":true}))
}
pub(super) fn options(app: &mut FilmcraftApp, params: &Value) -> Result<Value, String> {
    let s = &app.ui.panels.reverse_engineering;
    let number = |key: &str, current: f64| -> Result<f64, String> {
        match params.get(key) {
            Some(v) => v.as_f64().filter(|v| v.is_finite()).ok_or_else(|| format!("{key} must be a finite number")),
            None => Ok(current),
        }
    };
    let options = Options {
        move_pixels: number("movePixels", s.camera_options.move_pixels)?,
        zoom_fraction: number("zoomFraction", s.camera_options.zoom_fraction)?,
        roll_degrees: number("rollDegrees", s.camera_options.roll_degrees)?,
    };
    options.validate()?;
    app.ui.panels.reverse_engineering.camera_options = options;
    if reverse_tools::current_global(app) {
        build(app)?;
    }
    Ok(json!({"options":options}))
}
fn invoke(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str, params: Value) {
    if let Err(e) = crate::menus::invoke(app, ctx, id, params) {
        app.ui.status = e.clone();
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("reverse-tools-error"), e));
    } else {
        ctx.data_mut(|d| d.remove::<String>(egui::Id::new("reverse-tools-error")));
    }
}
fn movement_color(movement: Movement) -> egui::Color32 {
    match movement {
        Movement::Static => egui::Color32::from_gray(110),
        Movement::PanLeft | Movement::PanRight => egui::Color32::from_rgb(90, 160, 255),
        Movement::TiltUp | Movement::TiltDown => egui::Color32::from_rgb(120, 220, 140),
        Movement::ZoomIn | Movement::ZoomOut => egui::Color32::from_rgb(255, 190, 70),
        Movement::RollClockwise | Movement::RollCounterclockwise => egui::Color32::from_rgb(220, 120, 220),
        Movement::Mixed => egui::Color32::from_rgb(255, 110, 110),
        Movement::Unavailable => egui::Color32::from_gray(45),
    }
}
pub(super) fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    ui.heading("Camera moves");
    ui.label("Pan, tilt, zoom and roll labels from the fitted global image motion. Static-scene assumption.");
    if reverse_tools::is_running(ui.ctx()) {
        ui.label("Preparing the global fit…");
        let response = ui.button("Cancel preparation");
        app.auto.add("reverseEngineering.cancelGlobal", response.rect, "Cancel camera-moves prerequisite fitting");
        if response.clicked() {
            invoke(app, ui.ctx(), "reverseEngineering.cancelGlobal", json!({}));
        }
        return;
    }
    if super::reverse_motion::is_running(ui.ctx()) {
        ui.label("Preparing optical flow for camera moves…");
        super::reverse_motion::show(app, ui);
        return;
    }
    let mut options = app.ui.panels.reverse_engineering.camera_options;
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        ui.label("Movement ≥");
        let r = ui.add(egui::DragValue::new(&mut options.move_pixels).range(0.001..=1000.0).speed(0.01).suffix(" px"));
        app.auto.add("reverseEngineering.camera.movePixels", r.rect, "Centre displacement needed for pan/tilt");
        changed |= r.changed();
        ui.label("Zoom ≥");
        let r = ui.add(egui::DragValue::new(&mut options.zoom_fraction).range(0.00001..=1.0).speed(0.0005));
        app.auto.add("reverseEngineering.camera.zoomFraction", r.rect, "Scale change needed for zoom");
        changed |= r.changed();
        ui.label("Roll ≥");
        let r = ui.add(egui::DragValue::new(&mut options.roll_degrees).range(0.001..=90.0).speed(0.01).suffix("°"));
        app.auto.add("reverseEngineering.camera.rollDegrees", r.rect, "Rotation needed for roll");
        changed |= r.changed();
    });
    if changed {
        invoke(
            app,
            ui.ctx(),
            "reverseEngineering.setCameraOptions",
            json!({"movePixels":options.move_pixels,"zoomFraction":options.zoom_fraction,"rollDegrees":options.roll_degrees}),
        );
    }
    let response = ui.button(if current(app) { "Reclassify" } else { "Classify camera moves" });
    app.auto.add("reverseEngineering.runCamera", response.rect, "Classify camera movement with the required flow and global fit");
    if response.clicked() {
        invoke(app, ui.ctx(), "reverseEngineering.runCamera", json!({}));
    }
    let Some(report) = app.ui.panels.reverse_engineering.camera_moves.clone().filter(|_| current(app)) else {
        return;
    };
    let shares: Vec<String> = report.summary.shares.iter().map(|s| format!("{} {:.0}%", s.movement.label(), s.fraction * 100.0)).collect();
    let summary = format!("Dominant: {} · {}", report.summary.dominant.label(), shares.join(" · "));
    let response = ui.label(&summary);
    app.auto.add("reverseEngineering.camera.summary", response.rect, &summary);
    ui.label("Segments (comparison i describes motion into frame i+1):");
    ui.horizontal_wrapped(|ui| {
        for (i, s) in report.summary.segments.iter().enumerate().take(300) {
            let text = format!("{} · frames {}–{} ({})", s.movement.label(), s.start_comparison + 2, s.end_comparison + 2, s.comparisons);
            let response = ui.add(egui::Button::new(egui::RichText::new(text).color(movement_color(s.movement))));
            app.auto.add(&format!("reverseEngineering.camera.segment.{i}"), response.rect, "Inspect the first frame of this segment");
            let hover = format!(
                "mean Δx {} · Δy {} · scale {} · rotation {}",
                s.mean_dx_pixels.map(|v| format!("{v:.3}px")).unwrap_or_else(|| "n/a".into()),
                s.mean_dy_pixels.map(|v| format!("{v:.3}px")).unwrap_or_else(|| "n/a".into()),
                s.mean_scale.map(|v| format!("{v:.5}")).unwrap_or_else(|| "n/a".into()),
                s.mean_rotation_degrees.map(|v| format!("{v:.3}°")).unwrap_or_else(|| "n/a".into())
            );
            if response.on_hover_text(hover).clicked() {
                invoke(app, ui.ctx(), "reverseEngineering.motionFrame", json!({"frameIndex":s.start_comparison + 1}));
            }
        }
    });
}
pub(super) fn graph(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    let Some(report) = app.ui.panels.reverse_engineering.camera_moves.clone().filter(|_| current(app)) else {
        return;
    };
    let classes = &report.summary.classifications;
    if classes.is_empty() {
        return;
    }
    ui.label("Camera movement per comparison · click to inspect");
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width().max(1.0), 28.0), egui::Sense::click());
    app.auto.add("reverseEngineering.camera.bar", rect, "Camera movement labels along the range");
    let painter = ui.painter().with_clip_rect(rect);
    let width = rect.width() / classes.len() as f32;
    for (i, c) in classes.iter().enumerate() {
        let block = egui::Rect::from_min_size(egui::pos2(rect.left() + width * i as f32, rect.top()), egui::vec2(width.max(1.0), rect.height()));
        painter.rect_filled(block, 0.0, movement_color(c.movement));
    }
    let frame = ui.ctx().data(|d| d.get_temp::<usize>(egui::Id::new("reverse-motion-frame"))).unwrap_or(0);
    if frame > 0 {
        let x = rect.left() + width * (frame - 1).min(classes.len() - 1) as f32 + width / 2.0;
        painter.line_segment([egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())], egui::Stroke::new(2.0, egui::Color32::WHITE));
    }
    if let Some(pos) = response.hover_pos() {
        let i = (((pos.x - rect.left()) / rect.width().max(1.0)) * classes.len() as f32).floor().max(0.0) as usize;
        if let Some(c) = classes.get(i.min(classes.len() - 1)) {
            response.clone().on_hover_text(format!(
                "Into frame {} · {} · Δx {} · Δy {} · scale {} · rotation {}",
                c.comparison_index + 2,
                c.movement.label(),
                c.dx_pixels.map(|v| format!("{v:.3}")).unwrap_or_else(|| "n/a".into()),
                c.dy_pixels.map(|v| format!("{v:.3}")).unwrap_or_else(|| "n/a".into()),
                c.scale.map(|v| format!("{v:.5}")).unwrap_or_else(|| "n/a".into()),
                c.rotation_degrees.map(|v| format!("{v:.3}°")).unwrap_or_else(|| "n/a".into())
            ));
            if response.clicked() {
                invoke(app, ui.ctx(), "reverseEngineering.motionFrame", json!({"frameIndex":c.comparison_index + 1}));
            }
        }
    }
}
pub(super) fn caption(app: &FilmcraftApp, ui: &mut egui::Ui, index: usize) {
    let Some(c) = app.ui.panels.reverse_engineering.camera_moves.as_ref().filter(|_| current(app)).and_then(|r| r.summary.classifications.get(index)) else {
        return;
    };
    ui.label(format!(
        "Camera: {} · Δx {} · Δy {} · scale {} · rotation {}",
        c.movement.label(),
        c.dx_pixels.map(|v| format!("{v:.3}px")).unwrap_or_else(|| "n/a".into()),
        c.dy_pixels.map(|v| format!("{v:.3}px")).unwrap_or_else(|| "n/a".into()),
        c.scale.map(|v| format!("{v:.5}")).unwrap_or_else(|| "n/a".into()),
        c.rotation_degrees.map(|v| format!("{v:.3}°")).unwrap_or_else(|| "n/a".into())
    ));
}

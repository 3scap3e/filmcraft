//! Named-object SAM 2.1 masks; isolated from project edits and point tracking.
use super::{panel_state::ReverseEngineeringCapture, reverse_engineering};
use crate::FilmcraftApp;
use filmcraft_vision::{
    execution::Device,
    sam2::{self, Object, Prompt},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Geometry {
    pub area_pixels: usize,
    pub centroid: Option<[f64; 2]>,
    pub bounds: Option<[f64; 4]>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub schema_version: u32,
    pub captures: Vec<ReverseEngineeringCapture>,
    pub project_revision: u64,
    pub completed_at_unix_ms: u64,
    pub inference: sam2::Inference,
    pub geometry: Vec<Vec<Option<Geometry>>>,
    pub limitations: Vec<String>,
}
#[derive(Clone)]
struct Job {
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicUsize>,
    total: usize,
    result: Arc<Mutex<Option<Result<Report, String>>>>,
}
fn job_id() -> egui::Id {
    egui::Id::new("reverse-sam2-job")
}
pub(super) fn is_running(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<Job>(job_id())).is_some()
}
pub(super) fn current(app: &FilmcraftApp) -> bool {
    let s = &app.ui.panels.reverse_engineering;
    s.sam_report.as_ref().is_some_and(|r| {
        r.project_revision == app.session.revision
            && r.captures == s.sam_captures
            && reverse_engineering::captures_are_current(app, &r.captures)
            && r.inference.tracks.iter().map(|t| &t.object).eq(s.sam_objects.iter())
    })
}
pub(super) fn prepare(app: &mut FilmcraftApp) -> Result<(), String> {
    let captures = reverse_engineering::range_captures(app, usize::MAX)?;
    let s = &mut app.ui.panels.reverse_engineering;
    if captures != s.sam_captures {
        s.sam_captures = captures;
        s.sam_objects.clear();
        s.sam_frame = 0;
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
        match result {
            Ok(r) if !job.cancel.load(Ordering::Relaxed) => {
                app.ui.panels.reverse_engineering.sam_report = Some(r);
                app.ui.status = "SAM 2.1 masks ready; inspect and save.".into();
            }
            Ok(_) => app.ui.status = "SAM cancelled; previous results preserved".into(),
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
    if id == "reverseEngineering.cancelSam" {
        if let Some(job) = ctx.data(|d| d.get_temp::<Job>(job_id())) {
            job.cancel.store(true, Ordering::Relaxed);
        }
        return Ok(json!({"cancelled":true}));
    }
    if id == "reverseEngineering.setSamDisplay" {
        let state = &mut app.ui.panels.reverse_engineering;
        let binary = params.get("binary").map(|v| v.as_bool().ok_or("Supply a boolean binary view")).transpose()?.unwrap_or(state.sam_binary_view);
        let color: Option<[u8; 3]> = params.get("color").map(|v| serde_json::from_value(v.clone()).map_err(|e| e.to_string())).transpose()?;
        if let Some(color) = color {
            if state.sam_colors.len() >= 8 && !state.sam_colors.contains_key(&state.sam_name) {
                return Err("At most eight custom object colors".into());
            }
            state.sam_colors.insert(state.sam_name.clone(), color);
        }
        state.sam_binary_view = binary;
        ctx.request_repaint();
        return Ok(json!({"displayOnly":true}));
    }
    if is_running(ctx)
        || super::reverse_motion::is_running(ctx)
        || super::reverse_tools::is_running(ctx)
        || super::reverse_tracking::is_running(ctx)
        || super::reverse_shots::is_running(ctx)
        || super::reverse_camera2d::is_running(ctx)
    {
        return Err("Cancel the running analysis before changing masks".into());
    }
    prepare(app)?;
    if id == "reverseEngineering.samFrame" {
        let index = params.get("frameIndex").and_then(Value::as_u64).and_then(|v| usize::try_from(v).ok()).ok_or("Supply frameIndex")?;
        let capture = app.ui.panels.reverse_engineering.sam_captures.get(index).ok_or("Frame outside mask range")?.clone();
        app.ui.panels.reverse_engineering.sam_frame = index;
        app.stop();
        app.session.set_playhead(capture.timeline_time);
        return Ok(json!({"frameIndex":index}));
    }
    if id == "reverseEngineering.setSamOptions" {
        let s = &mut app.ui.panels.reverse_engineering;
        let name = params.get("name").and_then(Value::as_str).unwrap_or(&s.sam_name).trim().to_owned();
        if name.is_empty() || name.chars().count() > 80 {
            return Err("Use a name of 1–80 characters".into());
        }
        let device = if let Some(v) = params.get("device") { serde_json::from_value(v.clone()).map_err(|e| e.to_string())? } else { s.sam_device };
        s.sam_name = name;
        s.sam_device = device;
        s.sam_foreground = params.get("foreground").and_then(Value::as_bool).unwrap_or(s.sam_foreground);
        return Ok(json!({"updated":true}));
    }
    if id == "reverseEngineering.clearSamObject" {
        let s = &mut app.ui.panels.reverse_engineering;
        s.sam_objects.retain(|o| o.name != s.sam_name);
        return Ok(json!({"cleared":true}));
    }
    if id == "reverseEngineering.addSamPoint" {
        let position: [f64; 2] = serde_json::from_value(params.get("position").cloned().ok_or("Supply normalized XY")?).map_err(|e| e.to_string())?;
        if position.iter().any(|v| !v.is_finite() || !(0.0..1.0).contains(v)) {
            return Err("Invalid point coordinates".into());
        }
        let s = &mut app.ui.panels.reverse_engineering;
        if let Some(o) = s.sam_objects.iter_mut().find(|o| o.name == s.sam_name) {
            if o.frame_index != s.sam_frame {
                return Err("This object has clicks on another frame. Return to that frame to refine, or clear this object to choose a new reference.".into());
            }
            if o.points.len() >= 64 {
                return Err("At most 64 clicks per object".into());
            }
            o.points.push(Prompt { position, foreground: s.sam_foreground });
        } else {
            if !s.sam_foreground {
                return Err("Start an object with a foreground click".into());
            }
            if s.sam_objects.len() >= 8 {
                return Err("At most eight objects".into());
            }
            s.sam_objects.push(Object { name: s.sam_name.clone(), frame_index: s.sam_frame, points: vec![Prompt { position, foreground: true }] });
        }
        return Ok(json!({"added":true}));
    }
    if id != "reverseEngineering.runSam" {
        return Err("Unknown SAM command".into());
    }
    let s = &app.ui.panels.reverse_engineering;
    sam2::validate_objects(&s.sam_objects, s.sam_captures.len())?;
    let captures = s.sam_captures.clone();
    let objects = s.sam_objects.clone();
    let device = s.sam_device;
    let revision = app.session.revision;
    let project = app.session.project.clone();
    let provider = app.session.media.provider(project.clone(), app.session.services.clone());
    let job =
        Job { cancel: Arc::new(AtomicBool::new(false)), progress: Arc::new(AtomicUsize::new(0)), total: captures.len(), result: Arc::new(Mutex::new(None)) };
    let worker = job.clone();
    let repaint = ctx.clone();
    std::thread::Builder::new().name("filmcraft-sam2".into()).spawn(move||{let result=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||->Result<Report,String>{
  let now=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e|e.to_string())?.as_nanos();
  let path=std::env::temp_dir().join(format!("filmcraft-sam2-{}-{now}",std::process::id()));std::fs::create_dir(&path).map_err(|e|e.to_string())?;
  struct Frames(std::path::PathBuf);impl Drop for Frames{fn drop(&mut self){let _=std::fs::remove_dir_all(&self.0);}}let files=Frames(path);
  let mut size=None;let mut bytes=0u64;
  for (i,capture) in captures.iter().enumerate(){if worker.cancel.load(Ordering::Relaxed){return Err("SAM cancelled".into());}let source=project.source_size(capture.item).ok_or("No video dimensions")?;let scale=(320.0/source.0.max(source.1).max(1) as f32).min(1.0);let image=filmcraft_render::render_item(&project,capture.item,capture.media_time,scale,&provider).ok_or("Could not render SAM frame")?;let current=[image.w as usize,image.h as usize];if size.is_some_and(|s|s!=current){return Err("Rendered mask frames have inconsistent dimensions".into());}size=Some(current);let raw=image.to_rgba8();bytes=bytes.checked_add(raw.len() as u64).ok_or("Frame spool size overflow")?;if bytes>1_073_741_824{return Err("Rendered frames exceed the 1 GiB temporary storage budget".into());}std::fs::write(files.0.join(format!("{i}.rgba")),raw).map_err(|e|format!("Could not store temporary SAM frame: {e}"))?;worker.progress.store(i+1,Ordering::Relaxed);repaint.request_repaint();}
  let [w,h]=size.ok_or("No frames")?;let inference=sam2::infer(w,h,files.0.clone(),captures.len(),objects,&worker.cancel,device)?;
  let geometry=inference.tracks.iter().map(|t|t.masks.iter().map(|m|m.as_ref().map(|m|m.measurements([w,h]).map(|(area_pixels,centroid,bounds)|Geometry{area_pixels,centroid,bounds})).transpose()).collect::<Result<Vec<_>,String>>()).collect::<Result<Vec<_>,String>>()?;
  Ok(Report{schema_version:1,captures,project_revision:revision,completed_at_unix_ms:(now/1_000_000).min(u64::MAX as u128) as u64,inference,geometry,limitations:vec!["Masks are SAM 2.1 predictions from user-labelled foreground/background clicks, not semantic recognition or ground truth.".into(),"Masks use rendered input dimensions up to 320 pixels on the longest side; area is measured in mask pixels, centroid/bounds are normalized.".into(),"Video processing uses overlapping 32-frame windows with carried masks; errors can accumulate. Empty endpoints stop continuation; missing masks are null.".into(),"Reference-frame corrections rerun the full selected range. No silent CPU fallback after an explicitly requested GPU failure.".into()]})
 })).unwrap_or_else(|_|Err("SAM worker failed; previous results preserved".into()));*worker.result.lock().unwrap_or_else(std::sync::PoisonError::into_inner)=Some(result);repaint.request_repaint();}).map_err(|e|e.to_string())?;
    ctx.data_mut(|d| d.insert_temp(job_id(), job));
    Ok(json!({"started":true}))
}
fn invoke(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str, params: Value) {
    if let Err(e) = crate::menus::invoke(app, ctx, id, params) {
        app.ui.status = e.clone();
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("reverse-tools-error"), e));
    } else {
        ctx.data_mut(|d| d.remove::<String>(egui::Id::new("reverse-tools-error")));
    }
}
pub(super) fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    ui.heading("Object masks · SAM 2.1");
    if let Some(job) = ui.ctx().data(|d| d.get_temp::<Job>(job_id())) {
        let done = job.progress.load(Ordering::Relaxed);
        ui.spinner();
        ui.label(if done < job.total { format!("Preparing frames · {done}/{}", job.total) } else { "Segmenting full range in 32-frame windows…".into() });
        let response = ui.button("Cancel segmentation");
        app.auto.add("reverseEngineering.cancelSam", response.rect, "Cancel segmentation");
        if response.clicked() {
            invoke(app, ui.ctx(), "reverseEngineering.cancelSam", json!({}));
        }
        return;
    }
    if let Err(e) = prepare(app) {
        ui.label(e);
        return;
    }
    let mut name = app.ui.panels.reverse_engineering.sam_name.clone();
    ui.horizontal_wrapped(|ui| {
        ui.label("Object");
        let response = ui.add(egui::TextEdit::singleline(&mut name).char_limit(80).desired_width(120.0));
        app.auto.add("reverseEngineering.sam.name", response.rect, "Object name");
        if response.changed() && !name.trim().is_empty() {
            invoke(app, ui.ctx(), "reverseEngineering.setSamOptions", json!({"name":name}));
        }
    });
    let objects = app.ui.panels.reverse_engineering.sam_objects.clone();
    ui.horizontal_wrapped(|ui| {
        for o in &objects {
            let response = ui.selectable_label(
                o.name == app.ui.panels.reverse_engineering.sam_name,
                egui::RichText::new(format!("{} · frame {} ({})", o.name, o.frame_index + 1, o.points.len())).color(object_color(app, &o.name)),
            );
            app.auto.add(&format!("reverseEngineering.sam.object.{}", o.name), response.rect, "Choose mask object");
            if response.clicked() {
                invoke(app, ui.ctx(), "reverseEngineering.setSamOptions", json!({"name":o.name}));
                invoke(app, ui.ctx(), "reverseEngineering.samFrame", json!({"frameIndex":o.frame_index}));
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        for (binary, label) in [(false, "Color overlay"), (true, "Mask only")] {
            let response = ui.selectable_label(app.ui.panels.reverse_engineering.sam_binary_view == binary, label);
            app.auto.add(&format!("reverseEngineering.sam.view.{binary}"), response.rect, label);
            if response.clicked() {
                invoke(app, ui.ctx(), "reverseEngineering.setSamDisplay", json!({"binary":binary}));
            }
        }
        ui.label("Mask color");
        let color = object_color(app, &app.ui.panels.reverse_engineering.sam_name);
        let mut rgb = [color.r(), color.g(), color.b()];
        let response = egui::color_picker::color_edit_button_srgb(ui, &mut rgb);
        app.auto.add("reverseEngineering.sam.maskColor", response.rect, "Color for the selected object's mask");
        if response.changed() {
            invoke(app, ui.ctx(), "reverseEngineering.setSamDisplay", json!({"color":rgb}));
        }
    });
    if app.ui.panels.reverse_engineering.sam_binary_view {
        ui.label("Selected object: white. Everything else: black. Switch to Color overlay to edit clicks.");
    }
    ui.horizontal_wrapped(|ui| {
        for (value, label) in [(true, "Include (+)"), (false, "Exclude (−)")] {
            let response = ui.selectable_label(app.ui.panels.reverse_engineering.sam_foreground == value, label);
            app.auto.add(&format!("reverseEngineering.sam.foreground.{value}"), response.rect, label);
            if response.clicked() {
                invoke(app, ui.ctx(), "reverseEngineering.setSamOptions", json!({"foreground":value}));
            }
        }
    });
    ui.label("Click inside the object on a visible frame. Add exclude clicks on the same frame to refine it, then analyze again.");
    ui.horizontal_wrapped(|ui| {
        ui.label("Execution");
        for device in [Device::Auto, Device::Cpu, Device::Gpu] {
            let response = ui.selectable_label(app.ui.panels.reverse_engineering.sam_device == device, device.as_str().to_uppercase());
            app.auto.add(&format!("reverseEngineering.sam.device.{}", device.as_str()), response.rect, "Execution device");
            if response.clicked() {
                invoke(app, ui.ctx(), "reverseEngineering.setSamOptions", json!({"device":device}));
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        for (id, label) in [("clearSamObject", "Clear this object"), ("runSam", "Analyze masks")] {
            let response = ui.add_enabled(!objects.is_empty(), egui::Button::new(label));
            app.auto.add(&format!("reverseEngineering.{id}"), response.rect, label);
            if response.clicked() {
                invoke(app, ui.ctx(), &format!("reverseEngineering.{id}"), json!({}));
            }
        }
    });
    if !current(app) && app.ui.panels.reverse_engineering.sam_report.is_some() {
        ui.label("Prompts changed. Analyze again to update masks.");
    }
}
pub(super) fn controls(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    let s = &app.ui.panels.reverse_engineering;
    let count = s.sam_captures.len();
    if count == 0 {
        return;
    }
    let frame = s.sam_frame.min(count - 1);
    ui.add_enabled_ui(!is_running(ui.ctx()), |ui| {
        ui.horizontal_wrapped(|ui| {
            for (label, i) in [("First", 0), ("Previous", frame.saturating_sub(1)), ("Next", (frame + 1).min(count - 1)), ("Last", count - 1)] {
                let response = ui.button(label);
                app.auto.add(&format!("reverseEngineering.sam.{label}"), response.rect, label);
                if response.clicked() {
                    invoke(app, ui.ctx(), "reverseEngineering.samFrame", json!({"frameIndex":i}));
                }
            }
            let mut jump = frame + 1;
            let response = ui.add(egui::DragValue::new(&mut jump).range(1..=count));
            app.auto.add("reverseEngineering.sam.jump", response.rect, "Jump to range frame");
            if response.changed() {
                invoke(app, ui.ctx(), "reverseEngineering.samFrame", json!({"frameIndex":jump-1}));
            }
        });
    });
    let s = &app.ui.panels.reverse_engineering;
    if let Some(c) = s.sam_captures.get(s.sam_frame) {
        ui.label(format!("Frame {}/{} · {}", s.sam_frame + 1, count, reverse_engineering::timecode(app, c)));
    }
    if current(app)
        && let Some(r) = s.sam_report.as_ref()
    {
        for (i, t) in r.inference.tracks.iter().enumerate() {
            let color = object_color(app, &t.object.name);
            if let Some(Some(g)) = r.geometry.get(i).and_then(|v| v.get(s.sam_frame)) {
                let center = g.centroid.map(|[x, y]| format!("{x:.4}, {y:.4}")).unwrap_or_else(|| "empty prediction".into());
                ui.colored_label(color, format!("{} · {} mask pixels · centroid {center}", t.object.name, g.area_pixels));
            } else {
                ui.colored_label(color, format!("{} · no mask prediction for this frame", t.object.name));
            }
        }
    }
}
pub(super) fn result(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    let s = &app.ui.panels.reverse_engineering;
    let captures = s.sam_captures.clone();
    let frame = s.sam_frame.min(captures.len().saturating_sub(1));
    if let Some(c) = captures.get(frame) {
        reverse_engineering::selected_picture(app, ui, c, 104.0);
        reverse_engineering::frame_strip(app, ui, &captures, frame, true);
    }
}
pub(super) fn overlay(app: &mut FilmcraftApp, ui: &egui::Ui, rect: egui::Rect) {
    let s = &app.ui.panels.reverse_engineering;
    let frame = s.sam_frame;
    let painter = ui.painter().with_clip_rect(rect);
    draw_masks(app, ui, rect, frame);
    for o in s.sam_objects.iter().filter(|_| !s.sam_binary_view) {
        if o.frame_index == frame {
            for point in &o.points {
                let p = rect.min + egui::vec2(point.position[0] as f32 * rect.width(), point.position[1] as f32 * rect.height());
                let color = if point.foreground { object_color(app, &o.name) } else { egui::Color32::LIGHT_RED };
                painter.circle_stroke(p, 4.0, egui::Stroke::new(2.0, color));
                painter.text(
                    p + egui::vec2(5.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    if point.foreground { "+" } else { "−" },
                    egui::FontId::proportional(12.0),
                    color,
                );
            }
        }
    }
    if !is_running(ui.ctx()) && !s.sam_binary_view {
        let response = ui.interact(rect.intersect(ui.clip_rect()), egui::Id::new("reverse-sam-point-pick"), egui::Sense::click());
        if response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            invoke(
                app,
                ui.ctx(),
                "reverseEngineering.addSamPoint",
                json!({"position":[((pos.x-rect.left())/rect.width().max(1.0)).clamp(0.0,0.999999),((pos.y-rect.top())/rect.height().max(1.0)).clamp(0.0,0.999999)]}),
            );
        }
    }
}

#[derive(Clone)]
struct MaskTextures {
    stamp: u64,
    frame: usize,
    display_key: String,
    textures: Vec<egui::TextureHandle>,
}
/// Paint only model mask pixels; shared by the large frame and filmstrip thumbnails.
pub(super) fn draw_masks(app: &FilmcraftApp, ui: &egui::Ui, rect: egui::Rect, frame: usize) {
    if !current(app) {
        return;
    }
    let Some(report) = app.ui.panels.reverse_engineering.sam_report.as_ref() else {
        return;
    };
    let [w, h] = report.inference.mask_size;
    if !(1..=320).contains(&w) || !(1..=320).contains(&h) {
        return;
    }
    let state = &app.ui.panels.reverse_engineering;
    let binary = state.sam_binary_view;
    let display_key = format!("{}:{}:{:?}", binary, state.sam_name, state.sam_colors);
    if binary {
        ui.painter().with_clip_rect(rect.intersect(ui.clip_rect())).rect_filled(rect, 0.0, egui::Color32::BLACK);
    }
    let id = egui::Id::new("reverse-sam-mask-preview-cache-v2");
    let mut cache = ui.ctx().data(|d| d.get_temp::<Vec<MaskTextures>>(id)).unwrap_or_default();
    cache.retain(|entry| entry.stamp == report.completed_at_unix_ms && entry.display_key == display_key);
    let entry = if let Some(index) = cache.iter().position(|entry| entry.frame == frame) {
        cache.remove(index)
    } else {
        let mut textures = Vec::new();
        for (i, track) in report.inference.tracks.iter().enumerate() {
            if binary && track.object.name != state.sam_name {
                continue;
            }
            let Some(Some(mask)) = track.masks.get(frame) else {
                continue;
            };
            if mask.runs.is_empty() {
                continue;
            }
            let Ok(display) = mask.display_pixels([w, h]) else {
                continue;
            };
            let color = object_color(app, &track.object.name);
            let pixels = display
                .into_iter()
                .map(|v| {
                    if binary {
                        if v > 0 { egui::Color32::WHITE } else { egui::Color32::TRANSPARENT }
                    } else {
                        match v {
                            1 => egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 155),
                            2 => egui::Color32::WHITE,
                            _ => egui::Color32::TRANSPARENT,
                        }
                    }
                })
                .collect();
            textures.push(ui.ctx().load_texture(format!("sam-mask-{frame}-{i}"), egui::ColorImage::new([w, h], pixels), egui::TextureOptions::NEAREST));
        }
        MaskTextures { stamp: report.completed_at_unix_ms, frame, display_key, textures }
    };
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    for texture in &entry.textures {
        painter.image(texture.id(), rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
    }
    if cache.len() >= 16 {
        cache.remove(0);
    }
    cache.push(entry);
    ui.ctx().data_mut(|d| d.insert_temp(id, cache));
}

fn object_color(app: &FilmcraftApp, name: &str) -> egui::Color32 {
    app.ui
        .panels
        .reverse_engineering
        .sam_colors
        .get(name)
        .map(|[r, g, b]| egui::Color32::from_rgb(*r, *g, *b))
        .unwrap_or_else(|| super::reverse_tracking::group_color(name))
}

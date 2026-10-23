//! Generate panel: describe a shot, pick its length and an optional still, choose where it goes,
//! and get an AI-generated clip on the timeline. Every action is a `generate.*` engine command
//! (`generate.video` runs as a background job; the engine imports and places the clip when it
//! finishes), so the panel, MCP and the control channel share one path.
//!
//! Automation ids: `generatePanel.prompt`, `generatePanel.seconds`, `generatePanel.chooseImage`,
//! `generatePanel.clearImage`, `generatePanel.imageRole.<first|last|both>`,
//! `generatePanel.atPlayhead`, `generatePanel.atCustom`, `generatePanel.atSeconds`,
//! `generatePanel.track`, `generatePanel.insert`, `generatePanel.run`, `generatePanel.cancel`,
//! `generatePanel.settings`. UI commands: `generatePanel.run`, `generatePanel.cancel`,
//! `generatePanel.chooseImage {path?}`, `generatePanel.clearImage`, `generatePanel.settings`.

use egui::{Rect, pos2, vec2};
use filmcraft_generate::ImageRole;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::FilmcraftApp;
use crate::theme::Tokens;

const PAD: f32 = 14.0;
const FOOTER_H: f32 = 76.0;
const IMAGE_EXTENSIONS: [&str; 7] = ["png", "jpg", "jpeg", "webp", "gif", "bmp", "tif"];

/// What the panel is asking for (UI state, serde so agents can read it).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GenerateDraft {
    pub prompt: String,
    /// Clip length in seconds (0 = the preference's default).
    pub seconds: f64,
    pub image: Option<String>,
    pub image_role: ImageRole,
    pub at_playhead: bool,
    /// The position when not at the playhead, in seconds.
    pub at_seconds: f64,
    /// Video track to place on (1-based, None = the source patch / V1).
    pub track: Option<usize>,
    /// Insert (ripple) instead of overwrite; None = the preference.
    pub insert: Option<bool>,
    /// The running generation job.
    pub job: Option<u64>,
    /// What the last generation ended with.
    pub last: String,
}

impl Default for GenerateDraft {
    fn default() -> Self {
        GenerateDraft {
            prompt: String::new(),
            seconds: 0.0,
            image: None,
            image_role: ImageRole::First,
            at_playhead: true,
            at_seconds: 0.0,
            track: None,
            insert: None,
            job: None,
            last: String::new(),
        }
    }
}

impl GenerateDraft {
    fn params(&self, default_seconds: f64) -> Value {
        let mut p = json!({"prompt": self.prompt.trim(), "seconds": if self.seconds > 0.0 { self.seconds } else { default_seconds }});
        if let Some(img) = &self.image {
            p["image"] = json!(img);
            p["imageRole"] = json!(self.image_role.id());
        }
        if !self.at_playhead {
            p["atSeconds"] = json!(self.at_seconds.max(0.0));
        }
        if let Some(t) = self.track {
            p["track"] = json!(format!("V{t}"));
        }
        if let Some(i) = self.insert {
            p["insert"] = json!(i);
        }
        p
    }
}

/// State of job `id`: `None` while running, else its result.
fn job_outcome(app: &FilmcraftApp, id: u64) -> Option<Result<String, String>> {
    use std::sync::atomic::Ordering;
    let Some(j) = app.session.jobs.iter().find(|j| j.id == id) else { return Some(Err("the job disappeared".into())) };
    if !j.progress.finished.load(Ordering::Relaxed) {
        return None;
    }
    Some(match &*j.result.lock().unwrap_or_else(std::sync::PoisonError::into_inner) {
        Some(Ok(_)) => Ok(j.progress.status.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()),
        Some(Err(e)) => Err(e.clone()),
        None => Err("the job stopped".into()),
    })
}

/// Finish the running generation's bookkeeping. Runs every frame, also when the panel is hidden.
pub fn poll(app: &mut FilmcraftApp) {
    if let Some(j) = app.ui.generate.job
        && let Some(r) = job_outcome(app, j)
    {
        app.ui.generate.job = None;
        app.ui.generate.last = match r {
            Ok(status) => {
                if status.is_empty() {
                    tl!("Clip generated and placed on the timeline").into()
                } else {
                    status
                }
            }
            Err(e) => tlf!("Generation failed: {e}", e),
        };
        app.ui.status = app.ui.generate.last.clone();
    }
}

/// Whether a generation is running.
pub fn busy(app: &FilmcraftApp) -> bool {
    app.ui.generate.job.is_some()
}

/// The UI commands of the panel.
pub fn command(app: &mut FilmcraftApp, _ctx: &egui::Context, id: &str, params: &Value) -> Result<Value, String> {
    match id {
        "generatePanel.run" => run(app, params),
        "generatePanel.cancel" => {
            let Some(job) = app.ui.generate.job else { return Err("no generation is running".into()) };
            app.session.execute("generate.cancel", json!({"job": job})).map_err(|e| e.to_string())
        }
        "generatePanel.chooseImage" => {
            let path = match params.get("path").and_then(Value::as_str) {
                Some(p) => Some(p.to_string()),
                None => app.hooks.pick_files.as_mut().and_then(|pick| pick(&IMAGE_EXTENSIONS).into_iter().next()),
            };
            app.ui.generate.image = path.clone();
            Ok(json!({"image": path}))
        }
        "generatePanel.clearImage" => {
            app.ui.generate.image = None;
            Ok(json!({"image": Value::Null}))
        }
        "generatePanel.settings" => crate::panels::settings::open(app, "generate"),
        _ => Err(format!("unknown command {id}")),
    }
}

fn run(app: &mut FilmcraftApp, extra: &Value) -> Result<Value, String> {
    if app.ui.generate.job.is_some() {
        return Err("a generation is already running".into());
    }
    let default_seconds = app.session.prefs.generate.default_seconds;
    let mut params = app.ui.generate.params(default_seconds);
    if let (Some(dst), Some(src)) = (params.as_object_mut(), extra.as_object()) {
        for (k, v) in src {
            dst.insert(k.clone(), v.clone());
        }
    }
    let r = app.session.execute("generate.video", params).map_err(|e| e.to_string())?;
    app.ui.generate.job = r["job"].as_u64();
    app.ui.generate.last.clear();
    app.ui.status = tl!("Generating the clip…").into();
    Ok(r)
}

fn provider(app: &mut FilmcraftApp) -> Value {
    app.session.execute("generate.providers", json!({})).map(|r| r["providers"][0].clone()).unwrap_or(Value::Null)
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    ui.painter().rect_filled(rect, 0.0, t.panel_bg);
    let provider = provider(app);
    let limits = provider["limits"].clone();
    let (min_s, max_s) = (limits["minSeconds"].as_f64().unwrap_or(5.167), limits["maxSeconds"].as_f64().unwrap_or(14.375));
    let default_seconds = limits
        .get("defaultSeconds")
        .and_then(Value::as_f64)
        .map(|d| app.session.prefs.generate.default_seconds.clamp(min_s, max_s).max(d.min(max_s)).min(max_s))
        .unwrap_or(8.0);
    let max_chars = limits["maxPromptChars"].as_u64().unwrap_or(800) as usize;
    let price = limits["pricePerSecondUsd"].as_f64().unwrap_or(0.0);
    let (w_px, h_px, fps) = (limits["width"].as_u64().unwrap_or(0), limits["height"].as_u64().unwrap_or(0), limits["fps"].as_f64().unwrap_or(0.0));
    let ready = provider["ready"].as_bool().unwrap_or(false);
    let reason = provider["reason"].as_str().unwrap_or("").to_string();
    let name = provider["name"].as_str().unwrap_or("Reactor FastH3").to_string();
    let busy = app.ui.generate.job.is_some();
    let job_status = app
        .ui
        .generate
        .job
        .and_then(|id| app.session.jobs.iter().find(|j| j.id == id))
        .map(|j| j.progress.status.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone());
    let video_tracks = app.session.active_sequence().map(|q| q.video_tracks.len()).unwrap_or(0);
    let has_seq = video_tracks > 0;
    let insert_pref = app.session.prefs.generate.insert;
    let mut actions: Vec<&'static str> = Vec::new();
    let auto = &mut app.auto;
    let d = &mut app.ui.generate;
    if d.seconds <= 0.0 {
        d.seconds = default_seconds;
    }

    let body = Rect::from_min_max(pos2(rect.min.x + PAD, rect.min.y + 8.0), pos2(rect.max.x - PAD, rect.max.y - FOOTER_H));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body).id_salt("generate-body"));
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(&mut child, |ui| {
        ui.set_max_width(ui.available_width() - 6.0);
        let w = ui.available_width();
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(&name).color(t.text).size(12.5).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (text, col) = if ready { (tl!("Ready"), t.accent) } else { (tl!("Not ready"), t.text_dim) };
                ui.label(egui::RichText::new(text).color(col).size(11.0));
            });
        });
        if !ready {
            egui::Frame::new().fill(t.field_bg).stroke(egui::Stroke::new(1.0, t.field_border)).corner_radius(4.0).inner_margin(8.0).show(ui, |ui| {
                ui.set_width(w - 18.0);
                ui.label(egui::RichText::new(crate::i18n::t(&reason)).color(t.text).size(11.5));
                let b = ui.small_button(tl!("Open Settings…"));
                auto.add("generatePanel.settings", visible(ui, b.rect), "Open Settings");
                if b.clicked() {
                    actions.push("generatePanel.settings");
                }
            });
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(tl!("Prompt")).color(t.text).size(12.5).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let n = d.prompt.chars().count();
                let col = if n > max_chars { t.danger } else { t.text_faint };
                ui.label(egui::RichText::new(format!("{n}/{max_chars}")).color(col).size(10.5).monospace());
            });
        });
        ui.add_space(2.0);
        let te = egui::TextEdit::multiline(&mut d.prompt)
            .hint_text(tl!("Describe the whole shot: style, subjects, action, camera, light and sound"))
            .desired_width(w)
            .desired_rows(6)
            .char_limit(max_chars)
            .interactive(!busy);
        let r = ui.add(te);
        auto.add("generatePanel.prompt", visible(ui, r.rect), "Prompt");
        ui.add_space(10.0);
        label(ui, &t, tl!("Length"));
        ui.spacing_mut().slider_width = w - 90.0;
        let s = ui.add_enabled(!busy, egui::Slider::new(&mut d.seconds, min_s..=max_s).fixed_decimals(2).suffix(" s"));
        auto.add("generatePanel.seconds", visible(ui, s.rect), "Length");
        ui.label(egui::RichText::new(tl!("The model snaps the length to the nearest one it can produce.")).color(t.text_faint).size(10.5));
        ui.add_space(10.0);
        label(ui, &t, tl!("Image (optional)"));
        match d.image.clone() {
            Some(path) => {
                let file = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(path.clone());
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(file).color(t.text).size(11.5)).on_hover_text(&path);
                    let b = ui.add_enabled(!busy, egui::Button::new(tl!("Remove")).small());
                    auto.add("generatePanel.clearImage", visible(ui, b.rect), "Remove image");
                    if b.clicked() {
                        actions.push("generatePanel.clearImage");
                    }
                });
                ui.add_space(4.0);
                for role in ImageRole::ALL {
                    let r = ui.add_enabled(!busy, egui::RadioButton::new(d.image_role == role, crate::i18n::t(role.label())));
                    auto.add(&format!("generatePanel.imageRole.{}", role.id()), visible(ui, r.rect), role.label());
                    if r.clicked() {
                        d.image_role = role;
                    }
                }
                ui.label(
                    egui::RichText::new(tl!("The image is fitted to the model's canvas; it is a frame target, not a style reference."))
                        .color(t.text_faint)
                        .size(10.5),
                );
            }
            None => {
                let b = ui.add_enabled(!busy, egui::Button::new(tl!("Choose image…")));
                auto.add("generatePanel.chooseImage", visible(ui, b.rect), "Choose image");
                if b.clicked() {
                    actions.push("generatePanel.chooseImage");
                }
            }
        }
        ui.add_space(10.0);
        label(ui, &t, tl!("Place"));
        ui.horizontal(|ui| {
            let r = ui.add_enabled(!busy, egui::RadioButton::new(d.at_playhead, tl!("At the playhead")));
            auto.add("generatePanel.atPlayhead", visible(ui, r.rect), "At the playhead");
            if r.clicked() {
                d.at_playhead = true;
            }
            let r = ui.add_enabled(!busy, egui::RadioButton::new(!d.at_playhead, tl!("At")));
            auto.add("generatePanel.atCustom", visible(ui, r.rect), "At a time");
            if r.clicked() {
                d.at_playhead = false;
            }
            let dv = ui
                .add_enabled(!busy && !d.at_playhead, egui::DragValue::new(&mut d.at_seconds).range(0.0..=360_000.0).speed(0.1).fixed_decimals(2).suffix(" s"));
            auto.add("generatePanel.atSeconds", visible(ui, dv.rect), "Position in seconds");
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(tl!("Track")).color(t.text_dim).size(11.5));
            let current = d.track.map(|i| format!("V{i}")).unwrap_or_else(|| tl!("Automatic").into());
            let cb = egui::ComboBox::from_id_salt("generate-track").selected_text(current).width(110.0).show_ui(ui, |ui| {
                if ui.selectable_label(d.track.is_none(), tl!("Automatic")).clicked() {
                    d.track = None;
                }
                for i in 1..=video_tracks {
                    if ui.selectable_label(d.track == Some(i), format!("V{i}")).clicked() {
                        d.track = Some(i);
                    }
                }
            });
            auto.add("generatePanel.track", visible(ui, cb.response.rect), "Track");
            let mut insert = d.insert.unwrap_or(insert_pref);
            let c = ui.add_enabled(!busy, egui::Checkbox::new(&mut insert, tl!("Insert (ripple)")));
            auto.add("generatePanel.insert", visible(ui, c.rect), "Insert");
            if c.changed() {
                d.insert = Some(insert);
            }
        });
        ui.add_space(10.0);
        let cost = (d.seconds * 2.0 * price).max(0.0);
        let mut line = tlf!("Estimated cost ≈ ${cost} (the session is billed per second, about twice the clip's length)", cost = format!("{cost:.3}"));
        if w_px > 0 {
            line.push_str(&format!(" · {w_px}×{h_px} · {fps:.0} fps"));
        }
        ui.label(egui::RichText::new(line).color(t.text_faint).size(10.5));
    });

    let fy = rect.max.y - FOOTER_H + 8.0;
    let can_run = ready && has_seq && !busy && !d.prompt.trim().is_empty() && d.prompt.chars().count() <= max_chars;
    let bw = rect.width() - PAD * 2.0;
    let br = Rect::from_min_size(pos2(rect.min.x + PAD, fy), vec2(if busy { bw * 0.62 } else { bw }, 28.0));
    let mut footer =
        ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(pos2(rect.min.x + PAD, fy), vec2(bw, FOOTER_H - 8.0))).id_salt("generate-footer"));
    footer.horizontal(|ui| {
        let label = if busy { tl!("Generating…") } else { tl!("Generate clip") };
        let b = ui
            .add_enabled(can_run, egui::Button::new(egui::RichText::new(label).strong()).min_size(br.size()).fill(if can_run { t.accent } else { t.field_bg }));
        auto.add("generatePanel.run", b.rect, "Generate clip");
        if b.clicked() && can_run {
            actions.push("generatePanel.run");
        }
        if busy {
            let c = ui.add(egui::Button::new(tl!("Cancel")).min_size(vec2(bw * 0.34, 28.0)));
            auto.add("generatePanel.cancel", c.rect, "Cancel generation");
            if c.clicked() {
                actions.push("generatePanel.cancel");
            }
        }
    });
    footer.add_space(4.0);
    let status = match (&job_status, d.last.is_empty()) {
        (Some(s), _) => s.clone(),
        (None, false) => d.last.clone(),
        (None, true) if !has_seq => tl!("Open a sequence to place the clip").into(),
        (None, true) => String::new(),
    };
    if !status.is_empty() {
        let r = footer.add(egui::Label::new(egui::RichText::new(status).color(t.text_dim).size(11.0)).truncate());
        auto.add("generatePanel.status", r.rect, "Generation status");
    }

    let ctx = ui.ctx().clone();
    for a in actions {
        if let Err(e) = command(app, &ctx, a, &json!({})) {
            app.ui.status = e;
        }
    }
    if app.ui.generate.job.is_some() {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
    }
}

/// The part of `r` the scroll area shows (what a click can reach).
fn visible(ui: &egui::Ui, r: Rect) -> Rect {
    r.intersect(ui.clip_rect())
}

fn label(ui: &mut egui::Ui, t: &Tokens, text: &str) {
    ui.label(egui::RichText::new(text).color(t.text).size(12.5).strong());
    ui.add_space(2.0);
}

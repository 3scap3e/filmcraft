//! The RE lane of the Timeline: reverse-engineering components applied to clips (2D cameras for
//! now), drawn in a fixed row above the caption tracks and the video tracks. Each component is a
//! bar aligned with its clip and its time, with a mini graph of the path (Δx, Δy, zoom) and marks
//! where the motion changes; hovering shows its facts, clicking selects the clip and opens the
//! 2D camera tool on it, the context menu removes it or exports it as a camera file. The lane is
//! informative: nothing on it is dragged or trimmed. Automation ids: `timeline.components.*`.
use egui::{Align2, Color32, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use filmcraft_project::{AppliedComponent, ChangeKind, ComponentMode, Sequence};
use filmcraft_time::Tick;
use serde_json::{Value, json};

use super::timeline::Layout;
use crate::FilmcraftApp;
use crate::dock::PanelKind;
use crate::theme::Tokens;

/// Height of the RE lane.
pub const ROW_H: f32 = 44.0;
const FILL: Color32 = Color32::from_rgb(0x2f, 0x6f, 0x6a);
const FILL_FOCUS: Color32 = Color32::from_rgb(0x3f, 0x95, 0x8e);
const FILL_STABILIZE: Color32 = Color32::from_rgb(0x6a, 0x5a, 0x2f);
const GRAPH_X: Color32 = Color32::from_rgb(0xff, 0xd1, 0x66);
const GRAPH_Y: Color32 = Color32::from_rgb(0x8c, 0xd6, 0xff);
const GRAPH_ZOOM: Color32 = Color32::from_rgb(0xff, 0x8c, 0xc8);

fn mark_color(kind: ChangeKind) -> Color32 {
    match kind {
        ChangeKind::MotionStarts => Color32::from_rgb(90, 200, 110),
        ChangeKind::MotionStops => Color32::from_rgb(120, 170, 220),
        ChangeKind::NoEvidence => Color32::from_rgb(200, 70, 70),
        ChangeKind::HighResidual => Color32::from_rgb(230, 160, 50),
    }
}

fn bar_rect(c: &AppliedComponent, lane: Rect, layout: &Layout) -> Rect {
    let x0 = layout.x_of(c.start);
    let x1 = layout.x_of(c.end()).max(x0 + 2.0);
    Rect::from_min_max(pos2(x0, lane.min.y + 3.0), pos2(x1, lane.max.y - 3.0))
}

/// The sample in force at `tick` (offset from the component start).
fn sample_at(c: &AppliedComponent, tick: i64) -> Option<&filmcraft_project::ComponentSample> {
    let i = c.samples.partition_point(|s| s.tick <= tick);
    c.samples.get(i.checked_sub(1)?)
}

/// Paint the header, the bars, their graphs and marks.
pub fn paint(app: &mut FilmcraftApp, ui: &mut egui::Ui, seq: &Sequence, area: Rect, layout: &Layout, t: &Tokens) {
    if seq.components.is_empty() {
        return;
    }
    let hw = app.ui.timeline.header_w;
    let content = layout.content;
    let painter = ui.painter().with_clip_rect(area);
    let lane = Rect::from_min_max(pos2(content.min.x, area.min.y), pos2(content.max.x, area.max.y));
    painter.rect_filled(lane, 0.0, t.caption_track_bg);
    painter.line_segment([pos2(area.min.x, area.max.y - 0.5), pos2(area.max.x, area.max.y - 0.5)], Stroke::new(1.0, t.tl_bg));
    let hrect = Rect::from_min_max(area.min, pos2(area.min.x + hw, area.max.y));
    painter.rect_filled(hrect, 0.0, t.tl_header_bg);
    painter.line_segment([pos2(hrect.min.x, hrect.max.y - 0.5), pos2(hrect.max.x, hrect.max.y - 0.5)], Stroke::new(1.0, t.separator));
    let badge = Rect::from_min_size(pos2(hrect.min.x + 13.0, area.min.y + 11.0), vec2(28.0, ROW_H - 22.0));
    painter.rect_filled(badge, 3.0, FILL);
    painter.text(badge.center(), Align2::CENTER_CENTER, "RE", Tokens::semibold(10.5), Color32::WHITE);
    let count = seq.components.len();
    let title = format!("{} · {} component{}", tl!("Components"), count, if count == 1 { "" } else { "s" });
    painter.with_clip_rect(Rect::from_min_max(pos2(hrect.min.x + 46.0, area.min.y), pos2(hrect.max.x - 4.0, area.max.y))).text(
        pos2(hrect.min.x + 50.0, area.center().y),
        Align2::LEFT_CENTER,
        &title,
        Tokens::ui(11.0),
        t.text_dim,
    );
    app.auto.add("timeline.components.header", badge, &title);
    let focused = app.ui.panels.reverse_engineering.focused_component;
    let p = painter.with_clip_rect(lane);
    let (v0, v1) = (layout.tick_at(content.min.x - 2.0), layout.tick_at(content.max.x + 2.0));
    for c in seq.components.iter().filter(|c| c.end() >= v0 && c.start <= v1) {
        let r = bar_rect(c, lane, layout);
        let is_focused = focused == Some(c.id.0);
        let fill = match (c.mode, is_focused) {
            (_, true) => FILL_FOCUS,
            (ComponentMode::Follow, false) => FILL,
            (ComponentMode::Stabilize, false) => FILL_STABILIZE,
        };
        p.rect_filled(r, 3.0, fill);
        if is_focused {
            p.rect_stroke(r, 3.0, Stroke::new(1.0, t.clip_selected_border), StrokeKind::Inside);
        }
        let visible = r.intersect(lane);
        if visible.width() > 24.0 && !c.samples.is_empty() {
            draw_graph(&p, c, r, visible);
        }
        for (time, kind) in c.changes() {
            let x = layout.x_of(time);
            if x >= visible.min.x && x <= visible.max.x {
                p.line_segment([pos2(x, r.min.y), pos2(x, r.max.y)], Stroke::new(1.5, mark_color(kind)));
            }
        }
        if visible.width() > 30.0 {
            let label = format!("{} · {}", c.name, c.mode.label());
            p.with_clip_rect(r.shrink2(vec2(4.0, 0.0)).intersect(lane)).text(
                pos2(visible.min.x + 5.0, r.min.y + 8.0),
                Align2::LEFT_CENTER,
                label,
                Tokens::ui(10.5),
                Color32::WHITE,
            );
        }
        app.auto.add(&format!("timeline.components.{}", c.id.0), visible, &c.name);
    }
    painter.line_segment([pos2(area.min.x + hw - 0.5, area.min.y), pos2(area.min.x + hw - 0.5, area.max.y)], Stroke::new(1.0, t.separator));
}

/// Δx, Δy and zoom of the path as three polylines, each normalised to the bar's lower half.
fn draw_graph(p: &egui::Painter, c: &AppliedComponent, bar: Rect, visible: Rect) {
    let (top, bottom) = (bar.min.y + 15.0, bar.max.y - 2.0);
    let mid = (top + bottom) / 2.0;
    let half = (bottom - top) / 2.0;
    let max_d = c.max_displacement().max(1.0);
    let (lo, hi) = c.scale_range();
    let max_z = (hi - 1.0).abs().max((1.0 - lo).abs()).max(0.001);
    p.line_segment([pos2(visible.min.x, mid), pos2(visible.max.x, mid)], Stroke::new(1.0, Color32::from_white_alpha(28)));
    let duration = c.duration.0.max(1) as f64;
    let columns = (visible.width() as usize).clamp(2, 4096);
    let mut xs: Vec<[egui::Pos2; 3]> = Vec::with_capacity(columns);
    for i in 0..columns {
        let x = visible.min.x + i as f32 / (columns - 1) as f32 * visible.width();
        let f = ((x - bar.min.x) / bar.width().max(1.0)).clamp(0.0, 1.0) as f64;
        let tick = (f * duration) as i64;
        let Some(s) = sample_at(c, tick) else { continue };
        let yx = mid - (s.dx / max_d) as f32 * half;
        let yy = mid - (s.dy / max_d) as f32 * half;
        let yz = mid - ((s.scale - 1.0) / max_z) as f32 * half;
        xs.push([pos2(x, yx.clamp(top, bottom)), pos2(x, yy.clamp(top, bottom)), pos2(x, yz.clamp(top, bottom))]);
    }
    if xs.len() < 2 {
        return;
    }
    for (k, color) in [(0usize, GRAPH_X), (1, GRAPH_Y), (2, GRAPH_ZOOM)] {
        let points: Vec<egui::Pos2> = xs.iter().map(|row| row[k]).collect();
        p.add(egui::Shape::line(points, Stroke::new(1.0, color)));
    }
}

fn facts(c: &AppliedComponent, seq: &Sequence) -> Vec<String> {
    let rate = seq.settings.frame_rate;
    let frames = rate.frame_at(c.end() - Tick(1)).saturating_sub(rate.frame_at(c.start)).saturating_add(1);
    let (lo, hi) = c.scale_range();
    let mut out = vec![
        format!("{} · {} · {}", c.name, c.kind.label(), c.mode.label()),
        format!("Clip {} · {:.2} s · {} frames", c.clip.0, c.duration.seconds(), frames),
        format!(
            "Source: {} {}×{} @ {:.3} fps, frames {}–{}",
            c.source.sequence, c.source.width, c.source.height, c.source.frame_rate, c.source.start_frame, c.source.end_frame
        ),
        format!("Path: {} samples · max displacement {:.1} px · zoom {:.3}–{:.3}", c.samples.len(), c.max_displacement(), lo, hi),
    ];
    if let Some(file) = &c.source.file {
        out.push(format!("File: {file}"));
    }
    let changes = c.changes();
    let high = changes.iter().filter(|(_, k)| *k == ChangeKind::HighResidual).count();
    if high > 0 {
        out.push(format!("{high} frame{} with a high residual: evidence the path overruled", if high == 1 { "" } else { "s" }));
    }
    for (time, kind) in changes.iter().take(8) {
        out.push(format!("  {:.2} s · {}", (*time - c.start).seconds(), kind.label()));
    }
    if changes.len() > 8 {
        out.push(format!("  … {} more changes", changes.len() - 8));
    }
    out
}

/// Hover, click and context menu on the bars (after the timeline's own interaction).
pub fn interact(app: &mut FilmcraftApp, ui: &mut egui::Ui, seq: &Sequence, area: Rect, layout: &Layout, _t: &Tokens) {
    let lane = Rect::from_min_max(pos2(layout.content.min.x, area.min.y), pos2(layout.content.max.x, area.max.y));
    let mut actions: Vec<(String, Value)> = Vec::new();
    let mut focus: Option<u64> = None;
    for c in &seq.components {
        let r = bar_rect(c, lane, layout).intersect(lane);
        if r.width() <= 0.0 {
            continue;
        }
        let resp = ui.interact(r, egui::Id::new(("re-component", c.id.0)), Sense::click());
        if resp.hovered() {
            egui::Tooltip::always_open(ui.ctx().clone(), ui.layer_id(), egui::Id::new(("re-component-tip", c.id.0)), egui::PopupAnchor::Pointer).show(|ui| {
                for line in facts(c, seq) {
                    ui.label(line);
                }
            });
        }
        if resp.clicked() {
            focus = Some(c.id.0);
            actions.push(("timeline.select".into(), json!({"clips": [c.clip.0]})));
            actions.push(("playhead.set".into(), json!({"time": c.start.0})));
            app.show_panel(PanelKind::ReverseEngineering);
            actions.push(("reverseEngineering.chooseTool".into(), json!({"tool": "camera2d"})));
        }
        resp.context_menu(|ui| {
            let r = ui.button(tl!("Remove Component"));
            app.auto.add(&format!("timeline.components.{}.menu.remove", c.id.0), r.rect, "Remove Component");
            if r.clicked() {
                actions.push(("components.remove".into(), json!({"id": c.id.0})));
                ui.close();
            }
            let r = ui.button(tl!("Export Camera File…"));
            app.auto.add(&format!("timeline.components.{}.menu.export", c.id.0), r.rect, "Export Camera File");
            if r.clicked() {
                actions.push(("reverseEngineering.exportComponent".into(), json!({"id": c.id.0})));
                ui.close();
            }
            let r = ui.button(tl!("Select Clip"));
            app.auto.add(&format!("timeline.components.{}.menu.select", c.id.0), r.rect, "Select Clip");
            if r.clicked() {
                actions.push(("timeline.select".into(), json!({"clips": [c.clip.0]})));
                ui.close();
            }
        });
    }
    if let Some(id) = focus {
        app.ui.panels.reverse_engineering.focused_component = Some(id);
    }
    let ctx = ui.ctx().clone();
    for (cmd, p) in actions {
        if let Err(e) = crate::menus::invoke(app, &ctx, &cmd, p) {
            app.ui.status = e;
        }
    }
}

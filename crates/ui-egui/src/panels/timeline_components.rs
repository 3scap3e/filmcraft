//! The RE lane of the Timeline: reverse-engineering components applied to clips (2D cameras,
//! looks), drawn as a track row above the caption tracks and the video tracks, at the video
//! tracks' height and with their header (badge, lock, eye). Each component is a clip-styled bar
//! aligned with its clip and its time, with a mini graph of the path (Δx, Δy, zoom) and marks
//! where the motion changes; hovering shows its facts, clicking selects the clip and opens its
//! tool, the context menu bypasses, removes or exports it. A component that follows its clip (a
//! look on an adjustment layer) is dragged and trimmed on the lane exactly like a clip: the same
//! snapping, the same ghost on its track, the same `timeline.move` / `timeline.trim` on release.
//! The eye bypasses every component, the lock stops lane edits. Automation ids:
//! `timeline.components.*`.
use egui::{Align2, Color32, CursorIcon, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use filmcraft_project::{AppliedComponent, ChangeKind, ComponentMode, Sequence};
use filmcraft_time::Tick;
use serde_json::{Value, json};

use super::timeline::{Drag, Layout};
use crate::FilmcraftApp;
use crate::dock::PanelKind;
use crate::icons::Icon;
use crate::theme::Tokens;
const FILL: Color32 = Color32::from_rgb(0x2f, 0x6f, 0x6a);
const FILL_STABILIZE: Color32 = Color32::from_rgb(0x6a, 0x5a, 0x2f);
const FILL_LOOK: Color32 = Color32::from_rgb(0x5a, 0x3f, 0x6e);
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

fn upper_y(hrect: Rect) -> f32 {
    hrect.min.y + (16.0f32).min(hrect.height() / 2.0)
}

fn small(hrect: Rect, x: f32, y: f32) -> Rect {
    Rect::from_center_size(pos2(hrect.min.x + x, y), vec2(18.0, 18.0))
}

fn lock_rect(hrect: Rect) -> Rect {
    small(hrect, 49.0, hrect.center().y)
}

fn eye_rect(hrect: Rect) -> Rect {
    small(hrect, 126.0, upper_y(hrect))
}

fn badge_rect(hrect: Rect) -> Rect {
    Rect::from_min_max(pos2(hrect.min.x + 61.0, hrect.min.y + 1.0), pos2(hrect.min.x + 85.0, hrect.max.y - 2.0))
}

/// The component's span as shown now: its clip's, moved or trimmed along with a drag in flight.
fn live_span(app: &FilmcraftApp, c: &AppliedComponent, seq: &Sequence) -> (Tick, Tick) {
    let (start, duration) = c.span(seq);
    if !c.follows_clip {
        return (start, duration);
    }
    match &app.tl.drag {
        Some(Drag::Move { clips, offset, .. }) if clips.contains(&c.clip) => ((start + *offset).max(Tick::ZERO), duration),
        Some(Drag::Trim { clip, edge, delta, .. }) if *clip == c.clip => {
            if let Some((_, it)) = seq.find_item(c.clip) {
                let mut e = it.clone();
                filmcraft_edit::move_edge(&mut e, *edge, *delta, true);
                return (e.start, e.duration);
            }
            (start, duration)
        }
        _ => (start, duration),
    }
}

fn bar_rect(app: &FilmcraftApp, c: &AppliedComponent, seq: &Sequence, lane: Rect, layout: &Layout) -> Rect {
    let (start, duration) = live_span(app, c, seq);
    let x0 = layout.x_of(start);
    let x1 = layout.x_of(start + duration).max(x0 + 2.0);
    Rect::from_min_max(pos2(x0, lane.min.y + 3.0), pos2(x1, lane.max.y - 3.0))
}

const EDGE_GRAB_PX: f32 = 7.0;

fn edge_at(bar: Rect, x: f32) -> Option<filmcraft_edit::Edge> {
    let grab = EDGE_GRAB_PX.min(bar.width() / 3.0);
    if x <= bar.min.x + grab {
        Some(filmcraft_edit::Edge::In)
    } else if x >= bar.max.x - grab {
        Some(filmcraft_edit::Edge::Out)
    } else {
        None
    }
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
    let count = seq.components.len();
    let title = format!("{} · {} component{}", tl!("Components"), count, if count == 1 { "" } else { "s" });
    let badge = badge_rect(hrect);
    super::timeline::patch_button(ui, badge, hrect, "RE", true, true, egui::Id::new("re-components-badge"), t);
    app.auto.add("timeline.components.header", badge, &title);
    if hrect.height() >= 40.0 {
        painter.text(pos2(hrect.min.x + 94.0, hrect.min.y + 34.0), Align2::LEFT_CENTER, &title, Tokens::ui(11.0), t.text_dim);
    }
    if app.ui.timeline.components_locked {
        painter.rect_filled(Rect::from_min_max(pos2(hrect.max.x - 4.0, hrect.min.y), hrect.max), 0.0, t.pressed);
    }
    let focused = app.ui.panels.reverse_engineering.focused_component;
    let p = painter.with_clip_rect(lane);
    let (v0, v1) = (layout.tick_at(content.min.x - 2.0), layout.tick_at(content.max.x + 2.0));
    let components: Vec<AppliedComponent> = seq
        .components
        .iter()
        .filter(|c| {
            let (start, duration) = live_span(app, c, seq);
            start + duration >= v0 && start <= v1
        })
        .cloned()
        .collect();
    for c in &components {
        let r = bar_rect(app, c, seq, lane, layout);
        let selected = focused == Some(c.id.0) || app.session.state.selection.contains(&c.clip);
        let base = match (c.kind, c.mode) {
            (filmcraft_project::ComponentKind::Look, _) => FILL_LOOK,
            (_, ComponentMode::Follow) => FILL,
            (_, ComponentMode::Stabilize) => FILL_STABILIZE,
        };
        let fill = if !c.enabled {
            t.disabled_clip_bg
        } else if selected {
            super::timeline::lighten(base, 0.06)
        } else {
            base
        };
        p.rect_filled(r, 0.0, fill);
        p.line_segment([pos2(r.min.x, r.min.y + 0.5), pos2(r.max.x, r.min.y + 0.5)], Stroke::new(1.0, super::timeline::lighten(fill, 0.15)));
        let visible = r.intersect(lane);
        if visible.width() > 24.0 && !c.samples.is_empty() && c.enabled {
            draw_graph(&p, c, r, visible);
        }
        for (time, kind) in c.changes() {
            let x = layout.x_of(time);
            if x >= visible.min.x && x <= visible.max.x {
                p.line_segment([pos2(x, r.min.y), pos2(x, r.max.y)], Stroke::new(1.5, mark_color(kind)));
            }
        }
        let clip_p = p.with_clip_rect(r.shrink2(vec2(1.0, 0.0)).intersect(lane));
        if visible.width() > 24.0 {
            let label = if c.enabled { c.name.clone() } else { format!("{} · bypassed", c.name) };
            clip_p.text(
                pos2(r.min.x + 6.0, r.min.y + 8.5),
                Align2::LEFT_CENTER,
                label,
                Tokens::ui(11.5),
                if c.enabled { Color32::from_rgb(0xd9, 0xd9, 0xd9) } else { t.text_faint },
            );
        }
        if visible.width() > 40.0 {
            clip_p.text(
                pos2(r.max.x - 6.0, r.min.y + 8.5),
                Align2::RIGHT_CENTER,
                c.kind.label(),
                egui::FontId::new(11.0, egui::FontFamily::Name("semibold".into())),
                Color32::from_rgb(0xe8, 0xc5, 0x4a),
            );
        }
        p.line_segment([pos2(r.min.x + 0.5, r.min.y), pos2(r.min.x + 0.5, r.max.y)], Stroke::new(1.0, Color32::BLACK));
        if selected {
            p.rect_stroke(r.shrink(1.0), 0.0, Stroke::new(1.0, Color32::BLACK), StrokeKind::Inside);
            p.rect_stroke(r, 0.0, Stroke::new(2.0, t.clip_selected_border), StrokeKind::Inside);
        }
        app.auto.add(&format!("timeline.components.{}", c.id.0), visible, &c.name);
        if c.follows_clip && r.width() > 2.0 * EDGE_GRAB_PX {
            let grab = EDGE_GRAB_PX.min(r.width() / 3.0);
            if r.min.x >= lane.min.x {
                app.auto.add(&format!("timeline.components.{}.in", c.id.0), Rect::from_min_max(r.min, pos2(r.min.x + grab, r.max.y)), "Trim the start");
            }
            if r.max.x <= lane.max.x {
                app.auto.add(&format!("timeline.components.{}.out", c.id.0), Rect::from_min_max(pos2(r.max.x - grab, r.min.y), r.max), "Trim the end");
            }
        }
    }
    // the frame selected in the 2D camera panel, when that tool is open on this sequence
    let state = &app.ui.panels.reverse_engineering;
    if state.active_tool == "camera2d"
        && let Some(capture) = state.camera2d_captures.get(state.camera2d_frame)
    {
        let x = layout.x_of(capture.timeline_time);
        if x >= lane.min.x && x <= lane.max.x {
            p.line_segment([pos2(x, lane.min.y), pos2(x, lane.max.y)], Stroke::new(1.0, Color32::from_white_alpha(160)));
        }
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
    let (start, duration) = c.span(seq);
    let frames = rate.frame_at(start + duration - Tick(1)).saturating_sub(rate.frame_at(start)).saturating_add(1);
    let (lo, hi) = c.scale_range();
    let mut out = vec![
        format!("{} · {} · {}", c.name, c.kind.label(), c.mode.label()),
        format!("Clip {} · {:.2} s · {} frames{}", c.clip.0, duration.seconds(), frames, if c.follows_clip { " · moves with its clip" } else { "" }),
        format!(
            "Source: {} {}×{} @ {:.3} fps, frames {}–{}",
            c.source.sequence, c.source.width, c.source.height, c.source.frame_rate, c.source.start_frame, c.source.end_frame
        ),
        format!("Path: {} samples · max displacement {:.1} px · zoom {:.3}–{:.3}", c.samples.len(), c.max_displacement(), lo, hi),
    ];
    if let Some(file) = &c.source.file {
        out.push(format!("File: {file}"));
    }
    if !c.enabled {
        out.push("Bypassed: the effect is switched off; enable the component to see it again".into());
    }
    if !c.summary.is_empty() {
        out.push(c.summary.clone());
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

/// Header toggles, hover, click, drag, trim and context menu on the bars (after the timeline's
/// own interaction).
pub fn interact(app: &mut FilmcraftApp, ui: &mut egui::Ui, seq: &Sequence, area: Rect, layout: &Layout, t: &Tokens) {
    let lane = Rect::from_min_max(pos2(layout.content.min.x, area.min.y), pos2(layout.content.max.x, area.max.y));
    let hrect = Rect::from_min_max(area.min, pos2(area.min.x + app.ui.timeline.header_w, area.max.y));
    let mut actions: Vec<(String, Value)> = Vec::new();
    let mut focus: Option<u64> = None;
    let mut jump_to: Option<Tick> = None;
    let all_on = seq.components.iter().all(|c| c.enabled);
    let locked = app.ui.timeline.components_locked;
    let lock_r = lock_rect(hrect);
    let lresp = crate::widgets::icon_toggle(
        ui,
        lock_r.intersect(hrect),
        if locked { Icon::Lock } else { Icon::Unlock },
        true,
        t,
        egui::Id::new("re-components-locked"),
        Some(if locked { t.icon_active } else { t.text_dim }),
    );
    app.auto.add("timeline.components.locked", lock_r, "Toggle RE lane lock");
    if lresp.on_hover_text("Lock the RE lane: its components cannot be dragged or trimmed here").clicked() {
        app.ui.timeline.components_locked = !locked;
    }
    let eye_r = eye_rect(hrect);
    let eresp = crate::widgets::icon_toggle(
        ui,
        eye_r.intersect(hrect),
        if all_on { Icon::Eye } else { Icon::EyeOff },
        true,
        t,
        egui::Id::new("re-components-bypass-all"),
        Some(t.text_dim),
    );
    app.auto.add("timeline.components.bypassAll", eye_r, if all_on { "Bypass all components" } else { "Enable all components" });
    let eresp = eresp.on_hover_text(if all_on {
        "Bypass every component: their effects switch off so the picture shows without them"
    } else {
        "Enable every component again"
    });
    if eresp.clicked() {
        actions.push(("components.setEnabled".into(), json!({"enabled": !all_on})));
    }
    let rate = seq.settings.frame_rate;
    let mut cursor: Option<CursorIcon> = None;
    for c in &seq.components {
        let full = bar_rect(app, c, seq, lane, layout);
        let r = full.intersect(lane);
        if r.width() <= 0.0 {
            continue;
        }
        let draggable = c.follows_clip && !locked && seq.find_item(c.clip).is_some();
        let resp = ui.interact(r, egui::Id::new(("re-component", c.id.0)), if draggable { Sense::click_and_drag() } else { Sense::click() });
        let pointer = resp.interact_pointer_pos().or(resp.hover_pos());
        if draggable && (resp.hovered() || resp.dragged()) {
            cursor = Some(match (pointer.and_then(|p| edge_at(full, p.x)), resp.dragged()) {
                (Some(_), _) => CursorIcon::ResizeHorizontal,
                (None, true) => CursorIcon::Grabbing,
                (None, false) => CursorIcon::Grab,
            });
        }
        if resp.hovered() && app.tl.drag.is_none() {
            egui::Tooltip::always_open(ui.ctx().clone(), ui.layer_id(), egui::Id::new(("re-component-tip", c.id.0)), egui::PopupAnchor::Pointer).show(|ui| {
                for line in facts(c, seq) {
                    ui.label(line);
                }
            });
        }
        if draggable
            && resp.drag_started()
            && let Some(p) = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos())
            && let Some((tid, _)) = seq.find_item(c.clip)
        {
            let t0 = layout.tick_at(p.x);
            app.tl.drag = Some(match edge_at(full, p.x) {
                Some(edge) => Drag::Trim { clip: c.clip, edge, mode: filmcraft_edit::TrimMode::Regular, delta: Tick::ZERO, from: t0 },
                None => Drag::Move { clips: vec![c.clip], grab_tick: t0, start_track: tid, offset: Tick::ZERO, track_delta: 0, copy: false },
            });
            focus = Some(c.id.0);
            if !app.session.state.selection.contains(&c.clip) {
                actions.push(("timeline.select".into(), json!({"clips": [c.clip.0]})));
            }
        }
        if draggable
            && (resp.dragged() || resp.drag_stopped())
            && let Some(p) = resp.interact_pointer_pos()
        {
            let t_here = layout.tick_at(p.x);
            let updated = match app.tl.drag.take() {
                Some(Drag::Move { clips, grab_tick, start_track, copy, .. }) if clips.contains(&c.clip) => {
                    let raw = rate.snap_nearest(t_here - grab_tick);
                    let (first, last) = seq.find_item(c.clip).map(|(_, i)| (i.start, i.end())).unwrap_or_default();
                    let s1 = super::timeline::snap(app, seq, layout, first + raw, &clips) - first;
                    let offset = if s1 != raw { s1 } else { super::timeline::snap(app, seq, layout, last + raw, &clips) - last };
                    Some(Drag::Move { clips, grab_tick, start_track, offset: offset.max(-first), track_delta: 0, copy })
                }
                Some(Drag::Trim { clip, edge, mode, from, .. }) if clip == c.clip => seq.find_item(clip).map(|(_, it)| {
                    let base = if edge == filmcraft_edit::Edge::In { it.start } else { it.end() };
                    let target = super::timeline::snap(app, seq, layout, rate.snap_nearest(base + (t_here - from)), &[clip]);
                    let delta = filmcraft_engine::commands::trim_delta(&app.session, clip, edge, mode, target - base).unwrap_or(target - base);
                    Drag::Trim { clip, edge, mode, delta, from }
                }),
                other => other,
            };
            app.tl.drag = updated;
        }
        if draggable && resp.drag_stopped() {
            match app.tl.drag.take() {
                Some(Drag::Move { clips, offset, .. }) if clips.contains(&c.clip) => {
                    if offset != Tick::ZERO
                        && let Some((tid, it)) = seq.find_item(c.clip)
                    {
                        actions.push((
                            "timeline.move".into(),
                            json!({"moves": [{"clip": c.clip.0, "track": tid.0, "time": (it.start + offset).max(Tick::ZERO).0}], "insert": false, "linked": false, "copy": false}),
                        ));
                    }
                }
                Some(Drag::Trim { clip, edge, mode, delta, .. }) if clip == c.clip => {
                    if delta != Tick::ZERO {
                        actions.push((
                            "timeline.trim".into(),
                            json!({"clip": clip.0, "edge": if edge == filmcraft_edit::Edge::In {"in"} else {"out"}, "mode": if mode == filmcraft_edit::TrimMode::Ripple {"ripple"} else {"regular"}, "delta": delta.0}),
                        ));
                    }
                }
                other => app.tl.drag = other,
            }
        }
        if resp.clicked()
            && app.ui.tool == crate::state::Tool::Razor
            && c.follows_clip
            && let Some(p) = resp.interact_pointer_pos()
        {
            let t = rate.snap_nearest(layout.tick_at(p.x));
            actions.push(("timeline.razor".into(), json!({"time": t.0, "clip": c.clip.0})));
            continue;
        }
        if resp.clicked() {
            focus = Some(c.id.0);
            let (start, duration) = c.span(seq);
            let at = resp.interact_pointer_pos().map(|p| layout.tick_at(p.x).clamp(start, start + duration)).unwrap_or(start);
            jump_to = Some(at);
            actions.push(("timeline.select".into(), json!({"clips": [c.clip.0]})));
            actions.push(("playhead.set".into(), json!({"time": at.0})));
            app.show_panel(PanelKind::ReverseEngineering);
            let tool = match c.kind {
                filmcraft_project::ComponentKind::Look => "look",
                filmcraft_project::ComponentKind::Camera2d => "camera2d",
            };
            actions.push(("reverseEngineering.chooseTool".into(), json!({"tool": tool})));
        }
        resp.context_menu(|ui| {
            let label = if c.enabled { tl!("Bypass Component") } else { tl!("Enable Component") };
            let r = ui.button(label);
            app.auto.add(&format!("timeline.components.{}.menu.bypass", c.id.0), r.rect, label);
            if r.clicked() {
                actions.push(("components.setEnabled".into(), json!({"id": c.id.0, "enabled": !c.enabled})));
                ui.close();
            }
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
            let r = ui.button(tl!("Copy Camera Description"));
            app.auto.add(&format!("timeline.components.{}.menu.copyDescription", c.id.0), r.rect, "Copy Camera Description");
            if r.clicked() {
                actions.push(("reverseEngineering.copyDescription".into(), json!({"id": c.id.0})));
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
    if let Some(cursor) = cursor {
        ui.ctx().set_cursor_icon(cursor);
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
    // the clicked time becomes the panel's selected frame when the analysed range covers it
    if let Some(at) = jump_to {
        let index = app
            .ui
            .panels
            .reverse_engineering
            .camera2d_captures
            .iter()
            .enumerate()
            .filter(|(_, c)| c.timeline_time <= at)
            .max_by_key(|(_, c)| c.timeline_time.0)
            .map(|(i, _)| i);
        if let Some(index) = index
            && let Err(e) = crate::menus::invoke(app, &ctx, "reverseEngineering.featureFrame", json!({"frameIndex": index}))
        {
            app.ui.status = e;
        }
    }
}

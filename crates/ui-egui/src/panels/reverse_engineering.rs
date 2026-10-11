//! Source-frame and range capture. Presentation state stays outside the editable project.
//! All capture/mark/preview actions route through `reverseEngineering.*` UI commands.

use egui::{Rect, vec2};
use filmcraft_project::{TrackItem, TrackKind};
use filmcraft_time::{Tick, TimeDisplay, format_time, parse_timecode};
use serde_json::{Value, json};

use super::panel_state::ReverseEngineeringCapture;
use crate::FilmcraftApp;

const MAX_SAMPLES: usize = 64;

fn selected_clip(app: &FilmcraftApp) -> Result<TrackItem, String> {
    let sequence = app.session.active_sequence().ok_or("Open a sequence and select a video clip in the timeline.")?;
    let clip = app
        .session
        .state
        .selection
        .iter()
        .find_map(|id| {
            let (track, clip) = sequence.find_item(*id)?;
            (sequence.track(track)?.kind == TrackKind::Video).then_some(clip)
        })
        .ok_or("Select a video clip in the timeline.")?;
    app.session.project.source_size(clip.item).ok_or("The selected clip has no video frames.")?;
    Ok(clip.clone())
}

fn capture_at(clip: &TrackItem, time: Tick) -> Result<ReverseEngineeringCapture, String> {
    if time < clip.start || time >= clip.end() {
        return Err("Move the playhead inside the selected video clip.".into());
    }
    Ok(ReverseEngineeringCapture { item: clip.item, media_time: clip.source_time_at(time), timeline_time: time, name: clip.name.clone() })
}

fn prepare_range(app: &mut FilmcraftApp, clip: &TrackItem) -> i64 {
    let rate = app.session.active_sequence().map(|sequence| sequence.settings.frame_rate).unwrap_or_default();
    let last = rate.frame_at(clip.end() - Tick(1)).saturating_sub(rate.frame_at(clip.start)).max(0);
    let state = &mut app.ui.panels.reverse_engineering;
    if state.range_clip != Some(clip.id) {
        state.range_clip = Some(clip.id);
        state.start_frame = 0;
        state.end_frame = last;
    }
    last
}

pub(super) fn reset_decode_timer(ctx: &egui::Context) {
    let now = ctx.input(|input| input.time);
    ctx.data_mut(|data| data.insert_temp(egui::Id::new("reverse-engineering-requested-at"), now));
    ctx.request_repaint();
}

pub fn command(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str, params: &Value) -> Result<Value, String> {
    if matches!(
        id,
        "reverseEngineering.cancelMotion"
            | "reverseEngineering.exportMotion"
            | "reverseEngineering.motionPair"
            | "reverseEngineering.setMotionDisplay"
            | "reverseEngineering.inspectMotionVector"
            | "reverseEngineering.motionFrame"
            | "reverseEngineering.reanalyzeFrame"
    ) {
        return super::reverse_motion::command(app, ctx, id, params);
    }
    if id == "reverseEngineering.selectSample" {
        let index = params.get("index").and_then(Value::as_u64).and_then(|index| usize::try_from(index).ok()).ok_or("Supply a valid sample index.")?;
        if index >= MAX_SAMPLES {
            return Err("Sample index is outside the captured range.".into());
        }
        let capture = app.ui.panels.reverse_engineering.samples.get(index).cloned().ok_or("Sample index is outside the captured range.")?;
        app.ui.panels.reverse_engineering.capture = Some(capture);
        reset_decode_timer(ctx);
        return Ok(json!({"index":index}));
    }
    if super::reverse_motion::is_running(ctx) {
        return Err("Analysis is running. Cancel it before changing the range.".into());
    }
    let clip = selected_clip(app)?;
    if id == "reverseEngineering.extractFrame" {
        let capture = capture_at(&clip, app.session.playhead())?;
        let result = json!({"item":capture.item.0,"mediaTime":capture.media_time.0,"timelineTime":capture.timeline_time.0});
        app.ui.panels.reverse_engineering.capture = Some(capture);
        reset_decode_timer(ctx);
        return Ok(result);
    }
    let last = prepare_range(app, &clip);
    let rate = app.session.active_sequence().map(|sequence| sequence.settings.frame_rate).unwrap_or_default();
    if id == "reverseEngineering.analyzeMotion" {
        let state = &app.ui.panels.reverse_engineering;
        let (start, end) = (state.start_frame, state.end_frame);
        if start < 0 || end <= start || end > last || end.saturating_sub(start) >= 300 {
            return Err("Choose a range of 2–300 frames for motion analysis.".into());
        }
        let base = rate.frame_at(clip.start);
        let captures = (start..=end).map(|frame| capture_at(&clip, rate.tick_of(base.saturating_add(frame)))).collect::<Result<Vec<_>, _>>()?;
        return super::reverse_motion::start(app, ctx, captures);
    }
    if id == "reverseEngineering.playRange" {
        let state = &app.ui.panels.reverse_engineering;
        if state.start_frame < 0 || state.end_frame < state.start_frame || state.end_frame > last {
            return Err("Select a valid range before replaying.".into());
        }
        let base = rate.frame_at(clip.start);
        let start = rate.tick_of(base.saturating_add(state.start_frame));
        let end = rate.tick_of(base.saturating_add(state.end_frame).saturating_add(1)).min(clip.end());
        app.session.set_playhead(start);
        app.playback.looping = false;
        app.play(1.0);
        app.playback.stop_at = Some(end);
        return Ok(json!({"start":start.0,"end":end.0}));
    }
    if id == "reverseEngineering.setRange" {
        let state = &app.ui.panels.reverse_engineering;
        let base = rate.frame_at(clip.start);
        let parse = |key: &str, time_key: &str, current: i64| -> Result<i64, String> {
            if let Some(value) = params.get(time_key) {
                let text = value.as_str().ok_or("Timestamp must be text in HH:MM:SS:FF format.")?;
                let drop_frame = app.session.active_sequence().is_some_and(|sequence| sequence.settings.drop_frame);
                return parse_timecode(text, rate, drop_frame, base.saturating_add(current))
                    .map(|frame| frame.saturating_sub(base))
                    .map_err(|error| error.to_string());
            }
            match params.get(key) {
                Some(value) => value.as_i64().ok_or_else(|| "Frame boundary must be an integer.".into()),
                None => Ok(current),
            }
        };
        let start = parse("startFrame", "startTimecode", state.start_frame)?;
        let end = parse("endFrame", "endTimecode", state.end_frame)?;
        if start < 0 || end < start || end > last {
            return Err("Choose timestamps inside the selected clip, with end at or after start.".into());
        }
        let state = &mut app.ui.panels.reverse_engineering;
        state.start_frame = start;
        state.end_frame = end;
        // Exact timestamp entry also previews the edited boundary in the Program monitor.
        let preview = if params.get("startTimecode").is_some() {
            Some(start)
        } else if params.get("endTimecode").is_some() {
            Some(end)
        } else {
            None
        };
        if let Some(frame) = preview {
            app.stop();
            app.session.set_playhead(rate.tick_of(base.saturating_add(frame)));
        }
        ctx.request_repaint();
        return Ok(json!({"startFrame":start,"endFrame":end}));
    }
    if matches!(id, "reverseEngineering.markStart" | "reverseEngineering.markEnd") {
        capture_at(&clip, app.session.playhead())?;
        let frame = rate.frame_at(app.session.playhead()).saturating_sub(rate.frame_at(clip.start));
        let state = &mut app.ui.panels.reverse_engineering;
        if id == "reverseEngineering.markStart" {
            state.start_frame = frame;
        } else {
            state.end_frame = frame;
        }
        return Ok(json!({"frame":frame}));
    }
    if id != "reverseEngineering.extractRange" {
        return Err(format!("Unknown reverse-engineering command: {id}"));
    }
    let state = &app.ui.panels.reverse_engineering;
    let (start, end, interval) = (state.start_frame, state.end_frame, state.sample_interval);
    if start < 0 || end < start || end > last {
        return Err("Choose a start and end inside the selected clip, with end at or after start.".into());
    }
    if interval <= 0 {
        return Err("The sampling interval must be at least one frame.".into());
    }
    let span = end.saturating_sub(start);
    let count = (span / interval).saturating_add(1).saturating_add(i64::from(span % interval != 0));
    if count > MAX_SAMPLES as i64 {
        return Err("This range needs more than 64 previews. Increase the sampling interval or shorten the range.".into());
    }
    let base = rate.frame_at(clip.start);
    let mut samples = Vec::with_capacity(MAX_SAMPLES);
    let mut frame = start;
    for _ in 0..MAX_SAMPLES {
        samples.push(capture_at(&clip, rate.tick_of(base.saturating_add(frame)))?);
        if frame == end {
            break;
        }
        frame = frame.saturating_add(interval).min(end);
    }
    let count = samples.len();
    app.ui.panels.reverse_engineering.capture = samples.first().cloned();
    app.ui.panels.reverse_engineering.samples = samples;
    reset_decode_timer(ctx);
    Ok(json!({"samples":count}))
}

fn invoke(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str, params: Value) {
    let error = crate::menus::invoke(app, ctx, id, params).err();
    if let Some(error) = &error {
        app.ui.status = error.clone();
    }
    ctx.data_mut(|data| data.insert_temp(egui::Id::new("reverse-engineering-range-error"), error));
}

pub(super) fn timecode(app: &FilmcraftApp, capture: &ReverseEngineeringCapture) -> String {
    let source_rate = app.session.project.item(capture.item).map(|item| item.frame_rate()).unwrap_or_default();
    let timeline_rate = app.session.active_sequence().map(|sequence| sequence.settings.frame_rate).unwrap_or(source_rate);
    let source = format_time(capture.media_time, source_rate, false, TimeDisplay::Timecode, 48000);
    let timeline = format_time(capture.timeline_time, timeline_rate, false, TimeDisplay::Timecode, 48000);
    format!("Source: {source} · Timeline: {timeline}")
}

fn preview_request_clock(ctx: &egui::Context, item: u64, time: i64, width: u32, now: f64) -> (egui::Id, f64) {
    let key = egui::Id::new(("reverse-preview-request", item, time, width));
    let started = ctx.data_mut(|data| *data.get_temp_mut_or_insert_with(key, || now));
    (key, started)
}

pub(super) fn picture(app: &mut FilmcraftApp, ui: &mut egui::Ui, capture: &ReverseEngineeringCapture, width: u32, id: &str, clickable: bool) -> bool {
    if app.session.project.item(capture.item).is_none() {
        ui.label(tl!("The captured source is no longer available."));
        return false;
    }
    let (item, media_time) = match app.session.project.resolve_media(capture.item) {
        Some((parent, _, Some(range))) => (parent, capture.media_time + range.start),
        _ => (capture.item, capture.media_time),
    };
    let now = ui.input(|input| input.time);
    // Start a timeout when this exact preview is first requested, not when the range was selected.
    // Newly visible filmstrip cells must get a chance to decode after a long analysis.
    let (request_id, started) = preview_request_clock(ui.ctx(), item.0, media_time.0, width, now);
    let frame = app.session.project.item(item).map(|item| item.frame_rate().frame_at(media_time));
    let cached = frame.and_then(|frame| app.texture_existing(&format!("thumb-{}-{}-{width}", item.0, frame)));
    let expired = now - started >= 15.0;
    let preview = if !expired || cached.is_some() { app.thumbnail(ui.ctx(), item, media_time, width) } else { None };
    if let Some((texture, size)) = preview {
        let scale = ui.available_width().max(1.0).min(size.x) / size.x.max(1.0);
        let scale = if id == "reverseEngineering.frame" { scale.min(ui.available_height().max(1.0) / size.y.max(1.0)) } else { scale };
        let image = egui::Image::new((texture, size * scale)).sense(if clickable { egui::Sense::click() } else { egui::Sense::hover() });
        let response = ui.add(image);
        app.auto.add(id, response.rect, &capture.name);
        if id == "reverseEngineering.frame" {
            super::reverse_motion::overlay(app, ui, capture, response.rect);
        }
        response.clicked()
    } else {
        // Compact placeholders keep the results strip from becoming a column of error text.
        if expired {
            let response = ui.button(if clickable { "Retry" } else { "Retry loading frame" });
            app.auto.add(&format!("{id}.retry"), response.rect, "Retry this source preview without reanalysis");
            if response.clicked() {
                ui.ctx().data_mut(|data| data.insert_temp(request_id, now));
                let _ = app.thumbnail(ui.ctx(), item, media_time, width);
                ui.ctx().request_repaint();
            }
        } else {
            ui.label(if clickable { "Loading…" } else { "Loading analyzed frame…" });
            ui.allocate_space(vec2(ui.available_width().min(width as f32).max(1.0), if clickable { 24.0 } else { 48.0 }));
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
        false
    }
}

/// Range gesture on the actual timeline clip. Leaves ruler scrubbing and ordinary
/// editing untouched when Range mode is off; the isolated hook is called before clip editing.
pub fn timeline_range(
    app: &mut FilmcraftApp,
    ui: &mut egui::Ui,
    sequence: &filmcraft_project::Sequence,
    layout: &super::timeline::Layout,
    video_area: Rect,
) -> bool {
    if !app.ui.panels.reverse_engineering.range_mode || !app.ui.dock.is_visible(crate::dock::PanelKind::ReverseEngineering) {
        return false;
    }
    if super::reverse_motion::is_running(ui.ctx()) {
        return false;
    }
    let Ok(clip) = selected_clip(app) else { return false };
    let Some((track, _)) = sequence.find_item(clip.id) else { return false };
    let Some(row) = layout.rows.iter().find(|row| row.track == track) else { return false };
    let rect = Rect::from_min_max(egui::pos2(layout.x_of(clip.start), row.rect.top() + 2.0), egui::pos2(layout.x_of(clip.end()), row.rect.bottom() - 2.0))
        .intersect(layout.content)
        .intersect(video_area)
        .intersect(ui.clip_rect());
    if !rect.is_positive() {
        return false;
    }
    let last = prepare_range(app, &clip);
    let rate = sequence.settings.frame_rate;
    let base = rate.frame_at(clip.start);
    let x_at = |frame: i64| layout.x_of(rate.tick_of(base.saturating_add(frame.clamp(0, last))));
    let state = &app.ui.panels.reverse_engineering;
    let (left, right) = (x_at(state.start_frame), x_at(state.end_frame.saturating_add(1)).max(x_at(state.end_frame) + 2.0));
    let selected = Rect::from_min_max(egui::pos2(left, rect.top()), egui::pos2(right, rect.bottom())).intersect(rect);
    ui.painter().rect_filled(selected, 0.0, app.tokens.accent.gamma_multiply(0.25));
    ui.painter().rect_stroke(selected, 0.0, egui::Stroke::new(2.0, app.tokens.accent), egui::StrokeKind::Inside);
    app.auto.add("reverseEngineering.range.timeline", rect, "Drag on this timeline clip to select a range");
    for (x, id) in [(left, "reverseEngineering.range.handleStart"), (right, "reverseEngineering.range.handleEnd")] {
        let handle = Rect::from_center_size(egui::pos2(x, rect.center().y), vec2(6.0, rect.height())).intersect(rect);
        ui.painter().rect_filled(handle, 0.0, app.tokens.accent);
        app.auto.add(id, handle, "Drag range boundary");
    }
    let drag_id = egui::Id::new(("reverse-engineering-timeline-drag", clip.id.0));
    let (pressed, down, released, origin, pointer) = ui.input(|input| {
        (
            input.pointer.primary_pressed(),
            input.pointer.primary_down(),
            input.pointer.primary_released(),
            input.pointer.press_origin(),
            input.pointer.interact_pos(),
        )
    });
    let frame_at = |x: f32| rate.frame_at(layout.tick_at(x)).saturating_sub(base).clamp(0, last);
    if pressed && let Some(origin) = origin.filter(|point| rect.contains(*point)) {
        let a = (origin.x - left).abs();
        let b = (origin.x - right).abs();
        let mode = if a.min(b) < 9.0 { if a <= b { i64::MIN } else { i64::MAX } } else { frame_at(origin.x) };
        ui.ctx().data_mut(|data| data.insert_temp(drag_id, mode));
        app.stop();
    }
    let active = ui.ctx().data(|data| data.get_temp::<i64>(drag_id));
    if let Some(mode) = active {
        if (down || released)
            && let Some(pointer) = pointer
        {
            let frame = frame_at(pointer.x);
            let state = &app.ui.panels.reverse_engineering;
            let (start, end) = match mode {
                i64::MIN => (frame.min(state.end_frame), state.end_frame),
                i64::MAX => (state.start_frame, frame.max(state.start_frame)),
                anchor => (anchor.min(frame), anchor.max(frame)),
            };
            invoke(app, ui.ctx(), "reverseEngineering.setRange", json!({"startFrame":start,"endFrame":end}));
            app.session.set_playhead(rate.tick_of(base.saturating_add(frame)));
            ui.ctx().request_repaint();
        }
        if released || !down {
            ui.ctx().data_mut(|data| data.remove::<i64>(drag_id));
        }
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        return true;
    }
    if pointer.is_some_and(|point| rect.contains(point)) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    false
}

pub(super) fn motion_is_current(app: &FilmcraftApp) -> bool {
    let state = &app.ui.panels.reverse_engineering;
    let Some(report) = state.motion.as_ref() else {
        return false;
    };
    let Ok(clip) = selected_clip(app) else {
        return false;
    };
    let Some(sequence) = app.session.active_sequence() else {
        return false;
    };
    let rate = sequence.settings.frame_rate;
    let base = rate.frame_at(clip.start);
    let first = capture_at(&clip, rate.tick_of(base.saturating_add(state.start_frame))).ok();
    let last = capture_at(&clip, rate.tick_of(base.saturating_add(state.end_frame))).ok();
    report.captures.first() == first.as_ref()
        && report.captures.last() == last.as_ref()
        && report.captures.len() as i64 == state.end_frame.saturating_sub(state.start_frame).saturating_add(1)
        && report.timeline_frame_rate == serde_json::to_value(rate).unwrap_or(Value::Null)
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let bounds = rect.shrink2(vec2(12.0, 10.0));
    let inspect_motion = app.ui.panels.reverse_engineering.range_mode && motion_is_current(app) && !super::reverse_motion::is_running(ui.ctx());
    let split = bounds.top() + bounds.height() * 0.32;
    let controls = if inspect_motion { Rect::from_min_max(bounds.min, egui::pos2(bounds.right(), split)) } else { bounds };
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(controls));
    egui::ScrollArea::vertical().id_salt("reverse-engineering-scroll").show(&mut child, |ui| {
        ui.label(egui::RichText::new(tl!("Reverse Engineering")).color(app.tokens.text));
        let intro = tl!("Inspect scenes, camera, lighting and motion.");
        let response = ui.label(egui::RichText::new(intro).color(app.tokens.text_faint));
        app.auto.add("reverseEngineering.intro", response.rect, intro);
        ui.separator();
        let selected = selected_clip(app);
        let label = selected.as_ref().map(|clip| format!("Video: {}", clip.name)).unwrap_or_else(|error| error.clone());
        let response = ui.label(&label);
        app.auto.add("reverseEngineering.selection", response.rect, &label);
        ui.horizontal(|ui| {
            for (range, label, id) in [(false, "Single image", "reverseEngineering.mode.frame"), (true, "Analyze range", "reverseEngineering.mode.range")] {
                let response = ui.selectable_value(&mut app.ui.panels.reverse_engineering.range_mode, range, label);
                app.auto.add(id, response.rect, label);
            }
        });
        if app.ui.panels.reverse_engineering.range_mode {
            if let Ok(clip) = &selected {
                prepare_range(app, clip);
                let rate = app.session.active_sequence().map(|sequence| sequence.settings.frame_rate).unwrap_or_default();
                ui.label("Drag on the selected clip in the main timeline. Scrub the ruler to inspect it in the Program monitor.");
                let drop_frame = app.session.active_sequence().is_some_and(|sequence| sequence.settings.drop_frame);
                ui.add_enabled_ui(!super::reverse_motion::is_running(ui.ctx()), |ui| {
                    egui::Grid::new("reverse-engineering-range-controls").num_columns(2).show(ui, |ui| {
                        for (start, label, id, mark) in [
                            (true, "Start", "reverseEngineering.range.start", "reverseEngineering.markStart"),
                            (false, "End", "reverseEngineering.range.end", "reverseEngineering.markEnd"),
                        ] {
                            ui.label(label);
                            ui.horizontal(|ui| {
                                let frame = if start { app.ui.panels.reverse_engineering.start_frame } else { app.ui.panels.reverse_engineering.end_frame };
                                let text =
                                    format_time(rate.tick_of(rate.frame_at(clip.start).saturating_add(frame)), rate, drop_frame, TimeDisplay::Timecode, 48000);
                                let draft_id = egui::Id::new(("reverse-engineering-timestamp", clip.id.0, start));
                                let mut draft = ui.ctx().data(|data| data.get_temp::<String>(draft_id)).unwrap_or(text.clone());
                                let response = ui.add(egui::TextEdit::singleline(&mut draft).desired_width(110.0).hint_text("HH:MM:SS:FF").id(draft_id));
                                app.auto.add(id, response.rect, &draft);
                                let commit = response.lost_focus() || (response.has_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)));
                                if commit && draft != text {
                                    let key = if start { "startTimecode" } else { "endTimecode" };
                                    invoke(app, ui.ctx(), "reverseEngineering.setRange", json!({key: draft}));
                                }
                                if response.has_focus() {
                                    ui.ctx().data_mut(|data| data.insert_temp(draft_id, draft));
                                } else {
                                    ui.ctx().data_mut(|data| data.remove::<String>(draft_id));
                                }
                                let response = ui.add_enabled(capture_at(clip, app.session.playhead()).is_ok(), egui::Button::new("Use playhead"));
                                app.auto.add(mark, response.rect, "Use playhead");
                                if response.clicked() {
                                    invoke(app, ui.ctx(), mark, json!({}));
                                }
                            });
                            ui.end_row();
                        }
                    });
                });
                if let Some(Some(error)) = ui.ctx().data(|data| data.get_temp::<Option<String>>(egui::Id::new("reverse-engineering-range-error"))) {
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                }
                let count = app.ui.panels.reverse_engineering.end_frame.saturating_sub(app.ui.panels.reverse_engineering.start_frame).saturating_add(1);
                ui.label(format!("1. Select range · {count} frames · {:.3} fps · every frame will be analyzed", rate.num as f64 / rate.den.max(1) as f64));
            }
            ui.horizontal(|ui| {
                let response = ui.add_enabled(selected.is_ok(), egui::Button::new("Replay range"));
                app.auto.add("reverseEngineering.playRange", response.rect, "Replay range");
                if response.clicked() {
                    invoke(app, ui.ctx(), "reverseEngineering.playRange", json!({}));
                }
                let response = ui.button("Stop");
                app.auto.add("reverseEngineering.stop", response.rect, "Stop range playback");
                if response.clicked() {
                    invoke(app, ui.ctx(), "playback.stop", json!({"target":"program"}));
                }
            });
            super::reverse_motion::show(app, ui);
        } else {
            let valid = selected.as_ref().is_ok_and(|clip| capture_at(clip, app.session.playhead()).is_ok());
            let response = ui.add_enabled(valid, egui::Button::new(tl!("Extract current frame")));
            app.auto.add("reverseEngineering.extractFrame", response.rect, "Extract current frame");
            if response.clicked() {
                invoke(app, ui.ctx(), "reverseEngineering.extractFrame", json!({}));
            }
            if selected.is_ok() && !valid {
                ui.label("Move the playhead inside the selected video clip.");
            }
        }
        if !app.ui.panels.reverse_engineering.range_mode
            && let Some(capture) = app.ui.panels.reverse_engineering.capture.clone()
        {
            ui.separator();
            ui.label(&capture.name);
            let label = timecode(app, &capture);
            let response = ui.label(&label);
            app.auto.add("reverseEngineering.timecode", response.rect, &label);
            picture(app, ui, &capture, 640, "reverseEngineering.frame", false);
        }
    });
    if inspect_motion {
        let results = Rect::from_min_max(egui::pos2(bounds.left(), split + 8.0), bounds.max);
        let mut result_ui = ui.new_child(egui::UiBuilder::new().max_rect(results));
        super::reverse_motion::show_result(app, &mut result_ui);
    }
}

#[cfg(test)]
mod preview_loading_tests {
    use super::*;
    #[test]
    fn late_visible_frames_have_independent_loading_deadlines() {
        let ctx = egui::Context::default();
        ctx.data_mut(|data| data.insert_temp(egui::Id::new("reverse-engineering-requested-at"), 0.0f64));
        let (_, first) = preview_request_clock(&ctx, 5, 100, 96, 30.0);
        assert_eq!(first, 30.0, "old analysis timer must not prevent initial thumbnail requests");
        let (_, next) = preview_request_clock(&ctx, 5, 101, 96, 60.0);
        assert_eq!(next, 60.0, "newly visible frame gets its own timeout");
        let (_, large) = preview_request_clock(&ctx, 5, 100, 640, 70.0);
        assert_eq!(large, 70.0, "large preview loads independently of thumbnail");
        assert_eq!(preview_request_clock(&ctx, 5, 100, 96, 80.0).1, 30.0, "same request retains bounded timeout");
    }
}

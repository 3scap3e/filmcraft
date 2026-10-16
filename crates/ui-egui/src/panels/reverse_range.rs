//! Explicit confirmation before replacing an analysis range and clearing its results.
use crate::FilmcraftApp;
use serde_json::{Value, json};
#[derive(Clone)]
struct Pending {
    start: i64,
    end: i64,
    old_start: i64,
    old_end: i64,
    sequence: Option<filmcraft_project::ItemId>,
}
fn key() -> egui::Id {
    egui::Id::new("reverse-range-confirmation")
}
pub(super) fn pending(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<Pending>(key())).is_some()
}
pub(super) fn request(app: &FilmcraftApp, ctx: &egui::Context, start: i64, end: i64) -> bool {
    let s = &app.ui.panels.reverse_engineering;
    if (s.start_frame, s.end_frame) == (start, end) {
        return false;
    }
    ctx.data_mut(|d| d.remove::<String>(egui::Id::new("reverse-range-confirm-error")));
    ctx.data_mut(|d| d.insert_temp(key(), Pending { start, end, old_start: s.start_frame, old_end: s.end_frame, sequence: app.session.state.active_sequence }));
    ctx.request_repaint();
    true
}
pub(super) fn confirm(app: &mut FilmcraftApp, ctx: &egui::Context, params: &Value) -> Result<Value, String> {
    let accept = params.get("accept").and_then(Value::as_bool).ok_or("Supply accept: true or false")?;
    let p = ctx.data(|d| d.get_temp::<Pending>(key())).ok_or("No range change is awaiting confirmation")?;
    if !accept {
        ctx.data_mut(|d| d.remove::<Pending>(key()));
        ctx.request_repaint();
        return Ok(json!({"cancelled":true}));
    }
    if super::reverse_motion::is_running(ctx)
        || super::reverse_tools::is_running(ctx)
        || (super::reverse_tracking::is_running(ctx) || super::reverse_sam::is_running(ctx))
        || super::reverse_shots::is_running(ctx)
        || super::reverse_camera2d::is_running(ctx)
    {
        return Err("Cancel analysis before accepting a new range".into());
    }
    let s = &app.ui.panels.reverse_engineering;
    if p.sequence != app.session.state.active_sequence || p.old_start != s.start_frame || p.old_end != s.end_frame {
        ctx.data_mut(|d| d.remove::<Pending>(key()));
        return Err("The timeline or range changed while confirmation was open. Select the range again.".into());
    }
    // Persistence must succeed before any working result or committed range is changed.
    super::reverse_tools::clear_range_results(app)?;
    let s = &mut app.ui.panels.reverse_engineering;
    s.start_frame = p.start;
    s.end_frame = p.end;
    s.active_tool = "menu".into();
    ctx.data_mut(|d| {
        d.remove::<Pending>(key());
        d.remove::<usize>(egui::Id::new("reverse-motion-frame"));
        d.remove::<usize>(egui::Id::new("reverse-motion-index"));
        d.remove::<String>(egui::Id::new("reverse-tools-error"));
    });
    app.ui.status = "New range selected; previous range analysis cleared.".into();
    ctx.request_repaint();
    Ok(json!({"startFrame":p.start,"endFrame":p.end,"cleared":true}))
}
pub(super) fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let Some(p) = ctx.data(|d| d.get_temp::<Pending>(key())) else {
        return;
    };
    let response = egui::Modal::new(key()).show(ctx, |ui| {
        ui.set_width(390.0);
        ui.heading("Start again with a new range?");
        ui.label(format!("New range: frames {}–{}", p.start.saturating_add(1), p.end.saturating_add(1)));
        ui.label("This removes the previous range’s saved and working analyses from every tool, clears point selections, and resets tool highlights.");
        ui.horizontal(|ui| {
            for (accept, label, id) in [(false, "Cancel", "reverseEngineering.range.cancel"), (true, "Yes, start again", "reverseEngineering.range.confirm")] {
                let response = ui.button(label);
                app.auto.add(id, response.rect, label);
                if response.clicked()
                    && let Err(e) = crate::menus::invoke(app, ctx, "reverseEngineering.confirmRangeChange", json!({"accept":accept}))
                {
                    app.ui.status = e.clone();
                    ctx.data_mut(|d| d.insert_temp(egui::Id::new("reverse-range-confirm-error"), e));
                }
            }
        });
        if let Some(e) = ctx.data(|d| d.get_temp::<String>(egui::Id::new("reverse-range-confirm-error"))) {
            ui.colored_label(egui::Color32::LIGHT_RED, e);
        }
    });
    if response.should_close() && pending(ctx) {
        let _ = confirm(app, ctx, &json!({"accept":false}));
    }
}

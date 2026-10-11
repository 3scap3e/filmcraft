//! Native workspace for reverse-engineering video. RE1 adds the dockable shell only.
//! Open with Window ▸ Reverse Engineering or `ui.panel.show {panel: "ReverseEngineering"}`.
//! Automation id: `reverseEngineering.intro` (the panel's introductory label).

use egui::{Rect, vec2};

use crate::FilmcraftApp;

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(12.0, 10.0))));
    egui::ScrollArea::vertical().id_salt("reverse-engineering-scroll").show(&mut child, |ui| {
        ui.label(egui::RichText::new(tl!("Reverse Engineering")).color(app.tokens.text));
        ui.add_space(8.0);
        let intro = tl!("Inspect scenes, camera, lighting and motion.");
        let response = ui.label(egui::RichText::new(intro).color(app.tokens.text_faint));
        app.auto.add("reverseEngineering.intro", response.rect, intro);
    });
}

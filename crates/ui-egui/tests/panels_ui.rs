//! Headless UI tests of the M8.9 / M12.6 panels: Lumetri Scopes (layouts, the settings menu,
//! footer), Metadata (edit a field by typing, undo), Timecode, Events, Progress and the Reference
//! Monitor, each opened from Window ▸ and driven by automation id through the control channel.
//!
//! Set `FILMCRAFT_UI_SNAPSHOT_DIR=<dir>` to also render offscreen with wgpu and write `panels-*.png`.

#![allow(dead_code)]

use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::control::ControlRequest;
use serde_json::{Value, json};

struct Driver {
    harness: Harness<'static, FilmcraftApp>,
    tx: Sender<ControlRequest>,
    snapshots: Option<std::path::PathBuf>,
}

impl Driver {
    fn with(session: Session) -> Self {
        let (tx, rx) = channel();
        let app = FilmcraftApp::new(session).with_control(rx);
        let snapshots = std::env::var_os("FILMCRAFT_UI_SNAPSHOT_DIR").map(std::path::PathBuf::from);
        let mut b = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000);
        if snapshots.is_some() {
            b = b.wgpu();
        }
        let harness = b.build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx, snapshots };
        d.frames(4);
        d
    }

    fn demo() -> Self {
        let mut session = Session::default();
        session.execute("file.openDemoProject", json!({})).expect("demo project");
        Self::with(session)
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            let ctx = self.harness.ctx.clone();
            let mut raw = std::mem::take(self.harness.input_mut());
            eframe::App::raw_input_hook(self.harness.state_mut(), &ctx, &mut raw);
            *self.harness.input_mut() = raw;
            self.harness.step();
        }
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        let (req, reply) = ControlRequest::new(method, params.clone());
        self.tx.send(req).unwrap();
        for _ in 0..600 {
            self.frames(1);
            if let Ok(v) = reply.try_recv() {
                return v;
            }
        }
        panic!("no reply to {method} {params}");
    }

    fn ok(&mut self, method: &str, params: Value) -> Value {
        let v = self.call(method, params.clone());
        assert_eq!(v["ok"], json!(true), "{method} {params} failed: {v}");
        let result = v["result"].clone();
        if result.get("confirmationRequired").and_then(Value::as_bool) == Some(true) {
            return self.ok("ui.menu.invoke", json!({"id":"reverseEngineering.confirmRangeChange","params":{"accept":true}}));
        }
        result
    }

    fn exec(&mut self, command: &str, params: Value) -> Value {
        self.ok("engine.execute", json!({"command": command, "params": params}))
    }

    fn click(&mut self, id: &str) {
        self.ok("ui.click", json!({"id": id}));
        self.frames(2);
    }

    fn ids(&mut self, prefix: &str) -> Vec<String> {
        let v = self.ok("ui.elements", json!({"prefix": prefix}));
        v.as_array().unwrap().iter().filter_map(|e| e["id"].as_str().map(str::to_string)).collect()
    }

    fn has(&mut self, id: &str) -> bool {
        self.ids(id).iter().any(|i| i == id)
    }

    fn label(&mut self, id: &str) -> String {
        let v = self.ok("ui.elements", json!({"prefix": id}));
        v.as_array().unwrap().iter().find(|e| e["id"] == json!(id)).and_then(|e| e["label"].as_str().map(str::to_string)).unwrap_or_default()
    }

    /// Step frames until `id` is registered (frames render on worker threads).
    fn wait_for(&mut self, id: &str) {
        for _ in 0..400 {
            if self.has(id) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
            self.frames(1);
        }
        panic!("{id} never appeared: {:?}", self.ids(id.split('.').next().unwrap()));
    }

    fn app(&mut self) -> &mut FilmcraftApp {
        self.harness.state_mut()
    }

    fn ui_panels(&mut self) -> Value {
        self.ok("ui.inspect", json!({}))["ui"]["panels"].clone()
    }

    /// Offscreen render of the window (or the bounding box of the elements under `prefix`).
    fn snapshot(&mut self, name: &str, prefix: Option<&str>) {
        let Some(dir) = self.snapshots.clone() else { return };
        let crop = prefix.map(|p| {
            let v = self.ok("ui.elements", json!({"prefix": p}));
            let mut bb = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
            for e in v.as_array().unwrap() {
                let r = &e["rect"];
                let (x, y, w, h) = (r[0].as_f64().unwrap() as f32, r[1].as_f64().unwrap() as f32, r[2].as_f64().unwrap() as f32, r[3].as_f64().unwrap() as f32);
                bb = [bb[0].min(x), bb[1].min(y), bb[2].max(x + w), bb[3].max(y + h)];
            }
            bb
        });
        self.frames(2);
        let img = match self.harness.render() {
            Ok(i) => i,
            Err(e) => {
                eprintln!("snapshot {name} skipped: {e}");
                return;
            }
        };
        let ppp = img.width() as f32 / 1600.0;
        let img = match crop {
            Some(bb) => {
                let x0 = ((bb[0] - 24.0) * ppp).max(0.0) as u32;
                let y0 = ((bb[1] - 30.0) * ppp).max(0.0) as u32;
                let x1 = (((bb[2] + 16.0) * ppp) as u32).min(img.width());
                let y1 = (((bb[3] + 46.0) * ppp) as u32).min(img.height());
                image::imageops::crop_imm(&img, x0, y0, x1 - x0, y1 - y0).to_image()
            }
            None => img,
        };
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{name}.png"));
        img.save(&path).unwrap();
        eprintln!("snapshot: {}", path.display());
    }
}

#[test]
fn every_panel_opens_from_the_window_menu() {
    let mut d = Driver::demo();
    let menu = d.ok("ui.menu.list", json!({}));
    let window: Vec<String> =
        menu.as_array().unwrap().iter().filter(|m| m["path"] == json!(["Window"])).filter_map(|m| m["label"].as_str().map(str::to_string)).collect();
    for p in ["Events", "Lumetri Scopes", "Metadata", "Progress", "Project Notes", "Reference Monitor", "Reverse Engineering", "Timecode"] {
        assert!(window.iter().any(|w| w == p), "{p} not in Window: {window:?}");
    }
    for (panel, key) in [
        ("LumetriScopes", "scopes.wrench"),
        ("Metadata", "metadata."),
        ("Timecode", "timecode.row.0"),
        ("Events", "events.clearAll"),
        ("Progress", "progress.showFinished"),
        ("ReferenceMonitor", "reference.gang"),
        ("ProjectNotes", "notes.text"),
        ("ReverseEngineering", "reverseEngineering.intro"),
    ] {
        d.ok("ui.menu.invoke", json!({"id": format!("window.panel.{panel}")}));
        d.frames(3);
        assert!(d.has(&format!("panel.tab.{panel}")), "{panel} has no tab");
        assert!(!d.ids(key).is_empty(), "{panel}: no {key} widgets");
        assert_eq!(d.ok("ui.inspect", json!({}))["ui"]["focused"], json!(panel));
    }
}

/// #284 point 3: the panel menu's Maximize Frame keeps the layout (Restore Frame brings it back),
/// and closing every panel still leaves a dock that Window ▸ can reopen panels into.
#[test]
fn panel_menu_maximize_restores_and_an_empty_dock_reopens() {
    use filmcraft_ui_egui::dock::{DockNode, PanelKind};
    let mut d = Driver::demo();
    let layout = d.app().ui.dock.clone();
    d.click("panel.menu.Program");
    assert_eq!(d.label("panel.menu.Program.maximize"), "Maximize Frame");
    d.click("panel.menu.Program.maximize");
    assert_eq!(d.app().ui.keys.maximized, Some(PanelKind::Program));
    assert!(!d.has("panel.Timeline"), "only the maximized panel is shown");
    d.click("panel.menu.Program");
    assert_eq!(d.label("panel.menu.Program.maximize"), "Restore Frame");
    d.click("panel.menu.Program.maximize");
    assert_eq!(d.app().ui.keys.maximized, None);
    assert_eq!(d.app().ui.dock, layout, "the layout survives maximize / restore");
    assert!(d.has("panel.Timeline"));

    let mut all = Vec::new();
    layout.panels(&mut all);
    for p in all {
        d.ok("ui.panel.close", json!({"panel": p.id()}));
    }
    assert_eq!(d.app().ui.dock, DockNode::Tabs { panels: vec![], active: 0 });
    d.ok("ui.menu.invoke", json!({"id": "window.panel.Program"}));
    d.ok("ui.menu.invoke", json!({"id": "window.panel.Timeline"}));
    d.frames(3);
    assert!(d.has("panel.tab.Program") && d.has("panel.tab.Timeline"));
}

#[test]
fn lumetri_scopes_layouts_menu_and_footer() {
    let mut d = Driver::demo();
    d.ok("ui.set", json!({"workspace": "Color"}));
    d.ok("ui.panel.show", json!({"panel": "LumetriScopes"}));
    // the default: RGB parade, Rec. 709, 8 Bit, Clamp Signal
    d.wait_for("scopes.view.parade");
    assert_eq!(d.label("scopes.space"), "Rec. 709");
    let p = d.ui_panels();
    assert_eq!((p["scopes"]["shown"].clone(), p["scopes"]["scale"].clone(), p["scopes"]["clamp"].clone()), (json!(["parade"]), json!("bits8"), json!(true)));
    d.snapshot("panels-scopes-parade", Some("scopes."));
    // the wrench menu: add the YUV vectorscope, switch the waveform type through its submenu
    d.click("scopes.wrench");
    assert!(d.has("scopes.menu.vectorscopeYuv"), "{:?}", d.ids("scopes.menu"));
    d.click("scopes.menu.vectorscopeYuv");
    d.wait_for("scopes.view.vectorscopeYuv");
    assert!(d.has("scopes.view.parade"));
    // Clamp Signal in the footer
    d.click("scopes.clamp");
    assert_eq!(d.ui_panels()["scopes"]["clamp"], json!(false));
    // all five scopes in a grid, through ui.set; then a preset
    d.ok("ui.set", json!({"panels": {"scopes": {"shown": ["vectorscopeYuv", "vectorscopeHls", "histogram", "parade", "waveform"], "waveformType": "yc", "paradeType": "rgbWhite", "brightness": "bright", "targets": "100"}}}));
    for k in ["vectorscopeYuv", "vectorscopeHls", "histogram", "parade", "waveform"] {
        d.wait_for(&format!("scopes.view.{k}"));
    }
    let rects: Vec<Value> = d.ok("ui.elements", json!({"prefix": "scopes.view."})).as_array().unwrap().iter().map(|e| e["rect"].clone()).collect();
    assert_eq!(rects.len(), 5);
    d.snapshot("panels-scopes-five", Some("scopes."));
    d.click("scopes.wrench");
    d.ok("ui.click", json!({"id": "scopes.menu.presets"}));
    d.frames(3);
    d.wait_for("scopes.menu.preset.5");
    d.click("scopes.menu.preset.5");
    let s = d.ui_panels()["scopes"].clone();
    assert_eq!(s["preset"], json!("Four Scopes (YUV, float, no clamp)"), "{s}");
    assert_eq!((s["scale"].clone(), s["clamp"].clone(), s["paradeType"].clone()), (json!("float"), json!(false), json!("yuv")));
    assert_eq!(s["shown"].as_array().unwrap().len(), 4);
    // colour space: Rec. 601 shows in the footer
    d.ok("ui.set", json!({"panels": {"scopes": {"colorSpace": "rec601"}}}));
    d.frames(3);
    assert_eq!(d.label("scopes.space"), "Rec. 601");
    // the numbers behind it, for agents
    let r = d.exec("scopes.read", json!({"scopes": ["histogram"], "colorSpace": "601", "bins": false}));
    assert_eq!(r["matrix"], "Bt601");
    assert!(r["histogram"]["samples"].as_u64().unwrap() > 1000, "{r}");
}

#[test]
fn metadata_panel_edits_a_field_with_undo() {
    let mut d = Driver::demo();
    let item = d.app().session.project.items.values().filter(|i| i.as_media().is_some()).map(|i| i.id).min().unwrap();
    d.app().session.state.project_selection = vec![item];
    d.app().ui.focused = filmcraft_ui_egui::dock::PanelKind::Project;
    d.ok("ui.menu.invoke", json!({"id": "window.panel.Metadata"}));
    d.ok("ui.set", json!({"focused": "Project"}));
    d.frames(3);
    let name = d.app().session.project.item(item).unwrap().name.clone();
    assert_eq!(d.label("metadata.header"), format!("Clip: {name}"));
    for id in [
        "metadata.field.Name",
        "metadata.label",
        "metadata.field.MediaStart",
        "metadata.field.MediaDuration",
        "metadata.field.Scene",
        "metadata.field.LogNote",
        "metadata.field.FilePath",
        "metadata.section.File",
    ] {
        assert!(d.has(id), "{id} missing: {:?}", d.ids("metadata."));
    }
    d.snapshot("panels-metadata", Some("metadata."));
    // type into Scene and commit with Enter: one undo step
    let undo0 = d.app().session.history.undo.len();
    d.click("metadata.field.Scene");
    d.ok("ui.type", json!({"text": "42A"}));
    d.frames(2);
    d.ok("ui.key", json!({"key": "Enter"}));
    d.frames(3);
    assert_eq!(d.app().session.project.item(item).unwrap().metadata.get("Scene").map(String::as_str), Some("42A"));
    assert_eq!(d.app().session.history.undo.len(), undo0 + 1);
    assert_eq!(d.label("metadata.field.Scene"), "42A");
    d.exec("edit.undo", json!({}));
    d.frames(2);
    assert!(!d.app().session.project.item(item).unwrap().metadata.contains_key("Scene"));
    // the same edit from an agent
    d.exec("metadata.set", json!({"item": item.0, "field": "Tape Name", "value": "A001"}));
    d.frames(2);
    assert_eq!(d.label("metadata.field.TapeName"), "A001");
}

/// #620: notes typed into Project Notes land in the project; one typing session is one undo step.
#[test]
fn project_notes_panel_types_into_the_project_with_undo() {
    let mut d = Driver::demo();
    d.ok("ui.menu.invoke", json!({"id": "window.panel.ProjectNotes"}));
    d.frames(3);
    let undo0 = d.app().session.history.undo.len();
    d.click("notes.text");
    d.ok("ui.type", json!({"text": "Fix logo"}));
    d.frames(2);
    d.ok("ui.key", json!({"key": "Enter"}));
    d.ok("ui.type", json!({"text": "at 1:02"}));
    d.frames(3);
    assert_eq!(d.app().session.project.notes, "Fix logo\nat 1:02");
    assert_eq!(d.app().session.history.undo.len(), undo0 + 1);
    assert_eq!(d.label("notes.text"), "Fix logo\nat 1:02");
    d.exec("edit.undo", json!({}));
    d.frames(2);
    assert_eq!(d.app().session.project.notes, "");
    // an agent writes them, the panel shows them
    d.exec("project.setNotes", json!({"text": "From the script"}));
    d.frames(2);
    assert_eq!(d.label("notes.text"), "From the script");
}

#[test]
fn timecode_panel_rows_and_modes() {
    let mut d = Driver::demo();
    d.exec("playhead.set", json!({"timecode": "00:00:02:00"}));
    d.ok("ui.menu.invoke", json!({"id": "window.panel.Timecode"}));
    d.frames(3);
    assert_eq!(d.label("timecode.row.0"), "00:00:02:00");
    d.click("timecode.addRow");
    assert_eq!(d.ui_panels()["timecode"]["rows"].as_array().unwrap().len(), 2);
    // the new row is a duration: the sequence's
    let (dur, df, rate) = {
        let q = d.app().session.active_sequence().unwrap();
        (q.duration(), q.settings.drop_frame, q.settings.frame_rate)
    };
    let want = filmcraft_time::format_time(dur, rate, df, filmcraft_time::TimeDisplay::Timecode, 48000);
    assert_eq!(d.label("timecode.row.1"), want);
    // frames display and Remaining through ui.set
    d.ok("ui.set", json!({"panels": {"timecode": {"rows": [{"mode": "current", "display": "Frames"}, {"mode": "remaining"}]}}}));
    d.frames(2);
    let frame = rate.frame_at(d.app().session.playhead());
    assert_eq!(d.label("timecode.row.0"), frame.to_string());
    let rem = filmcraft_time::format_time(dur - d.app().session.playhead(), rate, df, filmcraft_time::TimeDisplay::Timecode, 48000);
    assert_eq!(d.label("timecode.row.1"), rem);
    d.snapshot("panels-timecode", Some("timecode."));
    // right-click menu: Media Time
    d.ok("ui.click", json!({"id": "timecode.row.1", "button": "right"}));
    d.frames(2);
    if d.has("timecode.row.1.mode.media") {
        d.click("timecode.row.1.mode.media");
        assert_eq!(d.ui_panels()["timecode"]["rows"][1]["mode"], json!("media"));
    }
}

#[test]
fn events_and_progress_panels() {
    let mut d = Driver::demo();
    d.exec("events.clear", json!({}));
    d.ok("ui.menu.invoke", json!({"id": "window.panel.Events"}));
    d.frames(2);
    // a failed command shows up as an error row
    let r = d.call("engine.execute", json!({"command": "clip.rename", "params": {}}));
    assert_eq!(r["ok"], json!(false));
    d.frames(2);
    let rows = d.ids("events.row.");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(d.label(&rows[0]).contains("name"), "{}", d.label(&rows[0]));
    d.click(&rows[0]);
    assert!(d.has("events.details"));
    d.snapshot("panels-events", Some("events."));
    // filter: errors only still shows it; Clear All empties the list
    d.ok("ui.set", json!({"panels": {"events": {"level": "error"}}}));
    d.frames(2);
    assert_eq!(d.ids("events.row.").len(), 1);
    d.click("events.clearAll");
    assert!(d.ids("events.row.").is_empty());
    assert!(d.app().session.log.entries.is_empty());

    // Progress: a running job with a Cancel button
    let job = filmcraft_engine::Job {
        id: 4242,
        label: "Export demo.mp4".into(),
        progress: Arc::new(Default::default()),
        result: Arc::new(std::sync::Mutex::new(None)),
    };
    job.progress.total.store(10, std::sync::atomic::Ordering::Relaxed);
    job.progress.done.store(4, std::sync::atomic::Ordering::Relaxed);
    d.app().session.jobs.push(job.clone());
    d.ok("ui.menu.invoke", json!({"id": "window.panel.Progress"}));
    d.frames(3);
    assert!(d.label("progress.job.4242").contains("40 %"), "{}", d.label("progress.job.4242"));
    d.snapshot("panels-progress", Some("progress."));
    d.click("progress.cancel.4242");
    assert!(job.progress.cancel.load(std::sync::atomic::Ordering::Relaxed));
    *job.result.lock().unwrap() = Some(Err("cancelled".into()));
    d.frames(3);
    assert!(!d.has("progress.cancel.4242"));
    assert!(d.label("progress.job.4242").contains("Cancelled"));
    // the Events panel logged the job
    let ev = d.exec("events.list", json!({}));
    let msgs: Vec<&str> = ev["entries"].as_array().unwrap().iter().filter_map(|e| e["message"].as_str()).collect();
    assert!(msgs.contains(&"Started: Export demo.mp4") && msgs.contains(&"Cancelled: Export demo.mp4"), "{msgs:?}");
    // hide finished jobs
    d.click("progress.showFinished");
    assert!(!d.has("progress.job.4242"));
}

#[test]
fn reference_monitor_parks_gangs_and_shows_scopes() {
    let mut d = Driver::demo();
    d.ok("ui.menu.invoke", json!({"id": "window.panel.ReferenceMonitor"}));
    d.frames(3);
    assert_eq!(d.label("reference.timecode"), "00:00:00:00");
    d.exec("playhead.set", json!({"timecode": "00:00:03:00"}));
    d.frames(2);
    // parked: the playhead moved, the reference did not
    assert_eq!(d.label("reference.timecode"), "00:00:00:00");
    d.click("reference.matchPlayhead");
    assert_eq!(d.label("reference.timecode"), "00:00:03:00");
    d.click("reference.stepForward");
    assert_eq!(d.label("reference.timecode"), "00:00:03:01");
    // ganged: follows the playhead
    d.click("reference.gang");
    assert_eq!(d.ui_panels()["reference"]["ganged"], json!(true));
    d.exec("playhead.set", json!({"timecode": "00:00:01:00"}));
    d.frames(2);
    assert_eq!(d.label("reference.timecode"), "00:00:01:00");
    d.wait_for("reference.picture");
    d.snapshot("panels-reference-composite", Some("reference."));
    // scopes of the reference frame
    d.ok("ui.set", json!({"panels": {"reference": {"display": "scopes"}}}));
    d.wait_for("reference.scopes.view.vectorscopeYuv");
    d.wait_for("reference.scopes.view.waveform");
    d.snapshot("panels-reference-scopes", Some("reference."));
}

#[test]
fn scopes_of_colour_bars() {
    let mut s = Session::default();
    let id = s.execute("file.newBarsAndTone", json!({"seconds": 2})).unwrap()["item"].as_u64().unwrap();
    s.execute("file.newSequenceFromClip", json!({"items": [id]})).unwrap();
    let mut d = Driver::with(s);
    d.ok(
        "ui.set",
        json!({"workspace": "Color", "panels": {"scopes": {"shown": ["vectorscopeYuv", "histogram", "parade", "waveform"], "waveformType": "luma"}}}),
    );
    d.ok("ui.panel.show", json!({"panel": "LumetriScopes"}));
    for k in ["vectorscopeYuv", "histogram", "parade", "waveform"] {
        d.wait_for(&format!("scopes.view.{k}"));
    }
    d.snapshot("panels-scopes-bars", Some("scopes."));
    // the numbers: the six 75 % bars sit on their targets
    let r = d.exec("scopes.read", json!({"scopes": ["vectorscopeYuv", "parade"], "columns": 8}));
    let peaks = r["vectorscopeYuv"]["peaks"].as_array().unwrap();
    let angles: Vec<f64> = peaks.iter().take(7).map(|p| p["angleDeg"].as_f64().unwrap()).collect();
    for t in filmcraft_scopes::targets(filmcraft_scopes::Matrix::Bt709, 191.0 / 255.0) {
        let a = filmcraft_scopes::angle_deg(t.cb, t.cr) as f64;
        assert!(angles.iter().any(|x| (x - a).abs() < 1.5), "{} at {a:.1}° not in {angles:?}", t.name);
    }
}

/// RE1: the new shell is reachable through Window and automation without editing the project.
#[test]
fn reverse_engineering_shell_opens_closes_and_preserves_project() {
    use filmcraft_ui_egui::dock::{DockNode, PanelKind};
    let mut d = Driver::with(Session::default());
    let original = serde_json::to_value(&*d.app().session.project).unwrap();
    assert!(d.app().ui.dock.is_visible(PanelKind::ReverseEngineering));
    assert!(d.has("reverseEngineering.intro"));
    d.ok("ui.menu.invoke", json!({"id": "window.panel.ReverseEngineering"}));
    d.frames(2);
    assert!(d.app().ui.dock.is_visible(PanelKind::ReverseEngineering));
    assert!(d.has("reverseEngineering.intro"));
    let saved = serde_json::to_string(&d.app().ui.dock).unwrap();
    let restored: DockNode = serde_json::from_str(&saved).unwrap();
    assert!(restored.is_visible(PanelKind::ReverseEngineering));
    d.ok("ui.panel.close", json!({"panel": "ReverseEngineering"}));
    d.frames(2);
    assert!(!d.app().ui.dock.contains(PanelKind::ReverseEngineering));
    assert!(!d.has("reverseEngineering.intro"));
    d.ok("ui.panel.show", json!({"panel": "Reverse Engineering"}));
    d.frames(2);
    assert!(d.has("reverseEngineering.intro"));
    assert_eq!(serde_json::to_value(&*d.app().session.project).unwrap(), original);
    d.snapshot("panels-reverse-engineering", Some("reverseEngineering."));
}

#[test]
fn reverse_engineering_migrates_old_layout_once_and_keeps_close_choice() {
    use filmcraft_ui_egui::dock::{DockNode, PanelKind, SavedWorkspace, WorkspacePrefs};
    let old = DockNode::Tabs { panels: vec![PanelKind::Source, PanelKind::Metadata], active: 0 };
    let mut prefs = WorkspacePrefs::default();
    prefs.saved.push(SavedWorkspace { name: "Editing".into(), layout: serde_json::to_value(old).unwrap(), ..Default::default() });
    assert!(prefs.add_reverse_engineering());
    let mut layout: DockNode = serde_json::from_value(prefs.saved[0].layout.clone()).unwrap();
    assert!(layout.is_visible(PanelKind::ReverseEngineering));
    assert!(layout.contains(PanelKind::Metadata));
    layout.close(PanelKind::ReverseEngineering);
    prefs.saved[0].layout = serde_json::to_value(layout.clone()).unwrap();
    let mut restored: WorkspacePrefs = serde_json::from_str(&serde_json::to_string(&prefs).unwrap()).unwrap();
    assert!(!restored.add_reverse_engineering());
    assert_eq!(serde_json::from_value::<DockNode>(restored.saved[0].layout.clone()).unwrap(), layout);
}

#[test]
fn reverse_engineering_captures_timeline_and_keeps_frame_when_scrubbing() {
    let mut d = Driver::demo();
    let clip = d.app().session.active_sequence().unwrap().video_tracks.iter().flat_map(|track| track.items.iter()).next().unwrap().clone();
    d.app().session.state.selection = vec![clip.id];
    d.app().session.set_playhead(clip.start);
    d.ok("ui.panel.show", json!({"panel": "ReverseEngineering"}));
    d.frames(3);
    let sequence_id = d.app().session.state.active_sequence.unwrap();
    let sequence_name = d.app().session.project.item(sequence_id).unwrap().name.clone();
    assert_eq!(d.label("reverseEngineering.selection"), format!("Timeline: {sequence_name}"));
    let original = serde_json::to_value(&*d.app().session.project).unwrap();
    d.click("reverseEngineering.mode.frame");
    d.click("reverseEngineering.extractFrame");
    let captured = d.app().ui.panels.reverse_engineering.capture.clone().unwrap();
    assert_eq!(captured.item, sequence_id);
    assert_eq!(captured.media_time, clip.start);
    assert!(d.has("reverseEngineering.timecode"));
    for _ in 0..100 {
        if d.has("reverseEngineering.frame") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        d.frames(1);
    }
    assert!(d.has("reverseEngineering.frame"), "the captured frame must actually decode");
    let timeline_end = d.app().session.active_sequence().unwrap().duration();
    d.app().session.set_playhead(timeline_end);
    d.frames(3);
    assert_eq!(d.app().ui.panels.reverse_engineering.capture.as_ref(), Some(&captured));
    let reply = d.call("ui.menu.invoke", json!({"id":"reverseEngineering.extractFrame"}));
    assert_eq!(reply["ok"], false);
    assert_eq!(d.app().ui.panels.reverse_engineering.capture.as_ref(), Some(&captured));
    assert_eq!(serde_json::to_value(&*d.app().session.project).unwrap(), original);
    d.snapshot("panels-reverse-engineering-frame", Some("reverseEngineering."));
}

#[test]
fn reverse_engineering_range_samples_boundaries_previews_and_rejects_invalid_ranges() {
    let mut d = Driver::demo();
    let clip = d.app().session.active_sequence().unwrap().video_tracks.iter().flat_map(|track| track.items.iter()).next().unwrap().clone();
    let rate = d.app().session.active_sequence().unwrap().settings.frame_rate;
    let base = rate.frame_at(clip.start);
    d.app().session.state.selection = vec![clip.id];
    d.app().session.set_playhead(clip.start);
    d.ok("ui.panel.show", json!({"panel":"ReverseEngineering"}));
    d.click("reverseEngineering.mode.range");
    d.ok("ui.set", json!({"panels":{"reverseEngineering":{"startFrame":0,"endFrame":5,"sampleInterval":2}}}));
    let original = serde_json::to_value(&*d.app().session.project).unwrap();
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.extractRange"}));
    let samples = d.app().ui.panels.reverse_engineering.samples.clone();
    assert_eq!(samples.len(), 4);
    for (sample, frame) in samples.iter().zip([0, 2, 4, 5]) {
        assert_eq!(sample.timeline_time, rate.tick_of(base + frame));
        assert_eq!(sample.media_time, sample.timeline_time);
    }
    d.frames(3);
    assert!(!d.has("reverseEngineering.sample.0"), "range previews must not appear before analysis");
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.selectSample","params":{"index":3}}));
    assert_eq!(d.app().ui.panels.reverse_engineering.capture.as_ref(), samples.get(3));
    d.app().session.set_playhead(rate.tick_of(base + 1));
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.markStart"}));
    assert_eq!(d.app().ui.panels.reverse_engineering.start_frame, 1);
    d.app().session.set_playhead(rate.tick_of(base + 3));
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.markEnd"}));
    assert_eq!(d.app().ui.panels.reverse_engineering.end_frame, 3);
    assert!(d.app().ui.panels.reverse_engineering.samples.is_empty(), "accepting a new range clears the previous range's captures");
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.extractRange"}));
    let samples = d.app().ui.panels.reverse_engineering.samples.clone();
    assert_eq!(samples.len(), 2);
    let last = rate.frame_at(clip.end() - filmcraft_time::Tick(1)).saturating_sub(base);
    assert!(last >= 64, "demo clip must exercise the sample cap");
    for (start, end, interval) in [(4, 1, 1), (-1, 3, 1), (0, 3, 0), (0, i64::MAX, 1), (0, last, 1)] {
        let state = &mut d.app().ui.panels.reverse_engineering;
        state.start_frame = start;
        state.end_frame = end;
        state.sample_interval = interval;
        let reply = d.call("ui.menu.invoke", json!({"id":"reverseEngineering.extractRange"}));
        assert_eq!(reply["ok"], false);
        assert_eq!(d.app().ui.panels.reverse_engineering.samples, samples);
    }
    let reply = d.call("ui.menu.invoke", json!({"id":"reverseEngineering.selectSample","params":{"index":u64::MAX}}));
    assert_eq!(reply["ok"], false);
    assert_eq!(serde_json::to_value(&*d.app().session.project).unwrap(), original);
    d.ok("ui.set", json!({"panels":{"reverseEngineering":{"startFrame":0,"endFrame":5,"sampleInterval":2}}}));
    d.snapshot("panels-reverse-engineering-range", Some("reverseEngineering."));
}

#[test]
fn reverse_engineering_range_drag_and_exact_timestamps_stay_in_sync() {
    let mut d = Driver::demo();
    let clip = d.app().session.active_sequence().unwrap().video_tracks.iter().flat_map(|track| track.items.iter()).next().unwrap().clone();
    let rate = d.app().session.active_sequence().unwrap().settings.frame_rate;
    // Ranges are absolute timeline frames since R5: the drag gesture spans the whole sequence.
    let base = rate.frame_at(filmcraft_time::Tick::ZERO);
    let last = rate.frame_at(d.app().session.active_sequence().unwrap().duration() - filmcraft_time::Tick(1));
    d.app().session.state.selection = vec![clip.id];
    d.ok("ui.panel.show", json!({"panel":"ReverseEngineering"}));
    d.click("reverseEngineering.mode.range");
    let original_project = serde_json::to_value(&*d.app().session.project).unwrap();
    let elements = d.ok("ui.elements", json!({"prefix":"reverseEngineering.range.timeline"}));
    let rect = &elements[0]["rect"];
    let (x, y, w, h) = (rect[0].as_f64().unwrap(), rect[1].as_f64().unwrap(), rect[2].as_f64().unwrap(), rect[3].as_f64().unwrap());
    d.ok("ui.drag", json!({"from":{"x":x+w*0.25,"y":y+h*0.5},"to":{"x":x+w*0.75,"y":y+h*0.5},"steps":10}));
    d.frames(15);
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.confirmRangeChange","params":{"accept":true}}));
    let state = &d.app().ui.panels.reverse_engineering;
    assert!((state.start_frame - (last as f64 * 0.25).round() as i64).abs() <= 1);
    assert!((state.end_frame - (last as f64 * 0.75).round() as i64).abs() <= 1);
    assert_eq!(d.app().session.playhead(), rate.tick_of(base + d.app().ui.panels.reverse_engineering.end_frame));
    let timecode = |frame| filmcraft_time::format_time(rate.tick_of(base + frame), rate, false, filmcraft_time::TimeDisplay::Timecode, 48000);
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.setRange","params":{"startTimecode":timecode(2),"endTimecode":timecode(12)}}));
    assert_eq!(d.app().ui.panels.reverse_engineering.start_frame, 2);
    assert_eq!(d.app().ui.panels.reverse_engineering.end_frame, 12);
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.playRange"}));
    assert!(d.app().playback.playing);
    assert_eq!(d.app().session.playhead(), rate.tick_of(base + 2));
    assert_eq!(d.app().playback.stop_at, Some(rate.tick_of(base + 13)));
    assert!(!d.app().playback.looping);
    d.app().stop();
    assert!(!d.app().playback.playing);
    d.frames(2);
    assert_eq!(d.label("reverseEngineering.range.start"), timecode(2));
    assert_eq!(d.label("reverseEngineering.range.end"), timecode(12));
    d.click("reverseEngineering.range.start");
    d.ok("ui.key", json!({"key":"A","command":true}));
    d.ok("ui.type", json!({"text":timecode(4)}));
    d.ok("ui.key", json!({"key":"Enter"}));
    d.frames(3);
    assert_eq!(d.app().ui.panels.reverse_engineering.start_frame, 4);
    assert_eq!(d.app().session.playhead(), rate.tick_of(base + 4));
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.setRange","params":{"startTimecode":timecode(2)}}));
    for params in [json!({"startTimecode":"bad"}), json!({"startTimecode":timecode(13)}), json!({"endFrame":i64::MAX}), json!({"startTimecode":12})] {
        let reply = d.call("ui.menu.invoke", json!({"id":"reverseEngineering.setRange","params":params}));
        assert_eq!(reply["ok"], false);
        assert_eq!(d.app().ui.panels.reverse_engineering.start_frame, 2);
        assert_eq!(d.app().ui.panels.reverse_engineering.end_frame, 12);
    }
    assert_eq!(serde_json::to_value(&*d.app().session.project).unwrap(), original_project);
    let expected = rate.frame_at(d.app().session.playhead());
    for _ in 0..400 {
        if d.app().program_picture().is_some_and(|picture| picture.frame == expected) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
        d.frames(1);
    }
    assert_eq!(d.app().program_picture().map(|picture| picture.frame), Some(expected), "Program monitor must show the selected boundary");
    d.snapshot("panels-reverse-engineering-drag", Some("reverseEngineering."));
}

#[test]
#[ignore = "requires the installed external SEA-RAFT runtime; run explicitly"]
fn reverse_engineering_motion_analyzes_every_frame_exports_and_keeps_project_unchanged() {
    let mut d = Driver::demo();
    let clip = d.app().session.active_sequence().unwrap().video_tracks.iter().flat_map(|track| track.items.iter()).next().unwrap().clone();
    let rate = d.app().session.active_sequence().unwrap().settings.frame_rate;
    d.app().session.state.selection = vec![clip.id];
    d.ok("ui.panel.show", json!({"panel":"ReverseEngineering"}));
    d.click("reverseEngineering.mode.range");
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.setRange","params":{"startFrame":0,"endFrame":5}}));
    let before = serde_json::to_value(&*d.app().session.project).unwrap();
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.chooseTool","params":{"tool":"opticalFlow"}}));
    for _ in 0..6000 {
        if d.app().ui.panels.reverse_engineering.motion.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
        d.frames(1);
    }
    let report = d.app().ui.panels.reverse_engineering.motion.clone().expect("motion worker must return a report");
    assert_eq!(report.analyzer, "filmcraft-vision/sea-raft-M");
    assert_eq!(report.captures.len(), 6);
    assert_eq!(report.pairs.len(), 5);
    for (index, pair) in report.pairs.iter().enumerate() {
        assert_eq!(pair.from_timeline_tick, rate.tick_of(rate.frame_at(clip.start) + index as i64).0);
        assert_eq!(pair.to_timeline_tick, rate.tick_of(rate.frame_at(clip.start) + index as i64 + 1).0);
        assert!(pair.analyzed_size.iter().all(|n| *n > 0 && *n <= filmcraft_vision::sea_raft::MAX_SIZE));
        assert!(pair.vectors.iter().all(|v| v.from.iter().chain(v.to.iter()).all(|n| n.is_finite() && (0.0..=1.0).contains(n))));
    }
    d.frames(3); // Worker completion is polled separately from the panel layout pass.
    assert!(d.has("reverseEngineering.motionNext"));
    d.click("reverseEngineering.motionNext");
    assert_eq!(d.app().ui.panels.reverse_engineering.capture.as_ref(), report.captures.get(1));
    assert_eq!(d.app().session.playhead(), report.captures[1].timeline_time);
    let bad = d.call("ui.menu.invoke", json!({"id":"reverseEngineering.motionPair","params":{"index":u64::MAX}}));
    assert_eq!(bad["ok"], false);
    let display = json!({"enabled":true,"cutoffPixels":0.2,"arrowScale":4.0,"sampleStride":2});
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.setMotionDisplay","params":{"scope":"frame","index":1,"display":display}}));
    assert_eq!(d.app().ui.panels.reverse_engineering.motion.as_ref().unwrap().display.effective(1).cutoff_pixels, 0.2);
    assert_eq!(d.app().ui.panels.reverse_engineering.motion.as_ref().unwrap().display.effective(0).cutoff_pixels, 0.0);
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.setMotionDisplay","params":{"scope":"all","index":1,"display":display}}));
    assert!(d.app().ui.panels.reverse_engineering.motion.as_ref().unwrap().display.frame_overrides.is_empty());
    assert_eq!(d.app().ui.panels.reverse_engineering.motion.as_ref().unwrap().pairs, report.pairs);
    let report = d.app().ui.panels.reverse_engineering.motion.clone().unwrap();
    let path = std::env::temp_dir().join(format!("filmcraft-motion-test-{}.json", std::process::id()));
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.exportMotion","params":{"path":path}}));
    let exported: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(exported["pairs"].as_array().unwrap().len(), 5);
    assert_eq!(exported["settings"]["timelineFrameStep"], 1);
    assert_eq!(exported["ticksPerSecond"], 254_016_000_000i64);
    assert_eq!(exported["schemaVersion"], 5);
    assert_eq!(exported["display"]["allFrames"]["arrowScale"], 4.0);
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.setRange","params":{"startFrame":0,"endFrame":0}}));
    assert_eq!(d.call("ui.menu.invoke", json!({"id":"reverseEngineering.analyzeMotion"}))["ok"], false);
    assert_eq!(d.app().ui.panels.reverse_engineering.motion.as_ref(), Some(&report));
    assert_eq!(serde_json::to_value(&*d.app().session.project).unwrap(), before);
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.cancelMotion"}));
    d.frames(3);
    assert!(!d.has("reverseEngineering.frame"), "stale analysis must be hidden after a range edit");
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.setRange","params":{"startFrame":0,"endFrame":5}}));
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.motionFrame","params":{"frameIndex":5}}));
    assert_eq!(d.app().ui.panels.reverse_engineering.capture.as_ref(), report.captures.get(5));
    d.wait_for("reverseEngineering.frame");
    let elements = d.ok("ui.elements", json!({"prefix":"reverseEngineering.motionNext"}));
    let rect = &elements[0]["rect"];
    d.ok("ui.scroll", json!({"x":rect[0].as_f64().unwrap()+80.0,"y":rect[1].as_f64().unwrap()+20.0,"dx":0.0,"dy":-450.0}));
    d.frames(20);
    d.snapshot("panels-reverse-engineering-motion", Some("reverseEngineering."));
}

#[test]
fn reverse_engineering_range_crosses_clip_boundary_without_changing_project() {
    let mut d = Driver::demo();
    let sequence_id = d.app().session.state.active_sequence.unwrap();
    let rate = d.app().session.active_sequence().unwrap().settings.frame_rate;
    let mut first = d.app().session.active_sequence().unwrap().video_tracks[0].items[0].clone();
    first.start = filmcraft_time::Tick::ZERO;
    first.duration = rate.tick_of(3);
    let mut second = first.clone();
    second.id = filmcraft_project::ClipId(u64::MAX - 1);
    second.start = first.end();
    second.source_in = rate.tick_of(30);
    Arc::make_mut(&mut d.app().session.project).sequence_mut(sequence_id).unwrap().video_tracks[0].items = vec![first.clone(), second.clone()];
    d.app().session.state.selection = vec![first.id, second.id];
    d.ok("ui.panel.show", json!({"panel":"ReverseEngineering"}));
    d.frames(3);
    let original = serde_json::to_value(&*d.app().session.project).unwrap();
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.setRange","params":{"startFrame":2,"endFrame":4}}));
    d.app().ui.panels.reverse_engineering.sample_interval = 1;
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.extractRange"}));
    let captures = d.app().ui.panels.reverse_engineering.samples.clone();
    assert_eq!(captures.len(), 3);
    for (capture, frame) in captures.iter().zip(2..=4) {
        assert_eq!(capture.item, sequence_id);
        assert_eq!(capture.media_time, rate.tick_of(frame));
        assert_eq!(capture.timeline_time, rate.tick_of(frame));
    }
    assert_eq!(serde_json::to_value(&*d.app().session.project).unwrap(), original);
}

#[test]
fn reverse_engineering_new_range_waits_for_confirmation_and_cancel_preserves_progress() {
    let mut d = Driver::demo();
    let clip = d.app().session.active_sequence().unwrap().video_tracks[0].items[0].clone();
    d.app().session.state.selection = vec![clip.id];
    d.ok("ui.panel.show", json!({"panel":"ReverseEngineering"}));
    d.frames(3);
    let original = d.app().ui.panels.reverse_engineering.clone();
    let project = serde_json::to_value(&*d.app().session.project).unwrap();
    let response = d.call("ui.menu.invoke", json!({"id":"reverseEngineering.setRange","params":{"startFrame":1,"endFrame":5}}));
    assert_eq!(response["result"]["confirmationRequired"], true);
    assert_eq!(d.app().ui.panels.reverse_engineering, original);
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.confirmRangeChange","params":{"accept":false}}));
    assert_eq!(d.app().ui.panels.reverse_engineering, original);
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.setRange","params":{"startFrame":1,"endFrame":5}}));
    assert_eq!(d.app().ui.panels.reverse_engineering.start_frame, 1);
    assert_eq!(d.app().ui.panels.reverse_engineering.end_frame, 5);
    assert!(d.app().ui.panels.reverse_engineering.motion.is_none());
    assert!(d.app().ui.panels.reverse_engineering.tracking_queries.is_empty());
    assert_eq!(serde_json::to_value(&*d.app().session.project).unwrap(), project);
}

#[test]
fn reverse_engineering_shots_detect_structure_and_export_an_ai_brief() {
    let mut d = Driver::demo();
    let clip = d.app().session.active_sequence().unwrap().video_tracks[0].items[0].clone();
    d.app().session.state.selection = vec![clip.id];
    d.ok("ui.panel.show", json!({"panel":"ReverseEngineering"}));
    d.frames(3);
    let before = serde_json::to_value(&*d.app().session.project).unwrap();
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.setRange","params":{"startFrame":0,"endFrame":59}}));
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.chooseTool","params":{"tool":"shots"}}));
    let wait = |d: &mut Driver, palette: usize| {
        for _ in 0..12000 {
            if d.app().ui.panels.reverse_engineering.shots.as_ref().is_some_and(|r| r.palette_size == palette) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
            d.frames(1);
        }
        d.app().ui.panels.reverse_engineering.shots.clone().expect("shot worker must return a report")
    };
    let report = wait(&mut d, 8);
    assert_eq!(report.analyzer, "filmcraft-vision/shots-and-look-v1");
    assert_eq!(report.captures.len(), 60);
    assert_eq!(report.changes.len(), 60);
    assert_eq!(report.shots.len(), report.looks.len());
    assert!(!report.shots.is_empty());
    assert_eq!(report.shots[0].start_frame, 0);
    assert_eq!(report.shots.last().unwrap().end_frame, 59);
    for (shot, look) in report.shots.iter().zip(&report.looks) {
        assert_eq!(shot.black, look.is_none(), "black shots have no look; visible shots do: {shot:?}");
        if let Some(look) = look {
            assert!(!look.palette.is_empty() && look.palette.len() <= 8);
            look.validate().unwrap();
        }
    }
    let total: usize = report.shots.iter().map(|s| s.frame_count).sum();
    assert_eq!(total, 60, "shots partition the range");
    d.frames(3);
    assert!(d.has("reverseEngineering.shots.bar"));
    assert!(d.has("reverseEngineering.runShots"));
    let last = report.shots.len() - 1;
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.shotSelect","params":{"index":last}}));
    assert_eq!(d.app().ui.panels.reverse_engineering.shot_selected, last);
    assert_eq!(d.app().session.playhead(), report.captures[report.shots[last].start_frame].timeline_time);
    assert_eq!(d.app().ui.panels.reverse_engineering.capture.as_ref(), report.captures.get(report.shots[last].keyframe));
    assert_eq!(d.call("ui.menu.invoke", json!({"id":"reverseEngineering.shotSelect","params":{"index":u64::MAX}}))["ok"], false);
    assert_eq!(d.call("ui.menu.invoke", json!({"id":"reverseEngineering.setShotOptions","params":{"cutThreshold":f64::NAN}}))["ok"], false);
    assert_eq!(d.call("ui.menu.invoke", json!({"id":"reverseEngineering.setShotOptions","params":{"paletteSize":0}}))["ok"], false);
    let changed = d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.setShotOptions","params":{"paletteSize":4}}));
    assert_eq!(changed["redetecting"], true);
    let report = wait(&mut d, 4);
    assert!(report.looks.iter().flatten().all(|l| l.palette.len() <= 4));
    assert_eq!(report.changes.len(), 60);
    d.frames(3);
    let path = std::env::temp_dir().join(format!("filmcraft-brief-test-{}.md", std::process::id()));
    let exported = d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.exportBrief","params":{"path":path}}));
    assert_eq!(exported["shots"], json!(report.shots.len()));
    let markdown = std::fs::read_to_string(&path).unwrap();
    assert!(markdown.starts_with("# Breakdown: "), "{markdown}");
    assert!(markdown.contains("## Shot 1 ·"));
    assert!(markdown.contains("```prompt"));
    let json_path = path.with_extension("json");
    let data: Value = serde_json::from_slice(&std::fs::read(&json_path).unwrap()).unwrap();
    assert_eq!(data["format"], "filmcraft-ai-brief");
    assert_eq!(data["input"]["shots"].as_array().unwrap().len(), report.shots.len());
    std::fs::remove_file(&path).unwrap();
    std::fs::remove_file(&json_path).unwrap();
    let store = std::env::temp_dir().join(format!("filmcraft-shots-store-{}.json", std::process::id()));
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.saveToolResult","params":{"path":store}}));
    assert!(d.app().ui.panels.reverse_engineering.saved_results.iter().any(|r| serde_json::to_value(r).unwrap()["tool"] == "shots"));
    std::fs::remove_file(&store).unwrap();
    assert_eq!(serde_json::to_value(&*d.app().session.project).unwrap(), before);
    d.snapshot("panels-reverse-engineering-shots", Some("reverseEngineering."));
}

#[test]
fn reverse_engineering_camera2d_detects_feature_points_on_the_rendered_frame() {
    let mut d = Driver::demo();
    let clip = d.app().session.active_sequence().unwrap().video_tracks[0].items[0].clone();
    d.app().session.state.selection = vec![clip.id];
    d.ok("ui.panel.show", json!({"panel":"ReverseEngineering"}));
    d.frames(3);
    let before = serde_json::to_value(&*d.app().session.project).unwrap();
    // Frames 135–205 lie inside the demo's second shot (textured, no transition): the opening
    // dip-to-black and the smooth aurora shot have no corners and give no camera evidence.
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.setRange","params":{"startFrame":135,"endFrame":205}}));
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.chooseTool","params":{"tool":"camera2d"}}));
    d.frames(3);
    assert!(d.app().ui.panels.reverse_engineering.camera2d_report.is_none(), "choosing the tool shows it without starting an analysis");
    assert!(d.has("reverseEngineering.runFeatures") && d.has("reverseEngineering.camera2d.applyFile"));
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.runFeatures"}));
    let wait = |d: &mut Driver, max_points: usize| {
        for _ in 0..12000 {
            if d.app().ui.panels.reverse_engineering.camera2d_report.as_ref().is_some_and(|r| r.options.max_points == max_points) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
            d.frames(1);
        }
        d.app().ui.panels.reverse_engineering.camera2d_report.clone().expect("feature worker must return a report")
    };
    let report = wait(&mut d, 500);
    assert_eq!(report.analyzer, "filmcraft-vision/camera2d-trajectory-v1");
    assert_eq!(report.captures.len(), 71);
    assert_eq!(report.frames.len(), 71, "every frame of the range is analysed");
    assert_eq!(report.comparisons.len(), 70, "one comparison per consecutive pair");
    assert_eq!(report.trajectory.len(), 71, "one pose per frame");
    assert_eq!(report.trajectory[0].transform, [1.0, 0.0, 0.0, 0.0]);
    assert!(
        !report.anchors.is_empty()
            && report.anchors.iter().all(|a| [report.anchor_options.gap, report.anchor_options.long_gap].contains(&(a.to_frame - a.from_frame))
                && a.to_frame % report.anchor_options.step == 0)
    );
    assert!(report.anchors.iter().any(|a| a.to_frame - a.from_frame == report.anchor_options.long_gap), "long anchors bridge short gaps");
    let comparison = report
        .comparisons
        .iter()
        .find(|c| matches!(c.fit, filmcraft_vision::camera2d::Fit::Estimated { .. }))
        .unwrap_or_else(|| panic!("some lit comparison must carry camera evidence: {:?}", report.comparisons.iter().map(|c| c.state).collect::<Vec<_>>()));
    assert_eq!(comparison.to_frame, comparison.from_frame + 1);
    assert!(comparison.matches.len() >= 8, "{} matches between lit consecutive frames", comparison.matches.len());
    match &comparison.fit {
        filmcraft_vision::camera2d::Fit::Estimated { fit } => {
            assert!(fit.inliers.len() >= 8 && fit.inliers.len() <= comparison.matches.len());
            assert!(
                comparison.coverage_cells >= report.camera_options.min_coverage_cells && comparison.span.iter().all(|s| *s >= report.camera_options.min_span)
            );
            assert!(fit.center_displacement_px[0].abs() < 20.0 && fit.center_displacement_px[1].abs() < 20.0, "{:?}", fit.center_displacement_px);
            assert!((fit.scale - 1.0).abs() < 0.1 && fit.rotation_degrees.abs() < 5.0, "scale {} rotation {}", fit.scale, fit.rotation_degrees);
            assert!(fit.rms_error_px.is_finite());
            let (before, after) = (comparison.photometric_before.unwrap(), comparison.photometric_after.unwrap());
            assert!(before.is_finite() && after.is_finite() && before >= 0.0 && after >= 0.0);
            let expected = filmcraft_vision::camera2d::classify(
                &filmcraft_vision::camera2d::CameraFit {
                    camera: comparison.fit.clone(),
                    coverage_cells: comparison.coverage_cells,
                    span: comparison.span,
                    regions: comparison.regions.clone(),
                },
                comparison.photometric_before,
                comparison.photometric_after,
                report.camera_options.static_px,
            );
            assert_eq!(comparison.state, expected);
            assert_eq!(
                comparison.verified,
                matches!(comparison.state, filmcraft_vision::camera2d::CameraState::Static | filmcraft_vision::camera2d::CameraState::Moving)
            );
        }
        filmcraft_vision::camera2d::Fit::Insufficient { reason, .. } => panic!("consecutive lit frames should fit: {reason}"),
    }
    let still: Vec<&filmcraft_vision::trajectory::FramePose> = report
        .trajectory
        .iter()
        .filter(|p| report.comparisons.get(p.frame.wrapping_sub(1)).is_some_and(|c| c.state == filmcraft_vision::camera2d::CameraState::Static))
        .collect();
    for p in still {
        assert!(p.residual_px.is_finite() && p.scale.is_finite() && p.displacement_px.iter().all(|v| v.is_finite()));
    }
    assert_eq!(report.source_size, [1920, 1080]);
    assert_eq!(report.total_points, report.frames.iter().map(|f| f.features.len()).sum::<usize>());
    let lit = &report.frames[60];
    assert_eq!(lit.frame_index, 60);
    assert_eq!(lit.timeline_tick, report.captures[60].timeline_time.0);
    assert_eq!(lit.analyzed_size[0], 640, "{:?}", lit.analyzed_size);
    assert!(lit.mean_luma > 0.03, "the demo's second shot is lit, mean luma {}", lit.mean_luma);
    assert!(!lit.features.is_empty());
    for f in &lit.features {
        assert!(f.x >= 0.0 && f.y >= 0.0 && (f.x as usize) < lit.analyzed_size[0] && (f.y as usize) < lit.analyzed_size[1]);
        assert!((0.0..=1.0).contains(&f.response) && f.orientation.is_finite() && f.octave < 4);
    }
    assert_eq!(d.app().ui.panels.reverse_engineering.camera2d_frame, 35, "the viewer opens on the middle frame of the range");
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.featureFrame","params":{"frameIndex":60}}));
    assert_eq!(d.app().session.playhead(), report.captures[60].timeline_time);
    assert_eq!(d.app().ui.panels.reverse_engineering.camera2d_report.as_ref(), Some(&report), "navigation does not rerun detection");
    d.wait_for("reverseEngineering.frame");
    assert!(d.has("reverseEngineering.runFeatures"));
    assert!(d.has("reverseEngineering.camera2d.graph"));
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.setFeatureView","params":{"view":"matches"}}));
    d.frames(3);
    assert!(d.has("reverseEngineering.camera2d.view.matches"));
    assert_eq!(d.call("ui.menu.invoke", json!({"id":"reverseEngineering.setFeatureView","params":{"view":"nope"}}))["ok"], false);
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.setFeatureView","params":{"view":"points"}}));
    assert_eq!(d.call("ui.menu.invoke", json!({"id":"reverseEngineering.featureFrame","params":{"frameIndex":u64::MAX}}))["ok"], false);
    assert_eq!(d.call("ui.menu.invoke", json!({"id":"reverseEngineering.setFeatureOptions","params":{"octaves":0}}))["ok"], false);
    assert_eq!(d.call("ui.menu.invoke", json!({"id":"reverseEngineering.setFeatureOptions","params":{"ratio":2.0}}))["ok"], false);
    assert_eq!(d.call("ui.menu.invoke", json!({"id":"reverseEngineering.setFeatureOptions","params":{"tolerancePx":0.0}}))["ok"], false);
    assert_eq!(d.call("ui.menu.invoke", json!({"id":"reverseEngineering.setFeatureOptions","params":{"threshold":f64::NAN}}))["ok"], false);
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.setFeatureOptions","params":{"maxPoints":20}}));
    d.frames(3);
    assert!(!d.has("reverseEngineering.camera2d.graph"), "changed options hide the stale result until detection reruns");
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.runFeatures"}));
    let limited = wait(&mut d, 20);
    assert_eq!(limited.frames.len(), 71);
    assert!(limited.frames.iter().all(|f| f.features.len() <= 20));
    assert!(limited.frames[60].features.len() == 20);
    d.frames(3);
    let path = std::env::temp_dir().join(format!("filmcraft-camera2d-test-{}.json", std::process::id()));
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.exportTool","params":{"path":path}}));
    let exported: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(exported["tool"], "camera2d");
    assert_eq!(exported["result"]["frames"].as_array().unwrap().len(), 71);
    assert_eq!(exported["result"]["frames"][60]["features"].as_array().unwrap().len(), 20);
    assert_eq!(exported["result"]["totalPoints"], json!(limited.total_points));
    assert_eq!(exported["result"]["comparisons"].as_array().unwrap().len(), 70);
    assert!(exported["result"]["comparisons"][60]["fit"]["status"].is_string());
    assert!(exported["result"]["comparisons"][60]["state"].is_string());
    assert_eq!(exported["result"]["trajectory"].as_array().unwrap().len(), 71);
    assert!(exported["result"]["anchors"].as_array().is_some_and(|a| !a.is_empty()));
    assert!(d.has("reverseEngineering.camera2d.path"));
    assert_eq!(serde_json::to_value(&*d.app().session.project).unwrap(), before);
    d.snapshot("panels-reverse-engineering-camera2d", Some("reverseEngineering."));
}

#[test]
fn reverse_engineering_camera2d_exports_a_camera_file_and_applies_it_as_motion_keyframes() {
    use filmcraft_time::Tick;
    let mut d = Driver::demo();
    // A sequence holding one textured still: a rigid, static picture, which the 2D camera model
    // assumes. The demo's scenes are not rigid (parallax building layers in the city shot,
    // twinkling stars and drifting curtains in the aurora), so their "camera" is undefined.
    let dir = std::env::temp_dir().join(format!("filmcraft-camera2d-e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let still_path = dir.join("texture.tga");
    {
        let (w, h) = (1920usize, 1080usize);
        let mut seed = 0x9E37_79B9u32;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        let blobs: Vec<(usize, usize, usize, usize, [u8; 3])> = (0..900)
            .map(|_| {
                let x = (next() % w as u32) as usize;
                let y = (next() % h as u32) as usize;
                let bw = 6 + (next() % 36) as usize;
                let bh = 6 + (next() % 36) as usize;
                let c = [40 + (next() % 200) as u8, 40 + (next() % 200) as u8, 40 + (next() % 200) as u8];
                (x, y, bw, bh, c)
            })
            .collect();
        let mut tga = vec![0u8, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, (w & 255) as u8, (w >> 8) as u8, (h & 255) as u8, (h >> 8) as u8, 24, 0x20];
        for y in 0..h {
            for x in 0..w {
                let base = 90.0 + 25.0 * ((x as f32 * 0.021).sin() + (y as f32 * 0.017).cos());
                let mut px = [base as u8, (base * 0.9) as u8, (base * 1.1) as u8];
                for (bx, by, bw, bh, c) in &blobs {
                    if x >= *bx && x < bx + bw && y >= *by && y < by + bh {
                        px = *c;
                    }
                }
                tga.extend_from_slice(&[px[2], px[1], px[0]]);
            }
        }
        std::fs::write(&still_path, tga).unwrap();
    }
    let imported = d.exec("file.import", json!({"paths":[still_path.to_string_lossy()]}));
    let still = imported["items"][0].as_u64().expect("imported still");
    d.exec("file.newSequenceFromClip", json!({"items":[still]}));
    d.frames(2);
    let sequence = d.app().session.active_sequence().unwrap().clone();
    let rate = sequence.settings.frame_rate;
    assert_eq!((sequence.settings.width, sequence.settings.height), (1920, 1080));
    let (t_start, t_end) = (rate.tick_of(10), rate.tick_of(80));
    let clip = sequence
        .video_tracks
        .iter()
        .flat_map(|t| t.items.iter())
        .find(|i| i.start <= t_start && i.end() > t_end)
        .unwrap_or_else(|| {
            panic!(
                "the still must cover frames 10–80: {:?}",
                sequence.video_tracks.iter().flat_map(|t| t.items.iter().map(|i| (i.start, i.end()))).collect::<Vec<_>>()
            )
        })
        .clone();
    d.app().session.state.selection = vec![clip.id];
    d.ok("ui.panel.show", json!({"panel":"ReverseEngineering"}));
    d.frames(3);
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.setRange","params":{"startFrame":10,"endFrame":80}}));
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.chooseTool","params":{"tool":"camera2d"}}));
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.runFeatures"}));
    let wait = |d: &mut Driver, revision: u64| {
        for _ in 0..12000 {
            if d.app().ui.panels.reverse_engineering.camera2d_report.as_ref().is_some_and(|r| r.project_revision == revision) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
            d.frames(1);
        }
        d.app().ui.panels.reverse_engineering.camera2d_report.clone().expect("feature worker must return a report")
    };
    let revision = d.app().session.revision;
    let original = wait(&mut d, revision);
    assert_eq!(original.trajectory.len(), 71);
    assert_eq!(d.call("ui.menu.invoke", json!({"id":"reverseEngineering.applyCamera","params":{"mode":"sideways"}}))["ok"], false);
    assert_eq!(d.call("ui.menu.invoke", json!({"id":"reverseEngineering.applyCamera","params":{"path":"/nonexistent/camera.json"}}))["ok"], false);
    let exported_path = dir.join("camera.camera2d.json");
    let r = d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.exportCamera","params":{"path":exported_path}}));
    assert_eq!(r["frames"], json!(71));
    let exported: Value = serde_json::from_slice(&std::fs::read(&exported_path).unwrap()).unwrap();
    assert_eq!(exported["format"], "filmcraft.camera2d");
    assert_eq!(exported["version"], 1);
    assert_eq!(exported["source"]["width"], 1920);
    assert_eq!(exported["source"]["startFrame"], 10);
    assert_eq!(exported["source"]["endFrame"], 80);
    let path = exported["path"].as_array().unwrap();
    assert_eq!(path.len(), 71);
    assert_eq!(path[0]["tick"], 0);
    assert_eq!(path[0]["transform"], json!([1.0, 0.0, 0.0, 0.0]));
    assert!(path[0]["state"].is_null() && path[1]["state"].is_string());
    assert_eq!(path[10]["tick"].as_i64().unwrap(), (rate.tick_of(20) - rate.tick_of(10)).0);
    let motion = exported["motion"].as_array().unwrap();
    assert_eq!(motion.len(), 71);
    assert_eq!(motion[0]["position"], json!([960.0, 540.0]));
    assert_eq!(motion[0]["scalePercent"], 100.0);
    assert_eq!(motion[0]["rotationDegrees"], 0.0);
    // a synthetic pan and zoom about the frame centre, in sequence pixels
    let k = 640.0 / 1920.0;
    let synthetic: Vec<[f64; 4]> = (0..71)
        .map(|i| {
            let s = 1.0 + 0.002 * i as f64;
            [s, 0.0, 1.5 * i as f64 - (s - 1.0) * 960.0, -(i as f64) - (s - 1.0) * 540.0]
        })
        .collect();
    let mut file = exported.clone();
    for (i, frame) in file["path"].as_array_mut().unwrap().iter_mut().enumerate() {
        frame["transform"] = json!(synthetic[i]);
    }
    let synthetic_path = dir.join("synthetic.camera2d.json");
    std::fs::write(&synthetic_path, serde_json::to_vec(&file).unwrap()).unwrap();
    let before_apply = d.app().session.history.undo.len();
    let r = d.ok(
        "ui.menu.invoke",
        json!({"id":"reverseEngineering.applyCamera","params":{"path":synthetic_path,"mode":"follow","clips":[clip.id.0],"start":t_start.0}}),
    );
    assert_eq!(r["frames"], json!(71));
    assert_eq!(r["clips"][0]["keyframes"], json!(71));
    assert_eq!(d.app().session.history.undo.len(), before_apply + 1, "one undo step per clip");
    // the application is recorded as a component: the RE lane shows it, the panel lists it
    let components = d.app().session.active_sequence().unwrap().components.clone();
    assert_eq!(components.len(), 1);
    let component = &components[0];
    assert_eq!(component.clip, clip.id);
    assert_eq!(component.start, t_start);
    assert_eq!(component.samples.len(), 71);
    assert_eq!(component.params, vec!["position".to_string(), "rotation".to_string(), "scale".to_string()]);
    assert_eq!(component.source.file.as_deref(), Some(synthetic_path.to_string_lossy().as_ref()));
    assert_eq!(r["clips"][0]["component"], json!(component.id.0));
    assert_eq!(d.app().ui.panels.reverse_engineering.focused_component, Some(component.id.0));
    d.frames(3);
    assert!(d.has("timeline.components.header"), "the RE lane appears once a component exists");
    assert!(d.has(&format!("timeline.components.{}", component.id.0)), "the component bar is on the lane");
    assert!(d.has(&format!("reverseEngineering.camera2d.component.{}", component.id.0)), "the panel lists the component");
    let listed = d.exec("components.list", json!({}));
    assert_eq!(listed["components"][0]["frames"], json!(71));
    assert!(listed["components"][0]["changes"].as_array().is_some_and(Vec::is_empty), "the synthetic file kept the still's static states: nothing changes");
    assert!((listed["components"][0]["scaleRange"][1].as_f64().unwrap() - 1.14).abs() < 1e-6);
    d.ok("ui.set", json!({"timeline": {"componentsLane": false}}));
    d.frames(3);
    assert!(!d.has("timeline.components.header"), "the lane can be hidden from the wrench menu");
    d.ok("ui.set", json!({"timeline": {"componentsLane": true}}));
    d.frames(3);
    assert!(d.has("timeline.components.header"));
    let component_file = dir.join("component.camera2d.json");
    let r2 = d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.exportComponent","params":{"id":component.id.0,"path":component_file}}));
    assert_eq!(r2["frames"], json!(71));
    let rebuilt: Value = serde_json::from_slice(&std::fs::read(&component_file).unwrap()).unwrap();
    for i in [1usize, 35, 70] {
        let a = rebuilt["path"][i]["transform"].as_array().unwrap();
        for c in 0..4 {
            assert!((a[c].as_f64().unwrap() - synthetic[i][c]).abs() < 1e-6, "frame {i}: the component rebuilds the camera file: {a:?} vs {:?}", synthetic[i]);
        }
    }
    let item = d.app().session.active_sequence().unwrap().find_item(clip.id).unwrap().1.clone();
    let motion_effect = item.effect("motion").unwrap();
    for pid in ["position", "scale", "rotation"] {
        assert_eq!(motion_effect.param(pid).unwrap().keyframes.len(), 71, "{pid}");
    }
    let scale_at = |t: Tick| motion_effect.param("scale").unwrap().f64_at(item.source_time_at(t));
    assert!((scale_at(rate.tick_of(60)) / scale_at(t_start) - 1.1).abs() < 1e-9, "zoom 10 % after 50 frames");
    let src = d.app().session.project.source_size(item.item).unwrap();
    let par = d.app().session.project.source_par(item.item);
    let sequence_now = d.app().session.active_sequence().unwrap().clone();
    let motion_at = |t: Tick| filmcraft_render::motion_matrix(&sequence_now, &item, src, par, item.effect_time_at(t));
    let m0 = motion_at(t_start);
    for i in [1usize, 20, 50, 70] {
        let m = motion_at(rate.tick_of(10 + i as i64));
        let c = synthetic[i];
        let expected = [c[0] * m0.a - c[1] * m0.b, c[0] * m0.b + c[1] * m0.a, c[0] * m0.e - c[1] * m0.f + c[2], c[1] * m0.e + c[0] * m0.f + c[3]];
        assert!(
            (m.a - expected[0]).abs() < 1e-6 && (m.b - expected[1]).abs() < 1e-6 && (m.e - expected[2]).abs() < 1e-3 && (m.f - expected[3]).abs() < 1e-3,
            "frame {i}: rendered Motion {m:?} vs camera ∘ base {expected:?} (base {m0:?})"
        );
    }
    assert_eq!(motion_effect.param("uniform_scale").unwrap().value, filmcraft_project::ParamValue::Bool(true));
    // re-analysing the same range must find the synthetic camera on top of the original path
    d.frames(3);
    assert!(!d.has("reverseEngineering.camera2d.graph"), "the applied Motion changed the sequence: the result is stale");
    let revision = d.app().session.revision;
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.runFeatures"}));
    let after = wait(&mut d, revision);
    assert_eq!(after.trajectory.len(), 71);
    let compose =
        |o: [f64; 4], i: [f64; 4]| [o[0] * i[0] - o[1] * i[1], o[0] * i[1] + o[1] * i[0], o[0] * i[2] - o[1] * i[3] + o[2], o[1] * i[2] + o[0] * i[3] + o[3]];
    let mut worst = 0.0f64;
    for (i, pose) in after.trajectory.iter().enumerate() {
        let c = synthetic[i];
        let camera = [c[0], c[1], c[2] * k, c[3] * k];
        let expected = compose(camera, original.trajectory[i].transform);
        let centre = [320.0, 180.0];
        let got = [
            pose.transform[0] * centre[0] - pose.transform[1] * centre[1] + pose.transform[2],
            pose.transform[1] * centre[0] + pose.transform[0] * centre[1] + pose.transform[3],
        ];
        let want = [expected[0] * centre[0] - expected[1] * centre[1] + expected[2], expected[1] * centre[0] + expected[0] * centre[1] + expected[3]];
        worst = worst.max((got[0] - want[0]).hypot(got[1] - want[1]));
        if std::env::var_os("FILMCRAFT_TRAJECTORY_DEBUG").is_some() {
            let state = i
                .checked_sub(1)
                .and_then(|p| after.comparisons.get(p))
                .map(|c| format!("{:?} inl {} cells {} regions {}", c.state, c.matches.len(), c.coverage_cells, c.regions.len()));
            let before = original.trajectory[i].clone();
            eprintln!(
                "frame {i}: got d({:.2},{:.2}) s {:.4} r {:.3} res {:.2} | want d({:.2},{:.2}) s {:.4} | original d({:.2},{:.2}) s {:.4} | {}",
                got[0] - 320.0,
                got[1] - 180.0,
                pose.scale,
                pose.rotation_degrees,
                pose.residual_px,
                want[0] - 320.0,
                want[1] - 180.0,
                expected[0].hypot(expected[1]),
                before.displacement_px[0],
                before.displacement_px[1],
                before.scale,
                state.unwrap_or_default()
            );
        }
        assert!((pose.scale - expected[0].hypot(expected[1])).abs() < 0.02, "frame {i}: scale {} vs {}", pose.scale, expected[0].hypot(expected[1]));
    }
    assert!(worst < 1.5, "the applied camera is recovered within 1.5 analysis px, worst {worst}");
    // stabilising with the measured path makes the composite stand still
    let before_stabilize = d.app().session.history.undo.len();
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.applyCamera","params":{"mode":"stabilize","atTimeline":true}}));
    assert_eq!(d.app().session.history.undo.len(), before_stabilize + 1);
    let revision = d.app().session.revision;
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.runFeatures"}));
    let stabilized = wait(&mut d, revision);
    let worst = stabilized.trajectory.iter().map(|p| p.displacement_px[0].hypot(p.displacement_px[1])).fold(0.0f64, f64::max);
    let worst_scale = stabilized.trajectory.iter().map(|p| (p.scale - 1.0).abs()).fold(0.0f64, f64::max);
    assert!(worst < 1.5 && worst_scale < 0.02, "stabilising cancels the measured camera, worst {worst} px, scale {worst_scale}");
    assert_eq!(d.app().session.active_sequence().unwrap().components.len(), 2, "stabilising adds a second component");
    let stabilizer = d.app().session.active_sequence().unwrap().components[1].clone();
    assert_eq!(stabilizer.mode, filmcraft_project::ComponentMode::Stabilize);
    assert!(stabilizer.samples.iter().filter(|s| s.state.as_deref() == Some("moving")).count() > 60, "the measured path is moving almost everywhere");
    let listed = d.exec("components.list", json!({}));
    assert!(listed["components"][1]["changes"].as_array().is_some_and(|c| c.iter().any(|m| m["kind"] == json!("motionStarts"))), "{listed}");
    d.exec("components.remove", json!({"id": stabilizer.id.0}));
    assert_eq!(d.app().session.active_sequence().unwrap().components.len(), 1, "removing a component drops its record");
    d.exec("edit.undo", json!({}));
    assert_eq!(d.app().session.active_sequence().unwrap().components.len(), 2);
    d.exec("edit.undo", json!({}));
    d.exec("edit.undo", json!({}));
    let item = d.app().session.active_sequence().unwrap().find_item(clip.id).unwrap().1.clone();
    assert!(item.effect("motion").is_none_or(|m| m.param("scale").is_none_or(|p| p.keyframes.is_empty())), "two undos remove both applications");
    assert!(d.app().session.active_sequence().unwrap().components.is_empty());
    d.frames(3);
    assert!(!d.has("timeline.components.header"), "no components, no lane");
    std::fs::remove_dir_all(&dir).unwrap();
}

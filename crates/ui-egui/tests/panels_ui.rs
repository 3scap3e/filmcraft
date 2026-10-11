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
        v["result"].clone()
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
fn reverse_engineering_captures_selected_clip_and_keeps_frame_when_scrubbing() {
    let mut d = Driver::demo();
    let clip = d.app().session.active_sequence().unwrap().video_tracks.iter().flat_map(|track| track.items.iter()).next().unwrap().clone();
    d.app().session.state.selection = vec![clip.id];
    d.app().session.set_playhead(clip.start);
    d.ok("ui.panel.show", json!({"panel": "ReverseEngineering"}));
    d.frames(3);
    assert_eq!(d.label("reverseEngineering.selection"), format!("Video: {}", clip.name));
    let original = serde_json::to_value(&*d.app().session.project).unwrap();
    d.click("reverseEngineering.extractFrame");
    let captured = d.app().ui.panels.reverse_engineering.capture.clone().unwrap();
    assert_eq!(captured.item, clip.item);
    assert_eq!(captured.media_time, clip.source_time_at(clip.start));
    assert!(d.has("reverseEngineering.timecode"));
    for _ in 0..100 {
        if d.has("reverseEngineering.frame") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        d.frames(1);
    }
    assert!(d.has("reverseEngineering.frame"), "the captured frame must actually decode");
    d.app().session.set_playhead(clip.end());
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
    d.click("reverseEngineering.extractRange");
    let samples = d.app().ui.panels.reverse_engineering.samples.clone();
    assert_eq!(samples.len(), 4);
    for (sample, frame) in samples.iter().zip([0, 2, 4, 5]) {
        assert_eq!(sample.timeline_time, rate.tick_of(base + frame));
        assert_eq!(sample.media_time, clip.source_time_at(sample.timeline_time));
    }
    for _ in 0..100 {
        if d.has("reverseEngineering.sample.0") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        d.frames(1);
    }
    assert!(d.has("reverseEngineering.sample.0"), "filmstrip must show decoded frames");
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.selectSample","params":{"index":3}}));
    assert_eq!(d.app().ui.panels.reverse_engineering.capture.as_ref(), samples.get(3));
    d.app().session.set_playhead(rate.tick_of(base + 1));
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.markStart"}));
    assert_eq!(d.app().ui.panels.reverse_engineering.start_frame, 1);
    d.app().session.set_playhead(rate.tick_of(base + 3));
    d.ok("ui.menu.invoke", json!({"id":"reverseEngineering.markEnd"}));
    assert_eq!(d.app().ui.panels.reverse_engineering.end_frame, 3);
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
    let base = rate.frame_at(clip.start);
    let last = rate.frame_at(clip.end() - filmcraft_time::Tick(1)).saturating_sub(base);
    d.app().session.state.selection = vec![clip.id];
    d.ok("ui.panel.show", json!({"panel":"ReverseEngineering"}));
    d.click("reverseEngineering.mode.range");
    let original_project = serde_json::to_value(&*d.app().session.project).unwrap();
    let elements = d.ok("ui.elements", json!({"prefix":"reverseEngineering.range.timeline"}));
    let rect = &elements[0]["rect"];
    let (x, y, w, h) = (rect[0].as_f64().unwrap(), rect[1].as_f64().unwrap(), rect[2].as_f64().unwrap(), rect[3].as_f64().unwrap());
    d.ok("ui.drag", json!({"from":{"x":x+w*0.25,"y":y+h*0.5},"to":{"x":x+w*0.75,"y":y+h*0.5},"steps":10}));
    d.frames(15);
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
    d.click("reverseEngineering.stop");
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

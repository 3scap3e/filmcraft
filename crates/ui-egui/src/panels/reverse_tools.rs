//! Tool catalog, saved results, combined export, and global image-motion orchestration.
use super::{panel_state::ReverseEngineeringCapture, reverse_motion::MotionReport};
use crate::FilmcraftApp;
use filmcraft_vision::global_motion::{GlobalMotion, estimate};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalReport {
    pub schema_version: u32,
    pub analyzer: String,
    pub completed_at_unix_ms: u64,
    pub source_motion_revision: u64,
    pub source_completed_at_unix_ms: u64,
    pub residual_tolerance_pixels: f64,
    pub captures: Vec<ReverseEngineeringCapture>,
    pub pairs: Vec<GlobalMotion>,
    pub limitations: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "tool", content = "result", rename_all = "camelCase")]
pub enum SavedResult {
    OpticalFlow(MotionReport),
    GlobalMotion(GlobalReport),
    LocalMotion(super::reverse_local::LocalReport),
    MotionRegions(super::reverse_regions::RegionReport),
    PointTracking(super::reverse_tracking::TrackingReport),
    MotionOverTime(super::reverse_time::TimeReport),
    Discontinuities(super::reverse_discontinuities::Report),
    ObjectMasks(super::reverse_sam::Report),
    Shots(super::reverse_shots::ShotReport),
    CameraMoves(super::reverse_camera::CameraReport),
    #[serde(rename = "camera2d")]
    Features(super::reverse_camera2d::FeatureReport),
}
impl SavedResult {
    fn tool(&self) -> &str {
        match self {
            Self::OpticalFlow(_) => "opticalFlow",
            Self::GlobalMotion(_) => "globalMotion",
            Self::LocalMotion(_) => "localMotion",
            Self::MotionRegions(_) => "motionRegions",
            Self::PointTracking(_) => "trajectories",
            Self::MotionOverTime(_) => "motionOverTime",
            Self::Discontinuities(_) => "discontinuities",
            Self::ObjectMasks(_) => "objectMasks",
            Self::Shots(_) => "shots",
            Self::CameraMoves(_) => "cameraMoves",
            Self::Features(_) => "camera2d",
        }
    }
    fn captures(&self) -> &[ReverseEngineeringCapture] {
        match self {
            Self::OpticalFlow(r) => &r.captures,
            Self::GlobalMotion(r) => &r.captures,
            Self::LocalMotion(r) => &r.captures,
            Self::MotionRegions(r) => &r.captures,
            Self::PointTracking(r) => &r.captures,
            Self::MotionOverTime(r) => &r.captures,
            Self::Discontinuities(r) => &r.captures,
            Self::ObjectMasks(r) => &r.captures,
            Self::Shots(r) => &r.captures,
            Self::CameraMoves(r) => &r.captures,
            Self::Features(r) => &r.captures,
        }
    }
}
const COMPONENTS: [(&str, &str, bool); 12] = [
    ("camera2d", "2D camera", true),
    ("camera3d", "3D camera", false),
    ("shots", "Shots & look", true),
    ("look", "Look", false),
    ("dialogue", "Dialogue", false),
    ("music", "Music", false),
    ("soundEffects", "Sound effects", false),
    ("script", "Script", false),
    ("figureMovements", "Figure movements", false),
    ("objectMovements", "Object movements", false),
    ("textTitles", "Text & titles", false),
    ("effectsTransitions", "Effects & transitions", false),
];
const EVIDENCE: [(&str, &str, bool); 8] = [
    ("opticalFlow", "SEA-RAFT flow", true),
    ("globalMotion", "Global image motion", true),
    ("localMotion", "Local motion", true),
    ("motionRegions", "Motion regions", true),
    ("motionOverTime", "Motion over time", true),
    ("trajectories", "Point tracking", true),
    ("discontinuities", "Motion discontinuities", true),
    ("objectMasks", "Object masks · SAM 2.1", true),
];
fn tool_button(app: &mut FilmcraftApp, ui: &mut egui::Ui, id: &str, title: &str, implemented: bool, has_video: bool, label_width: f32) {
    let state = &app.ui.panels.reverse_engineering;
    let saved = state.saved_results.iter().any(|r| r.tool() == id && super::reverse_engineering::captures_are_current(app, r.captures()));
    let used = implemented && (saved || tool_ready(app, id));
    let stroke = if used { egui::Stroke::new(2.0, app.tokens.accent) } else { egui::Stroke::NONE };
    let enabled = implemented && has_video && app.ui.panels.reverse_engineering.range_mode;
    let response = ui.add_enabled(
        enabled,
        egui::Button::new(ui.painter().layout(title.into(), egui::TextStyle::Button.resolve(ui.style()), ui.visuals().text_color(), label_width))
            .wrap_mode(egui::TextWrapMode::Extend)
            .stroke(stroke),
    );
    app.auto.add(&format!("reverseEngineering.tool.{id}"), response.rect, title);
    if response.clicked() {
        invoke(app, ui.ctx(), "reverseEngineering.chooseTool", json!({"tool":id}));
    }
    if !implemented {
        response.on_disabled_hover_text("Coming soon");
    } else if !has_video {
        response.on_disabled_hover_text("Select a video clip in the timeline first");
    }
}
const TOOL_IDS: [&str; 11] = [
    "opticalFlow",
    "globalMotion",
    "localMotion",
    "motionRegions",
    "motionOverTime",
    "discontinuities",
    "objectMasks",
    "trajectories",
    "shots",
    "cameraMoves",
    "camera2d",
];
fn tool_ready(app: &FilmcraftApp, tool: &str) -> bool {
    match tool {
        "opticalFlow" => super::reverse_engineering::motion_is_current(app),
        "globalMotion" => current_global(app),
        "localMotion" => super::reverse_local::current(app),
        "motionRegions" => super::reverse_regions::current(app),
        "motionOverTime" => super::reverse_time::current(app),
        "discontinuities" => super::reverse_discontinuities::current(app),
        "objectMasks" => super::reverse_sam::current(app),
        "trajectories" => super::reverse_tracking::current(app),
        "shots" => super::reverse_shots::current(app),
        "cameraMoves" => super::reverse_camera::current(app),
        "camera2d" => super::reverse_camera2d::current(app),
        _ => false,
    }
}
#[derive(Clone)]
struct Job {
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicUsize>,
    total: usize,
    result: Arc<Mutex<Option<Result<GlobalReport, String>>>>,
}
fn job_id() -> egui::Id {
    egui::Id::new("reverse-global-motion-job")
}
pub(super) fn is_running(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<Job>(job_id())).is_some()
}
fn source_time(flow: &MotionReport) -> u64 {
    flow.audit.completed_runs.last().map(|r| r.completed_at_unix_ms).unwrap_or(0)
}
pub(super) fn current_global(app: &FilmcraftApp) -> bool {
    if !super::reverse_engineering::motion_is_current(app) {
        return false;
    }
    let state = &app.ui.panels.reverse_engineering;
    match (&state.motion, &state.global_motion) {
        (Some(flow), Some(global)) => {
            global.captures == flow.captures
                && global.source_motion_revision == flow.audit.revision
                && global.source_completed_at_unix_ms == source_time(flow)
                && global.residual_tolerance_pixels == state.global_tolerance
        }
        _ => false,
    }
}
pub(super) fn results_visible(app: &FilmcraftApp, ctx: &egui::Context) -> bool {
    if app.ui.panels.reverse_engineering.active_tool == "objectMasks" {
        return app.ui.panels.reverse_engineering.range_mode && !app.ui.panels.reverse_engineering.sam_captures.is_empty();
    }
    if app.ui.panels.reverse_engineering.active_tool == "trajectories" {
        return app.ui.panels.reverse_engineering.range_mode && !app.ui.panels.reverse_engineering.tracking_captures.is_empty();
    }
    if app.ui.panels.reverse_engineering.active_tool == "shots" {
        return app.ui.panels.reverse_engineering.range_mode && !super::reverse_shots::is_running(ctx) && super::reverse_shots::current(app);
    }
    if app.ui.panels.reverse_engineering.active_tool == "camera2d" {
        return app.ui.panels.reverse_engineering.range_mode && !super::reverse_camera2d::is_running(ctx) && super::reverse_camera2d::current(app);
    }
    if !app.ui.panels.reverse_engineering.range_mode || is_running(ctx) || super::reverse_motion::is_running(ctx) {
        return false;
    }
    tool_ready(app, &app.ui.panels.reverse_engineering.active_tool)
}
pub(super) fn poll(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let Some(job) = ctx.data(|d| d.get_temp::<Job>(job_id())) else {
        return;
    };
    let result = job.result.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
    if let Some(result) = result {
        ctx.data_mut(|d| d.remove::<Job>(job_id()));
        if job.cancel.load(Ordering::Relaxed) {
            app.ui.status = "Global motion cancelled; previous results preserved".into();
            return;
        }
        match result {
            Ok(report) => {
                app.ui.panels.reverse_engineering.global_motion = Some(report);
                app.ui.status = "Global image-motion fitting complete. Review and save the result.".into();
                if app.ui.panels.reverse_engineering.active_tool == "cameraMoves"
                    && current_global(app)
                    && let Err(e) = super::reverse_camera::build(app)
                {
                    app.ui.status = e.clone();
                    ctx.data_mut(|d| d.insert_temp(egui::Id::new("reverse-tools-error"), e));
                }
                if matches!(app.ui.panels.reverse_engineering.active_tool.as_str(), "localMotion" | "motionRegions") && current_global(app) {
                    let prepared = super::reverse_local::build(app).and_then(|value| {
                        if app.ui.panels.reverse_engineering.active_tool == "motionRegions" { super::reverse_regions::build(app) } else { Ok(value) }
                    });
                    if let Err(e) = prepared {
                        app.ui.status = e.clone();
                        ctx.data_mut(|d| d.insert_temp(egui::Id::new("reverse-tools-error"), e));
                    }
                }
            }
            Err(e) => {
                app.ui.status = e.clone();
                ctx.data_mut(|d| d.insert_temp(egui::Id::new("reverse-tools-error"), e));
            }
        }
    } else {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}
fn start_global(app: &mut FilmcraftApp, ctx: &egui::Context) -> Result<Value, String> {
    if !app.ui.panels.reverse_engineering.range_mode || !super::reverse_engineering::motion_is_current(app) {
        return Err("Select a range and analyze SEA-RAFT flow first".into());
    }
    let state = &app.ui.panels.reverse_engineering;
    let flow = state.motion.as_ref().ok_or("Missing optical flow")?;
    let tolerance = state.global_tolerance;
    if !tolerance.is_finite() || !(0.01..=10.0).contains(&tolerance) || flow.pairs.is_empty() || flow.pairs.len() > 299 {
        return Err("Invalid fit tolerance or flow range".into());
    }
    let report=GlobalReport {schema_version:1,analyzer:"filmcraft-vision/global-similarity-v1".into(),completed_at_unix_ms:0,source_motion_revision:flow.audit.revision,
        source_completed_at_unix_ms:source_time(flow),residual_tolerance_pixels:tolerance,captures:flow.captures.clone(),pairs:Vec::new(),
        limitations:vec!["Fits 2D image similarity (translation, rotation, uniform scale), not physical camera pose, lens parameters or depth.".into(),
            "A large moving foreground subject can dominate the fit. Optical-flow errors propagate into this result.".into(),
            "Supporting samples must cover at least three image quadrants and span 30% of image width and height; at least half the valid samples must support the fit.".into()]};
    let pairs = flow.pairs.clone();
    let job = Job { cancel: Arc::new(AtomicBool::new(false)), progress: Arc::new(AtomicUsize::new(0)), total: pairs.len(), result: Arc::new(Mutex::new(None)) };
    let worker = job.clone();
    let repaint = ctx.clone();
    std::thread::Builder::new()
        .name("filmcraft-global-motion".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<GlobalReport, String> {
                let mut report = report;
                let started = std::time::Instant::now();
                for (i, pair) in pairs.iter().enumerate() {
                    if worker.cancel.load(Ordering::Relaxed) {
                        return Err("Global motion cancelled".into());
                    }
                    if started.elapsed().as_secs() > 60 {
                        return Err("Global fitting timed out; use a shorter range".into());
                    }
                    report.pairs.push(estimate(pair, tolerance)?);
                    worker.progress.store(i + 1, Ordering::Relaxed);
                    repaint.request_repaint();
                }
                report.completed_at_unix_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|e| e.to_string())
                    .and_then(|d| u64::try_from(d.as_millis()).map_err(|e| e.to_string()))?;
                Ok(report)
            }))
            .unwrap_or_else(|_| Err("Global motion worker failed; previous results preserved".into()));
            *worker.result.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(result);
            repaint.request_repaint();
        })
        .map_err(|e| format!("Could not start global fitting: {e}"))?;
    ctx.data_mut(|d| d.insert_temp(job_id(), job));
    Ok(json!({"started":true}))
}
fn export(app: &mut FilmcraftApp, params: &Value, value: &Value) -> Result<Value, String> {
    let path = if let Some(path) = params.get("path") {
        Some(path.as_str().filter(|s| !s.trim().is_empty()).ok_or("Supply a nonempty export path")?.to_owned())
    } else {
        app.hooks.pick_save_as.as_mut().ok_or("No save dialog; supply path")?("Reverse engineering", &["json"], "reverse-engineering.json")
    };
    let Some(path) = path else {
        return Ok(Value::Null);
    };
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    std::fs::write(&path, bytes).map_err(|e| format!("Could not export analysis: {e}"))?;
    app.ui.status = format!("Exported {path}");
    Ok(json!({"path":path}))
}
fn saved_path(app: &FilmcraftApp) -> Option<std::path::PathBuf> {
    if let Some(path) = app.session.path.as_ref() {
        return Some(std::path::PathBuf::from(format!("{path}.analysis.json")));
    }
    let root = app.session.prefs_path.as_ref()?.parent()?;
    // Unsaved projects are namespaced by deterministic project content, not arbitrary item IDs.
    let bytes = serde_json::to_vec(app.session.project.as_ref()).ok()?;
    let fingerprint = bytes.iter().fold(14695981039346656037u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(1099511628211));
    Some(root.join("analysis-results").join(format!("unsaved-{fingerprint:016x}.json")))
}
fn persist(path: &std::path::Path, results: &[SavedResult]) -> Result<(), String> {
    if path.exists() {
        let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        if file.metadata().map_err(|e| e.to_string())?.len() > 100_000_000 {
            return Err("Existing saved-results file is too large; export to a new file instead".into());
        }
        let previous: Value = serde_json::from_reader(file).map_err(|e| format!("Existing file is not a valid result store; it was preserved: {e}"))?;
        if previous.get("format").and_then(Value::as_str) != Some("filmcraft-analysis-store")
            || previous.get("schemaVersion").and_then(Value::as_u64) != Some(1)
            || !previous.get("results").is_some_and(Value::is_array)
        {
            return Err("Existing file is not an analysis store and was preserved".into());
        }
    }
    let bytes = serde_json::to_vec(&json!({"format":"filmcraft-analysis-store","schemaVersion":1,"results":results})).map_err(|e| e.to_string())?;
    if bytes.len() > 100_000_000 {
        return Err("Saved analysis exceeds 100 MB; export and delete older results first".into());
    }
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| std::path::Path::new("."));
    std::fs::create_dir_all(parent).map_err(|e| format!("Could not create result directory: {e}"))?;
    let pending = path.with_extension(format!("pending-{}", std::process::id()));
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&pending).map_err(|e| format!("Could not create pending result file: {e}"))?;
    use std::io::Write;
    let result = file.write_all(&bytes).and_then(|_| file.sync_all()).and_then(|_| std::fs::rename(&pending, path));
    if let Err(e) = result {
        let _ = std::fs::remove_file(&pending);
        return Err(format!("Could not save results; previous file preserved: {e}"));
    }
    Ok(())
}
fn restore_saved(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let Some(path) = saved_path(app) else {
        return;
    };
    let key = egui::Id::new("reverse-tools-loaded-store");
    if ctx.data(|d| d.get_temp::<std::path::PathBuf>(key)).as_ref() == Some(&path) {
        return;
    }
    let loaded = (|| -> Result<Vec<SavedResult>, String> {
        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.to_string()),
        };
        if file.metadata().map_err(|e| e.to_string())?.len() > 100_000_000 {
            return Err("Saved analysis file exceeds 100 MB".into());
        }
        use std::io::Read;
        let value: Value = serde_json::from_reader(file.take(100_000_001)).map_err(|e| e.to_string())?;
        if value.get("format").and_then(Value::as_str) != Some("filmcraft-analysis-store") || value.get("schemaVersion").and_then(Value::as_u64) != Some(1) {
            return Err("Unsupported saved-results version".into());
        }
        let results: Vec<SavedResult> = serde_json::from_value(value.get("results").cloned().ok_or("Missing saved results")?).map_err(|e| e.to_string())?;
        if results.len() > 16 || results.iter().any(|r| !matches!(r.tool(), "trajectories" | "objectMasks" | "shots" | "camera2d") && r.captures().len() > 300)
        {
            return Err("Saved-result limits exceeded".into());
        }
        Ok(results)
    })();
    match loaded {
        Ok(results) => app.ui.panels.reverse_engineering.saved_results = results,
        Err(e) => app.ui.status = format!("Saved analysis could not be loaded: {e}"),
    }
    ctx.data_mut(|d| d.insert_temp(key, path));
}

pub fn command(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str, params: &Value) -> Result<Value, String> {
    if id == "reverseEngineering.cancelGlobal" {
        if let Some(job) = ctx.data(|d| d.get_temp::<Job>(job_id())) {
            job.cancel.store(true, Ordering::Relaxed);
        }
        return Ok(json!({"cancelled":true}));
    }
    if is_running(ctx)
        || super::reverse_motion::is_running(ctx)
        || (super::reverse_tracking::is_running(ctx) || super::reverse_sam::is_running(ctx))
        || super::reverse_shots::is_running(ctx)
        || super::reverse_camera2d::is_running(ctx)
    {
        return Err("Cancel the running analysis before changing tools or results".into());
    }
    if id == "reverseEngineering.setCameraOptions" {
        return super::reverse_camera::options(app, params);
    }
    if id == "reverseEngineering.exportBrief" {
        return export_brief(app, params);
    }
    if id == "reverseEngineering.setExecutionDevice" {
        let tool = params.get("tool").and_then(Value::as_str).ok_or("Supply model tool")?;
        let device: filmcraft_vision::execution::Device =
            serde_json::from_value(params.get("device").cloned().ok_or("Supply auto, cpu or gpu")?).map_err(|e| e.to_string())?;
        match tool {
            "opticalFlow" => app.ui.panels.reverse_engineering.flow_device = device,
            "trajectories" => app.ui.panels.reverse_engineering.tracking_device = device,
            _ => return Err("This tool does not run a model".into()),
        };
        return Ok(json!({"device":device,"appliesTo":"nextRun"}));
    }

    if id == "reverseEngineering.chooseTool" {
        let tool = params.get("tool").and_then(Value::as_str).ok_or("Supply a tool id")?;
        if tool != "menu" && !TOOL_IDS.contains(&tool) {
            return Err("This analytical layer is not implemented yet".into());
        }
        if tool != "menu" && super::reverse_engineering::selected_source(app).is_none() {
            return Err("Select a video clip in the timeline before opening a tool".into());
        }
        if tool != "menu" && !app.ui.panels.reverse_engineering.range_mode {
            return Err("Motion analysis needs a range with at least two frames".into());
        }
        if tool == "objectMasks" {
            app.ui.panels.reverse_engineering.active_tool = tool.into();
            super::reverse_sam::prepare(app)?;
            if !super::reverse_sam::current(app) {
                let saved = app.ui.panels.reverse_engineering.saved_results.iter().find_map(|r| match r {
                    SavedResult::ObjectMasks(report)
                        if super::reverse_engineering::captures_are_current(app, &report.captures)
                            && report.project_revision == app.session.revision
                            && report
                                .inference
                                .validate(
                                    &report.inference.tracks.iter().map(|t| t.object.clone()).collect::<Vec<_>>(),
                                    report.captures.len(),
                                    report.inference.mask_size,
                                )
                                .is_ok() =>
                    {
                        Some(report.clone())
                    }
                    _ => None,
                });
                if let Some(report) = saved {
                    let state = &mut app.ui.panels.reverse_engineering;
                    state.sam_objects = report.inference.tracks.iter().map(|t| t.object.clone()).collect();
                    state.sam_frame = state.sam_objects.first().map(|o| o.frame_index).unwrap_or(0);
                    state.sam_report = Some(report);
                }
            }
            ctx.request_repaint();
            return Ok(json!({"tool":tool}));
        }
        if tool == "camera2d" {
            app.ui.panels.reverse_engineering.active_tool = tool.into();
            super::reverse_camera2d::prepare(app)?;
            if !super::reverse_camera2d::current(app) {
                let saved = app.ui.panels.reverse_engineering.saved_results.iter().find_map(|r| match r {
                    SavedResult::Features(report)
                        if super::reverse_engineering::captures_are_current(app, &report.captures)
                            && report.project_revision == app.session.revision
                            && report.options.validate().is_ok() =>
                    {
                        Some(report.clone())
                    }
                    _ => None,
                });
                if let Some(report) = saved {
                    let state = &mut app.ui.panels.reverse_engineering;
                    state.camera2d_options = report.options;
                    state.camera2d_width = report.analysis_width;
                    state.camera2d_report = Some(report);
                }
            }
            ctx.request_repaint();
            return Ok(json!({"tool":tool,"analysis":if super::reverse_camera2d::current(app) { "current" } else { "notRun" }}));
        }
        if tool == "shots" {
            app.ui.panels.reverse_engineering.active_tool = tool.into();
            if !super::reverse_shots::current(app) {
                let saved = app.ui.panels.reverse_engineering.saved_results.iter().find_map(|r| match r {
                    SavedResult::Shots(report)
                        if super::reverse_engineering::captures_are_current(app, &report.captures)
                            && report.project_revision == app.session.revision
                            && report.shots.len() == report.looks.len()
                            && report.options.validate().is_ok() =>
                    {
                        Some(report.clone())
                    }
                    _ => None,
                });
                if let Some(report) = saved {
                    let state = &mut app.ui.panels.reverse_engineering;
                    state.shot_options = report.options;
                    state.shot_palette_size = report.palette_size;
                    state.shots = Some(report);
                }
            }
            ctx.request_repaint();
            if !super::reverse_shots::current(app) {
                super::reverse_shots::command(app, ctx, "reverseEngineering.runShots", &json!({}))?;
            }
            return Ok(json!({"tool":tool}));
        }
        if tool == "trajectories" {
            app.ui.panels.reverse_engineering.active_tool = tool.into();
            super::reverse_tracking::prepare(app)?;
            if !super::reverse_tracking::current(app) {
                let saved = app.ui.panels.reverse_engineering.saved_results.iter().find_map(|r| match r {
                    SavedResult::PointTracking(report)
                        if super::reverse_engineering::captures_are_current(app, &report.captures) && report.project_revision == app.session.revision =>
                    {
                        Some(report.clone())
                    }
                    _ => None,
                });
                if let Some(report) = saved {
                    let state = &mut app.ui.panels.reverse_engineering;
                    state.tracking_queries = report.queries.clone();
                    state.tracking_labels = report.point_labels.clone();
                    state.tracking_grid = report.selection_mode == "grid";
                    state.tracking_frame = report.queries.first().map(|q| q.frame_index).unwrap_or(0);
                    state.tracking_report = Some(report);
                }
            }
            ctx.request_repaint();
            return Ok(json!({"tool":tool}));
        }
        if tool != "menu" && !super::reverse_engineering::motion_is_current(app) {
            let saved = app.ui.panels.reverse_engineering.saved_results.iter().find_map(|r| match r {
                SavedResult::OpticalFlow(flow) if super::reverse_engineering::captures_are_current(app, &flow.captures) => Some(flow.clone()),
                _ => None,
            });
            if let Some(flow) = saved {
                app.ui.panels.reverse_engineering.motion = Some(flow);
            }
        }
        if matches!(tool, "globalMotion" | "localMotion" | "motionRegions" | "cameraMoves") && !current_global(app) {
            let saved = app.ui.panels.reverse_engineering.saved_results.iter().find_map(|r| match r {
                SavedResult::GlobalMotion(global) if super::reverse_engineering::captures_are_current(app, &global.captures) => Some(global.clone()),
                _ => None,
            });
            if let Some(global) = saved {
                app.ui.panels.reverse_engineering.global_motion = Some(global);
            }
        }
        if tool == "cameraMoves" && !super::reverse_camera::current(app) {
            let saved = app.ui.panels.reverse_engineering.saved_results.iter().find_map(|r| match r {
                SavedResult::CameraMoves(camera)
                    if super::reverse_engineering::captures_are_current(app, &camera.captures) && camera.summary.options.validate().is_ok() =>
                {
                    Some(camera.clone())
                }
                _ => None,
            });
            if let Some(camera) = saved {
                app.ui.panels.reverse_engineering.camera_options = camera.summary.options;
                app.ui.panels.reverse_engineering.camera_moves = Some(camera);
            }
        }
        if matches!(tool, "localMotion" | "motionRegions") && !super::reverse_local::current(app) {
            let saved = app.ui.panels.reverse_engineering.saved_results.iter().find_map(|r| match r {
                SavedResult::LocalMotion(local) if super::reverse_engineering::captures_are_current(app, &local.captures) => Some(local.clone()),
                _ => None,
            });
            if let Some(local) = saved {
                app.ui.panels.reverse_engineering.local_motion = Some(local);
            }
        }
        if tool == "motionRegions" && !super::reverse_regions::current(app) {
            let saved = app.ui.panels.reverse_engineering.saved_results.iter().find_map(|r| match r {
                SavedResult::MotionRegions(regions) if super::reverse_engineering::captures_are_current(app, &regions.captures) => Some(regions.clone()),
                _ => None,
            });
            if let Some(regions) = saved {
                app.ui.panels.reverse_engineering.motion_regions = Some(regions);
            }
        }
        if tool == "discontinuities" && !super::reverse_discontinuities::current(app) {
            let saved = app.ui.panels.reverse_engineering.saved_results.iter().find_map(|r| match r {
                SavedResult::Discontinuities(report)
                    if super::reverse_engineering::captures_are_current(app, &report.captures)
                        && report.threshold_pixels.is_finite()
                        && (0.001..=100.0).contains(&report.threshold_pixels) =>
                {
                    Some(report.clone())
                }
                _ => None,
            });
            if let Some(report) = saved {
                app.ui.panels.reverse_engineering.discontinuities = Some(report);
            }
        }
        app.ui.panels.reverse_engineering.active_tool = tool.into();
        ctx.request_repaint();
        if tool == "opticalFlow"
            || (matches!(tool, "globalMotion" | "localMotion" | "motionRegions" | "motionOverTime" | "discontinuities" | "cameraMoves")
                && !super::reverse_engineering::motion_is_current(app))
        {
            super::reverse_engineering::command(app, ctx, "reverseEngineering.analyzeMotion", &json!({}))?;
        }
        if matches!(tool, "globalMotion" | "localMotion" | "motionRegions" | "cameraMoves")
            && super::reverse_engineering::motion_is_current(app)
            && !current_global(app)
        {
            start_global(app, ctx)?;
        }
        if matches!(tool, "localMotion" | "motionRegions") && current_global(app) && !super::reverse_local::current(app) {
            super::reverse_local::build(app)?;
        }
        if tool == "motionRegions" && super::reverse_local::current(app) && !super::reverse_regions::current(app) {
            super::reverse_regions::build(app)?;
        }
        if tool == "cameraMoves" && current_global(app) && !super::reverse_camera::current(app) {
            super::reverse_camera::build(app)?;
        }
        return Ok(json!({"tool":tool}));
    }
    if id == "reverseEngineering.runCamera" {
        if !super::reverse_engineering::motion_is_current(app) {
            return super::reverse_engineering::command(app, ctx, "reverseEngineering.analyzeMotion", &json!({}));
        }
        if !current_global(app) {
            return start_global(app, ctx);
        }
        return super::reverse_camera::build(app);
    }
    if id == "reverseEngineering.setDiscontinuityOptions" {
        return super::reverse_discontinuities::options(app, params);
    }
    if id == "reverseEngineering.setLocalView" {
        let mode = params.get("mode").and_then(Value::as_str).ok_or("Supply comparison mode: raw or residual")?;
        let raw = match mode {
            "raw" => true,
            "residual" => false,
            _ => return Err("Comparison mode must be raw or residual".into()),
        };
        app.ui.panels.reverse_engineering.local_show_raw = raw;
        ctx.request_repaint();
        return Ok(json!({"mode":mode}));
    }
    if id == "reverseEngineering.setRegionOptions" {
        return super::reverse_regions::set_options(app, params);
    }
    if id == "reverseEngineering.runRegions" {
        if !super::reverse_local::current(app) {
            let preparation = command(app, ctx, "reverseEngineering.runLocal", &json!({}))?;
            if !super::reverse_local::current(app) {
                return Ok(preparation);
            }
        }
        return super::reverse_regions::build(app);
    }
    if id == "reverseEngineering.runLocal" {
        if !super::reverse_engineering::motion_is_current(app) {
            return super::reverse_engineering::command(app, ctx, "reverseEngineering.analyzeMotion", &json!({}));
        }
        if !current_global(app) {
            return start_global(app, ctx);
        }
        return super::reverse_local::build(app);
    }
    if id == "reverseEngineering.runGlobal" {
        if !super::reverse_engineering::motion_is_current(app) {
            return super::reverse_engineering::command(app, ctx, "reverseEngineering.analyzeMotion", &json!({}));
        }
        return start_global(app, ctx);
    }
    if id == "reverseEngineering.setGlobalOptions" {
        let tolerance = params.get("tolerancePixels").and_then(Value::as_f64).ok_or("Supply fit tolerance")?;
        if !tolerance.is_finite() || !(0.01..=10.0).contains(&tolerance) {
            return Err("Tolerance must be 0.01–10 analysis pixels".into());
        }
        let show = params.get("showSupport").and_then(Value::as_bool).unwrap_or(app.ui.panels.reverse_engineering.global_show_support);
        app.ui.panels.reverse_engineering.global_tolerance = tolerance;
        app.ui.panels.reverse_engineering.global_show_support = show;
        return Ok(json!({"tolerancePixels":tolerance,"showSupport":show}));
    }
    if id == "reverseEngineering.exportCombined" {
        let saved = &app.ui.panels.reverse_engineering.saved_results;
        let results: Vec<_> = saved
            .iter()
            .filter(|r| super::reverse_engineering::captures_are_current(app, r.captures()))
            .filter(|r| match r {
                SavedResult::Discontinuities(report) => !saved.iter().any(|s| match s {
                    SavedResult::OpticalFlow(flow) if flow.captures == report.captures => {
                        flow.audit.revision != report.source_motion_revision || source_time(flow) != report.source_completed_at_unix_ms
                    }
                    _ => false,
                }),
                SavedResult::MotionOverTime(report) => !saved.iter().any(|s| match s {
                    SavedResult::OpticalFlow(flow) if flow.captures == report.captures => {
                        flow.audit.revision != report.source_motion_revision || source_time(flow) != report.source_completed_at_unix_ms
                    }
                    _ => false,
                }),
                SavedResult::GlobalMotion(global) => !saved.iter().any(|s| match s {
                    SavedResult::OpticalFlow(flow) if flow.captures == global.captures => {
                        flow.audit.revision != global.source_motion_revision || source_time(flow) != global.source_completed_at_unix_ms
                    }
                    _ => false,
                }),
                SavedResult::LocalMotion(local) => !saved.iter().any(|s| match s {
                    SavedResult::OpticalFlow(flow) if flow.captures == local.captures => {
                        flow.audit.revision != local.source_motion_revision || source_time(flow) != local.source_flow_completed_at_unix_ms
                    }
                    SavedResult::GlobalMotion(global) if global.captures == local.captures => {
                        global.completed_at_unix_ms != local.source_global_completed_at_unix_ms
                            || global.residual_tolerance_pixels != local.residual_tolerance_pixels
                    }
                    _ => false,
                }),
                SavedResult::CameraMoves(camera) => !saved.iter().any(|s| match s {
                    SavedResult::OpticalFlow(flow) if flow.captures == camera.captures => {
                        flow.audit.revision != camera.source_motion_revision || source_time(flow) != camera.source_flow_completed_at_unix_ms
                    }
                    SavedResult::GlobalMotion(global) if global.captures == camera.captures => {
                        global.completed_at_unix_ms != camera.source_global_completed_at_unix_ms
                            || global.residual_tolerance_pixels != camera.residual_tolerance_pixels
                    }
                    _ => false,
                }),
                SavedResult::MotionRegions(regions) => !saved.iter().any(|s| match s {
                    SavedResult::OpticalFlow(flow) if flow.captures == regions.captures => {
                        flow.audit.revision != regions.source_motion_revision || source_time(flow) != regions.source_flow_completed_at_unix_ms
                    }
                    SavedResult::GlobalMotion(global) if global.captures == regions.captures => {
                        global.completed_at_unix_ms != regions.source_global_completed_at_unix_ms
                            || global.residual_tolerance_pixels != regions.residual_tolerance_pixels
                    }
                    SavedResult::LocalMotion(local) if local.captures == regions.captures => {
                        local.source_motion_revision != regions.source_motion_revision
                            || local.source_flow_completed_at_unix_ms != regions.source_flow_completed_at_unix_ms
                            || local.source_global_completed_at_unix_ms != regions.source_global_completed_at_unix_ms
                    }
                    _ => false,
                }),
                _ => true,
            })
            .cloned()
            .collect();
        if results.is_empty() {
            return Err("Save at least one tool result for the selected range before exporting".into());
        }
        return export(app, params, &json!({"schemaVersion":1,"scope":"selectedVideoRange","results":results}));
    }
    let tool = app.ui.panels.reverse_engineering.active_tool.clone();
    if !TOOL_IDS.contains(&tool.as_str()) {
        return Err("Choose an implemented tool first".into());
    }
    if id == "reverseEngineering.deleteToolResult" {
        let saved = app.ui.panels.reverse_engineering.saved_results.clone();
        let retained: Vec<SavedResult> =
            saved.into_iter().filter(|r| r.tool() != tool || !super::reverse_engineering::captures_are_current(app, r.captures())).collect();
        let path = saved_path(app).ok_or("Save the project first or supply a persistent results location")?;
        persist(&path, &retained)?;
        app.ui.panels.reverse_engineering.saved_results = retained;
        match tool.as_str() {
            "opticalFlow" => {
                app.ui.panels.reverse_engineering.motion = None;
                app.ui.panels.reverse_engineering.global_motion = None;
                app.ui.panels.reverse_engineering.local_motion = None;
                app.ui.panels.reverse_engineering.motion_regions = None;
            }
            "globalMotion" => {
                app.ui.panels.reverse_engineering.global_motion = None;
                app.ui.panels.reverse_engineering.local_motion = None;
                app.ui.panels.reverse_engineering.motion_regions = None;
            }
            "localMotion" => {
                app.ui.panels.reverse_engineering.local_motion = None;
                app.ui.panels.reverse_engineering.motion_regions = None;
            }
            "motionRegions" => app.ui.panels.reverse_engineering.motion_regions = None,
            "trajectories" => app.ui.panels.reverse_engineering.tracking_report = None,
            "objectMasks" => {
                app.ui.panels.reverse_engineering.sam_report = None;
            }
            "discontinuities" => {
                app.ui.panels.reverse_engineering.discontinuities = None;
                app.ui.panels.reverse_engineering.active_tool = "menu".into();
            }
            "motionOverTime" => {
                app.ui.panels.reverse_engineering.motion_time = None;
                app.ui.panels.reverse_engineering.active_tool = "menu".into();
            }
            "shots" => {
                app.ui.panels.reverse_engineering.shots = None;
                app.ui.panels.reverse_engineering.shot_selected = 0;
            }
            "cameraMoves" => app.ui.panels.reverse_engineering.camera_moves = None,
            "camera2d" => app.ui.panels.reverse_engineering.camera2d_report = None,
            _ => return Err("Choose an implemented tool".into()),
        };
        app.ui.status = "Deleted this tool result for the selected range".into();
        return Ok(json!({"deleted":true}));
    }
    if tool == "motionOverTime" && super::reverse_engineering::motion_is_current(app) && !super::reverse_time::current(app) {
        super::reverse_time::build(app)?;
    }
    let result = match tool.as_str() {
        "objectMasks" if super::reverse_sam::current(app) => {
            SavedResult::ObjectMasks(app.ui.panels.reverse_engineering.sam_report.clone().ok_or("Analyze masks first")?)
        }
        "opticalFlow" if super::reverse_engineering::motion_is_current(app) => {
            SavedResult::OpticalFlow(app.ui.panels.reverse_engineering.motion.clone().ok_or("Analyze first")?)
        }
        "globalMotion" if current_global(app) => SavedResult::GlobalMotion(app.ui.panels.reverse_engineering.global_motion.clone().ok_or("Analyze first")?),
        "localMotion" if super::reverse_local::current(app) => {
            SavedResult::LocalMotion(app.ui.panels.reverse_engineering.local_motion.clone().ok_or("Analyze first")?)
        }
        "motionRegions" if super::reverse_regions::current(app) => {
            SavedResult::MotionRegions(app.ui.panels.reverse_engineering.motion_regions.clone().ok_or("Analyze first")?)
        }
        "discontinuities" if super::reverse_discontinuities::current(app) => {
            SavedResult::Discontinuities(app.ui.panels.reverse_engineering.discontinuities.clone().ok_or("Analyze motion changes first")?)
        }
        "motionOverTime" if super::reverse_time::current(app) => {
            SavedResult::MotionOverTime(app.ui.panels.reverse_engineering.motion_time.clone().ok_or("Summarize motion first")?)
        }
        "trajectories" if super::reverse_tracking::current(app) => {
            SavedResult::PointTracking(app.ui.panels.reverse_engineering.tracking_report.clone().ok_or("Track points first")?)
        }
        "shots" if super::reverse_shots::current(app) => SavedResult::Shots(app.ui.panels.reverse_engineering.shots.clone().ok_or("Detect shots first")?),
        "cameraMoves" if super::reverse_camera::current(app) => {
            SavedResult::CameraMoves(app.ui.panels.reverse_engineering.camera_moves.clone().ok_or("Classify camera moves first")?)
        }
        "camera2d" if super::reverse_camera2d::current(app) => {
            SavedResult::Features(app.ui.panels.reverse_engineering.camera2d_report.clone().ok_or("Detect feature points first")?)
        }
        _ => return Err("Run the selected tool for the current range first".into()),
    };
    if id == "reverseEngineering.saveToolResult" {
        let path = if let Some(value) = params.get("path") {
            std::path::PathBuf::from(value.as_str().filter(|s| !s.trim().is_empty()).ok_or("Supply a nonempty save path")?)
        } else {
            saved_path(app).ok_or("Save the project first to establish a persistent result location")?
        };
        let mut saved = app.ui.panels.reverse_engineering.saved_results.clone();
        if let Some(index) = saved.iter().position(|r| r.tool() == result.tool() && r.captures() == result.captures()) {
            if let Some(slot) = saved.get_mut(index) {
                *slot = result;
            }
        } else {
            if saved.len() >= 16 {
                return Err("Saved-result limit reached (16); export/delete an older result first".into());
            }
            saved.push(result);
        }
        persist(&path, &saved)?;
        if let Some(SavedResult::PointTracking(report)) =
            saved.iter().find(|r| r.tool() == tool && r.captures() == app.ui.panels.reverse_engineering.tracking_captures)
        {
            let state = &mut app.ui.panels.reverse_engineering;
            state.tracking_queries = report.queries.clone();
            state.tracking_labels = report.point_labels.clone();
            state.tracking_report = Some(report.clone());
        }
        app.ui.panels.reverse_engineering.saved_results = saved;
        app.ui.status = format!("Tool result saved to {}", path.display());
        return Ok(json!({"saved":true,"path":path}));
    }
    if id == "reverseEngineering.exportTool" {
        let value = serde_json::to_value(result).map_err(|e| e.to_string())?;
        return export(app, params, &value);
    }
    Err("Unknown analytical tool command".into())
}
fn invoke(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str, params: Value) {
    if let Err(e) = crate::menus::invoke(app, ctx, id, params) {
        app.ui.status = e.clone();
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("reverse-tools-error"), e));
    } else {
        ctx.data_mut(|d| d.remove::<String>(egui::Id::new("reverse-tools-error")));
    }
}
fn status(app: &FilmcraftApp, tool: &str) -> String {
    let state = &app.ui.panels.reverse_engineering;
    let snapshot = state.saved_results.iter().find(|r| r.tool() == tool && super::reverse_engineering::captures_are_current(app, r.captures()));
    let other_range =
        state.saved_results.iter().any(|r| r.tool() == tool && r.captures().first().map(|c| c.item) == super::reverse_engineering::selected_source(app));
    let ready = tool_ready(app, tool);
    let same = match (snapshot, tool) {
        (Some(SavedResult::OpticalFlow(saved)), "opticalFlow") => {
            state.motion.as_ref().is_some_and(|draft| draft.audit == saved.audit && draft.display == saved.display)
        }
        (Some(SavedResult::GlobalMotion(saved)), "globalMotion") => state.global_motion.as_ref() == Some(saved),
        (Some(SavedResult::LocalMotion(saved)), "localMotion") => state.local_motion.as_ref() == Some(saved),
        (Some(SavedResult::MotionRegions(saved)), "motionRegions") => state.motion_regions.as_ref() == Some(saved),
        (Some(SavedResult::ObjectMasks(saved)), "objectMasks") => state.sam_report.as_ref() == Some(saved),
        (Some(SavedResult::Discontinuities(saved)), "discontinuities") => state.discontinuities.as_ref() == Some(saved),
        (Some(SavedResult::MotionOverTime(saved)), "motionOverTime") => state.motion_time.as_ref() == Some(saved),
        (Some(SavedResult::PointTracking(saved)), "trajectories") => state.tracking_report.as_ref() == Some(saved),
        (Some(SavedResult::Shots(saved)), "shots") => state.shots.as_ref() == Some(saved),
        (Some(SavedResult::CameraMoves(saved)), "cameraMoves") => state.camera_moves.as_ref() == Some(saved),
        (Some(SavedResult::Features(saved)), "camera2d") => state.camera2d_report.as_ref() == Some(saved),
        _ => false,
    };
    if ready && same {
        "Saved · current result".into()
    } else if ready && snapshot.is_some() {
        "Computed · unsaved changes".into()
    } else if ready {
        "Computed · not saved".into()
    } else if snapshot.is_some() {
        "Saved snapshot · open tool to restore or recompute".into()
    } else if other_range {
        "Saved for another range of this video".into()
    } else {
        "Not used for this video/range".into()
    }
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    restore_saved(app, ui.ctx());
    if let Some(error) = ui.ctx().data(|d| d.get_temp::<String>(egui::Id::new("reverse-tools-error"))) {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
    }
    let tool = app.ui.panels.reverse_engineering.active_tool.clone();
    if tool == "menu" {
        ui.separator();
        ui.label("2. Components: each one extracts that part of the scene as something you can save and reuse");
        let has_video = super::reverse_engineering::selected_source(app).is_some();
        let label_width = (ui.available_width() - 24.0).max(1.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
            ui.spacing_mut().button_padding = egui::vec2(12.0, 8.0);
            for (id, title, implemented) in COMPONENTS {
                tool_button(app, ui, id, title, implemented, has_video, label_width);
            }
        });
        ui.add_space(6.0);
        let header = egui::CollapsingHeader::new("Evidence").id_salt("reverse-evidence").default_open(false).show(ui, |ui| {
            ui.label("Raw analysis layers the components build on; inspect and export them as before.");
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
                ui.spacing_mut().button_padding = egui::vec2(12.0, 8.0);
                for (id, title, implemented) in EVIDENCE {
                    tool_button(app, ui, id, title, implemented, has_video, label_width);
                }
            });
            let has_saved = app.ui.panels.reverse_engineering.saved_results.iter().any(|r| super::reverse_engineering::captures_are_current(app, r.captures()));
            let response = ui.add_enabled(has_saved, egui::Button::new("Export saved results for this range"));
            app.auto.add("reverseEngineering.exportCombined", response.rect, "Export combined JSON of saved results for selected range");
            if response.clicked() {
                invoke(app, ui.ctx(), "reverseEngineering.exportCombined", json!({}));
            }
        });
        app.auto.add("reverseEngineering.evidence", header.header_response.rect, "Evidence layers");
        return;
    }
    let busy = is_running(ui.ctx())
        || super::reverse_motion::is_running(ui.ctx())
        || (super::reverse_tracking::is_running(ui.ctx()) || super::reverse_sam::is_running(ui.ctx()))
        || super::reverse_shots::is_running(ui.ctx())
        || super::reverse_camera2d::is_running(ui.ctx());
    let response = ui.add_enabled(!busy, egui::Button::new("← Tool menu"));
    app.auto.add("reverseEngineering.tool.back", response.rect, "Back to tool menu");
    if response.clicked() {
        invoke(app, ui.ctx(), "reverseEngineering.chooseTool", json!({"tool":"menu"}));
    }
    if tool == "opticalFlow" {
        super::reverse_motion::show(app, ui);
    } else if tool == "globalMotion" {
        ui.heading("Global image motion");
        ui.label("Shared 2D image movement—not a recovered physical camera angle.");
        if let Some(job) = ui.ctx().data(|d| d.get_temp::<Job>(job_id())) {
            ui.label(format!("Fitting {} / {} comparisons", job.progress.load(Ordering::Relaxed), job.total));
            let response = ui.button("Cancel fitting");
            app.auto.add("reverseEngineering.cancelGlobal", response.rect, "Cancel global fitting");
            if response.clicked() {
                invoke(app, ui.ctx(), "reverseEngineering.cancelGlobal", json!({}));
            }
            return;
        }
        if !super::reverse_engineering::motion_is_current(app) {
            if super::reverse_motion::is_running(ui.ctx()) {
                ui.label("Preparing optical flow for global motion…");
                super::reverse_motion::show(app, ui);
            } else {
                let response = ui.button("Analyze global motion");
                app.auto.add("reverseEngineering.runGlobal", response.rect, "Analyze global motion including its optical-flow prerequisite");
                if response.clicked() {
                    invoke(app, ui.ctx(), "reverseEngineering.runGlobal", json!({}));
                }
            }
            return;
        }
        let mut tolerance = app.ui.panels.reverse_engineering.global_tolerance;
        let mut support = app.ui.panels.reverse_engineering.global_show_support;
        ui.horizontal_wrapped(|ui| {
            ui.label("Fit tolerance (analysis px)");
            let response = ui.add(egui::DragValue::new(&mut tolerance).range(0.01..=10.0).speed(0.01));
            app.auto.add("reverseEngineering.global.tolerance", response.rect, "Maximum supporting-sample residual in analysis pixels");
        });
        let response = ui.checkbox(&mut support, "Show samples supporting the fitted model");
        app.auto.add("reverseEngineering.global.support", response.rect, "Show only samples mathematically supporting the fit");
        if tolerance != app.ui.panels.reverse_engineering.global_tolerance || support != app.ui.panels.reverse_engineering.global_show_support {
            invoke(app, ui.ctx(), "reverseEngineering.setGlobalOptions", json!({"tolerancePixels":tolerance,"showSupport":support}));
        }
        if !current_global(app) {
            ui.label("Run fitting to produce results for these settings. Previous results are hidden.");
        }
        if current_global(app)
            && let Some(report) = app.ui.panels.reverse_engineering.global_motion.as_ref()
        {
            let estimated = report.pairs.iter().filter(|p| matches!(p, GlobalMotion::Estimated { .. })).count();
            ui.label(format!(
                "Completed: {estimated} estimates · {} insufficient · input flow revision {}",
                report.pairs.len() - estimated,
                report.source_motion_revision
            ));
            ui.label("Select frame 2 or later below to inspect motion into that frame.");
        }
        let response = ui.button("Fit global motion for every comparison");
        app.auto.add("reverseEngineering.runGlobal", response.rect, "Fit similarity transform across all analyzed frame pairs");
        if response.clicked() {
            invoke(app, ui.ctx(), "reverseEngineering.runGlobal", json!({}));
        }
    }
    if tool == "localMotion" {
        ui.heading("Local motion");
        ui.label("SEA-RAFT movement minus the fitted global movement.");
        if let Some(job) = ui.ctx().data(|d| d.get_temp::<Job>(job_id())) {
            ui.label(format!("Preparing global fit · {} / {} comparisons", job.progress.load(Ordering::Relaxed), job.total));
            let response = ui.button("Cancel preparation");
            app.auto.add("reverseEngineering.cancelGlobal", response.rect, "Cancel local-motion prerequisite fitting");
            if response.clicked() {
                invoke(app, ui.ctx(), "reverseEngineering.cancelGlobal", json!({}));
            }
            return;
        }
        if super::reverse_motion::is_running(ui.ctx()) {
            ui.label("Preparing optical flow for local motion…");
            super::reverse_motion::show(app, ui);
            return;
        }
        let response = ui.button(if super::reverse_local::current(app) { "Recompute residuals" } else { "Analyze local motion" });
        app.auto.add("reverseEngineering.runLocal", response.rect, "Compute local residual motion with required flow and global fit");
        if response.clicked() {
            invoke(app, ui.ctx(), "reverseEngineering.runLocal", json!({}));
        }
        if super::reverse_local::current(app) {
            if let Some(report) = app.ui.panels.reverse_engineering.local_motion.as_ref() {
                let available = report.pairs.iter().filter(|p| matches!(p, filmcraft_vision::local_motion::LocalMotion::Estimated { .. })).count();
                ui.label(format!("{available} comparisons · {} unavailable", report.pairs.len() - available));
            }
            let frame = ui.ctx().data(|d| d.get_temp::<usize>(egui::Id::new("reverse-motion-frame"))).unwrap_or(0);
            if frame > 0 {
                ui.horizontal_wrapped(|ui| {
                    for (raw, label, mode) in [(true, "Raw SEA-RAFT", "raw"), (false, "Residual motion", "residual")] {
                        let response = ui.selectable_label(app.ui.panels.reverse_engineering.local_show_raw == raw, label);
                        app.auto.add(&format!("reverseEngineering.localView.{mode}"), response.rect, label);
                        if response.clicked() {
                            invoke(app, ui.ctx(), "reverseEngineering.setLocalView", json!({"mode":mode}));
                        }
                    }
                });
                ui.label("Same frame, sample positions and display settings. Only the measured displacement changes.");
                super::reverse_motion::display_controls(app, ui);
            } else {
                ui.label("First frame is the reference. Select a later frame to inspect residual motion.");
            }
        }
    }
    if tool == "discontinuities" {
        super::reverse_discontinuities::show(app, ui);
    }
    if tool == "shots" {
        super::reverse_shots::show(app, ui);
    }
    if tool == "cameraMoves" {
        super::reverse_camera::show(app, ui);
    }
    if tool == "camera2d" {
        super::reverse_camera2d::show(app, ui);
    }
    if tool == "motionOverTime" {
        super::reverse_time::show(app, ui);
    }
    if tool == "motionRegions" {
        super::reverse_regions::show(app, ui);
    }
    if tool == "objectMasks" {
        super::reverse_sam::show(app, ui);
    }
    if tool == "trajectories" {
        super::reverse_tracking::show(app, ui);
    }
    if !busy
        && results_visible(app, ui.ctx())
        && (tool != "trajectories" || super::reverse_tracking::current(app))
        && (tool != "objectMasks" || super::reverse_sam::current(app))
        && (tool != "shots" || super::reverse_shots::current(app))
        && (tool != "camera2d" || super::reverse_camera2d::current(app))
    {
        ui.horizontal_wrapped(|ui| {
            for (id, label) in [("saveToolResult", "Save result"), ("deleteToolResult", "Delete result"), ("exportTool", "Export this tool")] {
                let response = ui.button(label);
                app.auto.add(&format!("reverseEngineering.{id}"), response.rect, label);
                if response.clicked() {
                    invoke(app, ui.ctx(), &format!("reverseEngineering.{id}"), json!({}));
                }
            }
        });
        ui.label(status(app, &tool));
    }
}
pub(super) fn caption(app: &FilmcraftApp, ui: &mut egui::Ui, index: usize) {
    let Some(result) = app.ui.panels.reverse_engineering.global_motion.as_ref().and_then(|r| r.pairs.get(index)) else {
        return;
    };
    match result {
        GlobalMotion::Estimated { fit } => {
            ui.label(format!("Global estimate: center Δx {:.4}px · Δy {:.4}px", fit.center_displacement_pixels[0], fit.center_displacement_pixels[1]));
            ui.label(format!("Rotation {:.4}° · scale {:.6} · RMS residual {:.4}px", fit.rotation_degrees, fit.scale, fit.rms_error_pixels));
            ui.label(format!("Support: {} / {} samples ({:.1}%)", fit.supporting_sample_indices.len(), fit.tested_samples, fit.support_fraction * 100.0));
        }
        GlobalMotion::InsufficientEvidence { reason, .. } => {
            ui.label(format!("Insufficient evidence: {reason}"));
        }
    }
}
pub(super) fn overlay(app: &FilmcraftApp, ui: &egui::Ui, index: usize, rect: egui::Rect) {
    if !app.ui.panels.reverse_engineering.global_show_support || !current_global(app) {
        return;
    }
    let Some(GlobalMotion::Estimated { fit }) = app.ui.panels.reverse_engineering.global_motion.as_ref().and_then(|r| r.pairs.get(index)) else {
        return;
    };
    let Some(pair) = app.ui.panels.reverse_engineering.motion.as_ref().and_then(|r| r.pairs.get(index)) else {
        return;
    };
    let painter = ui.painter().with_clip_rect(rect);
    for i in fit.supporting_sample_indices.iter().take(2048) {
        if let Some(vector) = pair.vectors.get(*i) {
            if !vector.to.iter().all(|n| n.is_finite() && (0.0..=1.0).contains(n)) {
                continue;
            }
            let position = rect.min + egui::vec2(vector.to[0] * rect.width(), vector.to[1] * rect.height());
            painter.circle_filled(position, 1.5, egui::Color32::WHITE);
        }
    }
}

pub(super) fn clear_range_results(app: &mut FilmcraftApp) -> Result<(), String> {
    let saved = &app.ui.panels.reverse_engineering.saved_results;
    let retained: Vec<_> = saved.iter().filter(|r| !super::reverse_engineering::captures_are_current(app, r.captures())).cloned().collect();
    if retained.len() != saved.len() {
        let path = saved_path(app).ok_or("Cannot locate the saved analysis store; save the project first")?;
        persist(&path, &retained)?;
    }
    let s = &mut app.ui.panels.reverse_engineering;
    s.saved_results = retained;
    s.capture = None;
    s.samples.clear();
    s.motion = None;
    s.global_motion = None;
    s.local_motion = None;
    s.motion_regions = None;
    s.sam_report = None;
    s.sam_captures.clear();
    s.sam_objects.clear();
    s.sam_colors.clear();
    s.sam_frame = 0;
    s.discontinuities = None;
    s.motion_time = None;
    s.tracking_report = None;
    s.tracking_queries.clear();
    s.tracking_labels.clear();
    s.tracking_captures.clear();
    s.tracking_frame = 0;
    s.shots = None;
    s.shot_selected = 0;
    s.camera_moves = None;
    s.camera2d_captures.clear();
    s.camera2d_frame = 0;
    s.camera2d_report = None;
    Ok(())
}
fn shots_for_brief(app: &FilmcraftApp) -> Option<super::reverse_shots::ShotReport> {
    if super::reverse_shots::current(app) {
        return app.ui.panels.reverse_engineering.shots.clone();
    }
    app.ui.panels.reverse_engineering.saved_results.iter().find_map(|r| match r {
        SavedResult::Shots(report) if super::reverse_engineering::captures_are_current(app, &report.captures) && report.shots.len() == report.looks.len() => {
            Some(report.clone())
        }
        _ => None,
    })
}
fn export_brief(app: &mut FilmcraftApp, params: &Value) -> Result<Value, String> {
    use filmcraft_vision::brief::{BriefInput, ObjectPresence};
    let shots = shots_for_brief(app).ok_or("Run Shots & look for this range before exporting a brief")?;
    let state = &app.ui.panels.reverse_engineering;
    let camera = if super::reverse_camera::current(app) {
        state.camera_moves.clone()
    } else {
        state.saved_results.iter().find_map(|r| match r {
            SavedResult::CameraMoves(report) if report.captures == shots.captures => Some(report.clone()),
            _ => None,
        })
    };
    let masks = if super::reverse_sam::current(app) {
        state.sam_report.clone()
    } else {
        state.saved_results.iter().find_map(|r| match r {
            SavedResult::ObjectMasks(report) if report.captures == shots.captures => Some(report.clone()),
            _ => None,
        })
    };
    let mut objects = Vec::new();
    if let Some(masks) = masks.as_ref().filter(|m| m.captures == shots.captures) {
        let mask_pixels = masks.inference.mask_size.iter().product::<usize>().max(1) as f64;
        for (track, geometry) in masks.inference.tracks.iter().zip(&masks.geometry) {
            for shot in &shots.shots {
                let present: Vec<&super::reverse_sam::Geometry> = (shot.start_frame..=shot.end_frame)
                    .filter_map(|frame| geometry.get(frame).and_then(|g| g.as_ref()))
                    .filter(|g| g.area_pixels > 0)
                    .collect();
                if present.is_empty() {
                    continue;
                }
                let n = present.len() as f64;
                let centroids: Vec<[f64; 2]> = present.iter().filter_map(|g| g.centroid).collect();
                let mean_centroid = (!centroids.is_empty()).then(|| {
                    [centroids.iter().map(|c| c[0]).sum::<f64>() / centroids.len() as f64, centroids.iter().map(|c| c[1]).sum::<f64>() / centroids.len() as f64]
                });
                objects.push(ObjectPresence {
                    shot_index: shot.index,
                    name: track.object.name.clone(),
                    frames_present: present.len(),
                    frames_total: shot.frame_count,
                    mean_centroid,
                    mean_area_fraction: Some(present.iter().map(|g| g.area_pixels as f64 / mask_pixels).sum::<f64>() / n),
                });
                if objects.len() >= filmcraft_vision::brief::MAX_OBJECTS {
                    break;
                }
            }
        }
    }
    let rate = app.session.active_sequence().ok_or("No active sequence")?.settings.frame_rate;
    let title = shots.captures.first().map(|c| c.name.clone()).unwrap_or_else(|| "Timeline".into());
    let mut limitations: Vec<String> = shots.limitations.clone();
    if let Some(camera) = camera.as_ref() {
        limitations.extend(camera.limitations.iter().cloned());
    }
    if let Some(masks) = masks.as_ref() {
        limitations.extend(masks.limitations.iter().cloned());
    }
    limitations
        .push("This brief is a reconstruction from measured evidence and user labels; it is not the original creator's intent, script or prompt.".into());
    let input = BriefInput {
        title,
        frame_rate: rate.num as f64 / rate.den.max(1) as f64,
        frame_count: shots.captures.len(),
        start_seconds: shots.captures.first().map(|c| c.timeline_time.0 as f64 / filmcraft_time::TICKS_PER_SECOND as f64).unwrap_or(0.0),
        source_size: shots.source_size,
        shots: shots.shots.clone(),
        looks: shots.looks.clone(),
        camera: camera.as_ref().map(|c| c.summary.segments.clone()),
        objects,
        notes: Vec::new(),
        limitations,
    };
    let markdown = filmcraft_vision::brief::markdown(&input, shots.ticks_per_second)?;
    let path = if let Some(path) = params.get("path") {
        Some(path.as_str().filter(|s| !s.trim().is_empty()).ok_or("Supply a nonempty export path")?.to_owned())
    } else {
        app.hooks.pick_save_as.as_mut().ok_or("No save dialog; supply path")?("AI brief", &["md"], "breakdown.md")
    };
    let Some(path) = path else {
        return Ok(Value::Null);
    };
    let json_path = std::path::Path::new(&path).with_extension("json");
    let bytes = serde_json::to_vec_pretty(&json!({"schemaVersion":1,"format":"filmcraft-ai-brief","input":input})).map_err(|e| e.to_string())?;
    std::fs::write(&path, markdown.as_bytes()).map_err(|e| format!("Could not export brief: {e}"))?;
    std::fs::write(&json_path, bytes).map_err(|e| format!("Could not export brief data: {e}"))?;
    app.ui.status = format!("Exported {path} and {}", json_path.display());
    Ok(json!({"path":path,"jsonPath":json_path,"shots":input.shots.len(),"objects":input.objects.len(),"camera":input.camera.is_some()}))
}
pub(super) fn device_controls(app: &mut FilmcraftApp, ui: &mut egui::Ui, tracking: bool) {
    use filmcraft_vision::execution::Device;
    let current = if tracking { app.ui.panels.reverse_engineering.tracking_device } else { app.ui.panels.reverse_engineering.flow_device };
    ui.horizontal_wrapped(|ui| {
        ui.label("Execution");
        for (device, label) in [(Device::Auto, "Auto"), (Device::Cpu, "CPU"), (Device::Gpu, "GPU")] {
            let response = ui.add(egui::Button::new(label).selected(current == device));
            let tool = if tracking { "trajectories" } else { "opticalFlow" };
            app.auto.add(&format!("reverseEngineering.device.{tool}.{}", device.as_str()), response.rect, label);
            if response.clicked() {
                invoke(app, ui.ctx(), "reverseEngineering.setExecutionDevice", json!({"tool":tool,"device":device}));
            }
        }
    });
    if tracking && cfg!(target_os = "macos") {
        ui.label("Auto uses CPU on Apple devices. GPU tries MPS; runtime errors will be shown.");
    }
    ui.label("Execution changes apply to the next model run.");
}

#[cfg(test)]
mod saved_result_tests {
    use super::*;
    #[test]
    fn saved_results_replace_atomically_and_unrelated_files_are_preserved() {
        let path = std::env::temp_dir().join(format!("filmcraft-r5-store-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        persist(&path, &[]).unwrap();
        let value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["format"], "filmcraft-analysis-store");
        persist(&path, &[]).unwrap();
        std::fs::write(&path, b"unrelated user file").unwrap();
        assert!(persist(&path, &[]).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"unrelated user file");
        std::fs::remove_file(&path).unwrap();
    }
}

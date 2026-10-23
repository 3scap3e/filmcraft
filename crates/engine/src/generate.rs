//! Generate: AI video clips straight onto the timeline (the Generate panel).
//!
//! | command | does |
//! |---|---|
//! | `generate.providers` | the generators this build knows: limits, availability, whether a key is set |
//! | `generate.video` | start a generation as a background job; when it finishes, the MP4 is imported and placed on the timeline (one undo step) |
//! | `generate.status` | the running generations: phase, detail, billed seconds |
//! | `generate.cancel` | stop a generation (the provider session is ended) |
//! | `generate.inspect` | the generation record of a clip or item |
//!
//! **Where the clip goes.** At `atTime` / `atFrame` / `atSeconds` (default: the playhead) on
//! `track` (`V2` or a track id) if given, else the source patch's video track, else V1; its audio
//! goes to the patched audio track. `insert: true` ripples the clip in instead of overwriting.
//!
//! Files are named `Generated <n>.mp4` and saved where voice-over recordings go (Scratch Disks ▸
//! Captured, else next to the project, else `Generated` in the data or temporary directory).
//! The API key comes from Settings ▸ Generate, else the `REACTOR_API_KEY` environment variable.

use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{Value, json};

use filmcraft_generate as vg;
use filmcraft_project::{ClipId, Generation, ItemId, TrackId};
use filmcraft_time::{Tick, TimeRange};

use crate::commands::{CommandSpec, always, bad, bool_p, has_seq, str_p, time_p, u64_p};
use crate::{EngineError, Result, Session};

const BASE_NAME: &str = "Generated";

/// A running generation; [`poll`] imports and places its clip when the job finishes.
pub struct PendingGeneration {
    pub job: u64,
    pub provider: String,
    pub request: vg::GenerateRequest,
    pub video_track: Option<TrackId>,
    pub at: Tick,
    pub insert: bool,
    pub progress: Arc<vg::Progress>,
    pub result: Arc<Mutex<Option<vg::GeneratedClip>>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The API key for Reactor: Settings ▸ Generate, else the environment.
pub fn reactor_api_key(s: &Session) -> Option<String> {
    let k = s.prefs.generate.reactor_api_key.trim().to_string();
    if !k.is_empty() {
        return Some(k);
    }
    std::env::var("REACTOR_API_KEY").ok().map(|k| k.trim().to_string()).filter(|k| !k.is_empty())
}

/// Whether this build can run the Reactor client.
pub fn reactor_available() -> bool {
    cfg!(feature = "reactor")
}

fn provider_json(s: &Session) -> Value {
    let limits = vg::fast_h3_limits();
    let key = reactor_api_key(s).is_some();
    let installed = s.generator.is_some();
    json!({
        "id": vg::FAST_H3_ID,
        "name": vg::FAST_H3_NAME,
        "limits": limits,
        "available": installed || reactor_available(),
        "keySet": key || installed,
        "ready": installed || (reactor_available() && key),
        "reason": if installed || (reactor_available() && key) { Value::Null } else if !reactor_available() { json!("not available in this build (built without the `reactor` feature)") } else { json!("no API key: set one in Settings ▸ Generate") },
    })
}

fn providers(s: &mut Session, _p: &Value) -> Result<Value> {
    Ok(json!({"providers": [provider_json(s)], "defaultProvider": vg::FAST_H3_ID}))
}

/// The generator for `id`: the installed one, else the build's.
fn generator(s: &Session, cmd: &str, id: &str) -> Result<Arc<dyn vg::VideoGenerator>> {
    if let Some(g) = &s.generator {
        return Ok(g.clone());
    }
    if id != vg::FAST_H3_ID {
        return Err(bad(cmd, format!("unknown provider `{id}` (see generate.providers)")));
    }
    #[cfg(feature = "reactor")]
    {
        let key = reactor_api_key(s).ok_or_else(|| bad(cmd, "no API key: set one in Settings ▸ Generate (or REACTOR_API_KEY)"))?;
        let g = vg::reactor::FastH3::new(Box::new(move || Some(key.clone())));
        Ok(Arc::new(g))
    }
    #[cfg(not(feature = "reactor"))]
    {
        Err(bad(cmd, "video generation is not available in this build (built without the `reactor` feature)"))
    }
}

/// `<dir>/<base> <n>.mp4` with the first `n` used by neither a project item nor an existing file.
fn unique_path(s: &Session, dir: &str, base: &str) -> Result<String> {
    let names: std::collections::HashSet<String> = s.project.items.values().map(|i| i.name.clone()).collect();
    for k in 1..=99_999u32 {
        let file = format!("{base} {k}.mp4");
        let path = std::path::Path::new(dir).join(&file).to_string_lossy().into_owned();
        if !names.contains(&file) && s.services.file_size(&path).is_err() {
            return Ok(path);
        }
    }
    Err(EngineError::Other(format!("{dir}: no free file name for \"{base} <n>.mp4\"")))
}

/// The video track `v` names: `V2`, `2` (1-based label) or a track id.
fn video_track(s: &Session, cmd: &str, v: &Value) -> Result<TrackId> {
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let by_index = |i: usize| seq.video_tracks.get(i.wrapping_sub(1)).map(|t| t.id);
    let found = match v {
        Value::Number(n) => {
            n.as_u64().and_then(|n| seq.video_tracks.iter().find(|t| t.id.0 == n).map(|t| t.id).or_else(|| usize::try_from(n).ok().and_then(by_index)))
        }
        Value::String(x) => x.trim().trim_start_matches(['V', 'v']).parse::<usize>().ok().and_then(by_index),
        _ => None,
    };
    found.ok_or_else(|| bad(cmd, format!("no video track {v}")))
}

fn status_line(p: &vg::Progress) -> String {
    use std::sync::atomic::Ordering;
    let secs = p.session_ms.load(Ordering::Relaxed) / 1000;
    let detail = p.detail();
    let mut line = p.phase().label().to_string();
    if !detail.is_empty() {
        line.push_str(" · ");
        line.push_str(&detail);
    }
    if secs > 0 {
        line.push_str(&format!(" · session {secs} s"));
    }
    line
}

fn video(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "generate.video";
    let provider = str_p(p, "provider").unwrap_or(vg::FAST_H3_ID).to_string();
    let g = generator(s, cmd, &provider)?;
    let limits = g.limits();
    let prompt = str_p(p, "prompt").map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| bad(cmd, "`prompt` is required"))?.to_string();
    let seconds = match p.get("seconds") {
        Some(v) => v.as_f64().ok_or_else(|| bad(cmd, "`seconds` must be a number"))?,
        None => {
            let d = s.prefs.generate.default_seconds;
            if d.is_finite() { limits.clamp_seconds(d) } else { limits.default_seconds }
        }
    };
    let image = str_p(p, "image").map(|x| std::path::PathBuf::from(x.to_string()));
    let image_role = match str_p(p, "imageRole") {
        Some(r) => vg::ImageRole::from_id(r).ok_or_else(|| bad(cmd, "`imageRole` must be first, last or both"))?,
        None => vg::ImageRole::First,
    };
    let seed = match p.get("seed") {
        Some(Value::Null) | None => None,
        Some(v) => Some(v.as_u64().ok_or_else(|| bad(cmd, "`seed` must be a whole number"))?),
    };
    let at = time_p(s, p, "at").or_else(|| p.get("time").and_then(Value::as_i64).map(Tick)).unwrap_or(s.playhead());
    if at < Tick::ZERO {
        return Err(bad(cmd, "the position cannot be negative"));
    }
    let video_track = match p.get("track") {
        Some(v) => Some(video_track(s, cmd, v)?),
        None => None,
    };
    let insert = bool_p(p, "insert").unwrap_or(s.prefs.generate.insert);
    let dir = crate::voiceover::media_dir(s, p, BASE_NAME);
    let path = unique_path(s, &dir, BASE_NAME)?;
    let request = vg::GenerateRequest { prompt, seconds, image, image_role, seed, output: std::path::PathBuf::from(&path) };
    vg::validate(&request, &limits).map_err(|e| bad(cmd, e.to_string()))?;
    if let Some(img) = &request.image
        && !img.is_file()
    {
        return Err(bad(cmd, format!("{}: no such image", img.display())));
    }
    if !cfg!(target_arch = "wasm32") {
        std::fs::create_dir_all(&dir).map_err(|e| EngineError::Other(format!("{dir}: {e}")))?;
    }
    let short: String = request.prompt.chars().take(40).collect();
    let label = format!("Generating video: {short}{}", if request.prompt.chars().count() > 40 { "…" } else { "" });
    let job = crate::narration::new_job(s, label);
    let progress = Arc::new(vg::Progress::default());
    let result: Arc<Mutex<Option<vg::GeneratedClip>>> = Default::default();
    let (prog, res, vprog, vres, req) = (job.progress.clone(), job.result.clone(), progress.clone(), result.clone(), request.clone());
    let wait = bool_p(p, "wait").unwrap_or(false);
    let estimate = limits.estimate_usd(seconds);
    let id = crate::narration::spawn(s, job, wait, "filmcraft-generate", move || {
        let t0 = web_time::Instant::now();
        *prog.status.lock().unwrap_or_else(PoisonError::into_inner) = "Starting".into();
        let r = match g.generate(&req, &vprog) {
            Ok(clip) => {
                let msg = format!("{:.1} s clip saved to {}", clip.seconds, clip.path.display());
                *vres.lock().unwrap_or_else(PoisonError::into_inner) = Some(clip);
                Ok(msg)
            }
            Err(e) => {
                vprog.set_phase(vg::Phase::Failed);
                vprog.set_detail(e.to_string());
                Err(e.to_string())
            }
        };
        crate::narration::finish(&prog, &res, r, t0);
    })?;
    s.generate_jobs.push(PendingGeneration { job: id, provider: provider.clone(), request, video_track, at, insert, progress, result });
    let mut placed = Value::Null;
    if wait {
        placed = poll(s).into_iter().find(|r| r["job"].as_u64() == Some(id)).map(|r| r["placed"].clone()).unwrap_or(Value::Null);
    }
    Ok(json!({"job": id, "path": path, "provider": provider, "at": at.0, "seconds": seconds, "estimateUsd": estimate, "placed": placed}))
}

/// Mirror progress into the job, then import and place the clips of finished jobs (one undo step
/// each). Called once per UI frame from [`Session::poll_persistence`]. Returns what was placed.
pub fn poll(s: &mut Session) -> Vec<Value> {
    use std::sync::atomic::Ordering;
    let mut finished = Vec::new();
    for (i, pj) in s.generate_jobs.iter().enumerate() {
        let Some(job) = s.jobs.iter().find(|j| j.id == pj.job) else {
            finished.push(i);
            continue;
        };
        if job.progress.cancel.load(Ordering::Relaxed) {
            pj.progress.request_cancel();
        }
        if job.progress.finished.load(Ordering::Relaxed) {
            finished.push(i);
        } else {
            *job.progress.status.lock().unwrap_or_else(PoisonError::into_inner) = status_line(&pj.progress);
        }
    }
    let mut out = Vec::new();
    for i in finished.into_iter().rev() {
        let pj = s.generate_jobs.remove(i);
        let ok = s.jobs.iter().find(|j| j.id == pj.job).is_some_and(|j| matches!(&*lock(&j.result), Some(Ok(_))));
        if !ok {
            continue;
        }
        let Some(clip) = lock(&pj.result).take() else { continue };
        match place(s, &pj, clip) {
            Ok(v) => out.push(json!({"job": pj.job, "placed": v})),
            Err(e) => s.error_toast("generate.video", format!("Generate: {e}")),
        }
    }
    out
}

/// Import the finished MP4 and place it on the timeline, recording the generation on its item.
fn place(s: &mut Session, pj: &PendingGeneration, clip: vg::GeneratedClip) -> Result<Value> {
    let cmd = "generate.video";
    let path = clip.path.to_string_lossy().into_owned();
    let seq_id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let n0 = s.history.undo.len();
    let result = (|| -> Result<Value> {
        let item = if s.services.reader(&path).is_some() {
            crate::commands::import_streamed(s, &path, None)?
        } else {
            let bytes = s.services.read_file(&path).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
            crate::commands::import_bytes(s, &path, bytes.into(), None)?
        };
        let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
        let targeting = s.targeting();
        let vdest = pj
            .video_track
            .filter(|t| seq.video_tracks.iter().any(|x| x.id == *t))
            .or(targeting.video_dest.filter(|t| seq.video_tracks.iter().any(|x| x.id == *t)))
            .or_else(|| seq.video_tracks.first().map(|t| t.id));
        let adest = targeting.audio_dest.filter(|t| seq.audio_tracks.iter().any(|x| x.id == *t)).or_else(|| seq.audio_tracks.first().map(|t| t.id));
        let dur =
            crate::media_duration(&s.project, &s.media, item).filter(|d| *d > Tick::ZERO).unwrap_or_else(|| Tick::from_seconds_f64(clip.seconds.max(0.04)));
        let clips: Vec<ClipId> = crate::commands::place_item(s, item, TimeRange::new(Tick::ZERO, dur), pj.at, vdest, adest, pj.insert, "Generate Video", None)?;
        let created = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis().min(u64::MAX as u128) as u64).unwrap_or(0);
        let record = Generation {
            provider: pj.provider.clone(),
            prompt: pj.request.prompt.clone(),
            seconds: clip.seconds,
            image_role: pj.request.image.as_ref().map(|_| pj.request.image_role.id().to_string()),
            image: pj.request.image.as_ref().map(|p| p.to_string_lossy().into_owned()),
            seed: clip.seed.or(pj.request.seed),
            provider_clip_id: clip.provider_clip_id.clone(),
            width: clip.width,
            height: clip.height,
            fps: clip.fps,
            session_seconds: clip.session_seconds,
            created_unix_ms: created,
        };
        record.check().map_err(|e| bad(cmd, e))?;
        let sel = clips.clone();
        s.edit("Generate Video", move |pr, st| {
            pr.generations.insert(item, record);
            st.selection = sel;
            Ok(())
        })?;
        let seq = s.project.sequence(seq_id).ok_or(EngineError::NoSequence)?;
        let track_label = vdest.and_then(|v| seq.video_tracks.iter().position(|t| t.id == v)).map(|i| format!("V{}", i + 1)).unwrap_or_default();
        s.toast(format!("Generated clip placed on {track_label}"));
        Ok(json!({
            "item": item.0,
            "clips": clips.iter().map(|c| c.0).collect::<Vec<_>>(),
            "track": track_label,
            "start": pj.at.0,
            "duration": dur.0,
            "path": path,
            "seconds": clip.seconds,
            "sessionSeconds": clip.session_seconds,
            "providerClipId": clip.provider_clip_id,
        }))
    })();
    crate::clip_ops::collapse_history(s, n0, "Generate Video");
    if result.is_err() && s.history.undo.len() > n0 {
        let _ = s.undo();
        s.history.redo.clear();
    }
    result
}

fn status(s: &mut Session, _p: &Value) -> Result<Value> {
    use std::sync::atomic::Ordering;
    let limits = vg::fast_h3_limits();
    let jobs: Vec<Value> = s
        .generate_jobs
        .iter()
        .map(|pj| {
            let done = s.jobs.iter().find(|j| j.id == pj.job).is_some_and(|j| j.progress.finished.load(Ordering::Relaxed));
            json!({
                "job": pj.job,
                "provider": pj.provider,
                "prompt": pj.request.prompt,
                "seconds": pj.request.seconds,
                "phase": pj.progress.phase().id(),
                "phaseLabel": pj.progress.phase().label(),
                "detail": pj.progress.detail(),
                "sessionSeconds": pj.progress.session_ms.load(Ordering::Relaxed) as f64 / 1000.0,
                "bytesSaved": pj.progress.bytes_saved.load(Ordering::Relaxed),
                "estimateUsd": limits.estimate_usd(pj.request.seconds),
                "finished": done,
                "at": pj.at.0,
                "path": pj.request.output.to_string_lossy(),
            })
        })
        .collect();
    Ok(json!({"generations": jobs}))
}

fn cancel(s: &mut Session, p: &Value) -> Result<Value> {
    use std::sync::atomic::Ordering;
    let want = u64_p(p, "job");
    let mut n = 0;
    for pj in &s.generate_jobs {
        if want.is_some_and(|w| w != pj.job) {
            continue;
        }
        pj.progress.request_cancel();
        if let Some(j) = s.jobs.iter().find(|j| j.id == pj.job) {
            j.progress.cancel.store(true, Ordering::Relaxed);
        }
        n += 1;
    }
    if n == 0 && want.is_some() {
        return Err(bad("generate.cancel", "no such generation"));
    }
    Ok(json!({"cancelled": n}))
}

fn inspect(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "generate.inspect";
    let (item, clip) = match u64_p(p, "item") {
        Some(i) => (ItemId(i), None),
        None => {
            let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
            let c = u64_p(p, "clip")
                .map(ClipId)
                .or_else(|| s.state.selection.first().copied())
                .ok_or_else(|| bad(cmd, "pass `item` or `clip`, or select a clip"))?;
            (seq.find_item(c).map(|(_, it)| it.item).ok_or_else(|| bad(cmd, "no such clip"))?, Some(c.0))
        }
    };
    let g = s.project.generations.get(&item).ok_or_else(|| bad(cmd, "not a generated clip"))?;
    Ok(json!({"item": item.0, "clip": clip, "generation": g}))
}

fn spec(
    id: &'static str,
    label: &'static str,
    params: &'static str,
    enabled: fn(&Session) -> std::result::Result<(), String>,
    run: fn(&mut Session, &Value) -> Result<Value>,
    journal: bool,
) -> CommandSpec {
    CommandSpec { id, label, menu: &[], shortcut: None, params, enabled, run, journal }
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        spec("generate.providers", "List Video Generators", "{}", always, providers, false),
        spec(
            "generate.video",
            "Generate Video",
            r#"{"prompt":str,"seconds":5.167..14.375?,"image":path?,"imageRole":"first|last|both"?,"seed":u64?,"provider":str?,"track":"V1"|id?,"atTime":ticks?,"atFrame":int?,"atSeconds":f64?,"insert":bool?,"dir":str?,"wait":bool?}"#,
            has_seq,
            video,
            true,
        ),
        spec("generate.status", "Video Generation Status", "{}", always, status, false),
        spec("generate.cancel", "Cancel Video Generation", r#"{"job":id?}"#, always, cancel, false),
        spec("generate.inspect", "Inspect Generated Clip", r#"{"clip":id?,"item":id?}"#, always, inspect, false),
    ]
}

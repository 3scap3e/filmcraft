//! Sound: what the mix of the selected range is made of, measured once over the whole range and
//! read per shot. Loudness (BS.1770), one-second segments labelled speech / music / effects /
//! ambience / silence under named rules, tempo and a beat grid with how many cuts land on beats,
//! and the dialogue lines when a speech model is installed. Exportable as a `filmcraft.sound`
//! file. Pure Rust; the only model is the optional Whisper transcriber the Text panel already uses.
//!
//! Commands: `reverseEngineering.runSound`, `reverseEngineering.cancelSound`,
//! `reverseEngineering.setSoundOptions {"segmentSeconds"?,"minBpm"?,"maxBpm"?,"beatTolerance"?,"dialogue"?}`,
//! `reverseEngineering.soundShot {"index"}`, `reverseEngineering.exportSound {"path"?}`,
//! `reverseEngineering.copySoundDescription {}`, `reverseEngineering.soundBeatMarkers {}`.
use super::{panel_state::ReverseEngineeringCapture, reverse_engineering};
use crate::FilmcraftApp;
use filmcraft_sound::{Analysis, Line, Options, RangeSound, SegmentKind, Word, tempo::CutsOnBeats};
use filmcraft_time::TICKS_PER_SECOND;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

pub const MAX_RANGE_FRAMES: usize = filmcraft_vision::shots::MAX_FRAMES;
pub const SOUND_FORMAT: &str = "filmcraft.sound";
/// Audio mixed per worker step, in sequence frames.
const MIX_CHUNK: usize = 48_000;
/// Longest pause inside one dialogue line, seconds.
const LINE_GAP_SECONDS: f64 = 0.6;

/// Options of the tool as kept in the panel state: the analysis options plus whether dialogue
/// is transcribed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SoundOptions {
    pub analysis: Options,
    /// Transcribe the dialogue with the speech model when one is available.
    pub dialogue: bool,
}

impl Default for SoundOptions {
    fn default() -> Self {
        SoundOptions { analysis: Options::default(), dialogue: true }
    }
}

/// The sound of one shot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SoundShot {
    pub index: usize,
    pub start_frame: usize,
    pub end_frame: usize,
    pub start_tick: i64,
    pub end_tick: i64,
    pub keyframe: usize,
    pub sound: RangeSound,
    /// Dialogue lines that start inside the shot.
    pub lines: Vec<Line>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SoundReport {
    pub schema_version: u32,
    pub analyzer: String,
    pub captures: Vec<ReverseEngineeringCapture>,
    pub project_revision: u64,
    pub completed_at_unix_ms: u64,
    pub options: SoundOptions,
    /// Timeline time of the first analysed sample, ticks.
    pub start_tick: i64,
    /// Timeline time just after the last analysed sample, ticks.
    pub end_tick: i64,
    /// The shots came from a current Shots & look result (else the whole range is one shot).
    pub from_shots: bool,
    pub analysis: Analysis,
    pub shots: Vec<SoundShot>,
    /// Shot boundaries against the beat grid (None without a tempo or without several shots).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cuts_on_beats: Option<CutsOnBeats>,
    /// Dialogue over the whole range, when it was transcribed.
    pub dialogue: Vec<Line>,
    /// Why there is no dialogue, when it was asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dialogue_skipped: Option<String>,
    pub summary: String,
    pub limitations: Vec<String>,
}

/// The exported `filmcraft.sound` file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SoundFile {
    pub format: String,
    pub version: u32,
    pub source: SoundSource,
    pub summary: String,
    pub loudness: filmcraft_sound::Loudness,
    pub shares: filmcraft_sound::Shares,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tempo: Option<filmcraft_sound::Tempo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cuts_on_beats: Option<CutsOnBeats>,
    pub regions: Vec<filmcraft_sound::Region>,
    pub segments: Vec<filmcraft_sound::Segment>,
    pub shots: Vec<SoundShot>,
    pub dialogue: Vec<Line>,
    pub rules: Vec<filmcraft_sound::Rule>,
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SoundSource {
    pub sequence: String,
    pub sample_rate: u32,
    pub channels: usize,
    pub frame_rate: f64,
    pub start_frame: i64,
    pub end_frame: i64,
    pub start_seconds: f64,
    pub seconds: f64,
    pub analyzer: String,
}

#[derive(Clone)]
struct Job {
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicUsize>,
    total: usize,
    /// 0 mixing, 1 analysing, 2 transcribing.
    stage: Arc<AtomicUsize>,
    result: Arc<Mutex<Option<Result<SoundReport, String>>>>,
}

fn job_id() -> egui::Id {
    egui::Id::new("reverse-sound-job")
}

fn after_shots_id() -> egui::Id {
    egui::Id::new("reverse-sound-after-shots")
}

pub(super) fn awaiting_shots(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(after_shots_id())).unwrap_or(false)
}

pub(super) fn is_running(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<Job>(job_id())).is_some() || awaiting_shots(ctx)
}

pub(super) fn current(app: &FilmcraftApp) -> bool {
    let s = &app.ui.panels.reverse_engineering;
    s.sound_report.as_ref().is_some_and(|r| {
        r.project_revision == app.session.revision
            && r.options == s.sound_options
            && !r.shots.is_empty()
            && reverse_engineering::captures_are_current(app, &r.captures)
    })
}

fn limitations(analysis: &Analysis, dialogue: bool) -> Vec<String> {
    let mut out = analysis.limitations.clone();
    out.push("Shots come from the Shots & look result when it is current for the same range, else the range is one shot; a shot's sound is the share of each kind, the loudness and the beats inside its time span.".into());
    out.push(if dialogue {
        "Dialogue lines come from the speech model named in Settings ▸ Media Analysis & Transcription, run on a 16 kHz mono downmix of the range; words carry the recogniser's confidence and speaker labels are its clustering, not identities.".into()
    } else {
        "Dialogue was not transcribed (switched off or no speech model installed), so lines are absent even where speech is detected.".into()
    });
    out
}

fn timestamp() -> Result<u64, String> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())
        .and_then(|d| u64::try_from(d.as_millis()).map_err(|e| e.to_string()))
}

/// The shots to describe: from a current Shots & look result over the same captures, else one.
fn shot_plan(app: &FilmcraftApp, captures: &[ReverseEngineeringCapture]) -> (bool, Vec<(usize, usize, usize)>) {
    let last = captures.len().saturating_sub(1);
    if super::reverse_shots::current(app)
        && let Some(report) = app.ui.panels.reverse_engineering.shots.as_ref()
        && report.captures == captures
        && !report.shots.is_empty()
    {
        return (true, report.shots.iter().map(|s| (s.start_frame.min(last), s.end_frame.min(last), s.keyframe.min(last))).collect());
    }
    (false, vec![(0, last, last / 2)])
}

pub(super) fn poll(app: &mut FilmcraftApp, ctx: &egui::Context) {
    if awaiting_shots(ctx) {
        if super::reverse_shots::is_running(ctx) {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            return;
        }
        ctx.data_mut(|d| d.remove::<bool>(after_shots_id()));
        if super::reverse_shots::current(app) {
            if let Err(e) = start(app, ctx) {
                app.ui.status = e.clone();
                ctx.data_mut(|d| d.insert_temp(egui::Id::new("reverse-tools-error"), e));
            }
        } else if ctx.data(|d| d.get_temp::<String>(egui::Id::new("reverse-tools-error"))).is_none() {
            app.ui.status = "Shot detection did not finish, so the sound was not analysed".into();
        }
        return;
    }
    let Some(job) = ctx.data(|d| d.get_temp::<Job>(job_id())) else {
        return;
    };
    let result = job.result.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
    if let Some(result) = result {
        ctx.data_mut(|d| d.remove::<Job>(job_id()));
        match result {
            Ok(report) if !job.cancel.load(Ordering::Relaxed) => {
                app.ui.status = report.summary.clone();
                let selected = app.ui.panels.reverse_engineering.sound_selected.min(report.shots.len().saturating_sub(1));
                app.ui.panels.reverse_engineering.sound_report = Some(report);
                let _ = select(app, ctx, selected);
            }
            Ok(_) => app.ui.status = "Sound analysis cancelled; previous result preserved".into(),
            Err(e) => {
                app.ui.status = e.clone();
                ctx.data_mut(|d| d.insert_temp(egui::Id::new("reverse-tools-error"), e));
            }
        }
    } else {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}

fn select(app: &mut FilmcraftApp, ctx: &egui::Context, index: usize) -> Result<Value, String> {
    let report = app.ui.panels.reverse_engineering.sound_report.as_ref().ok_or("Analyse the sound first")?;
    let shot = report.shots.get(index).ok_or("Shot index outside the result")?;
    let capture = report.captures.get(shot.keyframe).cloned().ok_or("Shot keyframe outside the range")?;
    let start = report.captures.get(shot.start_frame).map(|c| c.timeline_time).ok_or("Shot start outside the range")?;
    let (start_frame, end_frame) = (shot.start_frame, shot.end_frame);
    app.ui.panels.reverse_engineering.sound_selected = index;
    app.ui.panels.reverse_engineering.capture = Some(capture);
    app.stop();
    app.session.set_playhead(start);
    reverse_engineering::reset_decode_timer(ctx);
    Ok(json!({"index":index,"startFrame":start_frame,"endFrame":end_frame}))
}

fn seconds(tick: i64, start_tick: i64) -> f64 {
    (tick.saturating_sub(start_tick)) as f64 / TICKS_PER_SECOND as f64
}

/// A transcript's words as dialogue words in seconds from the analysed start.
fn transcript_words(transcript: &filmcraft_project::Transcript) -> Vec<Word> {
    transcript
        .words
        .iter()
        .take(filmcraft_sound::dialogue::MAX_WORDS)
        .map(|w| Word {
            text: w.text.clone(),
            start_seconds: w.start.0 as f64 / TICKS_PER_SECOND as f64,
            end_seconds: w.end.0 as f64 / TICKS_PER_SECOND as f64,
            speaker: w.speaker,
            confidence: w.confidence,
        })
        .collect()
}

fn start(app: &mut FilmcraftApp, ctx: &egui::Context) -> Result<Value, String> {
    if is_running(ctx) {
        return Err("Sound analysis is already running".into());
    }
    if cfg!(target_arch = "wasm32") {
        return Err("Sound analysis currently requires the desktop app.".into());
    }
    let captures = reverse_engineering::range_captures(app, MAX_RANGE_FRAMES)?;
    let (first, last) = match (captures.first(), captures.last()) {
        (Some(f), Some(l)) => (f.clone(), l.clone()),
        _ => return Err("No frames selected".into()),
    };
    let options = app.ui.panels.reverse_engineering.sound_options.clone();
    options.analysis.validate()?;
    if !super::reverse_shots::current(app) {
        if !super::reverse_shots::is_running(ctx) {
            super::reverse_shots::start(app, ctx, None)?;
        }
        ctx.data_mut(|d| d.insert_temp(after_shots_id(), true));
        app.ui.status = "Detecting shots first; the sound is then read per shot".into();
        return Ok(json!({"started":true,"detectingShots":true}));
    }
    let (from_shots, plan) = shot_plan(app, &captures);
    let sequence = app.session.active_sequence().ok_or("Open a sequence")?;
    let seq_id = app.session.state.active_sequence.ok_or("No active sequence")?;
    let sample_rate = sequence.settings.sample_rate.max(1);
    let frame = sequence.settings.frame_rate.frame_duration();
    let start_tick = first.timeline_time;
    let end_tick = last.timeline_time + frame;
    if end_tick <= start_tick {
        return Err("The range is empty".into());
    }
    let start_sample = start_tick.to_units_floor(sample_rate as i64).max(0);
    let end_sample = end_tick.to_units_floor(sample_rate as i64).max(start_sample);
    let total_samples = usize::try_from(end_sample - start_sample).map_err(|_| "Range too long")?;
    if total_samples == 0 {
        return Err("The range holds no audio samples".into());
    }
    if total_samples > (sample_rate as usize).saturating_mul(filmcraft_sound::MAX_SECONDS as usize) {
        return Err(format!("Choose a range of at most {} seconds of audio", filmcraft_sound::MAX_SECONDS));
    }
    let transcriber = if options.dialogue { filmcraft_engine::transcript::speech_transcriber(&app.session, None).ok() } else { None };
    let dialogue_skipped = match (options.dialogue, transcriber.is_some()) {
        (false, _) => Some("Dialogue transcription switched off in the options".to_string()),
        (true, false) => Some("No speech model is available (install one in Settings ▸ Media Analysis & Transcription)".to_string()),
        (true, true) => None,
    };
    let speech_options = filmcraft_engine::transcript::speech_options(&app.session);
    let revision = app.session.revision;
    let project = app.session.project.clone();
    let provider = app.session.media.provider(project.clone(), app.session.services.clone());
    let hops = total_samples.div_ceil(filmcraft_sound::features::HOP).max(1);
    let job = Job {
        cancel: Arc::new(AtomicBool::new(false)),
        progress: Arc::new(AtomicUsize::new(0)),
        total: hops,
        stage: Arc::new(AtomicUsize::new(0)),
        result: Arc::new(Mutex::new(None)),
    };
    let worker = job.clone();
    let repaint = ctx.clone();
    std::thread::Builder::new()
        .name("filmcraft-sound".into())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<SoundReport, String> {
                let started = std::time::Instant::now();
                let seq = project.sequence(seq_id).ok_or("The sequence is no longer available")?;
                let mut channels: Vec<Vec<f32>> = Vec::new();
                let mut done = 0usize;
                while done < total_samples {
                    if worker.cancel.load(Ordering::Relaxed) {
                        return Err("Sound analysis cancelled".into());
                    }
                    if started.elapsed().as_secs() > 1800 {
                        return Err("Sound analysis timed out; choose a shorter range".into());
                    }
                    let n = MIX_CHUNK.min(total_samples - done);
                    let buf = filmcraft_render::audio::mix_sequence(&project, seq, start_sample.saturating_add(done as i64), n, &provider);
                    if channels.is_empty() {
                        channels = vec![Vec::with_capacity(total_samples); buf.channels.len().clamp(1, filmcraft_sound::MAX_CHANNELS)];
                    }
                    for (c, dst) in channels.iter_mut().enumerate() {
                        let src = buf.channels.get(c).map(Vec::as_slice).unwrap_or_default();
                        dst.extend((0..n).map(|i| src.get(i).copied().unwrap_or(0.0)));
                    }
                    done += n;
                    worker.progress.store(done.div_ceil(filmcraft_sound::features::HOP), Ordering::Relaxed);
                    repaint.request_repaint();
                }
                worker.stage.store(1, Ordering::Relaxed);
                let slices: Vec<&[f32]> = channels.iter().map(Vec::as_slice).collect();
                let cancel = worker.cancel.clone();
                let progress = worker.progress.clone();
                let paint = repaint.clone();
                let mut report_progress = |h: usize, _: usize| {
                    progress.store(h, Ordering::Relaxed);
                    if h.is_multiple_of(512) {
                        paint.request_repaint();
                    }
                    !cancel.load(Ordering::Relaxed)
                };
                let analysis = filmcraft_sound::analyze(&slices, sample_rate, &options.analysis, &mut report_progress)?;
                let mut dialogue = Vec::new();
                let mut dialogue_skipped = dialogue_skipped;
                if let Some(t) = transcriber.as_ref() {
                    worker.stage.store(2, Ordering::Relaxed);
                    repaint.request_repaint();
                    let speech_rate = filmcraft_speech::SAMPLE_RATE;
                    let mono = filmcraft_sound::downmix(&slices, total_samples);
                    let out_len = usize::try_from((total_samples as u64).saturating_mul(u64::from(speech_rate)) / u64::from(sample_rate)).unwrap_or(0);
                    let mut resampler = filmcraft_audio_dsp::resample::StreamResampler::new(sample_rate, speech_rate);
                    let resampled = resampler.process(0, out_len, |start, frames| {
                        let start = usize::try_from(start.max(0)).unwrap_or(0);
                        vec![(0..frames).map(|i| mono.get(start + i).copied().unwrap_or(0.0)).collect()]
                    });
                    let audio = resampled.into_iter().next().unwrap_or_default();
                    let cancel = worker.cancel.clone();
                    match t.transcribe(&audio, &speech_options, &mut |_, _| !cancel.load(Ordering::Relaxed)) {
                        Ok(mut transcript) => {
                            transcript.normalize();
                            dialogue = filmcraft_sound::dialogue::lines(&transcript_words(&transcript), LINE_GAP_SECONDS);
                            if dialogue.is_empty() {
                                dialogue_skipped = Some("The speech model found no words in the range".into());
                            }
                        }
                        Err(e) => {
                            if worker.cancel.load(Ordering::Relaxed) {
                                return Err("Sound analysis cancelled".into());
                            }
                            dialogue_skipped = Some(format!("Dialogue transcription failed: {e}"));
                        }
                    }
                }
                let origin = start_tick.0;
                let mut shots = Vec::new();
                let mut cuts = Vec::new();
                for (index, (start, end, keyframe)) in plan.into_iter().enumerate() {
                    let shot_start = captures.get(start).map(|c| c.timeline_time.0).ok_or("Shot start outside the range")?;
                    let shot_end = captures.get(end).map(|c| c.timeline_time.0 + frame.0).ok_or("Shot end outside the range")?;
                    let (a, b) = (seconds(shot_start, origin), seconds(shot_end, origin));
                    if index > 0 {
                        cuts.push(a);
                    }
                    let sound = filmcraft_sound::range_sound(&analysis, a, b);
                    let lines = dialogue.iter().filter(|l| l.start_seconds >= a && l.start_seconds < b).cloned().collect();
                    shots.push(SoundShot { index, start_frame: start, end_frame: end, start_tick: shot_start, end_tick: shot_end, keyframe, sound, lines });
                }
                let cuts_on_beats = match analysis.tempo.as_ref() {
                    Some(t) if !cuts.is_empty() => Some(filmcraft_sound::tempo::cuts_on_beats(&cuts, &t.beats, options.analysis.beat_tolerance_seconds)),
                    _ => None,
                };
                let mut summary = analysis.summary.clone();
                if let Some(c) = &cuts_on_beats {
                    summary.push_str(&format!(" · {} of {} cuts on a beat", c.on_beat, c.total));
                }
                if !dialogue.is_empty() {
                    summary.push_str(&format!(" · {} dialogue line{}", dialogue.len(), if dialogue.len() == 1 { "" } else { "s" }));
                }
                let limitations = limitations(&analysis, transcriber.is_some());
                Ok(SoundReport {
                    schema_version: 1,
                    analyzer: analysis.analyzer.clone(),
                    captures,
                    project_revision: revision,
                    completed_at_unix_ms: timestamp()?,
                    options,
                    start_tick: origin,
                    end_tick: end_tick.0,
                    from_shots,
                    analysis,
                    shots,
                    cuts_on_beats,
                    dialogue,
                    dialogue_skipped,
                    summary,
                    limitations,
                })
            }))
            .unwrap_or_else(|_| Err("The sound worker failed; previous results preserved".into()));
            *worker.result.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(outcome);
            repaint.request_repaint();
        })
        .map_err(|e| format!("Could not start the sound analysis: {e}"))?;
    ctx.data_mut(|d| d.insert_temp(job_id(), job));
    Ok(json!({"started":true,"samples":total_samples,"sampleRate":sample_rate}))
}

fn frame_rate(app: &FilmcraftApp) -> f64 {
    app.session.active_sequence().map(|q| q.settings.frame_rate.as_f64()).unwrap_or(0.0)
}

fn sequence_name(app: &FilmcraftApp) -> String {
    app.session.state.active_sequence.and_then(|id| app.session.project.item(id)).map(|i| i.name.clone()).unwrap_or_default()
}

fn sound_file(app: &FilmcraftApp, report: &SoundReport) -> Result<SoundFile, String> {
    app.session.active_sequence().ok_or("Open a sequence")?;
    let state = &app.ui.panels.reverse_engineering;
    let a = &report.analysis;
    Ok(SoundFile {
        format: SOUND_FORMAT.into(),
        version: 1,
        source: SoundSource {
            sequence: sequence_name(app),
            sample_rate: a.sample_rate,
            channels: a.channels,
            frame_rate: frame_rate(app),
            start_frame: state.start_frame,
            end_frame: state.end_frame,
            start_seconds: report.start_tick as f64 / TICKS_PER_SECOND as f64,
            seconds: a.seconds,
            analyzer: report.analyzer.clone(),
        },
        summary: report.summary.clone(),
        loudness: a.loudness.clone(),
        shares: a.shares,
        tempo: a.tempo.clone(),
        cuts_on_beats: report.cuts_on_beats.clone(),
        regions: a.regions.clone(),
        segments: a.segments.clone(),
        shots: report.shots.clone(),
        dialogue: report.dialogue.clone(),
        rules: a.rules.clone(),
        limitations: report.limitations.clone(),
    })
}

fn current_report(app: &FilmcraftApp) -> Result<SoundReport, String> {
    if !current(app) {
        return Err("Analyse the sound first; the result must be current".into());
    }
    app.ui.panels.reverse_engineering.sound_report.clone().ok_or_else(|| "No sound result".into())
}

fn export_sound(app: &mut FilmcraftApp, params: &Value) -> Result<Value, String> {
    let report = current_report(app)?;
    let file = sound_file(app, &report)?;
    let path = if let Some(path) = params.get("path") {
        Some(path.as_str().filter(|s| !s.trim().is_empty()).ok_or("Supply a nonempty export path")?.to_owned())
    } else {
        app.hooks.pick_save_as.as_mut().ok_or("No save dialog; supply path")?("Sound", &["json"], "sound.sound.json")
    };
    let Some(path) = path else {
        return Ok(Value::Null);
    };
    let bytes = serde_json::to_vec_pretty(&file).map_err(|e| e.to_string())?;
    std::fs::write(&path, bytes).map_err(|e| format!("Could not export the sound: {e}"))?;
    app.ui.status = format!("Exported sound ({} shots, {} dialogue lines) to {path}", file.shots.len(), file.dialogue.len());
    Ok(json!({"path":path,"shots":file.shots.len(),"lines":file.dialogue.len()}))
}

fn copy_description(app: &mut FilmcraftApp, ctx: &egui::Context) -> Result<Value, String> {
    let report = current_report(app)?;
    let file = sound_file(app, &report)?;
    let description = serde_json::to_value(&file).map_err(|e| e.to_string())?;
    ctx.copy_text(serde_json::to_string_pretty(&description).map_err(|e| e.to_string())?);
    app.ui.status = format!("Sound description copied ({} shots)", file.shots.len());
    Ok(description)
}

/// Put a timeline marker on every beat of the grid inside the range (`markers.add`).
fn beat_markers(app: &mut FilmcraftApp) -> Result<Value, String> {
    let report = current_report(app)?;
    let tempo = report.analysis.tempo.as_ref().ok_or("No steady tempo was found, so there are no beats to mark")?;
    let bpm = tempo.bpm;
    let mut added = 0usize;
    for (i, beat) in tempo.beats.iter().enumerate().take(4096) {
        let tick = report.start_tick.saturating_add((beat * TICKS_PER_SECOND as f64).round() as i64);
        if tick >= report.end_tick {
            break;
        }
        app.session
            .execute("markers.add", json!({"time": tick, "name": format!("Beat {} · {bpm:.0} BPM", i + 1), "color": "cyan"}))
            .map_err(|e| e.to_string())?;
        added += 1;
    }
    if let Some(r) = app.ui.panels.reverse_engineering.sound_report.as_mut() {
        r.project_revision = app.session.revision;
    }
    app.ui.status = format!("{added} beat markers added at {bpm:.0} BPM");
    Ok(json!({"markers":added,"bpm":bpm}))
}

fn set_options(app: &mut FilmcraftApp, params: &Value) -> Result<Value, String> {
    let mut options = app.ui.panels.reverse_engineering.sound_options.clone();
    if let Some(v) = params.get("segmentSeconds") {
        options.analysis.segment_seconds = v.as_f64().ok_or("segmentSeconds must be a number")?;
    }
    if let Some(v) = params.get("minBpm") {
        options.analysis.min_bpm = v.as_f64().ok_or("minBpm must be a number")?;
    }
    if let Some(v) = params.get("maxBpm") {
        options.analysis.max_bpm = v.as_f64().ok_or("maxBpm must be a number")?;
    }
    if let Some(v) = params.get("beatTolerance") {
        options.analysis.beat_tolerance_seconds = v.as_f64().ok_or("beatTolerance must be a number")?;
    }
    if let Some(v) = params.get("dialogue") {
        options.dialogue = v.as_bool().ok_or("dialogue must be true or false")?;
    }
    options.analysis.validate()?;
    app.ui.panels.reverse_engineering.sound_options = options.clone();
    serde_json::to_value(&options).map_err(|e| e.to_string())
}

pub(super) fn command(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str, params: &Value) -> Result<Value, String> {
    if id == "reverseEngineering.cancelSound" {
        if awaiting_shots(ctx) {
            ctx.data_mut(|d| d.remove::<bool>(after_shots_id()));
            super::reverse_shots::cancel(ctx);
        }
        if let Some(job) = ctx.data(|d| d.get_temp::<Job>(job_id())) {
            job.cancel.store(true, Ordering::Relaxed);
        }
        return Ok(json!({"cancelled":true}));
    }
    if is_running(ctx) {
        return Err("Wait for the sound analysis to finish or cancel it first".into());
    }
    match id {
        "reverseEngineering.runSound" => start(app, ctx),
        "reverseEngineering.soundShot" => {
            let index = params.get("index").and_then(Value::as_u64).and_then(|v| usize::try_from(v).ok()).ok_or("Supply a shot index")?;
            if !current(app) {
                return Err("Analyse the sound for the current range before selecting a shot".into());
            }
            select(app, ctx, index)
        }
        "reverseEngineering.setSoundOptions" => set_options(app, params),
        "reverseEngineering.exportSound" => export_sound(app, params),
        "reverseEngineering.copySoundDescription" => copy_description(app, ctx),
        "reverseEngineering.soundBeatMarkers" => beat_markers(app),
        _ => Err(format!("Unknown sound command: {id}")),
    }
}

fn invoke(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str, params: Value) {
    if let Err(e) = crate::menus::invoke(app, ctx, id, params) {
        app.ui.status = e.clone();
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("reverse-tools-error"), e));
    } else {
        ctx.data_mut(|d| d.remove::<String>(egui::Id::new("reverse-tools-error")));
    }
}

fn badge(ui: &mut egui::Ui, text: &str, fill: egui::Color32) -> egui::Response {
    egui::Frame::new()
        .fill(fill)
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).color(egui::Color32::WHITE).size(11.0));
        })
        .response
}

fn kind_color(kind: SegmentKind) -> egui::Color32 {
    match kind {
        SegmentKind::Silence => egui::Color32::from_gray(70),
        SegmentKind::Speech => egui::Color32::from_rgb(70, 140, 220),
        SegmentKind::Music => egui::Color32::from_rgb(170, 90, 200),
        SegmentKind::Effects => egui::Color32::from_rgb(220, 140, 50),
        SegmentKind::Ambience => egui::Color32::from_rgb(70, 160, 120),
    }
}

/// The range's regions as a coloured strip with the loudness curve under it and the beats as
/// ticks; `start`/`end` in seconds select the span drawn.
fn strip(app: &mut FilmcraftApp, ui: &mut egui::Ui, report: &SoundReport, start: f64, end: f64, id: &str) {
    let width = ui.available_width().max(1.0);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 54.0), egui::Sense::hover());
    app.auto.add(id, rect, "Sound regions, loudness curve and beats over time");
    let painter = ui.painter_at(rect);
    let span = (end - start).max(1e-6);
    let x_of = |t: f64| rect.left() + (((t - start) / span).clamp(0.0, 1.0) as f32) * rect.width();
    let regions_rect = egui::Rect::from_min_max(rect.min, egui::pos2(rect.right(), rect.top() + 16.0));
    painter.rect_filled(regions_rect, 2.0, egui::Color32::from_gray(40));
    for r in &report.analysis.regions {
        let (a, b) = (r.start_seconds.max(start), r.end_seconds.min(end));
        if b <= a {
            continue;
        }
        let rr = egui::Rect::from_min_max(egui::pos2(x_of(a), regions_rect.top()), egui::pos2(x_of(b), regions_rect.bottom()));
        painter.rect_filled(rr, 0.0, kind_color(r.kind));
    }
    let curve_rect = egui::Rect::from_min_max(egui::pos2(rect.left(), regions_rect.bottom() + 2.0), egui::pos2(rect.right(), rect.bottom() - 6.0));
    painter.rect_filled(curve_rect, 2.0, egui::Color32::from_gray(28));
    let step = report.analysis.loudness.curve_seconds;
    if step > 0.0 {
        let mut points = Vec::new();
        let first = (start / step).floor().max(0.0) as usize;
        let last = ((end / step).ceil().max(0.0) as usize).min(report.analysis.loudness.momentary_curve.len());
        for (i, v) in report.analysis.loudness.momentary_curve.iter().enumerate().skip(first).take(last.saturating_sub(first)) {
            let t = i as f64 * step;
            let y = ((f64::from(*v).clamp(-60.0, 0.0) + 60.0) / 60.0) as f32;
            points.push(egui::pos2(x_of(t), curve_rect.bottom() - y * curve_rect.height()));
        }
        if points.len() > 1 {
            painter.add(egui::Shape::line(points, egui::Stroke::new(1.0, egui::Color32::from_rgb(120, 200, 120))));
        }
    }
    if let Some(t) = &report.analysis.tempo {
        for b in t.beats.iter().filter(|b| **b >= start && **b <= end) {
            let x = x_of(*b);
            painter.line_segment(
                [egui::pos2(x, rect.bottom() - 6.0), egui::pos2(x, rect.bottom())],
                egui::Stroke::new(1.0, egui::Color32::from_rgb(90, 200, 220)),
            );
        }
    }
    for s in &report.shots {
        let a = seconds(s.start_tick, report.start_tick);
        if a > start && a < end {
            let x = x_of(a);
            painter.line_segment([egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())], egui::Stroke::new(1.0, egui::Color32::from_gray(230)));
        }
    }
    if let Some(pos) = response.hover_pos() {
        let t = start + ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64 * span;
        if let Some(seg) = report.analysis.segments.iter().find(|s| t >= s.start_seconds && t < s.end_seconds) {
            response.on_hover_text(format!(
                "{:.1} s · {} ({:.0} % sure) · {:.1} dBFS · flatness {:.2} · modulation {:.1} dB",
                t,
                seg.kind.label(),
                seg.confidence * 100.0,
                seg.mean_db,
                seg.flatness,
                seg.modulation_db
            ));
        }
    }
}

fn legend(ui: &mut egui::Ui, shares: &filmcraft_sound::Shares) {
    ui.horizontal_wrapped(|ui| {
        for kind in [SegmentKind::Speech, SegmentKind::Music, SegmentKind::Effects, SegmentKind::Ambience, SegmentKind::Silence] {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 2.0, kind_color(kind));
            ui.label(egui::RichText::new(format!("{} {:.0} %", kind.label(), shares.get(kind) * 100.0)).size(11.0));
        }
    });
}

fn loudness_line(a: &Analysis) -> String {
    let l = &a.loudness;
    match l.integrated_lufs {
        Some(i) => format!(
            "{i:.1} LUFS integrated · range {:.1} LU · true peak {} · momentary max {}",
            l.range_lu,
            l.true_peak_dbtp.map(|p| format!("{p:.1} dBTP")).unwrap_or_else(|| "silent".into()),
            l.max_momentary_lufs.map(|p| format!("{p:.1} LUFS")).unwrap_or_else(|| "silent".into())
        ),
        None => "Below the loudness gate: the range is silent".into(),
    }
}

/// The Sound page: header, verdict, actions, strip, shots with their dialogue.
pub(super) fn page(app: &mut FilmcraftApp, ui: &mut egui::Ui, bounds: egui::Rect) {
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(bounds));
    child.set_clip_rect(bounds.intersect(ui.clip_rect()));
    child.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
    egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt("reverse-sound-page").show(&mut child, |ui| {
        ui.set_width(bounds.width().max(1.0));
        page_body(app, ui, bounds);
    });
}

fn page_body(app: &mut FilmcraftApp, ui: &mut egui::Ui, bounds: egui::Rect) {
    let running = ui.ctx().data(|d| d.get_temp::<Job>(job_id()));
    let awaiting = awaiting_shots(ui.ctx());
    let shots_current = super::reverse_shots::current(app);
    let captures = reverse_engineering::range_captures(app, MAX_RANGE_FRAMES);
    let count = captures.as_ref().map(|c| c.len()).unwrap_or(0);
    let is_current = current(app);
    let saved =
        app.ui.panels.reverse_engineering.saved_results.iter().any(|r| r.tool() == "sound" && reverse_engineering::captures_are_current(app, r.captures()));
    let has_report = app.ui.panels.reverse_engineering.sound_report.is_some();
    let status = if awaiting {
        ("Detecting shots…", egui::Color32::from_rgb(90, 110, 170))
    } else if running.is_some() {
        ("Analysing…", egui::Color32::from_rgb(90, 110, 170))
    } else if is_current && saved {
        ("Analysed · saved", egui::Color32::from_rgb(40, 130, 90))
    } else if is_current {
        ("Analysed", egui::Color32::from_rgb(40, 130, 90))
    } else if has_report {
        ("Stale · analyse again", egui::Color32::from_rgb(170, 120, 40))
    } else {
        ("Not analysed", egui::Color32::from_gray(90))
    };
    let fps = frame_rate(app);
    let context = {
        let state = &app.ui.panels.reverse_engineering;
        match captures.as_ref().ok().and_then(|c| c.first()) {
            Some(first) => format!("{} · frames {}–{} · {count} frames · {fps:.3} fps", first.name, state.start_frame, state.end_frame),
            None => String::new(),
        }
    };
    let busy = running.is_some() || awaiting;
    ui.horizontal(|ui| {
        let response = ui.add_enabled(!busy, egui::Button::new("← Tools"));
        app.auto.add("reverseEngineering.tool.back", response.rect, "Back to tool menu");
        if response.clicked() {
            invoke(app, ui.ctx(), "reverseEngineering.chooseTool", json!({"tool":"menu"}));
        }
        ui.heading("Sound");
        let response = badge(ui, status.0, status.1);
        app.auto.add("reverseEngineering.status", response.rect, status.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(&context).color(app.tokens.text_faint));
        });
    });
    if awaiting {
        ui.horizontal(|ui| {
            ui.spinner();
            let text = match super::reverse_shots::progress(ui.ctx()) {
                Some((0, done, total)) => format!("Detecting shots first · reading frame {done} / {total}"),
                Some((_, done, _)) => format!("Detecting shots first · shot keyframe {done}"),
                None => "Detecting shots first…".to_string(),
            };
            ui.label(text);
            let response = ui.button("Cancel");
            app.auto.add("reverseEngineering.cancelSound", response.rect, "Cancel the shot detection and the sound analysis");
            if response.clicked() {
                invoke(app, ui.ctx(), "reverseEngineering.cancelSound", json!({}));
            }
        });
        ui.label(egui::RichText::new("The sound is read shot by shot, so the range's shots are detected first.").color(app.tokens.text_faint));
        return;
    }
    if let Some(job) = running {
        ui.horizontal(|ui| {
            ui.spinner();
            let text = match job.stage.load(Ordering::Relaxed) {
                0 => format!("Mixing the range · {} / {}", job.progress.load(Ordering::Relaxed), job.total),
                1 => format!("Analysing · hop {} / {}", job.progress.load(Ordering::Relaxed), job.total),
                _ => "Transcribing the dialogue…".to_string(),
            };
            ui.label(text);
            let response = ui.button("Cancel");
            app.auto.add("reverseEngineering.cancelSound", response.rect, "Cancel sound analysis");
            if response.clicked() {
                invoke(app, ui.ctx(), "reverseEngineering.cancelSound", json!({}));
            }
        });
        return;
    }
    if let Err(e) = &captures {
        ui.label(e);
    }
    if let Some(error) = ui.ctx().data(|d| d.get_temp::<String>(egui::Id::new("reverse-tools-error"))) {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
    }
    let report = app.ui.panels.reverse_engineering.sound_report.clone().filter(|_| is_current);
    let (headline, quality) = match &report {
        Some(r) => (
            r.summary.clone(),
            format!(
                "{} shot{}{} · {}",
                r.shots.len(),
                if r.shots.len() == 1 { "" } else { "s" },
                if r.from_shots { "" } else { " (shots not detected: the whole range as one)" },
                loudness_line(&r.analysis)
            ),
        ),
        None if has_report => {
            ("Options, range or project changed: analyse again to read the sound.".to_string(), "The previous result is kept hidden until then.".to_string())
        }
        None => (
            "Not analysed yet.".to_string(),
            "Analyse mixes the range's audio and reads its loudness, what each second is (speech, music, effects, ambience, silence), the tempo and beats, and the dialogue when a speech model is installed.".to_string(),
        ),
    };
    egui::Frame::group(ui.style()).inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let response = ui.label(egui::RichText::new(&headline).strong().size(15.0));
        app.auto.add("reverseEngineering.verdict", response.rect, &headline);
        let response = ui.label(egui::RichText::new(&quality).color(app.tokens.text_dim));
        app.auto.add("reverseEngineering.quality", response.rect, &quality);
        if let Some(r) = &report {
            legend(ui, &r.analysis.shares);
            strip(app, ui, r, 0.0, r.analysis.seconds, "reverseEngineering.sound.strip");
            if let Some(skipped) = &r.dialogue_skipped {
                ui.label(egui::RichText::new(skipped).color(app.tokens.text_faint).size(11.0));
            }
            ui.horizontal_wrapped(|ui| {
                let response = ui.button("Copy description");
                app.auto.add(
                    "reverseEngineering.copySoundDescription",
                    response.rect,
                    "Copy the complete sound description as JSON: loudness, segments, regions, tempo, beats, shots, dialogue and the rules behind the words",
                );
                if response.clicked() {
                    invoke(app, ui.ctx(), "reverseEngineering.copySoundDescription", json!({}));
                }
                ui.label(
                    egui::RichText::new("the words above are labels; the numbers and their rules travel in the description")
                        .color(app.tokens.text_faint)
                        .size(11.0),
                );
            });
        }
    });
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        if report.is_some() {
            let open = app.ui.panels.reverse_engineering.details_open;
            let response = ui.selectable_label(open, if open { "▾ Details" } else { "▸ Details" });
            app.auto.add("reverseEngineering.details", response.rect, "Every number behind the words");
            if response.clicked() {
                app.ui.panels.reverse_engineering.details_open = !open;
            }
        }
        let open = app.ui.panels.reverse_engineering.advanced_open;
        let response = ui.selectable_label(open, if open { "▾ Advanced" } else { "▸ Advanced" });
        app.auto.add("reverseEngineering.advanced", response.rect, "Segment length, tempo range, beat tolerance and dialogue");
        if response.clicked() {
            app.ui.panels.reverse_engineering.advanced_open = !open;
        }
        ui.separator();
        let label = if is_current { "Analyse again" } else { "Analyse" };
        let response = ui.add_enabled(count > 0, egui::Button::new(egui::RichText::new(label).strong()));
        app.auto.add(
            "reverseEngineering.runSound",
            response.rect,
            if shots_current {
                "Mix the range's audio and read loudness, speech / music / effects / ambience, tempo, beats and dialogue per shot"
            } else {
                "Detect the shots of the range, then read the sound of every one of them"
            },
        );
        if response.clicked() {
            invoke(app, ui.ctx(), "reverseEngineering.runSound", json!({}));
        }
        if is_current {
            let response = ui.button("Export sound…");
            app.auto.add(
                "reverseEngineering.sound.export",
                response.rect,
                "Save the loudness, segments, tempo, beats, shots, dialogue and rules as a sound file",
            );
            if response.clicked() {
                invoke(app, ui.ctx(), "reverseEngineering.exportSound", json!({}));
            }
            let has_tempo = report.as_ref().is_some_and(|r| r.analysis.tempo.is_some());
            let response = ui.add_enabled(has_tempo, egui::Button::new("Beat markers"));
            app.auto.add("reverseEngineering.soundBeatMarkers", response.rect, "Add a timeline marker on every beat of the grid inside the range");
            if response.clicked() {
                invoke(app, ui.ctx(), "reverseEngineering.soundBeatMarkers", json!({}));
            }
        }
    });
    if is_current {
        super::reverse_tools::result_actions(app, ui, "sound", false);
    }
    if app.ui.panels.reverse_engineering.advanced_open {
        let mut o = app.ui.panels.reverse_engineering.sound_options.clone();
        let mut changed = false;
        ui.horizontal_wrapped(|ui| {
            ui.label("Segment");
            let r = ui.add(egui::DragValue::new(&mut o.analysis.segment_seconds).range(0.25..=10.0).speed(0.05).suffix(" s"));
            app.auto.add("reverseEngineering.sound.segmentSeconds", r.rect, "Length of each classified segment");
            changed |= r.changed();
            ui.label("Tempo");
            let r = ui.add(egui::DragValue::new(&mut o.analysis.min_bpm).range(30.0..=300.0).speed(1.0));
            app.auto.add("reverseEngineering.sound.minBpm", r.rect, "Slowest tempo searched, BPM");
            changed |= r.changed();
            ui.label("–");
            let r = ui.add(egui::DragValue::new(&mut o.analysis.max_bpm).range(30.0..=300.0).speed(1.0).suffix(" BPM"));
            app.auto.add("reverseEngineering.sound.maxBpm", r.rect, "Fastest tempo searched, BPM");
            changed |= r.changed();
            ui.label("Beat tolerance");
            let mut ms = o.analysis.beat_tolerance_seconds * 1000.0;
            let r = ui.add(egui::DragValue::new(&mut ms).range(0.0..=1000.0).speed(1.0).suffix(" ms"));
            app.auto.add("reverseEngineering.sound.beatTolerance", r.rect, "A cut counts as on a beat within this distance of one");
            if r.changed() {
                o.analysis.beat_tolerance_seconds = ms / 1000.0;
                changed = true;
            }
            let r = ui.checkbox(&mut o.dialogue, "Transcribe dialogue");
            app.auto.add("reverseEngineering.sound.dialogue", r.rect, "Run the speech model on the range for dialogue lines");
            changed |= r.changed();
        });
        if changed {
            invoke(
                app,
                ui.ctx(),
                "reverseEngineering.setSoundOptions",
                json!({"segmentSeconds":o.analysis.segment_seconds,"minBpm":o.analysis.min_bpm,"maxBpm":o.analysis.max_bpm,"beatTolerance":o.analysis.beat_tolerance_seconds,"dialogue":o.dialogue}),
            );
        }
    }
    let Some(report) = report else { return };
    ui.add_space(6.0);
    let selected = app.ui.panels.reverse_engineering.sound_selected.min(report.shots.len().saturating_sub(1));
    let details = app.ui.panels.reverse_engineering.details_open;
    let wide = bounds.width() >= 700.0;
    let list = |app: &mut FilmcraftApp, ui: &mut egui::Ui| {
        for shot in &report.shots {
            let title = format!(
                "Shot {} · frames {}–{} · {} · {}",
                shot.index + 1,
                shot.start_frame + 1,
                shot.end_frame + 1,
                shot.sound.dominant.label(),
                shot.sound.summary
            );
            let response = ui.selectable_label(selected == shot.index, &title);
            app.auto.add(&format!("reverseEngineering.sound.shot.{}", shot.index), response.rect, &title);
            if response.clicked() {
                invoke(app, ui.ctx(), "reverseEngineering.soundShot", json!({"index":shot.index}));
            }
            if selected == shot.index {
                let (a, b) = (seconds(shot.start_tick, report.start_tick), seconds(shot.end_tick, report.start_tick));
                strip(app, ui, &report, a, b, &format!("reverseEngineering.sound.shot.{}.strip", shot.index));
                for line in &shot.lines {
                    let speaker = line.speaker.map(|s| format!("Speaker {} · ", s + 1)).unwrap_or_default();
                    let text = format!("{speaker}{:.1}–{:.1} s · “{}”", line.start_seconds, line.end_seconds, line.text);
                    let r = ui.label(egui::RichText::new(&text).color(app.tokens.text_dim).size(11.5));
                    app.auto.add(&format!("reverseEngineering.sound.shot.{}.line.{}", shot.index, line.index), r.rect, &text);
                }
                if details && let Some(m) = shot.sound.mean_lufs {
                    ui.label(format!(
                        "{m:.1} LUFS mean · momentary max {} · {} beats",
                        shot.sound.max_momentary_lufs.map(|p| format!("{p:.1} LUFS")).unwrap_or_else(|| "silent".into()),
                        shot.sound.beats
                    ));
                }
            }
        }
    };
    let keyframe = report.shots.get(selected).and_then(|s| report.captures.get(s.keyframe)).cloned();
    if wide {
        let gap = 12.0;
        let left_w = ((bounds.width() - gap) * 0.6).max(1.0);
        let right_w = (bounds.width() - gap - left_w).max(1.0);
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(left_w);
                list(app, ui);
            });
            ui.add_space(gap);
            ui.vertical(|ui| {
                ui.set_width(right_w);
                if let Some(k) = &keyframe {
                    reverse_engineering::picture(app, ui, k, right_w.min(480.0) as u32, "reverseEngineering.sound.keyframe", false);
                    let caption = format!("Shot {} keyframe · {}", selected + 1, reverse_engineering::timecode(app, k));
                    ui.label(egui::RichText::new(caption).color(app.tokens.text_faint).size(11.0));
                }
            });
        });
    } else {
        if let Some(k) = &keyframe {
            reverse_engineering::picture(app, ui, k, bounds.width().min(480.0) as u32, "reverseEngineering.sound.keyframe", false);
        }
        list(app, ui);
    }
    if details {
        ui.separator();
        ui.label(egui::RichText::new("Whole range").strong());
        let a = &report.analysis;
        ui.label(loudness_line(a));
        match &a.tempo {
            Some(t) => ui.label(format!("{:.1} BPM · period {:.3} s · confidence {:.2} · {} beats", t.bpm, t.period_seconds, t.confidence, t.beats.len())),
            None => ui.label("No steady tempo"),
        };
        if let Some(c) = &report.cuts_on_beats {
            ui.label(format!("{} of {} cuts within {:.0} ms of a beat ({:.0} %)", c.on_beat, c.total, c.tolerance_seconds * 1000.0, c.share * 100.0));
        }
        ui.label(format!("Noise floor {:.1} dBFS · {} segments · {} regions", a.floor_db, a.segments.len(), a.regions.len()));
        for r in &a.regions {
            ui.label(
                egui::RichText::new(format!(
                    "{:.1}–{:.1} s · {} · {:.1} dBFS · {:.0} % sure",
                    r.start_seconds,
                    r.end_seconds,
                    r.kind.label(),
                    r.mean_db,
                    r.mean_confidence * 100.0
                ))
                .size(11.5),
            );
        }
        if !report.dialogue.is_empty() {
            ui.label(egui::RichText::new("Dialogue").strong());
            for line in &report.dialogue {
                let speaker = line.speaker.map(|s| format!("Speaker {} · ", s + 1)).unwrap_or_default();
                ui.label(egui::RichText::new(format!("{speaker}{:.1}–{:.1} s · {}", line.start_seconds, line.end_seconds, line.text)).size(11.5));
            }
        }
        ui.label(egui::RichText::new("Rules").strong());
        for r in &a.rules {
            ui.label(egui::RichText::new(format!("{}: {}", r.name, r.rule)).color(app.tokens.text_dim).size(11.0));
        }
        for l in &report.limitations {
            ui.label(egui::RichText::new(l).color(app.tokens.text_faint).size(11.0));
        }
    }
}

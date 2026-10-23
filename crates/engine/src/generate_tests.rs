use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use filmcraft_generate::{GenerateError, GenerateRequest, GeneratedClip, Limits, Phase, Progress, VideoGenerator};
use filmcraft_media::DemoScene;
use filmcraft_project::ItemId;
use filmcraft_time::Tick;
use serde_json::json;

use crate::Session;
use crate::media_test_util::{make_movie, session_with, tmp_dir};

/// A generator that copies a fixture movie to the requested output (or fails on demand).
struct Fake {
    fixture: PathBuf,
    fail: Option<String>,
    seen: Mutex<Vec<GenerateRequest>>,
}

impl VideoGenerator for Fake {
    fn id(&self) -> &str {
        "fake"
    }
    fn name(&self) -> &str {
        "Fake"
    }
    fn limits(&self) -> Limits {
        filmcraft_generate::fast_h3_limits()
    }
    fn ready(&self) -> Result<(), GenerateError> {
        Ok(())
    }
    fn generate(&self, request: &GenerateRequest, progress: &Progress) -> Result<GeneratedClip, GenerateError> {
        self.seen.lock().unwrap().push(request.clone());
        progress.set_phase(Phase::Generating);
        progress.set_detail("faking");
        if let Some(e) = &self.fail {
            return Err(GenerateError::Failed(e.clone()));
        }
        if progress.cancelled() {
            return Err(GenerateError::Cancelled);
        }
        std::fs::copy(&self.fixture, &request.output)?;
        progress.set_phase(Phase::Done);
        Ok(GeneratedClip {
            path: request.output.clone(),
            seconds: 1.0,
            width: 160,
            height: 120,
            fps: 24.0,
            provider_clip_id: "clip-1".into(),
            seed: Some(1000),
            session_seconds: 3.5,
        })
    }
}

fn session(fail: Option<&str>) -> (Session, PathBuf, Arc<Fake>) {
    let dir = tmp_dir("generate");
    let base = dir.join("base.mp4");
    make_movie(&base, DemoScene::CityNight, 160, 120, 48);
    let fixture = dir.join("fixture.mp4");
    make_movie(&fixture, DemoScene::Aurora, 160, 120, 24);
    let (mut s, _, _) = session_with(&[&base]);
    let fake = Arc::new(Fake { fixture, fail: fail.map(str::to_string), seen: Default::default() });
    s.generator = Some(fake.clone());
    (s, dir, fake)
}

#[test]
fn providers_describe_the_known_generator_and_its_limits() {
    let s = &mut Session::default();
    let r = s.execute("generate.providers", json!({})).unwrap();
    let p = &r["providers"][0];
    assert_eq!(p["id"], "reactor-fast-h3");
    assert_eq!(p["limits"]["maxSeconds"], 14.375);
    assert_eq!(p["limits"]["maxPromptChars"], 800);
    assert!(p["ready"].is_boolean());
    let r = s.execute("prefs.schema", json!({"category": "generate"})).unwrap();
    let text = r.to_string();
    assert!(text.contains("generate.reactorApiKey") && text.contains("\"secret\""), "{text}");
}

#[test]
fn a_generated_clip_is_placed_at_the_playhead_in_one_undo_step() {
    let (mut s, dir, fake) = session(None);
    let at = Tick::from_seconds_f64(0.5);
    s.set_playhead(at);
    let n0 = s.history.undo.len();
    let r = s.execute("generate.video", json!({"prompt": "a lighthouse at dusk", "seconds": 8.0, "dir": dir.to_string_lossy(), "wait": true})).unwrap();
    assert!(r["path"].as_str().unwrap().ends_with("Generated 1.mp4"), "{r}");
    assert_eq!(r["at"], at.0);
    let placed = &r["placed"];
    assert_eq!(placed["track"], "V1", "{r}");
    assert_eq!(placed["start"], at.0);
    assert_eq!(placed["sessionSeconds"], 3.5);
    let item = ItemId(placed["item"].as_u64().unwrap());
    let g = s.project.generations.get(&item).expect("the generation is recorded on the item");
    assert_eq!(g.prompt, "a lighthouse at dusk");
    assert_eq!((g.seconds, g.seed, g.provider_clip_id.as_str()), (1.0, Some(1000), "clip-1"));
    let seq = s.active_sequence().unwrap();
    let v: Vec<_> = seq.video_tracks[0].items.iter().filter(|it| it.item == item).collect();
    assert_eq!(v.len(), 1, "the clip is on V1");
    assert_eq!(v[0].start, at);
    assert!(seq.audio_tracks[0].items.iter().any(|it| it.item == item), "its audio is on A1");
    assert_eq!(s.history.undo.len(), n0 + 1, "import + placement + record collapse into one step");
    assert_eq!(s.history.undo.last().unwrap().0, "Generate Video");
    let seen = fake.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].seconds, 8.0);
    drop(seen);
    let r = s.execute("generate.inspect", json!({"item": item.0})).unwrap();
    assert_eq!(r["generation"]["prompt"], "a lighthouse at dusk");
    s.undo();
    assert!(!s.project.generations.contains_key(&item));
    assert!(s.active_sequence().unwrap().video_tracks[0].items.iter().all(|it| it.item != item));
    assert!(s.generate_jobs.is_empty());
}

#[test]
fn insert_ripples_and_an_explicit_track_is_honoured() {
    let (mut s, dir, _) = session(None);
    s.execute("sequence.addTracks", json!({"video": 1, "audio": 0})).unwrap();
    let before_end = s.active_sequence().unwrap().video_tracks[0].items[0].end();
    let r = s
        .execute("generate.video", json!({"prompt": "x", "dir": dir.to_string_lossy(), "track": "V2", "atSeconds": 0.0, "insert": true, "wait": true}))
        .unwrap();
    assert_eq!(r["placed"]["track"], "V2", "{r}");
    let seq = s.active_sequence().unwrap();
    assert!(seq.video_tracks[1].items.iter().any(|it| it.start == Tick::ZERO), "placed on V2 at 0");
    assert!(seq.video_tracks[0].items[0].end() > before_end, "insert rippled the base clip later");
    assert_eq!(r["seconds"], 8.0, "the default length comes from the preferences");
    assert!(s.execute("generate.video", json!({"prompt": "x", "track": "V9", "dir": dir.to_string_lossy()})).is_err());
}

#[test]
fn bad_requests_fail_before_any_job_starts() {
    let (mut s, dir, fake) = session(None);
    let d = dir.to_string_lossy().into_owned();
    assert!(s.execute("generate.video", json!({"dir": &d})).is_err(), "prompt required");
    assert!(s.execute("generate.video", json!({"prompt": "x", "seconds": 1.0, "dir": &d})).is_err(), "too short");
    assert!(s.execute("generate.video", json!({"prompt": "x", "image": dir.join("missing.png").to_string_lossy(), "dir": &d})).is_err(), "missing image");
    assert!(s.execute("generate.video", json!({"prompt": "x", "imageRole": "middle", "dir": &d})).is_err());
    assert!(s.execute("generate.video", json!({"prompt": "x", "atTime": -1, "dir": &d})).is_err());
    assert!(fake.seen.lock().unwrap().is_empty(), "nothing was generated");
    assert!(s.jobs.is_empty());
}

#[test]
fn a_failed_generation_places_nothing_and_reports_the_reason() {
    let (mut s, dir, _) = session(Some("the model refused"));
    let items = s.project.items.len();
    let r = s.execute("generate.video", json!({"prompt": "x", "dir": dir.to_string_lossy(), "wait": true})).unwrap();
    assert!(r["placed"].is_null());
    let job = s.jobs.iter().find(|j| j.id == r["job"].as_u64().unwrap()).unwrap().to_json();
    assert!(job["result"]["error"].as_str().unwrap().contains("the model refused"), "{job}");
    assert_eq!(s.project.items.len(), items, "nothing imported");
    assert!(s.generate_jobs.is_empty());
    let r = s.execute("generate.status", json!({})).unwrap();
    assert_eq!(r["generations"].as_array().unwrap().len(), 0);
}

#[test]
fn cancel_marks_the_job_and_its_progress() {
    let (mut s, dir, _) = session(None);
    let r = s.execute("generate.video", json!({"prompt": "x", "dir": dir.to_string_lossy(), "wait": true})).unwrap();
    assert!(s.execute("generate.cancel", json!({"job": 999})).is_err());
    let r2 = s.execute("generate.cancel", json!({})).unwrap();
    assert_eq!(r2["cancelled"], 0, "the finished job is gone: {r}");
    let st = s.execute("generate.status", json!({})).unwrap();
    assert!(st["generations"].as_array().unwrap().is_empty());
}

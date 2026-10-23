//! What a generator is asked for, what it gives back, and how it reports on the way.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::PoisonError;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// How an optional still is used by the model.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ImageRole {
    /// The clip opens on the image and animates forward.
    #[default]
    First,
    /// The clip ends on the image.
    Last,
    /// The clip opens and ends on the same image.
    Both,
}

impl ImageRole {
    pub const ALL: [ImageRole; 3] = [ImageRole::First, ImageRole::Last, ImageRole::Both];

    pub fn id(self) -> &'static str {
        match self {
            ImageRole::First => "first",
            ImageRole::Last => "last",
            ImageRole::Both => "both",
        }
    }

    pub fn from_id(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.id() == s)
    }

    pub fn label(self) -> &'static str {
        match self {
            ImageRole::First => "First frame only",
            ImageRole::Last => "Last frame only",
            ImageRole::Both => "First and last frame",
        }
    }
}

/// One generation: a prompt, a length, an optional still, and where the clip goes on disk.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateRequest {
    pub prompt: String,
    pub seconds: f64,
    /// A still the clip opens from or ends on (a PNG, JPEG or other common image file).
    #[serde(default)]
    pub image: Option<PathBuf>,
    #[serde(default)]
    pub image_role: ImageRole,
    /// Reproducibility seed; the provider's default when None.
    #[serde(default)]
    pub seed: Option<u64>,
    /// Where the finished MP4 is written.
    pub output: PathBuf,
}

/// What a provider can do, so the UI offers only valid choices.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    pub min_seconds: f64,
    pub max_seconds: f64,
    pub default_seconds: f64,
    pub max_prompt_chars: usize,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub image_roles: Vec<ImageRole>,
    /// Price of one connected second, in US dollars (0 when unknown).
    pub price_per_second_usd: f64,
}

impl Limits {
    /// `seconds` clamped into the provider's range (NaN becomes the default).
    pub fn clamp_seconds(&self, seconds: f64) -> f64 {
        if !seconds.is_finite() {
            return self.default_seconds;
        }
        seconds.clamp(self.min_seconds, self.max_seconds)
    }

    /// A rough cost: the session is open for about twice the clip's length (build + playback).
    pub fn estimate_usd(&self, seconds: f64) -> f64 {
        (self.clamp_seconds(seconds) * 2.0 * self.price_per_second_usd).max(0.0)
    }
}

/// Reactor's FastH3 model (reactor.inc): what it accepts and produces. Known to every build, so
/// the UI can describe the provider even when the client is compiled out.
pub fn fast_h3_limits() -> Limits {
    Limits {
        min_seconds: 5.167,
        max_seconds: 14.375,
        default_seconds: 8.0,
        max_prompt_chars: 800,
        width: 1024,
        height: 768,
        fps: 24.0,
        image_roles: vec![ImageRole::First, ImageRole::Last, ImageRole::Both],
        price_per_second_usd: 0.0125,
    }
}

/// The finished clip.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedClip {
    pub path: PathBuf,
    /// The length the provider actually produced (the request's `seconds` snapped).
    pub seconds: f64,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    /// The provider's id of the clip, for provenance.
    pub provider_clip_id: String,
    /// The seed the clip was built with, when the provider reports it.
    pub seed: Option<u64>,
    /// Seconds the billed session was open.
    pub session_seconds: f64,
}

/// Where a generation is, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Idle,
    Token,
    Connecting,
    Uploading,
    Queued,
    Generating,
    Playing,
    Saving,
    Done,
    Failed,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::Idle => "Idle",
            Phase::Token => "Signing in",
            Phase::Connecting => "Connecting and waiting for a GPU",
            Phase::Uploading => "Uploading the image",
            Phase::Queued => "Queued",
            Phase::Generating => "Generating the clip",
            Phase::Playing => "Receiving the clip",
            Phase::Saving => "Saving the clip",
            Phase::Done => "Done",
            Phase::Failed => "Failed",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Phase::Idle => "idle",
            Phase::Token => "token",
            Phase::Connecting => "connecting",
            Phase::Uploading => "uploading",
            Phase::Queued => "queued",
            Phase::Generating => "generating",
            Phase::Playing => "playing",
            Phase::Saving => "saving",
            Phase::Done => "done",
            Phase::Failed => "failed",
        }
    }

    fn from_index(i: u64) -> Phase {
        match i {
            1 => Phase::Token,
            2 => Phase::Connecting,
            3 => Phase::Uploading,
            4 => Phase::Queued,
            5 => Phase::Generating,
            6 => Phase::Playing,
            7 => Phase::Saving,
            8 => Phase::Done,
            9 => Phase::Failed,
            _ => Phase::Idle,
        }
    }

    fn index(self) -> u64 {
        match self {
            Phase::Idle => 0,
            Phase::Token => 1,
            Phase::Connecting => 2,
            Phase::Uploading => 3,
            Phase::Queued => 4,
            Phase::Generating => 5,
            Phase::Playing => 6,
            Phase::Saving => 7,
            Phase::Done => 8,
            Phase::Failed => 9,
        }
    }
}

/// Shared progress of one generation: the phase, a line of detail, bytes saved, the time the
/// billed session has been open, and the cancel flag the generator polls.
#[derive(Default, Debug)]
pub struct Progress {
    phase: AtomicU64,
    detail: Mutex<String>,
    pub bytes_saved: AtomicU64,
    /// Milliseconds the billed session has been open (0 before it opens).
    pub session_ms: AtomicU64,
    pub cancel: AtomicBool,
}

impl Progress {
    pub fn set_phase(&self, phase: Phase) {
        self.phase.store(phase.index(), Ordering::Relaxed);
    }

    pub fn phase(&self) -> Phase {
        Phase::from_index(self.phase.load(Ordering::Relaxed))
    }

    pub fn set_detail(&self, detail: impl Into<String>) {
        *self.detail.lock().unwrap_or_else(PoisonError::into_inner) = detail.into();
    }

    pub fn detail(&self) -> String {
        self.detail.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    pub fn request_cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[derive(Debug, thiserror::Error)]
pub enum GenerateError {
    #[error("no API key: set one in Settings ▸ Generate")]
    NoApiKey,
    #[error("{0}")]
    BadRequest(String),
    #[error("cancelled")]
    Cancelled,
    #[error("network: {0}")]
    Network(String),
    #[error("the provider refused: {0}")]
    Refused(String),
    #[error("the clip failed to build: {0}")]
    Failed(String),
    #[error("timed out: {0}")]
    Timeout(String),
    #[error("protocol: {0}")]
    Protocol(String),
    #[error("{0}")]
    Io(String),
    #[error("not available in this build (built without the `reactor` feature)")]
    Unavailable,
}

impl From<std::io::Error> for GenerateError {
    fn from(e: std::io::Error) -> Self {
        GenerateError::Io(e.to_string())
    }
}

/// A video generator. One call is one clip; the generator reports through `progress`, stops
/// soon after `progress.cancel` is set, and never leaves a billed session open when it returns.
pub trait VideoGenerator: Send + Sync {
    /// Stable id (`reactor-fast-h3`).
    fn id(&self) -> &str;
    /// Shown in the UI (`Reactor FastH3`).
    fn name(&self) -> &str;
    fn limits(&self) -> Limits;
    /// Whether the generator can run now (a key is set, the build has the feature).
    fn ready(&self) -> Result<(), GenerateError>;
    fn generate(&self, request: &GenerateRequest, progress: &Progress) -> Result<GeneratedClip, GenerateError>;
}

/// Check a request against `limits` before anything is billed.
pub fn validate(request: &GenerateRequest, limits: &Limits) -> Result<(), GenerateError> {
    let prompt = request.prompt.trim();
    if prompt.is_empty() {
        return Err(GenerateError::BadRequest("the prompt is empty".into()));
    }
    if prompt.chars().count() > limits.max_prompt_chars {
        return Err(GenerateError::BadRequest(format!("the prompt is longer than {} characters", limits.max_prompt_chars)));
    }
    if !request.seconds.is_finite() || request.seconds < limits.min_seconds - 1e-9 || request.seconds > limits.max_seconds + 1e-9 {
        return Err(GenerateError::BadRequest(format!("the length must be between {:.3} and {:.3} seconds", limits.min_seconds, limits.max_seconds)));
    }
    if request.image.is_some() && !limits.image_roles.contains(&request.image_role) {
        return Err(GenerateError::BadRequest(format!("this provider does not support the image role `{}`", request.image_role.id())));
    }
    if request.output.as_os_str().is_empty() {
        return Err(GenerateError::BadRequest("no output path".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> Limits {
        Limits {
            min_seconds: 5.167,
            max_seconds: 14.375,
            default_seconds: 8.0,
            max_prompt_chars: 800,
            width: 1024,
            height: 768,
            fps: 24.0,
            image_roles: vec![ImageRole::First],
            price_per_second_usd: 0.0125,
        }
    }

    fn request() -> GenerateRequest {
        GenerateRequest { prompt: "a cat".into(), seconds: 8.0, image: None, image_role: ImageRole::First, seed: None, output: PathBuf::from("out.mp4") }
    }

    #[test]
    fn requests_are_checked_against_the_limits_before_anything_is_billed() {
        let l = limits();
        assert!(validate(&request(), &l).is_ok());
        assert!(validate(&GenerateRequest { prompt: "  ".into(), ..request() }, &l).is_err());
        assert!(validate(&GenerateRequest { prompt: "x".repeat(801), ..request() }, &l).is_err());
        assert!(validate(&GenerateRequest { seconds: 3.0, ..request() }, &l).is_err());
        assert!(validate(&GenerateRequest { seconds: f64::NAN, ..request() }, &l).is_err());
        assert!(validate(&GenerateRequest { image: Some(PathBuf::from("a.png")), image_role: ImageRole::Last, ..request() }, &l).is_err());
        assert!(validate(&GenerateRequest { image: Some(PathBuf::from("a.png")), image_role: ImageRole::First, ..request() }, &l).is_ok());
        assert!(validate(&GenerateRequest { output: PathBuf::new(), ..request() }, &l).is_err());
    }

    #[test]
    fn limits_clamp_and_estimate() {
        let l = limits();
        assert_eq!(l.clamp_seconds(1.0), 5.167);
        assert_eq!(l.clamp_seconds(100.0), 14.375);
        assert_eq!(l.clamp_seconds(f64::NAN), 8.0);
        assert!((l.estimate_usd(8.0) - 0.2).abs() < 1e-9);
    }

    #[test]
    fn progress_round_trips_phase_and_detail() {
        let p = Progress::default();
        assert_eq!(p.phase(), Phase::Idle);
        for ph in
            [Phase::Token, Phase::Connecting, Phase::Uploading, Phase::Queued, Phase::Generating, Phase::Playing, Phase::Saving, Phase::Done, Phase::Failed]
        {
            p.set_phase(ph);
            assert_eq!(p.phase(), ph);
            assert_eq!(Phase::from_index(ph.index()), ph);
        }
        p.set_detail("x");
        assert_eq!(p.detail(), "x");
        assert!(!p.cancelled());
        p.request_cancel();
        assert!(p.cancelled());
        assert_eq!(ImageRole::from_id("both"), Some(ImageRole::Both));
        assert_eq!(serde_json::to_value(ImageRole::Last).unwrap(), "last");
    }
}

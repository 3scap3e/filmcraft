//! The grammar of cinematography on top of the measured 2D camera: the solved path and the
//! per-comparison evidence become *beats* (static, pan, tilt, push in, pull out, roll, handheld
//! shake, no evidence) with their time span, size and speed. Every label is a classification of
//! the numbers next to it under rules that travel with the result, so whoever reads it (a person
//! or a model outside FilmCraft) can reinterpret the measurements under other criteria. Derived
//! from the measurements, never invented; FilmCraft writes no prompts.
use crate::camera2d::CameraState;
use crate::trajectory::FramePose;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Move {
    Static,
    PanLeft,
    PanRight,
    TiltUp,
    TiltDown,
    PushIn,
    PullOut,
    RollClockwise,
    RollCounterclockwise,
    HandheldShake,
    Drift,
    NoEvidence,
}

impl Move {
    /// The honest term for what a 2D similarity can see. The image plane cannot tell a turn from
    /// a sideways move (pan vs truck, tilt vs pedestal) nor a lens change from a forward move
    /// (zoom vs dolly): those need parallax, which the 3D camera brings later, so the terms name
    /// both readings.
    pub fn term(self) -> &'static str {
        match self {
            Move::Static => "static shot",
            Move::PanLeft => "pan or truck left",
            Move::PanRight => "pan or truck right",
            Move::TiltUp => "tilt or pedestal up",
            Move::TiltDown => "tilt or pedestal down",
            Move::PushIn => "zoom or dolly in",
            Move::PullOut => "zoom or dolly out",
            Move::RollClockwise => "roll clockwise",
            Move::RollCounterclockwise => "roll counterclockwise",
            Move::HandheldShake => "handheld shake",
            Move::Drift => "slight drift",
            Move::NoEvidence => "no camera evidence",
        }
    }
    /// The cinematography terms this image motion is consistent with; more than one when the 2D
    /// measurement cannot decide between them.
    pub fn alternatives(self) -> &'static [&'static str] {
        match self {
            Move::Static => &["static shot", "locked-off"],
            Move::PanLeft => &["pan left", "truck left"],
            Move::PanRight => &["pan right", "truck right"],
            Move::TiltUp => &["tilt up", "pedestal up"],
            Move::TiltDown => &["tilt down", "pedestal down"],
            Move::PushIn => &["zoom in", "dolly in", "push in"],
            Move::PullOut => &["zoom out", "dolly out", "pull out"],
            Move::RollClockwise => &["roll clockwise", "Dutch tilt clockwise"],
            Move::RollCounterclockwise => &["roll counterclockwise", "Dutch tilt counterclockwise"],
            Move::HandheldShake => &["handheld shake", "camera shake"],
            Move::Drift => &["slight drift", "float"],
            Move::NoEvidence => &[],
        }
    }
}

/// An extreme form of a move, read from its rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Extreme {
    /// A pan (or truck) so fast the image smears: at least `WHIP_PAN_RATE` frame widths per second.
    WhipPan,
    /// A zoom (or dolly) that lands almost at once: at least `CRASH_ZOOM_RATE` of scale per second.
    CrashZoom,
}

/// How steady the camera held during a beat, from the high-frequency jitter of its path (the
/// per-frame displacement minus a short moving average), in frame widths RMS. See [`Rules`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Steadiness {
    /// Tripod-still: jitter below `steadiness_locked_off_below`.
    LockedOff,
    /// A smooth move (dolly, gimbal, good handheld): below `steadiness_smooth_below`.
    Smooth,
    /// Visible handheld shake: below `steadiness_minor_below`.
    MinorShake,
    /// Severe shake: at or above `steadiness_minor_below`.
    SevereShake,
}

impl Steadiness {
    pub fn term(self) -> &'static str {
        match self {
            Steadiness::LockedOff => "locked off",
            Steadiness::Smooth => "smooth",
            Steadiness::MinorShake => "minor shake",
            Steadiness::SevereShake => "severe shake",
        }
    }
}

/// How a move accelerates: from the share of its progress made in each third of the beat.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VelocityProfile {
    Linear,
    /// The first third covers less than `velocity_eased_below_ratio` of the middle third.
    EaseIn,
    /// The last third covers less than that share of the middle third.
    EaseOut,
    EaseInOut,
}

impl VelocityProfile {
    pub fn term(self) -> &'static str {
        match self {
            VelocityProfile::Linear => "linear",
            VelocityProfile::EaseIn => "ease-in",
            VelocityProfile::EaseOut => "ease-out",
            VelocityProfile::EaseInOut => "ease-in-out",
        }
    }
}

/// How much the evidence behind a beat can be trusted.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Confidence {
    /// Share of the beat's comparisons that were solid (a static or moving camera was verified).
    pub solid_fraction: f64,
    /// Mean fitting residual of the beat's frames, source pixels.
    pub mean_residual_px: f64,
    /// `solid_fraction` below `uncertain_below_fraction`.
    pub uncertain: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Speed {
    Slow,
    Medium,
    Fast,
}

impl Speed {
    pub fn word(self) -> &'static str {
        match self {
            Speed::Slow => "slow",
            Speed::Medium => "steady",
            Speed::Fast => "fast",
        }
    }
}

/// One stretch of the shot with a single camera behaviour.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Beat {
    pub primary: Move,
    /// A second movement happening at the same time (a pan with a push in).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary: Option<Move>,
    pub start_frame: usize,
    pub end_frame: usize,
    pub start_seconds: f64,
    pub end_seconds: f64,
    /// Centre displacement over the beat in source pixels.
    pub displacement_px: [f64; 2],
    /// Zoom factor over the beat (end scale / start scale).
    pub zoom: f64,
    pub roll_degrees: f64,
    /// Largest frame-to-frame displacement during the beat, source pixels (the size of a shake).
    pub peak_step_px: f64,
    /// The number the speed label was read from: frame widths per second for pans and tilts,
    /// scale fraction per second for zooms, degrees/45 per second for rolls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed_rate: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<Speed>,
    /// Direction of the image displacement in degrees on screen: 0 = right, 90 = down, 180 =
    /// left, −90 = up. Present when the beat translates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction_degrees: Option<f64>,
    /// The minor axis of the displacement is at least `diagonal_min_ratio` of the major one.
    #[serde(default)]
    pub diagonal: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extreme: Option<Extreme>,
    /// The cinematography terms the primary move is consistent with (see [`Move::alternatives`]).
    #[serde(default)]
    pub alternatives: Vec<String>,
    /// How steady the camera held, read from `jitter_rms`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steadiness: Option<Steadiness>,
    /// RMS of the high-frequency jitter of the path over the beat, in frame widths.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jitter_rms: Option<f64>,
    /// Dominant frequency of that jitter in Hz (zero crossings of the residual along its stronger
    /// axis), for beats of at least `shake_hz_min_seconds` whose jitter is not locked off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shake_hz: Option<f64>,
    /// How the move accelerates; present for moving beats of at least `velocity_min_frames`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub velocity_profile: Option<VelocityProfile>,
    /// A still beat held at a roll of at least `canted_min_degrees` from the first frame (Dutch angle).
    #[serde(default)]
    pub canted: bool,
    /// Mean roll of the beat relative to the first frame of the shot, degrees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cant_degrees: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<Confidence>,
}

/// The thresholds every label in the beats was derived with. They are choices, not standards:
/// the grammar of cinematography has no numeric definition of "slow", so the numbers travel
/// with the words.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rules {
    /// Runs of comparisons shorter than this merge into their neighbour.
    pub min_run_frames: usize,
    /// A moving run between two still runs is a handheld shake when it is at most this long…
    pub shake_max_frames: usize,
    /// …and its net travel is below this multiple of its largest frame-to-frame step.
    pub shake_net_below_peak_times: f64,
    /// Centre displacement (source pixels) below which a beat has no pan or tilt.
    pub translation_min_px: f64,
    /// Scale change (fraction) below which a beat has no zoom.
    pub zoom_min_fraction: f64,
    /// Rotation (degrees) below which a beat has no roll.
    pub roll_min_degrees: f64,
    /// Speed label boundaries on `speed_rate`: below the first is slow, below the second steady, else fast.
    pub speed_slow_below: f64,
    pub speed_steady_below: f64,
    pub speed_rate_unit: String,
    /// Pan rate (frame widths per second) from which a pan is a whip pan.
    pub whip_pan_rate_min: f64,
    /// Zoom rate (scale fraction per second) from which a zoom is a crash zoom.
    pub crash_zoom_rate_min: f64,
    /// Minor over major displacement axis from which a translation is diagonal.
    pub diagonal_min_ratio: f64,
    /// Steadiness boundaries on `jitter_rms` (frame widths RMS of the path minus its moving
    /// average): below the first locked off, below the second smooth, below the third minor
    /// shake, else severe shake.
    pub steadiness_locked_off_below: f64,
    pub steadiness_smooth_below: f64,
    pub steadiness_minor_below: f64,
    /// Frames of the centred moving average the jitter is measured against.
    pub steadiness_window_frames: usize,
    /// Beats shorter than this (seconds) report no shake frequency.
    pub shake_hz_min_seconds: f64,
    /// A third of a move covering less than this share of the middle third is eased.
    pub velocity_eased_below_ratio: f64,
    /// Moving beats shorter than this many frames report no velocity profile.
    pub velocity_min_frames: usize,
    /// Roll (degrees) a still beat must hold to be canted (Dutch angle).
    pub canted_min_degrees: f64,
    /// Share of solid comparisons below which a beat is uncertain.
    pub uncertain_below_fraction: f64,
    /// Evidence states counted as solid: static and moving (verified either way).
    pub solid_states: Vec<String>,
    /// What the 2D image plane cannot decide, so terms name both readings.
    pub ambiguities: Vec<String>,
    /// Evidence states read as "still": static and unverified (a motion that explains nothing better).
    pub still_states: Vec<String>,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            min_run_frames: MIN_RUN,
            shake_max_frames: SHAKE_MAX,
            shake_net_below_peak_times: SHAKE_NET_BELOW_PEAK,
            translation_min_px: TRANSLATION_MIN_PX,
            zoom_min_fraction: ZOOM_MIN,
            roll_min_degrees: ROLL_MIN_DEGREES,
            speed_slow_below: SPEED_SLOW_BELOW,
            speed_steady_below: SPEED_STEADY_BELOW,
            speed_rate_unit: "pan/tilt: frame widths per second; zoom: scale fraction per second; roll: degrees/45 per second".into(),
            whip_pan_rate_min: WHIP_PAN_RATE,
            crash_zoom_rate_min: CRASH_ZOOM_RATE,
            diagonal_min_ratio: DIAGONAL_MIN_RATIO,
            steadiness_locked_off_below: STEADY_LOCKED_OFF,
            steadiness_smooth_below: STEADY_SMOOTH,
            steadiness_minor_below: STEADY_MINOR,
            steadiness_window_frames: STEADY_WINDOW,
            shake_hz_min_seconds: SHAKE_HZ_MIN_SECONDS,
            velocity_eased_below_ratio: EASED_BELOW_RATIO,
            velocity_min_frames: VELOCITY_MIN_FRAMES,
            canted_min_degrees: CANTED_MIN_DEGREES,
            uncertain_below_fraction: UNCERTAIN_BELOW,
            solid_states: vec!["static".into(), "moving".into()],
            ambiguities: vec![
                "pan vs truck and tilt vs pedestal: a turn and a sideways move give the same image motion without parallax".into(),
                "zoom vs dolly: a lens change and a forward move give the same image scale change without parallax".into(),
            ],
            still_states: vec!["static".into(), "unverified".into()],
        }
    }
}

/// The measurements the beats were read from, so a prompt can say what frame they describe.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cinematography {
    pub frame: Frame,
    pub duration_seconds: f64,
    pub beats: Vec<Beat>,
    /// The thresholds the labels were derived with.
    pub rules: Rules,
    /// One sentence a person reads: the camera of the shot in cinema terms. A derived label,
    /// like every word in the beats; the numbers are the measurement.
    pub summary: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Still,
    Moving,
    Gap,
}

fn phase_of(state: CameraState) -> Phase {
    match state {
        CameraState::Static | CameraState::Unverified => Phase::Still,
        CameraState::Moving => Phase::Moving,
        CameraState::NoEvidence => Phase::Gap,
    }
}

/// Frames shorter than this merge into their neighbour; moving runs no longer than `SHAKE_MAX`
/// between still runs are shakes rather than moves. See [`Rules`].
const MIN_RUN: usize = 3;
const SHAKE_MAX: usize = 12;
const SHAKE_NET_BELOW_PEAK: f64 = 2.0;
const TRANSLATION_MIN_PX: f64 = 2.0;
const ZOOM_MIN: f64 = 0.02;
const ROLL_MIN_DEGREES: f64 = 1.0;
const SPEED_SLOW_BELOW: f64 = 0.08;
const SPEED_STEADY_BELOW: f64 = 0.25;
const WHIP_PAN_RATE: f64 = 1.0;
const CRASH_ZOOM_RATE: f64 = 0.5;
const DIAGONAL_MIN_RATIO: f64 = 0.4;
const STEADY_LOCKED_OFF: f64 = 0.0005;
const STEADY_SMOOTH: f64 = 0.003;
const STEADY_MINOR: f64 = 0.01;
const STEADY_WINDOW: usize = 5;
const SHAKE_HZ_MIN_SECONDS: f64 = 1.0;
const EASED_BELOW_RATIO: f64 = 0.5;
const VELOCITY_MIN_FRAMES: usize = 6;
const CANTED_MIN_DEGREES: f64 = 3.0;
const UNCERTAIN_BELOW: f64 = 0.6;

fn seconds(frame: usize, fps: f64) -> f64 {
    if fps > 0.0 { frame as f64 / fps } else { 0.0 }
}

/// The progress of `primary` between two poses: path length for pans and tilts (source pixels),
/// |log scale| for zooms, degrees for rolls.
fn progress(primary: Move, a: &FramePose, b: &FramePose, px_scale: f64) -> f64 {
    match primary {
        Move::PanLeft | Move::PanRight | Move::TiltUp | Move::TiltDown => {
            ((b.displacement_px[0] - a.displacement_px[0]) * px_scale).hypot((b.displacement_px[1] - a.displacement_px[1]) * px_scale)
        }
        Move::PushIn | Move::PullOut => {
            if a.scale > 0.0 && b.scale > 0.0 {
                (b.scale / a.scale).ln().abs()
            } else {
                0.0
            }
        }
        Move::RollClockwise | Move::RollCounterclockwise => (b.rotation_degrees - a.rotation_degrees).abs(),
        _ => 0.0,
    }
}

/// Velocity profile of a move from the progress made in each third of its frames.
fn velocity_profile(primary: Move, poses: &[FramePose], px_scale: f64) -> Option<VelocityProfile> {
    if !matches!(
        primary,
        Move::PanLeft | Move::PanRight | Move::TiltUp | Move::TiltDown | Move::PushIn | Move::PullOut | Move::RollClockwise | Move::RollCounterclockwise
    ) {
        return None;
    }
    let steps = poses.len().checked_sub(1)?;
    if steps < VELOCITY_MIN_FRAMES {
        return None;
    }
    let mut thirds = [0.0f64; 3];
    for (i, w) in poses.windows(2).enumerate() {
        let k = (i * 3 / steps).min(2);
        thirds[k] += progress(primary, &w[0], &w[1], px_scale);
    }
    let middle = thirds[1];
    if !(middle.is_finite() && middle > 0.0) {
        return Some(VelocityProfile::Linear);
    }
    let eased_in = thirds[0] < EASED_BELOW_RATIO * middle;
    let eased_out = thirds[2] < EASED_BELOW_RATIO * middle;
    Some(match (eased_in, eased_out) {
        (true, true) => VelocityProfile::EaseInOut,
        (true, false) => VelocityProfile::EaseIn,
        (false, true) => VelocityProfile::EaseOut,
        (false, false) => VelocityProfile::Linear,
    })
}

/// Confidence of a beat from its comparisons' states and its frames' fitting residuals.
fn confidence_of(states: &[CameraState], poses: &[FramePose], px_scale: f64) -> Option<Confidence> {
    if states.is_empty() {
        return None;
    }
    let solid = states.iter().filter(|s| matches!(s, CameraState::Static | CameraState::Moving)).count();
    let solid_fraction = solid as f64 / states.len() as f64;
    let residuals: Vec<f64> = poses.iter().skip(1).map(|p| p.residual_px * px_scale).filter(|r| r.is_finite()).collect();
    let mean_residual_px = if residuals.is_empty() { 0.0 } else { residuals.iter().sum::<f64>() / residuals.len() as f64 };
    Some(Confidence { solid_fraction, mean_residual_px, uncertain: solid_fraction < UNCERTAIN_BELOW })
}

/// The jitter of a path: each pose's displacement (source pixels) minus the centred moving
/// average of `STEADY_WINDOW` frames (the window shrinks symmetrically at the ends, so a
/// straight path has no residual anywhere), as (x, y) residuals per frame.
fn jitter(poses: &[FramePose], px_scale: f64) -> Vec<[f64; 2]> {
    let n = poses.len();
    if n < 2 {
        return Vec::new();
    }
    let half = STEADY_WINDOW / 2;
    (0..n)
        .map(|i| {
            let h = half.min(i).min(n - 1 - i);
            let (lo, hi) = (i - h, i + h);
            let count = (hi - lo + 1) as f64;
            let (mut mx, mut my) = (0.0, 0.0);
            for p in poses.get(lo..=hi).unwrap_or(&[]) {
                mx += p.displacement_px[0];
                my += p.displacement_px[1];
            }
            let p = &poses[i];
            [(p.displacement_px[0] - mx / count) * px_scale, (p.displacement_px[1] - my / count) * px_scale]
        })
        .collect()
}

/// Steadiness of a beat: (RMS jitter in frame widths, class, dominant frequency in Hz).
fn steadiness_of(poses: &[FramePose], px_scale: f64, width: f64, fps: f64, beat_seconds: f64) -> Option<(f64, Steadiness, Option<f64>)> {
    let residual = jitter(poses, px_scale);
    if residual.is_empty() {
        return None;
    }
    let n = residual.len() as f64;
    let (sx, sy) = residual.iter().fold((0.0, 0.0), |(ax, ay), r| (ax + r[0] * r[0], ay + r[1] * r[1]));
    let rms = ((sx + sy) / n).sqrt() / width.max(1.0);
    if !rms.is_finite() {
        return None;
    }
    let class = if rms < STEADY_LOCKED_OFF {
        Steadiness::LockedOff
    } else if rms < STEADY_SMOOTH {
        Steadiness::Smooth
    } else if rms < STEADY_MINOR {
        Steadiness::MinorShake
    } else {
        Steadiness::SevereShake
    };
    let hz = (class != Steadiness::LockedOff && beat_seconds >= SHAKE_HZ_MIN_SECONDS && fps > 0.0).then(|| {
        let axis = usize::from(sy > sx);
        let crossings = residual.windows(2).filter(|w| (w[0][axis] < 0.0) != (w[1][axis] < 0.0)).count() as f64;
        crossings / 2.0 / (residual.len() as f64 / fps)
    });
    Some((rms, class, hz.filter(|h| h.is_finite() && *h > 0.0)))
}

/// The moves of a moving beat from its end poses: the dominant axis of the pan or tilt, a zoom
/// beyond 2 %, a roll beyond 1°; the largest becomes primary.
fn moves_of(dx: f64, dy: f64, zoom: f64, roll: f64, width: f64, height: f64) -> Vec<Move> {
    let mut candidates: Vec<(f64, Move)> = Vec::new();
    let translation = dx.hypot(dy);
    if translation >= TRANSLATION_MIN_PX {
        let normalised = translation / width.max(1.0);
        let m = if dx.abs() >= dy.abs() {
            if dx > 0.0 { Move::PanRight } else { Move::PanLeft }
        } else if dy > 0.0 {
            Move::TiltDown
        } else {
            Move::TiltUp
        };
        candidates.push((normalised, m));
    }
    if (zoom - 1.0).abs() >= ZOOM_MIN {
        candidates.push(((zoom - 1.0).abs(), if zoom > 1.0 { Move::PushIn } else { Move::PullOut }));
    }
    if roll.abs() >= ROLL_MIN_DEGREES {
        candidates.push((roll.abs() / 45.0, if roll > 0.0 { Move::RollClockwise } else { Move::RollCounterclockwise }));
    }
    let _ = height;
    candidates.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    candidates.into_iter().map(|c| c.1).collect()
}

/// Speed of a beat from how much of the frame it crosses per second (pans and tilts), the zoom
/// per second or the roll per second.
fn speed_of(primary: Move, beat_seconds: f64, displacement_px: [f64; 2], zoom: f64, roll: f64, width: f64) -> Option<(f64, Speed)> {
    if beat_seconds <= 0.0 {
        return None;
    }
    let rate = match primary {
        Move::PanLeft | Move::PanRight | Move::TiltUp | Move::TiltDown => displacement_px[0].hypot(displacement_px[1]) / width.max(1.0) / beat_seconds,
        Move::PushIn | Move::PullOut => (zoom - 1.0).abs() / beat_seconds,
        Move::RollClockwise | Move::RollCounterclockwise => roll.abs() / 45.0 / beat_seconds,
        _ => return None,
    };
    let speed = if rate < SPEED_SLOW_BELOW {
        Speed::Slow
    } else if rate < SPEED_STEADY_BELOW {
        Speed::Medium
    } else {
        Speed::Fast
    };
    Some((rate, speed))
}

/// Read the beats of a shot. `states` has one entry per comparison (frame i−1 → i), `trajectory`
/// one pose per frame, `px_scale` converts analysis pixels to source pixels.
pub fn beats(states: &[CameraState], trajectory: &[FramePose], frame: &Frame, px_scale: f64) -> Result<Vec<Beat>, String> {
    if states.is_empty() || trajectory.len() != states.len() + 1 {
        return Err("Beats need one state per comparison and one pose per frame".into());
    }
    if !(frame.fps.is_finite() && frame.fps > 0.0 && px_scale.is_finite()) || px_scale <= 0.0 {
        return Err("Frame rate and pixel scale must be positive".into());
    }
    let mut runs: Vec<(Phase, usize, usize)> = Vec::new();
    for (i, s) in states.iter().enumerate() {
        let phase = phase_of(*s);
        match runs.last_mut() {
            Some((p, _, end)) if *p == phase => *end = i + 1,
            _ => runs.push((phase, i, i + 1)),
        }
    }
    let mut merged: Vec<(Phase, usize, usize)> = Vec::new();
    for run in runs {
        let short = run.2 - run.1 < MIN_RUN;
        match merged.last_mut() {
            Some(last) if short || last.0 == run.0 => last.2 = run.2,
            _ => merged.push(run),
        }
    }
    let (width, height) = (f64::from(frame.width.max(1)), f64::from(frame.height.max(1)));
    let total = merged.len();
    let mut out = Vec::with_capacity(total);
    for (k, (phase, start, end)) in merged.iter().enumerate() {
        let (Some(from), Some(to)) = (trajectory.get(*start), trajectory.get(*end)) else { continue };
        let dx = (to.displacement_px[0] - from.displacement_px[0]) * px_scale;
        let dy = (to.displacement_px[1] - from.displacement_px[1]) * px_scale;
        let zoom = if from.scale.abs() > 1e-9 { to.scale / from.scale } else { 1.0 };
        let roll = to.rotation_degrees - from.rotation_degrees;
        let peak_step = trajectory
            .get(*start..=*end)
            .map(|poses| {
                poses
                    .windows(2)
                    .map(|w| {
                        ((w[1].displacement_px[0] - w[0].displacement_px[0]) * px_scale).hypot((w[1].displacement_px[1] - w[0].displacement_px[1]) * px_scale)
                    })
                    .fold(0.0f64, f64::max)
            })
            .unwrap_or(0.0);
        let (start_s, end_s) = (seconds(*start, frame.fps), seconds(*end, frame.fps));
        let frames = end - start;
        let between_stills =
            k > 0 && k + 1 < total && merged.get(k - 1).is_some_and(|r| r.0 == Phase::Still) && merged.get(k + 1).is_some_and(|r| r.0 == Phase::Still);
        let moves = moves_of(dx, dy, zoom, roll, width, height);
        let (primary, secondary) = match phase {
            Phase::Still => (Move::Static, None),
            Phase::Gap => (Move::NoEvidence, None),
            Phase::Moving => {
                if frames <= SHAKE_MAX && between_stills && dx.hypot(dy) < peak_step * SHAKE_NET_BELOW_PEAK {
                    (Move::HandheldShake, None)
                } else {
                    match moves.as_slice() {
                        [] => (Move::Drift, None),
                        [one] => (*one, None),
                        [one, two, ..] => (*one, Some(*two)),
                    }
                }
            }
        };
        let rated = speed_of(primary, end_s - start_s, [dx, dy], zoom, roll, width);
        let translates = matches!(primary, Move::PanLeft | Move::PanRight | Move::TiltUp | Move::TiltDown)
            || matches!(secondary, Some(Move::PanLeft | Move::PanRight | Move::TiltUp | Move::TiltDown));
        let direction_degrees = (translates && dx.hypot(dy) >= TRANSLATION_MIN_PX).then(|| dy.atan2(dx).to_degrees());
        let diagonal = translates && dx.abs().min(dy.abs()) >= DIAGONAL_MIN_RATIO * dx.abs().max(dy.abs()) && dx.hypot(dy) >= TRANSLATION_MIN_PX;
        let extreme = match (primary, rated) {
            (Move::PanLeft | Move::PanRight | Move::TiltUp | Move::TiltDown, Some((rate, _))) if rate >= WHIP_PAN_RATE => Some(Extreme::WhipPan),
            (Move::PushIn | Move::PullOut, Some((rate, _))) if rate >= CRASH_ZOOM_RATE => Some(Extreme::CrashZoom),
            _ => None,
        };
        let poses = trajectory.get(*start..=*end).unwrap_or(&[]);
        let steady = match phase {
            Phase::Gap => None,
            _ => steadiness_of(poses, px_scale, width, frame.fps, end_s - start_s),
        };
        let velocity_profile = if *phase == Phase::Moving { velocity_profile(primary, poses, px_scale) } else { None };
        let cant_degrees = (*phase == Phase::Still && !poses.is_empty())
            .then(|| poses.iter().map(|p| p.rotation_degrees).sum::<f64>() / poses.len() as f64)
            .filter(|c| c.is_finite());
        let canted = cant_degrees.is_some_and(|c| c.abs() >= CANTED_MIN_DEGREES);
        let confidence = if *phase == Phase::Gap { None } else { confidence_of(states.get(*start..*end).unwrap_or(&[]), poses, px_scale) };
        out.push(Beat {
            primary,
            secondary,
            start_frame: *start,
            end_frame: *end,
            start_seconds: start_s,
            end_seconds: end_s,
            displacement_px: [dx, dy],
            zoom,
            roll_degrees: roll,
            peak_step_px: peak_step,
            speed_rate: rated.map(|r| r.0),
            speed: rated.map(|r| r.1),
            direction_degrees,
            diagonal,
            extreme,
            alternatives: primary.alternatives().iter().map(|s| s.to_string()).collect(),
            steadiness: steady.map(|s| s.1),
            jitter_rms: steady.map(|s| s.0),
            shake_hz: steady.and_then(|s| s.2),
            velocity_profile,
            canted,
            cant_degrees,
            confidence,
        });
    }
    Ok(out)
}

/// ", handheld (minor shake, 4 Hz)" when a beat shakes; nothing for locked-off or smooth beats.
fn steadiness_note(beat: &Beat) -> String {
    match beat.steadiness {
        Some(s @ (Steadiness::MinorShake | Steadiness::SevereShake)) => match beat.shake_hz {
            Some(hz) => format!(", handheld ({}, {hz:.0} Hz)", s.term()),
            None => format!(", handheld ({})", s.term()),
        },
        _ => String::new(),
    }
}

fn describe(beat: &Beat, width: f64) -> String {
    let span = beat.end_seconds - beat.start_seconds;
    let dur = format!("{span:.1} s");
    let fraction = |px: f64| format!("{:.0} % of the frame", (px / width.max(1.0) * 100.0).round());
    let shake = steadiness_note(beat);
    let doubt = if beat.confidence.is_some_and(|c| c.uncertain) { " (uncertain)" } else { "" };
    match beat.primary {
        Move::Static if beat.canted => format!("canted static shot (Dutch angle, {:.0}°) for {dur}{shake}{doubt}", beat.cant_degrees.unwrap_or(0.0).abs()),
        Move::Static => format!("static shot for {dur}{shake}{doubt}"),
        Move::NoEvidence => format!("no camera evidence for {dur} (cut, flash or featureless frames)"),
        Move::HandheldShake => format!("handheld shake for {dur}, up to {:.0} px per frame{doubt}", beat.peak_step_px),
        Move::Drift => format!("slight drift for {dur}{shake}{doubt}"),
        primary => {
            let speed = match beat.extreme {
                Some(Extreme::WhipPan) => "whip ".to_string(),
                Some(Extreme::CrashZoom) => "crash ".to_string(),
                None => beat.speed.map(|s| format!("{} ", s.word())).unwrap_or_default(),
            };
            let eased = match beat.velocity_profile {
                Some(p @ (VelocityProfile::EaseIn | VelocityProfile::EaseOut | VelocityProfile::EaseInOut)) => format!("{} ", p.term()),
                _ => String::new(),
            };
            let diagonal = if beat.diagonal { "diagonal " } else { "" };
            let size = match primary {
                Move::PanLeft | Move::PanRight | Move::TiltUp | Move::TiltDown => {
                    format!(" across {}", fraction(beat.displacement_px[0].hypot(beat.displacement_px[1])))
                }
                Move::PushIn | Move::PullOut => format!(" by {:.0} %", (beat.zoom - 1.0).abs() * 100.0),
                Move::RollClockwise | Move::RollCounterclockwise => format!(" by {:.1}°", beat.roll_degrees.abs()),
                _ => String::new(),
            };
            let also = match beat.secondary {
                Some(Move::PushIn) | Some(Move::PullOut) => {
                    format!(" while {} {:.0} %", beat.secondary.map(Move::term).unwrap_or(""), (beat.zoom - 1.0).abs() * 100.0)
                }
                Some(m) => format!(" with a {}", m.term()),
                None => String::new(),
            };
            format!("{speed}{eased}{diagonal}{}{size} over {dur}{also}{shake}{doubt}", primary.term())
        }
    }
}

/// The whole cinematography of a shot: beats, a one-sentence summary, the prompt block and the
/// generator-specific vocabularies.
pub fn cinematography(states: &[CameraState], trajectory: &[FramePose], frame: Frame, px_scale: f64) -> Result<Cinematography, String> {
    let beats = beats(states, trajectory, &frame, px_scale)?;
    let width = f64::from(frame.width.max(1));
    let duration_seconds = seconds(states.len(), frame.fps);
    let parts: Vec<String> = beats.iter().map(|b| format!("{} ({:.1}–{:.1} s)", describe(b, width), b.start_seconds, b.end_seconds)).collect();
    let summary = format!("Camera: {}.", parts.join("; then "));
    Ok(Cinematography { frame, duration_seconds, beats, rules: Rules::default(), summary })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pose(frame: usize, dx: f64, dy: f64, scale: f64) -> FramePose {
        FramePose { frame, transform: [scale, 0.0, dx, dy], displacement_px: [dx, dy], scale, rotation_degrees: 0.0, residual_px: 0.0 }
    }
    fn frame() -> Frame {
        Frame { width: 1920, height: 1080, fps: 24.0 }
    }

    #[test]
    fn a_pan_between_two_holds_reads_as_three_beats_with_speed() {
        let mut states = vec![CameraState::Static; 24];
        states.extend(std::iter::repeat_n(CameraState::Moving, 48));
        states.extend(std::iter::repeat_n(CameraState::Static, 24));
        let mut trajectory: Vec<FramePose> = (0..=24).map(|i| pose(i, 0.0, 0.0, 1.0)).collect();
        trajectory.extend((25..=72).map(|i| pose(i, -((i - 24) as f64) * 4.0, 0.0, 1.0)));
        trajectory.extend((73..=96).map(|i| pose(i, -192.0, 0.0, 1.0)));
        let c = cinematography(&states, &trajectory, frame(), 3.0).unwrap();
        assert_eq!(c.beats.len(), 3);
        assert_eq!(c.beats[0].primary, Move::Static);
        assert_eq!(c.beats[1].primary, Move::PanLeft);
        assert_eq!(c.beats[1].secondary, None);
        assert_eq!(c.beats[1].displacement_px, [-576.0, 0.0]);
        assert_eq!(c.beats[1].speed, Some(Speed::Medium), "576 px of 1920 in 2 s = 15 % of the frame per second");
        assert_eq!(c.beats[2].primary, Move::Static);
        assert_eq!(
            c.summary,
            "Camera: static shot for 1.0 s (0.0–1.0 s); then steady pan or truck left across 30 % of the frame over 2.0 s (1.0–3.0 s); then static shot for 1.0 s (3.0–4.0 s)."
        );
        assert!((c.beats[1].speed_rate.unwrap() - 0.15).abs() < 1e-9, "the number behind the label travels with it");
        assert_eq!(c.beats[1].alternatives, vec!["pan left", "truck left"]);
        assert!((c.beats[1].direction_degrees.unwrap() - 180.0).abs() < 1e-9 && !c.beats[1].diagonal);
        assert_eq!(c.beats[1].extreme, None);
        assert_eq!(c.beats[0].alternatives, vec!["static shot", "locked-off"]);
        assert!(c.rules.ambiguities.len() == 2 && c.rules.whip_pan_rate_min == 1.0);
        assert_eq!(c.beats[0].speed_rate, None);
        assert_eq!(c.duration_seconds, 4.0);
        assert_eq!(c.rules, Rules::default());
        assert!(c.rules.speed_slow_below < c.rules.speed_steady_below && c.rules.min_run_frames == 3 && c.rules.shake_max_frames == 12);
        let json = serde_json::to_value(&c).unwrap();
        assert!(json["rules"]["speedRateUnit"].is_string() && json["beats"][1]["speedRate"].is_number());
        assert!(json.get("prompt").is_none() && json.get("lumaConcepts").is_none(), "FilmCraft writes no prompts and no vendor vocabularies");
    }

    #[test]
    fn a_short_jolt_between_holds_is_a_handheld_shake_and_a_zoom_with_pan_has_a_secondary_move() {
        let mut states = vec![CameraState::Static; 24];
        states.extend(std::iter::repeat_n(CameraState::Moving, 8));
        states.extend(std::iter::repeat_n(CameraState::Static, 24));
        let trajectory: Vec<FramePose> = (0..=56)
            .map(|i| {
                let dx = match i {
                    25 => -16.0,
                    26 => -4.0,
                    27 => 8.0,
                    28 => 8.0,
                    29 => 0.0,
                    30 => -4.0,
                    31 => -8.0,
                    _ => -8.0,
                };
                pose(i, if i >= 25 { dx } else { 0.0 }, 0.0, 1.0)
            })
            .collect();
        let c = cinematography(&states, &trajectory, frame(), 3.0).unwrap();
        assert_eq!(c.beats.len(), 3, "{:?}", c.beats.iter().map(|b| b.primary).collect::<Vec<_>>());
        assert_eq!(c.beats[1].primary, Move::HandheldShake);
        assert!(c.beats[1].peak_step_px >= 48.0, "{}", c.beats[1].peak_step_px);
        assert!(c.summary.contains("handheld shake for 0.3 s"), "{}", c.summary);

        let states = vec![CameraState::Moving; 48];
        let trajectory: Vec<FramePose> = (0..=48).map(|i| pose(i, i as f64 * 2.0, 0.0, 1.0 + 0.005 * i as f64)).collect();
        let c = cinematography(&states, &trajectory, frame(), 3.0).unwrap();
        assert_eq!(c.beats.len(), 1);
        assert_eq!(c.beats[0].primary, Move::PushIn, "a 24 % zoom outweighs a 15 % pan");
        assert_eq!(c.beats[0].secondary, Some(Move::PanRight));
        assert!(c.summary.contains("zoom or dolly in by 24 % over 2.0 s with a pan or truck right"), "{}", c.summary);
        assert_eq!(c.beats[0].speed, Some(Speed::Medium));
        assert_eq!(c.beats[0].alternatives, vec!["zoom in", "dolly in", "push in"]);
        assert!((c.beats[0].direction_degrees.unwrap() - 0.0).abs() < 1e-9, "the secondary pan still records its direction");
    }

    #[test]
    fn whip_pans_crash_zooms_and_diagonals_are_named_from_their_rates() {
        let states = vec![CameraState::Moving; 6];
        let trajectory: Vec<FramePose> = (0..=6).map(|i| pose(i, -(i as f64) * 100.0, (i as f64) * 60.0, 1.0)).collect();
        let c = cinematography(&states, &trajectory, frame(), 1.0).unwrap();
        let b = &c.beats[0];
        assert_eq!(b.primary, Move::PanLeft);
        assert_eq!(b.extreme, Some(Extreme::WhipPan), "600 px of 1920 in 0.25 s is 1.25 frame widths per second");
        assert!(b.diagonal, "360 down against 600 left is a 60 % minor axis");
        assert!((b.direction_degrees.unwrap() - 149.036).abs() < 0.01, "{:?}", b.direction_degrees);
        assert!(c.summary.starts_with("Camera: whip diagonal pan or truck left across"), "{}", c.summary);

        let states = vec![CameraState::Moving; 6];
        let trajectory: Vec<FramePose> = (0..=6).map(|i| pose(i, 0.0, 0.0, 1.0 + 0.05 * i as f64)).collect();
        let c = cinematography(&states, &trajectory, frame(), 1.0).unwrap();
        assert_eq!(c.beats[0].extreme, Some(Extreme::CrashZoom), "30 % in 0.25 s is 1.2 of scale per second");
        assert!(c.summary.starts_with("Camera: crash zoom or dolly in by 30 %"), "{}", c.summary);
        assert_eq!(c.beats[0].direction_degrees, None);
        let json = serde_json::to_value(&c).unwrap();
        assert_eq!(json["beats"][0]["extreme"], "crashZoom");
        assert_eq!(json["rules"]["crashZoomRateMin"], 0.5);
    }

    #[test]
    fn steadiness_reads_the_jitter_and_its_frequency() {
        let states = vec![CameraState::Moving; 48];
        let smooth: Vec<FramePose> = (0..=48).map(|i| pose(i, -(i as f64) * 4.0, 0.0, 1.0)).collect();
        let c = cinematography(&states, &smooth, frame(), 3.0).unwrap();
        assert_eq!(c.beats[0].steadiness, Some(Steadiness::LockedOff), "a perfectly linear pan has no jitter: {:?}", c.beats[0].jitter_rms);
        assert_eq!(c.beats[0].shake_hz, None);
        assert!(!c.summary.contains("handheld"), "{}", c.summary);

        let shaky: Vec<FramePose> =
            (0..=48).map(|i| pose(i, -(i as f64) * 4.0, (2.0 * std::f64::consts::PI * 4.0 * i as f64 / 24.0).sin() * 4.0, 1.0)).collect();
        let c = cinematography(&states, &shaky, frame(), 3.0).unwrap();
        let b = &c.beats[0];
        assert_eq!(b.primary, Move::PanLeft);
        assert_eq!(b.steadiness, Some(Steadiness::MinorShake), "12 px amplitude on 1920 is ≈0.4 % RMS: {:?}", b.jitter_rms);
        let hz = b.shake_hz.unwrap();
        assert!((hz - 4.0).abs() <= 0.6, "{hz}");
        assert!(c.summary.contains("over 2.0 s, handheld (minor shake, 4 Hz)"), "{}", c.summary);
        assert_eq!(c.rules.steadiness_window_frames, 5);
        let json = serde_json::to_value(&c).unwrap();
        assert_eq!(json["beats"][0]["steadiness"], "minorShake");
        assert_eq!(json["rules"]["steadinessMinorBelow"], 0.01);

        let states = vec![CameraState::Static; 6];
        let severe: Vec<FramePose> = (0..=6).map(|i| pose(i, if i % 2 == 0 { 30.0 } else { -30.0 }, 0.0, 1.0)).collect();
        let c = cinematography(&states, &severe, frame(), 1.0).unwrap();
        assert_eq!(c.beats[0].steadiness, Some(Steadiness::SevereShake));
        assert_eq!(c.beats[0].shake_hz, None, "too short for a frequency");
        assert!(c.summary.starts_with("Camera: static shot for 0.2 s, handheld (severe shake)"), "{}", c.summary);
    }

    #[test]
    fn easing_canted_holds_and_confidence_are_read_from_the_path() {
        let states = vec![CameraState::Moving; 48];
        let eased: Vec<FramePose> = (0..=48)
            .map(|i| {
                let t = i as f64 / 48.0;
                let s = t * t * t * (t * (6.0 * t - 15.0) + 10.0);
                pose(i, 0.0, 0.0, 1.0 + 0.3 * s)
            })
            .collect();
        let c = cinematography(&states, &eased, frame(), 1.0).unwrap();
        assert_eq!(c.beats[0].primary, Move::PushIn);
        assert_eq!(c.beats[0].velocity_profile, Some(VelocityProfile::EaseInOut));
        assert!(c.summary.starts_with("Camera: steady ease-in-out zoom or dolly in by 30 %"), "{}", c.summary);
        let linear: Vec<FramePose> = (0..=48).map(|i| pose(i, i as f64 * 4.0, 0.0, 1.0)).collect();
        let c = cinematography(&states, &linear, frame(), 1.0).unwrap();
        assert_eq!(c.beats[0].velocity_profile, Some(VelocityProfile::Linear));
        assert!(!c.summary.contains("linear"), "linear is the default and stays unsaid: {}", c.summary);
        let ease_out: Vec<FramePose> = (0..=48).map(|i| pose(i, 200.0 * (1.0 - (1.0 - i as f64 / 48.0).powi(3)), 0.0, 1.0)).collect();
        let c = cinematography(&states, &ease_out, frame(), 1.0).unwrap();
        assert_eq!(c.beats[0].velocity_profile, Some(VelocityProfile::EaseOut), "{:?}", c.beats[0]);

        let mut states = vec![CameraState::Moving; 6];
        states.extend(std::iter::repeat_n(CameraState::Static, 24));
        let canted: Vec<FramePose> = (0..=30)
            .map(|i| FramePose {
                frame: i,
                transform: [1.0, 0.0, 0.0, 0.0],
                displacement_px: [0.0, 0.0],
                scale: 1.0,
                rotation_degrees: (i.min(6) as f64),
                residual_px: 0.5,
            })
            .collect();
        let c = cinematography(&states, &canted, frame(), 2.0).unwrap();
        assert_eq!(c.beats.len(), 2);
        assert_eq!(c.beats[0].primary, Move::RollClockwise);
        assert!(c.beats[1].canted && (c.beats[1].cant_degrees.unwrap() - 6.0).abs() < 1e-9);
        assert!(c.summary.contains("canted static shot (Dutch angle, 6°) for 1.0 s"), "{}", c.summary);
        let conf = c.beats[1].confidence.unwrap();
        assert_eq!((conf.solid_fraction, conf.uncertain), (1.0, false));
        assert!((conf.mean_residual_px - 1.0).abs() < 1e-9, "residuals are scaled to source pixels");
        assert!(!c.beats[0].canted, "a moving beat is never canted");

        let mut states = vec![CameraState::Unverified; 20];
        states.extend(std::iter::repeat_n(CameraState::Static, 4));
        let still: Vec<FramePose> = (0..=24).map(|i| pose(i, 0.0, 0.0, 1.0)).collect();
        let c = cinematography(&states, &still, frame(), 1.0).unwrap();
        let conf = c.beats[0].confidence.unwrap();
        assert!(conf.uncertain && (conf.solid_fraction - 4.0 / 24.0).abs() < 1e-9);
        assert!(c.summary.starts_with("Camera: static shot for 1.0 s (uncertain)"), "{}", c.summary);
        let json = serde_json::to_value(&c).unwrap();
        assert_eq!(json["beats"][0]["confidence"]["uncertain"], true);
        assert_eq!(json["rules"]["cantedMinDegrees"], 3.0);
        assert_eq!(json["rules"]["uncertainBelowFraction"], 0.6);
        assert_eq!(json["rules"]["velocityEasedBelowRatio"], 0.5);
    }

    #[test]
    fn gaps_and_hostile_input() {
        let mut states = vec![CameraState::Moving; 24];
        states.extend(std::iter::repeat_n(CameraState::NoEvidence, 24));
        let trajectory: Vec<FramePose> = (0..=48).map(|i| pose(i, 0.0, -(i.min(24) as f64) * 10.0, 1.0)).collect();
        let c = cinematography(&states, &trajectory, frame(), 1.0).unwrap();
        assert_eq!(c.beats[0].primary, Move::TiltUp);
        assert!((c.beats[0].direction_degrees.unwrap() + 90.0).abs() < 1e-9, "up is −90° on screen");
        assert_eq!(c.beats[0].speed, Some(Speed::Medium), "240 px of 1920 in 1 s is 12.5 % of the frame per second");
        assert_eq!(c.beats[1].primary, Move::NoEvidence);
        assert_eq!(c.beats[1].steadiness, None, "no evidence, no steadiness");
        assert!(c.summary.contains("no camera evidence for 1.0 s"), "{}", c.summary);
        assert!(cinematography(&[], &[], frame(), 1.0).is_err());
        assert!(cinematography(&states, &trajectory[..3], frame(), 1.0).is_err());
        assert!(cinematography(&states, &trajectory, Frame { width: 1, height: 1, fps: 0.0 }, 1.0).is_err());
        assert!(cinematography(&states, &trajectory, frame(), f64::NAN).is_err());
    }
}

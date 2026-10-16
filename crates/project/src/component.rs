//! Reverse-engineering components applied to a sequence, shown on the timeline's RE lane: what
//! was applied (a 2D camera for now), on which clip and when, where it came from, which effect
//! parameters it wrote, and a per-frame summary of its path so the editor can draw it and mark
//! where the motion changes.
use crate::{ClipId, bounded_time_range};
use filmcraft_time::Tick;
use serde::{Deserialize, Serialize};

pub const MAX_COMPONENT_SAMPLES: usize = 36_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ComponentId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ComponentKind {
    Camera2d,
}

impl ComponentKind {
    pub fn label(self) -> &'static str {
        match self {
            ComponentKind::Camera2d => "2D camera",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ComponentMode {
    Follow,
    Stabilize,
}

impl ComponentMode {
    pub fn label(self) -> &'static str {
        match self {
            ComponentMode::Follow => "follow",
            ComponentMode::Stabilize => "stabilize",
        }
    }
}

/// Where the component was measured.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ComponentSource {
    pub sequence: String,
    pub width: u32,
    pub height: u32,
    pub frame_rate: f64,
    pub start_frame: i64,
    pub end_frame: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

/// One frame of the component's path: offset from its start, centre displacement in sequence
/// pixels, scale, rotation in degrees, solve residual in pixels and the evidence state of the
/// comparison into the frame (`static`, `moving`, `unverified`, `noEvidence`; none for frame 0).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentSample {
    pub tick: i64,
    pub dx: f64,
    pub dy: f64,
    pub scale: f64,
    pub rotation: f64,
    pub residual: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

/// Why a mark is drawn on the lane at a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChangeKind {
    /// The camera starts moving after standing still.
    MotionStarts,
    /// The camera comes to rest after moving.
    MotionStops,
    /// A stretch without evidence begins (a cut, a flash, a featureless frame).
    NoEvidence,
    /// A frame whose measurements disagree with the solved path by more than 2 px.
    HighResidual,
}

impl ChangeKind {
    pub fn label(self) -> &'static str {
        match self {
            ChangeKind::MotionStarts => "motion starts",
            ChangeKind::MotionStops => "motion stops",
            ChangeKind::NoEvidence => "no evidence",
            ChangeKind::HighResidual => "high residual",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppliedComponent {
    pub id: ComponentId,
    pub kind: ComponentKind,
    pub name: String,
    pub mode: ComponentMode,
    pub clip: ClipId,
    pub start: Tick,
    pub duration: Tick,
    /// The effect whose parameters were written (`motion`).
    pub effect: String,
    /// The parameters written, so removing the component removes exactly its keyframes.
    pub params: Vec<String>,
    #[serde(default)]
    pub source: ComponentSource,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub samples: Vec<ComponentSample>,
}

impl AppliedComponent {
    pub fn end(&self) -> Tick {
        self.start + self.duration
    }

    pub fn check(&self) -> Result<(), String> {
        if !bounded_time_range(self.start, self.duration) || self.duration <= Tick::ZERO {
            return Err("component time range is outside supported bounds".into());
        }
        if self.samples.len() > MAX_COMPONENT_SAMPLES || self.params.len() > 64 || self.name.len() > 4096 {
            return Err("component holds too many samples, parameters or characters".into());
        }
        let mut previous = -1i64;
        for s in &self.samples {
            if s.tick < 0 || s.tick > self.duration.0 || s.tick <= previous {
                return Err("component samples must have increasing offsets inside its duration".into());
            }
            if ![s.dx, s.dy, s.scale, s.rotation, s.residual].iter().all(|v| v.is_finite()) {
                return Err("component samples must be finite".into());
            }
            previous = s.tick;
        }
        if !self.source.frame_rate.is_finite() {
            return Err("component source frame rate must be finite".into());
        }
        Ok(())
    }

    /// The frames where something changes, for the lane's marks and the tooltip.
    pub fn changes(&self) -> Vec<(Tick, ChangeKind)> {
        let mut out = Vec::new();
        let moving = |s: &ComponentSample| s.state.as_deref() == Some("moving");
        let none = |s: &ComponentSample| s.state.as_deref() == Some("noEvidence");
        for w in self.samples.windows(2) {
            let (a, b) = (&w[0], &w[1]);
            let at = Tick(self.start.0.saturating_add(b.tick));
            if none(b) && !none(a) {
                out.push((at, ChangeKind::NoEvidence));
            } else if moving(b) && !moving(a) {
                out.push((at, ChangeKind::MotionStarts));
            } else if !moving(b) && !none(b) && moving(a) {
                out.push((at, ChangeKind::MotionStops));
            }
            if b.residual > 2.0 && a.residual <= 2.0 {
                out.push((at, ChangeKind::HighResidual));
            }
        }
        out
    }

    /// Largest centre displacement of the path, in sequence pixels.
    pub fn max_displacement(&self) -> f64 {
        self.samples.iter().map(|s| s.dx.hypot(s.dy)).fold(0.0, f64::max)
    }

    /// Zoom range of the path as (min, max) scale.
    pub fn scale_range(&self) -> (f64, f64) {
        self.samples.iter().fold((1.0f64, 1.0f64), |(lo, hi), s| (lo.min(s.scale), hi.max(s.scale)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(tick: i64, dx: f64, state: Option<&str>, residual: f64) -> ComponentSample {
        ComponentSample { tick, dx, dy: 0.0, scale: 1.0, rotation: 0.0, residual, state: state.map(str::to_string) }
    }

    #[test]
    fn changes_mark_motion_edges_gaps_and_high_residuals() {
        let c = AppliedComponent {
            id: ComponentId(1),
            kind: ComponentKind::Camera2d,
            name: "cam".into(),
            mode: ComponentMode::Follow,
            clip: ClipId(7),
            start: Tick(1000),
            duration: Tick(700),
            effect: "motion".into(),
            params: vec!["position".into()],
            source: ComponentSource::default(),
            samples: vec![
                sample(0, 0.0, None, 0.0),
                sample(100, 0.0, Some("static"), 0.0),
                sample(200, 2.0, Some("moving"), 0.0),
                sample(300, 4.0, Some("moving"), 3.0),
                sample(400, 4.0, Some("static"), 0.0),
                sample(500, 4.0, Some("noEvidence"), 0.0),
                sample(600, 6.0, Some("moving"), 0.0),
            ],
        };
        assert!(c.check().is_ok());
        assert_eq!(
            c.changes(),
            vec![
                (Tick(1200), ChangeKind::MotionStarts),
                (Tick(1300), ChangeKind::HighResidual),
                (Tick(1400), ChangeKind::MotionStops),
                (Tick(1500), ChangeKind::NoEvidence),
                (Tick(1600), ChangeKind::MotionStarts),
            ]
        );
        assert_eq!(c.end(), Tick(1700));
        assert_eq!(c.max_displacement(), 6.0);
        let mut bad = c.clone();
        bad.samples[3].tick = 150;
        assert!(bad.check().is_err());
        let mut bad = c.clone();
        bad.samples[1].dx = f64::NAN;
        assert!(bad.check().is_err());
        let mut bad = c;
        bad.duration = Tick::ZERO;
        assert!(bad.check().is_err());
    }
}

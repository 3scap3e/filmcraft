//! Video generation for FilmCraft: a [`VideoGenerator`] turns a prompt (and optionally a still)
//! into a clip on disk, reporting progress phase by phase, so the engine can import it and place
//! it on the timeline. The first generator is Reactor's FastH3 model (feature `reactor`).

pub mod hls;
#[cfg(feature = "reactor")]
pub mod reactor;
pub mod request;

pub use request::{GenerateError, GenerateRequest, GeneratedClip, ImageRole, Limits, Phase, Progress, VideoGenerator, fast_h3_limits, validate};

/// The id and name of the Reactor FastH3 generator, known to every build.
pub const FAST_H3_ID: &str = "reactor-fast-h3";
pub const FAST_H3_NAME: &str = "Reactor FastH3";

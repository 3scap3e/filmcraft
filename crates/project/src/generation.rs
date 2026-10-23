//! Generations: video clips made by an AI video model (the Generate panel).
//!
//! A [`Generation`] belongs to the project item of the MP4 that was generated
//! (`Project::generations`, keyed by the item id): the prompt, length, seed and provider that
//! made it, so the clip can be inspected, reproduced or described later.

use serde::{Deserialize, Serialize};

/// Longest prompt kept, in bytes.
pub const MAX_PROMPT_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Generation {
    /// Generator id (`reactor-fast-h3`).
    pub provider: String,
    pub prompt: String,
    /// The length the provider produced, in seconds.
    pub seconds: f64,
    /// How the optional still was used (`first`, `last`, `both`).
    pub image_role: Option<String>,
    /// Path of the still the clip opened from or ended on.
    pub image: Option<String>,
    pub seed: Option<u64>,
    /// The provider's own id of the clip.
    pub provider_clip_id: String,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    /// Seconds the provider's billed session was open.
    pub session_seconds: f64,
    /// When the clip was generated (Unix milliseconds).
    pub created_unix_ms: u64,
}

impl Default for Generation {
    fn default() -> Self {
        Generation {
            provider: String::new(),
            prompt: String::new(),
            seconds: 0.0,
            image_role: None,
            image: None,
            seed: None,
            provider_clip_id: String::new(),
            width: 0,
            height: 0,
            fps: 0.0,
            session_seconds: 0.0,
            created_unix_ms: 0,
        }
    }
}

impl Generation {
    /// Check values read from a project file or a command.
    pub fn check(&self) -> Result<(), String> {
        if self.prompt.len() > MAX_PROMPT_BYTES {
            return Err(format!("the prompt is longer than {} KB", MAX_PROMPT_BYTES / 1024));
        }
        if !self.seconds.is_finite() || self.seconds < 0.0 {
            return Err("bad clip length".into());
        }
        if !self.fps.is_finite() || self.fps < 0.0 {
            return Err("bad frame rate".into());
        }
        if !self.session_seconds.is_finite() || self.session_seconds < 0.0 {
            return Err("bad session length".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generations_round_trip_and_reject_bad_numbers() {
        let g = Generation {
            provider: "reactor-fast-h3".into(),
            prompt: "a cat".into(),
            seconds: 8.0,
            seed: Some(7),
            width: 1024,
            height: 768,
            fps: 24.0,
            ..Default::default()
        };
        assert!(g.check().is_ok());
        let v = serde_json::to_value(&g).unwrap();
        assert_eq!(v["providerClipId"], "");
        assert_eq!(serde_json::from_value::<Generation>(v).unwrap(), g);
        assert!(Generation { seconds: f64::NAN, ..g.clone() }.check().is_err());
        assert!(Generation { fps: -1.0, ..g.clone() }.check().is_err());
        assert!(Generation { prompt: "x".repeat(MAX_PROMPT_BYTES + 1), ..g }.check().is_err());
        let old: Generation = serde_json::from_str("{}").unwrap();
        assert_eq!(old, Generation::default());
    }
}

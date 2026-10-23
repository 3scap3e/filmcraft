//! Dialogue lines grouped from transcribed words (the words come from the speech layer above).

use serde::{Deserialize, Serialize};

/// Most words accepted in one grouping.
pub const MAX_WORDS: usize = 1_000_000;

/// A transcribed word with times in seconds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Word {
    pub text: String,
    pub start_seconds: f64,
    pub end_seconds: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<u32>,
    pub confidence: f32,
}

/// A line: consecutive words of one speaker without a long pause between them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Line {
    pub index: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<u32>,
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub text: String,
    pub words: usize,
    /// Mean word confidence, 0–1.
    pub confidence: f32,
}

/// Group `words` into lines: a new line starts at a speaker change or a pause longer than
/// `max_gap_seconds`. Words with non-finite or reversed times are skipped.
pub fn lines(words: &[Word], max_gap_seconds: f64) -> Vec<Line> {
    let gap = if max_gap_seconds.is_finite() { max_gap_seconds.max(0.0) } else { 0.6 };
    let mut out: Vec<Line> = Vec::new();
    let mut confidence_sum = 0.0f32;
    for w in words.iter().take(MAX_WORDS) {
        if !w.start_seconds.is_finite() || !w.end_seconds.is_finite() || w.end_seconds < w.start_seconds {
            continue;
        }
        let text = w.text.trim();
        if text.is_empty() {
            continue;
        }
        let continues = out.last().is_some_and(|l| l.speaker == w.speaker && w.start_seconds - l.end_seconds <= gap);
        match out.last_mut() {
            Some(l) if continues => {
                l.end_seconds = l.end_seconds.max(w.end_seconds);
                l.text.push(' ');
                l.text.push_str(text);
                l.words += 1;
                confidence_sum += w.confidence.clamp(0.0, 1.0);
                l.confidence = confidence_sum / l.words as f32;
            }
            _ => {
                confidence_sum = w.confidence.clamp(0.0, 1.0);
                out.push(Line {
                    index: out.len(),
                    speaker: w.speaker,
                    start_seconds: w.start_seconds,
                    end_seconds: w.end_seconds,
                    text: text.to_string(),
                    words: 1,
                    confidence: confidence_sum,
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, start: f64, end: f64, speaker: u32) -> Word {
        Word { text: text.into(), start_seconds: start, end_seconds: end, speaker: Some(speaker), confidence: 0.9 }
    }

    #[test]
    fn words_split_into_lines_at_pauses_and_speaker_changes() {
        let words = vec![
            word("Hello", 0.0, 0.3, 0),
            word("there", 0.35, 0.6, 0),
            word("Hi", 1.8, 2.0, 1),
            word("", 2.1, 2.2, 1),
            word("back", 2.2, 2.5, 1),
            word("broken", f64::NAN, 3.0, 1),
            word("late", 2.6, 2.5, 1),
        ];
        let grouped = lines(&words, 0.6);
        assert_eq!(grouped.len(), 2, "{grouped:#?}");
        assert_eq!(grouped[0].text, "Hello there");
        assert_eq!(grouped[1].text, "Hi back");
        assert_eq!(grouped[1].words, 2);
        assert_eq!(grouped[1].speaker, Some(1));
        assert!((grouped[1].confidence - 0.9).abs() < 1e-6);
        assert!(lines(&[], 0.6).is_empty());
    }
}

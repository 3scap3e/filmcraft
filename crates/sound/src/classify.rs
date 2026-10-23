//! Segments of fixed length labelled silence / speech / music / effects / ambience from the hop
//! features, under named heuristic rules, and the same labels merged into regions.

use super::features::{Hop, SILENCE_DB};
use super::{Options, Rule};
use serde::{Deserialize, Serialize};

/// What a stretch of the mix is made of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SegmentKind {
    Silence,
    Speech,
    Music,
    Effects,
    Ambience,
}

impl SegmentKind {
    pub fn label(self) -> &'static str {
        match self {
            SegmentKind::Silence => "silence",
            SegmentKind::Speech => "speech",
            SegmentKind::Music => "music",
            SegmentKind::Effects => "effects",
            SegmentKind::Ambience => "ambience",
        }
    }
}

/// One classified segment with the measurements it was judged on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    pub index: usize,
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub kind: SegmentKind,
    /// Margin of the winning score over the runner-up, 0–1.
    pub confidence: f32,
    /// Mean hop level, dBFS.
    pub mean_db: f32,
    /// Peak hop level, dBFS.
    pub peak_db: f32,
    /// Standard deviation of the hop level, dB: how much the level moves (speech moves most).
    pub modulation_db: f32,
    /// Share of hops more than 10 dB under the segment mean: pauses.
    pub pause_share: f32,
    pub centroid_hz: f32,
    pub flatness: f32,
    pub flux: f32,
    pub zcr: f32,
    pub low_share: f32,
    pub speech_share: f32,
    pub high_share: f32,
    /// Onset peaks per second.
    pub onset_rate: f32,
    pub scores: Scores,
}

/// The score of every kind, 0–1.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Scores {
    pub speech: f32,
    pub music: f32,
    pub effects: f32,
    pub ambience: f32,
}

/// Consecutive segments of one kind.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Region {
    pub index: usize,
    pub kind: SegmentKind,
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub segments: usize,
    pub mean_confidence: f32,
    pub mean_db: f32,
}

/// Hop levels are clamped here before the level statistics, so digital silence inside a segment
/// counts as a quiet pause rather than minus infinity.
const LEVEL_FLOOR_DB: f32 = -80.0;

fn ramp(x: f32, from: f32, to: f32) -> f32 {
    if (to - from).abs() < f32::EPSILON {
        return if x >= to { 1.0 } else { 0.0 };
    }
    ((x - from) / (to - from)).clamp(0.0, 1.0)
}

fn mean(values: impl Iterator<Item = f32>) -> f32 {
    let (mut s, mut n) = (0.0f64, 0usize);
    for v in values {
        s += f64::from(v);
        n += 1;
    }
    if n == 0 { 0.0 } else { (s / n as f64) as f32 }
}

/// The classified segments of `hops`.
pub fn segments(hops: &[Hop], hop_seconds: f64, segment_seconds: f64, floor_db: f64, total_seconds: f64) -> Vec<Segment> {
    if hops.is_empty() || hop_seconds <= 0.0 || segment_seconds <= 0.0 {
        return Vec::new();
    }
    let per = ((segment_seconds / hop_seconds).round() as usize).max(1);
    let onset_threshold = {
        let m = mean(hops.iter().map(|h| h.onset));
        m * 1.5
    };
    let floor = floor_db as f32;
    let mut out = Vec::with_capacity(hops.len().div_ceil(per));
    for (index, chunk) in hops.chunks(per).enumerate() {
        let start_seconds = index as f64 * per as f64 * hop_seconds;
        let end_seconds = (start_seconds + chunk.len() as f64 * hop_seconds).min(total_seconds.max(start_seconds));
        let levels: Vec<f32> = chunk.iter().map(|h| h.rms_db.max(LEVEL_FLOOR_DB)).collect();
        let energy = mean(levels.iter().map(|l| 10f32.powf(l / 10.0)));
        let mean_db = if energy > 0.0 { (10.0 * energy.log10()).max(LEVEL_FLOOR_DB) } else { LEVEL_FLOOR_DB };
        let peak_db = chunk.iter().map(|h| h.rms_db).fold(SILENCE_DB, f32::max);
        let level_mean = mean(levels.iter().copied());
        let modulation_db = mean(levels.iter().map(|l| (l - level_mean).powi(2))).sqrt();
        let pause_share = levels.iter().filter(|l| **l < mean_db - 10.0).count() as f32 / levels.len().max(1) as f32;
        let centroid_hz = mean(chunk.iter().map(|h| h.centroid_hz));
        let flatness = mean(chunk.iter().map(|h| h.flatness));
        let flux = mean(chunk.iter().map(|h| h.flux));
        let zcr = mean(chunk.iter().map(|h| h.zcr));
        let low_share = mean(chunk.iter().map(|h| h.low_share));
        let speech_share = mean(chunk.iter().map(|h| h.speech_share));
        let high_share = mean(chunk.iter().map(|h| h.high_share));
        let mut peaks = 0usize;
        for (i, h) in chunk.iter().enumerate() {
            let before = i.checked_sub(1).and_then(|j| chunk.get(j)).map_or(0.0, |p| p.onset);
            let after = chunk.get(i + 1).map_or(0.0, |n| n.onset);
            if h.onset > onset_threshold && h.onset >= before && h.onset > after {
                peaks += 1;
            }
        }
        let seconds = (chunk.len() as f64 * hop_seconds).max(1e-6);
        let onset_rate = (peaks as f64 / seconds) as f32;
        let silent = mean_db < -60.0 || mean_db < floor + 3.0;
        let scores = Scores {
            speech: 0.45 * ramp(modulation_db, 4.0, 10.0) + 0.3 * ramp(speech_share, 0.5, 0.8) + 0.25 * ramp(pause_share, 0.05, 0.25),
            music: 0.4 * ramp(modulation_db, 7.0, 2.0) + 0.35 * ramp(flatness, 0.15, 0.03) + 0.25 * ramp(low_share + high_share, 0.2, 0.5),
            effects: 0.4 * ramp(flatness, 0.15, 0.4) + 0.3 * ramp(peak_db - mean_db, 8.0, 18.0) + 0.3 * ramp(onset_rate, 1.0, 6.0),
            ambience: 0.4 * ramp(flatness, 0.15, 0.4) + 0.4 * ramp(modulation_db, 6.0, 1.5) + 0.2 * ramp(mean_db, floor + 25.0, floor + 10.0),
        };
        let (kind, confidence) = if silent {
            (SegmentKind::Silence, 1.0)
        } else {
            let mut ranked = [
                (SegmentKind::Speech, scores.speech),
                (SegmentKind::Music, scores.music),
                (SegmentKind::Effects, scores.effects),
                (SegmentKind::Ambience, scores.ambience),
            ];
            ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
            let top = ranked.first().copied().unwrap_or((SegmentKind::Effects, 0.0));
            let second = ranked.get(1).map_or(0.0, |r| r.1);
            (top.0, (top.1 - second).clamp(0.0, 1.0))
        };
        out.push(Segment {
            index,
            start_seconds,
            end_seconds,
            kind,
            confidence,
            mean_db,
            peak_db,
            modulation_db,
            pause_share,
            centroid_hz,
            flatness,
            flux,
            zcr,
            low_share,
            speech_share,
            high_share,
            onset_rate,
            scores,
        });
    }
    out
}

/// Consecutive segments of one kind, merged.
pub fn regions(segments: &[Segment]) -> Vec<Region> {
    let mut out: Vec<Region> = Vec::new();
    let mut confidence_sum = 0.0f32;
    let mut db_sum = 0.0f32;
    for s in segments {
        match out.last_mut() {
            Some(r) if r.kind == s.kind => {
                r.end_seconds = s.end_seconds;
                r.segments += 1;
                confidence_sum += s.confidence;
                db_sum += s.mean_db;
                r.mean_confidence = confidence_sum / r.segments as f32;
                r.mean_db = db_sum / r.segments as f32;
            }
            _ => {
                confidence_sum = s.confidence;
                db_sum = s.mean_db;
                out.push(Region {
                    index: out.len(),
                    kind: s.kind,
                    start_seconds: s.start_seconds,
                    end_seconds: s.end_seconds,
                    segments: 1,
                    mean_confidence: s.confidence,
                    mean_db: s.mean_db,
                });
            }
        }
    }
    out
}

/// The rules behind every label, as stated in the result.
pub fn rules(options: &Options) -> Vec<Rule> {
    let r = |name: &str, rule: String| Rule { name: name.into(), rule };
    vec![
        r("features", format!("Every {} samples a {}-sample Hann frame gives RMS level (dBFS), spectral centroid (Hz), spectral flatness (0 tone – 1 noise), flux, zero-crossing rate, power shares below {} Hz / {}–{} Hz / above {} Hz and an onset strength (positive change of log-compressed magnitudes).", super::features::HOP, super::features::FRAME, super::features::LOW_HZ, super::features::LOW_HZ, super::features::HIGH_HZ, super::features::HIGH_HZ)),
        r("segment", format!("Segments are about {:.2} s long (a whole number of hops); each is judged on the energy mean, peak and standard deviation (modulation) of its hop levels (clamped at −80 dBFS), its pause share (hops 10 dB under the mean), the means of the spectral features and its onset peaks per second.", options.segment_seconds)),
        r("floor", "The noise floor is the 10th percentile of hop level over hops above −100 dBFS.".into()),
        r("silence", "mean level < −60 dBFS, or < floor + 3 dB".into()),
        r("speech", "0.45 × ramp(modulation 4→10 dB) + 0.30 × ramp(speech-band share 0.5→0.8) + 0.25 × ramp(pause share 0.05→0.25)".into()),
        r("music", "0.40 × ramp(modulation 7→2 dB, steadier is higher) + 0.35 × ramp(flatness 0.15→0.03, more tonal is higher) + 0.25 × ramp(low + high band share 0.2→0.5)".into()),
        r("effects", "0.40 × ramp(flatness 0.15→0.4) + 0.30 × ramp(peak − mean 8→18 dB) + 0.30 × ramp(onset peaks 1→6 per second)".into()),
        r("ambience", "0.40 × ramp(flatness 0.15→0.4) + 0.40 × ramp(modulation 6→1.5 dB) + 0.20 × ramp(level floor+25→floor+10 dB, quieter is higher)".into()),
        r("label", "A non-silent segment takes the kind with the highest score; confidence is the margin over the runner-up. Regions merge consecutive segments of one kind.".into()),
        r("tempo", format!("Onset strength minus its 1 s moving mean, half-wave rectified, autocorrelated over lags for {:.0}–{:.0} BPM and weighted by a log-normal preference around 120 BPM; the beat grid is the best lag with the phase that maximises onset energy; confidence combines the peak's height over the mean autocorrelation and its ratio to lag 0.", options.min_bpm, options.max_bpm)),
        r("cutsOnBeats", format!("A cut is on a beat when a grid beat lies within {:.0} ms of it.", options.beat_tolerance_seconds * 1000.0)),
        r("loudness", "ITU-R BS.1770-4 / EBU R128: K-weighted, momentary 400 ms every 100 ms, integrated with the −70 LUFS absolute and −10 LU relative gates, loudness range per EBU Tech 3342, true peak by oversampling.".into()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hop(rms_db: f32, flatness: f32, speech_share: f32, onset: f32) -> Hop {
        Hop {
            rms_db,
            centroid_hz: 1000.0,
            flatness,
            flux: 0.1,
            zcr: 0.1,
            low_share: (1.0 - speech_share) / 2.0,
            speech_share,
            high_share: (1.0 - speech_share) / 2.0,
            onset,
        }
    }

    #[test]
    fn steady_tonal_hops_are_music_and_modulated_speech_band_hops_are_speech() {
        let music: Vec<Hop> = (0..100).map(|_| hop(-20.0, 0.02, 0.5, 0.1)).collect();
        let speech: Vec<Hop> = (0..100).map(|i| hop(if i % 10 < 6 { -18.0 } else { -40.0 }, 0.08, 0.9, 0.2)).collect();
        let silence: Vec<Hop> = (0..100).map(|_| hop(SILENCE_DB, 0.0, 0.0, 0.0)).collect();
        let mut all = music;
        all.extend(speech);
        all.extend(silence);
        let segs = segments(&all, 0.01, 1.0, -40.0, 3.0);
        let kinds: Vec<SegmentKind> = segs.iter().map(|s| s.kind).collect();
        assert_eq!(kinds, vec![SegmentKind::Music, SegmentKind::Speech, SegmentKind::Silence], "{segs:#?}");
        assert!(segs[0].confidence > 0.1);
        let regions = regions(&segs);
        assert_eq!(regions.len(), 3);
        assert_eq!(regions[2].start_seconds, 2.0);
        assert!(segments(&[], 0.01, 1.0, -40.0, 0.0).is_empty());
        assert!(segments(&all, 0.0, 1.0, -40.0, 3.0).is_empty());
        assert!(!rules(&Options::default()).is_empty());
    }
}

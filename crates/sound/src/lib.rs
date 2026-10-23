//! Sound analysis for the Reverse Engineering panel: what a mixed signal is made of, in numbers
//! that a person or a language model can read and that a generator can reproduce.
//!
//! Pure Rust, no model. One pass over the mix gives
//! * short-time features per hop ([`features`]): level, spectral centroid, flatness, flux, zero
//!   crossings, band shares and an onset strength;
//! * BS.1770 loudness ([`loudness`]): the momentary curve, integrated loudness, loudness range,
//!   true peak;
//! * one-second segments labelled silence / speech / music / effects / ambience under named
//!   heuristic rules ([`classify`]), merged into regions;
//! * tempo and a beat grid from the onset strength ([`tempo`]), and how many of a set of cuts land
//!   on beats;
//! * dialogue lines grouped from transcribed words ([`dialogue`]), the words themselves coming from
//!   the speech crate above this layer.
//!
//! Every number is a measurement of the mix handed in; every word is a classification of the
//! numbers next to it under the rules listed in [`Analysis::rules`]. Inputs are treated as hostile:
//! rates, channel counts and lengths are capped, nothing panics on an empty or odd signal.

pub mod classify;
pub mod dialogue;
pub mod features;
pub mod loudness;
pub mod tempo;

use serde::{Deserialize, Serialize};

pub use classify::{Region, Segment, SegmentKind};
pub use dialogue::{Line, Word};
pub use features::Hop;
pub use loudness::Loudness;
pub use tempo::{CutsOnBeats, Tempo};

/// Longest signal analysed, in seconds.
pub const MAX_SECONDS: u32 = 3600;
/// Most channels accepted.
pub const MAX_CHANNELS: usize = 8;
/// Sample rates accepted (Hz).
pub const SAMPLE_RATE_RANGE: std::ops::RangeInclusive<u32> = 8_000..=192_000;
/// Identifier recorded with every result.
pub const ANALYZER: &str = "filmcraft-sound/mix-v1";

/// Tunable thresholds of the analysis.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Options {
    /// Length of one classified segment in seconds (0.25–10).
    pub segment_seconds: f64,
    /// Slowest tempo searched (BPM).
    pub min_bpm: f64,
    /// Fastest tempo searched (BPM).
    pub max_bpm: f64,
    /// A cut counts as on a beat within this many seconds of one.
    pub beat_tolerance_seconds: f64,
}

impl Default for Options {
    fn default() -> Self {
        Options { segment_seconds: 1.0, min_bpm: 60.0, max_bpm: 200.0, beat_tolerance_seconds: 0.08 }
    }
}

impl Options {
    pub fn validate(&self) -> Result<(), String> {
        let finite = [self.segment_seconds, self.min_bpm, self.max_bpm, self.beat_tolerance_seconds].iter().all(|v| v.is_finite());
        if !finite {
            return Err("sound options must be finite numbers".into());
        }
        if !(0.25..=10.0).contains(&self.segment_seconds) {
            return Err("segment length must be 0.25–10 seconds".into());
        }
        if !(30.0..=300.0).contains(&self.min_bpm) || !(30.0..=300.0).contains(&self.max_bpm) || self.min_bpm >= self.max_bpm {
            return Err("tempo range must be 30–300 BPM with min below max".into());
        }
        if !(0.0..=1.0).contains(&self.beat_tolerance_seconds) {
            return Err("beat tolerance must be 0–1 seconds".into());
        }
        Ok(())
    }
}

/// A named heuristic rule, as stated in the result.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    pub name: String,
    pub rule: String,
}

/// The complete result of [`analyze`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Analysis {
    pub analyzer: String,
    pub sample_rate: u32,
    pub channels: usize,
    pub seconds: f64,
    /// Seconds between two feature hops.
    pub hop_seconds: f64,
    pub loudness: Loudness,
    /// The signal's noise floor: the 10th percentile of hop level over the non-digital-silence hops, dBFS.
    pub floor_db: f64,
    pub segments: Vec<Segment>,
    pub regions: Vec<Region>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tempo: Option<Tempo>,
    /// Time share of each kind over the whole signal, 0–1.
    pub shares: Shares,
    pub summary: String,
    pub rules: Vec<Rule>,
    pub limitations: Vec<String>,
}

/// Time shares of the segment kinds, 0–1, summing to 1 over a nonempty signal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Shares {
    pub silence: f64,
    pub speech: f64,
    pub music: f64,
    pub effects: f64,
    pub ambience: f64,
}

impl Shares {
    pub fn get(&self, kind: SegmentKind) -> f64 {
        match kind {
            SegmentKind::Silence => self.silence,
            SegmentKind::Speech => self.speech,
            SegmentKind::Music => self.music,
            SegmentKind::Effects => self.effects,
            SegmentKind::Ambience => self.ambience,
        }
    }

    fn add(&mut self, kind: SegmentKind, seconds: f64) {
        match kind {
            SegmentKind::Silence => self.silence += seconds,
            SegmentKind::Speech => self.speech += seconds,
            SegmentKind::Music => self.music += seconds,
            SegmentKind::Effects => self.effects += seconds,
            SegmentKind::Ambience => self.ambience += seconds,
        }
    }

    fn normalized(mut self, total: f64) -> Self {
        if total > 0.0 {
            self.silence /= total;
            self.speech /= total;
            self.music /= total;
            self.effects /= total;
            self.ambience /= total;
        }
        self
    }

    /// The kind with the largest share (a tie goes to the sounding kind, not to silence).
    pub fn dominant(&self) -> SegmentKind {
        let mut best = (SegmentKind::Silence, self.silence);
        for kind in [SegmentKind::Speech, SegmentKind::Music, SegmentKind::Effects, SegmentKind::Ambience] {
            let v = self.get(kind);
            if v > best.1 || (v == best.1 && best.0 == SegmentKind::Silence && v > 0.0) {
                best = (kind, v);
            }
        }
        best.0
    }

    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        for kind in [SegmentKind::Speech, SegmentKind::Music, SegmentKind::Effects, SegmentKind::Ambience, SegmentKind::Silence] {
            let v = self.get(kind);
            if v >= 0.005 {
                parts.push(format!("{} {:.0} %", kind.label(), v * 100.0));
            }
        }
        if parts.is_empty() { "no sound".into() } else { parts.join(", ") }
    }
}

/// What a time span of an analysed signal sounds like: for one shot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RangeSound {
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub shares: Shares,
    pub dominant: SegmentKind,
    /// Energy mean of the momentary loudness over the span, LUFS (None when silent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mean_lufs: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_momentary_lufs: Option<f64>,
    pub beats: usize,
    pub summary: String,
}

/// Progress callback: `(done, total)` hops; return false to cancel.
pub type Progress<'a> = &'a mut dyn FnMut(usize, usize) -> bool;

fn check_signal(channels: &[&[f32]], sample_rate: u32) -> Result<usize, String> {
    if !SAMPLE_RATE_RANGE.contains(&sample_rate) {
        return Err(format!("sample rate {sample_rate} Hz is outside {}–{} Hz", SAMPLE_RATE_RANGE.start(), SAMPLE_RATE_RANGE.end()));
    }
    if channels.is_empty() || channels.len() > MAX_CHANNELS {
        return Err(format!("1–{MAX_CHANNELS} channels are analysed"));
    }
    let len = channels.iter().map(|c| c.len()).min().unwrap_or(0);
    if len == 0 {
        return Err("the signal is empty".into());
    }
    let max = (sample_rate as usize).saturating_mul(MAX_SECONDS as usize);
    if len > max {
        return Err(format!("the signal is longer than {MAX_SECONDS} seconds"));
    }
    Ok(len)
}

/// Mean of the channels, sample by sample, over their common length.
pub fn downmix(channels: &[&[f32]], len: usize) -> Vec<f32> {
    let k = 1.0 / channels.len().max(1) as f32;
    (0..len).map(|i| channels.iter().map(|c| c.get(i).copied().unwrap_or(0.0)).sum::<f32>() * k).collect()
}

/// Analyse a planar signal (one slice per channel, equal lengths).
pub fn analyze(channels: &[&[f32]], sample_rate: u32, options: &Options, progress: Progress) -> Result<Analysis, String> {
    options.validate()?;
    let len = check_signal(channels, sample_rate)?;
    let seconds = len as f64 / sample_rate as f64;
    let mono = downmix(channels, len);
    let hop_seconds = features::HOP as f64 / sample_rate as f64;
    let hops = features::extract(&mono, sample_rate, progress)?;
    let loud = loudness::measure(channels, sample_rate, len);
    let floor_db = features::noise_floor(&hops);
    let onset: Vec<f32> = hops.iter().map(|h| h.onset).collect();
    let tempo = tempo::estimate(&onset, hop_seconds, options.min_bpm, options.max_bpm);
    let segments = classify::segments(&hops, hop_seconds, options.segment_seconds, floor_db, seconds);
    let regions = classify::regions(&segments);
    let mut shares = Shares::default();
    let mut total = 0.0;
    for s in &segments {
        let d = (s.end_seconds - s.start_seconds).max(0.0);
        shares.add(s.kind, d);
        total += d;
    }
    let shares = shares.normalized(total);
    let summary = summary_text(sample_rate, channels.len(), seconds, &loud, &shares, tempo.as_ref());
    Ok(Analysis {
        analyzer: ANALYZER.into(),
        sample_rate,
        channels: channels.len(),
        seconds,
        hop_seconds,
        loudness: loud,
        floor_db,
        segments,
        regions,
        tempo,
        shares,
        summary,
        rules: classify::rules(options),
        limitations: limitations(),
    })
}

fn summary_text(sample_rate: u32, channels: usize, seconds: f64, loud: &Loudness, shares: &Shares, tempo: Option<&Tempo>) -> String {
    let layout = match channels {
        1 => "mono".to_string(),
        2 => "stereo".to_string(),
        n => format!("{n} channels"),
    };
    let mut parts = vec![format!("{:.1} kHz {layout} · {seconds:.1} s", sample_rate as f64 / 1000.0)];
    match loud.integrated_lufs {
        Some(i) => parts.push(format!(
            "{i:.1} LUFS integrated, range {:.1} LU, true peak {}",
            loud.range_lu,
            loud.true_peak_dbtp.map(|p| format!("{p:.1} dBTP")).unwrap_or_else(|| "silent".into())
        )),
        None => parts.push("below the loudness gate (silent)".into()),
    }
    parts.push(shares.describe());
    if let Some(t) = tempo {
        parts.push(format!("{:.0} BPM (confidence {:.2}), {} beats", t.bpm, t.confidence, t.beats.len()));
    } else {
        parts.push("no steady tempo".into());
    }
    parts.join(" · ")
}

fn limitations() -> Vec<String> {
    vec![
        "The analysis runs on the sequence mix handed in (all audio tracks together), not on separate stems: a voice over music is classified by whichever dominates each second.".into(),
        "Speech, music, effects and ambience are heuristic labels from level modulation, spectral flatness, band shares and onsets under the rules listed; they are not a trained classifier and can confuse sung vocals with speech or dense percussion with effects.".into(),
        "Tempo comes from the autocorrelation of the onset strength, preferring tempi near 120 BPM; a half or double tempo is a known failure mode, and the beat grid is a constant period aligned once, so it drifts on material whose tempo changes.".into(),
        "Loudness follows ITU-R BS.1770-4 / EBU R128 on the mix as rendered at the sequence's sample rate.".into(),
    ]
}

/// What the signal sounds like between two times (seconds): kind shares, loudness and beats.
pub fn range_sound(analysis: &Analysis, start_seconds: f64, end_seconds: f64) -> RangeSound {
    let start = start_seconds.max(0.0).min(analysis.seconds);
    let end = end_seconds.max(start).min(analysis.seconds);
    let mut shares = Shares::default();
    let mut total = 0.0;
    for s in &analysis.segments {
        let overlap = (s.end_seconds.min(end) - s.start_seconds.max(start)).max(0.0);
        if overlap > 0.0 {
            shares.add(s.kind, overlap);
            total += overlap;
        }
    }
    let shares = shares.normalized(total);
    let step = analysis.loudness.curve_seconds;
    let mut energy = 0.0;
    let mut n = 0usize;
    let mut max = f64::NEG_INFINITY;
    if step > 0.0 {
        let first = (start / step).floor().max(0.0) as usize;
        let last = ((end / step).ceil().max(0.0) as usize).min(analysis.loudness.momentary_curve.len());
        for v in analysis.loudness.momentary_curve.iter().skip(first).take(last.saturating_sub(first)) {
            let v = f64::from(*v);
            if v.is_finite() && v > -120.0 {
                energy += 10f64.powf(v / 10.0);
                n += 1;
                max = max.max(v);
            }
        }
    }
    let mean_lufs = (n > 0 && energy > 0.0).then(|| 10.0 * (energy / n as f64).log10());
    let max_momentary_lufs = max.is_finite().then_some(max);
    let beats = analysis.tempo.as_ref().map_or(0, |t| t.beats.iter().filter(|b| **b >= start && **b < end).count());
    let dominant = shares.dominant();
    let mut summary = shares.describe();
    if let Some(m) = mean_lufs {
        summary.push_str(&format!(" · {m:.1} LUFS"));
    }
    if beats > 0 {
        summary.push_str(&format!(" · {beats} beat{}", if beats == 1 { "" } else { "s" }));
    }
    RangeSound { start_seconds: start, end_seconds: end, shares, dominant, mean_lufs, max_momentary_lufs, beats, summary }
}

#[cfg(test)]
pub(crate) mod synth {
    //! Deterministic test signals.

    /// A tiny linear congruential generator for reproducible noise.
    pub struct Lcg(pub u64);

    impl Lcg {
        pub fn next_f32(&mut self) -> f32 {
            self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            ((self.0 >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
        }
    }

    pub fn noise(len: usize, amplitude: f32, seed: u64) -> Vec<f32> {
        let mut g = Lcg(seed);
        (0..len).map(|_| g.next_f32() * amplitude).collect()
    }

    /// A sustained chord with light vibrato: tonal, steady, wide.
    pub fn music(len: usize, sr: u32, amplitude: f32) -> Vec<f32> {
        let freqs = [110.0f32, 220.0, 330.0, 440.0, 660.0, 880.0, 1760.0, 3520.0];
        (0..len)
            .map(|i| {
                let t = i as f32 / sr as f32;
                let vib = 1.0 + 0.003 * (2.0 * std::f32::consts::PI * 5.0 * t).sin();
                let s: f32 = freqs.iter().enumerate().map(|(k, f)| (2.0 * std::f32::consts::PI * f * vib * t).sin() / (k as f32 + 1.0)).sum();
                s * amplitude * 0.35
            })
            .collect()
    }

    /// Band-limited bursts at a syllable rate with pauses: speech-like level modulation.
    pub fn speech(len: usize, sr: u32, amplitude: f32) -> Vec<f32> {
        let mut g = Lcg(7);
        let mut state = [0.0f32; 4];
        (0..len)
            .map(|i| {
                let t = i as f32 / sr as f32;
                let syllable = (2.0 * std::f32::consts::PI * 4.0 * t).sin().max(0.0).powi(2);
                let word = if (t * 0.7).fract() < 0.6 { 1.0 } else { 0.0 };
                let x = g.next_f32();
                state[0] = 0.7 * state[0] + 0.3 * x;
                state[1] = 0.5 * state[1] + 0.5 * state[0];
                let voiced = [(160.0, 0.4), (480.0, 0.3), (800.0, 0.4), (1200.0, 0.3), (2400.0, 0.15)]
                    .iter()
                    .map(|(f, a)| (2.0 * std::f32::consts::PI * f * t).sin() * a)
                    .sum::<f32>();
                (voiced * 0.6 + state[1] * 1.2) * syllable * word * amplitude
            })
            .collect()
    }

    /// Sharp clicks every `period` samples on a quiet noise bed.
    pub fn clicks(len: usize, period: usize, amplitude: f32) -> Vec<f32> {
        let mut out = noise(len, 0.002, 3);
        let period = period.max(1);
        for (i, v) in out.iter_mut().enumerate() {
            let phase = i % period;
            if phase < 64 {
                *v += amplitude * (1.0 - phase as f32 / 64.0);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostile_inputs_are_errors_not_panics() {
        let mut p = |_: usize, _: usize| true;
        assert!(analyze(&[], 48_000, &Options::default(), &mut p).is_err());
        assert!(analyze(&[&[][..]], 48_000, &Options::default(), &mut p).is_err());
        assert!(analyze(&[&[0.0f32; 10][..]], 100, &Options::default(), &mut p).is_err());
        let bad = Options { min_bpm: 250.0, max_bpm: 100.0, ..Options::default() };
        assert!(analyze(&[&[0.0f32; 10][..]], 48_000, &bad, &mut p).is_err());
        let odd: Vec<f32> = vec![f32::NAN, f32::INFINITY, -1e30, 1e30, 0.0];
        let r = analyze(&[&odd[..]], 8_000, &Options::default(), &mut p);
        assert!(r.is_ok(), "{r:?}");
    }

    #[test]
    fn a_single_sample_and_a_cancelled_run_are_handled() {
        let mut p = |_: usize, _: usize| true;
        let r = analyze(&[&[0.5f32][..]], 48_000, &Options::default(), &mut p).expect("one sample");
        assert_eq!(r.segments.len(), 1);
        assert!(r.tempo.is_none());
        let mut cancel = |_: usize, _: usize| false;
        assert!(analyze(&[&[0.5f32; 48_000][..]], 48_000, &Options::default(), &mut cancel).is_err());
    }

    #[test]
    fn music_speech_and_silence_are_told_apart_and_summed_into_shares() {
        let sr = 16_000;
        let mut signal = synth::music(sr as usize * 4, sr, 0.5);
        signal.extend(synth::speech(sr as usize * 4, sr, 0.5));
        signal.extend(std::iter::repeat_n(0.0, sr as usize * 2));
        let mut p = |_: usize, _: usize| true;
        let a = analyze(&[&signal[..]], sr, &Options::default(), &mut p).expect("analysis");
        assert_eq!(a.segments.len(), 10, "{:?}", a.segments.iter().map(|s| s.kind).collect::<Vec<_>>());
        let kinds: Vec<SegmentKind> = a.segments.iter().map(|s| s.kind).collect();
        assert!(kinds[..4].iter().filter(|k| **k == SegmentKind::Music).count() >= 3, "{kinds:?}");
        assert!(kinds[4..8].iter().filter(|k| **k == SegmentKind::Speech).count() >= 3, "{kinds:?}");
        assert!(kinds[8..].iter().all(|k| *k == SegmentKind::Silence), "{kinds:?}");
        let total = a.shares.silence + a.shares.speech + a.shares.music + a.shares.effects + a.shares.ambience;
        assert!((total - 1.0).abs() < 1e-9);
        assert!(a.shares.silence > 0.15 && a.shares.silence < 0.25);
        assert!(a.loudness.integrated_lufs.is_some());
        assert!(!a.regions.is_empty() && a.regions.len() <= a.segments.len());
        let music = range_sound(&a, 0.0, 4.0);
        assert_eq!(music.dominant, SegmentKind::Music, "{music:?}");
        assert!(music.mean_lufs.is_some());
        let quiet = range_sound(&a, 8.0, 10.0);
        assert_eq!(quiet.dominant, SegmentKind::Silence);
        assert!(serde_json::to_string(&a).is_ok());
    }

    #[test]
    fn clicks_give_a_tempo_and_cuts_on_beats_are_counted() {
        let sr = 16_000;
        let period = sr as usize / 2;
        let signal = synth::clicks(sr as usize * 12, period, 0.8);
        let mut p = |_: usize, _: usize| true;
        let a = analyze(&[&signal[..]], sr, &Options::default(), &mut p).expect("analysis");
        let t = a.tempo.as_ref().expect("tempo");
        assert!((t.bpm - 120.0).abs() < 3.0, "{t:?}");
        assert!(t.confidence > 0.5, "{t:?}");
        let on = tempo::cuts_on_beats(&[1.0, 2.5, 3.21, 5.0], &t.beats, 0.08);
        assert_eq!(on.total, 4);
        assert_eq!(on.on_beat, 3, "{on:?}");
        let noise = synth::noise(sr as usize * 6, 0.3, 11);
        let n = analyze(&[&noise[..]], sr, &Options::default(), &mut p).expect("analysis");
        assert!(n.tempo.as_ref().is_none_or(|t| t.confidence < 0.5), "{:?}", n.tempo);
    }
}

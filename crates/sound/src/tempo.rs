//! Tempo and a beat grid from the onset strength, and how a set of cuts relates to the beats.

use serde::{Deserialize, Serialize};

/// Most beats kept in a grid.
pub const MAX_BEATS: usize = 1_000_000;

/// A steady tempo found in the signal.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tempo {
    pub bpm: f64,
    /// Beat period, seconds.
    pub period_seconds: f64,
    /// 0–1: how much the onsets repeat at that period.
    pub confidence: f64,
    /// Times of the beat grid, seconds, from the first aligned beat to the end.
    pub beats: Vec<f64>,
}

/// How a set of cut times relates to a beat grid.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CutsOnBeats {
    pub total: usize,
    pub on_beat: usize,
    pub tolerance_seconds: f64,
    /// Share of cuts on a beat, 0–1.
    pub share: f64,
    /// Signed distance from every cut to its nearest beat, ms (positive: the cut is late).
    pub offsets_ms: Vec<f64>,
}

/// Estimate the tempo of an onset-strength sequence sampled every `hop_seconds`.
pub fn estimate(onset: &[f32], hop_seconds: f64, min_bpm: f64, max_bpm: f64) -> Option<Tempo> {
    if onset.len() < 8 || !positive(hop_seconds) || !positive(min_bpm) || !positive(max_bpm - min_bpm) {
        return None;
    }
    let window = ((1.0 / hop_seconds).round() as usize).clamp(1, onset.len());
    let mut local: Vec<f32> = Vec::with_capacity(onset.len());
    let mut sum = 0.0f64;
    let mut queue: std::collections::VecDeque<f32> = std::collections::VecDeque::with_capacity(window + 1);
    for &o in onset {
        let o = if o.is_finite() { o.max(0.0) } else { 0.0 };
        queue.push_back(o);
        sum += f64::from(o);
        if queue.len() > window
            && let Some(gone) = queue.pop_front()
        {
            sum -= f64::from(gone);
        }
        let mean = sum / queue.len().max(1) as f64;
        local.push((f64::from(o) - mean).max(0.0) as f32);
    }
    let energy: f64 = local.iter().map(|v| f64::from(*v) * f64::from(*v)).sum();
    if energy <= 1e-12 {
        return None;
    }
    let lag_min = ((60.0 / (max_bpm * hop_seconds)).round() as usize).max(1);
    let lag_max = ((60.0 / (min_bpm * hop_seconds)).round() as usize).max(lag_min + 1);
    if lag_max >= local.len() / 2 {
        return None;
    }
    let acf = |lag: usize| -> f64 {
        let n = local.len().saturating_sub(lag);
        if n == 0 {
            return 0.0;
        }
        let s: f64 = local.iter().zip(local.iter().skip(lag)).map(|(a, b)| f64::from(*a) * f64::from(*b)).sum();
        s / n as f64
    };
    let acf0 = acf(0);
    if acf0 <= 0.0 {
        return None;
    }
    let mut best: Option<(usize, f64, f64)> = None;
    let mut mean_acf = 0.0f64;
    let mut count = 0usize;
    for lag in lag_min..=lag_max {
        let a = acf(lag);
        let bpm = 60.0 / (lag as f64 * hop_seconds);
        let w = (-0.5 * ((bpm / 120.0).log2() / 0.9).powi(2)).exp();
        mean_acf += a;
        count += 1;
        let score = a * w;
        if best.is_none_or(|b| score > b.2) {
            best = Some((lag, a, score));
        }
    }
    let (lag, peak, _) = best?;
    let mean_acf = mean_acf / count.max(1) as f64;
    let a_prev = if lag > 0 { acf(lag - 1) } else { peak };
    let a_next = acf(lag + 1);
    let denom = a_prev - 2.0 * peak + a_next;
    let shift = if denom.abs() > 1e-12 { (0.5 * (a_prev - a_next) / denom).clamp(-0.5, 0.5) } else { 0.0 };
    let period_hops = lag as f64 + shift;
    let period_seconds = period_hops * hop_seconds;
    if !positive(period_seconds) {
        return None;
    }
    let prominence = ((peak - mean_acf) / peak.max(1e-12)).clamp(0.0, 1.0);
    let ratio = (peak / acf0 / 0.4).clamp(0.0, 1.0);
    let confidence = (prominence * ratio).clamp(0.0, 1.0);
    let chain = track_beats(&local, period_hops);
    let beats: Vec<f64> = chain.iter().take(MAX_BEATS).map(|t| ((*t as f64 + ONSET_LAG_HOPS) * hop_seconds).min(local.len() as f64 * hop_seconds)).collect();
    let mut intervals: Vec<f64> = beats.windows(2).map(|w| w[1] - w[0]).filter(|d| *d > 0.0).collect();
    let period_seconds = if intervals.is_empty() {
        period_seconds
    } else {
        intervals.sort_by(f64::total_cmp);
        intervals.get(intervals.len() / 2).copied().unwrap_or(period_seconds)
    };
    if !positive(period_seconds) {
        return None;
    }
    Some(Tempo { bpm: 60.0 / period_seconds, period_seconds, confidence, beats })
}

fn positive(x: f64) -> bool {
    x.is_finite() && x > 0.0
}

/// An onset lands in the frame whose tail first covers it, so its hop starts about one and a half
/// hops before the onset itself ([`super::features::FRAME`] is two hops).
const ONSET_LAG_HOPS: f64 = 1.5;

/// Dynamic-programming beat tracking (Ellis 2007): every hop's score is its onset strength plus
/// the best earlier beat between half and twice the period back, penalised by how far that gap
/// strays from the period; the best chain is traced back from the end.
fn track_beats(local: &[f32], period_hops: f64) -> Vec<usize> {
    let n = local.len();
    let max = local.iter().copied().fold(0.0f32, f32::max);
    if n == 0 || !positive(f64::from(max)) || !period_hops.is_finite() || period_hops < 1.0 {
        return Vec::new();
    }
    let alpha = 100.0f64;
    let lo = ((period_hops * 0.5).round() as usize).max(1);
    let hi = ((period_hops * 2.0).round() as usize).max(lo + 1);
    let mut score = vec![0.0f64; n];
    let mut back: Vec<Option<usize>> = vec![None; n];
    for t in 0..n {
        let o = f64::from(local.get(t).copied().unwrap_or(0.0) / max);
        let mut best: Option<(usize, f64)> = None;
        for d in lo..=hi {
            let Some(tau) = t.checked_sub(d) else { break };
            let s = score.get(tau).copied().unwrap_or(0.0) - alpha * (d as f64 / period_hops).ln().powi(2);
            if best.is_none_or(|b| s > b.1) {
                best = Some((tau, s));
            }
        }
        let (value, link) = match best {
            Some((tau, s)) if s > 0.0 => (o + s, Some(tau)),
            _ => (o, None),
        };
        if let (Some(sc), Some(bk)) = (score.get_mut(t), back.get_mut(t)) {
            *sc = value;
            *bk = link;
        }
    }
    let from = n.saturating_sub(hi);
    let mut end = from;
    for t in from..n {
        if score.get(t).copied().unwrap_or(0.0) > score.get(end).copied().unwrap_or(0.0) {
            end = t;
        }
    }
    let mut chain = Vec::new();
    let mut t = end;
    while chain.len() <= n {
        chain.push(t);
        match back.get(t).copied().flatten() {
            Some(tau) if tau < t => t = tau,
            _ => break,
        }
    }
    chain.reverse();
    chain
}

/// Which of `cuts` (seconds) fall within `tolerance` of a beat.
pub fn cuts_on_beats(cuts: &[f64], beats: &[f64], tolerance: f64) -> CutsOnBeats {
    let tolerance = if tolerance.is_finite() { tolerance.max(0.0) } else { 0.0 };
    let mut offsets_ms = Vec::with_capacity(cuts.len());
    let mut on_beat = 0usize;
    for &cut in cuts {
        if !cut.is_finite() || beats.is_empty() {
            offsets_ms.push(f64::NAN);
            continue;
        }
        let i = beats.partition_point(|b| *b < cut);
        let candidates = [i.checked_sub(1).and_then(|j| beats.get(j)), beats.get(i)];
        let nearest = candidates.into_iter().flatten().copied().min_by(|a, b| (cut - a).abs().total_cmp(&(cut - b).abs()));
        match nearest {
            Some(b) => {
                let offset = cut - b;
                if offset.abs() <= tolerance {
                    on_beat += 1;
                }
                offsets_ms.push(offset * 1000.0);
            }
            None => offsets_ms.push(f64::NAN),
        }
    }
    let offsets_ms: Vec<f64> = offsets_ms.into_iter().map(|o| if o.is_finite() { o } else { 0.0 }).collect();
    let total = cuts.len();
    CutsOnBeats { total, on_beat, tolerance_seconds: tolerance, share: if total > 0 { on_beat as f64 / total as f64 } else { 0.0 }, offsets_ms }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_periodic_onset_train_gives_its_tempo_and_grid() {
        let hop = 0.01;
        let onset: Vec<f32> = (0..2000).map(|i| if i % 50 == 7 { 1.0 } else { 0.02 }).collect();
        let t = estimate(&onset, hop, 60.0, 200.0).expect("tempo");
        assert!((t.bpm - 120.0).abs() < 1.0, "{t:?}");
        assert!(t.confidence > 0.5, "{t:?}");
        assert!((t.beats[0] - 0.085).abs() < 0.011, "{:?}", &t.beats[..3]);
        assert!(t.beats.len() >= 39 && t.beats.len() <= 41, "{}", t.beats.len());
        let on = cuts_on_beats(&[0.085, 0.65, 1.085, f64::NAN], &t.beats, 0.03);
        assert_eq!(on.on_beat, 2, "{on:?}");
        assert_eq!(on.offsets_ms.len(), 4);
        assert!(cuts_on_beats(&[1.0], &[], 0.1).on_beat == 0);
    }

    #[test]
    fn flat_or_short_onsets_give_no_tempo() {
        assert!(estimate(&[0.0; 2000], 0.01, 60.0, 200.0).is_none());
        assert!(estimate(&[1.0; 4], 0.01, 60.0, 200.0).is_none());
        assert!(estimate(&[1.0; 2000], 0.0, 60.0, 200.0).is_none());
        assert!(estimate(&[1.0; 2000], 0.01, 200.0, 60.0).is_none());
        let nan = vec![f32::NAN; 2000];
        assert!(estimate(&nan, 0.01, 60.0, 200.0).is_none());
    }
}

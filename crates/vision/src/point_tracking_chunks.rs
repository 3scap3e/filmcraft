//! Bounded overlapping tracker windows, carrying visible point identities in both directions.
use crate::{
    execution::Device,
    point_tracking::{self, Inference, Observation, Query},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub query: Query,
    pub observations: Vec<Option<Observation>>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chunk {
    pub start_frame: usize,
    pub end_frame: usize,
    pub reference_frame: usize,
    pub direction: String,
    pub point_indices: Vec<usize>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Result {
    pub analyzer: String,
    pub frame_count: usize,
    pub tracks: Vec<Track>,
    pub runtime: Value,
    #[serde(default)]
    pub chunks: Vec<Chunk>,
}
pub type Frame = (usize, usize, Vec<u8>);
/// Injected inference enables deterministic boundary/cancellation tests without downloading a model.
pub fn track_with<F, I, P>(
    count: usize,
    queries: Vec<Query>,
    cancel: &AtomicBool,
    window: usize,
    mut render: F,
    mut infer: I,
    mut progress: P,
) -> std::result::Result<Result, String>
where
    F: FnMut(usize) -> std::result::Result<Frame, String>,
    I: FnMut(Vec<Vec<u8>>, usize, usize, Vec<Query>) -> std::result::Result<Inference, String>,
    P: FnMut(usize, usize),
{
    if cancel.load(Ordering::Relaxed) {
        return Err("Point tracking cancelled".into());
    }
    point_tracking::validate_queries(&queries, count)?;
    if !(2..=point_tracking::MAX_FRAMES).contains(&window) {
        return Err("Invalid tracking chunk size".into());
    }
    let bytes =
        count.checked_mul(queries.len()).and_then(|n| n.checked_mul(std::mem::size_of::<Option<Observation>>())).ok_or("Tracking result size overflow")?;
    if bytes > 512 * 1024 * 1024 {
        return Err("Tracking output exceeds the 512 MB working-memory budget; fewer query points reduce storage while preserving the full range".into());
    }
    let mut tracks = Vec::new();
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (i, query) in queries.iter().enumerate() {
        let mut observations = Vec::new();
        observations.try_reserve_exact(count).map_err(|e| format!("Cannot allocate full-range tracking output: {e}"))?;
        observations.resize(count, None);
        tracks.push(Track { query: query.clone(), observations });
        groups.entry(query.frame_index).or_default().push(i);
    }
    let total = count.checked_mul(groups.len()).ok_or("Progress size overflow")?;
    let mut done = 0usize;
    let mut chunks = Vec::new();
    let mut runtime = Value::Null;
    for (&anchor, indices) in &groups {
        for forward in [true, false] {
            if forward && anchor == count - 1 || !forward && anchor == 0 {
                continue;
            }
            let mut boundary = anchor;
            let mut active: Vec<_> = indices.iter().map(|i| (*i, queries[*i].position)).collect();
            loop {
                if cancel.load(Ordering::Relaxed) {
                    return Err("Point tracking cancelled".into());
                }
                let (start, end) =
                    if forward { (boundary, boundary.saturating_add(window - 1).min(count - 1)) } else { (boundary.saturating_sub(window - 1), boundary) };
                if active.is_empty() {
                    let skipped = if forward { count - 1 - boundary } else { boundary };
                    done = done.saturating_add(skipped).min(total);
                    progress(done, total);
                    break;
                }
                let mut frames = Vec::new();
                let mut dimensions = None;
                for frame in start..=end {
                    if cancel.load(Ordering::Relaxed) {
                        return Err("Point tracking cancelled".into());
                    }
                    let (w, h, pixels) = render(frame)?;
                    if !(64..=512).contains(&w)
                        || !(64..=512).contains(&h)
                        || pixels.len() != w.checked_mul(h).and_then(|n| n.checked_mul(4)).ok_or("Tracking frame overflow")?
                    {
                        return Err("Invalid tracking chunk frame".into());
                    }
                    if dimensions.is_some_and(|size| size != (w, h)) {
                        return Err("Tracking frame dimensions changed".into());
                    }
                    dimensions = Some((w, h));
                    frames.push(pixels);
                }
                let reference = if forward { 0 } else { end - start };
                let local_queries = active.iter().map(|(_, position)| Query { frame_index: reference, position: *position }).collect();
                let (w, h) = dimensions.ok_or("Empty tracking chunk")?;
                let result = infer(frames, w, h, local_queries)?;
                if result.frame_count != end - start + 1 || result.tracks.len() != active.len() {
                    return Err("Chunk output dimensions do not match its inputs".into());
                }
                runtime = result.runtime;
                chunks.push(Chunk {
                    start_frame: start,
                    end_frame: end,
                    reference_frame: boundary,
                    direction: if forward { "forward" } else { "backward" }.into(),
                    point_indices: active.iter().map(|(i, _)| *i).collect(),
                });
                let next_boundary = if forward { end } else { start };
                let mut next = Vec::new();
                for ((index, _), prediction) in active.iter().zip(result.tracks) {
                    if prediction.observations.len() != end - start + 1 {
                        return Err("Incomplete chunk observations".into());
                    }
                    let target = tracks.get_mut(*index).ok_or("Invalid carried point identity")?;
                    for (offset, observation) in prediction.observations.iter().enumerate() {
                        let slot = target.observations.get_mut(start + offset).ok_or("Chunk outside full range")?;
                        if slot.is_none() {
                            *slot = Some(observation.clone());
                        }
                    }
                    if let Some(observation) = prediction
                        .observations
                        .get(next_boundary - start)
                        .filter(|o| o.visible && o.position.iter().all(|v| v.is_finite() && (0.0..1.0).contains(v)))
                    {
                        next.push((*index, observation.position));
                    }
                }
                done = done.saturating_add(end - start).min(total);
                progress(done, total);
                if forward && end == count - 1 || !forward && start == 0 {
                    break;
                }
                boundary = next_boundary;
                active = next;
            }
        }
        done = done.saturating_add(1).min(total);
        progress(done, total);
    }
    runtime["chunking"] =
        json!({"windowFrames":window,"overlapFrames":1,"continuation":"visible in-bounds points only; lost continuations remain null","driftRisk":true});
    Ok(Result { analyzer: "BootsTAPIR".into(), frame_count: count, tracks, runtime, chunks })
}
pub fn track<F, P>(count: usize, queries: Vec<Query>, cancel: &AtomicBool, device: Device, render: F, progress: P) -> std::result::Result<Result, String>
where
    F: FnMut(usize) -> std::result::Result<Frame, String>,
    P: FnMut(usize, usize),
{
    track_with(
        count,
        queries,
        cancel,
        point_tracking::MAX_FRAMES,
        render,
        |frames, w, h, queries| point_tracking::infer_with_device(w, h, frames, queries, cancel, device),
        progress,
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::point_tracking::Track as Prediction;
    #[test]
    fn long_ranges_cover_every_frame_from_middle_reference_without_resetting_identity() {
        let q = Query { frame_index: 125, position: [0.4, 0.5] };
        let mut max_batch = 0;
        let result = track_with(
            301,
            vec![q.clone()],
            &AtomicBool::new(false),
            120,
            |frame| Ok((64, 64, vec![frame as u8; 64 * 64 * 4])),
            |frames, _, _, queries| {
                max_batch = max_batch.max(frames.len());
                let n = frames.len();
                Ok(Inference {
                    analyzer: "BootsTAPIR".into(),
                    frame_count: n,
                    runtime: json!({}),
                    tracks: queries
                        .into_iter()
                        .map(|query| Prediction {
                            observations: (0..n)
                                .map(|i| Observation {
                                    position: [query.position[0] + (i as f64 - query.frame_index as f64) * 0.0001, 0.5],
                                    visible: true,
                                    visibility_score: 0.9,
                                })
                                .collect(),
                            query,
                        })
                        .collect(),
                })
            },
            |_, _| {},
        )
        .unwrap();
        assert_eq!(max_batch, 120);
        assert_eq!(result.tracks[0].query, q);
        assert!(result.tracks[0].observations.iter().all(Option::is_some));
        assert!((result.tracks[0].observations[300].as_ref().unwrap().position[0] - 0.4175).abs() < 1e-10);
        assert!((result.tracks[0].observations[0].as_ref().unwrap().position[0] - 0.3875).abs() < 1e-10);
    }
    #[test]
    fn lost_boundary_points_are_not_reseeded_or_invented() {
        let mut calls = 0;
        let result = track_with(
            250,
            vec![Query { frame_index: 0, position: [0.5, 0.5] }],
            &AtomicBool::new(false),
            120,
            |_| Ok((64, 64, vec![0; 64 * 64 * 4])),
            |frames, _, _, queries| {
                calls += 1;
                let n = frames.len();
                Ok(Inference {
                    analyzer: "BootsTAPIR".into(),
                    frame_count: n,
                    runtime: json!({}),
                    tracks: queries
                        .into_iter()
                        .map(|query| Prediction { query, observations: vec![Observation { position: [0.5, 0.5], visible: false, visibility_score: 0.1 }; n] })
                        .collect(),
                })
            },
            |_, _| {},
        )
        .unwrap();
        assert_eq!(calls, 1);
        assert!(result.tracks[0].observations[119].is_some());
        assert!(result.tracks[0].observations[120..].iter().all(Option::is_none));
    }
    #[test]
    fn cancellation_and_invalid_chunk_sizes_fail_before_inference() {
        let q = vec![Query { frame_index: 0, position: [0.5, 0.5] }];
        assert!(
            track_with(
                250,
                q.clone(),
                &AtomicBool::new(true),
                120,
                |_| Err("should not render".into()),
                |_, _, _, _| Err("should not infer".into()),
                |_, _| {}
            )
            .is_err()
        );
        assert!(
            track_with(250, q, &AtomicBool::new(false), 1, |_| Err("should not render".into()), |_, _, _, _| Err("should not infer".into()), |_, _| {})
                .is_err()
        );
    }
}

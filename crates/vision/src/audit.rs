//! Completion records distinguish a successful rerun from identical predictions.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletedRun {
    pub revision: u64,
    pub completed_at_unix_ms: u64,
    pub scope: String,
    /// Same 1-based frame number shown by the results viewer; None means the full range.
    pub frame_number: Option<usize>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AnalysisAudit {
    pub revision: u64,
    pub frame_revisions: BTreeMap<usize, u64>,
    pub completed_runs: Vec<CompletedRun>,
}
impl AnalysisAudit {
    /// Call only after a successful complete result has been accepted. Failure is atomic.
    pub fn complete(&mut self, pair_index: Option<usize>, pair_count: usize, completed_at_unix_ms: u64) -> Result<(), String> {
        if pair_count == 0 || pair_count > 299 || pair_index.is_some_and(|i| i >= pair_count) {
            return Err("Cannot record reanalysis outside the analyzed frame range".into());
        }
        let revision = self.revision.checked_add(1).ok_or("Analysis revision overflow")?;
        let mut frame_revisions = self.frame_revisions.clone();
        let frames: Vec<usize> = if let Some(index) = pair_index { vec![index + 2] } else { (2..=pair_count + 1).collect() };
        for frame in frames {
            let value = frame_revisions.get(&frame).copied().unwrap_or(0).checked_add(1).ok_or("Frame revision overflow")?;
            frame_revisions.insert(frame, value);
        }
        self.revision = revision;
        self.frame_revisions = frame_revisions;
        if self.completed_runs.len() >= 64 {
            self.completed_runs.drain(..self.completed_runs.len() - 63);
        }
        self.completed_runs.push(CompletedRun {
            revision,
            completed_at_unix_ms,
            scope: if pair_index.is_some() { "frame" } else { "all" }.into(),
            frame_number: pair_index.map(|i| i + 2),
        });
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identical_predictions_can_have_verified_frame_rerun_records() {
        let mut audit = AnalysisAudit::default();
        audit.complete(None, 118, 1000).unwrap();
        audit.complete(Some(114), 118, 2000).unwrap();
        assert_eq!(audit.revision, 2);
        assert_eq!(audit.frame_revisions.get(&116), Some(&2));
        assert_eq!(audit.frame_revisions.get(&115), Some(&1));
        assert_eq!(audit.frame_revisions.get(&117), Some(&1));
        let run = audit.completed_runs.last().unwrap();
        assert_eq!(run.frame_number, Some(116));
        assert_eq!(run.completed_at_unix_ms, 2000);
        assert_eq!(run.scope, "frame");
        assert_eq!(serde_json::from_value::<AnalysisAudit>(serde_json::to_value(&audit).unwrap()).unwrap(), audit);
    }
    #[test]
    fn invalid_and_overflowed_records_do_not_change_audit() {
        let mut audit = AnalysisAudit::default();
        let before = audit.clone();
        assert!(audit.complete(Some(usize::MAX), 118, 1).is_err());
        assert_eq!(audit, before);
        audit.revision = u64::MAX;
        let before = audit.clone();
        assert!(audit.complete(None, 118, 1).is_err());
        assert_eq!(audit, before);
    }
}

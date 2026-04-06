//! Regression hunt for noisy engine-strength changes.
//!
//! This is deliberately not a pure binary search. The runner samples a fixed
//! known-good baseline across the range, shrinks to the first bad window it can
//! justify, then linearly scans the final window and confirms the culprit.

use anyhow::Result;
use chrono::Utc;
use std::collections::{BTreeMap, HashMap, HashSet};
use tracing::info;
use uuid::Uuid;

use crate::scheduler::priority;
use crate::types::*;

const SAMPLE_WINDOW_MAX: usize = 6;
const SAMPLE_TARGETS: [usize; 3] = [1, 2, 3];

pub struct BisectRunner;

impl BisectRunner {
    pub fn new(_storage: crate::storage::Storage) -> Self {
        Self
    }

    /// Start a new regression hunt session.
    pub fn start_bisect(
        &self,
        engine_id: &str,
        good_revision_id: &str,
        bad_revision_id: &str,
        commit_range: Vec<String>,
    ) -> Result<BisectSession> {
        if commit_range.len() < 2 {
            anyhow::bail!("Need at least 2 commits in range to bisect");
        }

        let phase = if commit_range.len() <= SAMPLE_WINDOW_MAX {
            HuntPhase::Scanning
        } else {
            HuntPhase::Sampling
        };

        let mut session = BisectSession {
            id: Uuid::new_v4().to_string(),
            engine_id: engine_id.to_string(),
            good_revision_id: good_revision_id.to_string(),
            bad_revision_id: bad_revision_id.to_string(),
            commit_range,
            current_index: None,
            current_job_id: None,
            phase,
            pending_indices: Vec::new(),
            probe_history: Vec::new(),
            candidate_revision_id: None,
            candidate_index: None,
            status: BisectStatus::Running,
            culprit_revision_id: None,
        };
        self.replan(&mut session);

        info!(
            "Starting regression hunt: baseline={}, bad={}, range={}",
            &good_revision_id[..8.min(good_revision_id.len())],
            &bad_revision_id[..8.min(bad_revision_id.len())],
            session.commit_range.len()
        );

        Ok(session)
    }

    /// Get the next probe to run, mutating the session state if needed.
    pub fn next_commit_to_test(&self, session: &mut BisectSession) -> Option<BisectStep> {
        if session.status != BisectStatus::Running || session.current_job_id.is_some() {
            return None;
        }

        loop {
            match session.phase {
                HuntPhase::Found => {
                    session.status = BisectStatus::Found;
                    return session
                        .culprit_revision_id
                        .clone()
                        .map(|culprit| BisectStep::Found { culprit });
                }
                HuntPhase::Failed => {
                    session.status = BisectStatus::Failed;
                    return Some(BisectStep::Failed {
                        reason: "Regression hunt became inconclusive".into(),
                    });
                }
                HuntPhase::Confirming => {
                    let index = session.candidate_index?;
                    session.current_index = Some(index);
                    return Some(BisectStep::Test {
                        commit_hash: session.commit_range[index].clone(),
                        index,
                        remaining_range: session.commit_range.len(),
                        phase: session.phase,
                    });
                }
                HuntPhase::Sampling | HuntPhase::Scanning => {
                    if let Some(index) = pop_front(&mut session.pending_indices) {
                        session.current_index = Some(index);
                        return Some(BisectStep::Test {
                            commit_hash: session.commit_range[index].clone(),
                            index,
                            remaining_range: session.commit_range.len(),
                            phase: session.phase,
                        });
                    }

                    if !self.advance_without_new_result(session) {
                        return None;
                    }
                }
            }
        }
    }

    /// Process a completed probe and advance the hunt.
    pub fn process_result(
        &self,
        session: &mut BisectSession,
        tested_revision_id: &str,
        job_id: &str,
        verdict: ProbeVerdict,
    ) -> BisectAction {
        let Some(index) = session.current_index.take() else {
            session.phase = HuntPhase::Failed;
            session.status = BisectStatus::Failed;
            return BisectAction::Failed {
                reason: "Bisect session lost its active probe".into(),
            };
        };

        session.current_job_id = None;
        session.probe_history.push(ProbeRecord {
            commit_hash: session.commit_range[index].clone(),
            revision_id: tested_revision_id.to_string(),
            index,
            verdict,
            job_id: job_id.to_string(),
        });

        match session.phase {
            HuntPhase::Sampling => {}
            HuntPhase::Scanning => {
                if verdict == ProbeVerdict::Bad {
                    session.phase = HuntPhase::Confirming;
                    session.candidate_index = Some(index);
                    session.candidate_revision_id = Some(tested_revision_id.to_string());
                }
            }
            HuntPhase::Confirming => match verdict {
                ProbeVerdict::Bad => {
                    session.phase = HuntPhase::Found;
                    session.status = BisectStatus::Found;
                    session.culprit_revision_id = Some(tested_revision_id.to_string());
                }
                ProbeVerdict::Good | ProbeVerdict::Uncertain => {
                    session.phase = HuntPhase::Scanning;
                    session.candidate_index = None;
                    session.candidate_revision_id = None;
                }
            },
            HuntPhase::Found => {}
            HuntPhase::Failed => {}
        }

        match self.next_commit_to_test(session) {
            Some(BisectStep::Test {
                commit_hash,
                index,
                remaining_range,
                phase,
            }) => BisectAction::TestNext {
                commit_hash,
                index,
                remaining: remaining_range,
                phase,
            },
            Some(BisectStep::Found { culprit }) => BisectAction::Found { culprit },
            Some(BisectStep::Failed { reason }) => BisectAction::Failed { reason },
            None => {
                if let Some(culprit) = session.culprit_revision_id.clone() {
                    BisectAction::Found { culprit }
                } else {
                    BisectAction::Failed {
                        reason: "Regression hunt did not produce another probe".into(),
                    }
                }
            }
        }
    }

    /// Create a high-priority test job for a regression-hunt probe.
    pub fn create_bisect_job(
        &self,
        engine_id: &str,
        test_revision_id: &str,
        baseline_revision_id: &str,
        tc: TimeControl,
    ) -> TestJob {
        TestJob {
            id: Uuid::new_v4().to_string(),
            engine_id: engine_id.to_string(),
            dev_revision_id: test_revision_id.to_string(),
            base_revision_id: baseline_revision_id.to_string(),
            branch_context: None,
            time_control: tc,
            opening_book: None,
            status: TestStatus::Queued,
            priority: priority::BISECT,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            result: None,
            job_type: JobType::Bisect,
        }
    }

    fn advance_without_new_result(&self, session: &mut BisectSession) -> bool {
        match session.phase {
            HuntPhase::Sampling => {
                let Some((good_idx, bad_idx)) = self.first_bad_window(session) else {
                    session.phase = HuntPhase::Failed;
                    session.status = BisectStatus::Failed;
                    return true;
                };
                self.rebase_window(session, good_idx, bad_idx);
                self.replan(session);
                true
            }
            HuntPhase::Scanning => match self.evaluate_scan_window(session) {
                ScanOutcome::Found { revision_id } => {
                    session.phase = HuntPhase::Found;
                    session.status = BisectStatus::Found;
                    session.culprit_revision_id = Some(revision_id);
                    true
                }
                ScanOutcome::NeedsMoreData => {
                    session.phase = HuntPhase::Failed;
                    session.status = BisectStatus::Failed;
                    true
                }
            },
            HuntPhase::Confirming | HuntPhase::Found | HuntPhase::Failed => false,
        }
    }

    fn replan(&self, session: &mut BisectSession) {
        session.current_index = None;
        session.current_job_id = None;

        if session.commit_range.len() <= 2 {
            if let Some(revision_id) =
                self.revision_id_for_hash(session, session.commit_range.last())
            {
                session.phase = HuntPhase::Found;
                session.status = BisectStatus::Found;
                session.culprit_revision_id = Some(revision_id);
            } else {
                session.phase = HuntPhase::Failed;
                session.status = BisectStatus::Failed;
            }
            session.pending_indices.clear();
            return;
        }

        match session.phase {
            HuntPhase::Sampling => {
                let planned = self.sample_indices(session);
                if planned.is_empty() {
                    session.phase = HuntPhase::Scanning;
                    self.replan(session);
                    return;
                }
                session.pending_indices = planned;
            }
            HuntPhase::Scanning => {
                session.pending_indices = self.scan_indices(session);
            }
            HuntPhase::Confirming => {
                session.pending_indices.clear();
            }
            HuntPhase::Found | HuntPhase::Failed => {
                session.pending_indices.clear();
            }
        }
    }

    fn sample_indices(&self, session: &BisectSession) -> Vec<usize> {
        let len = session.commit_range.len();
        if len <= SAMPLE_WINDOW_MAX {
            return Vec::new();
        }

        let tested = session
            .probe_history
            .iter()
            .map(|record| record.index)
            .collect::<HashSet<_>>();

        let mut indices = SAMPLE_TARGETS
            .iter()
            .map(|slot| ((len - 1) * slot) / 4)
            .map(|idx| idx.clamp(1, len - 2))
            .filter(|idx| !tested.contains(idx))
            .collect::<Vec<_>>();
        indices.sort_unstable();
        indices.dedup();
        indices
    }

    fn scan_indices(&self, session: &BisectSession) -> Vec<usize> {
        let verdicts = self.verdicts_by_index(session);
        let mut indices = Vec::new();
        for index in 1..session.commit_range.len().saturating_sub(1) {
            if !verdicts.contains_key(&index) {
                indices.push(index);
            }
        }
        indices
    }

    fn first_bad_window(&self, session: &BisectSession) -> Option<(usize, usize)> {
        let mut known = self
            .verdicts_by_index(session)
            .into_iter()
            .filter(|(_, verdict)| *verdict != ProbeVerdict::Uncertain)
            .collect::<Vec<_>>();
        known.sort_by_key(|(index, _)| *index);

        for pair in known.windows(2) {
            let (left_idx, left_verdict) = pair[0];
            let (right_idx, right_verdict) = pair[1];
            if left_verdict == ProbeVerdict::Good && right_verdict == ProbeVerdict::Bad {
                return Some((left_idx, right_idx));
            }
        }

        None
    }

    fn rebase_window(&self, session: &mut BisectSession, start: usize, end: usize) {
        let new_range = session.commit_range[start..=end].to_vec();
        let positions = new_range
            .iter()
            .enumerate()
            .map(|(index, hash)| (hash.clone(), index))
            .collect::<HashMap<_, _>>();

        session.probe_history = session
            .probe_history
            .iter()
            .filter_map(|record| {
                positions.get(&record.commit_hash).map(|index| {
                    let mut record = record.clone();
                    record.index = *index;
                    record
                })
            })
            .collect();

        session.commit_range = new_range;
        session.pending_indices.clear();
        session.current_index = None;
        session.current_job_id = None;
        session.candidate_index = None;
        session.candidate_revision_id = None;
        session.phase = if session.commit_range.len() <= SAMPLE_WINDOW_MAX {
            HuntPhase::Scanning
        } else {
            HuntPhase::Sampling
        };

        info!(
            "Regression hunt narrowed to {} commits",
            session.commit_range.len()
        );
    }

    fn evaluate_scan_window(&self, session: &BisectSession) -> ScanOutcome {
        let verdicts = self.verdicts_by_index(session);
        for index in 1..session.commit_range.len() {
            match verdicts.get(&index).copied() {
                Some(ProbeVerdict::Good) => continue,
                Some(ProbeVerdict::Bad) => {
                    let previous_all_good =
                        (0..index).all(|prior| verdicts.get(&prior) == Some(&ProbeVerdict::Good));
                    if !previous_all_good {
                        return ScanOutcome::NeedsMoreData;
                    }

                    if let Some(revision_id) =
                        self.revision_id_for_hash(session, session.commit_range.get(index))
                    {
                        return ScanOutcome::Found { revision_id };
                    }
                    return ScanOutcome::NeedsMoreData;
                }
                Some(ProbeVerdict::Uncertain) | None => return ScanOutcome::NeedsMoreData,
            }
        }

        ScanOutcome::NeedsMoreData
    }

    fn verdicts_by_index(&self, session: &BisectSession) -> BTreeMap<usize, ProbeVerdict> {
        let mut verdicts = BTreeMap::from([
            (0, ProbeVerdict::Good),
            (session.commit_range.len() - 1, ProbeVerdict::Bad),
        ]);
        for record in &session.probe_history {
            verdicts.insert(record.index, record.verdict);
        }
        verdicts
    }

    fn revision_id_for_hash(
        &self,
        session: &BisectSession,
        commit_hash: Option<&String>,
    ) -> Option<String> {
        let commit_hash = commit_hash?;
        session
            .probe_history
            .iter()
            .find(|record| &record.commit_hash == commit_hash)
            .map(|record| record.revision_id.clone())
            .or_else(|| {
                if session.commit_range.first() == Some(commit_hash) {
                    Some(session.good_revision_id.clone())
                } else {
                    None
                }
            })
            .or_else(|| {
                if session.commit_range.last() == Some(commit_hash) {
                    Some(session.bad_revision_id.clone())
                } else {
                    None
                }
            })
    }
}

fn pop_front<T>(values: &mut Vec<T>) -> Option<T> {
    if values.is_empty() {
        None
    } else {
        Some(values.remove(0))
    }
}

enum ScanOutcome {
    Found { revision_id: String },
    NeedsMoreData,
}

/// Result of asking the regression hunt what to do next.
pub enum BisectStep {
    /// Test this commit against the fixed baseline.
    Test {
        commit_hash: String,
        index: usize,
        remaining_range: usize,
        phase: HuntPhase,
    },
    /// We found the culprit.
    Found { culprit: String },
    /// The hunt could not localize a culprit safely.
    Failed { reason: String },
}

/// Action to take after processing a probe result.
pub enum BisectAction {
    /// Test the next commit.
    TestNext {
        commit_hash: String,
        index: usize,
        remaining: usize,
        phase: HuntPhase,
    },
    /// Regression hunt is complete.
    Found { culprit: String },
    /// Regression hunt became inconclusive.
    Failed { reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Storage;

    #[test]
    fn starts_large_ranges_in_sampling_mode() -> Result<()> {
        let storage = Storage::in_memory()?;
        let runner = BisectRunner::new(storage);
        let session = runner.start_bisect(
            "engine",
            "good-rev",
            "bad-rev",
            vec![
                "c0".into(),
                "c1".into(),
                "c2".into(),
                "c3".into(),
                "c4".into(),
                "c5".into(),
                "c6".into(),
            ],
        )?;

        assert_eq!(session.phase, HuntPhase::Sampling);
        assert_eq!(session.pending_indices, vec![1, 3, 4]);
        Ok(())
    }

    #[test]
    fn sampling_shrinks_to_first_bad_window() -> Result<()> {
        let storage = Storage::in_memory()?;
        let runner = BisectRunner::new(storage);
        let mut session = runner.start_bisect(
            "engine",
            "good-rev",
            "bad-rev",
            vec![
                "c0".into(),
                "c1".into(),
                "c2".into(),
                "c3".into(),
                "c4".into(),
                "c5".into(),
                "c6".into(),
            ],
        )?;

        assert!(matches!(
            runner.next_commit_to_test(&mut session),
            Some(BisectStep::Test { index: 1, .. })
        ));
        assert!(matches!(
            runner.process_result(&mut session, "rev-1", "job-1", ProbeVerdict::Good),
            BisectAction::TestNext { index: 3, .. }
        ));
        assert!(matches!(
            runner.process_result(&mut session, "rev-3", "job-2", ProbeVerdict::Bad),
            BisectAction::TestNext { index: 4, .. }
        ));
        let action = runner.process_result(&mut session, "rev-4", "job-3", ProbeVerdict::Bad);

        assert!(matches!(
            action,
            BisectAction::TestNext {
                phase: HuntPhase::Scanning,
                ..
            }
        ));
        assert_eq!(session.commit_range, vec!["c1", "c2", "c3"]);
        Ok(())
    }

    #[test]
    fn confirming_requires_two_bad_results() -> Result<()> {
        let storage = Storage::in_memory()?;
        let runner = BisectRunner::new(storage);
        let mut session = runner.start_bisect(
            "engine",
            "good-rev",
            "bad-rev",
            vec!["c0".into(), "c1".into(), "c2".into(), "c3".into()],
        )?;

        assert!(matches!(
            runner.next_commit_to_test(&mut session),
            Some(BisectStep::Test { index: 1, .. })
        ));
        assert!(matches!(
            runner.process_result(&mut session, "rev-1", "job-1", ProbeVerdict::Good),
            BisectAction::TestNext { index: 2, .. }
        ));
        let action = runner.process_result(&mut session, "rev-2", "job-2", ProbeVerdict::Bad);

        assert!(matches!(
            action,
            BisectAction::TestNext {
                index: 2,
                phase: HuntPhase::Confirming,
                ..
            }
        ));
        let action = runner.process_result(&mut session, "rev-2", "job-3", ProbeVerdict::Bad);
        assert!(matches!(
            action,
            BisectAction::Found { ref culprit } if culprit == "rev-2"
        ));
        Ok(())
    }
}

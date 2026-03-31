//! Bisect mode: binary search to find the exact commit that caused a regression.
//!
//! Given a "good" commit (strong) and a "bad" commit (weak), bisect will
//! test the midpoint, determine if it's good or bad, and narrow the range
//! until the guilty commit is found.

use anyhow::{Context, Result};
use chrono::Utc;
use uuid::Uuid;
use tracing::info;

use crate::scheduler::priority;
use crate::storage::Storage;
use crate::types::*;

pub struct BisectRunner {
    storage: Storage,
}

impl BisectRunner {
    pub fn new(storage: Storage) -> Self {
        Self { storage }
    }

    /// Start a new bisect session
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

        let session = BisectSession {
            id: Uuid::new_v4().to_string(),
            engine_id: engine_id.to_string(),
            good_revision_id: good_revision_id.to_string(),
            bad_revision_id: bad_revision_id.to_string(),
            commit_range,
            current_index: None,
            status: BisectStatus::Running,
            culprit_revision_id: None,
        };

        info!(
            "Starting bisect: good={}, bad={}, range={}",
            &good_revision_id[..8.min(good_revision_id.len())],
            &bad_revision_id[..8.min(bad_revision_id.len())],
            session.commit_range.len()
        );

        Ok(session)
    }

    /// Get the next commit to test in the bisect process
    pub fn next_commit_to_test(&self, session: &BisectSession) -> Option<BisectStep> {
        if session.status != BisectStatus::Running {
            return None;
        }

        let range = &session.commit_range;
        if range.len() <= 2 {
            // We've narrowed it down - the culprit is the second commit
            return Some(BisectStep::Found {
                culprit: range.last()?.clone(),
            });
        }

        let mid = range.len() / 2;
        Some(BisectStep::Test {
            commit_hash: range[mid].clone(),
            index: mid,
            remaining_range: range.len(),
        })
    }

    /// Process the result of a bisect test and narrow the range
    pub fn process_result(
        &self,
        session: &mut BisectSession,
        tested_index: usize,
        is_good: bool,
    ) -> BisectAction {
        if is_good {
            // The tested commit is good, so the regression is after it
            // New range: [tested_index..end]
            session.commit_range = session.commit_range[tested_index..].to_vec();
            info!(
                "Bisect: commit is GOOD, narrowing to {} commits",
                session.commit_range.len()
            );
        } else {
            // The tested commit is bad, so the regression is before it
            // New range: [start..=tested_index]
            session.commit_range = session.commit_range[..=tested_index].to_vec();
            info!(
                "Bisect: commit is BAD, narrowing to {} commits",
                session.commit_range.len()
            );
        }

        // Check if we've found it
        if session.commit_range.len() <= 2 {
            let culprit = session.commit_range.last().unwrap().clone();
            session.status = BisectStatus::Found;
            session.culprit_revision_id = Some(culprit.clone());
            info!("Bisect complete! Culprit: {}", &culprit[..8.min(culprit.len())]);
            BisectAction::Found { culprit }
        } else {
            let mid = session.commit_range.len() / 2;
            BisectAction::TestNext {
                commit_hash: session.commit_range[mid].clone(),
                index: mid,
                remaining: session.commit_range.len(),
            }
        }
    }

    /// Create a high-priority test job for a bisect step
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
}

/// Result of asking the bisect runner what to do next
pub enum BisectStep {
    /// Test this commit
    Test {
        commit_hash: String,
        index: usize,
        remaining_range: usize,
    },
    /// We found the culprit
    Found { culprit: String },
}

/// Action to take after processing a bisect result
pub enum BisectAction {
    /// Test the next commit
    TestNext {
        commit_hash: String,
        index: usize,
        remaining: usize,
    },
    /// Bisect is complete
    Found { culprit: String },
}

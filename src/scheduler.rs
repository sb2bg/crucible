//! Smart scheduler for test job prioritization.
//!
//! Priority logic:
//! 1. Bisect jobs (highest - user is actively waiting)
//! 2. Manual/user-requested tests
//! 3. Branch HEAD commits (most recent work)
//! 4. Tagged releases (important milestones)
//! 5. Sequential fill-in (working backwards from HEAD)
//!
//! The scheduler also handles re-prioritizing when new commits are pushed.

use anyhow::Result;
use chrono::Utc;
use uuid::Uuid;

use crate::config::Config;
use crate::storage::Storage;
use crate::types::*;

/// Priority levels (higher = more urgent)
pub mod priority {
    pub const BISECT: i32 = 1000;
    pub const MANUAL: i32 = 900;
    pub const BRANCH_HEAD: i32 = 800;
    pub const TAGGED_RELEASE: i32 = 700;
    pub const RECENT_COMMIT: i32 = 500;
    pub const BACKFILL: i32 = 100;
}

pub struct Scheduler {
    storage: Storage,
    config: Config,
}

impl Scheduler {
    pub fn new(storage: Storage, config: Config) -> Self {
        Self { storage, config }
    }

    /// Scan for new commits and create test jobs for them
    pub fn schedule_engine(&self, engine_id: &str) -> Result<Vec<TestJob>> {
        let revisions = self.storage.get_revisions_for_engine(engine_id)?;
        if revisions.len() < 2 {
            return Ok(Vec::new());
        }

        let mut new_jobs = Vec::new();

        // Find revisions that don't have a test job yet
        // For each untested revision, create a job testing it against its predecessor
        for i in 1..revisions.len() {
            let dev = &revisions[i];
            let base = &revisions[i - 1];

            // Skip if dev isn't built yet
            if dev.build_status != BuildStatus::Success {
                continue;
            }
            if base.build_status != BuildStatus::Success {
                continue;
            }

            let priority = self.compute_priority(dev, &revisions);
            let tc = TimeControl {
                base_time_ms: self.config.testing.time_control.base_ms,
                increment_ms: self.config.testing.time_control.increment_ms,
                nodes: self.config.testing.time_control.nodes,
            };

            let job = TestJob {
                id: Uuid::new_v4().to_string(),
                engine_id: engine_id.to_string(),
                dev_revision_id: dev.id.clone(),
                base_revision_id: base.id.clone(),
                time_control: tc,
                opening_book: self.config.testing.opening_book.clone(),
                status: TestStatus::Queued,
                priority,
                created_at: Utc::now(),
                started_at: None,
                completed_at: None,
                result: None,
                job_type: JobType::Sequential,
            };
            new_jobs.push(job);
        }

        Ok(new_jobs)
    }

    /// Compute priority for a revision based on its properties
    fn compute_priority(&self, rev: &EngineRevision, all_revisions: &[EngineRevision]) -> i32 {
        let mut prio = priority::BACKFILL;

        // Is it a tagged release?
        if rev.is_release || rev.tag.is_some() {
            prio = prio.max(priority::TAGGED_RELEASE);
        }

        // Is it the HEAD of its branch?
        let is_head = all_revisions
            .iter()
            .filter(|r| r.branch == rev.branch)
            .last()
            .map_or(false, |last| last.id == rev.id);

        if is_head {
            prio = prio.max(priority::BRANCH_HEAD);
        }

        // Recency bonus: more recent commits get higher priority within their tier
        let age_days = (Utc::now() - rev.commit_date).num_days();
        let recency_bonus = (30 - age_days.min(30)) as i32; // 0-30 bonus
        if prio < priority::TAGGED_RELEASE {
            prio += recency_bonus;
        }

        prio
    }

    /// Create a manual test job with high priority
    pub fn schedule_manual_test(
        &self,
        engine_id: &str,
        dev_revision_id: &str,
        base_revision_id: &str,
    ) -> Result<TestJob> {
        let tc = TimeControl {
            base_time_ms: self.config.testing.time_control.base_ms,
            increment_ms: self.config.testing.time_control.increment_ms,
            nodes: self.config.testing.time_control.nodes,
        };

        let job = TestJob {
            id: Uuid::new_v4().to_string(),
            engine_id: engine_id.to_string(),
            dev_revision_id: dev_revision_id.to_string(),
            base_revision_id: base_revision_id.to_string(),
            time_control: tc,
            opening_book: self.config.testing.opening_book.clone(),
            status: TestStatus::Queued,
            priority: priority::MANUAL,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            result: None,
            job_type: JobType::Manual,
        };
        self.storage.insert_test_job(&job)?;
        Ok(job)
    }

    /// Boost priority of all pending jobs for a specific branch
    /// (called when new commits are pushed to that branch)
    pub fn reprioritize_branch(&self, _engine_id: &str, _branch: &str) -> Result<()> {
        // TODO: Bump priority of HEAD commit jobs, demote older ones
        Ok(())
    }
}

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
use crate::git::branch_pattern_matches;
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
        let engine = self
            .storage
            .get_engine_by_id(engine_id)?
            .ok_or_else(|| anyhow::anyhow!("engine '{}' not found", engine_id))?;
        let revisions = self.storage.get_branch_revisions_for_engine(engine_id)?;
        if revisions.len() < 2 {
            return Ok(Vec::new());
        }

        let mut new_jobs = Vec::new();
        let mut revisions_by_branch =
            std::collections::BTreeMap::<String, Vec<EngineRevision>>::new();
        for revision in revisions {
            revisions_by_branch
                .entry(revision.branch.clone())
                .or_default()
                .push(revision);
        }

        let canonical_main_head = revisions_by_branch
            .get("main")
            .and_then(|revisions| latest_successful_revision(revisions).cloned());

        for (branch_name, branch_revisions) in revisions_by_branch.iter_mut() {
            branch_revisions.sort_by_key(|rev| rev.commit_date);
            if is_experimental_branch(&engine, branch_name) {
                let Some(dev) = latest_successful_revision(branch_revisions) else {
                    continue;
                };
                let Some(base) = canonical_main_head.as_ref() else {
                    continue;
                };
                if dev.id == base.id {
                    continue;
                }
                if should_schedule_pair(&self.storage, engine_id, dev, base, JobType::Sequential)? {
                    new_jobs.push(self.make_job(
                        dev,
                        base,
                        self.compute_priority(dev, branch_revisions),
                    ));
                }
                continue;
            }

            for pair in branch_revisions.windows(2) {
                let base = &pair[0];
                let dev = &pair[1];

                if should_schedule_pair(&self.storage, engine_id, dev, base, JobType::Sequential)? {
                    new_jobs.push(self.make_job(
                        dev,
                        base,
                        self.compute_priority(dev, branch_revisions),
                    ));
                }
            }
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
        let age_days = (Utc::now() - rev.commit_date).num_days().clamp(0, 30);
        let recency_bonus = 30 - age_days as i32; // 0-30 bonus
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
            branch_context: None,
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

    fn make_job(&self, dev: &EngineRevision, base: &EngineRevision, priority: i32) -> TestJob {
        let tc = TimeControl {
            base_time_ms: self.config.testing.time_control.base_ms,
            increment_ms: self.config.testing.time_control.increment_ms,
            nodes: self.config.testing.time_control.nodes,
        };

        TestJob {
            id: Uuid::new_v4().to_string(),
            engine_id: dev.engine_id.clone(),
            dev_revision_id: dev.id.clone(),
            base_revision_id: base.id.clone(),
            branch_context: Some(dev.branch.clone()),
            time_control: tc,
            opening_book: self.config.testing.opening_book.clone(),
            status: TestStatus::Queued,
            priority,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            result: None,
            job_type: JobType::Sequential,
        }
    }
}

fn is_experimental_branch(engine: &Engine, branch: &str) -> bool {
    engine
        .experimental_branches
        .iter()
        .any(|pattern| branch_pattern_matches(pattern, branch))
}

fn latest_successful_revision(revisions: &[EngineRevision]) -> Option<&EngineRevision> {
    revisions
        .iter()
        .rev()
        .find(|revision| revision.build_status == BuildStatus::Success)
}

fn should_schedule_pair(
    storage: &Storage,
    engine_id: &str,
    dev: &EngineRevision,
    base: &EngineRevision,
    job_type: JobType,
) -> Result<bool> {
    if dev.build_status != BuildStatus::Success || base.build_status != BuildStatus::Success {
        return Ok(false);
    }

    if dev.binary_fingerprint.is_some() && dev.binary_fingerprint == base.binary_fingerprint {
        return Ok(false);
    }

    if storage.has_test_job(engine_id, &dev.id, &base.id, job_type)? {
        return Ok(false);
    }

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn test_engine() -> Engine {
        Engine {
            id: "engine-1".into(),
            name: "engine".into(),
            repo_url: "https://example.invalid/repo.git".into(),
            local_path: std::path::PathBuf::from("/tmp/engine"),
            branches: vec!["main".into(), "dev".into()],
            experimental_branches: vec!["exp/*".into()],
            build_cmd: "make".into(),
            binary_path: "engine".into(),
            start_from: None,
        }
    }

    fn test_revision(
        engine_id: &str,
        branch: &str,
        suffix: &str,
        offset_days: i64,
    ) -> EngineRevision {
        EngineRevision {
            id: format!("{}-{}", branch, suffix),
            engine_id: engine_id.into(),
            commit_hash: format!("{}{}", branch, suffix),
            commit_message: format!("{} {}", branch, suffix),
            commit_date: Utc::now() + Duration::days(offset_days),
            branch: branch.into(),
            tag: None,
            is_release: false,
            binary_path: Some(std::path::PathBuf::from(format!("/tmp/{}", suffix))),
            binary_fingerprint: None,
            build_status: BuildStatus::Success,
        }
    }

    #[test]
    fn schedules_adjacent_revisions_within_each_branch() -> Result<()> {
        let storage = Storage::in_memory()?;
        let engine = test_engine();
        storage.insert_engine(&engine)?;

        let revisions = vec![
            test_revision(&engine.id, "main", "a1", 0),
            test_revision(&engine.id, "main", "a2", 1),
            test_revision(&engine.id, "dev", "b1", 2),
            test_revision(&engine.id, "dev", "b2", 3),
        ];
        for revision in &revisions {
            storage.insert_revision(revision)?;
        }

        let scheduler = Scheduler::new(storage, Config::default());
        let jobs = scheduler.schedule_engine(&engine.id)?;

        assert_eq!(jobs.len(), 2);
        assert!(jobs
            .iter()
            .any(|job| { job.dev_revision_id == "main-a2" && job.base_revision_id == "main-a1" }));
        assert!(jobs
            .iter()
            .any(|job| { job.dev_revision_id == "dev-b2" && job.base_revision_id == "dev-b1" }));
        assert!(jobs.iter().all(|job| job.branch_context.is_some()));
        Ok(())
    }

    #[test]
    fn does_not_schedule_duplicate_sequential_jobs() -> Result<()> {
        let storage = Storage::in_memory()?;
        let engine = test_engine();
        storage.insert_engine(&engine)?;

        let base = test_revision(&engine.id, "main", "a1", 0);
        let dev = test_revision(&engine.id, "main", "a2", 1);
        storage.insert_revision(&base)?;
        storage.insert_revision(&dev)?;

        let existing_job = TestJob {
            id: "job-1".into(),
            engine_id: engine.id.clone(),
            dev_revision_id: dev.id.clone(),
            base_revision_id: base.id.clone(),
            branch_context: Some("main".into()),
            time_control: TimeControl::stc(),
            opening_book: None,
            status: TestStatus::Completed,
            priority: priority::BACKFILL,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            result: None,
            job_type: JobType::Sequential,
        };
        storage.insert_test_job(&existing_job)?;

        let scheduler = Scheduler::new(storage, Config::default());
        let jobs = scheduler.schedule_engine(&engine.id)?;

        assert!(jobs.is_empty());
        Ok(())
    }

    #[test]
    fn preserves_shared_commit_membership_across_branches() -> Result<()> {
        let storage = Storage::in_memory()?;
        let engine = test_engine();
        storage.insert_engine(&engine)?;

        let shared_main = test_revision(&engine.id, "main", "shared", 0);
        let mut shared_dev = shared_main.clone();
        shared_dev.branch = "dev".into();

        let main_head = test_revision(&engine.id, "main", "main2", 1);
        let dev_head = test_revision(&engine.id, "dev", "dev2", 2);

        storage.insert_revision(&shared_main)?;
        storage.insert_revision(&shared_dev)?;
        storage.insert_revision(&main_head)?;
        storage.insert_revision(&dev_head)?;

        let scheduler = Scheduler::new(storage, Config::default());
        let jobs = scheduler.schedule_engine(&engine.id)?;

        assert!(jobs.iter().any(
            |job| job.dev_revision_id == main_head.id && job.base_revision_id == shared_main.id
        ));
        assert!(jobs.iter().any(
            |job| job.dev_revision_id == dev_head.id && job.base_revision_id == shared_main.id
        ));
        Ok(())
    }

    #[test]
    fn skips_sequential_jobs_for_identical_binaries() -> Result<()> {
        let storage = Storage::in_memory()?;
        let engine = test_engine();
        storage.insert_engine(&engine)?;

        let mut base = test_revision(&engine.id, "main", "a1", 0);
        let mut dev = test_revision(&engine.id, "main", "a2", 1);
        base.binary_fingerprint = Some("same-binary".into());
        dev.binary_fingerprint = Some("same-binary".into());

        storage.insert_revision(&base)?;
        storage.insert_revision(&dev)?;

        let scheduler = Scheduler::new(storage, Config::default());
        let jobs = scheduler.schedule_engine(&engine.id)?;

        assert!(jobs.is_empty());
        Ok(())
    }

    #[test]
    fn experimental_head_runs_against_main_head() -> Result<()> {
        let storage = Storage::in_memory()?;
        let engine = Engine {
            branches: vec!["main".into()],
            experimental_branches: vec!["exp/*".into()],
            ..test_engine()
        };
        storage.insert_engine(&engine)?;

        let shared = test_revision(&engine.id, "main", "base", 0);
        let mut shared_exp = shared.clone();
        shared_exp.branch = "exp/nullmove".into();

        let mut main_head = test_revision(&engine.id, "main", "main2", 1);
        let mut exp_head = test_revision(&engine.id, "exp/nullmove", "exp2", 2);
        main_head.binary_fingerprint = Some("main-head".into());
        exp_head.binary_fingerprint = Some("exp-head".into());

        storage.insert_revision(&shared)?;
        storage.insert_revision(&shared_exp)?;
        storage.insert_revision(&main_head)?;
        storage.insert_revision(&exp_head)?;

        let scheduler = Scheduler::new(storage, Config::default());
        let jobs = scheduler.schedule_engine(&engine.id)?;

        assert!(jobs.iter().any(|job| {
            job.dev_revision_id == exp_head.id
                && job.base_revision_id == main_head.id
                && job.branch_context.as_deref() == Some("exp/nullmove")
        }));
        assert!(!jobs.iter().any(|job| {
            job.dev_revision_id == exp_head.id && job.base_revision_id == shared.id
        }));
        Ok(())
    }
}

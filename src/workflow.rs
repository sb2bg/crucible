use anyhow::{Context, Result};
use tracing::{debug, warn};

use crate::bisect::BisectRunner;
use crate::config::Config;
use crate::git::{short_hash, GitManager};
use crate::storage::Storage;
use crate::types::{BisectSession, BuildStatus, Engine, TimeControl};

pub fn configured_time_control(config: &Config) -> TimeControl {
    TimeControl {
        base_time_ms: config.testing.time_control.base_ms,
        increment_ms: config.testing.time_control.increment_ms,
        nodes: config.testing.time_control.nodes,
    }
}

pub fn sync_engine_revisions(
    storage: &Storage,
    engine: &Engine,
    git_mgr: &GitManager,
    repo: &git2::Repository,
) -> Result<()> {
    let branches = git_mgr.resolve_branch_patterns(repo, &tracked_branches(engine))?;
    for branch in &branches {
        let revisions =
            git_mgr.list_commits(repo, branch, &engine.id, engine.start_from.as_deref())?;
        debug!(
            "Engine '{}' branch '{}': {} commits",
            engine.name,
            branch,
            revisions.len()
        );
        for revision in &revisions {
            storage.insert_revision(revision)?;
        }
    }

    let revisions = storage.get_revisions_for_engine(&engine.id)?;
    for revision in revisions
        .iter()
        .filter(|revision| revision.build_status == BuildStatus::Pending)
    {
        match git_mgr.build_revision(repo, &revision.commit_hash) {
            Ok(binary) => {
                let fingerprint = GitManager::fingerprint_binary(&binary)?;
                storage.update_build_status(
                    &revision.id,
                    BuildStatus::Success,
                    Some(&binary),
                    Some(&fingerprint),
                )?;
            }
            Err(err) => {
                warn!(
                    "Build failed for {}: {}",
                    short_hash(&revision.commit_hash),
                    err
                );
                storage.update_build_status(&revision.id, BuildStatus::Failed, None, None)?;
            }
        }
    }

    Ok(())
}

pub fn queue_bisect_probe(
    storage: &Storage,
    bisect_runner: &BisectRunner,
    session: &mut BisectSession,
    engine_id: &str,
    baseline_revision_id: &str,
    commit_hash: &str,
    config: &Config,
) -> Result<()> {
    let test_revision = storage
        .get_revision_by_hash_prefix(engine_id, commit_hash)?
        .with_context(|| format!("Could not resolve bisect probe '{}'", commit_hash))?;
    let job = bisect_runner.create_bisect_job(
        engine_id,
        &test_revision.id,
        baseline_revision_id,
        configured_time_control(config),
    );
    session.current_job_id = Some(job.id.clone());
    storage.insert_test_job(&job)?;
    Ok(())
}

fn tracked_branches(engine: &Engine) -> Vec<String> {
    let mut branches = engine.branches.clone();
    branches.extend(engine.experimental_branches.clone());
    branches
}

use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::storage::Storage;
use crate::training::{list_training_runs, TrainingRunSummary};
use crate::types::{BisectSession, EloDataPoint, Engine, EngineRevision, JobSummary, SystemStatus};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportBundle {
    pub schema_version: u32,
    pub generated_at: chrono::DateTime<Utc>,
    pub status: SystemStatus,
    pub engines: Vec<ExportEngine>,
    pub jobs: Vec<JobSummary>,
    pub bisect_sessions: Vec<BisectSession>,
    pub training_runs: Vec<TrainingRunSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportEngine {
    pub engine: Engine,
    pub revisions: Vec<EngineRevision>,
    pub timeline: Vec<EloDataPoint>,
}

pub fn build_export_bundle(storage: &Storage, config: &Config) -> Result<ExportBundle> {
    let mut status = storage.get_system_status()?;
    status.uptime_seconds = 0;

    let engines = storage
        .get_engines()?
        .into_iter()
        .map(|engine| build_export_engine(storage, engine))
        .collect::<Result<Vec<_>>>()?;

    Ok(ExportBundle {
        schema_version: 1,
        generated_at: Utc::now(),
        status,
        engines,
        jobs: storage.list_all_jobs()?,
        bisect_sessions: storage.list_all_bisect_sessions()?,
        training_runs: list_training_runs(&config.training.output_dir)?,
    })
}

fn build_export_engine(storage: &Storage, engine: Engine) -> Result<ExportEngine> {
    Ok(ExportEngine {
        timeline: storage.get_elo_timeline(&engine.id, None)?,
        revisions: storage.get_revisions_for_engine(&engine.id)?,
        engine,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::types::{
        BuildStatus, EngineRevision, GameResult, JobType, TestJob, TestStatus, TimeControl,
    };

    fn test_engine() -> Engine {
        Engine {
            id: "engine-1".into(),
            name: "engine".into(),
            repo_url: "https://example.invalid/repo.git".into(),
            local_path: std::path::PathBuf::from("/tmp/engine"),
            branches: vec!["main".into()],
            build_cmd: "make".into(),
            binary_path: "engine".into(),
            start_from: None,
        }
    }

    fn test_revision(engine_id: &str, id: &str, hash: &str) -> EngineRevision {
        EngineRevision {
            id: id.into(),
            engine_id: engine_id.into(),
            commit_hash: hash.into(),
            commit_message: format!("commit {}", hash),
            commit_date: Utc::now(),
            branch: "main".into(),
            tag: None,
            is_release: false,
            binary_path: Some(std::path::PathBuf::from(format!("/tmp/{}", id))),
            binary_fingerprint: None,
            build_status: BuildStatus::Success,
        }
    }

    #[test]
    fn builds_export_bundle() -> Result<()> {
        let storage = Storage::in_memory()?;
        let mut config = Config::default();
        config.training.output_dir =
            std::env::temp_dir().join(format!("crucible-export-test-{}", uuid::Uuid::new_v4()));

        let engine = test_engine();
        let base = test_revision(&engine.id, "rev-base", "aaaa");
        let dev = test_revision(&engine.id, "rev-dev", "bbbb");
        let job = TestJob {
            id: "job-1".into(),
            engine_id: engine.id.clone(),
            dev_revision_id: dev.id.clone(),
            base_revision_id: base.id.clone(),
            time_control: TimeControl::stc(),
            opening_book: None,
            status: TestStatus::Completed,
            priority: 0,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            result: None,
            job_type: JobType::Sequential,
        };

        storage.insert_engine(&engine)?;
        storage.insert_revision(&base)?;
        storage.insert_revision(&dev)?;
        storage.insert_test_job(&job)?;
        storage.update_job_result(
            &job.id,
            1,
            0,
            0,
            10.0,
            5.0,
            0.9,
            crate::types::SprtResult::H1Accepted,
        )?;

        let bundle = build_export_bundle(&storage, &config)?;
        assert_eq!(bundle.schema_version, 1);
        assert_eq!(bundle.engines.len(), 1);
        assert_eq!(bundle.jobs.len(), 1);

        let _ = std::fs::remove_dir_all(&config.training.output_dir);
        let _ = GameResult::Draw;
        Ok(())
    }
}

//! SQLite storage layer.
//!
//! All test results, engine revisions, and job state are persisted here.
//! The database is the source of truth for the Elo timeline.

use anyhow::Result;
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::types::*;

#[derive(Clone)]
pub struct Storage {
    conn: Arc<Mutex<Connection>>,
}

impl Storage {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        let storage = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        storage.migrate()?;
        Ok(storage)
    }

    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let storage = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        storage.migrate()?;
        Ok(storage)
    }

    fn migrate(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS engines (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                repo_url TEXT NOT NULL,
                local_path TEXT NOT NULL,
                branches TEXT NOT NULL,  -- JSON array
                build_cmd TEXT NOT NULL,
                binary_path TEXT NOT NULL,
                start_from TEXT
            );

            CREATE TABLE IF NOT EXISTS revisions (
                id TEXT PRIMARY KEY,
                engine_id TEXT NOT NULL REFERENCES engines(id),
                commit_hash TEXT NOT NULL,
                commit_message TEXT NOT NULL,
                commit_date TEXT NOT NULL,
                branch TEXT NOT NULL,
                tag TEXT,
                is_release INTEGER NOT NULL DEFAULT 0,
                binary_path TEXT,
                build_status TEXT NOT NULL DEFAULT 'Pending',
                UNIQUE(engine_id, commit_hash)
            );

            CREATE TABLE IF NOT EXISTS test_jobs (
                id TEXT PRIMARY KEY,
                engine_id TEXT NOT NULL REFERENCES engines(id),
                dev_revision_id TEXT NOT NULL REFERENCES revisions(id),
                base_revision_id TEXT NOT NULL REFERENCES revisions(id),
                time_control TEXT NOT NULL,  -- JSON
                opening_book TEXT,
                status TEXT NOT NULL DEFAULT 'Queued',
                priority INTEGER NOT NULL DEFAULT 0,
                job_type TEXT NOT NULL DEFAULT 'Sequential',
                created_at TEXT NOT NULL,
                started_at TEXT,
                completed_at TEXT,
                wins INTEGER NOT NULL DEFAULT 0,
                losses INTEGER NOT NULL DEFAULT 0,
                draws INTEGER NOT NULL DEFAULT 0,
                elo_diff REAL,
                elo_error REAL,
                los REAL,
                sprt_result TEXT DEFAULT 'Inconclusive'
            );

            CREATE TABLE IF NOT EXISTS games (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                test_job_id TEXT NOT NULL REFERENCES test_jobs(id),
                game_number INTEGER NOT NULL,
                result TEXT NOT NULL,
                pgn TEXT NOT NULL,
                opening TEXT NOT NULL DEFAULT '',
                move_count INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS bisect_sessions (
                id TEXT PRIMARY KEY,
                engine_id TEXT NOT NULL REFERENCES engines(id),
                good_revision_id TEXT NOT NULL REFERENCES revisions(id),
                bad_revision_id TEXT NOT NULL REFERENCES revisions(id),
                commit_range TEXT NOT NULL,  -- JSON array
                current_index INTEGER,
                status TEXT NOT NULL DEFAULT 'Running',
                culprit_revision_id TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_revisions_engine
                ON revisions(engine_id, commit_date);
            CREATE INDEX IF NOT EXISTS idx_revisions_hash
                ON revisions(engine_id, commit_hash);
            CREATE INDEX IF NOT EXISTS idx_jobs_status
                ON test_jobs(status, priority DESC);
            CREATE INDEX IF NOT EXISTS idx_jobs_engine
                ON test_jobs(engine_id, created_at);
            CREATE INDEX IF NOT EXISTS idx_games_job
                ON games(test_job_id, game_number);
            ",
        )?;
        Ok(())
    }

    // ── Engine CRUD ──────────────────────────────────────────────

    pub fn insert_engine(&self, engine: &Engine) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO engines (id, name, repo_url, local_path, branches, build_cmd, binary_path, start_from)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                engine.id,
                engine.name,
                engine.repo_url,
                engine.local_path.to_string_lossy().to_string(),
                serde_json::to_string(&engine.branches)?,
                engine.build_cmd,
                engine.binary_path,
                engine.start_from,
            ],
        )?;
        Ok(())
    }

    pub fn get_engines(&self) -> Result<Vec<Engine>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT * FROM engines")?;
        let engines = stmt
            .query_map([], |row| {
                let branches_str: String = row.get(4)?;
                let local_path_str: String = row.get(3)?;
                Ok(Engine {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    repo_url: row.get(2)?,
                    local_path: std::path::PathBuf::from(local_path_str),
                    branches: serde_json::from_str(&branches_str).unwrap_or_default(),
                    build_cmd: row.get(5)?,
                    binary_path: row.get(6)?,
                    start_from: row.get(7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(engines)
    }

    // ── Revision CRUD ────────────────────────────────────────────

    pub fn insert_revision(&self, rev: &EngineRevision) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO revisions (id, engine_id, commit_hash, commit_message, commit_date, branch, tag, is_release, binary_path, build_status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                rev.id,
                rev.engine_id,
                rev.commit_hash,
                rev.commit_message,
                rev.commit_date.to_rfc3339(),
                rev.branch,
                rev.tag,
                rev.is_release as i32,
                rev.binary_path.as_ref().map(|p| p.to_string_lossy().to_string()),
                serde_json::to_string(&rev.build_status)?,
            ],
        )?;
        Ok(())
    }

    pub fn get_revisions_for_engine(&self, engine_id: &str) -> Result<Vec<EngineRevision>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, engine_id, commit_hash, commit_message, commit_date, branch, tag, is_release, binary_path, build_status
             FROM revisions WHERE engine_id = ?1 ORDER BY commit_date ASC",
        )?;
        let revs = stmt
            .query_map(params![engine_id], |row| {
                let date_str: String = row.get(4)?;
                let status_str: String = row.get(9)?;
                let binary_str: Option<String> = row.get(8)?;
                Ok(EngineRevision {
                    id: row.get(0)?,
                    engine_id: row.get(1)?,
                    commit_hash: row.get(2)?,
                    commit_message: row.get(3)?,
                    commit_date: chrono::DateTime::parse_from_rfc3339(&date_str)
                        .unwrap()
                        .with_timezone(&chrono::Utc),
                    branch: row.get(5)?,
                    tag: row.get(6)?,
                    is_release: row.get::<_, i32>(7)? != 0,
                    binary_path: binary_str.map(std::path::PathBuf::from),
                    build_status: serde_json::from_str(&status_str).unwrap_or(BuildStatus::Pending),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(revs)
    }

    pub fn update_build_status(
        &self,
        revision_id: &str,
        status: BuildStatus,
        binary_path: Option<&Path>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE revisions SET build_status = ?1, binary_path = ?2 WHERE id = ?3",
            params![
                serde_json::to_string(&status)?,
                binary_path.map(|p| p.to_string_lossy().to_string()),
                revision_id,
            ],
        )?;
        Ok(())
    }

    // ── Test Job CRUD ────────────────────────────────────────────

    pub fn insert_test_job(&self, job: &TestJob) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO test_jobs (id, engine_id, dev_revision_id, base_revision_id, time_control, opening_book, status, priority, job_type, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                job.id,
                job.engine_id,
                job.dev_revision_id,
                job.base_revision_id,
                serde_json::to_string(&job.time_control)?,
                job.opening_book,
                serde_json::to_string(&job.status)?,
                job.priority,
                serde_json::to_string(&job.job_type)?,
                job.created_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn get_next_job(&self) -> Result<Option<TestJob>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, engine_id, dev_revision_id, base_revision_id, time_control, opening_book, status, priority, job_type, created_at
             FROM test_jobs WHERE status = '\"Queued\"' ORDER BY priority DESC, created_at ASC LIMIT 1",
        )?;
        let mut rows = stmt.query_map([], |row| {
            let tc_str: String = row.get(4)?;
            let status_str: String = row.get(6)?;
            let jt_str: String = row.get(8)?;
            let date_str: String = row.get(9)?;
            Ok(TestJob {
                id: row.get(0)?,
                engine_id: row.get(1)?,
                dev_revision_id: row.get(2)?,
                base_revision_id: row.get(3)?,
                time_control: serde_json::from_str(&tc_str).unwrap(),
                opening_book: row.get(5)?,
                status: serde_json::from_str(&status_str).unwrap(),
                priority: row.get(7)?,
                job_type: serde_json::from_str(&jt_str).unwrap(),
                created_at: chrono::DateTime::parse_from_rfc3339(&date_str)
                    .unwrap()
                    .with_timezone(&chrono::Utc),
                started_at: None,
                completed_at: None,
                result: None,
            })
        })?;
        Ok(rows.next().transpose()?)
    }

    pub fn update_job_result(
        &self,
        job_id: &str,
        wins: u32,
        losses: u32,
        draws: u32,
        elo_diff: f64,
        elo_error: f64,
        los: f64,
        sprt_result: SprtResult,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE test_jobs SET wins = ?1, losses = ?2, draws = ?3, elo_diff = ?4, elo_error = ?5, los = ?6, sprt_result = ?7 WHERE id = ?8",
            params![
                wins,
                losses,
                draws,
                elo_diff,
                elo_error,
                los,
                serde_json::to_string(&sprt_result)?,
                job_id,
            ],
        )?;
        Ok(())
    }

    pub fn set_job_status(&self, job_id: &str, status: TestStatus) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        match status {
            TestStatus::Running => {
                conn.execute(
                    "UPDATE test_jobs SET status = ?1, started_at = ?2 WHERE id = ?3",
                    params![serde_json::to_string(&status)?, now, job_id],
                )?;
            }
            TestStatus::Completed | TestStatus::Failed | TestStatus::Cancelled => {
                conn.execute(
                    "UPDATE test_jobs SET status = ?1, completed_at = ?2 WHERE id = ?3",
                    params![serde_json::to_string(&status)?, now, job_id],
                )?;
            }
            _ => {
                conn.execute(
                    "UPDATE test_jobs SET status = ?1 WHERE id = ?2",
                    params![serde_json::to_string(&status)?, job_id],
                )?;
            }
        }
        Ok(())
    }

    // ── Game records ─────────────────────────────────────────────

    pub fn insert_game(&self, job_id: &str, record: &GameRecord) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO games (test_job_id, game_number, result, pgn, opening, move_count)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                job_id,
                record.game_number,
                serde_json::to_string(&record.result)?,
                record.pgn,
                record.opening,
                record.move_count,
            ],
        )?;
        Ok(())
    }

    // ── Elo timeline queries ─────────────────────────────────────

    pub fn get_elo_timeline(&self, engine_id: &str, branch: Option<&str>) -> Result<Vec<EloDataPoint>> {
        let conn = self.conn.lock().unwrap();
        let query = if branch.is_some() {
            "SELECT r.id, r.commit_hash, r.commit_message, r.commit_date, r.branch, r.tag, r.is_release,
                    j.elo_diff, j.elo_error, (j.wins + j.losses + j.draws) as total_games
             FROM revisions r
             JOIN test_jobs j ON j.dev_revision_id = r.id
             WHERE r.engine_id = ?1 AND r.branch = ?2 AND j.status = '\"Completed\"'
             ORDER BY r.commit_date ASC"
        } else {
            "SELECT r.id, r.commit_hash, r.commit_message, r.commit_date, r.branch, r.tag, r.is_release,
                    j.elo_diff, j.elo_error, (j.wins + j.losses + j.draws) as total_games
             FROM revisions r
             JOIN test_jobs j ON j.dev_revision_id = r.id
             WHERE r.engine_id = ?1 AND j.status = '\"Completed\"'
             ORDER BY r.commit_date ASC"
        };

        let mut stmt = conn.prepare(query)?;
        let params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = if let Some(b) = branch {
            vec![Box::new(engine_id.to_string()), Box::new(b.to_string())]
        } else {
            vec![Box::new(engine_id.to_string())]
        };

        let points = stmt
            .query_map(rusqlite::params_from_iter(params_vec.iter()), |row| {
                let date_str: String = row.get(3)?;
                Ok(EloDataPoint {
                    revision_id: row.get(0)?,
                    commit_hash: row.get(1)?,
                    commit_message: row.get(2)?,
                    commit_date: chrono::DateTime::parse_from_rfc3339(&date_str)
                        .unwrap()
                        .with_timezone(&chrono::Utc),
                    branch: row.get(4)?,
                    tag: row.get(5)?,
                    is_release: row.get::<_, i32>(6)? != 0,
                    elo: row.get(7)?,
                    elo_error: row.get(8)?,
                    games_played: row.get(9)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(points)
    }

    // ── Statistics ────────────────────────────────────────────────

    pub fn get_system_status(&self) -> Result<SystemStatus> {
        let conn = self.conn.lock().unwrap();

        let active: u32 = conn.query_row(
            "SELECT COUNT(*) FROM test_jobs WHERE status = '\"Running\"'",
            [], |r| r.get(0),
        )?;
        let queued: u32 = conn.query_row(
            "SELECT COUNT(*) FROM test_jobs WHERE status = '\"Queued\"'",
            [], |r| r.get(0),
        )?;
        let completed: u32 = conn.query_row(
            "SELECT COUNT(*) FROM test_jobs WHERE status = '\"Completed\"'",
            [], |r| r.get(0),
        )?;
        let engines: u32 = conn.query_row(
            "SELECT COUNT(*) FROM engines", [], |r| r.get(0),
        )?;
        let total_games: u64 = conn.query_row(
            "SELECT COALESCE(SUM(wins + losses + draws), 0) FROM test_jobs",
            [], |r| r.get(0),
        )?;

        Ok(SystemStatus {
            active_jobs: active,
            queued_jobs: queued,
            completed_jobs: completed,
            engines_tracked: engines,
            total_games_played: total_games,
            uptime_seconds: 0, // Set by the caller
            games_per_minute: 0.0,
        })
    }
}

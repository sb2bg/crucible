//! SQLite storage layer.
//!
//! All test results, engine revisions, and job state are persisted here.
//! The database is the source of truth for the Elo timeline.

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension, Row, TransactionBehavior};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::types::*;

#[derive(Clone)]
pub struct Storage {
    conn: Arc<Mutex<Connection>>,
}

impl Storage {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Self::open_connection(path)?;
        let storage = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        storage.migrate()?;
        Ok(storage)
    }

    pub fn in_memory() -> Result<Self> {
        let conn = Self::open_in_memory_connection()?;
        let storage = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        storage.migrate()?;
        Ok(storage)
    }

    fn open_connection(path: &Path) -> Result<Connection> {
        let conn = Connection::open(path)?;
        Self::configure_connection(&conn)?;
        Ok(conn)
    }

    fn open_in_memory_connection() -> Result<Connection> {
        let conn = Connection::open_in_memory()?;
        Self::configure_connection(&conn)?;
        Ok(conn)
    }

    fn configure_connection(conn: &Connection) -> Result<()> {
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Ok(())
    }

    fn migrate(&self) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS engines (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                repo_url TEXT NOT NULL,
                local_path TEXT NOT NULL,
                branches TEXT NOT NULL,  -- JSON array
                experimental_branches TEXT NOT NULL DEFAULT '[]',  -- JSON array
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
                binary_fingerprint TEXT,
                build_status TEXT NOT NULL DEFAULT 'Pending',
                UNIQUE(engine_id, commit_hash)
            );

            CREATE TABLE IF NOT EXISTS revision_branches (
                revision_id TEXT NOT NULL REFERENCES revisions(id),
                branch TEXT NOT NULL,
                PRIMARY KEY (revision_id, branch)
            );

            CREATE TABLE IF NOT EXISTS test_jobs (
                id TEXT PRIMARY KEY,
                engine_id TEXT NOT NULL REFERENCES engines(id),
                dev_revision_id TEXT NOT NULL REFERENCES revisions(id),
                base_revision_id TEXT NOT NULL REFERENCES revisions(id),
                branch_context TEXT,
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
                move_count INTEGER NOT NULL DEFAULT 0,
                created_at TEXT
            );

            CREATE TABLE IF NOT EXISTS bisect_sessions (
                id TEXT PRIMARY KEY,
                engine_id TEXT NOT NULL REFERENCES engines(id),
                good_revision_id TEXT NOT NULL REFERENCES revisions(id),
                bad_revision_id TEXT NOT NULL REFERENCES revisions(id),
                commit_range TEXT NOT NULL,  -- JSON array
                current_index INTEGER,
                current_job_id TEXT,
                phase TEXT NOT NULL DEFAULT '\"Sampling\"',
                pending_indices TEXT NOT NULL DEFAULT '[]',
                probe_history TEXT NOT NULL DEFAULT '[]',
                candidate_revision_id TEXT,
                candidate_index INTEGER,
                status TEXT NOT NULL DEFAULT 'Running',
                culprit_revision_id TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_revisions_engine
                ON revisions(engine_id, commit_date);
            CREATE INDEX IF NOT EXISTS idx_revisions_hash
                ON revisions(engine_id, commit_hash);
            CREATE INDEX IF NOT EXISTS idx_revision_branches_branch
                ON revision_branches(branch, revision_id);
            CREATE INDEX IF NOT EXISTS idx_jobs_status
                ON test_jobs(status, priority DESC);
            CREATE INDEX IF NOT EXISTS idx_jobs_engine
                ON test_jobs(engine_id, created_at);
            CREATE INDEX IF NOT EXISTS idx_games_job
                ON games(test_job_id, game_number);
            ",
        )?;
        self.ensure_bisect_column(&tx, "current_job_id", "TEXT")?;
        self.ensure_bisect_column(&tx, "phase", "TEXT NOT NULL DEFAULT 'Sampling'")?;
        self.ensure_bisect_column(&tx, "pending_indices", "TEXT NOT NULL DEFAULT '[]'")?;
        self.ensure_bisect_column(&tx, "probe_history", "TEXT NOT NULL DEFAULT '[]'")?;
        self.ensure_bisect_column(&tx, "candidate_revision_id", "TEXT")?;
        self.ensure_bisect_column(&tx, "candidate_index", "INTEGER")?;
        self.ensure_engine_column(&tx, "experimental_branches", "TEXT NOT NULL DEFAULT '[]'")?;
        self.ensure_revision_column(&tx, "binary_fingerprint", "TEXT")?;
        self.ensure_test_job_column(&tx, "branch_context", "TEXT")?;
        self.ensure_games_column(&tx, "created_at", "TEXT")?;
        tx.execute_batch(
            "
            INSERT OR IGNORE INTO revision_branches (revision_id, branch)
            SELECT id, branch FROM revisions;

            UPDATE revisions
            SET build_status = trim(build_status, '\"')
            WHERE build_status LIKE '\"%\"';

            UPDATE test_jobs
            SET status = trim(status, '\"'),
                job_type = trim(job_type, '\"'),
                sprt_result = trim(sprt_result, '\"')
            WHERE status LIKE '\"%\"'
               OR job_type LIKE '\"%\"'
               OR sprt_result LIKE '\"%\"';

            UPDATE games
            SET result = trim(result, '\"')
            WHERE result LIKE '\"%\"';

            UPDATE bisect_sessions
            SET status = trim(status, '\"'),
                phase = trim(phase, '\"')
            WHERE status LIKE '\"%\"'
               OR phase LIKE '\"%\"';
            ",
        )?;
        tx.commit()?;
        Ok(())
    }

    fn ensure_bisect_column(
        &self,
        conn: &Connection,
        column_name: &str,
        column_sql: &str,
    ) -> Result<()> {
        let mut stmt = conn.prepare("PRAGMA table_info(bisect_sessions)")?;
        let columns = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        if !columns.iter().any(|name| name == column_name) {
            conn.execute(
                &format!(
                    "ALTER TABLE bisect_sessions ADD COLUMN {} {}",
                    column_name, column_sql
                ),
                [],
            )?;
        }
        Ok(())
    }

    fn ensure_revision_column(
        &self,
        conn: &Connection,
        column_name: &str,
        column_sql: &str,
    ) -> Result<()> {
        let mut stmt = conn.prepare("PRAGMA table_info(revisions)")?;
        let columns = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        if !columns.iter().any(|name| name == column_name) {
            conn.execute(
                &format!(
                    "ALTER TABLE revisions ADD COLUMN {} {}",
                    column_name, column_sql
                ),
                [],
            )?;
        }
        Ok(())
    }

    fn ensure_engine_column(
        &self,
        conn: &Connection,
        column_name: &str,
        column_sql: &str,
    ) -> Result<()> {
        let mut stmt = conn.prepare("PRAGMA table_info(engines)")?;
        let columns = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        if !columns.iter().any(|name| name == column_name) {
            conn.execute(
                &format!(
                    "ALTER TABLE engines ADD COLUMN {} {}",
                    column_name, column_sql
                ),
                [],
            )?;
        }
        Ok(())
    }

    fn ensure_test_job_column(
        &self,
        conn: &Connection,
        column_name: &str,
        column_sql: &str,
    ) -> Result<()> {
        let mut stmt = conn.prepare("PRAGMA table_info(test_jobs)")?;
        let columns = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        if !columns.iter().any(|name| name == column_name) {
            conn.execute(
                &format!(
                    "ALTER TABLE test_jobs ADD COLUMN {} {}",
                    column_name, column_sql
                ),
                [],
            )?;
        }
        Ok(())
    }

    fn ensure_games_column(
        &self,
        conn: &Connection,
        column_name: &str,
        column_sql: &str,
    ) -> Result<()> {
        let mut stmt = conn.prepare("PRAGMA table_info(games)")?;
        let columns = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        if !columns.iter().any(|name| name == column_name) {
            conn.execute(
                &format!(
                    "ALTER TABLE games ADD COLUMN {} {}",
                    column_name, column_sql
                ),
                [],
            )?;
        }
        Ok(())
    }

    // ── Engine CRUD ──────────────────────────────────────────────

    pub fn insert_engine(&self, engine: &Engine) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO engines (id, name, repo_url, local_path, branches, experimental_branches, build_cmd, binary_path, start_from)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                engine.id,
                engine.name,
                engine.repo_url,
                engine.local_path.to_string_lossy().to_string(),
                serde_json::to_string(&engine.branches)?,
                serde_json::to_string(&engine.experimental_branches)?,
                engine.build_cmd,
                engine.binary_path,
                engine.start_from,
            ],
        )?;
        Ok(())
    }

    pub fn get_engines(&self) -> Result<Vec<Engine>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, repo_url, local_path, branches, experimental_branches, build_cmd, binary_path, start_from
             FROM engines",
        )?;
        let engines = stmt
            .query_map([], |row| {
                let branches_str: String = row.get(4)?;
                let experimental_branches_str: String = row.get(5)?;
                let local_path_str: String = row.get(3)?;
                Ok(Engine {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    repo_url: row.get(2)?,
                    local_path: std::path::PathBuf::from(local_path_str),
                    branches: serde_json::from_str(&branches_str).unwrap_or_default(),
                    experimental_branches: serde_json::from_str(&experimental_branches_str)
                        .unwrap_or_default(),
                    build_cmd: row.get(6)?,
                    binary_path: row.get(7)?,
                    start_from: row.get(8)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(engines)
    }

    pub fn get_engine_by_name(&self, name: &str) -> Result<Option<Engine>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, repo_url, local_path, branches, experimental_branches, build_cmd, binary_path, start_from
             FROM engines WHERE name = ?1 LIMIT 1",
        )?;

        let engine = stmt
            .query_row(params![name], |row| {
                let branches_str: String = row.get(4)?;
                let experimental_branches_str: String = row.get(5)?;
                let local_path_str: String = row.get(3)?;
                Ok(Engine {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    repo_url: row.get(2)?,
                    local_path: std::path::PathBuf::from(local_path_str),
                    branches: serde_json::from_str(&branches_str).unwrap_or_default(),
                    experimental_branches: serde_json::from_str(&experimental_branches_str)
                        .unwrap_or_default(),
                    build_cmd: row.get(6)?,
                    binary_path: row.get(7)?,
                    start_from: row.get(8)?,
                })
            })
            .optional()?;

        Ok(engine)
    }

    pub fn get_engine_by_id(&self, engine_id: &str) -> Result<Option<Engine>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, repo_url, local_path, branches, experimental_branches, build_cmd, binary_path, start_from
             FROM engines WHERE id = ?1 LIMIT 1",
        )?;

        let engine = stmt
            .query_row(params![engine_id], |row| {
                let branches_str: String = row.get(4)?;
                let experimental_branches_str: String = row.get(5)?;
                let local_path_str: String = row.get(3)?;
                Ok(Engine {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    repo_url: row.get(2)?,
                    local_path: std::path::PathBuf::from(local_path_str),
                    branches: serde_json::from_str(&branches_str).unwrap_or_default(),
                    experimental_branches: serde_json::from_str(&experimental_branches_str)
                        .unwrap_or_default(),
                    build_cmd: row.get(6)?,
                    binary_path: row.get(7)?,
                    start_from: row.get(8)?,
                })
            })
            .optional()?;

        Ok(engine)
    }

    // ── Revision CRUD ────────────────────────────────────────────

    pub fn insert_revision(&self, rev: &EngineRevision) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO revisions (id, engine_id, commit_hash, commit_message, commit_date, branch, tag, is_release, binary_path, binary_fingerprint, build_status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
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
                rev.binary_fingerprint,
                encode_build_status(rev.build_status),
            ],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO revision_branches (revision_id, branch) VALUES (?1, ?2)",
            params![rev.id, rev.branch],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn get_revisions_for_engine(&self, engine_id: &str) -> Result<Vec<EngineRevision>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, engine_id, commit_hash, commit_message, commit_date, branch, tag, is_release, binary_path, binary_fingerprint, build_status
             FROM revisions WHERE engine_id = ?1 ORDER BY commit_date ASC",
        )?;
        let revs = stmt
            .query_map(params![engine_id], |row| {
                let date_str: String = row.get(4)?;
                let status_str: String = row.get(10)?;
                let binary_str: Option<String> = row.get(8)?;
                Ok(EngineRevision {
                    id: row.get(0)?,
                    engine_id: row.get(1)?,
                    commit_hash: row.get(2)?,
                    commit_message: row.get(3)?,
                    commit_date: parse_timestamp_column(&date_str, 4)?,
                    branch: row.get(5)?,
                    tag: row.get(6)?,
                    is_release: row.get::<_, i32>(7)? != 0,
                    binary_path: binary_str.map(std::path::PathBuf::from),
                    binary_fingerprint: row.get(9)?,
                    build_status: decode_build_status(&status_str)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(revs)
    }

    pub fn get_branch_revisions_for_engine(&self, engine_id: &str) -> Result<Vec<EngineRevision>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT r.id, r.engine_id, r.commit_hash, r.commit_message, r.commit_date, rb.branch, r.tag, r.is_release, r.binary_path, r.binary_fingerprint, r.build_status
             FROM revisions r
             JOIN revision_branches rb ON rb.revision_id = r.id
             WHERE r.engine_id = ?1
             ORDER BY r.commit_date ASC, rb.branch ASC",
        )?;
        let revs = stmt
            .query_map(params![engine_id], map_revision_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(revs)
    }

    pub fn get_revision_branches(&self, revision_id: &str) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT branch
             FROM revision_branches
             WHERE revision_id = ?1
             ORDER BY branch ASC",
        )?;
        let branches = stmt
            .query_map(params![revision_id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(branches)
    }

    pub fn get_revision_by_id(&self, revision_id: &str) -> Result<Option<EngineRevision>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, engine_id, commit_hash, commit_message, commit_date, branch, tag, is_release, binary_path, binary_fingerprint, build_status
             FROM revisions WHERE id = ?1 LIMIT 1",
        )?;

        let revision = stmt
            .query_row(params![revision_id], |row| {
                let date_str: String = row.get(4)?;
                let status_str: String = row.get(10)?;
                let binary_str: Option<String> = row.get(8)?;
                Ok(EngineRevision {
                    id: row.get(0)?,
                    engine_id: row.get(1)?,
                    commit_hash: row.get(2)?,
                    commit_message: row.get(3)?,
                    commit_date: parse_timestamp_column(&date_str, 4)?,
                    branch: row.get(5)?,
                    tag: row.get(6)?,
                    is_release: row.get::<_, i32>(7)? != 0,
                    binary_path: binary_str.map(std::path::PathBuf::from),
                    binary_fingerprint: row.get(9)?,
                    build_status: decode_build_status(&status_str)?,
                })
            })
            .optional()?;

        Ok(revision)
    }

    pub fn get_revision_by_hash_prefix(
        &self,
        engine_id: &str,
        hash_prefix: &str,
    ) -> Result<Option<EngineRevision>> {
        let conn = self.conn.lock().unwrap();
        let like = format!("{}%", hash_prefix);
        let mut stmt = conn.prepare(
            "SELECT id, engine_id, commit_hash, commit_message, commit_date, branch, tag, is_release, binary_path, binary_fingerprint, build_status
             FROM revisions
             WHERE engine_id = ?1 AND commit_hash LIKE ?2
             ORDER BY commit_date ASC
             LIMIT 2",
        )?;

        let revisions = stmt
            .query_map(params![engine_id, like], |row| {
                let date_str: String = row.get(4)?;
                let status_str: String = row.get(10)?;
                let binary_str: Option<String> = row.get(8)?;
                Ok(EngineRevision {
                    id: row.get(0)?,
                    engine_id: row.get(1)?,
                    commit_hash: row.get(2)?,
                    commit_message: row.get(3)?,
                    commit_date: parse_timestamp_column(&date_str, 4)?,
                    branch: row.get(5)?,
                    tag: row.get(6)?,
                    is_release: row.get::<_, i32>(7)? != 0,
                    binary_path: binary_str.map(std::path::PathBuf::from),
                    binary_fingerprint: row.get(9)?,
                    build_status: decode_build_status(&status_str)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        match revisions.as_slice() {
            [] => Ok(None),
            [revision] => Ok(Some(revision.clone())),
            _ => anyhow::bail!("Commit prefix '{}' is ambiguous", hash_prefix),
        }
    }

    pub fn get_revision_by_ref_prefix(
        &self,
        engine_id: &str,
        revision_ref: &str,
    ) -> Result<Option<EngineRevision>> {
        if let Some(revision) = self.get_revision_by_hash_prefix(engine_id, revision_ref)? {
            return Ok(Some(revision));
        }

        let conn = self.conn.lock().unwrap();
        let like = format!("{}%", revision_ref);
        let mut stmt = conn.prepare(
            "SELECT id, engine_id, commit_hash, commit_message, commit_date, branch, tag, is_release, binary_path, binary_fingerprint, build_status
             FROM revisions
             WHERE engine_id = ?1 AND tag IS NOT NULL AND tag LIKE ?2
             ORDER BY commit_date ASC
             LIMIT 2",
        )?;

        let revisions = stmt
            .query_map(params![engine_id, like], |row| {
                let date_str: String = row.get(4)?;
                let status_str: String = row.get(10)?;
                let binary_str: Option<String> = row.get(8)?;
                Ok(EngineRevision {
                    id: row.get(0)?,
                    engine_id: row.get(1)?,
                    commit_hash: row.get(2)?,
                    commit_message: row.get(3)?,
                    commit_date: parse_timestamp_column(&date_str, 4)?,
                    branch: row.get(5)?,
                    tag: row.get(6)?,
                    is_release: row.get::<_, i32>(7)? != 0,
                    binary_path: binary_str.map(std::path::PathBuf::from),
                    binary_fingerprint: row.get(9)?,
                    build_status: decode_build_status(&status_str)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        match revisions.as_slice() {
            [] => Ok(None),
            [revision] => Ok(Some(revision.clone())),
            _ => anyhow::bail!("Revision reference '{}' is ambiguous", revision_ref),
        }
    }

    pub fn update_build_status(
        &self,
        revision_id: &str,
        status: BuildStatus,
        binary_path: Option<&Path>,
        binary_fingerprint: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE revisions SET build_status = ?1, binary_path = ?2, binary_fingerprint = ?3 WHERE id = ?4",
            params![
                encode_build_status(status),
                binary_path.map(|p| p.to_string_lossy().to_string()),
                binary_fingerprint,
                revision_id,
            ],
        )?;
        Ok(())
    }

    // ── Test Job CRUD ────────────────────────────────────────────

    pub fn insert_test_job(&self, job: &TestJob) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO test_jobs (id, engine_id, dev_revision_id, base_revision_id, branch_context, time_control, opening_book, status, priority, job_type, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                job.id,
                job.engine_id,
                job.dev_revision_id,
                job.base_revision_id,
                job.branch_context,
                serde_json::to_string(&job.time_control)?,
                job.opening_book,
                encode_test_status(job.status),
                job.priority,
                encode_job_type(job.job_type),
                job.created_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn has_test_job(
        &self,
        engine_id: &str,
        dev_revision_id: &str,
        base_revision_id: &str,
        job_type: JobType,
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let count: u32 = conn.query_row(
            "SELECT COUNT(*) FROM test_jobs
             WHERE engine_id = ?1
               AND dev_revision_id = ?2
               AND base_revision_id = ?3
               AND job_type = ?4",
            params![
                engine_id,
                dev_revision_id,
                base_revision_id,
                encode_job_type(job_type),
            ],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn claim_next_job(&self) -> Result<Option<TestJob>> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut stmt = tx.prepare(
            "SELECT id, engine_id, dev_revision_id, base_revision_id, branch_context, time_control, opening_book, status, priority, job_type, created_at, started_at, completed_at
             FROM test_jobs
             WHERE status = ?1
             ORDER BY priority DESC, created_at ASC
             LIMIT 1",
        )?;
        let mut job = stmt
            .query_row(
                params![encode_test_status(TestStatus::Queued)],
                map_test_job_row,
            )
            .optional()?;
        drop(stmt);

        let Some(mut job) = job.take() else {
            tx.commit()?;
            return Ok(None);
        };

        let now = chrono::Utc::now();
        let changed = tx.execute(
            "UPDATE test_jobs
             SET status = ?1, started_at = ?2
             WHERE id = ?3 AND status = ?4",
            params![
                encode_test_status(TestStatus::Running),
                now.to_rfc3339(),
                job.id,
                encode_test_status(TestStatus::Queued),
            ],
        )?;

        if changed == 1 {
            tx.commit()?;
            job.status = TestStatus::Running;
            job.started_at = Some(now);
            Ok(Some(job))
        } else {
            tx.rollback()?;
            Ok(None)
        }
    }

    pub fn requeue_running_jobs(&self) -> Result<usize> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM games
             WHERE test_job_id IN (
                 SELECT id FROM test_jobs WHERE status = ?1
             )",
            params![encode_test_status(TestStatus::Running)],
        )?;
        let reset = tx.execute(
            "UPDATE test_jobs
             SET status = ?1,
                 started_at = NULL,
                 completed_at = NULL,
                 wins = 0,
                 losses = 0,
                 draws = 0,
                 elo_diff = NULL,
                 elo_error = NULL,
                 los = NULL,
                 sprt_result = ?2
             WHERE status = ?3",
            params![
                encode_test_status(TestStatus::Queued),
                encode_sprt_result(SprtResult::Inconclusive),
                encode_test_status(TestStatus::Running),
            ],
        )?;
        tx.commit()?;
        Ok(reset)
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
                encode_sprt_result(sprt_result),
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
                    params![encode_test_status(status), now, job_id],
                )?;
            }
            TestStatus::Completed | TestStatus::Failed | TestStatus::Cancelled => {
                conn.execute(
                    "UPDATE test_jobs SET status = ?1, completed_at = ?2 WHERE id = ?3",
                    params![encode_test_status(status), now, job_id],
                )?;
            }
            _ => {
                conn.execute(
                    "UPDATE test_jobs SET status = ?1 WHERE id = ?2",
                    params![encode_test_status(status), job_id],
                )?;
            }
        }
        Ok(())
    }

    pub fn get_job_status(&self, job_id: &str) -> Result<Option<TestStatus>> {
        let conn = self.conn.lock().unwrap();
        let status = conn
            .query_row(
                "SELECT status FROM test_jobs WHERE id = ?1 LIMIT 1",
                params![job_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;

        status
            .as_deref()
            .map(decode_test_status)
            .transpose()
            .map_err(Into::into)
    }

    pub fn cancel_job(&self, job_id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE test_jobs
             SET status = ?1, completed_at = ?2
             WHERE id = ?3 AND status IN (?4, ?5)",
            params![
                encode_test_status(TestStatus::Cancelled),
                chrono::Utc::now().to_rfc3339(),
                job_id,
                encode_test_status(TestStatus::Queued),
                encode_test_status(TestStatus::Running),
            ],
        )?;
        Ok(changed > 0)
    }

    // ── Game records ─────────────────────────────────────────────

    pub fn insert_game(&self, job_id: &str, record: &GameRecord) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO games (test_job_id, game_number, result, pgn, opening, move_count, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                job_id,
                record.game_number,
                encode_game_result(record.result),
                record.pgn,
                record.opening,
                record.move_count,
                chrono::Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn list_recent_jobs(&self, limit: usize) -> Result<Vec<JobSummary>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT
                j.id,
                j.engine_id,
                e.name,
                j.dev_revision_id,
                dev.commit_hash,
                j.base_revision_id,
                base.commit_hash,
                j.branch_context,
                j.status,
                j.priority,
                j.job_type,
                j.created_at,
                j.started_at,
                j.completed_at,
                j.wins,
                j.losses,
                j.draws,
                j.elo_diff,
                j.elo_error,
                j.los,
                j.sprt_result
             FROM test_jobs j
             JOIN engines e ON e.id = j.engine_id
             JOIN revisions dev ON dev.id = j.dev_revision_id
             JOIN revisions base ON base.id = j.base_revision_id
             ORDER BY j.created_at DESC
             LIMIT ?1",
        )?;

        let jobs = stmt
            .query_map(params![limit as i64], |row| {
                let status_str: String = row.get(8)?;
                let job_type_str: String = row.get(10)?;
                let created_at: String = row.get(11)?;
                let started_at: Option<String> = row.get(12)?;
                let completed_at: Option<String> = row.get(13)?;
                let sprt_result: Option<String> = row.get(20)?;
                Ok(JobSummary {
                    id: row.get(0)?,
                    engine_id: row.get(1)?,
                    engine_name: row.get(2)?,
                    dev_revision_id: row.get(3)?,
                    dev_commit_hash: row.get(4)?,
                    base_revision_id: row.get(5)?,
                    base_commit_hash: row.get(6)?,
                    branch_context: row.get(7)?,
                    status: decode_test_status(&status_str)?,
                    priority: row.get(9)?,
                    job_type: decode_job_type(&job_type_str)?,
                    created_at: parse_timestamp_column(&created_at, 11)?,
                    started_at: parse_optional_timestamp_column(started_at, 12)?,
                    completed_at: parse_optional_timestamp_column(completed_at, 13)?,
                    wins: row.get(14)?,
                    losses: row.get(15)?,
                    draws: row.get(16)?,
                    elo_diff: row.get(17)?,
                    elo_error: row.get(18)?,
                    los: row.get(19)?,
                    sprt_result: sprt_result.as_deref().map(decode_sprt_result).transpose()?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(jobs)
    }

    pub fn list_all_jobs(&self) -> Result<Vec<JobSummary>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT
                j.id,
                j.engine_id,
                e.name,
                j.dev_revision_id,
                dev.commit_hash,
                j.base_revision_id,
                base.commit_hash,
                j.branch_context,
                j.status,
                j.priority,
                j.job_type,
                j.created_at,
                j.started_at,
                j.completed_at,
                j.wins,
                j.losses,
                j.draws,
                j.elo_diff,
                j.elo_error,
                j.los,
                j.sprt_result
             FROM test_jobs j
             JOIN engines e ON e.id = j.engine_id
             JOIN revisions dev ON dev.id = j.dev_revision_id
             JOIN revisions base ON base.id = j.base_revision_id
             ORDER BY j.created_at DESC",
        )?;

        let jobs = stmt
            .query_map([], |row| {
                let status_str: String = row.get(8)?;
                let job_type_str: String = row.get(10)?;
                let created_at: String = row.get(11)?;
                let started_at: Option<String> = row.get(12)?;
                let completed_at: Option<String> = row.get(13)?;
                let sprt_result: Option<String> = row.get(20)?;
                Ok(JobSummary {
                    id: row.get(0)?,
                    engine_id: row.get(1)?,
                    engine_name: row.get(2)?,
                    dev_revision_id: row.get(3)?,
                    dev_commit_hash: row.get(4)?,
                    base_revision_id: row.get(5)?,
                    base_commit_hash: row.get(6)?,
                    branch_context: row.get(7)?,
                    status: decode_test_status(&status_str)?,
                    priority: row.get(9)?,
                    job_type: decode_job_type(&job_type_str)?,
                    created_at: parse_timestamp_column(&created_at, 11)?,
                    started_at: parse_optional_timestamp_column(started_at, 12)?,
                    completed_at: parse_optional_timestamp_column(completed_at, 13)?,
                    wins: row.get(14)?,
                    losses: row.get(15)?,
                    draws: row.get(16)?,
                    elo_diff: row.get(17)?,
                    elo_error: row.get(18)?,
                    los: row.get(19)?,
                    sprt_result: sprt_result.as_deref().map(decode_sprt_result).transpose()?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(jobs)
    }

    pub fn list_jobs_for_revision(
        &self,
        revision_id: &str,
        limit: usize,
    ) -> Result<Vec<JobSummary>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT
                j.id,
                j.engine_id,
                e.name,
                j.dev_revision_id,
                dev.commit_hash,
                j.base_revision_id,
                base.commit_hash,
                j.branch_context,
                j.status,
                j.priority,
                j.job_type,
                j.created_at,
                j.started_at,
                j.completed_at,
                j.wins,
                j.losses,
                j.draws,
                j.elo_diff,
                j.elo_error,
                j.los,
                j.sprt_result
             FROM test_jobs j
             JOIN engines e ON e.id = j.engine_id
             JOIN revisions dev ON dev.id = j.dev_revision_id
             JOIN revisions base ON base.id = j.base_revision_id
             WHERE j.dev_revision_id = ?1 OR j.base_revision_id = ?1
             ORDER BY j.created_at DESC
             LIMIT ?2",
        )?;

        let jobs = stmt
            .query_map(params![revision_id, limit as i64], |row| {
                let status_str: String = row.get(8)?;
                let job_type_str: String = row.get(10)?;
                let created_at: String = row.get(11)?;
                let started_at: Option<String> = row.get(12)?;
                let completed_at: Option<String> = row.get(13)?;
                let sprt_result: Option<String> = row.get(20)?;
                Ok(JobSummary {
                    id: row.get(0)?,
                    engine_id: row.get(1)?,
                    engine_name: row.get(2)?,
                    dev_revision_id: row.get(3)?,
                    dev_commit_hash: row.get(4)?,
                    base_revision_id: row.get(5)?,
                    base_commit_hash: row.get(6)?,
                    branch_context: row.get(7)?,
                    status: decode_test_status(&status_str)?,
                    priority: row.get(9)?,
                    job_type: decode_job_type(&job_type_str)?,
                    created_at: parse_timestamp_column(&created_at, 11)?,
                    started_at: parse_optional_timestamp_column(started_at, 12)?,
                    completed_at: parse_optional_timestamp_column(completed_at, 13)?,
                    wins: row.get(14)?,
                    losses: row.get(15)?,
                    draws: row.get(16)?,
                    elo_diff: row.get(17)?,
                    elo_error: row.get(18)?,
                    los: row.get(19)?,
                    sprt_result: sprt_result.as_deref().map(decode_sprt_result).transpose()?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(jobs)
    }

    // ── Bisect session CRUD ──────────────────────────────────────

    pub fn insert_bisect_session(&self, session: &BisectSession) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO bisect_sessions
             (id, engine_id, good_revision_id, bad_revision_id, commit_range, current_index, current_job_id, phase, pending_indices, probe_history, candidate_revision_id, candidate_index, status, culprit_revision_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                session.id,
                session.engine_id,
                session.good_revision_id,
                session.bad_revision_id,
                serde_json::to_string(&session.commit_range)?,
                session.current_index.map(|i| i as i64),
                session.current_job_id,
                encode_hunt_phase(session.phase),
                serde_json::to_string(&session.pending_indices)?,
                serde_json::to_string(&session.probe_history)?,
                session.candidate_revision_id,
                session.candidate_index.map(|i| i as i64),
                encode_bisect_status(session.status),
                session.culprit_revision_id,
            ],
        )?;
        Ok(())
    }

    pub fn update_bisect_session(&self, session: &BisectSession) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE bisect_sessions
             SET good_revision_id = ?1,
                 bad_revision_id = ?2,
                 commit_range = ?3,
                 current_index = ?4,
                 current_job_id = ?5,
                 phase = ?6,
                 pending_indices = ?7,
                 probe_history = ?8,
                 candidate_revision_id = ?9,
                 candidate_index = ?10,
                 status = ?11,
                 culprit_revision_id = ?12
             WHERE id = ?13",
            params![
                session.good_revision_id,
                session.bad_revision_id,
                serde_json::to_string(&session.commit_range)?,
                session.current_index.map(|i| i as i64),
                session.current_job_id,
                encode_hunt_phase(session.phase),
                serde_json::to_string(&session.pending_indices)?,
                serde_json::to_string(&session.probe_history)?,
                session.candidate_revision_id,
                session.candidate_index.map(|i| i as i64),
                encode_bisect_status(session.status),
                session.culprit_revision_id,
                session.id,
            ],
        )?;
        Ok(())
    }

    pub fn get_running_bisect_sessions(&self) -> Result<Vec<BisectSession>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, engine_id, good_revision_id, bad_revision_id, commit_range, current_index, current_job_id, phase, pending_indices, probe_history, candidate_revision_id, candidate_index, status, culprit_revision_id
             FROM bisect_sessions
             WHERE status = ?1",
        )?;

        let sessions = stmt
            .query_map(
                params![encode_bisect_status(BisectStatus::Running)],
                |row| {
                    let commit_range: String = row.get(4)?;
                    let phase: String = row.get(7)?;
                    let pending_indices: String = row.get(8)?;
                    let probe_history: String = row.get(9)?;
                    let status: String = row.get(12)?;
                    let current_index: Option<i64> = row.get(5)?;
                    let candidate_index: Option<i64> = row.get(11)?;
                    Ok(BisectSession {
                        id: row.get(0)?,
                        engine_id: row.get(1)?,
                        good_revision_id: row.get(2)?,
                        bad_revision_id: row.get(3)?,
                        commit_range: serde_json::from_str(&commit_range).unwrap_or_default(),
                        current_index: current_index.map(|i| i as usize),
                        current_job_id: row.get(6)?,
                        phase: decode_hunt_phase(&phase)?,
                        pending_indices: serde_json::from_str(&pending_indices).unwrap_or_default(),
                        probe_history: serde_json::from_str(&probe_history).unwrap_or_default(),
                        candidate_revision_id: row.get(10)?,
                        candidate_index: candidate_index.map(|i| i as usize),
                        status: decode_bisect_status(&status)?,
                        culprit_revision_id: row.get(13)?,
                    })
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(sessions)
    }

    pub fn list_all_bisect_sessions(&self) -> Result<Vec<BisectSession>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, engine_id, good_revision_id, bad_revision_id, commit_range, current_index, current_job_id, phase, pending_indices, probe_history, candidate_revision_id, candidate_index, status, culprit_revision_id
             FROM bisect_sessions
             ORDER BY rowid DESC",
        )?;

        let sessions = stmt
            .query_map([], |row| {
                let commit_range: String = row.get(4)?;
                let current_index: Option<i64> = row.get(5)?;
                let phase: String = row.get(7)?;
                let pending_indices: String = row.get(8)?;
                let probe_history: String = row.get(9)?;
                let candidate_index: Option<i64> = row.get(11)?;
                let status: String = row.get(12)?;
                Ok(BisectSession {
                    id: row.get(0)?,
                    engine_id: row.get(1)?,
                    good_revision_id: row.get(2)?,
                    bad_revision_id: row.get(3)?,
                    commit_range: serde_json::from_str(&commit_range).unwrap_or_default(),
                    current_index: current_index.map(|i| i as usize),
                    current_job_id: row.get(6)?,
                    phase: decode_hunt_phase(&phase)?,
                    pending_indices: serde_json::from_str(&pending_indices).unwrap_or_default(),
                    probe_history: serde_json::from_str(&probe_history).unwrap_or_default(),
                    candidate_revision_id: row.get(10)?,
                    candidate_index: candidate_index.map(|i| i as usize),
                    status: decode_bisect_status(&status)?,
                    culprit_revision_id: row.get(13)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(sessions)
    }

    pub fn get_bisect_session_by_id(&self, session_id: &str) -> Result<Option<BisectSession>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, engine_id, good_revision_id, bad_revision_id, commit_range, current_index, current_job_id, phase, pending_indices, probe_history, candidate_revision_id, candidate_index, status, culprit_revision_id
             FROM bisect_sessions
             WHERE id = ?1
             LIMIT 1",
        )?;

        let session = stmt
            .query_row(params![session_id], |row| {
                let commit_range: String = row.get(4)?;
                let phase: String = row.get(7)?;
                let pending_indices: String = row.get(8)?;
                let probe_history: String = row.get(9)?;
                let status: String = row.get(12)?;
                let current_index: Option<i64> = row.get(5)?;
                let candidate_index: Option<i64> = row.get(11)?;
                Ok(BisectSession {
                    id: row.get(0)?,
                    engine_id: row.get(1)?,
                    good_revision_id: row.get(2)?,
                    bad_revision_id: row.get(3)?,
                    commit_range: serde_json::from_str(&commit_range).unwrap_or_default(),
                    current_index: current_index.map(|i| i as usize),
                    current_job_id: row.get(6)?,
                    phase: decode_hunt_phase(&phase)?,
                    pending_indices: serde_json::from_str(&pending_indices).unwrap_or_default(),
                    probe_history: serde_json::from_str(&probe_history).unwrap_or_default(),
                    candidate_revision_id: row.get(10)?,
                    candidate_index: candidate_index.map(|i| i as usize),
                    status: decode_bisect_status(&status)?,
                    culprit_revision_id: row.get(13)?,
                })
            })
            .optional()?;

        Ok(session)
    }

    pub fn mark_bisect_session_failed_for_job(&self, job_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE bisect_sessions
             SET status = ?1, phase = ?2, current_job_id = NULL, current_index = NULL
             WHERE current_job_id = ?3 AND status = ?4",
            params![
                encode_bisect_status(BisectStatus::Failed),
                encode_hunt_phase(HuntPhase::Failed),
                job_id,
                encode_bisect_status(BisectStatus::Running),
            ],
        )?;
        Ok(())
    }

    pub fn cancel_bisect_session(&self, session_id: &str) -> Result<bool> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current_job_id: Option<String> = tx
            .query_row(
                "SELECT current_job_id FROM bisect_sessions WHERE id = ?1 LIMIT 1",
                params![session_id],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        let updated = tx.execute(
            "UPDATE bisect_sessions
             SET status = ?1, phase = ?2, current_job_id = NULL, current_index = NULL
             WHERE id = ?3 AND status = ?4",
            params![
                encode_bisect_status(BisectStatus::Failed),
                encode_hunt_phase(HuntPhase::Failed),
                session_id,
                encode_bisect_status(BisectStatus::Running),
            ],
        )?;
        if updated == 0 {
            tx.commit()?;
            return Ok(false);
        }

        if let Some(job_id) = current_job_id {
            tx.execute(
                "UPDATE test_jobs
                 SET status = ?1, completed_at = ?2
                 WHERE id = ?3 AND status IN (?4, ?5)",
                params![
                    encode_test_status(TestStatus::Cancelled),
                    chrono::Utc::now().to_rfc3339(),
                    job_id,
                    encode_test_status(TestStatus::Queued),
                    encode_test_status(TestStatus::Running),
                ],
            )?;
        }

        tx.commit()?;
        Ok(true)
    }

    // ── Elo timeline queries ─────────────────────────────────────

    pub fn get_elo_timeline(
        &self,
        engine_id: &str,
        branch: Option<&str>,
    ) -> Result<Vec<EloDataPoint>> {
        let conn = self.conn.lock().unwrap();
        let query = if branch.is_some() {
            "SELECT r.id, r.commit_hash, r.commit_message, r.commit_date, COALESCE(j.branch_context, rb.branch), r.tag, r.is_release,
                    j.elo_diff, j.elo_error, (j.wins + j.losses + j.draws) as total_games
             FROM revisions r
             JOIN revision_branches rb ON rb.revision_id = r.id
             JOIN test_jobs j ON j.dev_revision_id = r.id
             WHERE r.engine_id = ?1 AND rb.branch = ?2 AND j.status = 'Completed' AND j.elo_diff IS NOT NULL AND j.elo_error IS NOT NULL
             ORDER BY r.commit_date ASC"
        } else {
            "SELECT r.id, r.commit_hash, r.commit_message, r.commit_date, COALESCE(j.branch_context, r.branch), r.tag, r.is_release,
                    j.elo_diff, j.elo_error, (j.wins + j.losses + j.draws) as total_games
             FROM revisions r
             JOIN test_jobs j ON j.dev_revision_id = r.id
             WHERE r.engine_id = ?1 AND j.status = 'Completed' AND j.elo_diff IS NOT NULL AND j.elo_error IS NOT NULL
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
                    commit_date: parse_timestamp_column(&date_str, 3)?,
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
            "SELECT COUNT(*) FROM test_jobs WHERE status = 'Running'",
            [],
            |r| r.get(0),
        )?;
        let queued: u32 = conn.query_row(
            "SELECT COUNT(*) FROM test_jobs WHERE status = 'Queued'",
            [],
            |r| r.get(0),
        )?;
        let completed: u32 = conn.query_row(
            "SELECT COUNT(*) FROM test_jobs WHERE status = 'Completed'",
            [],
            |r| r.get(0),
        )?;
        let engines: u32 = conn.query_row("SELECT COUNT(*) FROM engines", [], |r| r.get(0))?;
        let total_games: u64 = conn.query_row(
            "SELECT COALESCE(SUM(wins + losses + draws), 0) FROM test_jobs",
            [],
            |r| r.get(0),
        )?;
        let cutoff = (chrono::Utc::now() - chrono::Duration::minutes(15)).to_rfc3339();
        let recent_games: u64 = conn.query_row(
            "SELECT COUNT(*) FROM games WHERE created_at IS NOT NULL AND created_at >= ?1",
            params![cutoff],
            |r| r.get(0),
        )?;

        Ok(SystemStatus {
            active_jobs: active,
            queued_jobs: queued,
            completed_jobs: completed,
            engines_tracked: engines,
            total_games_played: total_games,
            uptime_seconds: 0, // Set by the caller
            games_per_minute: recent_games as f64 / 15.0,
        })
    }

    pub fn has_running_jobs_for_engine(&self, engine_id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let count: u32 = conn.query_row(
            "SELECT COUNT(*) FROM test_jobs WHERE engine_id = ?1 AND status = ?2",
            params![engine_id, encode_test_status(TestStatus::Running)],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn delete_engine(&self, engine_id: &str) -> Result<bool> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        let running_jobs: u32 = tx.query_row(
            "SELECT COUNT(*) FROM test_jobs WHERE engine_id = ?1 AND status = ?2",
            params![engine_id, encode_test_status(TestStatus::Running)],
            |row| row.get(0),
        )?;
        if running_jobs > 0 {
            anyhow::bail!("cannot delete engine while jobs are still running");
        }

        tx.execute(
            "DELETE FROM games
             WHERE test_job_id IN (SELECT id FROM test_jobs WHERE engine_id = ?1)",
            params![engine_id],
        )?;
        tx.execute(
            "DELETE FROM bisect_sessions WHERE engine_id = ?1",
            params![engine_id],
        )?;
        tx.execute(
            "DELETE FROM test_jobs WHERE engine_id = ?1",
            params![engine_id],
        )?;
        tx.execute(
            "DELETE FROM revision_branches
             WHERE revision_id IN (SELECT id FROM revisions WHERE engine_id = ?1)",
            params![engine_id],
        )?;
        tx.execute(
            "DELETE FROM revisions WHERE engine_id = ?1",
            params![engine_id],
        )?;
        let deleted = tx.execute("DELETE FROM engines WHERE id = ?1", params![engine_id])?;

        tx.commit()?;
        Ok(deleted > 0)
    }
}

fn map_test_job_row(row: &Row<'_>) -> rusqlite::Result<TestJob> {
    let tc_str: String = row.get(5)?;
    let status_str: String = row.get(7)?;
    let jt_str: String = row.get(9)?;
    let created_at: String = row.get(10)?;
    let started_at: Option<String> = row.get(11)?;
    let completed_at: Option<String> = row.get(12)?;
    Ok(TestJob {
        id: row.get(0)?,
        engine_id: row.get(1)?,
        dev_revision_id: row.get(2)?,
        base_revision_id: row.get(3)?,
        branch_context: row.get(4)?,
        time_control: serde_json::from_str(&tc_str).map_err(json_column_error)?,
        opening_book: row.get(6)?,
        status: decode_test_status(&status_str)?,
        priority: row.get(8)?,
        job_type: decode_job_type(&jt_str)?,
        created_at: parse_timestamp_column(&created_at, 10)?,
        started_at: parse_optional_timestamp_column(started_at, 11)?,
        completed_at: parse_optional_timestamp_column(completed_at, 12)?,
        result: None,
    })
}

fn map_revision_row(row: &Row<'_>) -> rusqlite::Result<EngineRevision> {
    let date_str: String = row.get(4)?;
    let status_str: String = row.get(10)?;
    let binary_str: Option<String> = row.get(8)?;
    Ok(EngineRevision {
        id: row.get(0)?,
        engine_id: row.get(1)?,
        commit_hash: row.get(2)?,
        commit_message: row.get(3)?,
        commit_date: parse_timestamp_column(&date_str, 4)?,
        branch: row.get(5)?,
        tag: row.get(6)?,
        is_release: row.get::<_, i32>(7)? != 0,
        binary_path: binary_str.map(std::path::PathBuf::from),
        binary_fingerprint: row.get(9)?,
        build_status: decode_build_status(&status_str)?,
    })
}

fn parse_timestamp_column(
    value: &str,
    column_index: usize,
) -> rusqlite::Result<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                column_index,
                rusqlite::types::Type::Text,
                Box::new(err),
            )
        })
}

fn parse_optional_timestamp_column(
    value: Option<String>,
    column_index: usize,
) -> rusqlite::Result<Option<chrono::DateTime<chrono::Utc>>> {
    value
        .as_deref()
        .map(|ts| parse_timestamp_column(ts, column_index))
        .transpose()
}

fn json_column_error(err: serde_json::Error) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
}

fn normalize_db_enum(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value)
}

fn encode_build_status(value: BuildStatus) -> &'static str {
    match value {
        BuildStatus::Pending => "Pending",
        BuildStatus::Building => "Building",
        BuildStatus::Success => "Success",
        BuildStatus::Failed => "Failed",
    }
}

fn decode_build_status(value: &str) -> rusqlite::Result<BuildStatus> {
    match normalize_db_enum(value) {
        "Pending" => Ok(BuildStatus::Pending),
        "Building" => Ok(BuildStatus::Building),
        "Success" => Ok(BuildStatus::Success),
        "Failed" => Ok(BuildStatus::Failed),
        other => Err(invalid_enum_error("BuildStatus", other)),
    }
}

fn encode_test_status(value: TestStatus) -> &'static str {
    match value {
        TestStatus::Queued => "Queued",
        TestStatus::Running => "Running",
        TestStatus::Completed => "Completed",
        TestStatus::Cancelled => "Cancelled",
        TestStatus::Failed => "Failed",
    }
}

fn decode_test_status(value: &str) -> rusqlite::Result<TestStatus> {
    match normalize_db_enum(value) {
        "Queued" => Ok(TestStatus::Queued),
        "Running" => Ok(TestStatus::Running),
        "Completed" => Ok(TestStatus::Completed),
        "Cancelled" => Ok(TestStatus::Cancelled),
        "Failed" => Ok(TestStatus::Failed),
        other => Err(invalid_enum_error("TestStatus", other)),
    }
}

fn encode_job_type(value: JobType) -> &'static str {
    match value {
        JobType::Sequential => "Sequential",
        JobType::Baseline => "Baseline",
        JobType::Bisect => "Bisect",
        JobType::Manual => "Manual",
    }
}

fn decode_job_type(value: &str) -> rusqlite::Result<JobType> {
    match normalize_db_enum(value) {
        "Sequential" => Ok(JobType::Sequential),
        "Baseline" => Ok(JobType::Baseline),
        "Bisect" => Ok(JobType::Bisect),
        "Manual" => Ok(JobType::Manual),
        other => Err(invalid_enum_error("JobType", other)),
    }
}

fn encode_sprt_result(value: SprtResult) -> &'static str {
    match value {
        SprtResult::Inconclusive => "Inconclusive",
        SprtResult::H1Accepted => "H1Accepted",
        SprtResult::H0Accepted => "H0Accepted",
    }
}

fn decode_sprt_result(value: &str) -> rusqlite::Result<SprtResult> {
    match normalize_db_enum(value) {
        "Inconclusive" => Ok(SprtResult::Inconclusive),
        "H1Accepted" => Ok(SprtResult::H1Accepted),
        "H0Accepted" => Ok(SprtResult::H0Accepted),
        other => Err(invalid_enum_error("SprtResult", other)),
    }
}

fn encode_game_result(value: GameResult) -> &'static str {
    match value {
        GameResult::WhiteWin => "WhiteWin",
        GameResult::BlackWin => "BlackWin",
        GameResult::Draw => "Draw",
    }
}

fn encode_bisect_status(value: BisectStatus) -> &'static str {
    match value {
        BisectStatus::Running => "Running",
        BisectStatus::Found => "Found",
        BisectStatus::Failed => "Failed",
    }
}

fn decode_bisect_status(value: &str) -> rusqlite::Result<BisectStatus> {
    match normalize_db_enum(value) {
        "Running" => Ok(BisectStatus::Running),
        "Found" => Ok(BisectStatus::Found),
        "Failed" => Ok(BisectStatus::Failed),
        other => Err(invalid_enum_error("BisectStatus", other)),
    }
}

fn encode_hunt_phase(value: HuntPhase) -> &'static str {
    match value {
        HuntPhase::Sampling => "Sampling",
        HuntPhase::Scanning => "Scanning",
        HuntPhase::Confirming => "Confirming",
        HuntPhase::Found => "Found",
        HuntPhase::Failed => "Failed",
    }
}

fn decode_hunt_phase(value: &str) -> rusqlite::Result<HuntPhase> {
    match normalize_db_enum(value) {
        "Sampling" => Ok(HuntPhase::Sampling),
        "Scanning" => Ok(HuntPhase::Scanning),
        "Confirming" => Ok(HuntPhase::Confirming),
        "Found" => Ok(HuntPhase::Found),
        "Failed" => Ok(HuntPhase::Failed),
        other => Err(invalid_enum_error("HuntPhase", other)),
    }
}

fn invalid_enum_error(kind: &'static str, value: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        format!("invalid {} value '{}'", kind, value).into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn test_engine() -> Engine {
        Engine {
            id: "engine-1".into(),
            name: "engine".into(),
            repo_url: "https://example.invalid/repo.git".into(),
            local_path: std::path::PathBuf::from("/tmp/engine"),
            branches: vec!["main".into()],
            experimental_branches: vec!["exp/*".into()],
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

    fn test_job(engine_id: &str, dev_revision_id: &str, base_revision_id: &str) -> TestJob {
        TestJob {
            id: "job-1".into(),
            engine_id: engine_id.into(),
            dev_revision_id: dev_revision_id.into(),
            base_revision_id: base_revision_id.into(),
            branch_context: Some("main".into()),
            time_control: TimeControl::stc(),
            opening_book: None,
            status: TestStatus::Completed,
            priority: 0,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            result: None,
            job_type: JobType::Sequential,
        }
    }

    #[test]
    fn cancels_running_job() -> Result<()> {
        let storage = Storage::in_memory()?;
        let engine = test_engine();
        let base = test_revision(&engine.id, "rev-base", "aaaa");
        let dev = test_revision(&engine.id, "rev-dev", "bbbb");
        let mut job = test_job(&engine.id, &dev.id, &base.id);
        job.status = TestStatus::Running;
        job.job_type = JobType::Manual;

        storage.insert_engine(&engine)?;
        storage.insert_revision(&base)?;
        storage.insert_revision(&dev)?;
        storage.insert_test_job(&job)?;

        assert!(storage.cancel_job(&job.id)?);
        assert_eq!(
            storage.get_job_status(&job.id)?,
            Some(TestStatus::Cancelled)
        );
        Ok(())
    }

    #[test]
    fn preserves_experimental_branches_on_engine_roundtrip() -> Result<()> {
        let storage = Storage::in_memory()?;
        let engine = test_engine();
        storage.insert_engine(&engine)?;

        let stored = storage
            .get_engine_by_id(&engine.id)?
            .expect("engine exists");
        assert_eq!(stored.experimental_branches, vec!["exp/*"]);
        Ok(())
    }

    #[test]
    fn reads_engine_rows_correctly_after_migrating_old_column_order() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        Storage::configure_connection(&conn)?;
        conn.execute_batch(
            "
            CREATE TABLE engines (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                repo_url TEXT NOT NULL,
                local_path TEXT NOT NULL,
                branches TEXT NOT NULL,
                build_cmd TEXT NOT NULL,
                binary_path TEXT NOT NULL,
                start_from TEXT
            );
            ",
        )?;

        let storage = Storage {
            conn: Arc::new(Mutex::new(conn)),
        };
        storage.migrate()?;

        {
            let conn = storage.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO engines (id, name, repo_url, local_path, branches, build_cmd, binary_path, start_from, experimental_branches)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    "engine-1",
                    "engine",
                    "https://example.invalid/repo.git",
                    "/tmp/engine",
                    "[\"main\"]",
                    "make",
                    "engine",
                    "v1.0.0",
                    "[\"exp/*\"]",
                ],
            )?;
        }

        let stored = storage.get_engines()?;
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].build_cmd, "make");
        assert_eq!(stored[0].binary_path, "engine");
        assert_eq!(stored[0].start_from.as_deref(), Some("v1.0.0"));
        assert_eq!(stored[0].experimental_branches, vec!["exp/*"]);
        Ok(())
    }

    #[test]
    fn cancelling_bisect_session_cancels_current_job() -> Result<()> {
        let storage = Storage::in_memory()?;
        let engine = test_engine();
        let base = test_revision(&engine.id, "rev-base", "aaaa");
        let dev = test_revision(&engine.id, "rev-dev", "bbbb");
        let mut job = test_job(&engine.id, &dev.id, &base.id);
        job.status = TestStatus::Running;
        job.job_type = JobType::Bisect;

        storage.insert_engine(&engine)?;
        storage.insert_revision(&base)?;
        storage.insert_revision(&dev)?;
        storage.insert_test_job(&job)?;
        storage.insert_bisect_session(&BisectSession {
            id: "bisect-1".into(),
            engine_id: engine.id.clone(),
            good_revision_id: base.id.clone(),
            bad_revision_id: dev.id.clone(),
            commit_range: vec![base.commit_hash.clone(), dev.commit_hash.clone()],
            current_index: Some(1),
            current_job_id: Some(job.id.clone()),
            phase: HuntPhase::Sampling,
            pending_indices: vec![],
            probe_history: Vec::new(),
            candidate_revision_id: None,
            candidate_index: None,
            status: BisectStatus::Running,
            culprit_revision_id: None,
        })?;

        assert!(storage.cancel_bisect_session("bisect-1")?);
        assert_eq!(
            storage.get_job_status(&job.id)?,
            Some(TestStatus::Cancelled)
        );
        let session = storage.get_bisect_session_by_id("bisect-1")?.unwrap();
        assert_eq!(session.status, BisectStatus::Failed);
        assert_eq!(session.phase, HuntPhase::Failed);
        assert!(session.current_job_id.is_none());
        Ok(())
    }

    #[test]
    fn deletes_engine_and_related_rows() -> Result<()> {
        let storage = Storage::in_memory()?;
        let engine = test_engine();
        let base = test_revision(&engine.id, "rev-base", "aaaa");
        let dev = test_revision(&engine.id, "rev-dev", "bbbb");
        let job = test_job(&engine.id, &dev.id, &base.id);
        let session = BisectSession {
            id: "bisect-1".into(),
            engine_id: engine.id.clone(),
            good_revision_id: base.id.clone(),
            bad_revision_id: dev.id.clone(),
            commit_range: vec![base.commit_hash.clone(), dev.commit_hash.clone()],
            current_index: None,
            current_job_id: None,
            phase: HuntPhase::Sampling,
            pending_indices: vec![1],
            probe_history: Vec::new(),
            candidate_revision_id: None,
            candidate_index: None,
            status: BisectStatus::Running,
            culprit_revision_id: None,
        };

        storage.insert_engine(&engine)?;
        storage.insert_revision(&base)?;
        storage.insert_revision(&dev)?;
        storage.insert_test_job(&job)?;
        storage.insert_game(
            &job.id,
            &GameRecord {
                game_number: 1,
                result: GameResult::Draw,
                pgn: "*".into(),
                opening: "startpos".into(),
                move_count: 1,
            },
        )?;
        storage.insert_bisect_session(&session)?;

        assert!(storage.delete_engine(&engine.id)?);
        assert!(storage.get_engine_by_name(&engine.name)?.is_none());
        assert!(storage.get_revision_by_id(&base.id)?.is_none());
        assert!(storage.get_revision_by_id(&dev.id)?.is_none());
        assert!(storage.get_job_status(&job.id)?.is_none());
        assert!(storage.get_bisect_session_by_id(&session.id)?.is_none());

        let conn = storage.conn.lock().unwrap();
        let games: u32 = conn.query_row("SELECT COUNT(*) FROM games", [], |row| row.get(0))?;
        let revisions: u32 =
            conn.query_row("SELECT COUNT(*) FROM revisions", [], |row| row.get(0))?;
        let engines: u32 = conn.query_row("SELECT COUNT(*) FROM engines", [], |row| row.get(0))?;

        assert_eq!(games, 0);
        assert_eq!(revisions, 0);
        assert_eq!(engines, 0);
        Ok(())
    }

    #[test]
    fn refuses_to_delete_engine_with_running_jobs() -> Result<()> {
        let storage = Storage::in_memory()?;
        let engine = test_engine();
        let base = test_revision(&engine.id, "rev-base", "aaaa");
        let dev = test_revision(&engine.id, "rev-dev", "bbbb");
        let mut job = test_job(&engine.id, &dev.id, &base.id);
        job.status = TestStatus::Running;

        storage.insert_engine(&engine)?;
        storage.insert_revision(&base)?;
        storage.insert_revision(&dev)?;
        storage.insert_test_job(&job)?;

        let err = storage.delete_engine(&engine.id).unwrap_err().to_string();
        assert!(err.contains("cannot delete engine while jobs are still running"));
        assert!(storage.get_engine_by_name(&engine.name)?.is_some());
        Ok(())
    }

    #[test]
    fn requeues_running_jobs_and_clears_partial_progress() -> Result<()> {
        let storage = Storage::in_memory()?;
        let engine = test_engine();
        let base = test_revision(&engine.id, "rev-base", "aaaa");
        let dev = test_revision(&engine.id, "rev-dev", "bbbb");
        let mut job = test_job(&engine.id, &dev.id, &base.id);
        job.status = TestStatus::Running;
        job.started_at = Some(Utc::now());

        storage.insert_engine(&engine)?;
        storage.insert_revision(&base)?;
        storage.insert_revision(&dev)?;
        storage.insert_test_job(&job)?;
        storage.update_job_result(&job.id, 3, 2, 1, 4.2, 1.0, 0.8, SprtResult::Inconclusive)?;
        storage.insert_game(
            &job.id,
            &GameRecord {
                game_number: 1,
                result: GameResult::Draw,
                pgn: "*".into(),
                opening: "startpos".into(),
                move_count: 1,
            },
        )?;

        assert_eq!(storage.requeue_running_jobs()?, 1);

        let conn = storage.conn.lock().unwrap();
        let (status, started_at, wins, losses, draws, sprt_result): (
            String,
            Option<String>,
            u32,
            u32,
            u32,
            String,
        ) = conn.query_row(
            "SELECT status, started_at, wins, losses, draws, sprt_result
             FROM test_jobs
             WHERE id = ?1",
            params![job.id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )?;
        let games: u32 = conn.query_row(
            "SELECT COUNT(*) FROM games WHERE test_job_id = ?1",
            params![job.id],
            |row| row.get(0),
        )?;
        drop(conn);

        assert_eq!(decode_test_status(&status)?, TestStatus::Queued);
        assert!(started_at.is_none());
        assert_eq!((wins, losses, draws), (0, 0, 0));
        assert_eq!(decode_sprt_result(&sprt_result)?, SprtResult::Inconclusive);
        assert_eq!(games, 0);
        Ok(())
    }

    #[test]
    fn resolves_revision_by_tag_prefix() -> Result<()> {
        let storage = Storage::in_memory()?;
        let engine = test_engine();
        let mut rev = test_revision(&engine.id, "rev-tagged", "abcd");
        rev.tag = Some("v1.2.0".into());

        storage.insert_engine(&engine)?;
        storage.insert_revision(&rev)?;

        let resolved = storage
            .get_revision_by_ref_prefix(&engine.id, "v1.2")?
            .expect("tagged revision should resolve");
        assert_eq!(resolved.id, rev.id);
        Ok(())
    }

    #[test]
    fn system_status_reports_recent_games_per_minute() -> Result<()> {
        let storage = Storage::in_memory()?;
        let engine = test_engine();
        let base = test_revision(&engine.id, "rev-base", "aaaa");
        let dev = test_revision(&engine.id, "rev-dev", "bbbb");
        let job = test_job(&engine.id, &dev.id, &base.id);

        storage.insert_engine(&engine)?;
        storage.insert_revision(&base)?;
        storage.insert_revision(&dev)?;
        storage.insert_test_job(&job)?;

        for game_number in 1..=3 {
            storage.insert_game(
                &job.id,
                &GameRecord {
                    game_number,
                    result: GameResult::Draw,
                    pgn: "*".into(),
                    opening: "startpos".into(),
                    move_count: 1,
                },
            )?;
        }

        let status = storage.get_system_status()?;
        assert!(status.games_per_minute > 0.0);
        Ok(())
    }

    #[test]
    fn lists_jobs_for_revision_when_revision_is_dev_or_base() -> Result<()> {
        let storage = Storage::in_memory()?;
        let engine = test_engine();
        let base = test_revision(&engine.id, "rev-base", "aaaa");
        let dev = test_revision(&engine.id, "rev-dev", "bbbb");
        let newer = test_revision(&engine.id, "rev-newer", "cccc");

        let mut job_one = test_job(&engine.id, &dev.id, &base.id);
        job_one.id = "job-1".into();
        job_one.created_at = Utc::now() - chrono::Duration::minutes(5);

        let mut job_two = test_job(&engine.id, &newer.id, &dev.id);
        job_two.id = "job-2".into();
        job_two.job_type = JobType::Manual;
        job_two.created_at = Utc::now();

        storage.insert_engine(&engine)?;
        storage.insert_revision(&base)?;
        storage.insert_revision(&dev)?;
        storage.insert_revision(&newer)?;
        storage.insert_test_job(&job_one)?;
        storage.insert_test_job(&job_two)?;

        let jobs = storage.list_jobs_for_revision(&dev.id, 10)?;
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].id, job_two.id);
        assert_eq!(jobs[1].id, job_one.id);
        Ok(())
    }
}

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A chess engine registered for testing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Engine {
    pub id: String,
    pub name: String,
    pub repo_url: String,
    pub local_path: PathBuf,
    /// The branch(es) to track
    pub branches: Vec<String>,
    /// Experimental branch patterns to test separately from the canonical history
    #[serde(default)]
    pub experimental_branches: Vec<String>,
    /// Build command (e.g., "make" or "cargo build --release")
    pub build_cmd: String,
    /// Path to the resulting binary, relative to repo root
    pub binary_path: String,
    /// Starting commit hash or tag to begin testing from
    pub start_from: Option<String>,
}

/// A specific commit/version of an engine that has been (or will be) built
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineRevision {
    pub id: String,
    pub engine_id: String,
    pub commit_hash: String,
    pub commit_message: String,
    pub commit_date: DateTime<Utc>,
    pub branch: String,
    pub tag: Option<String>,
    pub is_release: bool,
    pub binary_path: Option<PathBuf>,
    pub binary_fingerprint: Option<String>,
    pub build_status: BuildStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BuildStatus {
    Pending,
    Building,
    Success,
    Failed,
}

/// A test job: one revision vs another
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestJob {
    pub id: String,
    pub engine_id: String,
    pub dev_revision_id: String,
    pub base_revision_id: String,
    /// Branch lane that created this job, when it comes from branch-local scheduling
    #[serde(default)]
    pub branch_context: Option<String>,
    pub time_control: TimeControl,
    pub opening_book: Option<String>,
    pub status: TestStatus,
    pub priority: i32,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub result: Option<TestResult>,
    pub job_type: JobType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TestStatus {
    Queued,
    Running,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobType {
    /// Standard sequential test: this commit vs previous
    Sequential,
    /// Testing against a fixed baseline (e.g., a tagged release)
    Baseline,
    /// Part of a bisect search
    Bisect,
    /// User-requested manual test
    Manual,
}

/// Time control specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeControl {
    pub base_time_ms: u64,
    pub increment_ms: u64,
    /// Optional node limit (for reproducible testing)
    pub nodes: Option<u64>,
}

impl TimeControl {
    pub fn stc() -> Self {
        Self {
            base_time_ms: 10_000,
            increment_ms: 100,
            nodes: None,
        }
    }

    pub fn ltc() -> Self {
        Self {
            base_time_ms: 60_000,
            increment_ms: 600,
            nodes: None,
        }
    }
}

impl std::fmt::Display for TimeControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:.1}+{:.2}",
            self.base_time_ms as f64 / 1000.0,
            self.increment_ms as f64 / 1000.0
        )
    }
}

/// Result of a completed test
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    pub wins: u32,
    pub losses: u32,
    pub draws: u32,
    pub elo_diff: f64,
    pub elo_error: f64,
    pub los: f64,
    pub sprt_result: SprtResult,
    pub games: Vec<GameRecord>,
}

/// Flattened test job data for status surfaces
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobSummary {
    pub id: String,
    pub engine_id: String,
    pub engine_name: String,
    pub dev_revision_id: String,
    pub dev_commit_hash: String,
    pub base_revision_id: String,
    pub base_commit_hash: String,
    pub branch_context: Option<String>,
    pub status: TestStatus,
    pub priority: i32,
    pub job_type: JobType,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub wins: u32,
    pub losses: u32,
    pub draws: u32,
    pub elo_diff: Option<f64>,
    pub elo_error: Option<f64>,
    pub los: Option<f64>,
    pub sprt_result: Option<SprtResult>,
}

impl TestResult {
    pub fn total_games(&self) -> u32 {
        self.wins + self.losses + self.draws
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SprtResult {
    /// Still running, not yet conclusive
    Inconclusive,
    /// H1 accepted: the change is likely an improvement
    H1Accepted,
    /// H0 accepted: the change is likely not an improvement
    H0Accepted,
    /// The configured fixed game count completed; inspect Elo and its error bar
    FixedGames,
}

/// A single game record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameRecord {
    pub game_number: u32,
    pub result: GameResult,
    pub pgn: String,
    pub opening: String,
    pub move_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameResult {
    WhiteWin,
    BlackWin,
    Draw,
}

/// Elo data point for the timeline graph
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EloDataPoint {
    pub revision_id: String,
    pub commit_hash: String,
    pub commit_message: String,
    pub commit_date: DateTime<Utc>,
    pub branch: String,
    pub tag: Option<String>,
    pub is_release: bool,
    pub elo: f64,
    pub elo_error: f64,
    pub games_played: u32,
}

/// A bisect session tracking a regression search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BisectSession {
    pub id: String,
    pub engine_id: String,
    /// Fixed known-good baseline used for all probes
    pub good_revision_id: String,
    /// User-supplied known-bad endpoint
    pub bad_revision_id: String,
    /// Current candidate window, inclusive of good and bad boundaries
    pub commit_range: Vec<String>,
    /// Current index being tested within commit_range
    pub current_index: Option<usize>,
    /// Active bisect job id, if one is in flight
    pub current_job_id: Option<String>,
    /// Current search phase
    pub phase: HuntPhase,
    /// Remaining indices to probe in the current phase
    pub pending_indices: Vec<usize>,
    /// Recorded outcomes for prior probes
    pub probe_history: Vec<ProbeRecord>,
    /// Candidate under confirmation, if any
    pub candidate_revision_id: Option<String>,
    /// Candidate index under confirmation, if any
    pub candidate_index: Option<usize>,
    pub status: BisectStatus,
    /// The commit identified as causing the regression
    pub culprit_revision_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BisectStatus {
    Running,
    Found,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HuntPhase {
    Sampling,
    Scanning,
    Confirming,
    Found,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProbeVerdict {
    Good,
    Bad,
    Uncertain,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeRecord {
    pub commit_hash: String,
    pub revision_id: String,
    pub index: usize,
    pub verdict: ProbeVerdict,
    pub job_id: String,
}

/// Live status of the system
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemStatus {
    pub active_jobs: u32,
    pub queued_jobs: u32,
    pub completed_jobs: u32,
    pub engines_tracked: u32,
    pub total_games_played: u64,
    pub uptime_seconds: u64,
    pub games_per_minute: f64,
}

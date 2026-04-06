use anyhow::{anyhow, Result};
use chrono::Utc;
use cozy_chess::{util::parse_uci_move, Board, Color, GameStatus};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use uuid::Uuid;

use crate::engine::uci::{SearchScore, UciEngine};
use crate::types::{GameResult, TimeControl};

const UCI_CLOCK_GRACE_MS: u64 = 1_000;
const MAX_MOVES_PER_GAME: u32 = 500;

pub struct SelfPlayDataConfig {
    pub engine_id: String,
    pub engine_name: String,
    pub revision_id: String,
    pub revision_hash: String,
    pub binary_path: PathBuf,
    pub time_control: TimeControl,
    pub opening_book: Option<Vec<String>>,
    pub games: u32,
    pub hash_mb: u32,
    pub threads: u32,
    pub output_dir: PathBuf,
    pub depth: u32,
    pub kind: TrainingRunKind,
}

pub struct SelfPlayDataSummary {
    pub run_dir: PathBuf,
    pub games_played: u32,
    pub samples_written: usize,
    pub depth_counts: BTreeMap<u32, usize>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TrainingRunKind {
    SelfPlay,
    Regression,
    Idle,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TrainingRunStatus {
    Running,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TrainingDepthMode {
    Exact,
    Min,
}

impl Default for TrainingRunStatus {
    fn default() -> Self {
        Self::Completed
    }
}

impl std::fmt::Display for TrainingRunStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TrainingRunStatus::Running => write!(f, "running"),
            TrainingRunStatus::Completed => write!(f, "completed"),
            TrainingRunStatus::Cancelled => write!(f, "cancelled"),
            TrainingRunStatus::Failed => write!(f, "failed"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TrainingRunDescriptor {
    pub engine_id: String,
    pub engine_name: String,
    pub revision_id: String,
    pub revision_hash: String,
    pub time_control: String,
    pub games_requested: Option<u32>,
    pub kind: TrainingRunKind,
    pub source_job_id: Option<String>,
    pub source_role: Option<String>,
    pub depth_mode: TrainingDepthMode,
    pub depth_value: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrainingRunSummary {
    pub engine_id: String,
    pub engine_name: String,
    pub revision_id: String,
    pub revision_hash: String,
    pub games_requested: Option<u32>,
    pub games_played: u32,
    pub samples_written: usize,
    pub time_control: String,
    pub output_depth_counts: BTreeMap<u32, usize>,
    pub created_at: String,
    #[serde(default = "default_training_run_kind")]
    pub kind: TrainingRunKind,
    #[serde(default)]
    pub status: TrainingRunStatus,
    #[serde(default)]
    pub source_job_id: Option<String>,
    #[serde(default)]
    pub source_role: Option<String>,
    #[serde(default = "default_training_depth_mode")]
    pub depth_mode: TrainingDepthMode,
    #[serde(default = "default_training_depth_value_metadata", alias = "min_depth")]
    pub depth_value: u32,
    pub run_dir: PathBuf,
}

#[derive(Debug, Clone)]
struct PendingSample {
    game_number: u32,
    ply: u32,
    fen: String,
    side_to_move: Color,
    depth: u32,
    score: Option<SearchScore>,
    bestmove: String,
    opening: String,
}

#[derive(Debug, Clone)]
pub struct CollectedTrainingSample {
    pub game_number: u32,
    pub ply: u32,
    pub opening: String,
    pub fen: String,
    pub side_to_move: Color,
    pub depth: u32,
    pub score: Option<SearchScore>,
    pub bestmove: String,
    pub result: GameResult,
}

#[derive(Debug, Clone, Serialize)]
struct PersistedTrainingSample {
    engine_id: String,
    engine_name: String,
    revision_id: String,
    revision_hash: String,
    game_number: u32,
    ply: u32,
    opening: String,
    fen: String,
    side_to_move: String,
    depth: u32,
    score_cp: Option<i32>,
    score_mate: Option<i32>,
    bestmove: String,
    result: i8,
}

#[derive(Debug, Serialize, Deserialize)]
struct RunMetadata {
    engine_id: String,
    engine_name: String,
    revision_id: String,
    revision_hash: String,
    games_requested: Option<u32>,
    games_played: u32,
    samples_written: usize,
    time_control: String,
    output_depth_counts: BTreeMap<u32, usize>,
    created_at: String,
    #[serde(default = "default_training_run_kind")]
    kind: TrainingRunKind,
    #[serde(default)]
    status: TrainingRunStatus,
    #[serde(default)]
    source_job_id: Option<String>,
    #[serde(default)]
    source_role: Option<String>,
    #[serde(default = "default_training_depth_mode")]
    depth_mode: TrainingDepthMode,
    #[serde(default = "default_training_depth_value_metadata", alias = "min_depth")]
    depth_value: u32,
}

pub struct TrainingRunWriter {
    run_dir: PathBuf,
    metadata: RunMetadata,
    seen_games: BTreeSet<u32>,
}

fn default_training_run_kind() -> TrainingRunKind {
    TrainingRunKind::SelfPlay
}

fn default_training_depth_mode() -> TrainingDepthMode {
    TrainingDepthMode::Min
}

fn default_training_depth_value_metadata() -> u32 {
    1
}

impl TrainingRunWriter {
    pub fn begin(base: &Path, descriptor: TrainingRunDescriptor) -> Result<Self> {
        let run_dir = prepare_run_dir(
            base,
            &descriptor.engine_name,
            &descriptor.revision_hash,
            descriptor.kind,
            descriptor.source_job_id.as_deref(),
            descriptor.source_role.as_deref(),
        )?;
        let metadata = RunMetadata {
            engine_id: descriptor.engine_id,
            engine_name: descriptor.engine_name,
            revision_id: descriptor.revision_id,
            revision_hash: descriptor.revision_hash,
            games_requested: descriptor.games_requested,
            games_played: 0,
            samples_written: 0,
            time_control: descriptor.time_control,
            output_depth_counts: BTreeMap::new(),
            created_at: Utc::now().to_rfc3339(),
            kind: descriptor.kind,
            status: TrainingRunStatus::Running,
            source_job_id: descriptor.source_job_id,
            source_role: descriptor.source_role,
            depth_mode: descriptor.depth_mode,
            depth_value: descriptor.depth_value,
        };
        let writer = Self {
            run_dir,
            metadata,
            seen_games: BTreeSet::new(),
        };
        writer.write_metadata()?;
        Ok(writer)
    }

    pub fn run_dir(&self) -> &Path {
        &self.run_dir
    }

    pub fn games_played(&self) -> u32 {
        self.metadata.games_played
    }

    pub fn samples_written(&self) -> usize {
        self.metadata.samples_written
    }

    pub fn depth_counts(&self) -> &BTreeMap<u32, usize> {
        &self.metadata.output_depth_counts
    }

    pub fn record_game(&mut self, game_number: u32) -> Result<()> {
        if self.seen_games.insert(game_number) {
            self.metadata.games_played += 1;
            self.write_metadata()?;
        }
        Ok(())
    }

    pub fn append_samples(&mut self, samples: &[CollectedTrainingSample]) -> Result<()> {
        if samples.is_empty() {
            return Ok(());
        }

        let mut writers: HashMap<u32, BufWriter<std::fs::File>> = HashMap::new();

        for sample in samples {
            let keep = match self.metadata.depth_mode {
                TrainingDepthMode::Exact => sample.depth == self.metadata.depth_value,
                TrainingDepthMode::Min => sample.depth >= self.metadata.depth_value,
            };
            if !keep {
                continue;
            }
            let entry = self
                .metadata
                .output_depth_counts
                .entry(sample.depth)
                .or_insert(0);
            *entry += 1;
            self.metadata.samples_written += 1;

            if !writers.contains_key(&sample.depth) {
                let path = self
                    .run_dir
                    .join(format!("depth-{:03}.jsonl", sample.depth));
                let file = OpenOptions::new().create(true).append(true).open(path)?;
                writers.insert(sample.depth, BufWriter::new(file));
            }
            let writer = writers
                .get_mut(&sample.depth)
                .expect("writer inserted for depth bucket");

            let (score_cp, score_mate) = match sample.score {
                Some(SearchScore::Cp(value)) => (Some(value), None),
                Some(SearchScore::Mate(value)) => (None, Some(value)),
                None => (None, None),
            };

            let row = PersistedTrainingSample {
                engine_id: self.metadata.engine_id.clone(),
                engine_name: self.metadata.engine_name.clone(),
                revision_id: self.metadata.revision_id.clone(),
                revision_hash: self.metadata.revision_hash.clone(),
                game_number: sample.game_number,
                ply: sample.ply,
                opening: sample.opening.clone(),
                fen: sample.fen.clone(),
                side_to_move: match sample.side_to_move {
                    Color::White => "white".to_string(),
                    Color::Black => "black".to_string(),
                },
                depth: sample.depth,
                score_cp,
                score_mate,
                bestmove: sample.bestmove.clone(),
                result: match sample.result {
                    GameResult::WhiteWin => 1,
                    GameResult::Draw => 0,
                    GameResult::BlackWin => -1,
                },
            };
            serde_json::to_writer(&mut *writer, &row)?;
            writer.write_all(b"\n")?;
        }

        for writer in writers.values_mut() {
            writer.flush()?;
        }

        self.write_metadata()?;
        Ok(())
    }

    pub fn set_status(&mut self, status: TrainingRunStatus) -> Result<()> {
        self.metadata.status = status;
        self.write_metadata()
    }

    fn write_metadata(&self) -> Result<()> {
        fs::write(
            self.run_dir.join("metadata.json"),
            serde_json::to_vec_pretty(&self.metadata)?,
        )?;
        Ok(())
    }
}

pub fn list_training_runs(base: &Path) -> Result<Vec<TrainingRunSummary>> {
    let mut runs = Vec::new();
    if !base.exists() {
        return Ok(runs);
    }

    for engine_entry in fs::read_dir(base)? {
        let engine_entry = engine_entry?;
        if !engine_entry.file_type()?.is_dir() {
            continue;
        }

        for revision_entry in fs::read_dir(engine_entry.path())? {
            let revision_entry = revision_entry?;
            if !revision_entry.file_type()?.is_dir() {
                continue;
            }

            for run_entry in fs::read_dir(revision_entry.path())? {
                let run_entry = run_entry?;
                if !run_entry.file_type()?.is_dir() {
                    continue;
                }

                let metadata_path = run_entry.path().join("metadata.json");
                if !metadata_path.exists() {
                    continue;
                }

                let metadata = fs::read(&metadata_path)?;
                let run: RunMetadata = serde_json::from_slice(&metadata)?;
                runs.push(TrainingRunSummary {
                    engine_id: run.engine_id,
                    engine_name: run.engine_name,
                    revision_id: run.revision_id,
                    revision_hash: run.revision_hash,
                    games_requested: run.games_requested,
                    games_played: run.games_played,
                    samples_written: run.samples_written,
                    time_control: run.time_control,
                    output_depth_counts: run.output_depth_counts,
                    created_at: run.created_at,
                    kind: run.kind,
                    status: run.status,
                    source_job_id: run.source_job_id,
                    source_role: run.source_role,
                    depth_mode: run.depth_mode,
                    depth_value: run.depth_value,
                    run_dir: run_entry.path(),
                });
            }
        }
    }

    runs.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| right.run_dir.cmp(&left.run_dir))
    });
    Ok(runs)
}

pub fn run_selfplay_data_generation(config: SelfPlayDataConfig) -> Result<SelfPlayDataSummary> {
    let openings = config
        .opening_book
        .clone()
        .unwrap_or_else(|| vec!["startpos".to_string()]);
    let mut writer = TrainingRunWriter::begin(
        &config.output_dir,
        TrainingRunDescriptor {
            engine_id: config.engine_id.clone(),
            engine_name: config.engine_name.clone(),
            revision_id: config.revision_id.clone(),
            revision_hash: config.revision_hash.clone(),
            time_control: config.time_control.to_string(),
            games_requested: Some(config.games),
            kind: config.kind,
            source_job_id: None,
            source_role: None,
            depth_mode: TrainingDepthMode::Exact,
            depth_value: config.depth,
        },
    )?;

    let outcome = (|| -> Result<SelfPlayDataSummary> {
        for (game_index, opening) in openings.iter().cycle().enumerate() {
            if writer.games_played() >= config.games {
                break;
            }

            let samples = play_selfplay_game(
                &config.binary_path,
                opening,
                &config.time_control,
                config.hash_mb,
                config.threads,
                (game_index as u32) + 1,
            )?;
            writer.record_game((game_index as u32) + 1)?;
            writer.append_samples(&samples)?;
        }

        writer.set_status(TrainingRunStatus::Completed)?;

        Ok(SelfPlayDataSummary {
            run_dir: writer.run_dir().to_path_buf(),
            games_played: writer.games_played(),
            samples_written: writer.samples_written(),
            depth_counts: writer.depth_counts().clone(),
        })
    })();

    if outcome.is_err() {
        let _ = writer.set_status(TrainingRunStatus::Failed);
    }
    outcome
}

fn prepare_run_dir(
    base: &Path,
    engine_name: &str,
    revision_hash: &str,
    kind: TrainingRunKind,
    source_job_id: Option<&str>,
    source_role: Option<&str>,
) -> Result<PathBuf> {
    let prefix = match kind {
        TrainingRunKind::SelfPlay => "selfplay".to_string(),
        TrainingRunKind::Regression => {
            let job = source_job_id
                .map(|value| value.chars().take(8).collect::<String>())
                .unwrap_or_else(|| "job".to_string());
            let role = source_role.unwrap_or("unknown");
            format!("regression-{}-{}", job, role)
        }
        TrainingRunKind::Idle => "idle".to_string(),
    };
    let run_dir = base
        .join(sanitize_path_component(engine_name))
        .join(revision_hash)
        .join(format!(
            "{}-{}-{}",
            prefix,
            Utc::now().format("%Y%m%dT%H%M%S%.fZ"),
            Uuid::new_v4()
        ));
    fs::create_dir_all(&run_dir)?;
    Ok(run_dir)
}

fn play_selfplay_game(
    binary_path: &Path,
    opening: &str,
    tc: &TimeControl,
    hash_mb: u32,
    threads: u32,
    game_number: u32,
) -> Result<Vec<CollectedTrainingSample>> {
    let mut white = UciEngine::launch(binary_path, "white")?;
    let mut black = UciEngine::launch(binary_path, "black")?;

    for engine in [&mut white, &mut black] {
        engine.set_option("Hash", &hash_mb.to_string())?;
        engine.set_option("Threads", &threads.to_string())?;
        engine.ucinewgame()?;
    }

    let mut moves: Vec<String> = Vec::new();
    let mut board = parse_opening_board(opening)?;
    let mut seen_positions = HashMap::new();
    record_position(&mut seen_positions, &board);
    let position = if opening == "startpos" {
        "startpos".to_string()
    } else {
        format!("fen {}", opening)
    };
    let mut pending: Vec<PendingSample> = Vec::new();
    let mut move_count = 0;
    let mut wtime = tc.base_time_ms;
    let mut btime = tc.base_time_ms;

    let result = loop {
        if move_count >= MAX_MOVES_PER_GAME {
            break GameResult::Draw;
        }

        let side_to_move = board.side_to_move();
        let is_white_turn = side_to_move == Color::White;
        let current = if is_white_turn {
            &mut white
        } else {
            &mut black
        };
        let fen = board.to_string();
        let search = if let Some(nodes) = tc.nodes {
            current.go_nodes(&position, &moves, nodes, None)?
        } else {
            let remaining_before_move = if is_white_turn { wtime } else { btime };
            let turn_start = Instant::now();
            let search = current.go_position(
                &position,
                &moves,
                wtime,
                btime,
                tc.increment_ms,
                tc.increment_ms,
                per_move_timeout(remaining_before_move, tc.increment_ms),
                None,
            )?;
            let elapsed_ms = turn_start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
            if is_white_turn {
                wtime = wtime.saturating_sub(elapsed_ms);
            } else {
                btime = btime.saturating_sub(elapsed_ms);
            }
            if elapsed_ms >= remaining_before_move {
                break opponent_win(side_to_move);
            }
            search
        };

        let Some(bestmove) = search.bestmove else {
            break resolve_no_move_result(&board);
        };

        if bestmove == "(none)" || bestmove == "0000" {
            break resolve_no_move_result(&board);
        }
        if !UciEngine::is_valid_move(&bestmove) {
            break opponent_win(side_to_move);
        }

        let parsed_move = parse_uci_move(&board, &bestmove)
            .map_err(|_| anyhow!("invalid move '{}'", bestmove))?;
        if !board.is_legal(parsed_move) {
            break opponent_win(side_to_move);
        }

        if let Some(info) = search.info {
            pending.push(PendingSample {
                game_number,
                ply: move_count,
                fen,
                side_to_move,
                depth: info.depth,
                score: info.score,
                bestmove: bestmove.clone(),
                opening: opening.to_string(),
            });
        }

        moves.push(bestmove);
        move_count += 1;
        board.play(parsed_move);

        if tc.nodes.is_none() {
            if is_white_turn {
                wtime += tc.increment_ms;
            } else {
                btime += tc.increment_ms;
            }
        }

        if record_position(&mut seen_positions, &board) >= 3 {
            break GameResult::Draw;
        }

        match board.status() {
            GameStatus::Won => {
                break if is_white_turn {
                    GameResult::WhiteWin
                } else {
                    GameResult::BlackWin
                };
            }
            GameStatus::Drawn => break GameResult::Draw,
            GameStatus::Ongoing => {}
        }
    };

    white.quit()?;
    black.quit()?;

    Ok(pending
        .into_iter()
        .map(|sample| CollectedTrainingSample {
            game_number: sample.game_number,
            ply: sample.ply,
            opening: sample.opening,
            fen: sample.fen,
            side_to_move: sample.side_to_move,
            depth: sample.depth,
            score: sample.score,
            bestmove: sample.bestmove,
            result: perspective_result(result, sample.side_to_move),
        })
        .collect())
}

fn parse_opening_board(opening: &str) -> Result<Board> {
    if opening == "startpos" {
        Ok(Board::default())
    } else {
        opening
            .parse::<Board>()
            .map_err(|_| anyhow!("invalid opening FEN '{}'", opening))
    }
}

fn per_move_timeout(remaining_ms: u64, increment_ms: u64) -> Duration {
    Duration::from_millis(
        remaining_ms
            .saturating_add(increment_ms)
            .saturating_add(UCI_CLOCK_GRACE_MS),
    )
}

fn opponent_win(side_to_move: Color) -> GameResult {
    match side_to_move {
        Color::White => GameResult::BlackWin,
        Color::Black => GameResult::WhiteWin,
    }
}

fn resolve_no_move_result(board: &Board) -> GameResult {
    match board.status() {
        GameStatus::Drawn => GameResult::Draw,
        GameStatus::Won | GameStatus::Ongoing => opponent_win(board.side_to_move()),
    }
}

fn record_position(seen_positions: &mut HashMap<u64, u8>, board: &Board) -> u8 {
    let entry = seen_positions.entry(board.hash()).or_insert(0);
    *entry = entry.saturating_add(1);
    *entry
}

fn perspective_result(result: GameResult, side_to_move: Color) -> GameResult {
    match (result, side_to_move) {
        (GameResult::Draw, _) => GameResult::Draw,
        (GameResult::WhiteWin, Color::White) => GameResult::WhiteWin,
        (GameResult::WhiteWin, Color::Black) => GameResult::BlackWin,
        (GameResult::BlackWin, Color::White) => GameResult::BlackWin,
        (GameResult::BlackWin, Color::Black) => GameResult::WhiteWin,
    }
}

fn sanitize_path_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perspective_result_is_relative_to_side_to_move() {
        assert_eq!(
            perspective_result(GameResult::WhiteWin, Color::White),
            GameResult::WhiteWin
        );
        assert_eq!(
            perspective_result(GameResult::WhiteWin, Color::Black),
            GameResult::BlackWin
        );
        assert_eq!(
            perspective_result(GameResult::BlackWin, Color::White),
            GameResult::BlackWin
        );
        assert_eq!(
            perspective_result(GameResult::BlackWin, Color::Black),
            GameResult::WhiteWin
        );
    }

    #[test]
    fn sanitizes_engine_name_for_output_paths() {
        assert_eq!(sanitize_path_component("Stockfish Dev"), "Stockfish-Dev");
        assert_eq!(sanitize_path_component("lc0+cuda"), "lc0-cuda");
    }

    #[test]
    fn prepares_unique_run_dirs() -> Result<()> {
        let base = std::env::temp_dir().join(format!("crucible-training-test-{}", Uuid::new_v4()));
        let first = prepare_run_dir(
            &base,
            "Sykora",
            "abcd",
            TrainingRunKind::SelfPlay,
            None,
            None,
        )?;
        let second = prepare_run_dir(
            &base,
            "Sykora",
            "abcd",
            TrainingRunKind::SelfPlay,
            None,
            None,
        )?;

        assert_ne!(first, second);
        fs::remove_dir_all(base)?;
        Ok(())
    }

    #[test]
    fn lists_training_runs_from_metadata_files() -> Result<()> {
        let base = std::env::temp_dir().join(format!("crucible-training-list-{}", Uuid::new_v4()));
        let older_dir = base.join("Sykora").join("oldrev").join("old-run");
        let newer_dir = base.join("Sykora").join("newrev").join("new-run");
        fs::create_dir_all(&older_dir)?;
        fs::create_dir_all(&newer_dir)?;

        let older = RunMetadata {
            engine_id: "engine-1".into(),
            engine_name: "Sykora".into(),
            revision_id: "rev-old".into(),
            revision_hash: "oldrev".into(),
            games_requested: Some(10),
            games_played: 8,
            samples_written: 100,
            time_control: "10+0.1".into(),
            output_depth_counts: BTreeMap::from([(10, 60), (11, 40)]),
            created_at: "2026-04-01T00:00:00Z".into(),
            kind: TrainingRunKind::SelfPlay,
            status: TrainingRunStatus::Completed,
            source_job_id: None,
            source_role: None,
            depth_mode: TrainingDepthMode::Exact,
            depth_value: 10,
        };
        let newer = RunMetadata {
            engine_id: "engine-1".into(),
            engine_name: "Sykora".into(),
            revision_id: "rev-new".into(),
            revision_hash: "newrev".into(),
            games_requested: Some(12),
            games_played: 12,
            samples_written: 140,
            time_control: "10+0.1".into(),
            output_depth_counts: BTreeMap::from([(11, 50), (12, 90)]),
            created_at: "2026-04-02T00:00:00Z".into(),
            kind: TrainingRunKind::Regression,
            status: TrainingRunStatus::Running,
            source_job_id: Some("job-1".into()),
            source_role: Some("dev".into()),
            depth_mode: TrainingDepthMode::Min,
            depth_value: 12,
        };

        fs::write(
            older_dir.join("metadata.json"),
            serde_json::to_vec_pretty(&older)?,
        )?;
        fs::write(
            newer_dir.join("metadata.json"),
            serde_json::to_vec_pretty(&newer)?,
        )?;

        let runs = list_training_runs(&base)?;
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].revision_hash, "newrev");
        assert_eq!(runs[0].samples_written, 140);
        assert_eq!(runs[0].kind, TrainingRunKind::Regression);
        assert_eq!(runs[0].status, TrainingRunStatus::Running);
        assert_eq!(runs[1].revision_hash, "oldrev");
        assert_eq!(runs[1].output_depth_counts.get(&10), Some(&60));

        fs::remove_dir_all(base)?;
        Ok(())
    }

    #[test]
    fn no_move_result_uses_board_side_to_move() -> Result<()> {
        let board = parse_opening_board("7k/6Q1/6K1/8/8/8/8/8 b - - 0 1")?;
        assert_eq!(resolve_no_move_result(&board), GameResult::WhiteWin);
        Ok(())
    }

    #[test]
    fn writer_filters_using_depth_mode() -> Result<()> {
        let base =
            std::env::temp_dir().join(format!("crucible-training-min-depth-{}", Uuid::new_v4()));
        let mut writer = TrainingRunWriter::begin(
            &base,
            TrainingRunDescriptor {
                engine_id: "engine-1".into(),
                engine_name: "Sykora".into(),
                revision_id: "rev-1".into(),
                revision_hash: "abcd".into(),
                time_control: "10+0.1".into(),
                games_requested: Some(1),
                kind: TrainingRunKind::SelfPlay,
                source_job_id: None,
                source_role: None,
                depth_mode: TrainingDepthMode::Exact,
                depth_value: 10,
            },
        )?;
        writer.append_samples(&[
            CollectedTrainingSample {
                game_number: 1,
                ply: 1,
                opening: "startpos".into(),
                fen: Board::default().to_string(),
                side_to_move: Color::White,
                depth: 9,
                score: Some(SearchScore::Cp(12)),
                bestmove: "e2e4".into(),
                result: GameResult::WhiteWin,
            },
            CollectedTrainingSample {
                game_number: 1,
                ply: 2,
                opening: "startpos".into(),
                fen: Board::default().to_string(),
                side_to_move: Color::Black,
                depth: 10,
                score: Some(SearchScore::Cp(18)),
                bestmove: "e7e5".into(),
                result: GameResult::BlackWin,
            },
        ])?;

        assert_eq!(writer.samples_written(), 1);
        assert_eq!(writer.depth_counts().get(&9), None);
        assert_eq!(writer.depth_counts().get(&10), Some(&1));

        let mut min_writer = TrainingRunWriter::begin(
            &base,
            TrainingRunDescriptor {
                engine_id: "engine-1".into(),
                engine_name: "Sykora".into(),
                revision_id: "rev-2".into(),
                revision_hash: "efgh".into(),
                time_control: "10+0.1".into(),
                games_requested: Some(1),
                kind: TrainingRunKind::Regression,
                source_job_id: Some("job-1".into()),
                source_role: Some("dev".into()),
                depth_mode: TrainingDepthMode::Min,
                depth_value: 10,
            },
        )?;
        min_writer.append_samples(&[
            CollectedTrainingSample {
                game_number: 1,
                ply: 1,
                opening: "startpos".into(),
                fen: Board::default().to_string(),
                side_to_move: Color::White,
                depth: 9,
                score: Some(SearchScore::Cp(12)),
                bestmove: "e2e4".into(),
                result: GameResult::WhiteWin,
            },
            CollectedTrainingSample {
                game_number: 1,
                ply: 2,
                opening: "startpos".into(),
                fen: Board::default().to_string(),
                side_to_move: Color::Black,
                depth: 10,
                score: Some(SearchScore::Cp(18)),
                bestmove: "e7e5".into(),
                result: GameResult::BlackWin,
            },
            CollectedTrainingSample {
                game_number: 1,
                ply: 3,
                opening: "startpos".into(),
                fen: Board::default().to_string(),
                side_to_move: Color::White,
                depth: 12,
                score: Some(SearchScore::Cp(20)),
                bestmove: "g1f3".into(),
                result: GameResult::WhiteWin,
            },
        ])?;
        assert_eq!(min_writer.samples_written(), 2);
        assert_eq!(min_writer.depth_counts().get(&10), Some(&1));
        assert_eq!(min_writer.depth_counts().get(&12), Some(&1));

        fs::remove_dir_all(base)?;
        Ok(())
    }
}

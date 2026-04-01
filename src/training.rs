use anyhow::{anyhow, Result};
use chrono::Utc;
use cozy_chess::{util::parse_uci_move, Board, Color, GameStatus};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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
}

pub struct SelfPlayDataSummary {
    pub run_dir: PathBuf,
    pub games_played: u32,
    pub samples_written: usize,
    pub depth_counts: BTreeMap<u32, usize>,
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

#[derive(Debug, Clone, Serialize)]
struct TrainingSample {
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

#[derive(Debug, Serialize)]
struct RunMetadata {
    engine_id: String,
    engine_name: String,
    revision_id: String,
    revision_hash: String,
    games_requested: u32,
    games_played: u32,
    samples_written: usize,
    time_control: String,
    output_depth_counts: BTreeMap<u32, usize>,
    created_at: String,
}

pub fn run_selfplay_data_generation(config: SelfPlayDataConfig) -> Result<SelfPlayDataSummary> {
    let openings = config
        .opening_book
        .clone()
        .unwrap_or_else(|| vec!["startpos".to_string()]);
    let run_dir = prepare_run_dir(
        &config.output_dir,
        &config.engine_name,
        &config.revision_hash,
    )?;
    let mut depth_counts = BTreeMap::new();
    let mut games_played = 0;

    for (game_index, opening) in openings.iter().cycle().enumerate() {
        if games_played >= config.games {
            break;
        }

        let pending_samples = play_selfplay_game(
            &config.binary_path,
            opening,
            &config.time_control,
            config.hash_mb,
            config.threads,
            (game_index as u32) + 1,
        )?;
        persist_samples(&run_dir, &config, pending_samples, &mut depth_counts)?;
        games_played += 1;
    }

    let samples_written = depth_counts.values().sum();
    let metadata = RunMetadata {
        engine_id: config.engine_id.clone(),
        engine_name: config.engine_name.clone(),
        revision_id: config.revision_id.clone(),
        revision_hash: config.revision_hash.clone(),
        games_requested: config.games,
        games_played,
        samples_written,
        time_control: config.time_control.to_string(),
        output_depth_counts: depth_counts.clone(),
        created_at: Utc::now().to_rfc3339(),
    };
    fs::write(
        run_dir.join("metadata.json"),
        serde_json::to_vec_pretty(&metadata)?,
    )?;

    Ok(SelfPlayDataSummary {
        run_dir,
        games_played,
        samples_written,
        depth_counts,
    })
}

fn prepare_run_dir(base: &Path, engine_name: &str, revision_hash: &str) -> Result<PathBuf> {
    let run_dir = base
        .join(sanitize_path_component(engine_name))
        .join(revision_hash)
        .join(Utc::now().format("%Y%m%dT%H%M%SZ").to_string());
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
) -> Result<Vec<(PendingSample, GameResult)>> {
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

        let is_white_turn = move_count % 2 == 0;
        let current = if is_white_turn {
            &mut white
        } else {
            &mut black
        };
        let fen = board.to_string();
        let side_to_move = board.side_to_move();
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
                break opponent_win(is_white_turn);
            }
            search
        };

        let Some(bestmove) = search.bestmove else {
            break resolve_no_move_result(&board, is_white_turn);
        };

        if bestmove == "(none)" || bestmove == "0000" {
            break resolve_no_move_result(&board, is_white_turn);
        }
        if !UciEngine::is_valid_move(&bestmove) {
            break opponent_win(is_white_turn);
        }

        let parsed_move = parse_uci_move(&board, &bestmove)
            .map_err(|_| anyhow!("invalid move '{}'", bestmove))?;
        if !board.is_legal(parsed_move) {
            break opponent_win(is_white_turn);
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
        .map(|sample| {
            let perspective = perspective_result(result, sample.side_to_move);
            (sample, perspective)
        })
        .collect())
}

fn persist_samples(
    run_dir: &Path,
    config: &SelfPlayDataConfig,
    samples: Vec<(PendingSample, GameResult)>,
    depth_counts: &mut BTreeMap<u32, usize>,
) -> Result<()> {
    let mut writers: HashMap<u32, BufWriter<std::fs::File>> = HashMap::new();

    for (sample, result) in samples {
        let entry = depth_counts.entry(sample.depth).or_insert(0);
        *entry += 1;

        if !writers.contains_key(&sample.depth) {
            let path = run_dir.join(format!("depth-{:03}.jsonl", sample.depth));
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

        let row = TrainingSample {
            engine_id: config.engine_id.clone(),
            engine_name: config.engine_name.clone(),
            revision_id: config.revision_id.clone(),
            revision_hash: config.revision_hash.clone(),
            game_number: sample.game_number,
            ply: sample.ply,
            opening: sample.opening,
            fen: sample.fen,
            side_to_move: match sample.side_to_move {
                Color::White => "white".to_string(),
                Color::Black => "black".to_string(),
            },
            depth: sample.depth,
            score_cp,
            score_mate,
            bestmove: sample.bestmove,
            result: match result {
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

    Ok(())
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

fn opponent_win(is_white_turn: bool) -> GameResult {
    if is_white_turn {
        GameResult::BlackWin
    } else {
        GameResult::WhiteWin
    }
}

fn resolve_no_move_result(board: &Board, is_white_turn: bool) -> GameResult {
    match board.status() {
        GameStatus::Drawn => GameResult::Draw,
        GameStatus::Won | GameStatus::Ongoing => opponent_win(is_white_turn),
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
}

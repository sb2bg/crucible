//! Match runner: plays games between two engine revisions.
//!
//! Supports concurrent games, opening books, and real-time
//! SPRT evaluation to stop early when a result is conclusive.

use anyhow::Result;
use cozy_chess::{util::parse_uci_move, Color, GameStatus};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::{error, info};

use crate::chess_rules::{
    has_insufficient_material, opponent_win, parse_opening_board, per_move_timeout,
    perspective_result, record_position, resolve_no_move_result, MAX_MOVES_PER_GAME,
};
use crate::engine::uci::UciEngine;
use crate::sprt::{self, SprtBounds};
use crate::training::CollectedTrainingSample;
use crate::types::*;

/// Event emitted during a match for live updates
#[derive(Debug, Clone)]
pub enum MatchEvent {
    GameStarted {
        game_number: u32,
    },
    GameCompleted {
        game_number: u32,
        result: GameResult,
        record: GameRecord,
        training_samples: Vec<TaggedTrainingSample>,
    },
    SprtUpdate {
        wins: u32,
        draws: u32,
        losses: u32,
        llr_status: SprtResult,
    },
    MatchCompleted {
        result: TestResult,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrainingSampleSource {
    Dev,
    Base,
}

#[derive(Debug, Clone)]
pub struct TaggedTrainingSample {
    pub source: TrainingSampleSource,
    pub sample: CollectedTrainingSample,
}

/// Configuration for a match between two engine versions
pub struct MatchConfig {
    pub dev_binary: std::path::PathBuf,
    pub base_binary: std::path::PathBuf,
    pub time_control: TimeControl,
    pub opening_book: Option<Vec<String>>,
    pub sprt_bounds: SprtBounds,
    pub max_games: u32,
    pub hash_mb: u32,
    pub threads: u32,
    pub cancel_flag: Option<Arc<AtomicBool>>,
}

/// Run a full match between dev and base engines
pub async fn run_match(
    config: MatchConfig,
    event_tx: mpsc::UnboundedSender<MatchEvent>,
) -> Result<TestResult> {
    let mut wins: u32 = 0;
    let mut draws: u32 = 0;
    let mut losses: u32 = 0;
    let mut games: Vec<GameRecord> = Vec::new();

    let openings = config
        .opening_book
        .unwrap_or_else(|| vec!["startpos".to_string()]);

    let mut game_number: u32 = 0;

    for opening in openings.iter().cycle() {
        if is_cancelled(config.cancel_flag.as_deref()) {
            anyhow::bail!("match cancelled");
        }
        if game_number >= config.max_games {
            break;
        }

        // Play a pair of games (swap colors)
        for swap in [false, true] {
            game_number += 1;
            if game_number > config.max_games {
                break;
            }

            let _ = event_tx.send(MatchEvent::GameStarted { game_number });

            let result = play_single_game(
                &config.dev_binary,
                &config.base_binary,
                opening,
                &config.time_control,
                config.hash_mb,
                config.threads,
                game_number,
                swap,
                config.cancel_flag.clone(),
            )
            .await;

            match result {
                Ok((game_result, pgn, move_count, training_samples)) => {
                    match game_result {
                        GameResult::WhiteWin if !swap => wins += 1,
                        GameResult::BlackWin if !swap => losses += 1,
                        GameResult::WhiteWin if swap => losses += 1,
                        GameResult::BlackWin if swap => wins += 1,
                        GameResult::Draw => draws += 1,
                        _ => {}
                    }

                    let record = GameRecord {
                        game_number,
                        result: game_result,
                        pgn,
                        opening: opening.clone(),
                        move_count,
                    };
                    let event_record = record.clone();
                    games.push(record);

                    let _ = event_tx.send(MatchEvent::GameCompleted {
                        game_number,
                        result: game_result,
                        record: event_record,
                        training_samples,
                    });
                }
                Err(e) => {
                    if is_cancelled(config.cancel_flag.as_deref()) {
                        anyhow::bail!("match cancelled");
                    }
                    error!("Game {} failed: {}", game_number, e);
                    let _ = event_tx.send(MatchEvent::Error {
                        message: format!("Game {} failed: {}", game_number, e),
                    });
                    continue;
                }
            }

            // Check SPRT after each game
            let sprt_result = sprt::sprt_test(wins, draws, losses, &config.sprt_bounds);
            let _ = event_tx.send(MatchEvent::SprtUpdate {
                wins,
                draws,
                losses,
                llr_status: sprt_result,
            });

            if sprt_result != SprtResult::Inconclusive {
                info!(
                    "SPRT concluded after {} games: {:?} (W:{} D:{} L:{})",
                    game_number, sprt_result, wins, draws, losses
                );
                let result = build_test_result(wins, draws, losses, sprt_result, games);
                let _ = event_tx.send(MatchEvent::MatchCompleted {
                    result: result.clone(),
                });
                return Ok(result);
            }
        }
    }

    // Max games reached without SPRT conclusion
    let sprt_result = sprt::sprt_test(wins, draws, losses, &config.sprt_bounds);
    let result = build_test_result(wins, draws, losses, sprt_result, games);
    let _ = event_tx.send(MatchEvent::MatchCompleted {
        result: result.clone(),
    });
    Ok(result)
}

fn build_test_result(
    wins: u32,
    draws: u32,
    losses: u32,
    sprt_result: SprtResult,
    games: Vec<GameRecord>,
) -> TestResult {
    TestResult {
        wins,
        losses,
        draws,
        elo_diff: sprt::wdl_to_elo(wins, draws, losses),
        elo_error: sprt::elo_error(wins, draws, losses),
        los: sprt::los(wins, losses),
        sprt_result,
        games,
    }
}

/// Play a single game between two engines
async fn play_single_game(
    dev_binary: &Path,
    base_binary: &Path,
    opening: &str,
    tc: &TimeControl,
    hash_mb: u32,
    threads: u32,
    game_number: u32,
    swap_colors: bool,
    cancel_flag: Option<Arc<AtomicBool>>,
) -> Result<(GameResult, String, u32, Vec<TaggedTrainingSample>)> {
    // Run in a blocking thread since UCI I/O is synchronous
    let dev = dev_binary.to_path_buf();
    let base = base_binary.to_path_buf();
    let opening = opening.to_string();
    let tc = tc.clone();

    tokio::task::spawn_blocking(move || {
        play_game_blocking(
            &dev,
            &base,
            &opening,
            &tc,
            hash_mb,
            threads,
            game_number,
            swap_colors,
            cancel_flag,
        )
    })
    .await?
}

fn play_game_blocking(
    dev_binary: &Path,
    base_binary: &Path,
    opening: &str,
    tc: &TimeControl,
    hash_mb: u32,
    threads: u32,
    game_number: u32,
    swap_colors: bool,
    cancel_flag: Option<Arc<AtomicBool>>,
) -> Result<(GameResult, String, u32, Vec<TaggedTrainingSample>)> {
    let (white_bin, black_bin) = if swap_colors {
        (base_binary, dev_binary)
    } else {
        (dev_binary, base_binary)
    };

    let mut white = UciEngine::launch(white_bin, "white")?;
    let mut black = UciEngine::launch(black_bin, "black")?;

    // Configure engines
    for engine in [&mut white, &mut black] {
        engine.set_option("Hash", &hash_mb.to_string())?;
        engine.set_option("Threads", &threads.to_string())?;
        engine.ucinewgame()?;
    }

    let mut moves: Vec<String> = Vec::new();
    let mut pgn_moves = String::new();
    let mut wtime = tc.base_time_ms;
    let mut btime = tc.base_time_ms;
    let mut move_count: u32 = 0;
    let mut board = parse_opening_board(opening)?;
    let mut seen_positions = Vec::new();
    record_position(&mut seen_positions, &board);
    let mut pending_samples: Vec<PendingTaggedSample> = Vec::new();

    let position = if opening == "startpos" {
        "startpos".to_string()
    } else {
        format!("fen {}", opening)
    };

    loop {
        if is_cancelled(cancel_flag.as_deref()) {
            anyhow::bail!("match cancelled");
        }
        if move_count >= MAX_MOVES_PER_GAME {
            // Adjudicate as draw
            return Ok((
                GameResult::Draw,
                pgn_moves,
                move_count,
                finalize_training_samples(pending_samples, GameResult::Draw),
            ));
        }

        let side_to_move = board.side_to_move();
        let is_white_turn = side_to_move == Color::White;
        let current = if is_white_turn {
            &mut white
        } else {
            &mut black
        };

        let search = if let Some(nodes) = tc.nodes {
            match current.go_nodes(&position, &moves, nodes, cancel_flag.as_deref()) {
                Ok(outcome) => outcome,
                Err(err) => {
                    if is_cancelled(cancel_flag.as_deref()) {
                        return Err(err);
                    }
                    error!("engine search failed: {}", err);
                    let result = opponent_win(side_to_move);
                    return Ok((
                        result,
                        pgn_moves,
                        move_count,
                        finalize_training_samples(pending_samples, result),
                    ));
                }
            }
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
                cancel_flag.as_deref(),
            );
            let elapsed_ms = turn_start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
            if is_white_turn {
                wtime = wtime.saturating_sub(elapsed_ms);
            } else {
                btime = btime.saturating_sub(elapsed_ms);
            }
            if elapsed_ms >= remaining_before_move {
                let result = if is_white_turn {
                    GameResult::BlackWin
                } else {
                    GameResult::WhiteWin
                };
                return Ok((
                    result,
                    pgn_moves,
                    move_count,
                    finalize_training_samples(pending_samples, result),
                ));
            }
            match search {
                Ok(outcome) => outcome,
                Err(err) => {
                    if is_cancelled(cancel_flag.as_deref()) {
                        return Err(err);
                    }
                    error!("engine search failed: {}", err);
                    let result = opponent_win(side_to_move);
                    return Ok((
                        result,
                        pgn_moves,
                        move_count,
                        finalize_training_samples(pending_samples, result),
                    ));
                }
            }
        };

        let Some(mv) = search.bestmove else {
            let result = resolve_no_move_result(&board);
            return Ok((
                result,
                pgn_moves,
                move_count,
                finalize_training_samples(pending_samples, result),
            ));
        };

        if mv != "(none)" && mv != "0000" {
            if !UciEngine::is_valid_move(&mv) {
                error!("engine returned invalid move '{}'", mv);
                let result = opponent_win(side_to_move);
                return Ok((
                    result,
                    pgn_moves,
                    move_count,
                    finalize_training_samples(pending_samples, result),
                ));
            }
            let parsed_move = match parse_uci_move(&board, &mv) {
                Ok(parsed_move) => parsed_move,
                Err(_) => {
                    error!("engine returned unparsable move '{}'", mv);
                    let result = opponent_win(side_to_move);
                    return Ok((
                        result,
                        pgn_moves,
                        move_count,
                        finalize_training_samples(pending_samples, result),
                    ));
                }
            };
            if !board.is_legal(parsed_move) {
                error!("engine returned illegal move '{}'", mv);
                let result = opponent_win(side_to_move);
                return Ok((
                    result,
                    pgn_moves,
                    move_count,
                    finalize_training_samples(pending_samples, result),
                ));
            }
            if let Some(info) = search.info {
                pending_samples.push(PendingTaggedSample {
                    source: sample_source(is_white_turn, swap_colors),
                    game_number,
                    ply: move_count,
                    fen: board.to_string(),
                    side_to_move,
                    depth: info.depth,
                    score: info.score,
                    bestmove: mv.clone(),
                    opening: opening.to_string(),
                });
            }
            if !pgn_moves.is_empty() {
                pgn_moves.push(' ');
            }
            if side_to_move == Color::White {
                pgn_moves.push_str(&format!("{}. ", board.fullmove_number()));
            } else if moves.is_empty() {
                pgn_moves.push_str(&format!("{}... ", board.fullmove_number()));
            }
            pgn_moves.push_str(&mv);

            moves.push(mv);
            move_count += 1;
            board.play(parsed_move);

            // Apply increment after a completed move.
            if tc.nodes.is_none() {
                if is_white_turn {
                    wtime += tc.increment_ms;
                } else {
                    btime += tc.increment_ms;
                }
            }

            if record_position(&mut seen_positions, &board) >= 3 {
                return Ok((
                    GameResult::Draw,
                    pgn_moves,
                    move_count,
                    finalize_training_samples(pending_samples, GameResult::Draw),
                ));
            }

            if has_insufficient_material(&board) {
                return Ok((
                    GameResult::Draw,
                    pgn_moves,
                    move_count,
                    finalize_training_samples(pending_samples, GameResult::Draw),
                ));
            }

            match board.status() {
                GameStatus::Won => {
                    let result = if is_white_turn {
                        GameResult::WhiteWin
                    } else {
                        GameResult::BlackWin
                    };
                    return Ok((
                        result,
                        pgn_moves,
                        move_count,
                        finalize_training_samples(pending_samples, result),
                    ));
                }
                GameStatus::Drawn => {
                    return Ok((
                        GameResult::Draw,
                        pgn_moves,
                        move_count,
                        finalize_training_samples(pending_samples, GameResult::Draw),
                    ));
                }
                GameStatus::Ongoing => {}
            }
        } else {
            let result = resolve_no_move_result(&board);
            return Ok((
                result,
                pgn_moves,
                move_count,
                finalize_training_samples(pending_samples, result),
            ));
        }
    }
}

#[derive(Debug, Clone)]
struct PendingTaggedSample {
    source: TrainingSampleSource,
    game_number: u32,
    ply: u32,
    opening: String,
    fen: String,
    side_to_move: Color,
    depth: u32,
    score: Option<crate::engine::uci::SearchScore>,
    bestmove: String,
}

fn sample_source(is_white_turn: bool, swap_colors: bool) -> TrainingSampleSource {
    if swap_colors == is_white_turn {
        TrainingSampleSource::Base
    } else {
        TrainingSampleSource::Dev
    }
}

fn finalize_training_samples(
    samples: Vec<PendingTaggedSample>,
    result: GameResult,
) -> Vec<TaggedTrainingSample> {
    samples
        .into_iter()
        .map(|sample| TaggedTrainingSample {
            source: sample.source,
            sample: CollectedTrainingSample {
                game_number: sample.game_number,
                ply: sample.ply,
                opening: sample.opening,
                fen: sample.fen,
                side_to_move: sample.side_to_move,
                depth: sample.depth,
                score: sample.score,
                bestmove: sample.bestmove,
                result: perspective_result(result, sample.side_to_move),
            },
        })
        .collect()
}

fn is_cancelled(flag: Option<&AtomicBool>) -> bool {
    flag.is_some_and(|flag| flag.load(Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use cozy_chess::Board;

    #[test]
    fn none_result_on_stalemate_is_draw() -> Result<()> {
        let board = parse_opening_board("7k/5Q2/7K/8/8/8/8/8 b - - 0 1")?;
        assert_eq!(resolve_no_move_result(&board), GameResult::Draw);
        Ok(())
    }

    #[test]
    fn none_result_on_checkmate_is_loss_for_side_to_move() -> Result<()> {
        let board = parse_opening_board("7k/6Q1/6K1/8/8/8/8/8 b - - 0 1")?;
        assert_eq!(resolve_no_move_result(&board), GameResult::WhiteWin);
        Ok(())
    }

    #[test]
    fn repeated_positions_trigger_threefold_counter() {
        let board = Board::default();
        let mut seen_positions = Vec::new();
        assert_eq!(record_position(&mut seen_positions, &board), 1);
        assert_eq!(record_position(&mut seen_positions, &board), 2);
        assert_eq!(record_position(&mut seen_positions, &board), 3);
    }

    #[test]
    fn repetition_uses_fide_equivalent_positions() -> Result<()> {
        let board_a =
            parse_opening_board("rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq e3 0 1")?;
        let board_b =
            parse_opening_board("rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 4 3")?;
        assert_ne!(board_a.hash(), board_b.hash());
        assert!(board_a.same_position(&board_b));

        let mut seen_positions = Vec::new();
        assert_eq!(record_position(&mut seen_positions, &board_a), 1);
        assert_eq!(record_position(&mut seen_positions, &board_b), 2);
        Ok(())
    }

    #[test]
    fn insufficient_material_bare_kings_is_draw() -> Result<()> {
        let board = parse_opening_board("8/8/8/8/8/8/4k3/4K3 w - - 0 1")?;
        assert!(has_insufficient_material(&board));
        assert_eq!(resolve_no_move_result(&board), GameResult::Draw);
        Ok(())
    }

    #[test]
    fn insufficient_material_single_minor_is_draw() -> Result<()> {
        let board = parse_opening_board("8/8/8/8/8/8/4k3/3NK3 w - - 0 1")?;
        assert!(has_insufficient_material(&board));
        Ok(())
    }

    #[test]
    fn parses_standard_uci_castling_for_cozy_chess() -> Result<()> {
        let board = parse_opening_board("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1")?;
        let mv = parse_uci_move(&board, "e1g1").map_err(|_| anyhow!("failed to parse castle"))?;
        assert!(board.is_legal(mv));
        Ok(())
    }

    #[test]
    fn black_to_move_opening_uses_ellipsis_pgn_prefix() -> Result<()> {
        let board = parse_opening_board("7k/8/8/8/8/8/8/7K b - - 0 42")?;
        let mut pgn_moves = String::new();
        let moves: Vec<String> = Vec::new();

        if board.side_to_move() == Color::White {
            pgn_moves.push_str(&format!("{}. ", board.fullmove_number()));
        } else if moves.is_empty() {
            pgn_moves.push_str(&format!("{}... ", board.fullmove_number()));
        }

        assert_eq!(pgn_moves, "42... ");
        Ok(())
    }
}

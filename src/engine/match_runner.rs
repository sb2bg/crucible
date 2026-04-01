//! Match runner: plays games between two engine revisions.
//!
//! Supports concurrent games, opening books, and real-time
//! SPRT evaluation to stop early when a result is conclusive.

use anyhow::{anyhow, Result};
use cozy_chess::{util::parse_uci_move, Board, GameStatus};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{error, info};

use crate::engine::uci::UciEngine;
use crate::sprt::{self, SprtBounds};
use crate::types::*;

const UCI_CLOCK_GRACE_MS: u64 = 1_000;

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
                swap,
                config.cancel_flag.clone(),
            )
            .await;

            match result {
                Ok((game_result, pgn, move_count)) => {
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
    swap_colors: bool,
    cancel_flag: Option<Arc<AtomicBool>>,
) -> Result<(GameResult, String, u32)> {
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
    swap_colors: bool,
    cancel_flag: Option<Arc<AtomicBool>>,
) -> Result<(GameResult, String, u32)> {
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
    let max_moves = 500; // adjudication
    let mut board = parse_opening_board(opening)?;
    let mut seen_positions = HashMap::new();
    record_position(&mut seen_positions, &board);

    let position = if opening == "startpos" {
        "startpos".to_string()
    } else {
        format!("fen {}", opening)
    };

    loop {
        if is_cancelled(cancel_flag.as_deref()) {
            anyhow::bail!("match cancelled");
        }
        if move_count >= max_moves {
            // Adjudicate as draw
            return Ok((GameResult::Draw, pgn_moves, move_count));
        }

        let is_white_turn = move_count % 2 == 0;
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
                    return Ok((opponent_win(is_white_turn), pgn_moves, move_count));
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
                return Ok((result, pgn_moves, move_count));
            }
            match search {
                Ok(outcome) => outcome,
                Err(err) => {
                    if is_cancelled(cancel_flag.as_deref()) {
                        return Err(err);
                    }
                    error!("engine search failed: {}", err);
                    return Ok((opponent_win(is_white_turn), pgn_moves, move_count));
                }
            }
        };

        let Some(mv) = search.bestmove else {
            let result = if is_white_turn {
                GameResult::BlackWin
            } else {
                GameResult::WhiteWin
            };
            return Ok((result, pgn_moves, move_count));
        };

        if mv != "(none)" && mv != "0000" {
            if !UciEngine::is_valid_move(&mv) {
                error!("engine returned invalid move '{}'", mv);
                return Ok((opponent_win(is_white_turn), pgn_moves, move_count));
            }
            let parsed_move = match parse_uci_move(&board, &mv) {
                Ok(parsed_move) => parsed_move,
                Err(_) => {
                    error!("engine returned unparsable move '{}'", mv);
                    return Ok((opponent_win(is_white_turn), pgn_moves, move_count));
                }
            };
            if !board.is_legal(parsed_move) {
                error!("engine returned illegal move '{}'", mv);
                return Ok((opponent_win(is_white_turn), pgn_moves, move_count));
            }
            if !pgn_moves.is_empty() {
                pgn_moves.push(' ');
            }
            if move_count % 2 == 0 {
                pgn_moves.push_str(&format!("{}. ", move_count / 2 + 1));
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
                return Ok((GameResult::Draw, pgn_moves, move_count));
            }

            match board.status() {
                GameStatus::Won => {
                    let result = if is_white_turn {
                        GameResult::WhiteWin
                    } else {
                        GameResult::BlackWin
                    };
                    return Ok((result, pgn_moves, move_count));
                }
                GameStatus::Drawn => {
                    return Ok((GameResult::Draw, pgn_moves, move_count));
                }
                GameStatus::Ongoing => {}
            }
        } else {
            return Ok((
                resolve_no_move_result(&board, is_white_turn),
                pgn_moves,
                move_count,
            ));
        }
    }
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

fn is_cancelled(flag: Option<&AtomicBool>) -> bool {
    flag.is_some_and(|flag| flag.load(Ordering::Relaxed))
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
        GameStatus::Won | GameStatus::Ongoing => {
            if is_white_turn {
                GameResult::BlackWin
            } else {
                GameResult::WhiteWin
            }
        }
    }
}

fn record_position(seen_positions: &mut HashMap<u64, u8>, board: &Board) -> u8 {
    let entry = seen_positions.entry(board.hash()).or_insert(0);
    *entry = entry.saturating_add(1);
    *entry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_result_on_stalemate_is_draw() -> Result<()> {
        let board = parse_opening_board("7k/5Q2/7K/8/8/8/8/8 b - - 0 1")?;
        assert_eq!(resolve_no_move_result(&board, false), GameResult::Draw);
        Ok(())
    }

    #[test]
    fn none_result_on_checkmate_is_loss_for_side_to_move() -> Result<()> {
        let board = parse_opening_board("7k/6Q1/6K1/8/8/8/8/8 b - - 0 1")?;
        assert_eq!(resolve_no_move_result(&board, false), GameResult::WhiteWin);
        Ok(())
    }

    #[test]
    fn repeated_positions_trigger_threefold_counter() {
        let board = Board::default();
        let mut seen_positions = HashMap::new();
        assert_eq!(record_position(&mut seen_positions, &board), 1);
        assert_eq!(record_position(&mut seen_positions, &board), 2);
        assert_eq!(record_position(&mut seen_positions, &board), 3);
    }

    #[test]
    fn parses_standard_uci_castling_for_cozy_chess() -> Result<()> {
        let board = parse_opening_board("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1")?;
        let mv = parse_uci_move(&board, "e1g1").map_err(|_| anyhow!("failed to parse castle"))?;
        assert!(board.is_legal(mv));
        Ok(())
    }
}

//! Match runner: plays games between two engine revisions.
//!
//! Supports concurrent games, opening books, and real-time
//! SPRT evaluation to stop early when a result is conclusive.

use anyhow::Result;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tracing::{info, warn, error};

use crate::engine::uci::UciEngine;
use crate::sprt::{self, SprtBounds};
use crate::types::*;

/// Event emitted during a match for live updates
#[derive(Debug, Clone)]
pub enum MatchEvent {
    GameStarted { game_number: u32 },
    GameCompleted { game_number: u32, result: GameResult },
    SprtUpdate { wins: u32, draws: u32, losses: u32, llr_status: SprtResult },
    MatchCompleted { result: TestResult },
    Error { message: String },
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

    let openings = config.opening_book.unwrap_or_else(|| {
        vec!["startpos".to_string()]
    });

    let mut game_number: u32 = 0;

    for opening in openings.iter().cycle() {
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
                    games.push(record);

                    let _ = event_tx.send(MatchEvent::GameCompleted {
                        game_number,
                        result: game_result,
                    });
                }
                Err(e) => {
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
) -> Result<(GameResult, String, u32)> {
    // Run in a blocking thread since UCI I/O is synchronous
    let dev = dev_binary.to_path_buf();
    let base = base_binary.to_path_buf();
    let opening = opening.to_string();
    let tc = tc.clone();

    tokio::task::spawn_blocking(move || {
        play_game_blocking(&dev, &base, &opening, &tc, hash_mb, threads, swap_colors)
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

    let position = if opening == "startpos" {
        "startpos".to_string()
    } else {
        format!("fen {}", opening)
    };

    loop {
        if move_count >= max_moves {
            // Adjudicate as draw
            return Ok((GameResult::Draw, pgn_moves, move_count));
        }

        let is_white_turn = move_count % 2 == 0;
        let current = if is_white_turn { &mut white } else { &mut black };

        let bestmove_line = if let Some(nodes) = tc.nodes {
            current.go_nodes(&position, &moves, nodes)?
        } else {
            current.go_position(
                &position,
                &moves,
                wtime,
                btime,
                tc.increment_ms,
                tc.increment_ms,
            )?
        };

        let bestmove = UciEngine::parse_bestmove(&bestmove_line);

        match bestmove {
            Some(mv) if mv != "(none)" && mv != "0000" => {
                if !pgn_moves.is_empty() {
                    pgn_moves.push(' ');
                }
                if move_count % 2 == 0 {
                    pgn_moves.push_str(&format!("{}. ", move_count / 2 + 1));
                }
                pgn_moves.push_str(&mv);

                moves.push(mv);
                move_count += 1;

                // Add increment
                if is_white_turn {
                    wtime += tc.increment_ms;
                } else {
                    btime += tc.increment_ms;
                }
            }
            _ => {
                // Engine has no legal move or resigned
                let result = if is_white_turn {
                    GameResult::BlackWin
                } else {
                    GameResult::WhiteWin
                };
                return Ok((result, pgn_moves, move_count));
            }
        }
    }
}

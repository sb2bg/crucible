use anyhow::{anyhow, Result};
use cozy_chess::{BitBoard, Board, Color, GameStatus, Piece};
use std::time::Duration;

use crate::types::GameResult;

const UCI_CLOCK_GRACE_MS: u64 = 1_000;
pub const MAX_MOVES_PER_GAME: u32 = 500;

pub fn parse_opening_board(opening: &str) -> Result<Board> {
    if opening == "startpos" {
        Ok(Board::default())
    } else {
        opening
            .parse::<Board>()
            .map_err(|_| anyhow!("invalid opening FEN '{}'", opening))
    }
}

pub fn per_move_timeout(remaining_ms: u64, increment_ms: u64) -> Duration {
    Duration::from_millis(
        remaining_ms
            .saturating_add(increment_ms)
            .saturating_add(UCI_CLOCK_GRACE_MS),
    )
}

pub fn opponent_win(side_to_move: Color) -> GameResult {
    match side_to_move {
        Color::White => GameResult::BlackWin,
        Color::Black => GameResult::WhiteWin,
    }
}

pub fn resolve_no_move_result(board: &Board) -> GameResult {
    if has_insufficient_material(board) {
        return GameResult::Draw;
    }
    match board.status() {
        GameStatus::Drawn => GameResult::Draw,
        GameStatus::Won | GameStatus::Ongoing => opponent_win(board.side_to_move()),
    }
}

pub fn record_position(seen_positions: &mut Vec<Board>, board: &Board) -> u8 {
    let count = seen_positions
        .iter()
        .filter(|previous| previous.same_position(board))
        .count()
        .saturating_add(1);
    seen_positions.push(board.clone());
    u8::try_from(count).unwrap_or(u8::MAX)
}

pub fn has_insufficient_material(board: &Board) -> bool {
    if !(board.pieces(Piece::Pawn).is_empty()
        && board.pieces(Piece::Rook).is_empty()
        && board.pieces(Piece::Queen).is_empty())
    {
        return false;
    }

    let white_bishops = board.colored_pieces(Color::White, Piece::Bishop);
    let black_bishops = board.colored_pieces(Color::Black, Piece::Bishop);
    let white_knights = board.colored_pieces(Color::White, Piece::Knight);
    let black_knights = board.colored_pieces(Color::Black, Piece::Knight);

    let bishops = white_bishops.len() + black_bishops.len();
    let knights = white_knights.len() + black_knights.len();
    let total_minors = bishops + knights;

    if total_minors == 0 || total_minors == 1 {
        return true;
    }

    if bishops == 0 {
        return total_minors <= 2;
    }

    if knights == 0 {
        if bishops == 1 {
            return true;
        }
        return bishops_are_single_color(white_bishops | black_bishops);
    }

    bishops == 1 && knights == 1
}

fn bishops_are_single_color(bishops: BitBoard) -> bool {
    bishops.is_subset(BitBoard::LIGHT_SQUARES) || bishops.is_subset(BitBoard::DARK_SQUARES)
}

pub fn perspective_result(result: GameResult, side_to_move: Color) -> GameResult {
    match (result, side_to_move) {
        (GameResult::Draw, _) => GameResult::Draw,
        (GameResult::WhiteWin, Color::White) => GameResult::WhiteWin,
        (GameResult::WhiteWin, Color::Black) => GameResult::BlackWin,
        (GameResult::BlackWin, Color::White) => GameResult::BlackWin,
        (GameResult::BlackWin, Color::Black) => GameResult::WhiteWin,
    }
}

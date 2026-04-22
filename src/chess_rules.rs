use anyhow::{anyhow, Context, Result};
use cozy_chess::{BitBoard, Board, Color, GameStatus, Piece};
use shakmaty::{fen::Epd, CastlingMode, Chess};
use std::fs;
use std::time::Duration;

use crate::types::GameResult;

const UCI_CLOCK_GRACE_MS: u64 = 1_000;
pub const MAX_MOVES_PER_GAME: u32 = 500;
pub const STARTING_POSITION_EPD: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq -";
pub const STARTING_POSITION_FEN: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

pub fn parse_opening_board(opening: &str) -> Result<Board> {
    opening
        .parse::<Board>()
        .map_err(|_| anyhow!("invalid opening FEN '{}'", opening))
}

pub fn load_opening_book(path: Option<&str>) -> Result<Option<Vec<String>>> {
    let Some(path) = path else {
        return Ok(None);
    };

    let contents = fs::read_to_string(path)
        .with_context(|| format!("Failed to read opening book '{}'", path))?;
    let mut openings = Vec::new();

    for (line_idx, line) in contents.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let fen = normalize_opening_epd(line).with_context(|| {
            format!("Invalid EPD opening in '{}' at line {}", path, line_idx + 1)
        })?;
        openings.push(fen);
    }

    if openings.is_empty() {
        Ok(None)
    } else {
        Ok(Some(openings))
    }
}

pub fn normalize_opening_epd(epd_line: &str) -> Result<String> {
    let (position, operations) = split_epd_position(epd_line)?;
    let epd = Epd::from_ascii(position.as_bytes())
        .map_err(|err| anyhow!("invalid EPD position '{}': {}", position, err))?;
    let _: Chess = epd
        .clone()
        .into_position(CastlingMode::Standard)
        .map_err(|err| anyhow!("illegal EPD position '{}': {}", position, err))?;

    let clocks = parse_epd_operations(operations)?;
    let fen = format!(
        "{} {} {}",
        epd, clocks.halfmove_clock, clocks.fullmove_number
    );
    parse_opening_board(&fen)?;
    Ok(fen)
}

fn split_epd_position(epd_line: &str) -> Result<(String, &str)> {
    let mut fields = Vec::with_capacity(4);
    let mut search_start = 0;

    for field in epd_line.split_whitespace().take(4) {
        let offset = epd_line[search_start..]
            .find(field)
            .ok_or_else(|| anyhow!("invalid EPD '{}'", epd_line))?;
        let start = search_start + offset;
        let end = start + field.len();
        fields.push(field);
        search_start = end;
    }

    if fields.len() != 4 {
        anyhow::bail!("EPD openings must contain exactly four position fields");
    }

    Ok((fields.join(" "), epd_line[search_start..].trim()))
}

#[derive(Debug, Clone, Copy)]
struct EpdClocks {
    halfmove_clock: u8,
    fullmove_number: u16,
}

fn parse_epd_operations(operations: &str) -> Result<EpdClocks> {
    let mut clocks = EpdClocks {
        halfmove_clock: 0,
        fullmove_number: 1,
    };

    if operations.is_empty() {
        return Ok(clocks);
    }

    let mut clause = String::new();
    let mut in_string = false;
    let mut escaped = false;
    let mut saw_clause = false;

    for ch in operations.chars() {
        if in_string {
            clause.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => {
                in_string = true;
                clause.push(ch);
            }
            ';' => {
                parse_epd_operation(clause.trim(), &mut clocks)?;
                clause.clear();
                saw_clause = true;
            }
            _ => clause.push(ch),
        }
    }

    if in_string {
        anyhow::bail!("unterminated EPD string operand");
    }

    if !clause.trim().is_empty() || !saw_clause {
        anyhow::bail!("EPD operations must be terminated by semicolons");
    }

    Ok(clocks)
}

fn parse_epd_operation(operation: &str, clocks: &mut EpdClocks) -> Result<()> {
    if operation.is_empty() {
        anyhow::bail!("empty EPD operation");
    }

    let mut parts = operation.split_whitespace();
    let opcode = parts
        .next()
        .ok_or_else(|| anyhow!("missing EPD operation opcode"))?;

    match opcode {
        "hmvc" => {
            let value = parts
                .next()
                .ok_or_else(|| anyhow!("hmvc operation is missing a value"))?;
            if parts.next().is_some() {
                anyhow::bail!("hmvc operation has too many operands");
            }
            clocks.halfmove_clock = value
                .parse::<u8>()
                .with_context(|| format!("invalid hmvc value '{}'", value))?;
            if clocks.halfmove_clock > 100 {
                anyhow::bail!("hmvc value must be <= 100");
            }
        }
        "fmvn" => {
            let value = parts
                .next()
                .ok_or_else(|| anyhow!("fmvn operation is missing a value"))?;
            if parts.next().is_some() {
                anyhow::bail!("fmvn operation has too many operands");
            }
            clocks.fullmove_number = value
                .parse::<u16>()
                .with_context(|| format!("invalid fmvn value '{}'", value))?;
            if clocks.fullmove_number == 0 {
                anyhow::bail!("fmvn value must be greater than zero");
            }
        }
        _ => {}
    }

    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_epd_to_full_fen() -> Result<()> {
        assert_eq!(
            normalize_opening_epd(STARTING_POSITION_EPD)?,
            STARTING_POSITION_FEN
        );
        Ok(())
    }

    #[test]
    fn epd_clock_operations_set_fen_counters() -> Result<()> {
        let fen = normalize_opening_epd("7k/8/8/8/8/8/8/7K b - - hmvc 12; fmvn 42;")?;
        assert_eq!(fen, "7k/8/8/8/8/8/8/7K b - - 12 42");
        Ok(())
    }

    #[test]
    fn ignores_non_position_epd_operations() -> Result<()> {
        let fen = normalize_opening_epd(
            r#"rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - bm e4; id "start position";"#,
        )?;
        assert_eq!(fen, STARTING_POSITION_FEN);
        Ok(())
    }

    #[test]
    fn rejects_startpos_in_opening_book() {
        assert!(normalize_opening_epd("startpos").is_err());
    }

    #[test]
    fn rejects_full_fen_as_opening_book_line() {
        assert!(normalize_opening_epd(STARTING_POSITION_FEN).is_err());
    }
}

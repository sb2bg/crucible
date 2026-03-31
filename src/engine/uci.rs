//! UCI (Universal Chess Interface) protocol handler.
//!
//! Manages communication with UCI-compatible chess engines,
//! sending commands and parsing responses.

use anyhow::{Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};
use tracing::debug;

/// A running UCI engine process
pub struct UciEngine {
    name: String,
    process: Child,
    stdin: std::process::ChildStdin,
    stdout_rx: Receiver<String>,
}

impl UciEngine {
    /// Launch an engine from its binary path
    pub fn launch(binary_path: &Path, name: &str) -> Result<Self> {
        let mut process = Command::new(binary_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context(format!("Failed to launch engine: {:?}", binary_path))?;

        let stdin = process.stdin.take().context("No stdin")?;
        let stdout = process.stdout.take().context("No stdout")?;
        let (stdout_tx, stdout_rx) = mpsc::channel();
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        if stdout_tx.send(line).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        let mut engine = Self {
            name: name.to_string(),
            process,
            stdin,
            stdout_rx,
        };

        engine.send_cmd("uci")?;
        engine.wait_for("uciok", Duration::from_secs(10))?;

        Ok(engine)
    }

    /// Send a command to the engine
    pub fn send_cmd(&mut self, cmd: &str) -> Result<()> {
        debug!("[{}] >> {}", self.name, cmd);
        writeln!(self.stdin, "{}", cmd)?;
        self.stdin.flush()?;
        Ok(())
    }

    /// Wait until a line starting with the expected prefix appears
    pub fn wait_for(&mut self, prefix: &str, timeout: Duration) -> Result<String> {
        let start = Instant::now();
        loop {
            if start.elapsed() >= timeout {
                anyhow::bail!("Timeout waiting for '{}' from {}", prefix, self.name);
            }
            let remaining = timeout.saturating_sub(start.elapsed());
            let line = match self.stdout_rx.recv_timeout(remaining) {
                Ok(line) => line,
                Err(RecvTimeoutError::Timeout) => {
                    anyhow::bail!("Timeout waiting for '{}' from {}", prefix, self.name);
                }
                Err(RecvTimeoutError::Disconnected) => {
                    anyhow::bail!(
                        "Engine '{}' exited while waiting for '{}'",
                        self.name,
                        prefix
                    );
                }
            };
            debug!("[{}] << {}", self.name, line);
            if line.starts_with(prefix) {
                return Ok(line);
            }
        }
    }

    /// Set a UCI option
    pub fn set_option(&mut self, name: &str, value: &str) -> Result<()> {
        self.send_cmd(&format!("setoption name {} value {}", name, value))
    }

    /// Tell the engine to be ready
    pub fn isready(&mut self) -> Result<()> {
        self.send_cmd("isready")?;
        self.wait_for("readyok", Duration::from_secs(30))?;
        Ok(())
    }

    /// Start a new game
    pub fn ucinewgame(&mut self) -> Result<()> {
        self.send_cmd("ucinewgame")?;
        self.isready()
    }

    /// Set position and make the engine search
    pub fn go_position(
        &mut self,
        position: &str,
        moves: &[String],
        wtime: u64,
        btime: u64,
        winc: u64,
        binc: u64,
        timeout: Duration,
    ) -> Result<Option<String>> {
        let moves_str = if moves.is_empty() {
            String::new()
        } else {
            format!(" moves {}", moves.join(" "))
        };

        self.send_cmd(&format!("position {}{}", position, moves_str))?;
        self.send_cmd(&format!(
            "go wtime {} btime {} winc {} binc {}",
            wtime, btime, winc, binc
        ))?;

        self.wait_for_bestmove(timeout)
    }

    /// Search with a node limit
    pub fn go_nodes(
        &mut self,
        position: &str,
        moves: &[String],
        nodes: u64,
    ) -> Result<Option<String>> {
        let moves_str = if moves.is_empty() {
            String::new()
        } else {
            format!(" moves {}", moves.join(" "))
        };

        self.send_cmd(&format!("position {}{}", position, moves_str))?;
        self.send_cmd(&format!("go nodes {}", nodes))?;

        self.wait_for_bestmove(Duration::from_secs(300))
    }

    /// Parse "bestmove e2e4 ponder d7d5" -> "e2e4"
    pub fn parse_bestmove(line: &str) -> Option<String> {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 && parts[0] == "bestmove" {
            Some(parts[1].to_string())
        } else {
            None
        }
    }

    pub fn is_valid_move(mv: &str) -> bool {
        let bytes = mv.as_bytes();
        if !(bytes.len() == 4 || bytes.len() == 5) {
            return false;
        }

        let is_file = |b: u8| (b'a'..=b'h').contains(&b);
        let is_rank = |b: u8| (b'1'..=b'8').contains(&b);

        if !is_file(bytes[0]) || !is_rank(bytes[1]) || !is_file(bytes[2]) || !is_rank(bytes[3]) {
            return false;
        }

        if bytes.len() == 5 {
            matches!(bytes[4], b'q' | b'r' | b'b' | b'n')
        } else {
            true
        }
    }

    fn wait_for_bestmove(&mut self, timeout: Duration) -> Result<Option<String>> {
        let start = Instant::now();
        loop {
            if start.elapsed() >= timeout {
                return Ok(None);
            }
            let remaining = timeout.saturating_sub(start.elapsed());
            let line = match self.stdout_rx.recv_timeout(remaining) {
                Ok(line) => line,
                Err(RecvTimeoutError::Timeout) => return Ok(None),
                Err(RecvTimeoutError::Disconnected) => {
                    anyhow::bail!("Engine '{}' exited while waiting for 'bestmove'", self.name);
                }
            };
            debug!("[{}] << {}", self.name, line);
            if line.starts_with("bestmove") {
                return Ok(Some(line));
            }
        }
    }

    /// Quit the engine
    pub fn quit(&mut self) -> Result<()> {
        let _ = self.send_cmd("quit");
        let _ = self.process.wait();
        Ok(())
    }
}

impl Drop for UciEngine {
    fn drop(&mut self) {
        let _ = self.send_cmd("quit");
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::UciEngine;

    #[test]
    fn validates_basic_uci_moves() {
        assert!(UciEngine::is_valid_move("e2e4"));
        assert!(UciEngine::is_valid_move("a7a8q"));
        assert!(!UciEngine::is_valid_move("foo"));
        assert!(!UciEngine::is_valid_move("e9e4"));
        assert!(!UciEngine::is_valid_move("e2e4x"));
    }
}

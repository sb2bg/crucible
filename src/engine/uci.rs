//! UCI (Universal Chess Interface) protocol handler.
//!
//! Manages communication with UCI-compatible chess engines,
//! sending commands and parsing responses.

use anyhow::{Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use tracing::{debug, warn};

/// A running UCI engine process
pub struct UciEngine {
    name: String,
    process: Child,
    stdin: std::process::ChildStdin,
    reader: BufReader<std::process::ChildStdout>,
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
        let reader = BufReader::new(stdout);

        let mut engine = Self {
            name: name.to_string(),
            process,
            stdin,
            reader,
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

    /// Read one line from the engine
    pub fn read_line(&mut self) -> Result<String> {
        let mut line = String::new();
        self.reader.read_line(&mut line)?;
        let line = line.trim_end().to_string();
        debug!("[{}] << {}", self.name, line);
        Ok(line)
    }

    /// Wait until a line starting with the expected prefix appears
    pub fn wait_for(&mut self, prefix: &str, timeout: Duration) -> Result<String> {
        let start = Instant::now();
        loop {
            if start.elapsed() > timeout {
                anyhow::bail!("Timeout waiting for '{}' from {}", prefix, self.name);
            }
            let line = self.read_line()?;
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
    ) -> Result<String> {
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

        // Wait for bestmove
        self.wait_for("bestmove", Duration::from_secs(300))
    }

    /// Search with a node limit
    pub fn go_nodes(&mut self, position: &str, moves: &[String], nodes: u64) -> Result<String> {
        let moves_str = if moves.is_empty() {
            String::new()
        } else {
            format!(" moves {}", moves.join(" "))
        };

        self.send_cmd(&format!("position {}{}", position, moves_str))?;
        self.send_cmd(&format!("go nodes {}", nodes))?;

        self.wait_for("bestmove", Duration::from_secs(300))
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

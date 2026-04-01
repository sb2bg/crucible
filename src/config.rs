//! Configuration for Crucible.
//!
//! Config is loaded from `crucible.toml` in the working directory.
//! Engines can be added via CLI or by editing the config file directly.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,

    #[serde(default)]
    pub server: ServerConfig,

    #[serde(default)]
    pub testing: TestingConfig,

    #[serde(default)]
    pub training: TrainingConfig,

    #[serde(default)]
    pub engines: Vec<EngineConfig>,
}

fn default_data_dir() -> PathBuf {
    dirs_next()
}

fn dirs_next() -> PathBuf {
    PathBuf::from(".crucible")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_web_port")]
    pub web_port: u16,
    #[serde(default = "default_web_host")]
    pub web_host: String,
    /// Optional bearer token required for /api/admin/* routes
    pub admin_token: Option<String>,
}

fn default_web_port() -> u16 {
    8877
}
fn default_web_host() -> String {
    "127.0.0.1".into()
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            web_port: default_web_port(),
            web_host: default_web_host(),
            admin_token: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestingConfig {
    /// Number of test jobs to execute in parallel
    #[serde(default = "default_concurrency")]
    pub concurrency: u32,
    /// Default time control
    #[serde(default)]
    pub time_control: TimeControlConfig,
    /// SPRT bounds
    #[serde(default)]
    pub sprt: SprtConfig,
    /// Default opening book (EPD/PGN file path)
    pub opening_book: Option<String>,
    /// How many games between each pair before giving up if SPRT is inconclusive
    #[serde(default = "default_max_games")]
    pub max_games: u32,
    /// Hash size in MB for UCI engines
    #[serde(default = "default_hash_mb")]
    pub hash_mb: u32,
    /// Number of threads per engine
    #[serde(default = "default_engine_threads")]
    pub engine_threads: u32,
    /// How often the daemon polls repos and schedules new work
    #[serde(default = "default_poll_interval_seconds")]
    pub poll_interval_seconds: u64,
}

fn default_concurrency() -> u32 {
    1
}
fn default_max_games() -> u32 {
    10_000
}
fn default_hash_mb() -> u32 {
    16
}
fn default_engine_threads() -> u32 {
    1
}
fn default_poll_interval_seconds() -> u64 {
    60
}

impl Default for TestingConfig {
    fn default() -> Self {
        Self {
            concurrency: default_concurrency(),
            time_control: TimeControlConfig::default(),
            sprt: SprtConfig::default(),
            opening_book: None,
            max_games: default_max_games(),
            hash_mb: default_hash_mb(),
            engine_threads: default_engine_threads(),
            poll_interval_seconds: default_poll_interval_seconds(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrainingConfig {
    #[serde(default = "default_training_output_dir")]
    pub output_dir: PathBuf,
    #[serde(default = "default_selfplay_games")]
    pub selfplay_games: u32,
}

fn default_training_output_dir() -> PathBuf {
    PathBuf::from(".crucible/training")
}

fn default_selfplay_games() -> u32 {
    100
}

impl Default for TrainingConfig {
    fn default() -> Self {
        Self {
            output_dir: default_training_output_dir(),
            selfplay_games: default_selfplay_games(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeControlConfig {
    #[serde(default = "default_base_ms")]
    pub base_ms: u64,
    #[serde(default = "default_inc_ms")]
    pub increment_ms: u64,
    pub nodes: Option<u64>,
}

fn default_base_ms() -> u64 {
    10_000
}
fn default_inc_ms() -> u64 {
    100
}

impl Default for TimeControlConfig {
    fn default() -> Self {
        Self {
            base_ms: default_base_ms(),
            increment_ms: default_inc_ms(),
            nodes: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SprtConfig {
    #[serde(default = "default_elo0")]
    pub elo0: f64,
    #[serde(default = "default_elo1")]
    pub elo1: f64,
    #[serde(default = "default_alpha")]
    pub alpha: f64,
    #[serde(default = "default_beta")]
    pub beta: f64,
}

fn default_elo0() -> f64 {
    0.0
}
fn default_elo1() -> f64 {
    5.0
}
fn default_alpha() -> f64 {
    0.05
}
fn default_beta() -> f64 {
    0.05
}

impl Default for SprtConfig {
    fn default() -> Self {
        Self {
            elo0: default_elo0(),
            elo1: default_elo1(),
            alpha: default_alpha(),
            beta: default_beta(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineConfig {
    pub name: String,
    pub repo: String,
    #[serde(default = "default_branches")]
    pub branches: Vec<String>,
    pub build_cmd: String,
    pub binary_path: String,
    /// Start testing from this commit/tag (default: test everything)
    pub start_from: Option<String>,
}

fn default_branches() -> Vec<String> {
    vec!["main".into()]
}

impl Default for Config {
    fn default() -> Self {
        Self {
            data_dir: default_data_dir(),
            server: ServerConfig::default(),
            testing: TestingConfig::default(),
            training: TrainingConfig::default(),
            engines: Vec::new(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        if path.exists() {
            let contents = std::fs::read_to_string(path)?;
            let config: Config = toml::from_str(&contents)?;
            config.validate()?;
            Ok(config)
        } else {
            let config = Config::default();
            config.validate()?;
            Ok(config)
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let contents = toml::to_string_pretty(self)?;
        std::fs::write(path, contents)?;
        Ok(())
    }

    /// Generate an example config file
    pub fn example() -> String {
        let example = Config {
            data_dir: PathBuf::from(".crucible"),
            server: ServerConfig::default(),
            testing: TestingConfig {
                concurrency: 4,
                ..Default::default()
            },
            training: TrainingConfig::default(),
            engines: vec![EngineConfig {
                name: "my-engine".into(),
                repo: "https://github.com/user/chess-engine".into(),
                branches: vec!["main".into(), "dev".into()],
                build_cmd: "cargo build --release".into(),
                binary_path: "target/release/my-engine".into(),
                start_from: Some("v1.0.0".into()),
            }],
        };
        toml::to_string_pretty(&example).unwrap()
    }

    pub fn validate(&self) -> Result<()> {
        if self.testing.concurrency == 0 {
            anyhow::bail!("testing.concurrency must be at least 1");
        }
        if self.testing.max_games == 0 {
            anyhow::bail!("testing.max_games must be at least 1");
        }
        if self.testing.hash_mb == 0 {
            anyhow::bail!("testing.hash_mb must be at least 1");
        }
        if self.testing.engine_threads == 0 {
            anyhow::bail!("testing.engine_threads must be at least 1");
        }
        if self.testing.poll_interval_seconds == 0 {
            anyhow::bail!("testing.poll_interval_seconds must be at least 1");
        }
        if self.training.selfplay_games == 0 {
            anyhow::bail!("training.selfplay_games must be at least 1");
        }
        if self.testing.time_control.base_ms == 0 && self.testing.time_control.nodes.is_none() {
            anyhow::bail!("time control must specify positive base_ms or nodes");
        }
        if matches!(self.testing.time_control.nodes, Some(0)) {
            anyhow::bail!("testing.time_control.nodes must be greater than 0 when set");
        }
        if self.testing.sprt.elo0 >= self.testing.sprt.elo1 {
            anyhow::bail!("testing.sprt.elo0 must be less than elo1");
        }
        if !(0.0 < self.testing.sprt.alpha && self.testing.sprt.alpha < 1.0) {
            anyhow::bail!("testing.sprt.alpha must be between 0 and 1");
        }
        if !(0.0 < self.testing.sprt.beta && self.testing.sprt.beta < 1.0) {
            anyhow::bail!("testing.sprt.beta must be between 0 and 1");
        }
        if self
            .server
            .admin_token
            .as_deref()
            .is_some_and(|token| token.trim().is_empty())
        {
            anyhow::bail!("server.admin_token cannot be empty when set");
        }
        Ok(())
    }
}

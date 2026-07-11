//! Configuration for Crucible.
//!
//! Config is loaded from `crucible.toml` in the working directory.
//! Engines can be added via CLI or by editing the config file directly.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

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
    pub gate: GateConfig,

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
    /// Optional bearer token required for protected dashboard routes.
    /// Required when web_host is not loopback.
    pub admin_token: Option<String>,
}

fn default_web_port() -> u16 {
    8877
}
fn default_web_host() -> String {
    "127.0.0.1".into()
}

const PLACEHOLDER_ADMIN_TOKENS: &[&str] = &[
    "change-me",
    "changeme",
    "change_me",
    "replace-me",
    "replace_me",
    "replace-this-with-a-random-token",
    "change-this-to-a-secure-random-string",
    "paste-generated-token-here",
];

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
    /// Optional path to a line-based EPD opening suite
    pub opening_book: Option<String>,
    /// How many games between each pair before giving up if SPRT is inconclusive
    #[serde(default = "default_max_games")]
    pub max_games: u32,
    /// When set, canonical progression matches play exactly this many games
    /// instead of stopping on an SPRT decision.
    pub progression_games: Option<u32>,
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
            progression_games: None,
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
    #[serde(default = "default_collect_from_tests")]
    pub collect_from_tests: bool,
    #[serde(default = "default_training_min_depth", alias = "min_depth")]
    pub regression_min_depth: u32,
    #[serde(default = "default_training_selfplay_depth")]
    pub selfplay_depth: u32,
    #[serde(default)]
    pub idle_selfplay: bool,
    #[serde(default = "default_idle_batch_games")]
    pub idle_batch_games: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GateConfig {
    #[serde(default)]
    pub opponents: Vec<GateOpponentConfig>,
    #[serde(default)]
    pub profiles: Vec<GateProfileConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateOpponentConfig {
    pub name: String,
    pub binary_path: PathBuf,
    #[serde(default)]
    pub options: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateProfileConfig {
    pub name: String,
    pub opponents: Vec<String>,
    #[serde(default = "default_gate_games_per_opponent")]
    pub games_per_opponent: u32,
    #[serde(default)]
    pub time_control: Option<TimeControlConfig>,
    pub opening_book: Option<String>,
    #[serde(default)]
    pub min_score_delta: f64,
}

fn default_gate_games_per_opponent() -> u32 {
    100
}

fn default_training_output_dir() -> PathBuf {
    PathBuf::from(".crucible/training")
}

fn default_selfplay_games() -> u32 {
    100
}

fn default_collect_from_tests() -> bool {
    true
}

fn default_training_min_depth() -> u32 {
    10
}

fn default_training_selfplay_depth() -> u32 {
    10
}

fn default_idle_batch_games() -> u32 {
    1
}

impl Default for TrainingConfig {
    fn default() -> Self {
        Self {
            output_dir: default_training_output_dir(),
            selfplay_games: default_selfplay_games(),
            collect_from_tests: default_collect_from_tests(),
            regression_min_depth: default_training_min_depth(),
            selfplay_depth: default_training_selfplay_depth(),
            idle_selfplay: false,
            idle_batch_games: default_idle_batch_games(),
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
    #[serde(default = "default_sprt_min_games")]
    pub min_games: u32,
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
fn default_sprt_min_games() -> u32 {
    16
}

impl Default for SprtConfig {
    fn default() -> Self {
        Self {
            elo0: default_elo0(),
            elo1: default_elo1(),
            alpha: default_alpha(),
            beta: default_beta(),
            min_games: default_sprt_min_games(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineConfig {
    pub name: String,
    pub repo: String,
    #[serde(default = "default_branches")]
    pub branches: Vec<String>,
    #[serde(default)]
    pub experimental_branches: Vec<String>,
    pub build_cmd: String,
    pub binary_path: String,
    /// Start testing from this commit/tag (default: test everything)
    pub start_from: Option<String>,
}

fn default_branches() -> Vec<String> {
    vec!["main".into()]
}

pub fn validate_engine_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        anyhow::bail!("engine name cannot be empty");
    }
    if name != name.trim() {
        anyhow::bail!("engine name cannot have leading or trailing whitespace");
    }
    if name.contains('/') || name.contains('\\') {
        anyhow::bail!("engine name must be a single path component");
    }

    let path = Path::new(name);
    let mut components = path.components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(_)), None) => Ok(()),
        _ => anyhow::bail!("engine name must be a single path component"),
    }
}

pub fn validate_engine_binary_path(binary_path: &str) -> Result<()> {
    if binary_path.trim().is_empty() {
        anyhow::bail!("engine binary_path cannot be empty");
    }
    if binary_path != binary_path.trim() {
        anyhow::bail!("engine binary_path cannot have leading or trailing whitespace");
    }
    if binary_path.contains('\\') {
        anyhow::bail!("engine binary_path must use '/' separators");
    }

    let path = Path::new(binary_path);
    if path.is_absolute() {
        anyhow::bail!("engine binary_path must be relative to the repository root");
    }
    if !path
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        anyhow::bail!("engine binary_path cannot contain '.', '..', or root components");
    }

    Ok(())
}

pub fn engine_repo_path(data_dir: &Path, engine_name: &str) -> Result<PathBuf> {
    validate_engine_name(engine_name)?;
    Ok(data_dir.join("repos").join(engine_name))
}

pub fn validate_engine_storage_path(data_dir: &Path, local_path: &Path) -> Result<()> {
    let repos_root = data_dir
        .join("repos")
        .canonicalize()
        .map_err(|err| anyhow::anyhow!("could not resolve repo data directory: {}", err))?;
    let target = local_path
        .canonicalize()
        .map_err(|err| anyhow::anyhow!("could not resolve engine data directory: {}", err))?;

    if !target.starts_with(&repos_root) {
        anyhow::bail!(
            "refusing to delete engine data outside '{}'",
            repos_root.display()
        );
    }

    Ok(())
}

fn is_loopback_web_host(host: &str) -> bool {
    let host = host.trim();
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }

    let host = host
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(host);

    host.parse::<std::net::IpAddr>()
        .is_ok_and(|addr| addr.is_loopback())
}

fn token_is_placeholder(token: &str) -> bool {
    let normalized = token.trim().to_ascii_lowercase();
    PLACEHOLDER_ADMIN_TOKENS.contains(&normalized.as_str())
}

impl Default for Config {
    fn default() -> Self {
        Self {
            data_dir: default_data_dir(),
            server: ServerConfig::default(),
            testing: TestingConfig::default(),
            training: TrainingConfig::default(),
            gate: GateConfig::default(),
            engines: Vec::new(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        if path.exists() {
            let contents = std::fs::read_to_string(path)?;
            Self::parse(&contents)
        } else {
            let config = Config::default();
            config.validate()?;
            Ok(config)
        }
    }

    pub fn parse(contents: &str) -> Result<Self> {
        let config: Config = toml::from_str(contents)?;
        config.validate()?;
        Ok(config)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let contents = self.to_toml_string()?;
        std::fs::write(path, contents)?;
        Ok(())
    }

    pub fn to_toml_string(&self) -> Result<String> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// Generate an example config file
    pub fn example() -> String {
        let example = Config {
            data_dir: PathBuf::from(".crucible"),
            server: ServerConfig::default(),
            testing: TestingConfig {
                concurrency: 4,
                progression_games: Some(200),
                ..Default::default()
            },
            training: TrainingConfig::default(),
            gate: GateConfig {
                opponents: vec![
                    GateOpponentConfig {
                        name: "Stockfish".into(),
                        binary_path: PathBuf::from("/opt/engines/stockfish"),
                        options: BTreeMap::new(),
                    },
                    GateOpponentConfig {
                        name: "Ethereal".into(),
                        binary_path: PathBuf::from("/opt/engines/ethereal"),
                        options: BTreeMap::new(),
                    },
                ],
                profiles: vec![GateProfileConfig {
                    name: "release".into(),
                    opponents: vec!["Stockfish".into(), "Ethereal".into()],
                    games_per_opponent: default_gate_games_per_opponent(),
                    time_control: None,
                    opening_book: None,
                    min_score_delta: 0.0,
                }],
            },
            engines: vec![EngineConfig {
                name: "my-engine".into(),
                repo: "https://github.com/user/chess-engine".into(),
                branches: vec!["main".into(), "dev".into()],
                experimental_branches: vec!["exp/*".into()],
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
        if let Some(games) = self.testing.progression_games {
            if games < 2 {
                anyhow::bail!("testing.progression_games must be at least 2 when set");
            }
            if games % 2 != 0 {
                anyhow::bail!(
                    "testing.progression_games must be even so each opening uses both colors"
                );
            }
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
        if self.training.regression_min_depth == 0 {
            anyhow::bail!("training.regression_min_depth must be at least 1");
        }
        if self.training.selfplay_depth == 0 {
            anyhow::bail!("training.selfplay_depth must be at least 1");
        }
        if self.training.idle_batch_games == 0 {
            anyhow::bail!("training.idle_batch_games must be at least 1");
        }
        let opponent_names = self
            .gate
            .opponents
            .iter()
            .map(|opponent| opponent.name.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        for opponent in &self.gate.opponents {
            if opponent.name.trim().is_empty() {
                anyhow::bail!("gate opponent names cannot be empty");
            }
        }
        for profile in &self.gate.profiles {
            if profile.name.trim().is_empty() {
                anyhow::bail!("gate profile names cannot be empty");
            }
            if profile.opponents.is_empty() {
                anyhow::bail!(
                    "gate profile '{}' must reference at least one opponent",
                    profile.name
                );
            }
            if profile.games_per_opponent == 0 {
                anyhow::bail!(
                    "gate profile '{}' must set games_per_opponent to at least 1",
                    profile.name
                );
            }
            if profile.games_per_opponent % 2 != 0 {
                anyhow::bail!(
                    "gate profile '{}' games_per_opponent must be even so each opening uses both colors",
                    profile.name
                );
            }
            for opponent in &profile.opponents {
                if !opponent_names.contains(opponent.as_str()) {
                    anyhow::bail!(
                        "gate profile '{}' references unknown opponent '{}'",
                        profile.name,
                        opponent
                    );
                }
            }
            if let Some(tc) = &profile.time_control {
                if tc.base_ms == 0 && tc.nodes.is_none() {
                    anyhow::bail!(
                        "gate profile '{}' time control must specify positive base_ms or nodes",
                        profile.name
                    );
                }
                if matches!(tc.nodes, Some(0)) {
                    anyhow::bail!(
                        "gate profile '{}' time control nodes must be greater than 0 when set",
                        profile.name
                    );
                }
            }
        }
        if self.testing.time_control.base_ms == 0 && self.testing.time_control.nodes.is_none() {
            anyhow::bail!("time control must specify positive base_ms or nodes");
        }
        if matches!(self.testing.time_control.nodes, Some(0)) {
            anyhow::bail!("testing.time_control.nodes must be greater than 0 when set");
        }
        for engine in &self.engines {
            validate_engine_name(&engine.name)
                .map_err(|err| anyhow::anyhow!("invalid engine '{}': {}", engine.name, err))?;
            validate_engine_binary_path(&engine.binary_path).map_err(|err| {
                anyhow::anyhow!("invalid engine '{}' binary_path: {}", engine.name, err)
            })?;
            if engine.repo.trim().is_empty() {
                anyhow::bail!("engine '{}' repo cannot be empty", engine.name);
            }
            if engine.build_cmd.trim().is_empty() {
                anyhow::bail!("engine '{}' build_cmd cannot be empty", engine.name);
            }
            if engine.branches.is_empty() && engine.experimental_branches.is_empty() {
                anyhow::bail!(
                    "engine '{}' must define branches and/or experimental_branches",
                    engine.name
                );
            }
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
        if self.testing.sprt.min_games == 0 {
            anyhow::bail!("testing.sprt.min_games must be at least 1");
        }
        match self.server.admin_token.as_deref() {
            Some(token) if token.trim().is_empty() => {
                anyhow::bail!("server.admin_token cannot be empty when set");
            }
            Some(token) if token_is_placeholder(token) => {
                anyhow::bail!("server.admin_token must be changed from the placeholder value");
            }
            _ => {}
        }
        if !is_loopback_web_host(&self.server.web_host) && self.server.admin_token.is_none() {
            anyhow::bail!(
                "server.admin_token is required when server.web_host is not a loopback address"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Config, ServerConfig};

    #[test]
    fn rejects_placeholder_admin_token() {
        let mut config = Config::default();
        config.server.admin_token = Some("changeme".into());
        assert!(config.validate().is_err());
    }

    #[test]
    fn requires_admin_token_for_non_loopback_hosts() {
        let mut config = Config::default();
        config.server.web_host = "0.0.0.0".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn allows_tokenless_loopback_hosts() {
        let config = Config {
            server: ServerConfig {
                web_host: "127.0.0.1".into(),
                admin_token: None,
                ..ServerConfig::default()
            },
            ..Config::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn rejects_unsafe_engine_paths() {
        let contents = r#"
[[engines]]
name = "../outside"
repo = "https://example.invalid/repo.git"
branches = ["main"]
build_cmd = "make"
binary_path = "../engine"
"#;
        assert!(Config::parse(contents).is_err());
    }

    #[test]
    fn progression_games_must_preserve_color_pairs() {
        let mut config = Config::default();
        config.testing.progression_games = Some(101);
        assert!(config.validate().is_err());

        config.testing.progression_games = Some(100);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn gate_games_must_preserve_color_pairs() {
        let contents = r#"
[[gate.opponents]]
name = "Stockfish"
binary_path = "/opt/engines/stockfish"

[[gate.profiles]]
name = "release"
opponents = ["Stockfish"]
games_per_opponent = 3
"#;
        assert!(Config::parse(contents).is_err());
    }
}

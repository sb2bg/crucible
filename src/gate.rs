use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::chess_rules::load_opening_book;
use crate::config::{Config, GateProfileConfig};
use crate::engine::match_runner::{run_match, MatchConfig, MatchStopRule};
use crate::sprt::SprtBounds;
use crate::sprt::{elo_error, los, wdl_to_elo};
use crate::types::{Engine, EngineRevision, TestResult, TimeControl};
use crate::workflow::configured_time_control;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateMatchSummary {
    pub opponent_name: String,
    pub result: TestResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateSideSummary {
    pub revision_id: String,
    pub commit_hash: String,
    pub matches: Vec<GateMatchSummary>,
    pub wins: u32,
    pub draws: u32,
    pub losses: u32,
    pub score_pct: f64,
    pub elo_diff: f64,
    pub elo_error: f64,
    pub los: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateHeadToHeadSummary {
    pub result: TestResult,
    pub score_pct: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GateVerdict {
    Pass,
    Fail,
    Tie,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateRunSummary {
    pub generated_at: DateTime<Utc>,
    pub engine_name: String,
    pub profile_name: String,
    pub candidate: GateSideSummary,
    pub baseline: GateSideSummary,
    pub head_to_head: GateHeadToHeadSummary,
    pub score_delta_pct: f64,
    pub min_score_delta: f64,
    pub verdict: GateVerdict,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateRunRecord {
    #[serde(flatten)]
    pub summary: GateRunSummary,
    pub output_path: PathBuf,
    pub file_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateProfileSummary {
    pub name: String,
    pub opponents: Vec<String>,
    pub games_per_opponent: u32,
    pub min_score_delta: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateRunProgress {
    pub completed_matches: u32,
    pub total_matches: u32,
    pub message: String,
}

impl GateSideSummary {
    pub fn from_matches(
        revision_id: String,
        commit_hash: String,
        matches: Vec<GateMatchSummary>,
    ) -> Self {
        let wins = matches.iter().map(|entry| entry.result.wins).sum();
        let draws = matches.iter().map(|entry| entry.result.draws).sum();
        let losses = matches.iter().map(|entry| entry.result.losses).sum();
        let total = wins + draws + losses;
        let score_pct = if total == 0 {
            0.0
        } else {
            ((wins as f64) + 0.5 * (draws as f64)) / (total as f64) * 100.0
        };
        let elo_diff = wdl_to_elo(wins, draws, losses);
        let elo_error = elo_error(wins, draws, losses);
        let los = los(wins, losses);

        Self {
            revision_id,
            commit_hash,
            matches,
            wins,
            draws,
            losses,
            score_pct,
            elo_diff,
            elo_error,
            los,
        }
    }
}

impl GateRunSummary {
    pub fn new(
        engine_name: String,
        profile_name: String,
        candidate: GateSideSummary,
        baseline: GateSideSummary,
        head_to_head: TestResult,
        min_score_delta: f64,
    ) -> Self {
        let score_delta_pct = candidate.score_pct - baseline.score_pct;
        let verdict = if score_delta_pct > min_score_delta {
            GateVerdict::Pass
        } else if score_delta_pct < -min_score_delta {
            GateVerdict::Fail
        } else {
            GateVerdict::Tie
        };

        Self {
            generated_at: Utc::now(),
            engine_name,
            profile_name,
            candidate,
            baseline,
            head_to_head: GateHeadToHeadSummary::from_result(head_to_head),
            score_delta_pct,
            min_score_delta,
            verdict,
        }
    }
}

impl GateHeadToHeadSummary {
    pub fn from_result(result: TestResult) -> Self {
        let total = result.wins + result.draws + result.losses;
        let score_pct = if total == 0 {
            0.0
        } else {
            ((result.wins as f64) + 0.5 * (result.draws as f64)) / (total as f64) * 100.0
        };

        Self { result, score_pct }
    }
}

pub fn resolve_gate_profile<'a>(
    config: &'a Config,
    profile_name: &str,
) -> Result<&'a GateProfileConfig> {
    config
        .gate
        .profiles
        .iter()
        .find(|profile| profile.name == profile_name)
        .with_context(|| format!("Unknown gate profile '{}'", profile_name))
}

pub fn default_gate_output_path(data_dir: &Path, profile: &str) -> PathBuf {
    data_dir.join("gates").join(format!(
        "{}-{}.json",
        chrono::Utc::now().format("%Y%m%dT%H%M%SZ"),
        profile
    ))
}

pub fn list_gate_runs(data_dir: &Path) -> Result<Vec<GateRunRecord>> {
    let gate_dir = data_dir.join("gates");
    if !gate_dir.exists() {
        return Ok(Vec::new());
    }

    let mut runs = Vec::new();
    for entry in fs::read_dir(&gate_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }

        let summary: GateRunSummary = serde_json::from_slice(&fs::read(&path)?)?;
        let file_name = entry.file_name().to_string_lossy().into_owned();
        runs.push(GateRunRecord {
            summary,
            output_path: path,
            file_name,
        });
    }

    runs.sort_by(|left, right| {
        right
            .summary
            .generated_at
            .cmp(&left.summary.generated_at)
            .then_with(|| right.file_name.cmp(&left.file_name))
    });
    Ok(runs)
}

pub fn write_gate_summary(output: &Path, summary: &GateRunSummary) -> Result<()> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(output, serde_json::to_vec_pretty(summary)?)?;
    Ok(())
}

pub fn gate_profile_summaries(config: &Config) -> Vec<GateProfileSummary> {
    config
        .gate
        .profiles
        .iter()
        .map(|profile| GateProfileSummary {
            name: profile.name.clone(),
            opponents: profile.opponents.clone(),
            games_per_opponent: profile.games_per_opponent,
            min_score_delta: profile.min_score_delta,
        })
        .collect()
}

pub async fn run_release_gate(
    config: &Config,
    engine: &Engine,
    candidate_revision: &EngineRevision,
    baseline_revision: &EngineRevision,
    profile: &GateProfileConfig,
) -> Result<GateRunSummary> {
    run_release_gate_with_progress(
        config,
        engine,
        candidate_revision,
        baseline_revision,
        profile,
        None,
        |_| {},
    )
    .await
}

pub async fn run_release_gate_with_progress<F>(
    config: &Config,
    engine: &Engine,
    candidate_revision: &EngineRevision,
    baseline_revision: &EngineRevision,
    profile: &GateProfileConfig,
    cancel_flag: Option<Arc<AtomicBool>>,
    mut on_progress: F,
) -> Result<GateRunSummary>
where
    F: FnMut(GateRunProgress) + Send,
{
    let candidate_binary = candidate_revision.binary_path.clone().with_context(|| {
        format!(
            "Candidate revision '{}' is missing a built binary",
            candidate_revision.commit_hash
        )
    })?;
    let baseline_binary = baseline_revision.binary_path.clone().with_context(|| {
        format!(
            "Baseline revision '{}' is missing a built binary",
            baseline_revision.commit_hash
        )
    })?;
    let time_control = profile
        .time_control
        .as_ref()
        .map(|tc| TimeControl {
            base_time_ms: tc.base_ms,
            increment_ms: tc.increment_ms,
            nodes: tc.nodes,
        })
        .unwrap_or_else(|| configured_time_control(config));
    let opening_book = load_opening_book(
        profile
            .opening_book
            .as_deref()
            .or(config.testing.opening_book.as_deref()),
    )?;
    let fixed_length_bounds = SprtBounds {
        elo0: config.testing.sprt.elo0,
        elo1: config.testing.sprt.elo1,
        alpha: config.testing.sprt.alpha,
        beta: config.testing.sprt.beta,
        min_games: profile.games_per_opponent.saturating_add(1),
    };
    let (candidate_matches, baseline_matches, head_to_head) = run_gate_tasks(
        config,
        &candidate_binary,
        &baseline_binary,
        &profile.opponents,
        &opening_book,
        &time_control,
        fixed_length_bounds,
        profile.games_per_opponent,
        cancel_flag,
        &mut on_progress,
    )
    .await?;

    Ok(GateRunSummary::new(
        engine.name.clone(),
        profile.name.clone(),
        GateSideSummary::from_matches(
            candidate_revision.id.clone(),
            candidate_revision.commit_hash.clone(),
            candidate_matches,
        ),
        GateSideSummary::from_matches(
            baseline_revision.id.clone(),
            baseline_revision.commit_hash.clone(),
            baseline_matches,
        ),
        head_to_head,
        profile.min_score_delta,
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GateTaskKind {
    CandidateVsOpponent(usize),
    BaselineVsOpponent(usize),
    HeadToHead,
}

#[allow(clippy::too_many_arguments)]
async fn run_gate_tasks(
    config: &Config,
    candidate_binary: &Path,
    baseline_binary: &Path,
    opponent_names: &[String],
    opening_book: &Option<Vec<String>>,
    time_control: &TimeControl,
    sprt_bounds: SprtBounds,
    games_per_opponent: u32,
    cancel_flag: Option<Arc<AtomicBool>>,
    on_progress: &mut impl FnMut(GateRunProgress),
) -> Result<(Vec<GateMatchSummary>, Vec<GateMatchSummary>, TestResult)> {
    let worker_count = usize::try_from(config.testing.concurrency.max(1)).unwrap_or(1);
    let mut tasks = Vec::new();
    for (index, opponent_name) in opponent_names.iter().enumerate() {
        let opponent = config
            .gate
            .opponents
            .iter()
            .find(|opponent| opponent.name == *opponent_name)
            .with_context(|| format!("Unknown gate opponent '{}'", opponent_name))?;
        tasks.push((
            GateTaskKind::CandidateVsOpponent(index),
            MatchConfig {
                dev_binary: candidate_binary.to_path_buf(),
                base_binary: opponent.binary_path.clone(),
                dev_options: Vec::new(),
                base_options: opponent
                    .options
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect(),
                time_control: time_control.clone(),
                opening_book: opening_book.clone(),
                sprt_bounds,
                stop_rule: MatchStopRule::FixedGames,
                max_games: games_per_opponent,
                hash_mb: config.testing.hash_mb,
                threads: config.testing.engine_threads,
                cancel_flag: cancel_flag.clone(),
            },
        ));
        tasks.push((
            GateTaskKind::BaselineVsOpponent(index),
            MatchConfig {
                dev_binary: baseline_binary.to_path_buf(),
                base_binary: opponent.binary_path.clone(),
                dev_options: Vec::new(),
                base_options: opponent
                    .options
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect(),
                time_control: time_control.clone(),
                opening_book: opening_book.clone(),
                sprt_bounds,
                stop_rule: MatchStopRule::FixedGames,
                max_games: games_per_opponent,
                hash_mb: config.testing.hash_mb,
                threads: config.testing.engine_threads,
                cancel_flag: cancel_flag.clone(),
            },
        ));
    }

    tasks.push((
        GateTaskKind::HeadToHead,
        MatchConfig {
            dev_binary: candidate_binary.to_path_buf(),
            base_binary: baseline_binary.to_path_buf(),
            dev_options: Vec::new(),
            base_options: Vec::new(),
            time_control: time_control.clone(),
            opening_book: opening_book.clone(),
            sprt_bounds,
            stop_rule: MatchStopRule::FixedGames,
            max_games: games_per_opponent,
            hash_mb: config.testing.hash_mb,
            threads: config.testing.engine_threads,
            cancel_flag: cancel_flag.clone(),
        },
    ));

    let mut candidate_results = vec![None; opponent_names.len()];
    let mut baseline_results = vec![None; opponent_names.len()];
    let mut head_to_head = None;
    let mut pending = tasks.into_iter();
    let mut workers = tokio::task::JoinSet::new();
    let total_matches = (opponent_names.len() as u32) * 2 + 1;
    let mut completed_matches = 0u32;
    on_progress(GateRunProgress {
        completed_matches,
        total_matches,
        message: format!("0 / {} matches complete", total_matches),
    });

    loop {
        if cancel_flag
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
        {
            anyhow::bail!("gate cancelled");
        }
        while workers.len() < worker_count {
            let Some((kind, match_config)) = pending.next() else {
                break;
            };
            workers.spawn(async move {
                let (event_tx, _event_rx) = mpsc::unbounded_channel();
                let result = run_match(match_config, event_tx).await;
                (kind, result)
            });
        }

        let Some(joined) = workers.join_next().await else {
            break;
        };
        let (kind, result) = joined.map_err(|err| anyhow!("gate task panicked: {}", err))?;
        let result = result?;
        completed_matches += 1;

        match kind {
            GateTaskKind::CandidateVsOpponent(index) => {
                candidate_results[index] = Some(GateMatchSummary {
                    opponent_name: opponent_names[index].clone(),
                    result,
                });
            }
            GateTaskKind::BaselineVsOpponent(index) => {
                baseline_results[index] = Some(GateMatchSummary {
                    opponent_name: opponent_names[index].clone(),
                    result,
                });
            }
            GateTaskKind::HeadToHead => {
                head_to_head = Some(result);
            }
        }

        on_progress(GateRunProgress {
            completed_matches,
            total_matches,
            message: format!("{} / {} matches complete", completed_matches, total_matches),
        });
    }

    let candidate_matches = candidate_results
        .into_iter()
        .enumerate()
        .map(|(index, summary)| {
            summary.with_context(|| {
                format!(
                    "missing candidate gate result for opponent '{}'",
                    opponent_names[index]
                )
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let baseline_matches = baseline_results
        .into_iter()
        .enumerate()
        .map(|(index, summary)| {
            summary.with_context(|| {
                format!(
                    "missing baseline gate result for opponent '{}'",
                    opponent_names[index]
                )
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let head_to_head = head_to_head.context("missing gate head-to-head result")?;

    Ok((candidate_matches, baseline_matches, head_to_head))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SprtResult;

    fn result(wins: u32, draws: u32, losses: u32) -> TestResult {
        TestResult {
            wins,
            draws,
            losses,
            elo_diff: 0.0,
            elo_error: 0.0,
            los: 0.0,
            sprt_result: SprtResult::Inconclusive,
            games: Vec::new(),
        }
    }

    #[test]
    fn gate_side_summary_computes_score_percentage() {
        let summary = GateSideSummary::from_matches(
            "rev".into(),
            "abcd".into(),
            vec![
                GateMatchSummary {
                    opponent_name: "A".into(),
                    result: result(3, 1, 0),
                },
                GateMatchSummary {
                    opponent_name: "B".into(),
                    result: result(1, 2, 1),
                },
            ],
        );
        assert_eq!(summary.wins, 4);
        assert_eq!(summary.draws, 3);
        assert_eq!(summary.losses, 1);
        assert!((summary.score_pct - 68.75).abs() < 1e-6);
        assert!(summary.elo_diff.is_finite());
        assert!(summary.elo_error.is_finite());
        assert!((0.0..=1.0).contains(&summary.los));
    }

    #[test]
    fn gate_verdict_uses_score_delta_threshold() {
        let candidate = GateSideSummary::from_matches(
            "rev1".into(),
            "aaaa".into(),
            vec![GateMatchSummary {
                opponent_name: "A".into(),
                result: result(6, 2, 2),
            }],
        );
        let baseline = GateSideSummary::from_matches(
            "rev0".into(),
            "bbbb".into(),
            vec![GateMatchSummary {
                opponent_name: "A".into(),
                result: result(5, 2, 3),
            }],
        );

        let summary = GateRunSummary::new(
            "Sykora".into(),
            "release".into(),
            candidate,
            baseline,
            result(7, 1, 2),
            1.0,
        );

        assert_eq!(summary.verdict, GateVerdict::Pass);
        assert!(summary.score_delta_pct > 1.0);
        assert!((summary.head_to_head.score_pct - 75.0).abs() < 1e-6);
    }
}

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::TestResult;

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

        Self {
            revision_id,
            commit_hash,
            matches,
            wins,
            draws,
            losses,
            score_pct,
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

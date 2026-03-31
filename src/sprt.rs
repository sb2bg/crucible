//! Sequential Probability Ratio Test (SPRT) implementation.
//!
//! This is the heart of engine testing. SPRT lets us determine with statistical
//! confidence whether a change made the engine stronger or weaker, without
//! having to play a fixed (huge) number of games.

use crate::types::SprtResult;

/// SPRT bounds configuration
#[derive(Debug, Clone, Copy)]
pub struct SprtBounds {
    /// Elo threshold below which we consider "no improvement" (H0)
    pub elo0: f64,
    /// Elo threshold above which we consider "improvement" (H1)
    pub elo1: f64,
    /// Type I error rate (false positive)
    pub alpha: f64,
    /// Type II error rate (false negative)
    pub beta: f64,
}

impl Default for SprtBounds {
    fn default() -> Self {
        Self {
            elo0: 0.0,
            elo1: 5.0,
            alpha: 0.05,
            beta: 0.05,
        }
    }
}

impl SprtBounds {
    /// Standard regression test: is this commit no weaker?
    pub fn regression() -> Self {
        Self {
            elo0: -5.0,
            elo1: 0.0,
            alpha: 0.05,
            beta: 0.05,
        }
    }

    /// Standard improvement test: is this commit stronger?
    pub fn improvement() -> Self {
        Self::default()
    }

    /// Wider bounds for faster (less precise) testing
    pub fn fast() -> Self {
        Self {
            elo0: 0.0,
            elo1: 10.0,
            alpha: 0.05,
            beta: 0.05,
        }
    }

    /// Lower/upper decision boundaries (log-likelihood ratio)
    fn boundaries(&self) -> (f64, f64) {
        let lower = (self.beta / (1.0 - self.alpha)).ln();
        let upper = ((1.0 - self.beta) / self.alpha).ln();
        (lower, upper)
    }
}

/// Convert Elo difference to expected score (win probability)
pub fn elo_to_score(elo: f64) -> f64 {
    1.0 / (1.0 + 10.0_f64.powf(-elo / 400.0))
}

/// Convert win/draw/loss counts to estimated Elo difference
pub fn wdl_to_elo(wins: u32, draws: u32, losses: u32) -> f64 {
    let total = (wins + draws + losses) as f64;
    if total == 0.0 {
        return 0.0;
    }
    let score = (wins as f64 + draws as f64 * 0.5) / total;
    score_to_elo(score)
}

/// Convert a score (0.0 to 1.0) to Elo difference
pub fn score_to_elo(score: f64) -> f64 {
    if score <= 0.0 || score >= 1.0 {
        return if score <= 0.0 { -1000.0 } else { 1000.0 };
    }
    -400.0 * (1.0 / score - 1.0).log10()
}

/// Calculate the error margin on the Elo estimate (95% confidence)
pub fn elo_error(wins: u32, draws: u32, losses: u32) -> f64 {
    let total = (wins + draws + losses) as f64;
    if total < 2.0 {
        return f64::INFINITY;
    }

    let w = wins as f64 / total;
    let d = draws as f64 / total;
    let l = losses as f64 / total;

    let score = w + d * 0.5;
    let variance = w * (1.0 - score).powi(2) + d * (0.5 - score).powi(2) + l * score.powi(2);
    let stddev = (variance / total).sqrt();

    // Convert score stddev to Elo stddev
    // d(Elo)/d(score) = -400 / (ln(10) * score * (1 - score))
    let deriv = 400.0 / (std::f64::consts::LN_10 * score * (1.0 - score));

    // 1.96 * stddev for 95% confidence
    1.96 * stddev * deriv.abs()
}

/// Likelihood of Superiority (LOS): probability that the true Elo diff > 0
pub fn los(wins: u32, losses: u32) -> f64 {
    if wins + losses == 0 {
        return 0.5;
    }
    // Using the normal approximation
    let total = (wins + losses) as f64;
    let p = wins as f64 / total;
    let z = (p - 0.5) / (0.25 / total).sqrt();
    0.5 * (1.0 + erf(z / std::f64::consts::SQRT_2))
}

/// Run SPRT on the current WDL tallies
pub fn sprt_test(wins: u32, draws: u32, losses: u32, bounds: &SprtBounds) -> SprtResult {
    let total = wins + draws + losses;
    if total < 4 {
        return SprtResult::Inconclusive;
    }

    let llr = log_likelihood_ratio(wins, draws, losses, bounds.elo0, bounds.elo1);
    let (lower, upper) = bounds.boundaries();

    if llr >= upper {
        SprtResult::H1Accepted
    } else if llr <= lower {
        SprtResult::H0Accepted
    } else {
        SprtResult::Inconclusive
    }
}

/// Compute the log-likelihood ratio for the pentanomial/trinomial model
fn log_likelihood_ratio(wins: u32, draws: u32, losses: u32, elo0: f64, elo1: f64) -> f64 {
    let total = (wins + draws + losses) as f64;
    if total == 0.0 {
        return 0.0;
    }

    let w = wins as f64 / total;
    let d = draws as f64 / total;
    let s0 = elo_to_score(elo0);
    let s1 = elo_to_score(elo1);
    let s = w + d * 0.5;

    // Avoid log(0)
    if s <= 0.0 || s >= 1.0 {
        return if s >= 1.0 { 100.0 } else { -100.0 };
    }

    // BayesElo trinomial LLR approximation
    // LLR ≈ n * [ (s - s0)^2 / (2 * s * (1-s)) - (s - s1)^2 / (2 * s * (1-s)) ]
    // Simplified: LLR ≈ n * (s1 - s0) * (2*s - s0 - s1) / (2 * s * (1-s))
    let variance = s * (1.0 - s);
    if variance <= 0.0 {
        return 0.0;
    }

    total * (s1 - s0) * (2.0 * s - s0 - s1) / (2.0 * variance)
}

/// Error function approximation (Abramowitz and Stegun)
fn erf(x: f64) -> f64 {
    let a1 = 0.254829592;
    let a2 = -0.284496736;
    let a3 = 1.421413741;
    let a4 = -1.453152027;
    let a5 = 1.061405429;
    let p = 0.3275911;

    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let x = x.abs();
    let t = 1.0 / (1.0 + p * x);
    let y = 1.0 - (((((a5 * t + a4) * t) + a3) * t + a2) * t + a1) * t * (-x * x).exp();

    sign * y
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_elo_to_score() {
        // Equal strength -> 0.5
        assert!((elo_to_score(0.0) - 0.5).abs() < 1e-6);
        // +400 Elo -> ~0.909
        assert!((elo_to_score(400.0) - 0.9090909).abs() < 1e-4);
    }

    #[test]
    fn test_wdl_to_elo() {
        // Equal results -> 0 Elo
        assert!((wdl_to_elo(100, 200, 100) - 0.0).abs() < 1e-6);
        // All wins -> clamped high
        assert!(wdl_to_elo(100, 0, 0) > 500.0);
    }

    #[test]
    fn test_los() {
        // Equal wins/losses -> 0.5
        assert!((los(100, 100) - 0.5).abs() < 0.01);
        // Many more wins -> near 1.0
        assert!(los(200, 50) > 0.99);
    }

    #[test]
    fn test_sprt_conclusive() {
        let bounds = SprtBounds::default();
        // Overwhelming evidence of improvement
        let result = sprt_test(500, 400, 100, &bounds);
        assert_eq!(result, SprtResult::H1Accepted);
    }

    #[test]
    fn test_sprt_inconclusive_early() {
        let bounds = SprtBounds::default();
        // Too few games
        let result = sprt_test(2, 1, 1, &bounds);
        assert_eq!(result, SprtResult::Inconclusive);
    }
}

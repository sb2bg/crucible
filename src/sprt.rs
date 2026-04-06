//! Sequential Probability Ratio Test (SPRT) implementation.
//!
//! This is the heart of engine testing. SPRT lets us determine with statistical
//! confidence whether a change made the engine stronger or weaker, without
//! having to play a fixed (huge) number of games.

use anyhow::{bail, Result};

use crate::types::SprtResult;

const LOGISTIC_SCALE: f64 = 400.0;
const MIN_DRAW_SCALE: f64 = 1.0;
const MAX_DRAW_SCALE: f64 = 1.0e6;
const PROB_EPSILON: f64 = 1.0e-12;

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
    /// Minimum number of games required before allowing a decision
    pub min_games: u32,
}

impl Default for SprtBounds {
    fn default() -> Self {
        Self {
            elo0: 0.0,
            elo1: 5.0,
            alpha: 0.05,
            beta: 0.05,
            min_games: 16,
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
            min_games: 16,
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
            min_games: 16,
        }
    }

    /// Lower/upper decision boundaries (log-likelihood ratio)
    fn boundaries(&self) -> (f64, f64) {
        let lower = (self.beta / (1.0 - self.alpha)).ln();
        let upper = ((1.0 - self.beta) / self.alpha).ln();
        (lower, upper)
    }

    pub fn validate(&self) -> Result<()> {
        if !(0.0 < self.alpha && self.alpha < 1.0) {
            bail!("SPRT alpha must be between 0 and 1");
        }
        if !(0.0 < self.beta && self.beta < 1.0) {
            bail!("SPRT beta must be between 0 and 1");
        }
        if self.elo0 >= self.elo1 {
            bail!("SPRT elo0 must be less than elo1");
        }
        if self.min_games == 0 {
            bail!("SPRT min_games must be at least 1");
        }
        Ok(())
    }

    pub fn is_valid(&self) -> bool {
        self.validate().is_ok()
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
    if !bounds.is_valid() {
        return SprtResult::Inconclusive;
    }
    let total = wins + draws + losses;
    if total < bounds.min_games {
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

/// Compute the BayesElo-style trinomial log-likelihood ratio.
///
/// This uses the exact W/D/L likelihood from BayesElo's outcome model and
/// profiles out the nuisance `drawelo` parameter independently under H0 and H1.
fn log_likelihood_ratio(wins: u32, draws: u32, losses: u32, elo0: f64, elo1: f64) -> f64 {
    if wins + draws + losses == 0 {
        return 0.0;
    }

    let ll0 = profile_log_likelihood(wins, draws, losses, elo0);
    let ll1 = profile_log_likelihood(wins, draws, losses, elo1);
    ll1 - ll0
}

fn profile_log_likelihood(wins: u32, draws: u32, losses: u32, elo: f64) -> f64 {
    let draw_scale = mle_draw_scale(wins, draws, losses, elo);
    bayeselo_log_likelihood(wins, draws, losses, elo, draw_scale)
}

fn mle_draw_scale(wins: u32, draws: u32, losses: u32, elo: f64) -> f64 {
    if draws == 0 {
        return MIN_DRAW_SCALE;
    }

    let mut low = MIN_DRAW_SCALE;
    let mut high = 2.0;
    while draw_scale_derivative(wins, draws, losses, elo, high) > 0.0 && high < MAX_DRAW_SCALE {
        low = high;
        high = (high * 2.0).min(MAX_DRAW_SCALE);
    }

    if draw_scale_derivative(wins, draws, losses, elo, low) <= 0.0 {
        return low;
    }

    for _ in 0..96 {
        let mid = (low + high) * 0.5;
        if draw_scale_derivative(wins, draws, losses, elo, mid) > 0.0 {
            low = mid;
        } else {
            high = mid;
        }
    }

    (low + high) * 0.5
}

fn draw_scale_derivative(wins: u32, draws: u32, losses: u32, elo: f64, draw_scale: f64) -> f64 {
    let win_scale = 10.0_f64.powf(-elo / LOGISTIC_SCALE);
    let loss_scale = 10.0_f64.powf(elo / LOGISTIC_SCALE);
    let t = draw_scale.max(MIN_DRAW_SCALE);

    let draws_term = if draws == 0 {
        0.0
    } else {
        2.0 * draws as f64 * t / (t * t - 1.0)
    };
    let win_term = (wins + draws) as f64 * win_scale / (1.0 + win_scale * t);
    let loss_term = (losses + draws) as f64 * loss_scale / (1.0 + loss_scale * t);

    draws_term - win_term - loss_term
}

fn bayeselo_log_likelihood(wins: u32, draws: u32, losses: u32, elo: f64, draw_scale: f64) -> f64 {
    let (p_win, p_draw, p_loss) = bayeselo_probabilities(elo, draw_scale);
    wins as f64 * p_win.max(PROB_EPSILON).ln()
        + draws as f64 * p_draw.max(PROB_EPSILON).ln()
        + losses as f64 * p_loss.max(PROB_EPSILON).ln()
}

fn bayeselo_probabilities(elo: f64, draw_scale: f64) -> (f64, f64, f64) {
    let win_scale = 10.0_f64.powf(-elo / LOGISTIC_SCALE);
    let loss_scale = 10.0_f64.powf(elo / LOGISTIC_SCALE);
    let t = draw_scale.max(MIN_DRAW_SCALE);

    let p_win = 1.0 / (1.0 + win_scale * t);
    let p_loss = 1.0 / (1.0 + loss_scale * t);
    let p_draw = 1.0 - p_win - p_loss;

    (p_win, p_draw.max(0.0), p_loss)
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

    #[test]
    fn test_invalid_bounds_are_rejected() {
        let bounds = SprtBounds {
            elo0: 5.0,
            elo1: 0.0,
            alpha: 1.5,
            beta: 0.05,
            min_games: 16,
        };
        assert!(bounds.validate().is_err());
        assert_eq!(sprt_test(100, 100, 100, &bounds), SprtResult::Inconclusive);
    }

    #[test]
    fn test_perfect_short_run_stays_inconclusive_before_min_games() {
        let bounds = SprtBounds::default();
        let result = sprt_test(4, 0, 0, &bounds);
        assert_eq!(result, SprtResult::Inconclusive);
    }

    #[test]
    fn bayeselo_probabilities_match_expected_symmetry() {
        let (p_win, p_draw, p_loss) = bayeselo_probabilities(0.0, 10.0_f64.powf(97.3 / 400.0));
        assert!((p_win - p_loss).abs() < 1e-12);
        assert!((p_win + p_draw + p_loss - 1.0).abs() < 1e-12);
    }

    #[test]
    fn draw_scale_profiles_to_boundary_without_draws() {
        let draw_scale = mle_draw_scale(10, 0, 10, 0.0);
        assert!((draw_scale - 1.0).abs() < 1e-12);
    }

    #[test]
    fn llr_flips_sign_when_wins_and_losses_are_swapped() {
        let forward = log_likelihood_ratio(80, 40, 20, 0.0, 5.0);
        let reverse = log_likelihood_ratio(20, 40, 80, 0.0, 5.0);
        assert!(forward > 0.0);
        assert!(reverse < 0.0);
    }
}

//! Phase 4 / A6: confidence — the feedback loop for experiences.
//!
//! Score formula (borrowed reference semantics):
//!   score = PA × (base + UF×w_uf + UA×w_ua + R×w_r) × DS
//!   PA = success / (success + failure + 1)         prediction accuracy
//!   UF = min(frequency, 10) / 10                   usage-frequency amplifier
//!   UA = (positive_fb + 1) / (positive_fb + negative_fb + 2)  user acceptance
//!   R  = e^(-0.05 × days_since_last_use)           freshness (~14d half-life)
//!   DS = data stability (default 1.0; derived from output consistency later)

use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ConfidenceWeights {
    pub base: f32,
    pub usage_frequency: f32,
    pub user_acceptance: f32,
    pub freshness: f32,
}

impl Default for ConfidenceWeights {
    fn default() -> Self {
        Self {
            base: 0.50,
            usage_frequency: 0.20,
            user_acceptance: 0.15,
            freshness: 0.15,
        }
    }
}

/// Multi-dimensional counters maintained per experience.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ConfidenceCounters {
    pub success_count: u32,
    pub failure_count: u32,
    pub streak: u32,
    /// Consecutive failures (drives the Phase 9 lifecycle demotion).
    pub failure_streak: u32,
    pub frequency: f32,
    pub total_usage: u32,
    pub positive_fb: u32,
    pub negative_fb: u32,
    /// Last usage timestamp (seconds) — the aging basis for forgetting.
    pub last_used: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedbackKind {
    Success,
    Failure,
    UserPositive,
    UserNegative,
}

/// Maturity level derived from the score (borrowed raw/learning/automatic).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ConfidenceLevel {
    Raw,
    Learning,
    Automatic,
}

impl ConfidenceLevel {
    pub fn from_score(score: f32) -> Self {
        if score >= 0.75 {
            ConfidenceLevel::Automatic
        } else if score >= 0.35 {
            ConfidenceLevel::Learning
        } else {
            ConfidenceLevel::Raw
        }
    }
}

impl ConfidenceCounters {
    /// Apply one feedback event to the counters.
    pub fn apply(&mut self, feedback: FeedbackKind) {
        match feedback {
            FeedbackKind::Success => {
                self.success_count += 1;
                self.streak += 1;
                self.failure_streak = 0;
                self.frequency = self.frequency * 0.7 + 1.0;
                self.total_usage += 1;
            }
            FeedbackKind::Failure => {
                self.failure_count += 1;
                self.streak = 0;
                self.failure_streak += 1;
                self.total_usage += 1;
            }
            FeedbackKind::UserPositive => self.positive_fb += 1,
            FeedbackKind::UserNegative => self.negative_fb += 1,
        }
    }

    /// Time-based decay of usage frequency (call per tick, e.g. daily).
    pub fn tick(&mut self) {
        self.frequency *= 0.7;
    }

    /// Mark the moment this experience was used (refreshes freshness).
    pub fn touch(&mut self, now_secs: u64) {
        self.last_used = Some(now_secs);
    }

    pub fn score(
        &self,
        weights: &ConfidenceWeights,
        days_since_last_use: f32,
        data_stability: f32,
    ) -> f32 {
        let pa = self.success_count as f32
            / (self.success_count + self.failure_count + 1) as f32;
        let uf = (self.frequency / 10.0).min(1.0);
        let ua = (self.positive_fb + 1) as f32
            / (self.positive_fb + self.negative_fb + 2) as f32;
        let freshness = (-0.05 * days_since_last_use).exp();
        pa
            * (weights.base
                + uf * weights.usage_frequency
                + ua * weights.user_acceptance
                + freshness * weights.freshness)
            * data_stability
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_default_counters_score_zero_and_raw() {
        let counters = ConfidenceCounters::default();
        let score = counters.score(&ConfidenceWeights::default(), 0.0, 1.0);
        assert_eq!(score, 0.0);
        assert_eq!(ConfidenceLevel::from_score(score), ConfidenceLevel::Raw);
    }

    #[test]
    fn successes_raise_score_and_level() {
        let mut counters = ConfidenceCounters::default();
        for _ in 0..5 {
            counters.apply(FeedbackKind::Success);
        }
        let score = counters.score(&ConfidenceWeights::default(), 0.0, 1.0);
        assert!(score > 0.35, "score was {score}");
        assert_eq!(ConfidenceLevel::from_score(score), ConfidenceLevel::Learning);
    }

    #[test]
    fn high_evidence_reaches_automatic() {
        let mut counters = ConfidenceCounters::default();
        for _ in 0..20 {
            counters.apply(FeedbackKind::Success);
        }
        counters.apply(FeedbackKind::UserPositive);
        counters.apply(FeedbackKind::UserPositive);
        let score = counters.score(&ConfidenceWeights::default(), 0.0, 1.0);
        assert!(score >= 0.75, "score was {score}");
        assert_eq!(ConfidenceLevel::from_score(score), ConfidenceLevel::Automatic);
    }

    #[test]
    fn failure_resets_streak_and_lowers_score() {
        let mut counters = ConfidenceCounters::default();
        counters.apply(FeedbackKind::Success);
        counters.apply(FeedbackKind::Success);
        let before = counters.score(&ConfidenceWeights::default(), 0.0, 1.0);
        counters.apply(FeedbackKind::Failure);
        assert_eq!(counters.streak, 0);
        let after = counters.score(&ConfidenceWeights::default(), 0.0, 1.0);
        assert!(after < before);
    }

    #[test]
    fn tick_decays_frequency() {
        let mut counters = ConfidenceCounters::default();
        counters.apply(FeedbackKind::Success);
        let before = counters.frequency;
        counters.tick();
        assert!(counters.frequency < before);
    }

    #[test]
    fn failure_streak_tracks_consecutive_failures() {
        let mut counters = ConfidenceCounters::default();
        counters.apply(FeedbackKind::Success);
        assert_eq!(counters.failure_streak, 0);
        counters.apply(FeedbackKind::Failure);
        counters.apply(FeedbackKind::Failure);
        assert_eq!(counters.failure_streak, 2);
        counters.apply(FeedbackKind::Success);
        assert_eq!(counters.failure_streak, 0);
    }

    #[test]
    fn staleness_reduces_freshness_factor() {
        let mut counters = ConfidenceCounters::default();
        for _ in 0..5 {
            counters.apply(FeedbackKind::Success);
        }
        let fresh = counters.score(&ConfidenceWeights::default(), 0.0, 1.0);
        let stale = counters.score(&ConfidenceWeights::default(), 30.0, 1.0);
        assert!(stale < fresh);
    }
}

//! Phase 9: the experience state machine.
//!
//! Lifecycle: NEW → CANDIDATE → VALIDATED → ACTIVE → DECAYING → DISABLED,
//! driven by two forces:
//! 1. explicit actions (`LifecycleAction`: validate / activate / disable /
//!    revalidate) — user or system decisions;
//! 2. automatic confidence-driven transitions (`apply_feedback`) — successes
//!    promote, failures demote, decay can recover.

use super::experience::ExperienceStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleAction {
    /// CANDIDATE → VALIDATED (user/system validation of a compiled draft).
    Validate,
    /// VALIDATED → ACTIVE (explicit activation).
    Activate,
    /// any → DISABLED (explicit disable).
    Disable,
    /// DISABLED → VALIDATED (manual re-enable).
    Revalidate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleError {
    IllegalTransition {
        from: ExperienceStatus,
        action: LifecycleAction,
    },
    NotFound,
}

/// Result of the time-based forgetting check (M2-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForgettingDecision {
    Keep,
    DemoteToDecaying,
    Disable,
}

impl std::fmt::Display for LifecycleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LifecycleError::IllegalTransition { from, action } => {
                write!(f, "illegal transition from {from:?} via {action:?}")
            }
            LifecycleError::NotFound => write!(f, "experience not found"),
        }
    }
}

pub struct ExperienceLifecycle;

impl ExperienceLifecycle {
    /// Score needed for VALIDATED (matches `ConfidenceLevel::Learning`).
    pub const VALIDATED_THRESHOLD: f32 = 0.35;
    /// Score needed for ACTIVE (matches `ConfidenceLevel::Automatic`).
    pub const ACTIVE_THRESHOLD: f32 = 0.75;
    /// Consecutive failures that demote ACTIVE → DECAYING.
    pub const DECAYING_FAILURES: u32 = 3;
    /// Consecutive failures that demote DECAYING/ACTIVE → DISABLED.
    pub const DISABLE_FAILURES: u32 = 5;
    /// Consecutive successes that recover DECAYING → ACTIVE.
    pub const RECOVERY_STREAK: u32 = 2;
    /// Days without use after which a used experience goes stale (DECAYING).
    pub const STALE_DAYS: u64 = 30;
    /// Days without use after which a stale experience is disabled.
    pub const FORGET_DAYS: u64 = 60;

    /// Explicit transition. Returns the new status, or an error for illegal
    /// pairs.
    pub fn transition(
        current: ExperienceStatus,
        action: LifecycleAction,
    ) -> Result<ExperienceStatus, LifecycleError> {
        let next = match (current, action) {
            (ExperienceStatus::New, LifecycleAction::Validate)
            | (ExperienceStatus::Candidate, LifecycleAction::Validate) => {
                ExperienceStatus::Validated
            }
            (ExperienceStatus::Validated, LifecycleAction::Activate) => ExperienceStatus::Active,
            (ExperienceStatus::Decaying, LifecycleAction::Activate) => ExperienceStatus::Active,
            (ExperienceStatus::Decaying, LifecycleAction::Validate) => ExperienceStatus::Validated,
            (_, LifecycleAction::Disable) => ExperienceStatus::Disabled,
            (ExperienceStatus::Disabled, LifecycleAction::Revalidate) => {
                ExperienceStatus::Validated
            }
            _ => {
                return Err(LifecycleError::IllegalTransition { from: current, action });
            }
        };
        Ok(next)
    }

    /// Automatic, confidence-driven transition after a feedback event.
    pub fn apply_feedback(
        status: ExperienceStatus,
        score: f32,
        streak: u32,
        failure_streak: u32,
    ) -> ExperienceStatus {
        match status {
            ExperienceStatus::New | ExperienceStatus::Candidate => {
                if score >= Self::VALIDATED_THRESHOLD {
                    ExperienceStatus::Validated
                } else {
                    status
                }
            }
            ExperienceStatus::Validated => {
                if score >= Self::ACTIVE_THRESHOLD {
                    ExperienceStatus::Active
                } else {
                    ExperienceStatus::Validated
                }
            }
            ExperienceStatus::Active => {
                if failure_streak >= Self::DISABLE_FAILURES {
                    ExperienceStatus::Disabled
                } else if failure_streak >= Self::DECAYING_FAILURES
                    || score < Self::VALIDATED_THRESHOLD
                {
                    ExperienceStatus::Decaying
                } else {
                    ExperienceStatus::Active
                }
            }
            ExperienceStatus::Decaying => {
                if failure_streak >= Self::DISABLE_FAILURES {
                    ExperienceStatus::Disabled
                } else if streak >= Self::RECOVERY_STREAK {
                    ExperienceStatus::Active
                } else {
                    ExperienceStatus::Decaying
                }
            }
            ExperienceStatus::Disabled => ExperienceStatus::Disabled,
        }
    }

    /// Time-based forgetting (M2-3, precise algorithm):
    ///
    /// - `pinned` experiences are exempt — they are never forgotten by time,
    ///   only by an explicit `Disable`.
    /// - Experiences never used (`last_used = None`) have no aging basis and
    ///   are kept; NEW/CANDIDATE are self-pruned by the feedback lifecycle,
    ///   not by time (v1).
    /// - VALIDATED/ACTIVE: age >= STALE_DAYS (30) → DECAYING;
    ///   age >= FORGET_DAYS (60) → DISABLED.
    /// - DECAYING: age >= FORGET_DAYS (60) → DISABLED.
    pub fn forgetting(
        status: ExperienceStatus,
        last_used_secs: Option<u64>,
        now_secs: u64,
        pinned: bool,
    ) -> ForgettingDecision {
        if pinned {
            return ForgettingDecision::Keep;
        }
        let Some(last_used) = last_used_secs else {
            return ForgettingDecision::Keep;
        };
        let age_days = now_secs.saturating_sub(last_used) / 86_400;
        match status {
            ExperienceStatus::Validated | ExperienceStatus::Active => {
                if age_days >= Self::FORGET_DAYS {
                    ForgettingDecision::Disable
                } else if age_days >= Self::STALE_DAYS {
                    ForgettingDecision::DemoteToDecaying
                } else {
                    ForgettingDecision::Keep
                }
            }
            ExperienceStatus::Decaying => {
                if age_days >= Self::FORGET_DAYS {
                    ForgettingDecision::Disable
                } else {
                    ForgettingDecision::Keep
                }
            }
            _ => ForgettingDecision::Keep,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_transitions() {
        assert_eq!(
            ExperienceLifecycle::transition(ExperienceStatus::Candidate, LifecycleAction::Validate),
            Ok(ExperienceStatus::Validated)
        );
        assert_eq!(
            ExperienceLifecycle::transition(ExperienceStatus::Validated, LifecycleAction::Activate),
            Ok(ExperienceStatus::Active)
        );
        assert_eq!(
            ExperienceLifecycle::transition(ExperienceStatus::Active, LifecycleAction::Disable),
            Ok(ExperienceStatus::Disabled)
        );
        assert_eq!(
            ExperienceLifecycle::transition(ExperienceStatus::Disabled, LifecycleAction::Revalidate),
            Ok(ExperienceStatus::Validated)
        );
    }

    #[test]
    fn illegal_transition_is_rejected() {
        assert!(matches!(
            ExperienceLifecycle::transition(
                ExperienceStatus::Candidate,
                LifecycleAction::Activate
            ),
            Err(LifecycleError::IllegalTransition { .. })
        ));
    }

    #[test]
    fn successes_promote_candidate_to_validated_then_active() {
        assert_eq!(
            ExperienceLifecycle::apply_feedback(ExperienceStatus::Candidate, 0.40, 1, 0),
            ExperienceStatus::Validated
        );
        assert_eq!(
            ExperienceLifecycle::apply_feedback(ExperienceStatus::Validated, 0.80, 10, 0),
            ExperienceStatus::Active
        );
    }

    #[test]
    fn failures_demote_active_to_decaying_then_disabled() {
        assert_eq!(
            ExperienceLifecycle::apply_feedback(ExperienceStatus::Active, 0.5, 0, 3),
            ExperienceStatus::Decaying
        );
        assert_eq!(
            ExperienceLifecycle::apply_feedback(ExperienceStatus::Decaying, 0.3, 0, 5),
            ExperienceStatus::Disabled
        );
    }

    #[test]
    fn decaying_recovers_on_success_streak() {
        assert_eq!(
            ExperienceLifecycle::apply_feedback(ExperienceStatus::Decaying, 0.4, 2, 0),
            ExperienceStatus::Active
        );
    }

    #[test]
    fn disabled_is_terminal() {
        assert_eq!(
            ExperienceLifecycle::apply_feedback(ExperienceStatus::Disabled, 0.9, 5, 0),
            ExperienceStatus::Disabled
        );
    }

    #[test]
    fn forgetting_pinned_is_exempt() {
        for status in [
            ExperienceStatus::Validated,
            ExperienceStatus::Active,
            ExperienceStatus::Decaying,
        ] {
            assert_eq!(
                ExperienceLifecycle::forgetting(status, Some(0), 100 * 86_400, true),
                ForgettingDecision::Keep
            );
        }
    }

    #[test]
    fn forgetting_ages_active_to_decaying_then_disabled() {
        let now = 1_700_000_000u64;
        let last_used = now - 31 * 86_400;
        assert_eq!(
            ExperienceLifecycle::forgetting(ExperienceStatus::Active, Some(last_used), now, false),
            ForgettingDecision::DemoteToDecaying
        );
        let older = now - 61 * 86_400;
        assert_eq!(
            ExperienceLifecycle::forgetting(ExperienceStatus::Decaying, Some(older), now, false),
            ForgettingDecision::Disable
        );
    }

    #[test]
    fn forgetting_keeps_fresh_and_never_used() {
        let now = 1_700_000_000u64;
        assert_eq!(
            ExperienceLifecycle::forgetting(ExperienceStatus::Validated, Some(now - 10 * 86_400), now, false),
            ForgettingDecision::Keep
        );
        assert_eq!(
            ExperienceLifecycle::forgetting(ExperienceStatus::Active, None, now, false),
            ForgettingDecision::Keep
        );
        assert_eq!(
            ExperienceLifecycle::forgetting(ExperienceStatus::Candidate, Some(0), now, false),
            ForgettingDecision::Keep
        );
    }
}

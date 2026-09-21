//! Action Gate domain: the synchronous takeover contract.
//!
//! Contract (step-gate.md §8.2): after `decide` returns a Hit, execute +
//! verify + return happen inside the SAME synchronous call. When the Gate
//! call returns, the takeover is over: a final `GateHitResult` with
//! completion_status (completed / partial / failed) and verification_status
//! has been produced and control has returned to the Agent. This is a
//! control-flow transaction, never a world-state atomicity promise.

use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;

use super::action::ActionProposal;
use super::experience::Experience;
use super::predicate::StateFact;

/// Four takeover abilities (step-gate.md §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum GateTier {
    /// Replace one tool call.
    A,
    /// Replace one tool call + verify.
    B,
    /// Replace a contiguous chain of tool calls (all inside the experience).
    C,
    /// Replace a whole exploration transition the Agent would have had to
    /// discover across several LLM turns.
    D,
}

/// Outcome of the synchronous `decide` step: mechanical, near-zero cost.
/// MISS means pass-through to the original tool executor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GateDecision {
    Miss,
    Hit {
        experience: String,
        tier: GateTier,
    },
    /// Preconditions no longer hold, but every postcondition already holds.
    /// This is a successful no-op, not a takeover and not a pass-through:
    /// re-entry must not wake the LLM to rediscover that the work is done.
    Satisfied {
        experience: String,
        tier: GateTier,
        /// Facts already observed by `decide`; reused by the no-op result so
        /// re-entry does not probe the same world twice.
        state_after: Vec<StateFact>,
    },
    /// A template bound deterministically into a concrete Experience. The
    /// instantiated body travels with the decision so execution never has to
    /// re-bind or invent missing parameters.
    TemplateHit {
        experience: String,
        tier: GateTier,
        bindings: BTreeMap<String, String>,
        /// `ExperienceTemplate::binding_fingerprint` of `bindings`; carried on
        /// the decision so the audit record never re-derives it.
        fingerprint: String,
        instantiated: Experience,
    },
}

impl GateDecision {
    pub fn is_hit(&self) -> bool {
        matches!(self, GateDecision::Hit { .. })
    }

    pub fn is_satisfied(&self) -> bool {
        matches!(self, GateDecision::Satisfied { .. })
    }

    /// Any decision that must not fall through to the original tool.
    pub fn is_takeover(&self) -> bool {
        self.is_hit() || self.is_satisfied() || matches!(self, GateDecision::TemplateHit { .. })
    }

    pub fn experience_name(&self) -> Option<&str> {
        match self {
            GateDecision::Miss => None,
            GateDecision::Hit { experience, .. }
            | GateDecision::Satisfied { experience, .. }
            | GateDecision::TemplateHit { experience, .. } => Some(experience),
        }
    }
}

/// How far the takeover got. These are final: the Gate never returns while an
/// Experience is still running (control-flow transaction).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionStatus {
    Completed,
    Partial,
    Failed,
}

/// Whether the postconditions were actually verified against evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    Verified,
    Unverified,
    Failed,
}

/// One workflow step that actually executed, with its own evidence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutedStep {
    pub action: String,
    pub args: serde_json::Value,
    pub evidence: Option<String>,
}

/// A side effect that occurred but is not (fully) captured by predicates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutedSideEffect {
    pub description: String,
    pub evidence: Option<String>,
}

/// Audit record for a template-mediated takeover.
///
/// A `TemplateHit` is the only path where the executed body was *derived* at
/// decision time, so it is the only path that needs an audit record: which
/// template was used, which values were captured from the request, and a
/// stable fingerprint of the instantiated instance. No field here is ever
/// produced by a model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateBindingAudit {
    pub template: String,
    pub bindings: BTreeMap<String, String>,
    /// `ExperienceTemplate::binding_fingerprint` of the captured bindings.
    pub fingerprint: String,
}

/// The closed Result handed back to the Agent (step-gate.md §9).
///
/// `completion_status == Completed` is only reachable together with
/// `verification_status == Verified` (see constructors): an Experience never
/// claims completion without verified postconditions, and never swallows a
/// partial/failed outcome. The constructor fixes the status flags; whether
/// the postconditions are really met is enforced by the runtime's
/// verification order, not by the type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateHitResult {
    pub proposed_action: ActionProposal,
    pub state_before: Vec<StateFact>,
    pub executed: Vec<ExecutedStep>,
    pub state_after: Vec<StateFact>,
    pub execution_evidence: Vec<String>,
    pub executed_side_effects: Vec<ExecutedSideEffect>,
    pub completion_status: CompletionStatus,
    pub verification_status: VerificationStatus,
    /// Present only when the takeover went through a parameterized template.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_audit: Option<TemplateBindingAudit>,
}

impl GateHitResult {
    /// Claim completion. Only call after postconditions were verified against
    /// real evidence; this constructor is the only way to obtain Verified.
    #[allow(clippy::too_many_arguments)]
    pub fn completed(
        proposed_action: ActionProposal,
        state_before: Vec<StateFact>,
        executed: Vec<ExecutedStep>,
        state_after: Vec<StateFact>,
        execution_evidence: Vec<String>,
        executed_side_effects: Vec<ExecutedSideEffect>,
    ) -> Self {
        Self {
            proposed_action,
            state_before,
            executed,
            state_after,
            execution_evidence,
            executed_side_effects,
            completion_status: CompletionStatus::Completed,
            verification_status: VerificationStatus::Verified,
            template_audit: None,
        }
    }

    /// Honest partial outcome: something ran, but postconditions are not all
    /// verified. Never claims completion.
    #[allow(clippy::too_many_arguments)]
    pub fn partial(
        proposed_action: ActionProposal,
        state_before: Vec<StateFact>,
        executed: Vec<ExecutedStep>,
        state_after: Vec<StateFact>,
        execution_evidence: Vec<String>,
        executed_side_effects: Vec<ExecutedSideEffect>,
    ) -> Self {
        Self {
            proposed_action,
            state_before,
            executed,
            state_after,
            execution_evidence,
            executed_side_effects,
            completion_status: CompletionStatus::Partial,
            verification_status: VerificationStatus::Unverified,
            template_audit: None,
        }
    }

    /// Failed takeover: side effects (if any) are reported for Agent
    /// takeover; the Experience never retries silently.
    #[allow(clippy::too_many_arguments)]
    pub fn failed(
        proposed_action: ActionProposal,
        state_before: Vec<StateFact>,
        executed: Vec<ExecutedStep>,
        state_after: Vec<StateFact>,
        execution_evidence: Vec<String>,
        executed_side_effects: Vec<ExecutedSideEffect>,
    ) -> Self {
        Self {
            proposed_action,
            state_before,
            executed,
            state_after,
            execution_evidence,
            executed_side_effects,
            completion_status: CompletionStatus::Failed,
            verification_status: VerificationStatus::Failed,
            template_audit: None,
        }
    }

    /// A takeover counts as successful only when completed AND verified.
    pub fn is_success(&self) -> bool {
        self.completion_status == CompletionStatus::Completed
            && self.verification_status == VerificationStatus::Verified
    }

    /// Attach the template binding audit (only TemplateHit takeovers carry it).
    pub fn with_template_audit(mut self, audit: Option<TemplateBindingAudit>) -> Self {
        self.template_audit = audit;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::predicate::StateFact;

    fn proposal() -> ActionProposal {
        ActionProposal::new(
            "exec_command",
            serde_json::json!({ "cmd": "create probe file" }),
        )
    }

    #[test]
    fn miss_means_pass_through() {
        let decision = GateDecision::Miss;
        assert!(!decision.is_hit());
        assert!(!decision.is_satisfied());
        assert!(!decision.is_takeover());
    }

    #[test]
    fn satisfied_is_a_successful_noop_not_a_pass_through() {
        let decision = GateDecision::Satisfied {
            experience: "move_done".into(),
            tier: GateTier::B,
            state_after: vec![StateFact::new(
                "file:source.txt.exists",
                serde_json::json!(false),
                "probe",
                1,
            )],
        };
        assert!(!decision.is_hit());
        assert!(decision.is_satisfied());
        assert!(decision.is_takeover());
    }

    #[test]
    fn completed_implies_verified() {
        let result = GateHitResult::completed(
            proposal(),
            vec![],
            vec![],
            vec![StateFact::new(
                "file:probe.txt.exists",
                serde_json::json!(true),
                "ls",
                1,
            )],
            vec!["postconditions verified".into()],
            vec![],
        );
        assert!(result.is_success());
        assert_eq!(result.verification_status, VerificationStatus::Verified);
    }

    #[test]
    fn partial_never_claims_completion_and_reports_side_effects() {
        let result = GateHitResult::partial(
            proposal(),
            vec![],
            vec![ExecutedStep {
                action: "write_file".into(),
                args: serde_json::json!({ "path": "probe.txt" }),
                evidence: Some("exit 0".into()),
            }],
            vec![],
            vec![],
            vec![ExecutedSideEffect {
                description: "created probe.txt".into(),
                evidence: Some("dir".into()),
            }],
        );
        assert!(!result.is_success());
        assert_eq!(result.completion_status, CompletionStatus::Partial);
        assert!(!result.executed_side_effects.is_empty());
    }

    #[test]
    fn failed_is_a_final_status_for_agent_takeover() {
        let result = GateHitResult::failed(
            proposal(),
            vec![],
            vec![],
            vec![],
            vec!["step 2: exec_command failed with exit 1".into()],
            vec![],
        );
        assert!(!result.is_success());
        assert_eq!(result.verification_status, VerificationStatus::Failed);
    }
}

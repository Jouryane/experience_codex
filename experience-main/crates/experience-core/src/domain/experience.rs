//! Experience: the compiled, re-runnable form of a known transition.
//!
//! An Experience is NOT a memory text and NOT a plan. Its workflow is a list
//! of pre-compiled steps, each mapping to a real Tool/Executor capability;
//! nothing here is left for an LLM to interpret at execution time. This is
//! the schema locked in p1-development-plan.md v1.3.

use serde::Deserialize;
use serde::Serialize;

use super::action::ActionPattern;
use super::predicate::Predicate;
use super::predicate::StateFact;
use super::predicate::TruthValue;
use super::predicate::fact_value;

/// One compiled workflow step, mapped to a real tool capability.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowStep {
    /// Tool capability name, e.g. `write_file` or `exec_command`.
    pub action: String,
    /// Concrete arguments for that capability.
    pub args: serde_json::Value,
}

impl WorkflowStep {
    pub fn new(action: impl Into<String>, args: serde_json::Value) -> Self {
        Self {
            action: action.into(),
            args,
        }
    }
}

/// How the runtime observes the world after execution (P1 scope).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VerificationStep {
    /// Read a file; optionally require exact content.
    ReadFile {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect_content: Option<String>,
    },
    /// Probe an environment predicate directly (e.g. path exists).
    Probe { predicate: Predicate },
}

/// What the Experience does when a workflow step fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailurePolicy {
    /// Stop immediately, verify the actual state, report partial/failed.
    StopAndReport,
}

/// Whether/how side effects can be compensated. P1: unsupported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UndoPolicy {
    Unsupported,
}

/// Lifecycle status. P1 subset plus the L2 qualification states:
/// candidates are storable but never enter the ACTIVE matching index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExperienceStatus {
    Draft,
    /// L1 output: learned but never executed (architectural invariant).
    Candidate,
    /// L2 output: passed the qualification chain, eligible for activation.
    Validated,
    Active,
    /// Evidence/usage-driven demotion (pin exempt); never inferred from
    /// inactivity alone (L2 v2 ruling: low frequency lowers priority, it
    /// does not decay quality).
    Decaying,
    Disabled,
}

/// Qualification/management actions accepted by the state transition table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualificationAction {
    Validate,
    Activate,
    /// Override of the *automatic activation decision* only; the
    /// VALIDATED prerequisite is never bypassed.
    ForceActivate,
    Invalidate,
    Disable,
    Revalidate,
    /// Automatic lifecycle producer (L3 usage feedback): negative evidence
    /// lowers confidence below the active floor; ACTIVE -> DECAYING.
    Decay,
}

impl ExperienceStatus {
    /// Single source of truth for status transitions (L2 C1). API and
    /// management surfaces must call this; never mutate status directly.
    pub fn transition(&self, action: QualificationAction) -> Result<Self, String> {
        match action {
            QualificationAction::Validate => match self {
                ExperienceStatus::Candidate => Ok(ExperienceStatus::Validated),
                _ => Err(format!("validate requires CANDIDATE, was {self:?}")),
            },
            QualificationAction::Activate | QualificationAction::ForceActivate => {
                if matches!(self, ExperienceStatus::Validated) {
                    Ok(ExperienceStatus::Active)
                } else {
                    Err(format!(
                        "activation requires VALIDATED, was {self:?} (override cannot skip qualification)"
                    ))
                }
            }
            QualificationAction::Invalidate => match self {
                ExperienceStatus::Candidate => Ok(ExperienceStatus::Disabled),
                _ => Err(format!("invalidate requires CANDIDATE, was {self:?}")),
            },
            QualificationAction::Disable => Ok(ExperienceStatus::Disabled),
            QualificationAction::Revalidate => match self {
                ExperienceStatus::Disabled | ExperienceStatus::Decaying => {
                    Ok(ExperienceStatus::Validated)
                }
                _ => Err(format!("revalidate requires DISABLED/DECAYING, was {self:?}")),
            },
            QualificationAction::Decay => match self {
                ExperienceStatus::Active => Ok(ExperienceStatus::Decaying),
                _ => Err(format!("decay requires ACTIVE, was {self:?}")),
            },
        }
    }
}

/// A known transition: `trigger + preconditions -> workflow -> postconditions`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Experience {
    pub name: String,
    pub trigger: ActionPattern,
    /// State that must already hold for the transition to be applicable.
    pub preconditions: Vec<Predicate>,
    /// Pre-compiled steps; the Experience never plans at runtime.
    pub workflow: Vec<WorkflowStep>,
    /// State that must hold after execution for completion to be claimable.
    pub postconditions: Vec<Predicate>,
    /// How to observe the postconditions (evidence, not assertion).
    pub verification: Vec<VerificationStep>,
    pub failure_policy: FailurePolicy,
    pub undo: UndoPolicy,
    pub status: ExperienceStatus,
}

/// A schema-level defect in an Experience.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaIssue {
    EmptyName,
    EmptyTriggerTool,
    EmptyWorkflow,
    EmptyStepAction(usize),
    EmptyPostconditions,
}

impl Experience {
    /// Structural validation (validation ladder, level 1: schema valid).
    /// Returns every defect found; an empty Vec means structurally valid.
    pub fn schema_issues(&self) -> Vec<SchemaIssue> {
        let mut issues = Vec::new();
        if self.name.trim().is_empty() {
            issues.push(SchemaIssue::EmptyName);
        }
        if self.trigger.tool.trim().is_empty() {
            issues.push(SchemaIssue::EmptyTriggerTool);
        }
        if self.workflow.is_empty() {
            issues.push(SchemaIssue::EmptyWorkflow);
        } else {
            for (index, step) in self.workflow.iter().enumerate() {
                if step.action.trim().is_empty() {
                    issues.push(SchemaIssue::EmptyStepAction(index));
                }
            }
        }
        if self.postconditions.is_empty() {
            issues.push(SchemaIssue::EmptyPostconditions);
        }
        issues
    }

    pub fn is_schema_valid(&self) -> bool {
        self.schema_issues().is_empty()
    }

    /// Do the observed facts satisfy every postcondition?
    ///
    /// This is the honesty gate for claiming `completed`: completion must not
    /// be reported unless the postconditions are actually verified against
    /// real evidence (hard constraint 2: Experience does not forge state).
    pub fn postconditions_satisfied_by(&self, state_after: &[StateFact]) -> bool {
        self.postconditions.iter().all(|condition| {
            condition.probe(fact_value(state_after, &condition.key)) == TruthValue::True
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn probe_file_experience() -> Experience {
        Experience {
            name: "create_probe_file".into(),
            trigger: ActionPattern {
                tool: "exec_command".into(),
                command_pattern: Some("create probe file".into()),
            },
            preconditions: vec![Predicate::new("cwd.exists", serde_json::json!(true))],
            workflow: vec![WorkflowStep::new(
                "write_file",
                serde_json::json!({
                    "path": "probe.txt",
                    "content": "EXPERIENCE_GATE_SUCCESS"
                }),
            )],
            postconditions: vec![
                Predicate::new("file:probe.txt.exists", serde_json::json!(true)),
                Predicate::new(
                    "file:probe.txt.content",
                    serde_json::json!("EXPERIENCE_GATE_SUCCESS"),
                ),
            ],
            verification: vec![VerificationStep::ReadFile {
                path: "probe.txt".into(),
                expect_content: Some("EXPERIENCE_GATE_SUCCESS".into()),
            }],
            failure_policy: FailurePolicy::StopAndReport,
            undo: UndoPolicy::Unsupported,
            status: ExperienceStatus::Active,
        }
    }

    #[test]
    fn sample_experience_is_schema_valid() {
        let experience = probe_file_experience();
        assert!(experience.is_schema_valid());
    }

    #[test]
    fn schema_rejects_empty_name_and_workflow() {
        let mut experience = probe_file_experience();
        experience.name = "   ".into();
        experience.workflow.clear();
        let issues = experience.schema_issues();
        assert!(issues.contains(&SchemaIssue::EmptyName));
        assert!(issues.contains(&SchemaIssue::EmptyWorkflow));
    }

    #[test]
    fn status_transition_matrix_enforces_validated_activation() {
        use super::QualificationAction as A;
        use super::ExperienceStatus as S;
        assert_eq!(
            S::Candidate.transition(A::Validate).unwrap(),
            S::Validated
        );
        assert_eq!(S::Validated.transition(A::Activate).unwrap(), S::Active);
        assert_eq!(
            S::Validated.transition(A::ForceActivate).unwrap(),
            S::Active
        );
        // No CANDIDATE -> ACTIVE channel, even with an override.
        assert!(S::Candidate.transition(A::Activate).is_err());
        assert!(S::Candidate.transition(A::ForceActivate).is_err());
        assert_eq!(
            S::Candidate.transition(A::Invalidate).unwrap(),
            S::Disabled
        );
        assert_eq!(
            S::Disabled.transition(A::Revalidate).unwrap(),
            S::Validated
        );
        assert_eq!(S::Active.transition(A::Decay).unwrap(), S::Decaying);
        assert!(S::Validated.transition(A::Decay).is_err());
        assert!(S::Candidate.transition(A::Decay).is_err());
        assert!(S::Candidate.transition(A::Revalidate).is_err());
        assert!(S::Active.transition(A::Validate).is_err());
    }

    #[test]
    fn postconditions_must_be_backed_by_real_facts() {
        let experience = probe_file_experience();
        let real = vec![
            StateFact::new("file:probe.txt.exists", serde_json::json!(true), "ls", 1),
            StateFact::new(
                "file:probe.txt.content",
                serde_json::json!("EXPERIENCE_GATE_SUCCESS"),
                "read probe.txt",
                1,
            ),
        ];
        assert!(experience.postconditions_satisfied_by(&real));

        // Missing/wrong evidence must NOT satisfy postconditions.
        let forged = vec![StateFact::new(
            "file:probe.txt.exists",
            serde_json::json!(true),
            "assumed",
            1,
        )];
        assert!(!experience.postconditions_satisfied_by(&forged));
    }
}

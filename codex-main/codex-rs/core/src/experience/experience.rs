//! Experience: the executable structure (Phase 8) and its lifecycle (Phase 9).
//!
//! An experience is NOT a memory text ("user liked monthly file organization").
//! It is a re-runnable program: `trigger` + `conditions` + `workflow`, carrying
//! its own confidence and risk, versioned and serializable.

use std::collections::HashMap;

use serde::Deserialize;
use serde::Serialize;

use super::experience_state::ExperienceId;
use super::experience_state::RiskLevel;

/// Experience kinds, mirroring the reference implementation. Execution kinds
/// (reflex/result) match strictly and may bypass the LLM; reference/process
/// kinds are semantic and may only assist or hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExperienceKind {
    Reflex,
    Process,
    Result,
    Reference,
}

/// Lifecycle of an experience (Phase 9):
/// NEW -> CANDIDATE -> VALIDATED -> ACTIVE -> DECAYING -> DISABLED.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ExperienceStatus {
    New,
    Candidate,
    Validated,
    Active,
    Decaying,
    Disabled,
}

/// Trigger: the situation in which this experience should be considered.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExperienceTrigger {
    pub object: Option<String>,
    pub location: Option<String>,
    pub goal: Option<String>,
    pub keywords: Vec<String>,
    pub context: HashMap<String, serde_json::Value>,
}

/// A condition over a state element; checked via `ExperienceState::satisfies`
/// without the LLM.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExperienceCondition {
    pub key: String,
    pub expected: serde_json::Value,
}

/// One executable step of a workflow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExperienceWorkflowStep {
    pub name: String,
    pub args: serde_json::Value,
}

/// Input scope of an experience (decision logic §14): what range of inputs
/// the workflow is known to handle without adaptation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputScope {
    Single,
    Batch,
    Any,
    /// Not declared yet — conservative default: never grants ①.
    Unknown,
}

/// Explicit applicability metadata (decision logic §14.6). Experiences
/// without it are treated conservatively (no ExperienceOnly) until the
/// experience-gen skill re-distills them with metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Applicability {
    pub input_scope: InputScope,
    pub applicable_objects: Vec<String>,
    /// Workflow uses parameter placeholders / assets scripts with arguments,
    /// so it can be adapted to a different input of the same scope.
    pub parameterized: bool,
}

impl Default for Applicability {
    fn default() -> Self {
        Self {
            input_scope: InputScope::Unknown,
            applicable_objects: Vec::new(),
            parameterized: false,
        }
    }
}

impl Applicability {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

/// The executable path of an experience.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExperienceWorkflow {
    pub steps: Vec<ExperienceWorkflowStep>,
}

/// An experience: past behavior compressed into a re-runnable program.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Experience {
    pub id: ExperienceId,
    pub name: String,
    pub kind: ExperienceKind,
    pub trigger: ExperienceTrigger,
    pub conditions: Vec<ExperienceCondition>,
    pub workflow: ExperienceWorkflow,
    /// How to know the task was actually completed (silence-audit candidate 3;
    /// sourced from the agent's final evidence during generation).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_criteria: Option<String>,
    /// Known ways this task class fails (verification walls, refused calls,
    /// API errors) — Reference material, not workflow steps.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failure_modes: Vec<String>,
    /// Self-contained material assets (script/template content) keyed by name;
    /// workflow steps reference them instead of deleted temp paths.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub assets: HashMap<String, String>,
    /// Explicit applicability for prior path decision (§14); default is
    /// conservative (Unknown scope, not parameterized).
    #[serde(default, skip_serializing_if = "Applicability::is_default")]
    pub applicability: Applicability,
    /// Short display title (user-renamable). `name` stays the source/task
    /// text; `title` is what the management UI shows by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// User note attached through the management channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Unix seconds when the experience was first created (best effort).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<u64>,
    pub confidence: f32,
    pub risk: RiskLevel,
    pub status: ExperienceStatus,
    pub version: u32,
}

/// Structural validation failure for an experience (Phase 8).
#[derive(Debug, Clone, PartialEq)]
pub enum ExperienceValidationError {
    EmptyName,
    /// No keywords / object / location / goal: the matcher can never hit it.
    EmptyTrigger,
    EmptyWorkflow,
    EmptyStepName(usize),
    ConfidenceOutOfRange(f32),
    InvalidVersion(u32),
}

impl std::fmt::Display for ExperienceValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExperienceValidationError::EmptyName => write!(f, "experience name is empty"),
            ExperienceValidationError::EmptyTrigger => {
                write!(f, "trigger has no keywords/object/location/goal")
            }
            ExperienceValidationError::EmptyWorkflow => {
                write!(f, "workflow has no executable steps")
            }
            ExperienceValidationError::EmptyStepName(index) => {
                write!(f, "workflow step {index} has an empty name")
            }
            ExperienceValidationError::ConfidenceOutOfRange(score) => {
                write!(f, "confidence {score} is outside [0, 1]")
            }
            ExperienceValidationError::InvalidVersion(version) => {
                write!(f, "version {version} must be >= 1")
            }
        }
    }
}

impl Experience {
    /// Compact, LLM-readable description of the experience's content and step
    /// decomposition.
    ///
    /// This materializes the LLM's right to question an experience (质疑权):
    /// the LLM is the conscious mind; it does not need to supervise every
    /// action, but it can always inspect what an experience claims and how it
    /// decomposes the task — especially when results do not meet expectations.
    pub fn reference_text(&self) -> String {
        let mut text = format!(
            "experience[{}] name={} kind={:?} status={:?} confidence={:.2} risk={:?}\n",
            self.id, self.name, self.kind, self.status, self.confidence, self.risk
        );
        text.push_str(&format!(
            "trigger: object={:?} location={:?} goal={:?} keywords={:?}\n",
            self.trigger.object, self.trigger.location, self.trigger.goal, self.trigger.keywords
        ));
        for condition in &self.conditions {
            text.push_str(&format!("condition: {} == {}\n", condition.key, condition.expected));
        }
        for (index, step) in self.workflow.steps.iter().enumerate() {
            text.push_str(&format!("step {}: {} {}\n", index + 1, step.name, step.args));
        }
        if let Some(criteria) = &self.completion_criteria {
            text.push_str(&format!("completion_criteria: {criteria}\n"));
        }
        for mode in &self.failure_modes {
            text.push_str(&format!("failure_mode: {mode}\n"));
        }
        for (name, _) in &self.assets {
            text.push_str(&format!("asset: {name}\n"));
        }
        text
    }

    /// Structural validation (Phase 8): an experience must be a re-runnable
    /// program — it needs a name, a trigger the matcher can hit, at least one
    /// executable step, a sane confidence, and a version >= 1.
    pub fn validate(&self) -> Result<(), ExperienceValidationError> {
        if self.name.trim().is_empty() {
            return Err(ExperienceValidationError::EmptyName);
        }
        let trigger_has_signal = !self.trigger.keywords.is_empty()
            || self
                .trigger
                .object
                .as_deref()
                .is_some_and(|value| !value.is_empty())
            || self
                .trigger
                .location
                .as_deref()
                .is_some_and(|value| !value.is_empty())
            || self
                .trigger
                .goal
                .as_deref()
                .is_some_and(|value| !value.is_empty());
        if !trigger_has_signal {
            return Err(ExperienceValidationError::EmptyTrigger);
        }
        if self.workflow.steps.is_empty() {
            return Err(ExperienceValidationError::EmptyWorkflow);
        }
        for (index, step) in self.workflow.steps.iter().enumerate() {
            if step.name.trim().is_empty() {
                return Err(ExperienceValidationError::EmptyStepName(index));
            }
        }
        if !(0.0..=1.0).contains(&self.confidence) {
            return Err(ExperienceValidationError::ConfidenceOutOfRange(self.confidence));
        }
        if self.version == 0 {
            return Err(ExperienceValidationError::InvalidVersion(self.version));
        }
        Ok(())
    }

    /// Bump the version when a corrected / updated experience is stored.
    pub fn bump_version(&mut self) {
        self.version += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn experience_serializes_as_executable_structure() {
        let exp = Experience {
            id: ExperienceId("exp_1".to_string()),
            name: "move_pdfs_to_archive".to_string(),
            kind: ExperienceKind::Reflex,
            trigger: ExperienceTrigger {
                object: Some("pdf".to_string()),
                location: Some("downloads".to_string()),
                goal: Some("archive".to_string()),
                keywords: vec!["pdf".to_string(), "archive".to_string()],
                context: HashMap::new(),
            },
            conditions: vec![ExperienceCondition {
                key: "environment.detection.downloads_exists".to_string(),
                expected: json!(true),
            }],
            workflow: ExperienceWorkflow {
                steps: vec![ExperienceWorkflowStep {
                    name: "scan_files".to_string(),
                    args: json!({ "location": "downloads" }),
                }],
            },
            completion_criteria: None,
            failure_modes: Vec::new(),
            assets: Default::default(),
            applicability: Default::default(),
            title: None,
            note: None,
            created_at: None,
            confidence: 0.94,
            risk: RiskLevel::Low,
            status: ExperienceStatus::Active,
            version: 1,
        };

        let encoded = serde_json::to_value(&exp).expect("serialize");
        let decoded: Experience = serde_json::from_value(encoded).expect("deserialize");
        assert_eq!(decoded.id, exp.id);
        assert_eq!(decoded.workflow.steps[0].name, "scan_files");
        assert_eq!(decoded.status, ExperienceStatus::Active);
    }

    #[test]
    fn completion_failure_assets_roundtrip_and_render() {
        let exp = Experience {
            id: ExperienceId("exp_meta".to_string()),
            name: "read creator dashboard".to_string(),
            title: Some("读创作者看板".to_string()),
            note: Some("演示备注".to_string()),
            created_at: Some(1234567890),
            kind: ExperienceKind::Process,
            trigger: ExperienceTrigger {
                keywords: vec!["曝光数".to_string(), "观看数".to_string()],
                ..Default::default()
            },
            conditions: Vec::new(),
            workflow: ExperienceWorkflow {
                steps: vec![ExperienceWorkflowStep {
                    name: "exec_command".to_string(),
                    args: json!({"cmd": "node read.mjs"}),
                }],
            },
            completion_criteria: Some("读到曝光数与观看数且账号匹配".to_string()),
            failure_modes: vec!["重启浏览器触发人机验证".to_string()],
            assets: [("read.mjs".to_string(), "console.log('x')".to_string())]
                .into_iter()
                .collect(),
            applicability: Default::default(),
            confidence: 0.9,
            risk: RiskLevel::Low,
            status: ExperienceStatus::Active,
            version: 1,
        };
        assert_eq!(exp.validate(), Ok(()));
        let text = exp.reference_text();
        assert!(text.contains("completion_criteria: 读到曝光数与观看数且账号匹配"));
        assert!(text.contains("failure_mode: 重启浏览器触发人机验证"));
        assert!(text.contains("asset: read.mjs"));
        let json = serde_json::to_string(&exp).expect("serialize");
        let decoded: Experience = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded.completion_criteria, exp.completion_criteria);
        assert_eq!(decoded.failure_modes, exp.failure_modes);
        assert_eq!(decoded.assets, exp.assets);
        assert_eq!(decoded.title, exp.title);
        assert_eq!(decoded.note, exp.note);
        assert_eq!(decoded.created_at, exp.created_at);
    }

    #[test]
    fn reference_text_exposes_content_for_llm_questioning() {
        let exp = Experience {
            id: ExperienceId("exp_1".to_string()),
            name: "move_pdfs_to_archive".to_string(),
            kind: ExperienceKind::Reflex,
            trigger: ExperienceTrigger {
                keywords: vec!["pdf".to_string(), "archive".to_string()],
                ..Default::default()
            },
            conditions: Vec::new(),
            workflow: ExperienceWorkflow {
                steps: vec![
                    ExperienceWorkflowStep {
                        name: "scan_files".to_string(),
                        args: json!({ "location": "downloads" }),
                    },
                    ExperienceWorkflowStep {
                        name: "move_file".to_string(),
                        args: json!({ "target": "archive" }),
                    },
                ],
            },
            completion_criteria: None,
            failure_modes: Vec::new(),
            assets: Default::default(),
            applicability: Default::default(),
            title: None,
            note: None,
            created_at: None,
            confidence: 0.94,
            risk: RiskLevel::Low,
            status: ExperienceStatus::Active,
            version: 1,
        };
        let text = exp.reference_text();
        assert!(text.contains("move_pdfs_to_archive"));
        assert!(text.contains("step 1: scan_files"));
        assert!(text.contains("step 2: move_file"));
        assert!(text.contains("keywords=[\"pdf\", \"archive\"]"));
    }

    #[test]
    fn validation_accepts_wellformed_experience() {
        let exp = Experience {
            id: ExperienceId("exp_1".to_string()),
            name: "move_pdfs_to_archive".to_string(),
            kind: ExperienceKind::Reflex,
            trigger: ExperienceTrigger {
                keywords: vec!["pdf".to_string()],
                ..Default::default()
            },
            conditions: Vec::new(),
            workflow: ExperienceWorkflow {
                steps: vec![ExperienceWorkflowStep {
                    name: "scan_files".to_string(),
                    args: json!({}),
                }],
            },
            completion_criteria: None,
            failure_modes: Vec::new(),
            assets: Default::default(),
            applicability: Default::default(),
            title: None,
            note: None,
            created_at: None,
            confidence: 0.9,
            risk: RiskLevel::Low,
            status: ExperienceStatus::Active,
            version: 1,
        };
        assert_eq!(exp.validate(), Ok(()));
    }

    #[test]
    fn validation_rejects_broken_experiences() {
        let base = Experience {
            id: ExperienceId("exp_1".to_string()),
            name: "move_pdfs".to_string(),
            kind: ExperienceKind::Reflex,
            trigger: ExperienceTrigger {
                keywords: vec!["pdf".to_string()],
                ..Default::default()
            },
            conditions: Vec::new(),
            workflow: ExperienceWorkflow {
                steps: vec![ExperienceWorkflowStep {
                    name: "scan_files".to_string(),
                    args: json!({}),
                }],
            },
            completion_criteria: None,
            failure_modes: Vec::new(),
            assets: Default::default(),
            applicability: Default::default(),
            title: None,
            note: None,
            created_at: None,
            confidence: 0.9,
            risk: RiskLevel::Low,
            status: ExperienceStatus::Active,
            version: 1,
        };

        let mut empty_trigger = base.clone();
        empty_trigger.trigger = ExperienceTrigger::default();
        assert_eq!(
            empty_trigger.validate(),
            Err(ExperienceValidationError::EmptyTrigger)
        );

        let mut empty_workflow = base.clone();
        empty_workflow.workflow = ExperienceWorkflow::default();
        assert_eq!(
            empty_workflow.validate(),
            Err(ExperienceValidationError::EmptyWorkflow)
        );

        let mut bad_confidence = base.clone();
        bad_confidence.confidence = 1.5;
        assert_eq!(
            bad_confidence.validate(),
            Err(ExperienceValidationError::ConfidenceOutOfRange(1.5))
        );

        let mut bad_version = base.clone();
        bad_version.version = 0;
        assert_eq!(
            bad_version.validate(),
            Err(ExperienceValidationError::InvalidVersion(0))
        );
    }

    #[test]
    fn bump_version_increments() {
        let mut exp = Experience {
            id: ExperienceId("exp_1".to_string()),
            name: "n".to_string(),
            kind: ExperienceKind::Reflex,
            trigger: ExperienceTrigger {
                keywords: vec!["pdf".to_string()],
                ..Default::default()
            },
            conditions: Vec::new(),
            workflow: ExperienceWorkflow {
                steps: vec![ExperienceWorkflowStep {
                    name: "s".to_string(),
                    args: json!({}),
                }],
            },
            completion_criteria: None,
            failure_modes: Vec::new(),
            assets: Default::default(),
            applicability: Default::default(),
            title: None,
            note: None,
            created_at: None,
            confidence: 0.9,
            risk: RiskLevel::Low,
            status: ExperienceStatus::Active,
            version: 1,
        };
        exp.bump_version();
        assert_eq!(exp.version, 2);
    }
}

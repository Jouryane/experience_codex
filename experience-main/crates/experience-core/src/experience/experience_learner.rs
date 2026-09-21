//! Phase 7: the LLM as learner — turning successful LLM execution traces into
//! Experience candidates.
//!
//! First encounter: MISS → LLM → reasoning → tool calls → success. The learner
//! detects repeatability (consecutive similar successes, or explicit user
//! confirmation) and compiles the trace into an `ExperienceDraft` (status
//! CANDIDATE). Validation / promotion to ACTIVE belongs to the Phase 9 state
//! machine.

use std::collections::HashMap;

use serde::Deserialize;
use serde::Serialize;

use super::experience::Experience;
use super::experience::ExperienceCondition;
use super::experience::ExperienceKind;
use super::experience::ExperienceStatus;
use super::experience::ExperienceTrigger;
use super::experience::ExperienceWorkflow;
use super::experience::ExperienceWorkflowStep;
use super::experience_matcher::tokenize;
use super::experience_state::ExperienceId;
use super::experience_state::RiskLevel;

/// One tool/action invocation captured from an LLM trace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceAction {
    pub name: String,
    pub args: serde_json::Value,
}

/// One step of a captured process, with its observed outcome. This is the
/// "how the task was actually done" record the silence audit found missing:
/// per-step success, the tool's output summary and the correlation id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceStep {
    pub name: String,
    pub args: serde_json::Value,
    pub call_id: Option<String>,
    /// `Some(true)`/`Some(false)` once the tool result was observed;
    /// `None` when the output never arrived (aborted / not correlated).
    pub ok: Option<bool>,
    /// Truncated tool output text (the evidence of what this step did).
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TraceOutcome {
    Success,
    Failure,
}

/// A captured LLM execution trace: what the task was, what was done, and how
/// it ended. `actions` is the flat ordered list used by the workflow compiler;
/// `steps` carries the per-step outcome detail; `environment_snapshot` and
/// `final_evidence` record the context in which the task succeeded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExperienceTrace {
    pub task: String,
    pub actions: Vec<TraceAction>,
    #[serde(default)]
    pub steps: Vec<TraceStep>,
    #[serde(default)]
    pub environment_snapshot: Option<serde_json::Value>,
    #[serde(default)]
    pub final_evidence: Option<String>,
    pub outcome: TraceOutcome,
    pub llm_used: bool,
}

/// Collects tool-call actions from a turn's sampling requests (M2-1).
///
/// Read-only: recording must never change the execution path. The original
/// Codex loop keeps its exact behavior; this is a passive observer.
#[derive(Debug, Clone, Default)]
pub struct TraceRecorder {
    actions: Vec<TraceAction>,
    steps: Vec<TraceStep>,
    /// call_id → index into `steps` (and `actions`, they stay parallel).
    pending: HashMap<String, usize>,
}

impl TraceRecorder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one function call without a correlation id (tests / fallback).
    pub fn record(&mut self, name: &str, args: serde_json::Value) {
        self.record_call(name, args, None);
    }

    /// Record one function call (name + parsed arguments + optional call id).
    /// Empty names are ignored.
    pub fn record_call(&mut self, name: &str, args: serde_json::Value, call_id: Option<String>) {
        if !name.is_empty() {
            self.actions.push(TraceAction {
                name: name.to_string(),
                args,
            });
            let index = self.steps.len();
            self.steps.push(TraceStep {
                name: name.to_string(),
                args: serde_json::Value::Null,
                call_id: call_id.clone(),
                ok: None,
                summary: None,
            });
            // Keep step args in sync with the recorded action.
            let last = self.steps.last_mut().expect("just pushed");
            last.args = self.actions[index].args.clone();
            if let Some(call_id) = call_id {
                self.pending.insert(call_id, index);
            }
        }
    }

    /// Mark a previously recorded call with its observed outcome.
    pub fn record_outcome(&mut self, call_id: &str, ok: bool, summary: Option<String>) {
        if let Some(index) = self.pending.get(call_id).copied() {
            if let Some(step) = self.steps.get_mut(index) {
                step.ok = Some(ok);
                if summary.is_some() {
                    step.summary = summary;
                }
            }
        }
    }

    pub fn actions(&self) -> &[TraceAction] {
        &self.actions
    }

    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }

    pub fn steps(&self) -> &[TraceStep] {
        &self.steps
    }

    pub fn clear(&mut self) {
        self.actions.clear();
        self.steps.clear();
        self.pending.clear();
    }

    /// Snapshot the recorded actions as a trace.
    pub fn into_trace(
        &self,
        task: String,
        outcome: TraceOutcome,
        llm_used: bool,
    ) -> ExperienceTrace {
        self.into_trace_with_context(task, outcome, llm_used, None, None)
    }

    /// Snapshot the recorded process as a trace, including the environment
    /// snapshot and the final evidence observed by the agent.
    pub fn into_trace_with_context(
        &self,
        task: String,
        outcome: TraceOutcome,
        llm_used: bool,
        environment_snapshot: Option<serde_json::Value>,
        final_evidence: Option<String>,
    ) -> ExperienceTrace {
        ExperienceTrace {
            task,
            actions: self.actions.clone(),
            steps: self.steps.clone(),
            environment_snapshot,
            final_evidence,
            outcome,
            llm_used,
        }
    }
}

/// Derive reusable environment preconditions from a learning snapshot.
///
/// This closes the state↔experience loop (silence-audit candidate 2): the
/// conditions an experience carries are no longer left empty or invented by
/// the distiller — they are distilled from the environment the task actually
/// succeeded in. Only *generic, reusable* capabilities are promoted (e.g.
/// subprocesses allowed, OS/git presence); volatile values (cwd, URLs) are
/// deliberately excluded so the experience is not over-bound to one run.
pub(crate) fn conditions_from_snapshot(
    snapshot: Option<&serde_json::Value>,
) -> Vec<ExperienceCondition> {
    let Some(snapshot) = snapshot else {
        return Vec::new();
    };
    let mut conditions = Vec::new();

    if let Some(elements) = snapshot.get("elements").and_then(|value| value.as_array()) {
        for element in elements {
            let Some(key) = element.get("key").and_then(|value| value.as_str()) else {
                continue;
            };
            if key != "runtime.subprocess_allowed" {
                continue;
            }
            let Some(value) = element.get("value") else {
                continue;
            };
            conditions.push(ExperienceCondition {
                key: key.to_string(),
                expected: value.clone(),
            });
        }
    }

    if let Some(detections) = snapshot
        .get("environment")
        .and_then(|value| value.get("detections"))
        .and_then(|value| value.as_array())
    {
        for detection in detections {
            let Some(key) = detection.get("key").and_then(|value| value.as_str()) else {
                continue;
            };
            if !matches!(key, "os" | "git_repo") {
                continue;
            }
            let present = detection
                .get("present")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            if present {
                conditions.push(ExperienceCondition {
                    key: format!("environment.detection.{key}"),
                    expected: serde_json::Value::Bool(true),
                });
            }
        }
    }

    conditions
}

/// Merge extra conditions into `base`, keeping each (key, expected) once.
pub(crate) fn merge_conditions(
    base: &mut Vec<ExperienceCondition>,
    extra: Vec<ExperienceCondition>,
) {
    for condition in extra {
        if !base.contains(&condition) {
            base.push(condition);
        }
    }
}

/// A compiled experience waiting for validation (usually status CANDIDATE;
/// user-confirmed drafts enter directly as VALIDATED).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExperienceDraft {
    pub experience: Experience,
    pub source_trace_task: String,
    pub created_at: String,
}

/// Initial confidence of a candidate (raw, not yet validated).
pub(crate) const INITIAL_CANDIDATE_CONFIDENCE: f32 = 0.20;

/// Confidence of a user-confirmed draft (admitted directly as VALIDATED).
pub(crate) const USER_CONFIRMED_CONFIDENCE: f32 = 0.90;

/// Messy runs (long fumbling traces) must NOT be auto-confirmed into VALIDATED
/// experiences — they would freeze the waste. Above this step count a
/// user-confirmed trace still lands as CANDIDATE for review/distillation.
pub(crate) const MAX_AUTO_CONFIRM_STEPS: usize = 20;

/// Minimal English stopwords filtered from trigger keywords (the garbled
/// "??" tokens came from encoding; stopwords remove low-signal words).
const KEYWORD_STOPWORDS: &[&str] = &[
    "the", "a", "an", "and", "or", "to", "of", "in", "on", "for", "with",
    "you", "your", "did", "do", "is", "are", "please", "then", "this",
    "that", "task", "file", "files", "png", "move", "moved", "using",
    "from", "into", "folder", "reply", "one", "sentence", "what", "done",
];

/// Deterministic prune markers: tool calls that are environment probing,
/// package installation, or inspection noise — never part of the minimal
/// success path of a repeatable behavior.
const PRUNE_MARKERS: &[&str] = &[
    "pip install",
    "pip list",
    "pip show",
    "python --version",
    "python -V",
    "Get-Process",
    "Get-Command",
    "Get-CimInstance",
    "Get-ItemProperty",
    "Test-Path",
    "Get-ChildItem",
    "Get-Content",
    "where.exe",
];

/// Repeatability evidence accumulated per task signature.
#[derive(Debug, Clone, Default)]
pub(crate) struct Evidence {
    pub(crate) successes: u32,
    pub(crate) failures: u32,
    pub(crate) last_trace: Option<ExperienceTrace>,
}

/// Detects repeatable behavior and compiles traces into drafts.
#[derive(Debug, Clone)]
pub struct ExperienceLearner {
    min_successes: u32,
    evidence: HashMap<String, Evidence>,
}

impl Default for ExperienceLearner {
    fn default() -> Self {
        Self::new(2)
    }
}

impl ExperienceLearner {
    pub fn new(min_successes: u32) -> Self {
        Self {
            min_successes: min_successes.max(1),
            evidence: HashMap::new(),
        }
    }

    /// Observe a trace outcome; returns a draft when the repeatability
    /// threshold is met or the user explicitly confirmed the behavior.
    pub fn observe(
        &mut self,
        trace: &ExperienceTrace,
        user_confirmed: bool,
    ) -> Option<ExperienceDraft> {
        let signature = signature_of(trace);
        let candidate_id =
            ExperienceId(format!("cand-{:016x}", fnv1a(signature.as_bytes())));
        let evidence = self.evidence.entry(signature).or_default();
        match trace.outcome {
            TraceOutcome::Success => {
                // Necessity gate (learner side): a trace whose steps were all
                // observed as failures/unknown is a "fake success" (the model
                // declared success without a successful tool run). Freezing it
                // would only teach the agent how to fail. Traces without
                // per-step detail (legacy capture / tests) are admitted to
                // keep old behavior.
                if !trace_has_success_evidence(trace) {
                    return None;
                }
                evidence.successes += 1;
                evidence.failures = 0;
                evidence.last_trace = Some(trace.clone());
                let actions = prune_actions(&trace.actions);
                if user_confirmed || evidence.successes >= self.min_successes {
                    if actions.is_empty() {
                        // Nothing executable to compile: not a repeatable
                        // behavior program.
                        None
                    } else {
                        // Quality gate: messy traces (many steps) are not
                        // auto-confirmed; they stay CANDIDATE for review.
                        let confirmed = user_confirmed && actions.len() <= MAX_AUTO_CONFIRM_STEPS;
                        let mut pruned_trace = trace.clone();
                        pruned_trace.actions = actions;
                        Some(build_draft(&pruned_trace, candidate_id, confirmed))
                    }
                } else {
                    None
                }
            }
            TraceOutcome::Failure => {
                evidence.failures += 1;
                evidence.successes = 0;
                evidence.last_trace = Some(trace.clone());
                None
            }
        }
    }

    pub(crate) fn evidence(&self) -> &HashMap<String, Evidence> {
        &self.evidence
    }

}

/// FNV-1a 64-bit — deterministic, platform-independent id source.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Remove probing / installation / inspection noise and consecutive duplicate
/// actions from a successful trace. This is the deterministic half of
/// experience distillation (the LLM half compresses what remains).
fn prune_actions(actions: &[TraceAction]) -> Vec<TraceAction> {
    let mut pruned: Vec<TraceAction> = Vec::new();
    let mut last: Option<String> = None;
    for action in actions {
        let is_noise = action.name == "exec_command"
            && action
                .args
                .get("cmd")
                .and_then(|v| v.as_str())
                .is_some_and(|cmd| PRUNE_MARKERS.iter().any(|m| cmd.contains(m)));
        if is_noise {
            continue;
        }
        // Drop consecutive duplicates (retry loops).
        let key = format!("{}:{}", action.name, action.args);
        if last.as_deref() == Some(key.as_str()) {
            continue;
        }
        last = Some(key);
        pruned.push(action.clone());
    }
    pruned
}

/// A repeatability signature: the task text, normalized.
fn signature_of(trace: &ExperienceTrace) -> String {
    tokenize(&trace.task).join(" ")
}

/// Compile a successful trace into an ExperienceDraft. User-confirmed traces
/// are admitted as VALIDATED with high confidence; others stay CANDIDATE.
fn build_draft(
    trace: &ExperienceTrace,
    id: ExperienceId,
    user_confirmed: bool,
) -> ExperienceDraft {
    let mut seen = std::collections::HashSet::new();
    let keywords: Vec<String> = tokenize(&trace.task)
        .into_iter()
        .filter(|token| {
            !KEYWORD_STOPWORDS.contains(&token.as_str()) && seen.insert(token.clone())
        })
        .collect();
    let kind = if trace.actions.len() <= 1 {
        ExperienceKind::Reflex
    } else {
        ExperienceKind::Process
    };
    let (status, confidence) = if user_confirmed {
        (ExperienceStatus::Validated, USER_CONFIRMED_CONFIDENCE)
    } else {
        (ExperienceStatus::Candidate, INITIAL_CANDIDATE_CONFIDENCE)
    };
    let workflow = ExperienceWorkflow {
        steps: trace
            .actions
            .iter()
            .map(|action| ExperienceWorkflowStep {
                name: action.name.clone(),
                args: action.args.clone(),
            })
            .collect(),
    };
    ExperienceDraft {
        experience: Experience {
            id,
            name: trace.task.clone(),
            kind,
            trigger: ExperienceTrigger {
                keywords,
                ..Default::default()
            },
            conditions: Vec::new(),
            workflow,
            completion_criteria: trace.final_evidence.clone(),
            failure_modes: Vec::new(),
            assets: HashMap::new(),
            applicability: Default::default(),
            title: None,
            note: None,
            created_at: Some(now_secs()),
            confidence,
            risk: RiskLevel::Low,
            status,
            version: 1,
        },
        source_trace_task: trace.task.clone(),
        created_at: now_string(),
    }
}

/// Necessity gate (learner side): does this trace contain at least one
/// observed-successful step?
pub(crate) fn trace_has_success_evidence(trace: &ExperienceTrace) -> bool {
    if trace.steps.is_empty() {
        // No per-step detail captured (legacy / tests): cannot judge, keep
        // the previous permissive behavior.
        return true;
    }
    trace.steps.iter().any(|step| step.ok == Some(true))
}

fn now_string() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn trace(task: &str, outcome: TraceOutcome) -> ExperienceTrace {
        ExperienceTrace {
            task: task.to_string(),
            actions: vec![
                TraceAction {
                    name: "tool.shell".to_string(),
                    args: json!({ "command": "ls" }),
                },
                TraceAction {
                    name: "deploy.env".to_string(),
                    args: json!({ "key": "k", "value": "v" }),
                },
            ],
            steps: Vec::new(),
            environment_snapshot: None,
            final_evidence: None,
            outcome,
            llm_used: true,
        }
    }

    #[test]
    fn single_success_below_threshold_returns_none() {
        let mut learner = ExperienceLearner::new(2);
        assert!(learner.observe(&trace("move pdfs", TraceOutcome::Success), false).is_none());
    }

    #[test]
    fn consecutive_similar_successes_produce_draft() {
        let mut learner = ExperienceLearner::new(2);
        learner.observe(&trace("move pdfs to archive", TraceOutcome::Success), false);
        let draft = learner.observe(&trace("move pdfs to archive", TraceOutcome::Success), false);
        assert!(draft.is_some());
        let draft = draft.unwrap();
        assert_eq!(draft.experience.status, ExperienceStatus::Candidate);
        assert_eq!(draft.experience.workflow.steps.len(), 2);
        assert_eq!(draft.experience.workflow.steps[0].name, "tool.shell");
        assert_eq!(draft.experience.confidence, INITIAL_CANDIDATE_CONFIDENCE);
    }

    #[test]
    fn user_confirmation_immediately_drafts() {
        let mut learner = ExperienceLearner::new(3);
        let draft = learner
            .observe(&trace("archive downloads", TraceOutcome::Success), true)
            .expect("user-confirmed trace drafts immediately");
        assert_eq!(draft.experience.status, ExperienceStatus::Validated);
        assert_eq!(draft.experience.confidence, USER_CONFIRMED_CONFIDENCE);
    }

    #[test]
    fn recorder_correlates_call_outcomes_and_builds_process_trace() {
        let mut recorder = TraceRecorder::new();
        recorder.record_call(
            "exec_command",
            json!({ "cmd": "Get-Process chrome" }),
            Some("c1".to_string()),
        );
        recorder.record_call(
            "exec_command",
            json!({ "cmd": "Move-Item a b" }),
            Some("c2".to_string()),
        );
        recorder.record_outcome("c1", false, Some("process listing too noisy".to_string()));
        recorder.record_outcome("c2", true, Some("moved 1 file".to_string()));

        let trace = recorder.into_trace_with_context(
            "move png to pic".to_string(),
            TraceOutcome::Success,
            true,
            Some(json!({ "cwd": "D:\\work", "git": true })),
            Some("moved 首页1.png into pic".to_string()),
        );
        assert_eq!(trace.steps.len(), 2);
        assert_eq!(trace.steps[0].call_id.as_deref(), Some("c1"));
        assert_eq!(trace.steps[0].ok, Some(false));
        assert_eq!(trace.steps[1].ok, Some(true));
        assert_eq!(trace.steps[1].summary.as_deref(), Some("moved 1 file"));
        assert_eq!(trace.actions.len(), 2);
        assert_eq!(trace.environment_snapshot.as_ref().unwrap()["cwd"], "D:\\work");
        assert_eq!(trace.final_evidence.as_deref(), Some("moved 首页1.png into pic"));
    }

    #[test]
    fn conditions_derive_from_snapshot_and_merge_dedupe() {
        let snapshot = json!({
            "elements": [
                {"key": "runtime.subprocess_allowed", "value": true},
                {"key": "env.browser.logged_in", "value": true},
            ],
            "environment": {
                "detections": [
                    {"key": "os", "present": true},
                    {"key": "git_repo", "present": true},
                ]
            },
            "executed_steps": ["a", "b"],
        });
        let mut conditions = conditions_from_snapshot(Some(&snapshot));
        assert!(conditions.contains(&ExperienceCondition {
            key: "runtime.subprocess_allowed".to_string(),
            expected: json!(true),
        }));
        // Non-whitelisted element keys are NOT promoted (would over-bind).
        assert!(!conditions.iter().any(|c| c.key == "env.browser.logged_in"));
        assert!(conditions.contains(&ExperienceCondition {
            key: "environment.detection.os".to_string(),
            expected: json!(true),
        }));
        assert!(conditions.contains(&ExperienceCondition {
            key: "environment.detection.git_repo".to_string(),
            expected: json!(true),
        }));
        let extra = conditions.clone();
        merge_conditions(&mut conditions, extra);
        assert_eq!(conditions.len(), 3);
        assert!(conditions_from_snapshot(None).is_empty());
    }

    #[test]
    fn fake_success_without_successful_steps_is_refused() {
        let mut learner = ExperienceLearner::new(1);
        let mut fake = trace("download stock data", TraceOutcome::Success);
        fake.steps = vec![
            TraceStep {
                name: "exec_command".to_string(),
                args: json!({"cmd": "pip install akshare"}),
                call_id: Some("c1".to_string()),
                ok: Some(false),
                summary: Some("blocked".to_string()),
            },
            TraceStep {
                name: "exec_command".to_string(),
                args: json!({"cmd": "python fetch.py"}),
                call_id: Some("c2".to_string()),
                ok: Some(false),
                summary: Some("crashed".to_string()),
            },
        ];
        assert!(learner.observe(&fake, true).is_none());
        // Legacy trace without per-step detail is still admitted.
        assert!(
            learner
                .observe(&trace("legacy task", TraceOutcome::Success), true)
                .is_some()
        );
    }

    #[test]
    fn ids_are_stable_across_instances_and_distinct_per_task() {
        let mut a = ExperienceLearner::new(1);
        let mut b = ExperienceLearner::new(1);
        let da = a.observe(&trace("move png to pic", TraceOutcome::Success), true).unwrap();
        let db = b.observe(&trace("move png to pic", TraceOutcome::Success), true).unwrap();
        assert_eq!(da.experience.id, db.experience.id);
        let mut c = ExperienceLearner::new(1);
        let dc = c.observe(&trace("open creator center", TraceOutcome::Success), true).unwrap();
        assert_ne!(da.experience.id, dc.experience.id);
    }

    #[test]
    fn messy_trace_is_not_auto_confirmed() {
        let mut learner = ExperienceLearner::new(1);
        let mut messy = trace("open creator center", TraceOutcome::Success);
        messy.actions = (0..=MAX_AUTO_CONFIRM_STEPS)
            .map(|i| TraceAction {
                name: format!("step_{i}"),
                args: serde_json::json!({}),
            })
            .collect();
        let draft = learner.observe(&messy, true).expect("still drafts");
        assert_eq!(draft.experience.status, ExperienceStatus::Candidate);
    }

    #[test]
    fn prune_drops_probes_installs_and_duplicates() {
        let actions = vec![
            TraceAction { name: "exec_command".into(), args: json!({"cmd":"python --version"}) },
            TraceAction { name: "exec_command".into(), args: json!({"cmd":"pip install pywinauto"}) },
            TraceAction { name: "exec_command".into(), args: json!({"cmd":"Get-Process chrome"}) },
            TraceAction { name: "exec_command".into(), args: json!({"cmd":"Move-Item a b"}) },
            TraceAction { name: "exec_command".into(), args: json!({"cmd":"Move-Item a b"}) },
        ];
        let pruned = prune_actions(&actions);
        assert_eq!(pruned.len(), 1);
        assert!(pruned[0].args["cmd"].as_str().unwrap().contains("Move-Item"));
    }

    #[test]
    fn failure_resets_evidence() {
        let mut learner = ExperienceLearner::new(2);
        learner.observe(&trace("move pdfs", TraceOutcome::Success), false);
        learner.observe(&trace("move pdfs", TraceOutcome::Failure), false);
        assert!(learner.observe(&trace("move pdfs", TraceOutcome::Success), false).is_none());
    }

    #[test]
    fn task_signatures_are_independent() {
        let mut learner = ExperienceLearner::new(2);
        learner.observe(&trace("move pdfs", TraceOutcome::Success), false);
        learner.observe(&trace("review project", TraceOutcome::Success), false);
        learner.observe(&trace("review project", TraceOutcome::Success), false);
        let draft = learner.observe(&trace("move pdfs", TraceOutcome::Success), false);
        assert!(draft.is_some(), "move-pdfs reached threshold on its 2nd success");
    }

    #[test]
    fn single_action_trace_is_reflex() {
        let mut learner = ExperienceLearner::new(1);
        let mut one_step = trace("check git repo", TraceOutcome::Success);
        one_step.actions.truncate(1);
        let draft = learner.observe(&one_step, false).expect("draft");
        assert_eq!(draft.experience.kind, ExperienceKind::Reflex);
    }

    #[test]
    fn empty_action_trace_is_not_compiled() {
        let mut learner = ExperienceLearner::new(1);
        let mut no_actions = trace("think about it", TraceOutcome::Success);
        no_actions.actions.clear();
        assert!(learner.observe(&no_actions, true).is_none());
    }

    #[test]
    fn trace_recorder_collects_and_snapshots() {
        let mut recorder = TraceRecorder::new();
        assert!(recorder.is_empty());
        recorder.record("tool.shell", json!({ "command": "ls" }));
        recorder.record("", json!({})); // empty name ignored
        assert_eq!(recorder.actions().len(), 1);

        let trace = recorder.into_trace("move pdfs".to_string(), TraceOutcome::Success, true);
        assert_eq!(trace.actions[0].name, "tool.shell");
        assert_eq!(trace.task, "move pdfs");
        assert!(trace.llm_used);
    }
}

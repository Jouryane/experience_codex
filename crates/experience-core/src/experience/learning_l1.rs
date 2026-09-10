//! L1 learning sink (docs/l1-learning-design.md, v2 rulings).
//!
//! Power boundary is structural: the L1 pipeline may only ever produce
//! `ExperienceStatus::Candidate` through a `CandidateWriter` that exposes no
//! ACTIVATE/qualification capability. Activation belongs to L2.
//!
//! Recorded L1-internal decisions (2026-09-09):
//! - candidate storage = the P1 domain store; only ACTIVE experiences enter
//!   the matching index, so candidates are storable but inert by status;
//! - deterministic parameter capture is deferred: typed trace v1 tool_call
//!   carries no args summary, so building honest parameterized workflow steps
//!   needs either an optional args summary in the trace or L2 workspace
//!   evidence. The necessity gate and writer land first; distill wiring
//!   follows that decision.

use crate::domain::action::ActionPattern;
use crate::domain::capability::canonical_tool;
use crate::domain::capability::CANONICAL_TOOLS;
use crate::domain::experience::Experience;
use crate::domain::experience::ExperienceStatus;
use crate::domain::experience::FailurePolicy;
use crate::domain::experience::UndoPolicy;
use crate::domain::experience::WorkflowStep;
use crate::domain::predicate::Predicate;
use crate::experience::trace_event::TraceEvent;
use crate::experience::trace_reader::RoundTrace;
use crate::store::ExperienceStore;

/// Threshold: a round with at most this many tool calls is "clean short" and
/// can skip LLM distillation.
pub const MAX_CLEAN_TOOL_STEPS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    TerminalNotSuccess,
    NoActionEvidence,
    RepeatNoImprovement,
    DistillUnavailable,
    ToolBoundaryInvalid,
    UnstableShape,
}

impl RejectReason {
    pub fn label(&self) -> &'static str {
        match self {
            RejectReason::TerminalNotSuccess => "terminal_not_success",
            RejectReason::NoActionEvidence => "no_action_evidence",
            RejectReason::RepeatNoImprovement => "repeat_no_improvement",
            RejectReason::DistillUnavailable => "distill_unavailable",
            RejectReason::ToolBoundaryInvalid => "tool_boundary_invalid",
            RejectReason::UnstableShape => "unstable_shape",
        }
    }
}

/// Conservative ephemeral-identifier heuristic (NOT a randomness verdict):
/// long hex-like tokens (commit hashes, uuids, sha256...) are refused as
/// `likely_ephemeral_identifier`. Business parameters that merely look like
/// hashes are also conservatively refused at L1/L2 - this is a rejection
/// rule, never an `invalid` assertion.
pub fn likely_ephemeral_identifier(text: &str) -> bool {
    text.split(|character: char| {
        !(character.is_ascii_alphanumeric() || character == '-' || character == '_')
    })
    .any(|token| {
        token.len() >= 16
            && token
                .chars()
                .all(|character| character.is_ascii_hexdigit() || character == '-')
    })
}

fn round_has_ephemeral_identifier(round: &RoundTrace) -> bool {
    round.events.iter().any(|event| match event {
        TraceEvent::ToolCall {
            args_summary: Some(summary),
            ..
        } => likely_ephemeral_identifier(summary),
        _ => false,
    })
}

/// Stable candidate name: lowercase signature with whitespace normalized to
/// underscores so names stay filesystem/URL/index friendly.
pub fn candidate_name(task: &str) -> String {
    let signature = task_signature(task).replace(' ', "_");
    format!("cand_{signature}")
}

/// Validate gate 2: every workflow step resolves to a known tool with a
/// non-empty argument object. Returns the offending action on failure.
pub fn validate_tool_boundary(candidate: &Experience) -> Result<(), String> {
    for step in &candidate.workflow {
        if !CANONICAL_TOOLS.contains(&step.action.as_str()) {
            return Err(format!("unknown tool '{}'", step.action));
        }
        if !step.args.is_object() || step.args.as_object().is_some_and(|map| map.is_empty()) {
            return Err(format!("tool '{}' has no usable arguments", step.action));
        }
    }
    Ok(())
}

/// L1 distiller contract: a clean/short round becomes a schema-valid draft.
pub trait L1Distiller {
    fn distill(&self, round: &RoundTrace) -> Option<Experience>;
    /// Dirty rounds (> clean threshold) are refused unless the distiller
    /// explicitly accepts them (LLM compiler); deterministic fallback does
    /// not (Stage S3).
    fn accepts_dirty(&self) -> bool {
        false
    }
}

/// Deterministic fallback: parameterized tools (e.g. exec_command) are only
/// distilled when the trace carries an args_summary; anything else is refused
/// honestly (no fabricated parameters). LLM compiler wiring lands separately.
pub struct DeterministicDistiller;

impl L1Distiller for DeterministicDistiller {
    fn distill(&self, round: &RoundTrace) -> Option<Experience> {
        let mut workflow = Vec::new();
        let mut trigger_tool = None;
        for event in &round.events {
            let TraceEvent::ToolCall { name, args_summary, .. } = event else {
                continue;
            };
            let tool = canonical_tool(name)?;
            // Seam 3 (L2 review): write_file/read_file need content/path
            // argument shapes that the current trace cannot provide; only
            // exec_command candidates can be honestly parameterized today.
            if tool != "exec_command" {
                return None;
            }
            let summary = args_summary.as_deref()?;
            workflow.push(WorkflowStep::new(
                tool.clone(),
                serde_json::json!({ "cmd": summary }),
            ));
            if trigger_tool.is_none() {
                trigger_tool = Some(tool);
            }
        }
        let tool = trigger_tool?;
        Some(Experience {
            name: candidate_name(&round.task),
            trigger: ActionPattern {
                tool,
                command_pattern: None,
            },
            preconditions: vec![Predicate::new("cwd.exists", serde_json::json!(true))],
            workflow,
            // Hypotheses for L2 qualification, never completion claims.
            postconditions: vec![Predicate::new(
                "candidate.pending_validation",
                serde_json::json!(true),
            )],
            verification: Vec::new(),
            failure_policy: FailurePolicy::StopAndReport,
            undo: UndoPolicy::Unsupported,
            status: ExperienceStatus::Draft,
        })
    }
}

/// One-shot L1 sink: necessity -> distill -> CandidateWriter. Returns the
/// rejection reason label when nothing is learned, or None when a candidate
/// was written.
pub fn sink_once(
    round: &RoundTrace,
    terminal_success: bool,
    existing_same_signature: bool,
    store: &mut ExperienceStore,
    distiller: &dyn L1Distiller,
) -> Result<Option<&'static str>, String> {
    match decide(round, terminal_success, existing_same_signature) {
        Necessity::Reject(reason) => return Ok(Some(reason.label())),
        Necessity::Dirty => {
            if !distiller.accepts_dirty() {
                // P1-1: without an LLM compiler, dirty rounds are refused
                // instead of being distilled as-is (no prune yet).
                return Ok(Some(RejectReason::DistillUnavailable.label()));
            }
        }
        Necessity::CleanShort => {}
    }
    if round_has_ephemeral_identifier(round) {
        return Ok(Some(RejectReason::UnstableShape.label()));
    }
    let Some(draft) = distiller.distill(round) else {
        return Ok(Some(RejectReason::DistillUnavailable.label()));
    };
    if let Err(issue) = validate_tool_boundary(&draft) {
        // Audit detail is not part of the enum label; log via caller later.
        let _ = issue;
        return Ok(Some(RejectReason::ToolBoundaryInvalid.label()));
    }
    CandidateWriter::new(store).write_candidate(draft)?;
    Ok(None)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Necessity {
    Reject(RejectReason),
    CleanShort,
    Dirty,
}

/// Stable task signature for repeat detection (exact-enough first cut).
pub fn task_signature(task: &str) -> String {
    task.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// L1 necessity gate (docs §4, implemented subset): terminal status, action
/// evidence, repeat detection, clean/dirty routing. Unstable-shape detection
/// needs tool args and lands with the distill decision.
pub fn decide(
    round: &RoundTrace,
    terminal_success: bool,
    existing_same_signature: bool,
) -> Necessity {
    if !terminal_success {
        return Necessity::Reject(RejectReason::TerminalNotSuccess);
    }
    let tool_calls = round
        .events
        .iter()
        .filter(|event| matches!(event, TraceEvent::ToolCall { .. }))
        .count();
    if tool_calls == 0 {
        return Necessity::Reject(RejectReason::NoActionEvidence);
    }
    if existing_same_signature {
        return Necessity::Reject(RejectReason::RepeatNoImprovement);
    }
    if tool_calls <= MAX_CLEAN_TOOL_STEPS {
        Necessity::CleanShort
    } else {
        Necessity::Dirty
    }
}

/// L1 writer with the CANDIDATE-only power boundary: forcing the status at
/// the API boundary means the L1 pipeline cannot accidentally persist an
/// executable experience, no matter what its draft claims.
pub struct CandidateWriter<'a> {
    store: &'a mut ExperienceStore,
}

impl<'a> CandidateWriter<'a> {
    pub fn new(store: &'a mut ExperienceStore) -> Self {
        Self { store }
    }

    pub fn write_candidate(&mut self, draft: Experience) -> Result<(), String> {
        let mut candidate = draft;
        candidate.status = ExperienceStatus::Candidate;
        self.store
            .insert(candidate)
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::action::ActionPattern;
    use crate::domain::experience::FailurePolicy;
    use crate::domain::experience::UndoPolicy;
    use crate::domain::experience::WorkflowStep;
    use crate::domain::predicate::Predicate;

    fn round_with_tool_calls(count: usize) -> RoundTrace {
        let mut events = Vec::new();
        for _ in 0..count {
            events.push(TraceEvent::ToolCall {
                name: "exec_command".into(),
                call_id: None,
                args_summary: Some("mv *.png pic/".into()),
            });
        }
        RoundTrace {
            task: "move png files".into(),
            events,
        }
    }

    #[test]
    fn necessity_rejects_non_terminal_and_no_action_rounds() {
        assert_eq!(
            decide(&round_with_tool_calls(1), false, false),
            Necessity::Reject(RejectReason::TerminalNotSuccess)
        );
        assert_eq!(
            decide(&round_with_tool_calls(0), true, false),
            Necessity::Reject(RejectReason::NoActionEvidence)
        );
    }

    #[test]
    fn necessity_rejects_repeat_and_routes_clean_vs_dirty() {
        assert_eq!(
            decide(&round_with_tool_calls(2), true, true),
            Necessity::Reject(RejectReason::RepeatNoImprovement)
        );
        assert_eq!(
            decide(&round_with_tool_calls(2), true, false),
            Necessity::CleanShort
        );
        assert_eq!(
            decide(&round_with_tool_calls(MAX_CLEAN_TOOL_STEPS + 1), true, false),
            Necessity::Dirty
        );
    }

    fn schema_ok_draft(name: &str) -> Experience {
        Experience {
            name: name.into(),
            trigger: ActionPattern {
                tool: "exec_command".into(),
                command_pattern: Some("probe".into()),
            },
            preconditions: vec![Predicate::new("cwd.exists", serde_json::json!(true))],
            workflow: vec![WorkflowStep::new(
                "exec_command",
                serde_json::json!({ "cmd": "probe" }),
            )],
            postconditions: vec![Predicate::new(
                "candidate.pending_validation",
                serde_json::json!(true),
            )],
            verification: Vec::new(),
            failure_policy: FailurePolicy::StopAndReport,
            undo: UndoPolicy::Unsupported,
            status: ExperienceStatus::Draft,
        }
    }

    #[test]
    fn writer_forces_candidate_and_rejects_invalid_or_duplicate() {
        let mut store = ExperienceStore::default();
        {
            let mut writer = CandidateWriter::new(&mut store);
            writer
                .write_candidate(schema_ok_draft("cand-1"))
                .expect("schema-valid draft should be accepted");
            let duplicate = writer.write_candidate(schema_ok_draft("cand-1"));
            assert!(duplicate.is_err(), "duplicate candidate must be rejected");

            let mut broken = schema_ok_draft("cand-broken");
            broken.workflow.clear();
            assert!(writer.write_candidate(broken).is_err());
        }
        let stored = store.get("cand-1").expect("candidate persisted");
        assert_eq!(stored.status, ExperienceStatus::Candidate);
        assert!(store.get("cand-broken").is_none());
    }

    #[test]
    fn sink_writes_candidate_from_clean_round_with_args() {
        let mut store = ExperienceStore::default();
        let round = round_with_tool_calls(1);
        let result = sink_once(
            &round,
            true,
            false,
            &mut store,
            &DeterministicDistiller,
        );
        assert_eq!(result, Ok(None), "candidate should be written");
        let name = candidate_name(&round.task);
        let stored = store.get(&name).expect("candidate persisted");
        assert_eq!(stored.status, ExperienceStatus::Candidate);
        assert_eq!(
            stored.workflow[0].args.get("cmd").and_then(serde_json::Value::as_str),
            Some("mv *.png pic/")
        );
    }

    #[test]
    fn sink_refuses_when_args_summary_is_missing() {
        let mut store = ExperienceStore::default();
        let round = RoundTrace {
            task: "move png files".into(),
            events: vec![TraceEvent::ToolCall {
                name: "exec_command".into(),
                call_id: None,
                args_summary: None,
            }],
        };
        let result = sink_once(
            &round,
            true,
            false,
            &mut store,
            &DeterministicDistiller,
        );
        assert_eq!(result, Ok(Some("distill_unavailable")));
        assert_eq!(store.get("cand_move_png_files"), None);
    }

    #[test]
    fn sink_rejects_repeat_without_writing() {
        let mut store = ExperienceStore::default();
        let round = round_with_tool_calls(1);
        let result = sink_once(&round, true, true, &mut store, &DeterministicDistiller);
        assert_eq!(result, Ok(Some("repeat_no_improvement")));
        assert_eq!(store.get("cand_move_png_files"), None);
    }

    #[test]
    fn sink_rejects_unknown_tool_at_boundary() {
        let mut store = ExperienceStore::default();
        let round = RoundTrace {
            task: "mystery task".into(),
            events: vec![TraceEvent::ToolCall {
                name: "mystery_tool".into(),
                call_id: None,
                args_summary: Some("something".into()),
            }],
        };
        let result = sink_once(
            &round,
            true,
            false,
            &mut store,
            &DeterministicDistiller,
        );
        assert_eq!(result, Ok(Some("distill_unavailable")));
        assert!(store.get("cand_mystery_task").is_none());
    }

    #[test]
    fn sink_rejects_dirty_round_even_with_args() {
        let mut store = ExperienceStore::default();
        let round = round_with_tool_calls(MAX_CLEAN_TOOL_STEPS + 1);
        let result = sink_once(
            &round,
            true,
            false,
            &mut store,
            &DeterministicDistiller,
        );
        assert_eq!(result, Ok(Some("distill_unavailable")));
        assert!(store.get("cand_move_png_files").is_none());
    }

    #[test]
    fn command_execution_is_canonicalized_to_exec_command() {
        let mut store = ExperienceStore::default();
        let round = RoundTrace {
            task: "canonical task".into(),
            events: vec![TraceEvent::ToolCall {
                name: "commandExecution".into(),
                call_id: None,
                args_summary: Some("touch x".into()),
            }],
        };
        let result = sink_once(
            &round,
            true,
            false,
            &mut store,
            &DeterministicDistiller,
        );
        assert_eq!(result, Ok(None));
        let stored = store.get("cand_canonical_task").expect("candidate stored");
        assert_eq!(stored.trigger.tool, "exec_command");
        assert_eq!(stored.workflow[0].action, "exec_command");
    }

    #[test]
    fn tool_boundary_rejects_unknown_action_directly() {
        let mut draft = schema_ok_draft("boundary");
        draft.workflow[0].action = "mystery_tool".into();
        assert!(validate_tool_boundary(&draft).is_err());
    }

    #[test]
    fn sink_rejects_likely_ephemeral_identifier_conservatively() {
        let mut store = ExperienceStore::default();
        let round = RoundTrace {
            task: "update commit".into(),
            events: vec![TraceEvent::ToolCall {
                name: "exec_command".into(),
                call_id: None,
                args_summary: Some("git checkout a1b2c3d4e5f60718293a4b5c6d7e8f90".into()),
            }],
        };
        let result = sink_once(
            &round,
            true,
            false,
            &mut store,
            &DeterministicDistiller,
        );
        assert_eq!(result, Ok(Some("unstable_shape")));
        assert!(store.get("cand_update_commit").is_none());
    }

    struct DirtyAcceptingDistiller;

    impl L1Distiller for DirtyAcceptingDistiller {
        fn accepts_dirty(&self) -> bool {
            true
        }

        fn distill(&self, _round: &RoundTrace) -> Option<Experience> {
            Some(schema_ok_draft("cand_dirty_llm"))
        }
    }

    #[test]
    fn sink_refuses_dirty_without_accepting_distiller() {
        let mut store = ExperienceStore::default();
        let round = round_with_tool_calls(MAX_CLEAN_TOOL_STEPS + 1);
        let result = sink_once(
            &round,
            true,
            false,
            &mut store,
            &DeterministicDistiller,
        );
        assert_eq!(result, Ok(Some("distill_unavailable")));
        assert!(store.is_empty());
    }

    #[test]
    fn sink_dirty_round_writes_when_distiller_accepts_dirty() {
        let mut store = ExperienceStore::default();
        let round = round_with_tool_calls(MAX_CLEAN_TOOL_STEPS + 1);
        let result = sink_once(&round, true, false, &mut store, &DirtyAcceptingDistiller);
        assert_eq!(result, Ok(None), "dirty round must be compilable");
        let stored = store.get("cand_dirty_llm").expect("candidate persisted");
        assert_eq!(stored.status, ExperienceStatus::Candidate);
    }
}

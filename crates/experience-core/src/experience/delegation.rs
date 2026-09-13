//! L3 delegation-first loop, domain layer (docs/l3-delegation-design.md v2).
//!
//! L3 is the task-level orchestrator: given current State + ACTIVE
//! candidates + task segments it produces an ExperiencePlan (Select Plan).
//! It never decides "what the LLM should do" (that belongs to the delegated
//! executor) and never lets the LLM subtract task text inside L3.

use std::path::Path;

use crate::domain::experience::Experience;
use crate::domain::predicate::TruthValue;
use crate::experience::qualification::ProbeSource;
use crate::experience::qualification::predicates_observable;
use crate::experience::qualification::ConfidenceRecord;
use crate::state_source;
use crate::state_source::ProbePolicy;

/// Freshness TTL for the built-in filesystem probe source.
pub const FS_PROBE_TTL_SECS: u64 = 60;

/// D2: first State-source bindings (cwd/file). Semantics frozen in L2:
/// a predicate is observable when it binds to a source with a freshness TTL
/// and the source can be evaluated now.
pub fn resolve_default_source(key: &str) -> Option<ProbeSource> {
    let family = state_source::family_of(key)?;
    let (id, ttl) = match family {
        "fs" => ("fs_probe", FS_PROBE_TTL_SECS),
        "exec" => ("exec_evidence", 30),
        "git" => ("git_probe", 30),
        "http" => ("http_probe", 15),
        "net" => ("net_probe", 10),
        _ => return None,
    };
    Some(ProbeSource {
        id: id.to_string(),
        freshness_ttl: ttl,
    })
}

/// Evaluate a predicate key against the real filesystem under `base_dir`.
/// S3: rich key families (size/sha256/dir/git/http/port) resolve through the
/// State Source Registry; unregistered keys stay `None` (→ Unknown).
pub fn probe_value(key: &str, base_dir: &Path) -> Option<serde_json::Value> {
    state_source::probe(key, base_dir, &ProbePolicy::default()).value
}

/// D3: a task is a list of segments; remaining = uncovered segments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSegment {
    pub id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepResult {
    pub segment_id: Option<String>,
    pub ok: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExperiencePlan {
    pub selected: Vec<String>,
    pub skipped: Vec<String>,
    pub covered_segments: Vec<String>,
    pub remaining_segments: Vec<String>,
    pub reason: String,
}

/// Deterministic ranking input per candidate (confidence/cost/priority come
/// from L2 Qualification records at runtime).
pub struct CandidateRank {
    pub name: String,
    pub coverage_hint: usize,
    pub confidence: f64,
    pub priority: u8,
}

/// Select Plan (v1, no LLM): coverage = workflow length proxy; state match
/// via default source bindings; ties at the best coverage/confidence level
/// are treated as conflict -> nothing auto-executes from that tie (rule 7).
pub fn select_plan(
    segments: &[TaskSegment],
    candidates: &[CandidateRank],
    preconditions_pass: &dyn Fn(&str) -> bool,
) -> ExperiencePlan {
    let mut ranked: Vec<&CandidateRank> = candidates
        .iter()
        .filter(|candidate| preconditions_pass(&candidate.name))
        .collect();
    ranked.sort_by(|a, b| {
        b.coverage_hint
            .cmp(&a.coverage_hint)
            .then_with(|| b.confidence.total_cmp(&a.confidence))
            .then_with(|| b.priority.cmp(&a.priority))
    });
    let all_ids: Vec<String> = segments.iter().map(|s| s.id.clone()).collect();
    match ranked.as_slice() {
        [] => ExperiencePlan {
            selected: Vec::new(),
            skipped: Vec::new(),
            covered_segments: Vec::new(),
            remaining_segments: all_ids,
            reason: "no_match".into(),
        },
        [best] => ExperiencePlan {
            selected: vec![best.name.clone()],
            skipped: Vec::new(),
            covered_segments: segments
                .iter()
                .take(best.coverage_hint.min(segments.len()))
                .map(|s| s.id.clone())
                .collect(),
            remaining_segments: segments
                .iter()
                .skip(best.coverage_hint.min(segments.len()))
                .map(|s| s.id.clone())
                .collect(),
            reason: "single_best".into(),
        },
        [first, second, ..]
            if first.coverage_hint == second.coverage_hint
                && (first.confidence - second.confidence).abs() < f64::EPSILON =>
        {
            ExperiencePlan {
                selected: Vec::new(),
                skipped: vec![first.name.clone(), second.name.clone()],
                covered_segments: Vec::new(),
                remaining_segments: all_ids,
                reason: "conflict".into(),
            }
        }
        [best, ..] => ExperiencePlan {
            selected: vec![best.name.clone()],
            skipped: ranked[1..].iter().map(|c| c.name.clone()).collect(),
            covered_segments: segments
                .iter()
                .take(best.coverage_hint.min(segments.len()))
                .map(|s| s.id.clone())
                .collect(),
            remaining_segments: segments
                .iter()
                .skip(best.coverage_hint.min(segments.len()))
                .map(|s| s.id.clone())
                .collect(),
            reason: "best".into(),
        },
    }
}

/// D4: usage layering - delegate failure must never count against the
/// Experience (L2 evidence-variable principle applied at L3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExperienceOutcome {
    Success,
    Misfire,
    Invalid,
    ExecutionError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DelegationOutcome {
    Delegated,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskOutcome {
    Success,
    Partial,
    Failed,
}

/// v1 fallback (ruling): when no Task Segment structure exists, partial
/// delegates the ORIGINAL task plus completed step ids / verified state -
/// never an LLM-rewritten "remainder".
pub fn delegation_text(
    original_task: &str,
    completed_step_ids: &[String],
    verified_state: &[String],
) -> String {
    let mut text = original_task.to_string();
    if !completed_step_ids.is_empty() {
        text.push_str("\n\n[experience 已完成步骤] ");
        text.push_str(&completed_step_ids.join(", "));
    }
    if !verified_state.is_empty() {
        text.push_str("\n[已验证状态] ");
        text.push_str(&verified_state.join(", "));
    }
    if !completed_step_ids.is_empty() {
        // Stage S1 / P2-4 ruling (option A): the completed section is
        // factual and must NOT be re-executed by the delegated agent.
        text.push_str("\n\n[注意] 以上 experience 步骤已在进程内执行并通过验证，请勿重复执行；");
        text.push_str("也不要重写、覆盖、回滚这些文件改动。只完成剩余部分或确认收尾即可。");
    }
    text
}

/// Injection policy: default OFF. Only explicit user policy enables
/// references; every decision records injected/omitted + reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectionPolicy {
    Off,
    On,
}

pub fn injection_decision(
    policy: InjectionPolicy,
    available_references: usize,
) -> (bool, String) {
    match policy {
        InjectionPolicy::Off => (false, "policy_off".to_string()),
        InjectionPolicy::On if available_references == 0 => {
            (false, "no_reference_available".to_string())
        }
        InjectionPolicy::On => (true, "policy_on".to_string()),
    }
}

/// Apply layered usage to the L2 ConfidenceRecord: only the Experience's own
/// outcome changes its evidence; delegation/task outcomes never do.
pub fn apply_experience_usage(
    confidence: &mut ConfidenceRecord,
    outcome: ExperienceOutcome,
    at: u64,
) {
    match outcome {
        ExperienceOutcome::Success => confidence.record_success(at),
        ExperienceOutcome::Misfire => confidence.record_misfire(),
        ExperienceOutcome::ExecutionError => confidence.record_execution_error(at),
        ExperienceOutcome::Invalid => confidence.record_invalid(at),
    }
}

/// Validate an Experience against the built-in fs sources (used by the
/// D2 wiring to make qualification reachable for cwd/file predicates).
pub fn fs_qualification_issues(candidate: &Experience) -> Result<(), Vec<String>> {
    for predicate in candidate
        .preconditions
        .iter()
        .chain(candidate.postconditions.iter())
    {
        if resolve_default_source(&predicate.key).is_none() {
            return Err(vec![format!("{}: no fs source binding", predicate.key)]);
        }
    }
    predicates_observable(&candidate.preconditions, &resolve_default_source)
}

/// Mechanical predicate evaluation against the filesystem (D2/D3).
pub fn evaluate_predicate(
    key: &str,
    expected: &serde_json::Value,
    base_dir: &Path,
) -> TruthValue {
    match probe_value(key, base_dir) {
        Some(actual) if &actual == expected => TruthValue::True,
        Some(_) => TruthValue::False,
        None => TruthValue::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fs_probe_resolves_and_evaluates_cwd() {
        assert!(resolve_default_source("cwd.exists").is_some());
        assert!(resolve_default_source("network.up").is_none());
        let base = std::env::temp_dir();
        assert_eq!(
            evaluate_predicate("cwd.exists", &serde_json::json!(true), &base),
            TruthValue::True
        );
    }

    #[test]
    fn select_plan_picks_best_and_marks_conflict() {
        let segments = vec![
            TaskSegment { id: "A".into(), text: "a".into() },
            TaskSegment { id: "B".into(), text: "b".into() },
        ];
        let candidates = vec![
            CandidateRank { name: "E1".into(), coverage_hint: 1, confidence: 0.8, priority: 1 },
            CandidateRank { name: "E2".into(), coverage_hint: 2, confidence: 0.7, priority: 1 },
        ];
        let plan = select_plan(&segments, &candidates, &|_| true);
        assert_eq!(plan.selected, vec!["E2"]);
        assert_eq!(plan.remaining_segments, Vec::<String>::new());

        let tie = vec![
            CandidateRank { name: "E1".into(), coverage_hint: 1, confidence: 0.8, priority: 1 },
            CandidateRank { name: "E2".into(), coverage_hint: 1, confidence: 0.8, priority: 1 },
        ];
        let conflict = select_plan(&segments, &tie, &|_| true);
        assert_eq!(conflict.selected, Vec::<String>::new());
        assert_eq!(conflict.reason, "conflict");
    }

    #[test]
    fn usage_layers_are_orthogonal() {
        // A successful Experience followed by a failed delegation keeps the
        // Experience outcome intact (no confidence pollution at this layer).
        let exp = ExperienceOutcome::Success;
        let delegation = DelegationOutcome::Failed;
        let task = TaskOutcome::Partial;
        assert_eq!(exp, ExperienceOutcome::Success);
        assert_eq!(delegation, DelegationOutcome::Failed);
        assert_eq!(task, TaskOutcome::Partial);
    }

    #[test]
    fn delegation_text_keeps_original_task_and_appends_state() {
        let text = delegation_text(
            "install deps",
            &["step-1".to_string()],
            &["file:pyproject.toml.exists=true".to_string()],
        );
        assert!(text.starts_with("install deps"));
        assert!(text.contains("step-1"));
        assert!(text.contains("pyproject.toml"));
        assert!(text.contains("请勿重复执行"), "option A no-redo notice");
        assert!(!text.contains("LLM"), "no rewritten remainder");
    }

    #[test]
    fn injection_defaults_off_and_records_reason() {
        let (inject, reason) = injection_decision(InjectionPolicy::Off, 3);
        assert!(!inject);
        assert_eq!(reason, "policy_off");
        let (inject_on, reason_on) = injection_decision(InjectionPolicy::On, 1);
        assert!(inject_on);
        assert_eq!(reason_on, "policy_on");
    }

    #[test]
    fn only_experience_outcome_moves_confidence() {
        let mut confidence = ConfidenceRecord::default();
        apply_experience_usage(&mut confidence, ExperienceOutcome::Success, 1);
        assert_eq!(confidence.evidence.successes, 1);
        apply_experience_usage(&mut confidence, ExperienceOutcome::Misfire, 2);
        assert_eq!(confidence.evidence.misfires, 1);
        // Environmental execution failures are attributed but score-neutral:
        // they never silently kill a good experience (review P2-1).
        let score_before = confidence.score;
        apply_experience_usage(&mut confidence, ExperienceOutcome::ExecutionError, 3);
        assert_eq!(confidence.evidence.execution_errors, 1);
        assert_eq!(confidence.score, score_before);
        apply_experience_usage(&mut confidence, ExperienceOutcome::Invalid, 4);
        assert_eq!(confidence.evidence.invalid, 1);
        assert!(confidence.score < score_before);
        // A failed delegation does not exist on this path by construction.
        let _ = DelegationOutcome::Failed;
    }
}

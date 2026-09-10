//! L2 qualification chain (docs/l2-qualification-design.md v2).
//!
//! - C2: validate gates 3-5 - predicate observability through State
//!   Source/Probe bindings (not a closed ontology), dry-run replay that
//!   validates ToolRouter/runner behavior only, completion-criterion
//!   observability.
//! - C3: confidence (evidence quality) and activity (recency) are two
//!   orthogonal variables; L2 outputs ExperienceQualification and never
//!   holds the final execution decision (that is L3's joint decision).

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;
use serde::Serialize;

use crate::domain::capability::CANONICAL_TOOLS;
use crate::domain::experience::Experience;
use crate::domain::experience::ExperienceStatus;
use crate::domain::predicate::Predicate;

/// Frozen State-source semantics for L3 (no full State registry here).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeSource {
    pub id: String,
    /// Freshness TTL in seconds; sources without a TTL cannot back
    /// long-lived predicates.
    pub freshness_ttl: u64,
}

/// Gate 3/5: every predicate must bind to a State source that can be
/// reliably evaluated *now* (existence + freshness contract). Unknown keys
/// are rejected; this is not a membership test against a fixed vocabulary.
pub fn predicates_observable(
    predicates: &[Predicate],
    resolve: &dyn Fn(&str) -> Option<ProbeSource>,
) -> Result<(), Vec<String>> {
    let mut problems = Vec::new();
    for predicate in predicates {
        if predicate.key == "candidate.pending_validation" {
            problems.push(format!(
                "{}: placeholder completion criterion is not evidence",
                predicate.key
            ));
            continue;
        }
        match resolve(&predicate.key) {
            Some(source) if source.freshness_ttl > 0 => {}
            Some(_) => problems.push(format!("{}: source has no freshness ttl", predicate.key)),
            None => problems.push(format!("{}: no observable state source binding", predicate.key)),
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

/// Gate 4: dry-run replay that validates ToolRouter/runner behavior only.
/// exec_command is parsed but never executed; write_file is replayed inside
/// an isolated scratch dir (no real-workspace inference). A scratch success
/// proves the runner accepts the step, never that the experience works in
/// the real workspace - that is Gate 5 territory.
pub fn dry_run_replayable(candidate: &Experience) -> Result<(), String> {
    let scratch = std::env::temp_dir().join(format!(
        "exp-qual-dryrun-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
    let outcome = (|| {
        for step in &candidate.workflow {
            if !CANONICAL_TOOLS.contains(&step.action.as_str()) {
                return Err(format!("unknown tool '{}'", step.action));
            }
            match step.action.as_str() {
                "exec_command" => {
                    let Some(cmd) = step.args.get("cmd").and_then(serde_json::Value::as_str) else {
                        return Err("exec_command requires string arg 'cmd'".to_string());
                    };
                    if cmd.trim().is_empty() {
                        return Err("exec_command 'cmd' is empty".to_string());
                    }
                }
                "write_file" => {
                    let Some(content) = step
                        .args
                        .get("content")
                        .and_then(serde_json::Value::as_str)
                    else {
                        return Err("write_file requires string arg 'content'".to_string());
                    };
                    let target = scratch.join("written.txt");
                    std::fs::write(&target, content).map_err(|e| e.to_string())?;
                    let read_back = std::fs::read_to_string(&target).map_err(|e| e.to_string())?;
                    if read_back != content {
                        return Err("write_file scratch round-trip mismatch".to_string());
                    }
                }
                "read_file" => {
                    if step.args.get("path").is_none() {
                        return Err("read_file requires arg 'path'".to_string());
                    }
                }
                _ => unreachable!("canonical tool checked above"),
            }
        }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&scratch);
    outcome
}

/// Evidence (quality) and Activity (recency) are orthogonal (v2 ruling).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub successes: u64,
    pub misfires: u64,
    pub invalid: u64,
    /// Environmental execution failures (io/permission/runner), counted
    /// separately and score-neutral (review P2-1: never merge with invalid).
    #[serde(default)]
    pub execution_errors: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Activity {
    pub usage_count: u64,
    pub last_used_at: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfidenceRecord {
    pub score: f64,
    pub evidence: Evidence,
    pub activity: Activity,
}

impl Default for ConfidenceRecord {
    fn default() -> Self {
        Self {
            score: 0.5,
            evidence: Evidence::default(),
            activity: Activity::default(),
        }
    }
}

impl ConfidenceRecord {
    /// Evidence update functions: success raises, misfire is neutral
    /// (MissingSource/ParamMismatch is not the experience's fault), invalid
    /// lowers. Activity is tracked separately and never decays the score.
    pub fn record_success(&mut self, at: u64) {
        self.evidence.successes += 1;
        self.activity.usage_count += 1;
        self.activity.last_used_at = at;
        self.score = (self.score + 0.08).min(1.0);
    }

    pub fn record_misfire(&mut self) {
        self.evidence.misfires += 1;
        // Neutral by design: misfire does not assert the experience invalid.
    }

    /// Environmental failure (io/permission) is attributed, counted, and
    /// score-neutral — it must never silently kill a good experience.
    pub fn record_execution_error(&mut self, at: u64) {
        self.evidence.execution_errors += 1;
        self.activity.last_used_at = at;
    }

    pub fn record_invalid(&mut self, at: u64) {
        self.evidence.invalid += 1;
        self.activity.last_used_at = at;
        self.score = (self.score - 0.2).max(0.0);
    }
}

/// L2 output: "this experience is qualified". The final execution decision
/// belongs to L3 (State + Qualification + cost + priority + conflict +
/// intent); takeover belongs to L4.
#[derive(Debug, Clone, PartialEq)]
pub struct ExperienceQualification {
    pub valid: bool,
    pub lifecycle: ExperienceStatus,
    pub confidence: ConfidenceRecord,
}

/// Full qualification check: preconditions observable (gate 3) + dry-run
/// replay (gate 4) + completion criteria observable (gate 5). Returns the
/// list of blocking issues; an empty result means the candidate may be
/// transitioned to VALIDATED via the domain transition table.
pub fn qualify(
    candidate: &Experience,
    resolve: &dyn Fn(&str) -> Option<ProbeSource>,
) -> Result<(), Vec<String>> {
    predicates_observable(&candidate.preconditions, resolve)?;
    dry_run_replayable(candidate).map_err(|issue| vec![issue])?;
    predicates_observable(&candidate.postconditions, resolve)
}

/// Convenience probe map for tests/callers with static source bindings.
pub fn resolve_from(map: &HashMap<String, ProbeSource>) -> impl Fn(&str) -> Option<ProbeSource> + '_ {
    move |key| map.get(key).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::action::ActionPattern;
    use crate::domain::experience::FailurePolicy;
    use crate::domain::experience::UndoPolicy;
    use crate::domain::experience::WorkflowStep;
    use crate::domain::predicate::StateFact;

    fn sample(preconditions: Vec<Predicate>, workflow: Vec<WorkflowStep>, post: Vec<Predicate>) -> Experience {
        Experience {
            name: "qual".into(),
            trigger: ActionPattern {
                tool: "exec_command".into(),
                command_pattern: None,
            },
            preconditions,
            workflow,
            postconditions: post,
            verification: Vec::new(),
            failure_policy: FailurePolicy::StopAndReport,
            undo: UndoPolicy::Unsupported,
            status: crate::domain::experience::ExperienceStatus::Candidate,
        }
    }

    fn sources() -> HashMap<String, ProbeSource> {
        HashMap::from([(
            "file:probe.txt.exists".to_string(),
            ProbeSource {
                id: "file_probe".into(),
                freshness_ttl: 60,
            },
        )])
    }

    #[test]
    fn gate3_rejects_unbound_predicates_and_passes_bound_ones() {
        let ok = predicates_observable(
            &[Predicate::new("file:probe.txt.exists", serde_json::json!(true))],
            &resolve_from(&sources()),
        );
        assert!(ok.is_ok());
        let missing = predicates_observable(
            &[Predicate::new("network.vpn.up", serde_json::json!(true))],
            &resolve_from(&sources()),
        );
        assert!(missing.is_err());
    }

    #[test]
    fn gate5_rejects_placeholder_completion_criterion() {
        let experience = sample(
            vec![Predicate::new("file:probe.txt.exists", serde_json::json!(true))],
            vec![WorkflowStep::new("read_file", serde_json::json!({ "path": "probe.txt" }))],
            vec![Predicate::new(
                "candidate.pending_validation",
                serde_json::json!(true),
            )],
        );
        assert!(predicates_observable(&experience.postconditions, &resolve_from(&sources())).is_err());
    }

    #[test]
    fn gate4_write_file_round_trips_in_scratch_only() {
        let experience = sample(
            Vec::new(),
            vec![WorkflowStep::new(
                "write_file",
                serde_json::json!({ "content": "hello" }),
            )],
            Vec::new(),
        );
        assert!(dry_run_replayable(&experience).is_ok());
    }

    #[test]
    fn confidence_evidence_never_flips_truth() {
        let fact = StateFact::new("file:x", serde_json::json!(1), "probe", 1);
        let predicate = Predicate::new("file:x", serde_json::json!(1));
        let mut record = ConfidenceRecord::default();
        let initial = record.score;
        record.record_success(2);
        assert!(record.score > initial);
        record.record_misfire();
        record.record_invalid(3);
        // Counters changed; the observed fact and its truth value did not.
        assert_eq!(fact.value, serde_json::json!(1));
        assert_eq!(predicate.probe(Some(&fact.value)), crate::domain::predicate::TruthValue::True);
    }
}

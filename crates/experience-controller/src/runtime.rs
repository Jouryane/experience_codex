//! The synchronous Gate runtime: decide -> (execute + verify + return).

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use experience_core::domain::action::ActionProposal;
use experience_core::domain::experience::Experience;
use experience_core::domain::experience::VerificationStep;
use experience_core::domain::gate::ExecutedStep;
use experience_core::domain::gate::GateDecision;
use experience_core::domain::gate::GateHitResult;
use experience_core::domain::gate::GateTier;
use experience_core::domain::predicate::Predicate;
use experience_core::domain::predicate::StateFact;
use experience_core::domain::predicate::TruthValue;
use experience_core::store::ExperienceStore;
use serde_json::Value;

use super::probe::Probe;
use super::runner::CapabilityRunner;

/// Environment context handed to every Gate call (M3: cwd only; State
/// predicates extend this later).
#[derive(Debug, Clone)]
pub struct GateContext {
    pub cwd: PathBuf,
}

/// Final outcome of one synchronous gate call.
#[derive(Debug, Clone, PartialEq)]
pub enum GateOutcome {
    /// No experience matched: pass through to the original tool executor.
    Miss,
    /// Experience took over and fully resolved (control-flow transaction).
    Hit(GateHitResult),
}

impl GateOutcome {
    pub fn is_hit(&self) -> bool {
        matches!(self, GateOutcome::Hit(_))
    }
}

/// P1 Runtime: owns the Store and the Probe/Runner capabilities, exposes the
/// synchronous decide + execute boundary.
pub struct ExperienceGateRuntime {
    store: ExperienceStore,
    probe: Box<dyn Probe>,
    runner: Box<dyn CapabilityRunner>,
    cancel: Arc<AtomicBool>,
}

impl ExperienceGateRuntime {
    pub fn new(
        store: ExperienceStore,
        probe: Box<dyn Probe>,
        runner: Box<dyn CapabilityRunner>,
    ) -> Self {
        Self {
            store,
            probe,
            runner,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Request cancellation; honored between workflow steps (a single step's
    /// synchronous tool call is not interrupted mid-call — bounded work).
    pub fn request_cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn reset_cancel(&self) {
        self.cancel.store(false, Ordering::Relaxed);
    }

    pub fn store(&self) -> &ExperienceStore {
        &self.store
    }

    /// Stage 1 of the gate: mechanical trigger filter + precondition probes.
    /// Returns a Hit only for the first candidate whose preconditions all
    /// hold (P1: no scoring, no threshold guessing).
    pub fn decide(&self, proposal: &ActionProposal, context: &GateContext) -> GateDecision {
        for candidate in self.store.candidates_for(proposal) {
            if self.preconditions_hold(candidate, context) {
                return GateDecision::Hit {
                    experience: candidate.name.clone(),
                    tier: tier_for(candidate),
                };
            }
        }
        GateDecision::Miss
    }

    /// Stage 2 of the gate: run the matched experience's compiled workflow,
    /// verify postconditions against real evidence, and return the final
    /// GateHitResult. Never called on a Miss.
    pub fn execute(
        &self,
        proposal: &ActionProposal,
        context: &GateContext,
        decision: &GateDecision,
    ) -> GateHitResult {
        let name = match decision {
            GateDecision::Hit { experience, .. } => experience,
            GateDecision::Miss => panic!("execute() called on a Miss; gate must pass through"),
        };
        let experience = self
            .store
            .get(name)
            .expect("GateDecision references an experience that left the store");
        self.execute_experience(proposal, context, experience)
    }

    /// The full synchronous gate: decide then, on Hit, execute+verify+return
    /// inside the same call (step-gate.md §8.2).
    pub fn gate(&self, proposal: &ActionProposal, context: &GateContext) -> GateOutcome {
        match self.decide(proposal, context) {
            GateDecision::Miss => GateOutcome::Miss,
            decision @ GateDecision::Hit { .. } => {
                GateOutcome::Hit(self.execute(proposal, context, &decision))
            }
        }
    }

    fn preconditions_hold(&self, experience: &Experience, context: &GateContext) -> bool {
        experience
            .preconditions
            .iter()
            .all(|predicate| self.probe_predicate(predicate, context) == TruthValue::True)
    }

    fn probe_predicate(&self, predicate: &Predicate, context: &GateContext) -> TruthValue {
        let observed = self.probe.probe(context, &predicate.key);
        predicate.probe(observed.as_ref())
    }

    fn execute_experience(
        &self,
        proposal: &ActionProposal,
        context: &GateContext,
        experience: &Experience,
    ) -> GateHitResult {
        let state_before: Vec<StateFact> = experience
            .preconditions
            .iter()
            .filter_map(|predicate| {
                let observed = self.probe.probe(context, &predicate.key)?;
                Some(StateFact::new(
                    predicate.key.clone(),
                    observed,
                    "probe".to_string(),
                    now_secs(),
                ))
            })
            .collect();

        let mut executed = Vec::new();
        let mut execution_evidence = Vec::new();
        let mut executed_side_effects = Vec::new();

        // Workflow: run pre-compiled steps; a failure stops the chain.
        for step in &experience.workflow {
            if self.cancel.load(Ordering::Relaxed) {
                execution_evidence.push("workflow interrupted by cancellation".to_string());
                return GateHitResult::failed(
                    proposal.clone(),
                    state_before,
                    executed,
                    self.observe_after(context, experience),
                    execution_evidence,
                    executed_side_effects,
                );
            }
            match self.runner.run(context, step) {
                Ok(outcome) => {
                    executed.push(ExecutedStep {
                        action: step.action.clone(),
                        args: step.args.clone(),
                        evidence: Some(outcome.evidence.clone()),
                    });
                    execution_evidence.push(outcome.evidence.clone());
                    executed_side_effects.extend(outcome.side_effects);
                }
                Err(error) => {
                    execution_evidence.push(format!(
                        "step '{}' failed: {error}",
                        step.action
                    ));
                    return GateHitResult::failed(
                        proposal.clone(),
                        state_before,
                        executed,
                        self.observe_after(context, experience),
                        execution_evidence,
                        executed_side_effects,
                    );
                }
            }
        }

        // Verify postconditions against real evidence (never asserted).
        let (state_after, verification_evidence) =
            self.verify_postconditions(context, experience);
        execution_evidence.extend(verification_evidence);

        if experience.postconditions_satisfied_by(&state_after) {
            GateHitResult::completed(
                proposal.clone(),
                state_before,
                executed,
                state_after,
                execution_evidence,
                executed_side_effects,
            )
        } else {
            GateHitResult::partial(
                proposal.clone(),
                state_before,
                executed,
                state_after,
                execution_evidence,
                executed_side_effects,
            )
        }
    }

    /// Observe the actual state after a failure so the Agent can take over
    /// with facts, not guesses.
    fn observe_after(&self, context: &GateContext, experience: &Experience) -> Vec<StateFact> {
        experience
            .postconditions
            .iter()
            .filter_map(|predicate| {
                let observed = self.probe.probe(context, &predicate.key)?;
                Some(StateFact::new(
                    predicate.key.clone(),
                    observed,
                    "probe after failure".to_string(),
                    now_secs(),
                ))
            })
            .collect()
    }

    /// Run the verification steps and turn observations into StateFacts.
    fn verify_postconditions(
        &self,
        context: &GateContext,
        experience: &Experience,
    ) -> (Vec<StateFact>, Vec<String>) {
        let mut facts = Vec::new();
        let mut evidence = Vec::new();
        for step in &experience.verification {
            match step {
                VerificationStep::ReadFile {
                    path,
                    expect_content,
                } => {
                    let target = context.cwd.join(path);
                    match fs::read_to_string(&target) {
                        Ok(content) => {
                            facts.push(StateFact::new(
                                format!("file:{path}.exists"),
                                Value::Bool(true),
                                format!("read {path}"),
                                now_secs(),
                            ));
                            if let Some(expected) = expect_content {
                                if content == *expected {
                                    facts.push(StateFact::new(
                                        format!("file:{path}.content"),
                                        Value::String(content.clone()),
                                        format!("read {path}"),
                                        now_secs(),
                                    ));
                                    evidence.push(format!(
                                        "verification: {path} content matches"
                                    ));
                                } else {
                                    evidence.push(format!(
                                        "verification: {path} content mismatch"
                                    ));
                                }
                            }
                        }
                        Err(error) => evidence.push(format!(
                            "verification: failed to read {path}: {error}"
                        )),
                    }
                }
                VerificationStep::Probe { predicate } => {
                    let observed = self.probe.probe(context, &predicate.key);
                    match predicate.probe(observed.as_ref()) {
                        TruthValue::True => {
                            if let Some(value) = observed {
                                facts.push(StateFact::new(
                                    predicate.key.clone(),
                                    value,
                                    "probe".to_string(),
                                    now_secs(),
                                ));
                            }
                            evidence.push(format!("verification: {} holds", predicate.key));
                        }
                        TruthValue::False => evidence.push(format!(
                            "verification: {} does not hold",
                            predicate.key
                        )),
                        TruthValue::Unknown => evidence.push(format!(
                            "verification: {} not observable",
                            predicate.key
                        )),
                    }
                }
            }
        }
        (facts, evidence)
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// P1 tier mapping: one-step workflow = B (replace + verify); longer chains
/// = C. D (replace whole exploration) requires high-confidence + strong
/// state conditions and is not auto-assigned.
fn tier_for(experience: &Experience) -> GateTier {
    match experience.workflow.len() {
        0 => GateTier::B, // unreachable (schema requires >= 1)
        1 => GateTier::B,
        _ => GateTier::C,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use experience_core::domain::action::ActionPattern;
    use experience_core::domain::experience::ExperienceStatus;
    use experience_core::domain::experience::FailurePolicy;
    use experience_core::domain::experience::UndoPolicy;
    use experience_core::domain::experience::WorkflowStep;
    use experience_core::domain::gate::CompletionStatus;
    use experience_core::domain::predicate::Predicate;

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("exp-gate-{tag}-{nanos}"))
    }

    fn probe_file_experience(name: &str) -> Experience {
        Experience {
            name: name.into(),
            trigger: ActionPattern {
                tool: "exec_command".into(),
                command_pattern: Some("create probe file".into()),
            },
            preconditions: vec![Predicate::new("cwd.exists", Value::Bool(true))],
            workflow: vec![WorkflowStep::new(
                "write_file",
                serde_json::json!({
                    "path": "probe.txt",
                    "content": "EXPERIENCE_GATE_SUCCESS"
                }),
            )],
            postconditions: vec![
                Predicate::new("file:probe.txt.exists", Value::Bool(true)),
                Predicate::new(
                    "file:probe.txt.content",
                    Value::String("EXPERIENCE_GATE_SUCCESS".into()),
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

    fn setup(tag: &str) -> (ExperienceGateRuntime, GateContext, PathBuf) {
        let dir = temp_dir(tag);
        fs::create_dir_all(&dir).unwrap();
        let mut store = ExperienceStore::default();
        store.insert(probe_file_experience("create_probe_file")).unwrap();
        let runtime = ExperienceGateRuntime::new(
            store,
            Box::new(super::super::probe::LocalProbe),
            Box::new(super::super::runner::LocalRunner),
        );
        (runtime, GateContext { cwd: dir.clone() }, dir)
    }

    #[test]
    fn decide_hits_when_preconditions_hold() {
        let (runtime, context, _dir) = setup("hit");
        let proposal = ActionProposal::new(
            "exec_command",
            serde_json::json!({ "cmd": "create probe file" }),
        );
        assert!(runtime.decide(&proposal, &context).is_hit());
    }

    #[test]
    fn decide_misses_when_command_does_not_match() {
        let (runtime, context, _dir) = setup("miss");
        let proposal =
            ActionProposal::new("exec_command", serde_json::json!({ "cmd": "dir" }));
        assert_eq!(runtime.decide(&proposal, &context), GateDecision::Miss);
    }

    #[test]
    fn gate_full_cycle_creates_and_verifies_file() {
        let (runtime, context, dir) = setup("cycle");
        let proposal = ActionProposal::new(
            "exec_command",
            serde_json::json!({ "cmd": "create probe file" }),
        );
        let outcome = runtime.gate(&proposal, &context);
        assert!(outcome.is_hit());
        let GateOutcome::Hit(result) = outcome else {
            unreachable!()
        };
        assert!(result.is_success());
        assert_eq!(result.completion_status, CompletionStatus::Completed);
        assert_eq!(
            fs::read_to_string(dir.join("probe.txt")).unwrap(),
            "EXPERIENCE_GATE_SUCCESS"
        );
        assert!(!result.executed.is_empty());
        assert!(!result.executed_side_effects.is_empty());
        assert!(!result.state_after.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn mismatched_verification_returns_partial_not_completed() {
        let dir = temp_dir("partial");
        fs::create_dir_all(&dir).unwrap();
        let mut store = ExperienceStore::default();
        // Workflow writes different content than verification expects.
        let mut experience = probe_file_experience("bad_probe");
        experience.workflow = vec![WorkflowStep::new(
            "write_file",
            serde_json::json!({
                "path": "probe.txt",
                "content": "WRONG CONTENT"
            }),
        )];
        store.insert(experience).unwrap();
        let runtime = ExperienceGateRuntime::new(
            store,
            Box::new(super::super::probe::LocalProbe),
            Box::new(super::super::runner::LocalRunner),
        );
        let context = GateContext { cwd: dir.clone() };
        let proposal = ActionProposal::new(
            "exec_command",
            serde_json::json!({ "cmd": "create probe file" }),
        );
        let GateOutcome::Hit(result) = runtime.gate(&proposal, &context) else {
            panic!("expected hit");
        };
        assert!(!result.is_success());
        assert_eq!(result.completion_status, CompletionStatus::Partial);
        assert!(!result.executed.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn failing_step_stops_workflow_and_reports_failure() {
        let dir = temp_dir("fail");
        fs::create_dir_all(&dir).unwrap();
        let mut store = ExperienceStore::default();
        // exec_command is unsupported by LocalRunner: the step must fail
        // loudly instead of being skipped or silently completed.
        let mut experience = probe_file_experience("failing");
        experience.workflow = vec![WorkflowStep::new(
            "exec_command",
            serde_json::json!({ "cmd": "echo broken" }),
        )];
        store.insert(experience).unwrap();
        let runtime = ExperienceGateRuntime::new(
            store,
            Box::new(super::super::probe::LocalProbe),
            Box::new(super::super::runner::LocalRunner),
        );
        let context = GateContext { cwd: dir.clone() };
        let proposal = ActionProposal::new(
            "exec_command",
            serde_json::json!({ "cmd": "create probe file" }),
        );
        let GateOutcome::Hit(result) = runtime.gate(&proposal, &context) else {
            panic!("expected hit");
        };
        assert!(!result.is_success());
        assert_eq!(result.completion_status, CompletionStatus::Failed);
        assert!(
            result
                .execution_evidence
                .iter()
                .any(|line| line.contains("unsupported workflow action"))
        );
        let _ = fs::remove_dir_all(&dir);
    }
}

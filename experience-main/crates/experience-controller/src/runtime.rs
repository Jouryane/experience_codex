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
use experience_core::domain::experience::WorkflowStep;
use experience_core::domain::experience::VerificationStep;
use experience_core::domain::gate::ExecutedStep;
use experience_core::domain::gate::GateDecision;
use experience_core::domain::gate::GateHitResult;
use experience_core::domain::gate::GateTier;
use experience_core::domain::gate::TemplateBindingAudit;
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
            if let Some(state_after) = self.postcondition_facts(candidate, context) {
                return GateDecision::Satisfied {
                    experience: candidate.name.clone(),
                    tier: tier_for(candidate),
                    state_after,
                };
            }
        }
        for template in self.store.templates_for(proposal) {
            let Ok((instantiated, bindings)) = template.bind(proposal, &context.cwd) else {
                continue;
            };
            if self.preconditions_hold(&instantiated, context) {
                return GateDecision::TemplateHit {
                    experience: instantiated.name.clone(),
                    tier: tier_for(&instantiated),
                    fingerprint: template.binding_fingerprint(&bindings),
                    bindings,
                    instantiated,
                };
            }
            if let Some(state_after) = self.postcondition_facts(&instantiated, context) {
                return GateDecision::Satisfied {
                    experience: instantiated.name.clone(),
                    tier: tier_for(&instantiated),
                    state_after,
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
        if let GateDecision::TemplateHit { instantiated, .. } = decision {
            let audit = match decision {
                GateDecision::TemplateHit {
                    experience,
                    bindings,
                    fingerprint,
                    ..
                } => Some(TemplateBindingAudit {
                    template: experience.clone(),
                    bindings: bindings.clone(),
                    fingerprint: fingerprint.clone(),
                }),
                _ => None,
            };
            let mut result = self.execute_experience(proposal, context, instantiated);
            if let Some(audit) = audit {
                result.execution_evidence.push(format!(
                    "template '{}' bound {} parameter(s); fingerprint {}",
                    audit.template,
                    audit.bindings.len(),
                    audit.fingerprint
                ));
                result = result.with_template_audit(Some(audit));
            }
            return result;
        }
        let name = match decision {
            GateDecision::Hit { experience, .. } => experience,
            GateDecision::Satisfied { experience, .. } => experience,
            GateDecision::TemplateHit { .. } => unreachable!("handled above"),
            GateDecision::Miss => panic!("execute() called on a Miss; gate must pass through"),
        };
        let experience = self
            .store
            .get(name)
            .expect("GateDecision references an experience that left the store");
        match decision {
            GateDecision::Hit { .. } => self.execute_experience(proposal, context, experience),
            GateDecision::Satisfied { state_after, .. } => {
                self.execute_already_satisfied(proposal, state_after.clone())
            }
            GateDecision::TemplateHit { .. } => unreachable!("handled above"),
            GateDecision::Miss => unreachable!("Miss handled above"),
        }
    }

    /// The full synchronous gate: decide then, on Hit, execute+verify+return
    /// inside the same call (step-gate.md §8.2).
    pub fn gate(&self, proposal: &ActionProposal, context: &GateContext) -> GateOutcome {
        match self.decide(proposal, context) {
            GateDecision::Miss => GateOutcome::Miss,
            decision @ (GateDecision::Hit { .. }
            | GateDecision::Satisfied { .. }
            | GateDecision::TemplateHit { .. }) => {
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

    /// Probe every postcondition once. `Some(facts)` means all postconditions
    /// hold and the observed facts can be reused by the no-op result.
    fn postcondition_facts(
        &self,
        experience: &Experience,
        context: &GateContext,
    ) -> Option<Vec<StateFact>> {
        if experience.postconditions.is_empty() {
            return None;
        }
        let mut facts = Vec::with_capacity(experience.postconditions.len());
        for predicate in &experience.postconditions {
            let observed = self.probe.probe(context, &predicate.key)?;
            if predicate.probe(Some(&observed)) != TruthValue::True {
                return None;
            }
            facts.push(StateFact::new(
                predicate.key.clone(),
                observed,
                "probe".to_string(),
                now_secs(),
            ));
        }
        Some(facts)
    }

    fn execute_already_satisfied(
        &self,
        proposal: &ActionProposal,
        state_after: Vec<StateFact>,
    ) -> GateHitResult {
        let mut evidence = vec![
            "experience already satisfied; no workflow step was executed".to_string(),
        ];
        evidence.extend(
            state_after
                .iter()
                .map(|fact| format!("verification: {} holds", fact.key)),
        );
        GateHitResult::completed(
            proposal.clone(),
            Vec::new(),
            Vec::new(),
            state_after,
            evidence,
            Vec::new(),
        )
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
                    // A replayed process step is only "the same step" when it
                    // succeeds the way it succeeded when it was observed. A
                    // non-zero exit means the premise moved; the body must not
                    // keep going and then claim completion from unrelated
                    // postconditions. Authors can opt out per step with
                    // `"expect_exit": <n>`.
                    if let Some(expected) = expected_exit_code(step) {
                        if outcome.exit_code != Some(expected) {
                            execution_evidence.push(format!(
                                "step '{}' exited {:?}, expected {}: {}",
                                step.action, outcome.exit_code, expected, outcome.evidence
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

/// Exit code a process step must produce for the takeover to continue.
///
/// `Some(n)` only for the process capabilities (`exec`, `exec_command`): a
/// file step has no exit code, so it is never constrained by this rule. The
/// default is 0 because an induced body only replays commands that succeeded
/// when they were observed; `"expect_exit": n` overrides that for bodies whose
/// recorded command legitimately returns non-zero.
fn expected_exit_code(step: &WorkflowStep) -> Option<i32> {
    if step.action != "exec" && step.action != "exec_command" {
        return None;
    }
    let declared = step
        .args
        .get("expect_exit")
        .and_then(serde_json::Value::as_i64);
    Some(declared.map(|value| value as i32).unwrap_or(0))
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
            Box::new(super::super::runner::LocalRunner::default()),
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
            Box::new(super::super::runner::LocalRunner::default()),
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
        // exec_command is denied by the default capability policy: the step
        // must fail loudly instead of being skipped or silently completed.
        let mut experience = probe_file_experience("failing");
        experience.workflow = vec![WorkflowStep::new(
            "exec_command",
            serde_json::json!({ "cmd": "echo broken" }),
        )];
        store.insert(experience).unwrap();
        let runtime = ExperienceGateRuntime::new(
            store,
            Box::new(super::super::probe::LocalProbe),
            Box::new(super::super::runner::LocalRunner::default()),
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
                .any(|line| line.contains("policy denied step 'exec_command'"))
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn second_move_is_satisfied_noop_not_a_miss() {
        let dir = temp_dir("satisfied");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("source.txt"), "payload").unwrap();

        let experience = Experience {
            name: "move_source".into(),
            trigger: ActionPattern {
                tool: "exec_command".into(),
                command_pattern: Some("move source".into()),
            },
            preconditions: vec![Predicate::new(
                "file:source.txt.exists",
                serde_json::json!(true),
            )],
            workflow: vec![WorkflowStep::new(
                "move_file",
                serde_json::json!({
                    "source": "source.txt",
                    "target": "target.txt"
                }),
            )],
            postconditions: vec![
                Predicate::new("file:source.txt.exists", serde_json::json!(false)),
                Predicate::new("file:target.txt.exists", serde_json::json!(true)),
            ],
            verification: vec![
                VerificationStep::Probe {
                    predicate: Predicate::new(
                        "file:source.txt.exists",
                        serde_json::json!(false),
                    ),
                },
                VerificationStep::Probe {
                    predicate: Predicate::new(
                        "file:target.txt.exists",
                        serde_json::json!(true),
                    ),
                },
            ],
            failure_policy: FailurePolicy::StopAndReport,
            undo: UndoPolicy::Unsupported,
            status: ExperienceStatus::Active,
        };
        let mut store = ExperienceStore::default();
        store.insert(experience).unwrap();
        let runtime = ExperienceGateRuntime::new(
            store,
            Box::new(super::super::probe::LocalProbe),
            Box::new(super::super::runner::LocalRunner::default()),
        );
        let context = GateContext { cwd: dir.clone() };
        let proposal = ActionProposal::new(
            "exec_command",
            serde_json::json!({ "cmd": "move source" }),
        );

        let GateOutcome::Hit(first) = runtime.gate(&proposal, &context) else {
            panic!("first call must execute");
        };
        assert!(first.is_success());
        assert!(!first.executed.is_empty());
        assert!(!dir.join("source.txt").exists());
        assert!(dir.join("target.txt").exists());

        let decision = runtime.decide(&proposal, &context);
        assert!(decision.is_satisfied(), "second call must be satisfied");
        let GateOutcome::Hit(second) = runtime.gate(&proposal, &context) else {
            panic!("satisfied call must still return a closed result");
        };
        assert!(second.is_success());
        assert!(second.executed.is_empty());
        assert!(dir.join("target.txt").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn template_binds_and_executes_in_one_synchronous_gate() {
        use experience_core::domain::template::ExperienceTemplate;
        use experience_core::domain::template::TemplateParameter;

        let dir = temp_dir("template");
        fs::create_dir_all(dir.join("inbox")).unwrap();
        fs::write(dir.join("inbox/a.pdf"), "payload").unwrap();

        let template = ExperienceTemplate {
            name: "move_any_file".into(),
            trigger: ActionPattern {
                tool: "task".into(),
                command_pattern: Some("M9 move".into()),
            },
            parameters: vec![
                TemplateParameter {
                    name: "source".into(),
                    source: "task".into(),
                    kind: experience_core::domain::template::PARAM_KIND_PATH.into(),
                    prefix: Some("[source=".into()),
                    suffix: Some("]".into()),
                    required: true,
                },
                TemplateParameter {
                    name: "target".into(),
                    source: "task".into(),
                    kind: experience_core::domain::template::PARAM_KIND_PATH.into(),
                    prefix: Some("[target=".into()),
                    suffix: Some("]".into()),
                    required: true,
                },
            ],
            workflow: vec![WorkflowStep::new(
                "move_file",
                serde_json::json!({
                    "source": "${source}",
                    "target": "${target}"
                }),
            )],
            preconditions: vec![Predicate::new(
                "file:${source}.exists",
                serde_json::json!(true),
            )],
            postconditions: vec![
                Predicate::new("file:${source}.exists", serde_json::json!(false)),
                Predicate::new("file:${target}.exists", serde_json::json!(true)),
            ],
            verification: vec![
                VerificationStep::Probe {
                    predicate: Predicate::new(
                        "file:${source}.exists",
                        serde_json::json!(false),
                    ),
                },
                VerificationStep::Probe {
                    predicate: Predicate::new(
                        "file:${target}.exists",
                        serde_json::json!(true),
                    ),
                },
            ],
            failure_policy: FailurePolicy::StopAndReport,
            undo: UndoPolicy::Unsupported,
            status: ExperienceStatus::Active,
        };
        let mut store = ExperienceStore::default();
        store.insert_template(template).unwrap();
        let runtime = ExperienceGateRuntime::new(
            store,
            Box::new(super::super::probe::LocalProbe),
            Box::new(super::super::runner::LocalRunner::default()),
        );
        let context = GateContext { cwd: dir.clone() };
        let proposal = ActionProposal::new(
            "task",
            serde_json::json!({
                "task": "M9 move [source=inbox/a.pdf] [target=library/a.pdf]"
            }),
        );

        let decision = runtime.decide(&proposal, &context);
        let GateDecision::TemplateHit {
            bindings,
            fingerprint,
            ..
        } = &decision
        else {
            panic!("expected template hit, got {decision:?}");
        };
        assert_eq!(bindings["source"], "inbox/a.pdf");
        assert_eq!(bindings["target"], "library/a.pdf");
        assert_eq!(fingerprint.len(), 16, "stable fingerprint is 16 hex chars");

        let GateOutcome::Hit(result) = runtime.gate(&proposal, &context) else {
            panic!("template gate must execute");
        };
        assert!(result.is_success());
        // Audit: the takeover records exactly which template bound which
        // values, with no model participation anywhere in the path.
        let audit = result
            .template_audit
            .as_ref()
            .expect("template takeover must carry a binding audit");
        assert_eq!(audit.template, "move_any_file");
        assert_eq!(audit.bindings["source"], "inbox/a.pdf");
        assert_eq!(audit.fingerprint, *fingerprint);
        assert!(
            result
                .execution_evidence
                .iter()
                .any(|line| line.contains("template 'move_any_file'")),
            "{:?}",
            result.execution_evidence
        );
        assert!(!dir.join("inbox/a.pdf").exists());
        assert!(dir.join("library/a.pdf").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn exact_hit_carries_no_template_audit() {
        let (runtime, context, dir) = setup("exact-no-audit");
        let proposal = ActionProposal::new(
            "exec_command",
            serde_json::json!({ "cmd": "create probe file" }),
        );
        let GateOutcome::Hit(result) = runtime.gate(&proposal, &context) else {
            panic!("exact hit expected");
        };
        assert!(result.is_success());
        assert!(result.template_audit.is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    fn process_experience(script: &str, verification: Vec<VerificationStep>) -> Experience {
        Experience {
            name: "process_probe".into(),
            trigger: ActionPattern {
                tool: "exec_command".into(),
                command_pattern: Some("run process probe".into()),
            },
            preconditions: vec![Predicate::new("cwd.exists", Value::Bool(true))],
            workflow: vec![WorkflowStep::new(
                "exec",
                serde_json::json!({ "program": "python", "args": ["-c", script] }),
            )],
            postconditions: vec![Predicate::new("cwd.exists", Value::Bool(true))],
            verification,
            failure_policy: FailurePolicy::StopAndReport,
            undo: UndoPolicy::Unsupported,
            status: ExperienceStatus::Active,
        }
    }

    fn process_runtime(experience: Experience, dir: &PathBuf) -> (ExperienceGateRuntime, GateContext) {
        let mut policy = experience_core::policy::CapabilityPolicy::default();
        policy.exec.mode = experience_core::policy::EXEC_ALLOWLIST.to_string();
        policy.exec.allow = vec!["python".to_string()];
        let mut store = ExperienceStore::default();
        store.insert(experience).unwrap();
        let runtime = ExperienceGateRuntime::new(
            store,
            Box::new(super::super::probe::LocalProbe),
            Box::new(super::super::runner::LocalRunner::with_policy(policy)),
        );
        (runtime, GateContext { cwd: dir.clone() })
    }

    #[test]
    fn a_process_step_that_exits_non_zero_fails_the_takeover() {
        let dir = temp_dir("exec-nonzero");
        fs::create_dir_all(&dir).unwrap();
        let experience = process_experience(
            "import sys; sys.exit(3)",
            vec![VerificationStep::Probe {
                predicate: Predicate::new("cwd.exists", Value::Bool(true)),
            }],
        );
        let (runtime, context) = process_runtime(experience, &dir);
        let proposal = ActionProposal::new(
            "exec_command",
            serde_json::json!({ "cmd": "run process probe" }),
        );
        let GateOutcome::Hit(result) = runtime.gate(&proposal, &context) else {
            panic!("must be a hit");
        };
        assert!(!result.is_success(), "{:?}", result.execution_evidence);
        assert_eq!(result.completion_status, CompletionStatus::Failed);
        assert!(
            result
                .execution_evidence
                .iter()
                .any(|line| line.contains("expected 0")),
            "{:?}",
            result.execution_evidence
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_process_step_that_exits_zero_is_verified_by_its_assertions() {
        let dir = temp_dir("exec-zero");
        fs::create_dir_all(&dir).unwrap();
        let experience = process_experience(
            "print('probe ok')",
            vec![VerificationStep::Probe {
                predicate: Predicate::new("cwd.exists", Value::Bool(true)),
            }],
        );
        let (runtime, context) = process_runtime(experience, &dir);
        let proposal = ActionProposal::new(
            "exec_command",
            serde_json::json!({ "cmd": "run process probe" }),
        );
        let GateOutcome::Hit(result) = runtime.gate(&proposal, &context) else {
            panic!("must be a hit");
        };
        assert!(result.is_success(), "{:?}", result.execution_evidence);
        assert_eq!(result.completion_status, CompletionStatus::Completed);
        let _ = fs::remove_dir_all(&dir);
    }
}

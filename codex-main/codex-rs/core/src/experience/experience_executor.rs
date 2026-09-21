//! A4: deterministic executor for experience workflows.
//!
//! Executes `workflow.steps` in order while maintaining the task execution
//! state (`executed_steps`, `last_action`, `last_result`, `task.status`).
//! `task.status == Completed` is the ONLY basis for skipping the LLM — this
//! module produces exactly that signal.

use super::experience::Experience;
use super::experience::ExperienceWorkflowStep;
use super::experience_state::ActionResult;
use super::experience_state::ElementSource;
use super::experience_state::ExecutionMode;
use super::experience_state::ExperienceAction;
use super::experience_state::ExperienceState;
use super::experience_state::Task;
use super::experience_state::TaskStatus;
use super::process_log::ProcessStep;
use futures::future::BoxFuture;

/// Outcome of executing an experience workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionOutcome {
    /// All steps succeeded and the task is complete: the LLM is not needed.
    Completed,
    /// A step failed: the LLM should intervene (with the execution state).
    Failed,
    /// The workflow has no executable steps: nothing was deployed, so the LLM
    /// must decide.
    NeedsLlm,
}

/// How a single workflow step is executed. The session layer provides the real
/// implementation backed by ToolCallRuntime / exec channel; tests provide a
/// fake. The executor itself stays deterministic and session-independent.
/// Async because real tool execution (approvals / sandbox / streaming) is
/// inherently async.
pub trait ExperienceActionRunner: Send {
    fn run<'a>(&'a mut self, step: &'a ExperienceWorkflowStep) -> BoxFuture<'a, ActionResult>;
}

/// Execute an experience's workflow against the current state.
pub async fn execute_workflow(
    state: &mut ExperienceState,
    experience: &Experience,
    runner: &mut dyn ExperienceActionRunner,
) -> ExecutionOutcome {
    if experience.workflow.steps.is_empty() {
        match &mut state.task {
            Some(task) => task.status = TaskStatus::NeedsLlm,
            None => {
                state.task = Some(Task {
                    description: experience.name.clone(),
                    status: TaskStatus::NeedsLlm,
                });
            }
        }
        state.execution_mode = ExecutionMode::Conflict;
        return ExecutionOutcome::NeedsLlm;
    }

    state.active_experience = Some(experience.id.clone());
    state.execution_mode = ExecutionMode::Reflex;
    state
        .task
        .get_or_insert_with(|| Task {
            description: experience.name.clone(),
            status: TaskStatus::InProgress,
        })
        .status = TaskStatus::InProgress;

    for step in &experience.workflow.steps {
        let result = runner.run(step).await;
        state.executed_steps.push(step.name.clone());
        state.last_action = Some(ExperienceAction {
            name: step.name.clone(),
            args: step.args.clone(),
        });
        if let Some(deployed) = &result.deployed {
            state.deployed.push(deployed.clone());
            state.upsert_element(super::experience_state::StateElement {
                key: deployed.key.clone(),
                value: deployed.value.clone(),
                source: ElementSource::Deployed,
                verified_at: None,
                ttl: None,
            });
        }
        state.last_result = Some(result.clone());
        state.process_log.push(ProcessStep::experience(
            step.name.clone(),
            format!("ok={} {}", result.ok, result.summary),
        ));
        if !result.ok {
            state.task.as_mut().unwrap().status = TaskStatus::Failed;
            state.execution_mode = ExecutionMode::Conflict;
            return ExecutionOutcome::Failed;
        }
    }

    state.task.as_mut().unwrap().status = TaskStatus::Completed;
    ExecutionOutcome::Completed
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use serde_json::json;

    struct FakeRunner {
        fail_on: Option<usize>,
        calls: usize,
    }

    impl ExperienceActionRunner for FakeRunner {
        fn run<'a>(
            &'a mut self,
            _step: &'a ExperienceWorkflowStep,
        ) -> BoxFuture<'a, ActionResult> {
            Box::pin(async move {
                self.calls += 1;
                let ok = self.fail_on != Some(self.calls);
                ActionResult {
                    ok,
                    summary: format!("step {}", self.calls),
                    deployed: None,
                }
            })
        }
    }

    fn experience_with_steps(names: &[&str]) -> Experience {
        Experience {
            id: super::super::experience_state::ExperienceId("e1".to_string()),
            name: "test".to_string(),
            kind: super::super::experience::ExperienceKind::Reflex,
            trigger: Default::default(),
            conditions: Vec::new(),
            workflow: super::super::experience::ExperienceWorkflow {
                steps: names
                    .iter()
                    .map(|name| ExperienceWorkflowStep {
                        name: name.to_string(),
                        args: json!({}),
                    })
                    .collect(),
            },
            completion_criteria: None,
            failure_modes: Vec::new(),
            assets: Default::default(),
            applicability: Default::default(),
            title: None,
            note: None,
            created_at: None,
            confidence: 0.9,
            risk: super::super::experience_state::RiskLevel::Low,
            status: super::super::experience::ExperienceStatus::Active,
            version: 1,
        }
    }

    fn state_with_task() -> ExperienceState {
        let mut state = ExperienceState::default();
        state.task = Some(Task {
            description: "move pdfs to archive".to_string(),
            status: TaskStatus::Pending,
        });
        state
    }

    #[test]
    fn all_steps_ok_completes_task() {
        let mut state = state_with_task();
        let exp = experience_with_steps(&["scan_files", "move_file"]);
        let mut runner = FakeRunner { fail_on: None, calls: 0 };
        let outcome = block_on(execute_workflow(&mut state, &exp, &mut runner));
        assert_eq!(outcome, ExecutionOutcome::Completed);
        assert_eq!(state.task.unwrap().status, TaskStatus::Completed);
        assert_eq!(state.executed_steps, vec!["scan_files", "move_file"]);
        assert_eq!(state.last_action.unwrap().name, "move_file");
        assert_eq!(state.process_log.len(), 2);
        assert_eq!(state.process_log[0].action, "scan_files");
        assert_eq!(state.process_log[1].action, "move_file");
    }

    #[test]
    fn failing_step_stops_and_marks_failed() {
        let mut state = state_with_task();
        let exp = experience_with_steps(&["scan_files", "move_file", "cleanup"]);
        let mut runner = FakeRunner { fail_on: Some(2), calls: 0 };
        let outcome = block_on(execute_workflow(&mut state, &exp, &mut runner));
        assert_eq!(outcome, ExecutionOutcome::Failed);
        assert_eq!(state.task.unwrap().status, TaskStatus::Failed);
        assert_eq!(state.executed_steps, vec!["scan_files", "move_file"]);
        assert_eq!(runner.calls, 2);
    }

    #[test]
    fn empty_workflow_needs_llm() {
        let mut state = state_with_task();
        let exp = experience_with_steps(&[]);
        let mut runner = FakeRunner { fail_on: None, calls: 0 };
        let outcome = block_on(execute_workflow(&mut state, &exp, &mut runner));
        assert_eq!(outcome, ExecutionOutcome::NeedsLlm);
        assert_eq!(state.task.unwrap().status, TaskStatus::NeedsLlm);
        assert_eq!(runner.calls, 0);
    }

    #[test]
    fn missing_task_is_created_from_experience() {
        let mut state = ExperienceState::default();
        let exp = experience_with_steps(&["scan_files"]);
        let mut runner = FakeRunner { fail_on: None, calls: 0 };
        let outcome = block_on(execute_workflow(&mut state, &exp, &mut runner));
        assert_eq!(outcome, ExecutionOutcome::Completed);
        assert_eq!(state.task.unwrap().status, TaskStatus::Completed);
    }

    #[test]
    fn deployment_action_records_into_state() {
        struct DeployRunner;

        impl ExperienceActionRunner for DeployRunner {
            fn run<'a>(
                &'a mut self,
                step: &'a ExperienceWorkflowStep,
            ) -> futures::future::BoxFuture<'a, ActionResult> {
                Box::pin(async move {
                    let value = step
                        .args
                        .get("value")
                        .cloned()
                        .unwrap_or(serde_json::json!(null));
                    ActionResult {
                        ok: true,
                        summary: format!("deployed {}", step.name),
                        deployed: Some(super::super::experience_state::DeployedElement {
                            key: step
                                .args
                                .get("key")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            value,
                            action: step.name.clone(),
                            ok: true,
                            at: "t0".to_string(),
                        }),
                    }
                })
            }
        }

        let mut state = state_with_task();
        let mut exp = experience_with_steps(&["deploy.env"]);
        exp.workflow.steps[0].args = json!({ "key": "toolchain.override", "value": "stable-x86_64-pc-windows-gnu" });
        let mut runner = DeployRunner;
        let outcome = block_on(execute_workflow(&mut state, &exp, &mut runner));
        assert_eq!(outcome, ExecutionOutcome::Completed);
        assert_eq!(state.deployed.len(), 1);
        assert_eq!(state.deployed[0].key, "toolchain.override");
        assert_eq!(
            state.lookup("toolchain.override"),
            Some(json!("stable-x86_64-pc-windows-gnu"))
        );
    }
}

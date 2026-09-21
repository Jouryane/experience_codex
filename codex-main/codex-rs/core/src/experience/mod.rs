//! Experience subsystem: State + Experience + LLM three-layer control.
//!
//! The Experience Runtime sits *before* the LLM in the agent loop and owns the
//! decision of whether the LLM is invoked at all. In this first cut the runtime
//! is a skeleton: `experience_state` defines the state shape, and the gate in
//! `session/turn.rs` (`run_turn`) calls `should_call_llm`, which always returns
//! `InvokeLLM` until matcher / workflow / confidence / store land in later
//! phases.

mod experience_runtime;
mod experience;
mod experience_confidence;
mod experience_executor;
mod experience_learner;
mod experience_lifecycle;
mod experience_matcher;
mod process_log;
mod experience_store;
mod experience_state;

pub(crate) use experience_confidence::ConfidenceCounters;
pub(crate) use experience_confidence::ConfidenceLevel;
pub(crate) use experience_confidence::ConfidenceWeights;
pub(crate) use experience_confidence::FeedbackKind;
pub(crate) use experience_executor::ExecutionOutcome;
pub(crate) use experience_executor::ExperienceActionRunner;
pub(crate) use experience_executor::execute_workflow;
pub(crate) use experience_learner::ExperienceDraft;
pub(crate) use experience_learner::ExperienceLearner;
pub(crate) use experience_learner::ExperienceTrace;
pub(crate) use experience_learner::TraceAction;
pub(crate) use experience_learner::TraceOutcome;
pub(crate) use experience_learner::TraceRecorder;
pub(crate) use experience_learner::TraceStep;
pub(crate) use experience_learner::MAX_AUTO_CONFIRM_STEPS;
pub(crate) use experience_lifecycle::ExperienceLifecycle;
pub(crate) use experience_lifecycle::ForgettingDecision;
pub(crate) use experience_lifecycle::LifecycleAction;
pub(crate) use experience_lifecycle::LifecycleError;
pub(crate) use experience_runtime::ControlDecision;
pub(crate) use experience_runtime::DecisionInput;
pub(crate) use experience_runtime::ExperienceCandidate;
pub(crate) use experience_runtime::ExperienceManagementEntry;
pub(crate) use experience_runtime::ExperienceRuntime;
pub(crate) use experience_runtime::TickReport;
pub(crate) use experience_matcher::content_similarity;
pub(crate) use experience_matcher::match_experiences;
pub(crate) use process_log::ProcessActor;
pub(crate) use process_log::ProcessStep;
pub(crate) use process_log::render_process_log;
pub(crate) use experience_store::ExperienceStore;
pub(crate) use experience::Experience;
pub(crate) use experience::Applicability;
pub(crate) use experience::ExperienceCondition;
pub(crate) use experience::ExperienceKind;
pub(crate) use experience::ExperienceStatus;
pub(crate) use experience::ExperienceTrigger;
pub(crate) use experience::ExperienceWorkflow;
pub(crate) use experience::ExperienceWorkflowStep;
pub(crate) use experience::InputScope;
pub(crate) use experience_state::ActionResult;
pub(crate) use experience_state::ConfigState;
pub(crate) use experience_state::DeployedElement;
pub(crate) use experience_state::ElementSource;
pub(crate) use experience_state::EnvironmentDetection;
pub(crate) use experience_state::EnvironmentState;
pub(crate) use experience_state::ExperienceContext;
pub(crate) use experience_state::ExperienceId;
pub(crate) use experience_state::ExperienceState;
pub(crate) use experience_state::Goal;
pub(crate) use experience_state::NetworkState;
pub(crate) use experience_state::RiskLevel;
pub(crate) use experience_state::StateElement;
pub(crate) use experience_state::Task;
pub(crate) use experience_state::TaskStatus;
pub(crate) use experience_state::ToolState;

/// Explicit master switch (next-phase requirement): when disabled, the whole
/// experience system is bypassed — matching, execution and learning are all
/// skipped so the agent behaves exactly like the original Codex loop.
/// Default: enabled. Set EXPERIENCE_ENABLED=0/false/no/off to disable.
pub(crate) fn enabled() -> bool {
    match std::env::var("EXPERIENCE_ENABLED") {
        Ok(value) => !matches!(
            value.trim().to_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        ),
        Err(_) => true,
    }
}

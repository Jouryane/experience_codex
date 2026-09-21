//! A1: State construction — build `ExperienceState` from real turn/step data.
//!
//! The experience module itself stays independent of session internals; this
//! adapter is the only place that maps session types into `ExperienceState`.
//! Everything filled here is cheap and deterministic: no LLM involvement.

use codex_protocol::user_input::UserInput;

use crate::experience::ConfigState;
use crate::experience::ElementSource;
use crate::experience::EnvironmentDetection;
use crate::experience::EnvironmentState;
use crate::experience::ExperienceContext;
use crate::experience::ExperienceState;
use crate::experience::Goal;
use crate::experience::NetworkState;
use crate::experience::StateElement;
use crate::experience::Task;
use crate::experience::TaskStatus;
use crate::experience::ToolState;
use crate::session::step_context::StepContext;
use crate::session::turn_context::TurnContext;

/// A1: populate the state projection from the current turn and step.
pub(crate) fn build_experience_state(
    turn_context: &TurnContext,
    step_context: &StepContext,
    user_input: &[UserInput],
) -> ExperienceState {
    let intent = user_text(user_input);
    let mut state = ExperienceState {
        current_goal: Some(Goal {
            description: intent.clone(),
        }),
        current_context: ExperienceContext {
            environment_key: None,
            intent: Some(intent.clone()),
        },
        task: (!intent.is_empty()).then(|| Task {
            description: intent.clone(),
            status: TaskStatus::Pending,
        }),
        ..ExperienceState::default()
    };
    refresh_state_elements(&mut state, turn_context, step_context);
    probe_runtime_capabilities(&mut state);
    state
}

/// A1 (loop variant): refresh the cheap state elements from the current step
/// while PRESERVING task / executed_steps / active_experience / last_action /
/// last_result — this gives cross-sampling continuity inside `run_turn`.
pub(crate) fn update_experience_state(
    state: &mut ExperienceState,
    turn_context: &TurnContext,
    step_context: &StepContext,
) {
    refresh_state_elements(state, turn_context, step_context);
}

/// Fill the deterministic state elements (environment / network / tools /
/// config / detections). No LLM involvement.
fn refresh_state_elements(
    state: &mut ExperienceState,
    turn_context: &TurnContext,
    step_context: &StepContext,
) {
    let mut environment = EnvironmentState::default();
    let mut environment_key: Option<String> = None;
    let mut workspace_roots: Vec<String> = Vec::new();
    let mut platform_os: Option<String> = None;
    for turn_environment in step_context.environments.turn_environments() {
        let environment_id = turn_environment.selection().environment_id.clone();
        environment.environment_id = Some(environment_id.clone());
        environment.cwd = Some(turn_environment.cwd().inferred_native_path_string());
        environment_key = Some(environment_id);
        platform_os = turn_environment.executor_platform_os.clone();
        workspace_roots = turn_environment
            .workspace_roots()
            .iter()
            .map(|path| path.inferred_native_path_string())
            .collect();
        // v1: use the first ready environment.
        // TODO(phase 4): map all environments and run cheap detections
        // (git repo, dependency dirs, processes) into detections.
        break;
    }
    environment.sandbox_level = Some(format!("{:?}", turn_context.windows_sandbox_level));

    // Cheap deterministic detections (v1): platform OS + git repo presence.
    let mut detections = Vec::new();
    if let Some(os) = platform_os {
        detections.push(EnvironmentDetection {
            key: "os".to_string(),
            present: true,
            detail: Some(os),
        });
    }
    if let Some(cwd) = environment.cwd.as_ref() {
        let git_present = std::path::Path::new(cwd).join(".git").exists();
        detections.push(EnvironmentDetection {
            key: "git_repo".to_string(),
            present: git_present,
            detail: None,
        });
    }
    environment.detections = detections;

    let tools = ToolState {
        available: step_context
            .selected_capability_roots
            .iter()
            .map(|root| root.selected_root().id.clone())
            .collect(),
    };

    state.environment = environment;
    state.current_context.environment_key = environment_key;
    state.network = NetworkState {
        policy: turn_context
            .network
            .as_ref()
            .map(|_| "configured".to_string()),
        proxy: None,
        reachability: None,
    };
    state.tools = tools;
    state.config = ConfigState {
        workspace_roots,
        entries: state.config.entries.clone(),
    };
}

/// Probe runtime capabilities once per turn (v1: can this environment spawn
/// subprocesses?). Sandboxes that forbid subprocess generation — the
/// cargo-check ordeal — become a checkable state element so experiences can
/// decide before attempting any build/compile deployment.
fn probe_runtime_capabilities(state: &mut ExperienceState) {
    // v1 probe; in restrictive sandboxes the spawn may fail — that failure IS
    // the signal we want to record. A timeout wrapper is a later refinement.
    let allowed = std::process::Command::new("cmd")
        .args(["/c", "exit 0"])
        .output()
        .is_ok();
    let verified_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .ok();
    state.upsert_element(StateElement {
        key: "runtime.subprocess_allowed".to_string(),
        value: serde_json::Value::Bool(allowed),
        source: ElementSource::Detection,
        verified_at,
        ttl: None,
    });
}

/// Concatenate the user's text / mentions into a single intent string.
pub(crate) fn user_text(user_input: &[UserInput]) -> String {
    user_input
        .iter()
        .filter_map(|input| match input {
            UserInput::Text { text, .. } => Some(text.clone()),
            UserInput::Mention { path, .. } => Some(path.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

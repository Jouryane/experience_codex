//! Experience State: the minimal projection of "where the agent is right now".
//!
//! This is deliberately NOT chat history, NOT RAG, NOT memory, and NOT prompt
//! context. It exists so the Experience Runtime can decide — before the LLM is
//! invoked — whether a stored experience already knows how to act.

use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;

use super::process_log::render_process_log;
use super::process_log::ProcessStep;

/// Stable identifier of a stored experience (lifecycle in later phases:
/// NEW -> CANDIDATE -> VALIDATED -> ACTIVE -> DECAYING -> DISABLED).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ExperienceId(pub String);

impl std::fmt::Display for ExperienceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// The agent's current goal. First version is free-form; later phases may
/// structure it (and connect it to a goal store).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Goal {
    pub description: String,
}

/// Situational context used by the experience matcher. First version is a
/// deliberately small projection (environment key + intent excerpt).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExperienceContext {
    /// Coarse environment key (e.g. os + cwd + tool set) for matching.
    pub environment_key: Option<String>,
    /// Free-form user intent excerpt.
    pub intent: Option<String>,
}

/// One detected fact about the environment (environment detection), e.g.
/// "git_repo present", "node_modules present", "browser reachable".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvironmentDetection {
    pub key: String,
    pub present: bool,
    pub detail: Option<String>,
}

/// Environment state elements that experiences depend on.
///
/// First-principles contract: a HIT must be able to restore or skip the
/// environment setup the experience relies on (cwd, sandbox, detections)
/// without spending LLM tokens.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvironmentState {
    pub environment_id: Option<String>,
    pub cwd: Option<String>,
    pub sandbox_level: Option<String>,
    pub detections: Vec<EnvironmentDetection>,
}

/// Network state elements that experiences depend on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkState {
    pub policy: Option<String>,
    pub proxy: Option<String>,
    pub reachability: Option<String>,
}

/// Tool/capability state elements that experiences depend on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolState {
    pub available: Vec<String>,
}

/// Config state elements (workspace roots, project settings).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigState {
    pub workspace_roots: Vec<String>,
    pub entries: HashMap<String, String>,
}

/// Provenance of a state element (for freshness / audit / LLM inspection).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ElementSource {
    Turn,
    Step,
    Detection,
    Deployed,
    Config,
}

/// One checkable / deployable state element in the unified registry.
///
/// The registry is the single surface shared by condition checking
/// (`satisfies`), deployment (`deploy.*` actions) and LLM inspection
/// (`lookup` / `handoff_payload`). Elements with a TTL decay and must be
/// re-probed (drives the Phase 9 state-machine linkage).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateElement {
    pub key: String,
    pub value: serde_json::Value,
    pub source: ElementSource,
    pub verified_at: Option<u64>,
    pub ttl: Option<u64>,
}

/// What an experience deployment action actually applied (LLM-readable).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeployedElement {
    pub key: String,
    pub value: serde_json::Value,
    pub action: String,
    pub ok: bool,
    pub at: String,
}

/// Execution status of the current task. This is the observable signal that
/// decides whether the LLM must intervene after an experience attempt:
/// `Completed` is the ONLY basis for skipping the LLM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
    NeedsLlm,
}

impl Default for TaskStatus {
    fn default() -> Self {
        TaskStatus::Pending
    }
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            TaskStatus::Pending => "pending",
            TaskStatus::InProgress => "in_progress",
            TaskStatus::Completed => "completed",
            TaskStatus::Failed => "failed",
            TaskStatus::NeedsLlm => "needs_llm",
        };
        f.write_str(s)
    }
}

/// The task itself: what the agent is trying to accomplish, plus its status.
/// The LLM reads this (together with `executed_steps`) when it intervenes
/// after the experience attempt's time lag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub description: String,
    pub status: TaskStatus,
}

/// Which of the three agent behaviors is active right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionMode {
    /// Experience match + confidence above threshold: act without the LLM.
    Reflex,
    /// No applicable experience: the LLM deliberates.
    Deliberation,
    /// Experience conflicts with the current state: stop and ask / delegate.
    Conflict,
}

impl Default for ExecutionMode {
    fn default() -> Self {
        ExecutionMode::Deliberation
    }
}

/// Risk level declared by an experience and assessed for the current state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RiskLevel {
    Low,
    Medium,
    High,
}

impl Default for RiskLevel {
    fn default() -> Self {
        RiskLevel::Low
    }
}

/// One executable step of an experience workflow, or the last action taken.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExperienceAction {
    pub name: String,
    pub args: serde_json::Value,
}

/// Result of the last executed action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionResult {
    pub ok: bool,
    pub summary: String,
    /// Set when the action deployed a state element (the executor records it
    /// into `ExperienceState::deployed` and the element registry).
    pub deployed: Option<DeployedElement>,
}

/// Describes the agent's current situation for the Experience Runtime.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExperienceState {
    pub current_goal: Option<Goal>,
    pub current_context: ExperienceContext,
    pub environment: EnvironmentState,
    pub network: NetworkState,
    pub tools: ToolState,
    pub config: ConfigState,
    /// Unified state-element registry (runtime probes, detections, deployed).
    pub elements: Vec<StateElement>,
    /// Elements deployed by the active experience (质疑权 / handoff 基础).
    pub deployed: Vec<DeployedElement>,
    /// Task process log: who did what (experience / LLM / tool), in order
    /// (M2-4 "任务详细过程").
    pub process_log: Vec<ProcessStep>,
    pub task: Option<Task>,
    pub executed_steps: Vec<String>,
    pub active_experience: Option<ExperienceId>,
    pub execution_mode: ExecutionMode,
    pub confidence: f32,
    pub risk: RiskLevel,
    pub last_action: Option<ExperienceAction>,
    pub last_result: Option<ActionResult>,
}

impl ExperienceState {
    /// Unified key lookup over the state elements.
    ///
    /// The state is both the condition source for experiences AND the
    /// inspection surface for the LLM: when a condition passes, the LLM does
    /// not need to read the state; when it fails and the LLM intervenes, it
    /// can read the very same keys.
    pub fn lookup(&self, key: &str) -> Option<serde_json::Value> {
        let s = |v: &str| serde_json::Value::String(v.to_string());
        match key {
            "goal.description" => self.current_goal.as_ref().map(|g| s(&g.description)),
            "context.intent" => self.current_context.intent.as_ref().map(|v| s(v)),
            "context.environment_key" => self.current_context.environment_key.as_ref().map(|v| s(v)),
            "environment.id" => self.environment.environment_id.as_ref().map(|v| s(v)),
            "environment.cwd" => self.environment.cwd.as_ref().map(|v| s(v)),
            "environment.sandbox_level" => self.environment.sandbox_level.as_ref().map(|v| s(v)),
            "network.policy" => self.network.policy.as_ref().map(|v| s(v)),
            "network.proxy" => self.network.proxy.as_ref().map(|v| s(v)),
            "network.reachability" => self.network.reachability.as_ref().map(|v| s(v)),
            "tools.available" => Some(serde_json::Value::Array(
                self.tools
                    .available
                    .iter()
                    .map(|v| serde_json::Value::String(v.clone()))
                    .collect(),
            )),
            "config.workspace_roots" => Some(serde_json::Value::Array(
                self.config
                    .workspace_roots
                    .iter()
                    .map(|v| serde_json::Value::String(v.clone()))
                    .collect(),
            )),
            "config.entries" => serde_json::to_value(&self.config.entries).ok(),
            "task.description" => self.task.as_ref().map(|t| s(&t.description)),
            "task.status" => self.task.as_ref().map(|t| s(&t.status.to_string())),
            "task.completed" => self
                .task
                .as_ref()
                .map(|t| serde_json::Value::Bool(t.status == TaskStatus::Completed)),
            "executed_steps" => Some(serde_json::Value::Array(
                self.executed_steps
                    .iter()
                    .map(|v| serde_json::Value::String(v.clone()))
                    .collect(),
            )),
            "last_action.name" => self.last_action.as_ref().map(|a| s(&a.name)),
            "last_result.ok" => self.last_result.as_ref().map(|r| serde_json::Value::Bool(r.ok)),
            _ => {
                if let Some(detection_key) = key.strip_prefix("environment.detection.") {
                    self.environment
                        .detections
                        .iter()
                        .find(|d| d.key == detection_key)
                        .map(|d| serde_json::Value::Bool(d.present))
                } else {
                    self.elements
                        .iter()
                        .find(|element| element.key == key)
                        .map(|element| element.value.clone())
                }
            }
        }
    }

    /// Whether a condition (`key == expected`) is satisfied by the current
    /// state. This is the non-LLM check path.
    pub fn satisfies(&self, key: &str, expected: &serde_json::Value) -> bool {
        self.lookup(key).as_ref() == Some(expected)
    }

    /// Insert or replace a registry element by key.
    pub fn upsert_element(&mut self, element: StateElement) {
        if let Some(existing) = self
            .elements
            .iter_mut()
            .find(|existing| existing.key == element.key)
        {
            *existing = element;
        } else {
            self.elements.push(element);
        }
    }

    /// Carry session-persistent parts from a previous state into a freshly
    /// built turn state (M2-2: experience and LLM share one continuing
    /// ledger).
    ///
    /// Inherited: registry elements, deployed traces, and the environment /
    /// network / tools / config baselines (refreshed by the step anyway).
    /// Turn-scoped fields are NOT overwritten — except `task`/`executed_steps`
    /// which are inherited when the new input is empty (a continuation turn).
    pub fn inherit_session(&mut self, previous: &ExperienceState) {
        self.elements = previous.elements.clone();
        self.deployed = previous.deployed.clone();
        self.environment = previous.environment.clone();
        self.network = previous.network.clone();
        self.tools = previous.tools.clone();
        self.config = previous.config.clone();
        if self.task.is_none() {
            self.task = previous.task.clone();
            self.executed_steps = previous.executed_steps.clone();
            self.process_log = previous.process_log.clone();
        }
    }

    /// Expire state elements whose TTL has passed (M2-3). Returns the expired
    /// keys. Only elements with `verified_at` + `ttl` are subject to aging.
    pub fn tick(&mut self, now_secs: u64) -> Vec<String> {
        let mut expired = Vec::new();
        self.elements.retain(|element| {
            let is_expired = element
                .ttl
                .is_some_and(|ttl| element.verified_at.unwrap_or(0).saturating_add(ttl) <= now_secs);
            if is_expired {
                expired.push(element.key.clone());
            }
            !is_expired
        });
        expired
    }

    /// M2-4 result echo: a user-visible message for an experience-completed
    /// task, including the detailed process (回检视图).
    pub fn render_result_message(&self) -> String {
        let task = self
            .task
            .as_ref()
            .map(|task| task.description.clone())
            .unwrap_or_default();
        let steps = if self.executed_steps.is_empty() {
            "无".to_string()
        } else {
            self.executed_steps.join(" → ")
        };
        let deployed = if self.deployed.is_empty() {
            "无".to_string()
        } else {
            self.deployed
                .iter()
                .map(|element| format!("{}={}", element.key, element.value))
                .collect::<Vec<_>>()
                .join(", ")
        };
        format!(
            "任务「{task}」已完成（经验执行）。\n执行步骤: {steps}\n部署: {deployed}\n\n任务详细过程:\n{}",
            render_process_log(&self.process_log)
        )
    }

    /// A5: the payload the LLM reads when it intervenes after the time lag —
    /// the task itself, its status, what the experience already executed, and
    /// the state elements. Built from the same keys as condition checking, so
    /// "if the check passes the LLM does not read; if it fails the LLM reads
    /// the same keys" holds.
    pub fn handoff_payload(&self) -> serde_json::Value {
        serde_json::json!({
            "task": self.lookup("task.description"),
            "task_status": self.lookup("task.status"),
            "task_completed": self.lookup("task.completed"),
            "executed_steps": self.lookup("executed_steps"),
            "last_action": self.lookup("last_action.name"),
            "last_result_ok": self.lookup("last_result.ok"),
            "environment_cwd": self.lookup("environment.cwd"),
            "environment_sandbox": self.lookup("environment.sandbox_level"),
            "network_policy": self.lookup("network.policy"),
            "tools": self.lookup("tools.available"),
        })
    }

    /// Compact learning snapshot: the environment and state that surrounded
    /// the task. This is attached to `ExperienceTrace::environment_snapshot`
    /// so distilled experiences can record *what environment the task
    /// succeeded in* — the silence-audit gap between "current state" and
    /// "state this task needs".
    pub fn snapshot_for_learning(&self) -> serde_json::Value {
        let environment_id = self.environment.environment_id.clone();
        let cwd = self.environment.cwd.clone();
        let sandbox_level = self.environment.sandbox_level.clone();
        let network_policy = self.network.policy.clone();
        let network_reachability = self.network.reachability.clone();
        let tools = self.tools.available.clone();
        serde_json::json!({
            "task_status": self.task.as_ref().map(|task| task.status.to_string()),
            "environment": {
                "id": environment_id,
                "cwd": cwd,
                "sandbox_level": sandbox_level,
                "detections": self
                    .environment
                    .detections
                    .iter()
                    .map(|detection| {
                        serde_json::json!({
                            "key": detection.key,
                            "present": detection.present,
                            "detail": detection.detail,
                        })
                    })
                    .collect::<Vec<_>>(),
            },
            "network": {
                "policy": network_policy,
                "reachability": network_reachability,
            },
            "tools": tools,
            "elements": self
                .elements
                .iter()
                .map(|element| {
                    serde_json::json!({
                        "key": element.key,
                        "value": element.value,
                        "source": format!("{:?}", element.source),
                    })
                })
                .collect::<Vec<_>>(),
            "executed_steps": self
                .executed_steps
                .iter()
                .take(200)
                .cloned()
                .collect::<Vec<_>>(),
            "deployed": self
                .deployed
                .iter()
                .map(|deployed| deployed.key.clone())
                .collect::<Vec<_>>(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn default_state_is_deliberation_without_experience() {
        let state = ExperienceState::default();
        assert_eq!(state.execution_mode, ExecutionMode::Deliberation);
        assert_eq!(state.risk, RiskLevel::Low);
        assert_eq!(state.confidence, 0.0);
        assert!(state.current_goal.is_none());
        assert!(state.active_experience.is_none());
        assert!(state.last_action.is_none());
        assert!(state.last_result.is_none());
        assert!(state.environment.detections.is_empty());
        assert!(state.tools.available.is_empty());
        assert!(state.config.entries.is_empty());
        assert!(state.task.is_none());
        assert!(state.executed_steps.is_empty());
    }

    #[test]
    fn lookup_exposes_state_elements_for_llm_inspection() {
        let mut state = ExperienceState::default();
        state.environment.cwd = Some("/tmp".to_string());
        state.environment.detections.push(EnvironmentDetection {
            key: "git_repo".to_string(),
            present: true,
            detail: None,
        });
        state.tools.available.push("shell".to_string());
        state.config.workspace_roots.push("D:\\experience_codex".to_string());

        assert_eq!(
            state.lookup("environment.cwd"),
            Some(json!("/tmp"))
        );
        assert_eq!(state.lookup("environment.detection.git_repo"), Some(json!(true)));
        assert_eq!(state.lookup("tools.available"), Some(json!(["shell"])));

        // Conditions are checked without the LLM.
        assert!(state.satisfies("environment.detection.git_repo", &json!(true)));
        assert!(!state.satisfies("network.reachability", &json!("online")));
    }

    #[test]
    fn task_execution_state_is_readable_by_llm() {
        let mut state = ExperienceState::default();
        state.task = Some(Task {
            description: "move pdfs to archive".to_string(),
            status: TaskStatus::InProgress,
        });
        state.executed_steps.push("scan_files".to_string());
        state.executed_steps.push("create_month_directory".to_string());

        assert_eq!(state.lookup("task.description"), Some(json!("move pdfs to archive")));
        assert_eq!(state.lookup("task.status"), Some(json!("in_progress")));
        assert_eq!(state.lookup("task.completed"), Some(json!(false)));
        assert_eq!(
            state.lookup("executed_steps"),
            Some(json!(["scan_files", "create_month_directory"]))
        );

        state.task.as_mut().unwrap().status = TaskStatus::Completed;
        assert_eq!(state.lookup("task.completed"), Some(json!(true)));
    }

    #[test]
    fn handoff_payload_contains_task_and_execution_state() {
        let mut state = ExperienceState::default();
        state.task = Some(Task {
            description: "move pdfs to archive".to_string(),
            status: TaskStatus::InProgress,
        });
        state.executed_steps.push("scan_files".to_string());
        let payload = state.handoff_payload();
        assert_eq!(payload["task"], json!("move pdfs to archive"));
        assert_eq!(payload["task_status"], json!("in_progress"));
        assert_eq!(payload["executed_steps"], json!(["scan_files"]));
    }

    #[test]
    fn registry_upsert_and_lookup() {
        let mut state = ExperienceState::default();
        state.upsert_element(StateElement {
            key: "runtime.subprocess_allowed".to_string(),
            value: json!(false),
            source: ElementSource::Detection,
            verified_at: None,
            ttl: None,
        });
        assert_eq!(state.lookup("runtime.subprocess_allowed"), Some(json!(false)));
        assert!(state.satisfies("runtime.subprocess_allowed", &json!(false)));

        // Upsert replaces by key.
        state.upsert_element(StateElement {
            key: "runtime.subprocess_allowed".to_string(),
            value: json!(true),
            source: ElementSource::Detection,
            verified_at: None,
            ttl: None,
        });
        assert_eq!(state.elements.len(), 1);
        assert_eq!(state.lookup("runtime.subprocess_allowed"), Some(json!(true)));
    }

    #[test]
    fn inherit_session_carries_persistent_parts_but_keeps_new_task() {
        let mut previous = ExperienceState::default();
        previous.elements.push(StateElement {
            key: "runtime.subprocess_allowed".to_string(),
            value: json!(true),
            source: ElementSource::Detection,
            verified_at: None,
            ttl: None,
        });
        previous.deployed.push(DeployedElement {
            key: "toolchain.override".to_string(),
            value: json!("stable-x86_64-pc-windows-gnu"),
            action: "deploy.env".to_string(),
            ok: true,
            at: "t0".to_string(),
        });
        previous.task = Some(Task {
            description: "old task".to_string(),
            status: TaskStatus::Completed,
        });

        let mut fresh = ExperienceState::default();
        fresh.task = Some(Task {
            description: "new task".to_string(),
            status: TaskStatus::Pending,
        });
        fresh.executed_steps.push("scan".to_string());
        fresh.inherit_session(&previous);

        assert_eq!(fresh.elements.len(), 1);
        assert_eq!(fresh.deployed.len(), 1);
        assert_eq!(fresh.task.unwrap().description, "new task");
        assert_eq!(fresh.executed_steps, vec!["scan"]);
    }

    #[test]
    fn inherit_session_keeps_task_when_no_new_input() {
        let mut previous = ExperienceState::default();
        previous.task = Some(Task {
            description: "continue old".to_string(),
            status: TaskStatus::InProgress,
        });
        previous.executed_steps.push("step1".to_string());

        let mut fresh = ExperienceState::default(); // no new task
        fresh.inherit_session(&previous);
        assert_eq!(fresh.task.unwrap().description, "continue old");
        assert_eq!(fresh.executed_steps, vec!["step1"]);
    }

    #[test]
    fn tick_expires_elements_past_ttl() {
        let mut state = ExperienceState::default();
        state.elements.push(StateElement {
            key: "short_lived".to_string(),
            value: json!(true),
            source: ElementSource::Detection,
            verified_at: Some(100),
            ttl: Some(10),
        });
        state.elements.push(StateElement {
            key: "no_ttl".to_string(),
            value: json!(true),
            source: ElementSource::Detection,
            verified_at: Some(100),
            ttl: None,
        });
        assert!(state.tick(109).is_empty());
        let expired = state.tick(111);
        assert_eq!(expired, vec!["short_lived"]);
        assert_eq!(state.elements.len(), 1);
        assert_eq!(state.elements[0].key, "no_ttl");
    }

    #[test]
    fn render_result_message_includes_detailed_process() {
        let mut state = ExperienceState::default();
        state.task = Some(Task {
            description: "move pdfs to archive".to_string(),
            status: TaskStatus::Completed,
        });
        state.executed_steps.push("scan_files".to_string());
        state.deployed.push(DeployedElement {
            key: "toolchain.override".to_string(),
            value: json!("gnu"),
            action: "deploy.env".to_string(),
            ok: true,
            at: "t0".to_string(),
        });
        state
            .process_log
            .push(ProcessStep::experience("scan_files", "ok=true done"));
        let message = state.render_result_message();
        assert!(message.contains("move pdfs to archive"));
        assert!(message.contains("scan_files"));
        assert!(message.contains("toolchain.override"));
        assert!(message.contains("任务详细过程"));
        assert!(message.contains("[1] 经验: scan_files — ok=true done"));
    }
}

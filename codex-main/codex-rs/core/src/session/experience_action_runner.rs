//! Session-backed experience action runner (①/② wiring).
//!
//! v1 action set:
//! - safe read-only environment queries: env.cwd / env.detect.git / fs.exists /
//!   experience.noop;
//! - tool actions (`tool.<name>` or a bare tool name such as `exec_command`):
//!   routed through `ToolRouter::build_tool_call` and
//!   `ToolCallRuntime::handle_tool_call` — the sanctioned path with approvals,
//!   sandbox, and tool recording. Learned workflow steps record bare names, so
//!   any action that is not a built-in experience action falls back to tool
//!   dispatch instead of failing as "unknown".
//!
//! This module also hosts the ③ reference-prefill builders and records A6
//! feedback at the execution point.

use codex_protocol::ResponseItemId;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use futures::future::BoxFuture;
use tokio_util::sync::CancellationToken;

use crate::experience::ActionResult;
use crate::experience::ControlDecision;
use crate::experience::DeployedElement;
use crate::experience::ExecutionOutcome;
use crate::experience::Experience;
use crate::experience::ExperienceActionRunner;
use crate::experience::ExperienceState;
use crate::experience::ExperienceWorkflow;
use crate::experience::ExperienceWorkflowStep;
use crate::experience::FeedbackKind;
use crate::experience::execute_workflow;
use crate::session::session::Session;
use crate::tools::parallel::ToolCallRuntime;
use crate::tools::router::ToolRouter;

pub(crate) struct SessionExperienceRunner {
    cwd: String,
    tool_runtime: Option<ToolCallRuntime>,
    cancellation_token: CancellationToken,
    next_call_id: u64,
}

impl SessionExperienceRunner {
    pub(crate) fn new(
        cwd: String,
        tool_runtime: Option<ToolCallRuntime>,
        cancellation_token: CancellationToken,
    ) -> Self {
        Self {
            cwd,
            tool_runtime,
            cancellation_token,
            next_call_id: 0,
        }
    }

    async fn run_tool_action(
        &mut self,
        tool_name: &str,
        step: &ExperienceWorkflowStep,
    ) -> ActionResult {
        let Some(tool_runtime) = self.tool_runtime.as_ref() else {
            return ActionResult {
                ok: false,
                summary: "tool runtime unavailable".to_string(),
                deployed: None,
            };
        };
        let call_id = format!("experience-{:04}", self.next_call_id);
        self.next_call_id += 1;
        let item = ResponseItem::FunctionCall {
            id: Some(ResponseItemId::new("fc")),
            name: tool_name.to_string(),
            namespace: None,
            arguments: step.args.to_string(),
            encrypted_function_args: None,
            call_id,
            internal_chat_message_metadata_passthrough: None,
        };
        let call = match ToolRouter::build_tool_call(item) {
            Ok(Some(call)) => call,
            Ok(None) => {
                return ActionResult {
                    ok: false,
                    summary: format!("tool not advertised: {tool_name}"),
                    deployed: None,
                };
            }
            Err(error) => {
                return ActionResult {
                    ok: false,
                    summary: format!("tool call build error: {error:?}"),
                    deployed: None,
                };
            }
        };
        match tool_runtime
            .clone()
            .handle_tool_call(call, self.cancellation_token.clone())
            .await
        {
            Ok(envelope) => {
                let item = envelope.into_item();
                let summary = response_item_text(&item);
                ActionResult {
                    ok: true,
                    summary,
                    deployed: None,
                }
            }
            Err(error) => ActionResult {
                ok: false,
                summary: format!("tool execution error: {error}"),
                deployed: None,
            },
        }
    }

    async fn run_inner(&mut self, step: &ExperienceWorkflowStep) -> ActionResult {
        match step.name.as_str() {
            "env.cwd" => ActionResult {
                ok: true,
                summary: self.cwd.clone(),
                deployed: None,
            },
            "env.detect.git" => {
                let present = std::path::Path::new(&self.cwd).join(".git").exists();
                ActionResult {
                    ok: true,
                    summary: format!("git_repo={present}"),
                    deployed: None,
                }
            }
            "fs.exists" => {
                let path = step
                    .args
                    .get("path")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                let ok = !path.is_empty() && std::path::Path::new(path).exists();
                ActionResult {
                    ok,
                    summary: format!("fs.exists={ok}: {path}"),
                    deployed: None,
                }
            }
            "experience.noop" => ActionResult {
                ok: true,
                summary: "noop".to_string(),
                deployed: None,
            },
            "deploy.env" => {
                let key = step
                    .args
                    .get("key")
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    .to_string();
                let value = step
                    .args
                    .get("value")
                    .cloned()
                    .unwrap_or(serde_json::json!(null));
                let ok = !key.is_empty();
                ActionResult {
                    ok,
                    summary: format!("deploy.env {key}={value}"),
                    deployed: Some(DeployedElement {
                        key,
                        value,
                        action: "deploy.env".to_string(),
                        ok,
                        at: now_string(),
                    }),
                }
            }
            name if name.starts_with("tool.") => self.run_tool_action(&name[5..], step).await,
            other => self.run_tool_action(other, step).await,
        }
    }
}

fn now_string() -> String {
    now_secs().to_string()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

impl ExperienceActionRunner for SessionExperienceRunner {
    fn run<'a>(
        &'a mut self,
        step: &'a ExperienceWorkflowStep,
    ) -> BoxFuture<'a, ActionResult> {
        Box::pin(self.run_inner(step))
    }
}

/// Dry-run validation at the tool boundary (auto-pipeline stage 4): every
/// non-built-in workflow step must resolve to an advertised tool and carry
/// non-empty arguments. Nothing is executed — a distilled workflow only gets
/// promoted to ACTIVE after this replayability check passes.
pub(crate) fn validate_replayable(workflow: &ExperienceWorkflow) -> Result<(), String> {
    for (index, step) in workflow.steps.iter().enumerate() {
        let name = step.name.as_str();
        if matches!(
            name,
            "env.cwd" | "env.detect.git" | "fs.exists" | "experience.noop" | "deploy.env"
        ) {
            continue;
        }
        let tool_name = name.strip_prefix("tool.").unwrap_or(name);
        if tool_name.is_empty() {
            return Err(format!("step {index} has an empty tool name"));
        }
        if step.args.as_object().is_none_or(|args| args.is_empty()) {
            return Err(format!(
                "step {index} ({tool_name}) carries no arguments; refusing to activate"
            ));
        }
        let item = ResponseItem::FunctionCall {
            id: Some(ResponseItemId::new("fc")),
            name: tool_name.to_string(),
            namespace: None,
            arguments: step.args.to_string(),
            encrypted_function_args: None,
            call_id: format!("experience-validate-{index}"),
            internal_chat_message_metadata_passthrough: None,
        };
        match ToolRouter::build_tool_call(item) {
            Ok(Some(_)) => {}
            Ok(None) => return Err(format!("step {index}: tool not advertised: {tool_name}")),
            Err(error) => return Err(format!("step {index}: tool call build error: {error:?}")),
        }
    }
    Ok(())
}

/// Extract the output text of a response item (tool results are messages with
/// OutputText content).
fn response_item_text(item: &ResponseItem) -> String {
    match item {
        ResponseItem::Message { content, .. } => content
            .iter()
            .filter_map(|content_item| match content_item {
                ContentItem::OutputText { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// If the decision is ①/② and the experience exists, execute its workflow and
/// record the outcome as A6 feedback. Returns `Some(outcome)` when an
/// experience was executed; `None` when the decision is not ①/② or the
/// experience is missing from the store.
pub(crate) async fn execute_experience_if_hit(
    session: &Session,
    state: &mut ExperienceState,
    decision: &ControlDecision,
    tool_runtime: Option<ToolCallRuntime>,
    cancellation_token: CancellationToken,
) -> Option<ExecutionOutcome> {
    let experience_id = match decision {
        ControlDecision::ExperienceOnly(id) | ControlDecision::ExperienceFirst(id) => id,
        _ => return None,
    };
    let band = match decision {
        ControlDecision::ExperienceOnly(_) => "experience_only",
        ControlDecision::ExperienceFirst(_) => "experience_first",
        _ => "unknown",
    };
    let experience = session
        .services
        .experience_runtime
        .lock()
        .await
        .store()
        .get(experience_id)
        .cloned()?;
    let cwd = state.environment.cwd.clone().unwrap_or_default();
    let mut runner = SessionExperienceRunner::new(cwd, tool_runtime, cancellation_token);
    let outcome = execute_workflow(state, &experience, &mut runner).await;
    let mut runtime = session.services.experience_runtime.lock().await;
    let task = state.task.as_ref().map(|task| task.description.clone());
    match outcome {
        ExecutionOutcome::Completed => {
            runtime.record_feedback(experience_id, FeedbackKind::Success, now_secs());
            runtime.record_usage(
                experience_id,
                band,
                Some("success".to_string()),
                Some(session.thread_id.to_string()),
                None,
                "codex-exec".to_string(),
                task,
                now_secs(),
            );
        }
        ExecutionOutcome::Failed | ExecutionOutcome::NeedsLlm => {
            // §14.5: attribute the failure before touching confidence. Stale
            // parameters / missing sources / refusals are misfires (no quality
            // penalty); only genuine tool/semantic errors are invalid.
            if failure_is_misfire(state) {
                let note = state
                    .last_result
                    .as_ref()
                    .map(|result| result.summary.clone())
                    .unwrap_or_else(|| "replay misfire (unattributed)".to_string());
                let note = note.chars().take(240).collect::<String>();
                runtime.record_misfire(experience_id, note);
                runtime.record_usage(
                    experience_id,
                    band,
                    Some("misfire".to_string()),
                    Some(session.thread_id.to_string()),
                    None,
                    "codex-exec".to_string(),
                    task,
                    now_secs(),
                );
            } else {
                runtime.record_feedback(experience_id, FeedbackKind::Failure, now_secs());
                runtime.record_usage(
                    experience_id,
                    band,
                    Some("invalid".to_string()),
                    Some(session.thread_id.to_string()),
                    None,
                    "codex-exec".to_string(),
                    task,
                    now_secs(),
                );
            }
        }
    }
    Some(outcome)
}

/// Distinguish "the call was wrong for this situation" (misfire) from "the
/// experience's logic is wrong" (invalid). v1 is text-based on the failed
/// step's result/args; a structured FailureReason is the planned upgrade.
fn failure_is_misfire(state: &ExperienceState) -> bool {
    let haystack = state
        .last_result
        .as_ref()
        .map(|result| result.summary.as_str())
        .unwrap_or_default()
        .to_lowercase();
    const MISFIRE_MARKERS: &[&str] = &[
        "不存在",
        "找不到",
        "无法找到",
        "not found",
        "cannot find",
        "文件不存在",
        "拒绝",
        "refused",
        "aborted",
        "access denied",
        "拒绝访问",
    ];
    MISFIRE_MARKERS
        .iter()
        .any(|marker| haystack.contains(marker))
}

/// ③ reference prefill: what the parallel LLM reads (and can question) when an
/// experience is used as reference.
pub(crate) fn build_reference_prefill(state: &ExperienceState, experience: &Experience) -> String {
    format!(
        "参考经验（供校验与质疑；任务未完成时请检查步骤分解是否符合实际）:\n{}\n当前执行状态:\n{}",
        experience.reference_text(),
        state.handoff_payload()
    )
}

/// Build a user text message item (mirrors codex's hook-prompt item builder).
pub(crate) fn build_text_message_item(text: &str) -> Option<ResponseItem> {
    if text.trim().is_empty() {
        return None;
    }
    Some(ResponseItem::Message {
        id: Some(ResponseItemId::new("msg")),
        role: "user".to_string(),
        content: vec![ContentItem::InputText {
            text: text.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::experience::ExperienceWorkflow;
    use crate::experience::ExperienceWorkflowStep;

    /// Problem-2 verification (silence-audit): the current dry-run check
    /// validates that every step resolves to an advertised tool with
    /// arguments — it cannot see *semantic* errors. A distilled experience
    /// whose workflow reads the WRONG site/metric for the task still passes
    /// and would activate. This test pins that limitation so the fix
    /// (completion-criteria evidence) has a target.
    #[test]
    fn replayable_check_cannot_detect_semantic_errors() {
        // Task was "read 曝光数/观看数 from Xiaohongshu creator center", but
        // the distilled workflow navigates to Douyin and only checks that the
        // word 播放量 exists — tool names and args are both valid.
        let wrong_site = ExperienceWorkflow {
            steps: vec![
                ExperienceWorkflowStep {
                    name: "exec_command".to_string(),
                    args: serde_json::json!({"cmd": "node -e \"fetch('https://creator.douyin.com').then(r=>r.text()).then(t=>console.log(t.includes('播放量')))\"", "yield_time_ms": 20000}),
                },
                ExperienceWorkflowStep {
                    name: "exec_command".to_string(),
                    args: serde_json::json!({"cmd": "echo 曝光数=0 观看数=0 (guessed, not read)"}),
                },
            ],
        };
        assert_eq!(validate_replayable(&wrong_site), Ok(()));
    }
}

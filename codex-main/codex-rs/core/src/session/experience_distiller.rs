//! Automatic experience pipeline (M2 steps 3-4): prune → distill → validate →
//! activate, with zero user interaction.
//!
//! Stage ownership:
//! - capture + prune already live in `experience_learner` (step-1);
//! - this module owns **distill** (a small, tool-free LLM compile call that
//!   turns a messy CANDIDATE workflow into a minimal re-runnable one),
//!   **validate** (structural check + tool-boundary dry run, nothing executes)
//!   and **activate** (`ExperienceRuntime::admit_auto_experience`).
//!
//! The LLM is used here as a *learner/compiler only*: it never decides whether
//! an experience runs. The resulting workflow is validated deterministically
//! before it becomes ACTIVE.

use std::sync::Arc;
use std::collections::HashMap;

use codex_async_utils::OrCancelExt;
use codex_rollout_trace::InferenceTraceContext;
use futures::prelude::*;
use tokio_util::sync::CancellationToken;

use crate::client::ModelClientSession;
use crate::client_common::Prompt;
use crate::client_common::ResponseEvent;
use crate::experience::Experience;
use crate::experience::ExperienceDraft;
use crate::experience::ExperienceKind;
use crate::experience::ExperienceState;
use crate::experience::ExperienceStatus;
use crate::experience::ExperienceTrace;
use crate::experience::ExperienceTrigger;
use crate::experience::ExperienceWorkflow;
use crate::experience::ExperienceWorkflowStep;
use crate::experience::MAX_AUTO_CONFIRM_STEPS;
use crate::experience::ProcessStep;
use crate::experience::RiskLevel;
use crate::experience::TraceStep;
use crate::responses_metadata::CodexResponsesRequestKind;
use crate::session::experience_action_runner::build_text_message_item;
use crate::session::experience_action_runner::validate_replayable;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use crate::stream_events_utils::raw_assistant_output_text_from_item;

/// Confidence of an LLM-distilled experience that passed validation.
const DISTILLED_CONFIDENCE: f32 = 0.90;

/// Run the automatic pipeline after a successful LLM-driven turn:
/// 1. already-VALIDATED drafts (short + auto-confirmed) → activate directly;
/// 2. CANDIDATE drafts with a messy (> threshold) workflow → LLM distill;
/// 3. short CANDIDATE drafts (repeated success, no auto-confirm) → validate
///    the captured workflow as-is;
/// 4. dry-run validation at the tool boundary; only then ACTIVE.
///
/// Everything is best-effort: any failure leaves the stored experience as
/// CANDIDATE (safe, never half-activated).
pub(crate) async fn run_auto_pipeline(
    sess: &Arc<Session>,
    turn_context: &Arc<TurnContext>,
    client_session: &mut ModelClientSession,
    experience_state: &mut ExperienceState,
    trace: &ExperienceTrace,
    draft: ExperienceDraft,
    cancellation_token: CancellationToken,
) {
    let candidate_id = draft.experience.id.clone();
    // "Messy" is judged on the raw trace (which may contain waste that the
    // deterministic pruner could not recognize), not only on the pruned
    // workflow — a 56-step Instagram exploration still needs distillation
    // even if pruning brought the stored workflow under the threshold.
    let messy = trace.actions.len() > MAX_AUTO_CONFIRM_STEPS
        || draft.experience.workflow.steps.len() > MAX_AUTO_CONFIRM_STEPS;

    // Stage 2: distill. A messy CANDIDATE needs a small LLM compile call;
    // a short one (or an already-VALIDATED auto-confirmed one) can use the
    // captured workflow as-is.
    let candidate = if messy {
        match llm_distill(
            sess,
            turn_context,
            client_session,
            trace,
            &draft.experience,
            &cancellation_token,
        )
        .await
        {
            Some(distilled) => distilled,
            None => {
                tracing::warn!(
                    experience_id = %candidate_id,
                    "LLM distillation failed; candidate left for review"
                );
                experience_state.process_log.push(ProcessStep::llm(
                    "经验蒸馏失败",
                    format!("candidate {candidate_id} 保留 CANDIDATE"),
                ));
                return;
            }
        }
    } else {
        draft.experience
    };

    // Stage 3: validate. Structure is enforced by `admit_auto_experience`
    // (upsert_validated); here we additionally dry-run the workflow at the
    // tool boundary so a hallucinated tool/step never activates.
    if let Err(reason) = validate_replayable(&candidate.workflow) {
        tracing::warn!(
            experience_id = %candidate_id,
            reason = %reason,
            "distilled experience failed replayability check; candidate left for review"
        );
        experience_state.process_log.push(ProcessStep::llm(
            "经验蒸馏·校验失败",
            format!("candidate {candidate_id}: {reason}"),
        ));
        return;
    }

    // System validation passed (structure + dry-run): the distilled workflow
    // is admitted as VALIDATED; `admit_auto_experience` then promotes it.
    let mut candidate = candidate;
    candidate.status = ExperienceStatus::Validated;

    // Stage 4: activate (replaces the candidate under the same id, bumps the
    // version, promotes VALIDATED → ACTIVE).
    match sess
        .services
        .experience_runtime
        .lock()
        .await
        .admit_auto_experience(candidate)
    {
        Ok(ExperienceStatus::Active) => {
            tracing::info!(
                experience_id = %candidate_id,
                "auto-pipeline activated distilled experience"
            );
            experience_state.process_log.push(ProcessStep::llm(
                "经验蒸馏",
                format!("candidate {candidate_id} → ACTIVE (自动校验通过)"),
            ));
        }
        Ok(status) => {
            tracing::info!(
                experience_id = %candidate_id,
                ?status,
                "auto-pipeline admitted distilled experience (awaiting activation)"
            );
            experience_state.process_log.push(ProcessStep::llm(
                "经验蒸馏",
                format!("candidate {candidate_id} → {status:?}"),
            ));
        }
        Err(error) => {
            tracing::warn!(
                experience_id = %candidate_id,
                %error,
                "auto-pipeline admission failed; candidate left for review"
            );
            experience_state.process_log.push(ProcessStep::llm(
                "经验蒸馏·入库失败",
                format!("candidate {candidate_id}: {error}"),
            ));
        }
    }
}

/// Detect an explicit user request to record this task as an experience
/// (user release channel). v1 is deliberately narrow phrase matching to avoid
/// false positives on ordinary tasks; a UI/slash-command flag is the planned
/// upgrade.
pub(crate) fn explicit_record_requested(text: &str) -> bool {
    let lower = text.trim().to_lowercase();
    const ZH_PHRASES: &[&str] = &[
        "记住这个做法",
        "记住这个流程",
        "记住这个操作",
        "以后都这样做",
        "以后就这样做",
        "沉淀为经验",
        "沉淀成经验",
        "保存为经验",
        "存为经验",
    ];
    const EN_PHRASES: &[&str] = &[
        "remember this procedure",
        "remember this workflow",
        "remember this operation",
        "save this as an experience",
        "record this as an experience",
        "make this an experience",
        "always do it this way",
    ];
    ZH_PHRASES.iter().any(|phrase| text.contains(phrase))
        || EN_PHRASES.iter().any(|phrase| lower.contains(phrase))
}

/// Stage 2: a small, tool-free LLM call that compiles a messy workflow into a
/// minimal re-runnable one. The result is parsed deterministically; nothing it
/// returns can execute on its own.
async fn llm_distill(
    sess: &Arc<Session>,
    turn_context: &Arc<TurnContext>,
    client_session: &mut ModelClientSession,
    trace: &ExperienceTrace,
    candidate: &Experience,
    cancellation_token: &CancellationToken,
) -> Option<Experience> {
    let steps: Vec<TraceStep> = if trace.steps.is_empty() {
        // No per-step outcome detail (older capture path / tests): annotate
        // the workflow steps as unknown-outcome.
        candidate
            .workflow
            .steps
            .iter()
            .map(|step| TraceStep {
                name: step.name.clone(),
                args: step.args.clone(),
                call_id: None,
                ok: None,
                summary: None,
            })
            .collect()
    } else {
        trace.steps.clone()
    };
    let prompt_text = build_distill_prompt(
        &candidate.name,
        &steps,
        trace.final_evidence.as_deref(),
        trace.environment_snapshot.as_ref(),
    );
    let input_item = build_text_message_item(&prompt_text)?;
    let prompt = Prompt {
        input: vec![input_item],
        ..Default::default()
    };
    let responses_metadata = sess
        .responses_metadata(turn_context.as_ref(), CodexResponsesRequestKind::Memory)
        .await;
    let mut stream = match client_session
        .stream(
            &prompt,
            turn_context.model_info(),
            &turn_context.session_telemetry,
            None,
            turn_context.reasoning_summary(),
            turn_context.config.service_tier.clone(),
            &responses_metadata,
            &InferenceTraceContext::disabled(),
        )
        .or_cancel(cancellation_token)
        .await
    {
        Ok(Ok(stream)) => stream,
        Ok(Err(error)) => {
            tracing::warn!(experience_id = %candidate.id, %error, "LLM distill stream error");
            return None;
        }
        Err(_) => return None,
    };

    let mut text = String::new();
    while let Some(event) = stream.next().await {
        match event {
            Ok(ResponseEvent::OutputItemDone(item)) => {
                if let Some(part) = raw_assistant_output_text_from_item(&item) {
                    text.push_str(&part);
                }
            }
            Ok(ResponseEvent::RateLimits(snapshot)) => {
                sess.update_rate_limits(turn_context.as_ref(), snapshot).await;
            }
            Ok(ResponseEvent::Completed { token_usage, .. }) => {
                let _ = sess
                    .update_token_usage_info(turn_context.as_ref(), token_usage.as_ref())
                    .await;
                break;
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    parse_distilled_experience(&text, candidate)
}

/// Pure: the model input for distillation. Built from the *process record*
/// (per-step outcome + output summary + environment + final evidence), not
/// from a flat action list — this is what lets the compiler distinguish
/// successful steps from one-off accidents.
pub(crate) fn build_distill_prompt(
    task: &str,
    steps: &[TraceStep],
    final_evidence: Option<&str>,
    environment_snapshot: Option<&serde_json::Value>,
) -> String {
    let mut actions = String::new();
    for (index, step) in steps.iter().enumerate() {
        let status = match step.ok {
            Some(true) => "ok",
            Some(false) => "FAILED",
            None => "unknown",
        };
        actions.push_str(&format!(
            "{}. [{status}] {} {}\n",
            index + 1,
            step.name,
            step.args
        ));
        if let Some(summary) = &step.summary {
            let summary = summary.replace('\n', " ").chars().take(240).collect::<String>();
            actions.push_str(&format!("   output: {summary}\n"));
        }
    }
    let evidence = final_evidence
        .map(|text| text.replace('\n', " ").chars().take(400).collect::<String>())
        .unwrap_or_default();
    let environment = environment_snapshot.map(|value| value.to_string()).unwrap_or_default();
    format!(
        "You are the behavior distiller for the experience_codex agent \
         runtime. This distillation follows the project skill 'experience-gen' \
         (.codex/skills/experience-gen/SKILL.md): only steps with real success \
         evidence belong in the workflow, the result must be self-contained \
         (no references to deleted temp files), and completion is judged by \
         what the agent actually observed. A task was just completed by \
         another agent run; the tool-call sequence below is the execution \
         trace (it includes exploration, probing and repeated attempts).\n\n\
         TASK: {task}\n\n\
         ENVIRONMENT SNAPSHOT (what the task ran in):\n{environment}\n\n\
         EXECUTED TOOL CALLS:\n{actions}\n\
         FINAL EVIDENCE (what the agent reported it observed):\n{evidence}\n\n\
         Distill this into the minimal, necessary, ordered workflow that \
         performs the task from scratch.\n\
         Rules:\n\
         1. Steps marked FAILED were not part of the success path — drop them \
         and any exploration, environment probing or repeated steps. Prefer \
         steps marked ok.\n\
         2. Step names must appear verbatim in the executed calls, and step \
         arguments must be copied from those calls (values, not placeholders).\n\
         3. Keep setup steps that make the workflow re-runnable (e.g. create \
         the destination directory if it may not exist).\n\
         4. The workflow must not depend on temporary files that are deleted \
         after the run (e.g. files under .codex-tmp or %TEMP%): inline such \
         content or make the step recreate it.\n\
         5. The task may be in Chinese; keep the original terms.\n\n\
         Answer with ONLY a JSON object (no markdown fences, no commentary):\n\
         {{\"name\": \"<one-sentence task description>\", \
         \"title\": \"<short display name, concise>\", \
         \"trigger\": {{\"object\": \"<optional>\", \"location\": \"<optional>\", \
         \"goal\": \"<optional>\", \"keywords\": [\"<3-10 concise terms>\"]}}, \
         \"workflow\": [{{\"name\": \"<tool name from the calls>\", \
         \"args\": {{<exact arguments copied from the call>}}}}], \
         \"completion_criteria\": \"<how you know the task is done>\", \
         \"failure_modes\": [\"<optional known failure ways>\"], \
         \"assets\": {{\"<file name>\": \"<content if any script must be persisted>\"}}, \
         \"applicability\": {{\"input_scope\": \"single|batch|any\", \
         \"applicable_objects\": [\"<object types the workflow handles>\"], \
         \"parameterized\": true}}, \
         \"risk\": \"low\"}}"
    )
}

/// Pure: parse the model's JSON answer into a VALIDATED `Experience` under the
/// candidate's id. Trigger keywords fall back to the candidate's own keywords,
/// then to the task text. Returns `None` when the answer is not usable.
pub(crate) fn parse_distilled_experience(text: &str, candidate: &Experience) -> Option<Experience> {
    let json = extract_json(text)?;
    let payload: DistillPayload = serde_json::from_str(json).ok()?;
    if payload.workflow.is_empty() {
        return None;
    }
    let mut steps = Vec::with_capacity(payload.workflow.len());
    for raw in payload.workflow {
        let name = raw.name.trim().to_string();
        if name.is_empty() {
            return None;
        }
        let args = raw
            .args
            .filter(|value| value.is_object())
            .unwrap_or_else(|| serde_json::json!({}));
        steps.push(ExperienceWorkflowStep { name, args });
    }
    let trigger = payload.trigger.unwrap_or_default();
    let mut keywords = trigger
        .keywords
        .unwrap_or_default()
        .into_iter()
        .map(|keyword| keyword.trim().to_string())
        .filter(|keyword| !keyword.is_empty())
        .collect::<Vec<_>>();
    if keywords.is_empty() {
        keywords = candidate.trigger.keywords.clone();
    }
    if keywords.is_empty() {
        keywords = vec![candidate.name.trim().to_string()];
    }
    let risk = risk_from_str(payload.risk.as_deref());
    let name = payload
        .name
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| candidate.name.clone());
    let distilled_title = payload
        .title
        .filter(|title| !title.trim().is_empty())
        .or_else(|| candidate.title.clone());
    let distilled_note = payload.note.filter(|note| !note.trim().is_empty());
    let experience = Experience {
        id: candidate.id.clone(),
        name,
        kind: if steps.len() == 1 {
            ExperienceKind::Reflex
        } else {
            ExperienceKind::Process
        },
        trigger: ExperienceTrigger {
            object: trigger.object,
            location: trigger.location,
            goal: trigger.goal,
            keywords,
            context: Default::default(),
        },
        conditions: Vec::new(),
        workflow: ExperienceWorkflow { steps },
        completion_criteria: payload
            .completion_criteria
            .filter(|criteria| !criteria.trim().is_empty())
            .or_else(|| candidate.completion_criteria.clone()),
        failure_modes: payload.failure_modes,
        assets: payload.assets,
        applicability: payload.applicability.map(Into::into).unwrap_or_default(),
        title: distilled_title,
        note: distilled_note,
        created_at: candidate.created_at,
        confidence: DISTILLED_CONFIDENCE,
        risk,
        status: ExperienceStatus::Validated,
        version: 1,
    };
    experience.validate().ok()?;
    Some(experience)
}

fn extract_json(text: &str) -> Option<&str> {
    let trimmed = text.trim();
    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    if end <= start {
        return None;
    }
    Some(&trimmed[start..=end])
}

fn risk_from_str(risk: Option<&str>) -> RiskLevel {
    match risk.map(|value| value.trim().to_lowercase()).as_deref() {
        Some("medium") => RiskLevel::Medium,
        Some("high") => RiskLevel::High,
        _ => RiskLevel::Low,
    }
}

#[derive(serde::Deserialize, Default)]
struct DistillPayload {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    trigger: Option<DistillTrigger>,
    #[serde(default)]
    workflow: Vec<DistillStep>,
    #[serde(default)]
    risk: Option<String>,
    #[serde(default)]
    completion_criteria: Option<String>,
    #[serde(default)]
    failure_modes: Vec<String>,
    #[serde(default)]
    assets: HashMap<String, String>,
    #[serde(default)]
    applicability: Option<DistillApplicability>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    note: Option<String>,
}

#[derive(serde::Deserialize, Default)]
struct DistillApplicability {
    #[serde(default)]
    input_scope: Option<String>,
    #[serde(default)]
    applicable_objects: Option<Vec<String>>,
    #[serde(default)]
    parameterized: Option<bool>,
}

impl From<DistillApplicability> for crate::experience::Applicability {
    fn from(value: DistillApplicability) -> Self {
        use crate::experience::InputScope;
        let input_scope = match value.input_scope.as_deref() {
            Some("single") => InputScope::Single,
            Some("batch") => InputScope::Batch,
            Some("any") => InputScope::Any,
            _ => InputScope::Unknown,
        };
        Self {
            input_scope,
            applicable_objects: value.applicable_objects.unwrap_or_default(),
            parameterized: value.parameterized.unwrap_or(false),
        }
    }
}

#[derive(serde::Deserialize, Default)]
struct DistillTrigger {
    #[serde(default)]
    object: Option<String>,
    #[serde(default)]
    location: Option<String>,
    #[serde(default)]
    goal: Option<String>,
    #[serde(default)]
    keywords: Option<Vec<String>>,
}

#[derive(serde::Deserialize)]
struct DistillStep {
    name: String,
    #[serde(default)]
    args: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::experience::ExperienceId;

    fn candidate() -> Experience {
        Experience {
            id: ExperienceId("cand-distill-test".to_string()),
            name: "把所有桌面 PNG 移动到 pic 目录".to_string(),
            kind: ExperienceKind::Process,
            trigger: ExperienceTrigger {
                keywords: vec!["png".to_string(), "移动".to_string()],
                ..Default::default()
            },
            conditions: Vec::new(),
            workflow: ExperienceWorkflow {
                steps: vec![ExperienceWorkflowStep {
                    name: "exec_command".to_string(),
                    args: serde_json::json!({"cmd": "Move-Item 1.png pic\\"}),
                }],
            },
            completion_criteria: None,
            failure_modes: Vec::new(),
            assets: Default::default(),
            applicability: Default::default(),
            title: None,
            note: None,
            created_at: None,
            confidence: 0.2,
            risk: RiskLevel::Low,
            status: ExperienceStatus::Candidate,
            version: 1,
        }
    }

    #[test]
    fn distill_prompt_embeds_task_and_steps() {
        let steps = vec![TraceStep {
            name: "exec_command".to_string(),
            args: serde_json::json!({"cmd": "Move-Item 1.png pic\\"}),
            call_id: Some("call-1".to_string()),
            ok: Some(true),
            summary: Some("moved 1 file".to_string()),
        }];
        let prompt = build_distill_prompt("move png", &steps, Some("moved 1.png to pic"), None);
        assert!(prompt.contains("move png"));
        assert!(prompt.contains("exec_command"));
        assert!(prompt.contains("Move-Item"));
        assert!(prompt.contains("[ok]"));
        assert!(prompt.contains("moved 1.png to pic"));
    }

    #[test]
    fn parse_accepts_fenced_json_and_merges_keywords() {
        let text = "```json\n{\"name\": \"移动桌面 PNG\", \
            \"trigger\": {\"location\": \"desktop\", \"keywords\": [\"png\", \"桌面\", \"移动\"]}, \
            \"workflow\": [{\"name\": \"exec_command\", \"args\": {\"cmd\": \"Move-Item *.png pic\\\\\"}}], \
            \"completion_criteria\": \"桌面 PNG 已清空且 pic 目录数量匹配\", \
            \"failure_modes\": [\"OneDrive 路径不存在时需先探测\"], \
            \"assets\": {\"read.mjs\": \"console.log('ok')\"}, \
            \"risk\": \"low\"}\n```";
        let parsed = parse_distilled_experience(text, &candidate()).expect("parsed");
        assert_eq!(parsed.status, ExperienceStatus::Validated);
        assert_eq!(parsed.kind, ExperienceKind::Reflex);
        assert_eq!(parsed.workflow.steps.len(), 1);
        assert_eq!(parsed.workflow.steps[0].name, "exec_command");
        assert!(parsed.trigger.keywords.contains(&"桌面".to_string()));
        assert!(parsed
            .completion_criteria
            .as_deref()
            .unwrap()
            .contains("pic 目录数量匹配"));
        assert_eq!(parsed.failure_modes.len(), 1);
        assert_eq!(parsed.assets.get("read.mjs").map(String::as_str), Some("console.log('ok')"));
        assert_eq!(parsed.confidence, DISTILLED_CONFIDENCE);
    }

    #[test]
    fn parse_falls_back_to_candidate_keywords() {
        let text = "{\"workflow\": [{\"name\": \"exec_command\", \"args\": {\"cmd\": \"x\"}}]}";
        let parsed = parse_distilled_experience(text, &candidate()).expect("parsed");
        assert!(parsed.trigger.keywords.contains(&"png".to_string()));
        assert!(parsed.trigger.keywords.contains(&"移动".to_string()));
    }

    #[test]
    fn parse_rejects_empty_workflow() {
        let text = "{\"workflow\": []}";
        assert!(parse_distilled_experience(text, &candidate()).is_none());
    }

    #[test]
    fn parse_rejects_empty_step_name() {
        let text = "{\"workflow\": [{\"name\": \"  \", \"args\": {}}]}";
        assert!(parse_distilled_experience(text, &candidate()).is_none());
    }

    #[test]
    fn explicit_record_requested_detects_intent_only() {
        assert!(explicit_record_requested("完成这个任务后，请记住这个流程，下次直接做"));
        assert!(explicit_record_requested("以后都这样做：先建目录再移动"));
        assert!(explicit_record_requested("Remember this workflow for next time"));
        assert!(explicit_record_requested("save this as an experience"));
        assert!(!explicit_record_requested("帮我把桌面上的文件移动到 pic"));
        assert!(!explicit_record_requested("查看这个项目的 git 状态"));
    }
}

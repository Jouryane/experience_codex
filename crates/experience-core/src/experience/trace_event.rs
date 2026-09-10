//! Typed trace v1 event model (P2 item 4): semantic milestones only.
//!
//! This is the frozen L1 input contract (docs/p2-trace-v1.md), owned by the
//! Experience domain. The Session Host adapter (agent-codex) produces these
//! events; the Learning side consumes them. Events are whitelisted at the
//! write boundary; reasoning noise is folded before persistence and
//! non-semantic notifications never become events.

use serde::Deserialize;
use serde::Serialize;

/// Schema version of the typed trace event model (v1).
pub const TRACE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TraceEvent {
    /// Task was queued; starts a new round (round boundaries are cut here).
    Submitted,
    /// Turn accepted and agent started.
    Accepted,
    AgentStarted,
    /// queue/start raced the host's auto-dispatch (fork app-server behavior).
    AlreadyStarted,
    /// One folded marker per consecutive reasoning burst.
    Reasoning,
    TurnCompleted,
    ToolCall {
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        call_id: Option<String>,
        /// Truncated, allow-listed argument summary (L1 distill input).
        /// Optional so pre-v1 traces and zero-arg tools stay valid.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        args_summary: Option<String>,
    },
    /// Tool result presence + correlation. Per-step success/failure is NOT
    /// recorded in v1: single-step failures are expressed through `Failed`
    /// events and/or the terminal session error (see docs/p2-trace-v1.md).
    ToolResult {
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        call_id: Option<String>,
    },
    /// Driver/protocol failure after a thread existed, or operator cancel.
    /// Terminal error also refuses distillation; this is a double guard.
    Failed {
        phase: String,
        message: String,
    },
    /// L3 first-class delegation: the runtime outsourced the remaining task
    /// to an executor. `task_summary` is a short label, never the full trace.
    Delegate {
        agent: String,
        task_summary: String,
        /// Structured plan context (ruling 2026-09-09): selected experience
        /// names survive label truncation; legacy JSON without the field
        /// reads back as an empty list.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        plan: Vec<String>,
    },
    /// Read-only migration shell for pre-v1 string traces. Never distilled.
    #[serde(rename = "legacy")]
    Legacy {
        label: String,
    },
}

impl TraceEvent {
    /// Stable one-line rendering for UI/probes. Legacy events render their
    /// original label verbatim.
    pub fn label(&self) -> String {
        match self {
            TraceEvent::Submitted => "submitted".into(),
            TraceEvent::Accepted => "accepted".into(),
            TraceEvent::AgentStarted => "agent_started".into(),
            TraceEvent::AlreadyStarted => "already_started".into(),
            TraceEvent::Reasoning => "reasoning".into(),
            TraceEvent::TurnCompleted => "turn_completed".into(),
            TraceEvent::ToolCall { name, .. } => format!("tool_call:{name}"),
            TraceEvent::ToolResult { name, .. } => format!("tool_result:{name}"),
            TraceEvent::Failed { phase, .. } => format!("failed:{phase}"),
            TraceEvent::Delegate { agent, .. } => format!("delegate:{agent}"),
            TraceEvent::Legacy { label } => label.clone(),
        }
    }

    /// Whether consecutive duplicates of this event are noise to fold away.
    pub fn is_noisy_repeat(&self) -> bool {
        matches!(self, TraceEvent::Reasoning)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn delegate_plan_field_roundtrips_in_json() {
        let event = TraceEvent::Delegate {
            agent: "codex".to_string(),
            task_summary: "task label".to_string(),
            plan: vec!["create_probe_file".to_string()],
        };
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["kind"], "delegate");
        assert_eq!(value["agent"], "codex");
        assert_eq!(value["plan"][0], "create_probe_file");
        let back: TraceEvent = serde_json::from_value(value).unwrap();
        assert_eq!(back, event);
    }

    #[test]
    fn legacy_delegate_without_plan_reads_as_empty() {
        let event: TraceEvent = serde_json::from_value(json!({
            "kind": "delegate",
            "agent": "codex",
            "task_summary": "old label"
        }))
        .unwrap();
        match event {
            TraceEvent::Delegate { plan, .. } => assert!(plan.is_empty()),
            other => panic!("unexpected event: {other:?}"),
        }
    }
}

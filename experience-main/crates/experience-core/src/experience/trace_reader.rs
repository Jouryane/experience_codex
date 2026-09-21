//! Typed trace v1 reader: round splitting/filtering for Learning input.
//!
//! Rules (docs/p2-trace-v1.md, phaseC review):
//! - input = a COMPLETED session's last round, cut at the final `submitted`;
//! - any session whose trace contains a `legacy` event is rejected outright
//!   (a legacy prefix would shift round indices after resume);
//! - a round without `turn_completed` is rejected (no completion evidence);
//! - the round task comes from `round_tasks[round]`, falling back to the
//!   session `task` when the array is missing;
//! - round selection is a parameter (default Last) so future learning modes
//!   (e.g. earlier failing rounds) can choose without a schema change.

use serde_json::Value;

use super::trace_event::TraceEvent;

/// Which round of a session is eligible distillation input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundSelection {
    /// The most recent round (default for v1 success-path learning).
    Last,
}

/// A distilled round: its task text and typed events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoundTrace {
    pub task: String,
    pub events: Vec<TraceEvent>,
}

impl RoundTrace {
    /// Rendered one-line labels (UI/probe convenience).
    pub fn labels(&self) -> Vec<String> {
        self.events.iter().map(TraceEvent::label).collect()
    }
}

/// Select the eligible round of a session JSON record (schema v2 shape),
/// or `None` when the session is not valid typed v1 distillation input.
pub fn select_round(session: &Value, selection: RoundSelection) -> Option<RoundTrace> {
    if session
        .get("status")
        .and_then(Value::as_str)
        != Some("completed")
    {
        return None;
    }
    let trace = session.get("trace").and_then(Value::as_array)?;
    let events: Vec<TraceEvent> = trace
        .iter()
        .map(|item| serde_json::from_value::<TraceEvent>(item.clone()))
        .collect::<Result<_, _>>()
        .ok()?;
    // P1-1 guard: any legacy entry (migrated pre-v1 prefix) makes round
    // indices unreliable after resume; reject the whole session.
    if events.iter().any(|event| matches!(event, TraceEvent::Legacy { .. })) {
        return None;
    }
    let last_submitted = events
        .iter()
        .rposition(|event| matches!(event, TraceEvent::Submitted))?;
    let rounds_before = events[..last_submitted]
        .iter()
        .filter(|event| matches!(event, TraceEvent::Submitted))
        .count();
    let round_events = events[last_submitted..].to_vec();
    // Completion evidence: the selected round must contain turn_completed.
    if !round_events
        .iter()
        .any(|event| matches!(event, TraceEvent::TurnCompleted))
    {
        return None;
    }
    let task = session
        .get("round_tasks")
        .and_then(Value::as_array)
        .and_then(|tasks| tasks.get(rounds_before))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            session
                .get("task")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default();
    match selection {
        RoundSelection::Last => Some(RoundTrace {
            task,
            events: round_events,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn completed_session(trace: Value, round_tasks: Value) -> Value {
        json!({
            "id": "s-1",
            "agent_id": "codex",
            "task": "fallback task",
            "cwd": null,
            "trace": trace,
            "round_tasks": round_tasks,
            "status": "completed",
            "created_at": 1,
            "finished_at": 2,
            "summary": "ok",
            "output": ""
        })
    }

    #[test]
    fn two_rounds_selects_the_last_round_and_its_task() {
        let session = completed_session(
            json!([
                {"kind": "submitted"},
                {"kind": "agentStarted"},
                {"kind": "toolCall", "name": "exec_command"},
                {"kind": "toolResult", "name": "exec_command"},
                {"kind": "turnCompleted"},
                {"kind": "submitted"},
                {"kind": "agentStarted"},
                {"kind": "toolCall", "name": "exec_command"},
                {"kind": "toolResult", "name": "exec_command"},
                {"kind": "turnCompleted"}
            ]),
            json!(["round one task", "round two task"]),
        );
        let round = select_round(&session, RoundSelection::Last).unwrap();
        assert_eq!(round.task, "round two task");
        assert!(round
            .events
            .iter()
            .all(|event| !matches!(event, TraceEvent::Submitted) || {
                round.events.first() == Some(&TraceEvent::Submitted)
            }));
        assert_eq!(round.events.len(), 5);
        assert!(round
            .events
            .iter()
            .any(|event| matches!(event, TraceEvent::TurnCompleted)));
        assert!(round.events[2..4]
            .iter()
            .any(|event| matches!(event, TraceEvent::ToolCall { name, .. } if name == "exec_command")));
    }

    #[test]
    fn legacy_prefix_session_is_rejected_even_with_typed_resume_tail() {
        let session = completed_session(
            json!([
                {"kind": "legacy", "label": "submitted"},
                {"kind": "legacy", "label": "turn_completed"},
                {"kind": "submitted"},
                {"kind": "agentStarted"},
                {"kind": "turnCompleted"}
            ]),
            json!(["old task", "resume task"]),
        );
        assert_eq!(select_round(&session, RoundSelection::Last), None);
    }

    #[test]
    fn round_without_turn_completed_is_rejected() {
        let session = completed_session(
            json!([
                {"kind": "submitted"},
                {"kind": "agentStarted"},
                {"kind": "toolCall", "name": "exec_command"}
            ]),
            json!(["unfinished task"]),
        );
        assert_eq!(select_round(&session, RoundSelection::Last), None);
    }

    #[test]
    fn missing_round_tasks_falls_back_to_session_task() {
        let session = json!({
            "id": "s-1",
            "agent_id": "codex",
            "task": "only task",
            "cwd": null,
            "trace": [
                {"kind": "submitted"},
                {"kind": "agentStarted"},
                {"kind": "turnCompleted"}
            ],
            "status": "completed",
            "created_at": 1,
            "finished_at": 2,
            "summary": "ok",
            "output": ""
        });
        let round = select_round(&session, RoundSelection::Last).unwrap();
        assert_eq!(round.task, "only task");
        assert_eq!(round.events.len(), 3);
    }
}

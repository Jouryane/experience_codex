//! Learning smoke (typed trace v1 contract probe):
//! sessions.json (read-only) -> trace_reader round selection -> distill ->
//! candidate Experience that passes schema validation.
//!
//! Guardrails (docs/p2-trace-v1.md):
//! - input = a COMPLETED session's LAST round; sessions containing legacy
//!   events and rounds without turn_completed are rejected by the reader;
//! - output goes to a scratch dir (deleted afterwards), never to store.json;
//! - candidate status = Draft (never Active); metadata tags
//!   source_trace_version=1.

use std::path::PathBuf;

use experience_core::domain::action::ActionPattern;
use experience_core::domain::experience::Experience;
use experience_core::domain::experience::ExperienceStatus;
use experience_core::domain::experience::FailurePolicy;
use experience_core::domain::experience::UndoPolicy;
use experience_core::domain::experience::WorkflowStep;
use experience_core::domain::predicate::Predicate;
use experience_core::experience::trace_event::TraceEvent;
use experience_core::experience::trace_reader::select_round;
use experience_core::experience::trace_reader::RoundSelection;
use experience_core::experience::trace_reader::RoundTrace;

fn main() {
    let sessions_path = std::env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(r"D:\experience_codex\experience-main\dist\.experience-home\sessions.json")
    });
    let round = read_latest_round(&sessions_path).unwrap_or_else(|error| {
        eprintln!("cannot read typed v1 trace: {error}");
        std::process::exit(1);
    });
    println!("task: {}", round.task);
    println!("round events: {}", round.events.len());
    for label in round.labels().iter().take(24) {
        println!("  {label}");
    }
    if round.events.len() > 24 {
        println!("  … (+{} more)", round.events.len() - 24);
    }

    let candidate = distill(&round.events);
    let issues = candidate.schema_issues();
    if !issues.is_empty() {
        eprintln!("candidate schema issues: {issues:?}");
        std::process::exit(1);
    }

    // Scratch output, immediately discarded (never store.json / never Active).
    let scratch = std::env::temp_dir().join("experience-learning-smoke");
    std::fs::create_dir_all(&scratch).expect("create scratch dir");
    let candidate_file = scratch.join("candidate.json");
    std::fs::write(
        &candidate_file,
        serde_json::to_string_pretty(&candidate).unwrap(),
    )
    .expect("write candidate");
    let meta = serde_json::json!({
        "source_trace_version": 1,
        "mode": "smoke",
        "status": "draft",
        "note": "typed v1 contract probe; reader rejects legacy/uncompleted"
    });
    let meta_file = scratch.join("meta.json");
    std::fs::write(&meta_file, serde_json::to_string_pretty(&meta).unwrap()).expect("write meta");

    println!("candidate name: {}", candidate.name);
    println!("candidate status: draft (never active)");
    println!("LEARNING_SMOKE: PASS (typed trace v1 -> distill -> schema-valid candidate)");
    let _ = std::fs::remove_dir_all(&scratch);
}

/// Latest session whose last completed round is valid typed v1 input.
fn read_latest_round(path: &PathBuf) -> Result<RoundTrace, String> {
    let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    let root: serde_json::Value = serde_json::from_str(&text).map_err(|error| error.to_string())?;
    let sessions = root
        .get("sessions")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "no sessions array".to_string())?;
    for session in sessions.iter().rev() {
        if let Some(round) = select_round(session, RoundSelection::Last) {
            return Ok(round);
        }
    }
    Err("no valid typed v1 session round found in sessions.json".to_string())
}

/// Minimal distill mapping (throwaway): tool_call markers become workflow
/// steps; everything else is context. Not a real Learning distill.
fn distill(events: &[TraceEvent]) -> Experience {
    let mut steps = Vec::new();
    for event in events {
        if let TraceEvent::ToolCall { name, .. } = event {
            steps.push(WorkflowStep::new(
                "exec_command",
                serde_json::json!({ "cmd": name }),
            ));
        }
    }
    if steps.is_empty() {
        steps.push(WorkflowStep::new(
            "exec_command",
            serde_json::json!({ "cmd": "noop" }),
        ));
    }
    Experience {
        name: "smoke_typed_trace_v1".to_string(),
        trigger: ActionPattern {
            tool: "exec_command".to_string(),
            command_pattern: Some("smoke".to_string()),
        },
        preconditions: vec![Predicate::new("cwd.exists", serde_json::json!(true))],
        workflow: steps,
        postconditions: vec![Predicate::new(
            "smoke.contract.ok",
            serde_json::json!(true),
        )],
        verification: Vec::new(),
        failure_policy: FailurePolicy::StopAndReport,
        undo: UndoPolicy::Unsupported,
        status: ExperienceStatus::Draft,
    }
}

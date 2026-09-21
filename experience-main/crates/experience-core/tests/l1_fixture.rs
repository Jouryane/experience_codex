//! No-token regression baseline for the L1 real acceptance artifacts.
//! The fixture mirrors what scripts/accept-l1-sink.ps1 produces: one inert
//! CANDIDATE (canonical exec_command) and a three-phase learning ledger.

use std::fs;

use experience_core::domain::experience::ExperienceStatus;
use experience_core::store::ExperienceStore;

fn fixture(name: &str) -> String {
    let root = env!("CARGO_MANIFEST_DIR");
    fs::read_to_string(format!(
        "{root}/tests/fixtures/l1/accept-baseline/{name}"
    ))
    .expect("fixture exists")
}

#[test]
fn fixture_candidate_is_inert_and_canonical() {
    let store = ExperienceStore::from_json(&fixture("store.json")).expect("store loads");
    let candidate = store
        .get("cand_accept_baseline")
        .expect("candidate present");
    assert_eq!(candidate.status, ExperienceStatus::Candidate);
    assert_eq!(candidate.trigger.tool, "exec_command");
    assert_eq!(candidate.workflow.len(), 1);
    assert_eq!(candidate.workflow[0].action, "exec_command");
    let cmd = candidate.workflow[0]
        .args
        .get("cmd")
        .and_then(serde_json::Value::as_str)
        .expect("cmd arg");
    assert!(cmd.contains("L1_SINK_COLD_OK"));
    let has_long_hex = cmd.split_whitespace().any(|token| {
        token.len() >= 16 && token.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
    });
    assert!(!has_long_hex, "cmd must not carry an ephemeral hex token");
}

#[test]
fn fixture_ledger_records_three_phase_order() {
    let ledger = fixture("learning-l1.json");
    let written = ledger.find("\"record_type\": \"written\"").expect("written");
    let repeat = ledger
        .find("rejected:repeat_no_improvement")
        .expect("repeat rejection");
    let noop = ledger
        .find("rejected:no_action_evidence")
        .expect("no-action rejection");
    assert!(written < repeat && repeat < noop, "phase order broken");
}

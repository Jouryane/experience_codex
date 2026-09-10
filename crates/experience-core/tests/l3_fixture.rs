//! No-token regression baseline for the L3 v2 execute-first real acceptance
//! artifacts (scripts/accept-l3-execute.ps1). The fixture mirrors the
//! execute phase: ACTIVE create_probe_file -> local write_file success ->
//! delegate with structured plan -> delegation_completed (v2 scope).

use std::fs;

use experience_core::domain::experience::ExperienceStatus;
use experience_core::store::ExperienceStore;

fn fixture(name: &str) -> String {
    let root = env!("CARGO_MANIFEST_DIR");
    fs::read_to_string(format!(
        "{root}/tests/fixtures/l3/accept-execute/{name}"
    ))
    .expect("fixture exists")
}

#[test]
fn fixture_store_active_is_locally_runnable() {
    let store = ExperienceStore::from_json(&fixture("store.json")).expect("store loads");
    let experience = store
        .get("create_probe_file")
        .expect("active seed present");
    assert_eq!(experience.status, ExperienceStatus::Active);
    assert_eq!(experience.trigger.tool, "exec_command");
    assert!(
        experience
            .workflow
            .iter()
            .all(|step| step.action == "write_file"),
        "seed must be locally runnable (write_file-only)"
    );
    let has_fs_postcondition = experience
        .postconditions
        .iter()
        .any(|predicate| predicate.key == "file:probe.txt.content");
    assert!(has_fs_postcondition, "completion criterion must be fs-observable");
}

#[test]
fn fixture_ledger_layers_execution_then_delegate_then_completed() {
    let ledger = fixture("learning-l1.json");
    let exec = ledger.find("\"session_id\": \"s-l3-fixture-r1\"").expect("round 1");
    let delegate = ledger.find("\"record_type\": \"delegate\"").expect("delegate");
    let injection = ledger.find("\"record_type\": \"injection\"").expect("injection");
    let written = ledger.find("\"record_type\": \"written\"").expect("written");
    let rejected = ledger
        .find("rejected:repeat_no_improvement")
        .expect("repeat rejection");
    let completed = ledger
        .rfind("\"record_type\": \"delegation_completed\"")
        .expect("delegation_completed");
    assert!(
        exec < delegate
            && delegate < injection
            && injection < written
            && written < rejected
            && rejected < completed,
        "ledger layering order broken"
    );
    assert_eq!(ledger.matches("executed_first:create_probe_file:success").count(), 2);
    assert_eq!(ledger.matches("\"reason\": \"policy_off\"").count(), 2);
    assert_eq!(ledger.matches("\"outcome\": \"omitted\"").count(), 2);
    assert_eq!(ledger.matches("scope=l3_v2").count(), 2);
    assert!(!ledger.contains("\"record_type\": \"decayed\""));
}

#[test]
fn fixture_delegate_trace_carries_structured_plan() {
    let sessions = fixture("sessions.json");
    assert!(sessions.contains("\"kind\": \"delegate\""));
    assert!(sessions.contains("\"plan\": ["));
    assert!(sessions.contains("\"create_probe_file\""));
    assert!(sessions.contains("[plan: create_probe_file]"));
}

#[test]
fn fixture_usage_reflects_two_successes_without_decay() {
    let usage = fixture("usage.json");
    assert!(usage.contains("\"successes\": 2"));
    assert!(usage.contains("\"usage_count\": 2"));
    assert!(usage.contains("\"misfires\": 0"));
    assert!(usage.contains("\"invalid\": 0"));
    assert!(usage.contains("\"execution_errors\": 0"));
    assert!(usage.contains("\"score\": 0.66"));
    assert!(usage.contains("\"schema_version\": 1"));
}

fn llm_fixture(name: &str) -> String {
    let root = env!("CARGO_MANIFEST_DIR");
    fs::read_to_string(format!(
        "{root}/tests/fixtures/l3/accept-llm/{name}"
    ))
    .expect("llm fixture exists")
}

#[test]
fn fixture_llm_dirty_round_writes_candidate() {
    let store = ExperienceStore::from_json(&llm_fixture("store.json")).expect("store loads");
    let candidate = store
        .get("create_seq_a_d_files")
        .expect("llm-compiled candidate present");
    assert_eq!(candidate.status, ExperienceStatus::Candidate);
    assert!(candidate.workflow.len() >= 4);
    assert!(
        candidate
            .workflow
            .iter()
            .all(|step| step.action == "exec_command")
    );
    let ledger = llm_fixture("learning-l1.json");
    assert!(ledger.contains("\"record_type\": \"written\""));
    let sessions = llm_fixture("sessions.json");
    let tool_calls = sessions.matches("\"kind\": \"toolCall\"").count();
    assert!(tool_calls > 3, "fixture round must be dirty");
    assert!(sessions.contains("\"kind\": \"turnCompleted\""));
}

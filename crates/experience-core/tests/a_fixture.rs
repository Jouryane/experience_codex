//! No-token baseline for Stage A reference layer: additive `references`
//! section with provenance/trust metadata, separate from executable bodies.

use std::fs;

fn fixture() -> String {
    let root = env!("CARGO_MANIFEST_DIR");
    fs::read_to_string(format!(
        "{root}/tests/fixtures/a/references/store.json"
    ))
    .expect("stage A fixture exists")
}

#[test]
fn fixture_reference_entry_carries_provenance_and_trust() {
    let store = fixture();
    assert!(store.contains("\"references\": {"));
    assert!(store.contains("\"ref_trae_dark_theme\""));
    assert!(store.contains("\"source_agent\": \"trae\""));
    assert!(store.contains("\"trust_level\": \"workspace_verified\""));
    assert!(store.contains("\"plugins_used\""));
}

#[test]
fn fixture_reference_is_not_an_executable_experience() {
    let store = fixture();
    let experiences = store.matches("\"workflow\":").count();
    assert_eq!(experiences, 1, "only the real experience has a workflow");
    assert!(store.contains("\"steps\": ["));
}

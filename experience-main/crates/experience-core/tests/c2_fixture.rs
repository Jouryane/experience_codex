//! No-token baseline for Stage C2 editing: adopt keeps the original name,
//! lifecycle status, scope and pinned state while replacing the body.

use std::fs;

fn fixture(name: &str) -> String {
    let root = env!("CARGO_MANIFEST_DIR");
    fs::read_to_string(format!("{root}/tests/fixtures/c2/edit/{name}"))
        .expect("c2 fixture exists")
}

#[test]
fn fixture_edit_adopt_preserves_identity_status_and_scope() {
    let original = fixture("store.json");
    let adopted = fixture("expected-after-adopt.json");
    assert!(adopted.contains("\"name\": \"create_probe_a\""));
    assert!(adopted.contains("\"status\": \"active\""));
    assert!(adopted.contains("\"probe-new.txt\""));
    assert!(adopted.contains("\"A-NEW\""));
    assert!(adopted.contains("\"create_probe_a\": \"scene-a\""));
    assert!(!adopted.contains("__draft"));
    assert!(!original.contains("__draft"));
}

#[test]
fn fixture_edit_draft_is_never_visible_in_original_store() {
    let original = fixture("store.json");
    assert!(!original.contains("__draft"));
}

fn preference_fixture() -> String {
    let root = env!("CARGO_MANIFEST_DIR");
    fs::read_to_string(format!(
        "{root}/tests/fixtures/c2/preference/store.json"
    ))
    .expect("c2 preference fixture exists")
}

#[test]
fn fixture_user_alias_and_preference_are_additive_metadata() {
    let store = preference_fixture();
    assert!(store.contains("\"display_names\": {"));
    assert!(store.contains("\"A 场景探针经验\""));
    assert!(store.contains("\"user_usage\": {"));
    assert!(store.contains("\"deny\""));
    assert!(store.contains("\"user_confidence\": {"));
    assert!(store.contains(": 0.9"));
}

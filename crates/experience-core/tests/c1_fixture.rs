//! No-token regression baseline for Stage C1 scope metadata: experiences
//! carry additive `scopes` (name -> scene), and unscoped experiences stay
//! visible in every scene (backward compatible).

use std::fs;

fn fixture(name: &str) -> String {
    let root = env!("CARGO_MANIFEST_DIR");
    fs::read_to_string(format!(
        "{root}/tests/fixtures/c1/scope/{name}"
    ))
    .expect("c1 fixture exists")
}

#[test]
fn fixture_scope_store_maps_experiences_to_scenes() {
    let store = fixture("store.json");
    assert!(store.contains("\"scopes\": {"));
    assert!(store.contains("\"create_probe_a\": \"scene-a\""));
    assert!(store.contains("\"cand_create_probe_b\": \"scene-b\""));
    assert!(store.contains("\"create_probe_global\""));
}

#[test]
fn fixture_scope_store_has_active_and_unscoped_experiences() {
    let store = fixture("store.json");
    let active = store.matches("\"status\": \"active\"").count();
    assert_eq!(active, 2);
    assert!(store.contains("\"status\": \"candidate\""));
}

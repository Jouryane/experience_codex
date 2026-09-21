//! No-token baseline for Stage C4 channel export shape: scope-filtered
//! export keeps scoped + unscoped experiences, drops out-of-scope ones, and
//! carries additive metadata maps.

use std::fs;

fn fixture() -> String {
    let root = env!("CARGO_MANIFEST_DIR");
    fs::read_to_string(format!(
        "{root}/tests/fixtures/c4/channel/export-scene-a.json"
    ))
    .expect("c4 export fixture exists")
}

#[test]
fn fixture_export_is_scope_filtered_and_keeps_unscoped() {
    let export = fixture();
    assert!(export.contains("\"create_probe_a\""));
    assert!(export.contains("\"create_probe_global\""));
    assert!(!export.contains("cand_create_probe_b"));
}

#[test]
fn fixture_export_carries_additive_metadata_maps() {
    let export = fixture();
    assert!(export.contains("\"display_names\": {}"));
    assert!(export.contains("\"user_usage\": {}"));
    assert!(export.contains("\"user_confidence\": {}"));
    assert!(export.contains("\"schema_version\": 1"));
}

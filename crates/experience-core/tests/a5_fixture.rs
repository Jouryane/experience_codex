//! A5 material fixture: the Trae A4 run-notes document is the absorbed
//! reference; this guards the document conventions and the derived
//! ReferenceEntry shape.

use std::fs;

fn workspace_file(relative: &str) -> String {
    let root = env!("CARGO_MANIFEST_DIR");
    fs::read_to_string(format!("{root}/../../{relative}"))
        .unwrap_or_else(|error| panic!("{relative}: {error}"))
}

#[test]
fn trae_a4_run_notes_follow_section_conventions() {
    let notes = workspace_file("docs/run-notes.md");
    for heading in [
        "目标与约束",
        "使用的工具与插件",
        "步骤与子步骤",
        "失败与修正",
        "最终产物清单与验证方法",
        "可复用的模板与参数位",
    ] {
        assert!(notes.contains(heading), "missing section {heading}");
    }
    assert!(notes.contains("absorb"));
    assert!(notes.contains("git diff"));
}

#[test]
fn a5_reference_fixture_carries_provenance_and_steps() {
    let reference = workspace_file("crates/experience-core/tests/fixtures/a5/trae-a4-reference.json");
    assert!(reference.contains("\"source_agent\": \"trae\""));
    assert!(reference.contains("\"trust_level\": \"declared\""));
    assert!(reference.contains("\"absorb page"));
    assert!(reference.contains("\"steps\""));
}

//! Fixture guard for run-notes parsing conventions (Stage A absorption).

use std::fs;

fn fixture() -> String {
    let root = env!("CARGO_MANIFEST_DIR");
    fs::read_to_string(format!(
        "{root}/tests/fixtures/a/run-notes/run-notes.md"
    ))
    .expect("run-notes fixture exists")
}

#[test]
fn fixture_run_notes_has_expected_sections() {
    let notes = fixture();
    for heading in [
        "目标与约束",
        "使用的工具与插件",
        "步骤与子步骤",
        "失败与修正",
        "最终产物与验证",
        "可复用模板与参数位",
    ] {
        assert!(notes.contains(heading), "missing section {heading}");
    }
    assert!(notes.contains("ui-kit"));
    assert!(notes.contains("git diff --stat"));
    assert!(notes.contains("src/theme.css"));
}

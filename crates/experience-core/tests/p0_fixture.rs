//! P0 acceptance fixture: the leak corpus must be fully masked by the
//! adaptive + innate redaction layers, idempotently.

use std::fs;

use experience_core::redact::Disclosure;
use experience_core::redact::RedactMode;
use experience_core::redact::Redactor;
use experience_core::safety::{FsGuardError, safe_join};

fn corpus() -> serde_json::Value {
    let root = env!("CARGO_MANIFEST_DIR");
    let text = fs::read_to_string(format!("{root}/tests/fixtures/p0/leak-corpus.json"))
        .expect("leak corpus");
    serde_json::from_str(&text).expect("valid corpus json")
}

#[test]
fn leak_corpus_is_masked_in_free_text_and_structured_values() {
    let corpus = corpus();
    let redactor = Redactor::new(b"p0-key", Disclosure::Structure, RedactMode::Standard);
    let free_text = corpus["free_text"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item.as_str().unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    let redacted_text = redactor.redact_text(&free_text);
    let redacted_json = redactor.redact_json(&corpus["structured"], None);
    let combined = format!("{redacted_text}\n{redacted_json}");
    for secret in corpus["must_not_appear"].as_array().unwrap() {
        let secret = secret.as_str().unwrap();
        assert!(!combined.contains(secret), "leaked: {secret}");
    }
    assert!(combined.contains("<masked:"));
    let twice = format!(
        "{}\n{}",
        redactor.redact_text(&redacted_text),
        redactor.redact_json(&redacted_json, None)
    );
    assert_eq!(combined, twice, "redaction must be idempotent");
}

#[test]
fn workspace_guard_rejects_escape_and_accepts_relative() {
    let dir = std::env::temp_dir().join(format!(
        "exp-p0-guard-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    assert!(safe_join(&dir, "src/app.ts").is_ok());
    assert!(matches!(
        safe_join(&dir, r"..\..\escape.txt"),
        Err(FsGuardError::Escapes)
    ));
    assert!(matches!(
        safe_join(&dir, r"C:\escape.txt"),
        Err(FsGuardError::Absolute)
    ));
    let _ = fs::remove_dir_all(&dir);
}

//! Executor capability: turns a pre-compiled WorkflowStep into real effects.

use std::fs;

use experience_core::domain::experience::WorkflowStep;
use experience_core::domain::gate::ExecutedSideEffect;
use experience_core::safety::safe_join;
use serde_json::Value;

use super::runtime::GateContext;

/// Result of one successful step execution.
#[derive(Debug, Clone, PartialEq)]
pub struct StepOutcome {
    pub evidence: String,
    pub side_effects: Vec<ExecutedSideEffect>,
}

/// Maps a compiled workflow action to a real tool capability.
///
/// P1 scope is deliberately small: `write_file` is implemented locally;
/// anything else is an explicit Unsupported error (never silently skipped).
pub trait CapabilityRunner: Send + Sync {
    fn run(&self, context: &GateContext, step: &WorkflowStep) -> Result<StepOutcome, String>;
}

/// Local, real-filesystem runner used by M3 before the Codex adapter lands.
#[derive(Debug, Clone, Default)]
pub struct LocalRunner;

impl CapabilityRunner for LocalRunner {
    fn run(&self, context: &GateContext, step: &WorkflowStep) -> Result<StepOutcome, String> {
        match step.action.as_str() {
            "write_file" => {
                let path = step
                    .args
                    .get("path")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "write_file: missing string arg 'path'".to_string())?;
                let content = step
                    .args
                    .get("content")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "write_file: missing string arg 'content'".to_string())?;
                let target = safe_join(&context.cwd, path)
                    .map_err(|error| format!("write_file: path guard rejected: {error}"))?;
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent).map_err(|error| {
                        format!("write_file: cannot create parent dir: {error}")
                    })?;
                }
                fs::write(&target, content).map_err(|error| {
                    format!("write_file: failed to write {}: {error}", target.display())
                })?;
                Ok(StepOutcome {
                    evidence: format!(
                        "write_file {} ({} bytes)",
                        path,
                        content.len()
                    ),
                    side_effects: vec![ExecutedSideEffect {
                        description: format!("wrote {path}"),
                        evidence: Some(target.display().to_string()),
                    }],
                })
            }
            other => Err(format!("unsupported workflow action: '{other}'")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use experience_core::domain::experience::WorkflowStep;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("exp-runner-{tag}-{nanos}"))
    }

    #[test]
    fn write_file_creates_path_with_content() {
        let dir = temp_dir("write");
        fs::create_dir_all(&dir).unwrap();
        let runner = LocalRunner;
        let context = GateContext { cwd: dir.clone() };
        let step = WorkflowStep::new(
            "write_file",
            serde_json::json!({ "path": "sub/probe.txt", "content": "EXPERIENCE_GATE_SUCCESS" }),
        );
        let outcome = runner.run(&context, &step).unwrap();
        assert!(outcome.evidence.contains("write_file"));
        assert_eq!(
            fs::read_to_string(dir.join("sub/probe.txt")).unwrap(),
            "EXPERIENCE_GATE_SUCCESS"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unsupported_action_is_explicit_error() {
        let dir = temp_dir("unsupported");
        fs::create_dir_all(&dir).unwrap();
        let runner = LocalRunner;
        let context = GateContext { cwd: dir.clone() };
        let step = WorkflowStep::new("exec_command", serde_json::json!({ "cmd": "dir" }));
        assert!(runner.run(&context, &step).unwrap_err().contains("unsupported"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_file_rejects_workspace_escape() {
        let dir = temp_dir("escape");
        fs::create_dir_all(&dir).unwrap();
        let runner = LocalRunner;
        let context = GateContext { cwd: dir.clone() };
        for bad in [r"..\..\escape.txt", r"C:\escape.txt", "sub/../../escape.txt"] {
            let step = WorkflowStep::new(
                "write_file",
                serde_json::json!({ "path": bad, "content": "x" }),
            );
            let error = runner.run(&context, &step).unwrap_err();
            assert!(error.contains("path guard"), "must guard {bad}: {error}");
        }
        let _ = fs::remove_dir_all(&dir);
    }
}

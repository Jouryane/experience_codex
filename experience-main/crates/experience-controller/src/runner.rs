//! Executor capability: turns a pre-compiled WorkflowStep into real effects.
//!
//! S1-b: the actual execution lives in `experience_core::exec` so the Gate
//! path and the L3 server path share one implementation (policy check,
//! workspace fence, Tier1 file operations, backups). `LocalRunner` is the
//! adapter that binds that executor to the controller's `GateContext`.

use std::path::PathBuf;

use experience_core::domain::experience::WorkflowStep;
use experience_core::domain::gate::ExecutedSideEffect;
use experience_core::exec::execute_step;
use experience_core::exec::StepContext;
use experience_core::policy::CapabilityPolicy;

use super::runtime::GateContext;

/// Result of one successful step execution.
#[derive(Debug, Clone, PartialEq)]
pub struct StepOutcome {
    pub evidence: String,
    pub side_effects: Vec<ExecutedSideEffect>,
    /// Tier2 process evidence. `None` for pure file actions.
    pub exit_code: Option<i32>,
    /// Captured stdout (redacted, capped by policy). Empty for file actions.
    pub stdout: String,
    /// Captured stderr (redacted, capped by policy). Empty for file actions.
    pub stderr: String,
}

impl StepOutcome {
    /// Process succeeded (or was a non-process action).
    pub fn succeeded(&self) -> bool {
        matches!(self.exit_code, None | Some(0))
    }
}

/// Maps a compiled workflow action to a real tool capability. Anything not
/// implemented is an explicit error (never silently skipped).
pub trait CapabilityRunner: Send + Sync {
    fn run(&self, context: &GateContext, step: &WorkflowStep) -> Result<StepOutcome, String>;
}

/// Local, real-filesystem runner used by M3 before the Codex adapter lands.
#[derive(Debug, Clone, Default)]
pub struct LocalRunner {
    /// Optional per-run backup root (S1-c); `None` keeps the Gate path
    /// backup-free, matching the synchronous in-process contract.
    backup_root: Option<PathBuf>,
    /// Effective capability policy for this runner. Defaults to the
    /// conservative built-in policy, which denies exec.
    policy: CapabilityPolicy,
}

impl LocalRunner {
    /// Runner that backs up every mutated path under `root` before the change.
    pub fn with_backup(root: impl Into<PathBuf>) -> Self {
        Self {
            backup_root: Some(root.into()),
            policy: CapabilityPolicy::default(),
        }
    }

    /// Runner using an explicit capability policy.
    pub fn with_policy(policy: CapabilityPolicy) -> Self {
        Self {
            backup_root: None,
            policy,
        }
    }

    /// Runner with both backup root and explicit capability policy.
    pub fn with_backup_and_policy(
        root: impl Into<PathBuf>,
        policy: CapabilityPolicy,
    ) -> Self {
        Self {
            backup_root: Some(root.into()),
            policy,
        }
    }
}

impl CapabilityRunner for LocalRunner {
    fn run(&self, context: &GateContext, step: &WorkflowStep) -> Result<StepOutcome, String> {
        let mut context = StepContext::new(&context.cwd, &self.policy);
        let mut entries = Vec::new();
        if let Some(root) = self.backup_root.as_deref() {
            context = context.with_backup(root, &mut entries);
        }
        execute_step(&mut context, step)
            .map(|execution| StepOutcome {
                side_effects: execution
                    .side_effects
                    .iter()
                    .map(|description| ExecutedSideEffect {
                        description: description.clone(),
                        evidence: None,
                    })
                    .collect(),
                evidence: execution.evidence,
                exit_code: execution.exit_code,
                stdout: execution.stdout,
                stderr: execution.stderr,
            })
            .map_err(|error| error.detail)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
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
        let runner = LocalRunner::default();
        let context = GateContext { cwd: dir.clone() };
        let step = WorkflowStep::new(
            "write_file",
            serde_json::json!({ "path": "sub/probe.txt", "content": "EXPERIENCE_GATE_SUCCESS" }),
        );
        let outcome = runner.run(&context, &step).unwrap();
        assert!(outcome.evidence.contains("wrote sub/probe.txt"));
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
        let runner = LocalRunner::default();
        let context = GateContext { cwd: dir.clone() };
        let step = WorkflowStep::new("exec_command", serde_json::json!({ "cmd": "dir" }));
        // S1-a/S1-b: the shared executor refuses exec before any side effect.
        let error = runner.run(&context, &step).unwrap_err();
        assert!(error.contains("policy denied"), "{error}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_file_rejects_workspace_escape() {
        let dir = temp_dir("escape");
        fs::create_dir_all(&dir).unwrap();
        let runner = LocalRunner::default();
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

    #[test]
    fn explicit_policy_allows_allowlisted_exec() {
        let dir = temp_dir("exec-policy");
        fs::create_dir_all(&dir).unwrap();
        let mut policy = CapabilityPolicy::default();
        policy.exec.mode = experience_core::policy::EXEC_ALLOWLIST.to_string();
        policy.exec.allow = vec!["python".to_string()];
        let runner = LocalRunner::with_policy(policy);
        let context = GateContext { cwd: dir.clone() };
        let step = WorkflowStep::new(
            "exec",
            serde_json::json!({
                "program": "python",
                "args": ["-c", "print('runner-exec-ok')"]
            }),
        );
        let outcome = runner.run(&context, &step).unwrap();
        assert_eq!(outcome.exit_code, Some(0), "{}", outcome.evidence);
        assert!(outcome.stdout.contains("runner-exec-ok"), "{}", outcome.stdout);
        assert!(outcome.succeeded());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_policy_still_denies_exec_after_enrichment() {
        let dir = temp_dir("exec-default-deny");
        fs::create_dir_all(&dir).unwrap();
        let runner = LocalRunner::default();
        let context = GateContext { cwd: dir.clone() };
        let step = WorkflowStep::new(
            "exec",
            serde_json::json!({
                "program": "python",
                "args": ["-c", "print('should-not-run')"]
            }),
        );
        let error = runner.run(&context, &step).unwrap_err();
        assert!(error.contains("policy denied"), "{error}");
        let _ = fs::remove_dir_all(&dir);
    }
}

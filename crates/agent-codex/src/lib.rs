//! Codex executor adapter: THE single implementation of the codex exec
//! wrapper (contract: agent-runtime). Running paths must use this crate
//! instead of private copies.

use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;

use agent_runtime::AgentExecutor;
use agent_runtime::AgentTask;
use agent_runtime::ExecutorCapability;
use agent_runtime::RunReport;

pub mod session_host;

/// Level 1 codex executor: shells out to `codex exec`.
pub struct CodexAdapter {
    pub exe: PathBuf,
}

impl CodexAdapter {
    pub fn new(exe: impl Into<PathBuf>) -> Self {
        Self { exe: exe.into() }
    }
}

impl AgentExecutor for CodexAdapter {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn capabilities(&self) -> Vec<ExecutorCapability> {
        vec![
            ExecutorCapability::TaskExecution,
            ExecutorCapability::SessionChannel,
        ]
    }

    fn run(&self, task: &AgentTask) -> RunReport {
        let mut prompt = task.text.clone();
        if let Some(reference) = &task.reference {
            prompt.push_str("\n\n[参考经验，仅作参考，可质疑/改编]\n");
            prompt.push_str(reference);
        }
        let mut child = match Command::new(&self.exe)
            .args(["exec", "--skip-git-repo-check"])
            .current_dir(task.cwd.as_deref().unwrap_or("."))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                return RunReport {
                    ok: false,
                    summary: format!("failed to spawn codex: {error}"),
                    raw_output: String::new(),
                };
            }
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(prompt.as_bytes());
        }
        let output = match child.wait_with_output() {
            Ok(output) => output,
            Err(error) => {
                return RunReport {
                    ok: false,
                    summary: format!("codex run error: {error}"),
                    raw_output: String::new(),
                };
            }
        };
        let raw = String::from_utf8_lossy(&output.stdout).into_owned();
        RunReport {
            ok: output.status.success(),
            summary: raw.lines().last().unwrap_or_default().to_string(),
            raw_output: raw,
        }
    }
}

/// Session-capable spawn: returns the live child (used by experience-server's
/// cancellable session worker). Single source of codex exec invocation.
pub fn spawn_codex(
    executable: &Path,
    task: &str,
    cwd: Option<&Path>,
) -> Result<Child, String> {
    let mut command = Command::new(executable);
    command
        .arg("exec")
        .arg("--skip-git-repo-check")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("无法启动 {}: {error}", executable.display()))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(task.as_bytes());
        let _ = stdin.write_all(b"\n");
    }
    Ok(child)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_declares_task_execution_and_session_channel() {
        let adapter = CodexAdapter::new("codex");
        assert_eq!(
            adapter.capabilities(),
            vec![
                ExecutorCapability::TaskExecution,
                ExecutorCapability::SessionChannel,
            ]
        );
        assert_eq!(adapter.id(), "codex");
    }
}

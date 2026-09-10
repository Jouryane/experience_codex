//! Agent Manager: executor configuration + connection state machine.
//!
//! Boundary (compatibility.md §4 / core-principle.md):
//! Experience does NOT own the agent and does NOT manage its secrets.
//! An agent is identified by pointing at its application DIRECTORY (the
//! launchable executable is discovered inside) or at an executable
//! path/name directly. Keys / login state / model provider stay inside the
//! agent application itself — Experience never asks for them.
//!
//! Fail-loud semantics (compatibility.md §1.1): readiness probes surface
//! exact errors; delegation refuses against an Error state.

use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;
use std::sync::mpsc;
use std::time::Duration;

use serde::Deserialize;
use serde::Serialize;

use crate::fs_browse::shortcut_target;

pub const KIND_CODEX_CLI: &str = "codex_cli";
pub const MODE_MANAGED: &str = "managed";
#[allow(dead_code)] // reserved: attached relationship for app-server style executors
pub const MODE_ATTACHED: &str = "attached";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    /// Saved but never checked this session.
    Configured,
    /// Readiness probe in flight.
    Checking,
    /// Ready to delegate tasks (managed CLI executor: no persistent
    /// process; tasks spawn on demand).
    Ready,
    /// Last probe failed — delegation must refuse with this message.
    Error,
    /// Explicitly marked not-in-use.
    Disconnected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentStatus {
    pub state: AgentState,
    pub message: String,
    pub updated_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}

impl AgentStatus {
    fn new(state: AgentState, message: impl Into<String>) -> Self {
        Self {
            state,
            message: message.into(),
            updated_at: now_secs(),
            pid: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub id: String,
    pub label: String,
    /// Executor kind. P1: `codex_cli` (Level 1 TaskExecution).
    pub kind: String,
    /// Connection relationship. codex_cli only supports `managed`.
    pub mode: String,
    /// Agent application directory (executable discovered inside), or the
    /// working location of the agent app. At least one of directory /
    /// executable must be set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
    /// Optional override: executable file name inside `directory`, a full
    /// path, or a command name on PATH.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,
    /// Optional flag appended for the readiness probe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_arg: Option<String>,
    /// Delegation channel policy: "exec" (one-shot) or "session" (real
    /// codex Session Host via app-server). Defaults to exec when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    /// Optional CODEX_HOME for session channel (which codex config to use).
    /// Env CODEX_HOME is the fallback; never a hardcoded dev path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_home: Option<String>,
}

impl AgentConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty()
            || !self
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err("id 只能包含字母/数字/_/-".to_string());
        }
        if self.label.trim().is_empty() {
            return Err("label 不能为空".to_string());
        }
        if self.directory.is_none() && self.executable.is_none() {
            return Err("请提供 agent 应用目录（directory）或可执行文件（executable）".to_string());
        }
        match self.kind.as_str() {
            KIND_CODEX_CLI => {
                if self.mode != MODE_MANAGED {
                    return Err(format!(
                        "codex_cli 仅支持 managed 模式（任务按需拉起）；attached 保留给 app-server 类 executor"
                    ));
                }
            }
            other => return Err(format!("unsupported agent kind: {other}")),
        }
        if let Some(channel) = &self.channel {
            if channel != "exec" && channel != "session" {
                return Err(format!("unknown channel '{channel}' (exec|session)"));
            }
        }
        Ok(())
    }

    /// Resolve the launchable executable. When only a directory is given,
    /// kind-specific candidate names are probed inside it.
    pub fn resolve_executable(&self) -> Result<PathBuf, String> {
        let directory = self.directory.as_deref().map(Path::new);
        if let Some(exe) = self.executable.as_deref() {
            let path = Path::new(exe);
            if path.is_absolute() {
                return resolve_launcher(path);
            }
            if path.components().count() > 1 {
                // Relative path with separators: resolve against directory.
                let Some(dir) = directory else {
                    return Err("相对路径的 executable 需要同时提供 directory".to_string());
                };
                let full = dir.join(path);
                return resolve_launcher(&full);
            }
            if let Some(dir) = directory {
                let full = dir.join(path);
                return resolve_launcher(&full);
            }
            // Plain command name: let the OS resolve via PATH.
            return Ok(path.to_path_buf());
        }
        let dir = directory.ok_or_else(|| "未提供 executable，也没有 directory".to_string())?;
        if !dir.is_dir() {
            return Err(format!("agent 目录不存在：{}", dir.display()));
        }
        for candidate in kind_candidates(&self.kind) {
            let full = dir.join(candidate);
            if full.is_file() {
                return Ok(full);
            }
        }
        Err(format!(
            "在目录 {} 中未发现可执行文件（查找：{:?}）",
            dir.display(),
            kind_candidates(&self.kind)
        ))
    }
}

fn resolve_launcher(path: &Path) -> Result<PathBuf, String> {
    if !path.is_file() {
        return Err(format!("文件不存在：{}", path.display()));
    }
    let is_lnk = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("lnk"))
        .unwrap_or(false);
    if !is_lnk {
        return Ok(path.to_path_buf());
    }
    let (target, args, _working_dir) = shortcut_target(&path.to_string_lossy())?;
    if !args.trim().is_empty() {
        return Err(format!(
            "快捷方式带启动参数（{args}），请直接选择其目标 exe：{target}"
        ));
    }
    let target_path = PathBuf::from(target);
    if !target_path.is_file() {
        return Err(format!("快捷方式目标不存在：{}", target_path.display()));
    }
    Ok(target_path)
}

fn kind_candidates(kind: &str) -> &'static [&'static str] {
    match kind {
        KIND_CODEX_CLI => &["codex.exe", "codex", "bin/codex.exe", "target/debug/codex.exe"],
        _ => &[],
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentEntry {
    pub config: AgentConfig,
    pub status: AgentStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AgentsFile {
    schema_version: u32,
    #[serde(default)]
    agents: Vec<AgentConfig>,
}

impl Default for AgentsFile {
    fn default() -> Self {
        Self {
            schema_version: 2,
            agents: Vec::new(),
        }
    }
}

pub struct AgentManager {
    agents: Vec<AgentEntry>,
}

impl AgentManager {
    pub fn load_from_path(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(json) => {
                let file: AgentsFile = serde_json::from_str(&json).unwrap_or_default();
                if !matches!(file.schema_version, 1 | 2) {
                    return Self::with_default_seed();
                }
                let mut manager = Self { agents: Vec::new() };
                for config in file.agents {
                    if config.validate().is_ok() {
                        manager.agents.push(AgentEntry {
                            status: AgentStatus::new(
                                AgentState::Configured,
                                "已配置；点击“连接检查”确认就绪",
                            ),
                            config,
                        });
                    }
                }
                if manager.agents.is_empty() {
                    return Self::with_default_seed();
                }
                manager
            }
            Err(_) => Self::with_default_seed(),
        }
    }

    fn with_default_seed() -> Self {
        let mut manager = Self { agents: Vec::new() };
        manager.agents.push(default_codex_entry());
        manager
    }

    pub fn save_to_path(&self, path: &Path) -> std::io::Result<()> {
        let file = AgentsFile {
            schema_version: 2,
            agents: self.agents.iter().map(|entry| entry.config.clone()).collect(),
        };
        let json = serde_json::to_string_pretty(&file)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, json)
    }

    pub fn list(&self) -> Vec<AgentEntry> {
        self.agents.clone()
    }

    pub fn get(&self, id: &str) -> Option<&AgentEntry> {
        self.agents.iter().find(|entry| entry.config.id == id)
    }

    pub fn create(&mut self, config: AgentConfig) -> Result<AgentEntry, String> {
        config.validate()?;
        if self.get(&config.id).is_some() {
            return Err(format!("agent '{}' already exists", config.id));
        }
        let entry = AgentEntry {
            status: AgentStatus::new(
                AgentState::Configured,
                "已配置；点击“连接检查”确认就绪",
            ),
            config,
        };
        self.agents.push(entry.clone());
        Ok(entry)
    }

    pub fn remove(&mut self, id: &str) -> Result<(), String> {
        let before = self.agents.len();
        self.agents.retain(|entry| entry.config.id != id);
        if self.agents.len() == before {
            return Err(format!("agent '{id}' not found"));
        }
        Ok(())
    }

    /// Managed readiness check (fail loud): spawn `<executable> --version`
    /// with a bounded timeout and surface the exact outcome.
    pub fn connect(&mut self, id: &str) -> Result<AgentStatus, String> {
        let index = self
            .agents
            .iter()
            .position(|entry| entry.config.id == id)
            .ok_or_else(|| format!("agent '{id}' not found"))?;
        self.agents[index].status =
            AgentStatus::new(AgentState::Checking, "正在检查 executor 就绪状态…");
        let config = self.agents[index].config.clone();
        let result = probe_executor(&config);
        let status = match result {
            Ok(message) => AgentStatus::new(AgentState::Ready, message),
            Err(error) => AgentStatus::new(
                AgentState::Error,
                format!("executor 不可用：{error}"),
            ),
        };
        self.agents[index].status = status.clone();
        Ok(status)
    }

    pub fn disconnect(&mut self, id: &str) -> Result<AgentStatus, String> {
        let index = self
            .agents
            .iter()
            .position(|entry| entry.config.id == id)
            .ok_or_else(|| format!("agent '{id}' not found"))?;
        let status = AgentStatus::new(
            AgentState::Disconnected,
            "已断开；CLI executor 无常驻进程，任务时按需拉起",
        );
        self.agents[index].status = status.clone();
        Ok(status)
    }

    /// Delegate one task to a READY managed CLI executor. Spawns
    /// `<executable> exec --skip-git-repo-check`, feeds the task on stdin
    /// and returns the captured output. Refuses loudly when not Ready.
    pub fn spawn_task(
        &self,
        id: &str,
        task: &str,
        cwd: Option<&Path>,
    ) -> Result<std::process::Child, String> {
        let entry = self
            .agents
            .iter()
            .find(|entry| entry.config.id == id)
            .ok_or_else(|| format!("agent '{id}' not found"))?;
        if entry.status.state != AgentState::Ready {
            return Err(format!(
                "agent '{}' 未就绪（state={:?}）；请先连接检查：{}",
                id, entry.status.state, entry.status.message
            ));
        }
        let executable = entry.config.resolve_executable()?;
        agent_codex::spawn_codex(&executable, task, cwd)
    }
}

fn default_codex_entry() -> AgentEntry {
    let (directory, executable) = discover_codex();
    let id = std::env::var("EXPERIENCE_DEFAULT_AGENT_ID").unwrap_or_else(|_| "codex".to_string());
    AgentEntry {
        config: AgentConfig {
            id: id.clone(),
            label: format!("{id} (cli executor)"),
            kind: KIND_CODEX_CLI.to_string(),
            mode: MODE_MANAGED.to_string(),
            directory,
            executable,
            version_arg: None,
            channel: None,
            codex_home: None,
        },
        status: AgentStatus::new(
            AgentState::Configured,
            "已配置；点击“连接检查”确认就绪",
        ),
    }
}

/// Discovery defaults (never the study/fork checkout): official install,
/// then PATH. Returns (directory, executable).
fn discover_codex() -> (Option<String>, Option<String>) {
    if let Ok(exe) = std::env::var("CODEX_EXE") {
        return (None, Some(exe));
    }
    if let Some(dir) = official_codex_dir() {
        return (Some(dir), None);
    }
    (None, Some("codex".to_string()))
}

/// `%LOCALAPPDATA%\OpenAI\Codex\bin\<version-hash>\codex.exe` (newest wins).
fn official_codex_dir() -> Option<String> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    let bin = Path::new(&local).join("OpenAI").join("Codex").join("bin");
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(&bin).ok()?.flatten() {
        let candidate = entry.path().join("codex.exe");
        if candidate.is_file() {
            let modified = std::fs::metadata(&candidate).ok()?.modified().ok()?;
            if best
                .as_ref()
                .map(|(time, _)| modified > *time)
                .unwrap_or(true)
            {
                best = Some((modified, entry.path()));
            }
        }
    }
    best.map(|(_, dir)| dir.to_string_lossy().into_owned())
}

fn probe_executor(config: &AgentConfig) -> Result<String, String> {
    let executable = config.resolve_executable()?;
    let version_arg = config
        .version_arg
        .clone()
        .unwrap_or_else(|| "--version".to_string());
    let mut command = Command::new(&executable);
    command
        .arg(version_arg)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Probe from the agent directory when provided (mirrors launching the
    // agent app itself); tasks still honor their own cwd.
    if let Some(dir) = config.directory.as_deref() {
        let dir = Path::new(dir);
        if dir.is_dir() {
            command.current_dir(dir);
        }
    }
    let child = command
        .spawn()
        .map_err(|error| format!("无法启动 {}: {error}", executable.display()))?;

    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let output = child.wait_with_output();
        let _ = sender.send(output);
    });
    match receiver.recv_timeout(Duration::from_secs(15)) {
        Ok(Ok(output)) => {
            if output.status.success() {
                let first = String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .next()
                    .unwrap_or("executor ready")
                    .trim()
                    .to_string();
                Ok(format!("{first} (managed: 任务按需拉起)"))
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let hint = stderr.lines().next().unwrap_or("unknown error").trim();
                Err(format!(
                    "probe 失败（exit {:?}）：{hint}",
                    output.status.code()
                ))
            }
        }
        Ok(Err(error)) => Err(format!("probe 运行错误：{error}")),
        Err(_) => Err("probe 超时（15s）".to_string()),
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "exp-agent-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn config(directory: Option<&str>, executable: Option<&str>) -> AgentConfig {
        AgentConfig {
            id: "codex".into(),
            label: "codex".into(),
            kind: KIND_CODEX_CLI.into(),
            mode: MODE_MANAGED.into(),
            directory: directory.map(str::to_string),
            executable: executable.map(str::to_string),
            version_arg: None,
            channel: None,
            codex_home: None,
        }
    }

    #[test]
    fn codex_cli_rejects_attached_mode() {
        let mut config = config(Some("."), None);
        config.mode = MODE_ATTACHED.into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn unknown_channel_is_rejected() {
        let mut config = config(Some("."), None);
        config.channel = Some("bogus".into());
        assert!(config.validate().is_err());
    }

    #[test]
    fn directory_resolution_discovers_executable() {
        let dir = temp_dir("resolve");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("codex.exe"), b"fake").unwrap();
        let config = config(Some(dir.to_str().unwrap()), None);
        let resolved = config.resolve_executable().unwrap();
        assert_eq!(resolved.file_name().unwrap(), "codex.exe");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn directory_without_agent_binary_fails_loudly() {
        let dir = temp_dir("missing");
        std::fs::create_dir_all(&dir).unwrap();
        let config = config(Some(dir.to_str().unwrap()), None);
        let error = config.resolve_executable().unwrap_err();
        assert!(error.contains("未发现可执行文件"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn connect_with_missing_executable_fails_loudly() {
        let mut manager = AgentManager { agents: Vec::new() };
        manager
            .create(config(None, Some("definitely-not-a-real-codex-xyz.exe")))
            .unwrap();
        let status = manager.connect("codex").unwrap();
        assert_eq!(status.state, AgentState::Error);
        assert!(status.message.contains("executor 不可用"));
    }

    #[test]
    fn disconnect_marks_cli_executor_as_on_demand() {
        let mut manager = AgentManager { agents: Vec::new() };
        manager.create(config(None, Some("codex"))).unwrap();
        let status = manager.disconnect("codex").unwrap();
        assert_eq!(status.state, AgentState::Disconnected);
    }

    #[test]
    fn save_and_load_round_trip_keeps_configs() {
        let path = temp_dir("agents").join("agents.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut manager = AgentManager { agents: Vec::new() };
        manager.create(config(None, Some("codex"))).unwrap();
        manager.save_to_path(&path).unwrap();
        let loaded = AgentManager::load_from_path(&path);
        assert_eq!(loaded.list().len(), 1);
        assert_eq!(
            loaded.list()[0].config.executable.as_deref(),
            Some("codex")
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}

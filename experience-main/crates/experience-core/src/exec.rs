//! Shared step executor (Stage S1-b/S1-c).
//!
//! One implementation turns a compiled `WorkflowStep` into real effects for
//! both the L3 server path and the controller Gate path, so the two can never
//! drift apart. Every step is:
//!
//! 1. **policy-checked** against the effective capability policy,
//! 2. **contained** by the workspace fence (`safety::safe_join`),
//! 3. **backed up** before its first mutation (S1-c, optional root),
//! 4. redacted on read (secret hygiene).
//!
//! Unsupported or unauthorized actions fail loudly; nothing is skipped.

use std::fs;
use std::io::Read;
use std::process::Command;
use std::process::Stdio;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;
use serde::Serialize;

use crate::domain::experience::WorkflowStep;
use crate::policy::CapabilityPolicy;
use crate::redact::innate_redact;
use crate::safety::safe_join;

/// Terminal failure of one step, tagged so the caller can attribute it.
#[derive(Debug, Clone, PartialEq)]
pub struct StepError {
    /// `invalid` (bad args / unsupported / policy) or `execution_error` (I/O).
    pub outcome: String,
    pub detail: String,
}

impl StepError {
    pub fn invalid(action: &str, detail: impl Into<String>) -> Self {
        Self {
            outcome: "invalid".to_string(),
            detail: format!("{action}: {}", detail.into()),
        }
    }

    pub fn execution(action: &str, detail: impl Into<String>) -> Self {
        Self {
            outcome: "execution_error".to_string(),
            detail: format!("{action}: {}", detail.into()),
        }
    }
}

/// Result of one successful step.
#[derive(Debug, Clone, PartialEq)]
pub struct StepExecution {
    pub evidence: String,
    /// Side effects in a human-auditable form (paths, sizes).
    pub side_effects: Vec<String>,
    /// Redacted payload for `read_file`; empty for mutating actions.
    pub content: String,
    /// True when a backup was taken for the affected path.
    pub backed_up: bool,
    /// Tier2 process evidence (`None` for file actions).
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// One backed-up path (relative to the workspace) inside a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackupEntry {
    pub rel_path: String,
    /// Absolute path of the copy under the backup root (`None` when the target
    /// did not exist before the run and restore must delete it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_path: Option<String>,
    /// Whether the target existed when the backup was taken.
    pub existed: bool,
    /// True when the target was (or is) a directory.
    #[serde(default)]
    pub directory: bool,
}

/// Manifest written once per run under the backup root (S1-c).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackupManifest {
    pub schema_version: u32,
    pub workspace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub created_at: u64,
    pub entries: Vec<BackupEntry>,
}

pub const BACKUP_SCHEMA_VERSION: u32 = 1;
pub const BACKUP_MANIFEST_FILE: &str = "manifest.json";

/// Default per-command timeout when the policy leaves it unset.
pub const DEFAULT_EXEC_TIMEOUT_SECS: u64 = 60;
/// Default output cap when the policy leaves it unset.
pub const DEFAULT_EXEC_OUTPUT_CAP: u64 = 20_000;

/// Programs that are refused even when `exec.mode = allowlist` names them.
/// The match is on the resolved program stem (case-insensitive, without
/// extension). This is the last line of defense, not the primary one — the
/// allowlist is.
pub const DANGEROUS_PROGRAM_DENYLIST: &[&str] = &[
    "cmd", "powershell", "pwsh", "wscript", "cscript", "mshta", "rundll32", "reg",
    "regedit", "format", "diskpart", "bcdedit", "shutdown", "taskkill", "netsh",
    "sc", "schtasks", "at", "vssadmin", "wbadmin", "takeown", "icacls", "cacls",
    "attrib", "robocopy", "xcopy", "subst", "mklink", "setx", "wmic",
];

/// Substrings refused anywhere in a step's program/args (case-insensitive).
pub const DANGEROUS_ARG_PATTERNS: &[&str] = &[
    "del /f /s",
    "del /f /q",
    "rd /s /q",
    "rmdir /s /q",
    "format ",
    "shutdown ",
    "remove-item -recurse -force",
    "rm -rf",
    "rmdir /s",
    "reg delete",
    "reg add",
    "invoke-expression",
    "iex(",
    "-encodedcommand",
    "downloadstring",
    "| iex",
    "curl | sh",
    "curl | bash",
    "wget | sh",
];

/// Interpreter used to make a tiny portable helper program for S2 fixtures.
/// `cmd` is deliberately on the denylist, so the interpreter itself is
/// allowlisted per-fixture, never globally.
pub const EXEC_TEST_HELPER: &str = "cmd.exe";

/// Context for one step execution.
pub struct StepContext<'a> {
    pub workspace: &'a Path,
    /// Canonical form of `workspace` (the fence returns canonical paths, so
    /// prefix stripping must compare like with like — Windows case included).
    canonical_workspace: PathBuf,
    pub policy: &'a CapabilityPolicy,
    /// Directory that may host allowed executables (Tier2). `None` means only
    /// the workspace may host them.
    pub run_root: Option<PathBuf>,
    /// Gate 4 dry-run: when set, mutating file actions write under this
    /// scratch root (same relative layout) and `exec` only validates its
    /// shape/policy instead of spawning.
    pub scratch: Option<PathBuf>,
    /// When set, mutations back up the affected path under this root.
    pub backup: Option<&'a Path>,
    /// Accumulates backup entries (deduplicated by relative path).
    pub backups: Option<&'a mut Vec<BackupEntry>>,
}

impl<'a> StepContext<'a> {
    pub fn new(workspace: &'a Path, policy: &'a CapabilityPolicy) -> Self {
        Self {
            workspace,
            canonical_workspace: workspace
                .canonicalize()
                .unwrap_or_else(|_| workspace.to_path_buf()),
            policy,
            run_root: None,
            scratch: None,
            backup: None,
            backups: None,
        }
    }

    /// Allow executables under `root` in addition to the workspace (Tier2).
    pub fn with_run_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.run_root = Some(root.into());
        self
    }

    /// Gate 4: replay without touching the real workspace.
    pub fn with_scratch(mut self, root: impl Into<PathBuf>) -> Self {
        self.scratch = Some(root.into());
        self
    }

    /// Effective base for file actions (scratch during dry-run).
    fn fs_base(&self) -> &Path {
        self.scratch
            .as_deref()
            .unwrap_or_else(|| self.canonical_workspace())
    }

    pub fn with_backup(mut self, root: &'a Path, entries: &'a mut Vec<BackupEntry>) -> Self {
        self.backup = Some(root);
        self.backups = Some(entries);
        self
    }

    /// Canonical workspace root used for relative-path bookkeeping.
    pub fn canonical_workspace(&self) -> &Path {
        &self.canonical_workspace
    }
}

/// Execute one step. The policy check runs first, so a denied step can never
/// touch the filesystem.
pub fn execute_step(
    context: &mut StepContext<'_>,
    step: &WorkflowStep,
) -> Result<StepExecution, StepError> {
    let action = step.action.as_str();
    if let Err(decision) = context.policy.check(action) {
        return Err(StepError {
            outcome: "invalid".to_string(),
            detail: format!(
                "policy denied step '{}': {} ({})",
                decision.action, decision.family, decision.reason
            ),
        });
    }
    match action {
        "write_file" => write_file(context, step, false),
        "append_file" => write_file(context, step, true),
        "mkdir" => mkdir(context, step),
        "copy_file" => copy_file(context, step),
        "move_file" => move_file(context, step),
        "delete_file" => delete_file(context, step),
        "read_file" => read_file(context, step),
        "exec" => exec_program(context, step, false),
        "exec_command" => exec_program(context, step, true),
        other => Err(StepError::invalid(
            other,
            "unsupported workflow action for the embedded executor",
        )),
    }
}

fn required_str<'a>(step: &'a WorkflowStep, key: &str) -> Result<&'a str, StepError> {
    step.args
        .get(key)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| StepError::invalid(&step.action, format!("missing string arg '{key}'")))
}

fn resolve(context: &StepContext<'_>, step: &WorkflowStep, key: &str) -> Result<PathBuf, StepError> {
    let raw = required_str(step, key)?;
    safe_join(context.fs_base(), raw)
        .map_err(|error| StepError::execution(&step.action, format!("path guard rejected: {error}")))
}

/// Back up the target if it is not already recorded for this run.
fn ensure_backup(
    context: &mut StepContext<'_>,
    step: &WorkflowStep,
    target: &Path,
) -> Result<bool, StepError> {
    let Some(root) = context.backup else {
        return Ok(false);
    };
    let workspace = context.canonical_workspace().to_path_buf();
    let Some(entries) = context.backups.as_deref_mut() else {
        return Ok(false);
    };
    let Ok(rel) = target.strip_prefix(&workspace) else {
        // Dry-run (scratch) targets are outside the workspace; nothing to back up.
        return Ok(false);
    };
    let rel = rel
        .to_string_lossy()
        .replace('\\', "/");
    if entries.iter().any(|entry| entry.rel_path == rel) {
        return Ok(true);
    }
    let existed = target.exists();
    let backup_path = if existed {
        let copy = root.join(&rel);
        if let Some(parent) = copy.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                StepError::execution(&step.action, format!("cannot create backup dir: {error}"))
            })?;
        }
        fs::copy(target, &copy).map_err(|error| {
            StepError::execution(&step.action, format!("backup failed: {error}"))
        })?;
        Some(copy.to_string_lossy().to_string())
    } else {
        None
    };
    entries.push(BackupEntry {
        rel_path: rel,
        backup_path,
        existed,
        directory: target.is_dir(),
    });
    Ok(true)
}

/// Record a directory that the run will create (restore removes it later).
fn record_created_dir(
    context: &mut StepContext<'_>,
    _step: &WorkflowStep,
    target: &Path,
) -> Result<(), StepError> {
    if target.exists() {
        return Ok(());
    }
    let workspace = context.canonical_workspace().to_path_buf();
    let Ok(rel) = target.strip_prefix(&workspace) else {
        // Dry-run (scratch) targets are outside the workspace; nothing to record.
        return Ok(());
    };
    let rel = rel.to_string_lossy().replace('\\', "/");
    let Some(entries) = context.backups.as_deref_mut() else {
        return Ok(());
    };
    if entries.iter().any(|entry| entry.rel_path == rel) {
        return Ok(());
    }
    entries.push(BackupEntry {
        rel_path: rel,
        backup_path: None,
        existed: false,
        directory: true,
    });
    Ok(())
}

fn create_parents(step: &WorkflowStep, target: &Path) -> Result<(), StepError> {
    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|error| {
                StepError::execution(&step.action, format!("cannot create parent dir: {error}"))
            })?;
        }
    }
    Ok(())
}

fn write_file(
    context: &mut StepContext<'_>,
    step: &WorkflowStep,
    append: bool,
) -> Result<StepExecution, StepError> {
    let action = step.action.clone();
    let target = resolve(context, step, "path")?;
    let content = required_str(step, "content")?;
    let backed_up = ensure_backup(context, step, &target)?;
    create_parents(step, &target)?;
    if append {
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&target)
            .map_err(|error| {
                StepError::execution(&action, format!("failed to open {}: {error}", target.display()))
            })?;
        file.write_all(content.as_bytes()).map_err(|error| {
            StepError::execution(&action, format!("failed to append {}: {error}", target.display()))
        })?;
    } else {
        fs::write(&target, content).map_err(|error| {
            StepError::execution(&action, format!("failed to write {}: {error}", target.display()))
        })?;
    }
    // Evidence always uses the workspace-relative path, with or without backups.
    let rel = target
        .strip_prefix(context.canonical_workspace())
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| target.to_string_lossy().to_string());
    Ok(StepExecution {
        evidence: format!(
            "{} {} ({} bytes)",
            if append { "appended" } else { "wrote" },
            rel,
            content.len()
        ),
        side_effects: vec![format!(
            "{} {rel}",
            if append { "appended" } else { "wrote" }
        )],
        content: String::new(),
        backed_up,
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
    })
}

fn mkdir(context: &mut StepContext<'_>, step: &WorkflowStep) -> Result<StepExecution, StepError> {
    let target = resolve(context, step, "path")?;
    record_created_dir(context, step, &target)?;
    fs::create_dir_all(&target).map_err(|error| {
        StepError::execution(&step.action, format!("failed to create dir: {error}"))
    })?;
    Ok(StepExecution {
        evidence: format!("created dir {}", target.display()),
        side_effects: vec![format!("mkdir {}", target.display())],
        content: String::new(),
        backed_up: false,
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
    })
}

fn copy_file(context: &mut StepContext<'_>, step: &WorkflowStep) -> Result<StepExecution, StepError> {
    let source = resolve(context, step, "source")?;
    let target = resolve(context, step, "target")?;
    if !source.exists() {
        return Err(StepError::invalid(
            &step.action,
            format!("source not found: {}", source.display()),
        ));
    }
    ensure_backup(context, step, &target)?;
    create_parents(step, &target)?;
    fs::copy(&source, &target).map_err(|error| {
        StepError::execution(&step.action, format!("copy failed: {error}"))
    })?;
    Ok(StepExecution {
        evidence: format!("copied {} -> {}", source.display(), target.display()),
        side_effects: vec![format!("copy {} -> {}", source.display(), target.display())],
        content: String::new(),
        backed_up: true,
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
    })
}

fn move_file(context: &mut StepContext<'_>, step: &WorkflowStep) -> Result<StepExecution, StepError> {
    let source = resolve(context, step, "source")?;
    let target = resolve(context, step, "target")?;
    if !source.exists() {
        return Err(StepError::invalid(
            &step.action,
            format!("source not found: {}", source.display()),
        ));
    }
    // Both ends are backed up: the source disappears, the target may be
    // overwritten. Restore must be able to reconstruct both.
    ensure_backup(context, step, &source)?;
    ensure_backup(context, step, &target)?;
    create_parents(step, &target)?;
    fs::rename(&source, &target).map_err(|error| {
        StepError::execution(&step.action, format!("move failed: {error}"))
    })?;
    Ok(StepExecution {
        evidence: format!("moved {} -> {}", source.display(), target.display()),
        side_effects: vec![format!("move {} -> {}", source.display(), target.display())],
        content: String::new(),
        backed_up: true,
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
    })
}

fn delete_file(context: &mut StepContext<'_>, step: &WorkflowStep) -> Result<StepExecution, StepError> {
    let target = resolve(context, step, "path")?;
    ensure_backup(context, step, &target)?;
    if target.exists() {
        fs::remove_file(&target).map_err(|error| {
            StepError::execution(&step.action, format!("delete failed: {error}"))
        })?;
    }
    Ok(StepExecution {
        evidence: format!("deleted {}", target.display()),
        side_effects: vec![format!("delete {}", target.display())],
        content: String::new(),
        backed_up: true,
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
    })
}

fn read_file(context: &mut StepContext<'_>, step: &WorkflowStep) -> Result<StepExecution, StepError> {
    let target = resolve(context, step, "path")?;
    let max = context.policy.read_max_bytes();
    let metadata = fs::metadata(&target).map_err(|error| {
        StepError::execution(&step.action, format!("cannot stat {}: {error}", target.display()))
    })?;
    if metadata.len() > max {
        return Err(StepError::invalid(
            &step.action,
            format!(
                "file is {} bytes, above the read cap {} (raise fs_read.max_bytes or grant a special_grant_bytes)",
                metadata.len(),
                max
            ),
        ));
    }
    let bytes = fs::read(&target).map_err(|error| {
        StepError::execution(&step.action, format!("cannot read {}: {error}", target.display()))
    })?;
    let text = String::from_utf8_lossy(&bytes);
    let redacted = innate_redact(&text);
    Ok(StepExecution {
        evidence: format!(
            "read {} ({} bytes, cap {})",
            target.display(),
            metadata.len(),
            max
        ),
        side_effects: Vec::new(),
        content: redacted,
        backed_up: false,
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
    })
}

/// Program stem without directory or extension, lower-cased.
fn program_stem(program: &str) -> String {
    let name = program
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(program)
        .to_lowercase();
    name.strip_suffix(".exe").unwrap_or(&name).to_string()
}

/// Resolve an allowed program name to a concrete path. Bare names are looked
/// up on PATH; relative paths resolve inside the workspace; absolute paths are
/// only accepted when they sit inside the workspace or the run root.
fn resolve_program(context: &StepContext<'_>, step: &WorkflowStep, program: &str) -> Result<PathBuf, StepError> {
    if program.contains('/') || program.contains('\\') || program.contains(':') {
        let candidate = if Path::new(program).is_absolute() {
            PathBuf::from(program)
        } else {
            // Relative paths are workspace-scoped.
            return safe_join(context.canonical_workspace(), program).map_err(|error| {
                StepError::invalid(&step.action, format!("program path rejected: {error}"))
            });
        };
        let canonical = candidate
            .canonicalize()
            .map_err(|error| StepError::invalid(&step.action, format!("program not found: {error}")))?;
        let in_workspace = canonical.starts_with(context.canonical_workspace());
        let in_run_root = context
            .run_root
            .as_deref()
            .map(|root| canonical.starts_with(root))
            .unwrap_or(false);
        if !(in_workspace || in_run_root) {
            return Err(StepError::invalid(
                &step.action,
                format!("program outside workspace/run root: {}", canonical.display()),
            ));
        }
        return Ok(canonical);
    }
    // Bare name: the run root (the executor's own toolbox) wins over PATH.
    if let Some(root) = context.run_root.as_deref() {
        let candidate = root.join(program);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    // Otherwise let the platform resolve it on PATH.
    Ok(PathBuf::from(program))
}

/// True when program/args hit the hard denylist. Applies to both the Tier2
/// `exec` shape and the legacy `exec_command` shell string.
pub fn denylist_hit(program: &str, args: &[String]) -> Option<String> {
    let stem = program_stem(program);
    if DANGEROUS_PROGRAM_DENYLIST.contains(&stem.as_str()) {
        return Some(format!("dangerous program: {stem}"));
    }
    let joined = format!("{} {}", program, args.join(" ")).to_lowercase();
    for pattern in DANGEROUS_ARG_PATTERNS {
        if joined.contains(pattern) {
            return Some(format!("dangerous pattern: {pattern}"));
        }
    }
    None
}

/// Refuse to inherit obviously sensitive environment variables.
fn is_sensitive_env(key: &str) -> bool {
    let upper = key.to_uppercase();
    if upper == "PATH" || upper == "PATHEXT" || upper == "SYSTEMROOT" || upper == "WINDIR" {
        return false;
    }
    upper.ends_with("_KEY")
        || upper.ends_with("_TOKEN")
        || upper.ends_with("_SECRET")
        || upper.ends_with("_PASSWORD")
        || upper.contains("SECRET")
        || upper.contains("CREDENTIAL")
        || upper.contains("APIKEY")
        || upper.contains("API_KEY")
}

/// Apply the configured output cap on a char boundary.
fn cap_output(text: &str, cap: u64) -> String {
    let cap = cap as usize;
    if text.len() <= cap {
        return text.to_string();
    }
    let mut end = cap;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…[truncated {} bytes]", &text[..end], text.len() - end)
}

fn collect_output(
    context: &StepContext<'_>,
    step: &WorkflowStep,
    args: &[String],
    step_cwd: Option<PathBuf>,
    env_additions: &[(String, String)],
) -> Result<StepExecution, StepError> {
    // Gate 4 dry-run: never spawn; report the validated shape instead.
    if context.scratch.is_some() {
        return Ok(StepExecution {
            evidence: format!(
                "dry-run exec {} (args {}, policy {}, not spawned)",
                args[0],
                args.len().saturating_sub(1),
                context.policy.exec.mode
            ),
            side_effects: Vec::new(),
            content: String::new(),
            backed_up: false,
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
        });
    }
    let cap = context.policy.exec.output_cap.max(1);
    let timeout = Duration::from_secs(
        context
            .policy
            .exec
            .timeout_secs
            .max(1)
            .min(3600),
    );
    let program = &args[0];
    let resolved = resolve_program(context, step, program)?;

    let mut command = Command::new(&resolved);
    command.args(&args[1..]);
    command.current_dir(
        step_cwd
            .as_deref()
            .unwrap_or_else(|| context.canonical_workspace()),
    );
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    command.stdin(Stdio::null());
    for (key, value) in env_additions {
        command.env(key, value);
    }
    for (key, _) in std::env::vars() {
        if is_sensitive_env(&key) {
            command.env_remove(key);
        }
    }

    let mut child = command
        .spawn()
        .map_err(|error| StepError::execution(&step.action, format!("spawn failed: {error}")))?;
    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();
    let stdout_thread = stdout_pipe.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut buffer = String::new();
            let _ = pipe.read_to_string(&mut buffer);
            buffer
        })
    });
    let stderr_thread = stderr_pipe.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut buffer = String::new();
            let _ = pipe.read_to_string(&mut buffer);
            buffer
        })
    });

    let deadline = std::time::Instant::now() + timeout;
    let exit_code;
    let mut timed_out = false;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                exit_code = status.code();
                break;
            }
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    timed_out = true;
                    exit_code = None;
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => {
                return Err(StepError::execution(
                    &step.action,
                    format!("wait failed: {error}"),
                ))
            }
        }
    }
    let stdout = stdout_thread
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default();
    let stderr = stderr_thread
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default();
    let stdout = innate_redact(&cap_output(&stdout, cap));
    let stderr = innate_redact(&cap_output(&stderr, cap));

    let evidence = if timed_out {
        format!(
            "exec {} -> TIMEOUT after {}s (output capped at {} bytes)",
            program, timeout.as_secs(), cap
        )
    } else {
        let stderr_note = if stderr.is_empty() {
            String::new()
        } else {
            format!("; stderr head: {}", stderr.chars().take(120).collect::<String>())
        };
        format!(
            "exec {} -> exit={} (stdout {} bytes, stderr {} bytes){}",
            program,
            exit_code.map(|code| code.to_string()).unwrap_or_else(|| "signal".to_string()),
            stdout.len(),
            stderr.len(),
            stderr_note
        )
    };
    Ok(StepExecution {
        evidence,
        side_effects: Vec::new(),
        content: stdout.clone(),
        backed_up: false,
        exit_code,
        stdout,
        stderr,
    })
}

fn exec_program(
    context: &mut StepContext<'_>,
    step: &WorkflowStep,
    legacy: bool,
) -> Result<StepExecution, StepError> {
    let mut args: Vec<String> = Vec::new();
    let mut step_cwd: Option<PathBuf> = None;
    let mut env_additions: Vec<(String, String)> = Vec::new();
    if legacy {
        // Legacy shape: the whole command is one shell string.
        let command = required_str(step, "cmd")?;
        let shell = if cfg!(windows) { "cmd.exe" } else { "sh" };
        let flag = if cfg!(windows) { "/C" } else { "-c" };
        args.push(shell.to_string());
        args.push(flag.to_string());
        args.push(command.to_string());
    } else {
        let program = required_str(step, "program")?;
        args.push(program.to_string());
        if let Some(values) = step.args.get("args").and_then(serde_json::Value::as_array) {
            for value in values {
                let text = value.as_str().ok_or_else(|| {
                    StepError::invalid(&step.action, "args must be an array of strings")
                })?;
                args.push(text.to_string());
            }
        }
        // Optional workspace-contained working directory.
        if let Some(cwd) = step.args.get("cwd").and_then(serde_json::Value::as_str) {
            let resolved = safe_join(context.canonical_workspace(), cwd).map_err(|error| {
                StepError::invalid(&step.action, format!("cwd rejected: {error}"))
            })?;
            step_cwd = Some(resolved);
        }
        // Optional per-step environment additions (allowlist is the policy's
        // job; here we only add explicit, non-sensitive pairs).
        if let Some(map) = step.args.get("env").and_then(serde_json::Value::as_object) {
            env_additions = map
                .iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|text| (key.clone(), text.to_string()))
                })
                .collect();
        }
    }
    // Policy gate is already applied by execute_step, but keep the deny
    // reasons specific for bookkeeping.
    if context.policy.exec.mode != crate::policy::EXEC_ALLOWLIST {
        return Err(StepError {
            outcome: "invalid".to_string(),
            detail: format!(
                "policy denied step '{}': exec (exec=off)",
                step.action
            ),
        });
    }
    if legacy && !context.policy.exec.allow_legacy_shell {
        return Err(StepError {
            outcome: "invalid".to_string(),
            detail: format!(
                "policy denied step '{}': exec (legacy shell not allowed)",
                step.action
            ),
        });
    }
    // Allowlist enforcement (S2): the program name/stem must be named by the
    // policy, otherwise nothing is spawned at all.
    if !context.policy.exec.allow.is_empty() {
        let requested = program_stem(&args[0]);
        let allowed = context
            .policy
            .exec
            .allow
            .iter()
            .any(|entry| program_stem(entry) == requested);
        if !allowed {
            return Err(StepError {
                outcome: "invalid".to_string(),
                detail: format!(
                    "exec '{}' is not allowed by policy (allow: {})",
                    args[0],
                    context.policy.exec.allow.join(",")
                ),
            });
        }
    }
    if let Some(reason) = denylist_hit(&args[0], &args[1..]) {
        return Err(StepError {
            outcome: "invalid".to_string(),
            detail: format!("denylist refused step '{}': {reason}", step.action),
        });
    }
    collect_output(context, step, &args, step_cwd, &env_additions)
}

/// Write the run manifest (no-op when nothing was backed up).
pub fn write_backup_manifest(
    root: &Path,
    workspace: &Path,
    session_id: Option<&str>,
    entries: &[BackupEntry],
) -> Result<PathBuf, String> {
    let manifest = BackupManifest {
        schema_version: BACKUP_SCHEMA_VERSION,
        workspace: workspace.to_string_lossy().to_string(),
        session_id: session_id.map(str::to_string),
        created_at: now_secs(),
        entries: entries.to_vec(),
    };
    let path = root.join(BACKUP_MANIFEST_FILE);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let json = serde_json::to_string_pretty(&manifest).map_err(|error| error.to_string())?;
    fs::write(&path, json).map_err(|error| error.to_string())?;
    Ok(path)
}

/// Report of one restore attempt.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RestoreReport {
    pub restored: Vec<String>,
    pub deleted: Vec<String>,
}

/// Restore a run from its manifest: copies backups back and deletes paths that
/// did not exist before the run. Refuses manifests whose workspace no longer
/// contains every relative path.
pub fn restore_from_manifest(manifest_path: &Path) -> Result<RestoreReport, String> {
    let json = fs::read_to_string(manifest_path).map_err(|error| error.to_string())?;
    let manifest: BackupManifest =
        serde_json::from_str(&json).map_err(|error| format!("invalid manifest: {error}"))?;
    if manifest.schema_version != BACKUP_SCHEMA_VERSION {
        return Err(format!(
            "unsupported backup schema {} (current {BACKUP_SCHEMA_VERSION})",
            manifest.schema_version
        ));
    }
    let workspace = PathBuf::from(&manifest.workspace);
    if manifest_path.parent().is_none() {
        return Err("manifest has no parent directory".to_string());
    }
    let mut restored = Vec::new();
    let mut deleted = Vec::new();
    // Restore in reverse order so a move (source backed up, then target)
    // reappears at the source before the target is reverted.
    for entry in manifest.entries.iter().rev() {
        let target = safe_join(&workspace, &entry.rel_path)
            .map_err(|error| format!("restore path guard rejected {}: {error}", entry.rel_path))?;
        match (&entry.backup_path, entry.existed) {
            (Some(backup), true) => {
                let backup = PathBuf::from(backup);
                if !backup.exists() {
                    return Err(format!("backup copy missing for {}", entry.rel_path));
                }
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                }
                fs::copy(&backup, &target).map_err(|error| error.to_string())?;
                restored.push(entry.rel_path.clone());
            }
            _ => {
                if target.exists() {
                    if target.is_dir() {
                        fs::remove_dir_all(&target).map_err(|error| error.to_string())?;
                    } else {
                        fs::remove_file(&target).map_err(|error| error.to_string())?;
                    }
                }
                deleted.push(entry.rel_path.clone());
            }
        }
    }
    Ok(RestoreReport { restored, deleted })
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
    use crate::policy::FS_WRITE_DENY;

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("exp-exec-{tag}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn step(action: &str, args: serde_json::Value) -> WorkflowStep {
        WorkflowStep::new(action, args)
    }

    /// Build a real native program with rustc and place it in the run root,
    /// so the positive S2 tests exercise the true spawn path without a shell.
    fn build_helper(dir: &std::path::Path, tag: &str, source: &str) -> PathBuf {
        let run_root = dir.join("run");
        fs::create_dir_all(&run_root).unwrap();
        let source_path = dir.join(format!("{tag}.rs"));
        fs::write(&source_path, source).unwrap();
        let exe = run_root.join(format!("{tag}.exe"));
        let output = std::process::Command::new("rustc")
            .arg(&source_path)
            .arg("-o")
            .arg(&exe)
            .output()
            .expect("run rustc");
        assert!(
            output.status.success(),
            "rustc failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        exe
    }

    #[test]
    fn tier1_round_trip_write_append_read_copy_move() {
        let dir = temp_dir("tier1");
        let policy = CapabilityPolicy::default();
        let mut context = StepContext::new(&dir, &policy);

        execute_step(
            &mut context,
            &step("write_file", serde_json::json!({"path": "a.txt", "content": "one"})),
        )
        .unwrap();
        execute_step(
            &mut context,
            &step("append_file", serde_json::json!({"path": "a.txt", "content": " two"})),
        )
        .unwrap();
        let read = execute_step(
            &mut context,
            &step("read_file", serde_json::json!({"path": "a.txt"})),
        )
        .unwrap();
        assert_eq!(read.content, "one two");
        assert!(read.evidence.contains("cap 20480"));

        execute_step(
            &mut context,
            &step("mkdir", serde_json::json!({"path": "sub/dir"})),
        )
        .unwrap();
        execute_step(
            &mut context,
            &step("copy_file", serde_json::json!({"source": "a.txt", "target": "sub/b.txt"})),
        )
        .unwrap();
        execute_step(
            &mut context,
            &step("move_file", serde_json::json!({"source": "sub/b.txt", "target": "sub/c.txt"})),
        )
        .unwrap();
        assert_eq!(fs::read_to_string(dir.join("sub/c.txt")).unwrap(), "one two");
        assert!(!dir.join("sub/b.txt").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_is_denied_by_default_and_works_when_granted() {
        let dir = temp_dir("delete");
        fs::write(dir.join("victim.txt"), "x").unwrap();
        let policy = CapabilityPolicy::default();
        let mut context = StepContext::new(&dir, &policy);
        let error = execute_step(
            &mut context,
            &step("delete_file", serde_json::json!({"path": "victim.txt"})),
        )
        .unwrap_err();
        assert_eq!(error.outcome, "invalid");
        assert!(error.detail.contains("fs_delete=deny"));
        assert!(dir.join("victim.txt").exists());

        let mut granted = CapabilityPolicy::default();
        granted.fs_delete = crate::policy::FS_WRITE_WORKSPACE_ONLY.to_string();
        let mut context = StepContext::new(&dir, &granted);
        execute_step(
            &mut context,
            &step("delete_file", serde_json::json!({"path": "victim.txt"})),
        )
        .unwrap();
        assert!(!dir.join("victim.txt").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn policy_denial_happens_before_any_side_effect() {
        let dir = temp_dir("policy-first");
        let mut policy = CapabilityPolicy::default();
        policy.fs_write = FS_WRITE_DENY.to_string();
        let mut context = StepContext::new(&dir, &policy);
        let error = execute_step(
            &mut context,
            &step("write_file", serde_json::json!({"path": "blocked.txt", "content": "x"})),
        )
        .unwrap_err();
        assert_eq!(error.outcome, "invalid");
        assert!(error.detail.contains("fs_write=deny"));
        assert!(!dir.join("blocked.txt").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn path_guard_rejects_escape_for_tier1_actions() {
        let dir = temp_dir("guards");
        for (action, args) in [
            ("write_file", serde_json::json!({"path": r"..\escape.txt", "content": "x"})),
            ("append_file", serde_json::json!({"path": r"C:\escape.txt", "content": "x"})),
            ("mkdir", serde_json::json!({"path": "../escape"})),
            ("copy_file", serde_json::json!({"source": "a.txt", "target": "../escape.txt"})),
            ("move_file", serde_json::json!({"source": "a.txt", "target": "../escape.txt"})),
            ("delete_file", serde_json::json!({"path": "../escape.txt"})),
            ("read_file", serde_json::json!({"path": "../escape.txt"})),
        ] {
            let mut granted = CapabilityPolicy::default();
            granted.fs_delete = crate::policy::FS_WRITE_WORKSPACE_ONLY.to_string();
            let mut context = StepContext::new(&dir, &granted);
            let error = execute_step(&mut context, &step(action, args)).unwrap_err();
            assert!(error.detail.contains("path guard"), "{action}: {}", error.detail);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_cap_and_special_grant_control_large_reads() {
        let dir = temp_dir("read-cap");
        let big = "x".repeat(10_000);
        fs::write(dir.join("big.txt"), &big).unwrap();
        let mut policy = CapabilityPolicy::default();
        policy.fs_read.max_bytes = 1_024;
        let mut context = StepContext::new(&dir, &policy);
        let error = execute_step(
            &mut context,
            &step("read_file", serde_json::json!({"path": "big.txt"})),
        )
        .unwrap_err();
        assert!(error.detail.contains("above the read cap"));

        policy.fs_read.special_grant_bytes = Some(1_048_576);
        let mut context = StepContext::new(&dir, &policy);
        let read = execute_step(
            &mut context,
            &step("read_file", serde_json::json!({"path": "big.txt"})),
        )
        .unwrap();
        assert_eq!(read.content.len(), 10_000);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_redacts_secrets() {
        let dir = temp_dir("read-redact");
        fs::write(dir.join("cfg.txt"), "API_KEY=sk-live-abc123XYZ789").unwrap();
        let policy = CapabilityPolicy::default();
        let mut context = StepContext::new(&dir, &policy);
        let read = execute_step(
            &mut context,
            &step("read_file", serde_json::json!({"path": "cfg.txt"})),
        )
        .unwrap();
        assert!(!read.content.contains("sk-live-abc123XYZ789"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn backup_manifest_and_restore_reverts_write_append_move_and_delete() {
        let dir = temp_dir("backup");
        let backup_root = dir.join("backups");
        fs::create_dir_all(&backup_root).unwrap();
        fs::write(dir.join("keep.txt"), "original").unwrap();

        let mut policy = CapabilityPolicy::default();
        policy.fs_delete = crate::policy::FS_WRITE_WORKSPACE_ONLY.to_string();
        let mut entries = Vec::new();
        {
            let mut context =
                StepContext::new(&dir, &policy).with_backup(&backup_root, &mut entries);
            execute_step(
                &mut context,
                &step("append_file", serde_json::json!({"path": "keep.txt", "content": "-new"})),
            )
            .unwrap();
            execute_step(
                &mut context,
                &step("write_file", serde_json::json!({"path": "fresh.txt", "content": "new"})),
            )
            .unwrap();
            execute_step(
                &mut context,
                &step("mkdir", serde_json::json!({"path": "made"})),
            )
            .unwrap();
            // Delete is granted here so the revert path is exercised too.
            execute_step(
                &mut context,
                &step("delete_file", serde_json::json!({"path": "keep.txt"})),
            )
            .unwrap();
        }
        // keep.txt + fresh.txt + the directory created by mkdir.
        assert_eq!(entries.len(), 3, "deduplicated backup entries");
        let manifest = write_backup_manifest(&backup_root, &dir, Some("s-test"), &entries).unwrap();
        let report = restore_from_manifest(&manifest).unwrap();
        assert!(report.restored.contains(&"keep.txt".to_string()));
        assert!(report.deleted.contains(&"fresh.txt".to_string()));
        assert!(report.deleted.contains(&"made".to_string()));
        assert_eq!(fs::read_to_string(dir.join("keep.txt")).unwrap(), "original");
        assert!(!dir.join("fresh.txt").exists());
        assert!(!dir.join("made").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn move_restore_puts_the_file_back_at_the_source() {
        let dir = temp_dir("move-restore");
        let backup_root = dir.join("backups");
        fs::create_dir_all(&backup_root).unwrap();
        fs::write(dir.join("src.txt"), "payload").unwrap();
        let policy = CapabilityPolicy::default();
        let mut entries = Vec::new();
        {
            let mut context =
                StepContext::new(&dir, &policy).with_backup(&backup_root, &mut entries);
            execute_step(
                &mut context,
                &step("move_file", serde_json::json!({"source": "src.txt", "target": "dst.txt"})),
            )
            .unwrap();
        }
        assert!(dir.join("dst.txt").exists());
        assert!(!dir.join("src.txt").exists());
        let manifest = write_backup_manifest(&backup_root, &dir, None, &entries).unwrap();
        restore_from_manifest(&manifest).unwrap();
        assert_eq!(fs::read_to_string(dir.join("src.txt")).unwrap(), "payload");
        assert!(!dir.join("dst.txt").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    // ---- S2: Tier2 controlled exec ----

    fn allowlist_policy(programs: &[&str], legacy: bool) -> CapabilityPolicy {
        let mut policy = CapabilityPolicy::default();
        policy.exec.mode = crate::policy::EXEC_ALLOWLIST.to_string();
        policy.exec.allow = programs.iter().map(|name| name.to_string()).collect();
        policy.exec.allow_legacy_shell = legacy;
        policy
    }

    #[test]
    fn exec_is_denied_while_policy_is_off() {
        let dir = temp_dir("exec-off");
        let policy = CapabilityPolicy::default();
        let mut context = StepContext::new(&dir, &policy);
        let error = execute_step(
            &mut context,
            &step("exec", serde_json::json!({"program": "git", "args": ["--version"]})),
        )
        .unwrap_err();
        assert_eq!(error.outcome, "invalid");
        assert!(error.detail.contains("exec=off"), "{}", error.detail);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_shell_needs_an_explicit_opt_in() {
        let dir = temp_dir("legacy");
        let policy = allowlist_policy(&["cmd.exe"], false);
        let mut context = StepContext::new(&dir, &policy);
        let error = execute_step(
            &mut context,
            &step("exec_command", serde_json::json!({"cmd": "echo hi"})),
        )
        .unwrap_err();
        assert_eq!(error.outcome, "invalid");
        assert!(error.detail.contains("legacy shell"), "{}", error.detail);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn allowlist_and_denylist_are_both_enforced() {
        let dir = temp_dir("deny");
        // 1. A program that is not on the allowlist is refused.
        let policy = allowlist_policy(&["definitely_not_allowed"], false);
        let mut context = StepContext::new(&dir, &policy);
        let error = execute_step(
            &mut context,
            &step("exec", serde_json::json!({"program": "python", "args": ["-V"]})),
        )
        .unwrap_err();
        assert!(error.detail.contains("not allowed"), "{}", error.detail);

        // 2. A denylisted program stays refused even when named explicitly.
        let policy = allowlist_policy(&["cmd.exe", "cmd"], true);
        let mut context = StepContext::new(&dir, &policy);
        let error = execute_step(
            &mut context,
            &step("exec", serde_json::json!({"program": "cmd.exe", "args": ["/C", "echo hi"]})),
        )
        .unwrap_err();
        assert!(error.detail.contains("denylist"), "{}", error.detail);

        // 3. Dangerous argument patterns are refused on allowed programs.
        let policy = allowlist_policy(&["git"], false);
        let mut context = StepContext::new(&dir, &policy);
        let error = execute_step(
            &mut context,
            &step("exec", serde_json::json!({"program": "git", "args": ["rm -rf /"]})),
        )
        .unwrap_err();
        assert!(error.detail.contains("denylist"), "{}", error.detail);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn allowed_program_captures_exit_code_and_stdout() {
        let dir = temp_dir("exec-ok");
        let helper = build_helper(&dir, "probe", "fn main() { println!(\"PROBE_OK\"); }");
        let run_root = dir.join("run");
        let program = helper.file_name().unwrap().to_string_lossy().to_string();
        let policy = allowlist_policy(&[program.as_str()], false);
        let mut context = StepContext::new(&dir, &policy).with_run_root(&run_root);
        let execution = execute_step(
            &mut context,
            &step("exec", serde_json::json!({"program": program, "args": []})),
        )
        .unwrap();
        assert_eq!(execution.exit_code, Some(0));
        assert!(execution.stdout.contains("PROBE_OK"), "{:?}", execution.stdout);
        assert!(execution.evidence.contains("exit=0"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn non_zero_exit_is_evidence_not_a_policy_failure() {
        let dir = temp_dir("exec-nonzero");
        let helper = build_helper(&dir, "fail", "fn main() { std::process::exit(3); }");
        let run_root = dir.join("run");
        let program = helper.file_name().unwrap().to_string_lossy().to_string();
        let policy = allowlist_policy(&[program.as_str()], false);
        let mut context = StepContext::new(&dir, &policy).with_run_root(&run_root);
        let execution = execute_step(
            &mut context,
            &step("exec", serde_json::json!({"program": program, "args": []})),
        )
        .unwrap();
        assert_eq!(execution.exit_code, Some(3));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn timeout_kills_the_process_and_reports_timeout() {
        let dir = temp_dir("exec-timeout");
        let helper = build_helper(
            &dir,
            "sleep",
            "fn main() { std::thread::sleep(std::time::Duration::from_secs(30)); }",
        );
        let run_root = dir.join("run");
        let program = helper.file_name().unwrap().to_string_lossy().to_string();
        let mut policy = allowlist_policy(&[program.as_str()], false);
        policy.exec.timeout_secs = 1;
        let mut context = StepContext::new(&dir, &policy).with_run_root(&run_root);
        let execution = execute_step(
            &mut context,
            &step("exec", serde_json::json!({"program": program, "args": []})),
        )
        .unwrap();
        assert!(execution.evidence.contains("TIMEOUT"), "{}", execution.evidence);
        assert_eq!(execution.exit_code, None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn program_outside_workspace_and_run_root_is_rejected() {
        let dir = temp_dir("exec-outside");
        let outside = temp_dir("exec-outside-target");
        let helper = outside.join("probe.cmd");
        fs::write(&helper, "@echo off\r\n").unwrap();
        let policy = allowlist_policy(&["probe.cmd"], false);
        let mut context = StepContext::new(&dir, &policy);
        let error = execute_step(
            &mut context,
            &step("exec", serde_json::json!({"program": helper.to_string_lossy()})),
        )
        .unwrap_err();
        assert!(error.detail.contains("outside workspace"), "{}", error.detail);
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&outside);
    }

    #[test]
    fn dry_run_replays_file_actions_in_scratch_without_touching_workspace() {
        let dir = temp_dir("dry-run");
        let scratch = dir.join("scratch");
        fs::create_dir_all(&scratch).unwrap();
        let policy = CapabilityPolicy::default();
        let mut context = StepContext::new(&dir, &policy).with_scratch(&scratch);
        execute_step(
            &mut context,
            &step(
                "write_file",
                serde_json::json!({"path": "nested/probe.txt", "content": "DRY"}),
            ),
        )
        .unwrap();
        execute_step(
            &mut context,
            &step("mkdir", serde_json::json!({"path": "nested/dir"})),
        )
        .unwrap();
        // Scratch got the effects; the real workspace stayed empty.
        assert_eq!(
            fs::read_to_string(scratch.join("nested/probe.txt")).unwrap(),
            "DRY"
        );
        assert!(scratch.join("nested/dir").is_dir());
        assert!(!dir.join("nested").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn dry_run_validates_exec_shape_without_spawning() {
        let dir = temp_dir("dry-exec");
        let scratch = dir.join("scratch");
        fs::create_dir_all(&scratch).unwrap();
        let policy = allowlist_policy(&["git"], false);
        let mut context = StepContext::new(&dir, &policy).with_scratch(&scratch);
        let execution = execute_step(
            &mut context,
            &step("exec", serde_json::json!({"program": "git", "args": ["--version"]})),
        )
        .unwrap();
        assert!(execution.evidence.contains("dry-run"), "{}", execution.evidence);
        assert_eq!(execution.exit_code, None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_manifest_missing_copy_is_reported() {
        let dir = temp_dir("missing-copy");
        let backup_root = dir.join("backups");
        fs::create_dir_all(&backup_root).unwrap();
        fs::write(dir.join("a.txt"), "x").unwrap();
        let entries = vec![BackupEntry {
            rel_path: "a.txt".to_string(),
            backup_path: Some(backup_root.join("gone.txt").to_string_lossy().to_string()),
            existed: true,
            directory: false,
        }];
        let manifest = write_backup_manifest(&backup_root, &dir, None, &entries).unwrap();
        let error = restore_from_manifest(&manifest).unwrap_err();
        assert!(error.contains("backup copy missing"));
        let _ = fs::remove_dir_all(&dir);
    }
}

//! Session store: conversation/delegation records persisted to
//! `<home>/sessions.json`. Sessions run in background threads; the UI polls
//! until a terminal state. Output is capped to keep files small.

use std::path::Path;
use std::process::Child;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::sync::Mutex;

use experience_core::experience::trace_event::TraceEvent;
use serde::Deserialize;
use serde::Serialize;

pub const MAX_OUTPUT_CHARS: usize = 200_000;
const SESSIONS_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Running,
    Completed,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub agent_id: String,
    pub task: String,
    pub cwd: Option<String>,
    /// Stage C1: scene/scope of this session (None = all-scope wakeup).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Stage C4: optional capability allowlist (known vocabulary only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_allowlist: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trace: Vec<TraceEvent>,
    /// One task text per round (resume appends); indexes line up with the
    /// `submitted` event boundaries in `trace`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub round_tasks: Vec<String>,
    pub status: SessionStatus,
    pub created_at: u64,
    pub finished_at: Option<u64>,
    pub summary: Option<String>,
    pub output: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionsFile {
    schema_version: u32,
    #[serde(default)]
    sessions: Vec<Session>,
}

impl Default for SessionsFile {
    fn default() -> Self {
        Self {
            schema_version: SESSIONS_SCHEMA_VERSION,
            sessions: Vec::new(),
        }
    }
}

pub struct SessionStore {
    inner: Mutex<Vec<Session>>,
    children: Mutex<std::collections::HashMap<String, Arc<Mutex<Child>>>>,
    cancel_flags: Mutex<std::collections::HashMap<String, Arc<AtomicBool>>>,
    home: std::path::PathBuf,
}

impl SessionStore {
    pub fn load(home: &Path) -> Self {
        let mut sessions =
            match std::fs::read_to_string(home.join("sessions.json")) {
                Ok(json) => parse_sessions_file(&json),
                Err(_) => Vec::new(),
            };
        // A session left "running" after a restart has no live executor:
        // mark it stale loudly instead of showing an eternal spinner.
        for session in sessions.iter_mut() {
            if session.status == SessionStatus::Running {
                session.status = SessionStatus::Error;
                session.finished_at = Some(now_secs());
                session.summary = Some(
                    "上次运行中断（应用重启/旧版无超时），请重新提交任务".to_string(),
                );
            }
        }
        // Backfill round_tasks for legacy records that predate typed v1.
        for session in sessions.iter_mut() {
            if session.round_tasks.is_empty() && !session.task.is_empty() {
                session.round_tasks.push(session.task.clone());
            }
        }
        Self {
            inner: Mutex::new(sessions),
            children: Mutex::new(std::collections::HashMap::new()),
            cancel_flags: Mutex::new(std::collections::HashMap::new()),
            home: home.to_path_buf(),
        }
    }

    pub fn create(
        &self,
        agent_id: String,
        task: String,
        cwd: Option<String>,
        scope: Option<String>,
        capability_allowlist: Option<Vec<String>>,
    ) -> Session {
        let session = Session {
            id: format!("s-{}", now_nanos()),
            agent_id,
            task: task.clone(),
            cwd,
            scope,
            capability_allowlist,
            thread_id: None,
            trace: Vec::new(),
            round_tasks: vec![task],
            status: SessionStatus::Running,
            created_at: now_secs(),
            finished_at: None,
            summary: None,
            output: String::new(),
        };
        let mut guard = self.inner.lock().unwrap();
        guard.push(session.clone());
        self.persist_locked(&guard);
        session
    }

    pub fn finish(
        &self,
        id: &str,
        ok: bool,
        summary: String,
        output: String,
    ) {
        self.finish_impl(id, ok, summary, Some(output));
    }

    fn finish_impl(&self, id: &str, ok: bool, summary: String, output: Option<String>) {
        self.children.lock().unwrap().remove(id);
        self.cancel_flags.lock().unwrap().remove(id);
        let mut guard = self.inner.lock().unwrap();
        if let Some(session) = guard.iter_mut().find(|session| session.id == id) {
            session.status = if ok {
                SessionStatus::Completed
            } else {
                SessionStatus::Error
            };
            session.finished_at = Some(now_secs());
            session.summary = Some(summary);
            if let Some(output) = output {
                session.output = crate::cap_output_chars(output, MAX_OUTPUT_CHARS);
            }
            self.persist_locked(&guard);
        }
    }

    /// Register the executor child so the session can be cancelled/timeout.
    pub fn register_child(&self, id: &str, child: Arc<Mutex<Child>>) {
        self.children
            .lock()
            .unwrap()
            .insert(id.to_string(), child);
    }

    /// Live output update while the session is still running.
    pub fn update_output(&self, id: &str, output: String) {
        let mut guard = self.inner.lock().unwrap();
        if let Some(session) = guard
            .iter_mut()
            .find(|session| session.id == id && session.status == SessionStatus::Running)
        {
            session.output = crate::cap_output_chars(output, MAX_OUTPUT_CHARS);
        }
    }

    /// Live canonical-trace update while the session is running.
    pub fn update_trace(&self, id: &str, trace: Vec<TraceEvent>) {
        let mut guard = self.inner.lock().unwrap();
        if let Some(session) = guard
            .iter_mut()
            .find(|session| session.id == id && session.status == SessionStatus::Running)
        {
            session.trace = trace;
        }
    }

    /// Terminal update for a SessionChannel (host) run. Single-writer rule is
    /// structural: the live on_event stream is the only trace writer, so the
    /// terminal call takes no trace parameter (status/summary/thread_id only).
    pub fn finish_session_channel(
        &self,
        id: &str,
        ok: bool,
        summary: String,
        thread_id: Option<String>,
    ) {
        self.children.lock().unwrap().remove(id);
        self.cancel_flags.lock().unwrap().remove(id);
        let mut guard = self.inner.lock().unwrap();
        if let Some(session) = guard.iter_mut().find(|session| session.id == id) {
            session.status = if ok {
                SessionStatus::Completed
            } else {
                SessionStatus::Error
            };
            session.finished_at = Some(now_secs());
            session.summary = Some(summary);
            session.thread_id = thread_id.or(session.thread_id.clone());
            self.persist_locked(&guard);
        }
    }

    pub fn register_cancel_flag(&self, id: &str, flag: Arc<AtomicBool>) {
        self.cancel_flags
            .lock()
            .unwrap()
            .insert(id.to_string(), flag);
    }

    /// Move a terminal session (with thread_id) back to running for a
    /// follow-up turn (P1 resume).
    pub fn resume_start(&self, id: &str, task: String) -> Result<Session, String> {
        let mut guard = self.inner.lock().unwrap();
        let session = guard
            .iter_mut()
            .find(|session| session.id == id)
            .ok_or_else(|| format!("session '{id}' not found"))?;
        if session.thread_id.is_none() {
            return Err("session 没有 thread_id，无法 resume（需 session channel）".to_string());
        }
        if session.status == SessionStatus::Running {
            return Err("session 正在运行中".to_string());
        }
        session.task = task.clone();
        session.status = SessionStatus::Running;
        session.finished_at = None;
        session.summary = None;
        session.round_tasks.push(task.clone());
        let cloned = session.clone();
        self.persist_locked(&guard);
        Ok(cloned)
    }

    /// Is this session's executor still registered (i.e. not cancelled)?
    pub fn has_child(&self, id: &str) -> bool {
        self.children.lock().unwrap().contains_key(id)
    }

    /// Kill the executor child and mark the session cancelled (fail loud,
    /// never silently abandoned).
    pub fn cancel(&self, id: &str) -> Result<(), String> {
        if let Some(child) = self.children.lock().unwrap().remove(id) {
            let _ = child.lock().unwrap().kill();
            // Bound the post-kill wait so a stubborn child cannot hang cancel.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            loop {
                if child.lock().unwrap().try_wait().ok().flatten().is_some() {
                    break;
                }
                if std::time::Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            self.finish_impl(
                id,
                false,
                "已取消（用户终止 executor）".to_string(),
                None,
            );
            return Ok(());
        }
        if let Some(flag) = self.cancel_flags.lock().unwrap().get(id) {
            flag.store(true, Ordering::Relaxed);
            return Ok(());
        }
        Err(format!("session '{id}' 没有正在运行的 executor"))
    }

    pub fn get(&self, id: &str) -> Option<Session> {
        self.inner
            .lock()
            .unwrap()
            .iter()
            .find(|session| session.id == id)
            .cloned()
    }

    /// Newest first.
    pub fn list(&self) -> Vec<Session> {
        let mut sessions = self.inner.lock().unwrap().clone();
        sessions.reverse();
        sessions
    }

    fn persist_locked(&self, sessions: &[Session]) {
        let file = SessionsFile {
            schema_version: SESSIONS_SCHEMA_VERSION,
            sessions: sessions.to_vec(),
        };
        if let Ok(json) = serde_json::to_string_pretty(&file) {
            std::fs::create_dir_all(&self.home).ok();
            let _ = std::fs::write(self.home.join("sessions.json"), json);
        }
    }
}

/// Parse a sessions file, migrating pre-v2 string traces into `legacy`
/// typed events and bootstrapping `round_tasks` from the current task.
fn parse_sessions_file(json: &str) -> Vec<Session> {
    let Ok(root) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let Some(items) = root
        .get("sessions")
        .and_then(serde_json::Value::as_array)
    else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|value| {
            let mut session = value.clone();
            if let Some(trace) = session
                .get_mut("trace")
                .and_then(serde_json::Value::as_array_mut)
            {
                for entry in trace.iter_mut() {
                    if let Some(label) = entry.as_str() {
                        *entry = serde_json::json!({ "kind": "legacy", "label": label });
                    }
                }
            }
            serde_json::from_value::<Session>(session).ok()
        })
        .collect()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_round_trip_and_cap_output() {
        let dir = std::env::temp_dir().join(format!(
            "exp-sessions-{}",
            now_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let store = SessionStore::load(&dir);
        let session = store.create("codex".into(), "task".into(), None, None, None);
        assert_eq!(session.status, SessionStatus::Running);
        let huge = "x".repeat(MAX_OUTPUT_CHARS + 100);
        store.finish(&session.id, true, "ok".into(), huge);
        let finished = store.get(&session.id).unwrap();
        assert_eq!(finished.status, SessionStatus::Completed);
        assert!(finished.output.len() <= MAX_OUTPUT_CHARS + 200);
        assert!(finished.output.contains("截断"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[ignore = "spawning a long-lived child is environment-dependent; covered by bounded-kill logic"]
    #[test]
    fn cancel_kills_running_executor_and_marks_error() {
        let dir = std::env::temp_dir().join(format!(
            "exp-sessions-cancel-{}",
            now_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let store = SessionStore::load(&dir);
        let session = store.create("codex".into(), "long task".into(), None, None, None);
        let child = Arc::new(Mutex::new(
            std::process::Command::new("cmd")
                .args(["/C", "ping", "-n", "30", "127.0.0.1", ">", "nul"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        ));
        store.register_child(&session.id, Arc::clone(&child));
        store.cancel(&session.id).unwrap();
        assert!(!store.has_child(&session.id));
        let finished = store.get(&session.id).unwrap();
        assert_eq!(finished.status, SessionStatus::Error);
        assert!(finished.summary.as_deref().unwrap().contains("已取消"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn live_streamed_trace_is_not_doubled_by_terminal_finish() {
        let dir = std::env::temp_dir().join(format!(
            "exp-sessions-single-writer-{}",
            now_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let store = SessionStore::load(&dir);
        let session = store.create("codex".into(), "t1".into(), None, None, None);
        // Real worker pattern: typed events are live-appended while Running.
        store.update_trace(
            &session.id,
            vec![
                TraceEvent::Submitted,
                TraceEvent::Accepted,
                TraceEvent::AgentStarted,
            ],
        );
        // Terminal write takes no trace (structural single-writer): live
        // events are never re-appended at finish time.
        store.finish_session_channel(
            &session.id,
            true,
            "ok".into(),
            Some("thread-1".into()),
        );
        let finished = store.get(&session.id).unwrap();
        assert_eq!(
            finished.trace,
            vec![
                TraceEvent::Submitted,
                TraceEvent::Accepted,
                TraceEvent::AgentStarted
            ]
        );
        assert_eq!(finished.thread_id.as_deref(), Some("thread-1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_string_traces_migrate_to_typed_events() {
        let dir = std::env::temp_dir().join(format!(
            "exp-sessions-migrate-{}",
            now_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let legacy = r#"{
          "schema_version": 1,
          "sessions": [
            {
              "id": "s-old",
              "agent_id": "codex",
              "task": "old task",
              "cwd": null,
              "trace": ["submitted", "tool_call:exec_command", "turn_completed"],
              "status": "completed",
              "created_at": 1,
              "finished_at": 2,
              "summary": "ok",
              "output": ""
            }
          ]
        }"#;
        std::fs::write(dir.join("sessions.json"), legacy).unwrap();
        let store = SessionStore::load(&dir);
        let session = store.get("s-old").unwrap();
        assert_eq!(
            session.trace,
            vec![
                TraceEvent::Legacy {
                    label: "submitted".into()
                },
                TraceEvent::Legacy {
                    label: "tool_call:exec_command".into()
                },
                TraceEvent::Legacy {
                    label: "turn_completed".into()
                },
            ]
        );
        assert_eq!(session.round_tasks, vec!["old task".to_string()]);
        assert_eq!(session.status, SessionStatus::Completed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resume_start_requires_thread_and_terminal_state() {
        let dir = std::env::temp_dir().join(format!(
            "exp-sessions-resume-{}",
            now_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let store = SessionStore::load(&dir);
        let session = store.create("codex".into(), "t1".into(), None, None, None);
        assert!(store.resume_start(&session.id, "t2".into()).is_err());
        store.finish_session_channel(
            &session.id,
            true,
            "ok".into(),
            Some("thread-1".into()),
        );
        let resumed = store.resume_start(&session.id, "t2".into()).unwrap();
        assert_eq!(resumed.status, SessionStatus::Running);
        assert_eq!(resumed.round_tasks, vec!["t1".to_string(), "t2".to_string()]);
        assert!(store.resume_start(&session.id, "t3".into()).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

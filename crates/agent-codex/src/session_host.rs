//! SessionChannel: drive `codex app-server --stdio` as a real Session Host.
//!
//! Lifecycle (one shared kernel): spawn -> initialize -> (thread/start |
//! thread/resume) -> queue/add -> queue/start -> event read -> completion
//! (real workspace side effect via caller predicate, or first
//! turn/completed + short quiet drain) -> graceful teardown. Single active
//! thread per host; protocol replies have bounded waits (30s); task
//! execution has NO wall clock (cancellation is explicit). Thread reuse
//! across host restarts is supported through `resume_thread`; concurrency is
//! out of scope.

use std::io::BufRead;
use std::io::BufReader;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::Child;
use std::process::Command;
use std::process::ExitStatus;
use std::process::Stdio;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::channel;
use std::time::Duration;

use serde_json::json;
use serde_json::Value;

use experience_core::experience::trace_event::TraceEvent;

const PROTOCOL_TIMEOUT: Duration = Duration::from_secs(30);
const TURN_QUIET_WINDOW: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
pub struct SessionHostConfig {
    pub exe: PathBuf,
    pub codex_home: Option<PathBuf>,
    /// Extra env vars (e.g. DEEPSEEK_API_KEY) inherited from the operator.
    pub extra_env: Vec<(String, String)>,
}

impl SessionHostConfig {
    pub fn new(exe: impl Into<PathBuf>) -> Self {
        Self {
            exe: exe.into(),
            codex_home: None,
            extra_env: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionOutcome {
    pub ok: bool,
    pub thread_id: Option<String>,
    /// Typed trace v1 events for this run (single-writer live stream).
    pub events: Vec<TraceEvent>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SessionTask {
    pub workspace: PathBuf,
    pub task: String,
}

/// Run one real agent session; `verify` must observe a real workspace side
/// effect (file/nonce etc.). No task wall clock: if verify never passes the
/// session runs until `cancel` is set or the host exits.
pub fn run_session_task(
    config: &SessionHostConfig,
    task: &SessionTask,
    cancel: &AtomicBool,
    verify: impl Fn(&Path) -> bool,
) -> Result<SessionOutcome, String> {
    run_session_driver(
        config,
        task,
        cancel,
        ThreadMode::New,
        Completion::Verify(Box::new(verify)),
        &|_event| {},
    )
}

/// SessionChannel run for generic Experience tasks: completion = first
/// `turn/completed` event + short quiet drain. Typed events are streamed
/// live through `on_event`.
pub fn run_session_task_until_turn(
    config: &SessionHostConfig,
    task: &SessionTask,
    cancel: &AtomicBool,
    on_event: impl Fn(&TraceEvent),
) -> Result<SessionOutcome, String> {
    run_session_driver(
        config,
        task,
        cancel,
        ThreadMode::New,
        Completion::TurnCompleted,
        &on_event,
    )
}

/// Resume an existing thread (P1): reuse `thread_id` on a fresh host, queue
/// the follow-up and wait for the next turn/completed.
pub fn resume_thread(
    config: &SessionHostConfig,
    thread_id: &str,
    task: &SessionTask,
    cancel: &AtomicBool,
    on_event: impl Fn(&TraceEvent),
) -> Result<SessionOutcome, String> {
    run_session_driver(
        config,
        task,
        cancel,
        ThreadMode::Resume { thread_id },
        Completion::TurnCompleted,
        &on_event,
    )
}

/// How a run attaches to a thread on the host.
#[derive(Clone, Copy)]
enum ThreadMode<'a> {
    /// Brand-new thread via thread/start; every later failure must keep the
    /// created thread_id so callers can backfill it into their records.
    New,
    /// Existing thread via thread/resume; the caller already owns thread_id.
    Resume { thread_id: &'a str },
}

/// What ends the event loop.
enum Completion<'a> {
    /// Poll the real workspace until the predicate holds.
    Verify(Box<dyn Fn(&Path) -> bool + 'a>),
    /// First turn/completed milestone, then a short quiet drain.
    TurnCompleted,
}

/// One shared Session Host kernel: spawn -> initialize -> attach (start or
/// resume) -> queue/add -> queue/start -> event loop -> graceful teardown.
/// The public entry points differ only in attachment mode, completion mode
/// and live event streaming.
fn run_session_driver(
    config: &SessionHostConfig,
    task: &SessionTask,
    cancel: &AtomicBool,
    mode: ThreadMode<'_>,
    completion: Completion<'_>,
    on_event: &dyn Fn(&TraceEvent),
) -> Result<SessionOutcome, String> {
    let mut host = HostProcess::spawn(config)?;
    let mut events: Vec<TraceEvent> = Vec::new();

    // initialize (pre-thread: every failure stops the host, then Err)
    send_handshake(
        &mut host,
        json!({
            "method": "initialize", "id": 1,
            "params": {
                "clientInfo": {"name": "experience-session-host", "version": "0.1.0"},
                "capabilities": {"experimentalApi": true}
            }
        }),
    )?;
    let init = reply_handshake(&mut host, 1)?;
    if init.get("error").is_some() {
        return protocol_fail(&mut host, format!("initialize error: {}", init["error"]));
    }
    send_handshake(&mut host, json!({"method": "initialized"}))?;

    // thread/start (new) or thread/resume (existing)
    let thread_id = match mode {
        ThreadMode::New => {
            send_handshake(
                &mut host,
                json!({
                    "method": "thread/start", "id": 2,
                    "params": {"cwd": task.workspace.to_string_lossy(), "threadSource": "experience-session-host"}
                }),
            )?;
            let started = reply_handshake(&mut host, 2)?;
            if let Some(error) = started.get("error") {
                return protocol_fail(&mut host, format!("thread/start error: {error}"));
            }
            match find_thread_id(started.get("result")) {
                Some(thread_id) => thread_id,
                None => {
                    return protocol_fail(
                        &mut host,
                        "threadId missing in thread/start reply".to_string(),
                    );
                }
            }
        }
        ThreadMode::Resume { thread_id } => {
            send_handshake(
                &mut host,
                json!({
                    "method": "thread/resume", "id": 2,
                    "params": {"threadId": thread_id}
                }),
            )?;
            let resumed = reply_handshake(&mut host, 2)?;
            if let Some(error) = resumed.get("error") {
                return protocol_fail(&mut host, format!("thread/resume error: {error}"));
            }
            thread_id.to_string()
        }
    };

    // Once a thread exists: a fresh thread/start run must surface later
    // failures as outcomes that keep thread_id (caller backfill); a resume
    // run fails as a driver error because the caller already owns thread_id.
    let fail = |host: &mut HostProcess,
                thread_id: String,
                mut events: Vec<TraceEvent>,
                phase: &str,
                message: String|
     -> Result<SessionOutcome, String> {
        events.push(TraceEvent::Failed {
            phase: phase.to_string(),
            message: message.clone(),
        });
        match mode {
            ThreadMode::New => fail_after_thread_start(host, thread_id, events, message),
            ThreadMode::Resume { .. } => {
                let _ = host.stop();
                Err(message)
            }
        }
    };

    // queue/add
    if let Err(error) = host.send(json!({
        "method": "thread/queue/add", "id": 3,
        "params": {
            "threadId": thread_id,
            "clientUserMessageId": format!("exp-{thread_id}-{}", nanos()),
            "input": [{"type": "text", "text": task.task}]
        }
    })) {
        return fail(&mut host, thread_id.clone(), events, "queue/add", error);
    }
    let queued = match host.next_reply(3, PROTOCOL_TIMEOUT) {
        Ok(reply) => reply,
        Err(error) => return fail(&mut host, thread_id.clone(), events, "queue/add", error),
    };
    if let Some(error) = queued.get("error") {
        return fail(
            &mut host,
            thread_id.clone(),
            events,
            "queue/add",
            format!("queue/add error: {error}"),
        );
    }
    push_event(&mut events, TraceEvent::Submitted, on_event);

    // queue/start. The host may auto-dispatch the queued turn right after
    // queue/add (idle-lifecycle wake, verified against the fork app-server on
    // 2026-09-08); queue/start then races it and reports "queue is empty" or
    // "active or pending turn". Both mean the turn is already starting, so we
    // wait for the event stream instead of failing. Any other reply error is
    // a real failure.
    if let Err(error) = host.send(json!({
        "method": "thread/queue/start", "id": 4,
        "params": {"threadId": thread_id}
    })) {
        return fail(&mut host, thread_id.clone(), events, "queue/start", error);
    }
    let started_turn = match host.next_reply(4, PROTOCOL_TIMEOUT) {
        Ok(reply) => reply,
        Err(error) => return fail(&mut host, thread_id.clone(), events, "queue/start", error),
    };
    if let Some(error) = started_turn.get("error") {
        let message = error.to_string();
        if !(message.contains("queue is empty") || message.contains("active or pending turn")) {
            return fail(
                &mut host,
                thread_id.clone(),
                events,
                "queue/start",
                format!("queue/start error: {error}"),
            );
        }
        push_event(&mut events, TraceEvent::AlreadyStarted, on_event);
    }
    push_event(&mut events, TraceEvent::Accepted, on_event);
    push_event(&mut events, TraceEvent::AgentStarted, on_event);

    // Event loop until the completion mode is satisfied, the run is
    // cancelled, or the host exits. No task wall clock.
    let mut saw_turn_completed = false;
    let mut quiet_since = std::time::Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return outcome_cancelled(&mut host, thread_id, events);
        }
        match &completion {
            Completion::Verify(verify) => {
                if verify(&task.workspace) {
                    return outcome_success(&mut host, thread_id, events);
                }
            }
            Completion::TurnCompleted => {
                if saw_turn_completed && quiet_since.elapsed() >= TURN_QUIET_WINDOW {
                    return outcome_success(&mut host, thread_id, events);
                }
            }
        }
        match host.try_wait() {
            Ok(Some(status)) => {
                return outcome_early_exit(&mut host, thread_id, events, status);
            }
            Ok(None) => {}
            Err(error) => return fail(&mut host, thread_id, events, "host", error),
        }
        match host.drain_event() {
            Some((method, payload)) => {
                if let Some(event) = classify_event(&method, &payload) {
                    if matches!(event, TraceEvent::TurnCompleted) {
                        saw_turn_completed = true;
                        quiet_since = std::time::Instant::now();
                    }
                    push_event(&mut events, event, on_event);
                }
            }
            None => std::thread::sleep(Duration::from_millis(300)),
        }
    }
}

fn push_event(trace: &mut Vec<TraceEvent>, event: TraceEvent, on_event: &dyn Fn(&TraceEvent)) {
    // Fold consecutive reasoning noise into a single marker (write-time).
    if event.is_noisy_repeat() && trace.last() == Some(&event) {
        return;
    }
    trace.push(event.clone());
    on_event(&event);
}

fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

/// Typed trace v1 mapping (whitelist): only semantic milestones become
/// events. Reasoning bursts are folded at write time; non-tool item
/// notifications (userMessage/agentMessage/fileChange) and transport noise
/// (thread/status, tokenUsage, rateLimits, turn/diff) are dropped here.
fn classify_event(method: &str, payload: &Value) -> Option<TraceEvent> {
    match method {
        "turn/completed" => Some(TraceEvent::TurnCompleted),
        "item/reasoning/textDelta" => Some(TraceEvent::Reasoning),
        "item/started" | "item/completed" => {
            let name = extract_tool_name(payload)?;
            let call_id = find_call_id(payload);
            if method == "item/completed" {
                Some(TraceEvent::ToolResult {
                    name,
                    call_id,
                })
            } else {
                Some(TraceEvent::ToolCall {
                    name,
                    call_id,
                    args_summary: extract_args_summary(payload),
                })
            }
        }
        _ => None,
    }
}

/// Best-effort, truncated argument summary for the L1 distill input.
/// Searches common argument-bearing keys; the value is serialized and capped
/// (lightweight principle: summaries stay small, raw payloads stay in audit).
fn extract_args_summary(value: &Value) -> Option<String> {
    fn find(value: &Value) -> Option<Value> {
        match value {
            Value::Object(map) => {
                for key in ["arguments", "args", "input", "cmd", "command"] {
                    if let Some(found) = map.get(key) {
                        return Some(found.clone());
                    }
                }
                map.values().find_map(find)
            }
            Value::Array(items) => items.iter().find_map(find),
            _ => None,
        }
    }
    let found = find(value)?;
    let text = found.as_str().map(str::to_string).unwrap_or_else(|| {
        serde_json::to_string(&found).unwrap_or_default()
    });
    const MAX_ARGS_SUMMARY_CHARS: usize = 240;
    if text.chars().count() > MAX_ARGS_SUMMARY_CHARS {
        let mut truncated: String = text.chars().take(MAX_ARGS_SUMMARY_CHARS).collect();
        truncated.push('…');
        Some(truncated)
    } else {
        Some(text)
    }
}

fn find_call_id(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => {
            for key in ["call_id", "callId", "id"] {
                if let Some(found) = map.get(key).and_then(Value::as_str) {
                    return Some(found.to_string());
                }
            }
            map.values().find_map(find_call_id)
        }
        Value::Array(items) => items.iter().find_map(find_call_id),
        _ => None,
    }
}

fn extract_tool_name(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => {
            let kind = map
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let has_call_id = map.contains_key("call_id") || map.contains_key("callId");
            if (kind.contains("function")
                || kind.contains("tool")
                || kind.contains("command")
                || kind.contains("shell")
                || has_call_id)
                && !kind.is_empty()
            {
                let name = map
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| kind.to_string());
                return Some(name);
            }
            for item in map.values() {
                if let Some(found) = extract_tool_name(item) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(items) => items.iter().find_map(extract_tool_name),
        _ => None,
    }
}

fn protocol_fail(host: &mut HostProcess, message: String) -> Result<SessionOutcome, String> {
    let _ = host.stop();
    Err(message)
}

/// Pre-thread protocol step: on failure stop the host first (no orphaned
/// app-server), then surface the driver error.
fn send_handshake(host: &mut HostProcess, value: Value) -> Result<(), String> {
    host.send(value).map_err(|error| {
        let _ = host.stop();
        error
    })
}

/// Pre-thread protocol step: on failure stop the host first (no orphaned
/// app-server), then surface the driver error.
fn reply_handshake(host: &mut HostProcess, expected_id: u64) -> Result<Value, String> {
    host.next_reply(expected_id, PROTOCOL_TIMEOUT).map_err(|error| {
        let _ = host.stop();
        error
    })
}

/// Terminal failure after `thread/start` succeeded: the thread exists on the
/// host even though the run failed early. Surface it as a failed outcome that
/// keeps `thread_id`, so callers backfill it into the session record instead
/// of losing track of the created thread.
fn fail_after_thread_start(
    host: &mut HostProcess,
    thread_id: String,
    events: Vec<TraceEvent>,
    message: String,
) -> Result<SessionOutcome, String> {
    let _ = host.stop();
    Ok(SessionOutcome {
        ok: false,
        thread_id: Some(thread_id),
        events,
        error: Some(message),
    })
}

fn outcome_success(
    host: &mut HostProcess,
    thread_id: String,
    events: Vec<TraceEvent>,
) -> Result<SessionOutcome, String> {
    let _ = host.stop();
    Ok(SessionOutcome {
        ok: true,
        thread_id: Some(thread_id),
        events,
        error: None,
    })
}

fn outcome_cancelled(
    host: &mut HostProcess,
    thread_id: String,
    mut events: Vec<TraceEvent>,
) -> Result<SessionOutcome, String> {
    let _ = host.stop();
    events.push(TraceEvent::Failed {
        phase: "cancelled".to_string(),
        message: "cancelled by operator".to_string(),
    });
    Ok(SessionOutcome {
        ok: false,
        thread_id: Some(thread_id),
        events,
        error: Some("cancelled by operator".into()),
    })
}

fn outcome_early_exit(
    host: &mut HostProcess,
    thread_id: String,
    mut events: Vec<TraceEvent>,
    status: ExitStatus,
) -> Result<SessionOutcome, String> {
    let _ = host.stop();
    events.push(TraceEvent::Failed {
        phase: "host".to_string(),
        message: format!("host exited early with {status}"),
    });
    Ok(SessionOutcome {
        ok: false,
        thread_id: Some(thread_id),
        events,
        error: Some(format!("host exited early with {status}")),
    })
}

fn find_thread_id(value: Option<&Value>) -> Option<String> {
    fn walk(value: &Value) -> Option<String> {
        match value {
            Value::Object(map) => {
                for (key, item) in map {
                    if (key == "threadId" || key == "thread_id" || key == "id")
                        && item.is_string()
                    {
                        return item.as_str().map(str::to_string);
                    }
                    if let Some(found) = walk(item) {
                        return Some(found);
                    }
                }
                None
            }
            Value::Array(items) => items.iter().find_map(walk),
            _ => None,
        }
    }
    value.and_then(walk)
}

struct HostProcess {
    child: Child,
    receiver: Receiver<Value>,
}

impl HostProcess {
    fn spawn(config: &SessionHostConfig) -> Result<Self, String> {
        let mut command = Command::new(&config.exe);
        command
            .arg("app-server")
            .arg("--stdio")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(home) = &config.codex_home {
            command.env("CODEX_HOME", home);
        }
        for (key, value) in &config.extra_env {
            command.env(key, value);
        }
        let mut child = command
            .spawn()
            .map_err(|error| format!("failed to spawn {}: {error}", config.exe.display()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "app-server stdout unavailable".to_string())?;
        let (sender, receiver) = channel();
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let Ok(value) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if sender.send(value).is_err() {
                    break;
                }
            }
        });
        Ok(Self { child, receiver })
    }

    fn send(&mut self, value: Value) -> Result<(), String> {
        let mut line = serde_json::to_string(&value).map_err(|e| e.to_string())?;
        line.push('\n');
        let stdin = self
            .child
            .stdin
            .as_mut()
            .ok_or_else(|| "app-server stdin unavailable".to_string())?;
        stdin
            .write_all(line.as_bytes())
            .map_err(|error| format!("write to app-server failed: {error}"))
    }

    fn next_reply(&mut self, expected_id: u64, timeout: Duration) -> Result<Value, String> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            match self.receiver.recv_timeout(remaining) {
                Ok(value) => {
                    if value.get("id").and_then(Value::as_u64) == Some(expected_id) {
                        return Ok(value);
                    }
                    // Notifications / other replies: ignore for P0-1.
                }
                Err(_) => {
                    return Err(format!(
                        "protocol timeout waiting for reply id={expected_id}"
                    ));
                }
            }
        }
    }

    fn drain_event(&mut self) -> Option<(String, Value)> {
        match self.receiver.try_recv() {
            Ok(value) => {
                let method = value
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                Some((method, value))
            }
            Err(_) => None,
        }
    }

    fn try_wait(&mut self) -> Result<Option<std::process::ExitStatus>, String> {
        self.child
            .try_wait()
            .map_err(|error| format!("host wait error: {error}"))
    }

    fn stop(&mut self) -> std::io::Result<()> {
        // Graceful: close stdin so the app-server flushes state/turn, then
        // wait briefly before killing (avoids stale active/pending turns).
        self.child.stdin = None;
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if self.child.try_wait()?.is_some() {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = self.child.kill();
        self.child.wait()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use experience_core::experience::trace_event::TraceEvent;

    #[test]
    fn classify_tool_call_captures_args_summary() {
        let payload = json!({
            "type": "functionCall",
            "name": "exec_command",
            "arguments": { "cmd": "mv a b", "cwd": "." }
        });
        let event = classify_event("item/started", &payload).expect("tool call classified");
        match event {
            TraceEvent::ToolCall {
                name,
                args_summary,
                ..
            } => {
                assert_eq!(name, "exec_command");
                let summary = args_summary.expect("args summary extracted");
                assert!(summary.contains("mv a b"), "summary={summary}");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn classify_drops_transport_noise() {
        assert_eq!(classify_event("thread/tokenUsage/updated", &json!({})), None);
        assert_eq!(
            classify_event("item/started", &json!({ "type": "userMessage" })),
            None
        );
    }
}

//! Test-only mock of `codex app-server --stdio`: speaks just enough JSON-RPC
//! to drive session_host failure paths deterministically (no LLM required).
//! Failure point is selected by EXP_MOCK_BEHAVIOR:
//!   queue_add_error       -> thread/queue/add replies with an error
//!   queue_start_error     -> thread/queue/start replies with an error
//!   auto_start            -> queue/start says "queue is empty" (the host
//!                            auto-dispatched the turn) then it completes
//!   thread_start_error    -> thread/start replies with an error
//!   resume_error          -> thread/resume replies with an error
//!   resume_queue_add_error-> thread/queue/add (resume) replies with an error
//!   active_turn           -> queue/start says the thread already has an
//!                            active/pending turn, then the turn completes

use std::io::BufRead;
use std::io::BufReader;
use std::io::Write;

use serde_json::json;

fn main() {
    let behavior = std::env::var("EXP_MOCK_BEHAVIOR").unwrap_or_default();
    let reader = BufReader::new(std::io::stdin().lock());
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let method = value
            .get("method")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        let id = value.get("id").and_then(serde_json::Value::as_u64);
        let mut notify_turn_completed = false;
        let reply = match (method.as_str(), behavior.as_str()) {
            ("initialize", _) => {
                Some(json!({"jsonrpc": "2.0", "id": id, "result": {"ok": true}}))
            }
            ("thread/start", "thread_start_error") => Some(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32602, "message": "mock thread/start rejected"}})),
            ("thread/start", _) => Some(json!({"jsonrpc": "2.0", "id": id, "result": {"thread": {"id": "mock-thread-1", "type": "thread"}}})),
            ("thread/resume", "resume_error") => Some(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32602, "message": "mock thread/resume rejected"}})),
            ("thread/resume", _) => Some(json!({"jsonrpc": "2.0", "id": id, "result": {"thread": {"id": "mock-thread-1"}}})),
            ("thread/queue/add", "queue_add_error") | ("thread/queue/add", "resume_queue_add_error") => {
                Some(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32602, "message": "mock queue/add rejected"}}))
            }
            ("thread/queue/add", _) => Some(json!({"jsonrpc": "2.0", "id": id, "result": {"queued_submission": {"id": "mock-q-1"}}})),
            ("thread/queue/start", "queue_start_error") => {
                Some(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32602, "message": "mock queue/start rejected"}}))
            }
            ("thread/queue/start", "auto_start") => {
                notify_turn_completed = true;
                Some(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32600, "message": "queue is empty"}}))
            }
            ("thread/queue/start", "active_turn") => {
                notify_turn_completed = true;
                Some(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32602, "message": "thread already has an active or pending turn (mock)"}}))
            }
            ("thread/queue/start", _) => Some(json!({"jsonrpc": "2.0", "id": id, "result": {"turn": {"id": "mock-turn-1", "status": "in_progress"}}})),
            _ => None,
        };
        let mut output = String::new();
        if let Some(reply) = reply {
            output.push_str(&serde_json::to_string(&reply).unwrap_or_default());
            output.push('\n');
        }
        if notify_turn_completed {
            output.push_str(r#"{"jsonrpc":"2.0","method":"turn/completed","params":{}}"#);
            output.push('\n');
        }
        if !output.is_empty() {
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            let _ = handle.write_all(output.as_bytes());
            let _ = handle.flush();
        }
    }
}

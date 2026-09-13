//! S4 acceptance through HTTP (no LLM): settings persist and survive restart,
//! environment variables override them, and the undo surface joins recent
//! experience executions with their backup snapshots.

use std::io::Read;
use std::io::Write;
use std::net::TcpStream;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;
use std::time::Duration;

use serde_json::json;
use serde_json::Value;

struct TestServer {
    base: String,
    child: std::process::Child,
    home: PathBuf,
}

impl TestServer {
    fn start(home: &Path, port: u16) -> Self {
        let exe = env!("CARGO_BIN_EXE_experience-server");
        let child = Command::new(exe)
            .arg("--home")
            .arg(home)
            .arg("--port")
            .arg(port.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn experience-server");
        let base = format!("http://127.0.0.1:{port}");
        for _ in 0..60 {
            if http("GET", &format!("{base}/api/health"), None).is_ok() {
                return Self {
                    base,
                    child,
                    home: home.to_path_buf(),
                };
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        panic!("server did not become ready");
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn http(method: &str, url: &str, body: Option<Value>) -> Result<Value, String> {
    let address = url
        .strip_prefix("http://")
        .ok_or_else(|| "url must start with http://".to_string())?;
    let (host_port, path) = match address.find('/') {
        Some(index) => (&address[..index], &address[index..]),
        None => (address, "/"),
    };
    let mut stream = TcpStream::connect(host_port).map_err(|error| error.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|error| error.to_string())?;
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host_port}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|error| error.to_string())?;
    let mut raw = String::new();
    stream
        .read_to_string(&mut raw)
        .map_err(|error| error.to_string())?;
    let (head, body) = raw
        .split_once("\r\n\r\n")
        .ok_or_else(|| "malformed response".to_string())?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| "no status line".to_string())?;
    if !(200..300).contains(&status) {
        return Err(format!("status {status}: {body}"));
    }
    if body.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(body).map_err(|error| format!("bad json: {error}: {body}"))
}

fn temp_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("exp-s4-{tag}-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn seed_agent(home: &Path, workspace: &Path) {
    let stub = home.join("stub.cmd");
    std::fs::write(&stub, "@echo off\r\nexit /b 0\r\n").unwrap();
    let agents = json!({
        "schema_version": 2,
        "agents": [{
            "id": "s4", "label": "s4", "kind": "codex_cli", "mode": "managed",
            "directory": home.to_string_lossy(), "executable": "stub.cmd",
            "channel": "session", "codex_home": home.to_string_lossy()
        }]
    });
    std::fs::write(
        home.join("agents.json"),
        serde_json::to_string_pretty(&agents).unwrap(),
    )
    .unwrap();
    let _ = workspace;
}

#[test]
fn settings_persist_and_env_overrides_file() {
    let dir = temp_dir("settings");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    seed_agent(&home, &dir);

    let server = TestServer::start(&home, 18994);
    let initial = http("GET", &format!("{}/api/settings", server.base), None).unwrap();
    assert_eq!(initial["settings"]["llm_compiler"], false);
    assert_eq!(initial["settings"]["injection_policy"], false);

    let updated = http(
        "PUT",
        &format!("{}/api/settings", server.base),
        Some(json!({
            "actor": "s4-test",
            "llm_compiler": true,
            "injection_policy": true,
            "undo_keep": 5
        })),
    )
    .unwrap();
    assert_eq!(updated["settings"]["llm_compiler"], true);
    assert_eq!(updated["effective"]["injection_policy"], true);
    assert!(home.join("settings.json").exists());

    // A fresh server on the same home must pick up the persisted settings.
    drop(server);
    let server2 = TestServer::start(&home, 18995);
    let reloaded = http("GET", &format!("{}/api/settings", server2.base), None).unwrap();
    assert_eq!(reloaded["settings"]["llm_compiler"], true);
    assert_eq!(reloaded["settings"]["injection_policy"], true);
    assert_eq!(reloaded["settings"]["undo_keep"], 5);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn undo_lists_recent_executions_with_snapshots() {
    let dir = temp_dir("undo");
    let home = dir.join("home");
    let workspace = dir.join("ws");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("existing.txt"), "base").unwrap();
    seed_agent(&home, &workspace);

    let experience = json!({
        "name": "s4_undo_probe",
        "trigger": {"tool": "exec_command", "command_pattern": "s4 undo probe"},
        "preconditions": [{"key": "cwd.exists", "expected": true}],
        "workflow": [
            {"action": "append_file", "args": {"path": "existing.txt", "content": "-appended"}}
        ],
        "postconditions": [{"key": "file:existing.txt.exists", "expected": true}],
        "verification": [],
        "failure_policy": "stop_and_report",
        "undo": "unsupported",
        "status": "active"
    });
    let store = json!({
        "schema_version": 1,
        "experiences": [experience],
        "pinned": [], "scopes": {}, "display_names": {}, "user_usage": {},
        "user_confidence": {}, "references": {}, "scope_policies": {}
    });
    std::fs::write(
        home.join("store.json"),
        serde_json::to_string_pretty(&store).unwrap(),
    )
    .unwrap();

    let server = TestServer::start(&home, 18996);
    let created = http(
        "POST",
        &format!("{}/api/sessions", server.base),
        Some(json!({"agent_id": "s4", "task": "s4 undo probe", "cwd": workspace})),
    )
    .unwrap();
    let session_id = created["id"].as_str().unwrap().to_string();
    assert_eq!(
        std::fs::read_to_string(workspace.join("existing.txt")).unwrap(),
        "base-appended"
    );

    let undo = http("GET", &format!("{}/api/undo?limit=10", server.base), None).unwrap();
    let runs = undo["runs"].as_array().unwrap();
    let run = runs
        .iter()
        .find(|run| run["candidate_name"] == "s4_undo_probe")
        .expect("execution listed in undo surface");
    assert_eq!(run["session_id"], session_id);
    let snapshots = run["snapshots"].as_array().unwrap();
    assert!(!snapshots.is_empty(), "backup snapshot attached");
    let snapshot = snapshots[0]["snapshot"].as_str().unwrap().to_string();

    let restored = http(
        "POST",
        &format!("{}/api/backups/{session_id}/restore", server.base),
        Some(json!({"snapshot": snapshot, "actor": "s4-test"})),
    )
    .unwrap();
    assert_eq!(restored["ok"], true);
    assert_eq!(
        std::fs::read_to_string(workspace.join("existing.txt")).unwrap(),
        "base"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

//! S2 acceptance through the real HTTP surface (no LLM): an allowlisted
//! program runs and produces exit-code/stdout evidence, the denylist refuses
//! a dangerous program, and the process verdict lands in the ledger.
//!
//! The fixture program is a real native executable built by rustc at test
//! time, so this exercises the actual Tier2 spawn path (no shell wrapper).

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

    fn home(&self) -> &Path {
        &self.home
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
    let dir = std::env::temp_dir().join(format!("exp-s2-{tag}-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Build a real native executable with rustc (used as an allowlisted program).
fn build_helper(dir: &Path, tag: &str, source: &str) -> PathBuf {
    let source_path = dir.join(format!("{tag}.rs"));
    std::fs::write(&source_path, source).unwrap();
    let exe = dir.join(format!("{tag}.exe"));
    let output = Command::new("rustc")
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

fn stub_executable(dir: &Path) -> PathBuf {
    let stub = dir.join("stub.cmd");
    std::fs::write(&stub, "@echo off\r\nexit /b 0\r\n").unwrap();
    stub
}

fn experience(name: &str, pattern: &str, step: Value, post: Value) -> Value {
    json!({
        "name": name,
        "trigger": {"tool": "exec_command", "command_pattern": pattern},
        "preconditions": [{"key": "cwd.exists", "expected": true}],
        "workflow": [step],
        "postconditions": [post],
        "verification": [],
        "failure_policy": "stop_and_report",
        "undo": "unsupported",
        "status": "active"
    })
}

#[test]
fn tier2_exec_runs_allowlisted_program_and_records_evidence() {
    let dir = temp_dir("allow");
    let home = dir.join("home");
    let workspace = dir.join("ws");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    let helper = build_helper(&dir, "probe", "fn main() { println!(\"PROBE_OK\"); }");
    let stub = stub_executable(&dir);
    // The L3 executor's run root is `<home>/run`: keep the fixture there.
    let run_root = home.join("run");
    std::fs::create_dir_all(&run_root).unwrap();
    let helper_in_run = run_root.join(helper.file_name().unwrap());
    std::fs::copy(&helper, &helper_in_run).unwrap();

    let allowed = helper_in_run
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let policy = json!({
        "__global__": {
            "fs_read": {"enabled": true, "workspace_only": true, "max_bytes": 20480},
            "fs_write": "workspace_only",
            "fs_delete": "deny",
            "exec": {"mode": "allowlist", "allow": [allowed], "timeout_secs": 30, "output_cap": 20000, "allow_legacy_shell": false},
            "network": {"mode": "off", "allow": []}
        }
    });
    let store = json!({
        "schema_version": 1,
        "experiences": [
            experience(
                "s2_exec_ok",
                "s2 exec ok",
                json!({"action": "exec", "args": {"program": allowed, "args": []}}),
                json!({"key": "process.exit_code:exec#0", "expected": 0})
            )
        ],
        "pinned": [], "scopes": {}, "display_names": {}, "user_usage": {},
        "user_confidence": {}, "references": {}, "scope_policies": policy
    });
    std::fs::write(
        home.join("store.json"),
        serde_json::to_string_pretty(&store).unwrap(),
    )
    .unwrap();
    let agents = json!({
        "schema_version": 2,
        "agents": [{
            "id": "s2", "label": "s2", "kind": "codex_cli", "mode": "managed",
            "directory": dir.to_string_lossy(), "executable": stub.file_name().unwrap().to_string_lossy(),
            "channel": "session", "codex_home": home.to_string_lossy()
        }]
    });
    std::fs::write(
        home.join("agents.json"),
        serde_json::to_string_pretty(&agents).unwrap(),
    )
    .unwrap();

    let server = TestServer::start(&home, 18990);
    let created = http(
        "POST",
        &format!("{}/api/sessions", server.base),
        Some(json!({"agent_id": "s2", "task": "s2 exec ok", "cwd": workspace})),
    )
    .expect("create session");
    let id = created["id"].as_str().unwrap().to_string();
    let ledger = std::fs::read_to_string(server.home().join("learning-l1.json")).unwrap_or_default();
    let record: Value = serde_json::from_str(&ledger).expect("ledger json");
    let records = record["records"].as_array().unwrap();
    let exec = records
        .iter()
        .find(|item| item["record_type"] == "experience_execution")
        .expect("experience_execution record");
    assert_eq!(exec["outcome"], "success", "{exec}");
    assert!(
        id.starts_with("s-"),
        "session id should look like a session: {id}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn tier2_denylist_refuses_dangerous_program_before_spawn() {
    let dir = temp_dir("deny");
    let home = dir.join("home");
    let workspace = dir.join("ws");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    let stub = stub_executable(&dir);

    // `cmd` may be allowlisted but the denylist still refuses to spawn it.
    let policy = json!({
        "__global__": {
            "fs_read": {"enabled": true, "workspace_only": true, "max_bytes": 20480},
            "fs_write": "workspace_only",
            "fs_delete": "deny",
            "exec": {"mode": "allowlist", "allow": ["cmd.exe", "cmd"], "timeout_secs": 30, "output_cap": 20000, "allow_legacy_shell": true},
            "network": {"mode": "off", "allow": []}
        }
    });
    let store = json!({
        "schema_version": 1,
        "experiences": [
            experience(
                "s2_exec_denied",
                "s2 exec denied",
                json!({"action": "exec", "args": {"program": "cmd.exe", "args": ["/C", "echo nope"]}}),
                json!({"key": "process.exit_code:exec#0", "expected": 0})
            )
        ],
        "pinned": [], "scopes": {}, "display_names": {}, "user_usage": {},
        "user_confidence": {}, "references": {}, "scope_policies": policy
    });
    std::fs::write(
        home.join("store.json"),
        serde_json::to_string_pretty(&store).unwrap(),
    )
    .unwrap();
    let agents = json!({
        "schema_version": 2,
        "agents": [{
            "id": "s2", "label": "s2", "kind": "codex_cli", "mode": "managed",
            "directory": dir.to_string_lossy(), "executable": stub.file_name().unwrap().to_string_lossy(),
            "channel": "session", "codex_home": home.to_string_lossy()
        }]
    });
    std::fs::write(
        home.join("agents.json"),
        serde_json::to_string_pretty(&agents).unwrap(),
    )
    .unwrap();

    let server = TestServer::start(&home, 18991);
    http(
        "POST",
        &format!("{}/api/sessions", server.base),
        Some(json!({"agent_id": "s2", "task": "s2 exec denied", "cwd": workspace})),
    )
    .expect("create session");
    let ledger = std::fs::read_to_string(server.home().join("learning-l1.json")).unwrap_or_default();
    let record: Value = serde_json::from_str(&ledger).expect("ledger json");
    let exec = record["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["record_type"] == "experience_execution")
        .expect("experience_execution record");
    assert_eq!(exec["outcome"], "invalid", "{exec}");
    let reason = exec["reason"].as_str().unwrap_or("");
    assert!(reason.contains("denylist"), "{reason}");
    let _ = std::fs::remove_dir_all(&dir);
}

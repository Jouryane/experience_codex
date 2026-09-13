//! State Source Registry (Stage S3).
//!
//! Every predicate the runtime accepts must be *produced by a registered
//! source*. A source declares how it observes, how fresh the observation is,
//! and whether the family is currently permitted. Keys with no source are
//! `Unknown` — and `Unknown` must never be treated as permission to act.
//!
//! Families:
//!
//! | family | keys | freshness |
//! |---|---|---|
//! | fs | `cwd.exists`, `file:<p>.exists/content/size/sha256`, `dir:<p>.exists` | read time |
//! | exec | `process.exit_code:<step_id>`, `process.stdout_contains:<step_id>` | run time (evidence map) |
//! | git | `git.dirty`, `git.branch`, `git.last_commit` | TTL 30s |
//! | http | `http.status:<url>`, `http.body_sha256:<url>` | TTL 15s |
//! | net | `port.open:<n>` | TTL 10s |

use std::net::TcpStream;
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use serde::Serialize;

/// One observation: value + provenance. `None` value means "no observation".
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProbeResult {
    pub value: Option<serde_json::Value>,
    /// Human-readable provenance (never contains secrets).
    pub source: String,
    pub observed_at: u64,
    pub ttl: Option<u64>,
}

impl ProbeResult {
    fn observed(value: serde_json::Value, source: impl Into<String>, ttl: Option<u64>) -> Self {
        Self {
            value: Some(value),
            source: source.into(),
            observed_at: now_secs(),
            ttl,
        }
    }

    fn unknown(source: impl Into<String>) -> Self {
        Self {
            value: None,
            source: source.into(),
            observed_at: now_secs(),
            ttl: None,
        }
    }
}

/// Capabilities the probe layer may use. Mirrors the execution policy family
/// names so a scope can grant "read state from the network" separately from
/// "run commands".
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProbePolicy {
    pub allow_network: bool,
    pub allow_git: bool,
    pub http_timeout_ms: u64,
    pub port_timeout_ms: u64,
}

impl Default for ProbePolicy {
    fn default() -> Self {
        Self {
            // File/exec evidence is always available; external observation is
            // opt-in (the caller wires the network policy here).
            allow_network: false,
            allow_git: true,
            http_timeout_ms: 2_000,
            port_timeout_ms: 500,
        }
    }
}

/// Predicate families the registry knows about.
pub const FAMILIES: &[&str] = &["fs", "exec", "git", "http", "net"];

/// True when `key` is produced by a registered source. Used by Gate 3/Gate 5:
/// a verdict that is not observable must not be claimed.
pub fn observable(key: &str) -> bool {
    family_of(key).is_some()
}

/// Registered source family for a key (None = unregistered → Unknown).
pub fn family_of(key: &str) -> Option<&'static str> {
    if key == "cwd.exists" || key.starts_with("file:") || key.starts_with("dir:") {
        return Some("fs");
    }
    if key.starts_with("process.") {
        return Some("exec");
    }
    if key.starts_with("git.") {
        return Some("git");
    }
    if key.starts_with("http.") {
        return Some("http");
    }
    if key.starts_with("port.") {
        return Some("net");
    }
    None
}

/// Probe one predicate key against `base_dir`.
pub fn probe(key: &str, base_dir: &Path, policy: &ProbePolicy) -> ProbeResult {
    if key == "cwd.exists" {
        return ProbeResult::observed(serde_json::json!(base_dir.is_dir()), "cwd", None);
    }
    if let Some(rest) = key.strip_prefix("file:") {
        return probe_file(rest, base_dir);
    }
    if let Some(rest) = key.strip_prefix("dir:") {
        if let Some(path) = rest.strip_suffix(".exists") {
            return ProbeResult::observed(
                serde_json::json!(base_dir.join(path).is_dir()),
                "fs.dir",
                None,
            );
        }
        return ProbeResult::unknown("fs.dir:unsupported-suffix");
    }
    if let Some(rest) = key.strip_prefix("git.") {
        return probe_git(rest, base_dir, policy);
    }
    if let Some(rest) = key.strip_prefix("http.") {
        return probe_http(rest, policy);
    }
    if let Some(rest) = key.strip_prefix("port.") {
        // Accept both `port.open:<n>` (documented form) and `port:<n>`.
        let port_part = rest.strip_prefix("open:").unwrap_or(rest);
        return probe_port(port_part, policy);
    }
    if let Some(rest) = key.strip_prefix("port:") {
        return probe_port(rest, policy);
    }
    ProbeResult::unknown(format!("unregistered:{key}"))
}

fn probe_file(rest: &str, base_dir: &Path) -> ProbeResult {
    if let Some(path) = rest.strip_suffix(".content") {
        let full = base_dir.join(path);
        return match std::fs::read_to_string(&full) {
            Ok(text) => ProbeResult::observed(serde_json::json!(text), "fs.read", None),
            Err(error) => {
                if full.exists() {
                    ProbeResult::unknown(format!("fs.read:error:{error}"))
                } else {
                    ProbeResult::observed(
                        serde_json::json!(false),
                        "fs.read:missing",
                        None,
                    )
                }
            }
        };
    }
    if let Some(path) = rest.strip_suffix(".exists") {
        return ProbeResult::observed(
            serde_json::json!(base_dir.join(path).exists()),
            "fs.stat",
            None,
        );
    }
    if let Some(path) = rest.strip_suffix(".size") {
        return match std::fs::metadata(base_dir.join(path)) {
            Ok(meta) => ProbeResult::observed(serde_json::json!(meta.len()), "fs.stat", None),
            Err(_) => ProbeResult::observed(serde_json::json!(false), "fs.stat:missing", None),
        };
    }
    if let Some(path) = rest.strip_suffix(".sha256") {
        return match std::fs::read(base_dir.join(path)) {
            Ok(bytes) => {
                let digest = crate::redact::sha256(&bytes);
                ProbeResult::observed(serde_json::json!(hex(&digest)), "fs.sha256", None)
            }
            Err(_) => ProbeResult::observed(serde_json::json!(false), "fs.sha256:missing", None),
        };
    }
    ProbeResult::unknown(format!("fs:unsupported-suffix:{rest}"))
}

fn probe_git(rest: &str, base_dir: &Path, policy: &ProbePolicy) -> ProbeResult {
    if !policy.allow_git {
        return ProbeResult::unknown("git:policy-off");
    }
    if rest == "branch" {
        if let Some(output) = run_git(&["rev-parse", "--abbrev-ref", "HEAD"], base_dir) {
            let value = output.trim().to_string();
            if value != "HEAD" && !value.is_empty() {
                return ProbeResult::observed(serde_json::json!(value), "git", Some(30));
            }
        }
        // Unborn branch (fresh `git init` with no commits): read HEAD directly.
        let head = std::fs::read_to_string(base_dir.join(".git/HEAD")).unwrap_or_default();
        let branch = head
            .trim()
            .strip_prefix("ref: refs/heads/")
            .map(str::to_string);
        return match branch {
            Some(branch) => ProbeResult::observed(serde_json::json!(branch), "git.head", Some(30)),
            None => ProbeResult::unknown("git:unavailable"),
        };
    }
    let args: &[&str] = match rest {
        "dirty" => &["status", "--porcelain"],
        "last_commit" => &["rev-parse", "HEAD"],
        _ => return ProbeResult::unknown(format!("git:unsupported:{rest}")),
    };
    match run_git(args, base_dir) {
        Some(output) => {
            let value = match rest {
                "dirty" => serde_json::json!(!output.trim().is_empty()),
                _ => serde_json::json!(output.trim()),
            };
            ProbeResult::observed(value, "git", Some(30))
        }
        None => ProbeResult::unknown("git:unavailable"),
    }
}

fn run_git(args: &[&str], base_dir: &Path) -> Option<String> {
    let mut command = Command::new("git");
    command.args(args);
    command.current_dir(base_dir);
    command.stdin(std::process::Stdio::null());
    let output = command.output().ok()?;
    if !output.status.success() && args.first() != Some(&"status") {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Minimal HTTP/1.0 GET over TcpStream: no TLS, no redirects, bounded read.
/// Enough to observe local services (`http://127.0.0.1:PORT/...`).
fn probe_http(rest: &str, policy: &ProbePolicy) -> ProbeResult {
    if !policy.allow_network {
        return ProbeResult::unknown("http:policy-off");
    }
    let (kind, url) = match rest.split_once(':') {
        Some(parts) => parts,
        None => return ProbeResult::unknown(format!("http:unsupported:{rest}")),
    };
    let Some(fetch) = fetch_http(url, policy.http_timeout_ms) else {
        return ProbeResult::unknown("http:unreachable");
    };
    match kind {
        "status" => ProbeResult::observed(serde_json::json!(fetch.status), "http", Some(15)),
        "body_sha256" => {
            let digest = crate::redact::sha256(fetch.body.as_bytes());
            ProbeResult::observed(serde_json::json!(hex(&digest)), "http", Some(15))
        }
        other => ProbeResult::unknown(format!("http:unsupported:{other}")),
    }
}

struct HttpFetch {
    status: u16,
    body: String,
}

fn fetch_http(url: &str, timeout_ms: u64) -> Option<HttpFetch> {
    let rest = url.strip_prefix("http://")?;
    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, port.parse::<u16>().ok()?),
        None => (authority, 80),
    };
    let mut stream = TcpStream::connect((host, port)).ok()?;
    let timeout = Duration::from_millis(timeout_ms.max(1));
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    use std::io::Read;
    use std::io::Write;
    let request = format!(
        "GET {path} HTTP/1.0\r\nHost: {authority}\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).ok()?;
    let mut raw = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                raw.extend_from_slice(&buffer[..read]);
                if raw.len() > 512 * 1024 {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let text = String::from_utf8_lossy(&raw).to_string();
    let (head, body) = text.split_once("\r\n\r\n")?;
    let status = head
        .lines()
        .next()?
        .split_whitespace()
        .nth(1)?
        .parse::<u16>()
        .ok()?;
    Some(HttpFetch {
        status,
        body: body.to_string(),
    })
}

fn probe_port(rest: &str, policy: &ProbePolicy) -> ProbeResult {
    let port = match rest.parse::<u16>() {
        Ok(port) => port,
        Err(_) => return ProbeResult::unknown(format!("port:invalid:{rest}")),
    };
    let timeout = Duration::from_millis(policy.port_timeout_ms.max(1));
    let open = [
        std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        std::net::SocketAddr::from(([0u16, 0, 0, 0, 0, 0, 0, 1], port)),
    ]
    .iter()
    .any(|addr| TcpStream::connect_timeout(addr, timeout).is_ok());
    ProbeResult::observed(serde_json::json!(open), "net.port", Some(10))
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("exp-state-{tag}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn registry_knows_registered_families_only() {
        assert_eq!(family_of("file:a.txt.exists"), Some("fs"));
        assert_eq!(family_of("dir:src.exists"), Some("fs"));
        assert_eq!(family_of("process.exit_code:exec#0"), Some("exec"));
        assert_eq!(family_of("git.dirty"), Some("git"));
        assert_eq!(family_of("http.status:http://127.0.0.1:1/"), Some("http"));
        assert_eq!(family_of("port.open:1"), Some("net"));
        assert_eq!(family_of("mystery.key"), None);
        assert!(!observable("mystery.key"));
    }

    #[test]
    fn fs_predicates_report_size_and_sha256() {
        let dir = temp_dir("fs");
        std::fs::write(dir.join("a.txt"), "hello").unwrap();
        let policy = ProbePolicy::default();
        let size = probe("file:a.txt.size", &dir, &policy);
        assert_eq!(size.value, Some(serde_json::json!(5)));
        let hash = probe("file:a.txt.sha256", &dir, &policy);
        let expected = {
            let digest = crate::redact::sha256(b"hello");
            let mut out = String::new();
            for byte in digest {
                out.push_str(&format!("{byte:02x}"));
            }
            out
        };
        assert_eq!(hash.value, Some(serde_json::json!(expected)));
        let missing = probe("file:gone.txt.sha256", &dir, &policy);
        assert_eq!(missing.value, Some(serde_json::json!(false)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dir_and_unregistered_keys_are_handled_honestly() {
        let dir = temp_dir("dir");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let policy = ProbePolicy::default();
        assert_eq!(
            probe("dir:src.exists", &dir, &policy).value,
            Some(serde_json::json!(true))
        );
        assert_eq!(
            probe("dir:missing.exists", &dir, &policy).value,
            Some(serde_json::json!(false))
        );
        assert_eq!(probe("mystery.key", &dir, &policy).value, None);
        assert_eq!(probe("file:a.txt.mtime", &dir, &policy).value, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn network_probes_are_unknown_while_policy_is_off() {
        let dir = temp_dir("net-off");
        let policy = ProbePolicy::default();
        assert_eq!(probe("http.status:http://127.0.0.1:1/", &dir, &policy).value, None);
        let port = probe("port.open:9", &dir, &policy);
        // `port.open` is a local observation and needs no network grant.
        assert!(port.value.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn port_probe_observes_a_bound_listener() {
        let dir = temp_dir("port");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let policy = ProbePolicy::default();
        let result = probe(&format!("port.open:{port}"), &dir, &policy);
        assert_eq!(result.value, Some(serde_json::json!(true)));
        assert_eq!(result.ttl, Some(10));
        drop(listener);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn git_probe_reports_dirty_branch_and_commit_when_available() {
        let dir = temp_dir("git");
        let policy = ProbePolicy::default();
        // No repository yet: the probe must be honest (Unknown), not "clean".
        let before = probe("git.branch", &dir, &policy);
        assert_eq!(before.value, None);

        let run = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(&dir)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            status.map(|status| status.success()).unwrap_or(false)
        };
        if !run(&["init", "-q"]) {
            let _ = std::fs::remove_dir_all(&dir);
            return; // git unavailable in this environment
        }
        std::fs::write(dir.join("tracked.txt"), "x").unwrap();
        let branch = probe("git.branch", &dir, &policy);
        assert!(branch.value.is_some(), "branch should be observable in a repo");
        let dirty = probe("git.dirty", &dir, &policy);
        assert_eq!(dirty.value, Some(serde_json::json!(true)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

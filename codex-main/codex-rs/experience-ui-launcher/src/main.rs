//! Experience UI launcher — a thin launcher exe (no GUI dependencies).
//!
//! It locates `codex.exe` (same directory or CODEX_EXE), sets CODEX_HOME to
//! the repo's `.codex-exp-home` when the env var is absent, starts
//! `codex experience ui`, and opens the default browser. If a server is
//! already listening on the port it just opens the browser and exits.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::Write;
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;

fn main() {
    let port = std::env::args()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(8765);
    let log_path = std::env::temp_dir().join("experience-ui-launcher.log");
    let log = |message: &str| {
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            let _ = writeln!(file, "{message}");
        }
    };

    let url = format!("http://127.0.0.1:{port}");
    if TcpStream::connect(("127.0.0.1", port)).is_ok() {
        let _ = open_browser(&url);
        return;
    }

    let codex = locate_codex();
    let Some(codex) = codex else {
        log("找不到 codex.exe（请把它与本启动器放同一目录，或设置 CODEX_EXE）");
        return;
    };

    let mut command = Command::new(&codex);
    command.args(["experience", "ui", "--port", &port.to_string()]);
    if std::env::var_os("CODEX_HOME").is_none() {
        if let Some(home) = locate_exp_home() {
            command.env("CODEX_HOME", home);
        }
    }
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    match command.spawn() {
        Ok(mut child) => {
            // Give the server a moment to bind before opening the browser.
            std::thread::sleep(std::time::Duration::from_millis(1200));
            let _ = open_browser(&url);
            let _ = child.wait();
        }
        Err(error) => log(&format!("启动 codex experience ui 失败: {error}")),
    }
}

fn locate_codex() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("CODEX_EXE") {
        return Some(PathBuf::from(path));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("codex.exe");
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

fn locate_exp_home() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    for ancestor in exe.ancestors().take(6) {
        let candidate = ancestor.join(".codex-exp-home");
        if candidate.join("experience").is_dir() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(windows)]
fn open_browser(url: &str) -> std::io::Result<()> {
    Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn()
        .map(|_| ())
}

#[cfg(not(windows))]
fn open_browser(_url: &str) -> std::io::Result<()> {
    Ok(())
}

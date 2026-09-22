//! Experience launcher — a thin, stable exe (reference-implementation launch
//! logic only).
//!
//! It locates `experience-server` next to itself (or via
//! EXPERIENCE_SERVER), reserves a free localhost port, spawns the server,
//! waits until it is listening, and opens the default browser. UI/API code
//! lives on disk; only this launcher is packaged once.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::Write;
use std::net::TcpListener;
use std::net::TcpStream;
use std::path::Path;
use std::path::PathBuf;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;
use std::time::Duration;

fn main() {
    let no_open = std::env::args().any(|value| value == "--no-open");
    let log_path = std::env::temp_dir().join("experience-launcher.log");
    let log = |message: &str| {
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            let _ = writeln!(file, "{message}");
        }
    };

    let home = std::env::var_os("EXPERIENCE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| default_home());
    std::fs::create_dir_all(&home).ok();
    let port_file = home.join(".port");

    // Single instance: a previous launcher left a port file; if a server is
    // still answering there, just surface it (open and exit, as the reference
    // implementation does).
    if let Ok(existing) = std::fs::read_to_string(&port_file) {
        if let Ok(port) = existing.trim().parse::<u16>() {
            if TcpStream::connect(("127.0.0.1", port)).is_ok() {
                let url = format!("http://127.0.0.1:{port}/");
                log("already running; opening browser only");
                if !no_open {
                    let _ = open_browser(&url);
                }
                return;
            }
        }
    }

    let Some(server) = locate_server() else {
        log("找不到 experience-server（请把它与本启动器放同一目录，或设置 EXPERIENCE_SERVER）");
        return;
    };
    let port = reserve_port();
    let url = format!("http://127.0.0.1:{port}/");

    let mut command = Command::new(&server);
    command
        .env("EXPERIENCE_PORT", port.to_string())
        .env("EXPERIENCE_HOME", &home);
    if let Some(ui_dir) = locate_ui_dir() {
        command.env("EXPERIENCE_UI_DIR", ui_dir);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            log(&format!("启动 experience-server 失败: {error}"));
            return;
        }
    };

    if !wait_for_service(port) {
        log("experience-server 未能监听端口");
        let _ = kill_child(child);
        return;
    }
    let _ = std::fs::write(&port_file, port.to_string());
    log(&format!("experience ready: {url}"));
    if !no_open {
        let _ = open_browser(&url);
    }
    let _ = wait_child(child);
    let _ = std::fs::remove_file(&port_file);
}

fn reserve_port() -> u16 {
    if let Ok(listener) = TcpListener::bind(("127.0.0.1", 0)) {
        if let Ok(address) = listener.local_addr() {
            return address.port();
        }
    }
    8766
}

fn wait_for_service(port: u16) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while std::time::Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    false
}

fn locate_server() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("EXPERIENCE_SERVER") {
        return Some(PathBuf::from(path));
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    for candidate in [
        dir.join("experience-server.exe"),
        dir.join("experience-server"),
    ] {
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn locate_ui_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    for candidate in [dir.join("ui"), dir.parent()?.join("ui")] {
        if candidate.join("index.html").is_file() {
            return Some(candidate);
        }
    }
    None
}

fn default_home() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .map(|dir| dir.parent().unwrap_or(&dir).join(".experience-home"))
        .unwrap_or_else(|| PathBuf::from(".experience-home"))
}

fn kill_child(mut child: Child) -> std::io::Result<()> {
    child.kill()?;
    child.wait()?;
    Ok(())
}

fn wait_child(mut child: Child) -> std::io::Result<()> {
    child.wait()?;
    Ok(())
}

#[cfg(windows)]
fn open_browser(url: &str) -> std::io::Result<()> {
    Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn()
        .map(|_| ())
}

#[cfg(not(windows))]
fn open_browser(url: &str) -> std::io::Result<()> {
    Command::new("xdg-open").arg(url).spawn().map(|_| ())
}

//! P0-1 acceptance runner: real fork Session Host via agent-codex's
//! SessionChannel. Run from your own terminal:
//!   cargo run -p agent-codex --example session_host_probe --offline
//! Expects: fork codex + .codex-exp-home + DEEPSEEK_API_KEY env (set by
//! your launcher scripts) or a token in ~/.codex/config.toml.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use agent_codex::session_host::run_session_task;
use agent_codex::session_host::SessionHostConfig;
use agent_codex::session_host::SessionTask;

fn main() {
    let exe = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"D:\experience_codex\codex-main\codex-rs\target\debug\codex.exe"));
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"D:\experience_codex\codex-main\.codex-exp-home"));
    let workspace = std::env::args()
        .nth(2)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"D:\experience_codex\experience-main\probe-e-workspace"));

    let nonce = format!("{:x}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos());
    let filename = format!("session-host-{nonce}.txt");
    let target = workspace.join(&filename);
    std::fs::create_dir_all(&workspace).expect("create workspace");

    let config = SessionHostConfig {
        exe,
        codex_home: Some(home),
        extra_env: vec![
            ("DEEPSEEK_API_KEY".to_string(), std::env::var("DEEPSEEK_API_KEY").unwrap_or_default()),
        ],
    };
    let task = SessionTask {
        workspace: workspace.clone(),
        task: format!(
            "Create a file named {filename} in {} whose content is exactly {nonce}, then stop.",
            workspace.display()
        ),
    };
    let cancel = AtomicBool::new(false);

    println!("target: {}", target.display());
    match run_session_task(&config, &task, &cancel, |dir| {
        let file = dir.join(&filename);
        file.is_file()
            && std::fs::read_to_string(&file).map(|text| text.trim() == nonce).unwrap_or(false)
    }) {
        Ok(outcome) => {
            println!("ok={} thread_id={:?}", outcome.ok, outcome.thread_id);
            for event in &outcome.events {
                println!("  {}", event.label());
            }
            if let Some(error) = outcome.error {
                println!("error: {error}");
            }
            if outcome.ok && target.exists() {
                println!("SESSION_HOST_PROBE: PASS");
            } else {
                println!("SESSION_HOST_PROBE: FAIL");
                std::process::exit(1);
            }
        }
        Err(error) => {
            println!("SESSION_HOST_PROBE: FAIL - {error}");
            std::process::exit(1);
        }
    }
}

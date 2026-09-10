//! P0-2 acceptance: SessionChannel on a REAL edit task (modify an existing
//! file while preserving sentinel lines; verify exact content).
//! Run from your own terminal:
//!   cargo run -p agent-codex --example session_host_edit_probe --offline

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use agent_codex::session_host::run_session_task;
use agent_codex::session_host::SessionHostConfig;
use agent_codex::session_host::SessionTask;

const FIXTURE: &str = "SENTINEL_A=keep-me-alpha\nEDIT_ME=original-value\nSENTINEL_B=keep-me-beta\n";

fn main() {
    let exe = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"D:\experience_codex\codex-main\codex-rs\target\debug\codex.exe"));
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"D:\experience_codex\codex-main\.codex-exp-home"));
    let workspace = PathBuf::from(r"D:\experience_codex\experience-main\probe-e-workspace");
    let file = workspace.join("edit-fixture.txt");

    std::fs::create_dir_all(&workspace).expect("create workspace");
    std::fs::write(&file, FIXTURE).expect("write fixture");
    let nonce = format!("{:x}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos());
    let replacement = format!("replaced-{nonce}");

    let config = SessionHostConfig {
        exe,
        codex_home: Some(home),
        extra_env: vec![(
            "DEEPSEEK_API_KEY".to_string(),
            std::env::var("DEEPSEEK_API_KEY").unwrap_or_default(),
        )],
    };
    let task = SessionTask {
        workspace: workspace.clone(),
        task: format!(
            "Edit the file {}: change the value after the line starting with EDIT_ME= to {replacement}. \
             Do NOT touch the lines starting with SENTINEL_A or SENTINEL_B. \
             Keep every other byte identical. Then read the file back and report the diff.",
            file.display()
        ),
    };
    let cancel = AtomicBool::new(false);

    println!("target : {}", file.display());
    println!("expect : {replacement} with both sentinels unchanged");
    match run_session_task(&config, &task, &cancel, |_dir| {
        let Ok(content) = std::fs::read_to_string(&file) else {
            return false;
        };
        let expected = format!(
            "SENTINEL_A=keep-me-alpha\nEDIT_ME={replacement}\nSENTINEL_B=keep-me-beta\n"
        );
        content == expected
    }) {
        Ok(outcome) => {
            println!("ok={} thread_id={:?}", outcome.ok, outcome.thread_id);
            for event in &outcome.events {
                println!("  {}", event.label());
            }
            if let Some(error) = outcome.error {
                println!("error: {error}");
            }
            let content = std::fs::read_to_string(&file).unwrap_or_default();
            println!("--- final file ---\n{content}---");
            let expected = format!(
                "SENTINEL_A=keep-me-alpha\nEDIT_ME={replacement}\nSENTINEL_B=keep-me-beta\n"
            );
            if outcome.ok && content == expected {
                println!("SESSION_HOST_EDIT_PROBE: PASS (sentinels preserved, exact content)");
            } else {
                println!("SESSION_HOST_EDIT_PROBE: FAIL");
                std::process::exit(1);
            }
        }
        Err(error) => {
            println!("SESSION_HOST_EDIT_PROBE: FAIL - {error}");
            std::process::exit(1);
        }
    }
}

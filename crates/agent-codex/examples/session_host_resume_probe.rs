//! P2 real acceptance (release gate): two rounds on ONE thread.
//!
//! Round 1 = `run_session_task_until_turn` on a fresh thread/start (the app's
//! session-channel path); round 2 = `resume_thread` on the same thread id
//! after the first host was torn down gracefully. Verifies real workspace
//! file side effects, a stable thread id, and that teardown does not leave a
//! dirty active/pending turn behind (thread/resume must start cleanly).
//!
//! Run from your own terminal with DEEPSEEK_API_KEY set (see run-*.ps1):
//!   cargo run -p agent-codex --example session_host_resume_probe --offline

use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use agent_codex::session_host::run_session_task_until_turn;
use agent_codex::session_host::resume_thread;
use agent_codex::session_host::SessionHostConfig;
use agent_codex::session_host::SessionTask;

fn main() {
    let exe = std::env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(r"D:\experience_codex\codex-main\codex-rs\target\debug\codex.exe")
    });
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"D:\experience_codex\codex-main\.codex-exp-home"));
    let workspace = std::env::args().nth(2).map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(r"D:\experience_codex\experience-main\probe-e-workspace")
    });
    std::fs::create_dir_all(&workspace).expect("create workspace");

    let nonce1 = format!("{:x}", nanos());
    let nonce2 = format!("{:x}", nanos());
    let file1 = workspace.join(format!("resume-round1-{nonce1}.txt"));
    let file2 = workspace.join(format!("resume-round2-{nonce2}.txt"));

    let config = SessionHostConfig {
        exe,
        codex_home: Some(home),
        extra_env: vec![(
            "DEEPSEEK_API_KEY".to_string(),
            std::env::var("DEEPSEEK_API_KEY").unwrap_or_default(),
        )],
    };
    let round1 = SessionTask {
        workspace: workspace.clone(),
        task: format!(
            "Create a file named {} in {} whose content is exactly {}, then stop.",
            file1.file_name().unwrap().to_string_lossy(),
            workspace.display(),
            nonce1
        ),
    };
    let round2 = SessionTask {
        workspace: workspace.clone(),
        task: format!(
            "Create a file named {} in {} whose content is exactly {}, then stop.",
            file2.file_name().unwrap().to_string_lossy(),
            workspace.display(),
            nonce2
        ),
    };
    let cancel = AtomicBool::new(false);

    println!("round1 target: {}", file1.display());
    let outcome1 = run_session_task_until_turn(&config, &round1, &cancel, |event| {
        println!("  r1 {}", event.label());
    })
    .expect("round 1 driver error");
    println!("round1 ok={} thread_id={:?}", outcome1.ok, outcome1.thread_id);
    if let Some(error) = &outcome1.error {
        println!("round1 error: {error}");
    }
    let thread_id = outcome1
        .thread_id
        .clone()
        .expect("round 1 must return a thread id");
    if !outcome1.ok || !content_is(file1.as_path(), &nonce1) {
        println!("SESSION_HOST_RESUME_PROBE: FAIL (round 1)");
        std::process::exit(1);
    }

    println!("round2 target: {}", file2.display());
    let outcome2 = resume_thread(&config, &thread_id, &round2, &cancel, |event| {
        println!("  r2 {}", event.label());
    })
    .expect("round 2 driver error");
    println!("round2 ok={} thread_id={:?}", outcome2.ok, outcome2.thread_id);
    if let Some(error) = &outcome2.error {
        println!("round2 error: {error}");
    }
    let same_thread = outcome2.thread_id.as_deref() == Some(thread_id.as_str());
    if !outcome2.ok || !same_thread || !content_is(file2.as_path(), &nonce2) {
        println!("SESSION_HOST_RESUME_PROBE: FAIL (round 2, same_thread={same_thread})");
        std::process::exit(1);
    }

    println!("SESSION_HOST_RESUME_PROBE: PASS (two rounds, same thread, no dirty active turn)");
}

fn content_is(file: &Path, expected: &str) -> bool {
    std::fs::read_to_string(file)
        .map(|text| text.trim() == expected)
        .unwrap_or(false)
}

fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

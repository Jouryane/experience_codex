//! P2 regression (item 3): once thread/start succeeds, every later failure
//! must return as a failed SessionOutcome that keeps thread_id (early-failure
//! backfill) instead of a bare Err that loses the created thread.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use agent_codex::session_host::run_session_task;
use agent_codex::session_host::run_session_task_until_turn;
use agent_codex::session_host::resume_thread;
use agent_codex::session_host::SessionHostConfig;
use agent_codex::session_host::SessionTask;

fn host_config(behavior: &str) -> SessionHostConfig {
    SessionHostConfig {
        exe: PathBuf::from(
            std::env::var_os("CARGO_BIN_EXE_mock_app_server")
                .expect("mock_app_server binary was not built"),
        ),
        codex_home: None,
        extra_env: vec![("EXP_MOCK_BEHAVIOR".to_string(), behavior.to_string())],
    }
}

fn task() -> SessionTask {
    SessionTask {
        workspace: std::env::temp_dir(),
        task: "mock task".to_string(),
    }
}

#[test]
fn until_turn_queue_add_failure_backfills_thread_id() {
    let cancel = AtomicBool::new(false);
    let outcome = run_session_task_until_turn(
        &host_config("queue_add_error"),
        &task(),
        &cancel,
        |_| {},
    )
    .expect("post-start failures should be outcomes, not bare errors");
    assert!(!outcome.ok);
    assert_eq!(outcome.thread_id.as_deref(), Some("mock-thread-1"));
    assert!(outcome.error.unwrap().contains("queue/add error"));
}

#[test]
fn until_turn_thread_start_error_is_bare_error() {
    let cancel = AtomicBool::new(false);
    let error = run_session_task_until_turn(
        &host_config("thread_start_error"),
        &task(),
        &cancel,
        |_| {},
    )
    .expect_err("a thread/start failure happens before any thread exists");
    assert!(error.contains("thread/start error"));
}

#[test]
fn until_turn_queue_start_failure_backfills_thread_id() {
    let cancel = AtomicBool::new(false);
    let outcome = run_session_task_until_turn(
        &host_config("queue_start_error"),
        &task(),
        &cancel,
        |_| {},
    )
    .expect("post-start failures should be outcomes, not bare errors");
    assert!(!outcome.ok);
    assert_eq!(outcome.thread_id.as_deref(), Some("mock-thread-1"));
    assert!(outcome.error.unwrap().contains("queue/start error"));
}

#[test]
fn until_turn_tolerates_host_auto_start_and_completes() {
    let cancel = AtomicBool::new(false);
    let outcome = run_session_task_until_turn(
        &host_config("auto_start"),
        &task(),
        &cancel,
        |_| {},
    )
    .expect("a queue-empty reply after queue/add means the host already started the turn");
    assert!(outcome.ok);
    assert_eq!(outcome.thread_id.as_deref(), Some("mock-thread-1"));
    assert!(
        outcome
            .events
            .iter()
            .any(|event| event.label().contains("already_started")),
        "auto-start tolerance should surface as an event"
    );
}

#[test]
fn verify_run_queue_add_failure_backfills_thread_id() {
    let cancel = AtomicBool::new(false);
    let outcome =
        run_session_task(&host_config("queue_add_error"), &task(), &cancel, |_| false)
            .expect("post-start failures should be outcomes, not bare errors");
    assert!(!outcome.ok);
    assert_eq!(outcome.thread_id.as_deref(), Some("mock-thread-1"));
    assert!(outcome.error.unwrap().contains("queue/add error"));
}

#[test]
fn verify_run_thread_start_error_is_bare_error() {
    let cancel = AtomicBool::new(false);
    let error = run_session_task(
        &host_config("thread_start_error"),
        &task(),
        &cancel,
        |_| false,
    )
    .expect_err("a thread/start failure happens before any thread exists");
    assert!(error.contains("thread/start error"));
}

#[test]
fn resume_thread_resume_error_is_explicit() {
    let cancel = AtomicBool::new(false);
    let error = resume_thread(
        &host_config("resume_error"),
        "mock-thread-1",
        &task(),
        &cancel,
        |_| {},
    )
    .expect_err("thread/resume failure happens before any queueing");
    assert!(error.contains("thread/resume error"));
}

#[test]
fn resume_thread_queue_add_failure_is_explicit() {
    let cancel = AtomicBool::new(false);
    let error = resume_thread(
        &host_config("resume_queue_add_error"),
        "mock-thread-1",
        &task(),
        &cancel,
        |_| {},
    )
    .expect_err("queue/add failure on resume is a driver error; the caller keeps thread_id");
    assert!(error.contains("queue/add error"));
}

#[test]
fn resume_thread_tolerates_active_turn_and_completes() {
    let cancel = AtomicBool::new(false);
    let outcome = resume_thread(
        &host_config("active_turn"),
        "mock-thread-1",
        &task(),
        &cancel,
        |_| {},
    )
    .expect("an already-active turn is tolerated and the run completes");
    assert!(outcome.ok);
    assert_eq!(outcome.thread_id.as_deref(), Some("mock-thread-1"));
    assert!(
        outcome
            .events
            .iter()
            .any(|event| event.label().contains("already_started")),
        "active-turn tolerance should surface as an event"
    );
}

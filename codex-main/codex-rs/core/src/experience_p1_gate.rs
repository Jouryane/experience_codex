//! M4: real Experience Gate (embedded Runtime, Model A).
//!
//! Replaces the experimental probe with the actual P1 runtime from
//! experience-main: codex-core embeds `experience-core` /
//! `experience-controller` as libraries and consults them on the dispatch
//! boundary, before the original tool executes.
//!
//! Env contract:
//!   EXPERIENCE_ENABLED     master switch (shared with the legacy subsystem)
//!   EXPERIENCE_GATE_STORE  path to a P1 store.json envelope
//!   EXPERIENCE_GATE_CWD    optional GateContext cwd override (tests)
//!
//! Availability semantics (compatibility.md §1.1): if the store cannot be
//! loaded the gate degrades to MISS and the original tool runs normally.
//! The gate never blocks the agent loop and never introduces a third Gate
//! outcome besides Hit/Miss.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use crate::experience_management::ExperienceUsageStore;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ResponseItem;
use experience_controller::probe::LocalProbe;
use experience_controller::runner::LocalRunner;
use experience_controller::runtime::ExperienceGateRuntime;
use experience_controller::runtime::GateContext;
use experience_core::domain::action::ActionProposal;
use experience_core::domain::gate::GateDecision;
use experience_core::domain::gate::GateHitResult;
use experience_core::store::ExperienceStore;

static RUNTIME: Mutex<Option<Arc<ExperienceGateRuntime>>> = Mutex::new(None);
/// Serializes gate usage.json read-modify-write (L4/WP7; same discipline as
/// the experience-main audit lock).
static GATE_USAGE_LOCK: Mutex<()> = Mutex::new(());

/// Fuse against takeover loops (step-gate §5): if the gate hits more than
/// MAX_HITS inside one rolling window, further hits degrade to MISS so the
/// original tool runs and the LLM cannot spin on the same action forever.
const MAX_GATE_HITS: u32 = 15;
const GATE_WINDOW: Duration = Duration::from_secs(60);
static GATE_HITS: Mutex<Option<(Instant, u32)>> = Mutex::new(None);

/// V2 learning source: the request text of the current turn, stashed so the
/// observation recorder can label calls with the task they served.
static TRACE_TASK: Mutex<Option<String>> = Mutex::new(None);
/// Serializes observation-file appends.
static TRACE_LOCK: Mutex<()> = Mutex::new(());

/// Remember the current turn's request text (no-op unless recording is on).
pub(crate) fn note_task_text(task_text: &str) {
    if std::env::var_os("EXPERIENCE_TRACE_OUT").is_none() {
        return;
    }
    if let Ok(mut guard) = TRACE_TASK.lock() {
        *guard = Some(task_text.to_string());
    }
}

/// Append one *model-driven* tool call to the observation log.
///
/// Only calls the LLM actually chose are recorded, and only after the Gate
/// declined to take over: a Gate-mediated call is not evidence of what the
/// model would do, and feeding it back into induction would let the
/// Experience learn from itself. The log is newline-delimited JSON, one line
/// per step, so a crashed run still leaves a usable prefix.
pub(crate) fn record_observed_call(item: &ResponseItem) {
    let Some(path) = std::env::var_os("EXPERIENCE_TRACE_OUT") else {
        return;
    };
    let ResponseItem::FunctionCall { name, arguments, .. } = item else {
        return;
    };
    let task = TRACE_TASK
        .lock()
        .ok()
        .and_then(|guard| guard.clone())
        .unwrap_or_default();
    let args = serde_json::from_str(arguments)
        .unwrap_or_else(|_| serde_json::Value::String(arguments.clone()));
    let line = serde_json::json!({
        "task": task,
        "action": name,
        "args": args,
    });
    let _guard = TRACE_LOCK.lock();
    let write = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(PathBuf::from(&path))
        .and_then(|mut file| {
            use std::io::Write;
            writeln!(file, "{line}")
        });
    if let Err(error) = write {
        tracing::warn!(
            path = %PathBuf::from(&path).display(),
            "failed to append experience observation: {error}"
        );
    }
}

/// Pseudo tool names for the turn-level adapter. These are not Agent tools:
/// they let the same canonical P1 Store express "this task is fully covered"
/// (`task`) or "run this known prefix, then hand the remainder to the LLM"
/// (`task_prefix`).
const TASK_GATE_TOOL: &str = "task";
const TASK_PREFIX_GATE_TOOL: &str = "task_prefix";
/// Pseudo tool for world-state triggers: an ACTIVE experience whose trigger
/// tool is `state` is executed because its preconditions hold right now,
/// without the model proposing anything and without the request mentioning it.
const STATE_GATE_TOOL: &str = "state";
/// Bounded per turn: a state seat may advance the world, it may never loop.
const MAX_STATE_TAKEOVERS: usize = 2;

fn gate_hit_allowed() -> bool {
    let mut guard = match GATE_HITS.lock() {
        Ok(guard) => guard,
        Err(_) => return true,
    };
    match *guard {
        Some((started, count)) if started.elapsed() < GATE_WINDOW => {
            if count >= MAX_GATE_HITS {
                tracing::warn!(
                    hits = count,
                    "experience gate hit limit reached; degrading to MISS (pass-through)"
                );
                false
            } else {
                *guard = Some((started, count + 1));
                true
            }
        }
        _ => {
            *guard = Some((Instant::now(), 1));
            true
        }
    }
}

fn enabled() -> bool {
    crate::experience::enabled() && resolved_store_path().is_some()
}

/// The one resolved store for this process. `EXPERIENCE_GATE_STORE` is an
/// override, not a prerequisite: without it the Gate reads the same
/// `<codex_home>/experience/store.json` the management surface writes.
fn resolved_store_path() -> Option<PathBuf> {
    let explicit = std::env::var_os("EXPERIENCE_GATE_STORE");
    if let Some(path) = explicit.as_ref().filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let codex_home = crate::config::find_codex_home().ok()?;
    let path = crate::experience_paths::resolve_store_path(codex_home.as_path());
    // Without an override the store must actually exist in canonical form;
    // a legacy envelope is not something the execution plane may adopt. A
    // missing file is created once, so a fresh install arms itself instead of
    // silently behaving like stock Codex.
    ensure_default_store(&path);
    match crate::experience_paths::detect_format(&path) {
        crate::experience_paths::StoreFormat::Canonical => Some(path),
        _ => None,
    }
}

/// "Is an execution seat configured?" — asked once per turn by the turn loop.
///
/// The turn-level adapter used to require `EXPERIENCE_GATE_STORE` to be set,
/// which meant the default home store only worked for the dispatch seam. Now an
/// explicit override still counts, and otherwise the default store counts as
/// soon as it exists (and it is created on first resolution).
pub(crate) fn store_configured() -> bool {
    // Decided once per process, like the cached runtime: the answer cannot
    // change under a running session, and the turn loop asks every turn.
    static ARMED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ARMED.get_or_init(|| {
        if std::env::var_os("EXPERIENCE_GATE_STORE").is_some_and(|value| !value.is_empty()) {
            return true;
        }
        if std::env::var_os("EXPERIENCE_TASK_GATE").is_some() {
            return true;
        }
        resolved_store_path().is_some()
    })
}

/// Is the world-state seat allowed to run?
///
/// Enabled by default once a store is configured (the seat is inert without an
/// experience that explicitly declares `trigger.tool = "state"`, and those are
/// only ever created deliberately). `EXPERIENCE_STATE_GATE=0/false/off/no`
/// turns it off, matching how `EXPERIENCE_ENABLED` is read elsewhere.
pub(crate) fn state_seat_enabled() -> bool {
    match std::env::var("EXPERIENCE_STATE_GATE") {
        Ok(value) => !matches!(
            value.trim().to_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        ),
        Err(_) => true,
    }
}

/// Create the canonical store at the DEFAULT location if it is not there yet.
///
/// Without this, "the file does not exist" silently means "every seat is off",
/// and a freshly installed fork would behave like stock Codex with no hint.
/// Only the default path is created: an explicit `EXPERIENCE_GATE_STORE`
/// override pointing at a missing file is a configuration mistake (or a
/// fixture that has not been written yet) and is left alone to fail loudly.
fn ensure_default_store(store_path: &PathBuf) -> bool {
    if std::env::var_os("EXPERIENCE_GATE_STORE").is_some() {
        return false;
    }
    if crate::experience_paths::detect_format(store_path)
        != crate::experience_paths::StoreFormat::Missing
    {
        return false;
    }
    let Some(parent) = store_path.parent() else {
        return false;
    };
    if let Err(error) = std::fs::create_dir_all(parent) {
        tracing::warn!(?error, "cannot create experience store directory");
        return false;
    }
    let envelope = "{\n  \"schema_version\": 1,\n  \"experiences\": [],\n  \"templates\": []\n}\n";
    match std::fs::write(store_path, envelope) {
        Ok(()) => {
            tracing::info!(
                path = %store_path.display(),
                "created empty canonical experience store; the execution seats are now armed"
            );
            true
        }
        Err(error) => {
            tracing::warn!(?error, "cannot create experience store");
            false
        }
    }
}

fn load_runtime() -> Option<Arc<ExperienceGateRuntime>> {
    let store_path = resolved_store_path()?;
    let store = match ExperienceStore::open(store_path) {
        Ok(store) => store,
        Err(error) => {
            tracing::warn!(
                ?error,
                "experience store unreadable; gate disabled (MISS)"
            );
            return None;
        }
    };
    tracing::info!(
        experiences = store.len(),
        "experience gate runtime loaded (P1 embedded, model A)"
    );
    let policy = store.global_policy();
    let runner = match std::env::var_os("EXPERIENCE_GATE_BACKUP_ROOT") {
        Some(root) => Box::new(LocalRunner::with_backup_and_policy(
            PathBuf::from(root),
            policy,
        )),
        None => Box::new(LocalRunner::with_policy(policy)),
    };
    Some(Arc::new(ExperienceGateRuntime::new(
        store,
        Box::new(LocalProbe),
        runner,
    )))
}

fn runtime() -> Option<Arc<ExperienceGateRuntime>> {
    if !enabled() {
        return None;
    }
    let mut guard = RUNTIME.lock().ok()?;
    if guard.is_none() {
        *guard = load_runtime();
    }
    guard.clone()
}

fn gate_context() -> GateContext {
    let cwd = std::env::var_os("EXPERIENCE_GATE_CWD")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    GateContext { cwd }
}

/// Turn-level decision handed back to `run_turn`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TaskGateOutcome {
    /// No canonical task/task_prefix experience matched.
    Miss,
    /// The known task was executed and verified; the LLM must not be sampled.
    Completed { experience: String, summary: String },
    /// A known prefix ran; the remainder still needs the LLM.
    Handoff { experience: String, summary: String },
}

/// Turn-level adapter over the same canonical runtime used by the dispatch
/// Gate. It tries full-task coverage first, then an explicitly marked prefix.
pub(crate) fn try_task_gate(task_text: &str) -> TaskGateOutcome {
    let Some(runtime) = runtime() else {
        return TaskGateOutcome::Miss;
    };
    let context = gate_context();

    let full = ActionProposal::new(
        TASK_GATE_TOOL,
        serde_json::json!({
            "task": task_text,
            "text": task_text,
        }),
    );
    let full_decision = runtime.decide(&full, &context);
    if full_decision.is_takeover() {
        if !gate_hit_allowed() {
            return TaskGateOutcome::Miss;
        }
        let experience = match &full_decision {
            GateDecision::Hit { experience, .. }
            | GateDecision::Satisfied { experience, .. }
            | GateDecision::TemplateHit { experience, .. } => experience.clone(),
            GateDecision::Miss => unreachable!("takeover checked"),
        };
        let result = runtime.execute(&full, &context, &full_decision);
        record_gate_usage(
            &experience,
            if result.is_success() {
                "experience_only"
            } else {
                "experience_first"
            },
            "codex-m6-task-gate",
            &result,
        );
        let summary = summarize(&result);
        return if result.is_success() {
            TaskGateOutcome::Completed {
                experience,
                summary,
            }
        } else {
            TaskGateOutcome::Handoff {
                experience,
                summary,
            }
        };
    }

    let prefix = ActionProposal::new(
        TASK_PREFIX_GATE_TOOL,
        serde_json::json!({
            "task": task_text,
            "text": task_text,
        }),
    );
    let prefix_decision = runtime.decide(&prefix, &context);
    if prefix_decision.is_takeover() {
        if !gate_hit_allowed() {
            return TaskGateOutcome::Miss;
        }
        let experience = match &prefix_decision {
            GateDecision::Hit { experience, .. }
            | GateDecision::Satisfied { experience, .. }
            | GateDecision::TemplateHit { experience, .. } => experience.clone(),
            GateDecision::Miss => unreachable!("takeover checked"),
        };
        let result = runtime.execute(&prefix, &context, &prefix_decision);
        record_gate_usage(
            &experience,
            "experience_first",
            "codex-m6-task-gate",
            &result,
        );
        return TaskGateOutcome::Handoff {
            experience,
            summary: summarize(&result),
        };
    }

    TaskGateOutcome::Miss
}

/// Consult the embedded Experience Gate for one FunctionCall.
/// World-state seat (V2): run ACTIVE experiences whose trigger is `state` and
/// whose preconditions hold in the world right now.
///
/// This is the seat the memory goal needs: the past changes the current path
/// *without being recalled*, and without the model proposing an action. It is
/// deliberately narrow:
///
/// - the store must already be canonical and `EXPERIENCE_STATE_GATE` must be
///   set, so the seat is never silently active;
/// - only experiences the author marked with trigger tool `state` are
///   considered, and only ACTIVE ones are indexed for matching;
/// - a `Satisfied` (already-done) decision ends the seat for this turn: it is
///   a no-op report, not a loop condition;
/// - at most [`MAX_STATE_TAKEOVERS`] takeovers per turn, and a failed takeover
///   hands control back immediately;
/// - every takeover goes through the same record path as any other Gate hit,
///   so it is auditable and cannot become a second controller.
pub(crate) struct StateTakeover {
    pub experience: String,
    pub summary: String,
    pub success: bool,
}

pub(crate) fn try_state_gate() -> Vec<StateTakeover> {
    let mut takeovers = Vec::new();
    if !state_seat_enabled() {
        return takeovers;
    }
    let Some(runtime) = runtime() else {
        return takeovers;
    };
    let context = gate_context();
    let proposal = ActionProposal::new(
        STATE_GATE_TOOL,
        serde_json::json!({ "trigger": "state" }),
    );
    for _ in 0..MAX_STATE_TAKEOVERS {
        let decision = runtime.decide(&proposal, &context);
        let experience = match &decision {
            GateDecision::Hit { experience, .. }
            | GateDecision::TemplateHit { experience, .. } => experience.clone(),
            // Already true in the world: report nothing to do and stop. The
            // seat must not re-run work that is verifiably done.
            GateDecision::Satisfied { experience, .. } => {
                tracing::info!(
                    experience = %experience,
                    "state trigger already satisfied; seat idle"
                );
                break;
            }
            GateDecision::Miss => break,
        };
        if !gate_hit_allowed() {
            break;
        }
        let result = runtime.execute(&proposal, &context, &decision);
        record_gate_usage(
            &experience,
            if result.is_success() {
                "experience_only"
            } else {
                "experience_first"
            },
            "codex-v2-state-gate",
            &result,
        );
        let success = result.is_success();
        takeovers.push(StateTakeover {
            experience,
            summary: summarize(&result),
            success,
        });
        if !success {
            break;
        }
    }
    takeovers
}

/// Consult the embedded Experience Gate for one FunctionCall.
///
/// Returns `(call_id, output_payload)` on a Hit — the caller must NOT
/// dispatch the original tool and must inject the payload as the tool
/// result instead. Returns None on Miss (pass-through) or when the runtime
/// is unavailable (safe degradation, never a third Gate semantic).
pub(crate) fn try_experience_gate(
    item: &ResponseItem,
) -> Option<(String, FunctionCallOutputPayload)> {
    let ResponseItem::FunctionCall {
        call_id,
        name,
        arguments,
        ..
    } = item
    else {
        return None;
    };
    let runtime = runtime()?;
    let args = serde_json::from_str(arguments).unwrap_or_else(|_| {
        // Arguments are JSON text normally; keep a text fallback so the
        // mechanical trigger pattern still sees the raw command.
        serde_json::Value::String(arguments.clone())
    });
    let proposal = ActionProposal::new(name.clone(), args);
    let context = gate_context();

    let decision = runtime.decide(&proposal, &context);
    if !decision.is_takeover() {
        return None;
    }
    let experience = match &decision {
        GateDecision::Hit { experience, .. }
        | GateDecision::Satisfied { experience, .. }
        | GateDecision::TemplateHit { experience, .. } => experience,
        GateDecision::Miss => unreachable!("takeover checked"),
    };
    if !gate_hit_allowed() {
        return None;
    }
    let result = runtime.execute(&proposal, &context, &decision);
    record_gate_usage(experience, "experience_only", "codex-m4-gate", &result);
    tracing::info!(
        call_id = %call_id,
        experience = %experience,
        "GATE HIT experience={experience}; original tool NOT dispatched"
    );
    Some((
        call_id.clone(),
        FunctionCallOutputPayload {
            body: FunctionCallOutputBody::Text(summarize(&result)),
            success: Some(result.is_success()),
        },
    ))
}

/// L4 (WP7): after a real Gate Hit, layer the outcome into the usage file
/// NEXT to EXPERIENCE_GATE_STORE. Only the Experience's own result is
/// recorded; delegation/loop outcomes never touch it.
fn record_gate_usage(experience: &str, band: &str, process: &str, result: &GateHitResult) {
    let Some(store_path) = resolved_store_path() else {
        return;
    };
    let usage_path = store_path.parent().map(|parent| parent.join("usage.json"));
    let Some(usage_path) = usage_path else {
        return;
    };
    let outcome = gate_usage_outcome(result).map(str::to_string);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let _guard = match GATE_USAGE_LOCK.lock() {
        Ok(guard) => guard,
        Err(_) => return,
    };
    let mut usage = ExperienceUsageStore::load_from_path(&usage_path);
    usage.record_logged_with_audit(
        experience,
        band,
        outcome,
        None,
        None,
        process.to_string(),
        None,
        result.template_audit.clone(),
        now,
    );
    if let Err(error) = usage.save_to_path(&usage_path) {
        tracing::warn!(?usage_path, "failed to persist gate usage: {error}");
    }
}

/// completed -> no special counter (hits/decision/log carry the success);
/// executed steps but not completed -> misfire (environment/premise, neutral);
/// nothing executed -> invalid (tool-boundary/semantic failure).
fn gate_usage_outcome(result: &GateHitResult) -> Option<&'static str> {
    gate_usage_outcome_for(result.is_success(), result.executed.is_empty())
}

fn gate_usage_outcome_for(success: bool, executed_empty: bool) -> Option<&'static str> {
    if success {
        None
    } else if executed_empty {
        Some("invalid")
    } else {
        Some("misfire")
    }
}

/// Render a GateHitResult as tool output the LLM can verify and continue
/// from (step-gate.md §9: closed Result, not a bare success flag).
fn summarize(result: &GateHitResult) -> String {
    let mut lines = Vec::new();
    lines.push(if result.is_success() {
        if result.executed.is_empty() {
            "EXPERIENCE GATE: already satisfied; no action needed; original tool NOT dispatched"
                .to_string()
        } else {
            "EXPERIENCE GATE: takeover completed; original tool NOT dispatched".to_string()
        }
    } else {
        "EXPERIENCE GATE: takeover resolved with issues; original tool NOT dispatched".to_string()
    });
    lines.push(format!(
        "status={:?} verification={:?}",
        result.completion_status, result.verification_status
    ));
    if !result.executed.is_empty() {
        lines.push("executed:".to_string());
        for step in &result.executed {
            lines.push(format!("- {} {}", step.action, step.args));
            if let Some(evidence) = &step.evidence {
                lines.push(format!("  evidence: {evidence}"));
            }
        }
    }
    if !result.executed_side_effects.is_empty() {
        lines.push("side effects:".to_string());
        for effect in &result.executed_side_effects {
            lines.push(format!("- {}", effect.description));
        }
    }
    if !result.state_after.is_empty() {
        lines.push("state_after:".to_string());
        for fact in &result.state_after {
            lines.push(format!("- {} = {}", fact.key, fact.value));
        }
    }
    if result.completion_status != experience_core::domain::gate::CompletionStatus::Completed {
        for evidence in &result.execution_evidence {
            lines.push(format!("note: {evidence}"));
        }
    }
    lines.join("\n")
}

#[cfg(test)]
pub(crate) fn reset_gate_runtime_for_test() {
    if let Ok(mut guard) = RUNTIME.lock() {
        *guard = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_usage_outcome_layers_success_misfire_invalid() {
        assert_eq!(gate_usage_outcome_for(true, false), None);
        assert_eq!(gate_usage_outcome_for(true, true), None);
        assert_eq!(gate_usage_outcome_for(false, false), Some("misfire"));
        assert_eq!(gate_usage_outcome_for(false, true), Some("invalid"));
    }

    #[test]
    fn m5_usage_fixture_mirrors_real_gate_success() {
        let root = env!("CARGO_MANIFEST_DIR");
        let json = std::fs::read_to_string(format!("{root}/../../fixtures/m5/gate-usage.json"))
            .expect("m5 usage fixture exists");
        let usage: ExperienceUsageStore = serde_json::from_str(&json).unwrap();
        let entry = &usage.entries["create_probe_file"];
        assert_eq!(entry.hits, 1);
        assert_eq!(entry.misfires, 0);
        assert_eq!(entry.invalid_failures, 0);
        assert_eq!(entry.decisions["experience_only"], 1);
        assert_eq!(entry.logs.len(), 1);
        assert_eq!(entry.logs[0].band, "experience_only");
    }
}

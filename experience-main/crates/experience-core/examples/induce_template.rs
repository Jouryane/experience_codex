//! Turn two recorded observations into one `Candidate` template.
//!
//! Usage:
//!   cargo run -p experience-core --example induce_template -- ^
//!       <obs_a.jsonl> <obs_b.jsonl> <store.json>
//!
//! Each observation file is the newline-delimited JSON written by the agent
//! when `EXPERIENCE_TRACE_OUT` is set: one line per model-driven tool call,
//! `{"task": ..., "action": ..., "args": ...}`.
//!
//! The program refuses to invent anything. It reports the induction verdict —
//! `induced <name>` or the specific `InduceReject` label — and never writes a
//! template that failed to prove itself against both observations. Provenance
//! is recorded (`deterministic_two_trace`) and the artifact lands as
//! `Candidate`, so a human qualification step must still activate it.

use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use experience_core::store::ExperienceStore;
use experience_core::shell_translate::canonicalize;
use experience_core::template_induce::induce_into_store;
use experience_core::template_induce::ObservedStep;
use experience_core::template_induce::ObservedTask;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("usage: induce_template <obs_a.jsonl> <obs_b.jsonl> <store.json>");
        std::process::exit(2);
    }
    let obs_a = PathBuf::from(&args[1]);
    let obs_b = PathBuf::from(&args[2]);
    let store_path = PathBuf::from(&args[3]);

    let a = match load_observation(&obs_a) {
        Ok(task) => task,
        Err(error) => {
            println!("REJECT unreadable_observation {}: {error}", obs_a.display());
            std::process::exit(1);
        }
    };
    let b = match load_observation(&obs_b) {
        Ok(task) => task,
        Err(error) => {
            println!("REJECT unreadable_observation {}: {error}", obs_b.display());
            std::process::exit(1);
        }
    };

    // Producer names (`shell`, `apply_patch`) become canonical capabilities
    // before induction; a trace that cannot be translated yields no candidate.
    let a = match canonicalize(&a) {
        Ok(task) => task,
        Err(reject) => {
            println!("REJECT {} ({reject})", reject.label());
            std::process::exit(1);
        }
    };
    let b = match canonicalize(&b) {
        Ok(task) => task,
        Err(reject) => {
            println!("REJECT {} ({reject})", reject.label());
            std::process::exit(1);
        }
    };
    println!("canonical_a_actions {}", a.steps.len());
    println!("canonical_b_actions {}", b.steps.len());

    let mut store = match ExperienceStore::open(&store_path) {
        Ok(store) => store,
        Err(error) => {
            println!("REJECT store_unreadable {error}");
            std::process::exit(1);
        }
    };
    let recorded_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    match induce_into_store(
        &mut store,
        &a,
        &b,
        "deterministic_two_trace",
        None,
        recorded_at,
    ) {
        Ok(name) => {
            if let Err(error) = store.save_to_path(&store_path) {
                println!("REJECT store_write_failed {error}");
                std::process::exit(1);
            }
            let template = store.template(&name).expect("just inserted");
            println!("induced {name}");
            println!("status {}", status_label(template.status));
            println!("parameters {}", template.parameters.len());
            for parameter in &template.parameters {
                println!(
                    "  {} source={} prefix={:?} suffix={:?} kind={}",
                    parameter.name,
                    parameter.source,
                    parameter.prefix,
                    parameter.suffix,
                    parameter.kind
                );
            }
            println!("workflow {} step(s)", template.workflow.len());
            for step in &template.workflow {
                println!("  {} {}", step.action, step.args);
            }
            println!("preconditions {}", template.preconditions.len());
            println!("postconditions {}", template.postconditions.len());
            println!("verification {}", template.verification.len());
        }
        Err(reject) => {
            println!("REJECT {} ({reject})", reject.label());
            std::process::exit(1);
        }
    }
}

/// Assemble one observation file. A file must describe exactly one task: the
/// moment two different requests appear in one file we cannot claim the steps
/// belong to one family, so it is refused instead of merged.
fn load_observation(path: &Path) -> Result<ObservedTask, String> {
    let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    let mut tasks: BTreeMap<String, Vec<ObservedStep>> = BTreeMap::new();
    for (line_number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(line)
            .map_err(|error| format!("line {}: {error}", line_number + 1))?;
        let task = value
            .get("task")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        let action = value
            .get("action")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("line {}: missing action", line_number + 1))?;
        let args = value
            .get("args")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        tasks
            .entry(task)
            .or_default()
            .push(ObservedStep::new(action, args));
    }
    match tasks.len() {
        0 => Err("no observations".to_string()),
        1 => {
            let (task, steps) = tasks.into_iter().next().expect("one entry");
            // The run is only an observation once its outcome was verified;
            // the acceptance script asserts the artifacts before calling here.
            Ok(ObservedTask::new(task, steps, true))
        }
        count => Err(format!("{count} distinct tasks in one observation file")),
    }
}

fn status_label(status: experience_core::domain::experience::ExperienceStatus) -> &'static str {
    use experience_core::domain::experience::ExperienceStatus as Status;
    match status {
        Status::Draft => "draft",
        Status::Candidate => "candidate",
        Status::Validated => "validated",
        Status::Active => "active",
        Status::Decaying => "decaying",
        Status::Disabled => "disabled",
    }
}

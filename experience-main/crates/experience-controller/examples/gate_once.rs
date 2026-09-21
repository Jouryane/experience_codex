//! Run one synchronous Gate call against a real store and print everything.
//!
//! Usage:
//!   gate_once <store.json> <cwd> <tool> <args-json>
//!
//! This is the deterministic probe used when a real-machine run reports a
//! takeover that did not verify: it shows the decision, the per-step evidence
//! and the postcondition facts, with no model in the loop.

use std::path::PathBuf;

use experience_controller::probe::LocalProbe;
use experience_controller::runner::LocalRunner;
use experience_controller::runtime::ExperienceGateRuntime;
use experience_controller::runtime::GateContext;
use experience_core::domain::action::ActionProposal;
use experience_core::domain::gate::GateDecision;
use experience_core::store::ExperienceStore;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 5 {
        eprintln!("usage: gate_once <store.json> <cwd> <tool> <args-json>");
        std::process::exit(2);
    }
    let store_path = PathBuf::from(&args[1]);
    let cwd = PathBuf::from(&args[2]);
    let tool = args[3].clone();
    let proposal_args: serde_json::Value =
        serde_json::from_str(&args[4]).expect("args must be JSON");

    let store = match ExperienceStore::open(&store_path) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("store unreadable: {error}");
            std::process::exit(1);
        }
    };
    let policy = store.global_policy();
    println!("policy exec={:?} allow={:?}", policy.exec.mode, policy.exec.allow);
    let runner = LocalRunner::with_policy(policy);
    let runtime = ExperienceGateRuntime::new(store, Box::new(LocalProbe), Box::new(runner));
    let context = GateContext { cwd };
    let proposal = ActionProposal::new(tool, proposal_args);

    let decision = runtime.decide(&proposal, &context);
    println!("decision {decision:?}");
    match decision {
        GateDecision::Miss => {}
        _ => {
            let result = runtime.execute(&proposal, &context, &decision);
            println!("success {}", result.is_success());
            println!("completion {:?}", result.completion_status);
            println!("verification {:?}", result.verification_status);
            println!("executed {} step(s)", result.executed.len());
            for step in &result.executed {
                println!("  {:<12} {:?}", step.action, step.evidence);
            }
            println!("evidence:");
            for line in &result.execution_evidence {
                println!("  {line}");
            }
            println!("state_after:");
            for fact in &result.state_after {
                println!("  {} = {} ({})", fact.key, fact.value, fact.evidence);
            }
            if let Some(audit) = &result.template_audit {
                println!("template {} {:?} {}", audit.template, audit.bindings, audit.fingerprint);
            }
        }
    }
}

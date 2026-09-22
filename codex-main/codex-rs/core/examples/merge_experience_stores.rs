//! Merge canonical Experience stores (and their usage ledgers) into one.
//!
//! Used to move the experiences produced by the acceptance runs into the
//! rebuilt Codex's own store, without going through `codex experience import`
//! (which only takes a single *Experience* and always lands it as CANDIDATE —
//! it cannot carry templates, pinning, scopes, provenance or the usage
//! records that the report shows).
//!
//! Usage:
//!   merge_experience_stores <target-store.json> <source-store.json>... [--as-candidate] [--replace]
//!
//! Rules:
//!   * identities are names, exactly like the store index;
//!   * an existing target artifact is kept unless --replace is given (conflicts
//!     are reported either way);
//!   * metadata for a newly added name (pin / scope / display name / user
//!     confidence / provenance) travels with it; scope policies are copied
//!     only when the target has none for that scope;
//!   * usage: hits/misfires/invalid add up, decision counters add up, logs and
//!     audit entries are concatenated and de-duplicated, so importing the same
//!     source twice does not inflate anything;
//!   * --as-candidate demotes everything added to CANDIDATE, i.e. inert.

use std::collections::BTreeMap;
use std::path::PathBuf;

use experience_core::domain::experience::Experience;
use experience_core::domain::experience::ExperienceStatus;
use experience_core::domain::template::ExperienceTemplate;
use experience_core::store::ExperienceStore;
use experience_core::store::StoreFile;

const USAGE_FILE: &str = "usage.json";

fn usage_path(store: &std::path::Path) -> PathBuf {
    store
        .parent()
        .map(|parent| parent.join(USAGE_FILE))
        .unwrap_or_else(|| PathBuf::from(USAGE_FILE))
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let as_candidate = take_flag(&mut args, "--as-candidate");
    let replace = take_flag(&mut args, "--replace");
    if args.len() < 2 {
        eprintln!(
            "usage: merge_experience_stores <target-store.json> <source-store.json>... \
             [--as-candidate] [--replace]"
        );
        std::process::exit(2);
    }
    let target_path = PathBuf::from(args.remove(0));
    let mut target = match ExperienceStore::open(&target_path) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("target store unreadable: {error}");
            std::process::exit(1);
        }
    };
    let mut target_usage = UsageLedger::load(&usage_path(&target_path));

    let mut added: Vec<(&'static str, String, String)> = Vec::new();
    let mut skipped: Vec<(String, String)> = Vec::new();
    for source in &args {
        let source_path = PathBuf::from(source);
        let raw = match std::fs::read_to_string(&source_path) {
            Ok(raw) => raw,
            Err(error) => {
                eprintln!("skipping {source}: {error}");
                continue;
            }
        };
        let file: StoreFile = match serde_json::from_str(&raw) {
            Ok(file) => file,
            Err(error) => {
                eprintln!("skipping {source}: not a canonical store ({error})");
                continue;
            }
        };
        println!("source {} : {} experience(s), {} template(s)", source, file.experiences.len(), file.templates.len());
        for incoming in file.experiences.clone() {
            let name = incoming.name.clone();
            let mut incoming = incoming;
            if as_candidate {
                incoming.status = ExperienceStatus::Candidate;
            }
            if target.get(&name).is_some() {
                if !replace {
                    skipped.push(("experience".into(), name));
                    continue;
                }
                if let Err(error) = target.replace_existing(&name, incoming.clone()) {
                    eprintln!("  could not replace {name}: {error}");
                    continue;
                }
            } else if let Err(error) = target.insert(incoming.clone()) {
                eprintln!("  could not insert {name}: {error}");
                continue;
            }
            copy_metadata(&file, &name, &mut target);
            added.push(("experience", name, format!("{:?}", incoming.status)));
        }
        for incoming in file.templates.clone() {
            let name = incoming.name.clone();
            let mut incoming: ExperienceTemplate = incoming;
            if as_candidate {
                incoming.status = ExperienceStatus::Candidate;
            }
            if target.template(&name).is_some() {
                if !replace {
                    skipped.push(("template".into(), name));
                    continue;
                }
                // No replace_existing for templates: drop the old one first.
                let _ = target.remove_template(&name);
            }
            if let Err(error) = target.insert_template(incoming.clone()) {
                eprintln!("  could not insert template {name}: {error}");
                continue;
            }
            copy_metadata(&file, &name, &mut target);
            added.push(("template", name, format!("{:?}", incoming.status)));
        }
        for (scope, policy) in &file.scope_policies {
            if !target.has_policy_for_scope(scope) {
                let _ = target.set_scope_policy(scope, Some(policy.clone()));
            }
        }
        target_usage.merge(&UsageLedger::load(&usage_path(&source_path)));
    }

    if let Err(error) = target.save_to_path(&target_path) {
        eprintln!("could not write target store: {error}");
        std::process::exit(1);
    }
    if let Err(error) = target_usage.save(&usage_path(&target_path)) {
        eprintln!("could not write target usage: {error}");
        std::process::exit(1);
    }

    println!();
    println!("added {} artifact(s) to {}", added.len(), target_path.display());
    for (kind, name, status) in &added {
        println!("  + [{kind:10}] {name}  [{status}]");
    }
    if !skipped.is_empty() {
        println!("kept the target's existing copy of {} artifact(s):", skipped.len());
        for (kind, name) in &skipped {
            println!("  = [{kind:10}] {name}");
        }
    }
    let active: Vec<&str> = target
        .all()
        .iter()
        .filter(|experience| experience.status == ExperienceStatus::Active)
        .map(|experience| experience.name.as_str())
        .collect();
    let active_templates: Vec<&str> = target
        .templates()
        .iter()
        .filter(|template| template.status == ExperienceStatus::Active)
        .map(|template| template.name.as_str())
        .collect();
    println!();
    println!(
        "target now: {} experience(s) ({} active), {} template(s) ({} active)",
        target.len(),
        active.len(),
        target.templates().len(),
        active_templates.len()
    );
    println!("usage entries: {}", target_usage.entries.len());
}

fn take_flag(args: &mut Vec<String>, flag: &str) -> bool {
    let before = args.len();
    args.retain(|argument| argument != flag);
    args.len() != before
}

fn copy_metadata(file: &StoreFile, name: &str, target: &mut ExperienceStore) {
    if file.pinned.iter().any(|pinned| pinned == name) {
        let _ = target.pin(name);
    }
    if let Some(scope) = file.scopes.get(name) {
        let _ = target.set_scope(name, Some(scope));
    }
    if let Some(display) = file.display_names.get(name) {
        let _ = target.set_display_name(name, Some(display));
    }
    if let Some(usage) = file.user_usage.get(name) {
        let _ = target.set_user_usage(name, Some(usage));
    }
    if let Some(confidence) = file.user_confidence.get(name) {
        let _ = target.set_user_confidence(name, Some(*confidence));
    }
    if let Some(origin) = file.candidate_origins.get(name) {
        target.set_candidate_origin(name, origin.clone());
    }
}

/// Minimal view of `usage.json` (the same shape codex-core writes) so this tool
/// stays independent of the crate's internal types.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct UsageLedger {
    #[serde(default)]
    entries: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    audit: Vec<serde_json::Value>,
}

impl UsageLedger {
    fn load(path: &std::path::Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    fn merge(&mut self, other: &UsageLedger) {
        for (name, incoming) in &other.entries {
            match self.entries.get_mut(name) {
                None => {
                    self.entries.insert(name.clone(), incoming.clone());
                }
                Some(existing) => merge_entry(existing, incoming),
            }
        }
        for record in &other.audit {
            if !self.audit.iter().any(|existing| same_audit(existing, record)) {
                self.audit.push(record.clone());
            }
        }
    }

    fn save(&self, path: &std::path::Path) -> Result<(), String> {
        let json = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        std::fs::write(path, json).map_err(|error| error.to_string())
    }
}

fn merge_entry(existing: &mut serde_json::Value, incoming: &serde_json::Value) {
    for counter in ["hits", "misfires", "invalid_failures"] {
        let sum = existing.get(counter).and_then(serde_json::Value::as_u64).unwrap_or(0)
            + incoming.get(counter).and_then(serde_json::Value::as_u64).unwrap_or(0);
        existing[counter] = serde_json::json!(sum);
    }
    if let Some(last) = incoming.get("last_used").and_then(serde_json::Value::as_u64) {
        let current = existing.get("last_used").and_then(serde_json::Value::as_u64).unwrap_or(0);
        existing["last_used"] = serde_json::json!(current.max(last));
    }
    if let Some(incoming_decisions) = incoming.get("decisions").and_then(|value| value.as_object()) {
        let decisions = existing
            .as_object_mut()
            .map(|map| map.entry("decisions").or_insert_with(|| serde_json::json!({})));
        if let Some(decisions) = decisions.and_then(|value| value.as_object_mut()) {
            for (band, count) in incoming_decisions {
                let sum = decisions.get(band).and_then(serde_json::Value::as_u64).unwrap_or(0)
                    + count.as_u64().unwrap_or(0);
                decisions.insert(band.clone(), serde_json::json!(sum));
            }
        }
    }
    if let (Some(existing_logs), Some(incoming_logs)) = (
        existing.as_object_mut().map(|map| map.entry("logs").or_insert_with(|| serde_json::json!([]))),
        incoming.get("logs").and_then(|value| value.as_array()),
    ) {
        if let Some(logs) = existing_logs.as_array_mut() {
            for log in incoming_logs {
                if !logs.iter().any(|existing_log| same_log(existing_log, log)) {
                    logs.push(log.clone());
                }
            }
            // Keep the ledger readable: oldest first.
            logs.sort_by_key(|log| log.get("at").and_then(serde_json::Value::as_u64).unwrap_or(0));
        }
    }
}

/// Two usage records describing the same take-over are the same record.
fn same_log(left: &serde_json::Value, right: &serde_json::Value) -> bool {
    let key = |value: &serde_json::Value| {
        (
            value.get("at").and_then(serde_json::Value::as_u64),
            value.get("band").and_then(serde_json::Value::as_str).map(str::to_string),
            value.get("process").and_then(serde_json::Value::as_str).map(str::to_string),
        )
    };
    key(left) == key(right)
}

fn same_audit(left: &serde_json::Value, right: &serde_json::Value) -> bool {
    let key = |value: &serde_json::Value| {
        (
            value.get("at").and_then(serde_json::Value::as_u64),
            value.get("action").and_then(serde_json::Value::as_str).map(str::to_string),
            value.get("id").and_then(serde_json::Value::as_str).map(str::to_string),
        )
    };
    key(left) == key(right)
}

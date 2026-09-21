//! Minimal self-contained usage/audit log (`usage.json`), ported from
//! codex-main's `experience_management.rs` without the codex-coupled facade.
//!
//! Lives NEXT to `store.json` under the same experience home: it is metadata
//! about experiences, never a second source of truth for their content.

use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

/// Per-experience usage/decision statistics.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageEntry {
    pub last_used: Option<u64>,
    pub hits: u32,
    pub misfires: u32,
    pub invalid_failures: u32,
    /// Decision band counts: "experience_only" / "experience_first" /
    /// "reference" / "delegate" / "abort".
    #[serde(default)]
    pub decisions: BTreeMap<String, u32>,
    /// Reference log: where/how this experience was used.
    #[serde(default)]
    pub logs: Vec<UsageLog>,
}

/// One reference-log entry: in which thread/process/turn the experience was
/// used, in which decision band, and with what effect.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageLog {
    pub at: u64,
    pub band: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    pub process: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

/// One audit record of a management write operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    pub at: u64,
    pub action: String,
    pub id: String,
    pub by: String,
    pub note: String,
}

/// `usage.json`: statistics + audit of the experience subsystem.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExperienceUsageStore {
    #[serde(default)]
    pub entries: BTreeMap<String, UsageEntry>,
    #[serde(default)]
    pub audit: Vec<AuditEntry>,
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl ExperienceUsageStore {
    pub fn load_from_path(path: &Path) -> Self {
        let mut usage = match std::fs::read_to_string(path) {
            Ok(json) => serde_json::from_str(&json).unwrap_or_default(),
            Err(_) => Self::default(),
        };
        usage.path = Some(path.to_path_buf());
        usage
    }

    pub fn save_to_path(&self, path: &Path) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        std::fs::write(path, json)
    }

    pub fn entry_mut(&mut self, id: &str) -> &mut UsageEntry {
        self.entries.entry(id.to_string()).or_default()
    }

    /// Record one usage with full reference context. `outcome` is None when
    /// the experience was referenced but not executed.
    pub fn record_logged(
        &mut self,
        id: &str,
        decision_band: &str,
        outcome: Option<String>,
        thread_id: Option<String>,
        turn_id: Option<String>,
        process: String,
        task: Option<String>,
        now_secs: u64,
    ) {
        let entry = self.entry_mut(id);
        entry.last_used = Some(now_secs);
        entry.hits = entry.hits.saturating_add(1);
        *entry.decisions.entry(decision_band.to_string()).or_default() += 1;
        match outcome {
            Some(ref outcome) if outcome == "misfire" => {
                entry.misfires = entry.misfires.saturating_add(1)
            }
            Some(ref outcome) if outcome == "invalid" => {
                entry.invalid_failures = entry.invalid_failures.saturating_add(1)
            }
            _ => {}
        }
        entry.logs.push(UsageLog {
            at: now_secs,
            band: decision_band.to_string(),
            outcome,
            thread_id,
            turn_id,
            process,
            task,
        });
        const MAX_LOGS: usize = 200;
        if entry.logs.len() > MAX_LOGS {
            entry.logs.drain(0..entry.logs.len() - MAX_LOGS);
        }
    }
}

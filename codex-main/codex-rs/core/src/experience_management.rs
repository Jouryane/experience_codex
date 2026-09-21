//! Public management facade for the experience store (M2-3b).
//!
//! The running agent persists the experience store to
//! `<codex_home>/experience/store.json` (M2-5). This file-based manager is
//! the data source for the management page / `codex experience` command:
//! list, pin (never-forget privilege), unpin, disable.

use std::path::Path;
use std::path::PathBuf;
use std::collections::BTreeMap;

use serde::Deserialize;
use serde::Serialize;

use crate::experience::Experience;
use crate::experience::ExperienceId;
use crate::experience::ExperienceStatus;
use crate::experience::ExperienceLifecycle;
use crate::experience::ExperienceStore;
use crate::experience::LifecycleAction;
use crate::experience::Applicability;
use crate::experience::ExperienceKind;
use crate::experience::ExperienceTrigger;
use crate::experience::ExperienceWorkflow;
use crate::experience::ExperienceWorkflowStep;
use crate::experience::InputScope;
use crate::experience::RiskLevel;

/// One row of the management page.
#[derive(Debug, Clone, PartialEq)]
pub struct ManagedExperience {
    pub id: String,
    pub name: String,
    pub status: String,
    pub confidence: f32,
    pub pinned: bool,
    pub last_used: Option<u64>,
}

/// File-backed manager operating on the same store the agent persists to.
pub struct ExperienceManager {
    path: PathBuf,
    store: ExperienceStore,
}

impl ExperienceManager {
    /// Open (create if needed) the default home-based store:
    /// `<codex_home>/experience/store.json`.
    pub fn open_default() -> anyhow::Result<Self> {
        let codex_home = crate::config::find_codex_home()?;
        let store_dir = codex_home.as_path().join("experience");
        std::fs::create_dir_all(&store_dir)?;
        Ok(Self::open(crate::experience_paths::resolve_store_path(
            codex_home.as_path(),
        )))
    }

    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let store = match ExperienceStore::load_from_path(&path) {
            Ok(store) => store,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => ExperienceStore::new(),
            Err(error) => {
                tracing::warn!(?path, "failed to load experience store: {error}");
                ExperienceStore::new()
            }
        };
        Self { path, store }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn list(&self) -> Vec<ManagedExperience> {
        self.store
            .all()
            .iter()
            .map(|experience| ManagedExperience {
                id: experience.id.0.clone(),
                name: experience.name.clone(),
                status: format!("{:?}", experience.status),
                confidence: experience.confidence,
                pinned: self.store.is_pinned(&experience.id),
                last_used: None,
            })
            .collect()
    }

    /// Grant the never-forget privilege.
    pub fn pin(&mut self, id: &str) -> anyhow::Result<()> {
        if !self.store.pin(&ExperienceId(id.to_string())) {
            anyhow::bail!("experience not found: {id}");
        }
        self.persist();
        Ok(())
    }

    /// Revoke the never-forget privilege.
    pub fn unpin(&mut self, id: &str) -> anyhow::Result<()> {
        if !self.store.unpin(&ExperienceId(id.to_string())) {
            anyhow::bail!("experience not pinned: {id}");
        }
        self.persist();
        Ok(())
    }

    /// Manually disable an experience (regardless of pin).
    pub fn disable(&mut self, id: &str) -> anyhow::Result<()> {
        let next = self
            .store
            .transition(&ExperienceId(id.to_string()), LifecycleAction::Disable)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        self.persist();
        tracing::info!(%id, ?next, "experience disabled by user");
        Ok(())
    }

    fn persist(&self) {
        if let Err(error) = self.store.save_to_path(&self.path) {
            tracing::warn!(path = ?self.path, "failed to persist experience store: {error}");
        }
    }
}

/// Per-experience usage/decision statistics (management channel, §3).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageEntry {
    pub last_used: Option<u64>,
    pub hits: u32,
    pub misfires: u32,
    pub invalid_failures: u32,
    /// decision band counts: "experience_only" / "experience_first" /
    /// "reference" / "delegate" / "abort".
    #[serde(default)]
    pub decisions: BTreeMap<String, u32>,
    /// Reference log: where/how this experience was used (step 2).
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
    /// Template binding audit for this hit (V2): which template bound which
    /// values, with a stable fingerprint of the instantiated instance. Absent
    /// for exact-instance takeovers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_audit: Option<experience_core::domain::gate::TemplateBindingAudit>,
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

/// Outcome of one recorded replay/decision, for usage accounting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageOutcome {
    Success,
    Misfire,
    Invalid,
}

/// `usage.json`: statistics + audit of the experience subsystem. Lives NEXT
/// to `store.json` under the same codex home — it is metadata about
/// experiences, never a second source of truth for their content.
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

    pub fn persist(&self) {
        if let Some(path) = &self.path {
            if let Err(error) = self.save_to_path(path) {
                tracing::warn!(?path, "failed to persist experience usage: {error}");
            }
        }
    }

    pub fn entry_mut(&mut self, id: &str) -> &mut UsageEntry {
        self.entries.entry(id.to_string()).or_default()
    }

    pub fn record(
        &mut self,
        id: &str,
        decision_band: &str,
        outcome: UsageOutcome,
        now_secs: u64,
    ) {
        self.record_logged(
            id,
            decision_band,
            Some(match outcome {
                UsageOutcome::Success => "success".to_string(),
                UsageOutcome::Misfire => "misfire".to_string(),
                UsageOutcome::Invalid => "invalid".to_string(),
            }),
            None,
            None,
            "management".to_string(),
            None,
            now_secs,
        );
    }

    /// Record one usage with full reference context (step 2). `outcome` is
    /// None when the experience was referenced but not executed.
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
        self.record_logged_with_audit(
            id,
            decision_band,
            outcome,
            thread_id,
            turn_id,
            process,
            task,
            None,
            now_secs,
        );
    }

    /// Same as [`record_logged`], but carries the template binding audit for
    /// parameterized takeovers.
    #[allow(clippy::too_many_arguments)]
    pub fn record_logged_with_audit(
        &mut self,
        id: &str,
        decision_band: &str,
        outcome: Option<String>,
        thread_id: Option<String>,
        turn_id: Option<String>,
        process: String,
        task: Option<String>,
        template_audit: Option<experience_core::domain::gate::TemplateBindingAudit>,
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
            template_audit,
        });
        const MAX_LOGS: usize = 200;
        if entry.logs.len() > MAX_LOGS {
            entry.logs.drain(0..entry.logs.len() - MAX_LOGS);
        }
    }

    pub fn audit(&mut self, action: &str, id: &str, by: &str, note: &str, now_secs: u64) {
        self.audit.push(AuditEntry {
            at: now_secs,
            action: action.to_string(),
            id: id.to_string(),
            by: by.to_string(),
            note: note.to_string(),
        });
        const MAX_AUDIT: usize = 500;
        if self.audit.len() > MAX_AUDIT {
            self.audit.drain(0..self.audit.len() - MAX_AUDIT);
        }
    }
}

/// Management domain service (P0). It is a VIEW + OPERATOR over the SAME
/// store.json the agent uses — never a private copy. All writes go through
/// ExperienceStore validation/lifecycle so modified experiences remain fully
/// usable by experience_codex. It holds no locks while idle; every mutating
/// call reloads the latest files first (optimistic, non-exclusive).
///
/// V2: the file that "the SAME store.json" refers to is resolved by
/// `experience_paths::resolve_store_path`, and the *format of that file*
/// selects the writer. A canonical (`schema_version`) store is managed by the
/// canonical backend below; the legacy envelope keeps its legacy writer. The
/// legacy writer can therefore never overwrite a canonical store, which was
/// the concrete data-loss path when both planes pointed at one path.
pub struct ExperienceManagementService {
    store_path: PathBuf,
    usage_path: PathBuf,
    trash_path: PathBuf,
    store: ExperienceStore,
    /// Present when the resolved file is a canonical store. When it is, every
    /// public method is served by the canonical backend and the legacy store
    /// above stays empty and unused.
    canonical: Option<Box<experience_core::store::ExperienceStore>>,
    format: crate::experience_paths::StoreFormat,
    usage: ExperienceUsageStore,
}

impl ExperienceManagementService {
    pub fn open_default() -> anyhow::Result<Self> {
        let codex_home = crate::config::find_codex_home()?;
        let store_dir = codex_home.as_path().join("experience");
        std::fs::create_dir_all(&store_dir)?;
        Ok(Self::open(crate::experience_paths::resolve_store_path(
            codex_home.as_path(),
        )))
    }

    pub fn open(store_path: impl Into<PathBuf>) -> Self {
        let store_path = store_path.into();
        let dir = store_path.parent().unwrap_or_else(|| Path::new(".")).to_path_buf();
        let usage_path = dir.join("usage.json");
        let trash_path = dir.join("trash.json");
        let format = crate::experience_paths::detect_format(&store_path);
        let canonical = if format == crate::experience_paths::StoreFormat::Canonical {
            match experience_core::store::ExperienceStore::open(&store_path) {
                Ok(store) => Some(Box::new(store)),
                Err(error) => {
                    tracing::warn!(
                        ?error,
                        path = ?store_path,
                        "canonical experience store unreadable; management falls back to legacy view"
                    );
                    None
                }
            }
        } else {
            None
        };
        let store = if canonical.is_some() {
            ExperienceStore::new()
        } else {
            load_store(&store_path)
        };
        let usage = ExperienceUsageStore::load_from_path(&usage_path);
        Self {
            store_path,
            usage_path,
            trash_path,
            store,
            canonical,
            format,
            usage,
        }
    }

    /// Which writer owns the resolved file.
    pub fn backend(&self) -> crate::experience_paths::StoreFormat {
        self.format
    }

    /// Reload latest files before a mutating call (never overwrite a store
    /// the running agent just changed).
    fn reload(&mut self) {
        if self.canonical.is_some() {
            if let Ok(store) = experience_core::store::ExperienceStore::open(&self.store_path) {
                self.canonical = Some(Box::new(store));
            }
            self.usage = ExperienceUsageStore::load_from_path(&self.usage_path);
            return;
        }
        self.store = load_store(&self.store_path);
        self.usage = ExperienceUsageStore::load_from_path(&self.usage_path);
    }

    fn persist(&self) {
        if let Some(store) = &self.canonical {
            if let Err(error) = store.save_to_path(&self.store_path) {
                tracing::warn!(path = ?self.store_path, "failed to persist canonical store: {error}");
            }
            self.usage.persist();
            return;
        }
        if let Err(error) = self.store.save_to_path(&self.store_path) {
            tracing::warn!(path = ?self.store_path, "failed to persist experience store: {error}");
        }
        self.usage.persist();
    }

    pub fn list(&mut self) -> Vec<ManagedRow> {
        self.reload();
        if let Some(store) = &self.canonical {
            return canonical_rows(store, &self.usage);
        }
        self.store
            .all()
            .into_iter()
            .map(|experience| {
                let usage = self
                    .usage
                    .entries
                    .get(experience.id.0.as_str())
                    .cloned()
                    .unwrap_or_default();
                ManagedRow {
                    id: experience.id.0.clone(),
                    name: experience.name.clone(),
                    title: experience.title.clone(),
                    note: experience.note.clone(),
                    created_at: experience.created_at,
                    kind: format!("{:?}", experience.kind),
                    status: format!("{:?}", experience.status),
                    confidence: experience.confidence,
                    pinned: self.store.is_pinned(&experience.id),
                    input_scope: format!("{:?}", experience.applicability.input_scope),
                    applicable_objects: experience.applicability.applicable_objects.clone(),
                    parameterized: experience.applicability.parameterized,
                    last_used: usage.last_used,
                    hits: usage.hits,
                    misfires: usage.misfires,
                    invalid_failures: usage.invalid_failures,
                }
            })
            .collect()
    }

    pub fn detail(&mut self, id: &str) -> anyhow::Result<ExperienceDetail> {
        self.reload();
        if let Some(store) = &self.canonical {
            return canonical_detail(store, &self.usage, id);
        }
        let experience = self
            .store
            .get(&ExperienceId(id.to_string()))
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("experience not found: {id}"))?;
        let usage = self
            .usage
            .entries
            .get(id)
            .cloned()
            .unwrap_or_default();
        let audits = self
            .usage
            .audit
            .iter()
            .filter(|audit| audit.id == id)
            .cloned()
            .collect();
        Ok(ExperienceDetail {
            experience,
            pinned: self.store.is_pinned(&ExperienceId(id.to_string())),
            usage,
            audits,
        })
    }

    pub fn pin(&mut self, id: &str, by: &str) -> anyhow::Result<()> {
        self.reload();
        if let Some(store) = &mut self.canonical {
            return canonical_pin(store, id, by, &mut self.usage, &self.store_path);
        }
        if !self.store.pin(&ExperienceId(id.to_string())) {
            anyhow::bail!("experience not found: {id}");
        }
        self.usage.audit("pin", id, by, "never-forget privilege granted", now_secs());
        self.persist();
        Ok(())
    }

    pub fn unpin(&mut self, id: &str, by: &str) -> anyhow::Result<()> {
        self.reload();
        if let Some(store) = &mut self.canonical {
            return canonical_unpin(store, id, by, &mut self.usage, &self.store_path);
        }
        if !self.store.unpin(&ExperienceId(id.to_string())) {
            anyhow::bail!("experience not pinned: {id}");
        }
        self.usage.audit("unpin", id, by, "never-forget privilege revoked", now_secs());
        self.persist();
        Ok(())
    }

    fn transition(&mut self, id: &str, action: LifecycleAction, by: &str, note: &str) -> anyhow::Result<ExperienceStatus> {
        self.reload();
        if let Some(store) = &mut self.canonical {
            return canonical_transition(store, id, action, by, note, &mut self.usage, &self.store_path);
        }
        let next = self
            .store
            .transition(&ExperienceId(id.to_string()), action)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        self.usage.audit(&format!("{action:?}"), id, by, note, now_secs());
        self.persist();
        Ok(next)
    }

    pub fn activate(&mut self, id: &str, by: &str) -> anyhow::Result<ExperienceStatus> {
        self.transition(id, LifecycleAction::Activate, by, "user activate")
    }

    pub fn revalidate(&mut self, id: &str, by: &str) -> anyhow::Result<ExperienceStatus> {
        self.transition(id, LifecycleAction::Revalidate, by, "user revalidate")
    }

    pub fn disable(&mut self, id: &str, by: &str) -> anyhow::Result<ExperienceStatus> {
        self.transition(id, LifecycleAction::Disable, by, "user disable")
    }

    pub fn delete(&mut self, id: &str, by: &str) -> anyhow::Result<()> {
        self.reload();
        if self.canonical.is_some() {
            return self.canonical_delete(id, by);
        }
        let experience = self
            .store
            .get(&ExperienceId(id.to_string()))
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("experience not found: {id}"))?;
        // Recoverable: push a copy into trash.json before removing.
        let mut trash: Vec<Experience> = std::fs::read_to_string(&self.trash_path)
            .ok()
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        trash.push(experience.clone());
        let json = serde_json::to_string_pretty(&trash)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        std::fs::write(&self.trash_path, json)?;
        self.store.remove(&experience.id);
        self.usage.audit("delete", id, by, "moved to trash.json (recoverable)", now_secs());
        self.persist();
        Ok(())
    }

    pub fn export_json(&mut self, id: &str) -> anyhow::Result<String> {
        self.reload();
        if let Some(store) = &self.canonical {
            return canonical_export(store, id);
        }
        let experience = self
            .store
            .get(&ExperienceId(id.to_string()))
            .ok_or_else(|| anyhow::anyhow!("experience not found: {id}"))?;
        Ok(serde_json::to_string_pretty(experience)?)
    }

    pub fn import_json(&mut self, json: &str, overwrite: bool, by: &str) -> anyhow::Result<ExperienceId> {
        self.reload();
        if self.canonical.is_some() {
            return self.canonical_import(json, overwrite, by);
        }
        let experience: Experience = serde_json::from_str(json)?;
        if !overwrite && self.store.get(&experience.id).is_some() {
            anyhow::bail!(
                "experience {} already exists; pass overwrite=true to replace",
                experience.id
            );
        }
        self.store
            .upsert_validated(experience.clone())
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        self.usage
            .audit("import", experience.id.0.as_str(), by, "imported via management service", now_secs());
        self.persist();
        Ok(experience.id)
    }

    /// Copy an experience as an editable CANDIDATE draft (new id) without
    /// touching the original — editing never risks the production copy.
    pub fn edit_as_draft(&mut self, id: &str, by: &str) -> anyhow::Result<ExperienceId> {
        self.reload();
        if self.canonical.is_some() {
            anyhow::bail!(
                "edit_as_draft is a legacy-store operation; on a canonical store, \
                 import an edited copy as a Candidate instead ({id})"
            );
        }
        let mut draft = self
            .store
            .get(&ExperienceId(id.to_string()))
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("experience not found: {id}"))?;
        draft.id = ExperienceId(format!(
            "edit-{}-{}",
            draft.id.0,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or(0)
        ));
        draft.version = draft.version.saturating_add(1);
        draft.status = ExperienceStatus::Candidate;
        self.store
            .upsert_validated(draft.clone())
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        self.usage
            .audit("edit_as_draft", id, by, &format!("draft {} created", draft.id), now_secs());
        self.persist();
        Ok(draft.id)
    }

    /// Adopt an edited draft: promote it and disable the replaced original.
    pub fn adopt_draft(
        &mut self,
        draft_id: &str,
        replace_id: &str,
        by: &str,
    ) -> anyhow::Result<ExperienceStatus> {
        self.reload();
        if self.canonical.is_some() {
            anyhow::bail!(
                "adopt_draft is a legacy-store operation; on a canonical store, \
                 activate the imported Candidate instead ({draft_id} -> {replace_id})"
            );
        }
        self.store
            .transition(&ExperienceId(draft_id.to_string()), LifecycleAction::Validate)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        self.store
            .transition(&ExperienceId(draft_id.to_string()), LifecycleAction::Activate)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        if replace_id != draft_id {
            self.store
                .transition(&ExperienceId(replace_id.to_string()), LifecycleAction::Disable)
                .map_err(|error| anyhow::anyhow!("{error}"))?;
        }
        let status = self
            .store
            .get(&ExperienceId(draft_id.to_string()))
            .map(|experience| experience.status)
            .unwrap_or(ExperienceStatus::Candidate);
        self.usage.audit(
            "adopt_draft",
            draft_id,
            by,
            &format!("replaced {replace_id}"),
            now_secs(),
        );
        self.persist();
        Ok(status)
    }

    /// Update display metadata: title / note / confidence (direct assignment
    /// with audit; step 3 operations). Empty strings clear title/note;
    /// None leaves the field unchanged. Confidence must stay in [0, 1].
    pub fn update_meta(
        &mut self,
        id: &str,
        title: Option<String>,
        note: Option<String>,
        confidence: Option<f32>,
        by: &str,
    ) -> anyhow::Result<()> {
        self.reload();
        if self.canonical.is_some() {
            return self.canonical_update_meta(id, title, note, confidence, by);
        }
        let mut experience = self
            .store
            .get(&ExperienceId(id.to_string()))
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("experience not found: {id}"))?;
        if let Some(title) = title {
            experience.title = Some(title).filter(|value| !value.trim().is_empty());
        }
        if let Some(note) = note {
            experience.note = Some(note).filter(|value| !value.trim().is_empty());
        }
        if let Some(confidence) = confidence {
            if !(0.0..=1.0).contains(&confidence) {
                anyhow::bail!("confidence must be within [0, 1]");
            }
            experience.confidence = confidence;
        }
        experience.version = experience.version.saturating_add(1);
        self.store
            .upsert_validated(experience)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        self.usage
            .audit("update_meta", id, by, "metadata updated", now_secs());
        self.persist();
        Ok(())
    }

    /// Update how the experience acts on state/runtime: its conditions
    /// (state requirements) and applicability (input scope/objects/
    /// parameterized). Values arrive as JSON for protocol transport.
    pub fn update_control(
        &mut self,
        id: &str,
        applicability: Option<serde_json::Value>,
        conditions: Option<Vec<serde_json::Value>>,
        by: &str,
    ) -> anyhow::Result<()> {
        self.reload();
        if self.canonical.is_some() {
            return self.canonical_update_control(id, applicability, conditions, by);
        }
        let mut experience = self
            .store
            .get(&ExperienceId(id.to_string()))
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("experience not found: {id}"))?;
        if let Some(value) = applicability {
            let parsed: crate::experience::Applicability = serde_json::from_value(value)?;
            experience.applicability = parsed;
        }
        if let Some(conditions) = conditions {
            let parsed: Vec<crate::experience::ExperienceCondition> = conditions
                .into_iter()
                .map(|value| serde_json::from_value(value))
                .collect::<Result<Vec<_>, _>>()?;
            experience.conditions = parsed;
        }
        experience.version = experience.version.saturating_add(1);
        self.store
            .upsert_validated(experience)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        self.usage
            .audit("update_control", id, by, "state/runtime applicability updated", now_secs());
        self.persist();
        Ok(())
    }

    // ---- canonical backend -------------------------------------------------
    //
    // These run when the resolved file carries a `schema_version` envelope,
    // i.e. when the running agent's execution plane reads this very file. The
    // legacy writer is never consulted, so it cannot clobber it.

    fn canonical_delete(&mut self, id: &str, by: &str) -> anyhow::Result<()> {
        let Some(store) = &mut self.canonical else {
            anyhow::bail!("canonical backend not active");
        };
        let removed = if store.get(id).is_some() {
            store.remove(id).map_err(|error| anyhow::anyhow!("{error}"))
        } else {
            store
                .remove_template(id)
                .map_err(|error| anyhow::anyhow!("{error}"))
        };
        removed?;
        self.usage
            .audit("delete", id, by, "removed from the canonical store", now_secs());
        self.persist();
        Ok(())
    }

    fn canonical_import(&mut self, json: &str, overwrite: bool, by: &str) -> anyhow::Result<ExperienceId> {
        let experience: experience_core::domain::experience::Experience = serde_json::from_str(json)?;
        let name = experience.name.clone();
        if !overwrite && self.canonical.as_ref().is_some_and(|store| store.get(&name).is_some()) {
            anyhow::bail!("experience {name} already exists; pass overwrite=true to replace");
        }
        {
            let Some(store) = &mut self.canonical else {
                anyhow::bail!("canonical backend not active");
            };
            if store.get(&name).is_some() {
                store
                    .replace_existing(&name, experience)
                    .map_err(|error| anyhow::anyhow!("{error}"))?;
            } else {
                // Imported artifacts land as CANDIDATE: an outside body never
                // arrives pre-qualified, and validation is what makes it
                // executable.
                let mut candidate = experience;
                candidate.status = experience_core::domain::experience::ExperienceStatus::Candidate;
                store
                    .insert(candidate)
                    .map_err(|error| anyhow::anyhow!("{error}"))?;
            }
        }
        self.usage
            .audit("import", &name, by, "imported into the canonical store as CANDIDATE", now_secs());
        self.persist();
        Ok(ExperienceId(name))
    }

    fn canonical_update_meta(
        &mut self,
        id: &str,
        title: Option<String>,
        note: Option<String>,
        confidence: Option<f32>,
        by: &str,
    ) -> anyhow::Result<()> {
        if let Some(confidence) = confidence {
            if !(0.0..=1.0).contains(&confidence) {
                anyhow::bail!("confidence must be within [0, 1]");
            }
        }
        {
            let Some(store) = &mut self.canonical else {
                anyhow::bail!("canonical backend not active");
            };
            if let Some(title) = title {
                let value = Some(title).filter(|value| !value.trim().is_empty());
                store
                    .set_display_name(id, value.as_deref())
                    .map_err(|error| anyhow::anyhow!("{error}"))?;
            }
            if let Some(confidence) = confidence {
                store
                    .set_user_confidence(id, Some(f64::from(confidence)))
                    .map_err(|error| anyhow::anyhow!("{error}"))?;
            }
            if let Some(note) = note {
                // Canonical identity carries no free-form note; the operator
                // note is preserved in the audit trail instead of being
                // invented as a field.
                if !note.trim().is_empty() {
                    self.usage
                        .audit("update_meta", id, by, &format!("note: {note}"), now_secs());
                }
            }
        }
        self.usage
            .audit("update_meta", id, by, "display metadata updated", now_secs());
        self.persist();
        Ok(())
    }

    fn canonical_update_control(
        &mut self,
        id: &str,
        applicability: Option<serde_json::Value>,
        conditions: Option<Vec<serde_json::Value>>,
        by: &str,
    ) -> anyhow::Result<()> {
        // The canonical model expresses "where this may be used" as a scene
        // scope. Anything else is refused rather than silently dropped.
        let scope = applicability
            .as_ref()
            .and_then(|value| value.get("scope"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        if conditions.is_some() {
            anyhow::bail!(
                "conditions are legacy-only; a canonical experience carries its own preconditions"
            );
        }
        {
            let Some(store) = &mut self.canonical else {
                anyhow::bail!("canonical backend not active");
            };
            if applicability.is_some() && scope.is_none() {
                anyhow::bail!("canonical applicability accepts {{\"scope\": \"...\"}} only");
            }
            store
                .set_scope(id, scope.as_deref())
                .map_err(|error| anyhow::anyhow!("{error}"))?;
        }
        self.usage
            .audit("update_control", id, by, "scene scope updated", now_secs());
        self.persist();
        Ok(())
    }
}

/// Canonical rows: experiences (exact) and templates (parameterized) in one
/// list, because from the operator's side both are one artifact family.
fn canonical_rows(
    store: &experience_core::store::ExperienceStore,
    usage: &ExperienceUsageStore,
) -> Vec<ManagedRow> {
    let entry = |name: &str| {
        usage
            .entries
            .get(name)
            .cloned()
            .unwrap_or_default()
    };
    let mut rows: Vec<ManagedRow> = store
        .all()
        .iter()
        .map(|experience| {
            let name = experience.name.as_str();
            let usage = entry(name);
            ManagedRow {
                id: name.to_string(),
                name: store.display_name_of(name).unwrap_or(name).to_string(),
                title: store.display_name_of(name).map(str::to_string),
                note: experience.trigger.command_pattern.clone(),
                created_at: store.candidate_origin_of(name).map(|origin| origin.recorded_at),
                kind: "Workflow".to_string(),
                status: format!("{:?}", experience.status),
                confidence: store
                    .user_confidence_of(name)
                    .map(|value| value as f32)
                    .unwrap_or(1.0),
                pinned: store.is_pinned(name),
                input_scope: store.scope_of(name).unwrap_or("Global").to_string(),
                applicable_objects: vec![experience.trigger.tool.clone()],
                parameterized: false,
                last_used: usage.last_used,
                hits: usage.hits,
                misfires: usage.misfires,
                invalid_failures: usage.invalid_failures,
            }
        })
        .collect();
    rows.extend(store.templates().iter().map(|template| {
        let name = template.name.as_str();
        let usage = entry(name);
        ManagedRow {
            id: name.to_string(),
            name: store.display_name_of(name).unwrap_or(name).to_string(),
            title: store.display_name_of(name).map(str::to_string),
            note: template.trigger.command_pattern.clone(),
            created_at: store.candidate_origin_of(name).map(|origin| origin.recorded_at),
            kind: "Template".to_string(),
            status: format!("{:?}", template.status),
            confidence: store
                .user_confidence_of(name)
                .map(|value| value as f32)
                .unwrap_or(1.0),
            pinned: store.is_pinned(name),
            input_scope: store.scope_of(name).unwrap_or("Global").to_string(),
            applicable_objects: vec![template.trigger.tool.clone()],
            parameterized: !template.parameters.is_empty(),
            last_used: usage.last_used,
            hits: usage.hits,
            misfires: usage.misfires,
            invalid_failures: usage.invalid_failures,
        }
    }));
    rows
}

fn canonical_detail(
    store: &experience_core::store::ExperienceStore,
    usage: &ExperienceUsageStore,
    id: &str,
) -> anyhow::Result<ExperienceDetail> {
    let projected = if let Some(experience) = store.get(id) {
        canonical_to_legacy(
            store,
            id,
            experience.trigger.tool.as_str(),
            &experience.workflow,
            experience.trigger.command_pattern.as_deref(),
            experience.status,
        )
    } else if let Some(template) = store.template(id) {
        canonical_to_legacy(
            store,
            id,
            template.trigger.tool.as_str(),
            &template.workflow,
            template.trigger.command_pattern.as_deref(),
            template.status,
        )
    } else {
        anyhow::bail!("experience not found: {id}");
    };
    Ok(ExperienceDetail {
        experience: projected,
        pinned: store.is_pinned(id),
        usage: usage.entries.get(id).cloned().unwrap_or_default(),
        audits: usage
            .audit
            .iter()
            .filter(|entry| entry.id == id)
            .cloned()
            .collect(),
    })
}

fn canonical_pin(
    store: &mut experience_core::store::ExperienceStore,
    id: &str,
    by: &str,
    usage: &mut ExperienceUsageStore,
    store_path: &Path,
) -> anyhow::Result<()> {
    store
        .pin(id)
        .map_err(|error| anyhow::anyhow!("experience not found: {id} ({error})"))?;
    usage.audit("pin", id, by, "never-forget privilege granted", now_secs());
    save_canonical(store, usage, store_path);
    Ok(())
}

fn canonical_unpin(
    store: &mut experience_core::store::ExperienceStore,
    id: &str,
    by: &str,
    usage: &mut ExperienceUsageStore,
    store_path: &Path,
) -> anyhow::Result<()> {
    if !store.is_pinned(id) {
        anyhow::bail!("experience not pinned: {id}");
    }
    store
        .unpin(id)
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    usage.audit("unpin", id, by, "never-forget privilege revoked", now_secs());
    save_canonical(store, usage, store_path);
    Ok(())
}

fn canonical_transition(
    store: &mut experience_core::store::ExperienceStore,
    id: &str,
    action: LifecycleAction,
    by: &str,
    note: &str,
    usage: &mut ExperienceUsageStore,
    store_path: &Path,
) -> anyhow::Result<ExperienceStatus> {
    use experience_core::domain::experience::ExperienceStatus as Canonical;
    use experience_core::domain::experience::QualificationAction as Action;

    let current = current_canonical_status(store, id)?;
    let steps: Vec<Action> = match action {
        LifecycleAction::Validate => vec![Action::Validate],
        LifecycleAction::Activate => {
            // Explicit user activation still walks the qualification chain:
            // a CANDIDATE is validated first, and VALIDATED is never skipped.
            match current {
                Canonical::Candidate => vec![Action::Validate, Action::Activate],
                Canonical::Validated => vec![Action::Activate],
                Canonical::Draft => anyhow::bail!(
                    "activate requires a validated candidate; '{id}' is still a draft"
                ),
                Canonical::Active => vec![],
                _ => vec![Action::Activate],
            }
        }
        LifecycleAction::Revalidate => vec![Action::Revalidate],
        LifecycleAction::Disable => vec![Action::Disable],
    };

    let mut next = current;
    for step in steps {
        next = transition_any(store, id, step)?;
    }
    usage.audit(&format!("{action:?}"), id, by, note, now_secs());
    save_canonical(store, usage, store_path);
    Ok(legacy_status(next))
}

fn transition_any(
    store: &mut experience_core::store::ExperienceStore,
    id: &str,
    action: experience_core::domain::experience::QualificationAction,
) -> anyhow::Result<experience_core::domain::experience::ExperienceStatus> {
    if store.get(id).is_some() {
        store
            .transition_status(id, action)
            .map_err(|error| anyhow::anyhow!("{error}"))
    } else {
        store
            .transition_template_status(id, action)
            .map_err(|error| anyhow::anyhow!("{error}"))
    }
}

fn current_canonical_status(
    store: &experience_core::store::ExperienceStore,
    id: &str,
) -> anyhow::Result<experience_core::domain::experience::ExperienceStatus> {
    if let Some(experience) = store.get(id) {
        return Ok(experience.status);
    }
    if let Some(template) = store.template(id) {
        return Ok(template.status);
    }
    anyhow::bail!("experience not found: {id}")
}

fn canonical_export(
    store: &experience_core::store::ExperienceStore,
    id: &str,
) -> anyhow::Result<String> {
    if let Some(experience) = store.get(id) {
        return Ok(serde_json::to_string_pretty(experience)?);
    }
    if let Some(template) = store.template(id) {
        return Ok(serde_json::to_string_pretty(template)?);
    }
    anyhow::bail!("experience not found: {id}")
}

fn canonical_to_legacy(
    store: &experience_core::store::ExperienceStore,
    id: &str,
    tool: &str,
    workflow: &[experience_core::domain::experience::WorkflowStep],
    command_pattern: Option<&str>,
    status: experience_core::domain::experience::ExperienceStatus,
) -> Experience {
    Experience {
        id: ExperienceId(id.to_string()),
        name: store.display_name_of(id).unwrap_or(id).to_string(),
        kind: ExperienceKind::Process,
        trigger: ExperienceTrigger {
            object: Some(tool.to_string()),
            location: None,
            goal: command_pattern.map(str::to_string),
            keywords: Vec::new(),
            context: Default::default(),
        },
        conditions: Vec::new(),
        workflow: ExperienceWorkflow {
            steps: workflow
                .iter()
                .map(|step| ExperienceWorkflowStep {
                    name: step.action.clone(),
                    args: step.args.clone(),
                })
                .collect(),
        },
        // Canonical bodies are verified by predicates; the management view
        // exposes them verbatim instead of inventing a summary.
        completion_criteria: Some(format!(
            "canonical body in {id}; postconditions are verified by the Gate"
        )),
        failure_modes: Vec::new(),
        assets: Default::default(),
        applicability: Applicability {
            input_scope: InputScope::Unknown,
            applicable_objects: vec![tool.to_string()],
            parameterized: store.template(id).is_some(),
        },
        title: store.display_name_of(id).map(str::to_string),
        note: command_pattern.map(str::to_string),
        created_at: store.candidate_origin_of(id).map(|origin| origin.recorded_at),
        confidence: store
            .user_confidence_of(id)
            .map(|value| value as f32)
            .unwrap_or(1.0),
        risk: RiskLevel::Low,
        status: legacy_status(status),
        version: 1,
    }
}

fn legacy_status(
    status: experience_core::domain::experience::ExperienceStatus,
) -> ExperienceStatus {
    use experience_core::domain::experience::ExperienceStatus as Canonical;
    match status {
        Canonical::Draft => ExperienceStatus::New,
        Canonical::Candidate => ExperienceStatus::Candidate,
        Canonical::Validated => ExperienceStatus::Validated,
        Canonical::Active => ExperienceStatus::Active,
        Canonical::Decaying => ExperienceStatus::Decaying,
        Canonical::Disabled => ExperienceStatus::Disabled,
    }
}

fn save_canonical(
    store: &experience_core::store::ExperienceStore,
    usage: &ExperienceUsageStore,
    store_path: &Path,
) {
    if let Err(error) = store.save_to_path(store_path) {
        tracing::warn!(?error, path = ?store_path, "failed to persist canonical store");
    }
    usage.persist();
}

fn load_store(path: &Path) -> ExperienceStore {
    match ExperienceStore::load_from_path(path) {
        Ok(store) => store,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => ExperienceStore::new(),
        Err(error) => {
            tracing::warn!(?path, "failed to load experience store: {error}");
            ExperienceStore::new()
        }
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// One row of the management list view.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedRow {
    pub id: String,
    pub name: String,
    pub title: Option<String>,
    pub note: Option<String>,
    pub created_at: Option<u64>,
    pub kind: String,
    pub status: String,
    pub confidence: f32,
    pub pinned: bool,
    pub input_scope: String,
    pub applicable_objects: Vec<String>,
    pub parameterized: bool,
    pub last_used: Option<u64>,
    pub hits: u32,
    pub misfires: u32,
    pub invalid_failures: u32,
}

/// Full detail view: the experience itself + its usage stats + audit trail.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExperienceDetail {
    pub experience: Experience,
    pub pinned: bool,
    pub usage: UsageEntry,
    pub audits: Vec<AuditEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::experience::Applicability;
    use crate::experience::ExperienceKind;
    use crate::experience::ExperienceTrigger;
    use crate::experience::ExperienceWorkflow;
    use crate::experience::ExperienceWorkflowStep;
    use crate::experience::RiskLevel;

    fn tmp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("exp-mgmt-{tag}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_experience(id: &str) -> Experience {
        Experience {
            id: ExperienceId(id.to_string()),
            name: "classify word docs".to_string(),
            kind: ExperienceKind::Process,
            trigger: ExperienceTrigger {
                keywords: vec!["word".to_string(), "分类".to_string()],
                ..Default::default()
            },
            conditions: Vec::new(),
            workflow: ExperienceWorkflow {
                steps: vec![ExperienceWorkflowStep {
                    name: "exec_command".to_string(),
                    args: serde_json::json!({"cmd": "classify.ps1"}),
                }],
            },
            completion_criteria: Some("all docs classified".to_string()),
            failure_modes: Vec::new(),
            assets: Default::default(),
            applicability: Applicability {
                input_scope: crate::experience::InputScope::Batch,
                applicable_objects: vec!["word".to_string()],
                parameterized: true,
            },
            title: Some("批量分类 Word".to_string()),
            note: None,
            created_at: Some(1234567890),
            confidence: 0.9,
            risk: RiskLevel::Low,
            status: ExperienceStatus::Active,
            version: 1,
        }
    }

    #[test]
    fn usage_store_roundtrips() {
        let dir = tmp_dir("usage");
        let path = dir.join("usage.json");
        let mut usage = ExperienceUsageStore::load_from_path(&path);
        usage.record("e1", "experience_only", UsageOutcome::Success, 100);
        usage.record("e1", "reference", UsageOutcome::Misfire, 200);
        usage.audit("disable", "e1", "test", "note", 300);
        usage.save_to_path(&path).unwrap();
        let loaded = ExperienceUsageStore::load_from_path(&path);
        assert_eq!(loaded.entries["e1"].hits, 2);
        assert_eq!(loaded.entries["e1"].misfires, 1);
        assert_eq!(loaded.entries["e1"].decisions["experience_only"], 1);
        assert_eq!(loaded.entries["e1"].last_used, Some(200));
        assert_eq!(loaded.entries["e1"].logs.len(), 2);
        assert_eq!(loaded.entries["e1"].logs[0].outcome.as_deref(), Some("success"));
        assert_eq!(loaded.entries["e1"].logs[1].band, "reference");
        assert_eq!(loaded.audit.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn service_lifecycle_trash_and_draft_flow() {
        let dir = tmp_dir("service");
        let store_path = dir.join("store.json");
        let mut service = ExperienceManagementService::open(store_path.clone());
        assert!(service.list().is_empty());

        let experience = sample_experience("exp-1");
        let json = serde_json::to_string(&experience).unwrap();
        service.import_json(&json, false, "test").unwrap();
        assert_eq!(service.list().len(), 1);
        let row = &service.list()[0];
        assert_eq!(row.status, "Active");
        assert_eq!(row.input_scope, "Batch");

        service.pin("exp-1", "test").unwrap();
        service.disable("exp-1", "test").unwrap();
        assert_eq!(service.list()[0].status, "Disabled");

        let draft_id = service.edit_as_draft("exp-1", "test").unwrap();
        assert_ne!(draft_id.0, "exp-1");
        let detail = service.detail(draft_id.0.as_str()).unwrap();
        assert_eq!(detail.experience.status, ExperienceStatus::Candidate);
        assert_eq!(detail.experience.version, 2);

        service.adopt_draft(draft_id.0.as_str(), "exp-1", "test").unwrap();
        let adopted = service.detail(draft_id.0.as_str()).unwrap();
        assert_eq!(adopted.experience.status, ExperienceStatus::Active);
        let replaced = service.detail("exp-1").unwrap();
        assert_eq!(replaced.experience.status, ExperienceStatus::Disabled);

        service.delete(draft_id.0.as_str(), "test").unwrap();
        assert!(service.detail(draft_id.0.as_str()).is_err());
        let trash_path = dir.join("trash.json");
        let trash: Vec<Experience> =
            serde_json::from_str(&std::fs::read_to_string(&trash_path).unwrap()).unwrap();
        assert_eq!(trash.len(), 1);
        assert_eq!(trash[0].id.0, draft_id.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn service_updates_meta_and_control() {
        let dir = tmp_dir("meta");
        let store_path = dir.join("store.json");
        let mut service = ExperienceManagementService::open(store_path.clone());
        let experience = sample_experience("exp-meta");
        let json = serde_json::to_string(&experience).unwrap();
        service.import_json(&json, false, "test").unwrap();

        service
            .update_meta("exp-meta", Some("短标题".to_string()), Some("备注".to_string()), Some(0.8), "test")
            .unwrap();
        let detail = service.detail("exp-meta").unwrap();
        assert_eq!(detail.experience.title.as_deref(), Some("短标题"));
        assert_eq!(detail.experience.note.as_deref(), Some("备注"));
        assert_eq!(detail.experience.confidence, 0.8);
        assert_eq!(detail.experience.version, 2);
        assert!(service
            .update_meta("exp-meta", None, None, Some(1.5), "test")
            .is_err());

        service
            .update_control(
                "exp-meta",
                Some(serde_json::json!({
                    "input_scope": "any",
                    "applicable_objects": ["word"],
                    "parameterized": true
                })),
                None,
                "test",
            )
            .unwrap();
        let detail = service.detail("exp-meta").unwrap();
        assert_eq!(
            format!("{:?}", detail.experience.applicability.input_scope),
            "Any"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

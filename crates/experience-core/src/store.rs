//! P1 Store: file persistence for compiled Experiences (M2).
//!
//! File format is a versioned envelope:
//!
//! ```json
//! {
//!   "schema_version": 1,
//!   "experiences": [ { ...Experience... } ]
//! }
//! ```
//!
//! Only ACTIVE experiences are indexed for matching; the index is keyed by
//! trigger tool (the mechanical first filter), keeping the Gate cost near a
//! local hash lookup. Store owns persistence only — usage/audit stay out of
//! this module.

use std::collections::HashMap;
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

use crate::domain::action::ActionProposal;
use crate::domain::experience::QualificationAction;
use crate::domain::experience::Experience;
use crate::domain::experience::ExperienceStatus;
use crate::policy::CapabilityPolicy;
use crate::policy::GLOBAL_SCOPE_KEY;
use crate::domain::experience::SchemaIssue;

/// Current envelope schema version.
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// Stage A reference material: agent-declared methods/notes with provenance.
/// NEVER executable; injected only as "reference, questionable" context.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReferenceEntry {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub steps: Vec<String>,
    #[serde(default)]
    pub tools_used: Vec<String>,
    #[serde(default)]
    pub plugins_used: Vec<String>,
    #[serde(default)]
    pub source_agent: Option<String>,
    /// True when the material is the agent's own declaration.
    #[serde(default)]
    pub declared: bool,
    /// declared | workspace_verified
    #[serde(default)]
    pub trust_level: String,
    #[serde(default)]
    pub evidence_summary: Option<String>,
    pub created_at: u64,
}

/// On-disk envelope.
/// Schema note: additive serde-default envelope fields (e.g. `pinned`) keep
/// schema_version=1 because old files still load and new files load on old
/// readers; breaking format changes bump CURRENT_SCHEMA_VERSION.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoreFile {
    pub schema_version: u32,
    #[serde(default)]
    pub experiences: Vec<Experience>,
    /// Pinned experience names (pin exempts ACTIVE from automatic decay).
    #[serde(default)]
    pub pinned: Vec<String>,
    /// Stage C1: additive scene/scope assignment (experience name -> scene).
    /// Unscoped experiences stay visible in every scene (backward compat).
    #[serde(default)]
    pub scopes: BTreeMap<String, String>,
    /// C2.1: user-facing alias (display only; internal name stays identity).
    #[serde(default)]
    pub display_names: BTreeMap<String, String>,
    /// C2.2: user usage decision per experience (auto|allow|deny).
    #[serde(default)]
    pub user_usage: BTreeMap<String, String>,
    /// C2.2: user confidence 0..1 (display/sort only; never evidence).
    #[serde(default)]
    pub user_confidence: BTreeMap<String, f64>,
    /// Stage A: reference materials (never executable).
    #[serde(default)]
    pub references: BTreeMap<String, ReferenceEntry>,
    /// Stage S1-a: per-scope capability policies (additive). The reserved key
    /// `__global__` holds the default; missing scopes inherit the default.
    #[serde(default)]
    pub scope_policies: BTreeMap<String, CapabilityPolicy>,
    /// S3.5 provenance: where an auto-learned experience came from. Kept out of
    /// `Experience` so the domain object stays a pure compiled artifact and
    /// hosts can attach their own provenance shapes.
    #[serde(default)]
    pub candidate_origins: BTreeMap<String, CandidateOrigin>,
}

/// Provenance for an auto-learned candidate (declaration-friendly: hosts may
/// extend this struct without touching the domain model).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateOrigin {
    /// Normalized task signature that produced the candidate.
    pub task_signature: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// `deterministic` | `llm` | `reference` | `manual` (free-form label).
    pub distiller: String,
    pub recorded_at: u64,
}

impl Default for StoreFile {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            experiences: Vec::new(),
            pinned: Vec::new(),
            scopes: BTreeMap::new(),
            display_names: BTreeMap::new(),
            user_usage: BTreeMap::new(),
            user_confidence: BTreeMap::new(),
            references: BTreeMap::new(),
            scope_policies: BTreeMap::new(),
            candidate_origins: BTreeMap::new(),
        }
    }
}

/// Errors from the file store.
#[derive(Debug)]
pub enum StoreError {
    Io(io::Error),
    InvalidSchemaVersion { found: u32, current: u32 },
    SchemaInvalid {
        name: String,
        issues: Vec<SchemaIssue>,
    },
    DuplicateName(String),
    NotFound(String),
    Transition(String),
    Serialization(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Io(error) => write!(formatter, "io error: {error}"),
            StoreError::InvalidSchemaVersion { found, current } => write!(
                formatter,
                "unsupported store schema version {found} (current: {current})"
            ),
            StoreError::SchemaInvalid { name, issues } => {
                write!(formatter, "experience '{name}' failed schema validation: {issues:?}")
            }
            StoreError::DuplicateName(name) => {
                write!(formatter, "experience '{name}' already exists")
            }
            StoreError::NotFound(name) => write!(formatter, "experience '{name}' not found"),
            StoreError::Transition(message) => write!(formatter, "illegal transition: {message}"),
            StoreError::Serialization(error) => write!(formatter, "serialization error: {error}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<io::Error> for StoreError {
    fn from(error: io::Error) -> Self {
        StoreError::Io(error)
    }
}

/// File-backed store of compiled Experiences.
#[derive(Debug, Clone)]
pub struct ExperienceStore {
    path: Option<PathBuf>,
    /// Insertion order is preserved for deterministic listing.
    experiences: Vec<Experience>,
    /// Pinned names survive reload and exempt ACTIVE from auto-decay.
    pinned: Vec<String>,
    /// Scene/scope assignment by experience name (additive metadata).
    scopes: HashMap<String, String>,
    /// User-facing alias by experience name.
    display_names: HashMap<String, String>,
    /// User usage decision (auto|allow|deny) by experience name.
    user_usage: HashMap<String, String>,
    /// User confidence 0..1 by experience name.
    user_confidence: HashMap<String, f64>,
    /// Stage A reference materials by id.
    references: BTreeMap<String, ReferenceEntry>,
    /// Stage S1-a: per-scope capability policies (reserved `__global__` key
    /// holds the default; unknown scopes inherit it).
    scope_policies: BTreeMap<String, CapabilityPolicy>,
    /// S3.5 provenance by experience name (auto-learned candidates).
    candidate_origins: BTreeMap<String, CandidateOrigin>,
    /// name -> position in `experiences`.
    by_name: HashMap<String, usize>,
    /// trigger tool -> active experience names (the matching surface index).
    active_by_tool: HashMap<String, Vec<String>>,
}

impl Default for ExperienceStore {
    fn default() -> Self {
        Self {
            path: None,
            experiences: Vec::new(),
            pinned: Vec::new(),
            scopes: HashMap::new(),
            display_names: HashMap::new(),
            user_usage: HashMap::new(),
            user_confidence: HashMap::new(),
            references: BTreeMap::new(),
            scope_policies: BTreeMap::new(),
            candidate_origins: BTreeMap::new(),
            by_name: HashMap::new(),
            active_by_tool: HashMap::new(),
        }
    }
}

impl ExperienceStore {
    /// Open a store backed by `path`. A missing file yields an empty store;
    /// an unreadable/corrupt file yields an error (no silent data loss).
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let path = path.into();
        let store = match fs::read_to_string(&path) {
            Ok(json) => Self::from_json(&json)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(error) => return Err(StoreError::Io(error)),
        };
        Ok(store.with_path(path))
    }

    /// Load from an already-read JSON string (used by tests and embedding).
    pub fn from_json(json: &str) -> Result<Self, StoreError> {
        let file: StoreFile = serde_json::from_str(json)
            .map_err(|error| StoreError::Serialization(error.to_string()))?;
        if file.schema_version != CURRENT_SCHEMA_VERSION {
            return Err(StoreError::InvalidSchemaVersion {
                found: file.schema_version,
                current: CURRENT_SCHEMA_VERSION,
            });
        }
        let mut store = Self::default();
        store.pinned = file.pinned.clone();
        store.scopes = file.scopes.into_iter().collect();
        store.display_names = file.display_names.into_iter().collect();
        store.user_usage = file.user_usage.into_iter().collect();
        store.user_confidence = file.user_confidence.into_iter().collect();
        store.references = file.references;
        store.scope_policies = file.scope_policies;
        store.candidate_origins = file.candidate_origins;
        for experience in file.experiences {
            store.insert(experience)?;
        }
        Ok(store)
    }

    fn with_path(mut self, path: PathBuf) -> Self {
        self.path = Some(path);
        self
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Insert a new experience. Rejects structurally invalid experiences and
    /// duplicate names; ACTIVE experiences enter the matching index.
    pub fn insert(&mut self, experience: Experience) -> Result<(), StoreError> {
        let issues = experience.schema_issues();
        if !issues.is_empty() {
            return Err(StoreError::SchemaInvalid {
                name: experience.name.clone(),
                issues,
            });
        }
        if self.by_name.contains_key(&experience.name) {
            return Err(StoreError::DuplicateName(experience.name.clone()));
        }
        self.by_name
            .insert(experience.name.clone(), self.experiences.len());
        if experience.status == ExperienceStatus::Active {
            self.active_by_tool
                .entry(experience.trigger.tool.clone())
                .or_default()
                .push(experience.name.clone());
        }
        self.experiences.push(experience);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&Experience> {
        self.by_name
            .get(name)
            .map(|index| &self.experiences[*index])
    }

    /// Single transition entry point: status changes go through the domain
    /// transition table and rebuild indexes; never mutate status directly.
    pub fn transition_status(
        &mut self,
        name: &str,
        action: QualificationAction,
    ) -> Result<ExperienceStatus, StoreError> {
        let index = self
            .by_name
            .get(name)
            .copied()
            .ok_or_else(|| StoreError::NotFound(name.to_string()))?;
        let next = self.experiences[index]
            .status
            .transition(action)
            .map_err(StoreError::Transition)?;
        self.experiences[index].status = next;
        self.reindex();
        Ok(next)
    }

    /// Atomic body replacement (C2): identity, lifecycle status, pinned and
    /// scope are preserved; only the executable body is replaced.
    pub fn replace_existing(
        &mut self,
        name: &str,
        mut replacement: Experience,
    ) -> Result<(), StoreError> {
        let index = self
            .by_name
            .get(name)
            .copied()
            .ok_or_else(|| StoreError::NotFound(name.to_string()))?;
        let issues = replacement.schema_issues();
        if !issues.is_empty() {
            return Err(StoreError::SchemaInvalid {
                name: name.to_string(),
                issues,
            });
        }
        let current = self.experiences[index].status;
        replacement.name = name.to_string();
        replacement.status = current;
        self.experiences[index] = replacement;
        self.reindex();
        Ok(())
    }

    pub fn all(&self) -> &[Experience] {
        &self.experiences
    }

    pub fn len(&self) -> usize {
        self.experiences.len()
    }

    pub fn is_empty(&self) -> bool {
        self.experiences.is_empty()
    }

    /// Remove an experience by name and rebuild the indexes.
    pub fn remove(&mut self, name: &str) -> Result<(), StoreError> {
        let index = self
            .by_name
            .remove(name)
            .ok_or_else(|| StoreError::NotFound(name.to_string()))?;
        self.pinned.retain(|pinned| pinned != name);
        self.scopes.remove(name);
        self.display_names.remove(name);
        self.user_usage.remove(name);
        self.user_confidence.remove(name);
        self.candidate_origins.remove(name);
        self.experiences.remove(index);
        self.reindex();
        Ok(())
    }

    /// Pin an experience by name (management face; serialized in the store
    /// envelope so pin survives reload).
    pub fn pin(&mut self, name: &str) -> Result<(), StoreError> {
        if !self.by_name.contains_key(name) {
            return Err(StoreError::NotFound(name.to_string()));
        }
        if !self.pinned.iter().any(|pinned| pinned == name) {
            self.pinned.push(name.to_string());
        }
        Ok(())
    }

    pub fn unpin(&mut self, name: &str) -> Result<(), StoreError> {
        self.pinned.retain(|pinned| pinned != name);
        Ok(())
    }

    pub fn is_pinned(&self, name: &str) -> bool {
        self.pinned.iter().any(|pinned| pinned == name)
    }

    pub fn pinned(&self) -> &[String] {
        &self.pinned
    }

    /// Scene assignment for one experience (None = visible everywhere).
    pub fn scope_of(&self, name: &str) -> Option<&str> {
        self.scopes.get(name).map(String::as_str)
    }

    /// An experience is usable in `scope` when it has no scene or its scene
    /// equals the requested one.
    pub fn is_in_scope(&self, name: &str, scope: Option<&str>) -> bool {
        match scope {
            None => true,
            Some(scope) => self
                .scopes
                .get(name)
                .map(|assigned| assigned == scope)
                .unwrap_or(true),
        }
    }

    /// Set or clear (None) the scene assignment; persists with the envelope.
    pub fn set_scope(&mut self, name: &str, scope: Option<&str>) -> Result<(), StoreError> {
        if !self.by_name.contains_key(name) {
            return Err(StoreError::NotFound(name.to_string()));
        }
        match scope {
            Some(scope) => {
                self.scopes.insert(name.to_string(), scope.to_string());
            }
            None => {
                self.scopes.remove(name);
            }
        }
        Ok(())
    }

    /// ACTIVE experiences usable in `scope` (unscoped ACTIVE included).
    pub fn active_in_scope(&self, scope: Option<&str>) -> Vec<&Experience> {
        self.experiences
            .iter()
            .filter(|experience| {
                experience.status == ExperienceStatus::Active
                    && self.is_in_scope(&experience.name, scope)
            })
            .collect()
    }

    /// Global default capability policy (falls back to built-in defaults when
    /// no policy was ever stored).
    pub fn global_policy(&self) -> CapabilityPolicy {
        self.scope_policies
            .get(GLOBAL_SCOPE_KEY)
            .cloned()
            .unwrap_or_default()
    }

    /// Effective capability policy for `scope`: its own policy file entry, or
    /// the global default. Unscoped calls resolve to the global default.
    pub fn policy_for_scope(&self, scope: Option<&str>) -> CapabilityPolicy {
        match scope {
            Some(scope) => self
                .scope_policies
                .get(scope)
                .cloned()
                .unwrap_or_else(|| self.global_policy()),
            None => self.global_policy(),
        }
    }

    /// Whether an explicit policy exists for `scope` (audit diagnostics).
    pub fn has_policy_for_scope(&self, scope: &str) -> bool {
        self.scope_policies.contains_key(scope)
    }

    /// Stored policies by scope (including the reserved global key).
    pub fn scope_policies(&self) -> &BTreeMap<String, CapabilityPolicy> {
        &self.scope_policies
    }

    /// S3.5: attach provenance to an auto-learned candidate.
    pub fn set_candidate_origin(&mut self, name: &str, origin: CandidateOrigin) {
        self.candidate_origins.insert(name.to_string(), origin);
    }

    /// Provenance for one experience, when recorded.
    pub fn candidate_origin_of(&self, name: &str) -> Option<&CandidateOrigin> {
        self.candidate_origins.get(name)
    }

    /// All recorded provenance (read-only view for hosts/UI).
    pub fn candidate_origins(&self) -> &BTreeMap<String, CandidateOrigin> {
        &self.candidate_origins
    }

    /// Set or clear (None) the policy for one scope; persists with the
    /// envelope. `validate_policy` is enforced by the caller/API.
    pub fn set_scope_policy(
        &mut self,
        scope: &str,
        policy: Option<CapabilityPolicy>,
    ) -> Result<(), StoreError> {
        if scope.trim().is_empty() {
            return Err(StoreError::Serialization("empty policy scope".to_string()));
        }
        match policy {
            Some(policy) => {
                self.scope_policies.insert(scope.to_string(), policy);
            }
            None => {
                self.scope_policies.remove(scope);
            }
        }
        Ok(())
    }

    pub fn display_name_of(&self, name: &str) -> Option<&str> {
        self.display_names.get(name).map(String::as_str)
    }

    pub fn set_display_name(&mut self, name: &str, display_name: Option<&str>) -> Result<(), StoreError> {
        if !self.by_name.contains_key(name) {
            return Err(StoreError::NotFound(name.to_string()));
        }
        match display_name {
            Some(value) => {
                self.display_names.insert(name.to_string(), value.to_string());
            }
            None => {
                self.display_names.remove(name);
            }
        }
        Ok(())
    }

    pub fn user_usage_of(&self, name: &str) -> Option<&str> {
        self.user_usage.get(name).map(String::as_str)
    }

    /// User may exclude an experience from their wakeup face (decision
    /// layer; lifecycle/evidence untouched).
    pub fn is_usage_allowed(&self, name: &str) -> bool {
        self.user_usage.get(name).map(|usage| usage != "deny").unwrap_or(true)
    }

    pub fn set_user_usage(&mut self, name: &str, usage: Option<&str>) -> Result<(), StoreError> {
        if !self.by_name.contains_key(name) {
            return Err(StoreError::NotFound(name.to_string()));
        }
        match usage {
            Some("auto") | Some("allow") | Some("deny") => {
                self.user_usage.insert(name.to_string(), usage.unwrap().to_string());
            }
            Some(_) => {
                return Err(StoreError::Transition(format!(
                    "invalid user usage '{}' (auto|allow|deny)",
                    usage.unwrap()
                )));
            }
            None => {
                self.user_usage.remove(name);
            }
        }
        Ok(())
    }

    pub fn user_confidence_of(&self, name: &str) -> Option<f64> {
        self.user_confidence.get(name).copied()
    }

    pub fn set_user_confidence(&mut self, name: &str, value: Option<f64>) -> Result<(), StoreError> {
        if !self.by_name.contains_key(name) {
            return Err(StoreError::NotFound(name.to_string()));
        }
        match value {
            Some(value) if (0.0..=1.0).contains(&value) => {
                self.user_confidence.insert(name.to_string(), (value * 100.0).round() / 100.0);
            }
            Some(_) => {
                return Err(StoreError::Transition(
                    "user confidence must be within 0.0..=1.0".to_string(),
                ));
            }
            None => {
                self.user_confidence.remove(name);
            }
        }
        Ok(())
    }

    pub fn references(&self) -> impl Iterator<Item = &ReferenceEntry> {
        self.references.values()
    }

    pub fn reference(&self, id: &str) -> Option<&ReferenceEntry> {
        self.references.get(id)
    }

    /// Insert a reference entry (references are not executable, so schema
    /// validation is limited to a non-empty id/title).
    pub fn insert_reference(&mut self, entry: ReferenceEntry) -> Result<(), StoreError> {
        if entry.id.trim().is_empty() || entry.title.trim().is_empty() {
            return Err(StoreError::SchemaInvalid {
                name: entry.id.clone(),
                issues: vec![SchemaIssue::EmptyName],
            });
        }
        if self.references.contains_key(&entry.id) {
            return Err(StoreError::DuplicateName(entry.id));
        }
        self.references.insert(entry.id.clone(), entry);
        Ok(())
    }

    pub fn remove_reference(&mut self, id: &str) -> Result<(), StoreError> {
        self.references
            .remove(id)
            .map(|_| ())
            .ok_or_else(|| StoreError::NotFound(id.to_string()))
    }

    /// References visible in a scope: unscoped entries are visible everywhere.
    pub fn references_in_scope(&self, scope: Option<&str>) -> Vec<&ReferenceEntry> {
        self.references
            .values()
            .filter(|entry| match scope {
                None => true,
                Some(scope) => entry.scope.as_deref().map_or(true, |value| value == scope),
            })
            .collect()
    }

    /// Mechanical candidate filter: ACTIVE experiences whose trigger tool
    /// matches the proposal tool, further narrowed by trigger pattern. The
    /// Gate runs this synchronously; MISS means pass-through.
    pub fn candidates_for(&self, proposal: &ActionProposal) -> Vec<&Experience> {
        let mut candidates = Vec::new();
        let Some(names) = self.active_by_tool.get(&proposal.tool) else {
            return candidates;
        };
        for name in names {
            if let Some(experience) = self.get(name) {
                if experience.trigger.matches(proposal) {
                    candidates.push(experience);
                }
            }
        }
        candidates
    }

    /// Persist to the configured path (no-op when the store is not backed by
    /// a file).
    pub fn save(&self) -> Result<(), StoreError> {
        match &self.path {
            Some(path) => self.save_to_path(path),
            None => Ok(()),
        }
    }

    /// Persist to an explicit path (creates parent directories).
    pub fn save_to_path(&self, path: &Path) -> Result<(), StoreError> {
        let file = StoreFile {
            schema_version: CURRENT_SCHEMA_VERSION,
            experiences: self.experiences.clone(),
            pinned: self.pinned.clone(),
            scopes: self
                .scopes
                .iter()
                .map(|(name, scope)| (name.clone(), scope.clone()))
                .collect(),
            display_names: self
                .display_names
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
            user_usage: self
                .user_usage
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
            user_confidence: self
                .user_confidence
                .iter()
                .map(|(name, value)| (name.clone(), *value))
                .collect(),
            references: self.references.clone(),
            scope_policies: self.scope_policies.clone(),
            candidate_origins: self.candidate_origins.clone(),
        };
        let json = serde_json::to_string_pretty(&file)
            .map_err(|error| StoreError::Serialization(error.to_string()))?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        atomic_write(path, json.as_bytes())
    }

    fn reindex(&mut self) {
        self.by_name.clear();
        self.active_by_tool.clear();
        for (index, experience) in self.experiences.iter().enumerate() {
            self.by_name.insert(experience.name.clone(), index);
            if experience.status == ExperienceStatus::Active {
                self.active_by_tool
                    .entry(experience.trigger.tool.clone())
                    .or_default()
                    .push(experience.name.clone());
            }
        }
    }
}

/// Best-effort atomic file write: write to a sibling temp file, then move it
/// over the destination (removing a pre-existing destination on Windows,
/// where rename does not overwrite).
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "store.json".to_string());
    let temp_path = path.with_file_name(format!(".{file_name}.tmp"));
    if let Err(error) = fs::write(&temp_path, bytes) {
        let _ = fs::remove_file(&temp_path);
        return Err(StoreError::Io(error));
    }
    match fs::rename(&temp_path, path) {
        Ok(()) => Ok(()),
        Err(_) => {
            // Windows: destination exists -> remove then retry once.
            match fs::remove_file(path).and_then(|()| fs::rename(&temp_path, path)) {
                Ok(()) => Ok(()),
                Err(second) => {
                    let _ = fs::remove_file(&temp_path);
                    Err(StoreError::Io(second))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::action::ActionPattern;
    use crate::domain::experience::VerificationStep;
    use crate::domain::experience::WorkflowStep;
    use crate::domain::predicate::Predicate;
    use crate::policy::FS_WRITE_DENY;
    use crate::policy::FS_WRITE_WORKSPACE_ONLY;

    #[test]
    fn transition_status_is_the_only_status_mutation_path() {
        let mut store = ExperienceStore::default();
        let mut experience = super::tests::probe_file_experience();
        experience.name = "qual-cand".into();
        experience.status = ExperienceStatus::Candidate;
        store.insert(experience).unwrap();

        assert_eq!(
            store
                .transition_status("qual-cand", QualificationAction::Validate)
                .unwrap(),
            ExperienceStatus::Validated
        );
        assert_eq!(
            store
                .transition_status("qual-cand", QualificationAction::Activate)
                .unwrap(),
            ExperienceStatus::Active
        );
        assert_eq!(store.get("qual-cand").unwrap().status, ExperienceStatus::Active);

        // Candidate -> ACTIVE is refused even through the store API.
        let mut blocked = super::tests::probe_file_experience();
        blocked.name = "qual-blocked".into();
        blocked.status = ExperienceStatus::Candidate;
        store.insert(blocked).unwrap();
        assert!(store
            .transition_status("qual-blocked", QualificationAction::Activate)
            .is_err());
    }

    pub fn probe_file_experience() -> Experience {
        Experience {
            name: "create_probe_file".into(),
            trigger: ActionPattern {
                tool: "exec_command".into(),
                command_pattern: Some("create probe file".into()),
            },
            preconditions: vec![Predicate::new("cwd.exists", serde_json::json!(true))],
            workflow: vec![WorkflowStep::new(
                "write_file",
                serde_json::json!({
                    "path": "probe.txt",
                    "content": "EXPERIENCE_GATE_SUCCESS"
                }),
            )],
            postconditions: vec![
                Predicate::new("file:probe.txt.exists", serde_json::json!(true)),
                Predicate::new(
                    "file:probe.txt.content",
                    serde_json::json!("EXPERIENCE_GATE_SUCCESS"),
                ),
            ],
            verification: vec![VerificationStep::ReadFile {
                path: "probe.txt".into(),
                expect_content: Some("EXPERIENCE_GATE_SUCCESS".into()),
            }],
            failure_policy: crate::domain::experience::FailurePolicy::StopAndReport,
            undo: crate::domain::experience::UndoPolicy::Unsupported,
            status: ExperienceStatus::Active,
        }
    }

    fn temp_store_path(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("exp-store-{tag}-{nanos}.json"))
    }

    #[test]
    fn insert_rejects_duplicate_and_invalid() {
        let mut store = ExperienceStore::default();
        store.insert(probe_file_experience()).unwrap();
        assert!(matches!(
            store.insert(probe_file_experience()),
            Err(StoreError::DuplicateName(_))
        ));

        let mut invalid = probe_file_experience();
        invalid.name = "".into();
        assert!(matches!(
            store.insert(invalid),
            Err(StoreError::SchemaInvalid { .. })
        ));
    }

    #[test]
    fn active_only_candidates_are_returned_by_mechanical_match() {
        let mut store = ExperienceStore::default();
        let mut disabled = probe_file_experience();
        disabled.name = "disabled_probe".into();
        disabled.status = ExperienceStatus::Disabled;
        store.insert(probe_file_experience()).unwrap();
        store.insert(disabled).unwrap();

        let hit = store.candidates_for(&ActionProposal::new(
            "exec_command",
            serde_json::json!({ "cmd": "please create probe file" }),
        ));
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0].name, "create_probe_file");

        let miss = store.candidates_for(&ActionProposal::new(
            "exec_command",
            serde_json::json!({ "cmd": "dir" }),
        ));
        assert!(miss.is_empty());

        let wrong_tool = store.candidates_for(&ActionProposal::new(
            "write_file",
            serde_json::json!({ "path": "probe.txt" }),
        ));
        assert!(wrong_tool.is_empty());
    }

    #[test]
    fn save_and_open_round_trips_with_versioned_envelope() {
        let path = temp_store_path("roundtrip");
        let mut store = ExperienceStore::default();
        store.insert(probe_file_experience()).unwrap();
        store.pin("create_probe_file").unwrap();
        store.save_to_path(&path).unwrap();

        let loaded = ExperienceStore::open(&path).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded.get("create_probe_file").unwrap().name, "create_probe_file");
        assert!(loaded.is_pinned("create_probe_file"));
        assert_eq!(loaded.pinned(), &["create_probe_file".to_string()]);
        assert_eq!(
            loaded.candidates_for(&ActionProposal::new(
                "exec_command",
                serde_json::json!({ "cmd": "create probe file" }),
            )),
            store.candidates_for(&ActionProposal::new(
                "exec_command",
                serde_json::json!({ "cmd": "create probe file" }),
            ))
        );
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn pin_and_unpin_roundtrip_and_remove_clears() {
        let mut store = ExperienceStore::default();
        store.insert(probe_file_experience()).unwrap();
        assert!(!store.is_pinned("create_probe_file"));
        store.pin("create_probe_file").unwrap();
        assert!(store.is_pinned("create_probe_file"));
        store.pin("missing").unwrap_err();
        store.unpin("create_probe_file").unwrap();
        assert!(!store.is_pinned("create_probe_file"));
        store.pin("create_probe_file").unwrap();
        store.remove("create_probe_file").unwrap();
        assert!(!store.is_pinned("create_probe_file"));
    }

    #[test]
    fn scope_policy_resolves_and_persists_with_global_fallback() {
        let mut store = ExperienceStore::default();
        // No stored policy at all -> built-in conservative default.
        assert_eq!(store.policy_for_scope(Some("scene-a")), CapabilityPolicy::default());

        // Global default applies to scopes without their own entry.
        let mut global = CapabilityPolicy::default();
        global.fs_write = FS_WRITE_DENY.to_string();
        store
            .set_scope_policy(GLOBAL_SCOPE_KEY, Some(global.clone()))
            .unwrap();
        assert_eq!(store.policy_for_scope(None), global);
        assert_eq!(store.policy_for_scope(Some("scene-a")), global);

        // Scope entry overrides the global default for that scope only.
        let mut scoped = global.clone();
        scoped.fs_write = FS_WRITE_WORKSPACE_ONLY.to_string();
        scoped.fs_read.max_bytes = 4_096;
        store
            .set_scope_policy("scene-a", Some(scoped.clone()))
            .unwrap();
        assert_eq!(store.policy_for_scope(Some("scene-a")), scoped);
        assert_eq!(store.policy_for_scope(Some("scene-b")), global);
        assert!(store.has_policy_for_scope("scene-a"));
        assert!(!store.has_policy_for_scope("scene-b"));

        let path = temp_store_path("scope-policy");
        store.save_to_path(&path).unwrap();
        let loaded = ExperienceStore::open(&path).unwrap();
        assert_eq!(loaded.policy_for_scope(Some("scene-a")), scoped);
        assert_eq!(loaded.policy_for_scope(Some("scene-b")), global);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn clearing_a_scope_policy_restores_global_fallback() {
        let mut store = ExperienceStore::default();
        let mut global = CapabilityPolicy::default();
        global.fs_delete = FS_WRITE_WORKSPACE_ONLY.to_string();
        store
            .set_scope_policy(GLOBAL_SCOPE_KEY, Some(global.clone()))
            .unwrap();
        let mut scoped = CapabilityPolicy::default();
        scoped.fs_write = FS_WRITE_DENY.to_string();
        store.set_scope_policy("scene-a", Some(scoped)).unwrap();
        assert!(!store.policy_for_scope(Some("scene-a")).allows("write_file"));

        store.set_scope_policy("scene-a", None).unwrap();
        let resolved = store.policy_for_scope(Some("scene-a"));
        assert!(resolved.allows("write_file"));
        assert_eq!(resolved, global);
        store.set_scope_policy("  ", None).unwrap_err();
    }

    #[test]
    fn scope_assignment_persists_and_filters_active() {
        let mut store = ExperienceStore::default();
        store.insert(probe_file_experience()).unwrap();
        let mut other = probe_file_experience();
        other.name = "other_probe".into();
        store.insert(other).unwrap();
        store.set_scope("create_probe_file", Some("scene-a")).unwrap();
        assert_eq!(store.scope_of("create_probe_file"), Some("scene-a"));
        assert!(store.is_in_scope("other_probe", Some("scene-a")));
        assert!(!store.is_in_scope("create_probe_file", Some("scene-b")));

        let path = temp_store_path("scope");
        store.save_to_path(&path).unwrap();
        let loaded = ExperienceStore::open(&path).unwrap();
        assert_eq!(loaded.scope_of("create_probe_file"), Some("scene-a"));
        let names: Vec<&str> = loaded
            .active_in_scope(Some("scene-a"))
            .iter()
            .map(|experience| experience.name.as_str())
            .collect();
        assert!(names.contains(&"create_probe_file"));
        assert!(names.contains(&"other_probe"));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn replace_existing_preserves_identity_status_scope_and_pin() {
        let mut store = ExperienceStore::default();
        store.insert(probe_file_experience()).unwrap();
        store.pin("create_probe_file").unwrap();
        store.set_scope("create_probe_file", Some("scene-a")).unwrap();
        let mut replacement = probe_file_experience();
        replacement.workflow[0].args = serde_json::json!({
            "path": "replaced.txt",
            "content": "REPLACED"
        });
        replacement.postconditions = vec![Predicate::new(
            "file:replaced.txt.exists",
            serde_json::json!(true),
        )];
        replacement.status = ExperienceStatus::Draft; // must be overwritten
        store.replace_existing("create_probe_file", replacement).unwrap();
        let stored = store.get("create_probe_file").unwrap();
        assert_eq!(stored.status, ExperienceStatus::Active);
        assert_eq!(
            stored.workflow[0]
                .args
                .get("content")
                .and_then(serde_json::Value::as_str),
            Some("REPLACED")
        );
        assert!(store.is_pinned("create_probe_file"));
        assert_eq!(store.scope_of("create_probe_file"), Some("scene-a"));
        assert!(store
            .candidates_for(&ActionProposal::new(
                "exec_command",
                serde_json::json!({ "cmd": "create probe file" }),
            ))
            .iter()
            .any(|experience| experience.name == "create_probe_file"));
    }

    #[test]
    fn user_alias_and_preferences_persist_and_clear() {
        let mut store = ExperienceStore::default();
        store.insert(probe_file_experience()).unwrap();
        store
            .set_display_name("create_probe_file", Some("探针经验"))
            .unwrap();
        store
            .set_user_usage("create_probe_file", Some("deny"))
            .unwrap();
        store.set_user_confidence("create_probe_file", Some(0.876)).unwrap();
        assert_eq!(store.display_name_of("create_probe_file"), Some("探针经验"));
        assert!(!store.is_usage_allowed("create_probe_file"));
        assert_eq!(store.user_confidence_of("create_probe_file"), Some(0.88));
        assert!(store.set_user_confidence("create_probe_file", Some(1.5)).is_err());
        assert!(store.set_user_usage("create_probe_file", Some("banana")).is_err());

        let path = temp_store_path("userpref");
        store.save_to_path(&path).unwrap();
        let mut loaded = ExperienceStore::open(&path).unwrap();
        assert_eq!(loaded.display_name_of("create_probe_file"), Some("探针经验"));
        assert_eq!(loaded.user_usage_of("create_probe_file"), Some("deny"));
        assert_eq!(loaded.user_confidence_of("create_probe_file"), Some(0.88));

        loaded.set_user_usage("create_probe_file", None).unwrap();
        loaded.set_display_name("create_probe_file", None).unwrap();
        loaded.set_user_confidence("create_probe_file", None).unwrap();
        assert!(loaded.user_usage_of("create_probe_file").is_none());
        assert!(loaded.display_name_of("create_probe_file").is_none());
        assert!(loaded.user_confidence_of("create_probe_file").is_none());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn reference_entries_persist_filter_and_reject_duplicates() {
        let mut store = ExperienceStore::default();
        let entry = ReferenceEntry {
            id: "ref_trae_dark".into(),
            title: "Trae dark theme".into(),
            scope: Some("scene-frontend".into()),
            tags: vec!["frontend".into()],
            body: "run-notes".into(),
            steps: vec!["scaffold".into(), "verify".into()],
            tools_used: vec!["terminal".into()],
            plugins_used: vec!["ui-kit".into()],
            source_agent: Some("trae".into()),
            declared: true,
            trust_level: "workspace_verified".into(),
            evidence_summary: Some("git diff --stat".into()),
            created_at: 1,
        };
        store.insert_reference(entry.clone()).unwrap();
        assert!(store.insert_reference(entry.clone()).is_err());
        let mut global = entry.clone();
        global.id = "ref_global".into();
        global.scope = None;
        store.insert_reference(global).unwrap();
        assert_eq!(store.references_in_scope(Some("scene-frontend")).len(), 2);
        assert_eq!(store.references_in_scope(Some("scene-other")).len(), 1);

        let path = temp_store_path("references");
        store.save_to_path(&path).unwrap();
        let mut loaded = ExperienceStore::open(&path).unwrap();
        assert_eq!(loaded.reference("ref_trae_dark").unwrap().title, "Trae dark theme");
        loaded.remove_reference("ref_trae_dark").unwrap();
        assert!(loaded.reference("ref_trae_dark").is_none());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn unsupported_schema_version_is_rejected() {
        let store = StoreFile {
            schema_version: 99,
            experiences: vec![],
            pinned: vec![],
            scopes: BTreeMap::new(),
            display_names: BTreeMap::new(),
            user_usage: BTreeMap::new(),
            user_confidence: BTreeMap::new(),
            references: BTreeMap::new(),
            scope_policies: BTreeMap::new(),
            candidate_origins: BTreeMap::new(),
        };
        let json = serde_json::to_string(&store).unwrap();
        assert!(matches!(
            ExperienceStore::from_json(&json),
            Err(StoreError::InvalidSchemaVersion { .. })
        ));
    }

    #[test]
    fn hand_written_fixture_loads_and_matches() {
        // Guard for experiences/create_probe_file.json: the manual P1
        // experience must always remain loadable and matchable.
        let fixture = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../experiences/create_probe_file.json"
        );
        let experience: Experience =
            serde_json::from_str(&fs::read_to_string(fixture).unwrap()).unwrap();
        assert!(experience.is_schema_valid());

        let mut store = ExperienceStore::default();
        store.insert(experience).unwrap();
        let hit = store.candidates_for(&ActionProposal::new(
            "exec_command",
            serde_json::json!({ "cmd": "create probe file" }),
        ));
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0].name, "create_probe_file");
    }

    #[test]
    fn synthetic_scale_scope_lookup_matches_linear_filter() {
        let mut store = ExperienceStore::default();
        for index in 0..10_000 {
            let mut experience = probe_file_experience();
            experience.name = format!("exp_{index:05}");
            store.insert(experience).unwrap();
            if index % 1000 != 0 {
                let scene = if index % 2 == 0 { "scene-a" } else { "scene-b" };
                store
                    .set_scope(&format!("exp_{index:05}"), Some(scene))
                    .unwrap();
            }
        }
        let mut indexed: Vec<String> = store
            .active_in_scope(Some("scene-a"))
            .iter()
            .map(|experience| experience.name.clone())
            .collect();
        let mut linear: Vec<String> = store
            .all()
            .iter()
            .filter(|experience| {
                experience.status == ExperienceStatus::Active
                    && store.is_in_scope(&experience.name, Some("scene-a"))
            })
            .map(|experience| experience.name.clone())
            .collect();
        indexed.sort();
        linear.sort();
        assert_eq!(indexed, linear);
        assert!(indexed.len() > 4_000);
    }

    #[test]
    #[ignore = "manual scale benchmark: cargo test -p experience-core -- --ignored synthetic_scale_100k"]
    fn synthetic_scale_100k_lookup_reports_ns() {
        let mut store = ExperienceStore::default();
        for index in 0..100_000 {
            let mut experience = probe_file_experience();
            experience.name = format!("exp_{index:06}");
            store.insert(experience).unwrap();
            store
                .set_scope(
                    &format!("exp_{index:06}"),
                    Some(if index % 2 == 0 { "scene-a" } else { "scene-b" }),
                )
                .unwrap();
        }
        let started = std::time::Instant::now();
        let count = store.active_in_scope(Some("scene-a")).len();
        let elapsed = started.elapsed().as_nanos();
        println!("100k active_in_scope hits={count} elapsed_ns={elapsed}");
        assert!(count > 49_000);
    }
}

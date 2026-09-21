//! Phase 4: the experience store — where experiences live across turns.
//!
//! v1 is an in-memory registry with JSON import/export (portable, testable,
//! aligned with Phase 8's serializable schema). A later phase may swap the
//! backend for SQLite (borrowed reference semantics) without changing this
//! API.

use std::collections::BTreeMap;
use std::collections::HashSet;

use super::experience::Experience;
use super::experience::ExperienceStatus;
use super::experience::ExperienceValidationError;
use super::experience_lifecycle::ExperienceLifecycle;
use super::experience_lifecycle::LifecycleAction;
use super::experience_lifecycle::LifecycleError;
use super::experience_state::ExperienceId;

#[derive(Debug, Clone, Default)]
pub struct ExperienceStore {
    experiences: BTreeMap<ExperienceId, Experience>,
    /// User-managed privilege: pinned experiences are exempt from time-based
    /// forgetting (M2-3). Set through the management surface.
    pinned: HashSet<ExperienceId>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ExperienceStoreSnapshot {
    experiences: Vec<Experience>,
    pinned: Vec<ExperienceId>,
}

impl ExperienceStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace by id.
    pub fn upsert(&mut self, experience: Experience) {
        self.experiences.insert(experience.id.clone(), experience);
    }

    /// Validate then insert/replace. Invalid experiences are rejected before
    /// they can pollute the store (Phase 8 enforcement point).
    pub fn upsert_validated(
        &mut self,
        experience: Experience,
    ) -> Result<(), ExperienceValidationError> {
        experience.validate()?;
        self.upsert(experience);
        Ok(())
    }

    pub fn get(&self, id: &ExperienceId) -> Option<&Experience> {
        self.experiences.get(id)
    }

    pub fn remove(&mut self, id: &ExperienceId) -> bool {
        self.experiences.remove(id).is_some()
    }

    /// Apply an explicit lifecycle transition to a stored experience.
    pub fn transition(
        &mut self,
        id: &ExperienceId,
        action: LifecycleAction,
    ) -> Result<ExperienceStatus, LifecycleError> {
        let Some(experience) = self.experiences.get_mut(id) else {
            return Err(LifecycleError::NotFound);
        };
        let next = ExperienceLifecycle::transition(experience.status, action)?;
        experience.status = next;
        Ok(next)
    }

    /// Grant / revoke the "never forgotten" privilege. Returns whether the id
    /// exists (pin) or was pinned (unpin).
    pub fn pin(&mut self, id: &ExperienceId) -> bool {
        if self.experiences.contains_key(id) {
            self.pinned.insert(id.clone());
            true
        } else {
            false
        }
    }

    pub fn unpin(&mut self, id: &ExperienceId) -> bool {
        self.pinned.remove(id)
    }

    pub fn is_pinned(&self, id: &ExperienceId) -> bool {
        self.pinned.contains(id)
    }

    pub fn pinned_ids(&self) -> Vec<ExperienceId> {
        self.pinned.iter().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.experiences.len()
    }

    pub fn is_empty(&self) -> bool {
        self.experiences.is_empty()
    }

    pub fn all(&self) -> Vec<&Experience> {
        self.experiences.values().collect()
    }

    /// Experiences allowed to participate in matching
    /// (lifecycle: VALIDATED / ACTIVE).
    pub fn matchable(&self) -> Vec<&Experience> {
        self.experiences
            .values()
            .filter(|experience| {
                matches!(
                    experience.status,
                    ExperienceStatus::Validated | ExperienceStatus::Active
                )
            })
            .collect()
    }

    /// Counts by lifecycle status (for dashboards / audit).
    pub fn stats(&self) -> BTreeMap<ExperienceStatus, usize> {
        let mut counts = BTreeMap::new();
        for experience in self.experiences.values() {
            *counts.entry(experience.status).or_insert(0) += 1;
        }
        counts
    }

    /// Portable export (Phase 8: experiences are executable, serializable
    /// structures, not memory text).
    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(&ExperienceStoreSnapshot {
            experiences: self.all().into_iter().cloned().collect(),
            pinned: self.pinned_ids(),
        })
    }

    pub fn from_json(json: &str) -> serde_json::Result<Self> {
        let snapshot: ExperienceStoreSnapshot = serde_json::from_str(json)?;
        let mut store = Self::new();
        for experience in snapshot.experiences {
            store.upsert(experience);
        }
        store.pinned = snapshot.pinned.into_iter().collect();
        Ok(store)
    }

    /// Persist the store (experiences + pinned privileges) as a JSON file.
    pub fn save_to_path(&self, path: &std::path::Path) -> std::io::Result<()> {
        let json = self
            .to_json()
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        std::fs::write(path, json)
    }

    pub fn load_from_path(path: &std::path::Path) -> std::io::Result<Self> {
        let json = std::fs::read_to_string(path)?;
        Self::from_json(&json)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn experience(id: &str, status: ExperienceStatus) -> Experience {
        Experience {
            id: ExperienceId(id.to_string()),
            name: id.to_string(),
            kind: super::super::experience::ExperienceKind::Reflex,
            trigger: super::super::experience::ExperienceTrigger {
                keywords: vec![id.to_string()],
                ..Default::default()
            },
            conditions: Vec::new(),
            workflow: super::super::experience::ExperienceWorkflow {
                steps: vec![super::super::experience::ExperienceWorkflowStep {
                    name: "scan".to_string(),
                    args: serde_json::json!({}),
                }],
            },
            completion_criteria: None,
            failure_modes: Vec::new(),
            assets: Default::default(),
            applicability: Default::default(),
            title: None,
            note: None,
            created_at: None,
            confidence: 0.9,
            risk: super::super::experience_state::RiskLevel::Low,
            status,
            version: 1,
        }
    }

    #[test]
    fn upsert_get_remove() {
        let mut store = ExperienceStore::new();
        store.upsert(experience("e1", ExperienceStatus::Active));
        assert_eq!(store.len(), 1);
        assert!(store.get(&ExperienceId("e1".to_string())).is_some());
        assert!(store.remove(&ExperienceId("e1".to_string())));
        assert!(store.is_empty());
    }

    #[test]
    fn matchable_filters_lifecycle() {
        let mut store = ExperienceStore::new();
        store.upsert(experience("active", ExperienceStatus::Active));
        store.upsert(experience("validated", ExperienceStatus::Validated));
        store.upsert(experience("candidate", ExperienceStatus::Candidate));
        store.upsert(experience("disabled", ExperienceStatus::Disabled));
        assert_eq!(store.matchable().len(), 2);
        assert_eq!(store.stats()[&ExperienceStatus::Active], 1);
        assert_eq!(store.stats()[&ExperienceStatus::Candidate], 1);
    }

    #[test]
    fn json_round_trip_preserves_experiences() {
        let mut store = ExperienceStore::new();
        store.upsert(experience("e1", ExperienceStatus::Active));
        store.upsert(experience("e2", ExperienceStatus::Candidate));
        let json = store.to_json().expect("serialize");
        let restored = ExperienceStore::from_json(&json).expect("deserialize");
        assert_eq!(restored.len(), 2);
        assert_eq!(restored.stats(), store.stats());
    }

    #[test]
    fn transition_updates_stored_status() {
        let mut store = ExperienceStore::new();
        store.upsert(experience("e1", ExperienceStatus::Candidate));
        let id = ExperienceId("e1".to_string());
        assert_eq!(
            store.transition(&id, LifecycleAction::Validate),
            Ok(ExperienceStatus::Validated)
        );
        assert_eq!(
            store.transition(&id, LifecycleAction::Activate),
            Ok(ExperienceStatus::Active)
        );
        assert_eq!(
            store.transition(&id, LifecycleAction::Disable),
            Ok(ExperienceStatus::Disabled)
        );
        assert_eq!(
            store.transition(&id, LifecycleAction::Revalidate),
            Ok(ExperienceStatus::Validated)
        );

        // Genuinely illegal pair: CANDIDATE + Activate.
        let mut fresh = ExperienceStore::new();
        fresh.upsert(experience("e2", ExperienceStatus::Candidate));
        assert!(matches!(
            fresh.transition(&ExperienceId("e2".to_string()), LifecycleAction::Activate),
            Err(LifecycleError::IllegalTransition { .. })
        ));
        assert!(matches!(
            store.transition(&ExperienceId("missing".to_string()), LifecycleAction::Validate),
            Err(LifecycleError::NotFound)
        ));
    }

    #[test]
    fn upsert_validated_rejects_broken_experiences() {
        let mut store = ExperienceStore::new();
        let mut broken = experience("e1", ExperienceStatus::Candidate);
        broken.workflow = Default::default();
        assert!(store.upsert_validated(broken).is_err());
        assert!(store.is_empty());

        store
            .upsert_validated(experience("ok", ExperienceStatus::Active))
            .expect("valid experience");
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn pin_privilege_management() {
        let mut store = ExperienceStore::new();
        store.upsert(experience("e1", ExperienceStatus::Active));
        assert!(store.pin(&ExperienceId("e1".to_string())));
        assert!(!store.pin(&ExperienceId("missing".to_string())));
        assert!(store.is_pinned(&ExperienceId("e1".to_string())));
        assert_eq!(store.pinned_ids(), vec![ExperienceId("e1".to_string())]);
        assert!(store.unpin(&ExperienceId("e1".to_string())));
        assert!(!store.is_pinned(&ExperienceId("e1".to_string())));
    }

    #[test]
    fn json_round_trip_preserves_pinned() {
        let mut store = ExperienceStore::new();
        store.upsert(experience("e1", ExperienceStatus::Active));
        store.pin(&ExperienceId("e1".to_string()));
        let json = store.to_json().expect("serialize");
        let restored = ExperienceStore::from_json(&json).expect("deserialize");
        assert!(restored.is_pinned(&ExperienceId("e1".to_string())));
        assert_eq!(restored.len(), 1);
    }

    #[test]
    fn save_and_load_file_round_trip() {
        let mut store = ExperienceStore::new();
        store.upsert(experience("e1", ExperienceStatus::Active));
        store.pin(&ExperienceId("e1".to_string()));
        let path = std::env::temp_dir().join(format!("expcheck-store-{}.json", std::process::id()));
        store.save_to_path(&path).expect("save");
        let loaded = ExperienceStore::load_from_path(&path).expect("load");
        let _ = std::fs::remove_file(&path);
        assert!(loaded.is_pinned(&ExperienceId("e1".to_string())));
        assert_eq!(loaded.len(), 1);
    }
}

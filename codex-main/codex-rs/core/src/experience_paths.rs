//! One place that decides where the Experience store lives.
//!
//! Before this module there were two independent answers: the Gate resolved
//! `EXPERIENCE_GATE_STORE` (and refused to run without it), while the
//! management surface always used `<codex_home>/experience/store.json`. Two
//! answers means two stores, which means the thing the operator pins is not
//! the thing the agent executes. Both sides now call [`resolve_store_path`].
//!
//! The env var keeps its meaning as an *override* (experiments, fixtures,
//! per-project stores). It is no longer a prerequisite for the Gate.

use std::path::Path;
use std::path::PathBuf;

/// Env override, shared by the execution and management planes.
pub const STORE_PATH_ENV: &str = "EXPERIENCE_GATE_STORE";

/// The single resolved location of the Experience store.
///
/// Order: explicit env override, else `<codex_home>/experience/store.json`.
pub fn resolve_store_path(codex_home: &Path) -> PathBuf {
    if let Some(override_path) = std::env::var_os(STORE_PATH_ENV) {
        if !override_path.is_empty() {
            return PathBuf::from(override_path);
        }
    }
    codex_home.join("experience").join("store.json")
}

/// On-disk format of a store file. The format is read from the file, never
/// assumed: two writers with different envelopes shared one path before, and
/// that is exactly how a canonical store could be clobbered by the legacy
/// writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreFormat {
    /// File does not exist yet.
    Missing,
    /// `schema_version` envelope owned by `experience-core`.
    Canonical,
    /// Pre-`schema_version` envelope owned by the embedded legacy subsystem.
    Legacy,
    /// Present but not parseable as either.
    Unknown,
}

impl StoreFormat {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Canonical => "canonical",
            Self::Legacy => "legacy",
            Self::Unknown => "unknown",
        }
    }
}

/// Detect the format of `path` by reading it. Never mutates anything.
pub fn detect_format(path: &Path) -> StoreFormat {
    let Ok(json) = std::fs::read_to_string(path) else {
        return if path.exists() {
            StoreFormat::Unknown
        } else {
            StoreFormat::Missing
        };
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) else {
        return StoreFormat::Unknown;
    };
    if value.get("schema_version").is_some() {
        return StoreFormat::Canonical;
    }
    if value.get("experiences").is_some() || value.get("pinned").is_some() {
        return StoreFormat::Legacy;
    }
    StoreFormat::Unknown
}

/// Consistency report for the store plane (surfaced by `codex experience
/// doctor`). `drift` is the important field: it is set when an override store
/// is active while the home store still exists, i.e. exactly the situation
/// that used to make the management page and the agent disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreDoctor {
    pub path: PathBuf,
    pub format: StoreFormat,
    pub experiences: usize,
    pub templates: usize,
    pub pinned: usize,
    pub usage_entries: usize,
    /// The home-based path, whether or not it is in use.
    pub home_path: PathBuf,
    pub home_format: StoreFormat,
    pub override_active: bool,
    pub drift: Option<String>,
}

impl StoreDoctor {
    pub fn is_healthy(&self) -> bool {
        self.drift.is_none() && self.format != StoreFormat::Unknown
    }
}

/// Inspect the resolved store and report what is actually there.
pub fn doctor(codex_home: &Path) -> StoreDoctor {
    let home_path = codex_home.join("experience").join("store.json");
    doctor_for_store(&resolve_store_path(codex_home), &home_path)
}

/// `doctor` with the two paths supplied explicitly.
///
/// The env override is process-global, so anything that must be *tested*
/// rather than *observed* goes through this function: callers that already
/// resolved their paths get a report that cannot be changed under them.
pub fn doctor_for_store(path: &Path, home_path: &Path) -> StoreDoctor {
    let path = path.to_path_buf();
    let home_path = home_path.to_path_buf();
    let override_active = path != home_path;
    let format = detect_format(&path);
    let (experiences, templates, pinned) = match format {
        StoreFormat::Canonical => match experience_core::store::ExperienceStore::open(&path) {
            Ok(store) => (store.len(), store.templates().len(), store.pinned().len()),
            Err(_) => (0, 0, 0),
        },
        _ => (0, 0, 0),
    };
    let usage_entries = read_usage_entries(&path);
    let home_format = detect_format(&home_path);

    let drift = match (override_active, home_format) {
        (true, StoreFormat::Missing) => None,
        (true, _) => Some(format!(
            "override store is active ({}) but a home store also exists ({}, {}); \
             management and execution resolve to the override, so the home store is stale",
            path.display(),
            home_path.display(),
            home_format.label()
        )),
        (false, _) => match format {
            StoreFormat::Legacy => Some(
                "the home store is in the legacy envelope; the execution plane only reads \
                 the canonical envelope, so nothing here can take over"
                    .to_string(),
            ),
            StoreFormat::Unknown => Some("the home store could not be parsed".to_string()),
            _ => None,
        },
    };

    StoreDoctor {
        path,
        format,
        experiences,
        templates,
        pinned,
        usage_entries,
        home_path,
        home_format,
        override_active,
        drift,
    }
}

fn read_usage_entries(store_path: &Path) -> usize {
    let usage_path = store_path
        .parent()
        .map(|parent| parent.join("usage.json"));
    let Some(usage_path) = usage_path else {
        return 0;
    };
    let Ok(json) = std::fs::read_to_string(usage_path) else {
        return 0;
    };
    serde_json::from_str::<serde_json::Value>(&json)
        .ok()
        .and_then(|value| value.get("entries").cloned())
        .and_then(|entries| entries.as_object().map(|map| map.len()))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("exp-paths-{tag}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn detects_each_envelope() {
        let dir = tmp_dir("format");
        let missing = dir.join("none.json");
        assert_eq!(detect_format(&missing), StoreFormat::Missing);

        let canonical = dir.join("canonical.json");
        std::fs::write(&canonical, r#"{"schema_version":1,"experiences":[]}"#).unwrap();
        assert_eq!(detect_format(&canonical), StoreFormat::Canonical);

        let legacy = dir.join("legacy.json");
        std::fs::write(&legacy, r#"{"experiences":[],"pinned":[]}"#).unwrap();
        assert_eq!(detect_format(&legacy), StoreFormat::Legacy);

        let broken = dir.join("broken.json");
        std::fs::write(&broken, "not json").unwrap();
        assert_eq!(detect_format(&broken), StoreFormat::Unknown);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn canonical_doctor_reports_the_home_store_without_override() {
        let dir = tmp_dir("doctor");
        let store_dir = dir.join("experience");
        std::fs::create_dir_all(&store_dir).unwrap();
        let mut store = experience_core::store::ExperienceStore::default();
        store
            .insert_template(experience_core::domain::template::ExperienceTemplate {
                name: "t1".into(),
                trigger: experience_core::domain::action::ActionPattern {
                    tool: "task".into(),
                    command_pattern: Some("do the thing".into()),
                },
                parameters: Vec::new(),
                workflow: vec![experience_core::domain::experience::WorkflowStep::new(
                    "mkdir",
                    serde_json::json!({ "path": "out" }),
                )],
                preconditions: Vec::new(),
                postconditions: vec![experience_core::domain::predicate::Predicate::new(
                    "file:out.exists",
                    serde_json::json!(true),
                )],
                verification: Vec::new(),
                failure_policy: experience_core::domain::experience::FailurePolicy::StopAndReport,
                undo: experience_core::domain::experience::UndoPolicy::Unsupported,
                status: experience_core::domain::experience::ExperienceStatus::Candidate,
            })
            .unwrap();
        store.save_to_path(&store_dir.join("store.json")).unwrap();

        let home_path = store_dir.join("store.json");
        let report = doctor_for_store(&home_path, &home_path);
        assert_eq!(report.format, StoreFormat::Canonical);
        assert_eq!(report.templates, 1);
        assert!(!report.override_active);
        assert!(report.is_healthy(), "{report:?}");
        assert_eq!(report.path, report.home_path);
        assert_eq!(report.home_format, StoreFormat::Canonical);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn doctor_reports_drift_when_an_override_shadows_a_home_store() {
        let dir = tmp_dir("drift");
        let override_path = dir.join("override.json");
        let home_path = dir.join("experience").join("store.json");
        std::fs::create_dir_all(home_path.parent().unwrap()).unwrap();
        std::fs::write(&override_path, r#"{"schema_version":1}"#).unwrap();
        std::fs::write(&home_path, r#"{"schema_version":1}"#).unwrap();

        let report = doctor_for_store(&override_path, &home_path);
        assert!(report.override_active);
        assert_eq!(report.format, StoreFormat::Canonical);
        assert!(!report.is_healthy(), "{report:?}");
        assert!(report.drift.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

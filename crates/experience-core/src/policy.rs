//! Capability policy (Stage S1-a): the single source of truth for what the
//! embedded executor is allowed to do.
//!
//! Model: every capability family carries an explicit permission value.
//! A **scope policy** may be stored per scope; a **session request** may only
//! narrow that policy ([`CapabilityPolicy::narrow`]) — never widen it. Denied
//! work is refused *before* any side effect happens.
//!
//! Defaults are deliberately conservative:
//! - `fs_write = workspace_only` (only action implemented today),
//! - `fs_delete = deny`,
//! - `exec = off`, `network = off`,
//! - `fs_read` is enabled but capped by `max_bytes`; the cap is configurable
//!   and may be raised by an explicit **special grant** (reserved channel for
//!   large-file reads — see [`FsReadPolicy::special_grant_bytes`]).

use serde::Deserialize;
use serde::Serialize;

/// Canonical capability families the executor understands.
pub const POLICY_FAMILIES: &[&str] = &["fs_read", "fs_write", "fs_delete", "exec", "network"];

/// Reserved scope key holding the global default policy in the store envelope.
pub const GLOBAL_SCOPE_KEY: &str = "__global__";

/// `fs_write` permission ladder.
pub const FS_WRITE_WORKSPACE_ONLY: &str = "workspace_only";
pub const FS_WRITE_DENY: &str = "deny";

/// `exec` permission values.
pub const EXEC_OFF: &str = "off";
pub const EXEC_ALLOWLIST: &str = "allowlist";

/// `network` permission values.
pub const NETWORK_OFF: &str = "off";
pub const NETWORK_ALLOWLIST: &str = "allowlist";

/// Default read cap: 20 KiB. Small enough to keep context tidy, large enough
/// for configs/manifests. Raise it per scope, or through `special_grant_bytes`
/// for a one-off exception (S1-b consumes both).
pub const DEFAULT_READ_MAX_BYTES: u64 = 20_480;

/// Largest special-grant value accepted (v1 hard ceiling).
pub const MAX_SPECIAL_GRANT_BYTES: u64 = 8 * 1024 * 1024;

/// `fs_read` policy: enabled + workspace containment + a size cap.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FsReadPolicy {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub workspace_only: bool,
    /// Base cap for reads under this policy.
    #[serde(default = "default_read_max_bytes")]
    pub max_bytes: u64,
    /// Explicit user-granted exception above `max_bytes`. `None` = no grant.
    /// Values are clamped to [`MAX_SPECIAL_GRANT_BYTES`] on validation.
    #[serde(default)]
    pub special_grant_bytes: Option<u64>,
}

impl Default for FsReadPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            workspace_only: true,
            max_bytes: DEFAULT_READ_MAX_BYTES,
            special_grant_bytes: None,
        }
    }
}

impl FsReadPolicy {
    /// Effective cap: the larger of `max_bytes` and an explicit special grant.
    /// Special grants never turn an `enabled = false` policy back on.
    pub fn effective_max_bytes(&self) -> u64 {
        let granted = self
            .special_grant_bytes
            .map(|value| value.min(MAX_SPECIAL_GRANT_BYTES))
            .unwrap_or(0);
        self.max_bytes.min(MAX_SPECIAL_GRANT_BYTES).max(granted)
    }
}

/// `exec` policy: off by default; allowlist mode is reserved for Tier 2.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecPolicy {
    #[serde(default = "default_exec_mode")]
    pub mode: String,
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default = "default_exec_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default = "default_exec_output_cap")]
    pub output_cap: u64,
    /// Legacy `exec_command{cmd}` shell strings. Default deny; Tier 2 only.
    #[serde(default)]
    pub allow_legacy_shell: bool,
}

impl Default for ExecPolicy {
    fn default() -> Self {
        Self {
            mode: EXEC_OFF.to_string(),
            allow: Vec::new(),
            timeout_secs: default_exec_timeout_secs(),
            output_cap: default_exec_output_cap(),
            allow_legacy_shell: false,
        }
    }
}

/// `network` policy: off by default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetworkPolicy {
    #[serde(default = "default_network_mode")]
    pub mode: String,
    #[serde(default)]
    pub allow: Vec<String>,
}

impl Default for NetworkPolicy {
    fn default() -> Self {
        Self {
            mode: NETWORK_OFF.to_string(),
            allow: Vec::new(),
        }
    }
}

/// Effective capability policy for one execution scope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapabilityPolicy {
    #[serde(default)]
    pub fs_read: FsReadPolicy,
    #[serde(default = "default_fs_write")]
    pub fs_write: String,
    #[serde(default = "default_fs_delete")]
    pub fs_delete: String,
    #[serde(default)]
    pub exec: ExecPolicy,
    #[serde(default)]
    pub network: NetworkPolicy,
}

impl Default for CapabilityPolicy {
    fn default() -> Self {
        Self {
            fs_read: FsReadPolicy::default(),
            fs_write: FS_WRITE_WORKSPACE_ONLY.to_string(),
            fs_delete: FS_WRITE_DENY.to_string(),
            exec: ExecPolicy::default(),
            network: NetworkPolicy::default(),
        }
    }
}

impl CapabilityPolicy {
    /// Effective read cap, honoring an explicit special grant.
    pub fn read_max_bytes(&self) -> u64 {
        self.fs_read.effective_max_bytes()
    }

    /// Permission verdict for a workflow action, with a human-readable reason
    /// suitable for ledger/audit (`reason` never contains secrets).
    pub fn check(&self, action: &str) -> Result<(), PolicyDecision> {
        match action {
            "write_file" | "append_file" | "mkdir" | "copy_file" | "move_file" => {
                if self.fs_write == FS_WRITE_DENY {
                    Err(PolicyDecision::new(
                        action,
                        "fs_write",
                        "fs_write=deny",
                    ))
                } else {
                    Ok(())
                }
            }
            "delete_file" => {
                if self.fs_delete != FS_WRITE_WORKSPACE_ONLY {
                    Err(PolicyDecision::new(
                        action,
                        "fs_delete",
                        "fs_delete=deny",
                    ))
                } else {
                    Ok(())
                }
            }
            "read_file" => {
                if self.fs_read.enabled {
                    Ok(())
                } else {
                    Err(PolicyDecision::new(action, "fs_read", "fs_read=disabled"))
                }
            }
            "exec" | "exec_command" => {
                if self.exec.mode == EXEC_ALLOWLIST {
                    Ok(())
                } else {
                    Err(PolicyDecision::new(action, "exec", "exec=off"))
                }
            }
            other => Err(PolicyDecision::new(
                other,
                "unknown",
                "unsupported_action",
            )),
        }
    }

    /// True when the action may run under this policy.
    pub fn allows(&self, action: &str) -> bool {
        self.check(action).is_ok()
    }

    /// Merge a requested (session-level) policy so that it can only *narrow*
    /// `self`. Widening fields are ignored; narrowing fields are applied.
    pub fn narrow(&self, requested: &CapabilityPolicy) -> CapabilityPolicy {
        let mut merged = self.clone();
        if !requested.fs_read.enabled {
            merged.fs_read.enabled = false;
        }
        if !requested.fs_read.workspace_only {
            // A session can never relax workspace containment.
            merged.fs_read.workspace_only = true;
        }
        let requested_read_cap = requested.fs_read.effective_max_bytes();
        if requested_read_cap < merged.read_max_bytes() {
            merged.fs_read.max_bytes = requested_read_cap;
            merged.fs_read.special_grant_bytes = None;
        }
        if requested.fs_write == FS_WRITE_DENY {
            merged.fs_write = FS_WRITE_DENY.to_string();
        }
        if requested.fs_delete != FS_WRITE_WORKSPACE_ONLY {
            merged.fs_delete = FS_WRITE_DENY.to_string();
        }
        if requested.exec.mode != EXEC_ALLOWLIST && merged.exec.mode == EXEC_ALLOWLIST {
            merged.exec = ExecPolicy::default();
        } else if requested.exec.mode == EXEC_ALLOWLIST {
            merged.exec.allow = intersection(&merged.exec.allow, &requested.exec.allow);
            merged.exec.timeout_secs = merged.exec.timeout_secs.min(requested.exec.timeout_secs);
            merged.exec.output_cap = merged.exec.output_cap.min(requested.exec.output_cap);
            merged.exec.allow_legacy_shell =
                merged.exec.allow_legacy_shell && requested.exec.allow_legacy_shell;
        }
        if requested.network.mode != NETWORK_ALLOWLIST && merged.network.mode == NETWORK_ALLOWLIST {
            merged.network = NetworkPolicy::default();
        } else if requested.network.mode == NETWORK_ALLOWLIST {
            merged.network.allow = intersection(&merged.network.allow, &requested.network.allow);
        }
        merged
    }
}

/// One refusal: which action, which family, and why. Kept small so it can be
/// written verbatim into the ledger without redaction risk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PolicyDecision {
    pub action: String,
    pub family: String,
    pub reason: String,
}

impl PolicyDecision {
    pub fn new(action: impl Into<String>, family: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            action: action.into(),
            family: family.into(),
            reason: reason.into(),
        }
    }
}

/// Validate a policy coming from the API before it is persisted. Returns a
/// human-readable problem string on the first issue.
pub fn validate_policy(policy: &CapabilityPolicy) -> Result<(), String> {
    if policy.fs_write != FS_WRITE_WORKSPACE_ONLY && policy.fs_write != FS_WRITE_DENY {
        return Err(format!("invalid fs_write '{}'", policy.fs_write));
    }
    if policy.fs_delete != FS_WRITE_WORKSPACE_ONLY && policy.fs_delete != FS_WRITE_DENY {
        return Err(format!("invalid fs_delete '{}'", policy.fs_delete));
    }
    if policy.exec.mode != EXEC_OFF && policy.exec.mode != EXEC_ALLOWLIST {
        return Err(format!("invalid exec.mode '{}'", policy.exec.mode));
    }
    if policy.exec.mode == EXEC_ALLOWLIST && policy.exec.allow.is_empty() {
        return Err("exec.mode=allowlist requires a non-empty allow list".to_string());
    }
    if policy.exec.timeout_secs == 0 || policy.exec.timeout_secs > 3600 {
        return Err("exec.timeout_secs must be within 1..=3600".to_string());
    }
    if policy.exec.output_cap == 0 || policy.exec.output_cap > 10 * 1024 * 1024 {
        return Err("exec.output_cap must be within 1..=10485760".to_string());
    }
    if policy.network.mode != NETWORK_OFF && policy.network.mode != NETWORK_ALLOWLIST {
        return Err(format!("invalid network.mode '{}'", policy.network.mode));
    }
    if policy.network.mode == NETWORK_ALLOWLIST && policy.network.allow.is_empty() {
        return Err("network.mode=allowlist requires a non-empty allow list".to_string());
    }
    if policy.fs_read.max_bytes == 0 {
        return Err("fs_read.max_bytes must be >= 1".to_string());
    }
    if let Some(grant) = policy.fs_read.special_grant_bytes {
        if grant == 0 || grant > MAX_SPECIAL_GRANT_BYTES {
            return Err(format!(
                "fs_read.special_grant_bytes must be within 1..={MAX_SPECIAL_GRANT_BYTES}"
            ));
        }
        if grant < policy.fs_read.max_bytes {
            return Err("fs_read.special_grant_bytes must be >= fs_read.max_bytes".to_string());
        }
    }
    Ok(())
}

/// True when `requested` is a subset of `base` for every family (used to
/// reject session allowlists that would widen the effective policy).
pub fn request_within(base: &CapabilityPolicy, requested: &CapabilityPolicy) -> bool {
    // Reads: a request may keep or lower the effective cap, never raise it.
    if requested.read_max_bytes() > base.read_max_bytes() {
        return false;
    }
    // Writes: a session can only ask for the base write level or less.
    if requested.fs_write != FS_WRITE_DENY && base.fs_write == FS_WRITE_DENY {
        return false;
    }
    // Deletes: base deny cannot be lifted by a session.
    if requested.fs_delete == FS_WRITE_WORKSPACE_ONLY && base.fs_delete != FS_WRITE_WORKSPACE_ONLY {
        return false;
    }
    // Exec: raising off -> allowlist, or requesting programs the base does not
    // list, is a widening. Base allowlist with no request for exec stays fine.
    if requested.exec.mode == EXEC_ALLOWLIST {
        if base.exec.mode != EXEC_ALLOWLIST {
            return false;
        }
        if !requested
            .exec
            .allow
            .iter()
            .all(|program| base.exec.allow.iter().any(|value| value == program))
        {
            return false;
        }
        if requested.exec.allow_legacy_shell && !base.exec.allow_legacy_shell {
            return false;
        }
        if requested.exec.timeout_secs > base.exec.timeout_secs
            || requested.exec.output_cap > base.exec.output_cap
        {
            return false;
        }
    }
    // Network: same shape as exec.
    if requested.network.mode == NETWORK_ALLOWLIST {
        if base.network.mode != NETWORK_ALLOWLIST {
            return false;
        }
        if !requested
            .network
            .allow
            .iter()
            .all(|host| base.network.allow.iter().any(|value| value == host))
        {
            return false;
        }
    }
    true
}

/// Map a canonical capability name (session `capabilities` list) onto its
/// policy family. Unknown names return `None`.
pub fn capability_family(name: &str) -> Option<&'static str> {
    match name {
        "read_file" => Some("fs_read"),
        "write_file" => Some("fs_write"),
        "delete_file" => Some("fs_delete"),
        "exec_command" | "exec" => Some("exec"),
        "network" => Some("network"),
        _ => None,
    }
}

fn intersection(left: &[String], right: &[String]) -> Vec<String> {
    let mut merged: Vec<String> = left
        .iter()
        .filter(|value| right.iter().any(|other| other == *value))
        .cloned()
        .collect();
    merged.sort();
    merged.dedup();
    merged
}

fn default_true() -> bool {
    true
}

fn default_read_max_bytes() -> u64 {
    DEFAULT_READ_MAX_BYTES
}

fn default_fs_write() -> String {
    FS_WRITE_WORKSPACE_ONLY.to_string()
}

fn default_fs_delete() -> String {
    FS_WRITE_DENY.to_string()
}

fn default_exec_mode() -> String {
    EXEC_OFF.to_string()
}

fn default_exec_timeout_secs() -> u64 {
    60
}

fn default_exec_output_cap() -> u64 {
    20_000
}

fn default_network_mode() -> String {
    NETWORK_OFF.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_conservative_but_keep_write_file_usable() {
        let policy = CapabilityPolicy::default();
        assert!(policy.allows("write_file"));
        assert!(!policy.allows("delete_file"));
        assert!(!policy.allows("exec_command"));
        assert!(!policy.allows("exec"));
        assert!(policy.allows("read_file"));
        assert_eq!(policy.read_max_bytes(), DEFAULT_READ_MAX_BYTES);
        assert_eq!(policy.network.mode, NETWORK_OFF);
    }

    #[test]
    fn unknown_action_is_refused_as_unsupported() {
        let policy = CapabilityPolicy::default();
        let decision = policy.check("mystery_tool").unwrap_err();
        assert_eq!(decision.family, "unknown");
        assert_eq!(decision.reason, "unsupported_action");
    }

    #[test]
    fn denial_reasons_are_stable_strings() {
        let policy = CapabilityPolicy::default();
        assert_eq!(policy.check("delete_file").unwrap_err().reason, "fs_delete=deny");
        assert_eq!(policy.check("exec").unwrap_err().reason, "exec=off");
    }

    #[test]
    fn session_policy_can_only_narrow_never_widen() {
        let base = CapabilityPolicy::default();

        // An empty-ish request (all defaults) changes nothing.
        let same = base.narrow(&CapabilityPolicy::default());
        assert_eq!(same, base);

        // Read-only session: write + delete + exec rejected.
        let read_only = CapabilityPolicy {
            fs_write: FS_WRITE_DENY.to_string(),
            fs_read: FsReadPolicy {
                max_bytes: 5_000,
                ..FsReadPolicy::default()
            },
            ..CapabilityPolicy::default()
        };
        let narrowed = base.narrow(&read_only);
        assert!(!narrowed.allows("write_file"));
        assert!(!narrowed.allows("delete_file"));
        assert!(narrowed.allows("read_file"));
        assert_eq!(narrowed.read_max_bytes(), 5_000);
        assert!(request_within(&base, &read_only));

        // A request that tries to widen must be detected, and can never win.
        let widening = CapabilityPolicy {
            fs_delete: FS_WRITE_WORKSPACE_ONLY.to_string(),
            exec: ExecPolicy {
                mode: EXEC_ALLOWLIST.to_string(),
                allow: vec!["pwsh".to_string()],
                ..ExecPolicy::default()
            },
            ..CapabilityPolicy::default()
        };
        assert!(!request_within(&base, &widening));
        let merged = base.narrow(&widening);
        assert!(!merged.allows("delete_file"));
        assert!(!merged.allows("exec"));
        assert_eq!(merged.fs_write, base.fs_write);
    }

    #[test]
    fn exec_allowlist_intersects_with_base() {
        let base = CapabilityPolicy {
            exec: ExecPolicy {
                mode: EXEC_ALLOWLIST.to_string(),
                allow: vec!["git".to_string(), "pwsh".to_string()],
                timeout_secs: 120,
                output_cap: 40_000,
                allow_legacy_shell: true,
            },
            ..CapabilityPolicy::default()
        };
        let requested = CapabilityPolicy {
            exec: ExecPolicy {
                mode: EXEC_ALLOWLIST.to_string(),
                allow: vec!["pwsh".to_string()],
                timeout_secs: 30,
                output_cap: 80_000,
                allow_legacy_shell: false,
            },
            ..CapabilityPolicy::default()
        };
        let merged = base.narrow(&requested);
        assert_eq!(merged.exec.allow, vec!["pwsh".to_string()]);
        assert_eq!(merged.exec.timeout_secs, 30);
        assert_eq!(merged.exec.output_cap, 40_000);
        assert!(!merged.exec.allow_legacy_shell);
        assert!(merged.allows("exec"));
    }

    #[test]
    fn disabling_exec_in_session_wins_even_when_base_allows_it() {
        let base = CapabilityPolicy {
            exec: ExecPolicy {
                mode: EXEC_ALLOWLIST.to_string(),
                allow: vec!["git".to_string()],
                ..ExecPolicy::default()
            },
            ..CapabilityPolicy::default()
        };
        let merged = base.narrow(&CapabilityPolicy::default());
        assert!(!merged.allows("exec"));
    }

    #[test]
    fn special_grant_raises_read_cap_within_ceiling() {
        let policy = FsReadPolicy {
            max_bytes: DEFAULT_READ_MAX_BYTES,
            special_grant_bytes: Some(1_048_576),
            ..FsReadPolicy::default()
        };
        assert_eq!(policy.effective_max_bytes(), 1_048_576);

        // Over-ceiling grants clamp instead of erroring at use time.
        let clamped = FsReadPolicy {
            special_grant_bytes: Some(MAX_SPECIAL_GRANT_BYTES * 4),
            ..FsReadPolicy::default()
        };
        assert_eq!(clamped.effective_max_bytes(), MAX_SPECIAL_GRANT_BYTES);
    }

    #[test]
    fn validate_rejects_bad_values_and_accepts_a_grant() {
        let mut bad = CapabilityPolicy::default();
        bad.exec.mode = "sure".to_string();
        assert!(validate_policy(&bad).unwrap_err().contains("exec.mode"));

        let mut bad = CapabilityPolicy::default();
        bad.fs_delete = "whatever".to_string();
        assert!(validate_policy(&bad).unwrap_err().contains("fs_delete"));

        let mut bad = CapabilityPolicy::default();
        bad.fs_read.special_grant_bytes = Some(DEFAULT_READ_MAX_BYTES - 1);
        assert!(validate_policy(&bad).unwrap_err().contains("special_grant_bytes"));

        let mut granted = CapabilityPolicy::default();
        granted.fs_read.special_grant_bytes = Some(1_048_576);
        assert!(validate_policy(&granted).is_ok());
    }

    #[test]
    fn capability_family_maps_known_names() {
        assert_eq!(capability_family("write_file"), Some("fs_write"));
        assert_eq!(capability_family("read_file"), Some("fs_read"));
        assert_eq!(capability_family("delete_file"), Some("fs_delete"));
        assert_eq!(capability_family("exec_command"), Some("exec"));
        assert_eq!(capability_family("computer_use"), None);
    }

    #[test]
    fn policy_round_trips_through_json() {
        let policy = CapabilityPolicy::default();
        let json = serde_json::to_string(&policy).unwrap();
        let back: CapabilityPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(back, policy);

        // Legacy/partial JSON still loads with conservative defaults.
        let partial: CapabilityPolicy = serde_json::from_str("{}").unwrap();
        assert_eq!(partial, policy);
    }
}

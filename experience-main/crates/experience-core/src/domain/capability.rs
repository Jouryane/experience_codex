//! Canonical tool capability vocabulary (single source of truth).
//!
//! L1/L2 distill candidates, the Gate proposal vocabulary and the
//! LocalRunner must all resolve through this module so producer names
//! (e.g. the fork app-server's `commandExecution`) never leak into
//! executable bodies.

/// Canonical capability names the Experience executor understands.
/// Tier1 file actions (`append_file`, `mkdir`, `copy_file`, `move_file`,
/// `delete_file`) are already part of the vocabulary so the policy layer can
/// gate them; the executor implementations land with Stage S1-b.
pub const CANONICAL_TOOLS: &[&str] = &[
    "exec_command",
    // Tier2 process form (`program` + `args`) accepted by `exec.rs` and
    // mapped to the `exec` capability family by `policy.rs`; listed here so
    // the vocabulary has exactly one source of truth.
    "exec",
    "write_file",
    "read_file",
    "append_file",
    "mkdir",
    "copy_file",
    "move_file",
    "delete_file",
];

/// Reserved capability identifiers: contract-only channels that are NOT
/// executable by the embedded runtime yet (Stage C4). `computer_use` is the
/// host-level UI automation adapter reserved for the future.
pub const RESERVED_CAPABILITIES: &[&str] = &["computer_use"];

/// Known capability vocabulary = canonical tools + reserved identifiers.
pub fn is_known_capability(name: &str) -> bool {
    CANONICAL_TOOLS.contains(&name) || RESERVED_CAPABILITIES.contains(&name)
}

/// Map a producer tool name to its canonical capability name. Unknown tools
/// return `None` and are refused before storage.
pub fn canonical_tool(name: &str) -> Option<String> {
    match name {
        "commandExecution" | "exec_command" => Some("exec_command".to_string()),
        "write_file" => Some("write_file".to_string()),
        "read_file" => Some("read_file".to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_vocabulary_covers_tools_and_reserved() {
        assert!(is_known_capability("exec_command"));
        assert!(is_known_capability("write_file"));
        assert!(is_known_capability("read_file"));
        assert!(is_known_capability("computer_use"));
        assert!(is_known_capability("append_file"));
        assert!(is_known_capability("mkdir"));
        assert!(is_known_capability("copy_file"));
        assert!(is_known_capability("move_file"));
        assert!(is_known_capability("delete_file"));
        assert!(!is_known_capability("mystery_tool"));
    }
}

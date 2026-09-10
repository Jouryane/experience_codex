//! Action Proposal and the mechanical trigger pattern that matches it.

use serde::Deserialize;
use serde::Serialize;

/// An action the Agent has proposed but not yet executed: the input to the
/// Action Gate (step-gate.md §4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionProposal {
    pub tool: String,
    pub args: serde_json::Value,
}

impl ActionProposal {
    pub fn new(tool: impl Into<String>, args: serde_json::Value) -> Self {
        Self {
            tool: tool.into(),
            args,
        }
    }

    /// Compact text view of the args, used by mechanical pattern matching.
    pub fn args_text(&self) -> String {
        self.args.to_string()
    }
}

/// Trigger of an Experience: which Agent action it may take over.
///
/// Matching is deliberately mechanical (P1: tool equality + case-insensitive
/// substring over the serialized args). MISS always means pass-through; there
/// is no fuzzy or semantic guess here (p1-development-plan.md §1.3).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ActionPattern {
    pub tool: String,
    /// Optional substring the serialized args must contain.
    pub command_pattern: Option<String>,
}

impl ActionPattern {
    pub fn matches(&self, proposal: &ActionProposal) -> bool {
        if self.tool != proposal.tool {
            return false;
        }
        match &self.command_pattern {
            None => true,
            Some(pattern) if pattern.is_empty() => true,
            Some(pattern) => proposal
                .args_text()
                .to_lowercase()
                .contains(&pattern.to_lowercase()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proposal(tool: &str, cmd: &str) -> ActionProposal {
        ActionProposal::new(tool, serde_json::json!({ "cmd": cmd }))
    }

    #[test]
    fn trigger_hits_only_exact_tool_and_substring() {
        let pattern = ActionPattern {
            tool: "exec_command".into(),
            command_pattern: Some("create probe file".into()),
        };
        assert!(pattern.matches(&proposal(
            "exec_command",
            "echo create probe file > probe.txt"
        )));
        // Wrong tool: no hit.
        assert!(!pattern.matches(&proposal(
            "write_file",
            "echo create probe file > probe.txt"
        )));
        // Same tool but unrelated command: MISS -> pass-through.
        assert!(!pattern.matches(&proposal("exec_command", "dir")));
    }

    #[test]
    fn pattern_without_command_matches_any_args_of_tool() {
        let pattern = ActionPattern {
            tool: "exec_command".into(),
            command_pattern: None,
        };
        assert!(pattern.matches(&proposal("exec_command", "anything at all")));
    }
}

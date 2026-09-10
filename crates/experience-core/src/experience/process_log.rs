//! Task process log (M2-4): lets the user open "任务详细过程" and see where
//! experience acted — even in multi-step tasks where experience and LLM
//! interleave. Every entry carries its actor.

use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProcessActor {
    /// Experience (or its workflow step / decision) acted.
    Experience,
    /// The LLM was invoked.
    Llm,
    /// A tool call executed.
    Tool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessStep {
    pub actor: ProcessActor,
    pub action: String,
    pub detail: String,
}

impl ProcessStep {
    pub fn experience(action: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            actor: ProcessActor::Experience,
            action: action.into(),
            detail: detail.into(),
        }
    }

    pub fn llm(action: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            actor: ProcessActor::Llm,
            action: action.into(),
            detail: detail.into(),
        }
    }

    pub fn tool(action: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            actor: ProcessActor::Tool,
            action: action.into(),
            detail: detail.into(),
        }
    }
}

/// Human-readable rendering of the task's detailed process (回检视图).
pub fn render_process_log(steps: &[ProcessStep]) -> String {
    steps
        .iter()
        .enumerate()
        .map(|(index, step)| {
            let tag = match step.actor {
                ProcessActor::Experience => "经验",
                ProcessActor::Llm => "LLM",
                ProcessActor::Tool => "工具",
            };
            let detail = if step.detail.is_empty() {
                String::new()
            } else {
                format!(" — {}", step.detail)
            };
            format!("[{}] {}: {}{}", index + 1, tag, step.action, detail)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_labels_actors_and_orders_steps() {
        let steps = vec![
            ProcessStep::experience("经验接管 e1", ""),
            ProcessStep::experience("scan_files", "ok"),
            ProcessStep::llm("sampling", "deliberation"),
            ProcessStep::tool("shell", r#"{"command":"ls"}"#),
        ];
        let text = render_process_log(&steps);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 4);
        assert!(lines[0].contains("[1] 经验: 经验接管 e1"));
        assert!(lines[1].contains("[2] 经验: scan_files — ok"));
        assert!(lines[2].contains("[3] LLM: sampling — deliberation"));
        assert!(lines[3].contains("[4] 工具: shell"));
    }
}

//! Agent runtime abstraction: Experience calls Agents through Executor,
//! never owns them.

use serde::Deserialize;
use serde::Serialize;

/// Declarative executor capability (step-gate §10). Adapters state what they
/// can do; behavior is never guessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutorCapability {
    /// Level 1: run(task) -> RunReport.
    TaskExecution,
    /// Level 2 (future): synchronous proposal interception on the dispatch
    /// boundary (only fork builds provide this today).
    ActionInterception,
    /// Workspace-aware session channel (delegate into a real agent session).
    SessionChannel,
}

/// One delegated task handed to an external agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTask {
    pub text: String,
    /// Optional working directory.
    pub cwd: Option<String>,
    /// Optional reference text injected as hints (③ Reference semantics).
    pub reference: Option<String>,
}

/// What the agent produced. Experience-core converts this into a learning
/// trace; the Runtime stores experiences, the agent never does.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunReport {
    pub ok: bool,
    pub summary: String,
    pub raw_output: String,
}

/// The Executor contract (Experience has the right to call, not to own).
pub trait AgentExecutor: Send + Sync {
    fn id(&self) -> &'static str;
    /// Declared capabilities — never inferred by behavior.
    fn capabilities(&self) -> Vec<ExecutorCapability> {
        vec![ExecutorCapability::TaskExecution]
    }
    fn run(&self, task: &AgentTask) -> RunReport;
}

/// User-configured agents (super plugins / outsourced executors).
pub struct AgentManager {
    executors: Vec<std::sync::Arc<dyn AgentExecutor>>,
}

impl AgentManager {
    pub fn new() -> Self {
        Self { executors: Vec::new() }
    }

    pub fn register(&mut self, executor: Box<dyn AgentExecutor>) {
        self.executors.push(std::sync::Arc::from(executor));
    }

    pub fn list(&self) -> Vec<String> {
        self.executors.iter().map(|e| e.id().to_string()).collect()
    }

    pub fn run(&self, id: &str, task: &AgentTask) -> Option<RunReport> {
        self.executors
            .iter()
            .find(|executor| executor.id() == id)
            .map(|executor| executor.run(task))
    }
}

impl std::fmt::Debug for AgentManager {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentManager")
            .field("executors", &self.list())
            .finish()
    }
}

impl Default for AgentManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake;
    impl AgentExecutor for Fake {
        fn id(&self) -> &'static str {
            "fake"
        }
        fn capabilities(&self) -> Vec<ExecutorCapability> {
            vec![ExecutorCapability::TaskExecution]
        }
        fn run(&self, task: &AgentTask) -> RunReport {
            RunReport {
                ok: true,
                summary: format!("handled: {}", task.text.chars().take(40).collect::<String>()),
                raw_output: String::new(),
            }
        }
    }

    #[test]
    fn manager_runs_registered_executor() {
        let mut manager = AgentManager::new();
        manager.register(Box::new(Fake));
        assert_eq!(manager.list(), vec!["fake"]);
        let report = manager
            .run("fake", &AgentTask { text: "build project".into(), cwd: None, reference: None })
            .unwrap();
        assert!(report.ok);
        assert!(report.summary.contains("build project"));
    }
}

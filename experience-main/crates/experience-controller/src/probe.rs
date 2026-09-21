//! State probing: how the Runtime observes the world before/after execution.

use std::fs;

use serde_json::Value;

use super::runtime::GateContext;

/// Observation interface for State predicates (P1 scope).
///
/// A probe answers "what is the current value of `key`?" with None meaning
/// "not observable / unknown" — never conflated with False.
pub trait Probe: Send + Sync {
    fn probe(&self, context: &GateContext, key: &str) -> Option<Value>;
}

/// P1 key grammar (see experience-main/docs/state-model.md):
/// - `cwd.exists`                          -> working directory exists
/// - `file:<relpath>.exists`               -> file exists
/// - `file:<relpath>.content`              -> file content (UTF-8)
#[derive(Debug, Clone, Default)]
pub struct LocalProbe;

impl Probe for LocalProbe {
    fn probe(&self, context: &GateContext, key: &str) -> Option<Value> {
        if key == "cwd.exists" {
            return Some(Value::Bool(context.cwd.exists()));
        }
        if let Some(rest) = key.strip_prefix("file:") {
            if let Some(relative) = rest.strip_suffix(".exists") {
                let path = context.cwd.join(relative);
                return Some(Value::Bool(path.exists()));
            }
            if let Some(relative) = rest.strip_suffix(".content") {
                let path = context.cwd.join(relative);
                return fs::read_to_string(path)
                    .ok()
                    .map(|content| Value::String(content));
            }
        }
        None
    }
}

/// Probe that answers only from an in-memory map (deterministic tests).
#[derive(Debug, Clone, Default)]
pub struct MapProbe {
    pub facts: std::collections::HashMap<String, Value>,
}

impl Probe for MapProbe {
    fn probe(&self, _context: &GateContext, key: &str) -> Option<Value> {
        self.facts.get(key).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_probe_reads_cwd_and_files() {
        let dir = std::env::temp_dir().join(format!(
            "exp-probe-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.txt"), "hello").unwrap();

        let probe = LocalProbe;
        let context = GateContext { cwd: dir.clone() };
        assert_eq!(probe.probe(&context, "cwd.exists"), Some(Value::Bool(true)));
        assert_eq!(
            probe.probe(&context, "file:a.txt.exists"),
            Some(Value::Bool(true))
        );
        assert_eq!(
            probe.probe(&context, "file:a.txt.content"),
            Some(Value::String("hello".into()))
        );
        assert_eq!(
            probe.probe(&context, "file:missing.txt.exists"),
            Some(Value::Bool(false))
        );
        assert_eq!(probe.probe(&context, "unknown.key"), None);
        let _ = fs::remove_dir_all(&dir);
    }
}

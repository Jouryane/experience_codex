//! Predicate: the smallest verifiable statement about the world.
//!
//! State is not a snapshot: it is a set of evidence-bearing predicates
//! `(key, value, evidence, verified_at, ttl)` (state-model.md). Preconditions
//! and postconditions of an Experience are *checks* over these facts; probing
//! returns a three-valued truth (True / False / Unknown).

use serde::Deserialize;
use serde::Serialize;

/// Result of probing one predicate against the world.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TruthValue {
    True,
    False,
    /// No observation exists yet — must not be treated as False.
    Unknown,
}

/// A check `key == expected`, used by preconditions and postconditions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Predicate {
    pub key: String,
    pub expected: serde_json::Value,
}

impl Predicate {
    pub fn new(key: impl Into<String>, expected: serde_json::Value) -> Self {
        Self {
            key: key.into(),
            expected,
        }
    }

    /// Probe against an observed value. Unknown when the key was never
    /// observed; False must never be conflated with Unknown.
    pub fn probe(&self, observed: Option<&serde_json::Value>) -> TruthValue {
        match observed {
            None => TruthValue::Unknown,
            Some(actual) if actual == &self.expected => TruthValue::True,
            Some(_) => TruthValue::False,
        }
    }
}

/// An observed fact: the evidence-bearing unit of State.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StateFact {
    pub key: String,
    pub value: serde_json::Value,
    /// How this fact was observed (command, file read, tool output...).
    pub evidence: String,
    /// Unix seconds when the fact was verified.
    pub verified_at: u64,
    /// Optional time-to-live in seconds; None = no expiry.
    pub ttl: Option<u64>,
}

impl StateFact {
    pub fn new(
        key: impl Into<String>,
        value: serde_json::Value,
        evidence: impl Into<String>,
        verified_at: u64,
    ) -> Self {
        Self {
            key: key.into(),
            value,
            evidence: evidence.into(),
            verified_at,
            ttl: None,
        }
    }

    /// Does this fact satisfy the given predicate?
    pub fn satisfies(&self, predicate: &Predicate) -> TruthValue {
        if self.key != predicate.key {
            return TruthValue::Unknown;
        }
        predicate.probe(Some(&self.value))
    }
}

/// Look up the observed value for a key across a set of facts.
pub fn fact_value<'a>(facts: &'a [StateFact], key: &str) -> Option<&'a serde_json::Value> {
    facts.iter().find(|fact| fact.key == key).map(|fact| &fact.value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_is_three_valued() {
        let predicate = Predicate::new("file:probe.txt.exists", serde_json::json!(true));
        assert_eq!(predicate.probe(None), TruthValue::Unknown);
        assert_eq!(predicate.probe(Some(&serde_json::json!(true))), TruthValue::True);
        assert_eq!(predicate.probe(Some(&serde_json::json!(false))), TruthValue::False);
    }

    #[test]
    fn state_fact_satisfies_predicate_by_key_and_value() {
        let fact = StateFact::new("file:a.exists", serde_json::json!(true), "ls", 1);
        assert_eq!(
            fact.satisfies(&Predicate::new("file:a.exists", serde_json::json!(true))),
            TruthValue::True
        );
        assert_eq!(
            fact.satisfies(&Predicate::new("file:b.exists", serde_json::json!(true))),
            TruthValue::Unknown
        );
    }
}

//! Read-only structural similarity (Stage S3.5 declaration).
//!
//! Deliberately conservative: this module never merges or mutates. It scores
//! how alike two experiences are *structurally* (trigger tool, normalized
//! pattern, workflow action skeleton), so a host can show "same family"
//! candidates and let a human (or an explicit compiler) decide.
//!
//! Why it exists: with a flat store and exact-signature dedup only, a family
//! of near-duplicate experiences is invisible. This is the "声明出来的妥协 +
//! 可替换扩展点" described in docs/learning-reuse-contract.md.

use std::collections::BTreeSet;

use crate::domain::experience::Experience;

/// Components used to compare two experiences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    pub tool: String,
    /// Lowercased pattern with collapsing whitespace ("" when unscoped).
    pub pattern: String,
    /// Workflow action skeleton, order-sensitive (`write_file>move_file`).
    pub actions: String,
    pub action_set: BTreeSet<String>,
}

impl Fingerprint {
    fn components(&self) -> BTreeSet<String> {
        let mut set = self
            .action_set
            .iter()
            .map(|action| format!("action:{action}"))
            .collect::<BTreeSet<_>>();
        set.insert(format!("tool:{}", self.tool));
        // Pattern words are individual components: "create manifest" vs
        // "manifest" should read as a family, not as unrelated strings.
        for word in self.pattern.split_whitespace() {
            if word.len() >= 3 {
                set.insert(format!("word:{word}"));
            }
        }
        set
    }
}

/// Structural fingerprint for one experience.
pub fn fingerprint(experience: &Experience) -> Fingerprint {
    let pattern = experience
        .trigger
        .command_pattern
        .as_deref()
        .unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let actions = experience
        .workflow
        .iter()
        .map(|step| step.action.as_str())
        .collect::<Vec<_>>()
        .join(">");
    let action_set = experience
        .workflow
        .iter()
        .map(|step| step.action.clone())
        .collect();
    Fingerprint {
        tool: experience.trigger.tool.clone(),
        pattern,
        actions,
        action_set,
    }
}

/// Jaccard similarity of normalized component sets: 0.0 (unrelated) .. 1.0 (same).
pub fn similarity(left: &Experience, right: &Experience) -> f64 {
    let left = fingerprint(left).components();
    let right = fingerprint(right).components();
    if left.is_empty() && right.is_empty() {
        return 1.0;
    }
    let union = left.union(&right).count() as f64;
    if union == 0.0 {
        return 0.0;
    }
    left.intersection(&right).count() as f64 / union
}

/// One "same family" cluster (read-only suggestion, never an auto-merge).
#[derive(Debug, Clone, PartialEq)]
pub struct Cluster {
    /// Human-readable label (`tool|pattern|actions`).
    pub label: String,
    pub members: Vec<String>,
    /// Pairwise similarity inside the cluster (max vs. first member).
    pub max_similarity: f64,
}

/// Group experiences whose structural similarity meets `threshold`.
/// Ordering is deterministic; singletons are returned as their own cluster
/// so the caller can decide whether to show them.
pub fn clusters(experiences: &[&Experience], threshold: f64) -> Vec<Cluster> {
    let mut result: Vec<Cluster> = Vec::new();
    for experience in experiences {
        let print = fingerprint(experience);
        let mut placed = false;
        for cluster in result.iter_mut() {
            let Some(first) = experiences
                .iter()
                .find(|candidate| candidate.name == cluster.members[0])
            else {
                continue;
            };
            let score = similarity(first, experience);
            if score >= threshold {
                cluster.members.push(experience.name.clone());
                cluster.max_similarity = cluster.max_similarity.max(score);
                placed = true;
                break;
            }
        }
        if !placed {
            result.push(Cluster {
                label: format!(
                    "{}|{}|{}",
                    print.tool, print.pattern, print.actions
                ),
                members: vec![experience.name.clone()],
                max_similarity: 0.0,
            });
        }
    }
    result
        .sort_by(|left, right| right.members.len().cmp(&left.members.len()).then(
            left.label.cmp(&right.label),
        ));
    result
}

/// Similarity report entry for one experience pair.
#[derive(Debug, Clone, PartialEq)]
pub struct Pair {
    pub left: String,
    pub right: String,
    pub similarity: f64,
}

/// All pairs at or above `threshold`, most similar first, capped.
pub fn pairs(experiences: &[&Experience], threshold: f64, cap: usize) -> Vec<Pair> {
    let mut pairs = Vec::new();
    for (index, left) in experiences.iter().enumerate() {
        for right in experiences.iter().skip(index + 1) {
            let score = similarity(left, right);
            if score >= threshold {
                pairs.push(Pair {
                    left: left.name.clone(),
                    right: right.name.clone(),
                    similarity: score,
                });
            }
        }
    }
    pairs.sort_by(|left, right| {
        right
            .similarity
            .partial_cmp(&left.similarity)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(left.left.cmp(&right.left))
    });
    pairs.truncate(cap);
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn experience(name: &str, pattern: &str, actions: &[&str]) -> Experience {
        serde_json::from_value(json!({
            "name": name,
            "trigger": {"tool": "exec_command", "command_pattern": pattern},
            "preconditions": [{"key": "cwd.exists", "expected": true}],
            "workflow": actions
                .iter()
                .map(|action| json!({"action": action, "args": {"path": "x"}}))
                .collect::<Vec<_>>(),
            "postconditions": [{"key": "file:x.exists", "expected": true}],
            "verification": [],
            "failure_policy": "stop_and_report",
            "undo": "unsupported",
            "status": "active"
        }))
        .unwrap()
    }

    #[test]
    fn identical_experiences_score_one() {
        let left = experience("a", "create manifest", &["write_file"]);
        let right = experience("b", "create manifest", &["write_file"]);
        assert_eq!(similarity(&left, &right), 1.0);
    }

    #[test]
    fn same_family_scores_high_and_includes_pattern() {
        let left = experience("a", "create manifest", &["write_file"]);
        let right = experience("b", "manifest", &["write_file"]);
        let score = similarity(&left, &right);
        // Same tool + same action set, different pattern wording.
        assert!(score > 0.6 && score < 1.0, "{score}");
    }

    #[test]
    fn different_family_scores_low() {
        let left = experience("a", "create manifest", &["write_file"]);
        let right = experience("b", "run tests", &["exec"]);
        assert!(similarity(&left, &right) < 0.3, "{}", similarity(&left, &right));
    }

    #[test]
    fn clusters_group_near_duplicates_and_keep_singletons() {
        let a = experience("a", "create manifest", &["write_file"]);
        let b = experience("b", "manifest", &["write_file"]);
        let c = experience("c", "run tests", &["exec"]);
        let items = vec![&a, &b, &c];
        let clusters = clusters(&items, 0.5);
        assert_eq!(clusters.len(), 2);
        let family = clusters
            .iter()
            .find(|cluster| cluster.members.len() == 2)
            .expect("near duplicates grouped");
        assert!(family.members.contains(&"a".to_string()));
        assert!(family.members.contains(&"b".to_string()));
    }

    #[test]
    fn pairs_are_sorted_and_capped() {
        let a = experience("a", "create manifest", &["write_file"]);
        let b = experience("b", "create manifest", &["write_file"]);
        let c = experience("c", "manifest", &["write_file"]);
        let items = vec![&a, &b, &c];
        let pairs = pairs(&items, 0.5, 1);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].similarity, 1.0);
    }
}

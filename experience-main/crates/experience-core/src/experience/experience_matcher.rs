//! Experience Matcher: answers the question "is there a corresponding
//! experience for the current state?" — the verification that turns the fast
//! pseudo-parallel path on.
//!
//! Verification is split into two layers on purpose:
//!
//! 1. **Content layer (this module)**: does the current task / intent / goal
//!    correspond to the experience's trigger? Produces ranked candidates.
//! 2. **State layer (`ExperienceRuntime::decide`)**: are the state elements
//!    the experience depends on actually present (conditions) and is it
//!    trusted enough (confidence x similarity thresholds)?
//!
//! The reference implementation's `matcher.py` matches on text features
//! (rule / tag / vector + BM25). Our first version is deterministic and
//! state-aware: keywords + trigger fields are matched against the task,
//! intent, and goal carried by `ExperienceState`.

use std::collections::HashSet;

use super::experience::Experience;
use super::experience::ExperienceKind;
use super::experience::ExperienceStatus;
use super::experience::ExperienceTrigger;
use super::experience_runtime::ExperienceCandidate;
use super::experience_state::ExperienceState;

const EMBED_DIM: usize = 96;

/// Minimum combined similarity for a candidate to exist at all. This is the
/// same floor as the parallel-LLM hint band (sim >= 0.10) and filters out
/// vector-noise matches from unrelated tasks.
pub const MIN_SIMILARITY: f32 = 0.10;

/// Only experiences that are trusted enough participate in matching.
fn is_matchable(status: ExperienceStatus) -> bool {
    matches!(
        status,
        ExperienceStatus::Validated | ExperienceStatus::Active
    )
}

/// FNV-1a 64-bit (deterministic across platforms; used for feature hashing).
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Simple tokenizer: lowercase, split on non-alphanumeric, keep non-empty.
pub(crate) fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(String::from)
        .collect()
}

/// The text the current state is matched against.
fn state_haystack(state: &ExperienceState) -> String {
    let mut haystack = String::new();
    if let Some(task) = &state.task {
        haystack.push_str(&task.description);
        haystack.push(' ');
    }
    if let Some(intent) = &state.current_context.intent {
        haystack.push_str(intent);
        haystack.push(' ');
    }
    if let Some(goal) = &state.current_goal {
        haystack.push_str(&goal.description);
        haystack.push(' ');
    }
    haystack
}

/// The text of an experience trigger (keywords + fields) used for matching.
fn trigger_text(trigger: &ExperienceTrigger) -> String {
    let mut text = trigger.keywords.join(" ");
    for field in [&trigger.object, &trigger.location, &trigger.goal] {
        if let Some(value) = field {
            text.push(' ');
            text.push_str(value);
        }
    }
    text
}

/// Rule score: keyword + trigger-field substring hits over the state haystack.
///
/// Scoring (v1, deterministic):
/// - haystack = task.description + context.intent + goal.description
/// - keyword hits: +1 each (substring, case-insensitive)
/// - trigger fields (object / location / goal) that appear in the haystack:
///   +1 each
/// - similarity = hits / (keywords.len() + 3), clamped to [0, 1]
///
/// Returns 0 when nothing matches (no experience for this task at all).
pub fn content_similarity(state: &ExperienceState, trigger: &ExperienceTrigger) -> f32 {
    let hay = state_haystack(state).to_lowercase();
    if hay.is_empty() {
        return 0.0;
    }

    let keyword_count = trigger.keywords.len();
    let mut hits = 0.0;
    for keyword in &trigger.keywords {
        if !keyword.is_empty() && hay.contains(&keyword.to_lowercase()) {
            hits += 1.0;
        }
    }
    let mut field_hits = 0.0;
    for field in [&trigger.object, &trigger.location, &trigger.goal] {
        if let Some(value) = field {
            if !value.is_empty() && hay.contains(&value.to_lowercase()) {
                field_hits += 1.0;
            }
        }
    }

    let denominator = (keyword_count + 3) as f32;
    if denominator == 0.0 {
        return 0.0;
    }
    ((hits + field_hits) / denominator).clamp(0.0, 1.0)
}

/// Tag score: Jaccard overlap between state tokens and trigger tokens.
fn tag_score(state: &ExperienceState, trigger: &ExperienceTrigger) -> f32 {
    let state_tokens: HashSet<String> = tokenize(&state_haystack(state)).into_iter().collect();
    let trigger_tokens: HashSet<String> = tokenize(&trigger_text(trigger)).into_iter().collect();
    if state_tokens.is_empty() || trigger_tokens.is_empty() {
        return 0.0;
    }
    let intersection = state_tokens.intersection(&trigger_tokens).count() as f32;
    let union = (state_tokens.len() + trigger_tokens.len()) as f32 - intersection;
    if union <= 0.0 {
        0.0
    } else {
        intersection / union
    }
}

/// Hashed n-gram embedding (1- and 2-grams, feature hashing, no external
/// model). Deterministic and fixed-dimension, mirroring the reference
/// implementation's embedding approach.
fn embed(text: &str) -> Vec<f32> {
    let mut vector = vec![0.0f32; EMBED_DIM];
    let compact: String = text
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(|c| c.to_lowercase())
        .collect();
    let chars: Vec<char> = compact.chars().collect();
    let mut grams: Vec<String> = Vec::new();
    for c in &chars {
        grams.push(c.to_string());
    }
    for i in 0..chars.len().saturating_sub(1) {
        grams.push(format!("{}{}", chars[i], chars[i + 1]));
    }
    for gram in &grams {
        let hash = fnv1a(gram.as_bytes());
        let index = (hash % EMBED_DIM as u64) as usize;
        let sign = if (hash / EMBED_DIM as u64) % 2 == 0 { 1.0 } else { -1.0 };
        vector[index] += sign;
    }
    let norm = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in &mut vector {
            *value /= norm;
        }
    }
    vector
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum();
    let nb: f32 = b.iter().map(|x| x * x).sum();
    if na <= 0.0 || nb <= 0.0 {
        0.0
    } else {
        dot / (na * nb).sqrt()
    }
}

/// Vector score: cosine between hashed n-gram embeddings of state and trigger.
fn vector_score(state: &ExperienceState, trigger: &ExperienceTrigger) -> f32 {
    let state_vector = embed(&state_haystack(state));
    let trigger_vector = embed(&trigger_text(trigger));
    cosine(&state_vector, &trigger_vector)
}

/// Type-differentiated matching weights (rule / tag / vector), mirroring the
/// reference implementation. Execution kinds favor exact rule matching;
/// reference/process kinds favor semantic vector matching.
pub fn kind_weights(kind: ExperienceKind) -> (f32, f32, f32) {
    match kind {
        ExperienceKind::Reflex => (0.65, 0.20, 0.15),
        ExperienceKind::Result => (0.60, 0.20, 0.20),
        ExperienceKind::Process => (0.45, 0.25, 0.30),
        ExperienceKind::Reference => (0.30, 0.20, 0.50),
    }
}

/// Combined similarity (rule / tag / vector, weighted by experience kind).
pub fn combined_similarity(state: &ExperienceState, experience: &Experience) -> f32 {
    let (rule_weight, tag_weight, vector_weight) = kind_weights(experience.kind);
    let rule = content_similarity(state, &experience.trigger);
    let tag = tag_score(state, &experience.trigger);
    let vector = vector_score(state, &experience.trigger);
    (rule * rule_weight + tag * tag_weight + vector * vector_weight).clamp(0.0, 1.0)
}

/// Match the current state against all experiences and return candidates
/// sorted by similarity (highest first).
///
/// This is the "有无对应经验" check: an empty result means the fast path is
/// off and the runtime must `Delegate` (④). Conditions are deliberately NOT
/// evaluated here — they belong to the state layer in `decide`.
pub fn match_experiences<'a>(
    state: &ExperienceState,
    experiences: impl IntoIterator<Item = &'a Experience>,
) -> Vec<ExperienceCandidate<'a>> {
    let mut candidates: Vec<ExperienceCandidate<'a>> = Vec::new();
    for experience in experiences {
        if !is_matchable(experience.status) {
            continue;
        }
        let similarity = combined_similarity(state, experience);
        if similarity < MIN_SIMILARITY {
            continue;
        }
        candidates.push(ExperienceCandidate {
            experience,
            similarity,
        });
    }
    candidates.sort_by(|a, b| {
        b.similarity
            .partial_cmp(&a.similarity)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    fn experience(status: ExperienceStatus, keywords: Vec<&str>) -> Experience {
        Experience {
            id: super::super::experience_state::ExperienceId("e1".to_string()),
            name: "test".to_string(),
            kind: super::super::experience::ExperienceKind::Reflex,
            trigger: ExperienceTrigger {
                keywords: keywords.into_iter().map(String::from).collect(),
                ..ExperienceTrigger::default()
            },
            conditions: Vec::new(),
            workflow: Default::default(),
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

    fn state_with_task(description: &str) -> ExperienceState {
        let mut state = ExperienceState::default();
        state.task = Some(super::super::experience_state::Task {
            description: description.to_string(),
            status: Default::default(),
        });
        state
    }

    #[test]
    fn no_matchable_experiences_returns_empty() {
        let state = state_with_task("move pdfs to archive");
        let experiences = vec![
            experience(ExperienceStatus::Candidate, vec!["pdf"]),
            experience(ExperienceStatus::Disabled, vec!["pdf"]),
        ];
        assert!(match_experiences(&state, &experiences).is_empty());
    }

    #[test]
    fn unrelated_experience_does_not_match() {
        let state = state_with_task("review project structure");
        let experiences = vec![experience(ExperienceStatus::Active, vec!["pdf", "archive"])];
        assert!(match_experiences(&state, &experiences).is_empty());
    }

    #[test]
    fn keyword_hit_produces_candidate() {
        let state = state_with_task("move pdfs to archive");
        let experiences = vec![experience(ExperienceStatus::Active, vec!["pdf", "archive"])];
        let candidates = match_experiences(&state, &experiences);
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].similarity > 0.0);
    }

    #[test]
    fn candidates_sorted_by_similarity_desc() {
        let state = state_with_task("move pdfs to archive");
        let mut weak = experience(ExperienceStatus::Active, vec!["pdf"]);
        weak.id = super::super::experience_state::ExperienceId("weak".to_string());
        let mut strong = experience(ExperienceStatus::Active, vec!["pdf", "archive", "move"]);
        strong.id = super::super::experience_state::ExperienceId("strong".to_string());
        let experiences = vec![weak, strong];
        let candidates = match_experiences(&state, &experiences);
        assert_eq!(candidates.len(), 2);
        assert!(candidates[0].similarity >= candidates[1].similarity);
    }

    #[test]
    fn conditions_are_not_evaluated_by_matcher() {
        // Conditions belong to the state layer (decide). The matcher only
        // verifies content existence; a candidate may still be rejected later.
        let state = state_with_task("move pdfs to archive");
        let mut exp = experience(ExperienceStatus::Active, vec!["pdf"]);
        exp.conditions = vec![super::super::experience::ExperienceCondition {
            key: "environment.detection.missing".to_string(),
            expected: serde_json::json!(true),
        }];
        assert_eq!(match_experiences(&state, &[exp]).len(), 1);
    }

    #[test]
    fn kind_weights_favor_rule_for_reflex_and_vector_for_reference() {
        let (reflex_rule, _, _) = kind_weights(ExperienceKind::Reflex);
        let (reference_rule, _, reference_vector) = kind_weights(ExperienceKind::Reference);
        assert!(reflex_rule > reference_rule);
        assert!(reference_vector > 0.4);
    }

    #[test]
    fn combined_similarity_is_weighted_sum_of_three_scores() {
        let state = state_with_task("move pdfs to archive");
        let exp = experience(ExperienceStatus::Active, vec!["pdf", "archive"]);
        let rule = content_similarity(&state, &exp.trigger);
        let tag = tag_score(&state, &exp.trigger);
        let vector = vector_score(&state, &exp.trigger);
        let (rule_weight, tag_weight, vector_weight) = kind_weights(exp.kind);
        let combined = combined_similarity(&state, &exp);
        let expected = rule * rule_weight + tag * tag_weight + vector * vector_weight;
        assert!((combined - expected).abs() < 1e-4);
    }

    #[test]
    fn semantic_similarity_alone_can_match_reference_kind() {
        // Different wording, same meaning: rule/tag may miss, vector catches it.
        let state = state_with_task("把 pdf 文档移动到归档目录");
        let mut exp = experience(ExperienceStatus::Active, vec!["归档", "移动"]);
        exp.kind = ExperienceKind::Reference;
        let experiences = [exp];
        let candidates = match_experiences(&state, &experiences);
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].similarity > 0.0);
    }
}

//! Deterministic template induction from two successful observations.
//!
//! The question this module answers is *not* "what would the model have
//! parameterized?" It is: **given two completed runs of the same task family,
//! which literal values are known to vary, and can each of them be captured
//! from the request text by a rule that demonstrably reproduces both observed
//! values?**
//!
//! That is the whole contract. Nothing here is inferred, scored or generated:
//!
//! 1. structural agreement is required (same actions, same arity, same shape);
//! 2. a differing literal becomes a parameter only if a `prefix`/`suffix`
//!    capture rule binds it back to the observed value in **both** source
//!    runs, through the same `capture_parameter` the Gate uses at bind time;
//! 3. ephemeral identifiers (hashes, uuids) are refused rather than turned
//!    into parameters;
//! 4. the induced artifact is always a `Candidate`, never `Active` — it still
//!    has to be validated by real execution before it may take over.
//!
//! So an induced parameter is a *verified* capture rule, not a guess. If no
//! such rule exists, induction fails and the caller keeps the exact instance
//! (or nothing).

use std::collections::BTreeMap;

use serde::Deserialize;
use serde::Serialize;

use crate::domain::action::ActionPattern;
use crate::domain::capability::CANONICAL_TOOLS;
use crate::domain::experience::ExperienceStatus;
use crate::domain::experience::FailurePolicy;
use crate::domain::experience::UndoPolicy;
use crate::domain::experience::WorkflowStep;
use crate::domain::experience::VerificationStep;
use crate::domain::predicate::Predicate;
use crate::domain::template::capture_parameter;
use crate::domain::template::ExperienceTemplate;
use crate::domain::template::TemplateParameter;
use crate::domain::template::PARAM_KIND_PATH;
use crate::domain::template::PARAM_KIND_TEXT;
use crate::experience::learning_l1::likely_ephemeral_identifier;

/// A single observed tool call. Deliberately decoupled from any host trace
/// type so codex-core can hand over its own recorder output directly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObservedStep {
    pub action: String,
    pub args: serde_json::Value,
}

impl ObservedStep {
    pub fn new(action: impl Into<String>, args: serde_json::Value) -> Self {
        Self {
            action: action.into(),
            args,
        }
    }
}

/// One completed observation of a task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObservedTask {
    pub task: String,
    pub steps: Vec<ObservedStep>,
    /// Only a successful observation may be induced from; the caller records
    /// this from the real outcome, it is never assumed.
    pub succeeded: bool,
}

impl ObservedTask {
    pub fn new(task: impl Into<String>, steps: Vec<ObservedStep>, succeeded: bool) -> Self {
        Self {
            task: task.into(),
            steps,
            succeeded,
        }
    }
}

/// Why two observations could not be turned into one template. Every variant
/// means "keep the exact instance / stay with the model" — never "guess".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InduceReject {
    TaskNotSuccessful { arm: &'static str },
    NoActionEvidence,
    StepCountMismatch { a: usize, b: usize },
    ActionMismatch { at: usize, a: String, b: String },
    UnknownCapability(String),
    UnstableShape { at: usize, reason: String },
    EphemeralIdentifier(String),
    NotCapturable(String),
    /// The two runs are identical: this is an exact instance, not a template.
    NoParameterDifference,
    /// Property-1 of the three contracts: no observable postcondition means
    /// the body could never be verified, so it must not become a template.
    NoVerifiableEffect,
    TooManyParameters(usize),
    /// The task texts share too little stable prefix to address this family.
    WeakTaskSignature { common_prefix: usize },
    /// The induced artifact was valid but the store refused it.
    Store(String),
}

impl InduceReject {
    pub fn label(&self) -> &'static str {
        match self {
            Self::TaskNotSuccessful { .. } => "task_not_successful",
            Self::NoActionEvidence => "no_action_evidence",
            Self::StepCountMismatch { .. } => "step_count_mismatch",
            Self::ActionMismatch { .. } => "action_mismatch",
            Self::UnknownCapability(_) => "unknown_capability",
            Self::UnstableShape { .. } => "unstable_shape",
            Self::EphemeralIdentifier(_) => "ephemeral_identifier",
            Self::NotCapturable(_) => "not_capturable",
            Self::NoParameterDifference => "no_parameter_difference",
            Self::NoVerifiableEffect => "no_verifiable_effect",
            Self::TooManyParameters(_) => "too_many_parameters",
            Self::WeakTaskSignature { .. } => "weak_task_signature",
            Self::Store(_) => "store_rejected",
        }
    }
}

impl std::fmt::Display for InduceReject {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TaskNotSuccessful { arm } => {
                write!(formatter, "observation '{arm}' did not succeed")
            }
            Self::NoActionEvidence => write!(formatter, "observation has no tool calls"),
            Self::StepCountMismatch { a, b } => {
                write!(formatter, "step count differs: {a} vs {b}")
            }
            Self::ActionMismatch { at, a, b } => {
                write!(formatter, "step {at} differs: '{a}' vs '{b}'")
            }
            Self::UnknownCapability(name) => write!(formatter, "unknown capability '{name}'"),
            Self::UnstableShape { at, reason } => {
                write!(formatter, "step {at} has an unstable shape: {reason}")
            }
            Self::EphemeralIdentifier(value) => {
                write!(formatter, "'{value}' looks like an ephemeral identifier")
            }
            Self::NotCapturable(value) => {
                write!(formatter, "'{value}' has no capture rule that binds both runs")
            }
            Self::NoParameterDifference => {
                write!(formatter, "observations are identical; induce an instance, not a template")
            }
            Self::NoVerifiableEffect => {
                write!(formatter, "workflow has no observable postcondition")
            }
            Self::TooManyParameters(count) => {
                write!(formatter, "{count} parameters exceeds the induction budget")
            }
            Self::WeakTaskSignature { common_prefix } => write!(
                formatter,
                "task texts share only {common_prefix} leading characters"
            ),
            Self::Store(reason) => write!(formatter, "store refused the candidate: {reason}"),
        }
    }
}

impl std::error::Error for InduceReject {}

/// Upper bound on induced parameters. A template that needs more than this is
/// not a reusable transition, it is a script with holes.
pub const MAX_INDUCED_PARAMETERS: usize = 8;
/// Longest value that may become a parameter.
pub const MAX_PARAMETER_VALUE_LEN: usize = 128;
/// Minimum shared leading text required to treat two requests as one family.
pub const MIN_TASK_SIGNATURE_PREFIX: usize = 8;

/// Induce one `Candidate` template from two successful observations.
///
/// The result is never `Active`: activation is the qualification chain's job.
pub fn induce(a: &ObservedTask, b: &ObservedTask) -> Result<ExperienceTemplate, InduceReject> {
    if !a.succeeded {
        return Err(InduceReject::TaskNotSuccessful { arm: "a" });
    }
    if !b.succeeded {
        return Err(InduceReject::TaskNotSuccessful { arm: "b" });
    }
    if a.steps.is_empty() || b.steps.is_empty() {
        return Err(InduceReject::NoActionEvidence);
    }
    if a.steps.len() != b.steps.len() {
        return Err(InduceReject::StepCountMismatch {
            a: a.steps.len(),
            b: b.steps.len(),
        });
    }
    for (step, (first, second)) in a.steps.iter().zip(b.steps.iter()).enumerate() {
        if first.action != second.action {
            return Err(InduceReject::ActionMismatch {
                at: step,
                a: first.action.clone(),
                b: second.action.clone(),
            });
        }
        if !CANONICAL_TOOLS.contains(&first.action.as_str()) {
            return Err(InduceReject::UnknownCapability(first.action.clone()));
        }
    }

    let mut inducer = Inducer {
        task_a: &a.task,
        task_b: &b.task,
        parameters: Vec::new(),
        rules: BTreeMap::new(),
        names: BTreeMap::new(),
    };

    let mut workflow = Vec::with_capacity(a.steps.len());
    for (index, first) in a.steps.iter().enumerate() {
        let args = inducer.parameterize(&first.args, &b.steps[index].args, None, index)?;
        workflow.push(WorkflowStep::new(first.action.clone(), args));
    }

    if inducer.parameters.is_empty() {
        return Err(InduceReject::NoParameterDifference);
    }
    if inducer.parameters.len() > MAX_INDUCED_PARAMETERS {
        return Err(InduceReject::TooManyParameters(inducer.parameters.len()));
    }

    let (preconditions, postconditions, verification) = effects_for(&workflow);
    if postconditions.is_empty() {
        return Err(InduceReject::NoVerifiableEffect);
    }

    let command_pattern = task_signature_prefix(&a.task, &b.task)?;
    let template = ExperienceTemplate {
        name: candidate_name(&a.task, &inducer.parameters),
        trigger: ActionPattern {
            tool: "task".to_string(),
            command_pattern: Some(command_pattern),
        },
        parameters: inducer.parameters,
        workflow,
        preconditions,
        postconditions,
        verification,
        failure_policy: FailurePolicy::StopAndReport,
        undo: UndoPolicy::Unsupported,
        status: ExperienceStatus::Candidate,
    };
    if !template.schema_issues().is_empty() {
        return Err(InduceReject::UnstableShape {
            at: 0,
            reason: format!("induced template failed schema: {:?}", template.schema_issues()),
        });
    }
    Ok(template)
}

/// Name for an induced candidate. Deterministic in the task signature and the
/// parameter names, so re-inducing the same family converges on one name
/// instead of accumulating near-duplicates.
pub fn candidate_name(task: &str, parameters: &[TemplateParameter]) -> String {
    let mut signature: String = task
        .to_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '_'
            }
        })
        .collect();
    while signature.contains("__") {
        signature = signature.replace("__", "_");
    }
    let signature = signature.trim_matches('_');
    let signature: String = signature.chars().take(40).collect();
    let mut shapes: Vec<String> = parameters
        .iter()
        .map(|parameter| sanitize(&parameter.name))
        .collect();
    shapes.sort();
    shapes.dedup();
    format!("tcand_{signature}__{}", shapes.join("_"))
}

/// Induce a template and write it into the store as a **Candidate**.
///
/// The learning invariant is enforced here, not by the caller: the artifact
/// lands as `Candidate` (never `Active`), and the provenance of the induction
/// is recorded so the qualification chain can audit where it came from.
pub fn induce_into_store(
    store: &mut crate::store::ExperienceStore,
    a: &ObservedTask,
    b: &ObservedTask,
    distiller: &str,
    session_id: Option<String>,
    recorded_at: u64,
) -> Result<String, InduceReject> {
    let template = induce(a, b)?;
    debug_assert_eq!(template.status, ExperienceStatus::Candidate);
    let name = template.name.clone();
    store
        .insert_template(template)
        .map_err(|error| InduceReject::Store(error.to_string()))?;
    store.set_candidate_origin(
        &name,
        crate::store::CandidateOrigin {
            task_signature: a.task.clone(),
            session_id,
            distiller: distiller.to_string(),
            recorded_at,
        },
    );
    Ok(name)
}

struct Inducer<'a> {
    task_a: &'a str,
    task_b: &'a str,
    parameters: Vec<TemplateParameter>,
    /// (prefix, suffix) rule text -> parameter name.
    rules: BTreeMap<(String, String), String>,
    names: BTreeMap<String, usize>,
}

impl Inducer<'_> {
    fn parameterize(
        &mut self,
        value_a: &serde_json::Value,
        value_b: &serde_json::Value,
        hint: Option<&str>,
        at: usize,
    ) -> Result<serde_json::Value, InduceReject> {
        use serde_json::Value;
        match (value_a, value_b) {
            (Value::String(text_a), Value::String(text_b)) => {
                if text_a == text_b {
                    return Ok(value_a.clone());
                }
                let placeholder = self.induce_parameter(text_a, text_b, hint, at)?;
                Ok(Value::String(format!("${{{placeholder}}}")))
            }
            (Value::Array(items_a), Value::Array(items_b)) => {
                if items_a.len() != items_b.len() {
                    return Err(InduceReject::UnstableShape {
                        at,
                        reason: format!("array length {} vs {}", items_a.len(), items_b.len()),
                    });
                }
                let mut rendered = Vec::with_capacity(items_a.len());
                for (index, (item_a, item_b)) in items_a.iter().zip(items_b.iter()).enumerate() {
                    rendered.push(self.parameterize(item_a, item_b, None, at)?);
                    let _ = index;
                }
                Ok(Value::Array(rendered))
            }
            (Value::Object(map_a), Value::Object(map_b)) => {
                if map_a.len() != map_b.len() {
                    return Err(InduceReject::UnstableShape {
                        at,
                        reason: format!("object arity {} vs {}", map_a.len(), map_b.len()),
                    });
                }
                let mut rendered = serde_json::Map::new();
                for (key, item_a) in map_a {
                    let Some(item_b) = map_b.get(key) else {
                        return Err(InduceReject::UnstableShape {
                            at,
                            reason: format!("key '{key}' missing from the second run"),
                        });
                    };
                    rendered.insert(
                        key.clone(),
                        self.parameterize(item_a, item_b, Some(key), at)?,
                    );
                }
                Ok(Value::Object(rendered))
            }
            (left, right) if left == right => Ok(left.clone()),
            _ => Err(InduceReject::UnstableShape {
                at,
                reason: "non-string value differs between runs".to_string(),
            }),
        }
    }

    fn induce_parameter(
        &mut self,
        value_a: &str,
        value_b: &str,
        hint: Option<&str>,
        at: usize,
    ) -> Result<String, InduceReject> {
        if value_a.len() > MAX_PARAMETER_VALUE_LEN || value_b.len() > MAX_PARAMETER_VALUE_LEN {
            return Err(InduceReject::NotCapturable(value_a.to_string()));
        }
        if likely_ephemeral_identifier(value_a) || likely_ephemeral_identifier(value_b) {
            return Err(InduceReject::EphemeralIdentifier(value_a.to_string()));
        }
        if value_a.trim().is_empty() || value_b.trim().is_empty() {
            return Err(InduceReject::NotCapturable(value_a.to_string()));
        }

        // A parameter can only exist if the observed value is literally
        // present in the request that produced it. Everything after this is
        // anchor selection, and every accepted anchor must pass `binds_to`
        // against both runs.
        for index_a in occurrences(self.task_a, value_a) {
            for index_b in occurrences(self.task_b, value_b) {
                let before_a = &self.task_a[..index_a];
                let before_b = &self.task_b[..index_b];
                let after_a = &self.task_a[index_a + value_a.len()..];
                let after_b = &self.task_b[index_b + value_b.len()..];
                for prefix in anchor_before(before_a, before_b) {
                    for suffix in anchor_after(after_a, after_b) {
                        if prefix.is_none() && suffix.is_none() {
                            continue;
                        }
                        if !binds_to(prefix.as_deref(), suffix.as_deref(), self.task_a, value_a) {
                            continue;
                        }
                        if !binds_to(prefix.as_deref(), suffix.as_deref(), self.task_b, value_b) {
                            continue;
                        }
                        return Ok(self.record_parameter(
                            hint,
                            prefix,
                            suffix,
                            value_a,
                            value_b,
                        ));
                    }
                }
            }
        }
        let _ = at;
        Err(InduceReject::NotCapturable(value_a.to_string()))
    }

    fn record_parameter(
        &mut self,
        hint: Option<&str>,
        prefix: Option<String>,
        suffix: Option<String>,
        value_a: &str,
        value_b: &str,
    ) -> String {
        // Dedupe on the rule text; `None` and `""` are the same rule for
        // capture purposes but only `None` may be serialized, because an empty
        // suffix would capture the empty string.
        let key = (
            prefix.clone().unwrap_or_default(),
            suffix.clone().unwrap_or_default(),
        );
        if let Some(existing) = self.rules.get(&key) {
            return existing.clone();
        }
        let name = self.unique_name(hint, &key.0, &key.1);
        self.rules.insert(key.clone(), name.clone());
        self.parameters.push(TemplateParameter {
            name: name.clone(),
            source: "task".to_string(),
            kind: if looks_like_path(value_a, value_b) {
                PARAM_KIND_PATH.to_string()
            } else {
                PARAM_KIND_TEXT.to_string()
            },
            prefix,
            suffix,
            required: true,
        });
        name
    }

    fn unique_name(&mut self, hint: Option<&str>, prefix: &str, suffix: &str) -> String {
        let base = match hint {
            Some(hint) if !sanitize(hint).is_empty() => sanitize(hint),
            _ => {
                let from_prefix = last_alnum_run(prefix);
                if from_prefix.is_empty() {
                    let from_suffix = last_alnum_run(suffix);
                    if from_suffix.is_empty() {
                        "value".to_string()
                    } else {
                        from_suffix
                    }
                } else {
                    from_prefix
                }
            }
        };
        let counter = self.names.entry(base.clone()).or_insert(0);
        *counter += 1;
        if *counter == 1 {
            base
        } else {
            format!("{base}_{counter}")
        }
    }
}

/// The bind-check invariant: applying a candidate rule to the request text
/// must reproduce exactly the value that was observed in that run.
fn binds_to(prefix: Option<&str>, suffix: Option<&str>, source: &str, expected: &str) -> bool {
    capture_parameter(source, prefix, suffix).is_some_and(|value| value == expected)
}

fn common_prefix(a: &str, b: &str) -> String {
    let mut end = 0;
    for (left, right) in a.chars().zip(b.chars()) {
        if left != right {
            break;
        }
        end += left.len_utf8();
    }
    a[..end].to_string()
}

/// Up to `limit` characters from the end of `text`, on a char boundary.
fn tail_chars(text: &str, limit: usize) -> String {
    let count = text.chars().count();
    if count <= limit {
        return text.to_string();
    }
    text.chars().skip(count - limit).collect()
}

/// Up to `limit` characters from the start of `text`, on a char boundary.
fn head_chars(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

/// Byte indices where `value` occurs in `text` (first few, in order).
fn occurrences(text: &str, value: &str) -> Vec<usize> {
    if value.is_empty() {
        return Vec::new();
    }
    text.match_indices(value)
        .map(|(index, _)| index)
        .take(4)
        .collect()
}

/// Prefix anchors are read off the text that precedes the value in each run.
/// The longest candidate comes first (most specific), and every candidate is
/// validated by `binds_to` before use — these are proposals, not decisions.
fn anchor_before(before_a: &str, before_b: &str) -> Vec<Option<String>> {
    let _ = before_b;
    let mut candidates = Vec::new();
    let window = tail_chars(before_a, 24);
    // Drop a partially captured leading word so the anchor stays readable.
    let window = window
        .trim_start_matches(|character: char| character.is_alphanumeric() || character == '_')
        .to_string();
    if !window.trim().is_empty() {
        candidates.push(Some(window.clone()));
        let last_word = window
            .rsplit(char::is_whitespace)
            .find(|piece| !piece.trim().is_empty())
            .map(str::to_string);
        if let Some(last_word) = last_word {
            if last_word != window {
                candidates.push(Some(last_word));
            }
        }
    }
    candidates.push(None);
    candidates
}

/// Suffix anchors are read off the text that follows the value in each run.
fn anchor_after(after_a: &str, after_b: &str) -> Vec<Option<String>> {
    let _ = after_b;
    let mut candidates = Vec::new();
    let window = head_chars(after_a, 24);
    // Drop a partially captured trailing word.
    let window = window
        .trim_end_matches(|character: char| character.is_alphanumeric() || character == '_')
        .to_string();
    if !window.trim().is_empty() {
        candidates.push(Some(window.clone()));
        let first_word = window
            .split(char::is_whitespace)
            .find(|piece| !piece.trim().is_empty())
            .map(str::to_string);
        if let Some(first_word) = first_word {
            if first_word != window {
                candidates.push(Some(first_word));
            }
        }
    }
    candidates.push(None);
    candidates
}

fn looks_like_path(a: &str, b: &str) -> bool {
    [a, b].iter().any(|value| {
        (value.contains('/') || value.contains('\\'))
            // A URL is a token, not a workspace path: it must never inherit
            // the "no drive / no .." path constraints.
            && !value.contains("://")
            && !value.starts_with('/')
            && !value.starts_with('\\')
    })
}

fn sanitize(input: &str) -> String {
    let mut output = String::new();
    for character in input.chars() {
        if character.is_ascii_alphanumeric() {
            output.push(character.to_ascii_lowercase());
        } else if !output.ends_with('_') {
            output.push('_');
        }
    }
    output.trim_matches('_').to_string()
}

fn last_alnum_run(input: &str) -> String {
    input
        .rsplit(|character: char| !character.is_ascii_alphanumeric())
        .find(|run| !run.is_empty())
        .map(sanitize)
        .unwrap_or_default()
}

/// Stable addressing prefix shared by both requests. The trigger uses it so an
/// induced template still matches mechanically, on text, and nothing else.
fn task_signature_prefix(a: &str, b: &str) -> Result<String, InduceReject> {
    let common = common_prefix(&a.to_lowercase(), &b.to_lowercase());
    let trimmed = match common.rfind(char::is_whitespace) {
        Some(index) if index >= MIN_TASK_SIGNATURE_PREFIX => common[..index].to_string(),
        _ => common.clone(),
    };
    let trimmed = trimmed.trim().to_string();
    if trimmed.chars().count() < MIN_TASK_SIGNATURE_PREFIX {
        return Err(InduceReject::WeakTaskSignature {
            common_prefix: trimmed.chars().count(),
        });
    }
    Ok(trimmed)
}

/// Observable effects of a parameterized workflow.
///
/// This is an explicit, documented table — not inference. A step whose effect
/// is not in the table contributes no predicate; if nothing in the workflow
/// contributes a postcondition, induction refuses (the body could not be
/// verified, and an unverifiable body may never take over).
fn effects_for(
    workflow: &[WorkflowStep],
) -> (Vec<Predicate>, Vec<Predicate>, Vec<VerificationStep>) {
    let mut preconditions = Vec::new();
    let mut postconditions = Vec::new();
    let mut verification = Vec::new();
    let push_unique = |list: &mut Vec<Predicate>, predicate: Predicate| {
        if !list.iter().any(|existing| existing == &predicate) {
            list.push(predicate);
        }
    };
    for step in workflow {
        let path = |key: &str| {
            step.args
                .get(key)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        };
        match step.action.as_str() {
            "move_file" => {
                if let Some(source) = path("source") {
                    push_unique(&mut preconditions, exists(&source, true));
                    push_unique(&mut postconditions, exists(&source, false));
                }
                if let Some(target) = path("target") {
                    push_unique(&mut postconditions, exists(&target, true));
                }
            }
            "copy_file" => {
                if let Some(source) = path("source") {
                    push_unique(&mut preconditions, exists(&source, true));
                }
                if let Some(target) = path("target") {
                    push_unique(&mut postconditions, exists(&target, true));
                }
            }
            "delete_file" => {
                if let Some(target) = path("path") {
                    push_unique(&mut preconditions, exists(&target, true));
                    push_unique(&mut postconditions, exists(&target, false));
                }
            }
            "mkdir" | "write_file" | "append_file" => {
                let Some(target) = path("path") else {
                    continue;
                };
                push_unique(&mut postconditions, exists(&target, true));
                if step.action == "write_file" {
                    if let Some(content) = path("content") {
                        // Only literal content can be asserted; templated
                        // content would need a capture rule of its own.
                        if !content.contains("${") && content.len() <= 256 {
                            push_unique(
                                &mut postconditions,
                                Predicate::new(
                                    format!("file:{target}.content"),
                                    serde_json::Value::String(content),
                                ),
                            );
                        }
                    }
                }
            }
            "read_file" => {
                if let Some(target) = path("path") {
                    push_unique(&mut preconditions, exists(&target, true));
                }
            }
            // `exec` / `exec_command` have no file-shaped effect the local
            // probe can verify. They may appear inside a workflow that has
            // verifiable file effects; on their own they induce nothing.
            _ => {}
        }
    }
    // The runtime verifies through `verification` steps; postconditions are
    // the assertions. An induced body that named assertions but no way to
    // observe them could never be proven complete, so every postcondition gets
    // its matching probe here.
    verification.extend(
        postconditions
            .iter()
            .cloned()
            .map(|predicate| VerificationStep::Probe { predicate }),
    );
    (preconditions, postconditions, verification)
}

fn exists(path: &str, expected: bool) -> Predicate {
    Predicate::new(
        format!("file:{path}.exists"),
        serde_json::Value::Bool(expected),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::action::ActionProposal;
    use crate::domain::experience::QualificationAction;
    use std::path::Path;

    fn move_observation(task: &str, source: &str, target: &str) -> ObservedTask {
        ObservedTask::new(
            task,
            vec![ObservedStep::new(
                "move_file",
                serde_json::json!({ "source": source, "target": target }),
            )],
            true,
        )
    }

    #[test]
    fn induces_a_binding_template_from_two_moves() {
        let a = move_observation(
            "Move inbox reports into the library: inbox/alpha.pdf -> library/alpha.pdf",
            "inbox/alpha.pdf",
            "library/alpha.pdf",
        );
        let b = move_observation(
            "Move inbox reports into the library: inbox/beta.pdf -> library/beta.pdf",
            "inbox/beta.pdf",
            "library/beta.pdf",
        );
        let template = induce(&a, &b).expect("two structured moves must induce");
        assert_eq!(template.status, ExperienceStatus::Candidate);
        assert_eq!(template.parameters.len(), 2);
        let names: Vec<&str> = template
            .parameters
            .iter()
            .map(|parameter| parameter.name.as_str())
            .collect();
        assert!(names.contains(&"source"), "{names:?}");
        assert!(names.contains(&"target"), "{names:?}");

        // The induced rule must bind a *third* pair it never saw.
        let third = ActionProposal::new(
            "task",
            serde_json::json!({
                "task": "Move inbox reports into the library: inbox/gamma.pdf -> library/gamma.pdf"
            }),
        );
        let (instance, bindings) = template.bind(&third, Path::new("C:/ws")).unwrap();
        assert_eq!(bindings["source"], "inbox/gamma.pdf");
        assert_eq!(bindings["target"], "library/gamma.pdf");
        assert_eq!(
            instance.workflow[0].args,
            serde_json::json!({
                "source": "inbox/gamma.pdf",
                "target": "library/gamma.pdf"
            })
        );
        assert!(instance
            .preconditions
            .iter()
            .any(|predicate| predicate.key == "file:inbox/gamma.pdf.exists"));
        assert!(instance
            .postconditions
            .iter()
            .any(|predicate| predicate.key == "file:library/gamma.pdf.exists"));
    }

    #[test]
    fn refuses_when_the_capture_rule_cannot_reproduce_the_observation() {
        // The value varies, but it never appears in the request text, so no
        // rule can bind it: this must be refused, not guessed.
        let a = ObservedTask::new(
            "Archive the quarterly report",
            vec![ObservedStep::new(
                "move_file",
                serde_json::json!({ "source": "q1.pdf", "target": "done/q1.pdf" }),
            )],
            true,
        );
        let b = ObservedTask::new(
            "Archive the quarterly report",
            vec![ObservedStep::new(
                "move_file",
                serde_json::json!({ "source": "q2.pdf", "target": "done/q2.pdf" }),
            )],
            true,
        );
        assert!(matches!(
            induce(&a, &b),
            Err(InduceReject::NotCapturable(_))
        ));
    }

    #[test]
    fn refuses_ephemeral_identifiers_and_identical_runs() {
        let a = ObservedTask::new(
            "Publish build artifact for release",
            vec![ObservedStep::new(
                "write_file",
                serde_json::json!({
                    "path": "out/report.txt",
                    "content": "commit 4f3a9c1b2d5e6f708192a3b4c5d6e7f8 verified"
                }),
            )],
            true,
        );
        let b = ObservedTask::new(
            "Publish build artifact for release",
            vec![ObservedStep::new(
                "write_file",
                serde_json::json!({
                    "path": "out/report.txt",
                    "content": "commit 9e8d7c6b5a4938271605f4e3d2c1b0a9 verified"
                }),
            )],
            true,
        );
        assert!(matches!(
            induce(&a, &b),
            Err(InduceReject::EphemeralIdentifier(_))
        ));

        let same = move_observation(
            "Move inbox/alpha.pdf into library",
            "inbox/alpha.pdf",
            "library/alpha.pdf",
        );
        assert_eq!(induce(&same, &same), Err(InduceReject::NoParameterDifference));
    }

    #[test]
    fn refuses_a_failed_or_unverifiable_observation() {
        let mut failed = move_observation(
            "Move inbox reports into the library: inbox/alpha.pdf -> library/alpha.pdf",
            "inbox/alpha.pdf",
            "library/alpha.pdf",
        );
        failed.succeeded = false;
        let ok = move_observation(
            "Move inbox reports into the library: inbox/beta.pdf -> library/beta.pdf",
            "inbox/beta.pdf",
            "library/beta.pdf",
        );
        assert!(matches!(
            induce(&failed, &ok),
            Err(InduceReject::TaskNotSuccessful { arm: "a" })
        ));

        // Two exec-only runs have no probe-observable effect.
        let exec_a = ObservedTask::new(
            "Inspect the upstream repository https://github.com/a/one and report",
            vec![ObservedStep::new(
                "exec",
                serde_json::json!({ "program": "git", "args": ["ls-remote", "https://github.com/a/one"] }),
            )],
            true,
        );
        let exec_b = ObservedTask::new(
            "Inspect the upstream repository https://github.com/b/two and report",
            vec![ObservedStep::new(
                "exec",
                serde_json::json!({ "program": "git", "args": ["ls-remote", "https://github.com/b/two"] }),
            )],
            true,
        );
        assert!(matches!(
            induce(&exec_a, &exec_b),
            Err(InduceReject::NoVerifiableEffect)
        ));
    }

    #[test]
    fn induced_name_is_stable_across_runs_of_the_same_family() {
        let a = move_observation(
            "Move inbox reports into the library: inbox/alpha.pdf -> library/alpha.pdf",
            "inbox/alpha.pdf",
            "library/alpha.pdf",
        );
        let b = move_observation(
            "Move inbox reports into the library: inbox/beta.pdf -> library/beta.pdf",
            "inbox/beta.pdf",
            "library/beta.pdf",
        );
        let c = move_observation(
            "Move inbox reports into the library: inbox/gamma.pdf -> library/gamma.pdf",
            "inbox/gamma.pdf",
            "library/gamma.pdf",
        );
        assert_eq!(induce(&a, &b).unwrap().name, induce(&b, &c).unwrap().name);
    }

    #[test]
    fn induced_candidate_is_inert_until_qualified() {
        use crate::store::ExperienceStore;

        let a = move_observation(
            "Move inbox reports into the library: inbox/alpha.pdf -> library/alpha.pdf",
            "inbox/alpha.pdf",
            "library/alpha.pdf",
        );
        let b = move_observation(
            "Move inbox reports into the library: inbox/beta.pdf -> library/beta.pdf",
            "inbox/beta.pdf",
            "library/beta.pdf",
        );
        let mut store = ExperienceStore::default();
        let name = induce_into_store(&mut store, &a, &b, "deterministic", None, 1).unwrap();

        // Inert while CANDIDATE: the Gate's template index does not see it.
        assert_eq!(store.template(&name).unwrap().status, ExperienceStatus::Candidate);
        let probe = ActionProposal::new(
            "task",
            serde_json::json!({
                "task": "Move inbox reports into the library: inbox/gamma.pdf -> library/gamma.pdf"
            }),
        );
        assert!(store.templates_for(&probe).is_empty());

        // Qualification: Validate then Activate, through the transition table.
        store
            .transition_template_status(&name, QualificationAction::Validate)
            .unwrap();
        store
            .transition_template_status(&name, QualificationAction::Activate)
            .unwrap();
        assert_eq!(store.templates_for(&probe).len(), 1);

        // Provenance survived.
        let origin = store.candidate_origin_of(&name).unwrap();
        assert_eq!(origin.distiller, "deterministic");
    }
}

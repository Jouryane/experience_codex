//! Parameterized Experience templates.
//!
//! A template is not executable. It becomes executable only after every
//! required parameter is captured deterministically and all placeholders are
//! rendered into a concrete `Experience`. This is the middle contract between
//! an exact instance and a non-executable reference.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;
use serde::Serialize;

use super::action::ActionPattern;
use super::action::ActionProposal;
use super::experience::Experience;
use super::experience::ExperienceStatus;
use super::experience::FailurePolicy;
use super::experience::SchemaIssue;
use super::experience::UndoPolicy;
use super::experience::VerificationStep;
use super::experience::WorkflowStep;
use super::predicate::Predicate;

/// Deterministic parameter capture rule.
///
/// The `kind` is a *constraint checked at bind time*, never a hint for a model
/// to fill in. Binding is a deterministic function of the request text and the
/// workspace path; if a required value cannot be captured and validated, the
/// template refuses to bind and the Gate falls through to the native path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateParameter {
    pub name: String,
    /// `task` | `action` | `cwd`
    pub source: String,
    /// `text` (default) | `path` | `token` — see `validate_value`.
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suffix: Option<String>,
    #[serde(default = "default_required")]
    pub required: bool,
}

fn default_required() -> bool {
    true
}

fn default_kind() -> String {
    PARAM_KIND_TEXT.to_string()
}

/// Unconstrained text.
pub const PARAM_KIND_TEXT: &str = "text";
/// A workspace-relative path: no absolute root, no drive letter, no `..`.
pub const PARAM_KIND_PATH: &str = "path";
/// A single token safe to place inside a command argument: no whitespace, no
/// shell metacharacters.
pub const PARAM_KIND_TOKEN: &str = "token";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateBindError {
    MissingParameter(String),
    MalformedCapture(String),
    UnknownPlaceholder(String),
    EmptyParameter(String),
    InvalidParameter {
        name: String,
        reason: String,
    },
}

impl std::fmt::Display for TemplateBindError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingParameter(name) => write!(formatter, "missing parameter '{name}'"),
            Self::MalformedCapture(name) => write!(formatter, "malformed capture for '{name}'"),
            Self::UnknownPlaceholder(name) => write!(formatter, "unknown placeholder '{name}'"),
            Self::EmptyParameter(name) => write!(formatter, "empty parameter '{name}'"),
            Self::InvalidParameter { name, reason } => {
                write!(formatter, "invalid parameter '{name}': {reason}")
            }
        }
    }
}

impl std::error::Error for TemplateBindError {}

/// A parameterized, non-executable experience body. `bind` renders it into a
/// concrete `Experience`; callers must never execute the template directly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExperienceTemplate {
    pub name: String,
    pub trigger: ActionPattern,
    pub parameters: Vec<TemplateParameter>,
    pub workflow: Vec<WorkflowStep>,
    pub preconditions: Vec<Predicate>,
    pub postconditions: Vec<Predicate>,
    pub verification: Vec<VerificationStep>,
    pub failure_policy: FailurePolicy,
    pub undo: UndoPolicy,
    pub status: ExperienceStatus,
}

impl ExperienceTemplate {
    pub fn schema_issues(&self) -> Vec<SchemaIssue> {
        let mut issues = Vec::new();
        if self.name.trim().is_empty() {
            issues.push(SchemaIssue::EmptyName);
        }
        if self.trigger.tool.trim().is_empty() {
            issues.push(SchemaIssue::EmptyTriggerTool);
        }
        if self.workflow.is_empty() {
            issues.push(SchemaIssue::EmptyWorkflow);
        }
        if self.postconditions.is_empty() {
            issues.push(SchemaIssue::EmptyPostconditions);
        }
        issues
    }

    pub fn is_schema_valid(&self) -> bool {
        self.schema_issues().is_empty()
    }

    /// Capture every parameter from the proposal/context, render the template,
    /// and return the concrete executable Experience plus binding provenance.
    pub fn bind(
        &self,
        proposal: &ActionProposal,
        cwd: &Path,
    ) -> Result<(Experience, BTreeMap<String, String>), TemplateBindError> {
        let mut bindings = BTreeMap::new();
        for parameter in &self.parameters {
            let source = source_text(&parameter.source, proposal, cwd);
            match capture(&source, parameter) {
                Some(value) if !value.is_empty() => {
                    validate_value(parameter, &value)?;
                    bindings.insert(parameter.name.clone(), value);
                }
                Some(_) if parameter.required => {
                    return Err(TemplateBindError::EmptyParameter(parameter.name.clone()));
                }
                None if parameter.required => {
                    return Err(TemplateBindError::MissingParameter(parameter.name.clone()));
                }
                _ => {}
            }
        }

        let experience = Experience {
            name: self.name.clone(),
            trigger: self.trigger.clone(),
            preconditions: render_predicates(&self.preconditions, &bindings)?,
            workflow: render_steps(&self.workflow, &bindings)?,
            postconditions: render_predicates(&self.postconditions, &bindings)?,
            verification: render_verification(&self.verification, &bindings)?,
            failure_policy: self.failure_policy,
            undo: self.undo,
            status: self.status,
        };
        Ok((experience, bindings))
    }

    /// Deterministic fingerprint of a bound instance: template name plus the
    /// captured bindings. Used for audit trails so a recorded hit can be
    /// traced back to the exact parameters that produced it.
    pub fn binding_fingerprint(&self, bindings: &BTreeMap<String, String>) -> String {
        let mut material = self.name.clone();
        for (name, value) in bindings {
            material.push('\u{1f}');
            material.push_str(name);
            material.push('=');
            material.push_str(value);
        }
        stable_fingerprint(&material)
    }
}

/// Stable, dependency-free FNV-1a fingerprint (16 hex chars).
///
/// This is an integrity/audit fingerprint, not a cryptographic hash: it must
/// stay stable across builds and platforms, which rules out `DefaultHasher`.
pub fn stable_fingerprint(material: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in material.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Bind-time constraint check for one captured value.
fn validate_value(parameter: &TemplateParameter, value: &str) -> Result<(), TemplateBindError> {
    let invalid = |reason: &str| TemplateBindError::InvalidParameter {
        name: parameter.name.clone(),
        reason: reason.to_string(),
    };
    match parameter.kind.as_str() {
        PARAM_KIND_PATH => {
            let normalized = value.replace('\\', "/");
            if normalized.starts_with('/') {
                return Err(invalid("path must be workspace-relative, not absolute"));
            }
            // `C:/...` or `C:...` — a drive-relative/absolute Windows path.
            let bytes = normalized.as_bytes();
            if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
                return Err(invalid("path must not name a drive"));
            }
            if normalized
                .split('/')
                .any(|component| component == "..")
            {
                return Err(invalid("path must not contain a '..' component"));
            }
            Ok(())
        }
        PARAM_KIND_TOKEN => {
            if value.chars().any(char::is_whitespace) {
                return Err(invalid("token must not contain whitespace"));
            }
            if value
                .chars()
                .any(|character| ";&|<>$`\"'\n\r".contains(character))
            {
                return Err(invalid("token must not contain shell metacharacters"));
            }
            Ok(())
        }
        // `text`, and any unknown kind, stay permissive: unknown kinds must
        // not silently become a new execution constraint.
        _ => Ok(()),
    }
}

fn source_text(source: &str, proposal: &ActionProposal, cwd: &Path) -> String {
    match source {
        "task" => proposal
            .args
            .get("task")
            .or_else(|| proposal.args.get("text"))
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| proposal.args_text()),
        "action" => proposal.args_text(),
        "cwd" => cwd.to_string_lossy().into_owned(),
        _ => proposal.args_text(),
    }
}

fn capture(source: &str, parameter: &TemplateParameter) -> Option<String> {
    capture_parameter(
        source,
        parameter.prefix.as_deref(),
        parameter.suffix.as_deref(),
    )
}

/// The one capture rule implementation in the codebase.
///
/// Template binding and template *induction* both go through this function, so
/// a capture rule recorded by the inducer is guaranteed to behave the same way
/// when the Gate binds it later. First match wins; the value is trimmed.
pub fn capture_parameter(
    source: &str,
    prefix: Option<&str>,
    suffix: Option<&str>,
) -> Option<String> {
    let start = match prefix {
        Some(prefix) => source.find(prefix)? + prefix.len(),
        None => 0,
    };
    let tail = &source[start..];
    let end = match suffix {
        Some(suffix) => tail.find(suffix)?,
        None => tail.len(),
    };
    Some(tail[..end].trim().to_string())
}

fn render_steps(
    steps: &[WorkflowStep],
    bindings: &BTreeMap<String, String>,
) -> Result<Vec<WorkflowStep>, TemplateBindError> {
    steps
        .iter()
        .map(|step| {
            Ok(WorkflowStep {
                action: step.action.clone(),
                args: render_value(&step.args, bindings)?,
            })
        })
        .collect()
}

fn render_predicates(
    predicates: &[Predicate],
    bindings: &BTreeMap<String, String>,
) -> Result<Vec<Predicate>, TemplateBindError> {
    predicates
        .iter()
        .map(|predicate| {
            Ok(Predicate {
                key: render_string(&predicate.key, bindings)?,
                expected: render_value(&predicate.expected, bindings)?,
            })
        })
        .collect()
}

fn render_verification(
    steps: &[VerificationStep],
    bindings: &BTreeMap<String, String>,
) -> Result<Vec<VerificationStep>, TemplateBindError> {
    steps
        .iter()
        .map(|step| match step {
            VerificationStep::ReadFile {
                path,
                expect_content,
            } => Ok(VerificationStep::ReadFile {
                path: render_string(path, bindings)?,
                expect_content: expect_content
                    .as_deref()
                    .map(|content| render_string(content, bindings))
                    .transpose()?,
            }),
            VerificationStep::Probe { predicate } => Ok(VerificationStep::Probe {
                predicate: render_predicates(
                    std::slice::from_ref(predicate),
                    bindings,
                )?
                .into_iter()
                .next()
                .expect("one predicate"),
            }),
        })
        .collect()
}

fn render_value(
    value: &serde_json::Value,
    bindings: &BTreeMap<String, String>,
) -> Result<serde_json::Value, TemplateBindError> {
    match value {
        serde_json::Value::String(text) => {
            Ok(serde_json::Value::String(render_string(text, bindings)?))
        }
        serde_json::Value::Array(items) => items
            .iter()
            .map(|item| render_value(item, bindings))
            .collect::<Result<Vec<_>, _>>()
            .map(serde_json::Value::Array),
        serde_json::Value::Object(map) => map
            .iter()
            .map(|(key, value)| Ok((key.clone(), render_value(value, bindings)?)))
            .collect::<Result<serde_json::Map<_, _>, _>>()
            .map(serde_json::Value::Object),
        other => Ok(other.clone()),
    }
}

fn render_string(
    input: &str,
    bindings: &BTreeMap<String, String>,
) -> Result<String, TemplateBindError> {
    let mut output = String::new();
    let mut rest = input;
    while let Some(start) = rest.find("${") {
        output.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            return Err(TemplateBindError::MalformedCapture(input.to_string()));
        };
        let name = &after[..end];
        let Some(value) = bindings.get(name) else {
            return Err(TemplateBindError::UnknownPlaceholder(name.to_string()));
        };
        output.push_str(value);
        rest = &after[end + 1..];
    }
    output.push_str(rest);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::experience::WorkflowStep;

    fn move_template() -> ExperienceTemplate {
        ExperienceTemplate {
            name: "move_any_file".into(),
            trigger: ActionPattern {
                tool: "task".into(),
                command_pattern: Some("M9 move".into()),
            },
            parameters: vec![
                TemplateParameter {
                    name: "source".into(),
                    source: "task".into(),
                    kind: PARAM_KIND_PATH.into(),
                    prefix: Some("[source=".into()),
                    suffix: Some("]".into()),
                    required: true,
                },
                TemplateParameter {
                    name: "target".into(),
                    source: "task".into(),
                    kind: PARAM_KIND_PATH.into(),
                    prefix: Some("[target=".into()),
                    suffix: Some("]".into()),
                    required: true,
                },
            ],
            workflow: vec![WorkflowStep::new(
                "move_file",
                serde_json::json!({
                    "source": "${source}",
                    "target": "${target}"
                }),
            )],
            preconditions: vec![Predicate::new(
                "file:${source}.exists",
                serde_json::json!(true),
            )],
            postconditions: vec![
                Predicate::new("file:${source}.exists", serde_json::json!(false)),
                Predicate::new("file:${target}.exists", serde_json::json!(true)),
            ],
            verification: vec![],
            failure_policy: FailurePolicy::StopAndReport,
            undo: UndoPolicy::Unsupported,
            status: ExperienceStatus::Active,
        }
    }

    #[test]
    fn binds_marked_task_into_exact_experience() {
        let template = move_template();
        let proposal = ActionProposal::new(
            "task",
            serde_json::json!({
                "task": "M9 move [source=inbox/a.pdf] [target=library/a.pdf]"
            }),
        );
        let (experience, bindings) = template.bind(&proposal, Path::new("C:/ws")).unwrap();
        assert_eq!(bindings["source"], "inbox/a.pdf");
        assert_eq!(bindings["target"], "library/a.pdf");
        assert_eq!(
            experience.workflow[0].args,
            serde_json::json!({
                "source": "inbox/a.pdf",
                "target": "library/a.pdf"
            })
        );
        assert_eq!(experience.preconditions[0].key, "file:inbox/a.pdf.exists");
        assert_eq!(experience.postconditions[1].key, "file:library/a.pdf.exists");
    }

    #[test]
    fn missing_required_parameter_refuses_to_bind() {
        let template = move_template();
        let proposal = ActionProposal::new(
            "task",
            serde_json::json!({ "task": "M9 move [source=inbox/a.pdf]" }),
        );
        assert!(matches!(
            template.bind(&proposal, Path::new("C:/ws")),
            Err(TemplateBindError::MissingParameter(name)) if name == "target"
        ));
    }

    #[test]
    fn path_parameter_rejects_escape_and_absolute() {
        let template = move_template();
        for bad in [
            "M9 move [source=../outside/a.pdf] [target=library/a.pdf]",
            "M9 move [source=C:/outside/a.pdf] [target=library/a.pdf]",
            "M9 move [source=/etc/passwd] [target=library/a.pdf]",
        ] {
            let proposal = ActionProposal::new("task", serde_json::json!({ "task": bad }));
            let error = template.bind(&proposal, Path::new("C:/ws")).unwrap_err();
            assert!(
                matches!(error, TemplateBindError::InvalidParameter { .. }),
                "{bad} -> {error}"
            );
        }
        let ok = ActionProposal::new(
            "task",
            serde_json::json!({
                "task": "M9 move [source=inbox/a.pdf] [target=lib/sub/a.pdf]"
            }),
        );
        assert!(template.bind(&ok, Path::new("C:/ws")).is_ok());
    }

    #[test]
    fn token_parameter_rejects_shell_metacharacters() {
        let mut template = move_template();
        template.parameters = vec![TemplateParameter {
            name: "ref".into(),
            source: "task".into(),
            kind: PARAM_KIND_TOKEN.into(),
            prefix: Some("[ref=".into()),
            suffix: Some("]".into()),
            required: true,
        }];
        template.workflow = vec![WorkflowStep::new(
            "exec",
            serde_json::json!({ "program": "git", "args": ["ls-remote", "${ref}"] }),
        )];
        template.preconditions = vec![];
        template.postconditions = vec![Predicate::new("cwd.exists", serde_json::json!(true))];

        let good = ActionProposal::new(
            "task",
            serde_json::json!({ "task": "probe [ref=https://github.com/openai/codex]" }),
        );
        let (experience, bindings) = template.bind(&good, Path::new("C:/ws")).unwrap();
        assert_eq!(bindings["ref"], "https://github.com/openai/codex");
        assert_eq!(
            experience.workflow[0].args["args"][1],
            serde_json::json!("https://github.com/openai/codex")
        );

        let bad = ActionProposal::new(
            "task",
            serde_json::json!({ "task": "probe [ref=github.com; rm -rf /]" }),
        );
        assert!(matches!(
            template.bind(&bad, Path::new("C:/ws")),
            Err(TemplateBindError::InvalidParameter { .. })
        ));
    }

    #[test]
    fn binding_fingerprint_is_stable_and_binding_sensitive() {
        let template = move_template();
        let mut a = BTreeMap::new();
        a.insert("source".to_string(), "inbox/a.pdf".to_string());
        let mut b = a.clone();
        b.insert("source".to_string(), "inbox/b.pdf".to_string());
        assert_eq!(template.binding_fingerprint(&a), template.binding_fingerprint(&a));
        assert_ne!(template.binding_fingerprint(&a), template.binding_fingerprint(&b));
    }
}

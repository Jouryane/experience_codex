//! Deterministic producer-name → canonical-capability translation.
//!
//! A real agent does not emit `move_file`; it emits `shell` with
//! `{"command": "mv inbox/a.zip archive/a.zip"}` or `apply_patch` with a patch
//! body. A workflow is written in canonical capabilities, so a trace has to be
//! translated before it can be induced from.
//!
//! This module is that translator, and it is deliberately a *table with a
//! refusal*, not a language model:
//!
//! - only the command shapes listed below are recognised;
//! - compound shell (pipes, command substitution, globs, background jobs,
//!   `cd`, variable expansion) is refused outright, because a body we cannot
//!   read exactly is a body we must not replay;
//! - every refusal is reported by name so the learning layer can say *why* it
//!   declined instead of silently dropping a step;
//! - a refused trace yields no candidate at all. Partial translation is never
//!   stored, because a workflow missing a step would take over and do less
//!   than the task asks.

use crate::template_induce::ObservedStep;
use crate::template_induce::ObservedTask;

/// Why a trace cannot be expressed in canonical capabilities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranslateReject {
    /// The tool itself has no canonical equivalent.
    UnsupportedTool(String),
    /// The tool is known but this particular invocation is not translatable.
    UnsupportedCommand { tool: String, command: String, reason: String },
}

impl TranslateReject {
    pub fn label(&self) -> &'static str {
        match self {
            Self::UnsupportedTool(_) => "unsupported_tool",
            Self::UnsupportedCommand { .. } => "unsupported_command",
        }
    }
}

impl std::fmt::Display for TranslateReject {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedTool(tool) => {
                write!(formatter, "tool '{tool}' has no canonical capability")
            }
            Self::UnsupportedCommand { tool, command, reason } => {
                write!(formatter, "{tool}('{command}') is not translatable: {reason}")
            }
        }
    }
}

impl std::error::Error for TranslateReject {}

/// Translate one observed task into canonical steps.
pub fn canonicalize(task: &ObservedTask) -> Result<ObservedTask, TranslateReject> {
    // One observation must be internally consistent: every step that declares
    // a working directory must declare the *same* one. The absolute value does
    // not matter — canonical steps are workspace-relative, which is exactly
    // what lets two runs of one family be compared even when they happened in
    // two different directories (or on two different machines).
    let mut declared: Option<String> = None;
    let mut steps = Vec::new();
    for step in &task.steps {
        if let Some(workdir) = declared_workdir(step) {
            match &declared {
                None => declared = Some(workdir),
                Some(first) if same_path(first, &workdir) => {}
                Some(first) => {
                    return Err(TranslateReject::UnsupportedCommand {
                        tool: step.action.clone(),
                        command: workdir,
                        reason: format!(
                            "steps of one observation ran in different working directories \
                             (first '{first}')"
                        ),
                    })
                }
            }
        }
        steps.extend(translate_step(step)?);
    }
    Ok(ObservedTask {
        task: task.task.clone(),
        steps,
        succeeded: task.succeeded,
    })
}

/// Same as [`canonicalize`], but refuses steps that ran outside
/// `workspace`: a relative path only means what the observation says it means
/// when the working directory matches, so a different `workdir` is refused
/// rather than silently re-based.
pub fn canonicalize_in_workspace(
    task: &ObservedTask,
    workspace: Option<&std::path::Path>,
) -> Result<ObservedTask, TranslateReject> {
    let mut steps = Vec::new();
    for step in &task.steps {
        guard_workdir(step, workspace)?;
        steps.extend(translate_step(step)?);
    }
    Ok(ObservedTask {
        task: task.task.clone(),
        steps,
        succeeded: task.succeeded,
    })
}

fn guard_workdir(
    step: &ObservedStep,
    workspace: Option<&std::path::Path>,
) -> Result<(), TranslateReject> {
    let Some(workspace) = workspace else {
        return Ok(());
    };
    let Some(declared) = declared_workdir(step) else {
        return Ok(());
    };
    if same_path(&declared, &workspace.to_string_lossy()) {
        return Ok(());
    }
    Err(TranslateReject::UnsupportedCommand {
        tool: step.action.clone(),
        command: declared,
        reason: format!(
            "step ran outside the observation workspace ({})",
            workspace.display()
        ),
    })
}

fn declared_workdir(step: &ObservedStep) -> Option<String> {
    step.args
        .get("workdir")
        .or_else(|| step.args.get("cwd"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// Two working directories are "the same" for observation purposes when the
/// paths resolve to the same place; if neither resolves (e.g. the directory is
/// gone), fall back to a case-insensitive string comparison.
fn same_path(left: &str, right: &str) -> bool {
    let left_path = std::path::Path::new(left);
    let right_path = std::path::Path::new(right);
    match (left_path.canonicalize(), right_path.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left.eq_ignore_ascii_case(right),
    }
}

/// Translate one tool call. Canonical names pass through unchanged so a trace
/// that already speaks the vocabulary is never re-interpreted.
pub fn translate_step(step: &ObservedStep) -> Result<Vec<ObservedStep>, TranslateReject> {
    match step.action.as_str() {
        "shell" | "exec_command" => translate_shell(step),
        "apply_patch" => translate_patch(step),
        // Polling a session for output already in flight is transport noise,
        // not a capability step: it changes nothing in the world and must not
        // become a workflow step. Feeding input is a different matter and is
        // refused, because replaying it would drive a process we did not model.
        "write_stdin" => match step.args.get("chars").and_then(serde_json::Value::as_str) {
            None | Some("") => Ok(Vec::new()),
            Some(chars) => Err(TranslateReject::UnsupportedCommand {
                tool: "write_stdin".to_string(),
                command: chars.to_string(),
                reason: "input to a live session cannot be replayed".to_string(),
            }),
        },
        "exec" | "read_file" | "write_file" | "append_file" | "mkdir" | "copy_file"
        | "move_file" | "delete_file" => Ok(vec![step.clone()]),
        other => Err(TranslateReject::UnsupportedTool(other.to_string())),
    }
}

fn translate_shell(step: &ObservedStep) -> Result<Vec<ObservedStep>, TranslateReject> {
    let command = step
        .args
        .get("command")
        .and_then(serde_json::Value::as_str)
        .or_else(|| step.args.get("cmd").and_then(serde_json::Value::as_str))
        .ok_or_else(|| TranslateReject::UnsupportedCommand {
            tool: step.action.clone(),
            command: step.args.to_string(),
            reason: "no command string".to_string(),
        })?;
    let reject = |reason: &str| TranslateReject::UnsupportedCommand {
        tool: step.action.clone(),
        command: command.to_string(),
        reason: reason.to_string(),
    };

    if command.contains('|')
        || command.contains("$(")
        || command.contains('`')
        || command.contains('*')
        || command.contains('&')
            && !command.contains("&&")
    {
        return Err(reject("pipe, substitution, glob or background job"));
    }

    let mut translated = Vec::new();
    for segment in command.split("&&").flat_map(|part| part.split(';')) {
        let segment = segment.trim();
        if segment.is_empty() {
            continue;
        }
        if segment.starts_with("cd ") {
            // A `cd` changes what the following paths mean; the canonical
            // workflow has one working directory, so this is refused rather
            // than silently reinterpreted.
            return Err(reject("cd is not part of a canonical workflow"));
        }
        translated.push(translate_segment(segment).map_err(|reason| reject(&reason))?);
    }
    if translated.is_empty() {
        return Err(reject("empty command"));
    }
    Ok(translated)
}

fn translate_segment(segment: &str) -> Result<ObservedStep, String> {
    // Simple output redirection: `<writer> <text> > <path>`.
    if let Some((left, right)) = split_redirect(segment) {
        let tokens = tokenize(left)?;
        let program = tokens.first().map(|token| token.to_lowercase()).unwrap_or_default();
        let target = right.trim().trim_matches('"').trim_matches('\'').to_string();
        if target.is_empty() {
            return Err("redirect without a path".to_string());
        }
        let content = match program.as_str() {
            "echo" | "write-output" => tokens[1..].join(" "),
            "printf" => tokens[1..].join(" "),
            other => return Err(format!("'{other}' with redirect is not translatable")),
        };
        return Ok(ObservedStep::new(
            "write_file",
            serde_json::json!({ "path": target, "content": content }),
        ));
    }

    let tokens = tokenize(segment)?;
    let Some(first) = tokens.first() else {
        return Err("empty segment".to_string());
    };
    let program = first.to_lowercase();
    let rest = &tokens[1..];
    match program.as_str() {
        "git" | "python" | "python3" | "py" | "node" | "cargo" | "go" | "npm" | "npx" => {
            Ok(ObservedStep::new(
                "exec",
                serde_json::json!({ "program": program, "args": rest }),
            ))
        }
        "mv" | "move" => {
            let (source, target) = two_paths(rest)?;
            Ok(ObservedStep::new(
                "move_file",
                serde_json::json!({ "source": source, "target": target }),
            ))
        }
        "move-item" => {
            let source = flag_value(rest, &["-path", "-literalpath"])?;
            let target = flag_value(rest, &["-destination"])?;
            Ok(ObservedStep::new(
                "move_file",
                serde_json::json!({ "source": source, "target": target }),
            ))
        }
        "cp" | "copy" => {
            let (source, target) = two_paths(rest)?;
            Ok(ObservedStep::new(
                "copy_file",
                serde_json::json!({ "source": source, "target": target }),
            ))
        }
        "copy-item" => {
            let source = flag_value(rest, &["-path", "-literalpath"])?;
            let target = flag_value(rest, &["-destination"])?;
            Ok(ObservedStep::new(
                "copy_file",
                serde_json::json!({ "source": source, "target": target }),
            ))
        }
        "mkdir" | "md" => {
            let path = rest
                .iter()
                .rev()
                .find(|token| !token.starts_with('-'))
                .cloned()
                .ok_or_else(|| "mkdir without a path".to_string())?;
            Ok(ObservedStep::new(
                "mkdir",
                serde_json::json!({ "path": path }),
            ))
        }
        "new-item" => {
            if !segment.to_lowercase().contains("directory") {
                return Err("new-item is only translatable for -ItemType Directory".to_string());
            }
            let path = flag_value(rest, &["-path", "-literalpath"])?;
            Ok(ObservedStep::new(
                "mkdir",
                serde_json::json!({ "path": path }),
            ))
        }
        "rm" | "del" | "erase" => {
            let path = rest
                .iter()
                .rev()
                .find(|token| !token.starts_with('-'))
                .cloned()
                .ok_or_else(|| "rm without a path".to_string())?;
            Ok(ObservedStep::new(
                "delete_file",
                serde_json::json!({ "path": path }),
            ))
        }
        "remove-item" => {
            let path = flag_value(rest, &["-path", "-literalpath"])?;
            Ok(ObservedStep::new(
                "delete_file",
                serde_json::json!({ "path": path }),
            ))
        }
        "set-content" => {
            let path = flag_value(rest, &["-path", "-literalpath"])?;
            let content = flag_value(rest, &["-value"]).unwrap_or_default();
            Ok(ObservedStep::new(
                "write_file",
                serde_json::json!({ "path": path, "content": content }),
            ))
        }
        other => Err(format!("'{other}' is not in the translatable vocabulary")),
    }
}

/// `apply_patch` → `write_file` for pure file additions. Update/delete hunks
/// are refused: a patch is not a canonical capability, and replaying a partial
/// interpretation of one is worse than declining.
fn translate_patch(step: &ObservedStep) -> Result<Vec<ObservedStep>, TranslateReject> {
    let patch = step
        .args
        .get("input")
        .or_else(|| step.args.get("patch"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| TranslateReject::UnsupportedCommand {
            tool: "apply_patch".to_string(),
            command: step.args.to_string(),
            reason: "no patch body".to_string(),
        })?;
    let reject = |reason: &str| TranslateReject::UnsupportedCommand {
        tool: "apply_patch".to_string(),
        command: patch.to_string(),
        reason: reason.to_string(),
    };
    let mut translated = Vec::new();
    let mut current_path: Option<String> = None;
    let mut content: Vec<String> = Vec::new();
    let flush = |path: &Option<String>, content: &mut Vec<String>, out: &mut Vec<ObservedStep>| {
        if let Some(path) = path {
            out.push(ObservedStep::new(
                "write_file",
                serde_json::json!({ "path": path, "content": content.join("\n") }),
            ));
        }
        content.clear();
    };
    for line in patch.lines() {
        if let Some(rest) = line.strip_prefix("*** Add File:") {
            flush(&current_path, &mut content, &mut translated);
            current_path = Some(rest.trim().to_string());
            continue;
        }
        if line.starts_with("*** Update File:") || line.starts_with("*** Delete File:") {
            return Err(reject("only 'Add File' hunks are translatable"));
        }
        if line.starts_with("*** ") {
            continue;
        }
        if current_path.is_none() {
            continue;
        }
        if let Some(text) = line.strip_prefix('+') {
            content.push(text.to_string());
        }
    }
    flush(&current_path, &mut content, &mut translated);
    if translated.is_empty() {
        return Err(reject("no added file"));
    }
    Ok(translated)
}

fn split_redirect(segment: &str) -> Option<(&str, &str)> {
    let mut in_single = false;
    let mut in_double = false;
    for (index, character) in segment.char_indices() {
        match character {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '>' if !in_single && !in_double => {
                return Some((&segment[..index], &segment[index + 1..]));
            }
            _ => {}
        }
    }
    None
}

/// Split on whitespace, keeping quoted spans together.
fn tokenize(segment: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    for character in segment.chars() {
        match quote {
            Some(active) if character == active => quote = None,
            Some(_) => current.push(character),
            None if character == '"' || character == '\'' => quote = Some(character),
            None if character.is_whitespace() => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            None => current.push(character),
        }
    }
    if quote.is_some() {
        return Err("unbalanced quote".to_string());
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    Ok(tokens)
}

fn two_paths(rest: &[String]) -> Result<(String, String), String> {
    let paths: Vec<&String> = rest.iter().filter(|token| !token.starts_with('-')).collect();
    match paths.as_slice() {
        [source, target, ..] => Ok(((*source).clone(), (*target).clone())),
        _ => Err("expected exactly two paths".to_string()),
    }
}

fn flag_value(rest: &[String], flags: &[&str]) -> Result<String, String> {
    for (index, token) in rest.iter().enumerate() {
        let lowered = token.to_lowercase();
        if flags.contains(&lowered.as_str()) {
            return rest
                .get(index + 1)
                .cloned()
                .ok_or_else(|| format!("{lowered} without a value"));
        }
        for flag in flags {
            if let Some(value) = lowered.strip_prefix(&format!("{flag}=")) {
                let _ = value;
                if let Some((_, raw)) = token.split_once('=') {
                    return Ok(raw.to_string());
                }
            }
        }
    }
    Err(format!("missing one of {flags:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell(command: &str) -> ObservedStep {
        ObservedStep::new("shell", serde_json::json!({ "command": command }))
    }

    #[test]
    fn translates_the_shapes_a_real_agent_emits() {
        let git = translate_step(&shell("git ls-remote https://github.com/openai/codex HEAD")).unwrap();
        assert_eq!(git[0].action, "exec");
        assert_eq!(git[0].args["program"], "git");
        assert_eq!(git[0].args["args"][1], "https://github.com/openai/codex");

        let mv = translate_step(&shell("mv inbox/a.zip archive/a.zip")).unwrap();
        assert_eq!(mv[0].action, "move_file");
        assert_eq!(mv[0].args["source"], "inbox/a.zip");
        assert_eq!(mv[0].args["target"], "archive/a.zip");

        let mkdir = translate_step(&shell("mkdir -p archive")).unwrap();
        assert_eq!(mkdir[0].action, "mkdir");

        let echo = translate_step(&shell("echo hello world > out/note.txt")).unwrap();
        assert_eq!(echo[0].action, "write_file");
        assert_eq!(echo[0].args["content"], "hello world");
        assert_eq!(echo[0].args["path"], "out/note.txt");

        let ps = translate_step(&shell(
            "Move-Item -Path inbox/b.zip -Destination archive/b.zip",
        ))
        .unwrap();
        assert_eq!(ps[0].action, "move_file");
        assert_eq!(ps[0].args["target"], "archive/b.zip");
    }

    #[test]
    fn compound_and_unknown_commands_are_refused_by_name() {
        for command in [
            "cat a.txt | grep x",
            "echo $(date) > stamp.txt",
            "cd sub && mv a b",
            "rm -rf *",
            "curl -o out.txt https://example.com",
        ] {
            let error = translate_step(&shell(command)).unwrap_err();
            assert_eq!(error.label(), "unsupported_command", "{command}");
        }
        let tool = translate_step(&ObservedStep::new("mystery_tool", serde_json::json!({})))
            .unwrap_err();
        assert_eq!(tool.label(), "unsupported_tool");
    }

    #[test]
    fn one_shell_call_can_carry_several_canonical_steps() {
        let steps = translate_step(&shell("mkdir -p archive && mv inbox/a.zip archive/a.zip"))
            .unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].action, "mkdir");
        assert_eq!(steps[1].action, "move_file");
    }

    #[test]
    fn added_files_from_a_patch_become_write_steps() {
        let patch = "*** Begin Patch\n*** Add File: out/manifest.txt\n+V2 manifest verified\n*** End Patch\n";
        let steps = translate_step(&ObservedStep::new(
            "apply_patch",
            serde_json::json!({ "input": patch }),
        ))
        .unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].action, "write_file");
        assert_eq!(steps[0].args["path"], "out/manifest.txt");
        assert_eq!(steps[0].args["content"], "V2 manifest verified");

        let update = "*** Begin Patch\n*** Update File: out/manifest.txt\n@@\n-a\n+b\n*** End Patch\n";
        assert!(translate_step(&ObservedStep::new(
            "apply_patch",
            serde_json::json!({ "input": update }),
        ))
        .is_err());
    }

    #[test]
    fn canonical_steps_pass_through_untouched() {
        let step = ObservedStep::new(
            "move_file",
            serde_json::json!({ "source": "a", "target": "b" }),
        );
        assert_eq!(translate_step(&step).unwrap(), vec![step]);
    }

    #[test]
    fn polling_a_session_is_not_a_step_but_feeding_one_is_refused() {
        let poll = ObservedStep::new(
            "write_stdin",
            serde_json::json!({ "session_id": 4, "chars": "" }),
        );
        assert!(translate_step(&poll).unwrap().is_empty());

        let feed = ObservedStep::new(
            "write_stdin",
            serde_json::json!({ "session_id": 4, "chars": "y\n" }),
        );
        assert_eq!(
            translate_step(&feed).unwrap_err().label(),
            "unsupported_command"
        );
    }

    #[test]
    fn a_step_that_ran_outside_the_workspace_is_refused() {
        let task = ObservedTask::new(
            "Move inbox/a.zip to archive/a.zip",
            vec![ObservedStep::new(
                "exec_command",
                serde_json::json!({ "cmd": "mv inbox/a.zip archive/a.zip", "workdir": "C:/elsewhere" }),
            )],
            true,
        );
        let error =
            canonicalize_in_workspace(&task, Some(std::path::Path::new("C:/workspace"))).unwrap_err();
        assert_eq!(error.label(), "unsupported_command");

        let inside = ObservedTask::new(
            "Move inbox/a.zip to archive/a.zip",
            vec![ObservedStep::new(
                "exec_command",
                serde_json::json!({ "cmd": "mv inbox/a.zip archive/a.zip", "workdir": "C:/workspace" }),
            )],
            true,
        );
        let translated =
            canonicalize_in_workspace(&inside, Some(std::path::Path::new("C:/workspace"))).unwrap();
        assert_eq!(translated.steps.len(), 1);
        assert_eq!(translated.steps[0].action, "move_file");
    }

    #[test]
    fn one_observation_may_not_mix_working_directories() {
        let mixed = ObservedTask::new(
            "Move inbox/a.zip to archive/a.zip",
            vec![
                ObservedStep::new(
                    "exec_command",
                    serde_json::json!({ "cmd": "mkdir -p archive", "workdir": "C:/one" }),
                ),
                ObservedStep::new(
                    "exec_command",
                    serde_json::json!({ "cmd": "mv inbox/a.zip archive/a.zip", "workdir": "C:/two" }),
                ),
            ],
            true,
        );
        assert_eq!(canonicalize(&mixed).unwrap_err().label(), "unsupported_command");

        // Two *separate* observations may use different directories: the steps
        // are workspace-relative, so the family still lines up.
        let left = ObservedTask::new(
            "Move inbox/a.zip to archive/a.zip",
            vec![ObservedStep::new(
                "exec_command",
                serde_json::json!({ "cmd": "mv inbox/a.zip archive/a.zip", "workdir": "C:/one" }),
            )],
            true,
        );
        let right = ObservedTask::new(
            "Move inbox/b.zip to archive/b.zip",
            vec![ObservedStep::new(
                "exec_command",
                serde_json::json!({ "cmd": "mv inbox/b.zip archive/b.zip", "workdir": "D:/two" }),
            )],
            true,
        );
        assert_eq!(canonicalize(&left).unwrap().steps[0].action, "move_file");
        assert_eq!(canonicalize(&right).unwrap().steps[0].action, "move_file");
    }
}

//! `codex experience` — management page for the experience store.
//!
//! Reads/writes the same file the running agent persists to
//! (`<codex_home>/experience/store.json`, M2-5). Experience is native to the
//! agent; this command is the user-facing management surface (M2-3b):
//! list, grant/revoke the never-forget privilege, disable.

use anyhow::Result;
use clap::Parser;
use codex_core::experience_management::ExperienceManagementService;
use codex_core::experience_management::ExperienceManager;
use serde_json::Value;
use serde_json::json;
use std::io::BufRead;
use std::io::Write;

#[derive(Debug, Parser)]
#[command(bin_name = "codex experience")]
pub struct ExperienceCli {
    #[command(subcommand)]
    pub subcommand: ExperienceSubcommand,
}

#[derive(Debug, clap::Subcommand)]
pub enum ExperienceSubcommand {
    /// List experiences (id / name / status / confidence / PIN).
    List,
    /// Grant the never-forget privilege to an experience.
    Pin { id: String },
    /// Revoke the never-forget privilege.
    Unpin { id: String },
    /// Disable an experience manually (works even when pinned).
    Disable { id: String },
    /// Activate an experience: validates a CANDIDATE first, then activates.
    /// Qualification is never skipped, and the write lands in the store the
    /// agent executes from.
    Activate { id: String },
    /// Print the store file path.
    Path,
    /// Diagnose the store plane: resolved path, on-disk format, contents and
    /// any divergence between the execution and management views.
    Doctor,
    /// Write a read-only HTML report of the store (experiences, templates,
    /// confidence, execution records, full detail) and print where it went.
    Html {
        /// Store to read; defaults to the same resolution the agent uses.
        #[arg(long)]
        store: Option<std::path::PathBuf>,
        /// Where to write the report; defaults to `experience-view.html` next
        /// to the store.
        #[arg(long)]
        out: Option<std::path::PathBuf>,
        /// Open the report in the default browser.
        #[arg(long)]
        open: bool,
    },
    /// Run a stdio MCP server exposing experience tools (list / pin /
    /// unpin / disable). External agents / UIs can access the experience
    /// store through MCP; the agent itself still uses the native runtime.
    Mcp,
    /// Launch the local web management page (spawns app-server --stdio as
    /// the backend; all requests go through the app-server protocol).
    Ui {
        /// Local HTTP port for the page.
        #[arg(long)]
        port: Option<u16>,
    },
}

pub fn run(subcommand: ExperienceSubcommand) -> Result<()> {
    match subcommand {
        ExperienceSubcommand::List => {
            let mut service = ExperienceManagementService::open_default()?;
            let rows = service.list();
            if rows.is_empty() {
                println!("(empty experience store)");
            }
            for row in rows {
                println!(
                    "{:<10}  {:<28} kind={:<9} status={:<10} confidence={:.2} pin={}",
                    row.id, row.name, row.kind, row.status, row.confidence, row.pinned
                );
            }
            Ok(())
        }
        ExperienceSubcommand::Pin { id } => {
            ExperienceManagementService::open_default()?.pin(&id, "cli")?;
            println!("pinned {id}");
            Ok(())
        }
        ExperienceSubcommand::Unpin { id } => {
            ExperienceManagementService::open_default()?.unpin(&id, "cli")?;
            println!("unpinned {id}");
            Ok(())
        }
        ExperienceSubcommand::Disable { id } => {
            ExperienceManagementService::open_default()?.disable(&id, "cli")?;
            println!("disabled {id}");
            Ok(())
        }
        ExperienceSubcommand::Activate { id } => {
            let status = ExperienceManagementService::open_default()?.activate(&id, "cli")?;
            println!("activated {id} -> {status:?}");
            Ok(())
        }
        ExperienceSubcommand::Path => {
            let manager = ExperienceManager::open_default()?;
            println!("{}", manager.path().display());
            Ok(())
        }
        ExperienceSubcommand::Doctor => {
            let codex_home = codex_core::config::find_codex_home()?;
            let report = codex_core::experience_paths::doctor(codex_home.as_path());
            println!("resolved store : {}", report.path.display());
            println!("format         : {}", report.format.label());
            println!(
                "contents       : {} experience(s), {} template(s), {} pinned",
                report.experiences, report.templates, report.pinned
            );
            println!("usage entries  : {}", report.usage_entries);
            println!(
                "override active: {}",
                if report.override_active { "yes" } else { "no" }
            );
            if report.override_active {
                println!(
                    "home store     : {} ({})",
                    report.home_path.display(),
                    report.home_format.label()
                );
            }
            match &report.drift {
                Some(drift) => {
                    println!("drift          : {drift}");
                    anyhow::bail!("experience store is not in a single-truth state");
                }
                None => {
                    println!("drift          : none");
                    Ok(())
                }
            }
        }
        ExperienceSubcommand::Html { store, out, open } => {
            let store_path = match store {
                Some(path) => path,
                None => {
                    let codex_home = codex_core::config::find_codex_home()?;
                    codex_core::experience_paths::resolve_store_path(codex_home.as_path())
                }
            };
            let html = codex_core::experience_report::render_html(&store_path)?;
            let target = match out {
                Some(path) => path,
                None => store_path
                    .parent()
                    .map(|parent| parent.join("experience-view.html"))
                    .unwrap_or_else(|| std::path::PathBuf::from("experience-view.html")),
            };
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&target, html)?;
            println!("experience report: {}", target.display());
            if open {
                open_in_browser(&target)?;
            }
            Ok(())
        }
        ExperienceSubcommand::Mcp => run_mcp_stdio(),
        ExperienceSubcommand::Ui { port } => crate::experience_ui::run_ui(port.unwrap_or(8765)),
    }
}

/// Hand the file to the OS. The report is a plain file on disk, so this is the
/// only integration step needed — no server, no port, no app-server.
fn open_in_browser(path: &std::path::Path) -> Result<()> {
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("cmd");
        command.args(["/C", "start", ""]);
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = std::process::Command::new("xdg-open");
    command.arg(path);
    command.spawn()?;
    Ok(())
}

fn tools_list() -> Value {
    let tool = |name: &str, description: &str, props: Value| {
        json!({"name": name, "description": description, "inputSchema": {"type":"object","properties":props}})
    };
    json!({
        "tools": [
            tool("experience_list", "List experiences (id/name/status/confidence/pin)", json!({})),
            tool("experience_pin", "Grant the never-forget privilege", json!({"id":{"type":"string"}})),
            tool("experience_unpin", "Revoke the never-forget privilege", json!({"id":{"type":"string"}})),
            tool("experience_disable", "Disable an experience", json!({"id":{"type":"string"}})),
        ]
    })
}

fn handle_tools_call(
    manager: &mut ExperienceManager,
    params: Option<&Value>,
) -> Result<Value, String> {
    let params = params.ok_or("missing params")?;
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or("missing tool name")?;
    let empty_arguments = json!({});
    let arguments = params.get("arguments").unwrap_or(&empty_arguments);
    let id = arguments.get("id").and_then(Value::as_str).unwrap_or("");
    let text = match name {
        "experience_list" => {
            let rows = manager.list();
            if rows.is_empty() {
                "(empty experience store)".to_string()
            } else {
                rows.iter()
                    .map(|row| {
                        format!(
                            "{} | {} | status={} | confidence={:.2} | pin={}",
                            row.id, row.name, row.status, row.confidence, row.pinned
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        }
        "experience_pin" => {
            manager.pin(id).map_err(|error| error.to_string())?;
            format!("pinned {id}")
        }
        "experience_unpin" => {
            manager.unpin(id).map_err(|error| error.to_string())?;
            format!("unpinned {id}")
        }
        "experience_disable" => {
            manager.disable(id).map_err(|error| error.to_string())?;
            format!("disabled {id}")
        }
        other => return Err(format!("unknown tool: {other}")),
    };
    Ok(json!({ "content": [{"type": "text", "text": text}] }))
}

/// Minimal JSON-RPC (stdio) MCP server: initialize / tools/list /
/// tools/call for experience.* tools. Preserves the original codex design —
/// this is only an outward adapter for the native experience store.
fn run_mcp_stdio() -> Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut manager = ExperienceManager::open_default()?;
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let message: Value = serde_json::from_str(&line)?;
        let id = message.get("id").cloned();
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("");
        let result = match method {
            "initialize" => Some(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "codex-experience", "version": "0.1.0" }
            })),
            "ping" => Some(json!({})),
            "tools/list" => Some(tools_list()),
            "tools/call" => match handle_tools_call(&mut manager, message.get("params")) {
                Ok(result) => Some(result),
                Err(error) => Some(json!({
                    "content": [{"type": "text", "text": error}],
                    "isError": true
                })),
            },
            // notifications (initialized/cancelled) and unknown methods: no reply
            _ => None,
        };
        if let Some(result) = result {
            let mut response = json!({ "jsonrpc": "2.0", "result": result });
            if let Some(id) = id {
                response["id"] = id;
            }
            let mut output = stdout.lock();
            writeln!(output, "{response}")?;
            output.flush()?;
        }
    }
    Ok(())
}

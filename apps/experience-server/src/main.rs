//! Experience local API server.
//!
//! Serves the Experience UI (static files on disk under `ui/www`) and a
//! localhost JSON API over the P1 store. The launcher exe only starts this
//! process; iterating on this code never requires repackaging the launcher.

mod agent_manager;
mod fs_browse;
mod session_manager;

use std::io::BufRead;
use std::io::BufReader;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::Child;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use agent_manager::AgentConfig;
use serde::Deserialize;
use serde::Serialize;
use agent_manager::AgentManager;
use agent_manager::AgentState;
use session_manager::Session;
use session_manager::SessionStore;
use experience_core::domain::experience::Experience;
use experience_core::domain::experience::ExperienceStatus;
use experience_core::domain::experience::QualificationAction;
use experience_core::domain::capability::is_known_capability;
use experience_core::domain::predicate::TruthValue;
use experience_core::experience::delegation::delegation_text;
use experience_core::experience::delegation::evaluate_predicate;
use experience_core::experience::delegation::apply_experience_usage;
use experience_core::experience::delegation::ExperienceOutcome;
use experience_core::experience::delegation::resolve_default_source;
use experience_core::experience::qualification::ConfidenceRecord;
use experience_core::experience::learning_l1::sink_once;
use experience_core::experience::learning_l1::candidate_name;
use experience_core::experience::learning_l1::DeterministicDistiller;
use experience_core::experience::learning_l1::L1Distiller;
use experience_core::experience::learning_l1::validate_tool_boundary;
use experience_core::experience::trace_reader::RoundTrace;
use experience_core::experience::qualification::qualify;
use experience_core::experience::trace_reader::select_round;
use experience_core::experience::trace_reader::RoundSelection;
use experience_core::experience::trace_event::TraceEvent;
use experience_core::experience::trace_event::TRACE_SCHEMA_VERSION;
use experience_core::store::ExperienceStore;
use experience_core::store::ReferenceEntry;
use experience_core::store::CandidateOrigin;
use experience_core::similarity;
use experience_core::policy::capability_family;
use experience_core::policy::validate_policy;
use experience_core::policy::CapabilityPolicy;
use experience_core::policy::GLOBAL_SCOPE_KEY;
use experience_core::exec::execute_step as exec_step;
use experience_core::exec::restore_from_manifest;
use experience_core::exec::write_backup_manifest;
use experience_core::exec::BackupEntry;
use experience_core::exec::StepContext;
use experience_core::state_source;
use experience_core::redact::Disclosure;
use experience_core::redact::RedactMode;
use experience_core::redact::Redactor;
use experience_core::redact::sha256;
use tiny_http::Header;
use tiny_http::Method;
use tiny_http::Request;
use tiny_http::Response;
use tiny_http::Server;
use tiny_http::StatusCode;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let home = arg_value(&args, "--home")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("EXPERIENCE_HOME").map(PathBuf::from))
        .unwrap_or_else(default_home);
    let ui_dir = arg_value(&args, "--ui-dir")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("EXPERIENCE_UI_DIR").map(PathBuf::from))
        .unwrap_or_else(find_ui_dir);
    let port = arg_value(&args, "--port")
        .or_else(|| std::env::var("EXPERIENCE_PORT").ok())
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(8766);

    std::fs::create_dir_all(&home).expect("create experience home");
    let store = ExperienceStore::open(home.join("store.json"))
        .unwrap_or_else(|error| {
            eprintln!("failed to open experience store: {error}");
            std::process::exit(1);
        });
    let agents = Arc::new(Mutex::new(AgentManager::load_from_path(&home.join(
        "agents.json",
    ))));
    let sessions = Arc::new(SessionStore::load(&home));
    let redactor = Redactor::new(
        &load_or_create_redaction_key(&home),
        redaction_disclosure(),
        redaction_mode(),
    );
    let app = Arc::new(App {
        store: Mutex::new(store),
        agents,
        sessions,
        audit: Mutex::new(()),
        settings: Mutex::new(Settings::load(&home)),
        home,
        ui_dir,
        redactor,
    });

    let address = format!("127.0.0.1:{port}");
    let server = Server::http(&address).unwrap_or_else(|error| {
        eprintln!("failed to bind {address}: {error}");
        std::process::exit(1);
    });
    println!("EXPERIENCE_READY http://{address}/");

    for request in server.incoming_requests() {
        let app = Arc::clone(&app);
        std::thread::spawn(move || {
            if let Err(error) = handle(app, request) {
                eprintln!("request error: {error}");
            }
        });
    }
}

struct App {
    store: Mutex<ExperienceStore>,
    agents: Arc<Mutex<AgentManager>>,
    sessions: Arc<SessionStore>,
    /// Serializes ledger/usage read-modify-write (review P1-2): concurrent
    /// session terminals must never drop audit records or usage increments.
    audit: Mutex<()>,
    home: PathBuf,
    ui_dir: PathBuf,
    redactor: Redactor,
    /// S4: file-backed settings; environment variables still win when set.
    settings: Mutex<Settings>,
}

/// S4 settings: persisted under `<home>/settings.json`, with environment
/// variables taking precedence when explicitly set (old deployments keep
/// working unchanged).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub llm_compiler: bool,
    #[serde(default)]
    pub injection_policy: bool,
    /// Undo surface: keep at most this many run snapshots per session.
    #[serde(default = "default_undo_keep")]
    pub undo_keep: u32,
}

fn default_undo_keep() -> u32 {
    20
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            llm_compiler: false,
            injection_policy: false,
            undo_keep: default_undo_keep(),
        }
    }
}

impl Settings {
    fn load(home: &Path) -> Self {
        match std::fs::read_to_string(home.join("settings.json")) {
            Ok(json) => serde_json::from_str(&json).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    fn save(&self, home: &Path) -> Result<(), String> {
        let path = home.join("settings.json");
        let json = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        std::fs::write(&path, json).map_err(|error| error.to_string())
    }
}

fn handle(app: Arc<App>, request: Request) -> Result<(), String> {
    let url = request.url().to_string();
    let method = request.method().clone();
    if url.starts_with("/api/") {
        return handle_api(app, method, url, request);
    }
    let relative = match url.split('?').next().unwrap_or("/") {
        "/" => "index.html".to_string(),
        other => other.trim_start_matches('/').to_string(),
    };
    serve_static(&app.ui_dir, &relative, request)
}

fn handle_api(
    app: Arc<App>,
    method: Method,
    url: String,
    mut request: Request,
) -> Result<(), String> {
    // Read the body once; a read failure still has the live request for a
    // real 400 response. Validation failures inside the routes become 400s
    // instead of a silent stderr line (the previous behavior dropped them).
    let body = match read_body(&mut request) {
        Ok(body) => body,
        Err(error) => return bad_request(request, error),
    };
    let mut owned = Some(request);
    let result = {
        let mut responder = Responder { slot: &mut owned };
        routes(app, method, &url, &mut responder, &body)
    };
    if result.is_err() {
        // A route returned before responding (validation/serialization):
        // answer with a 400 here so the client never hangs.
        if let Some(request) = owned.take() {
            let message = result.err().unwrap_or_default();
            return bad_request(request, message);
        }
    }
    result
}

/// Single-response guard for the API. Routes borrow this instead of the raw
/// request, so every branch can answer exactly once (validation failures
/// become JSON 400s rather than a dropped connection).
struct Responder<'r> {
    slot: &'r mut Option<Request>,
}

impl<'r> Responder<'r> {
    /// JSON answer used by every route. `status` is the HTTP status code.
    fn answer(&mut self, status: u16, value: &serde_json::Value) -> Result<(), String> {
        match self.slot.take() {
            Some(request) => json_response(request, status, value),
            None => Err("api route attempted to respond twice".to_string()),
        }
    }
}

fn routes(
    app: Arc<App>,
    method: Method,
    url: &str,
    mut responder: &mut Responder<'_>,
    body: &str,
) -> Result<(), String> {
    if url == "/api/health" {
        return responder.answer(200, &serde_json::json!({"ok": true}));
    }

    if url == "/api/usage" {
        return match method {
            Method::Get => {
                let _guard = app.audit.lock().unwrap();
                let usage = load_usage_unlocked(&app);
                responder.answer(200, &serde_json::to_value(&usage).unwrap())
            }
            _ => method_not_allowed(&mut responder),
        };
    }

    // S1-a capability policy surface. GET resolves the effective policy for a
    // scope (`?scope=`); PUT writes `__global__` or a scene policy, and can
    // clear an entry back to the global fallback with `"clear": true`.
    if url == "/api/policy" || url.starts_with("/api/policy?") {
        return match method {
            Method::Get => {
                let scope = query_param(&url, "scope");
                let run_root = app.home.join("run").to_string_lossy().to_string();
                let store = app.store.lock().unwrap();
                let effective = store.policy_for_scope(scope.as_deref());
                let scope_has_policy = scope
                    .as_deref()
                    .map(|value| store.has_policy_for_scope(value))
                    .unwrap_or(false);
                let stored: Vec<serde_json::Value> = store
                    .scope_policies()
                    .iter()
                    .map(|(scope, policy)| {
                        serde_json::json!({ "scope": scope, "policy": policy })
                    })
                    .collect();
                responder.answer(
                    200,
                    &serde_json::json!({
                        "scope": scope,
                        "scope_has_policy": scope_has_policy,
                        "effective": effective,
                        "stored": stored,
                        "run_root": run_root,
                    }),
                )
            }
            Method::Put => {
                let value: serde_json::Value = match serde_json::from_str(&body) {
                    Ok(value) => value,
                    Err(error) => {
                        return responder.answer(
                            400,
                            &serde_json::json!({ "error": format!("invalid policy body: {error}") }),
                        )
                    }
                };
                let scope = value
                    .get("scope")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| GLOBAL_SCOPE_KEY.to_string());
                let actor = value
                    .get("actor")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("local-user")
                    .to_string();
                let clear = value
                    .get("clear")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                if clear {
                    let mut store = app.store.lock().unwrap();
                    if let Err(error) = store.set_scope_policy(&scope, None) {
                        return responder.answer(
                            400,
                            &serde_json::json!({ "error": error.to_string() }),
                        );
                    }
                    if let Err(error) = store.save() {
                        return responder.answer(
                            500,
                            &serde_json::json!({ "error": error.to_string() }),
                        );
                    }
                    let effective = store.policy_for_scope(
                        (scope != GLOBAL_SCOPE_KEY).then_some(scope.as_str()),
                    );
                    drop(store);
                    append_policy_ledger(&app, &scope, &actor, "cleared", None);
                    return responder.answer(
                        200,
                        &serde_json::json!({
                            "ok": true,
                            "scope": scope,
                            "cleared": true,
                            "effective": effective,
                        }),
                    );
                }
                let policy: CapabilityPolicy = match value
                    .get("policy")
                    .cloned()
                    .map(serde_json::from_value)
                    .transpose()
                {
                    Ok(Some(policy)) => policy,
                    Ok(None) => {
                        return responder.answer(
                            400,
                            &serde_json::json!({ "error": "missing 'policy' object" }),
                        )
                    }
                    Err(error) => {
                        return responder.answer(
                            400,
                            &serde_json::json!({ "error": format!("invalid policy body: {error}") }),
                        )
                    }
                };
                if let Err(problem) = validate_policy(&policy) {
                    return responder.answer(
                        400,
                        &serde_json::json!({ "error": format!("invalid policy: {problem}") }),
                    );
                }
                let mut store = app.store.lock().unwrap();
                let reason = policy_summary(&policy);
                if let Err(error) = store.set_scope_policy(&scope, Some(policy.clone())) {
                    return responder.answer(
                        400,
                        &serde_json::json!({ "error": error.to_string() }),
                    );
                }
                if let Err(error) = store.save() {
                    return responder.answer(
                        500,
                        &serde_json::json!({ "error": error.to_string() }),
                    );
                }
                drop(store);
                append_policy_ledger(&app, &scope, &actor, "set", Some(&reason));
                responder.answer(
                    200,
                    &serde_json::json!({ "ok": true, "scope": scope, "policy": policy }),
                )
            }
            _ => method_not_allowed(&mut responder),
        };
    }

    // S1-c backup surface: list a session's run snapshots, restore one.
    if url == "/api/backups" || url.starts_with("/api/backups?") {
        return match method {
            Method::Get => {
                let session = query_param(&url, "session").unwrap_or_default();
                if !valid_backup_id(&session) {
                    return responder.answer(
                        400,
                        &serde_json::json!({ "error": "invalid 'session'" }),
                    );
                }
                let root = app.home.join("backups").join(&session);
                let mut snapshots: Vec<serde_json::Value> = Vec::new();
                if let Ok(entries) = std::fs::read_dir(&root) {
                    for entry in entries.flatten() {
                        if !entry.path().is_dir() {
                            continue;
                        }
                        let manifest_path = entry.path().join("manifest.json");
                        let manifest = std::fs::read_to_string(&manifest_path)
                            .ok()
                            .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok());
                        snapshots.push(serde_json::json!({
                            "snapshot": entry.file_name().to_string_lossy(),
                            "path": entry.path().to_string_lossy(),
                            "manifest": manifest,
                        }));
                    }
                }
                snapshots.sort_by(|left, right| {
                    left["snapshot"]
                        .as_str()
                        .unwrap_or("")
                        .cmp(right["snapshot"].as_str().unwrap_or(""))
                });
                responder.answer(
                    200,
                    &serde_json::json!({ "session": session, "backups": snapshots }),
                )
            }
            _ => method_not_allowed(&mut responder),
        };
    }

    if let Some(rest) = url.strip_prefix("/api/backups/") {
        if let Some(session) = rest.strip_suffix("/restore") {
            if method != Method::Post {
                return method_not_allowed(&mut responder);
            }
            if !valid_backup_id(session) {
                return responder.answer(
                    400,
                    &serde_json::json!({ "error": "invalid 'session'" }),
                );
            }
            let value: serde_json::Value = match serde_json::from_str(&body) {
                Ok(value) => value,
                Err(error) => {
                    return responder.answer(
                        400,
                        &serde_json::json!({ "error": format!("invalid restore body: {error}") }),
                    )
                }
            };
            let snapshot = value
                .get("snapshot")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            if !valid_backup_id(&snapshot) {
                return responder.answer(
                    400,
                    &serde_json::json!({ "error": "missing or invalid 'snapshot'" }),
                );
            }
            let manifest =
                app.home
                    .join("backups")
                    .join(session)
                    .join(&snapshot)
                    .join("manifest.json");
            if !manifest.exists() {
                return responder.answer(
                    404,
                    &serde_json::json!({ "error": "backup snapshot not found" }),
                );
            }
            let actor = value
                .get("actor")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("local-user")
                .to_string();
            match restore_from_manifest(&manifest) {
                Ok(report) => {
                    append_l1_ledger(
                        &app,
                        &L1LedgerRecord {
                            record_type: "backup_restored".to_string(),
                            candidate_name: session.to_string(),
                            session_id: Some(session.to_string()),
                            thread_id: None,
                            trace_version: None,
                            action: Some("restore".to_string()),
                            from: None,
                            to: None,
                            reason: Some(format!(
                                "actor={actor};snapshot={snapshot};restored={};deleted={}",
                                report.restored.len(),
                                report.deleted.len()
                            )),
                            outcome: "completed".to_string(),
                            recorded_at: l1_now_secs(),
                        },
                    );
                    return responder.answer(
                        200,
                        &serde_json::json!({
                            "ok": true,
                            "snapshot": snapshot,
                            "restored": report.restored,
                            "deleted": report.deleted,
                        }),
                    )
                }
                Err(error) => return responder.answer(
                    400,
                    &serde_json::json!({ "error": format!("restore failed: {error}") }),
                ),
            }
        } else {
            return responder.answer(404, &serde_json::json!({ "error": "unknown backups path" }))
        }
    }

    if url == "/api/audit" || url.starts_with("/api/audit?") {
        return match method {
            Method::Get => {
                let record_type = query_param(&url, "record_type");
                let name = query_param(&url, "name");
                let limit = query_param(&url, "limit")
                    .and_then(|value| value.parse::<usize>().ok());
                let _guard = app.audit.lock().unwrap();
                let ledger = std::fs::read_to_string(app.home.join("learning-l1.json"))
                    .unwrap_or_default();
                let records = audit_query(&ledger, record_type.as_deref(), name.as_deref(), limit);
                responder.answer(
                    200,
                    &serde_json::json!({ "records": records }),
                )
            }
            _ => method_not_allowed(&mut responder),
        };
    }

    if url == "/api/agents" {
        return match method {
            Method::Get => {
                let agents = app.agents.lock().unwrap();
                let list: Vec<serde_json::Value> = agents
                    .list()
                    .iter()
                    .map(|entry| serde_json::to_value(entry).unwrap())
                    .collect();
                responder.answer( 200, &serde_json::Value::Array(list))
            }
            Method::Post => {
                let config: AgentConfig = serde_json::from_str(&body)
                    .map_err(|error| format!("invalid agent body: {error}"))?;
                let mut agents = app.agents.lock().unwrap();
                agents.create(config).map_err(|error| error)?;
                agents_persist(&app, &agents);
                responder.answer( 201, &serde_json::json!({"ok": true}))
            }
            _ => method_not_allowed(&mut responder),
        };
    }

    if let Some(rest) = url.strip_prefix("/api/agents/") {
        let (id, action) = match rest.split_once('/') {
            Some((id, action)) => (id, Some(action)),
            None => (rest, None),
        };
        return match (method, action) {
            (Method::Get, None) => {
                let agents = app.agents.lock().unwrap();
                match agents.get(id) {
                    Some(entry) => {
                        responder.answer( 200, &serde_json::to_value(entry).unwrap())
                    }
                    None => not_found(&mut responder, format!("agent '{id}' not found")),
                }
            }
            (Method::Delete, None) => {
                let mut agents = app.agents.lock().unwrap();
                agents.remove(id).map_err(|error| error)?;
                agents_persist(&app, &agents);
                responder.answer( 200, &serde_json::json!({"ok": true}))
            }
            (Method::Post, Some("connect")) => {
                let mut agents = app.agents.lock().unwrap();
                let status = agents.connect(id).map_err(|error| error)?;
                responder.answer( 200, &serde_json::to_value(&status).unwrap())
            }
            (Method::Post, Some("disconnect")) => {
                let mut agents = app.agents.lock().unwrap();
                let status = agents.disconnect(id).map_err(|error| error)?;
                responder.answer( 200, &serde_json::to_value(&status).unwrap())
            }
            _ => method_not_allowed(&mut responder),
        };
    }

    if url == "/api/sessions" {
        return match method {
            Method::Get => {
                let sessions = app.sessions.list();
                let list: Vec<serde_json::Value> =
                    sessions.iter().map(session_summary).collect();
                responder.answer( 200, &serde_json::Value::Array(list))
            }
            Method::Post => {
                let value: serde_json::Value =
                    serde_json::from_str(&body).map_err(|error| error.to_string())?;
                let agent_id = value
                    .get("agent_id")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| "missing 'agent_id'".to_string())?
                    .to_string();
                let task = value
                    .get("task")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| "missing 'task'".to_string())?
                    .to_string();
                let cwd = value
                    .get("cwd")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                let scope = value
                    .get("scope")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                let capabilities = value
                    .get("capabilities")
                    .and_then(serde_json::Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .map(str::to_string)
                            .collect::<Vec<String>>()
                    });
                if let Some(list) = &capabilities {
                    for capability in list {
                        if !is_known_capability(capability) {
                            return Err(format!("unknown capability '{capability}'"));
                        }
                        // S1-a: a session may only *narrow* the effective
                        // policy; asking for something the scope forbids is
                        // refused up front instead of failing at execution.
                        let effective = {
                            let store = app.store.lock().unwrap();
                            store.policy_for_scope(scope.as_deref())
                        };
                        if !capability_within_policy(&effective, capability) {
                            return Err(format!(
                                "capability '{capability}' exceeds the policy for this scope"
                            ));
                        }
                    }
                }

                // Fail loud: never delegate against an Error/Checking state.
                let ready = {
                    let mut agents = app.agents.lock().unwrap();
                    let state = agents
                        .get(&agent_id)
                        .map(|entry| entry.status.state.clone())
                        .ok_or_else(|| format!("agent '{agent_id}' not found"))?;
                    match state {
                        AgentState::Ready => true,
                        AgentState::Configured | AgentState::Disconnected => {
                            agents
                                .connect(&agent_id)
                                .map_err(|error| error.to_string())?
                                .state
                                == AgentState::Ready
                        }
                        AgentState::Error => {
                            return Err(format!(
                                "agent '{agent_id}' 处于 error 态，请先修复：{}",
                                agents.get(&agent_id).unwrap().status.message
                            ));
                        }
                        AgentState::Checking => {
                            return Err("agent 正在检查中，请稍候重试".to_string());
                        }
                    }
                };
                if !ready {
                    return Err(format!("agent '{agent_id}' 未能就绪"));
                }

                let session = app.sessions.create(
                    agent_id.clone(),
                    task.clone(),
                    cwd.clone(),
                    scope,
                    capabilities,
                );
                let session_id = session.id.clone();
                let agents = Arc::clone(&app.agents);
                let sessions = Arc::clone(&app.sessions);
                let cwd_path = cwd.as_ref().map(PathBuf::from);
                let channel = agents
                    .lock()
                    .unwrap()
                    .get(&agent_id)
                    .and_then(|entry| entry.config.channel.clone());
                let codex_home_cfg = agents
                    .lock()
                    .unwrap()
                    .get(&agent_id)
                    .and_then(|entry| entry.config.codex_home.clone());
                if channel.as_deref() == Some("session") {
                    let executable = match agents
                        .lock()
                        .unwrap()
                        .get(&agent_id)
                        .map(|entry| entry.config.resolve_executable())
                    {
                        Some(Ok(path)) => path,
                        _ => {
                            let error = "agent executable 无法解析（session channel）".to_string();
                            sessions.finish(&session_id, false, error.clone(), error.clone());
                            return responder.answer(
                                500,
                                &serde_json::json!({ "error": error }),
                            );
                        }
                    };
                    let codex_home = codex_home_cfg
                        .map(PathBuf::from)
                        .or_else(|| std::env::var_os("CODEX_HOME").map(PathBuf::from))
                        .ok_or_else(|| {
                            "session channel 需要 codex_home（agent 配置或环境变量 CODEX_HOME）"
                                .to_string()
                        })?;
                    let home = app.home.clone();
                    let workspace = cwd_path.unwrap_or_else(|| home.clone());
                    let cancel = Arc::new(AtomicBool::new(false));
                    sessions.register_cancel_flag(&session_id, Arc::clone(&cancel));
                    let l3_context = l3_entry(&app, &sessions, &session_id, &task, &workspace);
                    std::thread::spawn(move || {
                        let config = agent_codex::session_host::SessionHostConfig {
                            exe: executable,
                            codex_home: Some(codex_home),
                            extra_env: Vec::new(),
                        };
                        let delegated_task = l3_context
                            .as_ref()
                            .map(|context| context.delegated_task.clone())
                            .unwrap_or_else(|| task.clone());
                        let task = agent_codex::session_host::SessionTask {
                            workspace,
                            task: delegated_task,
                        };
                        let sessions_for_trace = Arc::clone(&sessions);
                        let app_for_l1 = Arc::clone(&app);
                        let outcome = agent_codex::session_host::run_session_task_until_turn(
                            &config,
                            &task,
                            cancel.as_ref(),
                            |event| {
                                let trace = sessions_for_trace
                                    .get(&session_id)
                                    .map(|s| s.trace)
                                    .unwrap_or_default();
                                let mut next = trace;
                                next.push(event.clone());
                                sessions_for_trace.update_trace(&session_id, next);
                            },
                        );
                        // Trace is single-authored live via on_milestone while
                        // the session is Running; the terminal write must not
                        // append the full milestone list again (double-write).
                        match outcome {
                            Ok(outcome) => {
                                let session_ok = outcome.ok;
                                sessions.finish_session_channel(
                                    &session_id,
                                    outcome.ok,
                                    outcome.error.unwrap_or_else(|| "session completed".to_string()),
                                    outcome.thread_id,
                                );
                                if session_ok {
                                    l1_sink_session(&app_for_l1, &sessions, &session_id);
                                }
                                if let Some(context) = &l3_context {
                                    if context.track_delegation {
                                    let completion_reason = if context.executed.is_some() {
                                        "v2_executed_then_delegate"
                                    } else {
                                        "v1_plan_aware"
                                    };
                                    append_l1_ledger(
                                        &app_for_l1,
                                        &L1LedgerRecord {
                                            record_type: if session_ok {
                                                "delegation_completed"
                                            } else {
                                                "delegation_failed"
                                            }
                                            .to_string(),
                                            candidate_name: context.plan.clone(),
                                            session_id: Some(session_id.clone()),
                                            thread_id: None,
                                            trace_version: None,
                                            action: Some("delegate".to_string()),
                                            from: None,
                                            to: None,
                                            reason: Some(completion_reason.to_string()),
                                            outcome: if session_ok {
                                                "completed".to_string()
                                            } else {
                                                "failed".to_string()
                                            },
                                            recorded_at: l1_now_secs(),
                                        },
                                    );
                                    }
                                }
                            }
                            Err(error) => sessions.finish_session_channel(
                                &session_id,
                                false,
                                error,
                                None,
                            ),
                        }
                    });
                    return responder.answer(
                        201,
                        &serde_json::to_value(&session).unwrap(),
                    );
                }
                let spawned = agents
                    .lock()
                    .unwrap()
                    .spawn_task(&agent_id, &task, cwd_path.as_deref());
                match spawned {
                    Ok(child) => {
                        let child = Arc::new(Mutex::new(child));
                        sessions.register_child(&session_id, Arc::clone(&child));
                        std::thread::spawn(move || {
                            run_session_worker(
                                sessions,
                                child,
                                session_id,
                                session_timeout(),
                            )
                        });
                        responder.answer( 201, &serde_json::to_value(&session).unwrap())
                    }
                    Err(error) => {
                        sessions.finish(&session_id, false, error.clone(), error.clone());
                        responder.answer(
                            500,
                            &serde_json::json!({ "error": error }),
                        )
                    }
                }
            }
            _ => method_not_allowed(&mut responder),
        };
    }

    if let Some(id) = url.strip_prefix("/api/sessions/") {
        if let Some(resume_id) = id.strip_suffix("/resume") {
            if method != Method::Post {
                return method_not_allowed(&mut responder);
            }
            let value: serde_json::Value =
                serde_json::from_str(&body).map_err(|error| error.to_string())?;
            let follow_up = value
                .get("task")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "missing 'task'".to_string())?
                .to_string();
            let existing = app
                .sessions
                .get(resume_id)
                .ok_or_else(|| format!("session '{resume_id}' not found"))?;
            let thread_id = existing
                .thread_id
                .clone()
                .ok_or_else(|| "该会话无 thread_id，无法 resume".to_string())?;
            let agent_id = existing.agent_id.clone();
            let cwd = existing.cwd.clone();
            let executable = {
                let agents = app.agents.lock().unwrap();
                agents
                    .get(&agent_id)
                    .and_then(|entry| entry.config.resolve_executable().ok())
                    .ok_or_else(|| "agent executable 无法解析（resume）".to_string())?
            };
            let codex_home_cfg = app
                .agents
                .lock()
                .unwrap()
                .get(&agent_id)
                .and_then(|entry| entry.config.codex_home.clone());
            let codex_home = codex_home_cfg
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("CODEX_HOME").map(PathBuf::from))
                .ok_or_else(|| {
                    "session channel 需要 codex_home（agent 配置或环境变量 CODEX_HOME）"
                        .to_string()
                })?;
            let workspace = cwd
                .map(PathBuf::from)
                .unwrap_or_else(|| app.home.clone());
            let session = app.sessions.resume_start(resume_id, follow_up.clone())?;
            let session_id = session.id.clone();
            let sessions = Arc::clone(&app.sessions);
            let cancel = Arc::new(AtomicBool::new(false));
            sessions.register_cancel_flag(&session_id, Arc::clone(&cancel));
            let app_for_l1 = Arc::clone(&app);
            std::thread::spawn(move || {
                let config = agent_codex::session_host::SessionHostConfig {
                    exe: executable,
                    codex_home: Some(codex_home),
                    extra_env: Vec::new(),
                };
                let task = agent_codex::session_host::SessionTask {
                    workspace,
                    task: follow_up,
                };
                let outcome = agent_codex::session_host::resume_thread(
                    &config,
                    &thread_id,
                    &task,
                    cancel.as_ref(),
                    |event| {
                        let mut next = sessions
                            .get(&session_id)
                            .map(|s| s.trace)
                            .unwrap_or_default();
                        next.push(event.clone());
                        sessions.update_trace(&session_id, next);
                    },
                );
                // Same single-writer rule as the initial run: milestones are
                // already in the session trace from live updates.
                match outcome {
                    Ok(outcome) => {
                        let session_ok = outcome.ok;
                        sessions.finish_session_channel(
                            &session_id,
                            outcome.ok,
                            outcome.error.unwrap_or_else(|| "resume completed".to_string()),
                            outcome.thread_id,
                        );
                        if session_ok {
                            l1_sink_session(&app_for_l1, &sessions, &session_id);
                        }
                    }
                    Err(error) => sessions.finish_session_channel(
                        &session_id,
                        false,
                        error,
                        Some(thread_id),
                    ),
                }
            });
            return responder.answer( 201, &serde_json::to_value(&session).unwrap());
        }
        if let Some(action) = id.strip_suffix("/cancel") {
            if method != Method::Post {
                return method_not_allowed(&mut responder);
            }
            return match app.sessions.cancel(action) {
                Ok(()) => responder.answer( 200, &serde_json::json!({"ok": true})),
                Err(error) => not_found(&mut responder, error),
            };
        }
        return match method {
            Method::Get => match app.sessions.get(id) {
                Some(session) => {
                    responder.answer( 200, &serde_json::to_value(&session).unwrap())
                }
                None => not_found(&mut responder, format!("session '{id}' not found")),
            },
            _ => method_not_allowed(&mut responder),
        };
    }

    if url == "/api/fs/roots" {
        return match method {
            Method::Get => responder.answer(
                200,
                &serde_json::json!({ "roots": fs_browse::list_roots() }),
            ),
            _ => method_not_allowed(&mut responder),
        };
    }

    if url.starts_with("/api/fs/browse") {
        return match method {
            Method::Get => {
                let query = url.split_once('?').map(|(_, query)| query).unwrap_or("");
                let raw = query
                    .split('&')
                    .find_map(|pair| pair.strip_prefix("path="))
                    .unwrap_or("");
                let dir = percent_decode(raw);
                match fs_browse::browse(&dir) {
                    Ok((path, entries)) => responder.answer(
                        200,
                        &serde_json::json!({ "path": path, "entries": entries }),
                    ),
                    Err(error) => not_found(&mut responder, error),
                }
            }
            _ => method_not_allowed(&mut responder),
        };
    }

    if url == "/api/ingestion/parse" {
        return match method {
            Method::Post => {
                let value: serde_json::Value =
                    serde_json::from_str(&body).map_err(|error| error.to_string())?;
                let markdown = value
                    .get("markdown")
                    .or_else(|| value.get("content"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if markdown.trim().is_empty() {
                    return responder.answer(
                        400,
                        &serde_json::json!({ "error": "缺少 markdown/content" }),
                    );
                }
                let agent = value
                    .get("agent")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("generic")
                    .to_string();
                let scope = value
                    .get("scope")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                let safe_markdown = app.redactor.redact_text(&markdown);
                let draft = parse_run_notes(&safe_markdown);
                let steps = redact_vec(&app.redactor, &draft.steps);
                let tools = redact_vec(&app.redactor, &draft.tools_used);
                let plugins = redact_vec(&app.redactor, &draft.plugins_used);
                let artifacts = redact_vec(&app.redactor, &draft.artifacts);
                let evidence = redact_vec(&app.redactor, &draft.evidence_lines);
                let sections = redact_vec(&app.redactor, &draft.sections);
                let redactions = count_redactions(&safe_markdown)
                    + steps.iter().map(|value| count_redactions(value)).sum::<usize>()
                    + tools.iter().map(|value| count_redactions(value)).sum::<usize>()
                    + plugins.iter().map(|value| count_redactions(value)).sum::<usize>()
                    + artifacts.iter().map(|value| count_redactions(value)).sum::<usize>()
                    + evidence.iter().map(|value| count_redactions(value)).sum::<usize>();
                responder.answer(
                    200,
                    &serde_json::json!({
                        "task": draft.task,
                        "scope": scope,
                        "source": { "agent": agent, "declared": true },
                        "materials": {
                            "run_notes": safe_markdown,
                            "steps": steps,
                            "tools_used": tools,
                            "plugins_used": plugins,
                            "artifacts": artifacts,
                        },
                        "evidence": {
                            "provided": serde_json::Value::Null,
                            "suggested": evidence,
                        },
                        "sections": sections,
                        "redacted": true,
                        "redactions": redactions,
                    }),
                )
            }
            _ => method_not_allowed(&mut responder),
        };
    }

    if url == "/api/ingestion/run-notes" {
        return match method {
            Method::Post => {
                let value: serde_json::Value =
                    serde_json::from_str(&body).map_err(|error| error.to_string())?;
                let markdown = value
                    .get("markdown")
                    .or_else(|| value.get("content"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if markdown.trim().is_empty() {
                    return responder.answer(
                        400,
                        &serde_json::json!({ "error": "缺少 markdown/content" }),
                    );
                }
                let agent = value
                    .get("agent")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                let scope = value
                    .get("scope")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                let actor = value
                    .get("actor")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("user")
                    .to_string();
                let reason = value
                    .get("reason")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let diff_summary = value
                    .get("evidence")
                    .and_then(|evidence| evidence.get("diff_summary"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                let verified_file_count = value
                    .get("evidence")
                    .and_then(|evidence| evidence.get("verified_files"))
                    .and_then(serde_json::Value::as_array)
                    .map(|items| items.len())
                    .unwrap_or(0);
                let workspace_evidence = diff_summary.is_some() || verified_file_count > 0;
                let safe_markdown = app.redactor.redact_text(&markdown);
                let draft = parse_run_notes(&safe_markdown);
                let steps = redact_vec(&app.redactor, &draft.steps);
                let tools = redact_vec(&app.redactor, &draft.tools_used);
                let plugins = redact_vec(&app.redactor, &draft.plugins_used);
                let artifacts = redact_vec(&app.redactor, &draft.artifacts);
                let suggested = redact_vec(&app.redactor, &draft.evidence_lines);
                let redactions = count_redactions(&safe_markdown)
                    + steps.iter().map(|value| count_redactions(value)).sum::<usize>()
                    + tools.iter().map(|value| count_redactions(value)).sum::<usize>()
                    + plugins.iter().map(|value| count_redactions(value)).sum::<usize>()
                    + artifacts.iter().map(|value| count_redactions(value)).sum::<usize>()
                    + suggested.iter().map(|value| count_redactions(value)).sum::<usize>();
                let nanos = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_nanos())
                    .unwrap_or(0);
                let id = format!(
                    "ref_{}_{}",
                    slugify(agent.as_deref().unwrap_or("generic"), 24),
                    nanos
                );
                let mut evidence_parts = Vec::new();
                if let Some(diff) = &diff_summary {
                    evidence_parts.push(diff.clone());
                }
                if verified_file_count > 0 {
                    evidence_parts.push(format!("verified_files={verified_file_count}"));
                }
                if !artifacts.is_empty() {
                    evidence_parts.push(format!("artifacts={}", artifacts.join(",")));
                }
                if !suggested.is_empty() {
                    evidence_parts.push(format!(
                        "suggested={}",
                        l3_cap(&suggested.join("; "), 200)
                    ));
                }
                let entry = ReferenceEntry {
                    id: id.clone(),
                    title: draft.task.clone(),
                    scope: scope.clone(),
                    tags: Vec::new(),
                    body: safe_markdown.clone(),
                    steps: steps.clone(),
                    tools_used: tools.clone(),
                    plugins_used: plugins.clone(),
                    source_agent: agent.clone(),
                    declared: true,
                    trust_level: trust_level_from(workspace_evidence).to_string(),
                    evidence_summary: if evidence_parts.is_empty() {
                        None
                    } else {
                        Some(evidence_parts.join("; "))
                    },
                    created_at: l1_now_secs(),
                };
                let trust = entry.trust_level.clone();
                let steps_count = entry.steps.len();
                let mut store = app.store.lock().unwrap();
                store.insert_reference(entry).map_err(|error| error.to_string())?;
                persist(&app, &store);
                append_l1_ledger(
                    &app,
                    &L1LedgerRecord {
                        record_type: "ingested".to_string(),
                        candidate_name: id.clone(),
                        session_id: None,
                        thread_id: None,
                        trace_version: None,
                        action: Some("ingest_run_notes".to_string()),
                        from: None,
                        to: None,
                        reason: Some(format!(
                            "source=run-notes;trust={trust};scope={};reason={reason};actor={actor}",
                            scope.as_deref().unwrap_or("(unscoped)")
                        )),
                        outcome: "ok".to_string(),
                        recorded_at: l1_now_secs(),
                    },
                );
                responder.answer(
                    201,
                    &serde_json::json!({
                        "ok": true,
                        "reference_id": id,
                        "trust_level": trust,
                        "task": draft.task,
                        "steps": steps_count,
                        "redacted": true,
                        "redactions": redactions,
                    }),
                )
            }
            _ => method_not_allowed(&mut responder),
        };
    }

    if url == "/api/ingestion/guide" || url.starts_with("/api/ingestion/guide?") {
        return match method {
            Method::Get => {
                let agent = query_param(&url, "agent").unwrap_or_else(|| "generic".to_string());
                responder.answer( 200, &ingestion_guide(&agent))
            }
            _ => method_not_allowed(&mut responder),
        };
    }

    if url == "/api/ingestion/package" {
        return match method {
            Method::Post => {
                let value: serde_json::Value =
                    serde_json::from_str(&body).map_err(|error| error.to_string())?;
                let text = |path: &[&str]| -> Option<String> {
                    let mut current = &value;
                    for key in path {
                        current = current.get(key)?;
                    }
                    current.as_str().map(str::to_string)
                };
                let task = text(&["task"]).ok_or_else(|| "缺少 'task'".to_string())?;
                let actor = text(&["actor"]).unwrap_or_else(|| "user".to_string());
                let source_agent = text(&["source", "agent"]);
                let scope = text(&["scope"]);
                let run_notes = text(&["materials", "run_notes"]).unwrap_or_default();
                let collect = |path: &[&str]| -> Vec<String> {
                    let mut current = &value;
                    for key in path {
                        let Some(next) = current.get(key) else {
                            return Vec::new();
                        };
                        current = next;
                    }
                    current
                        .as_array()
                        .map(|items| {
                            items
                                .iter()
                                .filter_map(serde_json::Value::as_str)
                                .map(str::to_string)
                                .collect()
                        })
                        .unwrap_or_default()
                };
                let steps = collect(&["materials", "steps"]);
                let tools_used = collect(&["materials", "tools_used"]);
                let plugins_used = collect(&["materials", "plugins_used"]);
                let artifacts = collect(&["materials", "artifacts"]);
                let diff_summary = text(&["evidence", "diff_summary"]);
                let verified_file_count = value
                    .get("evidence")
                    .and_then(|evidence| evidence.get("verified_files"))
                    .and_then(serde_json::Value::as_array)
                    .map(|items| items.len())
                    .unwrap_or(0);
                let task = app.redactor.redact_text(&task);
                let run_notes = app.redactor.redact_text(&run_notes);
                let steps = redact_vec(&app.redactor, &steps);
                let tools_used = redact_vec(&app.redactor, &tools_used);
                let plugins_used = redact_vec(&app.redactor, &plugins_used);
                let artifacts = redact_vec(&app.redactor, &artifacts);
                let diff_summary = diff_summary.map(|value| app.redactor.redact_text(&value));
                let redactions = count_redactions(&task)
                    + count_redactions(&run_notes)
                    + steps.iter().map(|value| count_redactions(value)).sum::<usize>()
                    + tools_used.iter().map(|value| count_redactions(value)).sum::<usize>()
                    + plugins_used.iter().map(|value| count_redactions(value)).sum::<usize>()
                    + artifacts.iter().map(|value| count_redactions(value)).sum::<usize>()
                    + diff_summary
                        .as_deref()
                        .map(count_redactions)
                        .unwrap_or(0);
                let workspace_evidence = diff_summary.is_some() || verified_file_count > 0;
                let nanos = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_nanos())
                    .unwrap_or(0);
                let id = format!(
                    "ref_{}_{}",
                    slugify(source_agent.as_deref().unwrap_or("generic"), 24),
                    nanos
                );
                let mut evidence_parts = Vec::new();
                if let Some(diff) = &diff_summary {
                    evidence_parts.push(diff.clone());
                }
                if verified_file_count > 0 {
                    evidence_parts.push(format!("verified_files={verified_file_count}"));
                }
                if !artifacts.is_empty() {
                    evidence_parts.push(format!("artifacts={}", artifacts.join(",")));
                }
                let entry = ReferenceEntry {
                    id: id.clone(),
                    title: task,
                    scope: scope.clone(),
                    tags: Vec::new(),
                    body: run_notes,
                    steps,
                    tools_used,
                    plugins_used,
                    source_agent,
                    declared: true,
                    trust_level: trust_level_from(workspace_evidence).to_string(),
                    evidence_summary: if evidence_parts.is_empty() {
                        None
                    } else {
                        Some(evidence_parts.join("; "))
                    },
                    created_at: l1_now_secs(),
                };
                let trust = entry.trust_level.clone();
                let mut store = app.store.lock().unwrap();
                store.insert_reference(entry).map_err(|error| error.to_string())?;
                persist(&app, &store);
                append_l1_ledger(
                    &app,
                    &L1LedgerRecord {
                        record_type: "ingested".to_string(),
                        candidate_name: id.clone(),
                        session_id: None,
                        thread_id: None,
                        trace_version: None,
                        action: Some("ingest_reference".to_string()),
                        from: None,
                        to: None,
                        reason: Some(format!(
                            "trust={trust};scope={};actor={actor}",
                            scope.as_deref().unwrap_or("(unscoped)")
                        )),
                        outcome: "ok".to_string(),
                        recorded_at: l1_now_secs(),
                    },
                );
                responder.answer(
                    201,
                    &serde_json::json!({
                        "ok": true,
                        "reference_id": id,
                        "trust_level": trust,
                        "redacted": true,
                        "redactions": redactions,
                    }),
                )
            }
            _ => method_not_allowed(&mut responder),
        };
    }

    if url == "/api/references" || url.starts_with("/api/references?") {
        return match method {
            Method::Get => {
                let scope = query_param(&url, "scope");
                let tag = query_param(&url, "tag");
                let store = app.store.lock().unwrap();
                let mut entries: Vec<&ReferenceEntry> =
                    store.references_in_scope(scope.as_deref());
                if let Some(tag) = &tag {
                    entries.retain(|entry| entry.tags.iter().any(|value| value == tag));
                }
                let mut values: Vec<serde_json::Value> = entries
                    .iter()
                    .map(|entry| serde_json::to_value(entry).unwrap())
                    .collect();
                values.sort_by(|left, right| {
                    let left_at = left["created_at"].as_u64().unwrap_or(0);
                    let right_at = right["created_at"].as_u64().unwrap_or(0);
                    right_at.cmp(&left_at)
                });
                responder.answer(
                    200,
                    &serde_json::json!({ "references": values }),
                )
            }
            _ => method_not_allowed(&mut responder),
        };
    }

    if let Some(rest) = url.strip_prefix("/api/references/") {
        if let Some(id) = rest.strip_suffix("/promote") {
            if method != Method::Post {
                return method_not_allowed(&mut responder);
            }
            let actor = request_actor(&body);
            let entry = {
                let store = app.store.lock().unwrap();
                store
                    .reference(id)
                    .cloned()
                    .ok_or_else(|| format!("reference '{id}' not found"))?
            };
            if entry.steps.is_empty() {
                return responder.answer(
                    400,
                    &serde_json::json!({ "error": "reference 缺少 steps，无法提升为可执行候选" }),
                );
            }
            let agent_id = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|value| {
                    value
                        .get("agent")
                        .or_else(|| value.get("agent_id"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                });
            let (distiller, fallback_agent) =
                match resolve_agent_distiller(&app, agent_id.as_deref()) {
                Ok(resolved) => resolved,
                Err(message) => {
                    return responder.answer(
                        400,
                        &serde_json::json!({
                            "error": message,
                            "code": "compiler_unavailable",
                            "warnings": promote_warnings(),
                        }),
                    );
                }
            };
            let warnings = promote_warnings_with(fallback_agent);
            let round = RoundTrace {
                task: entry.title.clone(),
                events: entry
                    .steps
                    .iter()
                    .map(|step| TraceEvent::ToolCall {
                        name: "exec_command".to_string(),
                        call_id: None,
                        args_summary: Some(step.clone()),
                    })
                    .collect(),
            };
            let Some(mut draft) = distiller.distill(&round) else {
                return responder.answer(
                    400,
                    &serde_json::json!({ "error": "LLM compiler 未能抽出可执行体（诚实拒绝）" }),
                );
            };
            if let Err(issue) = validate_editable_body(&draft) {
                return responder.answer(
                    400,
                    &serde_json::json!({ "error": issue }),
                );
            }
            if draft.name.trim().is_empty() {
                draft.name = candidate_name(&entry.title);
            }
            draft.status = ExperienceStatus::Candidate;
            let candidate = draft.name.clone();
            let mut store = app.store.lock().unwrap();
            store.insert(draft).map_err(|error| error.to_string())?;
            persist(&app, &store);
            append_l1_ledger(
                &app,
                &L1LedgerRecord {
                    record_type: "promoted_reference".to_string(),
                    candidate_name: candidate.clone(),
                    session_id: None,
                    thread_id: None,
                    trace_version: None,
                    action: Some("promote_reference".to_string()),
                    from: Some(id.to_string()),
                    to: Some(candidate.clone()),
                    reason: Some(format!("actor={actor}")),
                    outcome: "ok".to_string(),
                    recorded_at: l1_now_secs(),
                },
            );
            return responder.answer(
                201,
                &serde_json::json!({
                    "ok": true,
                    "candidate": candidate,
                    "warnings": warnings,
                }),
            );
        }
    }

    if url == "/api/experiences/export" || url.starts_with("/api/experiences/export?") {
        return match method {
            Method::Get => {
                let scope = query_param(&url, "scope");
                let store = app.store.lock().unwrap();
                let export = export_experiences(&store, scope.as_deref());
                responder.answer( 200, &export)
            }
            _ => method_not_allowed(&mut responder),
        };
    }

    if url == "/api/experiences/import" {
        return match method {
            Method::Post => {
                let value: serde_json::Value =
                    serde_json::from_str(&body).map_err(|error| error.to_string())?;
                if value.get("scope").is_none() {
                    return responder.answer(
                        400,
                        &serde_json::json!({ "error": "import 需要 'scope'（可为 null）" }),
                    );
                }
                let scope = value
                    .get("scope")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                let actor = value
                    .get("actor")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("user")
                    .to_string();
                let payload = value
                    .get("payload")
                    .ok_or_else(|| "缺少 'payload'".to_string())?
                    .clone();
                let mut store = app.store.lock().unwrap();
                let count = import_experiences(&mut store, &payload, scope.as_deref())?;
                persist(&app, &store);
                append_l1_ledger(
                    &app,
                    &L1LedgerRecord {
                        record_type: "imported".to_string(),
                        candidate_name: format!("{count} experiences"),
                        session_id: None,
                        thread_id: None,
                        trace_version: None,
                        action: Some("import".to_string()),
                        from: None,
                        to: None,
                        reason: Some(format!("scope={};actor={actor}", scope.as_deref().unwrap_or("(unscoped)"))),
                        outcome: "ok".to_string(),
                        recorded_at: l1_now_secs(),
                    },
                );
                responder.answer(
                    201,
                    &serde_json::json!({ "ok": true, "imported": count }),
                )
            }
            _ => method_not_allowed(&mut responder),
        };
    }

    if url == "/api/state/snapshot" || url.starts_with("/api/state/snapshot?") {
        return match method {
            Method::Get => {
                let cwd = query_param(&url, "cwd")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| app.home.clone());
                responder.answer( 200, &state_snapshot(&cwd))
            }
            _ => method_not_allowed(&mut responder),
        };
    }

    if url == "/api/experiences" || url.starts_with("/api/experiences?") {
        return match method {
            Method::Get => {
                let scope_filter = query_param(&url, "scope");
                let status_filter = query_param(&url, "status");
                let usage_min = query_param(&url, "usage_min")
                    .and_then(|value| value.parse::<u64>().ok());
                let usage = if usage_min.is_some() {
                    let _guard = app.audit.lock().unwrap();
                    Some(load_usage_unlocked(&app))
                } else {
                    None
                };
                let mut summaries: Vec<serde_json::Value> = {
                    let store = app.store.lock().unwrap();
                    store
                        .all()
                        .iter()
                        .filter(|experience| {
                            store.is_in_scope(&experience.name, scope_filter.as_deref())
                                && status_filter.as_deref().map_or(true, |status| {
                                    status_str(experience.status) == status
                                })
                        })
                        .map(|experience| {
                            let mut item = summary(
                                experience,
                                store.is_pinned(&experience.name),
                                store.scope_of(&experience.name),
                            );
                            item["display_name"] = store
                                .display_name_of(&experience.name)
                                .map(|value| serde_json::json!(value))
                                .unwrap_or(serde_json::Value::Null);
                            item["user_usage"] = store
                                .user_usage_of(&experience.name)
                                .map(|value| serde_json::json!(value))
                                .unwrap_or(serde_json::Value::Null);
                            item["user_confidence"] = store
                                .user_confidence_of(&experience.name)
                                .map(|value| serde_json::json!(value))
                                .unwrap_or(serde_json::Value::Null);
                            // S3.5 provenance: hosts/UI can tell where an
                            // experience came from without touching domain types.
                            item["origin"] = store
                                .candidate_origin_of(&experience.name)
                                .map(|origin| serde_json::to_value(origin).unwrap_or(serde_json::Value::Null))
                                .unwrap_or(serde_json::Value::Null);
                            item
                        })
                        .collect()
                };
                if let (Some(min), Some(usage)) = (usage_min, usage) {
                    summaries.retain(|item| {
                        item.get("name")
                            .and_then(serde_json::Value::as_str)
                            .and_then(|name| usage.entries.get(name))
                            .map(|entry| entry.activity.usage_count >= min)
                            .unwrap_or(false)
                    });
                }
                responder.answer( 200, &serde_json::Value::Array(summaries))
            }
            Method::Post => {
                let experience: Experience = serde_json::from_str(&body)
                    .map_err(|error| format!("invalid experience body: {error}"))?;
                let mut store = app.store.lock().unwrap();
                store.insert(experience).map_err(|error| error.to_string())?;
                persist(&app, &store);
                responder.answer( 201, &serde_json::json!({"ok": true}))
            }
            _ => method_not_allowed(&mut responder),
        };
    }

    if url == "/api/experience-tree" || url.starts_with("/api/experience-tree?") {
        return match method {
            Method::Get => {
                let scope = query_param(&url, "scope");
                let status = query_param(&url, "status");
                let store = app.store.lock().unwrap();
                let tree = experience_tree(&store, scope.as_deref(), status.as_deref());
                responder.answer( 200, &tree)
            }
            _ => method_not_allowed(&mut responder),
        };
    }

    // S3.5 read-only similarity view: same-family clusters + ranked pairs.
    // Never merges; hosts/UI decide what to do with the suggestion.
    if url == "/api/similarity" || url.starts_with("/api/similarity?") {
        return match method {
            Method::Get => {
                let scope = query_param(&url, "scope");
                let threshold = query_param(&url, "threshold")
                    .and_then(|value| value.parse::<f64>().ok())
                    .unwrap_or(0.5);
                let cap = query_param(&url, "cap")
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(50);
                let store = app.store.lock().unwrap();
                let items: Vec<&Experience> = store
                    .all()
                    .iter()
                    .filter(|experience| {
                        store.is_in_scope(&experience.name, scope.as_deref())
                    })
                    .collect();
                let clusters: Vec<serde_json::Value> = similarity::clusters(&items, threshold)
                    .into_iter()
                    .map(|cluster| {
                        serde_json::json!({
                            "label": cluster.label,
                            "members": cluster.members,
                            "max_similarity": cluster.max_similarity,
                        })
                    })
                    .collect();
                let pairs: Vec<serde_json::Value> =
                    similarity::pairs(&items, threshold, cap)
                        .into_iter()
                        .map(|pair| {
                            serde_json::json!({
                                "left": pair.left,
                                "right": pair.right,
                                "similarity": pair.similarity,
                            })
                        })
                        .collect();
                responder.answer(
                    200,
                    &serde_json::json!({
                        "scope": scope,
                        "threshold": threshold,
                        "clusters": clusters,
                        "pairs": pairs,
                    }),
                )
            }
            _ => method_not_allowed(&mut responder),
        };
    }

    // S3 read-only registry view: which predicate families are registered and
    // how fresh their observations are. Hosts can use this to build their own
    // routing/UI on top of the same contract.
    if url == "/api/state/sources" {
        return match method {
            Method::Get => {
                let registry = serde_json::json!({
                    "families": [
                        {
                            "id": "fs",
                            "predicates": ["cwd.exists", "file:<p>.exists", "file:<p>.content", "file:<p>.size", "file:<p>.sha256", "dir:<p>.exists"],
                            "freshness_ttl_secs": 60,
                            "requires_policy": null
                        },
                        {
                            "id": "exec",
                            "predicates": ["process.exit_code:<step_id>", "process.stdout_contains:<step_id>"],
                            "freshness_ttl_secs": 30,
                            "requires_policy": "exec=allowlist"
                        },
                        {
                            "id": "git",
                            "predicates": ["git.dirty", "git.branch", "git.last_commit"],
                            "freshness_ttl_secs": 30,
                            "requires_policy": null
                        },
                        {
                            "id": "http",
                            "predicates": ["http.status:<url>", "http.body_sha256:<url>"],
                            "freshness_ttl_secs": 15,
                            "requires_policy": "network"
                        },
                        {
                            "id": "net",
                            "predicates": ["port.open:<n>"],
                            "freshness_ttl_secs": 10,
                            "requires_policy": null
                        }
                    ],
                    "unregistered_keys_are": "unknown",
                    "unknown_semantics": "not-executed",
                });
                responder.answer(200, &registry)
            }
            _ => method_not_allowed(&mut responder),
        };
    }

    // S4 settings surface: file-backed, environment variables still win.
    if url == "/api/settings" {
        return match method {
            Method::Get => {
                let settings = app.settings.lock().unwrap().clone();
                let env_compiler = std::env::var("EXPERIENCE_LLM_COMPILER").ok();
                let env_injection = std::env::var("EXPERIENCE_INJECTION_POLICY").ok();
                responder.answer(
                    200,
                    &serde_json::json!({
                        "settings": settings,
                        "effective": {
                            "llm_compiler": llm_compiler_setting(&app),
                            "injection_policy": injection_policy_setting(&app),
                        },
                        "env_override": {
                            "llm_compiler": env_compiler,
                            "injection_policy": env_injection,
                        },
                        "redaction": {
                            "mode": std::env::var("EXPERIENCE_REDACTION").unwrap_or_else(|_| "standard".to_string()),
                            "disclosure": std::env::var("EXPERIENCE_DISCLOSURE").unwrap_or_else(|_| "structure".to_string()),
                        },
                    }),
                )
            }
            Method::Put => {
                let value: serde_json::Value = match serde_json::from_str(&body) {
                    Ok(value) => value,
                    Err(error) => {
                        return responder.answer(
                            400,
                            &serde_json::json!({ "error": format!("invalid settings body: {error}") }),
                        )
                    }
                };
                let mut settings = app.settings.lock().unwrap();
                if let Some(value) = value.get("llm_compiler").and_then(serde_json::Value::as_bool) {
                    settings.llm_compiler = value;
                }
                if let Some(value) = value
                    .get("injection_policy")
                    .and_then(serde_json::Value::as_bool)
                {
                    settings.injection_policy = value;
                }
                if let Some(value) = value.get("undo_keep").and_then(serde_json::Value::as_u64) {
                    settings.undo_keep = value.clamp(1, 200) as u32;
                }
                let snapshot = settings.clone();
                if let Err(error) = settings.save(&app.home) {
                    return responder.answer(
                        500,
                        &serde_json::json!({ "error": error }),
                    );
                }
                drop(settings);
                append_l1_ledger(
                    &app,
                    &L1LedgerRecord {
                        record_type: "settings_updated".to_string(),
                        candidate_name: "settings".to_string(),
                        session_id: None,
                        thread_id: None,
                        trace_version: None,
                        action: Some("set_settings".to_string()),
                        from: None,
                        to: None,
                        reason: Some(format!(
                            "actor={};llm_compiler={};injection_policy={};undo_keep={}",
                            value
                                .get("actor")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("local-user"),
                            snapshot.llm_compiler,
                            snapshot.injection_policy,
                            snapshot.undo_keep
                        )),
                        outcome: "ok".to_string(),
                        recorded_at: l1_now_secs(),
                    },
                );
                responder.answer(
                    200,
                    &serde_json::json!({
                        "ok": true,
                        "settings": snapshot,
                        "effective": {
                            "llm_compiler": llm_compiler_setting(&app),
                            "injection_policy": injection_policy_setting(&app),
                        },
                    }),
                )
            }
            _ => method_not_allowed(&mut responder),
        };
    }

    // S4 undo surface: recent experience executions joined with their backup
    // snapshots, so the UI can offer one-click restore.
    if url == "/api/undo" || url.starts_with("/api/undo?") {
        return match method {
            Method::Get => {
                let limit = query_param(&url, "limit")
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(20);
                let _guard = app.audit.lock().unwrap();
                let ledger = std::fs::read_to_string(app.home.join("learning-l1.json"))
                    .unwrap_or_default();
                let records = audit_query(&ledger, Some("experience_execution"), None, Some(limit));
                let mut entries: Vec<serde_json::Value> = Vec::new();
                for record in records {
                    let session = record
                        .get("session_id")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let reason = record
                        .get("reason")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let mut snapshots: Vec<serde_json::Value> = Vec::new();
                    if valid_backup_id(&session) {
                        let root = app.home.join("backups").join(&session);
                        if let Ok(entries) = std::fs::read_dir(&root) {
                            for entry in entries.flatten() {
                                if !entry.path().is_dir() {
                                    continue;
                                }
                                let manifest = entry.path().join("manifest.json");
                                if !manifest.exists() {
                                    continue;
                                }
                                let parsed = std::fs::read_to_string(&manifest)
                                    .ok()
                                    .and_then(|json| {
                                        serde_json::from_str::<serde_json::Value>(&json).ok()
                                    });
                                snapshots.push(serde_json::json!({
                                    "snapshot": entry.file_name().to_string_lossy(),
                                    "path": entry.path().to_string_lossy(),
                                    "manifest": parsed,
                                }));
                            }
                        }
                    }
                    entries.push(serde_json::json!({
                        "candidate_name": record.get("candidate_name"),
                        "outcome": record.get("outcome"),
                        "session_id": session,
                        "recorded_at": record.get("recorded_at"),
                        "backup_hint": reason.contains("backup="),
                        "snapshots": snapshots,
                    }));
                }
                responder.answer(200, &serde_json::json!({ "runs": entries }))
            }
            _ => method_not_allowed(&mut responder),
        };
    }

    if let Some(rest) = url.strip_prefix("/api/experiences/") {
        let (name, action) = match rest.split_once('/') {
            Some((name, action)) => (name, Some(action)),
            None => (rest, None),
        };
        return match (method, action) {
            (Method::Get, None) => {
                let store = app.store.lock().unwrap();
                match store.get(name) {
                    Some(experience) => {
                        let mut value = serde_json::to_value(experience).unwrap();
                        if let Some(scope) = store.scope_of(name) {
                            value["scope"] = serde_json::json!(scope);
                        } else {
                            value["scope"] = serde_json::Value::Null;
                        }
                        value["display_name"] = store
                            .display_name_of(name)
                            .map(|display| serde_json::json!(display))
                            .unwrap_or(serde_json::Value::Null);
                        value["user_usage"] = store
                            .user_usage_of(name)
                            .map(|usage| serde_json::json!(usage))
                            .unwrap_or(serde_json::Value::Null);
                        value["user_confidence"] = store
                            .user_confidence_of(name)
                            .map(|value| serde_json::json!(value))
                            .unwrap_or(serde_json::Value::Null);
                        responder.answer( 200, &value)
                    }
                    None => not_found(&mut responder, format!("experience '{name}' not found")),
                }
            }
            (Method::Delete, None) => {
                let mut store = app.store.lock().unwrap();
                store.remove(name).map_err(|error| error.to_string())?;
                persist(&app, &store);
                responder.answer( 200, &serde_json::json!({"ok": true}))
            }
            (Method::Post, Some("scope")) => {
                let value: serde_json::Value =
                    serde_json::from_str(&body).map_err(|error| error.to_string())?;
                let scope = value
                    .get("scope")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                let actor = value
                    .get("actor")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("user")
                    .to_string();
                let mut store = app.store.lock().unwrap();
                store
                    .set_scope(name, scope.as_deref())
                    .map_err(|error| error.to_string())?;
                persist(&app, &store);
                append_l1_ledger(
                    &app,
                    &L1LedgerRecord {
                        record_type: "scoped".to_string(),
                        candidate_name: name.to_string(),
                        session_id: None,
                        thread_id: None,
                        trace_version: None,
                        action: Some("set_scope".to_string()),
                        from: None,
                        to: None,
                        reason: Some(format!(
                            "scope={};actor={actor}",
                            scope.as_deref().unwrap_or("(unscoped)")
                        )),
                        outcome: "ok".to_string(),
                        recorded_at: l1_now_secs(),
                    },
                );
                responder.answer(
                    200,
                    &serde_json::json!({ "ok": true, "scope": scope }),
                )
            }
            (Method::Post, Some("display_name")) => {
                let value: serde_json::Value =
                    serde_json::from_str(&body).map_err(|error| error.to_string())?;
                let display_name = value
                    .get("display_name")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .map(str::to_string);
                let actor = value
                    .get("actor")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("user")
                    .to_string();
                let mut store = app.store.lock().unwrap();
                store
                    .set_display_name(name, display_name.as_deref())
                    .map_err(|error| error.to_string())?;
                persist(&app, &store);
                append_l1_ledger(
                    &app,
                    &L1LedgerRecord {
                        record_type: "renamed".to_string(),
                        candidate_name: name.to_string(),
                        session_id: None,
                        thread_id: None,
                        trace_version: None,
                        action: Some("set_display_name".to_string()),
                        from: None,
                        to: None,
                        reason: Some(format!(
                            "display_name={};actor={actor}",
                            display_name.as_deref().unwrap_or("(cleared)")
                        )),
                        outcome: "ok".to_string(),
                        recorded_at: l1_now_secs(),
                    },
                );
                responder.answer(
                    200,
                    &serde_json::json!({ "ok": true, "display_name": display_name }),
                )
            }
            (Method::Post, Some("user-preference")) => {
                let value: serde_json::Value =
                    serde_json::from_str(&body).map_err(|error| error.to_string())?;
                let usage = value
                    .get("usage")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                let confidence = value
                    .get("confidence")
                    .and_then(serde_json::Value::as_f64);
                let actor = value
                    .get("actor")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("user")
                    .to_string();
                let reason = value
                    .get("reason")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let mut store = app.store.lock().unwrap();
                store
                    .set_user_usage(name, usage.as_deref())
                    .map_err(|error| error.to_string())?;
                store
                    .set_user_confidence(name, confidence)
                    .map_err(|error| error.to_string())?;
                persist(&app, &store);
                append_l1_ledger(
                    &app,
                    &L1LedgerRecord {
                        record_type: "preference_updated".to_string(),
                        candidate_name: name.to_string(),
                        session_id: None,
                        thread_id: None,
                        trace_version: None,
                        action: Some("set_user_preference".to_string()),
                        from: None,
                        to: None,
                        reason: Some(format!(
                            "usage={};confidence={};reason={};actor={actor}",
                            usage.as_deref().unwrap_or("auto"),
                            confidence
                                .map(|value| format!("{value:.2}"))
                                .unwrap_or_else(|| "neutral".to_string()),
                            reason
                        )),
                        outcome: "ok".to_string(),
                        recorded_at: l1_now_secs(),
                    },
                );
                responder.answer( 200, &serde_json::json!({ "ok": true }))
            }
            (Method::Post, Some("draft")) => {
                let actor = request_actor(&body);
                if name.ends_with("__draft") {
                    return responder.answer(
                        400,
                        &serde_json::json!({ "error": "draft 不能再 draft" }),
                    );
                }
                let mut store = app.store.lock().unwrap();
                let original = store
                    .get(name)
                    .cloned()
                    .ok_or_else(|| format!("experience '{name}' not found"))?;
                let draft_name = format!("{name}__draft");
                if store.get(&draft_name).is_some() {
                    return responder.answer(
                        400,
                        &serde_json::json!({ "error": "draft 已存在" }),
                    );
                }
                let mut draft = original;
                draft.name = draft_name.clone();
                draft.status = ExperienceStatus::Draft;
                store.insert(draft).map_err(|error| error.to_string())?;
                persist(&app, &store);
                append_l1_ledger(
                    &app,
                    &L1LedgerRecord {
                        record_type: "drafted".to_string(),
                        candidate_name: draft_name.clone(),
                        session_id: None,
                        thread_id: None,
                        trace_version: None,
                        action: Some("draft".to_string()),
                        from: Some(name.to_string()),
                        to: Some(draft_name),
                        reason: Some(format!("actor:{actor}")),
                        outcome: "ok".to_string(),
                        recorded_at: l1_now_secs(),
                    },
                );
                responder.answer( 201, &serde_json::json!({ "ok": true }))
            }
            (Method::Put, Some("body")) => {
                let mut edited: Experience = serde_json::from_str(&body)
                    .map_err(|error| format!("invalid experience body: {error}"))?;
                validate_editable_body(&edited)?;
                let actor = request_actor(&body);
                let mut store = app.store.lock().unwrap();
                let current = store
                    .get(name)
                    .map(|experience| experience.status)
                    .ok_or_else(|| format!("experience '{name}' not found"))?;
                if current != ExperienceStatus::Draft {
                    return responder.answer(
                        400,
                        &serde_json::json!({ "error": "仅 draft 可编辑；请先 POST /draft" }),
                    );
                }
                edited.name = name.to_string();
                store
                    .replace_existing(name, edited)
                    .map_err(|error| error.to_string())?;
                persist(&app, &store);
                append_l1_ledger(
                    &app,
                    &L1LedgerRecord {
                        record_type: "edited".to_string(),
                        candidate_name: name.to_string(),
                        session_id: None,
                        thread_id: None,
                        trace_version: None,
                        action: Some("edit_body".to_string()),
                        from: None,
                        to: None,
                        reason: Some(format!("actor:{actor}")),
                        outcome: "ok".to_string(),
                        recorded_at: l1_now_secs(),
                    },
                );
                responder.answer( 200, &serde_json::json!({ "ok": true }))
            }
            (Method::Post, Some("adopt")) => {
                let actor = request_actor(&body);
                let Some(original_name) = name.strip_suffix("__draft") else {
                    return responder.answer(
                        400,
                        &serde_json::json!({ "error": "adopt 目标必须是 __draft" }),
                    );
                };
                let mut store = app.store.lock().unwrap();
                let draft = store
                    .get(name)
                    .cloned()
                    .ok_or_else(|| format!("draft '{name}' not found"))?;
                if draft.status != ExperienceStatus::Draft {
                    return responder.answer(
                        400,
                        &serde_json::json!({ "error": "仅 Draft 状态可 adopt" }),
                    );
                }
                if store.get(original_name).is_none() {
                    return responder.answer(
                        404,
                        &serde_json::json!({ "error": format!("原经验 '{original_name}' 不存在") }),
                    );
                }
                validate_editable_body(&draft)?;
                let mut replacement = draft.clone();
                replacement.name = original_name.to_string();
                store
                    .replace_existing(original_name, replacement)
                    .map_err(|error| error.to_string())?;
                store.remove(name).map_err(|error| error.to_string())?;
                persist(&app, &store);
                append_l1_ledger(
                    &app,
                    &L1LedgerRecord {
                        record_type: "adopted".to_string(),
                        candidate_name: original_name.to_string(),
                        session_id: None,
                        thread_id: None,
                        trace_version: None,
                        action: Some("adopt".to_string()),
                        from: Some(name.to_string()),
                        to: Some(original_name.to_string()),
                        reason: Some(format!("actor:{actor}")),
                        outcome: "ok".to_string(),
                        recorded_at: l1_now_secs(),
                    },
                );
                responder.answer( 200, &serde_json::json!({ "ok": true }))
            }
            (Method::Post, Some(pin_action @ ("pin" | "unpin"))) => {
                let actor = request_actor(&body);
                let mut store = app.store.lock().unwrap();
                if pin_action == "pin" {
                    store.pin(name).map_err(|error| error.to_string())?;
                } else {
                    store.unpin(name).map_err(|error| error.to_string())?;
                }
                persist(&app, &store);
                append_l1_ledger(
                    &app,
                    &L1LedgerRecord {
                        record_type: if pin_action == "pin" {
                            "pinned".to_string()
                        } else {
                            "unpinned".to_string()
                        },
                        candidate_name: name.to_string(),
                        session_id: None,
                        thread_id: None,
                        trace_version: None,
                        action: Some(pin_action.to_string()),
                        from: None,
                        to: None,
                        reason: Some(format!("actor:{actor}")),
                        outcome: "ok".to_string(),
                        recorded_at: l1_now_secs(),
                    },
                );
                responder.answer( 200, &serde_json::json!({"ok": true}))
            }
            (Method::Patch, Some("status")) => {
                let value: serde_json::Value =
                    serde_json::from_str(&body).map_err(|error| error.to_string())?;
                let verb = value
                    .get("status")
                    .or_else(|| value.get("action"))
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| "missing 'status'/'action' string".to_string())?;
                // All status changes go through the domain transition table;
                // direct remove+insert status writes are removed (P2-1).
                let action = match verb {
                    "active" | "activate" => QualificationAction::Activate,
                    "force_activate" => QualificationAction::ForceActivate,
                    "invalidate" => QualificationAction::Invalidate,
                    "disable" | "disabled" | "draft" => QualificationAction::Disable,
                    "revalidate" => QualificationAction::Revalidate,
                    "validate" => QualificationAction::Validate,
                    other => {
                        return responder.answer(
                            400,
                            &serde_json::json!({
                                "error": format!("unknown status action '{other}'")
                            }),
                        );
                    }
                };
                let reason = value
                    .get("reason")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                if matches!(action, QualificationAction::ForceActivate)
                    && reason.as_deref().map(str::trim).unwrap_or_default().is_empty()
                {
                    return responder.answer(
                        400,
                        &serde_json::json!({
                            "error": "force_activate requires a 'reason'"
                        }),
                    );
                }
                let mut store = app.store.lock().unwrap();
                let experience = store
                    .get(name)
                    .cloned()
                    .ok_or_else(|| format!("experience '{name}' not found"))?;
                let current = experience.status;
                // Validate/Revalidate must pass the qualification gates. L3
                // State sources (cwd/file) are wired, so fs-bound predicates
                // qualify; unbound/placeholder predicates stay rejected.
                if matches!(action, QualificationAction::Validate)
                    || matches!(action, QualificationAction::Revalidate)
                {
                    if let Err(issues) = qualify(&experience, &resolve_default_source) {
                        return responder.answer(
                            400,
                            &serde_json::json!({ "error": "qualification failed", "issues": issues }),
                        );
                    }
                }
                // Idempotent no-ops for already-terminal targets.
                let next = if (matches!(action, QualificationAction::Activate)
                    || matches!(action, QualificationAction::ForceActivate))
                    && current == ExperienceStatus::Active
                {
                    current
                } else if matches!(action, QualificationAction::Disable)
                    && current == ExperienceStatus::Disabled
                {
                    current
                } else {
                    store
                        .transition_status(name, action)
                        .map_err(|error| error.to_string())?
                };
                persist(&app, &store);
                append_l1_ledger(
                    &app,
                    &L1LedgerRecord {
                        record_type: match action {
                            QualificationAction::Validate => "promoted".into(),
                            QualificationAction::Activate => "activated".into(),
                            QualificationAction::ForceActivate => "override".into(),
                            QualificationAction::Invalidate => "invalidated".into(),
                            QualificationAction::Disable => "disabled".into(),
                            QualificationAction::Revalidate => "revalidated".into(),
                            QualificationAction::Decay => "decayed".into(),
                        },
                        candidate_name: name.to_string(),
                        session_id: None,
                        thread_id: None,
                        trace_version: None,
                        action: Some(verb.to_string()),
                        from: Some(status_str(current).to_string()),
                        to: Some(status_str(next).to_string()),
                        reason,
                        outcome: "ok".into(),
                        recorded_at: l1_now_secs(),
                    },
                );
                responder.answer(
                    200,
                    &serde_json::json!({ "ok": true, "status": next }),
                )
            }
            _ => method_not_allowed(&mut responder),
        };
    }

    not_found(&mut responder, format!("unknown api path: {url}"))
}

fn summary(experience: &Experience, pinned: bool, scope: Option<&str>) -> serde_json::Value {
    serde_json::json!({
        "name": experience.name,
        "status": experience.status,
        "trigger_tool": experience.trigger.tool,
        "workflow_steps": experience.workflow.len(),
        "postconditions": experience.postconditions.len(),
        "pinned": pinned,
        "scope": scope,
    })
}

fn query_param(url: &str, key: &str) -> Option<String> {
    let query = url.split_once('?').map(|(_, query)| query).unwrap_or("");
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix(&format!("{key}=")))
        .map(percent_decode)
}

/// Family key derived from the experience name: optional `cand_` prefix is
/// stripped, then the first `_`-separated token is the family.
fn family_of(name: &str) -> String {
    let stripped = name.strip_prefix("cand_").unwrap_or(name);
    match stripped.split_once('_') {
        Some((family, _)) if !family.is_empty() => family.to_string(),
        _ => stripped.to_string(),
    }
}

/// Stage C1 tree view: Root -> Scene(scope) -> Family -> Experience.
/// Organization/navigation only; matching semantics are untouched.
fn experience_tree(
    store: &ExperienceStore,
    scope: Option<&str>,
    status: Option<&str>,
) -> serde_json::Value {
    let mut scenes: BTreeMap<String, BTreeMap<String, Vec<serde_json::Value>>> = BTreeMap::new();
    for experience in store.all() {
        if !store.is_in_scope(&experience.name, scope) {
            continue;
        }
        if let Some(status) = status {
            if status_str(experience.status) != status {
                continue;
            }
        }
        let scene = store
            .scope_of(&experience.name)
            .unwrap_or(if scope.is_some() { scope.unwrap() } else { "(未分类)" })
            .to_string();
        let family = family_of(&experience.name);
        scenes
            .entry(scene)
            .or_default()
            .entry(family)
            .or_default()
            .push(serde_json::json!({
                "kind": "experience",
                "name": experience.name,
                "status": experience.status,
                "pinned": store.is_pinned(&experience.name),
                "scope": store.scope_of(&experience.name),
            }));
    }
    let scene_nodes: Vec<serde_json::Value> = scenes
        .into_iter()
        .map(|(scene, families)| {
            let family_nodes: Vec<serde_json::Value> = families
                .into_iter()
                .map(|(family, experiences)| {
                    serde_json::json!({
                        "kind": "family",
                        "name": family,
                        "children": experiences,
                    })
                })
                .collect();
            serde_json::json!({
                "kind": "scene",
                "name": scene,
                "children": family_nodes,
            })
        })
        .collect();
    serde_json::json!({
        "kind": "root",
        "name": "experience",
        "children": scene_nodes,
    })
}

/// C4 embodied/scale channel: scope-filtered export (store envelope shape,
/// additive metadata included; identity maps filtered to the exported set).
fn export_experiences(store: &ExperienceStore, scope: Option<&str>) -> serde_json::Value {
    let names: Vec<String> = store
        .all()
        .iter()
        .filter(|experience| store.is_in_scope(&experience.name, scope))
        .map(|experience| experience.name.clone())
        .collect();
    let experiences: Vec<&Experience> = store
        .all()
        .iter()
        .filter(|experience| names.contains(&experience.name))
        .collect();
    let keep = |value: &str| names.iter().any(|name| name == value);
    serde_json::json!({
        "schema_version": 1,
        "experiences": experiences
            .iter()
            .map(|experience| serde_json::to_value(experience).unwrap())
            .collect::<Vec<_>>(),
        "pinned": store.pinned().iter().filter(|name| keep(name)).collect::<Vec<_>>(),
        "scopes": store
            .all()
            .iter()
            .filter(|experience| names.contains(&experience.name))
            .filter_map(|experience| {
                store
                    .scope_of(&experience.name)
                    .map(|scope| (experience.name.clone(), scope.to_string()))
            })
            .collect::<BTreeMap<String, String>>(),
        "display_names": store
            .all()
            .iter()
            .filter(|experience| names.contains(&experience.name))
            .filter_map(|experience| {
                store
                    .display_name_of(&experience.name)
                    .map(|value| (experience.name.clone(), value.to_string()))
            })
            .collect::<BTreeMap<String, String>>(),
        "user_usage": store
            .all()
            .iter()
            .filter(|experience| names.contains(&experience.name))
            .filter_map(|experience| {
                store
                    .user_usage_of(&experience.name)
                    .map(|value| (experience.name.clone(), value.to_string()))
            })
            .collect::<BTreeMap<String, String>>(),
        "user_confidence": store
            .all()
            .iter()
            .filter(|experience| names.contains(&experience.name))
            .filter_map(|experience| {
                store
                    .user_confidence_of(&experience.name)
                    .map(|value| (experience.name.clone(), value))
            })
            .collect::<BTreeMap<String, f64>>(),
    })
}

/// C4 channel import: payload = store envelope (or raw array); `scope` must
/// be present at the endpoint (may be null -> unscoped). Fail-fast on
/// duplicates or invalid bodies.
fn import_experiences(
    store: &mut ExperienceStore,
    payload: &serde_json::Value,
    scope: Option<&str>,
) -> Result<usize, String> {
    let array = payload
        .get("experiences")
        .and_then(serde_json::Value::as_array)
        .or_else(|| payload.as_array())
        .ok_or_else(|| "payload 缺少 experiences 数组".to_string())?;
    let payload_scopes: BTreeMap<String, String> = payload
        .get("scopes")
        .and_then(serde_json::Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(name, value)| {
                    value
                        .as_str()
                        .map(|scope| (name.clone(), scope.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    for (index, item) in array.iter().enumerate() {
        let experience: Experience =
            serde_json::from_value(item.clone()).map_err(|error| {
                format!("第 {} 条解析失败: {error}", index + 1)
            })?;
        validate_editable_body(&experience)?;
        let name = experience.name.clone();
        store
            .insert(experience)
            .map_err(|error| format!("第 {} 条: {error}", index + 1))?;
        // Explicit scope param wins; otherwise keep the payload's embedded
        // scope (null scope at the endpoint means "as exported").
        let assigned = scope
            .map(str::to_string)
            .or_else(|| payload_scopes.get(&name).cloned());
        store
            .set_scope(&name, assigned.as_deref())
            .map_err(|error| error.to_string())?;
    }
    Ok(array.len())
}

/// Read-only State snapshot for the embodied channel: directory listing only
/// (names/kind/size), never file contents, never a mutation surface.
fn state_snapshot(cwd: &Path) -> serde_json::Value {
    let exists = cwd.is_dir();
    let entries: Vec<serde_json::Value> = if exists {
        std::fs::read_dir(cwd)
            .map(|reader| {
                reader
                    .filter_map(Result::ok)
                    .map(|entry| {
                        let kind = entry
                            .file_type()
                            .map(|file_type| {
                                if file_type.is_dir() {
                                    "dir"
                                } else {
                                    "file"
                                }
                            })
                            .unwrap_or("unknown");
                        let size = entry
                            .metadata()
                            .map(|metadata| metadata.len())
                            .unwrap_or(0);
                        serde_json::json!({
                            "name": entry.file_name().to_string_lossy(),
                            "kind": kind,
                            "size": size,
                        })
                    })
                    .take(500)
                    .collect()
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    serde_json::json!({
        "cwd": cwd.to_string_lossy(),
        "exists": exists,
        "read_only": true,
        "entries": entries,
    })
}

fn session_summary(session: &Session) -> serde_json::Value {
    let chars: Vec<char> = session.task.chars().collect();
    let task = if chars.len() > 200 {
        let mut truncated: String = chars.iter().take(200).collect();
        truncated.push('…');
        truncated
    } else {
        session.task.clone()
    };
    serde_json::json!({
        "id": session.id,
        "agent_id": session.agent_id,
        "task": task,
        "scope": session.scope,
        "capability_allowlist": session.capability_allowlist,
        "status": session.status,
        "created_at": session.created_at,
        "finished_at": session.finished_at,
        "summary": session.summary,
    })
}

fn cap_output_chars(output: String, max: usize) -> String {
    let count = output.chars().count();
    if count <= max {
        return output;
    }
    let keep: String = output.chars().skip(count - max).collect();
    format!("…[输出过长，已截断 {count} 字符]\n{keep}")
}

/// Auto-kill timeout is DISABLED by default: real codex tasks often run far
/// longer than any wall clock, and silence is solved by visibility (live
/// output + user cancel), not by a limit. Only an explicit
/// `EXPERIENCE_SESSION_TIMEOUT_SECS` enables an opt-in kill.
fn session_timeout() -> Option<std::time::Duration> {
    std::env::var("EXPERIENCE_SESSION_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .map(std::time::Duration::from_secs)
}

/// Watch one delegated executor: live output and cancel detection. The only
/// terminal states are a real process exit or a user-initiated cancel; no
/// artificial wall-clock kill unless explicitly configured.
fn run_session_worker(
    sessions: Arc<SessionStore>,
    child: Arc<Mutex<Child>>,
    id: String,
    timeout: Option<std::time::Duration>,
) {
    let buffer: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    {
        let mut guard = child.lock().unwrap();
        if let Some(mut stdout) = guard.stdout.take() {
            let buffer = Arc::clone(&buffer);
            std::thread::spawn(move || drain_into(buffer, &mut stdout));
        }
        if let Some(mut stderr) = guard.stderr.take() {
            let buffer = Arc::clone(&buffer);
            std::thread::spawn(move || drain_into(buffer, &mut stderr));
        }
    }

    let started = Instant::now();
    let mut last_push = Instant::now();
    loop {
        if !sessions.has_child(&id) {
            return; // cancelled by the user
        }
        std::thread::sleep(std::time::Duration::from_millis(400));
        if last_push.elapsed().as_secs() >= 1 {
            let snapshot = buffer.lock().unwrap().clone();
            sessions.update_output(&id, snapshot);
            last_push = Instant::now();
        }
        let status = match child.lock().unwrap().try_wait() {
            Ok(Some(status)) => Some(status),
            Ok(None) => None,
            Err(error) => {
                sessions.finish(
                    &id,
                    false,
                    format!("executor 状态检查失败：{error}"),
                    buffer.lock().unwrap().clone(),
                );
                return;
            }
        };
        if let Some(status) = status {
            let raw = buffer.lock().unwrap().clone();
            let ok = status.success();
            let summary = raw
                .lines()
                .rev()
                .find(|line| !line.trim().is_empty())
                .unwrap_or_else(|| {
                    if ok {
                        "（无输出）"
                    } else {
                        "executor 执行失败"
                    }
                })
                .to_string();
            sessions.finish(&id, ok, summary, raw);
            return;
        }
        if let Some(timeout) = timeout {
            if Instant::now().duration_since(started) > timeout {
                let _ = child.lock().unwrap().kill();
                sessions.finish(
                    &id,
                    false,
                    format!(
                        "executor 超过 {} 秒未完成，已按显式配置自动终止",
                        timeout.as_secs()
                    ),
                    buffer.lock().unwrap().clone(),
                );
                return;
            }
        }
    }
}

fn drain_into(buffer: Arc<Mutex<String>>, reader: &mut dyn std::io::Read) {
    let mut buffered = BufReader::new(reader);
    loop {
        let mut bytes = Vec::new();
        match buffered.read_until(b'\n', &mut bytes) {
            Ok(0) => break,
            Ok(_) => {
                let text = String::from_utf8_lossy(&bytes);
                buffer.lock().unwrap().push_str(&text);
            }
            Err(_) => break,
        }
    }
}

fn persist(app: &App, store: &ExperienceStore) {
    if let Err(error) = store.save_to_path(&app.home.join("store.json")) {
        eprintln!("failed to persist store: {error}");
    }
}

/// L1 sink hook after a completed session round: typed v1 round ->
/// necessity -> distill -> CandidateWriter. Failures only log (audit side);
/// learning never alters the user-facing session path.
fn l1_sink_session(app: &App, sessions: &SessionStore, session_id: &str) {
    let Some(session) = sessions.get(session_id) else {
        return;
    };
    let Ok(value) = serde_json::to_value(&session) else {
        return;
    };
    let Some(round) = select_round(&value, RoundSelection::Last) else {
        return;
    };
    let name = candidate_name(&round.task);
    let mut store = app.store.lock().unwrap();
    let existing = store.get(&name).is_some();
    let distiller: Box<dyn L1Distiller> = l1_distiller_for(app, &session);
    let distiller_label = if distiller.accepts_dirty() { "llm" } else { "deterministic" };
    let ledger_outcome = match sink_once(&round, true, existing, &mut store, distiller.as_ref()) {
        Ok(None) => {
            store.set_candidate_origin(
                &name,
                CandidateOrigin {
                    task_signature: experience_core::experience::learning_l1::task_signature(
                        &round.task,
                    ),
                    session_id: Some(session_id.to_string()),
                    distiller: distiller_label.to_string(),
                    recorded_at: l1_now_secs(),
                },
            );
            persist(app, &store);
            eprintln!("l1 sink: wrote candidate {name} (session {session_id})");
            "written".to_string()
        }
        Ok(Some(reason)) => {
            eprintln!("l1 sink: skip session {session_id} reason={reason}");
            format!("rejected:{reason}")
        }
        Err(error) => {
            eprintln!("l1 sink error session {session_id}: {error}");
            format!("error:{error}")
        }
    };
    append_l1_ledger(
        app,
        &L1LedgerRecord {
            record_type: if ledger_outcome.starts_with("written") {
                "written"
            } else if ledger_outcome.starts_with("rejected") {
                "rejected"
            } else {
                "error"
            }
            .to_string(),
            candidate_name: name,
            session_id: Some(session_id.to_string()),
            thread_id: session.thread_id.clone(),
            trace_version: Some(TRACE_SCHEMA_VERSION),
            action: None,
            from: None,
            to: None,
            reason: None,
            outcome: ledger_outcome,
            recorded_at: l1_now_secs(),
        },
    );
}

/// Stage S3: choose the L1 distiller. `EXPERIENCE_LLM_COMPILER=on` routes
/// dirty rounds to a real one-shot compiler (codex exec, JSON-only); the
/// deterministic fallback stays the default and refuses dirty rounds.
fn l1_distiller_for(app: &App, session: &Session) -> Box<dyn L1Distiller> {
    match llm_distiller_available(app, Some(&session.agent_id)) {
        Some(distiller) => Box::new(distiller),
        None => Box::new(DeterministicDistiller),
    }
}

/// Resolve a concrete LLM compiler (env override or a managed agent), used by
/// L1 dirty rounds and by reference promotion (Stage A).
fn llm_distiller_available(app: &App, agent_id: Option<&str>) -> Option<CodexLlmDistiller> {
    if !llm_compiler_setting(app) {
        return None;
    }
    resolve_agent_distiller(app, agent_id).ok().map(|(distiller, _)| distiller)
}

/// Resolve a distiller for a user-initiated promote (no env gate: the click
/// itself is the consent). Only "no executable at all" is a hard refusal.
fn resolve_agent_distiller(
    app: &App,
    agent_id: Option<&str>,
) -> Result<(CodexLlmDistiller, bool), String> {
    let mut fallback = false;
    let (agent_exe, agent_home) = {
        let agents = app.agents.lock().ok();
        match agents {
            Some(agents) => match agent_id {
                Some(id) => match agents.get(id) {
                    Some(entry) => (
                        entry.config.resolve_executable().ok(),
                        entry.config.codex_home.clone(),
                    ),
                    None => (None, None),
                },
                None => agents
                    .list()
                    .iter()
                    .find_map(|entry| {
                        entry.config.resolve_executable().ok().map(|exe| {
                            (Some(exe), entry.config.codex_home.clone())
                        })
                    })
                    .map(|value| {
                        fallback = true;
                        value
                    })
                    .unwrap_or((None, None)),
            },
            None => (None, None),
        }
    };
    let exe = std::env::var_os("EXPERIENCE_LLM_CODEX")
        .map(PathBuf::from)
        .or(agent_exe)
        .ok_or_else(|| {
            "找不到可用的助手可执行文件（请在 Agents 页配置助手，或设置 EXPERIENCE_LLM_CODEX）"
                .to_string()
        })?;
    let codex_home = agent_home
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("CODEX_HOME").map(PathBuf::from));
    Ok((
        CodexLlmDistiller {
            exe,
            codex_home,
            redactor: Some(app.redactor.clone()),
        },
        fallback,
    ))
}

/// Supplementary notes returned with a successful promote (and shown in the
/// UI): this is a user-consented model call whose output is only a CANDIDATE.
fn promote_warnings() -> Vec<&'static str> {
    vec![
        "提升会调用一次助手模型（可能耗时并消耗 token）",
        "抽取结果只会写入 CANDIDATE，仍需 L2 验证（可观察判据）后才能激活",
        "参考材料缺少可复现步骤或可观察完成判据时，抽取可能失败或停留在参考经验",
    ]
}

fn promote_warnings_with(fallback_agent: bool) -> Vec<&'static str> {
    let mut warnings = promote_warnings();
    if fallback_agent {
        warnings.push("未显式指定助手，已使用自动发现的 codex；建议在调用时指定 agent");
    }
    warnings
}

/// Resolve the LLM compiler flag: explicit env var wins, otherwise the
/// file-backed setting (S4).
fn llm_compiler_setting(app: &App) -> bool {
    match std::env::var("EXPERIENCE_LLM_COMPILER").ok() {
        Some(value) => injection_policy_from(Some(value.as_str())),
        None => app.settings.lock().unwrap().llm_compiler,
    }
}

/// Resolve the injection policy: explicit env var wins, otherwise settings.
fn injection_policy_setting(app: &App) -> bool {
    match std::env::var("EXPERIENCE_INJECTION_POLICY").ok() {
        Some(value) => injection_policy_from(Some(value.as_str())),
        None => app.settings.lock().unwrap().injection_policy,
    }
}

/// One-shot LLM compiler: input is the pruned success-path summary (task +
/// tool steps), never raw trace text; output must be a single JSON Experience
/// (schema-validated downstream by L1 gate 2 + CandidateWriter).
struct CodexLlmDistiller {
    exe: PathBuf,
    codex_home: Option<PathBuf>,
    redactor: Option<Redactor>,
}

impl L1Distiller for CodexLlmDistiller {
    fn accepts_dirty(&self) -> bool {
        true
    }

    fn distill(&self, round: &RoundTrace) -> Option<Experience> {
        let prompt = llm_distill_prompt(round);
        let prompt = self
            .redactor
            .as_ref()
            .map(|redactor| redactor.redact_text(&prompt))
            .unwrap_or(prompt);
        let output = self.compile(&prompt)?;
        let mut draft = parse_llm_experience(&output)?;
        if draft.name.trim().is_empty() {
            draft.name = candidate_name(&round.task);
        }
        draft.status = ExperienceStatus::Candidate;
        Some(draft)
    }
}

impl CodexLlmDistiller {
    fn compile(&self, prompt: &str) -> Option<String> {
        let mut command = std::process::Command::new(&self.exe);
        command
            .arg("exec")
            .arg("--skip-git-repo-check")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .env("EXPERIENCE_ENABLED", "0");
        if let Some(home) = &self.codex_home {
            command.env("CODEX_HOME", home);
        }
        let mut child = command.spawn().ok()?;
        child
            .stdin
            .take()?
            .write_all(prompt.as_bytes())
            .ok()?;
        let mut stdout = child.stdout.take()?;
        let deadline = Instant::now() + std::time::Duration::from_secs(120);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        return None;
                    }
                    break;
                }
                Ok(None) => {
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        return None;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
                Err(_) => return None,
            }
        }
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).ok()?;
        String::from_utf8(bytes).ok()
    }
}

/// Minimal success-path summary for the compiler call (lightweight rule:
/// never inject the raw trace or reasoning text).
fn llm_distill_prompt(round: &RoundTrace) -> String {
    let mut lines = vec![format!("任务: {}", round.task), "已执行工具步骤:".to_string()];
    for event in &round.events {
        if let TraceEvent::ToolCall {
            name,
            args_summary,
            ..
        } = event
        {
            let args = args_summary.clone().unwrap_or_default();
            lines.push(format!("- {name}: {args}"));
        }
    }
    lines.push(
        "输出仅一个 JSON（不要 Markdown 解释），结构：{\"name\":string,\"trigger\":{\"tool\":\"exec_command\",\"command_pattern\":string|null},\"preconditions\":[{\"key\":\"cwd.exists\",\"expected\":true}],\"workflow\":[{\"action\":\"write_file|exec_command\",\"args\":{}}],\"postconditions\":[{\"key\":string,\"expected\":value}],\"verification\":[],\"failure_policy\":\"stop_and_report\",\"undo\":\"unsupported\",\"status\":\"candidate\"}"
            .to_string(),
    );
    lines.join("\n")
}

/// Robust JSON extraction: fenced code blocks or prose around the JSON are
/// tolerated; parsing/fallback stays deterministic (never trust shape).
fn parse_llm_experience(text: &str) -> Option<Experience> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end <= start {
        return None;
    }
    let candidate: Experience = serde_json::from_str(&text[start..=end]).ok()?;
    Some(candidate)
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct L1LedgerRecord {
    #[serde(default = "default_ledger_record_type")]
    record_type: String,
    candidate_name: String,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    thread_id: Option<String>,
    #[serde(default)]
    trace_version: Option<u32>,
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    outcome: String,
    recorded_at: u64,
}

fn default_ledger_record_type() -> String {
    "sink".to_string()
}

fn status_str(status: ExperienceStatus) -> &'static str {
    match status {
        ExperienceStatus::Draft => "draft",
        ExperienceStatus::Candidate => "candidate",
        ExperienceStatus::Validated => "validated",
        ExperienceStatus::Active => "active",
        ExperienceStatus::Decaying => "decaying",
        ExperienceStatus::Disabled => "disabled",
    }
}

/// L3 v2 scope label embedded in ledger reasons (audit can tell v1 from v2).
const L3_SCOPE_V2: &str = "scope=l3_v2";
/// v1 path keeps the exact historical reason string (backward compatible).
const L3_REASON_V1: &str = "policy_off;no_in_process_executor_v1";

#[derive(Clone)]
struct L3Executed {
    name: String,
    outcome: String,
}

#[derive(Clone)]
struct L3EntryContext {
    /// Plan context string (ACTIVE names joined); ledger candidate_name.
    plan: String,
    /// Task actually handed to the delegated executor: the ORIGINAL task
    /// plus completed step ids / verified state when an Experience ran first.
    delegated_task: String,
    executed: Option<L3Executed>,
    /// True when a plan-based delegate event/ledger belongs to this entry
    /// (reference-only injection does not create delegation records).
    track_delegation: bool,
}

#[derive(Clone)]
struct L3RunReport {
    outcome: String,
    completed_step_ids: Vec<String>,
    verified_state: Vec<String>,
    detail: String,
    /// Instantiated backup root for this run, when a backup was configured.
    backup_root: Option<String>,
}

/// L3 v2 task entry orchestrator (rulings 2026-09-09):
/// - ACTIVE presence keeps v1 plan-aware delegation semantics (event-free
///   only when no ACTIVE candidate exists);
/// - a text-matched, locally runnable (write_file-only) ACTIVE Experience is
///   executed first; unique best coverage wins, an equal-best tie is a
///   conflict and nothing auto-executes;
/// - whatever the Experience outcome, the ORIGINAL task is delegated with
///   completed step ids / verified state appended (v1 fallback; no LLM text
///   rewriting); delegation/task outcomes never touch Experience confidence.
/// Returns None only for pure delegation with no ACTIVE candidates.
fn l3_entry(
    app: &App,
    sessions: &SessionStore,
    session_id: &str,
    task: &str,
    workspace: &Path,
) -> Option<L3EntryContext> {
    // Stage C1: wake only ACTIVE experiences visible in the session scope
    // (scoped match + unscoped fallback); no scope keeps v1 all-ACTIVE.
    let session_scope = sessions.get(session_id).and_then(|session| session.scope);
    let active: Vec<Experience> = {
        let store = app.store.lock().unwrap();
        store
            .active_in_scope(session_scope.as_deref())
            .into_iter()
            .filter(|experience| store.is_usage_allowed(&experience.name))
            .cloned()
            .collect()
    };
    let plan_names: Vec<String> = active
        .iter()
        .map(|experience| experience.name.clone())
        .collect();
    let plan = plan_names.join("|");

    let matched: Vec<&Experience> = active
        .iter()
        .filter(|experience| l3_task_matches(experience, task))
        .collect();
    let mut skip_audit = String::new();
    let mut runnable: Vec<&Experience> = Vec::new();
    for candidate in matched.iter().copied() {
        if !l3_locally_runnable(candidate) {
            push_audit(
                &mut skip_audit,
                &format!("{}:workflow_not_locally_runnable", candidate.name),
            );
            continue;
        }
        if !l3_preconditions_pass(candidate, workspace) {
            push_audit(
                &mut skip_audit,
                &format!("{}:precondition_not_met", candidate.name),
            );
            continue;
        }
        // S3 Gate 3/Gate 4/Gate 5: the verdict must be observable, and a
        // dry-run must replay cleanly before we touch the real workspace.
        // S1-a/S1-d: policy denial is NOT a preflight skip — the candidate is
        // still selected and the execution is recorded (`invalid`, before any
        // side effect). That keeps the verdict auditable per experience and
        // lets the delegation reason explain the refusal.
        let effective_policy = effective_policy_for(app, candidate, session_scope.as_deref());
        if let Some(reason) = l3_gate_check(candidate, workspace, &effective_policy) {
            push_audit(&mut skip_audit, &format!("{}:{reason}", candidate.name));
            continue;
        }
        if let Some(denied) = l3_policy_blocks(&effective_policy, candidate) {
            push_audit(&mut skip_audit, &format!("{}:{denied}", candidate.name));
        }
        runnable.push(candidate);
    }

    let mut executed: Option<L3Executed> = None;
    let mut run_report: Option<L3RunReport> = None;
    match l3_pick_unique_best(&runnable) {
        Ok(experience) => {
            let effective_policy = effective_policy_for(app, experience, session_scope.as_deref());
            let report = l3_run_workflow(
                experience,
                workspace,
                &effective_policy,
                Some((&app.home, session_id)),
                &app.home,
            );
            append_l1_ledger(
                app,
                &L1LedgerRecord {
                    record_type: "experience_execution".to_string(),
                    candidate_name: experience.name.clone(),
                    session_id: Some(session_id.to_string()),
                    thread_id: None,
                    trace_version: None,
                    action: Some("execute".to_string()),
                    from: None,
                    to: None,
                    reason: {
                        let mut reason = l3_cap(&report.detail, 160);
                        if let Some(root) = &report.backup_root {
                            if !reason.is_empty() {
                                reason.push(';');
                            }
                            reason.push_str("backup=");
                            reason.push_str(&l3_cap(root, 120));
                        }
                        if reason.is_empty() {
                            None
                        } else {
                            Some(reason)
                        }
                    },
                    outcome: report.outcome.clone(),
                    recorded_at: l1_now_secs(),
                },
            );
            usage_record_experience(app, &experience.name, &report.outcome, l1_now_secs());
            executed = Some(L3Executed {
                name: experience.name.clone(),
                outcome: report.outcome.clone(),
            });
            run_report = Some(report);
        }
        Err(conflict) => push_audit(&mut skip_audit, &conflict),
    }

    let delegated_task = match &run_report {
        Some(report) => {
            let mut text =
                delegation_text(task, &report.completed_step_ids, &report.verified_state);
            if let Some(root) = &report.backup_root {
                text.push_str(&format!(
                    "\n[复原本轮文件改动] POST /api/backups/{session_id}/restore body {{\"snapshot\":\"{root}\"}}"
                ));
            }
            text
        }
        None => task.to_string(),
    };
    let delegate_reason = match &executed {
        Some(executed) => format!(
            "policy_off;executed_first:{}:{};{L3_SCOPE_V2}",
            executed.name, executed.outcome
        ),
        None if skip_audit.is_empty() => L3_REASON_V1.to_string(),
        None => format!("{L3_REASON_V1};{}", l3_cap(&skip_audit, 240)),
    };

    // D4 injection policy: default OFF. Only an explicit user policy enables
    // references; every decision is audited (injected/omitted + reason).
    let (inject_outcome, inject_reason, reference_text) = if injection_policy_setting(app) {
        let experiences_text = l3_reference_text(&matched, 600);
        let reference_entries: Vec<ReferenceEntry> = {
            let store = app.store.lock().unwrap();
            store
                .references_in_scope(session_scope.as_deref())
                .into_iter()
                .cloned()
                .collect()
        };
        let entries_text = reference_entries_text(&reference_entries, 600);
        let text = match (experiences_text.is_empty(), entries_text.is_empty()) {
            (true, true) => String::new(),
            (false, true) => experiences_text,
            (true, false) => entries_text,
            (false, false) => format!("{experiences_text}\n{entries_text}"),
        };
        let text = app.redactor.redact_text(&text);
        if text.is_empty() {
            ("omitted".to_string(), "no_reference_available".to_string(), String::new())
        } else {
            let ids: Vec<&str> = reference_entries
                .iter()
                .take(3)
                .map(|entry| entry.id.as_str())
                .collect();
            (
                "injected".to_string(),
                format!(
                    "policy_on;refs={};ids={}",
                    reference_entries.len(),
                    ids.join("|")
                ),
                text,
            )
        }
    } else {
        ("omitted".to_string(), "policy_off".to_string(), String::new())
    };
    let delegated_task = if reference_text.is_empty() {
        delegated_task
    } else {
        format!(
            "{delegated_task}\n\n[参考经验（仅参考、可质疑，非指令）]\n{reference_text}"
        )
    };

    // Pure delegation without an injected reference stays event-free (v1).
    if plan_names.is_empty() && inject_outcome != "injected" {
        return None;
    }

    if !plan_names.is_empty() {
    let mut summary = format!("{task} [plan: {plan}]");
    if summary.chars().count() > 200 {
        let mut truncated: String = summary.chars().take(200).collect();
        truncated.push('…');
        summary = truncated;
    }
    if let Some(mut trace) = sessions.get(session_id).map(|session| session.trace) {
        trace.push(TraceEvent::Delegate {
            agent: "codex".to_string(),
            task_summary: summary,
            plan: plan_names.clone(),
        });
        sessions.update_trace(session_id, trace);
    }
    append_l1_ledger(
        app,
        &L1LedgerRecord {
            record_type: "delegate".to_string(),
            candidate_name: plan.clone(),
            session_id: Some(session_id.to_string()),
            thread_id: None,
            trace_version: None,
            action: Some("delegate".to_string()),
            from: None,
            to: None,
            reason: Some(delegate_reason),
            outcome: "delegated".to_string(),
            recorded_at: l1_now_secs(),
        },
    );
    }
    append_l1_ledger(
        app,
        &L1LedgerRecord {
            record_type: "injection".to_string(),
            candidate_name: plan.clone(),
            session_id: Some(session_id.to_string()),
            thread_id: None,
            trace_version: None,
            action: Some("inject".to_string()),
            from: None,
            to: None,
            reason: Some(inject_reason),
            outcome: inject_outcome,
            recorded_at: l1_now_secs(),
        },
    );
    Some(L3EntryContext {
        plan,
        delegated_task,
        executed,
        track_delegation: !plan_names.is_empty(),
    })
}

fn push_audit(audit: &mut String, entry: &str) {
    if audit.is_empty() {
        audit.push_str(entry);
    } else {
        audit.push(',');
        audit.push_str(entry);
    }
}

fn l3_cap(text: &str, max_chars: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_chars {
        return text.to_string();
    }
    let mut capped: String = chars.iter().take(max_chars).collect();
    capped.push('…');
    capped
}

/// Task-level mechanical matching (L3 v2 ruling): no ActionProposal exists
/// at task entry, so a trigger matches when its tool is a canonical executor
/// tool and a non-empty command_pattern occurs in the task text
/// (case-insensitive substring). No-pattern Experiences never auto-execute
/// at task level (no Task Segments yet; conservative to avoid misfires).
fn l3_task_matches(experience: &Experience, task: &str) -> bool {
    let tool_ok = matches!(
        experience.trigger.tool.as_str(),
        "exec_command" | "write_file" | "read_file"
    );
    if !tool_ok {
        return false;
    }
    match experience.trigger.command_pattern.as_deref() {
        Some(pattern) if !pattern.trim().is_empty() => {
            let task_lower = task.to_lowercase();
            let pattern_lower = pattern.to_lowercase();
            match task_lower.find(&pattern_lower) {
                // Review P1-1: a negation/avoidance token right before the
                // pattern means the task asks NOT to perform the known
                // transition — auto-execution must not fire.
                Some(index) => !l3_negation_before(&task_lower, index),
                None => false,
            }
        }
        _ => false,
    }
}

/// Conservative auto-execution guard: inspect up to 32 characters before the
/// matched pattern for an explicit negative/avoidance intent.
fn l3_negation_before(task_lower: &str, pattern_start: usize) -> bool {
    let prefix = &task_lower[..pattern_start];
    let window: String = prefix
        .chars()
        .rev()
        .take(32)
        .collect::<Vec<char>>()
        .into_iter()
        .rev()
        .collect();
    let tokens: Vec<&str> = window
        .split(|character: char| !character.is_alphanumeric() && character != '\'')
        .filter(|token| !token.is_empty())
        .collect();
    const NEGATIONS: [&str; 8] = [
        "not", "never", "dont", "don't", "avoid", "skip", "without", "except",
    ];
    tokens.iter().any(|token| {
        NEGATIONS.contains(token) || *token == "no"
    })
}

/// The in-process executor surface is write_file-only (LocalRunner
/// contract); exec_command steps still have no in-process executor (v1
/// ruling: delegate instead of half-running).
fn l3_locally_runnable(experience: &Experience) -> bool {
    !experience.workflow.is_empty()
        && experience
            .workflow
            .iter()
            .all(|step| is_locally_executable_action(&step.action))
}

/// S1-b: actions the shared embedded executor implements.
fn is_locally_executable_action(action: &str) -> bool {
    matches!(
        action,
        "write_file"
            | "append_file"
            | "read_file"
            | "mkdir"
            | "copy_file"
            | "move_file"
            | "delete_file"
            | "exec"
            | "exec_command"
    )
}

fn l3_preconditions_pass(experience: &Experience, workspace: &Path) -> bool {
    experience
        .preconditions
        .iter()
        .all(|predicate| {
            // S3 Gate 3: an unregistered precondition is Unknown, not "met".
            state_source::observable(&predicate.key)
                && evaluate_predicate(&predicate.key, &predicate.expected, workspace)
                    == TruthValue::True
        })
}

/// Deterministic Select Plan without Task Segments: best coverage wins; an
/// equal-best tie is a conflict and nothing auto-executes (design rule 7).
fn l3_pick_unique_best<'a>(
    runnable: &'a [&'a Experience],
) -> Result<&'a Experience, String> {
    let Some(best_len) = runnable.iter().map(|exp| exp.workflow.len()).max() else {
        return Err(String::new());
    };
    let best: Vec<&&Experience> = runnable
        .iter()
        .filter(|exp| exp.workflow.len() == best_len)
        .collect();
    match best.as_slice() {
        [only] => Ok(only),
        many => Err(format!(
            "conflict:{}:no_auto_execute",
            many.iter()
                .map(|exp| exp.name.as_str())
                .collect::<Vec<_>>()
                .join("|")
        )),
    }
}

/// Effective capability policy for one experience: its own scope owns the
/// policy; the session scope is the fallback; otherwise the store default.
fn effective_policy_for(
    app: &App,
    experience: &Experience,
    session_scope: Option<&str>,
) -> CapabilityPolicy {
    let store = app.store.lock().unwrap();
    match store.scope_of(&experience.name) {
        Some(scope) => store.policy_for_scope(Some(scope)),
        None => store.policy_for_scope(session_scope),
    }
}

/// First workflow action refused by `policy`, formatted for audit as
/// `policy_denied:<action>:<family>:<reason>` (never contains step args).
fn l3_policy_blocks(policy: &CapabilityPolicy, experience: &Experience) -> Option<String> {
    for step in &experience.workflow {
        if let Err(decision) = policy.check(&step.action) {
            return Some(format!(
                "policy_denied:{}:{}:{}",
                decision.action, decision.family, decision.reason
            ));
        }
    }
    None
}

/// S3 Gate 3/4/5 for one candidate:
/// - every precondition/postcondition must bind to a registered state source;
/// - every step must be locally executable;
/// - the workflow must replay cleanly in a scratch dry-run.
fn l3_gate_check(
    experience: &Experience,
    workspace: &Path,
    policy: &CapabilityPolicy,
) -> Option<String> {
    for predicate in experience.preconditions.iter().chain(experience.postconditions.iter()) {
        if !state_source::observable(&predicate.key) {
            return Some(format!("unobservable_verdict:{}", predicate.key));
        }
    }
    if let Some(step) = experience
        .workflow
        .iter()
        .find(|step| !is_locally_executable_action(&step.action))
    {
        return Some(format!("unexecutable_step:{}", step.action));
    }
    let scratch = std::env::temp_dir().join(format!(
        "exp-l3-dryrun-{}-{}",
        std::process::id(),
        l1_now_secs()
    ));
    if std::fs::create_dir_all(&scratch).is_err() {
        return Some("dryrun_setup_failed".to_string());
    }
    let result = {
        let mut context = StepContext::new(workspace, policy).with_scratch(&scratch);
        let mut failure = None;
        for step in &experience.workflow {
            // Denials are recorded as `invalid` by the real run; the dry-run
            // only certifies that the step's shape is replayable.
            if policy.check(&step.action).is_err() {
                continue;
            }
            // Exec steps are shape-checked by the executor and policy-checked
            // above; the dry-run must never spawn, so it only asserts that the
            // step is something the executor knows how to validate.
            if step.action == "exec" || step.action == "exec_command" {
                continue;
            }
            if let Err(error) = exec_step(&mut context, step) {
                failure = Some(format!("dryrun_failed:{}", error.detail));
                break;
            }
        }
        failure
    };
    let _ = std::fs::remove_dir_all(&scratch);
    result
}

/// Directory-id guard for backup listing/restore (no separators, no `..`).
fn valid_backup_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

/// Whether a single requested capability fits inside `policy` (session
/// `capabilities` may only narrow the scope policy).
fn capability_within_policy(policy: &CapabilityPolicy, capability: &str) -> bool {
    match capability_family(capability) {
        Some("fs_read") => policy.allows("read_file"),
        Some("fs_write") => policy.allows("write_file"),
        Some("fs_delete") => policy.allows("delete_file"),
        // Legacy shell strings need the explicit opt-in even when the exec
        // family is in allowlist mode (Tier 2 legacy_shell rule).
        Some("exec") if capability == "exec_command" => {
            policy.allows("exec") && policy.exec.allow_legacy_shell
        }
        Some("exec") => policy.allows("exec"),
        Some("network") => policy.network.mode != "off",
        // Reserved channels (computer_use) are contract-only for now.
        _ => true,
    }
}

/// Compact, secret-free policy summary for ledger `reason` fields.
fn policy_summary(policy: &CapabilityPolicy) -> String {
    format!(
        "fs_read={}(max={});fs_write={};fs_delete={};exec={};network={}",
        policy.fs_read.enabled,
        policy.read_max_bytes(),
        policy.fs_write,
        policy.fs_delete,
        policy.exec.mode,
        policy.network.mode
    )
}

/// Audit one policy mutation (set/cleared) into the shared ledger.
fn append_policy_ledger(
    app: &App,
    scope: &str,
    actor: &str,
    action: &str,
    reason: Option<&str>,
) {
    append_l1_ledger(
        app,
        &L1LedgerRecord {
            record_type: "policy_updated".to_string(),
            candidate_name: scope.to_string(),
            session_id: None,
            thread_id: None,
            trace_version: None,
            action: Some(action.to_string()),
            from: None,
            to: None,
            reason: Some(match reason {
                Some(reason) => format!("actor={actor};{reason}"),
                None => format!("actor={actor}"),
            }),
            outcome: "completed".to_string(),
            recorded_at: l1_now_secs(),
        },
    );
}

fn l3_run_workflow(
    experience: &Experience,
    workspace: &Path,
    policy: &CapabilityPolicy,
    backup: Option<(&Path, &str)>,
    app_home: &Path,
) -> L3RunReport {
    // S1-c: each run gets its own backup root; the manifest is always written
    // so the UI can offer "undo this run" even when nothing changed.
    let backup_root: Option<PathBuf> = backup.map(|(home, session_id)| {
        home.join("backups")
            .join(session_id)
            .join(format!("run-{}", l1_now_secs()))
    });
    let mut entries: Vec<BackupEntry> = Vec::new();
    let mut completed_step_ids = Vec::new();
    let mut exec_results: BTreeMap<String, ProcessEvidence> = BTreeMap::new();
    let mut exec_notes: Vec<String> = Vec::new();
    let mut failure: Option<(String, String)> = None;
    {
        let mut context = StepContext::new(workspace, policy);
        context = context.with_run_root(app_home.join("run"));
        if let Some(root) = backup_root.as_deref() {
            context = context.with_backup(root, &mut entries);
        }
        for (index, step) in experience.workflow.iter().enumerate() {
            match exec_step(&mut context, step) {
                Ok(execution) => {
                    let id = format!("{}#{}", step.action, index);
                    exec_results.insert(
                        id.clone(),
                        ProcessEvidence {
                            exit_code: execution.exit_code,
                            stdout: execution.stdout.clone(),
                        },
                    );
                    completed_step_ids.push(id);
                    if !execution.stderr.is_empty() {
                        exec_notes.push(format!(
                            "stderr[{}]={}",
                            index,
                            execution
                                .stderr
                                .chars()
                                .take(120)
                                .collect::<String>()
                        ));
                    }
                    if !execution.stdout.is_empty() {
                        exec_notes.push(format!(
                            "stdout[{}]={}",
                            index,
                            execution
                                .stdout
                                .chars()
                                .take(120)
                                .collect::<String>()
                        ));
                    }
                }
                Err(error) => {
                    failure = Some((error.outcome, error.detail));
                    break;
                }
            }
        }
    }
    let (outcome, mut detail) = match failure {
        Some((failed_outcome, failed_detail)) => (failed_outcome, failed_detail),
        None => {
            let verified = l3_verified_state(experience, workspace, &exec_results);
            if verified.len() == experience.postconditions.len() {
                ("success".to_string(), String::new())
            } else {
                (
                    "misfire".to_string(),
                    "postconditions not satisfied after workflow".to_string(),
                )
            }
        }
    };
    if let Some(root) = backup_root.as_deref() {
        if let Err(error) =
            write_backup_manifest(root, workspace, backup.map(|(_, id)| id), &entries)
        {
            if detail.is_empty() {
                detail = format!("backup manifest failed: {error}");
            }
        }
    }
    if !exec_notes.is_empty() {
        let note = exec_notes.join(";");
        detail = if detail.is_empty() {
            note
        } else {
            format!("{detail};{note}")
        };
    }
    L3RunReport {
        outcome,
        completed_step_ids,
        verified_state: l3_verified_state(experience, workspace, &exec_results),
        detail,
        backup_root: backup_root.map(|path| path.to_string_lossy().to_string()),
    }
}

/// Process evidence captured for one completed step (S2): consumed by the
/// `process.*` predicates so a build/test verdict is evidence, not assertion.
#[derive(Debug, Clone, Default)]
struct ProcessEvidence {
    exit_code: Option<i32>,
    stdout: String,
}

/// Evaluate one postcondition, consulting process evidence after local probes.
fn l3_predicate_truth(
    key: &str,
    expected: &serde_json::Value,
    workspace: &Path,
    exec_results: &BTreeMap<String, ProcessEvidence>,
) -> TruthValue {
    // S3 Gate 5: a verdict that is not produced by a registered source is
    // Unknown — never True, never permission to claim completion.
    if !state_source::observable(key) {
        return TruthValue::Unknown;
    }
    if let Some(rest) = key.strip_prefix("process.exit_code:") {
        return match exec_results.get(rest).and_then(|entry| entry.exit_code) {
            Some(code) => match expected.as_i64() {
                Some(want) if want == i64::from(code) => TruthValue::True,
                Some(_) => TruthValue::False,
                None => TruthValue::Unknown,
            },
            None => TruthValue::Unknown,
        };
    }
    if let Some(rest) = key.strip_prefix("process.stdout_contains:") {
        let (step, needle) = (rest, expected.as_str().unwrap_or(""));
        if step.is_empty() || needle.is_empty() {
            return TruthValue::Unknown;
        }
        let Some(entry) = exec_results.get(step) else {
            return TruthValue::Unknown;
        };
        return if entry.stdout.contains(needle) {
            TruthValue::True
        } else {
            TruthValue::False
        };
    }
    evaluate_predicate(key, expected, workspace)
}

fn l3_verified_state(
    experience: &Experience,
    workspace: &Path,
    exec_results: &BTreeMap<String, ProcessEvidence>,
) -> Vec<String> {
    experience
        .postconditions
        .iter()
        .filter(|predicate| {
            l3_predicate_truth(&predicate.key, &predicate.expected, workspace, exec_results)
                == TruthValue::True
        })
        .map(|predicate| format!("{}={}", predicate.key, l3_value_text(&predicate.expected)))
        .collect()
}

fn l3_value_text(value: &serde_json::Value) -> String {
    value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string())
}

/// Injection policy reads `EXPERIENCE_INJECTION_POLICY`; anything but
/// on/enabled/1/true stays OFF (D4 default: no reference injection).
fn injection_policy_from(value: Option<&str>) -> bool {
    matches!(
        value.map(str::to_lowercase).as_deref(),
        Some("on" | "enabled" | "1" | "true")
    )
}

/// Compact reference text from distilled Experience bodies (never raw
/// trace): name + pre-compiled workflow shape + completion criteria.
fn l3_reference_text(experiences: &[&Experience], max_chars: usize) -> String {
    let lines: Vec<String> = experiences
        .iter()
        .map(|experience| {
            let steps: Vec<String> = experience
                .workflow
                .iter()
                .map(l3_step_summary)
                .collect();
            let postconditions: Vec<String> = experience
                .postconditions
                .iter()
                .map(|predicate| predicate.key.clone())
                .collect();
            format!(
                "- {}: {}; 完成判据: {}",
                experience.name,
                steps.join(" -> "),
                postconditions.join(", ")
            )
        })
        .collect();
    let joined = lines.join("\n");
    if joined.chars().count() > max_chars {
        l3_cap(&joined, max_chars)
    } else {
        joined
    }
}

fn l3_step_summary(
    step: &experience_core::domain::experience::WorkflowStep,
) -> String {
    let arg = |key: &str| {
        step.args
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("?")
            .to_string()
    };
    match step.action.as_str() {
        "write_file" => format!("write_file(path={})", arg("path")),
        "exec_command" => format!("exec_command(cmd={})", l3_cap(&arg("cmd"), 120)),
        "read_file" => format!("read_file(path={})", arg("path")),
        other => other.to_string(),
    }
}

/// Stage A: guidance for capturing agent material after the fact.
fn ingestion_guide(agent: &str) -> serde_json::Value {
    serde_json::json!({
        "agent": agent,
        "checklist": [
            "在受控 workspace（建议 git 仓库）中运行 agent，且只允许它改文件",
            "任务前后各做一次快照：git status / git diff --stat",
            "要求 agent 主动产出 run-notes.md（见 prompt_template）",
            "收集最终产物清单与可复验证据（文件路径、内容、hash）",
            "脱敏：不要包含密钥、账号、长 hex/uuid 参数",
        ],
        "workspace_steps": [
            "git init（或确认已有仓库）并提交初始状态",
            "运行 agent 完成任务",
            "git diff --stat > diff_summary.txt；保留 run-notes.md 与产物",
            "把材料按 ingestion package JSON 提交到 POST /api/ingestion/package"
        ],
        "prompt_template": "完成任务后，请输出 run-notes.md，包含：① 目标与约束；② 使用的工具与插件（含版本）；③ 步骤与子步骤（每步输入/输出与参数）；④ 失败与修正；⑤ 最终产物清单与验证方法；⑥ 可复用的模板与参数位（哪些值应参数化）。不要包含任何密钥/账号。"
    })
}

#[derive(Default, Clone)]
struct RunNotesDraft {
    task: String,
    steps: Vec<String>,
    tools_used: Vec<String>,
    plugins_used: Vec<String>,
    artifacts: Vec<String>,
    evidence_lines: Vec<String>,
    sections: Vec<String>,
}

/// Strip a markdown bullet/numbering prefix; None for non-bullet lines.
fn strip_bullet(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    for prefix in ["- ", "* ", "+ ", "• ", "· "] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            let value = rest.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    let digits: String = trimmed
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect();
    if !digits.is_empty() {
        let remainder = &trimmed[digits.len()..];
        if remainder.starts_with(['.', '、', ')', '）', '．']) {
            let value = remainder
                .trim_start_matches(['.', '、', ')', '）', '．'])
                .trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn classify_run_notes_heading(line: &str) -> Option<&'static str> {
    let heading = line
        .trim()
        .trim_start_matches('#')
        .trim()
        .to_lowercase();
    let checks: [(&str, &[&str]); 6] = [
        ("task", &["目标", "约束", "task", "goal", "objective"]),
        ("tools", &["工具", "插件", "tool", "plugin"]),
        ("steps", &["步骤", "子步骤", "流程", "step", "workflow"]),
        ("failure", &["失败", "修正", "问题", "failure", "fix"]),
        (
            "artifacts",
            &["产物", "验证", "evidence", "artifact", "diff", "output"],
        ),
        ("template", &["模板", "参数", "复用", "template", "reuse"]),
    ];
    checks
        .iter()
        .find(|(_, keywords)| keywords.iter().any(|keyword| heading.contains(keyword)))
        .map(|(category, _)| *category)
}

fn clean_plugin_name(item: &str) -> String {
    match item.split_once(['：', ':']) {
        Some((_, value)) if item.contains("插件") || item.to_lowercase().contains("plugin") => {
            value.trim().to_string()
        }
        _ => item.to_string(),
    }
}

/// Split a markdown table row into trimmed cells (None when not a table row).
fn table_cells(line: &str) -> Option<Vec<String>> {
    let trimmed = line.trim();
    if !trimmed.starts_with('|') || !trimmed.ends_with('|') {
        return None;
    }
    let cells: Vec<String> = trimmed
        .trim_matches('|')
        .split('|')
        .map(|cell| cell.trim().to_string())
        .collect();
    if cells.is_empty() {
        None
    } else {
        Some(cells)
    }
}

fn is_table_separator(cells: &[String]) -> bool {
    cells.iter().all(|cell| {
        !cell.is_empty()
            && cell
                .chars()
                .all(|character| character == '-' || character == ':' || character.is_whitespace())
    })
}

fn is_table_header(cells: &[String]) -> bool {
    let joined = cells.join(" ").to_lowercase();
    cells
        .first()
        .map(|first| {
            let first = first.to_lowercase();
            first.contains("类别")
                || first.contains("category")
                || first.contains("字段")
                || first.contains("名称")
                || first.contains("验证项")
        })
        .unwrap_or(false)
        && (joined.contains("名称")
            || joined.contains("version")
            || joined.contains("说明")
            || joined.contains("方法")
            || joined.contains("结果"))
}

fn table_row_text(cells: &[String]) -> String {
    l3_cap(&cells.join(" | "), 200)
}

/// Deterministic parser for agent-produced run-notes.md: title + sections
/// (goal/tools/steps/failure/artifacts/template) -> structured draft fields.
fn parse_run_notes(markdown: &str) -> RunNotesDraft {
    let mut draft = RunNotesDraft::default();
    let mut current: Option<&'static str> = None;
    for line in markdown.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            let label = trimmed.trim_start_matches('#').trim().to_string();
            if !label.is_empty() {
                draft.sections.push(label);
            }
            let level = trimmed.chars().take_while(|character| *character == '#').count();
            match classify_run_notes_heading(trimmed) {
                Some(category) => current = Some(category),
                // Only top-level unknown headings reset the section; nested
                // subheadings (### D1 …) stay inside the current section.
                None if level <= 2 => current = Some("other"),
                None => {}
            }
            continue;
        }
        if trimmed.is_empty() {
            continue;
        }
        match current {
            Some("task") => {
                if draft.task.is_empty() {
                    draft.task = trimmed.to_string();
                }
            }
            Some("tools") => {
                if let Some(cells) = table_cells(trimmed) {
                    if is_table_separator(&cells) || is_table_header(&cells) {
                        continue;
                    }
                    let joined = cells.join(" ");
                    let value = if cells.len() >= 3 {
                        format!("{}（{}）", cells[1], l3_cap(&cells[2], 80))
                    } else if cells.len() == 2 {
                        cells[1].clone()
                    } else {
                        joined.clone()
                    };
                    if joined.contains("插件") || joined.to_lowercase().contains("plugin") {
                        draft.plugins_used.push(value);
                    } else {
                        draft.tools_used.push(value);
                    }
                    continue;
                }
                if let Some(item) = strip_bullet(trimmed) {
                    let cleaned = clean_plugin_name(&item);
                    if cleaned.len() != item.len() {
                        draft.plugins_used.push(cleaned);
                    } else {
                        draft.tools_used.push(item);
                    }
                }
            }
            Some("steps") => {
                if let Some(item) = strip_bullet(trimmed) {
                    draft.steps.push(item);
                }
            }
            Some("artifacts") => {
                if let Some(cells) = table_cells(trimmed) {
                    if is_table_separator(&cells) || is_table_header(&cells) {
                        continue;
                    }
                    let row = table_row_text(&cells);
                    let lower = row.to_lowercase();
                    if lower.contains("diff") || lower.contains("sha") || row.contains("验证") {
                        draft.evidence_lines.push(row.clone());
                    }
                    draft.artifacts.push(row);
                    continue;
                }
                if let Some(item) = strip_bullet(trimmed) {
                    let lower = item.to_lowercase();
                    if lower.contains("diff") || lower.contains("sha") || item.contains("验证") {
                        draft.evidence_lines.push(item.clone());
                    }
                    draft.artifacts.push(item);
                }
            }
            _ => {}
        }
        if draft.task.is_empty() && current.is_none() {
            draft.task = trimmed.trim_start_matches('#').trim().to_string();
        }
    }
    if draft.steps.is_empty() {
        for line in markdown.lines() {
            if let Some(item) = strip_bullet(line) {
                draft.steps.push(item);
            }
        }
    }
    if draft.task.is_empty() {
        draft.task = markdown
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("run-notes")
            .trim_start_matches('#')
            .trim()
            .to_string();
    }
    draft.task = l3_cap(&draft.task, 200);
    draft
}

fn slugify(text: &str, max: usize) -> String {
    let slug: String = text
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = slug.trim_matches('_').replace("__", "_");
    trimmed.chars().take(max).collect()
}

fn trust_level_from(workspace_evidence: bool) -> &'static str {
    if workspace_evidence {
        "workspace_verified"
    } else {
        "declared"
    }
}

/// Render reference entries for the injection slot (never executable).
fn reference_entries_text(entries: &[ReferenceEntry], max_chars: usize) -> String {
    let lines: Vec<String> = entries
        .iter()
        .map(|entry| {
            let steps = if entry.steps.is_empty() {
                entry.body.clone()
            } else {
                entry.steps.join(" -> ")
            };
            format!(
                "- [ref:{}][{}] {}: {}",
                entry.id, entry.trust_level, entry.title, steps
            )
        })
        .collect();
    let joined = lines.join("\n");
    if joined.chars().count() > max_chars {
        l3_cap(&joined, max_chars)
    } else {
        joined
    }
}

const REDACTION_MARKERS: [&str; 5] = ["<masked:", "<payload:", "<id:", "<path:", "<host:"];

fn count_redactions(text: &str) -> usize {
    REDACTION_MARKERS
        .iter()
        .map(|marker| text.matches(marker).count())
        .sum()
}

fn redact_vec(redactor: &Redactor, values: &[String]) -> Vec<String> {
    values
        .iter()
        .map(|value| redactor.redact_text(value))
        .collect()
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct LedgerFile {
    schema_version: u32,
    #[serde(default)]
    records: Vec<L1LedgerRecord>,
}

/// Append one audit record (learning sink or status transition). Envelope is
/// schema_version 1 with typed record_type; legacy flat arrays still load.
/// Learning audit stays out of store.json and out of prompts.
fn append_l1_ledger(app: &App, record: &L1LedgerRecord) {
    let mut record = record.clone();
    record.candidate_name = app.redactor.redact_text(&record.candidate_name);
    record.outcome = app.redactor.redact_text(&record.outcome);
    record.action = record
        .action
        .as_deref()
        .map(|value| app.redactor.redact_text(value));
    record.reason = record
        .reason
        .as_deref()
        .map(|value| app.redactor.redact_text(value));
    let _guard = app.audit.lock().unwrap();
    let path = app.home.join("learning-l1.json");
    let mut file = std::fs::read_to_string(&path)
        .ok()
        .and_then(|json| {
            serde_json::from_str::<LedgerFile>(&json)
                .map(|file| Some(file))
                .or_else(|_| {
                    serde_json::from_str::<Vec<L1LedgerRecord>>(&json)
                        .map(|records| {
                            Some(LedgerFile {
                                schema_version: 1,
                                records,
                            })
                        })
                })
                .ok()
                .flatten()
        })
        .unwrap_or_else(|| LedgerFile {
            schema_version: 1,
            records: Vec::new(),
        });
    file.records.push(record.clone());
    if let Ok(json) = serde_json::to_string_pretty(&file) {
        let _ = std::fs::write(&path, json);
    }
}

fn l1_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// usage.json envelope: per-experience L2 ConfidenceRecord persisted NEXT to
/// store.json (confidence/activity never live inside the Experience body).
const USAGE_SCHEMA_VERSION: u32 = 1;
/// Negative-evidence score floor: ACTIVE decays below this (pin exempt).
const DECAY_SCORE_FLOOR: f64 = 0.30;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct UsageFile {
    schema_version: u32,
    #[serde(default)]
    entries: BTreeMap<String, ConfidenceRecord>,
}

impl Default for UsageFile {
    fn default() -> Self {
        Self {
            schema_version: USAGE_SCHEMA_VERSION,
            entries: BTreeMap::new(),
        }
    }
}

fn usage_path(app: &App) -> PathBuf {
    app.home.join("usage.json")
}

fn load_usage_unlocked(app: &App) -> UsageFile {
    std::fs::read_to_string(usage_path(app))
        .ok()
        .and_then(|json| serde_json::from_str::<UsageFile>(&json).ok())
        .unwrap_or_default()
}

/// Read-only audit query over learning-l1.json: newest first, optional
/// record_type/name filters and limit. Pure for unit tests.
fn audit_query(
    text: &str,
    record_type: Option<&str>,
    name: Option<&str>,
    limit: Option<usize>,
) -> Vec<serde_json::Value> {
    let parsed = serde_json::from_str::<LedgerFile>(text)
        .map(|file| file.records)
        .or_else(|_| {
            serde_json::from_str::<Vec<L1LedgerRecord>>(text)
                .map(|records| records)
        })
        .unwrap_or_default();
    let mut records: Vec<serde_json::Value> = parsed
        .into_iter()
        .filter_map(|record| serde_json::to_value(&record).ok())
        .collect();
    records.sort_by(|left, right| {
        let left_at = left
            .get("recorded_at")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let right_at = right
            .get("recorded_at")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        right_at.cmp(&left_at)
    });
    if let Some(record_type) = record_type {
        records.retain(|record| {
            record
                .get("record_type")
                .and_then(serde_json::Value::as_str)
                == Some(record_type)
        });
    }
    if let Some(name) = name {
        records.retain(|record| {
            record
                .get("candidate_name")
                .and_then(serde_json::Value::as_str)
                .map(|candidate| candidate.contains(name))
                .unwrap_or(false)
        });
    }
    if let Some(limit) = limit {
        records.truncate(limit);
    }
    records
}

fn save_usage_unlocked(app: &App, usage: &UsageFile) {
    if let Ok(json) = serde_json::to_string_pretty(usage) {
        let _ = std::fs::write(usage_path(app), json);
    }
}

fn round_score(score: f64) -> f64 {
    (score * 100.0).round() / 100.0
}

fn outcome_to_experience_outcome(outcome: &str) -> Option<ExperienceOutcome> {
    match outcome {
        "success" => Some(ExperienceOutcome::Success),
        "misfire" => Some(ExperienceOutcome::Misfire),
        "invalid" => Some(ExperienceOutcome::Invalid),
        "execution_error" => Some(ExperienceOutcome::ExecutionError),
        _ => None,
    }
}

/// D4 usage writeback (WP1): only the Experience's own outcome moves its
/// ConfidenceRecord; delegation/task outcomes never touch it.
fn usage_record_experience(app: &App, name: &str, outcome: &str, at: u64) {
    let Some(experience_outcome) = outcome_to_experience_outcome(outcome) else {
        return;
    };
    let score = {
        let _guard = app.audit.lock().unwrap();
        let mut usage = load_usage_unlocked(app);
        let entry = usage.entries.entry(name.to_string()).or_default();
        apply_experience_usage(entry, experience_outcome, at);
        // Persist a stable 2-decimal score (review P2-2: no float drift).
        entry.score = round_score(entry.score);
        let score = entry.score;
        save_usage_unlocked(app, &usage);
        score
    };
    maybe_auto_decay(app, name, score, at);
}

/// Decaying producer (L2 seam closure): negative evidence drops the score
/// under the ACTIVE floor -> domain Decay transition; pinned ACTIVE never
/// auto-decays. Activity/recency alone never triggers decay (L2 ruling 4).
fn maybe_auto_decay(app: &App, name: &str, score: f64, at: u64) {
    if score > DECAY_SCORE_FLOOR {
        return;
    }
    let mut store = app.store.lock().unwrap();
    if store.is_pinned(name) {
        return;
    }
    let Some(experience) = store.get(name).cloned() else {
        return;
    };
    if experience.status != ExperienceStatus::Active {
        return;
    }
    match store.transition_status(name, QualificationAction::Decay) {
        Ok(next) => {
            persist(app, &store);
            append_l1_ledger(
                app,
                &L1LedgerRecord {
                    record_type: "decayed".to_string(),
                    candidate_name: name.to_string(),
                    session_id: None,
                    thread_id: None,
                    trace_version: None,
                    action: Some("decay".to_string()),
                    from: Some(status_str(experience.status).to_string()),
                    to: Some(status_str(next).to_string()),
                    reason: Some(format!("auto_decay:usage_evidence:score={score:.2}")),
                    outcome: "decayed".to_string(),
                    recorded_at: at,
                },
            );
        }
        Err(error) => eprintln!("usage auto-decay skipped {name}: {error}"),
    }
}

fn agents_persist(app: &App, agents: &AgentManager) {
    if let Err(error) = agents.save_to_path(&app.home.join("agents.json")) {
        eprintln!("failed to persist agents: {error}");
    }
}

fn serve_static(ui_dir: &Path, relative: &str, request: Request) -> Result<(), String> {
    let root = ui_dir.canonicalize().unwrap_or_else(|_| ui_dir.to_path_buf());
    let candidate = root.join(relative);
    let candidate = candidate.canonicalize().unwrap_or(candidate);
    if !candidate.starts_with(&root) {
        let response = Response::from_string("path traversal rejected")
            .with_status_code(StatusCode(400));
        return request.respond(response).map_err(|error| error.to_string());
    }
    match std::fs::read(&candidate) {
        Ok(bytes) => {
            let content_type = content_type_for(relative);
            let response = Response::from_data(bytes)
                .with_status_code(StatusCode(200))
                .with_header(
                    Header::from_bytes(&b"Content-Type"[..], content_type.as_bytes()).unwrap(),
                );
            request.respond(response).map_err(|error| error.to_string())
        }
        Err(_) => {
            let response = Response::from_string(format!(
                "resource not found: {relative}"
            ))
            .with_status_code(StatusCode(404));
            request.respond(response).map_err(|error| error.to_string())
        }
    }
}

fn content_type_for(path: &str) -> &'static str {
    let extension = path.rsplit('.').next().unwrap_or("");
    match extension {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        _ => "application/octet-stream",
    }
}

fn json_response(request: Request, status: u16, value: &serde_json::Value) -> Result<(), String> {
    let body = serde_json::to_vec(value).unwrap();
    let response = Response::from_data(body)
        .with_status_code(StatusCode(status))
        .with_header(
            Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap(),
        );
    request.respond(response).map_err(|error| error.to_string())
}

/// 400 response for validation/policy/serialization failures.
fn bad_request(request: Request, message: String) -> Result<(), String> {
    json_response(request, 400, &serde_json::json!({ "error": message }))
}

fn not_found(responder: &mut Responder<'_>, message: String) -> Result<(), String> {
    responder.answer(404, &serde_json::json!({"error": message}))
}

fn method_not_allowed(responder: &mut Responder<'_>) -> Result<(), String> {
    responder.answer(405, &serde_json::json!({"error": "method not allowed"}))
}

fn read_body(request: &mut Request) -> Result<String, String> {
    let mut bytes = Vec::new();
    request
        .as_reader()
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    String::from_utf8(bytes).map_err(|_| "request body must be UTF-8 JSON".to_string())
}

fn request_actor(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("actor")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "user".to_string())
}

/// C2 edit gate: schema valid + tool boundary valid (canonical tools,
/// non-empty args). Lifecycle/name/status are NOT editable via body.
fn validate_editable_body(experience: &Experience) -> Result<(), String> {
    if !experience.is_schema_valid() {
        return Err("schema invalid (name/workflow/postconditions 不可为空)".to_string());
    }
    validate_tool_boundary(experience).map_err(|issue| format!("tool boundary: {issue}"))?;
    Ok(())
}

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|value| value == flag)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let high = hex_value(bytes[index + 1]);
            let low = hex_value(bytes[index + 2]);
            if let (Some(high), Some(low)) = (high, low) {
                decoded.push((high << 4) | low);
                index += 3;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn redaction_mode() -> RedactMode {
    match std::env::var("EXPERIENCE_REDACTION")
        .ok()
        .map(|value| value.to_lowercase())
        .as_deref()
    {
        Some("off") => RedactMode::Off,
        Some("strict") => RedactMode::Strict,
        _ => RedactMode::Standard,
    }
}

fn redaction_disclosure() -> Disclosure {
    match std::env::var("EXPERIENCE_DISCLOSURE")
        .ok()
        .map(|value| value.to_lowercase())
        .as_deref()
    {
        Some("metadata" | "metadata-only") => Disclosure::MetadataOnly,
        Some("content") => Disclosure::Content,
        _ => Disclosure::Structure,
    }
}

/// Per-store HMAC key ("comparable but irreversible"): created once at
/// `<home>/redaction.key`, derived from local entropy via SHA-256.
fn load_or_create_redaction_key(home: &Path) -> Vec<u8> {
    let path = home.join("redaction.key");
    if let Ok(bytes) = std::fs::read(&path) {
        if bytes.len() >= 32 {
            return bytes;
        }
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let seed = format!(
        "{nanos}:{}:{}",
        std::process::id(),
        home.to_string_lossy()
    );
    let key = sha256(seed.as_bytes()).to_vec();
    let _ = std::fs::write(&path, &key);
    key
}

fn default_home() -> PathBuf {
    std::env::var_os("EXPERIENCE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(Path::to_path_buf))
                .map(|dir| dir.parent().unwrap_or(&dir).join(".experience-home"))
                .unwrap_or_else(|| PathBuf::from(".experience-home"))
        })
}

fn find_ui_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("EXPERIENCE_UI_DIR").map(PathBuf::from) {
        return dir;
    }
    if let Ok(exe) = std::env::current_exe() {
        for ancestor in exe.ancestors().take(6) {
            let candidate = ancestor.join("ui").join("www");
            if candidate.join("index.html").is_file() {
                return candidate;
            }
        }
    }
    PathBuf::from("ui/www")
}

#[cfg(test)]
mod tests {
    use super::*;
    use session_manager::SessionStatus;

    fn long_cn_session() -> Session {
        Session {
            id: "s-test".into(),
            agent_id: "codex".into(),
            task: "好".repeat(500),
            cwd: None,
            scope: None,
            capability_allowlist: None,
            thread_id: None,
            trace: Vec::new(),
            round_tasks: Vec::new(),
            status: SessionStatus::Completed,
            created_at: 1,
            finished_at: Some(1),
            summary: Some("ok".into()),
            output: String::new(),
        }
    }

    #[test]
    fn session_summary_truncates_on_char_boundary() {
        let value = session_summary(&long_cn_session());
        let task = value.get("task").and_then(serde_json::Value::as_str).unwrap();
        assert!(task.ends_with('…'));
        assert!(task.chars().count() <= 201);
    }

    #[test]
    fn cap_output_never_splits_utf8() {
        let capped = cap_output_chars("好".repeat(1000), 300);
        assert!(capped.is_char_boundary(capped.len()));
        assert!(capped.contains("截断"));
    }

    fn probe_file_experience(status: &str) -> Experience {
        serde_json::from_value(serde_json::json!({
            "name": "create_probe_file",
            "trigger": {"tool": "exec_command", "command_pattern": "create probe file"},
            "preconditions": [{"key": "cwd.exists", "expected": true}],
            "workflow": [{
                "action": "write_file",
                "args": {"path": "probe.txt", "content": "EXPERIENCE_GATE_SUCCESS"}
            }],
            "postconditions": [
                {"key": "file:probe.txt.exists", "expected": true},
                {"key": "file:probe.txt.content", "expected": "EXPERIENCE_GATE_SUCCESS"}
            ],
            "verification": [],
            "failure_policy": "stop_and_report",
            "undo": "unsupported",
            "status": status
        }))
        .unwrap()
    }

    fn l3_temp_dir(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("exp-l3-unit-{tag}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn l3_task_match_is_mechanical_substring_on_pattern() {
        let experience = probe_file_experience("active");
        assert!(l3_task_matches(
            &experience,
            "please create probe file in this directory using exec_command"
        ));
        assert!(!l3_task_matches(
            &experience,
            "create a random unrelated file"
        ));
        // Review P1-1: negative/avoidance intent must never auto-execute.
        assert!(!l3_task_matches(
            &experience,
            "do not create probe file in this directory, just list files"
        ));
        assert!(!l3_task_matches(
            &experience,
            "skip creating probe file and leave the workspace untouched"
        ));
        assert!(!l3_task_matches(
            &experience,
            "avoid the create probe file flow entirely"
        ));
        assert!(l3_task_matches(
            &experience,
            "please create probe file in this directory using exec_command"
        ));
        let no_pattern: Experience = serde_json::from_value(serde_json::json!({
            "name": "any_exec",
            "trigger": {"tool": "exec_command", "command_pattern": null},
            "preconditions": [{"key": "cwd.exists", "expected": true}],
            "workflow": [{
                "action": "write_file",
                "args": {"path": "a.txt", "content": "x"}
            }],
            "postconditions": [{"key": "file:a.txt.exists", "expected": true}],
            "verification": [],
            "failure_policy": "stop_and_report",
            "undo": "unsupported",
            "status": "active"
        }))
        .unwrap();
        assert!(!l3_task_matches(&no_pattern, "anything at all"));
    }

    #[test]
    fn l3_locally_runnable_accepts_policy_known_actions_only() {
        let write = probe_file_experience("active");
        assert!(l3_locally_runnable(&write));
        let exec: Experience = serde_json::from_value(serde_json::json!({
            "name": "exec_only",
            "trigger": {"tool": "exec_command", "command_pattern": "run probe"},
            "preconditions": [],
            "workflow": [{
                "action": "exec_command",
                "args": {"cmd": "Set-Content probe.txt OK"}
            }],
            "postconditions": [{"key": "file:probe.txt.exists", "expected": true}],
            "verification": [],
            "failure_policy": "stop_and_report",
            "undo": "unsupported",
            "status": "active"
        }))
        .unwrap();
        // S2: exec is an executor-known action; the *policy* is what decides
        // whether it may run (it stays off by default).
        assert!(l3_locally_runnable(&exec));
        let unknown: Experience = serde_json::from_value(serde_json::json!({
            "name": "unknown_only",
            "trigger": {"tool": "exec_command", "command_pattern": "run probe"},
            "preconditions": [],
            "workflow": [{"action": "mystery_tool", "args": {}}],
            "postconditions": [{"key": "file:probe.txt.exists", "expected": true}],
            "verification": [],
            "failure_policy": "stop_and_report",
            "undo": "unsupported",
            "status": "active"
        }))
        .unwrap();
        assert!(!l3_locally_runnable(&unknown));
    }

    #[test]
    fn l3_workflow_success_writes_and_verifies_postconditions() {
        let dir = l3_temp_dir("success");
        let experience = probe_file_experience("active");
        let report = l3_run_workflow(&experience, &dir, &CapabilityPolicy::default(), None, &dir);
        assert_eq!(report.outcome, "success");
        assert_eq!(report.completed_step_ids, vec!["write_file#0".to_string()]);
        assert!(report.verified_state.iter().any(|s| s == "file:probe.txt.exists=true"));
        assert_eq!(
            std::fs::read_to_string(dir.join("probe.txt")).unwrap(),
            "EXPERIENCE_GATE_SUCCESS"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn l3_workflow_misfires_when_postconditions_not_met() {
        let dir = l3_temp_dir("misfire");
        let mut experience = probe_file_experience("active");
        experience.postconditions[1].expected =
            serde_json::json!("DIFFERENT_EXPECTED_CONTENT");
        let report = l3_run_workflow(&experience, &dir, &CapabilityPolicy::default(), None, &dir);
        assert_eq!(report.outcome, "misfire");
        assert!(report.detail.contains("postconditions"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn l3_workflow_unsupported_action_is_invalid() {
        let dir = l3_temp_dir("invalid");
        let exec: Experience = serde_json::from_value(serde_json::json!({
            "name": "exec_only",
            "trigger": {"tool": "exec_command", "command_pattern": "run probe"},
            "preconditions": [],
            "workflow": [{
                "action": "exec_command",
                "args": {"cmd": "Set-Content probe.txt OK"}
            }],
            "postconditions": [{"key": "file:probe.txt.exists", "expected": true}],
            "verification": [],
            "failure_policy": "stop_and_report",
            "undo": "unsupported",
            "status": "active"
        }))
        .unwrap();
        let report = l3_run_workflow(&exec, &dir, &CapabilityPolicy::default(), None, &dir);
        assert_eq!(report.outcome, "invalid");
        assert!(
            report.detail.contains("policy denied step 'exec_command'"),
            "detail: {}",
            report.detail
        );
        assert!(report.detail.contains("exec=off"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn l3_workflow_denies_write_when_policy_disallows_writes() {
        let dir = l3_temp_dir("policy-write");
        let experience = probe_file_experience("active");
        let mut policy = CapabilityPolicy::default();
        policy.fs_write = experience_core::policy::FS_WRITE_DENY.to_string();
        let report = l3_run_workflow(&experience, &dir, &policy, None, &dir);
        assert_eq!(report.outcome, "invalid");
        assert!(report.detail.contains("fs_write=deny"), "detail: {}", report.detail);
        assert!(report.completed_step_ids.is_empty());
        // The refusal happens before any side effect.
        assert!(!dir.join("probe.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn l3_policy_blocks_reports_family_and_reason() {
        let mut delete: Experience = probe_file_experience("active");
        delete.workflow = vec![serde_json::from_value(serde_json::json!({
            "action": "delete_file",
            "args": {"path": "probe.txt"}
        }))
        .unwrap()];
        let reason = l3_policy_blocks(&CapabilityPolicy::default(), &delete).unwrap();
        assert!(reason.starts_with("policy_denied:delete_file:fs_delete"), "{reason}");
        assert!(reason.contains("fs_delete=deny"));

        let policy = CapabilityPolicy::default();
        assert!(!policy.allows("delete_file"));
        assert!(policy.allows("read_file"));
    }

    #[test]
    fn session_capability_must_fit_scope_policy() {
        let mut policy = CapabilityPolicy::default();
        assert!(capability_within_policy(&policy, "write_file"));
        assert!(capability_within_policy(&policy, "read_file"));
        assert!(!capability_within_policy(&policy, "exec_command"));
        assert!(!capability_within_policy(&policy, "delete_file"));
        // Reserved channels stay contract-only (allowed) for now.
        assert!(capability_within_policy(&policy, "computer_use"));

        // allowlist mode still refuses legacy shell unless explicitly opted in.
        policy.exec.mode = experience_core::policy::EXEC_ALLOWLIST.to_string();
        policy.exec.allow = vec!["git".to_string()];
        assert!(!capability_within_policy(&policy, "exec_command"));
        assert!(capability_within_policy(&policy, "exec"));
        policy.exec.allow_legacy_shell = true;
        assert!(capability_within_policy(&policy, "exec_command"));

        policy.fs_write = experience_core::policy::FS_WRITE_DENY.to_string();
        assert!(!capability_within_policy(&policy, "write_file"));
        assert!(capability_within_policy(&policy, "read_file"));
    }

    #[test]
    fn policy_summary_is_secret_free_and_stable() {
        let summary = policy_summary(&CapabilityPolicy::default());
        assert!(summary.contains("fs_write=workspace_only"));
        assert!(summary.contains("exec=off"));
        assert!(summary.contains(&format!(
            "max={}",
            experience_core::policy::DEFAULT_READ_MAX_BYTES
        )));
    }

    #[test]
    fn s3_gate_rejects_unobservable_verdicts() {
        let dir = l3_temp_dir("s3-unobservable");
        let mut experience = probe_file_experience("active");
        experience.postconditions = vec![serde_json::from_value(
            serde_json::json!({"key": "mood.is_happy", "expected": true}),
        )
        .unwrap()];
        let reason = l3_gate_check(&experience, &dir, &CapabilityPolicy::default())
            .expect("unregistered predicate must be refused");
        assert!(
            reason.starts_with("unobservable_verdict:"),
            "{reason}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn s3_gate_accepts_registered_predicates_and_does_not_touch_workspace() {
        let dir = l3_temp_dir("s3-observable");
        let experience = probe_file_experience("active");
        let reason = l3_gate_check(&experience, &dir, &CapabilityPolicy::default());
        assert!(reason.is_none(), "{reason:?}");
        // Gate 4: the dry-run replayed into scratch, never the workspace.
        assert!(!dir.join("probe.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn s3_gate_supports_new_predicate_families() {
        let dir = l3_temp_dir("s3-families");
        let mut experience = probe_file_experience("active");
        experience.postconditions = vec![
            serde_json::from_value(
                serde_json::json!({"key": "file:probe.txt.sha256", "expected": "abc"}),
            )
            .unwrap(),
            serde_json::from_value(
                serde_json::json!({"key": "file:probe.txt.size", "expected": 1}),
            )
            .unwrap(),
            serde_json::from_value(
                serde_json::json!({"key": "process.exit_code:exec#0", "expected": 0}),
            )
            .unwrap(),
        ];
        assert!(
            l3_gate_check(&experience, &dir, &CapabilityPolicy::default()).is_none()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn l3_pick_unique_best_conflicts_on_equal_coverage() {
        let first = probe_file_experience("active");
        let second = probe_file_experience("active");
        let runnable: Vec<&Experience> = vec![&first, &second];
        assert!(l3_pick_unique_best(&runnable).is_err());

        let mut longer = probe_file_experience("active");
        longer.name = "longer_probe".to_string();
        longer.workflow.push(serde_json::from_value(serde_json::json!({
            "action": "write_file",
            "args": {"path": "probe2.txt", "content": "SECOND"}
        }))
        .unwrap());
        longer.postconditions.push(serde_json::from_value(
            serde_json::json!({"key": "file:probe2.txt.exists", "expected": true}),
        ).unwrap());
        let mixed: Vec<&Experience> = vec![&first, &longer];
        assert_eq!(
            l3_pick_unique_best(&mixed).unwrap().name,
            "longer_probe"
        );
    }

    #[test]
    fn l3_cap_truncates_by_char_count() {
        let capped = l3_cap(&"好".repeat(500), 200);
        assert!(capped.chars().count() <= 201);
        assert!(capped.ends_with('…'));
    }

    #[test]
    fn outcome_mapping_covers_l3_execution_bands() {
        assert_eq!(
            outcome_to_experience_outcome("success"),
            Some(ExperienceOutcome::Success)
        );
        assert_eq!(
            outcome_to_experience_outcome("misfire"),
            Some(ExperienceOutcome::Misfire)
        );
        assert_eq!(
            outcome_to_experience_outcome("invalid"),
            Some(ExperienceOutcome::Invalid)
        );
        assert_eq!(
            outcome_to_experience_outcome("execution_error"),
            Some(ExperienceOutcome::ExecutionError)
        );
        assert_eq!(outcome_to_experience_outcome("delegated"), None);
        assert!(DECAY_SCORE_FLOOR > 0.0 && DECAY_SCORE_FLOOR < 0.5);
    }

    #[test]
    fn fs_resolver_qualifies_write_file_candidate_and_rejects_placeholder() {
        let mut candidate = probe_file_experience("candidate");
        assert!(qualify(&candidate, &resolve_default_source).is_ok());
        candidate.postconditions[0].key = "candidate.pending_validation".to_string();
        assert!(qualify(&candidate, &resolve_default_source).is_err());
    }

    #[test]
    fn injection_policy_defaults_off_and_accepts_explicit_on() {
        assert!(!injection_policy_from(None));
        assert!(!injection_policy_from(Some("off")));
        assert!(!injection_policy_from(Some("false")));
        assert!(injection_policy_from(Some("on")));
        assert!(injection_policy_from(Some("ENABLED")));
        assert!(injection_policy_from(Some("1")));
    }

    #[test]
    fn reference_text_uses_experience_body_not_trace() {
        let experience = probe_file_experience("active");
        let text = l3_reference_text(&[&experience], 800);
        assert!(text.contains("create_probe_file"));
        assert!(text.contains("write_file(path=probe.txt)"));
        assert!(text.contains("file:probe.txt.content"));
        assert!(!text.contains("toolCall"));
        let capped = l3_reference_text(&[&experience], 40);
        assert!(capped.chars().count() <= 41);
    }

    #[test]
    fn score_rounds_to_two_decimals_and_actor_defaults_to_user() {
        assert_eq!(round_score(0.659_999_999_999_999_9), 0.66);
        assert_eq!(round_score(0.5), 0.5);
        assert_eq!(request_actor(""), "user");
        assert_eq!(request_actor(r#"{"actor":"alice"}"#), "alice");
        assert_eq!(request_actor("not json"), "user");
    }

    #[test]
    fn llm_parse_extracts_fenced_json_and_rejects_garbage() {
        let text = "```json\n{\"name\":\"e1\",\"trigger\":{\"tool\":\"exec_command\",\"command_pattern\":\"probe\"},\"preconditions\":[],\"workflow\":[{\"action\":\"write_file\",\"args\":{\"path\":\"a.txt\",\"content\":\"x\"}}],\"postconditions\":[{\"key\":\"file:a.txt.exists\",\"expected\":true}],\"verification\":[],\"failure_policy\":\"stop_and_report\",\"undo\":\"unsupported\",\"status\":\"candidate\"}\n```";
        let experience = parse_llm_experience(text).expect("fenced json parses");
        assert_eq!(experience.name, "e1");
        assert_eq!(experience.workflow[0].action, "write_file");
        assert!(parse_llm_experience("no json here").is_none());
    }

    #[test]
    fn llm_prompt_is_success_path_summary_not_raw_trace() {
        let round = RoundTrace {
            task: "move png files".to_string(),
            events: vec![TraceEvent::ToolCall {
                name: "exec_command".to_string(),
                call_id: None,
                args_summary: Some("mv *.png pic/".to_string()),
            }],
        };
        let prompt = llm_distill_prompt(&round);
        assert!(prompt.contains("任务: move png files"));
        assert!(prompt.contains("mv *.png pic/"));
        assert!(!prompt.contains("reasoning"));
    }

    #[test]
    fn c1_tree_groups_scenes_and_families_and_respects_scope() {
        let mut store = ExperienceStore::default();
        let scoped = probe_file_experience("active");
        store.insert(scoped).unwrap();
        store.set_scope("create_probe_file", Some("scene-a")).unwrap();
        let mut global = probe_file_experience("active");
        global.name = "create_other_file".into();
        store.insert(global).unwrap();

        let tree = experience_tree(&store, Some("scene-a"), None);
        let text = serde_json::to_string(&tree).unwrap();
        assert!(text.contains("\"scene-a\""));
        assert!(text.contains("create_probe_file"));
        assert!(text.contains("create_other_file"), "unscoped stays visible");
        assert!(text.contains("\"family\""));

        let scoped_only = experience_tree(&store, Some("scene-b"), None);
        let scoped_text = serde_json::to_string(&scoped_only).unwrap();
        assert!(scoped_text.contains("create_other_file"));
        assert!(!scoped_text.contains("create_probe_file"));
    }

    #[test]
    fn c4_export_import_roundtrip_preserves_scope_and_identity() {
        let mut source = ExperienceStore::default();
        let a = probe_file_experience("active");
        source.insert(a).unwrap();
        source.set_scope("create_probe_file", Some("scene-a")).unwrap();
        let mut global = probe_file_experience("active");
        global.name = "create_other_file".into();
        source.insert(global).unwrap();
        let mut other_scope = probe_file_experience("active");
        other_scope.name = "create_third_file".into();
        source.insert(other_scope).unwrap();
        source.set_scope("create_third_file", Some("scene-b")).unwrap();

        let export = export_experiences(&source, Some("scene-a"));
        let text = serde_json::to_string(&export).unwrap();
        assert!(text.contains("create_probe_file"));
        assert!(text.contains("create_other_file"));
        assert!(!text.contains("create_third_file"));

        let mut target = ExperienceStore::default();
        let count = import_experiences(&mut target, &export, None).unwrap();
        assert_eq!(count, 2);
        assert_eq!(target.scope_of("create_probe_file"), Some("scene-a"));
        assert_eq!(target.scope_of("create_other_file"), None);
    }

    #[test]
    fn c4_state_snapshot_is_read_only_directory_listing() {
        let dir = l3_temp_dir("snapshot");
        std::fs::write(dir.join("note.txt"), "hello").unwrap();
        let snap = state_snapshot(&dir);
        assert_eq!(snap["exists"], true);
        assert_eq!(snap["read_only"], true);
        let entries = snap["entries"].as_array().unwrap();
        assert!(entries
            .iter()
            .any(|entry| entry["name"] == "note.txt" && entry["kind"] == "file"));
        assert!(!serde_json::to_string(&snap).unwrap().contains("hello"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn audit_query_newest_first_with_type_name_and_limit_filters() {
        let ledger = r#"{
          "schema_version": 1,
          "records": [
            { "record_type": "written", "candidate_name": "cand_a", "outcome": "written", "recorded_at": 1 },
            { "record_type": "delegate", "candidate_name": "create_probe_file", "outcome": "delegated", "recorded_at": 2 },
            { "record_type": "delegation_completed", "candidate_name": "create_probe_file", "outcome": "completed", "recorded_at": 3 },
            { "record_type": "adopted", "candidate_name": "create_probe_file", "outcome": "ok", "recorded_at": 4 }
          ]
        }"#;
        let all = audit_query(ledger, None, None, None);
        assert_eq!(all.len(), 4);
        assert_eq!(all[0]["record_type"], "adopted");
        assert_eq!(all[3]["record_type"], "written");
        let typed = audit_query(ledger, Some("delegate"), None, None);
        assert_eq!(typed.len(), 1);
        let named = audit_query(ledger, None, Some("create_probe_file"), None);
        assert_eq!(named.len(), 3);
        let limited = audit_query(ledger, None, Some("create_probe_file"), Some(2));
        assert_eq!(limited.len(), 2);
    }

    #[test]
    fn stage_a_guide_and_reference_text_carry_trust_metadata() {
        let guide = ingestion_guide("trae");
        assert_eq!(guide["agent"], "trae");
        assert!(guide["prompt_template"]
            .as_str()
            .unwrap()
            .contains("run-notes"));
        assert!(guide["checklist"].as_array().unwrap().len() >= 3);
        assert_eq!(trust_level_from(true), "workspace_verified");
        assert_eq!(trust_level_from(false), "declared");

        let entry = ReferenceEntry {
            id: "ref_trae_dark".into(),
            title: "Trae dark theme".into(),
            scope: Some("scene-frontend".into()),
            tags: vec!["frontend".into()],
            body: "notes".into(),
            steps: vec!["scaffold".into(), "verify".into()],
            tools_used: vec![],
            plugins_used: vec!["ui-kit".into()],
            source_agent: Some("trae".into()),
            declared: true,
            trust_level: "workspace_verified".into(),
            evidence_summary: Some("git diff".into()),
            created_at: 1,
        };
        let text = reference_entries_text(&[entry], 800);
        assert!(text.contains("[ref:ref_trae_dark][workspace_verified]"));
        assert!(text.contains("scaffold -> verify"));
    }

    #[test]
    fn run_notes_parser_extracts_sections_from_fixture() {
        let root = env!("CARGO_MANIFEST_DIR");
        let notes = std::fs::read_to_string(format!(
            "{root}/../../crates/experience-core/tests/fixtures/a/run-notes/run-notes.md"
        ))
        .expect("run-notes fixture");
        let draft = parse_run_notes(&notes);
        assert!(draft.task.contains("暗色主题"), "task={}", draft.task);
        assert!(draft.steps.len() >= 6, "steps={:?}", draft.steps);
        assert!(draft.plugins_used.iter().any(|item| item.contains("ui-kit")));
        assert!(draft.tools_used.iter().any(|item| item.contains("终端")));
        assert!(draft.artifacts.iter().any(|item| item.contains("theme.css")));
        assert!(draft
            .evidence_lines
            .iter()
            .any(|line| line.contains("git diff")));
        assert!(draft.sections.iter().any(|section| section.contains("步骤")));

        let fallback = parse_run_notes("# plain title\n- first step\n- second step");
        assert_eq!(fallback.task, "plain title");
        assert_eq!(fallback.steps, vec!["first step", "second step"]);
    }

    #[test]
    fn run_notes_parser_supports_tables_and_nested_headings() {
        let notes = "# t\n\n## 使用的工具与插件\n\n| 类别 | 名称 | 版本/说明 |\n|---|---|---|\n| 终端 | PowerShell | Windows |\n| 插件 | ui-kit | 2.1 |\n\n## 步骤与子步骤\n\n### D1 设计\n\n1. 建 base.css\n   - token\n2. 建 api.js\n\n## 最终产物与验证\n\n| 验证项 | 方法 | 结果 |\n|---|---|---|\n| diff | git diff --stat | 4 files |\n";
        let draft = parse_run_notes(notes);
        assert!(draft.tools_used.iter().any(|item| item.contains("PowerShell")));
        assert!(draft.plugins_used.iter().any(|item| item.contains("ui-kit")));
        assert!(draft.steps.len() >= 3, "steps={:?}", draft.steps);
        assert!(draft
            .artifacts
            .iter()
            .any(|item| item.contains("git diff")));
        assert!(draft
            .evidence_lines
            .iter()
            .any(|item| item.contains("git diff")));
    }

    #[test]
    fn promote_warnings_cover_cost_and_validation() {
        let warnings = promote_warnings();
        assert!(warnings.len() >= 3);
        assert!(warnings.iter().any(|text| text.contains("token")));
        assert!(warnings.iter().any(|text| text.contains("CANDIDATE")));
        assert!(warnings.iter().any(|text| text.contains("验证")));
        let fallback = promote_warnings_with(true);
        assert_eq!(fallback.len(), warnings.len() + 1);
        assert!(fallback.iter().any(|text| text.contains("自动发现")));
    }
}

async function api(path, options = {}) {
  const response = await fetch(path, {
    headers: { "Content-Type": "application/json" },
    ...options,
  });
  const data = await response.json().catch(() => ({}));
  if (!response.ok) {
    throw new Error(data.error || `HTTP ${response.status}`);
  }
  return data;
}

const $ = (id) => document.getElementById(id);

let experiences = [];
let usage = { entries: {} };
let agents = [];
let currentExperience = null;
let currentSessionId = null;
let pollTimer = null;
let busy = false;
let fbHistory = [];

/* ---------- shell ---------- */
function showView(name) {
  for (const key of ["sessions", "experiences", "agents", "audit"]) {
    $(`view-${key}`).classList.toggle("hidden", key !== name);
    $(`nav-${key}`).classList.toggle("active", key === name);
  }
  if (name === "experiences") loadExperiences().catch(alert);
  if (name === "agents") loadAgents().catch(showAgentError);
  if (name === "sessions") refreshAgentSelect().catch(alert);
  if (name === "audit") loadAudit().catch(alert);
}

async function refreshHealth() {
  try {
    await api("/api/health");
    $("health").className = "health-dot ok";
    $("health-text").textContent = "API ok";
  } catch (error) {
    $("health").className = "health-dot bad";
    $("health-text").textContent = `API 不可用: ${error.message}`;
  }
}

$("nav-sessions").onclick = () => showView("sessions");
$("nav-experiences").onclick = () => showView("experiences");
$("nav-agents").onclick = () => showView("agents");
$("nav-audit").onclick = () => showView("audit");

/* ---------- sessions ---------- */
async function refreshAgentSelect() {
  agents = await api("/api/agents");
  const select = $("agent-select");
  const previous = select.value;
  select.innerHTML = "";
  for (const entry of agents) {
    const option = document.createElement("option");
    option.value = entry.config.id;
    option.textContent = `${entry.config.label} (${entry.status.state})`;
    select.appendChild(option);
  }
  if (previous && agents.some((entry) => entry.config.id === previous)) {
    select.value = previous;
  }
  updateAgentState();
  await loadSessionHistory();
}

function updateAgentState() {
  const id = $("agent-select").value;
  const entry = agents.find((agent) => agent.config.id === id);
  $("agent-state").textContent = entry
    ? `${entry.status.state} — ${entry.status.message}`
    : "无 executor";
}

function addMessage(kind, text, extra = {}) {
  const messages = $("messages");
  const placeholder = messages.querySelector(".placeholder");
  if (placeholder) placeholder.remove();
  const div = document.createElement("div");
  div.className = `msg ${kind}`;
  div.textContent = text;
  if (extra.sessionId) div.dataset.sessionId = extra.sessionId;
  if (extra.details) {
    const details = document.createElement("details");
    const summary = document.createElement("summary");
    summary.textContent = "查看完整输出";
    const pre = document.createElement("pre");
    pre.textContent = extra.details;
    details.append(summary, pre);
    div.appendChild(details);
  }
  messages.appendChild(div);
  messages.scrollTop = messages.scrollHeight;
  return div;
}

async function loadSessionHistory() {
  const sessions = await api("/api/sessions");
  const list = $("session-list");
  list.innerHTML = "";
  for (const session of sessions.slice(0, 40)) {
    const li = document.createElement("li");
    if (session.id === currentSessionId) li.classList.add("active");
    const task = document.createElement("div");
    task.className = "session-task";
    task.textContent = session.task;
    const meta = document.createElement("div");
    meta.className = "session-meta";
    meta.textContent = `${session.status} · ${session.agent_id}${session.scope ? ` · scope=${session.scope}` : ""}`;
    li.append(task, meta);
    li.onclick = () => openSession(session.id);
    list.appendChild(li);
  }
}

let runningUi = null;

function stopPolling() {
  if (pollTimer) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
}

function showCancelButton() {
  $("cancel-session").classList.remove("hidden");
}

function hideCancelButton() {
  $("cancel-session").classList.add("hidden");
}

function updateResumeButton(session) {
  const hasThread = Boolean(session.thread_id);
  const finished = session.status !== "running";
  $("resume-session").classList.toggle("hidden", !(hasThread && finished));
}

function makeRunningUi(session) {
  const messages = $("messages");
  const placeholder = messages.querySelector(".placeholder");
  if (placeholder) placeholder.remove();
  const root = document.createElement("div");
  root.className = "msg assistant running";
  const label = document.createElement("span");
  const details = document.createElement("details");
  const summary = document.createElement("summary");
  summary.textContent = "实时输出";
  const pre = document.createElement("pre");
  details.append(summary, pre);
  root.append(label, details);
  messages.appendChild(root);
  messages.scrollTop = messages.scrollHeight;
  const ui = { root, label, pre };
  updateRunningUi(ui, session);
  return ui;
}

function updateRunningUi(ui, session) {
  const elapsed = Math.max(
    0,
    Math.floor(Date.now() / 1000 - session.created_at),
  );
  ui.label.textContent =
    `运行中 ${elapsed}s（executor 执行中；已输出 ${session.output.length} 字符）`;
  if (session.output) {
    ui.pre.textContent = session.output.slice(-6000);
  }
  if (session.trace && session.trace.length) {
    ui.pre.textContent +=
      "\n--- trace ---\n" +
      session.trace.slice(-60).map(traceLine).join("\n");
  }
  ui.root.scrollIntoView({ block: "end" });
}

function renderFinished(session) {
  if (runningUi) {
    runningUi.root.remove();
    runningUi = null;
  }
  hideCancelButton();
  updateResumeButton(session);
  busy = false;
  $("send-task").disabled = false;
  if (session.status === "error") {
    addMessage("assistant error", session.summary || "执行失败", {
      details: traceText(session),
    });
  } else {
    addMessage("assistant", session.summary || "（无摘要）", {
      details: traceText(session),
    });
  }
}

function traceText(session) {
  const parts = [];
  if (session.output) parts.push(session.output);
  if (session.trace && session.trace.length) {
    parts.push("--- trace ---\n" + session.trace.map(traceLine).join("\n"));
  }
  return parts.join("\n");
}

// Typed trace v1 events -> one-line rendering; legacy string traces pass
// through unchanged.
function traceLine(t) {
  if (typeof t === "string") return t;
  switch (t.kind) {
    case "submitted":
      return "submitted";
    case "accepted":
      return "accepted";
    case "agentStarted":
      return "agent_started";
    case "alreadyStarted":
      return "already_started";
    case "reasoning":
      return "reasoning";
    case "turnCompleted":
      return "turn_completed";
    case "toolCall":
      return "tool_call:" + t.name;
    case "toolResult":
      return "tool_result:" + t.name;
    case "failed":
      return "failed:" + t.phase;
    case "legacy":
      return t.label || "";
    default:
      return t.kind || "";
  }
}

async function openSession(id) {
  stopPolling();
  runningUi = null;
  currentSessionId = id;
  const session = await api(`/api/sessions/${id}`);
  $("messages").innerHTML = "";
  addMessage("user", session.task);
  if (session.status === "running") {
    runningUi = makeRunningUi(session);
    showCancelButton();
    updateResumeButton(session);
    pollSession(id);
  } else {
    renderFinished(session);
  }
  loadSessionHistory();
}

function pollSession(id) {
  pollTimer = setInterval(async () => {
    try {
      const session = await api(`/api/sessions/${id}`);
      if (session.status === "running") {
        if (runningUi) updateRunningUi(runningUi, session);
        return;
      }
      stopPolling();
      renderFinished(session);
      loadSessionHistory();
    } catch (error) {
      stopPolling();
      if (runningUi) {
        runningUi.root.remove();
        runningUi = null;
      }
      busy = false;
      $("send-task").disabled = false;
      hideCancelButton();
      addMessage("assistant error", `轮询失败：${error.message}`);
    }
  }, 1500);
}

$("new-session").onclick = () => {
  stopPolling();
  hideCancelButton();
  $("resume-session").classList.add("hidden");
  runningUi = null;
  busy = false;
  $("send-task").disabled = false;
  currentSessionId = null;
  $("messages").innerHTML =
    '<div class="placeholder">选择一个 executor，输入任务开始委派。</div>';
  $("task-input").value = "";
};

$("cancel-session").onclick = async () => {
  if (!currentSessionId) return;
  try {
    await api(`/api/sessions/${currentSessionId}/cancel`, { method: "POST" });
    stopPolling();
    const session = await api(`/api/sessions/${currentSessionId}`);
    renderFinished(session);
    loadSessionHistory();
  } catch (error) {
    addMessage("assistant error", `取消失败：${error.message}`);
  }
};

$("resume-session").onclick = async () => {
  if (!currentSessionId) return;
  const text = prompt("继续任务内容（将使用同一 thread 追加一轮）：");
  if (!text) return;
  try {
    await api(`/api/sessions/${currentSessionId}/resume`, {
      method: "POST",
      body: JSON.stringify({ task: text }),
    });
    openSession(currentSessionId);
  } catch (error) {
    addMessage("assistant error", `resume 失败：${error.message}`);
  }
};

$("composer").onsubmit = async (event) => {
  event.preventDefault();
  if (busy) return;
  const task = $("task-input").value.trim();
  if (!task) return;
  const agentId = $("agent-select").value;
  if (!agentId) {
    alert("请先在 Agents 页配置 executor");
    return;
  }
  busy = true;
  $("send-task").disabled = true;
  const cwd = $("cwd-input").value.trim() || null;
  const scope = $("scope-input").value.trim() || null;
  addMessage("user", task);
  $("task-input").value = "";
  try {
    const session = await api("/api/sessions", {
      method: "POST",
      body: JSON.stringify({ agent_id: agentId, task, cwd, scope }),
    });
    currentSessionId = session.id;
    runningUi = makeRunningUi(session);
    showCancelButton();
    pollSession(session.id);
    loadSessionHistory();
  } catch (error) {
    busy = false;
    $("send-task").disabled = false;
    addMessage("assistant error", error.message);
  }
};

$("agent-select").onchange = updateAgentState;

/* ---------- experiences ---------- */
async function loadExperiences() {
  experiences = await api("/api/experiences");
  usage = await api("/api/usage").catch(() => ({ entries: {} }));
  const list = $("experience-list");
  list.innerHTML = "";
  if (experiences.length === 0) {
    const empty = document.createElement("li");
    empty.textContent = "（暂无经验，可通过 POST /api/experiences 注入）";
    list.appendChild(empty);
    return;
  }
  for (const item of experiences) {
    const li = document.createElement("li");
    li.onclick = () => openExperience(item.name);
    const name = document.createElement("div");
    name.className = "exp-name";
    name.textContent = item.name;
    const meta = document.createElement("div");
    meta.className = "exp-meta";
    const usageEntry = usage.entries[item.name];
    const usageText = usageEntry
      ? ` · score=${usageEntry.score} · succ=${usageEntry.evidence.successes}`
      : "";
    meta.textContent =
      `${item.status}${item.pinned ? " · 📌pinned" : ""} · tool=${item.trigger_tool} · ` +
      `steps=${item.workflow_steps} · post=${item.postconditions}${usageText}`;
    li.append(name, meta);
    list.appendChild(li);
  }
}

async function openExperience(name) {
  currentExperience = name;
  const experience = await api(`/api/experiences/${encodeURIComponent(name)}`);
  const summaryItem = experiences.find((item) => item.name === name);
  const pinned = summaryItem ? summaryItem.pinned : false;
  $("detail-name").textContent = experience.name;
  $("detail-body").textContent = JSON.stringify(experience, null, 2);
  $("pin-exp").textContent = pinned ? "取消固定" : "固定";
  // L2 action map: activation is only offered from VALIDATED; CANDIDATE and
  // other states never get a blanket "set active" button (P2-1 closure).
  const canActivate = experience.status === "validated";
  $("set-active").style.display = canActivate ? "" : "none";
  $("set-active").textContent = "激活";
  $("list-panel").classList.add("hidden");
  $("detail-panel").classList.remove("hidden");
}

function backToExperiences() {
  currentExperience = null;
  $("detail-panel").classList.add("hidden");
  $("list-panel").classList.remove("hidden");
  loadExperiences();
}

async function setExperienceStatus(status) {
  if (!currentExperience) return;
  await api(`/api/experiences/${encodeURIComponent(currentExperience)}/status`, {
    method: "PATCH",
    body: JSON.stringify({ status }),
  });
  openExperience(currentExperience);
}

async function removeExperience() {
  if (!currentExperience) return;
  if (!confirm(`删除经验 ${currentExperience}？`)) return;
  await api(`/api/experiences/${encodeURIComponent(currentExperience)}`, {
    method: "DELETE",
  });
  backToExperiences();
}

async function togglePin() {
  if (!currentExperience) return;
  const summaryItem = experiences.find((item) => item.name === currentExperience);
  const pinned = summaryItem ? summaryItem.pinned : false;
  await api(
    `/api/experiences/${encodeURIComponent(currentExperience)}/${pinned ? "unpin" : "pin"}`,
    { method: "POST", body: JSON.stringify({ actor: "user" }) }
  );
  openExperience(currentExperience);
}

/* ---------- agents ---------- */
function statusClass(state) {
  switch (state) {
    case "ready": return "badge ready";
    case "error": return "badge error";
    case "checking": return "badge checking";
    case "disconnected": return "badge disconnected";
    default: return "badge";
  }
}

function showAgentError(error) {
  const box = $("agent-error");
  box.textContent = error.message || String(error);
  box.classList.remove("hidden");
}

async function loadAgents() {
  const items = await api("/api/agents");
  const list = $("agent-list");
  list.innerHTML = "";
  for (const entry of items) {
    const config = entry.config;
    const status = entry.status;
    const li = document.createElement("li");
    const top = document.createElement("div");
    top.className = "agent-top";
    const id = document.createElement("span");
    id.className = "agent-id";
    id.textContent = config.label;
    const kind = document.createElement("span");
    kind.className = "agent-kind";
    kind.textContent =
      `${config.kind} · ${config.mode} · channel=${config.channel || "exec"}` +
      ` · ${config.directory || ""} ${config.executable || ""}`;
    const badge = document.createElement("span");
    badge.className = statusClass(status.state);
    badge.textContent = status.state;
    top.append(id, kind, badge);
    const msg = document.createElement("div");
    msg.className = "agent-msg";
    msg.textContent = status.message;
    const actions = document.createElement("div");
    actions.className = "agent-actions";
    const connect = document.createElement("button");
    connect.textContent = "连接检查";
    connect.onclick = () => agentAction(config.id, "connect").catch(showAgentError);
    const disconnect = document.createElement("button");
    disconnect.textContent = "断开";
    disconnect.onclick = () =>
      agentAction(config.id, "disconnect").catch(showAgentError);
    const remove = document.createElement("button");
    remove.textContent = "删除";
    remove.className = "danger";
    remove.onclick = async () => {
      if (!confirm(`删除 agent ${config.id}？`)) return;
      await api(`/api/agents/${encodeURIComponent(config.id)}`, {
        method: "DELETE",
      });
      loadAgents().catch(showAgentError);
    };
    actions.append(connect, disconnect, remove);
    li.append(top, msg, actions);
    list.appendChild(li);
  }
}

async function agentAction(id, action) {
  $("agent-error").classList.add("hidden");
  await api(`/api/agents/${encodeURIComponent(id)}/${action}`, {
    method: "POST",
  });
  loadAgents().catch(showAgentError);
}

$("refresh-agents").onclick = () => loadAgents().catch(showAgentError);

/* local file browser (agent executable / shortcut picker) */
async function openFileModal() {
  $("file-modal").classList.remove("hidden");
  const roots = await api("/api/fs/roots");
  const box = $("fb-roots");
  box.innerHTML = "";
  for (const root of roots.roots) {
    const button = document.createElement("button");
    button.textContent = root;
    button.onclick = () => fbNav(root);
    box.appendChild(button);
  }
  fbNav(roots.roots[0] || "");
}

async function fbNav(path) {
  try {
    const data = await api(`/api/fs/browse?path=${encodeURIComponent(path)}`);
    fbHistory = [data.path];
    renderFb(data);
  } catch (error) {
    const list = $("fb-list");
    list.innerHTML = "";
    const li = document.createElement("li");
    li.textContent = `无法打开：${error.message}`;
    list.appendChild(li);
  }
}

function renderFb(data) {
  $("fb-path").value = data.path;
  const list = $("fb-list");
  list.innerHTML = "";
  for (const entry of data.entries) {
    const li = document.createElement("li");
    const label = document.createElement("span");
    label.textContent = entry.is_dir ? `📁 ${entry.name}` : `📄 ${entry.name}`;
    const kind = document.createElement("span");
    kind.className = "fb-kind";
    kind.textContent = entry.is_dir ? "目录" : "选择";
    li.append(label, kind);
    li.onclick = () => {
      if (entry.is_dir) {
        fbHistory.push(data.path);
        fbNav(entry.path);
      } else {
        chooseExecutable(entry.path, data.path);
      }
    };
    list.appendChild(li);
  }
}

function chooseExecutable(filePath, dirPath) {
  $("agent-form").elements["executable"].value = filePath;
  const dirInput = $("agent-form").elements["directory"];
  if (!dirInput.value.trim()) {
    const index = Math.max(filePath.lastIndexOf("\\"), filePath.lastIndexOf("/"));
    if (index > 0) dirInput.value = filePath.slice(0, index);
  }
  $("file-modal").classList.add("hidden");
}

$("browse-exec").onclick = () => openFileModal().catch(showAgentError);
$("fb-close").onclick = () => $("file-modal").classList.add("hidden");
$("fb-go").onclick = () => fbNav($("fb-path").value).catch(showAgentError);
$("fb-up").onclick = () => {
  const current = $("fb-path").value;
  const index = Math.max(current.lastIndexOf("\\"), current.lastIndexOf("/"));
  if (index > 0) fbNav(current.slice(0, index));
};

$("agent-form").onsubmit = async (event) => {
  event.preventDefault();
  const form = new FormData(event.target);
  try {
    $("agent-error").classList.add("hidden");
    const payload = {
      id: form.get("id"),
      label: form.get("label"),
      kind: form.get("kind"),
      mode: form.get("mode"),
      directory: form.get("directory") || null,
      executable: form.get("executable") || null,
      channel: form.get("channel") || "exec",
      codex_home: form.get("codex_home") || null,
    };
    await api("/api/agents", { method: "POST", body: JSON.stringify(payload) });
    event.target.reset();
    loadAgents().catch(showAgentError);
  } catch (error) {
    showAgentError(error);
  }
};

/* ---------- boot ---------- */
refreshHealth();
refreshAgentSelect().catch(alert);

/* ---------- experiences tree / detail / edit (U2-U6) ---------- */

function esc(text) {
  return String(text ?? "").replace(/[&<>"']/g, (ch) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
  }[ch]));
}

function actorName() {
  return localStorage.getItem("exp-actor") || "user";
}

function familyOf(name) {
  const stripped = name.startsWith("cand_") ? name.slice(5) : name;
  const at = stripped.indexOf("_");
  return at > 0 ? stripped.slice(0, at) : stripped;
}

function usageCountOf(name) {
  const entry = usage.entries[name];
  return entry && entry.activity ? entry.activity.usage_count : 0;
}

async function refreshScopeOptions() {
  const scenes = new Set();
  for (const item of experiences) {
    if (item.scope) scenes.add(item.scope);
  }
  const datalist = $("scope-options");
  datalist.innerHTML = "";
  for (const scene of scenes) {
    const option = document.createElement("option");
    option.value = scene;
    datalist.appendChild(option);
  }
}

async function loadExperiences() {
  [experiences, usage] = await Promise.all([
    api("/api/experiences"),
    api("/api/usage"),
  ]);
  refreshScopeOptions();
  renderExperienceTree();
}

function filteredExperiences() {
  const status = $("status-filter").value;
  const scope = $("scope-filter").value.trim();
  const usageMin = Number($("usage-min").value || 0);
  const query = $("tree-search").value.trim().toLowerCase();
  return experiences.filter((item) => {
    if (status && item.status !== status) return false;
    if (scope && !(item.scope === scope || !item.scope)) return false;
    if (usageMin > 0 && usageCountOf(item.name) < usageMin) return false;
    if (query) {
      const haystack = `${item.name} ${item.display_name || ""}`.toLowerCase();
      if (!haystack.includes(query)) return false;
    }
    return true;
  });
}

function renderExperienceTree() {
  const root = $("experience-tree");
  root.innerHTML = "";
  const list = filteredExperiences();
  if (list.length === 0) {
    const empty = document.createElement("li");
    empty.textContent = "（无匹配经验）";
    root.appendChild(empty);
    return;
  }
  const scenes = new Map();
  for (const item of list) {
    const scene = item.scope || "(未分类)";
    if (!scenes.has(scene)) scenes.set(scene, new Map());
    const families = scenes.get(scene);
    const family = familyOf(item.name);
    if (!families.has(family)) families.set(family, []);
    families.get(family).push(item);
  }
  for (const [scene, families] of scenes) {
    const sceneLi = document.createElement("li");
    const sceneHead = document.createElement("div");
    sceneHead.className = "tree-scene";
    sceneHead.textContent = `${scene}（${[...families.values()].flat().length}）`;
    const sceneUl = document.createElement("ul");
    for (const [family, items] of families) {
      const famLi = document.createElement("li");
      const famHead = document.createElement("div");
      famHead.className = "tree-family";
      famHead.textContent = `${family}（${items.length}）`;
      const famUl = document.createElement("ul");
      for (const item of items) {
        const li = document.createElement("li");
        li.className = "tree-leaf";
        const name = document.createElement("span");
        name.textContent = item.display_name || item.name;
        name.title = `内部名: ${item.name}`;
        const badge = document.createElement("span");
        badge.className = "badge small";
        badge.textContent = `${item.status}${item.pinned ? " 📌" : ""}${item.user_usage === "deny" ? " ⛔" : ""}`;
        li.append(name, badge);
        li.onclick = () => openExperience(item.name).catch(alert);
        famUl.appendChild(li);
      }
      famLi.append(famHead, famUl);
      sceneUl.appendChild(famLi);
    }
    sceneLi.append(sceneHead, sceneUl);
    root.appendChild(sceneLi);
  }
}

async function openExperience(name) {
  currentExperience = name;
  const [detail, audit] = await Promise.all([
    api(`/api/experiences/${encodeURIComponent(name)}`),
    api(`/api/audit?name=${encodeURIComponent(name)}&limit=15`),
  ]);
  const summary = experiences.find((item) => item.name === name) || {};
  $("detail-name").textContent = summary.display_name || detail.name;
  $("detail-badges").innerHTML = [
    detail.status,
    summary.pinned ? "📌pinned" : null,
    detail.scope ? `scope=${detail.scope}` : "无 scope",
    detail.user_usage === "deny" ? "用户停用" : null,
  ]
    .filter(Boolean)
    .map((text) => `<span class="badge">${esc(text)}</span>`)
    .join("");
  renderMetaForms(detail, summary);
  renderActions(detail, summary);
  $("detail-body").textContent = JSON.stringify(detail, null, 2);
  $("detail-body").classList.remove("hidden");
  renderUsagePanel(detail);
  renderAuditList($("audit-list"), audit.records);
  $("editor-root").classList.add("hidden");
  $("list-panel").classList.add("hidden");
  $("detail-panel").classList.remove("hidden");
}

function renderMetaForms(detail) {
  const meta = $("detail-meta");
  meta.innerHTML = "";
  const alias = document.createElement("input");
  alias.value = detail.display_name || "";
  alias.placeholder = "别名（显示用）";
  const aliasBtn = button("设置别名", async () => {
    await api(`/api/experiences/${encodeURIComponent(currentExperience)}/display_name`, {
      method: "POST",
      body: JSON.stringify({ display_name: alias.value || null, actor: actorName() }),
    });
    openExperience(currentExperience);
  });
  const scope = document.createElement("input");
  scope.value = detail.scope || "";
  scope.placeholder = "scope（留空=无 scope）";
  const scopeBtn = button("设置场景", async () => {
    await api(`/api/experiences/${encodeURIComponent(currentExperience)}/scope`, {
      method: "POST",
      body: JSON.stringify({ scope: scope.value || null, actor: actorName() }),
    });
    openExperience(currentExperience);
  });
  const usageSel = document.createElement("select");
  for (const value of ["auto", "allow", "deny"]) {
    const option = document.createElement("option");
    option.value = value;
    option.textContent = value;
    option.selected = (detail.user_usage || "auto") === value;
    usageSel.appendChild(option);
  }
  const confidence = document.createElement("input");
  confidence.type = "number";
  confidence.min = "0";
  confidence.max = "1";
  confidence.step = "0.01";
  confidence.value = detail.user_confidence ?? "";
  confidence.placeholder = "用户评分(0..1)";
  const reason = document.createElement("input");
  reason.placeholder = "reason";
  const prefBtn = button("保存偏好", async () => {
    const confidenceValue = confidence.value === "" ? null : Number(confidence.value);
    await api(`/api/experiences/${encodeURIComponent(currentExperience)}/user-preference`, {
      method: "POST",
      body: JSON.stringify({
        usage: usageSel.value,
        confidence: confidenceValue,
        reason: reason.value,
        actor: actorName(),
      }),
    });
    openExperience(currentExperience);
  });
  meta.append(
    fieldLabel("别名", alias, aliasBtn),
    fieldLabel("场景", scope, scopeBtn),
    fieldLabel("偏好", usageSel, confidence, reason, prefBtn),
  );
}

function fieldLabel(label, ...controls) {
  const wrap = document.createElement("div");
  wrap.className = "meta-field";
  const title = document.createElement("label");
  title.textContent = label;
  wrap.append(title, ...controls);
  return wrap;
}

function button(text, onclick, danger = false) {
  const btn = document.createElement("button");
  btn.textContent = text;
  if (danger) btn.className = "danger";
  btn.onclick = onclick;
  return btn;
}

function renderActions(detail, summary = {}) {
  const box = $("exp-actions");
  box.innerHTML = "";
  const statusButtons = [];
  if (detail.status === "validated") {
    statusButtons.push(button("激活", () => setExperienceStatus("active")));
  }
  if (detail.status === "candidate") {
    statusButtons.push(button("验证", () => setExperienceStatus("validate")));
  }
  if (detail.status === "active") {
    statusButtons.push(button("停用", () => setExperienceStatus("disabled")));
    statusButtons.push(button("草稿化", startDraft));
  }
  if (detail.status === "disabled" || detail.status === "decaying") {
    statusButtons.push(button("重新验证", () => setExperienceStatus("revalidate")));
  }
  if (detail.status === "draft") {
    statusButtons.push(button("编辑体", openEditor));
    statusButtons.push(button("采纳", adoptDraft));
    statusButtons.push(button("放弃草稿", abandonDraft, true));
  }
  if (detail.status !== "draft") {
    statusButtons.push(button(summary.pinned ? "取消固定" : "固定", togglePin));
  }
  statusButtons.push(button("删除", removeExperience, true));
  statusButtons.forEach((btn) => box.appendChild(btn));
}

async function setExperienceStatus(status) {
  const reason = prompt(`状态操作: ${status}（actor=${actorName()}，如需 reason 请填写）`) || "";
  await api(`/api/experiences/${encodeURIComponent(currentExperience)}/status`, {
    method: "PATCH",
    body: JSON.stringify({ status, reason, actor: actorName() }),
  });
  openExperience(currentExperience);
}

async function togglePin() {
  const summary = experiences.find((item) => item.name === currentExperience) || {};
  const pinned = Boolean(summary.pinned);
  await api(
    `/api/experiences/${encodeURIComponent(currentExperience)}/${pinned ? "unpin" : "pin"}`,
    { method: "POST", body: JSON.stringify({ actor: actorName() }) },
  );
  openExperience(currentExperience);
}

async function removeExperience() {
  if (!confirm(`删除经验 ${currentExperience}？`)) return;
  await api(`/api/experiences/${encodeURIComponent(currentExperience)}`, { method: "DELETE" });
  backToExperiences();
}

async function startDraft() {
  await api(`/api/experiences/${encodeURIComponent(currentExperience)}/draft`, {
    method: "POST",
    body: JSON.stringify({ actor: actorName() }),
  });
  openExperience(`${currentExperience}__draft`);
}

function backToExperiences() {
  currentExperience = null;
  $("detail-panel").classList.add("hidden");
  $("list-panel").classList.remove("hidden");
  loadExperiences().catch(alert);
}

function renderUsagePanel(detail) {
  const entry = usage.entries[detail.name];
  const box = $("exp-usage");
  if (!entry) {
    box.textContent = "暂无 evidence/usage 记录";
    return;
  }
  box.innerHTML = "";
  const evidence = entry.evidence || {};
  const lines = [
    `evidence score=${entry.score}（successes=${evidence.successes} misfires=${evidence.misfires} invalid=${evidence.invalid} execution_errors=${evidence.execution_errors || 0}）`,
    `activity usage=${entry.activity ? entry.activity.usage_count : 0}`,
    detail.user_confidence != null ? `user score=${detail.user_confidence}（仅用户偏好，不改证据）` : "user score=neutral",
  ];
  const ul = document.createElement("ul");
  lines.forEach((line) => {
    const li = document.createElement("li");
    li.textContent = line;
    ul.appendChild(li);
  });
  box.appendChild(ul);
}

function renderAuditList(target, records) {
  target.innerHTML = "";
  if (!records || records.length === 0) {
    const li = document.createElement("li");
    li.textContent = "（无审计记录）";
    target.appendChild(li);
    return;
  }
  for (const record of records) {
    const li = document.createElement("li");
    const time = new Date((record.recorded_at || 0) * 1000).toLocaleString();
    li.textContent = `[${time}] ${record.record_type} · ${record.candidate_name} · ${record.outcome || ""}${record.reason ? ` · ${record.reason}` : ""}`;
    target.appendChild(li);
  }
}

/* ---------- draft body editor (U4) ---------- */

async function openEditor() {
  const editor = $("editor-root");
  editor.innerHTML = "";
  const detail = await api(`/api/experiences/${encodeURIComponent(currentExperience)}`);
  const h = document.createElement("h3");
  h.textContent = `编辑 Draft：${detail.name}`;
  const form = document.createElement("div");
  form.innerHTML = `
    <label>trigger.tool
      <select id="edit-tool">
        <option>exec_command</option><option>write_file</option><option>read_file</option>
      </select>
    </label>
    <label>command_pattern<input id="edit-pattern" /></label>
    <div><b>preconditions</b>（每行 key = JSON expected）<textarea id="edit-pre" rows="2"></textarea></div>
    <div><b>postconditions</b><textarea id="edit-post" rows="2"></textarea></div>
    <div><b>workflow</b>（每行 JSON: {"action":"write_file","args":{...}}）<textarea id="edit-workflow" rows="6"></textarea></div>
    <label>failure_policy<select id="edit-failure"><option>stop_and_report</option></select></label>
    <label>undo<select id="edit-undo"><option>unsupported</option></select></label>
  `;
  const field = (id) => form.querySelector(`#${id}`);
  field("edit-tool").value = detail.trigger.tool;
  field("edit-pattern").value = detail.trigger.command_pattern || "";
  field("edit-pre").value = (detail.preconditions || []).map((p) => `${p.key} = ${JSON.stringify(p.expected)}`).join("\n");
  field("edit-post").value = (detail.postconditions || []).map((p) => `${p.key} = ${JSON.stringify(p.expected)}`).join("\n");
  field("edit-workflow").value = (detail.workflow || []).map((step) => JSON.stringify(step)).join("\n");
  editor.append(
    h,
    form,
    button("保存（PUT body）", saveDraftBody),
  );
  editor.classList.remove("hidden");
  $("detail-body").classList.add("hidden");
}

function parseKeyValueLines(text) {
  return text
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => {
      const at = line.indexOf("=");
      if (at <= 0) throw new Error(`行缺少 '=': ${line}`);
      const key = line.slice(0, at).trim();
      const expected = JSON.parse(line.slice(at + 1).trim());
      return { key, expected };
    });
}

async function saveDraftBody() {
  const detail = await api(`/api/experiences/${encodeURIComponent(currentExperience)}`);
  const workflow = $("edit-workflow").value
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => JSON.parse(line));
  const body = {
    name: currentExperience,
    trigger: {
      tool: $("edit-tool").value,
      command_pattern: $("edit-pattern").value || null,
    },
    preconditions: parseKeyValueLines($("edit-pre").value),
    workflow,
    postconditions: parseKeyValueLines($("edit-post").value),
    verification: detail.verification || [],
    failure_policy: $("edit-failure").value,
    undo: $("edit-undo").value,
    status: "draft",
  };
  await api(`/api/experiences/${encodeURIComponent(currentExperience)}/body`, {
    method: "PUT",
    body: JSON.stringify(body),
  });
  openExperience(currentExperience);
}

async function adoptDraft() {
  const reason = prompt("adopt reason（actor 自动记录）") || "";
  await api(`/api/experiences/${encodeURIComponent(currentExperience)}/adopt`, {
    method: "POST",
    body: JSON.stringify({ reason, actor: actorName() }),
  });
  backToExperiences();
}

async function abandonDraft() {
  if (!confirm(`放弃草稿 ${currentExperience}？`)) return;
  await api(`/api/experiences/${encodeURIComponent(currentExperience)}`, { method: "DELETE" });
  backToExperiences();
}

/* ---------- audit view (U1) ---------- */

async function loadAudit() {
  const name = $("audit-name").value.trim();
  const type = $("audit-type").value.trim();
  const limit = Number($("audit-limit").value || 100);
  const params = new URLSearchParams();
  if (name) params.set("name", name);
  if (type) params.set("record_type", type);
  params.set("limit", String(limit));
  const data = await api(`/api/audit?${params.toString()}`);
  renderAuditList($("audit-records"), data.records);
}

$("refresh").onclick = () => loadExperiences().catch(alert);
$("back").onclick = backToExperiences;
$("tree-search").oninput = renderExperienceTree;
$("scope-filter").oninput = renderExperienceTree;
$("status-filter").onchange = renderExperienceTree;
$("usage-min").oninput = renderExperienceTree;
$("refresh-audit").onclick = () => loadAudit().catch(alert);
$("audit-name").onchange = () => loadAudit().catch(alert);
$("audit-type").onchange = () => loadAudit().catch(alert);
$("audit-limit").onchange = () => loadAudit().catch(alert);

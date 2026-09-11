/* ============================================================
 * agents.js — 助手管理页面业务逻辑
 * 依赖：api.js (api, $, esc, toast), layout.js
 * ============================================================ */

/* ---------- 状态映射 ---------- */

const STATE_LABELS = {
  ready: "就绪",
  error: "错误",
  checking: "检查中",
  disconnected: "离线",
};

function statusClass(state) {
  switch (state) {
    case "ready":
      return "badge badge-success";
    case "error":
      return "badge badge-danger";
    case "checking":
      return "badge badge-warning";
    case "disconnected":
      return "badge badge-default";
    default:
      return "badge badge-default";
  }
}

function stateLabel(state) {
  return STATE_LABELS[state] || state;
}

/* ---------- 错误提示 ---------- */

function showAgentError(error) {
  const box = $("agent-error");
  box.textContent = error.message || String(error);
  box.classList.remove("hidden");
}

function hideAgentError() {
  $("agent-error").classList.add("hidden");
}

/* ---------- 助手列表 ---------- */

let editingAgentId = null;

async function loadAgents() {
  hideAgentError();
  const items = await api("/api/agents");
  const list = $("agent-list");
  const empty = $("agent-empty");

  list.innerHTML = "";

  if (items.length === 0) {
    empty.classList.remove("hidden");
    return;
  }
  empty.classList.add("hidden");

  for (const entry of items) {
    const config = entry.config;
    const status = entry.status;
    const li = document.createElement("li");
    li.className = "agent-card";

    // 顶部行
    const top = document.createElement("div");
    top.className = "agent-card-top";

    const name = document.createElement("span");
    name.className = "agent-name";
    name.textContent = config.label || config.id;

    const typeBadge = document.createElement("span");
    typeBadge.className = "agent-type-badge";
    typeBadge.textContent = `${config.kind || "codex_cli"} · ${config.mode || "managed"}`;

    const statusWrap = document.createElement("span");
    statusWrap.className = "agent-status";
    const statusBadge = document.createElement("span");
    statusBadge.className = statusClass(status.state);
    statusBadge.textContent = stateLabel(status.state);
    statusWrap.appendChild(statusBadge);

    top.append(name, typeBadge, statusWrap);

    // 状态消息
    const msg = document.createElement("div");
    msg.className = "agent-msg";
    msg.textContent = status.message || "—";

    // 元信息
    const meta = document.createElement("div");
    meta.className = "agent-meta";
    const channel = config.channel || "exec";
    const channelLabel = channel === "session" ? "持续会话模式" : "单次运行模式";
    const dirText = config.directory ? ` · ${config.directory}` : "";
    const execText = config.executable ? ` · ${config.executable}` : "";
    meta.textContent = `ID: ${config.id} · ${channelLabel}${dirText}${execText}`;

    // 操作按钮
    const actions = document.createElement("div");
    actions.className = "agent-actions";

    const connectBtn = document.createElement("button");
    connectBtn.className = "btn btn-sm";
    connectBtn.textContent = "连接检查";
    connectBtn.onclick = () =>
      agentAction(config.id, "connect").catch(showAgentError);

    const disconnectBtn = document.createElement("button");
    disconnectBtn.className = "btn btn-sm";
    disconnectBtn.textContent = "断开";
    disconnectBtn.onclick = () =>
      agentAction(config.id, "disconnect").catch(showAgentError);

    const editBtn = document.createElement("button");
    editBtn.className = "btn btn-sm";
    editBtn.textContent = "编辑";
    editBtn.onclick = () => editAgent(entry);

    const deleteBtn = document.createElement("button");
    deleteBtn.className = "btn btn-sm btn-danger";
    deleteBtn.textContent = "删除";
    deleteBtn.onclick = async () => {
      if (!confirm(`删除助手 "${config.label || config.id}"？`)) return;
      try {
        await api(`/api/agents/${encodeURIComponent(config.id)}`, {
          method: "DELETE",
        });
        toast("助手已删除", "success");
        loadAgents().catch(showAgentError);
      } catch (error) {
        showAgentError(error);
      }
    };

    actions.append(connectBtn, disconnectBtn, editBtn, deleteBtn);

    li.append(top, msg, meta, actions);
    list.appendChild(li);
  }
}

/* ---------- 助手操作 ---------- */

async function agentAction(id, action) {
  hideAgentError();
  await api(`/api/agents/${encodeURIComponent(id)}/${action}`, {
    method: "POST",
  });
  loadAgents().catch(showAgentError);
}

/* ---------- 编辑助手 ---------- */

function editAgent(entry) {
  const config = entry.config;
  editingAgentId = config.id;

  $("form-title").textContent = "编辑助手";
  $("agent-id").value = config.id || "";
  $("agent-label").value = config.label || "";
  $("agent-directory").value = config.directory || "";
  $("agent-executable").value = config.executable || "";
  $("agent-codex-home").value = config.codex_home || "";

  // 运行模式
  const channel = config.channel || "exec";
  const radios = document.querySelectorAll('input[name="channel"]');
  radios.forEach((radio) => {
    radio.checked = radio.value === channel;
  });

  $("cancel-edit").classList.remove("hidden");
  $("save-agent").textContent = "保存修改";

  // 滚动到表单
  $("agent-form").scrollIntoView({ behavior: "smooth", block: "start" });
}

function resetForm() {
  editingAgentId = null;
  $("agent-form").reset();
  $("form-title").textContent = "添加助手";
  $("cancel-edit").classList.add("hidden");
  $("save-agent").textContent = "保存助手";
}

/* ---------- 文件浏览器 ---------- */

let fbHistory = [];

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
    li.ondblclick = () => {
      if (entry.is_dir) {
        fbHistory.push(data.path);
        fbNav(entry.path);
      }
    };
    li.onclick = () => {
      if (!entry.is_dir) {
        chooseExecutable(entry.path, data.path);
      }
    };
    list.appendChild(li);
  }
}

function chooseExecutable(filePath, dirPath) {
  $("agent-executable").value = filePath;
  const dirInput = $("agent-directory");
  if (!dirInput.value.trim()) {
    const index = Math.max(filePath.lastIndexOf("\\"), filePath.lastIndexOf("/"));
    if (index > 0) dirInput.value = filePath.slice(0, index);
  }
  $("file-modal").classList.add("hidden");
}

/* ---------- 表单提交 ---------- */

$("agent-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  hideAgentError();

  const form = event.target;
  const formData = new FormData(form);

  const channelRadio = document.querySelector('input[name="channel"]:checked');
  const channel = channelRadio ? channelRadio.value : "exec";

  const payload = {
    id: formData.get("id"),
    label: formData.get("label"),
    kind: formData.get("kind"),
    mode: formData.get("mode"),
    directory: formData.get("directory") || null,
    executable: formData.get("executable") || null,
    channel: channel,
    codex_home: formData.get("codex_home") || null,
  };

  try {
    await api("/api/agents", {
      method: "POST",
      body: JSON.stringify(payload),
    });
    toast(editingAgentId ? "助手已更新" : "助手已添加", "success");
    resetForm();
    loadAgents().catch(showAgentError);
  } catch (error) {
    showAgentError(error);
  }
});

/* ---------- 事件绑定 ---------- */

$("refresh-agents").addEventListener("click", () => {
  loadAgents().catch(showAgentError);
});

$("browse-exec").addEventListener("click", () => {
  openFileModal().catch(showAgentError);
});

$("fb-close").addEventListener("click", () => {
  $("file-modal").classList.add("hidden");
});

$("fb-go").addEventListener("click", () => {
  fbNav($("fb-path").value).catch(showAgentError);
});

$("fb-up").addEventListener("click", () => {
  const current = $("fb-path").value;
  const index = Math.max(current.lastIndexOf("\\"), current.lastIndexOf("/"));
  if (index > 0) {
    fbNav(current.slice(0, index)).catch(showAgentError);
  }
});

$("fb-path").addEventListener("keydown", (e) => {
  if (e.key === "Enter") {
    fbNav(e.target.value).catch(showAgentError);
  }
});

$("cancel-edit").addEventListener("click", resetForm);

// 点击弹窗外部关闭
$("file-modal").addEventListener("click", (e) => {
  if (e.target.id === "file-modal") {
    $("file-modal").classList.add("hidden");
  }
});

/* ---------- 页面初始化 ---------- */

loadAgents().catch(showAgentError);

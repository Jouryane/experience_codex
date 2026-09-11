/* ============================================================
 * tasks.js — 任务首页业务逻辑
 * 依赖：api.js（api, $, esc, toast）、layout.js（initLayout）
 * 术语：session → task（任务），agent/executor → 助手，
 *       scope → 场景，composer → 任务输入区
 * ============================================================ */

/* ---------- 状态变量 ---------- */

let agents = [];
let currentTaskId = null;
let pollTimer = null;
let runningUi = null;
let busy = false;

/* ---------- 助手列表 ---------- */

/**
 * 加载助手列表，填充下拉框
 */
async function refreshAgentSelect() {
  agents = await api("/api/agents");
  const select = $("agent-select");
  const previous = select.value;
  select.innerHTML = "";

  if (agents.length === 0) {
    const option = document.createElement("option");
    option.value = "";
    option.textContent = "暂无可用助手";
    option.disabled = true;
    select.appendChild(option);
  } else {
    for (const entry of agents) {
      const option = document.createElement("option");
      option.value = entry.config.id;
      option.textContent = `${entry.config.label}（${stateLabel(entry.status.state)}）`;
      select.appendChild(option);
    }
  }

  if (previous && agents.some((entry) => entry.config.id === previous)) {
    select.value = previous;
  }
  updateAgentState();
  await loadTaskHistory();
}

/**
 * 更新助手状态显示文字
 */
function updateAgentState() {
  const id = $("agent-select").value;
  const entry = agents.find((agent) => agent.config.id === id);
  const statusEl = $("agent-state-text");
  if (!statusEl) return;

  if (entry) {
    statusEl.textContent = `${stateLabel(entry.status.state)} — ${entry.status.message}`;
  } else {
    statusEl.textContent = "无可用助手";
  }
}

/**
 * 状态码 → 中文标签
 */
function stateLabel(state) {
  const map = {
    ready: "就绪",
    running: "运行中",
    error: "错误",
    checking: "检查中",
    disconnected: "已断开",
  };
  return map[state] || state;
}

/* ---------- 历史任务列表 ---------- */

/**
 * 加载历史任务列表
 */
async function loadTaskHistory() {
  const tasks = await api("/api/sessions");
  const list = $("task-list");
  list.innerHTML = "";

  if (tasks.length === 0) {
    const empty = document.createElement("div");
    empty.className = "task-list-empty";
    empty.textContent = "还没有任务，输入内容开始第一个任务";
    list.appendChild(empty);
    return;
  }

  for (const task of tasks.slice(0, 50)) {
    const li = document.createElement("li");
    li.className = "task-item";
    if (task.id === currentTaskId) li.classList.add("active");

    const title = document.createElement("div");
    title.className = "task-item-title";
    title.textContent = truncate(task.task, 30);

    const meta = document.createElement("div");
    meta.className = "task-item-meta";

    const dot = document.createElement("span");
    dot.className = `status-dot ${task.status}`;
    dot.title = statusLabel(task.status);

    const statusText = document.createElement("span");
    statusText.textContent = statusLabel(task.status);

    const timeText = document.createElement("span");
    timeText.textContent = formatTime(task.created_at);

    meta.append(dot, statusText, timeText);
    li.append(title, meta);

    li.onclick = () => openTask(task.id);
    list.appendChild(li);
  }
}

/**
 * 任务状态 → 中文标签
 */
function statusLabel(status) {
  const map = {
    running: "运行中",
    finished: "已完成",
    error: "失败",
    pending: "等待中",
  };
  return map[status] || status;
}

/**
 * 时间戳 → 友好时间显示
 */
function formatTime(timestamp) {
  if (!timestamp) return "";
  const date = new Date(timestamp * 1000);
  const now = new Date();
  const diffMs = now - date;
  const diffMin = Math.floor(diffMs / 60000);
  const diffHour = Math.floor(diffMs / 3600000);
  const diffDay = Math.floor(diffMs / 86400000);

  if (diffMin < 1) return "刚刚";
  if (diffMin < 60) return `${diffMin} 分钟前`;
  if (diffHour < 24) return `${diffHour} 小时前`;
  if (diffDay < 7) return `${diffDay} 天前`;
  return date.toLocaleDateString("zh-CN");
}

/**
 * 截断字符串
 */
function truncate(text, maxLen) {
  if (!text) return "";
  if (text.length <= maxLen) return text;
  return text.slice(0, maxLen) + "…";
}

/* ---------- 消息渲染 ---------- */

/**
 * 添加消息气泡
 * @param {string} kind - user / assistant / error
 * @param {string} text - 消息文本
 * @param {Object} [extra={}] - 额外参数（details, taskId 等）
 */
function addMessage(kind, text, extra = {}) {
  const messages = $("messages-area");
  const placeholder = messages.querySelector(".messages-empty");
  if (placeholder) placeholder.remove();

  const div = document.createElement("div");
  div.className = `msg ${kind}`;
  div.textContent = text;

  if (extra.taskId) div.dataset.taskId = extra.taskId;

  if (extra.details) {
    const details = document.createElement("details");
    const summary = document.createElement("summary");
    summary.textContent = "查看详细输出";
    const pre = document.createElement("pre");
    pre.textContent = extra.details;
    details.append(summary, pre);
    div.appendChild(details);
  }

  messages.appendChild(div);
  messages.scrollTop = messages.scrollHeight;
  return div;
}

/* ---------- 运行中状态 UI ---------- */

/**
 * 创建运行中状态 UI
 */
function makeRunningUi(task) {
  const messages = $("messages-area");
  const placeholder = messages.querySelector(".messages-empty");
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
  updateRunningUi(ui, task);
  return ui;
}

/**
 * 更新运行中状态 UI
 */
function updateRunningUi(ui, task) {
  const elapsed = Math.max(
    0,
    Math.floor(Date.now() / 1000 - task.created_at),
  );
  ui.label.textContent =
    `运行中 ${elapsed}s（助手执行中；已输出 ${task.output.length} 字符）`;

  if (task.output) {
    ui.pre.textContent = task.output.slice(-6000);
  }
  if (task.trace && task.trace.length) {
    ui.pre.textContent +=
      "\n--- trace ---\n" +
      task.trace.slice(-60).map(traceLine).join("\n");
  }
  ui.root.scrollIntoView({ block: "end" });
}

/**
 * trace 条目格式化
 */
function traceLine(t) {
  if (typeof t === "string") return t;
  switch (t.kind) {
    case "submitted":
      return "已提交";
    case "accepted":
      return "已接受";
    case "agentStarted":
      return "助手已启动";
    case "alreadyStarted":
      return "已在运行";
    case "reasoning":
      return "思考中";
    case "turnCompleted":
      return "轮次完成";
    case "toolCall":
      return "工具调用：" + t.name;
    case "toolResult":
      return "工具结果：" + t.name;
    case "failed":
      return "失败：" + t.phase;
    case "legacy":
      return t.label || "";
    default:
      return t.kind || "";
  }
}

/* ---------- 任务完成渲染 ---------- */

/**
 * 渲染任务完成状态
 */
function renderFinished(task) {
  if (runningUi) {
    runningUi.root.remove();
    runningUi = null;
  }
  hideCancelButton();
  updateResumeButton(task);
  busy = false;
  $("submit-btn").disabled = false;

  if (task.status === "error") {
    addMessage("assistant error", task.summary || "执行失败", {
      details: traceText(task),
    });
  } else {
    addMessage("assistant", task.summary || "（无摘要）", {
      details: traceText(task),
    });
  }
}

/**
 * 收集完整 trace 文本
 */
function traceText(task) {
  const parts = [];
  if (task.output) parts.push(task.output);
  if (task.trace && task.trace.length) {
    parts.push("--- trace ---\n" + task.trace.map(traceLine).join("\n"));
  }
  return parts.join("\n");
}

/* ---------- 打开 / 轮询任务 ---------- */

/**
 * 打开一个历史任务，加载消息
 */
async function openTask(id) {
  stopPolling();
  runningUi = null;
  currentTaskId = id;

  const task = await api(`/api/sessions/${id}`);
  const messages = $("messages-area");
  messages.innerHTML = "";

  addMessage("user", task.task);

  if (task.status === "running") {
    runningUi = makeRunningUi(task);
    showCancelButton();
    updateResumeButton(task);
    pollTask(id);
  } else {
    renderFinished(task);
  }

  // 更新场景下拉框值
  if (task.scope) {
    $("scope-input").value = task.scope;
  }

  loadTaskHistory();
}

/**
 * 轮询任务状态
 */
function pollTask(id) {
  pollTimer = setInterval(async () => {
    try {
      const task = await api(`/api/sessions/${id}`);
      if (task.status === "running") {
        if (runningUi) updateRunningUi(runningUi, task);
        return;
      }
      stopPolling();
      renderFinished(task);
      loadTaskHistory();
    } catch (error) {
      stopPolling();
      if (runningUi) {
        runningUi.root.remove();
        runningUi = null;
      }
      busy = false;
      $("submit-btn").disabled = false;
      hideCancelButton();
      addMessage("assistant error", `轮询失败：${error.message}`);
    }
  }, 1500);
}

function stopPolling() {
  if (pollTimer) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
}

/* ---------- 按钮显隐控制 ---------- */

function showCancelButton() {
  const btn = $("cancel-task-btn");
  if (btn) btn.classList.remove("hidden");
}

function hideCancelButton() {
  const btn = $("cancel-task-btn");
  if (btn) btn.classList.add("hidden");
}

function updateResumeButton(task) {
  const btn = $("resume-task-btn");
  if (!btn) return;
  const hasThread = Boolean(task.thread_id);
  const finished = task.status !== "running";
  btn.classList.toggle("hidden", !(hasThread && finished));
}

/* ---------- 新建任务 ---------- */

function newTask() {
  stopPolling();
  hideCancelButton();
  const resumeBtn = $("resume-task-btn");
  if (resumeBtn) resumeBtn.classList.add("hidden");
  runningUi = null;
  busy = false;
  $("submit-btn").disabled = false;
  currentTaskId = null;

  const messages = $("messages-area");
  messages.innerHTML = `
    <div class="messages-empty">
      <div class="empty-title">选择一个助手，告诉我你想做什么</div>
      <div class="empty-desc">在下方输入任务内容，点击「开始执行」</div>
    </div>
  `;
  $("task-input").value = "";
  loadTaskHistory();
}

/* ---------- 取消任务 ---------- */

async function cancelTask() {
  if (!currentTaskId) return;
  try {
    await api(`/api/sessions/${currentTaskId}/cancel`, { method: "POST" });
    stopPolling();
    const task = await api(`/api/sessions/${currentTaskId}`);
    renderFinished(task);
    loadTaskHistory();
    toast("已终止执行", "info");
  } catch (error) {
    addMessage("assistant error", `终止失败：${error.message}`);
  }
}

/* ---------- 继续任务 ---------- */

async function resumeTask() {
  if (!currentTaskId) return;
  const text = prompt("继续任务内容（将使用同一上下文追加一轮）：");
  if (!text) return;
  try {
    await api(`/api/sessions/${currentTaskId}/resume`, {
      method: "POST",
      body: JSON.stringify({ task: text }),
    });
    openTask(currentTaskId);
  } catch (error) {
    addMessage("assistant error", `继续执行失败：${error.message}`);
  }
}

/* ---------- 场景下拉补全 ---------- */

/**
 * 从经验列表中提取场景，填充 scope datalist
 */
async function refreshScopeOptions() {
  try {
    const experiences = await api("/api/experiences").catch(() => []);
    const scenes = new Set();
    for (const item of experiences) {
      if (item.scope) scenes.add(item.scope);
    }
    const datalist = $("scope-options");
    if (!datalist) return;
    datalist.innerHTML = "";
    for (const scene of scenes) {
      const option = document.createElement("option");
      option.value = scene;
      datalist.appendChild(option);
    }
  } catch (e) {
    // 静默失败，不影响核心功能
  }
}

/* ---------- 表单提交：新建任务 ---------- */

async function submitTask(event) {
  event.preventDefault();
  if (busy) return;

  const taskText = $("task-input").value.trim();
  if (!taskText) return;

  const agentId = $("agent-select").value;
  if (!agentId) {
    toast("请先选择助手", "error");
    return;
  }

  busy = true;
  $("submit-btn").disabled = true;

  const cwd = $("cwd-input").value.trim() || null;
  const scope = $("scope-input").value.trim() || null;

  addMessage("user", taskText);
  $("task-input").value = "";

  try {
    const task = await api("/api/sessions", {
      method: "POST",
      body: JSON.stringify({ agent_id: agentId, task: taskText, cwd, scope }),
    });
    currentTaskId = task.id;
    runningUi = makeRunningUi(task);
    showCancelButton();
    pollTask(task.id);
    loadTaskHistory();
  } catch (error) {
    busy = false;
    $("submit-btn").disabled = false;
    addMessage("assistant error", error.message);
  }
}

/* ---------- 事件绑定 ---------- */

function bindEvents() {
  // 新建任务按钮
  const newBtn = $("new-task-btn");
  if (newBtn) newBtn.onclick = newTask;

  // 取消 / 继续 按钮
  const cancelBtn = $("cancel-task-btn");
  if (cancelBtn) cancelBtn.onclick = cancelTask;

  const resumeBtn = $("resume-task-btn");
  if (resumeBtn) resumeBtn.onclick = resumeTask;

  // 助手选择变化
  const agentSelect = $("agent-select");
  if (agentSelect) agentSelect.onchange = updateAgentState;

  // 表单提交
  const form = $("task-form");
  if (form) form.onsubmit = submitTask;

  // Ctrl+Enter 快捷提交
  const textarea = $("task-input");
  if (textarea) {
    textarea.addEventListener("keydown", (e) => {
      if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
        e.preventDefault();
        if (form) form.requestSubmit();
      }
    });
  }
}

/* ---------- 页面初始化 ---------- */

async function initTasksPage() {
  bindEvents();

  try {
    await refreshAgentSelect();
  } catch (error) {
    toast(`加载助手列表失败：${error.message}`, "error");
  }

  // 异步加载场景选项
  refreshScopeOptions();
}

/* DOM 就绪后初始化 */
if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", initTasksPage);
} else {
  initTasksPage();
}

/* ============================================================
 * experiences.js — 经验库页面业务逻辑
 * 从 app.js 提取并改造，全中文界面，Codex 风格
 * 依赖：api.js（api, $, esc, toast, actorName）、layout.js
 * ============================================================ */

/* ---------- 全局状态 ---------- */
let experiences = [];
let usage = { entries: {} };
let currentExperience = null;

/* ---------- 工具函数 ---------- */

/**
 * 从经验名提取分类（family）
 * 规则：去掉 cand_ 前缀后，取第一个下划线前的部分
 */
function familyOf(name) {
  const stripped = name.startsWith("cand_") ? name.slice(5) : name;
  const at = stripped.indexOf("_");
  return at > 0 ? stripped.slice(0, at) : stripped;
}

/**
 * 获取经验的使用次数
 */
function usageCountOf(name) {
  const entry = usage.entries[name];
  return entry && entry.activity ? entry.activity.usage_count : 0;
}

/**
 * 状态对应的 CSS 类名（用于状态点颜色）
 */
function statusClass(status) {
  switch (status) {
    case "active":
      return "status-dot active";
    case "draft":
      return "status-dot draft";
    case "candidate":
      return "status-dot candidate";
    case "validated":
      return "status-dot validated";
    case "decaying":
      return "status-dot decaying";
    case "disabled":
      return "status-dot disabled";
    default:
      return "status-dot disabled";
  }
}

/**
 * 状态中文显示名
 */
function statusLabel(status) {
  const map = {
    active: "已启用",
    draft: "草稿",
    candidate: "待验证",
    validated: "已验证",
    decaying: "待评估",
    disabled: "已停用",
  };
  return map[status] || status;
}

/**
 * 状态对应的徽章 CSS 类
 */
function statusBadgeClass(status) {
  switch (status) {
    case "active":
    case "validated":
      return "badge badge-success";
    case "draft":
    case "candidate":
      return "badge badge-warning";
    case "decaying":
      return "badge badge-warning";
    case "disabled":
      return "badge badge-default";
    default:
      return "badge badge-default";
  }
}

/**
 * 解析 key = JSON value 格式的多行文本
 * 用于 preconditions / postconditions 的编辑与展示
 */
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

/**
 * 将 key-value 数组格式化为显示文本
 */
function formatKeyValueLines(items) {
  if (!items || items.length === 0) return "";
  return items.map((p) => `${p.key} = ${JSON.stringify(p.expected)}`).join("\n");
}

/**
 * 字段标签包装器
 */
function fieldLabel(label, ...controls) {
  const wrap = document.createElement("div");
  wrap.className = "meta-field";
  const title = document.createElement("label");
  title.textContent = label;
  wrap.append(title, ...controls);
  return wrap;
}

/**
 * 创建按钮
 */
function button(text, onclick, danger = false, primary = false, sm = false) {
  const btn = document.createElement("button");
  btn.textContent = text;
  btn.className = "btn";
  if (danger) btn.classList.add("btn-danger");
  if (primary) btn.classList.add("btn-primary");
  if (sm) btn.classList.add("btn-sm");
  btn.onclick = onclick;
  return btn;
}

/* ---------- 数据加载 ---------- */

/**
 * 加载经验列表 + usage 数据
 */
async function loadExperiences() {
  try {
    [experiences, usage] = await Promise.all([
      api("/api/experiences"),
      api("/api/usage").catch(() => ({ entries: {} })),
    ]);
    refreshScopeOptions();
    renderExperienceTree();
  } catch (error) {
    toast(`加载失败：${error.message}`, "error");
  }
}

/**
 * 刷新场景下拉选项
 */
function refreshScopeOptions() {
  const scenes = new Set();
  for (const item of experiences) {
    if (item.scope) scenes.add(item.scope);
  }
  const select = $("scope-filter");
  const currentValue = select.value;
  // 保留第一个"全部场景"选项
  select.innerHTML = '<option value="">全部场景</option>';
  for (const scene of scenes) {
    const option = document.createElement("option");
    option.value = scene;
    option.textContent = scene;
    select.appendChild(option);
  }
  // 恢复选中值
  if ([...scenes].includes(currentValue)) {
    select.value = currentValue;
  }
}

/* ---------- 筛选 ---------- */

/**
 * 应用搜索/筛选条件，返回过滤后的经验列表
 */
function filteredExperiences() {
  const status = $("status-filter").value;
  const scope = $("scope-filter").value.trim();
  const usageMin = Number($("usage-min").value || 0);
  const query = $("tree-search").value.trim().toLowerCase();

  return experiences.filter((item) => {
    // 状态过滤
    if (status && item.status !== status) return false;
    // 场景过滤
    if (scope && item.scope !== scope) return false;
    // 最少使用次数
    if (usageMin > 0 && usageCountOf(item.name) < usageMin) return false;
    // 搜索（名称 + 显示名）
    if (query) {
      const haystack = `${item.name} ${item.display_name || ""}`.toLowerCase();
      if (!haystack.includes(query)) return false;
    }
    return true;
  });
}

/* ---------- 树渲染 ---------- */

/**
 * 渲染经验树结构（场景 → 分类 → 经验）
 */
function renderExperienceTree() {
  const root = $("experience-tree");
  root.innerHTML = "";
  const list = filteredExperiences();

  if (list.length === 0) {
    const empty = document.createElement("li");
    empty.className = "tree-empty";
    empty.textContent = "（无匹配经验）";
    root.appendChild(empty);
    return;
  }

  // 按 场景 → 分类 → 经验 分组
  const scenes = new Map();
  for (const item of list) {
    const scene = item.scope || "（未分类）";
    if (!scenes.has(scene)) scenes.set(scene, new Map());
    const families = scenes.get(scene);
    const family = familyOf(item.name);
    if (!families.has(family)) families.set(family, []);
    families.get(family).push(item);
  }

  for (const [scene, families] of scenes) {
    const sceneLi = document.createElement("li");
    const sceneCount = [...families.values()].flat().length;

    const sceneHead = document.createElement("div");
    sceneHead.className = "tree-scene";
    sceneHead.innerHTML = `
      <span class="scene-name" title="${esc(scene)}">${esc(scene)}</span>
      <span class="count-badge">${sceneCount}</span>
    `;

    const sceneUl = document.createElement("ul");

    // 点击场景头展开/折叠
    sceneHead.addEventListener("click", (e) => {
      e.stopPropagation();
      sceneHead.classList.toggle("collapsed");
      sceneUl.classList.toggle("hidden");
    });

    for (const [family, items] of families) {
      const famLi = document.createElement("li");
      const famCount = items.length;

      const famHead = document.createElement("div");
      famHead.className = "tree-family";
      famHead.innerHTML = `
        <span class="family-name" title="${esc(family)}">${esc(family)}</span>
        <span class="count-badge">${famCount}</span>
      `;

      const famUl = document.createElement("ul");

      // 点击分类头展开/折叠
      famHead.addEventListener("click", (e) => {
        e.stopPropagation();
        famHead.classList.toggle("collapsed");
        famUl.classList.toggle("hidden");
      });

      for (const item of items) {
        const li = document.createElement("li");
        li.className = "tree-leaf";
        if (currentExperience === item.name) {
          li.classList.add("active");
        }

        const statusDot = document.createElement("span");
        statusDot.className = statusClass(item.status);
        statusDot.title = statusLabel(item.status);

        const nameSpan = document.createElement("span");
        nameSpan.className = "leaf-name";
        nameSpan.textContent = item.display_name || item.name;
        nameSpan.title = `内部名：${item.name}\n状态：${statusLabel(item.status)}\n使用次数：${usageCountOf(item.name)}`;

        const badgeSpan = document.createElement("span");
        badgeSpan.className = "leaf-badge";
        const usageCnt = usageCountOf(item.name);
        const pinMark = item.pinned ? "📌" : "";
        const denyMark = item.user_usage === "deny" ? "🚫" : "";
        badgeSpan.textContent = `${usageCnt}次${pinMark}${denyMark}`;

        li.append(statusDot, nameSpan, badgeSpan);
        li.addEventListener("click", () => openExperience(item.name));
        famUl.appendChild(li);
      }

      famLi.append(famHead, famUl);
      sceneUl.appendChild(famLi);
    }

    sceneLi.append(sceneHead, sceneUl);
    root.appendChild(sceneLi);
  }
}

/* ---------- 详情页：打开与渲染 ---------- */

/**
 * 打开经验详情
 */
async function openExperience(name) {
  currentExperience = name;
  try {
    const [detail, audit] = await Promise.all([
      api(`/api/experiences/${encodeURIComponent(name)}`),
      api(`/api/audit?name=${encodeURIComponent(name)}&limit=10`).catch(() => ({
        records: [],
      })),
    ]);

    const summary = experiences.find((item) => item.name === name) || {};

    // 显示详情区，隐藏空态
    $("detail-empty").classList.add("hidden");
    $("detail-content").classList.remove("hidden");

    // 标题
    $("detail-name").textContent = summary.display_name || detail.name;

    // 状态徽章行
    renderDetailBadges(detail, summary);

    // 操作按钮
    renderActions(detail, summary);

    // 基本信息
    renderMetaForms(detail, summary);

    // 评分区
    renderScoresPanel(detail);

    // 使用统计
    renderUsagePanel(detail);

    // 经验内容
    renderExpBody(detail);

    // 编辑区（仅草稿状态显示）
    if (detail.status === "draft") {
      $("editor-section").classList.remove("hidden");
      openEditor();
    } else {
      $("editor-section").classList.add("hidden");
    }

    // 操作记录
    renderAuditList($("audit-list"), audit.records || []);

    // 更新树中高亮
    renderExperienceTree();

    // 滚动到顶部
    $("detail-content").scrollTop = 0;
  } catch (error) {
    toast(`打开经验失败：${error.message}`, "error");
  }
}

/**
 * 渲染详情页顶部徽章行
 */
function renderDetailBadges(detail, summary) {
  const container = $("detail-badges");
  container.innerHTML = "";

  // 状态徽章
  const statusBadge = document.createElement("span");
  statusBadge.className = `badge ${statusBadgeClass(detail.status)}`;
  statusBadge.textContent = statusLabel(detail.status);
  container.appendChild(statusBadge);

  // 置顶
  if (summary.pinned) {
    const pinBadge = document.createElement("span");
    pinBadge.className = "badge badge-accent";
    pinBadge.textContent = "📌 置顶";
    container.appendChild(pinBadge);
  }

  // 场景
  const scopeBadge = document.createElement("span");
  scopeBadge.className = "badge badge-default";
  scopeBadge.textContent = detail.scope ? `场景：${detail.scope}` : "无场景";
  container.appendChild(scopeBadge);

  // 用户停用
  if (detail.user_usage === "deny") {
    const denyBadge = document.createElement("span");
    denyBadge.className = "badge badge-danger";
    denyBadge.textContent = "不使用";
    container.appendChild(denyBadge);
  }
}

/* ---------- 基本信息 / 元数据表单 ---------- */

/**
 * 渲染场景/别名/偏好编辑表单
 */
function renderMetaForms(detail, summary) {
  const meta = $("detail-meta");
  meta.innerHTML = "";

  // 显示名称（别名）
  const alias = document.createElement("input");
  alias.type = "text";
  alias.value = detail.display_name || "";
  alias.placeholder = "输入显示名称（留空使用内部名）";
  const aliasBtn = button("保存", async () => {
    try {
      await api(
        `/api/experiences/${encodeURIComponent(currentExperience)}/display_name`,
        {
          method: "POST",
          body: JSON.stringify({
            display_name: alias.value || null,
            actor: actorName(),
          }),
        }
      );
      toast("显示名称已更新", "success");
      openExperience(currentExperience);
    } catch (e) {
      toast(`保存失败：${e.message}`, "error");
    }
  }, false, false, true);
  meta.appendChild(fieldLabel("显示名称", alias, aliasBtn));

  // 场景
  const scope = document.createElement("input");
  scope.type = "text";
  scope.value = detail.scope || "";
  scope.placeholder = "输入场景名（留空=无场景）";
  const scopeBtn = button("保存", async () => {
    try {
      await api(
        `/api/experiences/${encodeURIComponent(currentExperience)}/scope`,
        {
          method: "POST",
          body: JSON.stringify({
            scope: scope.value || null,
            actor: actorName(),
          }),
        }
      );
      toast("场景已更新", "success");
      openExperience(currentExperience);
    } catch (e) {
      toast(`保存失败：${e.message}`, "error");
    }
  }, false, false, true);
  meta.appendChild(fieldLabel("场景", scope, scopeBtn));

  // 我的偏好
  const usageSel = document.createElement("select");
  const usageOptions = [
    { value: "auto", label: "跟随系统" },
    { value: "allow", label: "允许使用" },
    { value: "deny", label: "不使用此经验" },
  ];
  for (const opt of usageOptions) {
    const option = document.createElement("option");
    option.value = opt.value;
    option.textContent = opt.label;
    option.selected = (detail.user_usage || "auto") === opt.value;
    usageSel.appendChild(option);
  }

  const confidence = document.createElement("input");
  confidence.type = "number";
  confidence.min = "0";
  confidence.max = "1";
  confidence.step = "0.01";
  confidence.value = detail.user_confidence ?? "";
  confidence.placeholder = "0 ~ 1";
  confidence.title = "我的评价（0 ~ 1，仅影响个人排序）";

  const reason = document.createElement("input");
  reason.type = "text";
  reason.placeholder = "评价理由（可选）";

  const prefBtn = button("保存偏好", async () => {
    try {
      const confidenceValue =
        confidence.value === "" ? null : Number(confidence.value);
      await api(
        `/api/experiences/${encodeURIComponent(currentExperience)}/user-preference`,
        {
          method: "POST",
          body: JSON.stringify({
            usage: usageSel.value,
            confidence: confidenceValue,
            reason: reason.value,
            actor: actorName(),
          }),
        }
      );
      toast("偏好已保存", "success");
      openExperience(currentExperience);
    } catch (e) {
      toast(`保存失败：${e.message}`, "error");
    }
  }, false, false, true);

  const prefWrap = document.createElement("div");
  prefWrap.className = "meta-field";
  const prefLabel = document.createElement("label");
  prefLabel.textContent = "我的偏好";
  const prefControls = document.createElement("div");
  prefControls.style.cssText =
    "display:flex;flex-wrap:wrap;gap:8px;align-items:center;flex:1;";
  prefControls.append(usageSel, confidence, reason, prefBtn);
  prefWrap.append(prefLabel, prefControls);
  meta.appendChild(prefWrap);
}

/* ---------- 评分区 ---------- */

/**
 * 渲染评分区（系统评分 + 我的评价并排展示）
 */
function renderScoresPanel(detail) {
  const container = $("exp-scores");
  const entry = usage.entries[detail.name];

  const evidenceScore = entry ? entry.score : 0;
  const evidence = entry?.evidence || {};
  const userScore =
    detail.user_confidence != null
      ? Number(detail.user_confidence).toFixed(2)
      : "—";

  container.innerHTML = `
    <div class="score-card">
      <div class="score-label">系统评分（Evidence Score）</div>
      <div class="score-value">${evidenceScore.toFixed(2)}</div>
      <div class="score-detail">
        成功 ${evidence.successes || 0} · 失败 ${evidence.misfires || 0} · 无效 ${
    evidence.invalid || 0
  }
      </div>
    </div>
    <div class="score-card">
      <div class="score-label">我的评价（User Confidence）</div>
      <div class="score-value">${userScore}</div>
      <div class="score-detail">
        ${detail.user_confidence != null ? "已设置个人评价" : "未设置，跟随系统"}
      </div>
    </div>
  `;
}

/* ---------- 使用统计 ---------- */

/**
 * 渲染使用统计区
 */
function renderUsagePanel(detail) {
  const box = $("exp-usage");
  const entry = usage.entries[detail.name];

  if (!entry) {
    box.innerHTML = '<div class="usage-empty">暂无使用记录</div>';
    return;
  }

  const evidence = entry.evidence || {};
  const activity = entry.activity || {};

  const items = [
    { label: "使用次数", value: activity.usage_count || 0 },
    { label: "成功次数", value: evidence.successes || 0 },
    { label: "失败次数", value: evidence.misfires || 0 },
    { label: "无效次数", value: evidence.invalid || 0 },
    {
      label: "执行错误",
      value: evidence.execution_errors || 0,
    },
  ];

  box.innerHTML = `<ul>
    ${items
      .map(
        (item) =>
          `<li><span class="usage-label">${item.label}</span><span>${item.value}</span></li>`
      )
      .join("")}
  </ul>`;
}

/* ---------- 经验内容区 ---------- */

/**
 * 渲染经验内容（分开展示各字段）
 */
function renderExpBody(detail) {
  // trigger
  const triggerText = detail.trigger
    ? JSON.stringify(detail.trigger, null, 2)
    : "";
  $("body-trigger").textContent = triggerText;

  // preconditions
  $("body-pre").textContent = formatKeyValueLines(detail.preconditions);

  // steps (workflow)
  const workflow = detail.workflow || [];
  $("body-steps").textContent = workflow.length
    ? workflow.map((step) => JSON.stringify(step)).join("\n")
    : "";

  // postconditions
  $("body-post").textContent = formatKeyValueLines(detail.postconditions);

  // verification
  const verification = detail.verification || [];
  $("body-verification").textContent = verification.length
    ? verification.map((v) => JSON.stringify(v)).join("\n")
    : "";
}

/* ---------- 操作按钮 ---------- */

/**
 * 渲染操作按钮（状态转移按转移表显隐）
 *
 * 状态转移表：
 * - validated → active（激活）
 * - candidate → validated（验证）
 * - active → disabled（停用）
 * - active → draft（草稿化）
 * - disabled / decaying → candidate（重新验证）
 * - draft → validated（采纳）
 * - draft → 删除（放弃草稿）
 */
function renderActions(detail, summary = {}) {
  const box = $("exp-actions");
  box.innerHTML = "";

  const status = detail.status;

  // 修改此经验（草稿化入口，非草稿状态时显示）
  if (status !== "draft") {
    box.appendChild(
      button("修改此经验", () => startDraft(), false, false, false)
    );
  }

  // 置顶 / 取消置顶
  if (status !== "draft") {
    const pinLabel = summary.pinned ? "取消置顶" : "置顶";
    box.appendChild(button(pinLabel, () => togglePin()));
  }

  // 状态转移按钮
  if (status === "validated") {
    box.appendChild(
      button("启用", () => setExperienceStatus("active"), false, true)
    );
  }
  if (status === "candidate") {
    box.appendChild(
      button("开始验证", () => setExperienceStatus("validate"), false, true)
    );
  }
  if (status === "active") {
    box.appendChild(button("停用", () => setExperienceStatus("disabled")));
  }
  if (status === "disabled" || status === "decaying") {
    box.appendChild(
      button("重新验证", () => setExperienceStatus("revalidate"))
    );
  }

  // 草稿状态特有按钮
  if (status === "draft") {
    box.appendChild(button("保存草稿", () => saveDraftBody(), false, true));
    box.appendChild(button("应用修改", () => adoptDraft(), false, true));
    box.appendChild(button("放弃修改", () => abandonDraft(), true));
  }

  // 删除（非草稿状态时显示为危险操作）
  if (status !== "draft") {
    box.appendChild(button("删除", () => removeExperience(), true));
  }
}

/* ---------- 状态操作 ---------- */

/**
 * 更改经验状态
 */
async function setExperienceStatus(status) {
  const reason =
    prompt(`状态操作：${statusLabel(status)}\n操作人：${actorName()}\n请输入理由（可选）`) ||
    "";
  try {
    await api(
      `/api/experiences/${encodeURIComponent(currentExperience)}/status`,
      {
        method: "PATCH",
        body: JSON.stringify({ status, reason, actor: actorName() }),
      }
    );
    toast(`状态已更新为「${statusLabel(status)}」`, "success");
    openExperience(currentExperience);
  } catch (e) {
    toast(`操作失败：${e.message}`, "error");
  }
}

/**
 * 置顶 / 取消置顶
 */
async function togglePin() {
  const summary = experiences.find((item) => item.name === currentExperience) || {};
  const pinned = Boolean(summary.pinned);
  try {
    await api(
      `/api/experiences/${encodeURIComponent(currentExperience)}/${
        pinned ? "unpin" : "pin"
      }`,
      { method: "POST", body: JSON.stringify({ actor: actorName() }) }
    );
    toast(pinned ? "已取消置顶" : "已置顶", "success");
    // 刷新列表数据
    experiences = await api("/api/experiences");
    openExperience(currentExperience);
  } catch (e) {
    toast(`操作失败：${e.message}`, "error");
  }
}

/**
 * 删除经验（二次确认）
 */
async function removeExperience() {
  if (!confirm(`确认删除经验「${currentExperience}」？此操作不可恢复。`))
    return;
  try {
    await api(`/api/experiences/${encodeURIComponent(currentExperience)}`, {
      method: "DELETE",
    });
    toast("已删除", "success");
    backToTree();
  } catch (e) {
    toast(`删除失败：${e.message}`, "error");
  }
}

/* ---------- 草稿流程 ---------- */

/**
 * 创建草稿（从当前活跃经验派生）
 */
async function startDraft() {
  try {
    await api(
      `/api/experiences/${encodeURIComponent(currentExperience)}/draft`,
      {
        method: "POST",
        body: JSON.stringify({ actor: actorName() }),
      }
    );
    toast("已创建草稿", "success");
    const draftName = `${currentExperience}__draft`;
    // 刷新列表后打开草稿
    experiences = await api("/api/experiences");
    openExperience(draftName);
  } catch (e) {
    toast(`创建草稿失败：${e.message}`, "error");
  }
}

/**
 * 打开编辑器（草稿模式）
 */
async function openEditor() {
  const editor = $("editor-root");
  editor.innerHTML = "";

  try {
    const detail = await api(
      `/api/experiences/${encodeURIComponent(currentExperience)}`
    );

    const form = document.createElement("div");
    form.style.cssText = "display:flex;flex-direction:column;gap:10px;";

    form.innerHTML = `
      <div class="editor-field">
        <label>触发工具（trigger.tool）</label>
        <select id="edit-tool">
          <option value="exec_command">exec_command</option>
          <option value="write_file">write_file</option>
          <option value="read_file">read_file</option>
        </select>
      </div>
      <div class="editor-field">
        <label>命令模式（command_pattern）</label>
        <input id="edit-pattern" type="text" placeholder="例如：git commit *" />
      </div>
      <div class="editor-field">
        <label>前置条件（preconditions）—— 每行 key = JSON value</label>
        <textarea id="edit-pre" rows="3" placeholder="key = &quot;expected_value&quot;"></textarea>
      </div>
      <div class="editor-field">
        <label>执行步骤（workflow）—— 每行一个 JSON 对象</label>
        <textarea id="edit-workflow" rows="6" placeholder='{"action":"write_file","args":{...}}'></textarea>
      </div>
      <div class="editor-field">
        <label>后置条件（postconditions）—— 每行 key = JSON value</label>
        <textarea id="edit-post" rows="3" placeholder="key = &quot;expected_value&quot;"></textarea>
      </div>
      <div class="editor-field">
        <label>失败策略（failure_policy）</label>
        <select id="edit-failure">
          <option value="stop_and_report">stop_and_report</option>
        </select>
      </div>
      <div class="editor-field">
        <label>撤销支持（undo）</label>
        <select id="edit-undo">
          <option value="unsupported">unsupported</option>
        </select>
      </div>
    `;

    // 填充现有值
    const field = (id) => form.querySelector(`#${id}`);
    if (detail.trigger) {
      field("edit-tool").value = detail.trigger.tool || "exec_command";
      field("edit-pattern").value = detail.trigger.command_pattern || "";
    }
    field("edit-pre").value = formatKeyValueLines(detail.preconditions);
    field("edit-post").value = formatKeyValueLines(detail.postconditions);
    field("edit-workflow").value = (detail.workflow || [])
      .map((step) => JSON.stringify(step))
      .join("\n");
    if (detail.failure_policy)
      field("edit-failure").value = detail.failure_policy;
    if (detail.undo) field("edit-undo").value = detail.undo;

    const actionsRow = document.createElement("div");
    actionsRow.className = "editor-actions";
    actionsRow.appendChild(
      button("保存草稿", () => saveDraftBody(), false, true)
    );
    actionsRow.appendChild(
      button("应用修改", () => adoptDraft(), false, true)
    );
    actionsRow.appendChild(button("放弃修改", () => abandonDraft(), true));

    editor.append(form, actionsRow);
  } catch (e) {
    editor.innerHTML = `<div style="color:var(--danger);">编辑器加载失败：${esc(
      e.message
    )}</div>`;
  }
}

/**
 * 保存草稿内容
 */
async function saveDraftBody() {
  try {
    const detail = await api(
      `/api/experiences/${encodeURIComponent(currentExperience)}`
    );

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

    await api(
      `/api/experiences/${encodeURIComponent(currentExperience)}/body`,
      {
        method: "PUT",
        body: JSON.stringify(body),
      }
    );
    toast("草稿已保存", "success");
    openExperience(currentExperience);
  } catch (e) {
    toast(`保存失败：${e.message}`, "error");
  }
}

/**
 * 应用草稿（采纳）
 */
async function adoptDraft() {
  const reason =
    prompt("应用修改理由（可选）\n操作人：" + actorName()) || "";
  try {
    await api(
      `/api/experiences/${encodeURIComponent(currentExperience)}/adopt`,
      {
        method: "POST",
        body: JSON.stringify({ reason, actor: actorName() }),
      }
    );
    toast("已应用修改", "success");
    backToTree();
  } catch (e) {
    toast(`应用失败：${e.message}`, "error");
  }
}

/**
 * 放弃草稿
 */
async function abandonDraft() {
  if (!confirm(`确认放弃草稿「${currentExperience}」？`)) return;
  try {
    await api(`/api/experiences/${encodeURIComponent(currentExperience)}`, {
      method: "DELETE",
    });
    toast("已放弃草稿", "success");
    backToTree();
  } catch (e) {
    toast(`操作失败：${e.message}`, "error");
  }
}

/**
 * 返回树视图（清空详情选中）
 */
function backToTree() {
  currentExperience = null;
  $("detail-content").classList.add("hidden");
  $("detail-empty").classList.remove("hidden");
  loadExperiences();
}

/* ---------- 操作记录 ---------- */

/**
 * 渲染审计（操作）记录列表
 */
function renderAuditList(target, records) {
  target.innerHTML = "";
  if (!records || records.length === 0) {
    const li = document.createElement("li");
    li.className = "audit-empty";
    li.textContent = "（暂无操作记录）";
    target.appendChild(li);
    return;
  }
  for (const record of records) {
    const li = document.createElement("li");
    const time = new Date((record.recorded_at || 0) * 1000).toLocaleString(
      "zh-CN"
    );
    const name = record.candidate_name || record.experience_name || "";
    const outcome = record.outcome ? ` · ${record.outcome}` : "";
    const reason = record.reason ? ` · ${record.reason}` : "";
    const actor = record.actor ? ` · 操作人：${record.actor}` : "";
    li.innerHTML = `
      <span class="audit-time">${esc(time)}</span>
      <span class="audit-type">${esc(record.record_type || "")}</span>
      <span>${esc(name)}${esc(outcome)}${esc(reason)}${esc(actor)}</span>
    `;
    target.appendChild(li);
  }
}

/* ---------- 事件绑定 ---------- */

function bindEvents() {
  $("refresh-tree").addEventListener("click", () => loadExperiences());
  $("tree-search").addEventListener("input", renderExperienceTree);
  $("scope-filter").addEventListener("change", renderExperienceTree);
  $("status-filter").addEventListener("change", renderExperienceTree);
  $("usage-min").addEventListener("input", renderExperienceTree);
}

/* ---------- 页面初始化 ---------- */

function initExperiencesPage() {
  bindEvents();
  loadExperiences();
}

// DOM 就绪后初始化
if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", initExperiencesPage);
} else {
  initExperiencesPage();
}

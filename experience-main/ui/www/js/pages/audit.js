/* ============================================================
 * audit.js — 操作记录页面业务逻辑
 * 依赖：api.js (api, $, esc, toast), layout.js
 * ============================================================ */

/* ---------- 操作类型中文映射 ---------- */

const RECORD_TYPE_LABELS = {
  drafted: "创建草稿",
  edited: "修改内容",
  adopted: "应用修改",
  abandoned: "放弃草稿",
  activated: "启用经验",
  disabled: "停用经验",
  validated: "验证通过",
  deleted: "删除经验",
  pinned: "置顶",
  unpinned: "取消置顶",
  scope_changed: "更改场景",
  display_name_changed: "修改显示名称",
  user_preference_changed: "修改我的偏好",
};

function getRecordTypeLabel(type) {
  return RECORD_TYPE_LABELS[type] || type;
}

function getTypeBadgeClass(type) {
  const mapped = RECORD_TYPE_LABELS[type] ? `type-badge-${type}` : "type-badge-default";
  return `timeline-type ${mapped}`;
}

function getTimelineItemClass(type) {
  return `timeline-item type-${type}`;
}

/* ---------- 相对时间格式化 ---------- */

function formatRelativeTime(timestamp) {
  const now = Date.now() / 1000;
  const diff = now - timestamp;

  if (diff < 60) {
    return "刚刚";
  } else if (diff < 3600) {
    const minutes = Math.floor(diff / 60);
    return `${minutes} 分钟前`;
  } else if (diff < 86400) {
    const hours = Math.floor(diff / 3600);
    return `${hours} 小时前`;
  } else if (diff < 2592000) {
    const days = Math.floor(diff / 86400);
    return `${days} 天前`;
  } else if (diff < 31536000) {
    const months = Math.floor(diff / 2592000);
    return `${months} 个月前`;
  } else {
    const years = Math.floor(diff / 31536000);
    return `${years} 年前`;
  }
}

function formatAbsoluteTime(timestamp) {
  const date = new Date(timestamp * 1000);
  return date.toLocaleString("zh-CN", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
}

/* ---------- 加载操作记录 ---------- */

async function loadAudit() {
  const name = $("audit-name").value.trim();
  const type = $("audit-type").value.trim();
  const limit = Number($("audit-limit").value || 100);

  const params = new URLSearchParams();
  if (name) params.set("name", name);
  if (type) params.set("record_type", type);
  params.set("limit", String(limit));

  try {
    const data = await api(`/api/audit?${params.toString()}`);
    renderAuditTimeline(data.records || []);
  } catch (error) {
    toast(`加载失败：${error.message}`, "error");
  }
}

/* ---------- 渲染时间线 ---------- */

function renderAuditTimeline(records) {
  const timeline = $("audit-timeline");
  const empty = $("audit-empty");
  const count = $("audit-count");

  timeline.innerHTML = "";

  if (records.length === 0) {
    empty.classList.remove("hidden");
    count.textContent = "";
    return;
  }
  empty.classList.add("hidden");
  count.textContent = `共 ${records.length} 条记录`;

  for (const record of records) {
    const item = document.createElement("div");
    item.className = getTimelineItemClass(record.record_type);

    // 圆点
    const dot = document.createElement("div");
    dot.className = "timeline-dot";
    item.appendChild(dot);

    // 内容
    const content = document.createElement("div");
    content.className = "timeline-content";

    // 头部：时间 + 操作类型
    const head = document.createElement("div");
    head.className = "timeline-head";

    const time = document.createElement("span");
    time.className = "timeline-time";
    const timestamp = record.recorded_at || 0;
    time.textContent = formatRelativeTime(timestamp);
    time.title = formatAbsoluteTime(timestamp);
    head.appendChild(time);

    const typeBadge = document.createElement("span");
    typeBadge.className = getTypeBadgeClass(record.record_type);
    typeBadge.textContent = getRecordTypeLabel(record.record_type);
    head.appendChild(typeBadge);

    content.appendChild(head);

    // 主体：经验名称
    const body = document.createElement("div");
    body.className = "timeline-body";

    const expName = document.createElement("span");
    expName.className = "timeline-exp-name";
    expName.textContent = record.candidate_name || record.experience_name || "(未知经验)";
    body.appendChild(expName);

    // outcome 附加信息
    if (record.outcome && record.outcome !== "success") {
      const outcome = document.createElement("span");
      outcome.style.marginLeft = "8px";
      outcome.style.fontSize = "13px";
      outcome.style.color = "var(--text-muted)";
      outcome.textContent = `· ${record.outcome}`;
      body.appendChild(outcome);
    }

    content.appendChild(body);

    // 元信息：操作人
    const meta = document.createElement("div");
    meta.className = "timeline-meta";

    if (record.actor) {
      const actor = document.createElement("span");
      actor.className = "timeline-actor";
      actor.innerHTML = `👤 ${esc(record.actor)}`;
      meta.appendChild(actor);
    }

    if (record.scope) {
      const scope = document.createElement("span");
      scope.textContent = `场景：${record.scope}`;
      meta.appendChild(scope);
    }

    content.appendChild(meta);

    // 原因/备注
    if (record.reason) {
      const reason = document.createElement("div");
      reason.className = "timeline-reason";
      reason.textContent = record.reason;
      content.appendChild(reason);
    }

    item.appendChild(content);
    timeline.appendChild(item);
  }
}

/* ---------- 事件绑定 ---------- */

$("refresh-audit").addEventListener("click", () => {
  loadAudit();
});

$("audit-name").addEventListener("change", () => {
  loadAudit();
});

$("audit-type").addEventListener("change", () => {
  loadAudit();
});

$("audit-limit").addEventListener("change", () => {
  loadAudit();
});

// 回车键触发搜索
$("audit-name").addEventListener("keydown", (e) => {
  if (e.key === "Enter") {
    loadAudit();
  }
});

/* ---------- 页面初始化 ---------- */

loadAudit();

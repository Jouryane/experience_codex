/* ============================================================
 * settings.js — S4 设置页
 * 能力策略 / 学习与复用开关 / 撤销最近的经验执行
 * 依赖：api.js
 * ============================================================ */

let _policyScope = "__global__";

function actorOr() {
  return typeof actorName === "function" ? actorName() : "user";
}

/* ---------- 能力策略 ---------- */

async function loadPolicy(scope) {
  _policyScope = scope || _policyScope;
  const data = await api(`/api/policy?scope=${encodeURIComponent(_policyScope)}`);
  const policy = data.effective || {};
  $("policy-fs-write").value = policy.fs_write || "workspace_only";
  $("policy-fs-delete").value = policy.fs_delete || "deny";
  $("policy-exec-mode").value = (policy.exec && policy.exec.mode) || "off";
  $("policy-exec-allow").value = ((policy.exec && policy.exec.allow) || []).join(",");
  $("policy-read-max").value = (policy.fs_read && policy.fs_read.max_bytes) || 20480;
  const grant = policy.fs_read && policy.fs_read.special_grant_bytes;
  $("policy-read-grant").value = grant == null ? "" : grant;
  $("policy-status").textContent = data.scope_has_policy
    ? `已保存：${_policyScope}`
    : `当前继承全局默认（未单独保存 ${_policyScope}）`;
}

function policyBodyFromForm(scope) {
  const allow = $("policy-exec-allow")
    .value.split(",")
    .map((item) => item.trim())
    .filter(Boolean);
  const readMax = parseInt($("policy-read-max").value, 10) || 20480;
  const grantRaw = $("policy-read-grant").value.trim();
  const fsRead = { enabled: true, workspace_only: true, max_bytes: readMax };
  if (grantRaw) {
    fsRead.special_grant_bytes = parseInt(grantRaw, 10);
  }
  return {
    scope,
    actor: actorOr(),
    policy: {
      fs_read: fsRead,
      fs_write: $("policy-fs-write").value,
      fs_delete: $("policy-fs-delete").value,
      exec: {
        mode: $("policy-exec-mode").value,
        allow,
        timeout_secs: 60,
        output_cap: 20000,
        allow_legacy_shell: false,
      },
      network: { mode: "off", allow: [] },
    },
  };
}

async function savePolicy() {
  try {
    await api("/api/policy", {
      method: "PUT",
      body: JSON.stringify(policyBodyFromForm(_policyScope)),
    });
    toast("策略已保存", "success");
    await loadPolicy(_policyScope);
  } catch (error) {
    toast(`保存失败：${error.message}`, "error");
  }
}

async function clearPolicy() {
  try {
    await api("/api/policy", {
      method: "PUT",
      body: JSON.stringify({ scope: _policyScope, actor: actorOr(), clear: true }),
    });
    toast("已恢复继承", "success");
    await loadPolicy(_policyScope);
  } catch (error) {
    toast(`清除失败：${error.message}`, "error");
  }
}

/* ---------- 学习与复用开关 ---------- */

async function loadSettings() {
  const data = await api("/api/settings");
  $("setting-compiler").checked = !!data.settings.llm_compiler;
  $("setting-injection").checked = !!data.settings.injection_policy;
  $("setting-undo-keep").value = data.settings.undo_keep || 20;
  const overrides = Object.entries(data.env_override || {})
    .filter(([, value]) => value != null)
    .map(([key]) => key);
  $("settings-env-note").textContent = overrides.length
    ? `环境变量生效中：${overrides.join("、")}（优先于此处设置）`
    : "环境变量未设置，此处设置生效";
}

async function saveSettings() {
  try {
    await api("/api/settings", {
      method: "PUT",
      body: JSON.stringify({
        actor: actorOr(),
        llm_compiler: $("setting-compiler").checked,
        injection_policy: $("setting-injection").checked,
        undo_keep: parseInt($("setting-undo-keep").value, 10) || 20,
      }),
    });
    toast("设置已保存", "success");
    await loadSettings();
  } catch (error) {
    toast(`保存失败：${error.message}`, "error");
  }
}

/* ---------- 撤销最近的经验执行 ---------- */

async function loadUndo() {
  const data = await api("/api/undo?limit=20");
  const runs = data.runs || [];
  const list = $("undo-list");
  list.innerHTML = "";
  $("undo-empty").classList.toggle("hidden", runs.length > 0);

  runs.forEach((run) => {
    const item = document.createElement("div");
    item.className = "undo-item";
    const when = run.recorded_at
      ? new Date(run.recorded_at * 1000).toLocaleString()
      : "-";
    const snapshots = run.snapshots || [];
    item.innerHTML =
      `<div class="undo-head">` +
      `<strong>${esc(run.candidate_name || "-")}</strong>` +
      `<span class="badge">${esc(run.outcome || "-")}</span>` +
      `<span class="text-sm text-muted">${esc(when)}</span>` +
      `</div>` +
      `<div class="text-sm text-muted">session: ${esc(run.session_id || "-")}</div>`;

    if (snapshots.length === 0) {
      const note = document.createElement("div");
      note.className = "text-sm text-muted";
      note.textContent = "本次执行没有可回滚的文件改动";
      item.appendChild(note);
    } else {
      const actions = document.createElement("div");
      actions.className = "settings-actions";
      snapshots.forEach((snapshot) => {
        const button = document.createElement("button");
        button.className = "btn";
        button.textContent = `撤销 ${snapshot.snapshot}`;
        button.addEventListener("click", async () => {
          button.disabled = true;
          try {
            const result = await api(
              `/api/backups/${encodeURIComponent(run.session_id)}/restore`,
              {
                method: "POST",
                body: JSON.stringify({
                  snapshot: snapshot.snapshot,
                  actor: actorOr(),
                }),
              }
            );
            toast(
              `已恢复 ${result.restored.length} 个文件、移除 ${result.deleted.length} 个`,
              "success"
            );
            await loadUndo();
          } catch (error) {
            toast(`撤销失败：${error.message}`, "error");
            button.disabled = false;
          }
        });
        actions.appendChild(button);
      });
      item.appendChild(actions);
    }
    list.appendChild(item);
  });
}

/* ---------- 初始化 ---------- */

async function initSettings() {
  $("policy-load").addEventListener("click", () => {
    const scope = $("policy-scope").value.trim() || "__global__";
    loadPolicy(scope).catch((error) => toast(error.message, "error"));
  });
  $("policy-save").addEventListener("click", savePolicy);
  $("policy-clear").addEventListener("click", clearPolicy);
  $("settings-save").addEventListener("click", saveSettings);
  $("undo-refresh").addEventListener("click", () => {
    loadUndo().catch((error) => toast(error.message, "error"));
  });

  try {
    await Promise.all([loadPolicy("__global__"), loadSettings(), loadUndo()]);
  } catch (error) {
    toast(`加载失败：${error.message}`, "error");
  }
}

if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", initSettings);
} else {
  initSettings();
}

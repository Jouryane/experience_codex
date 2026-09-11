// 吸收向导页 — 业务逻辑
(function () {
  // ── 标签页切换 ──
  document.querySelectorAll(".absorb-tab").forEach(function (tab) {
    tab.addEventListener("click", function () {
      document.querySelectorAll(".absorb-tab").forEach(function (t) {
        t.classList.remove("active");
      });
      tab.classList.add("active");
      document.querySelectorAll(".absorb-panel").forEach(function (panel) {
        panel.classList.remove("active");
      });
      $("tab-" + tab.dataset.tab).classList.add("active");
      if (tab.dataset.tab === "refs") {
        loadReferences();
      }
    });
  });

  // ── 采集指南 ──
  async function loadGuide() {
    try {
      var guide = await api("/api/ingestion/guide?agent=trae");
      var checklist = $("checklist");
      (guide.checklist || []).forEach(function (item) {
        var li = document.createElement("li");
        li.textContent = item;
        checklist.appendChild(li);
      });
      var steps = $("workspace-steps");
      (guide.workspace_steps || []).forEach(function (step) {
        var li = document.createElement("li");
        li.innerHTML = step.replace(
          /`([^`]+)`/g,
          "<code>$1</code>"
        );
        steps.appendChild(li);
      });
      $("prompt-template").textContent =
        guide.prompt_template || "";
    } catch (err) {
      toast("加载采集指南失败：" + err.message, "error");
    }
  }

  // 复制模板
  $("copy-prompt").addEventListener("click", function () {
    var text = $("prompt-template").textContent;
    if (!text) return;
    navigator.clipboard.writeText(text).then(
      function () {
        toast("提示词模板已复制", "success");
      },
      function () {
        // 回退方案
        var ta = document.createElement("textarea");
        ta.value = text;
        document.body.appendChild(ta);
        ta.select();
        document.execCommand("copy");
        document.body.removeChild(ta);
        toast("提示词模板已复制", "success");
      }
    );
  });

  // ── 提交材料（三步流程） ──

  // 保存当前解析的 markdown（run-notes 提交时用）
  var currentMarkdown = "";

  // 步骤切换
  function showStep(name) {
    ["step-input", "step-confirm", "step-result"].forEach(function (id) {
      $(id).classList.add("hidden");
    });
    $("step-" + name).classList.remove("hidden");
  }

  // 文件选择
  $("choose-file").addEventListener("click", function () {
    $("file-input").click();
  });

  $("file-input").addEventListener("change", function (e) {
    var file = e.target.files[0];
    if (!file) return;
    readFile(file);
  });

  // 拖拽上传
  var uploadArea = $("upload-area");
  uploadArea.addEventListener("dragover", function (e) {
    e.preventDefault();
    uploadArea.classList.add("dragover");
  });
  uploadArea.addEventListener("dragleave", function () {
    uploadArea.classList.remove("dragover");
  });
  uploadArea.addEventListener("drop", function (e) {
    e.preventDefault();
    uploadArea.classList.remove("dragover");
    var file = e.dataTransfer.files[0];
    if (file) readFile(file);
  });

  function readFile(file) {
    var reader = new FileReader();
    reader.onload = function (e) {
      $("submit-markdown").value = e.target.result;
      toast("文件已加载：" + file.name, "success");
    };
    reader.readAsText(file);
  }

  // 清空
  $("submit-clear").addEventListener("click", function () {
    $("submit-markdown").value = "";
    $("submit-scope").value = "";
    $("submit-agent").value = "trae";
  });

  // 步骤 1 → 解析预览
  $("parse-btn").addEventListener("click", async function () {
    var md = $("submit-markdown").value.trim();
    if (!md) {
      toast("请先粘贴或上传 run-notes 内容", "error");
      return;
    }
    var agent = $("submit-agent").value.trim() || "trae";
    var scope = $("submit-scope").value.trim();

    $("parse-btn").disabled = true;
    try {
      var payload = { markdown: md, agent: agent };
      if (scope) payload.scope = scope;

      var res = await api("/api/ingestion/parse", {
        method: "POST",
        body: JSON.stringify(payload),
      });

      currentMarkdown = md;

      // 填充预览
      var pvTask = res.task || "（未识别到标题）";
      $("pv-task").textContent = pvTask;

      var stepsEl = $("pv-steps");
      stepsEl.innerHTML = "";
      (res.materials.steps || []).forEach(function (s) {
        var li = document.createElement("li");
        li.textContent = s;
        stepsEl.appendChild(li);
      });
      if (!res.materials.steps || res.materials.steps.length === 0) {
        stepsEl.innerHTML = '<li class="text-muted">（未识别到步骤）</li>';
      }

      $("pv-tools").textContent =
        (res.materials.tools_used || []).join("、") || "（无）";
      $("pv-plugins").textContent =
        (res.materials.plugins_used || []).join("、") || "（无）";
      $("pv-artifacts").textContent =
        (res.materials.artifacts || []).join("、") || "（无）";

      // 证据建议
      var suggested = (res.evidence && res.evidence.suggested) || [];
      if (suggested.length > 0) {
        $("pv-evidence-row").classList.remove("hidden");
        $("pv-evidence").textContent = suggested.join("；");
      } else {
        $("pv-evidence-row").classList.add("hidden");
      }

      showStep("confirm");
    } catch (err) {
      toast("解析失败：" + err.message, "error");
    }
    $("parse-btn").disabled = false;
  });

  // 返回修改
  $("confirm-back").addEventListener("click", function () {
    showStep("input");
  });

  // 步骤 2 → 确认入库
  $("confirm-submit").addEventListener("click", async function () {
    if (!currentMarkdown) {
      toast("内容为空，请重新粘贴", "error");
      return;
    }
    var agent = $("submit-agent").value.trim() || "trae";
    var scope = $("submit-scope").value.trim();

    var diff = $("confirm-diff").value.trim();
    var verifiedLines = $("confirm-verified").value.trim();
    var verifiedFiles = verifiedLines
      ? verifiedLines.split("\n").map(function (s) { return s.trim(); }).filter(Boolean)
      : [];

    var payload = {
      markdown: currentMarkdown,
      agent: agent,
      actor: actorName(),
    };
    if (scope) payload.scope = scope;

    // 附加证据（可选）
    if (diff || verifiedFiles.length > 0) {
      payload.evidence = {};
      if (diff) payload.evidence.diff_summary = diff;
      if (verifiedFiles.length > 0)
        payload.evidence.verified_files = verifiedFiles;
    }

    $("confirm-submit").disabled = true;
    try {
      var res = await api("/api/ingestion/run-notes", {
        method: "POST",
        body: JSON.stringify(payload),
      });

      var trustLabel =
        res.trust_level === "workspace_verified"
          ? "工作区验证"
          : "自行声明";
      var trustClass =
        res.trust_level === "workspace_verified"
          ? "badge-success"
          : "badge-warning";

      var box = $("result-box");
      box.className = "result-box success";
      box.innerHTML =
        "<h3>入库成功</h3>" +
        '<p>参考经验已保存，信任级别：<span class="badge ' +
        trustClass +
        '">' + trustLabel + "</span></p>" +
        "<p class=\"text-muted text-sm\">ID: " + esc(res.reference_id || "") + "</p>";

      showStep("result");
    } catch (err) {
      var errBox = $("result-box");
      errBox.className = "result-box error";
      errBox.innerHTML =
        "<h3>提交失败</h3>" +
        "<p>" + esc(err.message) + "</p>";
      showStep("result");
    }
    $("confirm-submit").disabled = false;
  });

  // 步骤 3 → 提交另一份
  $("result-new").addEventListener("click", function () {
    $("submit-markdown").value = "";
    $("confirm-diff").value = "";
    $("confirm-verified").value = "";
    currentMarkdown = "";
    showStep("input");
  });

  // ── 参考列表 ──
  async function loadReferences() {
    var scope = $("refs-scope").value.trim();
    var tag = $("refs-tag").value.trim();
    var qs = "";
    if (scope) qs += (qs ? "&" : "?") + "scope=" + encodeURIComponent(scope);
    if (tag) qs += (qs ? "&" : "?") + "tag=" + encodeURIComponent(tag);

    var list = $("refs-list");
    var empty = $("refs-empty");
    list.innerHTML = "";
    empty.classList.add("hidden");

    try {
      var data = await api("/api/references" + qs);
      var refs = data.references || [];
      if (refs.length === 0) {
        empty.classList.remove("hidden");
        return;
      }
      refs.forEach(function (ref) {
        list.appendChild(renderRefCard(ref));
      });
    } catch (err) {
      toast("加载参考列表失败：" + err.message, "error");
    }
  }

  function renderRefCard(ref) {
    var card = document.createElement("div");
    card.className = "ref-card";

    var trustBadge =
      ref.trust_level === "workspace_verified"
        ? '<span class="badge badge-success">工作区验证</span>'
        : '<span class="badge badge-warning">自行声明</span>';

    var scopeBadge = ref.scope
      ? '<span class="badge badge-default">' + esc(ref.scope) + "</span>"
      : "";
    var agentBadge = ref.source_agent
      ? '<span class="badge badge-accent">' + esc(ref.source_agent) + "</span>"
      : "";

    var stepsHtml = "";
    if (ref.steps && ref.steps.length > 0) {
      stepsHtml =
        '<div class="ref-card-steps">' +
        '<div class="ref-card-steps-label">步骤</div>' +
        "<ol>" +
        ref.steps
          .map(function (s) { return "<li>" + esc(s) + "</li>"; })
          .join("") +
        "</ol>" +
        "</div>";
    } else if (ref.body) {
      stepsHtml =
        '<div class="ref-card-steps">' +
        '<div class="ref-card-steps-label">运行笔记</div>' +
        '<div class="text-sm">' + esc(ref.body).substring(0, 300) +
        (ref.body.length > 300 ? "…" : "") + "</div>" +
        "</div>";
    }

    var toolsHtml = "";
    if (ref.tools_used && ref.tools_used.length > 0) {
      toolsHtml +=
        '<span>工具: ' +
        ref.tools_used.map(esc).join(", ") +
        "</span>";
    }
    if (ref.plugins_used && ref.plugins_used.length > 0) {
      toolsHtml +=
        '<span>插件: ' +
        ref.plugins_used.map(esc).join(", ") +
        "</span>";
    }

    var evidenceHtml = ref.evidence_summary
      ? '<div class="ref-card-evidence">' + esc(ref.evidence_summary) + "</div>"
      : "";

    var hasSteps = ref.steps && ref.steps.length > 0;

    card.innerHTML =
      '<div class="ref-card-header">' +
      '<div class="ref-card-title">' + esc(ref.title) + "</div>" +
      '<div class="ref-card-badges">' + trustBadge + scopeBadge + agentBadge + "</div>" +
      "</div>" +
      '<div class="ref-card-meta">' +
      '<span>ID: ' + esc(ref.id) + "</span>" +
      (ref.created_at
        ? '<span>' + formatTime(ref.created_at) + "</span>"
        : "") +
      "</div>" +
      stepsHtml +
      (toolsHtml ? '<div class="ref-card-meta">' + toolsHtml + "</div>" : "") +
      '<div class="ref-card-footer">' +
      evidenceHtml +
      '<div class="ref-card-actions">' +
      (hasSteps
        ? '<button class="btn btn-sm btn-primary" data-promote="' +
          esc(ref.id) + '">提升为候选</button>'
        : '<span class="text-xs text-muted">缺少步骤，无法提升</span>') +
      "</div>" +
      "</div>";

    // 绑定 promote 按钮
    if (hasSteps) {
      var btn = card.querySelector("[data-promote]");
      btn.addEventListener("click", function () {
        promoteReference(ref.id, card);
      });
    }

    return card;
  }

  async function promoteReference(id, card) {
    // 检查是否已有结果区
    var existing = card.querySelector(".promote-result");
    if (existing) existing.remove();

    var resultDiv = document.createElement("div");
    resultDiv.className = "promote-result info";
    resultDiv.textContent = "正在提升为候选经验…";
    card.querySelector(".ref-card-footer").appendChild(resultDiv);

    try {
      var res = await api("/api/references/" + id + "/promote", {
        method: "POST",
        body: JSON.stringify({ actor: actorName() }),
      });
      resultDiv.className = "promote-result success";
      resultDiv.textContent =
        "提升成功！候选经验「" +
        (res.candidate || id) +
        "」已创建，可在经验库中查看。";
    } catch (err) {
      resultDiv.className = "promote-result error";
      var msg = err.message;
      if (msg.indexOf("LLM compiler") !== -1) {
        resultDiv.textContent =
          "无法提升：需要先启用 LLM 编译器（设置环境变量 EXPERIENCE_LLM_COMPILER=on 和 EXPERIENCE_LLM_CODEX）";
      } else if (msg.indexOf("缺少 steps") !== -1) {
        resultDiv.textContent = "无法提升：该参考经验缺少步骤信息";
      } else if (msg.indexOf("未能抽出") !== -1) {
        resultDiv.textContent =
          "无法提升：LLM 编译器未能从步骤中抽出可执行体（诚实拒绝）";
      } else {
        resultDiv.textContent = "提升失败：" + msg;
      }
    }
  }

  // ── 工具函数 ──
  function formatTime(unix) {
    var d = new Date(unix * 1000);
    var now = new Date();
    var diff = (now - d) / 1000;
    if (diff < 60) return "刚刚";
    if (diff < 3600) return Math.floor(diff / 60) + " 分钟前";
    if (diff < 86400) return Math.floor(diff / 3600) + " 小时前";
    if (diff < 2592000) return Math.floor(diff / 86400) + " 天前";
    return (
      d.getFullYear() + "-" +
      String(d.getMonth() + 1).padStart(2, "0") + "-" +
      String(d.getDate()).padStart(2, "0")
    );
  }

  // ── 事件绑定 ──
  $("refs-refresh").addEventListener("click", loadReferences);
  $("refs-scope").addEventListener("change", loadReferences);
  $("refs-tag").addEventListener("change", loadReferences);

  $("refresh-all").addEventListener("click", function () {
    loadGuide();
    loadReferences();
  });

  // ── 初始化 ──
  loadGuide();
})();

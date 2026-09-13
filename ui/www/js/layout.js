/* ============================================================
 * layout.js — 侧边栏导航 + 健康检查 共享逻辑
 * 依赖：api.js（api, $ 函数）
 * ============================================================ */

let _healthTimer = null;

/**
 * 页面路径 → 导航 key 的映射
 * 新增页面时在此处注册即可自动高亮
 */
const NAV_ROUTES = [
  { key: "sessions", pattern: /^\/(index\.html)?$/i, label: "任务" },
  { key: "experiences", pattern: /^\/experiences/i, label: "经验库" },
  { key: "absorb", pattern: /^\/absorb/i, label: "经验吸收" },
  { key: "agents", pattern: /^\/agents/i, label: "助手" },
  { key: "audit", pattern: /^\/audit/i, label: "操作记录" },
  { key: "settings", pattern: /^\/settings/i, label: "设置" },
];

/**
 * 初始化侧边栏：
 * 1. 根据当前 URL 高亮对应菜单项
 * 2. 启动健康检查（首次 + 定时 30 秒）
 */
function initLayout() {
  highlightNav();
  bindNavClicks();
  refreshHealth();

  if (_healthTimer) clearInterval(_healthTimer);
  _healthTimer = setInterval(refreshHealth, 30000);
}

/**
 * 根据当前页面路径高亮对应的导航项
 */
function highlightNav() {
  const path = location.pathname || "/";
  const matched = NAV_ROUTES.find((route) => route.pattern.test(path));
  if (!matched) return;

  const navItems = document.querySelectorAll(".app-nav .nav-item");
  navItems.forEach((item) => {
    const isActive =
      item.dataset.nav === matched.key ||
      item.id === `nav-${matched.key}`;
    item.classList.toggle("active", isActive);
  });
}

/**
 * 绑定导航项点击事件
 * - 单页模式（存在 window.showView）：拦截跳转，调用 showView
 * - 多页模式：<a href> 原生跳转，无需额外处理
 */
function bindNavClicks() {
  const navItems = document.querySelectorAll(".app-nav .nav-item");
  navItems.forEach((item) => {
    item.addEventListener("click", (e) => {
      // 单页模式：拦截默认跳转，调用 showView
      if (typeof window.showView === "function") {
        e.preventDefault();
        const key = item.dataset.nav;
        if (key) window.showView(key);
      }
      // 多页模式：让 <a href> 原生跳转，不做任何处理
    });
  });
}

/**
 * 调用 /api/health，更新侧边栏底部状态
 */
async function refreshHealth() {
  const dot = document.getElementById("health");
  const text = document.getElementById("health-text");
  if (!dot || !text) return;

  dot.className = "health-dot checking";
  text.textContent = "检查中…";

  try {
    await api("/api/health");
    dot.className = "health-dot ok";
    text.textContent = "API 正常";
  } catch (error) {
    dot.className = "health-dot bad";
    text.textContent = `API 不可用：${error.message}`;
  }
}

/* ---------- 页面加载时自动初始化 ---------- */
if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", initLayout);
} else {
  initLayout();
}

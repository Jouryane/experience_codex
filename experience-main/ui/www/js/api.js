/* ============================================================
 * api.js — 统一 API 封装 + actor 记忆 + 工具函数
 * 从 app.js 提取的共享基础能力
 * ============================================================ */

/* ---------- API 封装 ---------- */

/**
 * 统一 fetch 封装
 * @param {string} path - 请求路径
 * @param {Object} [options={}] - fetch options
 * @returns {Promise<any>} 解析后的 JSON 数据
 * @throws {Error} 响应失败时抛出，包含服务端返回的 error 信息
 */
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

/* ---------- DOM 工具 ---------- */

/**
 * 按 id 获取元素的简写
 * @param {string} id
 * @returns {HTMLElement | null}
 */
const $ = (id) => document.getElementById(id);

/* ---------- actor 记忆（localStorage） ---------- */

const ACTOR_KEY = "experience_actor";

/**
 * 获取当前 actor 名称
 * @returns {string} 默认为 "user"
 */
function actorName() {
  return localStorage.getItem(ACTOR_KEY) || "user";
}

/**
 * 设置当前 actor 名称
 * @param {string} name
 */
function setActor(name) {
  if (name && name.trim()) {
    localStorage.setItem(ACTOR_KEY, name.trim());
  } else {
    localStorage.removeItem(ACTOR_KEY);
  }
}

/* ---------- HTML 转义 ---------- */

/**
 * 将文本安全地转义为 HTML 字符串，防止 XSS
 * @param {string | null | undefined} text
 * @returns {string}
 */
function esc(text) {
  const div = document.createElement("div");
  div.textContent = text == null ? "" : String(text);
  return div.innerHTML;
}

/* ---------- toast 通知 ---------- */

let _toastContainer = null;

/**
 * 顶部滑入的 toast 提示
 * @param {string} message - 提示内容
 * @param {"info" | "success" | "error"} [type="info"] - 通知类型
 * @param {number} [duration=3000] - 自动消失时间（毫秒）
 */
function toast(message, type = "info", duration = 3000) {
  if (!_toastContainer) {
    _toastContainer = document.createElement("div");
    _toastContainer.style.cssText =
      "position:fixed;top:16px;left:50%;transform:translateX(-50%);" +
      "z-index:9999;display:flex;flex-direction:column;gap:8px;pointer-events:none;";
    document.body.appendChild(_toastContainer);
  }

  const item = document.createElement("div");
  item.textContent = message;

  const baseStyle =
    "padding:8px 16px;border-radius:6px;font-size:13px;" +
    "box-shadow:0 2px 8px rgba(0,0,0,0.12);" +
    "transform:translateY(-20px);opacity:0;" +
    "transition:transform 0.25s ease,opacity 0.25s ease;";

  let bgColor = "#2d3748";
  let textColor = "#fff";
  if (type === "success") {
    bgColor = "#2f855a";
  } else if (type === "error") {
    bgColor = "#c53030";
  }

  item.style.cssText =
    baseStyle + `background:${bgColor};color:${textColor};`;

  _toastContainer.appendChild(item);

  // 触发滑入动画
  requestAnimationFrame(() => {
    item.style.transform = "translateY(0)";
    item.style.opacity = "1";
  });

  // 自动消失
  setTimeout(() => {
    item.style.transform = "translateY(-20px)";
    item.style.opacity = "0";
    setTimeout(() => {
      if (item.parentNode) {
        item.parentNode.removeChild(item);
      }
    }, 250);
  }, duration);
}

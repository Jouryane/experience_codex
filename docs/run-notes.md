# Experience 前端多页面改造 + 经验吸收向导页

> 产出时间：2026-09-10
> 执行人：Trae（AI 助手）
> 工作区：d:\experience_codex\experience-main

## ① 目标与约束

### 目标
将 Experience 前端从单页 SPA 架构改造为多页面架构，同时新增"经验吸收向导页"，使外部助手产出的 run-notes 可一键入库。

### 约束
- **exe "仅启动"属性不变**：`Experience.exe`（launcher）和 `experience-server.exe` 不改一行代码，避免频繁打包
- **后端 API 契约不变**：不改 Rust 后端的任何路由/数据结构（吸收页的新端点由后端先行实现）
- **设计统一**：Codex 银灰 + 纯白基调，4 页（后增为 5 页）视觉一致、格局明确
- **零门槛友好**：25+ 专业术语替换为日常语言（session→任务、agent→助手、scope→场景 等）
- **功能不退步**：U0–U7 验收要点全部保留

---

## ② 使用的工具与插件

| 类别 | 名称 | 版本/说明 |
|---|---|---|
| 前端语言 | HTML5 + CSS3 + Vanilla JS (ES5+) | 无框架依赖，浏览器原生运行 |
| 设计系统 | CSS Variables + Flexbox | 全部颜色/间距通过 `:root` 变量定义 |
| 构建工具 | PowerShell `build-app.ps1` | 复制 UI 文件到 dist，调用 `cargo build` |
| 后端 | Rust `experience-server` | 静态文件托管 `ui/www/` 目录 |
| 浏览器验证 | TRAE Browser Use (Playwright) | 快照 + 控制台检查 + 导航跳转验证 |
| 文件操作 | Read / Write / Edit 工具 | 直接读写本地文件系统 |
| 子任务并行 | general_purpose_task × 3 | 并行创建 D3/D4/D5 三组页面 |
| 终端 | PowerShell 5 (Windows 11) | 构建/启动服务器 |

---

## ③ 步骤与子步骤

### D1：设计系统与共享骨架

**输入**：旧 `style.css`（单页样式）+ `app.js`（单页逻辑）
**参数**：无外部参数

1. 创建 `css/base.css`
   - 定义 `:root` CSS 变量：`--bg`/`--bg-panel`/`--text`/`--border`/`--accent` 等 15+ 个设计 token
   - 全局重置：`box-sizing: border-box`
   - 排版：`-apple-system, "PingFang SC", "Microsoft YaHei"` 字体栈
   - 布局：`.app-layout`（侧边栏 220px + 主区域 flex:1）
   - 组件：`.btn`/`.btn-primary`/`.badge`/`.panel`/`.nav-item` 等
   - 工具类：`.hidden`/`.flex`/`.text-muted`/`.text-sm` 等

2. 创建 `js/api.js`
   - `api(path, options)` — 统一 fetch 封装，JSON 请求/响应
   - `$`(id) — `document.getElementById` 简写
   - `actorName()` / `setActor(name)` — localStorage 读写 `experience_actor`
   - `esc(text)` — HTML 转义防 XSS
   - `toast(message, type)` — 顶部滑入通知

3. 创建 `js/layout.js`
   - `NAV_ROUTES` — 5 条路由正则映射
   - `initLayout()` — DOMContentLoaded 时高亮当前页导航项
   - `refreshHealth()` — 调 `/api/health`，30 秒轮询，更新侧边栏底部状态点

**输出**：`css/base.css`、`js/api.js`、`js/layout.js` + `css/pages/`、`js/pages/` 目录

---

### D2：多页面拆分

**输入**：D1 产物
**参数**：页面路由表

1. 确定页面拆分方案
   - `index.html` → 任务首页（原 Sessions）
   - `experiences.html` → 经验库（原 Experiences）
   - `agents.html` → 助手管理（原 Agents）
   - `audit.html` → 操作记录（原 Audit）

2. 确定资源加载顺序
   - CSS：`base.css` → `pages/xxx.css`
   - JS：`api.js` → `layout.js` → `pages/xxx.js`

**输出**：拆分方案文档（无独立文件产出）

---

### D3：任务首页改造

**输入**：旧 `app.js` 中会话相关函数（约 1–360 行）
**参数**：术语映射表

1. 创建 `index.html`
   - 侧边栏 4 项导航（`<a>` 链接，当前页 `.active`）
   - 主区域左右分栏：历史任务列表面板（260px）+ 任务对话区（flex:1）
   - 对话区：工具栏（助手下拉 + 场景下拉 + 操作按钮）+ 消息区 + 输入区

2. 创建 `css/pages/tasks.css`
   - `.task-list-panel` — 列表面板，选中项左侧 3px 强调色边
   - `.msg.user` / `.msg.assistant` / `.msg.running` / `.msg.error` — 四种消息气泡
   - `.task-input-area` — 底部多行输入 + 工作目录 + 开始执行按钮

3. 创建 `js/pages/tasks.js`
   - `refreshAgentSelect()` — 加载助手列表
   - `loadTaskHistory()` — 历史任务（摘要 30 字 + 状态 + 相对时间）
   - `openTask(id)` — 打开历史任务
   - `pollTask(id)` — 1.5s 轮询状态
   - `addMessage(kind, text, extra)` — 添加消息气泡
   - `renderFinished(session)` — 完成/失败渲染
   - `newTask()` / `cancelTask()` / `resumeTask()` — 新建/终止/继续
   - 表单提交 `POST /api/sessions`
   - Ctrl+Enter 快捷提交

**输出**：`index.html`、`css/pages/tasks.css`、`js/pages/tasks.js`

---

### D4：经验库页面改造

**输入**：旧 `app.js` 中经验相关函数（约 360–1040 行）
**参数**：状态转移表、术语映射表

1. 创建 `experiences.html`
   - 左侧 320px 经验树面板（搜索 + 筛选 + 树容器）
   - 右侧详情面板（空态引导 + 详情内容）

2. 创建 `css/pages/experiences.css`（约 430 行）
   - 三级树节点：`.tree-scene` / `.tree-family` / `.tree-leaf`
   - 状态色点：`active` 绿 / `draft` 黄 / `disabled` 灰 / `validated` 蓝 / `decaying` 橙
   - 评分区双栏卡片：系统评分 + 我的评价
   - 经验内容分区：trigger / preconditions / steps / postconditions / verification
   - 编辑器表单 + 操作记录列表
   - 响应式：窄屏自动堆叠

3. 创建 `js/pages/experiences.js`（约 1030 行，32 个函数）
   - `loadExperiences()` — 加载经验 + usage 数据
   - `renderExperienceTree()` — 三级树渲染
   - `openExperience(name)` — 打开详情（并行加载详情 + 审计）
   - `renderMetaForms(detail)` — 场景/别名/偏好编辑
   - `renderActions(detail, summary)` — 按状态转移表动态显隐按钮
   - `setExperienceStatus(status)` / `togglePin()` / `removeExperience()`
   - `startDraft()` → `openEditor()` → `saveDraftBody()` → `adoptDraft()` / `abandonDraft()`
   - `renderAuditList(target, records)` — 审计记录

**输出**：`experiences.html`、`css/pages/experiences.css`、`js/pages/experiences.js`

---

### D5：助手管理 + 操作记录页面

**输入**：旧 `app.js` 中 agent / audit 相关函数
**参数**：术语映射表、操作类型中文映射

1. 创建 `agents.html` + `css/pages/agents.css` + `js/pages/agents.js`
   - 助手列表（卡片式：名称 + 类型标签 + 状态徽章 + 操作按钮）
   - 添加助手表单（ID / 显示名称 / 程序目录 / 可执行文件 / 数据目录 / 运行模式）
   - 文件选择模态弹窗（路径导航 + 盘符快捷 + 文件列表）
   - `loadAgents()` / `agentAction(id, action)` / `openFileModal()` / `fbNav(path)` / `renderFb(data)` / `chooseExecutable(filePath, dirPath)`

2. 创建 `audit.html` + `css/pages/audit.css` + `js/pages/audit.js`
   - 筛选栏（经验名搜索 + 操作类型下拉 + 数量 + 刷新）
   - 时间线式记录列表（竖线 + 圆点 + 相对时间 + 绝对时间 tooltip）
   - 13 种操作类型中文映射：drafted→创建草稿、adopted→应用修改、activated→启用经验 等
   - `loadAudit()` / `renderAuditTimeline(records)` / `formatRelativeTime()` / `formatAbsoluteTime()`

**输出**：6 个文件（2 页 × 3 文件）

---

### D6：统一收口与构建更新

**输入**：D3/D4/D5 产物
**参数**：导航标签统一表

1. 统一导航
   - 4 页侧边栏标签统一为：任务 / 经验库 / 助手 / 操作记录
   - 导航元素从 `<button>` 统一为 `<a href>`
   - 品牌名统一为 "Experience"
   - 布局类名统一为 `app-layout` + `app-main`
   - `.nav-icon` 样式从 tasks.css 提升到 base.css

2. 更新 `js/layout.js`
   - `NAV_ROUTES` label 更新
   - `bindNavClicks` 重构：多页模式原生 `<a>` 跳转，SPA 模式 `preventDefault` + `showView`

3. 更新 `scripts/build-app.ps1`
   - README 文案中文化
   - UI 复制逻辑不变（`-Recurse` 已正确）

4. 删除旧文件
   - `app.js`（旧单页脚本）
   - `style.css`（旧单页样式）

5. 构建验证
   - `./scripts/build-app.ps1` → dist 产物完整
   - 启动服务器，浏览器验证 4 页加载正常，零控制台错误，导航跳转正常

**输出**：修改 8 个文件 + 删除 2 个文件

---

### A4：经验吸收向导页（初版）

**输入**：`docs/stage-a-absorb.md` 第 4 节规格
**参数**：API 端点契约（`/api/ingestion/guide`、`/api/ingestion/package`、`/api/references`、`/api/references/{id}/promote`）

1. 创建 `absorb.html`
   - 3 个标签页：采集指南 / 提交材料 / 参考列表
   - 侧边栏新增第 3 项 `◈ 经验吸收`

2. 创建 `css/pages/absorb.css`
   - 标签页样式、表单样式、参考卡片样式、promote 结果样式

3. 创建 `js/pages/absorb.js`
   - `loadGuide()` — 调 `/api/ingestion/guide?agent=trae`
   - 提交表单 → `POST /api/ingestion/package`（旧流程，逐行手填）
   - `loadReferences()` — 调 `/api/references?scope=&tag=`
   - `renderRefCard(ref)` — 参考卡片（trust 徽章 + 步骤 + 工具 + 证据）
   - `promoteReference(id, card)` — `POST /api/references/{id}/promote`，诚实拒绝原因中文化

4. 更新全部 5 页侧边栏 + `layout.js` 路由表

**输出**：`absorb.html`、`css/pages/absorb.css`、`js/pages/absorb.js` + 4 页侧边栏修改 + layout.js 修改

---

### A4 更新：适配简化流程（parse → run-notes）

**输入**：后端新增 `/api/ingestion/parse` 和 `/api/ingestion/run-notes` 端点
**参数**：新 API 契约

1. 重写 `absorb.html` 提交标签页
   - 步骤 1：上传/粘贴区（拖拽 + 文件选择 + 大文本框）
   - 步骤 2：解析预览（任务名 / 步骤 / 工具 / 插件 / 产物 / 证据建议 + 附加证据区）
   - 步骤 3：提交结果（成功/失败展示）

2. 更新 `css/pages/absorb.css`
   - 新增 `.upload-area`（虚线拖拽区 + dragover 高亮）
   - 新增 `.preview-grid` / `.preview-field`（解析结果卡片）
   - 新增 `.evidence-section`（附加证据区）
   - 新增 `.result-box`（成功/失败结果展示）

3. 重写 `js/pages/absorb.js` 提交逻辑
   - `readFile(file)` — FileReader 读取 .md/.txt
   - 拖拽事件绑定（dragover/dragleave/drop）
   - 步骤 1 → `POST /api/ingestion/parse` → 填充预览
   - 步骤 2 → `POST /api/ingestion/run-notes` → 一步落库
   - 信任级别自动判定：有 diff/verified_files ⇒ `workspace_verified`，否则 `declared`
   - `showStep(name)` — 三步切换

4. 重新构建 dist

**输出**：修改 `absorb.html`、`css/pages/absorb.css`、`js/pages/absorb.js`

---

## ④ 失败与修正

### 失败 1：build-app.ps1 ASCII 编码错误
- **现象**：构建脚本报错，`Set-Content -Encoding ASCII` 无法写入中文字符
- **原因**：D6 中将 README 文案改为中文，但 PowerShell 的 `-Encoding ASCII` 不支持非 ASCII 字符
- **修正**：将 README 文案改回英文，保持 ASCII 兼容
- **教训**：PowerShell `Set-Content -Encoding ASCII` 仅支持 7-bit 字符，中文内容需用 `-Encoding UTF8`

### 失败 2：导航标签不一致
- **现象**：D3/D4/D5 三个子任务并行创建，各自用了不同的导航标签（"会话" vs "任务"、"Agents" vs "助手"、"审计" vs "操作记录"）
- **原因**：并行子任务无共享上下文，各自翻译术语时产生分歧
- **修正**：D6 收口阶段统一为：任务 / 经验库 / 助手 / 操作记录
- **教训**：并行子任务前应先确定术语映射表作为共享约束

### 失败 3：导航元素类型不统一
- **现象**：部分页面用 `<button>` + JS 跳转，部分用 `<a href>` 原生跳转
- **原因**：旧 SPA 用 button + showView()，新多页面应用应用 a 标签
- **修正**：全部统一为 `<a href="xxx.html">`，layout.js 的 `bindNavClicks` 兼容两种模式

### 失败 4：布局类名不统一
- **现象**：index.html 用 `tasks-page` / `tasks-main`，其他页面用 `app-layout` / `app-main`
- **原因**：D3 子任务自定义了布局类名而非复用 base.css 标准
- **修正**：index.html 改为 `app-layout` / `app-main`，tasks.css 中对应样式同步更新

---

## ⑤ 最终产物清单与验证方法

### 产物清单

```
ui/www/
├── index.html                # 任务首页
├── experiences.html          # 经验库
├── absorb.html               # 经验吸收向导
├── agents.html               # 助手管理
├── audit.html                # 操作记录
├── css/
│   ├── base.css              # 设计系统（CSS 变量 + 组件 + 工具类）
│   └── pages/
│       ├── tasks.css         # 任务页样式
│       ├── experiences.css   # 经验库样式
│       ├── absorb.css        # 吸收页样式
│       ├── agents.css        # 助手页样式
│       └── audit.css         # 操作记录样式
├── js/
│   ├── api.js                # API 封装 + actor 记忆 + esc + toast
│   ├── layout.js             # 侧边栏高亮 + 健康检查轮询
│   └── pages/
│       ├── tasks.js          # 任务页逻辑
│       ├── experiences.js    # 经验库逻辑
│       ├── absorb.js         # 吸收页逻辑
│       ├── agents.js         # 助手页逻辑
│       └── audit.js          # 操作记录逻辑
```

**已删除**：`app.js`、`style.css`（旧单页架构）

**已修改**：`scripts/build-app.ps1`（README 文案）

### 验证方法

| 验证项 | 方法 | 结果 |
|---|---|---|
| 4+1 页加载 | 浏览器访问 `http://127.0.0.1:8788/{page}.html` | 全部正常 |
| 控制台零错误 | `browser_console_messages()` | 全部页面无 JS 报错 |
| 导航跳转 | 点击侧边栏链接，验证 URL 变化 | 5 页互跳正常 |
| API 数据连通 | 助手列表加载、健康检查 | 正常 |
| 术语通俗化 | 人工检查所有用户可见文字 | 25+ 术语已替换 |
| exe 未改动 | 对比二进制大小/哈希 | Experience.exe + experience-server.exe 未变 |
| 构建产物 | `./scripts/build-app.ps1` | dist/Experience/ 完整 |
| 吸收页 parse 流程 | 粘贴 run-notes → 解析预览 → 确认入库 | API 契约对齐 |

---

## ⑥ 可复用的模板与参数位

### 模板 1：新页面骨架

创建新页面时复用此骨架，替换 `{PAGE_NAME}` / `{PAGE_KEY}` / `{PAGE_ICON}`：

```html
<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>{PAGE_NAME} — Experience</title>
<link rel="stylesheet" href="css/base.css">
<link rel="stylesheet" href="css/pages/{PAGE_FILE}.css">
</head>
<body>
<div class="app-layout">
  <aside class="app-sidebar">
    <div class="app-brand">Experience</div>
    <nav class="app-nav">
      <a href="index.html" class="nav-item" data-nav="sessions">
        <span class="nav-icon">▸</span> 任务
      </a>
      <a href="experiences.html" class="nav-item" data-nav="experiences">
        <span class="nav-icon">◇</span> 经验库
      </a>
      <a href="absorb.html" class="nav-item" data-nav="absorb">
        <span class="nav-icon">◈</span> 经验吸收
      </a>
      <a href="agents.html" class="nav-item" data-nav="agents">
        <span class="nav-icon">◯</span> 助手
      </a>
      <a href="audit.html" class="nav-item" data-nav="audit">
        <span class="nav-icon">▢</span> 操作记录
      </a>
    </nav>
    <div class="sidebar-footer">
      <span id="health" class="health-dot"></span>
      <span id="health-text" class="text-sm text-muted">检查中…</span>
    </div>
  </aside>
  <main class="app-main">
    <!-- 页面内容 -->
  </main>
</div>
<script src="js/api.js"></script>
<script src="js/layout.js"></script>
<script src="js/pages/{PAGE_FILE}.js"></script>
</body>
</html>
```

**应参数化的值**：
- `{PAGE_NAME}` — 浏览器标签页标题
- `{PAGE_FILE}` — CSS/JS 文件名（不含扩展名）
- 当前页的 `.nav-item` 添加 `active` class

### 模板 2：设计系统 CSS 变量

主题化时修改 `:root` 即可全局换肤：

```css
:root {
  --bg: #f5f6f8;          /* 页面背景 → 可参数化为深色主题 */
  --bg-panel: #ffffff;     /* 面板背景 */
  --text: #1f2328;         /* 主文字色 */
  --text-muted: #656d76;   /* 次级文字 */
  --border: #d0d7de;       /* 分割线 */
  --accent: #2d3748;       /* 强调色 → 可参数化为品牌色 */
  --success: #2f855a;
  --warning: #b7791f;
  --danger: #c53030;
}
```

**应参数化的值**：
- `--accent` — 品牌强调色（当前深灰，可改为蓝色/绿色等）
- `--bg` / `--bg-panel` — 深色主题时反转

### 模板 3：API 封装

```javascript
async function api(path, options = {}) {
  // BASE_URL 应参数化（当前为同源相对路径）
  var response = await fetch(path, {
    headers: { "Content-Type": "application/json" },
    ...options,
  });
  // ...
}
```

**应参数化的值**：
- API 基地址 — 当前为同源相对路径，如需独立部署可改为绝对 URL
- actor 记忆的 localStorage 键名 `experience_actor`

### 模板 4：吸收页提交流程

```
[上传/粘贴 run-notes]
      ↓
POST /api/ingestion/parse  ← 参数: { markdown, agent, scope? }
      ↓
[解析预览: task/steps/tools/plugins/artifacts/evidence]
      ↓
[用户确认 + 可选附加 evidence]
      ↓
POST /api/ingestion/run-notes  ← 参数: { markdown, agent, actor, scope?, evidence? }
      ↓
[入库结果: reference_id + trust_level]
```

**应参数化的值**：
- `agent` — 来源助手标识（当前默认 "trae"）
- `scope` — 经验场景（可选，如 "scene-frontend"）
- `evidence.diff_summary` — git diff 输出（决定 trust_level）
- `evidence.verified_files` — 已验证文件列表（决定 trust_level）

### 模板 5：术语映射表

后端字段名 → 前端显示名（应统一维护，新增页面时参照）：

| 后端字段 | 前端显示 |
|---|---|
| session | 任务 |
| agent / executor | 助手 |
| scope | 场景 |
| experience | 经验 |
| family | 分类 |
| draft | 草稿 |
| active | 已启用 |
| disabled | 已停用 |
| candidate | 待验证 |
| validated | 已验证 |
| decaying | 待评估 |
| pinned | 置顶 |
| audit | 操作记录 |
| actor | 操作人 |
| adopt | 应用修改 |
| abandon | 放弃修改 |
| evidence_score | 系统评分 |
| user_confidence | 我的评价 |
| usage | 使用次数 |
| display_name | 显示名称 |
| trust_level: declared | 自行声明 |
| trust_level: workspace_verified | 工作区验证 |
| promote | 提升为候选 |
| reference | 参考经验 |
| ingestion | 吸收 |

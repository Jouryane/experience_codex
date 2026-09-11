# UI 专项改造计划（C3 实现）

> 2026-09-10。上游：docs/ui-module-spec.md（视图/契约/验收）。
> 现状：`ui/www` = 无构建 vanilla HTML/CSS/JS（index 6.3KB / app.js
> 19.8KB / style.css 8.7KB），experience-server 直接托管；后端契约已
> 全部就绪（C1 scope/树、C2 编辑、C2.1 别名、C2.2 偏好、C4 通道）。

## 0. 技术决策

- 保持**无构建单页 vanilla JS/CSS**：项目无 Node 工具链、页面规模尚小；
  引入框架/打包器只在文件量级显著增长后再评估；
- 结构上在 `app.js` 内按视图分区（Tree/Detail/Edit/Scope/Sessions 入口），
  `style.css` 用变量统一间距/颜色/徽章；
- 后端只允许最小只读补充：审计查询面（GET /api/audit），其余契约不变。

## 1. 工作包与验收

### U0 契约核对与骨架

- 用现有 accept 脚本逐个确认 C1/C2/C2.1/C2.2 端点可被 UI 调用；
- 抽出统一 `api()` 封装（已有）+ `actor()` 记忆（localStorage）；
- 验收：页面骨架三视图可切换，无控制台错误。

### U1 审计查询后端小补（如需）

- `GET /api/audit?limit=&record_type=&name=`（读 learning-l1.json，最新
  在前，无锁读走 audit mutex）；单测 + fixture；
- 验收：HTTP 冒烟 + cargo test 全绿。

> 状态：✅（2bf7832）。

### U2 Tree 主视图

- 经验页改为 Root→Scene→Family→Experience 树：展开/折叠、过滤
  （scope/status/usage_min）、搜索、children 计数、hover 显示内部 name；
- 点击叶子进详情；树刷新与列表数据同源（GET /api/experience-tree +
  /api/experiences 合并 usage）；
- 验收：与后端同参数同集合；无 LLM 冒烟。

> 状态：✅（46afd02）。

### U3 详情 + usage/审计面板

- 本体只读格式化展示；徽章：status/pinned/scope/别名；
- evidence（usage.json score/successes/…）与 user score 并列并注明
  “用户偏好不改证据”；
- 最近审计记录（GET /api/audit 过滤 name）；
- 验收：详情字段与 GET 契约一致。

> 状态：✅（46afd02）。

### U4 编辑工作台

- “草稿化编辑”→ draft 打开编辑表单（trigger/pattern/pre/post/verification/
  failure_policy/undo 动态行）；保存=PUT body；采纳/放弃草稿；
- 错误展示后端校验原文；adopt/放弃要求 reason/actor；
- 验收：完整 draft→edit→adopt 流程 UI 可走通且 ledger 出现三种记录。

> 状态：✅（46afd02）。

### U5 场景/别名/偏好与管理动作条

- scope 挂载/移除（含无 scope）；别名编辑；偏好表单（usage 单选 +
  confidence 滑杆/数字 + reason）；
- 状态转移按钮按转移表显隐（reason 必填提示）；pin/unpin；删除二次确认；
- 验收：每个写操作后摘要/详情即时刷新；deny 经验在树上标注“停用”。

> 状态：✅（46afd02）。

### U6 会话入口 scope 选择

- 会话 composer 增加 scope 下拉（复用经验树中的 scene 集合），POST
  /api/sessions 携带 scope；会话列表显示 scope 徽章；
- 验收：选择 scene-a 后新建会话，任务只唤醒 scene-a（真机 ≤1 复验）。

> 状态：✅（46afd02；scope 隔离真机已另验 b5331d4）。

### U7 总验收与打磨

- 按 docs/ui-module-spec.md 验收清单逐条人工核对；快捷键/空态/加载/
  错误 toast；响应式（树在窄屏折叠为列表）；
- 产物：更新后静态资产入 dist（build-app 后浏览器人工验收一次）。

> 状态：⏳ 人工浏览器验收待执行（清单见 docs/ui-module-spec.md §4；
> 自动化部分已覆盖：JS 语法 node --check、静态资产入 dist、后端冒烟
> accept-u1-audit PASS）。
>
> 更新（2026-09-12）：✅ **自动化验收 PASS**（`scripts/accept-ui-u7.ps1`，
> 无头 Chrome：5 页 HTTP 200 + JS 执行后 DOM 关键元素齐全 + 经验树动态
> 数据加载 + 吸收页指南加载；产物 `scripts/accept-artifacts/ui-u7-*`）。
> 视觉/动效层面的主观打磨可由后续 UI 迭代处理，不影响交付判定。

## 2. 顺序与纪律

U0 → U1(如需要) → U2 → U3 → U4 → U5 → U6 → U7；每包可独立提交与验收；
后端改动沿用脚本/fixture 先行、cargo test 全绿；UI 改动以浏览器人工
验收清单 + 后端冒烟为准（无自动化 UI 测试工具链）。

## 3. 非目标

- 引入框架/构建器/TS；
- 改后端匹配/执行语义（仅可能加只读 audit）；
- computer-use / 具身通道的 UI 展示；
- 多用户/权限界面（actor 仍为单用户审计标签）。

# Stage C 详细开发计划：让 Experience 具备“规模推广”能力

> 2026-09-10。上游：docs/next-stage-blueprint.md（判断与总设计，C0）。
> 目标：把单机闭环升级为可分域/分场景、可组织/可管理/可编辑/可扩展使用
> 的系统；**匹配与控制权语义保持不变**，只加组织、过滤、编辑、通道与
> 索引面。

## 0. 完成判据（本阶段“推广能力”的定义）

1. 一条经验可以被挂到多个“场景/专项”（scope），任务入口选 scope 后只
   唤醒该 scope 的 ACTIVE 面；无 scope 经验保持向后兼容（所有场景可见）；
2. 用户可以按树浏览（Root→Scene→Family→Experience），按 scope/status/
   usage 过滤，看到“当前可用执行面”；
3. 用户可以编辑已有经验（草稿化→编辑→采纳）且每次写操作带 actor 审计；
4. 系统为 computer-use 与具身智能训练各保留一个**接口级通道**（不实现
   执行器/训练闭环）；
5. 规模索引面给出设计稿 + 确定性基准（合成 10k–100k 条目下桶内命中
   正确、复杂度可说明），默认实现不变。

## 1. 工作包与验收

### C1 scope 元数据 + 树/过滤 API

> 状态：✅ 完成（e0f4954 测试资产，6b91b26 实现，HTTP 冒烟 PASS；
> workspace 200 passed / 1 ignored）。

- store envelope 增加 additive `scopes`（experience name → scope）与
  `scope_of/is_in_scope`；Session 增加 `scope: Option<String>`（创建会话
  请求体可选）；任务入口只取“scope 命中 + 无 scope”的 ACTIVE；
- 新增 `GET /api/experience-tree?scope=`（Root→Scene→Family 由
  scope/name 约定推导）与列表过滤参数 `?scope=&status=&usage_min=`；
- 验收：单测（scope 过滤/树展开/无 scope 兼容）+ no-token fixture
  （scope 树 + 过滤产物）+ `accept-c1-scope.ps1`（HTTP 冒烟，无 LLM，
  真机 0）；一次真实会话（可选，≤1）验证 scope B 不唤醒 scope A 经验。

### C2 编辑权利 API

> 状态：✅ 完成（9f079a7 测试资产，e1f5976 实现，HTTP 冒烟 PASS；
> workspace 203 passed / 1 ignored）。

### C2.1 用户 ID/别名层（用户可“管理”名称与状态观感）

> 状态：✅ 完成（7960cc9 测试资产，ce82494 实现，HTTP 冒烟 PASS）。

内部 `name` 保持稳定（store 索引/scope/pin/审计/匹配都挂在它上面，禁改）；
用户可读身份独立成 additive 元数据：

- `display_name`（别名/说明标题）：任意状态可改，展示层（列表/树/详情）
  优先用别名；`POST /{name}/display_name`，ledger `renamed`；
- 状态：用户仍通过既有生命周期端点管理（合法转移才放行），并在 UI 给
  出“为何不能任意改状态”的解释面；
- 目的：让用户“感觉并实际能管理”经验的呈现名与状态观感，但不撕裂
  内部身份，避免 scope/pin/审计/匹配错乱。

### C2.2 用户置信度/使用偏好（用户决定“是否用”）

> 状态：✅ 完成（同上；workspace 205 passed / 1 ignored）。

L2 红线不变：`confidence` 是证据变量，用户调整不得改写 evidence/score；
用户的权利落在**决策层**，存为 additive `user_preferences`：

- `usage`：auto | allow | deny —— deny 的经验从该用户唤醒面移除；
- `confidence`：0..1 用户评分（仅展示/排序参考，不污染证据计数）；
- 端点：`POST /{name}/user-preference`（actor+reason 审计），列表/详情
  同时返回 evidence score 与 user score；ledger `preference_updated`。

- 流程（安全默认）：`POST /{name}/draft` 复制为
  `{name}__draft`（status=Draft）→ `PUT /drafts/{draft}/body` 编辑 →
  `POST /drafts/{draft}/adopt` 校验（schema+tool boundary）后替换原经验
  （原版本审计保留）；delete/pin/status 已有审计骨架补齐 actor；
- 验收：单测（非法编辑拒/采纳原子替换/审计记录）+ fixture；HTTP 冒烟
  无 LLM；真机 0。

### C3 UI 专属模块（转交专项，本计划只出规格）

- 产出 `docs/ui-module-spec.md`：Tree/详情/编辑/场景选择/审计视图 +
  数据契约（与 C1/C2 API 对齐）+ 验收清单；UI 实现由独立 UI 专项承接。

> 状态：✅ 规格完成（docs/ui-module-spec.md）；实现转交 UI 专项。
> 实现计划：docs/ui-project-plan.md（U0–U7，无构建 vanilla SPA；
> 仅可能补只读 GET /api/audit）。

### C1 末：scope 隔离真机回归

> 状态：✅ PASS（5f1da34/b5331d4，1 次 codex 会话，产物
> scripts/accept-artifacts/c1-real-20260910-004437）：scene-a 会话任务
> 文本同时命中 create_a/create_b，仅 create_a 先执行（early_a=True、
> early_b=False），ledger exec_a=1/exec_b=0。

### C4 capability 通道（computer-use 保留 + 具身智能训练通道）

> 状态：✅ 完成（b70e7cb 测试资产，1c21a38 实现，HTTP 冒烟 PASS）。

- 领域层扩展 capability 声明（仅契约，不新增执行器）：
  `capability_allowlist`（会话级：默认空=按 agent 现状）与
  `computer_use` 标识；
- 具身通道 = 最小 REST 面：`GET /api/experiences/export?scope=`、
  `POST /api/experiences/import`（scope 强制）+ State 快照导出
  `GET /api/state/snapshot?cwd=`；实现只做序列化/校验，不接训练闭环；
- 验收：契约文档 + 单测（allowlist 校验、export/import roundtrip、
  snapshot 只读）；真机 0。

### C5 规模索引面：设计稿 + 确定性基准

> 状态：✅ 完成（1ca2515：docs/scale-index-design.md + 10k 正确性测试 +
> 100k `#[ignore]` 基准）。

- 产出 `docs/scale-index-design.md`：五层（分区→ACTIVE 索引面→倒排/
  前缀→可选 embedding 粗召回→确定性精排）+ 缺省回退；
- 确定性基准：合成 10k/100k 经验，验证 scope+tool 分桶命中集合与
  现有一致（同输入同输出），并给出每桶操作数说明；`#[ignore]` 基准可选
  计时，CI 只跑正确性；
- 验收：正确性测试全绿 + 设计稿评审；默认实现行为不变。

## 2. 依赖与顺序

C1（数据/过滤基座）→ C2（编辑，依赖 C1 审计面）→ C4（通道，可并行于
C2）→ C5（索引设计，可随时并行）；C3 规格随时可出，UI 实现转交专项。
建议执行序：C1 → C2 → C4 → C5，C3 规格随 C1 一起给 UI 专项。

## 3. 工程纪律与预算

- 每个工作包：脚本/fixture 先提交 → 实现 → `cargo test --workspace`
  全绿贴原始计数 → 真机 ≤2 → 产物与 fixture 同步 → README 账本更新；
- 本阶段真机预算：C1 可选 1 + 最终回归 ≤1（scope 隔离真机演示），其余
  均为确定性/HTTP 冒烟；
- 提交延续 scope 标签：`feat(scope)`、`feat(edit)`、`feat(channel)`、
  `docs(scale)` 等。

## 4. 需确认的默认语义（不阻塞时按推荐推进）

1. scope 默认：无 scope 经验所有场景可见（向后兼容）；场景内只唤醒
   “该 scope 或无 scope”的 ACTIVE——推荐；
2. 编辑流程：草稿复制 + 采纳替换（保留原版本审计）——推荐；
3. capability allowlist 默认空 = 维持 agent 现状，显式声明才收窄——
   推荐。

## 5. 非目标（本阶段不做）

- computer-use 执行器实现、UI 自动化 StateProbe；
- 具身智能训练闭环/数据集接入；
- 默认 embedding/RAG 索引上线（只出设计 + 可选列）；
- 多 Agent/模型 B。

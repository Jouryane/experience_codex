# M2 实施计划（精确版）

> 状态：计划定稿（2026-09-02）
> 前置：M1 里程碑（阶段 1-9 逻辑初步完成，60 单测 + 全量 `cargo check -p codex-core` exit 0 + 启动测试通过）
> 目的：把 M2 从清单变为可执行的逐项规格，coding 过程中逐项对照，避免偏移。

> **实施状态**：M2-1 ✅、M2-2 ✅、M2-3 ✅、M2-4 ✅、M2-5 ✅（store 持久化
> 接入 runtime 生命周期：`<codex_home>/experience/store.json`，注册/反馈/
> 学习/迁移/pin/tick 后自动落盘，Session 启动加载；75 单测 + `cargo check
> -p codex-core` exit 0）。**M2-3b ✅**（`codex experience` 命令组：list /
> pin / unpin / disable / path，经 codex-core `experience_management` 公共
> 门面读写同一 store 文件；`cargo check -p codex-cli` exit 0）。
> M2-6 未开始。

## 0. 目标与三条纪律

M2 的本质不是新算法，而是**让经验系统真正活在 agent 循环里**。三条纪律贯穿所有工作项：

1. **④ Delegate 路径与基线逐位一致**——任何改动不得让"无经验时"的 LLM 行为漂移；
2. **同一循环承载四种形态**（纯 LLM / 经验+LLM 检查 / 经验参考 / 纯经验）——门禁不得在某种形态下泄漏或误判；
3. **循环内状态真实连续**——LLM 介入时读到的 task / executed_steps / deployed 必须是真实账本。

## 1. 范围

**In scope（6 项）**：

| 编号 | 项 | 一句话目标 |
|---|---|---|
| M2-1 | trace 采集接线 | LLM 成功 turn → ExperienceTrace → learner → Candidate 入库 |
| M2-2 | Session 级状态持久 + execution_mode 写回 | 状态跨 turn 连续；决策 → 行为模式写回 |
| M2-3 | freshness tick | StateElement TTL 过期 → 重探测/陈旧 → DECAYING 联动 |
| M2-4 | 结果回显 | ①/② 零 LLM 完成后用户能看到经验执行结果 |
| M2-5 | store 后端升级 | 内存 → 持久化（首选 SQLite，降级 JSON 文件） |
| M2-6 | 阶段 10 MCP 子集 | 外部 Agent 只读访问 Experience + 提交反馈 |

> 不变量（2026-09-02 声明）：M2-6 的 MCP 仅是**外部访问适配器**（供外部
> Agent / 管理 UI 使用），**绝不进入 agent 内部决策路径**；agent 使用
> Experience 只走原生 runtime（见 experience-runtime-design.md §0）。

**Out of scope**：经验编译器增强（LLM 生成经验结构）、多用户/多会话共享、真实沙箱工具动作的全覆盖、性能优化。

## 2. 工作项规格

### M2-1 trace 采集接线

- **目标**：原版"LLM 是学习器"从逻辑变为真实运行。LLM 实际参与的 turn 正常结束后，把该 turn 的工具调用序列编译为 `ExperienceTrace` 交给 learner。
- **现状**：`ExperienceLearner` / `observe_trace` 已实现并有单测；但没有真实 trace 来源。
- **改动文件**：
  - `session/turn.rs`：turn 级维护 `Vec<TraceAction>`；
  - `session/turn.rs::try_run_sampling_request`（流事件循环）：在 FunctionCall 事件分支**只读记录** `(name, arguments)`（不改变执行路径）；
  - `session/turn.rs::run_turn` 结束处：`needs_follow_up=false` 且无错误且 LLM 实际采样过 → 构造 `ExperienceTrace{ task: user intent, actions, outcome: Success, llm_used: true }` → `sess.services.experience_runtime.lock().await.observe_trace(&trace, false)`；
  - 被经验完全接管（零 LLM）的 turn **不产生 trace**。
- **验收**：单测（TraceRecorder 纯逻辑：从事件提取 name/args、去重、空 turn 不产出）；`cargo check` 绿；运行验证（网络可用后）：一次真实对话 → 库中出现 CANDIDATE。
- **风险**：记录点与真实工具调用不一致 → 记录只读、与执行解耦；功能开关可临时禁用。
- **实施记录**：`TraceRecorder`（experience_learner.rs，纯逻辑+单测）→
  作为 `&mut` 参数透传 `run_sampling_request`/`try_run_sampling_request`
  （两处签名+调用点）→ 在 `ResponseEvent::OutputItemAdded` 的
  `FunctionCall` 分支只读记录 name/arguments（arguments 按需 JSON 解析）→
  `run_turn` 结束时（`llm_sampled && !empty`）构造
  `ExperienceTrace{task=user_text, Success, llm_used=true}` →
  `observe_trace`（连续 N 次或用户确认才沉淀，默认 2 次）。零 LLM 的经验
  接管 turn 因提前 break 不产生 trace。记录不改变任何执行路径（原版行为
  逐位保留）。

### M2-2 Session 级状态持久 + execution_mode 写回

- **目标**：`ExperienceState` 跨 turn 连续；`ControlDecision::execution_mode()` 真正写回状态。
- **现状**：状态每 turn 由 `build_experience_state` 重建；`update_experience_state` 仅保证 turn 内连续；`execution_mode` 由 executor 设置，但决策点的映射未写回。
- **改动文件**：
  - `experience/experience_runtime.rs`：新增 `session_state: ExperienceState`（持久于 runtime 内）；
  - `experience/experience_state_builder.rs`：`build_experience_state` 增加"继承上一 turn 的 elements/deployed"逻辑；task 按新输入重置（新输入非空时），无新输入则保留；
  - `session/turn.rs` 决策点：决策后 `experience_state.execution_mode = experience_decision.execution_mode();`。
- **验收**：单测（runtime 保留/重置逻辑：同 turn 连续、新输入重置 task、registry/deployed 继承）；`cargo check` 绿。
- **风险**：task 判定（同一任务 vs 新任务）保守化——v1 只按"是否有新 user input"判定，避免误合并。
- **实施记录**：`ExperienceState::inherit_session(previous)`（继承
  elements/deployed/环境/网络/工具/配置基线；task/executed_steps 仅在"无新
  输入"时继承）；`ExperienceRuntime` 增加 `session_state` + `inherit_session`/
  `commit_session`；`run_turn` turn 开始继承、turn 结束提交；决策后
  `experience_state.execution_mode = experience_decision.execution_mode()`
  （①/②→Reflex、③/④→Deliberation、Abort→Conflict）。经验与 LLM 共享同一
  份延续账本。

### M2-3 freshness tick

- **目标**：带 TTL 的状态元素过期后不再被信任；陈旧经验向 DECAYING 迁移（阶段 9 收尾）。
- **现状**：`StateElement` 有 `verified_at/ttl` 但无 tick；`ExperienceLifecycle` 无"陈旧"路径。
- **改动文件**：
  - `experience/experience_state.rs`：`tick(&mut self, now)` —— 过期元素标记/移除，返回过期 key 列表；
  - `experience/experience_lifecycle.rs`：`ExperienceLifecycle::on_stale(status)` —— ACTIVE/VALIDATED → DECAYING；
  - `experience/experience_runtime.rs`：`tick(now)` —— 转发 state.tick + 对过期元素对应经验执行 on_stale；turn 开始时调用。
- **验收**：单测（过期移除、on_stale 迁移、runtime tick 联动）；`cargo check` 绿。
- **风险**：误杀活跃经验 → TTL 默认放宽（先不设 TTL = 永不过期），仅对显式带 TTL 的元素生效。
- **实施记录**：
  - **遗忘算法（v1 明确版）**：`ExperienceLifecycle::forgetting(status,
    last_used, now, pinned)` —— ① `pinned` → Keep（特权：永不被时间遗忘，
    仅显式 Disable）；② `last_used=None`（从未使用）→ Keep；③
    `age_days=(now-last_used)/86400`：VALIDATED/ACTIVE 且 age≥60（FORGET_DAYS）
    → DISABLED、age≥30（STALE_DAYS）→ DECAYING、否则 Keep；DECAYING 且
    age≥60 → DISABLED；NEW/CANDIDATE/DISABLED 不按时间遗忘（候选由反馈
    生命周期自我修剪）。
  - `ConfidenceCounters.last_used`：每次 `record_feedback(id, kind, now_secs)`
    时 `touch(now)`——经验被使用即刷新新鲜度。
  - `ExperienceState::tick(now)`：`verified_at + ttl ≤ now` 的元素移除并
    返回过期 key。
  - `ExperienceRuntime::tick(now) -> TickReport`：过期状态元素 + 逐经验执行
    遗忘（pin 豁免），每 turn 开始时由 run_turn 调用。
  - **不被遗忘特权**：`ExperienceStore::pin/unpin/is_pinned/pinned_ids`；
    `ExperienceRuntime::set_pinned/is_pinned`；pin 集合随 store JSON 持久化
    （快照格式：experiences + pinned）。
  - **store 文件持久化能力**：`save_to_path/load_from_path`（JSON 文件，
    M2-5 的降级后端前移为管理页面的数据源基础）。

### M2-3b 管理页面（用户随时介入"不被遗忘特权"）

- **目标**：用户能查看全部经验（名称/类型/状态/置信度/最后使用/pin 徽标），
  并对指定经验授予/撤销"不被遗忘特权"（pin/unpin），或手动禁用。
- **数据契约（核心 API 已实现）**：
  - `runtime.management_list() -> Vec<ExperienceManagementEntry{id, name, kind,
    status, confidence, pinned, last_used}>`；
  - `runtime.set_pinned(id, bool)` / `is_pinned(id)`；
  - `runtime.tick(now)`（手动触发一次遗忘）；`store.save_to_path/load_from_path`
    （页面数据持久化基础）。
- **页面交互规格**：
  - 列表列：名称 / 类型 / 状态（含 DECAYING 警示）/ 置信度 / 最后使用
    （距今天数）/ PIN 徽标；
  - 操作：PIN（授予特权）/ UNPIN（撤销）/ DISABLE（手动禁用）；筛选：
    全部 / 仅 pin / 仅 DECAYING；
  - 顶部按钮"立即遗忘检查"→ 调用 tick 并刷新列表。
- **落点与依赖**：页面需要持久化的 store 数据源，故**排在 M2-5 之后**或随
  M2-5 的 JSON 文件接入一并落地。呈现形式二选一（实现时定）：
  (a) `codex experience list|pin|unpin|disable`（codex-cli 命令组，产品内建）；
  (b) MCP `experience.*` 工具 + 外部页面（与 M2-6 合并）。

### M2-4 结果回显

- **目标**：①/② 完成路径下用户能看到结果（当前 `break` 只打日志）。
- **现状**：`execute_experience_if_hit` 返回 `Completed` 后 run_turn 直接 break；无用户可见输出。
- **改动文件**：
  - `session/experience_action_runner.rs`：`build_experience_result_message(state) -> String`（任务摘要 + executed_steps + deployed）；
  - `session/turn.rs`：Completed 分支 → `record_conversation_items` 写入结果消息 → break。
- **验收**：单测（回显文本纯函数）；运行验证（注入 env.cwd/noop 经验 → 零 LLM + 用户可见结果）。
- **风险**：结果格式与模型可见历史兼容 → 复用现有 `build_text_message_item` 模式。
- **实施记录**：
  - **结果回显**：`ExperienceState::render_result_message()`（任务摘要 +
    执行步骤 + 部署 + **任务详细过程**）；①/② Completed 分支经
    `record_conversation_items` 写回用户可见消息后 break。
  - **任务详细过程（回检视图）**：新模块 `process_log.rs` —— `ProcessActor`
    （经验/LLM/工具）+ `ProcessStep` + `render_process_log()`；`ExperienceState`
    增加 `process_log`（任务级，新输入重置/延续继承）。记录点：
    (a) 决策点：每个采样请求前的决策标记（经验接管/LLM 主导/冲突）；
    (b) executor：每条经验工作流步骤（含 ok 与摘要）；
    (c) 采样点：LLM 推理标记；
    (d) 采样返回后：把该次请求的工具调用按序 flush 进日志（保持多步骤
    交错的时间顺序）。用户可看到多步骤任务中经验在哪个环节发挥的作用。

### M2-5 store 后端升级

- **目标**：经验跨进程/重启存活。
- **现状**：内存 BTreeMap + JSON 导入导出（`to_json/from_json` 已有）。
- **方案（首选）**：SQLite（workspace 已有 sqlite 基建）；**降级路径**：固定路径 JSON 文件（启动加载、变更落盘）。
- **改动文件**：`experience/experience_store.rs` 后端抽象（保持 `upsert/get/matchable/stats/transition` API 不变，换存储实现）。
- **验收**：单测（落盘 → 重载 → 一致）；`cargo check` 绿。
- **风险**：sqlite 链接/并发 → 先做 JSON 文件版（已验证 to_json/from_json），SQLite 作为可选后端，不阻塞 M2 主线。

### M2-6 阶段 10 MCP 子集

- **目标**：外部 Agent 通过 MCP 访问 Experience。
- **现状**：`codex-mcp` 存在（MCP server），无 experience 命名空间。
- **方案**：新增 `experience.*` 工具子集（只读优先）：`experience_list`、`experience_get`、`experience_state`、`experience_feedback`、`experience_transition`、`experience_observe_trace`。参考 prior-art `mcp_server.py` 设计，先做最小可用集。
- **改动文件**：`codex-mcp` 工具注册 + handler（薄壳调用 experience module）。
- **验收**：工具 schema 注册测试 + handler 映射单测；运行验证（mcp 启动后工具列表含 experience.*）。
- **风险**：MCP 改动面大 → **拆为 M2b**（M2 主线完成后再做）；只读优先，写操作仅 feedback/transition。

## 3. 顺序与依赖

```text
M2-1 (trace) → M2-2 (持久+写回) → M2-3 (freshness)
             → M2-4 (回显)      → M2-5 (store 后端) → M2-6/M2b (MCP)
```

- 每个工作项完成后强制：单元测试全绿 + `cargo check -p codex-core` exit 0 → 提交（commit message 注明 M2-x）；
- M2-1 依赖 M2-2 的 task 语义（trace 的 task 来源），可先做 recorder 纯逻辑；
- M2-4 依赖 M2-2（回显需要连续状态）。

## 4. 验证与回归（每步强制）

1. 单元测试全绿（每项新增 ≥2 测试）；
2. `cargo check -p codex-core`（本环境 GNU 工具链可跑）exit 0；
3. **④ 回归**：无经验注入时 `codex exec` 行为与基线一致（无 experience warn、无 gate 副作用）；
4. 运行验证（网络+凭证可用后）：注入经验 → 零 LLM + 回显；真实对话 → trace → CANDIDATE。

## 5. 风险与对策

| 风险 | 对策 |
|---|---|
| 侵入采样/工具执行路径 | 记录只读、与执行解耦；必要时 feature 开关 |
| 状态连续性误判（任务合并/拆分） | task 判定保守化：仅按"新 user input"重置 |
| SQLite 引入复杂度 | 先 JSON 文件后端，SQLite 为可选后端 |
| MCP 改动面大 | 拆 M2b，只读优先 |
| 网络不可用 | 运行验证分两段：本机可测部分先行，端到端由用户侧执行 |

## 6. 完成定义（M2 Done）

- 6 项全部落地（M2-6 可降级为 M2b 完成或明确延期）；
- 单元测试总数更新、全量 check 绿、④ 回归通过；
- 至少一次端到端运行验证：① 命中 → 零 LLM → 用户可见结果；LLM 路径 → trace → CANDIDATE；
- 文档同步：架构图（§10.1）、进度评估（M2 完成记录）。

# P1 开发计划：最小可运行 Experience

> 2026-09-07。Level 2 已成立；本阶段从“实验”进入“工程”。
> v1.1（2026-09-07 评审收紧）：State=evidence predicates、workflow 预编译、
> trigger=Action pattern、Gate 接口先行、失败语义最小化、里程碑先
> Experience 后 Codex。
> v1.2（2026-09-07 逐条评审定稿）：probe 与 Match 交错、trigger pattern
> 粒度与适用范围、Gate=decide+execute 两级同步边界、失败返回
> executed_side_effects。
> v1.3（2026-09-07 语义收紧）：Gate 同步边界=**控制权事务**——Gate 返回
> 即接管终结（completed/partial/failed 三态已定），常驻契约而非 P1 临时
> 简化；不承诺世界状态原子事务（不隐式回滚、不把 partial 当 completed）。
> v1.4（2026-09-08 范围裁定）：P1 实际范围 = 核心闭环（M1–M5）+ Experience
> App 壳（launcher/server/ui/Agents 管理/会话委派，见 experience-app.md）。
> “多 Agent 适配”仍非目标（只接 codex）；App 壳与 Agents 管理不属于该
> 禁项，文档口径以此为准。

## 1. 目标

不实现学习系统，先让**一条人工创建的 Experience** 在真实 Codex loop 里
完成：

```text
Codex → FunctionCall → Gate → Match(Experience) → Execute(Workflow)
     → Verify(postconditions) → FunctionCallOutput → Codex 继续
```

### 1.1 三个硬约束

1. **Experience 不规划**：Workflow 必须是预编译步骤（每步映射到真实
   Tool / Executor capability），Runtime 不得让 LLM 解释后决定执行；
2. **Experience 不伪造状态**：State' 必须来自真实 side effect + evidence，
   未经验证的后置不得声称完成；
3. **Experience 不吞掉失败**：partial/failed 必须真实反馈给 Agent。

### 1.2 最小 Experience 结构（预编译，人工写入）

```yaml
name: create_probe_file
trigger:                     # Action pattern（Agent 现在想干什么）
  tool: exec_command
  match: { command_pattern: "create probe file" }
preconditions:               # 当前环境满足什么（State）
  - predicate: "cwd.exists" = true
workflow:                    # 预编译步骤，直接映射 Tool capability
  - action: write_file
    args: { path: "probe.txt", content: "EXPERIENCE_GATE_SUCCESS" }
postconditions:              # 执行后环境应变成什么
  - predicate: "file:probe.txt.exists" = true
  - predicate: "file:probe.txt.content" = "EXPERIENCE_GATE_SUCCESS"
verification: [ read file probe.txt ]
failure_policy: stop_and_report
undo: unsupported
status: ACTIVE
```

职责划分：trigger → Action pattern；preconditions → State；
postconditions → evidence-bearing 目标状态。

### 1.3 trigger 匹配粒度与适用范围（v1.2）

- trigger pattern 是机械匹配（P1：子串/regex），MISS 即放行，不做模糊猜测；
- 只覆盖“动作形态稳定”的经验（P1 用可控任务+精确动作）；
- 表达同一意图的方式多样时需未来的 reference/意图提示机制，不属于 P1。

## 2. 组件边界（依赖顺序，最后才 Learning）

| # | 组件 | 边界职责 | 不包含 |
|---|---|---|---|
| 1 | Experience Application | 用户输入入口、配置、最小 CLI/API | 多 Agent |
| 2 | Experience Runtime | 总控制器：State→Decision→Match→Gate→Execute→Result 编排 | 学习闭环（后置） |
| 3 | State / Predicate | 可验证谓词 `(key,value,evidence,verified_at,ttl)`，粒度约束见 state-model | 环境全貌、embedding |
| 4 | Transition / Workflow | 已知转移 S→S'：预编译 steps + postconditions + verification | 规划权、运行时解释 |
| 5 | Decision / Matching | Intent 过滤 + 谓词满足（三值）；无相似度/阈值打分 | RAG/检索器 |
| 6 | Gate | 两级同步契约：`decide(action,ctx)->GateDecision`；Hit 后同一边界内 execute+verify+return；四档 A-D；语义=控制权事务（返回即终结，不承诺世界状态原子性） | 通信方式、外部监听式异步、只判不执行 |
| 7 | Executor Adapter | 能力声明 TaskExecution / ActionInterception | 内部实现依赖 |
| 8 | Codex Executor | fork codex 的 dispatch 前钩子（已验证 Level2 位置）；P1=内嵌 Runtime（模型 A），共享 store；连接状态机与 §1.1 语义在 Adapter 契约中声明，IPC 不在 P1 实现 | 官方路径依赖、模型 B 的外部连接 |
| 9 | Store | Experience 持久化（schema 版本化、ACTIVE 匹配面索引） | usage 之外的职责 |
| 10 | Validation | 五级门：schema→tool boundary→state prerequisite→dry-run→observable completion | 学习期启发 |
| 11 | Usage / Audit | 调用记录/引用日志/统计 | 学习决策 |
| 12 | Learning / Distillation | necessity→distill→validate→activate（后续 P2） | —— |

## 3. 非目标（本阶段明确不做）

MCP、Skill、RAG、自动学习、蒸馏、多 Agent **适配**（executor 只接
codex）。注：Experience App 壳（launcher/server/Agents 管理页/会话委派）
属于产品形态建设，已在 P1 内完成（experience-app.md），不受“多 Agent
适配”禁项约束。

## 3.5 失败语义（v1.3）

- **Gate 返回语义 = 控制权事务**：decide→Hit→execute→verify→return 在
  同一同步调用内；Gate 返回 ⇔ 接管终结（completed/partial/failed 三态已
  定）。该同步边界是 Level 2 常驻契约，不是 P1 临时简化；P1 不承诺世界
  状态原子事务——不隐式回滚、不把 partial 冒充 completed（详见
  step-gate.md §8.2）；

- 步骤成功 → 更新对应 evidence；
- 步骤失败 → **立即停止 workflow**，验证当前实际 State，返回
  partial/failed，不得声称 postconditions 已完成；
- P1 不实现 rollback（`undo: unsupported`、`failure_policy:
  stop_and_report`），避免第一阶段陷入事务系统。
- 同步接管阻塞 Agent turn，因此 P1 workflow 必须可被中断终止，禁止无界
  接管：实现为**步骤间取消检查**（`request_cancel`，单步工具调用视为有界
  原子操作）；exec 级超时由会话层负责（默认不启用，见 compatibility
  §1.2）。
- 无论成败，GateHitResult 都返回 `executed_side_effects`
  （已成功步骤 + evidence 清单），供 Agent 接管排障——不能只给 State，
  因为无谓词覆盖的副作用无法由 State 重建。

## 3.6 probe 时序（v1.2）

不做全量 predicate probe。顺序：trigger/intent 粗滤 → 候选集 →
只 probe 候选的 preconditions → 精确判定。probe 与 Match 交错，控制成本。

## 4. P1 验收（最小闭环四问）

1. Codex 真实提出 create_probe_file 的 FunctionCall；
2. Gate 同步匹配到该经验（HIT，非哨兵）；
3. Experience 执行 workflow → postconditions 验证通过 → State' 成立；
4. 合法 FunctionCallOutput 回 Codex → Agent 继续正常。

验收标志：日志出现 `GATE HIT experience=create_probe_file`、
probe 文件真实存在且内容正确、模型正常继续收尾。

## 5. 与 codex-main 资产映射

- Store/Schema：沿用 experience-core（已搬运）+ experience-store 校验；
- State：experience_state（StateElement/predicate 化）；
- Gate：stream_events_utils dispatch 前钩子（由探针升级为真实
  Experience Match）；
- Runtime 编排：experience-main 新建 experience-controller。

## 6. 里程碑

- **M1 Domain Model**：Experience / Predicate / Transition / Result
  （纯类型，先编译跑通）——✅ 完成（2026-09-07，`c188f63`，
  `experience-core/src/domain/`）；
- **M2 Store + 手工 Experience**：文件 Store + 人工注入
  create_probe_file——✅ 完成（2026-09-07，`experience-core/src/store.rs`
  + `experiences/create_probe_file.json`，版本化信封 + ACTIVE 匹配面索引）；
- **M3 Runtime**：当前 predicates → trigger/intent 粗滤 → 候选 →
  候选 preconditions probe → Match → Execute → Verify postconditions →
  更新 State'（先用 fake/local executor 跑通）——✅ 完成（2026-09-07，
  `experience-controller/`，`ExperienceGateRuntime::gate` 同步闭环 +
  LocalProbe/LocalRunner，117 passed）；
- **M4 Codex Adapter**：`FunctionCall → ExperienceGate.decide → Runtime`
  （decide+execute+verify+return 在同一同步边界内；IPC/库内嵌不定死）；
  该边界语义=控制权事务：Gate 返回即接管终结，非世界状态原子事务；

  ✅ 完成（2026-09-07，codex-main `16067215b`）：模型 A 落地——codex-core
  内嵌 experience-core/experience-controller（path deps），dispatch 边界
  的探针替换为真实 `ExperienceGateRuntime.gate()`；`EXPERIENCE_GATE_STORE`
  指定共享 store，`EXPERIENCE_ENABLED` 总开关生效。确定性测试
  `gate_hit_runs_embedded_experience_and_skips_tool_dispatch` 通过：
  FunctionCall → HIT → 原 Tool 不 dispatch → Experience 执行并验证
  probe.txt → 合法 FunctionCallOutput 注入 → loop 继续。

  M4 拓扑决定（2026-09-07）：P1 采用**模型 A（内嵌 Runtime）**——把
  codex-main 探针升级为真实 ExperienceGate 接线，runtime 内嵌于 codex
  构建并与 Experience Application 共享 store，先把真实闭环验收掉；
  模型 B（外部 Runtime + IPC）只做 Adapter 契约设计，不在 P1 实现。

  Executor 连接契约（见 compatibility.md §1.1）：托管型（headless 控制
  通道 + 生命周期）与附着型（附加已开实例，不新开界面）显式声明；状态
  机 `configured → launching → connected / error` 在 UI 始终可见。原则：
  **不静默 ≠ Gate 阻塞**——Runtime 不可用时 Gate 只产生 MISS（安全
  降级，不引入新语义），由编排层显式呈现 Disconnected，拒绝“什么也没
  发生”；
- **M5 Real Loop Acceptance**：真实 Codex 闭环验收。

  ✅ 完成（2026-09-09，codex-main `a9c78eed5` 测试资产 + `7db612899`
  L4 usage）：gate 化二进制重建、`accept-m5-gate.ps1` 真机四问 PASS
  （GATE HIT 日志 / 原 Tool 未 dispatch / probe.txt 内嵌执行验证 /
  exit 0），GateHitResult→usage.json 写回与 fixture 已入库。验收口径 =
  §4 最小闭环四问（见 codex-main/docs/architecture/
  m5-gate-acceptance-plan.md）。

顺序原因：先让 Experience 自己跑通（M1-M3），再接 Codex（M4），避免两套
系统同时 debug 时无法归因。

## 7. 当前状态与下一步

- M1 Domain Model / M2 Store / M3 Runtime / M4 Codex Gate 已完成；
- Experience App 壳（launcher + server + ui/www + Agents 管理 + 会话委派）
  已完成（experience-app.md）；
- SessionChannel 已是产品委派路径（`channel=session`，exec 保留为
  fallback）；P2 五项收敛完成——session_host 公共内核、优雅收尾、
  thread_id 回填、typed trace v1 冻结（含 phaseC 审查加固），L0 smoke
  升级为 typed v1 契约探针，下一步 = L1 设计（见 README 双主线账目）；
- codex-main 侧 M5（真实 Gate 闭环）已完成（2026-09-09），L4 usage
  写回已随 WP7 落地；L0–L4 文档终点达成（模型 B / 多 Agent 非目标不变）。

### 7.1 阶段收口（2026-09-09，WP1–WP7 声明完成）

- 完成判定证据：双仓提交链
  （experience-main 108b688→8c421ff→70492f8→e041161→02e28b1；
  codex-main a9c78eed5→7db612899→57df9f1d9）、测试全绿（191+1 与
  gate 2+确定性复跑）、真机 accept-l3-loop / accept-m5-gate PASS；
- 非阻塞剩余与下一阶段（Stage S）见 docs/next-stage-plan.md；
  模型 B / 多 Agent 维持冻结。

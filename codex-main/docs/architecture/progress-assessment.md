# 架构改造进度评估（沙盘推演）

> 2026-09-09 状态注：本页为 2026-08-31 沙盘推演记录，阶段评估已被后续
> 真机账本取代。Experience 主线当前状态：M1–M4 ✅（16067215b）、M5 真机
> 四问 ✅（57df9f1d9，accept-m5-gate PASS）、L4 GateHitResult→usage 写回
> ✅（7db612899）；完整账本见 experience-main/README 与
> docs/architecture/m5-gate-acceptance-plan.md。

> 推演日期：2026-08-31
> 基线：`6478a751`（tag `baseline`）；当前 HEAD：`a95428280`（阶段 1-9 逻辑初步完成）

## 总体进度

**10 阶段综合完成度：约 78%**

判断依据：控制面缝隙（最重要的一刀）已经真实存在，但缝隙两侧还是空的——
左侧没有可命中的经验，右侧 HIT/AbortOrAsk 仍是占位。当前系统与基线的行为
完全一致（恒 MISS），所以是"地基已浇筑、墙体未起"的状态。

## 里程碑 M1：阶段 1-9 逻辑初步完成（2026-09-01）

**记录**：提交 `a95428280`。阶段 1-9 核心逻辑全部落地并通过单测（**60 个
单元测试**，scratch crate + GNU 工具链）。

**验证边界（如实）**：
- ✅ 纯 experience 模块：状态/A1、匹配/A2、决策/A3、执行器/A4、store、
  confidence/A6、学习器/阶段7、校验/阶段8、状态机/阶段9、行为映射/阶段6；
- ⏳ session 层接线（builder / runner / turn.rs）：已完成静态复核，待
  **全量编译测试**（用户真实终端运行 `cargo-check-gnu.ps1`，沙箱环境无法
  执行 build script 子进程，与代码无关）。

**待集成清单（M2 目标）**：
1. trace 采集接线（LLM 成功 turn → ExperienceTrace → learner）；
2. Session 级状态持久（跨 turn）；
3. freshness tick（StateElement TTL 衰减 → DECAYING 联动）；
4. `execution_mode` 写回 `ExperienceState`；
5. store 后端升级（SQLite）与结果回显；
6. 阶段 10 MCP 外部访问。

> M2 精确计划见 `docs/architecture/m2-plan.md`（逐项规格/验收/顺序/风险，coding
> 时逐项对照，避免偏移）。

## 分阶段进度

| 阶段 | 内容 | 状态 | 完成度 | 证据 / 说明 | 下一步 |
|---|---|---|---|---|---|
| 1 | 冻结基线 | ✅ | 100% | git 仓库、`upstream`/`origin`、tag `baseline`/`baseline-zip-snapshot`、BASELINE.md、合并策略 | 无（等待 fork push 时机） |
| 2 | 画 Agent Loop | ✅ | 100%（文档级） | experience-agent-loop.md：Session→Task→run_turn→run_sampling_request 全链路实测行号，插入点定位在 turn.rs:373-384 | 无 |
| 3 | Experience State | 🔶 | 97% | A1 完整 + lookup/handoff_payload ✅；**状态层收尾**：StateElement 注册表（source/upsert/lookup）✅、runtime 探测（runtime.subprocess_allowed）✅、deploy.env 动作 + executor 部署记录 ✅（38 测试）；Session 级持久与 freshness TTL 未做 | Session 级状态持久 + freshness 衰减 |
| 4 | Experience Runtime | 🔶 | 95% | matcher/decide/executor(异步)/store/confidence 全 ✅；executor 已异步化（BoxFuture）并接入 run_turn；**A6 执行点反馈已接入**（计数+刷新置信度） | 状态机迁移（阶段 9） |
| 5 | Agent Loop 改造（第一刀） | 🔶 | 70% | 决策点 + ①/② 执行（安全动作 + `tool.<name>` 经 ToolCallRuntime/审批/沙箱）✅；**③ 参考注入已实现**（每轮一次注入 prefill）✅；④ 基线不变 ✅ | 真实验收 + 结果回显 |

## 中间验证（何时做、做什么）

**时机：现在**。阶段 4 已收尾、阶段 5 主体已落地，接下来进入阶段 7（学习器）
之前必须验证——因为 session 层接线（experience_state_builder /
experience_action_runner / turn.rs）尚未经过真实编译与运行验证。

**验证内容**：
1. 单元层：experience 模块 36 个测试（已通过；纯 Rust、无 C 子进程，沙箱内可跑）；
2. 编译层（全量）：`cargo check -p codex-core` **已通过（2026-09-01，提权
   GNU 工具链 + w64devkit，exit 0）**——session 层接线类型正确；过程中发现并
   修复 3 个真实错误（build_tool_call 为关联函数、deploy.env key 移动后借用、
   dyn ExperienceActionRunner 需 Send）。剩余 40 个警告均为待集成 API 的
   死代码提示（learner/lifecycle/store 接口），随 M2 集成清单接线而消减。
3. 集成层（走通骨架）：注册安全动作集经验（env.cwd / noop），跑真实 turn，
   断言 LLM 调用 0 次 + A6 反馈计数更新；
4. 回归：④ Delegate 路径行为与基线一致。

**决策（2026-09-01）**：全量编译验证**降级为"用户方便时在真实终端运行
`cargo-check-gnu.ps1`"**，不阻塞 agent 改造推进。理由：阻塞来自执行环境沙箱
（禁止 build script 子进程，与工具链无关）；纯逻辑核心由 36 个单测兜底；
session 层接线为小块代码、错误可后置修复。该事件同时是状态层设计的靶点
（见 experience-state-design.md）。

**冒烟测试（2026-09-02，M2-1..M2-5 + M2-3b 后）✅**：
1. `cargo build -p codex-cli` 成功（7 分钟）；
2. `codex --version` / `--help`（含 experience 子命令）正常；
3. `codex doctor` 无崩溃，诊断结果与基线一致（auth/terminal/reachability
   为环境项）；
4. `codex experience list / pin / unpin / disable / path` 端到端通过
   （隔离 CODEX_HOME + 种子数据），pin 与 status 正确写回 store.json；
5. `codex exec` 会话启动回归通过：门禁/持久化代码零新增错误，行为与基线
   一致（仅网络环境错误 os error 10013）；真实会话成功加载 store 文件。

**实验里程碑 E1（2026-09-04，main，DeepSeek）：经验零 LLM 泛化重放成功**

冷启动：LLM 将桌面 `首页1.png` 移入 `pic`（1 次工具执行）→
`EXPERIENCE_AUTO_CONFIRM` 沉淀 Validated/0.90 经验。
暖启动（同类泛化）：任务改为"移动桌面**所有** PNG 到 pic"→ ExperienceOnly
命中 → 经验工作流（单条 `exec_command` glob 移动）零 LLM 执行 → 18 个 PNG
全部移入、桌面归零、无模型输出。过程暴露并修复：args 捕获时机
（OutputItemAdded→Done）、UTF-8 管道、经验步骤名需用环境实际工具词汇
（exec_command 而非 shell）、参数键 cmd。

**验证结论：Experience 能在同类任务上绕过 LLM 推理，确定性接管执行。**

**架构原则（2026-09-04 固化）：经验自动产生，用户零交互**

产生可用经验不得要求用户再次确认/参与，否则与"外置 MCP 经验工具"无异，
违背 Experience 内嵌于 agent 的本意。价值定义：经验让 agent 后续少碰壁，
把"一次性翻车成本"摊薄为复用的近零成本。

经验产生流水线（内建于 learner，非事后手工）：
```text
捕获(全量轨迹)
  → 剪枝(确定性: 丢弃被拒/失败/探测/安装调用, 去重, 只留成功必要动作)
  → 蒸馏(脏轨迹触发一次小 LLM compile: 剪枝后总结为参数化步骤+触发+依赖)
  → 校验(沙箱干跑蒸馏结果, 能重放才允许升 Active)
  → 激活(Validated/Active)
```

观察基准（Instagram 只读冷启动，2026-09-04）：97,952 token 中 >80–90% 为
探索/排障（原版 codex 在缺浏览器会话工具的 exec 环境下同样会烧）；真正最小
成功路径约 6–8 步。当前系统正确地把该 56 步轨迹留在 Candidate（质量门），
但缺"蒸馏成可用经验"的正向一步——即本流水线的 2–4 段。参数（hwnd/pid/账号）
由 State 层提供，不写死进步骤。

**M2 步骤 2：蒸馏框架落地（2026-09-04，提交 `4a1642fe4`）**

"缺蒸馏的正向一步"现已补上框架位（流水线第 2 段的接口层）：

- `ExperienceCompiler` trait（`compile(trace) -> Option<ExperienceDraft>`，
  Send + Sync）可注入 `ExperienceRuntime`（`set_compiler`）；
- `observe_trace` 蒸馏门禁：脏轨迹（> MAX_AUTO_CONFIRM_STEPS 步）停留
  CANDIDATE 时，若已注入 compiler 且蒸馏成功，则以蒸馏结果替换草稿入库——
  蒸馏器负责把"翻车轨迹"压成最小可重放步骤（通常 VALIDATED）；
- 蒸馏器本体按设计由 session 层注入 **LLM-backed compiler**（一次小模型调用，
  剪枝后总结为参数化步骤 + 触发 + 依赖）；当前先用确定性 tester 编译器验证
  门禁逻辑：messy 21 步轨迹 → 1 步 VALIDATED 经验；
- 附带修复：action runner 对未知/非内置动作降级为工具分发（learned workflow
  记录裸工具名如 `exec_command`），经验重放不再报 unknown；
- 验证：`cargo check -p codex-core` exit 0；experience 模块 **79/79** 测试通过。

下一步（流水线 3–4 段）：session 层接入真实 LLM distiller + 沙箱干跑校验
（能重放才升 Active）。

**M2 步骤 3-4：自动流水线接通（2026-09-04，提交 `23768c146`）**

上一步的"下一步"已完成——从蒸馏框架升级为**全自动产生流水线**，且保持
"经验自动产生、零用户交互"的架构原则：

- **蒸馏**：`session/experience_distiller.rs` 在脏轨迹（原始 trace 或修剪后
  workflow 超过 20 步）上触发一次**无工具小 LLM 编译调用**（复用 compact 同款
  `ModelClientSession::stream`）；模型只输出 JSON，解析/兜底均为确定性代码；
- **校验**：结构校验（`Experience::validate`）+ 工具边界干跑
  （`validate_replayable`：每步经 ToolRouter 解析、参数非空，**不执行任何动作**）；
- **激活**：`admit_auto_experience` 同 id 替换候选（版本递增）并
  VALIDATED → ACTIVE；
- **接线**：`run_turn` 成功 LLM turn 结束后，learner 草稿自动进入流水线；
  短/干净草稿直接激活，脏轨迹先蒸馏。任何失败都停在 CANDIDATE/VALIDATED，
  绝不半激活；
- 验证：`cargo check -p codex-core` exit 0、`cargo check -p codex-cli`
  exit 0；experience + distiller 单测 **85/85** 通过。

真实行为验证的下一步：重跑一次"脏冷启动 → 经验自动蒸馏 → 暖启动零 LLM
接管"对照（例如 Instagram 只读任务），用 store.json 检查步骤是否被压缩成
最小路径。

**计划偏移检查**：目标无偏移。实现顺序按用户指示调整为 3/4/5 先行（状态 →
runtime → 第一刀），A6 反馈与 ③ 参考注入提前补齐；真正的风险是"session 层
未验证代码积累"，因此现在验证。MSVC 安装讨论已关闭：运行时不需要、GNU
工具链已可用、且沙箱限制与编译器无关——安装它只会增加环境重量，不解决任何
问题。验证后再进阶段 7。
| 5 | Agent Loop 改造（第一刀） | 🔶 | 35% | 门禁已插入 run_turn 采样前，每次模型请求都经过 `should_call_llm`；当前恒 MISS，HIT/AbortOrAsk 占位回退 | 实现 HIT：命中后跳过 run_sampling_request |
| 6 | 三种 Agent 行为 | 🔶 | 50% | ExecutionMode 已定义；**ControlDecision::execution_mode() 映射已实现**（①/②→Reflex、③/④→Deliberation、Abort→Conflict）；executor 执行时置 Reflex/失败置 Conflict | 会话层把映射写回 state.execution_mode |
| 7 | LLM 作学习器 | 🔶 | 35% | ExperienceLearner 核心已实现（trace→证据累计→Draft(CANDIDATE)→注册 store，45 测试）；会话内 trace 采集（工具调用捕获）待全量编译后接线 | trace 采集接入 run_turn + 用户确认流 |
| 8 | 可执行结构 | 🔶 | 55% | Experience schema ✅；**结构校验已实现**（Experience::validate：名称/trigger 信号/workflow 非空/步名/置信度范围/版本）**+ 版本化（bump_version）+ store 强制校验（upsert_validated）**（60 测试） | 持久化后端（SQLite） |
| 9 | 状态机 | 🔶 | 60% | ExperienceLifecycle 已实现：显式迁移（Validate/Activate/Disable/Revalidate）+ 置信度驱动（≥0.35→VALIDATED、≥0.75→ACTIVE、连败3→DECAYING、5→DISABLED、恢复2→ACTIVE）；record_feedback 与 store.transition 联动（54 测试） | freshness/陈旧 tick 联动 → DECAYING |
| 10 | MCP 适配 | ⬜ | 0% | 无（参考实现有 mcp_server.py 40 工具可作蓝本） | 阶段后期 |

## 关键判断

1. **缝隙是项目成败的载体，已经就位**：`run_turn` 每次模型请求前经过
   `should_call_llm`，包括工具调用后的 follow-up。后续所有阶段都在这条缝隙上
   生长，不再需要动任务层。
2. **瓶颈已从"控制权"转移到"有东西可命中"**：下一阶段的关键不是继续加门禁，
   而是把 Experience schema、store、matcher 建起来，让 HIT 有据可依。
3. **借源参考（prior-art experience_runtime v0.2.1）与本规划同构**：request →
   encode/match/gate/run → feedback、execute/assist/delegate、置信度公式、
   SQLite store、MCP 40 工具，全部有现成蓝本，详见
   `experience-reference-prior-art.md`。

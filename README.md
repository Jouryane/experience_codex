# Experience

**Experience 是一个“会积累、会复用、可审计”的本地 Agent 编排与经验系统。**

它把 Agent（Codex / Trae / Claude Code / DeepSeek harness 等）当作可插拔的
执行器：已知且可验证的部分由 Experience 直接完成（先动手），无法确定的部分
再委派给 Agent；每次真实执行都可沉淀为**经验候选**，经资格验证后成为可复用
能力。经验可以按**场景（scope）**组织、按**用户偏好**取舍，并在任务执行过程中
作为参考注入或直接兑现。

核心原则：**不伪造状态、失败即停、先动手后委派、用户保有决策权**。

## 能做什么

- **任务编排**：任务入口按场景唤醒经验 → 本地执行可验证的步骤 → 只把剩余
  部分委派给 Agent；
- **学习闭环**：真实会话（typed trace）→ 必要性判定 → 确定性/LLM 蒸馏 →
  候选经验（CANDIDATE，不自动激活）；
- **资格与生命周期**：五级验证门 → VALIDATED → ACTIVE；证据变量驱动
  置信度/衰减，置顶（pin）豁免；
- **参考吸收**：从任意 Agent 的 run-notes.md / 材料包吸收参考经验，可一键
  解析预览；用户点击即可提升为候选（产物仅到 CANDIDATE）；
- **场景与个性化**：经验可挂载到场景（scope），用户可设别名、使用偏好
  （允许/停用）与个人评分——只影响你的决策视图，不改证据；
- **管理界面**：经验树、详情、草稿编辑、审计记录、场景与会话入口。

## 结构（骨架）

```text
experience-main/
  crates/
    experience-core/     # 纯逻辑：state/runtime/matcher/learner/store/
                         # lifecycle/confidence/executor/usage（自 codex-main
                         # 搬运，待去 codex 耦合适配）
    experience-controller/  # 上层控制器：decide → 直接执行或委派 adapter
    agent-runtime/          # AgentRuntime trait（run -> RunReport）
    agent-codex/            # CodexExecAdapter（唯一 codex exec 包装 + 能力声明）
  apps/
    experience-launcher/ # 薄启动器 exe（一次打包；prior-art 式启动逻辑）
    experience-server/   # 本地 API 进程（磁盘代码，随改随编）
  ui/www/                # 前端静态页（刷新即生效，无需打包）
```

> 产品定位：Experience 是本体，Agent（含 codex）是“外包 executor”；不对
> codex 做产品级改造/重建；本仓库配套的 Codex fork 只作为可插拔 runtime。

## 快速开始

```powershell
# 1) 构建（Rust）
cargo test --workspace --offline
cargo build -p experience-server --offline

# 2) 生成便携包（dist/Experience：launcher + server + ui）
powershell -ExecutionPolicy Bypass -File scripts/build-app.ps1

# 3) 运行：双击 dist/Experience/Experience.exe（自动起本地服务并打开浏览器）
```

- 数据目录：`dist/.experience-home`（store.json / usage.json /
  learning-l1.json / agents.json / sessions.json）
- 真机验收脚本：`scripts/accept-*.ps1`（详见 docs/rerun-matrix.md）

## 项目状态

- 学习与资格链（L1/L2）、外层委派闭环（L3）、内层 Gate 接管（L4/M5）
  均已通过真机验收；
- **预设经验 12 场景真机全部 PASS**（全自动 3 / 半自动 3 / 仅参考 3 /
  中间流程 3，见 docs/preset-scenes.md）；
- 当前重点：**规模与使用**——经验树/场景管理、编辑与个性化、外部 Agent
  材料吸收、UI 专项改造；
- 明确边界：不做多 Agent 自适配，不默认引入向量检索，不自动激活经验。
- 交付状态与缺口关闭记录：docs/delivery-status.md（2026-09-12：P0/P1 已关闭，
  多页面 UI + 吸收页 + U7 自动化验收通过）。

## 文档索引

| 文档 | 内容 |
|---|---|
| docs/usage-guide.md | 使用说明与第一性问题（规模、五类经验、经验如何形成） |
| docs/experience-app.md | 本地应用（launcher/server/UI）与运行方式 |
| docs/l1-learning-design.md | 学习回流与 L1–L4 分级口径 |
| docs/l2-qualification-design.md | 资格验证与生命周期 |
| docs/l3-delegation-design.md | 委派闭环与先动手 |
| docs/stage-a-absorb.md | 外部 Agent 材料吸收（run-notes 解析/promote） |
| docs/ui-module-spec.md · docs/ui-project-plan.md | 管理界面规格与实施计划 |
| docs/rerun-matrix.md | 全部真机/回归复跑矩阵 |
| docs/productization-security-execution-plan.md | 安全边界与执行/验证面产品化规划 |

## 许可与上游

- 本仓库使用 Apache-2.0（见 LICENSE）；
- 配套 Codex fork 源自 [openai/codex](https://github.com/openai/codex)
  （Apache-2.0），保留其 README/许可与 upstream 关系；fork 的本地实验
  改动（内嵌 Experience Gate 等）以 **DeepSeek 版本 Codex** 在本地复现，
  不要求对外发布 fork 分支；
- 本仓库中的经验核心（store/state/runtime 等）部分由该 fork 提炼而来，
  对外以本仓库为准。

---

## 开发账本（历史记录）

> 以下为内部开发过程记录，保留用于审计与复现，不代表对外承诺；
> 对外介绍以本节之上内容为准。

## 当前状态

- M0 资产搬运完成：`crates/experience-core/src/experience/*` 从 codex-main
  复制（Apache-2.0，LICENSE 随附），并完成独立编译适配：
  `experience_management` 依赖抽离为自洽的 `experience_usage`（usage.json
  单文件实现），4 处 let-chains 改写成 edition 2021 兼容形式。
- **P1 M1 完成（Domain Model）**：`crates/experience-core/src/domain/`
  落地 v1.3 架构的编译态 schema——ActionPattern（机械 trigger）、Predicate /
  StateFact（evidence-bearing）、Experience（预编译 workflow + pre/post
  conditions + verification + failure/undo policy）、GateDecision /
  GateHitResult（控制权事务：completed ⇒ verified 由构造器锁死，partial/
  failed 必须携带 executed_side_effects）。共 10 个单元测试，`cargo test
  --workspace` 103 passed。
- **P1 M2 完成（Store + 手工 Experience）**：`crates/experience-core/src/
  store.rs` 提供版本化 JSON 信封（schema_version=1）、ACTIVE 匹配面索引
  （按 trigger tool 键控）、原子落盘与 schema 校验；`experiences/
  create_probe_file.json` 为手工注入的验收经验（fixture 守卫测试保证随时
  可加载、可命中）。`cargo test --workspace` 108 passed。
- **P1 M3 完成（Runtime）**：`crates/experience-controller/` 编排同步 Gate
  闭环——Store 候选（trigger 粗滤）→ preconditions probe（三值，Unknown
  不放行）→ Match → 预编译 workflow 执行（LocalRunner：write_file）→
  失败即停 → 验证 postconditions（ReadFile/Probe 取证）→ State' facts →
  返回 completed/partial/failed。控制权事务由 `ExperienceGateRuntime::gate`
  一次性同步返回。`cargo test --workspace` 117 passed。
- **P1 M4 完成（Codex Adapter，模型 A）**：codex-main `16067215b`——探针
  升级为真实内嵌 Experience Gate（`FunctionCall → ActionProposal →
  ExperienceGateRuntime.gate()`，同一同步边界）；`EXPERIENCE_GATE_STORE`
  共享 store，`EXPERIENCE_ENABLED` 总开关生效；确定性测试证明 HIT 后原
  Tool 不 dispatch、Experience 真实写文件并验证、合法输出注入、loop 继续。
- **Experience App 骨架完成**：launcher（薄 exe）+ server（本地 API）+
  ui/www（磁盘静态页）三层；冒烟：POST 注入经验 → 列表/详情/状态/删除
  API 通、静态页可访问、launcher 自动起 server 并打开浏览器（详见
  docs/experience-app.md）。
- **Agent 管理视图完成**：/api/agents CRUD + managed 就绪检查（真实
  codex 探测为 ready、坏命令显式 error）+ UI Agents 页。Agent 配置支持
  **指向 agent 应用目录**（自动发现）或经**本地文件浏览**选择
  .exe/.lnk（快捷方式解析目标 exe）；默认种子只查本机真实安装
  （官方安装目录 / PATH），不写死学习用地址；label 仅显示名，模型/密钥
  属 agent 应用自身（compatibility.md §4 边界）。
- **桌面应用壳 + 会话入口完成**：UI 升级为侧栏应用（会话/经验/Agents）；
  会话页选择 executor 委派 `codex exec`（后台运行 + 轮询终态 + 完整输出
  可展开），error 态拒绝委派（失败有声）；`scripts/build-app.ps1` 产出
  `dist/Experience/` 便携包（launcher 一次打包，server/ui 磁盘化）。
- **会话运行保障完成**：实时输出/运行秒数、一键终止、重启后遗留 running
  会话标记为“中断”——executor 不再是黑盒。**自动超时默认撤除**（真实
  codex 任务常远超墙钟限制）：终态只由进程真实退出或用户主动终止产生；
  仅显式设置 `EXPERIENCE_SESSION_TIMEOUT_SECS` 才启用 opt-in kill。
- 待办：真机联调（配好 DeepSeek 密钥后跑通一次真实会话）、把 M4 内嵌
  Gate 结果纳入使用统计、经验自动沉淀入口。

迁移细节见 `experience-migration.md`。

## 最新状态（2026-09-08）

- P1 范围裁定（p1-development-plan v1.4）：核心闭环 M1–M4 + Experience App
  壳均属 P1；“多 Agent 适配”仍非目标（只接 codex）；
- 契约收敛：codex exec 包装唯一化为 `crates/agent-codex`（声明式能力
  TaskExecution 已进代码），experience-server 会话委派路径复用
  `spawn_codex`；agent-runtime 不再自带 CodexAdapter；
- Gate 保险丝（命中上限，滚动窗口内超限降级 MISS）与命中日志带经验名已
  加入 codex-main；workflow 步骤间可取消（request_cancel）；
- `cargo test --workspace`：129 passed（agent-codex 1 / agent-runtime 1 /
  controller 8 / core 108 / server 11）；
- 进行中：SessionChannel P0（`codex app-server --stdio` → thread/start →
  事件流，session-channel.md）。

> 2026-09-08 补充：computer-use 环境政策禁止自动化 Codex 桌面 UI——
> attached 架构不受影响，执行通道二分为：Protocol Host 探针
> （probe-e-fork-session.py，文件副作用 + nonce 判定）与 Human-in-the-loop
> 剪贴板交接（Experience 只组装任务 + 观察工作区），见 session-channel.md
> §B2。

> 2026-09-08（P0-3/P1 修复）：P0-1/P0-2/P0-3 全通过并接入 Experience——
> session 记录带 thread_id + trace（终态 append 不覆盖）、channel=session
> 走 SessionHost、/resume 同线程续跑、session cancel 经 flag kill host；
> codex_home 只来自 agent 配置或 env（无硬编码）；resume 单测、trace
> 延续回归、channel 校验均已补。

## 双主线进度（2026-09-09）

两条工作线分开记账，避免把“SessionChannel / Learning”与“Experience
Gate”混为一谈：

- **SessionChannel / Learning 线（本仓）**：P1 resume + P2 五项收敛完成
  ——session_host 公共内核（item1）、优雅收尾（item2）、早期失败回填
  thread_id（item3）、typed trace v1 冻结（item4，含 phaseC 审查加固：
  混合 legacy 会话整体拒绝、`trace_reader` 下沉 experience-core + 4 单测、
  ToolResult.ok 移除并明写“v1 不记单步成败”、`finish_session_channel`
  结构性去 trace 参数、cancel 写入 `failed{phase:"cancelled"}`）。L0
  smoke 已升级为 typed v1 契约探针（Draft、`source_trace_version=1`、
  不落 store）。**L1 自动沉淀已完成**（2026-09-09）：CandidateWriter
  权力边界 + necessity + 确定性 distill + validate 前两级 + 服务端会话
  终态钩子 + learning-l1.json audit 账本；真机冷沉淀 PASS（真实
  commandExecution → CANDIDATE）与同任务重跑 repeat 拒 PASS；下一步 =
  L2 资格形成链（输入契约 docs/p2-trace-v1.md，分级口径
  docs/l1-learning-design.md）。
- **L3 委派闭环（2026-09-09）**：域层 D1–D4 已落地（`delegation.rs`）。
  范围标签：**L3 v1**（f659bf9）= 任务入口计划性委派（ACTIVE → plan 名单
  → delegate 事件/账本；无 ACTIVE 纯委派零事件），真机
  `accept-l3-delegate.ps1` PASS；**L3 v2**（本批，脚本+fixture 见
  2cb0a81，实现 0be587e）= 任务文本机械匹配 ACTIVE → write_file-only workflow 本地先
  执行并验证 → v1 fallback 委派原任务 + 已完成步骤/已验证状态；
  `TraceEvent::Delegate` 新增结构化 `plan` 字段（截断不丢 selected
  名单，旧 JSON 兼容）；ledger 分层
  `experience_execution` / `delegate` / `delegation_completed|failed`，
  委托/任务结果不污染 Experience confidence（D4）。L4/M5（内层 Action
  Gate 接管）已完成，见下方 Experience Gate 线。
- **L3 外层闭环 WP1–WP4（2026-09-09，108b688/8c421ff）**：
  `usage.json` 持久化 ConfidenceRecord（experience_execution 写回，
  GET /api/usage 可观测）；负证据触发 Decay（ACTIVE→DECAYING，store
  envelope `pinned` 豁免，低频不衰减）；validate/revalidate 走 fs State
  源资格链（L2 seam 3 关闭）；`EXPERIENCE_INJECTION_POLICY` 注入策略
  （默认 off，injected/omitted 审计）；真机 `accept-l3-loop.ps1` 两轮
  PASS（usage successes 1→2、repeat 拒、无 decay）。Task Segment/全已知
  与 resume 语义裁定见 docs/l3-delegation-design.md §10。
- **Experience Gate 线（codex-main）**：M1–M4 完成（内嵌
  `ExperienceGateRuntime`，codex-main `16067215b`）；**M5 真实 Gate 闭环
  已完成（2026-09-09）**：gate 化 codex.exe 重建（19:18→20:07 含 L4
  usage），真机四问 PASS（`accept-m5-gate.ps1`，产物
  `codex-main/accept-m5-artifacts/m5-20260909-201123`）：stderr 真实
  `GATE HIT experience=create_probe_file`、原 Tool 未 dispatch（marker
  判别）、probe.txt 由内嵌 Experience 写并验证、exit 0、usage.json
  hits=1/misfires=0/invalid=0/experience_only。**L4 usage 写回已完成**
  （codex-main `7db612899`）：GateHitResult → usage.json 分层记录 +
  no-token fixture。L0–L4 文档终点已到达（多 Agent/模型 B 仍冻结）。

## 阶段状态与下一阶段（2026-09-09 收口）

- **WP1–WP7 可声明完成**（审查收口）：全部提交、双仓工作树 tracked 干净、
  测试全绿（experience-main workspace 191 passed / 1 ignored；codex-main
  gate 单测 2 passed + M4 确定性复跑 ok），真机证据齐备
  （accept-l3-loop PASS、accept-m5-gate PASS）。
- **总进度（文档 ladder）**：L0–L4 全程到达文档终点——L1 自动沉淀、
  L2 资格链（fs 源/Decay/pin/usage）、L3 外层编排（execute-first +
  usage 回写闭环）、L4/M5 内层 Gate 真机接管均已闭合；
  P1/P2/SessionChannel 资产全部可复跑。
- **非阻塞剩余（登记不关闭，不属于 WP1–WP7 范围）**：P2-4 委派文本
  重复执行已知段的语义裁定（需你决定）；L2 pin API 已就绪但 UI 未接；
  L1 脏轨迹 LLM compiler（write/read 蒸馏恢复）；跨仓 usage/审计一致性
  与一键复跑矩阵（列入 Stage S）。
- **下一阶段（Stage S）**：见 docs/next-stage-plan.md——P2-4 已按选项 A
  裁定（44a22d7），随后收 pin UI / LLM compiler / 跨仓一致性。
- **Stage S 推进（2026-09-09）**：S1 ✅（44a22d7，真机无 known-step
  重做）；S2 后端 ✅（9a05594），UI 部分转交后续 UI 专属模块；S3 ✅
  （82ccbdb + 真机 dirty→LLM compile→CANDIDATE，23:27:57）；S4 ✅
  （docs/rerun-matrix.md）。复跑清单见 docs/rerun-matrix.md。

## 项目总进度（2026-09-09 分析）

按文档 ladder，Experience 核心主线已完成到文档级终点：

- L0–L4 全程闭合：契约探针 → 学习（L1）→ 资格（L2，fs 源/Decay/pin/
  usage）→ 外层编排（L3 execute-first + 回测闭环）→ 内层接管（L4/M5
  真机 Gate + usage 写回）；P1/P2/SessionChannel 资产全部可复跑；
- Stage S（收口与演进）S1–S4 已交付：S1 委派“勿重复执行”裁定、
  S2 pin/usage 后端、S3 LLM compiler（真机 CANDIDATE）、S4 复跑矩阵；
- 测试基线：experience-main 196 passed / 1 ignored；codex-main gate
  单测 2 + M4 确定性复跑 ok；真机证据分布在 scripts/accept-artifacts 与
  codex-main/accept-m5-artifacts；
- 冻结/非目标：模型 B、多 Agent、Task Segment 自动拆分、policy 上限
  参数化、默认 embedding/RAG。

## 下一阶段目标（2026-09-09 起）

核心主题从“把闭环做对”转向“把规模与使用做清楚”：

1. **使用与第一性文档**：docs/usage-guide.md（Q1 万级不均匀唤醒讨论、
   Q2 百万级匹配与自行调整、Q3 五类经验执行过程、Q4 经验形成与存储）；
2. **规模分层设计（G-Scale）**：分桶/家族共性归纳/冷热分层/倒排与可选
   embedding 候选层——只讨论与设计，不改变默认执行语义（Q1/Q2 落地
   候选）；验收=设计稿 + 决定性基准（桶内不 miss）；
3. **S2 UI 专属模块**（转交）：pin/unpin + usage/生命周期可视化的前端
   专项（HTTP/ledger 已就绪）；
4. **运营面**：批量导入/导出、审计查询面分页过滤、容量与衰减治理；
5. **跨仓一致性收尾**：rich/domain 双轨 usage 口径对照、schema 升版
   策略、复跑矩阵并入 CI 文档、codex-core 预存在 Windows 集成用例栈
   溢出评估。

## 下一阶段蓝图（Stage C，2026-09-10）

完整设计见 docs/next-stage-blueprint.md，要点：

- **computer-use 判断**：不内嵌为默认执行器（宿主级插件、副作用强度、
  多 Agent/模型 B 冻结）；预留 `capabilities: computer_use` adapter
  通道，实现冻结；
- **规模分层**：数据分区 → ACTIVE 索引面 → 确定性精排 → per-scope 使用
  规则 → usage 治理（Q1/Q2 的落地方案）；
- **树状管理**：Root→Scene(scope)→Family→Experience 叶子，只做组织/导航
  视图，匹配语义不变；
- **UI 专项**：Tree/详情/编辑/场景选择/审计（转交 UI 专属模块）；
- **状态层**：单一转移入口 + usage 证据 + scope 过滤视图；
- **编辑与场景选择权力**：actor 审计的 draft/edit/replace + 入口 scope；
  具身智能训练只留 channel（scope + capability allowlist + 导入导出 +
  State 快照），不展开。

详细开发计划见 docs/stage-c-plan.md：C1 scope+树/过滤 → C2 编辑权利 →
C3 UI 规格转交专项 → C4 computer-use/具身通道 → C5 规模索引设计+确定性
基准；真机预算 ≤2，其余为确定性/HTTP 冒烟。

**Stage C 进度**：C1 ✅（6b91b26：scope 元数据、入口 scope 过滤唤醒、
experience-tree、列表 scope/status/usage_min 过滤；HTTP 冒烟 PASS）。
C2 ✅（e1f5976：draft → body 编辑 → adopt，actor 审计；直接编辑非
draft 被拒；HTTP 冒烟 PASS）。
C2.1/2.2 ✅（ce82494：display alias + user usage/confidence additive
元数据；deny 仅过滤用户唤醒面，证据/生命周期不动）。
C4 ✅（1c21a38：capability 词表 + 会话 allowlist + export/import/snapshot
具身通道，HTTP 冒烟 PASS）。
C5 ✅（1ca2515：scale-index 设计稿 + 10k 确定性正确性 + 100k 手动基准）。
C1 隔离真机回归 ✅（b5331d4：scene-a 会话仅唤醒 scene-a，exec_b=0）。
C3 UI 规格 ✅（docs/ui-module-spec.md，实现转交 UI 专项）。
UI 专项 U0–U6 ✅（46afd02：树/详情/编辑/审计/会话 scope；U7 人工浏览器
验收待执行，清单见 docs/ui-module-spec.md §4）。
Stage A 吸收面 A1–A3 ✅（277ae8b：参考层 + ingestion 通道 + 参考注入；
A4 转交 Trae，A5 用其过程材料验收，见 docs/stage-a-absorb.md）。
Stage A 增强 ✅（5084bfa：上传 run-notes.md 自动解析 → parse 自动填充 /
run-notes 一步式落库；用户无需逐行手填）。
Stage A 放行 ✅（3306b24：promote 不再要求 env 开关，用户点击即模型调用；
返回可能问题点 warnings；编译 120s 超时；无助手才结构化拒绝）。

工程纪律不变：脚本/fixture 先提交 → 实现 → cargo test 全绿贴原始计数 →
真机 ≤2 → 产物与 fixture 同步 → README 账本更新。

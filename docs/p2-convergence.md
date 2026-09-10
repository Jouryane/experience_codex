# P2 收敛：Runtime / Session 语义（2026-09-08 定稿）

## 目的

P0（能力）+ P1（最小 Experience/resume）已证明路径成立；P2 在做 L1
Learning 之前冻结 **trace 与 Session 语义**，避免“先蒸馏、后改语义来源”。

## 范围（按序）

1. **session_host 去重**：`run_session_task_until_turn` / `resume_thread`
   抽公共内核（spawn → initialize → (thread/start|thread/resume) →
   queue/add → queue/start → 事件循环 → 收尾）；
2. **host 优雅收尾**：先关 stdin、有界等待 flush，再 kill——根治首轮
   kill 遗留 “active/pending turn” 导致 resume 脏状态的问题；
3. **早期失败回填 thread_id**（thread/start 成功后任何失败都带上 id）；
4. **trace 升级 typed v1**：结构化事件 + `schema_version` + 终态/轮次/
   失败语义 + 完成证据——L1 的唯一合法输入；输入只取“单写者”trace
   （见进度与放行 §1），不得把双写历史当输入；
5. L1/L2/L3/L4 在 v1 冻结后接入（session-channel.md L0 smoke 已验契约）。

## 非目标

- 不做多线程并发 session；不做 host 崩溃自愈（保持“重启标记中断”）；
- 不引入 MCP/Skill/RAG。

## 轻量原则（2026-09-08）

Experience 库应保持**最小必要信息**，因为部分经验会随任务注入 agent
流程，冗余直接换算成 token：

- Experience 本体只存“可执行转移”：trigger / pre/postconditions /
  预编译 workflow / verification——不存日志、不存叙事、不存推理过程；
- trace 只存**语义里程碑**（typed v1 事件），噪音在写入前折叠，不落盘；
- Learning 蒸馏输入 = 最小成功路径证据；失败/审计属于 usage/audit，
  不进入经验本体；
- 存储冗余与 token 消耗不可两全时，默认选择轻量；重上下文只在用户显式
  需要时经 usage/audit 侧查询。

## 进度与放行（2026-09-08 审查 + 执行）

状态对账（按上文范围序号）：

- item 1（公共内核去重）✅ `46a6a22` + 真机修正：三个入口收敛为一个
  `run_session_driver` 内核，并随内核统一了 pre-thread 收尾；
- item 2（host 优雅收尾）✅ `c7b95c0`；
- item 3（早期失败回填 thread_id）✅ `af04df9` + mock 回归；
- item 4（typed trace v1）✅ 设计稿 docs/p2-trace-v1.md + 事件/持久化/
  UI/迁移落地（C1–C3），learning_smoke 已升级为 typed v1 消费（C4，
  `source_trace_version=1`）。phaseC 审查加固批（2026-09-09）：混合
  legacy 会话整体拒绝（P1-1）、`trace_reader` 下沉 experience-core +
  4 单测（P2-1）、ToolResult.ok 移除并规格化“v1 不记单步成败”（P2-2）、
  `finish_session_channel` 结构性去 trace 参数（P2-3）、cancel 写入
  `failed{phase:"cancelled"}`（D3）；
- item 5（L1–L4）🔶 L1 ✅ + L2 core ✅（C1–C4，60ad87a 起至 05ac4a7/
  ecc1b42）：转移矩阵（激活仅 VALIDATED）、五级门 + StateProbe 接口
  冻结、Evidence/Activity 正交、transition 单一入口 + API 收口 +
  force_activate reason 审计；fixture 无 token 回归入库。接缝（如实）：
  Decaying 生产者/decay 事件待 L3 usage；confidence 落 usage 待接线；
  VALIDATED 对真实候选生效依赖 L3 State 源接线；write/read 蒸馏已诚实
  拒收。L3–L4 ⏳。

**放行条件执行结果**（审查三项，均已关闭）：

1. **trace 单写者 ✅**：运行中 live 推送是 trace 唯一写入者；终态
   `finish_session_channel` 只写 status/summary/thread_id（`ce7e9c4`）。
   2026-09-08 真机 app 全链路（dist + session + resume 两轮）复核：
   `submitted/accepted/agent_started/turn_completed` 各恰好 2 次，无
   双写。v1 冻结语义来源沿用该规则，不得把双写历史当作 L1 输入。
2. **dist + 真机验收 ✅**：`scripts/build-app.ps1` 已重建 dist；
   真机两轮 PASS（fork codex + `.codex-exp-home` + 真实 API）：
   `session_host_resume_probe`（新 example）首轮 until_turn + 同 thread
   resume 二轮均 ok、thread_id 不变、teardown 后无脏 active turn；
   app HTTP 全链路同验通过。复跑：`cargo run -p agent-codex --example
   session_host_resume_probe --offline`（库级），或
   `scripts/accept-app-two-rounds.ps1`（dist app HTTP 级）。
   **真机修正（语义结论）**：fork app-server 在 queue/add 后可能
   auto-dispatch（idle-lifecycle wake）直接启动 turn；queue/start 竞态
   返回 “queue is empty” / “active or pending turn” 时视为“已自动
   启动”，进入事件循环等待（milestone
   `notification:queue/start:turn_already_started`），其他应答错误才
   失败——早期 until_turn 忽略应答、ee828cb 起显式失败的假设均被真机
   证伪，现语义与 fork 行为一致。
3. **确定性 mock 覆盖 ✅**：9 条用例覆盖 until_turn / verify-run /
   resume 三路径 + pre-thread 失败（thread/start、thread/resume）+
   `auto_start`（queue empty 后 turn 完成）+ `active_turn` 容错分支。

已登记债项：

> 2026-09-09 更新（WP1–WP7 收口）：L1–L4 全部到达文档终点——L1 自动
> 沉淀、L2 资格链（fs 源/Decay/pin/usage）、L3 外层闭环（execute-first
> + usage 回写，accept-l3-loop PASS）、L4/M5 内层 Gate（accept-m5-gate
> PASS + usage 写回）。剩余非阻塞 seam / 待裁定项见 README「阶段状态」
> 与 docs/next-stage-plan.md。

- pre-thread（initialize/thread/start/thread/resume 阶段）send/超时失败
  漏停 host → ✅ 随 item 1 统一由 `send_handshake` / `reply_handshake`
  先停 host 再 Err（`46a6a22`）；
- cancel 在协议阻塞应答（≤30s）期间不即时生效 → 接受，UI 行为注明；
- commit message 编号口径统一为 plan item 序号（`46a6a22` 起）；
- resume 会覆盖 session.task，原任务只留在 trace/summary → v1 定义
  “轮次”边界时一并定 schema。

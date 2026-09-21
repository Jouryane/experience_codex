# 05 · Experience 接进 DeepSeek Harness 会是什么样

> 所有座位名称与占用者都来自 `deepseek-ai/deepseek-harness` tag `dsh-v0.1.5-rc.2`
> 的生成文档，不是推测。
>
> **2026-09-17 状态**：DSH 不是活跃通道（私信发出无回音），活跃通道是 openai/codex
> （见 [08](08-outreach-and-interfaces.md)）。本文件保留，因为**座位占用表**与
> **N1–N9 清单**是可移植的分析，而且 Codex 侧呈现出完全同形的空缺。

## 一、DSH 自带的七张图（我们据此叠加）

| 图 | 文件 | 类型 |
|---|---|---|
| 模块依赖图 | `docs/module-graph.md` | generated |
| 工具 schema 目录 + 包图 | `docs/tool-catalog.md` | generated |
| **能力 seam 与核心服务** | `docs/capability-seams.md` | hybrid |
| base 组合 | `apps/cli/composition.md` | hybrid |
| **事件生产者/消费者矩阵** | `docs/event-producer-consumer.md` | hybrid |
| **agent turn / step 生命周期** | `docs/agent-lifecycle.md` | curated |
| **工具执行管线** | `docs/tool-execution-pipeline.md` | curated |

加粗的四张是回答本问题必需的：它们同时给出**座位**和**占用者**。

## 二、叠加图

```text
DSH 现有管线                                         Experience 的落点
──────────────────────────────────────────────────────────────────────────────
followup(content)
  → turn/start
  → agent/pre-step  waterfall  ← 16 个监听者        [E1] 任务入口（第 17 个）
        agent, agent-instructions, compaction-basic,       只能“委托 + 前置”，
        goal-round-driver, hooks-claude-code,              不能短路
        hooks-codex, plan-mode, repeat-tool-reminder,      ← 且会与 compaction
        session-checkpoint-policy, session-reference,        / tool-skill 争预算
        subagent-in-process-driver, time-context,
        tmux-context, tool-cordis, tool-skill, tool-subagent
  → agent/request → prepareCall()
  → llm/stream → assistant/message
  → tool/call            (durable: 记的是模型原始参数)
  → tools/pre-execute   waterfall  ← 3 个监听者     [E2] 只放策略：deny / ask
        hooks-claude-code, hooks-codex, tool-jobs          ★ 表达不了“接管”
  → ctx.tools.guard()   单调 deny（不可被翻案）      [E3] 硬拒绝
  → ctx.approval        ask 的一次性批准            [E4] 不可逆动作确认
  → tools/execute       around-dispatch ← 2 个      [E5] ★ 接管执行
        session-checkpoint-policy, timeout-policy         不调 next() 直接产出结果
  → tool body
  → tools/post-execute  waterfall  ← 5 个监听者     [E6] 结果富化 / 参考注入
        hooks-claude-code, hooks-codex,                    (additionalContexts)
        repeat-tool-reminder, spill-policy, tool-fs-search
  → finalizeContent → tools/result → tool/result     [E7] 学习回流证据
  → agent/turn-stopping serial ← 2 个
  → turn/end
```

一句话：**Experience 在 DSH 里不是一个插件，是“五到六个座位上的多个插件 + 一次会话格式升版”。**

## 三、座位占用表（本文件最有用的产出）

| 座位 | 现有占用者 | 数量 | Experience 能不能用 | 用来做什么 |
|---|---|---|---|---|
| `agent/pre-step` | 见上图 | **16** | 能，但拥挤且顺序承重 | 任务入口识别、注入 |
| `tools/pre-execute` | hooks×2, tool-jobs | 3 | **不能表达接管** | 只放 deny / ask |
| `ctx.tools.guard()` | guard 组两包 | ~2 | 能 | 硬拒绝（单调、不可翻案） |
| `ctx.approval` | — | seat | 能 | 不可逆动作二次确认 |
| `tools/execute` | session-checkpoint-policy, timeout-policy | **2** | **能，这是正确的接管座位** | 执行前缀并返回等价结果 |
| `tools/post-execute` | hooks×2, repeat-tool-reminder, spill-policy, tool-fs-search | 5 | 能 | 结果富化、参考注入 |
| `tools/result` | agent-instructions, subagent-in-process-driver, tool-present | 3 | 能 | 学习证据 |
| `agent/turn-stopping` | hooks×2 | 2 | 能 | 续跑驱动 |

**最有用的一条观察**：大家会本能选 `tools/pre-execute`，但它的决策类型是
`allow | deny | ask`，注释明写 input rewriting is excluded（"arguments are
already logged and presented"）——**它压根表达不了“由我替你做”**。
真正能做接管的 `tools/execute` 是整条管线里**最空**的座位（只有 2 个占用者）。

## 四、新发现的矛盾 N1–N9（本轮新发现）

| # | 矛盾 | 依据 | 影响 |
|---|---|---|---|
| **N1** | **DSH 里不存在“LLM 之前的先动手”这个座位** | `agent/session-start` 是 emit-only 且“CANNOT block startup”，派发是 detached、"may miss the first request"；`agent/pre-step` 是**消息准入**瀑布（`reject` / `enter`），reject 会让本轮不产生 step | “先动手”只有两条路：在 pre-step 里做副作用（off-contract，且每步都跑），或等模型提出第一个工具调用后在 `tools/execute` 接管（on-contract，但已经晚了） |
| **N2** | **接管返回值必须伪装成原工具的 canonical value** | `ToolDefinition.output.schema` 强制；wrapper 产出的成功会被 re-normalize through the resolved output declaration | `step-gate.md` §9 的 `GateHitResult` **在 DSH 里没有位置**；只能拆成 `meta` + `additionalContexts` + 一个 durable 事件 |
| **N3** | **接管会产出一对自相矛盾的事件** | `tool/call` 记的是模型原始参数，`tool/result` 记的是 Experience 合成的结果 | 顶撞 “model-visible means logged”。要修就要**扩展 `SessionEventMap`** → 触发**会话格式升版 + 相邻迁移包**。**这是最贵的一条** |
| **N4** | **接管的时间预算由被替代的工具声明** | `timeout-policy` 是 `tools/execute` wrapper，依据 `ToolDefinition.timeoutMs` | C/D 档长链会继承一个为单次调用声明的超时 |
| **N5** | **compaction 会把注入的参考经验吃掉** | `compaction-basic` 监听 `agent/pre-step` 与 `agent/request-error`，压力下做 tool-result pruning + summary | 注入策略把注入当稳定输入；在 DSH 里它**只是一个可被压缩掉的普通上下文** |
| **N6** | **`agent/pre-step` 的 16 个监听者是顺序承重的** | waterfall：不调 `next()` 直接返回决策会**短路后续全部监听者** | Experience 想做入口识别就必须严格委托；同时 `tool-skill` 与它争同一个 prompt 预算 |
| **N7** | **接管必须不破坏既有不变量** | `ctx.invariants`（package-owned invariant registry）真实存在，"model-visible means logged" 是运行时断言 | 验收标准不是“功能跑通”，而是“**不破坏既有不变量**” |
| **N8** | **`session-checkpoint-policy` 也在 `tools/execute` 上** | 事件矩阵 | 注册顺序决定“接管是否被记入 checkpoint”；顺序本身成为语义 |
| **N9** | **接管时“原工具没跑”这件事没有一等的表达** | `PreToolDecision` 无结果变体；短路只体现在 wrapper 没调 `next()` | 与 N3 同源 |

### N1–N9 重评（2026-09-17，据 openai/codex 讨论区回复与代码核对）

核心变量是**拦截模式 vs 注册模式**：注册模式（`thread/start.dynamicTools`）下，客户端
注册自己的工具、被调用时自行执行并返回，**不冒充任何内建工具**。这使多数矛盾消失。

| # | 重评 | 说明 |
|---|---|---|
| N1 | **收窄** | 主动腿已有座位（动态工具，文档化、实验性）；只有“不经模型调用的触发”仍无座位。DSH 侧同样只收窄到 `ctx.jobs` |
| N2 | **只对拦截模式成立** | 注册模式下动态工具有自己的内容契约（`content_items` + `success`），不需要冒充被替代工具的 schema |
| N3 | **注册模式下平台已提供** | `TurnItem::DynamicToolCall` 是 v2 一等 item，`thread_history.rs` 可从 request+response 事件重建；拦截模式仍成立 |
| N4 | **注册模式下不成立** | 动态工具调用有自己的 request/response 与时长记录；拦截模式仍成立 |
| N5 | **仍成立** | 参考注入在两种模式下都只是上下文，都会被 compaction 处理 |
| N6 | **仍成立** | Codex 侧是 hook 契约，DSH 侧是 16 监听者的 waterfall |
| N7 | **注册模式下自动满足** | 不冒充工具、不改会话格式 → 不触碰既有不变量；拦截模式仍成立 |
| N8 | **仍成立** | DSH 侧 `session-checkpoint-policy` 的注册顺序仍是语义 |
| N9 | **注册模式下不需要** | 它本来就不是“原工具”；拦截模式仍成立 |

## 五、收尾：九条矛盾收缩成一句

九条里五条只在**拦截模式**下存在，而拦截模式恰好是 [C46](07-time-dimension.md)
里被动弹射那条腿所依赖的形态。于是整张清单收缩成一句话：

> **经验能不能在模型没有问的时候起作用？**

这句话同时是 Codex 侧与 DSH 侧唯一剩下的架构请求。两个平台呈现**完全同形的空缺**：
DSH 的 `PreToolDecision` 只有 `allow | deny | ask`，Codex 的 `PreToolUse` 结果只有
阻止 / 改写参数 / 附加上下文——**都能拦，都不能代做，也都不主动触发**。

(2026-09-16 曾把本节写成“要对 DSH 提的三条请求”，并因此加过一段修订说明；
现已删除正文，只留上面这句结论。原始意图见 git 历史。)

## 六、附：接线时会用到的相邻 API（已核对存在）

| 需要的语义 | DSH 现成的机制 |
|---|---|
| 按身份限定经验可见面 | `tools/*` 的 scope-filtered dispatch + agent preset + `ToolRestriction` |
| 把一条经验暴露给模型 | `ctx.tools.register`（经验从“被命中”变成“被调用”） |
| 组织/导航视图 | `session/event` 渲染 + ConversationNode |
| 启用/门控策略（替代 env 变量） | profile + `cordis.patch.yml` 的配置行 |
| 离线批量重算 | `ctx.sessionQuery` / `session-log-export` |

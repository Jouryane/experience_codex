# 工程架构文档

> 基线：Experience as Orchestrator + Step/Action Gate（同步接管）。
> 对应 codex-main 中可用的实现内容均标注来源，便于迁移对照。

## 1. 总览：Experience 是独立于 Agent 的运行时

```text
                  Experience Application（对话/管理入口，自有 HTML UI）
                                │
                ┌───────────────┴───────────────┐
                │                               │
        Experience Runtime              Agent Manager
        （独立进程；State / Decision /        （用户选择并配置 Agent）
         Workflow / Learning / Store）          │
                │                     ┌─────────┼──────────┐
                │                     Codex    Claude    Trae …
                │                     Adapter  Adapter   Adapter
                │                     │
                └─────────────────────┴───────────────────┘
                                │
                         Agent Executor（统一契约）
                                │
                                ▼
                            外部 Agent（LLM + Tools）
```

原则：Experience 拥有“调用/接管权”，不拥有 Agent（core-principle.md）。

## 2. 构件划分

| 构件 | 职责 | codex-main 可用内容 |
|---|---|---|
| Experience State | 环境探测与状态注册表（StateElement/conditions/快照） | `experience/experience_state.rs` |
| Decision（外层） | 任务/子任务：直接做 or 委派 | `experience/experience_runtime.rs` decide；run_turn 采样前缝隙（参考） |
| Action Gate（内层） | Action Proposal→副作用前同步接管 | stream_events_utils：FunctionCall→build_tool_call 前缝隙 |
| Experience Workflow/Executor | 已知状态转移（State→State'）兑现 | `experience/experience_executor.rs`（trait 化 runner） |
| Learner/Distill | RunReport→trace→necessity→蒸馏→激活 | `experience/experience_learner.rs`、experience-gen 思路 |
| Store/Usage | 经验本体+引用日志/统计/审计（唯一事实源） | `experience/experience_store.rs`、experience_management usage 概念 |
| Management/UI | 列表/详情/操作/HTML 页面/launcher | codex-main HTML 页与 launcher（可复用） |
| Master switch | EXPERIENCE_ENABLED 关闭时全停用 | codex-main 已实现的总开关 |
| 兼容接口 | Experience MCP / Skill（外部，不进核心路径） | codex-main MCP adapter 经验 |

## 3. 控制流（两层）

### 3.1 任务级委派（外层）

```text
任务 → Experience State 构建
     → Decision：
         ① 已知且可直接做 → Experience 兑现（State→State'）
         ② 做不了 → Agent Manager 委派指定 Executor（先推进世界状态，
            再附带必要经验/状态作为第二位上下文）
```

### 3.2 动作级同步 Gate（内层）

```text
LLM → Action Proposal（已生成、未执行）
       ↓
      Experience Gate（执行路径上的同步点）
       ├─ MISS → Tool Executor 正常执行
       └─ HIT  → Experience 兑现已知转移（可接管动作或整条状态转移链）
                 → 返回 Result → Agent 继续
```

红线：Gate 必须是执行路径同步点；外部“监听事件再回调”只是观察，不是
Gate（step-gate.md §6）。

## 4. 目录规划（experience-main）

```text
crates/
  experience-core/       # 纯逻辑（已从 codex-main 搬运，待去耦）
  experience-controller/ # 外层 Decision + Learning 回流
  agent-runtime/         # AgentRuntime/AgentTask/RunReport/AgentManager（已有）
  agent-codex/           # CodexExecAdapter（唯一 codex exec 包装 + 能力
                         # 声明；experience-server 会话路径复用 spawn_codex）
apps/
  experience-server/     # 本地 API 进程（磁盘代码，随改随编）
  experience-launcher/   # 薄启动器 exe（一次打包）
```

## 5. 关键工程决策

- Experience Runtime 与 Agent 之间只交换“任务进、RunReport 出”，无共享内部状态；
- 经验数据归 Runtime（store/usage 单一事实源），Agent 永不私藏；
- 同步 Gate 能力分级：Level 2（dispatch 前同步钩子，自建 Codex 提供）/
  Level 1（仅委派+观察回流）；
- Gate 同步边界 = **控制权事务**：Gate 返回即接管终结
  （completed/partial/failed 三态已定），不承诺世界状态原子事务/隐式回滚；
  异步只存在于任务级委派（Level 1），动作级 Gate（Level 2）无异步变体；
- **不静默 ≠ Gate 阻塞**：Runtime 不可用时 Gate 只产生 MISS（安全降级，
  不引入第三个行为语义）；disconnected 由编排层/UI 显式呈现（状态 +
  委派显式错误）。控制路径可安全降级，系统状态不能无声丢失；
- Executor 连接分托管型（headless 控制通道，Experience 拥有生命周期）与
  附着型（附加已开实例，不新开界面）；P1 用模型 A（Runtime 内嵌 codex、
  共享 store），模型 B（外部 Runtime + IPC）只作 Adapter 契约设计；
- 模型无关：DeepSeek/OpenAI 等仅作为被委派 Agent 的 LLM 后端。

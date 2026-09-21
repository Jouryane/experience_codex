# 核心原则：Experience 拥有“调用权”，不拥有 Agent

> 2026-09-06 定稿。这是 experience-main 的架构第一原则，任何实现不得违反。

## 1. 我们一直在摆脱的

原版 Codex：LLM 是绝对控制器，代码只是执行环境。我们的经验系统此前
也走过弯路——把 Experience 塞进某个 Agent 内部，或让 Experience“拥有”
一个 Agent。两者都会退化为“谁内嵌、谁私有”。

## 2. 正确的结构

```text
                    Experience Application（用户界面/入口）
                          │
          ┌───────────────┴───────────────┐
          │                               │
  Experience Runtime              Agent Manager
  （独立进程，持有 State /          （用户选择/配置 Agent）
   Decision / Workflow /            │
   Learning / Store）               ├─ Codex  Adapter
          │                         ├─ Claude Adapter
          │                         ├─ Trae   Adapter
          │                         └─ …（可插拔）
          └───────────────┬───────────────┘
                          │
                     Agent Executor（统一执行入口）
                          │
                          ▼
                      外部 Agent
                          │
                          ▼
                   LLM + Tools（Agent 自有）
```

关键表述：

1. **Experience 不拥有 Agent**：它不内嵌、不 fork、不绑定任何 Agent；
2. **Experience 拥有调用 Agent 的权力**：经由统一 `Executor` 抽象
   （AgentRuntime trait），Runtime 在需要时“外包”任务；
3. **Agent 是超级插件 / 外包商**：用户可配置 Codex / Claude Code / Trae /
   DeepSeek Harness / WorkBuddy 等；
4. **先适配行业楷模 Codex**：不逐一适配所有 Agent 内部实现；把 Codex
   作为第一个（也是基准）executor agent，跑通闭环后其它 Adapter 照抄
   同一接口；
5. **Experience Runtime 是独立进程**：独立于具体 Agent App 运行，Agent
   只是它随时可以调用/替换的外包执行器。
6. **介入层级（Step/Action Gate）**：复杂任务中 Experience 的介入点在
   “Action Proposal 已生成、副作用尚未发生”之间（见 step-gate.md），
   而不是“每步决策前”或“任务整体前”。外层 Runtime 管委派，内层
   Action Gate 管动作接管；两者叠加，缺一不可。

## 2.5 经验影响的传递优先级（2026-09-06 校准）

Experience 对 Codex 的影响，按以下顺序（前者的权重远大于后者）：

1. **直接做掉（改变世界状态）**：Experience 收到任务/子任务后，先把
   “自己已经能做的部分”真正执行——恢复环境、创建资产、部署配置、
   完成已知流程——让世界状态被推进；
2. **只把剩余部分交给 Codex**：做不了/需要推理的部分才委派；此时把
   “必要的经验/状态”作为 Codex 的输入（参考文本或交接状态）带上，
   但这是第二位、补充性的。

推论：Codex 的起点永远建立在“Experience 已推进的世界”之上；上下文
参考永远不能替代 Experience 先动手。凡经验能做而未做、只把文本塞给
LLM 的实现，都违背本原则。

## 2.6 Experience 是并列行为路径，不是外挂大脑（2026-09-06 校准）

- Memory 告诉 LLM 过去发生了什么；Skill 告诉 LLM 怎么做；
  **Experience 直接把已学会的行为兑现到现实世界（State → State'）**；
- Experience 与 LLM 在 Agent 执行过程中并列：LLM 探索未知，
  Experience 兑现已知；
- Gate 是执行路径上的**同步点**（Action Proposal 后、副作用前），
  任何“外部监听 Codex 事件再回调”的实现都不是 Gate，只是观察；
- 匹配模型是 `(State, Intent/Action) → Known Transition → State'`，
  不是孤立地把 State 映射到经验。

## 3. 控制流（Runtime → Executor → LLM）

```text
任务 → Experience State
     → Experience Decision
         ├─ 可处理：Experience 行为直接执行（快路径，零/少 LLM）
         └─ 不可处理：委托 Agent Manager
                         └─ Executor(Agent) → LLM+Tools → 结果
     → 结果回流：necessity gate → distill → activate/update
```

## 3.5 delegate 是一等操作（2026-09-07 定稿）

Experience 是**没有 LLM 的 agent**：它能做的（已知转移）直接兑现；做不了
的，执行 **delegate 操作**把剩余任务外包给 executor。因此“把问题递给
codex”不是外围进程调用，而是 Experience 行为模型里与 `write_file` /
`exec_command` 平级的 action：

```text
delegate {
  agent: codex
  task: 剩余问题（Experience 先做掉能做的部分——core-principle §2.5）
  workspace: 项目根（写权限/审批由 codex 自己的配置决定，Experience 不设沙箱）
  references: 可选参考经验（第二位、标注可质疑）
}
```

工程含义：

- 决定“能不能做”= 匹配（无 LLM）；决定“怎么做”的推理全部在 executor 内，
  Experience 内部永远不偷偷长出 LLM；
- delegate 进入 trace：Learning/necessity gate 得以回答“这次为何外包、
  结果能否蒸馏成经验、下次自己能否做掉”——这是 Experience 变强的闭环
  入口；
- 委派只传“剩余部分”，参考经验是第二位补充；上下文不能替代 Experience
  先动手。

## 4. 工程含义

- Executor 是契约：`run(task, context) -> RunReport`，对上层隐藏 Agent
  内部；
- Runtime 与 Agent 之间只有“任务进、报告出”，没有共享内部状态；
- 经验数据（store/usage）归 Runtime，Agent 永不私藏；
- Codex 内嵌实现（codex-main）降级为参考实现，用于对照“Executor 外包”
  行为是否一致。

# Experience as Orchestrator：从“改造 Codex”到“控制 Agent Runtime”

> 日期：2026-09-06
> 状态：战略转向（待实施里程碑确认）

## 1. 为什么转向

原路线是“改动 codex 内部，让它具备 experience 能力”，但现实瓶颈：

- 官方 Desktop/登录路径会强制使用官方二进制（登录后我们的改造不生效；
  官方运行时还会“自愈”覆盖替换）；
- 官方 UI 无法显示 experience，前端只能另起 HTML；
- 强拼装导致使用不便、兼容性差。

同时观察到：OpenAI 开源 Codex（Apache-2.0，提供源码/构建/standalone
executable）的意图是让 Codex 成为 **Agent Runtime 标准**：

```text
              OpenAI Model
                   ↑
     ┌─────────────┴─────────────┐
     │                           │
 Codex Runtime               Other Runtime
     │
     ├── CLI  ├── IDE  ├── Desktop  └── Custom UI
```

experience_codex 的正确位置不在“Codex 内部那一格”，而在更上层：

```text
                  USER / EVENT
                        ↓
         Experience（state + decision + workflow + learning）
           上层控制器：先试、参考、委派、吸收、沉淀
                        ↓
      ┌─────────────────┴──────────────────┐
      │            Agent Runtime            │
      │         (adapter / 可插拔执行器)      │
      ├────────┬────────┬────────┬──────────┤
      │ Codex  │ Claude │ Trae   │ DeepSeek │ ... 
      │        │ Code   │        │ harness  │
      └────────┴────────┴────────┴──────────┘
```

Experience（state+Runtime）成为上层控制器，Codex 只是它可调用的一种
Agent 执行器；执行器还可替换为 Claude Code / Trae / DeepSeek harness /
WorkBuddy 等。这是兼容性与可落地性的真正来源。

## 2. 控制流（外部控制器版）

```text
输入 → Experience State 构建（自身探测环境）
     → Experience decide：
         ① 已知且同构可信 → Experience 直接执行（自身工具执行器）→ 完成
         ② 经验先做、判据不全 → 委派 agent 检查/接管
         ③ 相关但需改编 → 通过 adapter 注入参考后委派 agent
         ④ 无经验 → 委派 agent 自由执行
     → agent 返回 RunReport（结构化 trace/输出/结果）
     → necessity gate → distill（experience-gen skill）→ activate
```

关键点：

- Experience 拥有自己的“已知路径执行器”（快路径，零/少 LLM）；
- 未知/复杂路径通过 **Agent Runtime Adapter** 委派给任意 agent；
- 学习输入不再是 codex core 内部 hook，而是 adapter 返回的 RunReport；
- 可选 LLM（质疑/参考）由 Experience 自己持有，与具体 runtime 解耦。

## 3. 与现有资产的关系

- `core/src/experience/`：state/runtime/decision/learner/store/usage/
  lifecycle 大多是**纯逻辑、无 codex 内部耦合**，可抽取复用；
- turn.rs 内嵌接线：降级为“Codex 内嵌参考实现”；`EXPERIENCE_ENABLED`
  开关使内嵌路径可关闭，双轨并存过渡；
- management（store/usage/UI）天然属于控制器侧，直接复用。

## 4. 迁移路线

- **M0**：冻结现状（内嵌路径 = 参考实现，继续可测）。
- **M1**：定义 `AgentRuntime` trait：
  `run(task, context, options) -> RunReport{trace, stdout, exit, …}`
  + 首个 `CodexExecAdapter`（调用 `codex exec`，stdin 任务，
    解析输出为 RunReport）。
- **M2**：ExperienceController 复用现有决策/learner，把学习输入从内部
  hook 替换为 RunReport 转换（trace 适配）；验证闭环：冷任务委派
  CodexExecAdapter → 生成经验 → 二次任务由控制器直接完成（零委派）。
- **M3**：管理与 HTML UI 对接控制器侧 store/usage；启动器/桌面壳
  作为 UI 载体。
- **M4**：其它 adapter（Claude Code / Trae / DeepSeek harness…），
  以及 reference 注入协议（不同 runtime 的上下文注入方式）。

## 5. 边界与风险

- 权限/沙箱：由 adapter 侧 agent 的策略决定（codex exec 的 approval/
  sandbox 配置照旧）；
- reference 注入精度依赖 adapter 能力：M1 先以“stdin 提示注入参考文本”
  实现弱注入，后续按各 runtime 能力增强；
- trace 结构化程度因 runtime 而异：先统一到 RunReport 最小集
  （task/stdout/actions 摘要/结果），逐 adapter 扩展。

## 6. 待确认

1. 确认本转向为项目主线（内嵌路径保留为参考实现）；
2. 从 M1（trait + CodexExecAdapter 最小闭环）开工？

## 7. 落地点：Codex 的 Action Gate 缝隙（2026-09-06）

复杂任务中 Experience 的介入点是 **Action Proposal 生成后、副作用发生前**
（详见 experience-main/docs/step-gate.md）。Codex 原生存在该缝隙：

```text
ResponseEvent::OutputItemDone(FunctionCall)     ← 模型已生成工具调用
   → stream_events_utils::handle_output_item_done
   → ToolRouter::build_tool_call / ToolCallRuntime（真正执行前）
```

Action Gate 应插入 `build_tool_call` 之前：把 `(tool, args, cwd, state)`
交给 Experience Runtime 做本地查表级判断；HIT 则由 Experience 直接执行
（可接管单个 Action 或一整条状态转移链）并合成 Result 回给 LLM，MISS
原样放行。此前我们在 run_turn“每轮采样前”实现的 decide 属于任务/轮次级
Gate，对复杂任务内循环不够——需叠加本 Action Gate。

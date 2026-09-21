# 0002 · canonical Runtime 与 Codex 两条执行缝

> 状态：accepted · 2026-09-20
> 关联：[0001](0001-primary-goals-and-drift-guard.md)、
> [讨论 25](../discussions/25-first-principles-review-and-next-research.md)、
> codex-main `accept-m6.ps1`。

## 决策

`experience-main` 的 P1 Runtime 是唯一 canonical 执行运行时。Codex 侧只保留
两条薄适配缝，不再维护第二套可执行 Experience 逻辑：

| 缝 | 位置 | 触发 | 行为 |
|---|---|---|---|
| turn 级 | `core/src/session/turn.rs` | 任务文本命中 `task` / `task_prefix` | 完整命中时跳过 LLM；前缀命中时先执行已验证部分，再把剩余交给 LLM |
| dispatch 级 | `core/src/stream_events_utils.rs` | 模型 FunctionCall 的工具 + 参数命中 | 原工具不 dispatch，P1 Runtime 执行并验证，合成 `FunctionCallOutput` 返回原 loop |

两条缝共用同一个 `ExperienceGateRuntime`、同一个 Store、同一套 policy/执行器/
验证逻辑。

Gate 决策固定为三态：`Hit`（执行接管）、`Satisfied`（后置已成立，成功 no-op）、
`Miss`（原路径）。`Satisfied` 由 M7 真机重复调用发现并补入；它保证同一经验
重复使用时不会因为前置消失而落回 LLM。

## 开关规则

- 设置 `EXPERIENCE_GATE_STORE`：启用 canonical 执行路径；
- `EXPERIENCE_TASK_GATE=1`：即使没有显式 Store，也允许尝试 turn 级 Gate；
- `EXPERIENCE_LEGACY_OUTER=1`：仅在没有 canonical Store 时启用旧的
  `core/src/experience/*` 外层链，用于历史兼容；
- 未设置任何 Experience 开关：保持原生 Codex 行为。

旧 `core/src/experience/*` 暂时保留管理面和历史学习实现，但**不再默认拥有
执行路径**。

## 验收

`codex-main/accept-m6.ps1` 四场景：

1. 完整任务：fake provider 记录 0 次请求，文件正确，usage=`experience_only`，
   备份存在；
2. 中途动作：M5 原验收 PASS，原工具不执行；
3. 部分任务：prefix 只执行一次，剩余由 LLM 完成，usage=`experience_first`；
4. 未知任务：LLM 基线完成，Experience usage 为空。

## 后果

- 代码不再需要“Scheduler + Arbiter + 两个控制器”；
- Codex 仍拥有未知任务的探索权；Experience 只接管已验证转移；
- 管理面与执行面的 Store 收敛是后续独立工作项，不影响本决策的执行语义。

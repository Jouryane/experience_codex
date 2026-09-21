# M6：canonical Experience Runtime 与两条执行缝

> 2026-09-20。取代早期“先改内嵌 runtime、再补 Gate”的阶段口径。
> 本文描述当前执行面；旧 `core/src/experience/*` 只保留管理与显式兼容职责。

## 1. 结论

Codex 不再需要把 Agent loop 重写成 Scheduler + Arbiter。当前架构是：

```text
TurnInput
  → P1 Runtime: task / task_prefix 决策
      完整命中 → 执行 + 验证 → 返回，不采样 LLM
      前缀命中 → 执行 + 验证 → 把剩余任务交给 LLM
      未命中   → 原生 LLM loop
  → FunctionCall
      → P1 Runtime: dispatch Gate
          命中       → 执行 + 验证 → 注入 FunctionCallOutput，原工具不 dispatch
          已满足     → 成功 no-op（第三态 Satisfied），原工具不 dispatch
          未命中     → 原生工具执行
```

LLM 保留未知任务的探索权；Experience 拥有已验证转移的执行权。

## 2. 代码位置

| 位置 | 职责 |
|---|---|
| `core/src/experience_p1_gate.rs` | 共用 bridge：Store 加载、task/task_prefix Gate、dispatch Gate、usage、备份根 |
| `core/src/session/turn.rs` | turn 级缝：完整命中跳过 LLM；前缀命中注入已验证状态 |
| `core/src/stream_events_utils.rs` | dispatch 级缝：原工具前接管，合成合法 `FunctionCallOutput` |
| `core/Cargo.toml` | 依赖 `experience-core` / `experience-controller` |

## 3. 开关

```text
EXPERIENCE_GATE_STORE       canonical Store 路径；设置即启用执行面
EXPERIENCE_GATE_CWD         可选，测试/非默认工作目录
EXPERIENCE_GATE_BACKUP_ROOT 可选，执行前备份根
EXPERIENCE_TASK_GATE=1      可选，强制尝试 turn 级 Gate
EXPERIENCE_LEGACY_OUTER=1   仅兼容旧外层链；没有 canonical Store 时生效
```

## 4. M6 四场景

`accept-m6.ps1` 真实执行：

| 场景 | 判据 |
|---|---|
| 完整任务 | fake provider 请求数 0；产物正确；usage `experience_only`；备份存在 |
| 中途动作 | 原工具不 dispatch；产物由 Gate 执行；usage `experience_only` |
| 部分任务 | prefix 只执行 1 次；剩余由 LLM 完成；usage `experience_first` |
| 未知任务 | LLM 完成；Experience usage 为空 |

fixtures：

- `fixtures/m6/task-full-store.json`
- `fixtures/m6/task-partial-store.json`
- `fixtures/m6/empty-store.json`
- `fixtures/m6/fake_provider.py`

## 5. 已知边界

- 管理面/CLI/app-server 仍读写旧 `<codex_home>/experience/store.json`；
  执行面使用 `EXPERIENCE_GATE_STORE`。两者的 Store 收敛是后续独立工作项。
- 旧 `core/src/experience/*` 仍被管理面使用，但不在默认执行路径。
- turn 级 `task` / `task_prefix` 是适配器伪工具名，不是 Agent 可调用工具。

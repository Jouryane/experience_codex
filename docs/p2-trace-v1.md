# P2 item 4：typed trace v1（2026-09-09 设计稿）

## 目的

冻结 L1 Learning 的输入语义：trace 只存**语义里程碑**（typed v1 事件），
噪音在写入前折叠/丢弃；双写历史与 legacy 字符串 trace 永不进入蒸馏输入
（p2-convergence.md 放行 §1 / item 4）。

## 事件模型（experience-core `TraceEvent`）

```text
kind: submitted | accepted | agent_started | already_started | reasoning
    | turn_completed | tool_call | tool_result | failed | legacy
```

- `submitted`：本轮任务已入队（一轮的开始标记，round 边界由它切分）；
- `accepted` / `agent_started`：turn 已启动；
- `already_started`：queue/start 竞态命中 auto-dispatch（fork 行为，
  见 p2-convergence.md 放行 §2 真机修正）；
- `reasoning`：每段连续推理折叠为单条（写前折叠，沿用现 collapse）；
- `turn_completed`：本轮完成标记；
- `tool_call { name, call_id?, args_summary? }` / `tool_result { name,
  call_id? }`：可执行转移证据（L1 workflow 步骤来源）。`args_summary`
  为**可选、截断（≤240 字符）的参数摘要**（2026-09-09 L1 决策）：供
  确定性蒸馏产出诚实 workflow 参数；原始完整参数留在 audit 侧，不进
  trace。**v1 不记录单步成败**（无 ok 字段）：工具失败只以 `failed`
  事件/终态 error 表达；工具自我修正后完成属于正常成功路径；
- `failed { phase, message }`：协议/queue/host 早期失败（session 终态
  error 也拒绝蒸馏，双保险）；`phase="cancelled"` 表达用户取消
  （outcome_cancelled 写入），使 trace 与 summary 语义一致；
- `legacy { label }`：只读迁移壳，**禁止进入蒸馏输入**。

白名单：只落盘上述语义事件；`thread/status`、`tokenUsage`、`rateLimits`、
`turn/diff`、`item/*`（userMessage/agentMessage/fileChange 等非工具
通知）一律不落盘——真实 trace 由 48–71 行/轮收敛到 ~10 行/轮。

职责划分（phaseC 审查裁定 D1）：tool_call/tool_result = **步骤证据**；
workspace 副作用验证（probe / postcondition 谓词）= **完成证据**（法定
来源）；通知类 = audit 侧，不进 Experience。

## 轮次边界与任务归属

- session 追加式 trace 以 `submitted` 切分轮次：第 i 个 `submitted` 开始
  第 i 轮；
- `Session.round_tasks: Vec<String>` 与轮次一一对应：create 记 task，
  resume_start 追加 task（解决“resume 覆盖 session.task”债项）；
- **trace 含任何 `legacy` 事件的会话整体拒绝**（P1-1 修正：迁移前缀会
  使 resume 后轮次索引错位，最干净且符合“legacy 永不进蒸馏”）；
- L1 蒸馏输入 = **某一轮**（轮次选择是参数，默认 Last），其 task 取
  `round_tasks[round]`（缺省回退 session.task）；非目标轮属于
  usage/audit，不进 Experience；未来失败恢复类经验可选 earlier 轮，
  不改 schema。

## 终态/失败/完成证据

- 完成证据 = 终态 `completed` + 该轮含 `turn_completed` + thread_id 存在
  （verify 路径另有真实 workspace 副作用，属 probe 层证据）；
- 失败 = 终态 `error`（任何阶段）或轮内含 `failed` 事件 → 该轮永不
  蒸馏为 Success；取消同理；
- `schema_version`：trace 事件 schema 版本常量 1（`source_trace_version
  = 1` 于蒸馏元数据，替代 L0 的 0）。

## 持久化与迁移

- `Session.trace: Vec<TraceEvent>`（serde tag=kind, camelCase）；
  sessions.json 写 schema_version 2；
- 旧 v1 文件迁移：字符串 trace 逐条转为 `legacy` 事件，`round_tasks`
  由当前 task 兜底；读取后下次持久化即 v2；
- UI 按事件渲染 label（legacy 显示原字符串），无需清洗历史。

## 注入原则（轻量）

- 注入 agent 的参考经验只来自蒸馏后的 Experience 本体，不注入 trace；
- trace/usage 全量保留在本地，深度查证按需查询（audit 侧）。

## 实施拆分

1. C2 agent-codex（消费 experience-core `TraceEvent`）：白名单分类 +
   `failed` 事件 + 回调与 outcome 改 typed；mock 回归同步；
2. C3 experience-server：typed 持久化 + schema v2 迁移 + `round_tasks` +
   UI 渲染兼容；
3. C4 experience-core：`trace_reader` 下沉（轮次切分/过滤/任务归属 +
   4 类单测），learning_smoke 消费 reader，`source_trace_version=1`；
4. 回归：workspace 全量测试 + 真机验收脚本适配 typed trace 断言。

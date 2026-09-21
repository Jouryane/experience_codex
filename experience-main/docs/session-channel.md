# SessionChannel：把 Experience 接进 codex 的真实会话语境

> 2026-09-07。目标不是 demo：Experience 委派 codex 时必须具备 codex
> **真实 agent 的执行能力**（工作区写权限、审批/策略、可见会话），而不只是
> 连上 LLM。两条路径：

## 路径 A（探针，先证明 headless 全能力）

官方 `codex exec` 已提供最小无人值守写盘旋钮：

```text
codex exec --approve-for-me -C <workspace> <prompt>
```

- `--approve-for-me`：自动审批，**隐含 workspace-write 沙箱**（比
  danger-full-access 更小）；不能与显式 `-s` 同用（CLI 会拒绝）；
- 不推荐 `--dangerously-bypass-approvals-and-sandbox`（无沙箱，需用户显式
  接受才考虑）。

探针：`scripts/probe-a-exec-write.ps1`（在真实用户终端运行）。通过标准：
codex 能创建/写入工作区内文件并回读成功。

## 路径 B（产品目标：挂进 codex 真实会话语境）

Experience 要让 codex 以**它自己应用运行时的完整语境**执行：

```text
Experience（项目/会话上下文 + delegate 操作）
      │  SessionChannel adapter
      ▼
codex 真实会话（官方 app-server daemon / queue / remote-control）
      │  审批、工作区、可见性沿用 codex 自身语境
      ▼
事件回流 → Experience UI
```

官方 CLI 已暴露的实验性面：

- `codex queue --thread <uuid|name> --message <text>`（送消息进既有会话，
  支持 `--remote ws://…`）；
- `codex app-server daemon` + `codex app-server proxy`（本地控制 socket，
  JSON-RPC/stdio 代理）；
- `codex agents`（列出共享 daemon 上的会话）；
- `codex remote-control`（远程控制，实验性）。

P0 探针（只读为主，`scripts/probe-b-session-channel.ps1`）回答四个问题：

1. app-server daemon 是否在运行（`daemon version`；无 `status` 子命令）；
2. `codex agents` 需要 `--remote` 才能列会话——端点来自 daemon/proxy；
3. `queue` 的接口形状（thread 归属、message 文本、remote 端点）；
4. `app-server proxy`（stdio 桥）与 remote-control 是否可用作 Experience
   的长期通道。

## B1 裁决（2026-09-07 实测）

- Windows 官方 CLI：`app-server daemon` 生命周期 **Unix-only**；`codex
  agents` 是 TUI（非终端直接报错）→ “附着桌面 daemon”路线在 Windows
  不可行；
- 可行通道：**官方与 fork 的 `codex app-server` 都支持 `--stdio`
  （默认 listen stdio://）**，协议为 newline-delimited JSON-RPC 2.0：
  `initialize → initialized → 方法…`（codex-main
  `run-appserver-smoke.ps1` 已验证该线格式）；
- app-server 协议 v2 提供完整 agent 会话方法（schema 由
  `codex app-server generate-json-schema --out <dir> --experimental`
  生成）：
  `thread/start · thread/resume · thread/queue/add · thread/queue/start ·
  turn/start · turn/steer · thread/settings/update · thread/list …` 以及
  事件通知流（`thread/realtime/*`、`turn/*`、`item/*` 等）。

因此 B 的实现 = **Experience spawn `codex app-server --stdio`（真实二进制、
真实用户配置）→ JSON-RPC 建会话/送任务/改 settings → 订阅事件流**。
“codex 桌面窗口同步可见”在 Windows 官方版仍无公开注入接口——呈现放在
Experience 内（同一 codex 会话语境的流式视图）。

## B2 实施约束与两条通道（2026-09-08）

- 环境政策确认：computer-use 明确禁止自动化 Codex/ChatGPT 桌面 UI。
  因此“attached UI 代操作”不能由本模型的 UI 自动化实现——这只限制当前
  AI 操作环境，**不否定 attached 架构**（把“Experience 能否附着 Codex”
  与“模型能否替用户点击 Codex”彻底分开）；
- 通道 1 = Protocol Host（探针 A/B）：验证
  `codex app-server --stdio` 能否独立承载完整 Session Host。判定标准不是
  thread/start 是否返回 ID，而是 **workspace 内真实文件副作用 + nonce
  验证**（`scripts/probe-e-fork-session.py`，fork 二进制 + `.codex-exp-home`）；
  - A：可承载 → Adapter 直接走 app-server（不改官方产品、不碰 Desktop）；
  - B：fork 同样静默退出 → 结论升级为“Windows 公开 app-server 入口本身
    不是独立 Host”，停止在 app-server 上投入；
- 通道 2 = Human-in-the-loop Attached Adapter（剪贴板交接）：Experience
  只组装 Task/Context/经验 → 放上剪贴板 → 用户在**当前** Codex 会话粘贴
  发送；Experience 不碰权限边界（不登录/不读 token/不建 Session/不改
  sandbox/不操纵 lifecycle），只做 **Workspace Observer**（轮询文件副作用）
  完成验证与学习。官方 Desktop 是否开放通道不再等待。

### P0 步骤

1. ✅ A 探针通过：`--approve-for-me`（隐含 workspace-write）headless 可写；
2. ✅ B1 裁决：stdio app-server 是唯一可行通道；schema 已生成到
   `%TEMP%\exp-schema\v2\`（`ThreadStartParams`、`ThreadQueueAddParams`、
   `TurnStartParams`、`ThreadSettingsUpdateParams` 等）；
3. ⏳ probe-c：真实用户语境下 initialize/initialized/server/diagnostics
   握手验证（`scripts/probe-c-session-stdio.ps1`）——✅ 通过
   （2026-09-07：官方 app-server stdio JSON-RPC 通，codexHome 指向
   真实用户配置，收到服务端通知）；
4. ⏳ 依据 schema 构造最小 `thread/start`（workspace/任务文本/settings），
   跑通“建会话 → 事件回流 → 写 probe 文件”闭环；
   - `thread/start` 顶层无必填字段 → probe-d 先验证最小建会话
     （`scripts/probe-d-session-thread-start.ps1`）；
   - 之后接 `thread/queue/add` + `thread/queue/start`（input 结构待按
     `ThreadQueueAddParams` schema 定）驱动真实 turn。
5. ⏳ 用 SessionChannel 替换当前 `codex exec` 委派路径（exec 保留为
   fallback）。

## P0-1 最小正式实现（2026-09-08 定稿）

模块：`crates/agent-codex/src/session_host.rs`（能力：SessionChannel）。

生命周期（单会话互斥；协议应答有界等待 30s；任务执行**无墙钟**）：

```text
spawn(codex app-server --stdio)
  → initialize(id1) / initialized
  → thread/start(id2, {cwd, threadSource})
  → thread/queue/add(id3, {threadId, clientUserMessageId, input:[text]})
  → thread/queue/start(id4, {threadId})
  → 读取事件（原始行 → milestone 轻量提取）
  → completion = 调用方 verify 谓词为真（文件/nonce 等真实副作用）
  → teardown（kill host）
```

返回：`ok / thread_id / milestones / error`。milestone 先做轻量版
（Submitted/Accepted/AgentStarted/TurnCompleted + 原始方法名），P0-3 再
做语义化 canonical trace。cancel/resume/reuse/concurrent/host-restart 属
P1，不在本步实现（预留 thread_id 与 trace 容器字段）。

验收：probe-e 场景（fork + `.codex-exp-home`，workspace 建唯一 nonce 文件，
verify 谓词本地校验内容）经由 SessionHost 一次跑通；协议超时/启动失败
返回显式 error。

P0-3 进展（2026-09-08 实测）：轻量 canonical trace 已生效——真实编辑任务
输出 `userMessage / reasoning / tool_call:commandExecution /
tool_result:commandExecution / agentMessage / turn_completed` 的语义序列
（P0-1 PASS + P0-2 PASS + trace 验证 PASS）。下一步：trace（含 threadId）
持久化进 experience-server 会话记录 + UI 实时视图。

## L0 Learning smoke（2026-09-08）

隔离契约探针（`crates/experience-core/examples/learning_smoke.rs`）：
真实 canonical trace（sessions.json，只读）→ distill → 候选 Experience
通过 schema 校验。护栏：scratch 输出即弃；绝不写 store.json；status=Draft
（永不 Active）；候选标注 `source_trace_version=0`（语义未冻结）。

结果：**PASS**——数据契约端到端成立。同时暴露两点：旧 trace 存在 2.9 万
条 delta 噪音（collapse 修复后的新 trace 不受影响）；distill 只能读到
tool_call 标记，语义厚度不足。结论：正式 L1 必须以 **typed trace v1**
（结构化事件 + 版本号 + 终态/轮次/失败语义 + 完成证据）为输入；P2 收敛
完成后接入。

验收（P0 通过后）：Experience 能在**自己的测试会话**里 queue 一条任务、
观察事件回流、并在 codex 应用内可见；会话归属/权限校验明确；之后再把
SessionChannel 作为 Executor Capability 实现（start_turn + 事件流 +
cancel），delegate 操作按 core-principle §3.5 携带 workspace/参考经验。

## 边界（保持）

- 密钥/登录/审批/沙箱策略属于 codex 自身；Experience 只选“接哪个会话、
  任务是什么、经验可参考哪些”；
- 路径 A 的 `--approve-for-me` 属无人值守自动审批——写坏自负，由用户在
  codex 侧或 Agent 配置中显式授权，不在 Experience 里替用户决定；
- 在 P0 通过前不向任何真实会话发送消息（避免污染用户会话）。

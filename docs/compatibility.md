# 适配性文档

## 1. Executor 契约（对外适配的唯一接口）

```rust
AgentTask { text, cwd, reference? }
RunReport { ok, summary, raw_output }
trait AgentExecutor { id(); run(task) -> RunReport }
```

Executor 能力按声明形式化（与 step-gate §10 一致）：

```text
Executor Capability
  ├── TaskExecution        // Level 1
  ├── SessionChannel       // workspace 感知的会话委派：start_turn(workspace,
  │                        //   task, references) + 事件流 + cancel
  └── ActionInterception   // Level 2：proposal 拦截 + result injection
```

Adapter 必须显式声明自身能力；禁止行为猜测。

### 1.2 委派会话语义（2026-09-07 定稿）

- Experience 是**没有 LLM 的 agent**；把问题递给 executor（delegate）是它
  的一等操作（core-principle §3.5），携带：剩余任务文本、workspace（项目
  根）、可选参考经验；
- **不设自动超时**：真实 codex 任务常远超任何墙钟限制。会话的终态只有两
  种——executor 进程真实退出（完成/崩溃）或用户主动终止；“静默”由
  可见性解决（事件流 + “N 分钟无新输出”的知情提示），不由 kill 解决；
- 写权限/审批/沙箱由 executor 自己的配置决定（Experience 不设沙箱、不
  采集密钥）；Experience 只负责把 workspace 指到真实项目。**例外**：
  SessionChannel 需要指定 codex 配置目录（codex_home）以触达同一
  thread——来源只能是 agent 配置或环境变量 CODEX_HOME，禁止硬编码开发机
  路径；
- 用户“终止执行”保留为控制权操作；仅当显式设置
  `EXPERIENCE_SESSION_TIMEOUT_SECS` 时才启用 opt-in 自动 kill。

### 1.1 Executor 连接语义（避免通讯层静默）

Executor 与 Experience 之间存在两种关系，Adapter 必须显式声明属于哪种：

- **托管型（managed）**：Experience 按用户配置拉起 executor 并拥有其
  生命周期（spawn / 握手 / 心跳 / 退出清理）。这是 Gate 通道（Level 2）
  的唯一允许形态；
- **附着型（attached）**：用户已自行启动 executor，Experience 启动时
  发现并附加到已有实例，**不得再新开一个界面**。附着失败不是静默——
  状态栏显示 Disconnected 并提供“拉起/重试”入口。

自动拉起语义：托管型 executor 以 **headless / 控制通道模式**启动（例如
自建 codex 的 app-server 通道），不强制弹出交互 UI；UI 是否同时打开是
用户选项。Experience 关闭时，子进程跟随退出还是驻留必须显式配置，不留
僵尸进程。

连接状态机（Experience UI / Agent Manager 始终可见）：

```text
configured → launching → connected
                  ↘ error / disconnected（可重试）
```

核心原则：**“不静默”不等于“Gate 阻塞”。控制路径可以安全降级，但系统
状态不能无声丢失。** 因此：

- Gate 侧：Runtime 不可用时只产生 `MISS`（原 Tool 照常执行）——这是唯一
  允许的安全 fallback，**不引入新的 Gate 行为语义**（Gate 永远只有
  Hit / Miss，不存在第三个“disconnected”分支）；
- 编排层/UI 侧：disconnected 必须显式呈现（状态 + 委派任务时的显式错误），
  用户看到“Codex Executor: Disconnected”，而不是“什么也没发生”。

两层各司其职：Gate 静默安全降级 + 编排层显式告警，缺一不可。

## 2. Codex 适配（行业基准，优先完成）

### 2.1 Level 1（官方/外部形态）

- `CodexExecAdapter`：`codex exec`，stdin 任务 → stdout RunReport；
- 或 app-server：任务作为 turn/start 输入，事件流作观察与回流。

### 2.2 Level 2（自建 Codex 构建）

- 在 stream_events_utils：`FunctionCall` 到达、`build_tool_call` 解析成功
  后、**dispatch 前**，插入同步 Gate 回调（codex-main 预留该缝隙）；
- Gate 命中后由 Experience 兑现并合成 FunctionCallOutput 返回，MISS 原样
  放行——实现“LLM→Action→Experience→Result→LLM”。

### 2.3 Level 2 可行性实验（P0 验证项）

必须在 codex-main 验证：

1. FunctionCall 到达、build_tool_call 前，能稳定拿到 Action + 必要
   State/context；
2. Experience HIT → 不 dispatch 原 Tool；
3. 能生成合法 FunctionCallOutput（Codex 正常消费）；
4. Codex 采样循环正常继续。

实验未通过则“同步接管”不成立——它是 Level 2 的 gate。

### 2.5 M4 拓扑决定（2026-09-07 定稿）

- **P1（现在）：模型 A —— 内嵌 Runtime**。codex 构建内直接持有
  ExperienceGateRuntime，与 Experience Application 共享同一个 store
  路径；Gate 通道没有“进程间不可用”问题，连接语义服务于委派路径。
  这是 M4 验收（真实 LLM → Gate → Runtime）的最小化路径；
- **最终目标：模型 B —— 外部 Runtime + executor 客户端**。Experience
  进程持有 Runtime/Store，codex 的 dispatch 钩子经 IPC 询问；此时
  §1.1 的连接生命周期才是 Gate 通道的保证。模型 B 只做契约设计，不在
  P1 实现（IPC 通道选择届时再定）。

### 2.4 兼容接口（边界明确）

- **Experience MCP**：供外部 Agent 主动查询经验/回写反馈的 adapter，
  不是 Runtime 核心执行路径；
- **Experience Skill**：让 Agent 理解经验规范（如何参考/质疑/汇报），
  同样是外围文档，不是决策通道。

## 3. 多 Agent：暂时冻结

本阶段只适配 Codex（Apache-2.0，有源码与构建权）。Claude Code / Trae /
DeepSeek Harness / WorkBuddy 等暂不展开；解锁时按同一 Executor Capability
契约接入。

## 4. 配置与运行

- Agent 由用户在 Agent Manager 配置：id + label + kind + mode +
  **agent 应用目录（directory）**（可执行文件由 Experience 按 kind 在目录内
  发现）或可执行文件（executable）覆盖；exe / 快捷方式（.lnk）通过
  本地文件浏览选择（Experience 解析 .lnk 目标，绝不猜测安装路径）；
- label 只是显示名（列表识别），与模型/供应商无关；
- **密钥 / 登录态 / 模型供应商属于 Agent 应用自身**：用户已在对应 agent
  应用内配置（如 codex 的 config/provider），Experience 不采集、不存储、
  不传参；Experience 只负责按配置“拉起并调用”；
- Experience 应用使用自己的 home（经验库/store/agents/sessions），与
  Agent 的配置目录无关；
- 模型选择是 Agent 内部属性，Experience 不绑定任何模型供应商。

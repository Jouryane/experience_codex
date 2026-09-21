# Agent 控制流施工图（Experience 第一刀）

> 基线：`6478a751fde8884b2fdc76486fe23175a8e795d4`（见 `BASELINE.md`）
> 所有行号均为该基线下的实测行号，改动后需重新核对。
> 实施状态：第五阶段第一刀已落地 —— `experience/` 模块骨架 +
> `run_turn` 门禁（默认 MISS / InvokeLLM，行为不变）。

## 1. 目标

把控制权从"LLM 中心"反转：**Experience Runtime 位于 LLM 之前**，拥有
"是否调用 LLM"的决策权与阻断权。失败判据：如果最终结构仍是
`LLM → Experience Runtime`（把 Experience 变成 LLM 的一个工具），项目即失败。

```
User Turn → Task → State Update → Experience Runtime
                                     ├── HIT   → Experience Workflow → Tool → State Update
                                     └── MISS  → LLM → Tool → State Update → Experience Candidate
```

## 2. 当前控制流（实测）

### 2.1 整体链路

| 层级 | 位置 | 说明 |
|---|---|---|
| 入口 | app-server / TUI | 外部请求进入 Session |
| Session | `session/mod.rs`（`impl Session`） | 会话生命周期、上下文、审批、扩展 |
| Thread 管理 | `thread_manager.rs`（`ThreadManager`） | 线程（thread）调度与会话建立 |
| Task | `tasks/mod.rs`、`tasks/regular.rs:39` | `RegularTask::run`：循环调用 `run_turn` |
| Turn | `session/turn.rs:156` `run_turn` | 单轮驱动的核心循环（见 2.2） |
| 模型请求 | `session/turn.rs:1362` `run_sampling_request` | 构造 prompt、重试循环 |
| 流处理 | `session/turn.rs:2207` `try_run_sampling_request` | `client_session.stream`（2238）即真实 LLM API；事件循环中执行工具 |
| 工具执行 | `ToolCallRuntime`（`turn.rs:1375` 创建） | 复用 `tools/`、`exec_policy`、sandboxing 通道 |

### 2.2 `run_turn` 内部结构（关键缝隙）

```text
run_turn (turn.rs:156)
├─ run_pre_sampling_compact (172)
├─ capture_step_context (210)                  // 首个 step
├─ build_skills_and_plugins (253)
├─ loop (304)
│  ├─ capture_step_context (337)               // 后续 step（含工具结果后的重采样）
│  ├─ record_step_world_state (368)
│  ├─ sampling_request_input = history.for_prompt(...) (373)   ★ 模型可见输入已就绪
│  ├─ run_sampling_request (384)               ★★ 第一刀落点：LLM 请求缝隙
│  ├─ collect_post_sampling_state (424)        // token 占用、pending input
│  ├─ needs_follow_up ? continue : break
│  └─ run_turn_stop_hooks (505)                // 已有 should_block / should_stop 阻断先例
```

`tasks/regular.rs:77` 在 `run_turn` 返回后检查 pending input，有则立即以空输入再跑一轮，
无则结束本 Task。

### 2.3 其他采样入口（必须知悉，避免"唯一门禁"误判）

- `compact.rs:743`、`compact_remote_v2.rs:386`：自动压缩 / 远端压缩时的内部 LLM 调用。
  属于上下文维护，不属于 Agent 动作，第一版不 gate。
- `hook_runtime` / `guardian`：独立子系统。`guardian` 用 LLM 做审批裁决，与
  Experience 的确定性决策不同，保持独立。

## 3. 插入点结论（第一刀落点）

**位置**：`run_turn` 循环内、`turn.rs:373`（输入构造完成）与 `turn.rs:384`
（`run_sampling_request` 调用）之间。

**为什么是这里**：

1. 此处已具备全部决策所需上下文：`step_context`（环境/工具/配置）、`world_state`、
   `sampling_request_input`（模型可见输入）、`turn_context`。
2. 循环是 per-sampling-request 的：首次请求与工具调用后的每次 follow-up 都经过这里，
   单点即可覆盖整个 Agent 循环的 LLM 门禁。
3. 对 `run_sampling_request` 内部零侵入，第一刀改动面最小、回滚最安全。

**决策语义**：

```text
should_call_llm?
├── ExecuteExperience(id) → 不调用 run_sampling_request，改走 experience_executor
├── InvokeLLM             → 继续 run_sampling_request
└── AbortOrAsk            → 停止自动执行，转 LLM 或询问用户（Conflict）
```

**已落地（第一刀）**：`ExperienceRuntime` 挂在 `SessionServices`（与
`AgentControl` 同级）；`run_turn` 在构造 `sampling_request_input` 之后、
`run_sampling_request` 之前调用 `should_call_llm(&ExperienceState::default())`。
当前恒返回 `InvokeLLM`；HIT / AbortOrAsk 分支已写入 match，但暂回退 LLM 并记录
warn 日志。`ExperienceState` 目前传默认空状态，后续阶段在 turn 开始与工具执行后
维护真实状态。

## 4. State 更新时机

- **Turn 开始**（进入 `run_turn` 的 loop 前）：初始化 / 重建 `ExperienceState`，
  `current_goal` 取自 `turn_context` 与用户输入。
- **每次采样请求后**（`turn.rs:424` 附近）：根据 `SamplingRequestResult` 更新
  `last_action` / `last_result` / `confidence`。
- **每次工具执行后**：工具结果反映到 State。第一版以 sampling 输出为准，
  第二刀候选是在 `try_run_sampling_request` 的流事件循环里加细粒度观察点。
- **Turn 结束**：写回 `experience_store`，更新命中经验的置信度（成功 / 失败 / 衰减）。

## 5. 目标模块布局（命名一律 `experience_` 前缀）

```text
codex-rs/core/src/experience/
├── mod.rs
├── experience_runtime.rs     // should_call_llm 决策（总控制器）
├── experience_state.rs       // ExperienceState：只描述"此刻状态"
├── experience_matcher.rs     // State → 命中的 Experience
├── experience_workflow.rs    // Experience: trigger / condition / actions
├── experience_executor.rs    // 复用 ToolCallRuntime / exec 通道执行工作流
├── experience_confidence.rs  // success / failure / repetition / decay → confidence
├── experience_store.rs       // 持久化 + 状态机（NEW→CANDIDATE→VALIDATED→ACTIVE→DECAYING→DISABLED）
└── experience.rs             // 可序列化的 Experience 结构
```

`ExperienceState` 的职责边界：只描述 Agent 当前处于什么状态。它不是 chat history、
不是 RAG、不是 memory、不是 prompt context。

## 6. 复用的既有机制（不另起炉灶）

- `ToolCallRuntime`（`turn.rs:1375`）：Experience executor 直接复用，不建第二套执行栈。
- `exec_policy` / `sandboxing` / `tools/`：审批与沙箱在工具层已存在，Experience 执行
  同样受其约束。
- `hook_runtime` 的 `should_block` / `should_stop`：控制面先例，证明在采样前阻断是可行的；
  Experience 与其平行，不混用（hook 是提示/审批语义，Experience 是确定性执行语义）。
- `guardian`：审批自动化的 LLM 裁决，与 Experience 的确定性决策保持独立。

## 7. 第一刀完成标准（验收）

- 单元测试：matcher 命中 / 未命中、confidence 阈值、冲突检测。
- 集成测试（mock `client_session` 或 instrumentation）：
  - Reflex 场景断言 LLM 调用次数 = 0；
  - Deliberation 场景 LLM 调用次数与基线一致；
  - Conflict 场景先停止自动执行，再转 LLM / 询问。

## 8. 风险与未决点

- 采样入口不止一个（compaction 等）：第一版只 gate `run_turn` 主循环，文档明确边界。
- 若后续需要拦截"单次工具调用"（而非整个采样周期），需要在
  `try_run_sampling_request` 流事件循环内加观察点——这是第二刀候选。
- 三层（Session / Task / Turn）是否各自维护独立 `ExperienceState`：第一版以 Turn 为粒度，
  Session 级经验共享留到后续阶段。

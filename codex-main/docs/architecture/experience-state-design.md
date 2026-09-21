# Experience State 层设计（深入构思）

> 状态：设计稿（阶段 3 收尾与阶段 4 集成的蓝图）
> 触发事件：2026-09-01 的 cargo-check 全量编译排障（50+ 分钟、15-20 轮 LLM
> 试错，最终定位为沙箱禁止 build script 生成子进程）——这正是状态层未来要
> 消灭的"顽疾"，也是本设计的靶点用例。

## 1. 定位（第一性原理）

经验状态层不是 chat history、不是 RAG、不是 memory、不是 prompt context。
它是：

> **经验所依赖的状态元素的投影，加上"可部署性"。**

两条铁律：

1. **命中经验时，快速唤起/部署状态**（环境配置、环境检测、网络配置、工具配置、
   运行条件），避免对 LLM 资源的浪费；
2. **状态可被 LLM 检查**：条件通过 → LLM 不读取；条件不通过、LLM 介入时 →
   读同一套 key（已有 `lookup` / `satisfies` / `handoff_payload` 为雏形）。

## 2. 这次 cargo-check 事件教会我们什么

复盘这次排障，消耗时间的本质不是"环境难"，而是：

- 重复的确定性排查（PowerShell 语法、`rust-toolchain.toml` 覆盖、找 gcc、
  PATH、CC、沙箱限制）被交给 LLM 一轮轮推理；
- 每一步的状态（当前 PATH、当前工具链、沙箱是否允许子进程）没有结构化记录，
  只能靠重读输出重新推断。

**如果状态层成熟**，一条 `cargo-check-env` 经验会这样工作：

```text
trigger: 任务含 "cargo check" 且 环境=Windows 开发机
conditions:
  runtime.subprocess_allowed == true        # 沙箱允许子进程？→ 决定能否编译
  toolchain.gnu_installed == true           # GNU 工具链在位？
  toolchain.override == "stable-x86_64-pc-windows-gnu"   # rust-toolchain.toml 已覆盖
workflow:
  deploy: toolchain.override=...            # 状态部署：一次设置，不再反复试
  deploy: path.prepend=w64devkit\bin
  deploy: cc=gcc
  check: runtime.subprocess_allowed
  run: cargo check -p codex-core --jobs 4
```

沙箱不允许子进程时，`runtime.subprocess_allowed == false`，条件不满足 →
经验直接回答"本环境无法编译，请到真实终端运行脚本"→ **零 LLM 试错**。
这就是"利用经验快速唤起状态、避免 LLM 资源浪费"的完整闭环。

## 3. 状态的粒度与生命周期

### 3.1 三层粒度

| 粒度 | 生命周期 | 内容 | 当前状态 |
|---|---|---|---|
| Session 级 | 跨 turn 持久 | 环境基线、工具集、配置、网络策略 | 未实现（每次重建） |
| Turn 级 | 单 turn 内连续 | goal、task、executed_steps、执行模式 | 已实现（`update_experience_state` 保留连续性） |
| Step 级 | 单次采样 | cwd、detections、last_action/result | 已实现（refresh） |

### 3.2 六阶段生命周期

```text
build（构建）→ refresh（刷新）→ verify（校验）→ deploy（部署）→ record（记录）→ decay（衰减）
```

当前实现：build/refresh ✅（`build_experience_state` / `update_experience_state`）、
verify ✅（`satisfies`）、**deploy ❌、record ❌、decay ❌** —— 本设计补齐后三者。

## 4. 状态元素分类（taxonomy）

| 类别 | 元素示例 | 来源 | 可变性 |
|---|---|---|---|
| environment | environment_id / cwd / sandbox_level / os | turn/step | 低 |
| detection | git_repo / deps / 进程 / **subprocess_allowed** | 探测 | 中 |
| network | policy / proxy / reachability | turn/config | 中 |
| tools | available / 版本 / 能力 | step/capability | 低 |
| config | workspace_roots / entries | env/config | 低 |
| task | goal / status / executed_steps | input/executor | 高 |
| runtime | **沙箱限制（子进程许可）、并发/资源上限** | 环境探测 | 高 |
| deployed | 本次经验已部署的元素（env.set 等） | deploy 动作 | 高 |

**新增关键类别：`runtime` 与 `deployed`。** 本次事件根因（沙箱禁止子进程）已
实现为 `runtime.subprocess_allowed`（turn 开始时探测并进入注册表）；经验的
条件与部署都依赖它。

## 5. 状态部署（deploy）——核心新能力

### 5.1 概念

经验 HIT 后、执行 workflow 前，先**部署经验依赖的状态元素**，再执行。
部署是确定性的、可回滚的、带安全门的。

### 5.2 动作族扩展（当前 runner 只有"查询"，需要"设置"）

| 动作 | 语义 | 安全门 |
|---|---|---|
| `deploy.env` | 设置环境变量（进程内）✅ 已实现（runner） | 白名单 key |
| `config.apply` | 应用配置条目 | 只写本项目 config |
| `tool.ensure` | 确认/定位工具（gcc、rustc） | 只读定位 + 记录 |
| `runtime.probe` | 探测运行条件（subprocess 等） | 只读 |
| `deploy.record` | 记录已部署元素 | - |

高风险部署（跨工作区写、网络变更）→ `AbortOrAsk`，不自动执行。

### 5.3 部署结果进入状态

```rust
struct DeployedElement {
    key: String,
    value: serde_json::Value,
    action: String,          // env.set / config.apply ...
    ok: bool,
    at: String,              // 时间戳
}
// ExperienceState 新增: deployed: Vec<DeployedElement>
```

LLM 介入时可读 `deployed`，知道经验"已经做了什么"，这是质疑权的基础之一。

## 6. 统一状态注册表（设计）

把分散字段收敛为一个可查询、可部署、可读的注册表：

```rust
struct StateElement {
    key: String,                              // "runtime.subprocess_allowed"
    value: serde_json::Value,
    source: ElementSource,                    // Turn / Step / Detection / Deployed / Config
    verified_at: Option<u64>,                 // 新鲜度
    ttl: Option<u64>,                         // 过期即需重探测
}
enum ElementSource { Turn, Step, Detection, Deployed, Config }
```

- 条件检查（`satisfies`）、部署（`deploy.*`）、LLM 读取（`lookup` / `handoff_payload`）
  三路共用同一注册表；✅ 已实现：`StateElement` / `ElementSource` /
  `DeployedElement` 类型、`elements` 注册表 + `upsert_element` + lookup 统一
  key 面、executor 自动记录部署结果（`ActionResult.deployed` →
  `state.deployed` + 注册表，source=Deployed）；
- 带 freshness 的元素（detection/runtime）随 `tick` 衰减，陈旧 → 状态机
  Decaying（阶段 9 联动）；
- LLM 介入时只读"未通过 + 相关"元素的 delta，而非全量。

## 7. 演进路线（对应阶段）

| 阶段 | 动作 |
|---|---|
| 3 收尾 | StateElement 注册表 + runtime 探测（subprocess/资源）+ deployed 记录 + freshness；Session 级状态持久 |
| 4 集成 | deploy 动作族进 executor/runner（带安全门）；部署结果计入反馈（部署失败 → Failure） |
| 7 学习器 | LLM 成功 trace 中的"环境准备步骤"被编译为 deploy 型经验 |
| 9 状态机 | 显式迁移 + 置信度驱动迁移已实现（VALIDATED/ACTIVE/DECAYING/DISABLED）；state freshness 联动（陈旧 → Decaying）待做 |
| 10 MCP | state 按 handoff 契约只读暴露给外部 Agent |

## 8. 回检用例（cargo-check）

验收：在**同一个受限沙箱**里给出一条 `cargo-check-env` 经验：

1. `runtime.subprocess_allowed == false` 被探测并进入状态；
2. 条件不满足 → 决策为 AbortOrAsk / 明确结论"需真实终端"，**LLM 0 次**；
3. 在**真实终端**（subprocess 允许）→ 条件满足 → deploy 工具链状态 →
   执行 `cargo check` → Completed，**LLM 0 次**；
4. 反馈：成功/失败计入 confidence，经验在 raw→learning→automatic 间演化。

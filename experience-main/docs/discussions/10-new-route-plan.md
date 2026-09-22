# 10 · 新路线规划：从"改造宿主"到"做宿主的客户端"

> 2026-09-17。承接 [09](09-pull-push-architecture.md)。
> 本文件是**工作计划**，不是讨论记录。

## 一、三个核实结果（决定了计划形状）

### 1.1 `codex-main` 没有被改烂 —— 不需要重新下载

```
tracked 工作树        : CLEAN（32 个 dirty 全是未跟踪的实验产物）
相对自己的 baseline   : 59 files, +11873 / -1
改动的落点            : codex-rs/core(25) / docs/architecture(13) / cli(4)
                        app-server(3) / app-server-protocol(3) / launcher(2)
相对 upstream/main    : 领先 86 个提交，落后 ≥12 个
upstream remote       : 已配置（partial clone, blob:none）
```

改动**全部可辨识**：`core/src/experience/*`、`app-server-protocol/…/v2/experience.rs`、
`app-server/.../experience_processor.rs`、`cli/src/experience_*.rs`、文档与验收脚本。
这不是一团乱麻，是一个边界清楚的 fork。

**决定：保留它。重新下载没有收益。** 要改的是它的**角色**，不是它的内容。

如果哪天需要读"纯净的上游"，用 `git worktree add` 挂一个 `upstream/main` 的工作树
即可，不必再克隆一份。

### 1.2 Pull 腿根本不需要 fork —— 这是要证明的核心命题

```
PATH 上的 codex : %LOCALAPPDATA%\OpenAI\Codex\bin\<version-hash>\codex.exe
```

**官方 stock 二进制就在 PATH 上。** 而 fork 对 `app-server` 的改动是**纯增量**
（只新增 `experience.rs` / `experience_processor.rs`，没碰 dynamic tools），
所以 dynamic tool 接口是**纯上游能力**。

**决定：Pull 腿的验收标准必须是"跑在 stock 上"。**
跑在自己的 fork 上不算数——否则对方一句"你还是需要你的 fork"就把论证推翻了。
开发期用 fork 的 debug 二进制做便利宿主可以，但那只是便利。

### 1.3 上游协议仍在动

`baseline → upstream/main` 之间，`app-server` / `app-server-protocol` / `protocol`
已有 13 个文件、+588/−37 的变化（本地 ref 还停在 3 周前）。
**动手前先 `git fetch upstream`**，再核一遍 dynamic tool 相关形状有没有变。

## 二、`codex-main` 的新角色（三件事，都不是"被改造"）

| 角色 | 用途 | 边界 |
|---|---|---|
| **宿主** | 提供 app-server（开发期便利） | 验收不用它，用 stock |
| **协议真相来源** | `generate-json-schema` 已抓下约 250 个文件 | 只读 |
| **Push 腿的参照实现** | M4/M5 内嵌 Gate 是"如果我们能改，会怎么做"的证据 | **冻结，不再扩** |

配套动作（很小）：

1. 在 `codex-main/README.md` 的 fork notice 下加一行：**本 fork 不再接收新功能**；
2. **不 rebase、不改写那 86 个提交**——M5 证据的可复现性依赖它们现在的哈希；
3. 把 32 个未跟踪产物清理或归档（它们不是代码）。

## 三、新路线（四段）

### L0 · 冻结与角色重定义（半天）

就是第二节那三件小动作。完成判据：README 写明角色；`git status --untracked-files=no` 干净。

### L1 · Pull 腿的客户端（核心工作线）

**位置：`experience-main`，不是在 `codex-main`。**

为什么要独立：Pull 腿的全部价值恰恰是"**不改宿主也能拥有过去**"。
把它写进宿主仓库，等于把这句主张销毁。

职责：

```text
持有 experience registry
   → 从 registry 生成 dynamicTools 条目（不是手写！见 L2）
   → thread/start 注册
   → 收到 item/tool/call
   → 执行转移：策略 → 围栏 → 备份 → 执行 → 判据
   → 返回 { contentItems, success }
   → 写 learning trace 回 registry
```

候选形态（三选一，未定）：

| 形态 | 优点 | 代价 |
|---|---|---|
| Rust crate（复用 `experience-core`） | 谓词/执行/存储直接复用，与既有资产一致 | 编译与联调慢 |
| Python client（延续 `active_leg` 骨架） | 改得快，协议好调试 | 要重写执行与谓词层 |
| Python client + 调 `experience-server` HTTP | 真正复用核心，客户端只做协议 | 多一层进程边界 |

**验收（三条，缺一不可）：**

1. 跑在 **stock** codex 上，而不是 fork；
2. 模型**主动调用 ≥1 次**，且返回内容来自**真实执行 + 判据为 TRUE**（不是编的）；
3. 同一任务第二次运行，token 有**可测**的下降（哪怕很小）。

素材已就绪：协议 schema（`active_leg/`）、可用骨架（`active_leg/app_server_client.py`）、
已实测的握手（08 §4.1）。

### L2 · 表面成为投影 + 发现面

- 注册条目从**手写**改成**从 registry 派生**。这是"experience ≠ dynamic tool"的
  **实现约束**：手写的工具在外部看来就是普通工具，论证随之作废；
- 核对 `deferLoading` 与发现面的真实关系（[09 §五](09-pull-push-architecture.md) 未核项 1）；
- 完成判据：**新增一条经验不需要改客户端代码**。

### L3 · 三方对比测试

Skill / Dynamic Tool / Experience 三列，**每一格都要有实测数字**
（表格见 09 §四，含补的两行）。这是 L4 唯一的弹药。

### L4 · Push 的请求

带 L1–L3 的结果去谈。到那时的问题已经非常窄：
**有没有一个机制，其发起者是世界状态而不是模型调用？**

## 四、风险与前置

| # | 风险 | 处理 |
|---|---|---|
| R1 | 本地 `upstream/main` ref 已过期 3 周，协议可能已变 | 动 L1 前先 `git fetch upstream` 并复核 dynamic tool 形状 |
| R2 | 用 fork 的 debug 二进制开发，久而久之把"跑在 stock 上"这条验收忘掉 | 把 stock 路径写进 L1 验收，作为硬条件 |
| R3 | 官方二进制的"自愈覆盖"历史 | 与本路线**无关**：我们不替换它，只作为客户端连它的 app-server |
| R4 | `deferLoading` 与发现面未核实 | L2 的第一件事 |
| R5 | 手写工具混进 registry | L2 的实现约束 + 在 L1 就定下"生成而非手写"的结构 |

## 五、与旧路线的关系（不要误读）

这不是推翻 3.5 层，是**换入口**：

| | 旧路线（lvl1） | 新路线（lvl2 / 本文件） |
|---|---|---|
| 宿主的角色 | 被改造的对象 | 被连接的运行时 |
| 产品的形态 | 带宿主补丁的层 | 宿主的客户端 |
| 需要谁批准 | 需要（要改内核） | **不需要**（Pull 腿） |
| 冻结的资产 | M4/M5 内嵌 Gate | 降为**证据**，不再扩 |
| 保留的资产 | — | registry、谓词、执行器、备份/回滚、学习闭环 |

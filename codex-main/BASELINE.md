# Experience Codex 基线（Baseline）

本仓库是 [openai/codex](https://github.com/openai/codex)（Apache-2.0）的改造分支。
项目目标：把 Codex 从 "LLM-centric Agent" 改造成
"State + Experience + LLM 三层控制 Agent"，让 **LLM 不再拥有 Agent 的绝对控制权**。

## 仓库拓扑

| 角色 | 地址 | 说明 |
|---|---|---|
| `upstream` | `https://github.com/openai/codex.git` | OpenAI 官方仓库，只读参考 |
| `origin` | 未设置 | 我们的 fork，创建后执行 `git remote add origin <fork-url>` |
| 本地 `main` | `6478a751` | 固定基线 |

## 基线（Baseline）

- 上游固定 commit：`6478a751fde8884b2fdc76486fe23175a8e795d4`
  - 日期：2026-08-29
  - 提交信息：`Organize bundled Rust resources under asset directories (#41477)`
  - 本地 tag：`baseline`
- 原始 zip 快照 commit（保留出处）：`4882ccdb8ab9d02c4efd2aa64c1608f18242f4fe`
  - 本地 tag：`baseline-zip-snapshot`

### 快照如何对应到 `6478a751`

1. 对本地快照与上游 `main` 逐提交做树内容对比，忽略 Windows 不保留的执行位
   （`100644`/`100755`）与符号链接类型位（`120000`/`100644`）差异，按真实内容差异取最小。
2. `6478a751` 与快照内容完全一致（只剩模式差异）。
3. 快照磁盘上含 `.vscode/` 三个文件，但 `.gitignore:25` 的 `.vscode/` 会让普通
   `git add` 跳过它们；基线提交已用 `git add -f` 强制纳入，保证提交与 zip 内容一致。

## 合并策略

OpenAI 更新 `main` 后，由我们主动决定是否合并：

```bash
git fetch upstream
git log --oneline main..upstream/main   # 审查上游新增
git log --oneline upstream/main..main   # 审查我们自己的改动
git merge upstream/main                 # 或 cherry-pick 目标 commit
```

未经审查不自动合并。

## 命名约定（避免与原文件冲突）

本项目新增内容一律使用 `experience_` 前缀（目录、文件、类型、模块），例如：

- 新模块：`codex-rs/core/src/experience/`
- 文件：`experience_runtime.rs`、`experience_state.rs`、`experience_matcher.rs`、
  `experience_workflow.rs`、`experience_executor.rs`、`experience_confidence.rs`、
  `experience_store.rs`、`experience.rs`、`mod.rs`

原因：`codex-rs/core/src/state/` 已被占用，且其语义是"线程/会话持久化状态"
（`SessionState`、`TurnState`），**不是** Experience State。直接用 `state.rs`
会产生歧义。

## Windows 环境说明

- `core.autocrlf=false` 已设置，避免提交时改写行尾。
- Windows 不保留执行位（`core.filemode` 无效），工作区文件一律 `100644`；
  与上游 `100755` 的差异仅为模式差异，内容一致。
- 涉及执行位的脚本在 Windows 上需通过调用方显式处理（如 `.cmd`/`.ps1` 包装或
  `sh` 执行）。

## 项目文档

- 控制流施工图：`docs/architecture/experience-agent-loop.md`
- 当前 canonical 执行面：`docs/architecture/m6-canonical-runtime.md`
- M6 验收：`accept-m6.ps1`

# 真机/回归复跑矩阵（2026-09-09）

## 前置

- DeepSeek token：`~/.codex/config.toml`（脚本自动读取，不落盘）；
- dist server：`scripts/build-app.ps1`（codex-main 侧 gate 化二进制：
  `codex-rs/target/debug/codex.exe`，改动后 `cargo build -p codex-cli`）；
- 真机会话成本为“每次 codex 会话”，脚本内已声明预算。

## 脚本清单

| 脚本 | 覆盖 | 真实会话 | 通过状态 |
|---|---|---|---|
| experience-main `accept-l1-sink.ps1` | L1 冷沉淀/repeat/no-op | 3 | ✅ |
| experience-main `accept-app-two-rounds.ps1` | session channel 两轮 + trace 单写 | 2 | ✅ |
| experience-main `accept-l3-delegate.ps1` | L3 v1 有/无种子 | 2 | ✅ |
| experience-main `accept-l3-execute.ps1` | L3 v2 execute-first / match-miss | 2 | ✅ |
| experience-main `accept-l3-loop.ps1` | 外层闭环 + usage + 无 known-step 重做（S1） | 2 | ✅ |
| experience-main `accept-l3-llm-compile.ps1` | S3 dirty 轮 → LLM compiler → CANDIDATE | 2（主会话+compile） | ✅ |
| codex-main `accept-m5-gate.ps1` | M5 四问 + L4 usage | 1 | ✅ |
| codex-main `accept-m6.ps1` | canonical 四场景：完整任务 0 LLM、中途 Gate、部分前缀、未知基线 | 2（内层复用 M5，另含 2 轮真实会话） | ✅ |
| codex-main `validate-m7-warm.ps1` | 桌面论文迁移：冷启动 → 经验赋值 → 首次复用 0 LLM → 重入 `Satisfied` | 冷启动 1 + 复用 2 | ✅ |
| codex-main `validate-m8-complex.ps1` | 复杂任务两臂：网络/终端/manifest + 文件前缀经验；基线 10 请求 vs 经验臂 6 请求 | 2 | ✅ |
| codex-main `validate-m9-template.ps1` | ExperienceTemplate 参数化：同一模板移动两个不同文件，两轮均 0 LLM | 2 | ✅ |
| codex-main `validate-m10-v2-path.ps1` | V2 全链路：两次真实冷启动（终端+网络+文件三步任务）→ 双轨迹归纳出 CANDIDATE → 产品 CLI 激活同一 Store → 热执行 0 LLM（含模板绑定审计） | 2（冷）+ 0（热，fake provider 计数） | ✅ |

> M6 起，canonical 执行运行时固定为 `experience-main` 的 P1 Runtime；
> `codex-main/core/src/experience/*` 仅保留管理面与显式兼容。

## 无 token 回归（cargo test）

- experience-main workspace：experience-core 215 passed / 1 ignored（含
  l1/l3 fixture + 模板归纳 + 命令翻译），experience-controller 16 passed；
- codex-main：`cargo test -p codex-core --lib experience_p1_gate`（2）+ M4
  确定性 `gate_hit_runs_embedded_experience_and_skips_tool_dispatch`；
- 注意：codex-core 集成测试 `tests/all.rs` 存在与 Gate 无关的 Windows
  栈溢出用例（预存在），本矩阵用 `--lib` 规避。

## 产物归档

- experience-main：`scripts/accept-artifacts/<name>-<ts>/`（gitignore）；
- codex-main：`accept-m5-artifacts/m5-<ts>/`（gitignore）；
- codex-main：`accept-m6-artifacts/m6-<ts>/`（gitignore）；
- codex-main：`m7-live-artifacts/`（现场验证产物，保留 stdout/stderr/usage）；
- codex-main：`m8-complex-artifacts/`（代理 dump、两臂 stdout、usage、backup）；
- codex-main：`m9-template-artifacts/`（模板两轮 stdout、usage、绑定结果）；
- codex-main：`m10-v2-artifacts/`（两次真实观测 jsonl、归纳判定、CLI 三态输出、
  热执行 stdout、usage 审计）；
- 每次真机后把规范形态同步进 `crates/experience-core/tests/fixtures/`
  或 `codex-main/fixtures/m5/`（no-token 回归）。

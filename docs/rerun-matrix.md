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

## 无 token 回归（cargo test）

- experience-main workspace：195 passed / 1 ignored（含 l1/l3 fixture）；
- codex-main：`cargo test -p codex-core --lib experience_p1_gate`（2）+ M4
  确定性 `gate_hit_runs_embedded_experience_and_skips_tool_dispatch`；
- 注意：codex-core 集成测试 `tests/all.rs` 存在与 Gate 无关的 Windows
  栈溢出用例（预存在），本矩阵用 `--lib` 规避。

## 产物归档

- experience-main：`scripts/accept-artifacts/<name>-<ts>/`（gitignore）；
- codex-main：`accept-m5-artifacts/m5-<ts>/`（gitignore）；
- 每次真机后把规范形态同步进 `crates/experience-core/tests/fixtures/`
  或 `codex-main/fixtures/m5/`（no-token 回归）。

# WP6/WP7 真机验收方案：M5 真实 Gate 闭环 + L4 usage 写回

> 日期：2026-09-09。前置：gate 化 codex.exe 已重建（19:18:26，WP7 后
> 20:07:51），确定性回归
> `gate_hit_runs_embedded_experience_and_skips_tool_dispatch` 与
> `gate_usage_outcome_layers_success_misfire_invalid` PASS。
> 真机执行：共 4 次 codex 会话（3 次 gate HIT：2 次为脚本 harness 缺陷
> 造成的重跑、1 次最终干净 PASS；1 次无参数启动失败未触达 Gate）；
> 最终轮 2026-09-09 20:11:23 PASS。
> 关键教训：fixture 的 `verification` 必须带 read_file（否则 postconditions
> 无 evidence facts，控制器诚实判 partial→usage misfire）。

## 1. 目标（M5 四问 + L4 usage）

1. 真实 Codex loop 中，LLM 提出 exec_command（cmd 含
   `create probe file`）→ 内嵌 Gate HIT（stderr 出现
   `GATE HIT experience=create_probe_file; original tool NOT dispatched`）；
2. **原 Tool 确实未 dispatch**：被拦截命令本会写 marker
   `tool-ran.txt`，HIT 后 marker 不存在（判别性证据）；
3. Experience 内嵌执行并验证：`probe.txt` 真实存在且内容
   = `EXPERIENCE_GATE_SUCCESS`（由 Gate 写，非会话委派路径）；
4. 合法 FunctionCallOutput 注入 → 模型继续并正常收尾（exit 0）；
5. （L4/WP7）GateHitResult → `<EXPERIENCE_GATE_STORE> 旁 usage.json`
   写回：entries.create_probe_file.hits=1、
   decisions.experience_only=1、logs≥1，作为审计查询面。

## 2. 运行拓扑

- exe：`codex-rs/target/debug/codex.exe`（M4+fuse）
- CODEX_HOME：`codex-main/.codex-exp-home`（DeepSeek，approval never，
  danger-full-access）
- 环境：`EXPERIENCE_ENABLED=1`、`EXPERIENCE_GATE_STORE=fixtures/m5/
  gate-store.json`、`EXPERIENCE_GATE_CWD=<workspace>`、`RUST_LOG=info`
- workspace：`codex-main/accept-m5-workspace`（trusted 项目，已 gitignore）
- 判别逻辑：prompt 要求 exec_command 的命令“打印 create probe file +
  写 tool-ran.txt + 不写 probe.txt”。HIT ⇒ marker 不出现、probe 由
  Experience 写；MISS ⇒ marker 出现 → FAIL。

## 3. WP7（L4）实现要点（本批随方案落地）

- `experience_p1_gate.rs`：Gate HIT 后把 GateHitResult 映射为 usage 记录
  （band=`experience_only`；completed→无 outcome；有执行步骤但未完成→
  misfire；无执行步骤→invalid）写入 EXPERIENCE_GATE_STORE 旁 usage.json；
- usage 文件结构与经验侧（experience-main）同源语义：hits/decisions/
  logs/misfires/invalid 分层，不给 experience 本体加字段；
- 确定性测试：结果映射函数（success/partial/failed × executed 有无）；
  真机断言 usage.json 增量。

## 4. 范围边界与后续

- P2-4（委派文本不诱导重复执行已知段）：已有 execute 相产物显示 codex
  会重跑 `Set-Content probe.txt`；完整修复需要 Task Segment 或新的
  “剩余任务”裁定，**不在本轮自动决定**，作为发现项报告。
- 不扩展：多 Agent、模型 B、pin UI、policy 上限参数化。

## 5. 复跑

```powershell
powershell -ExecutionPolicy Bypass -File accept-m5-gate.ps1
```

## 6. 2026-09-18 跨仓漂移修复 + 复跑记录

**漂移**：`codex-rs/core` 以 path 依赖引用 `experience-main/crates/experience-*`。
experience-main 的 S1-b（`a5b6cbc`，2026-09-12）把 `LocalRunner` 从单元结构改成
字段结构（`backup_root: Option<PathBuf>`，只留 `default()` / `with_backup()`），
而本仓内嵌 Gate 仍构造 `Box::new(LocalRunner)`。结果：`codex-rs/core` 自
2026-09-12 起无法编译，`docs/rerun-matrix.md` 里“codex-main gate 2 passed”
这一行在事实上失效（9/9 的 `codex.exe` 早于该漂移，所以历史证据仍成立，
但“可复现”不再成立）。

**修复**：`core/src/experience_p1_gate.rs` 改为 `LocalRunner::default()`——
`backup_root = None` 与 M5 验收时的“Gate 路径不带备份”契约一致，语义不变。

**复跑证据（2026-09-18）**：

| 项 | 命令 | 结果 |
|---|---|---|
| gate 单测 | `cargo test -p codex-core --lib experience_p1_gate` | 2 passed / 0 failed（`gate_usage_outcome_layers_success_misfire_invalid`、`m5_usage_fixture_mirrors_real_gate_success`） |
| M4 确定性 | `cargo test -p codex-core --lib gate_hit_runs_embedded_experience_and_skips_tool_dispatch` | 1 passed |
| 二进制重建 | `cargo build -p codex-cli`（GNU） | `target/debug/codex.exe` 19:21:23，1,792,957,987 字节 |
| 真机四问 | `powershell -ExecutionPolicy Bypass -File accept-m5-gate.ps1` | **PASS**（1 次会话） |

产物 `accept-m5-artifacts/m5-20260918-192213/`：`exit=0`；stderr
`GATE HIT experience=create_probe_file; original tool NOT dispatched`；
`probe.txt` = `EXPERIENCE_GATE_SUCCESS`（23 B）；`tool-ran.txt` 缺失；
`usage.json` hits=1 / decisions.experience_only=1 / logs=1 / misfires=0 /
invalid_failures=0。即：M5 四问与 L4 usage 写回在今天的 HEAD 上重新成立，
且走的是 S1-b 之后的共享执行器（`experience_core::exec`）。

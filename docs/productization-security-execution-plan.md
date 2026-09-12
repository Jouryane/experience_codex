# 产品化规划：安全边界 × 执行与验证能力（2026-09-12）

> 目的：把当前“研究型技术预览”推进到可长期使用的产品形态。
> 本文只规划两类硬能力：**安全边界**（能放心执行）与**执行/验证面**
> （有能力执行、可证明完成）。规模索引与多 Agent 另见 Stage C5/冻结项。

## 0. 现状与问题（作为设计输入）

- 执行面：进程内仅 `write_file`（`experience-controller/runner.rs`、
  `experience-server/main.rs` 的 `l3_execute_step`）；
- 验证面：只有 `cwd.exists` / `file:*.exists|content`；
- 两个 P0 安全问题：
  1. **路径可逃逸**：`workspace.join(path)` 未做 containment，绝对路径或
     `..\..\` 可写到工作区之外（L3 与 L4 Gate 共用该模式）；
  2. **参数未脱敏**：`args_summary` 原样进 trace/ledger，并作为 LLM
     compiler 输入；密钥/连接串可能被持久化或外发。
- 会话能力白名单（Stage C4）只做了“词表校验”，尚未在执行路径上强制。

## 1. 安全边界（P0 → P1）

### 1.1 工作区围栏（Workspace Containment）

统一实现（core 提供、server/controller 共用）：
`safe_join(workspace, rel) -> Result<PathBuf, PathPolicyError>`

规则（全部拒绝，而非“尽力”）：

- 拒绝绝对路径、盘符路径（`C:\`、`C:foo`）、UNC（`\\server\share`）；
- 词法归一化 `..` / `.`；任何 `..` 逃出 workspace 一律拒绝；
- 允许路径必须落在 **canonical workspace 根**之内（Windows 大小写不敏感；
  用 canonicalize 后前缀比较）；
- symlink/junction：解析真实路径后再做前缀检查，逃逸即拒绝；
- 拒绝 Windows ADS（`file.txt:stream`）与保留设备名（CON/PRN/NUL…）；
- 写入前对“最近存在的父目录”做 canonicalize，避免延迟创建绕过。

落地：L3 `l3_execute_step` 与 L4 `LocalRunner` 全部改为 `safe_join`；
新增单测：绝对/盘符/UNC/`..`/symlink/ADS/大小写边界。

### 1.2 敏感参数脱敏（Redaction）

新增 `experience-core::redact`，在**三处强制调用**：

- trace 写入（agent-codex 提取 args_summary 时）；
- ledger/audit 与 usage 记录；
- LLM compiler 输入、参考注入文本、run-notes 入库前。

默认规则（best-effort + 可审计）：

- 键名形如 `*_KEY|*_TOKEN|*_SECRET|PASSWORD|PASSWD|CREDENTIAL` 的值 → `<redacted>`;
- `Authorization: Bearer …`、`sk-…`、`ghp_…`、AWS key、URL `://user:pass@` → 脱敏；
- 形如 `"token":"…"` / `"password":"…"` 的 JSON 字段 → 脱敏；
- 长随机串（≥24、混合大小写数字、非路径）在 strict 模式下 → 脱敏；
- 策略分级：`EXPERIENCE_REDACTION=strict|standard|off`（默认 standard；
  strict 额外丢弃 exec 参数、只留命令体摘要）。

审计要求：脱敏必须**幂等**、不改变非敏感字段；脱敏命中记入审计
（`redacted:<kind>:<count>`），但不记录原值。

### 1.3 能力与权限（Capability Policy）

把 C4 的 `capability_allowlist` 从“词表”升级为**执行强制**：

```json
{
  "scope_policy": {
    "fs_write": "workspace_only",
    "exec": { "enabled": false, "allow": ["pwsh","python","node","git","npm"], "timeout_secs": 60, "output_cap": 20000 },
    "network": "off",
    "backup": "on"
  }
}
```

- 默认：`fs_write=workspace_only`、`exec=off`、`network=off`；
- 会话级 `capability_allowlist` 只能**收窄** scope policy，不能放宽；
- 未授权步骤 → 步骤失败（invalid），失败即停并如实反馈；
- 危险模式 denylist（`format`、`del /f /s`、注册表、`shutdown` 等）
  即使 exec 开启也拒绝，并写审计。

### 1.4 可回滚与熔断（备份 / 熔断）

- 写操作前对目标文件做备份：`<home>/backups/<session>/<ts>/<relpath>`；
- 经验执行失败或用户撤销 → 支持按 session 回滚（v1 手动端点，
  产品化后在 UI 提供“撤销本次经验执行”）；
- 熔断：单会话步骤数上限、连续失败阈值、Gate 命中窗口上限（已有 fuse）
  统一配置；超限降级为委派并审计。

## 2. 执行面能力（Tiered Runner）

### Tier 0（已具备）：`write_file`（加围栏后）

### Tier 1：受控文件操作（低风险，默认可用）

- `read_file`（返回内容摘要；受大小/脱敏限制）
- `append_file`、`mkdir`、`copy_file`、`move_file`（全部走 `safe_join`）
- `delete_file`：默认关闭；开启需 scope policy + 备份

### Tier 2：受控命令执行（默认关闭，按 scope 白名单开启）

- 新步骤形态（推荐）：`exec{program, args[], cwd?, env_allowlist?, timeout_secs}`
  —— 不走 shell 拼接，program 必须在 allowlist；
- 兼容旧形态：`exec_command{cmd}` 仅在 scope policy `legacy_shell=allow`
  时放行（默认拒绝）；
- 运行约束：cwd 必须在 workspace；最小 env；超时击杀；输出截断并脱敏；
  退出码写入 State（供验证使用）；
- 危险动作 denylist + 用户确认（产品化 UI 提供一次性确认）。

### Tier 3（后续，独立设计）：HTTP/浏览器/DB 等外部能力

- 以 adapter 契约形式接入（capability 声明 + 权限 + 证据），
  不在本轮实现；computer-use 仍保持“通道保留、实现冻结”。

## 3. 验证面能力（State Source Registry + Predicates）

### 3.1 统一接口（沿用 L2 冻结语义）

```text
StateSource/Probe:  value(key) -> Option<(value, source, observed_at, ttl)>
Predicate:          key = expected（三值：True/False/Unknown）
```

每个 predicate 必须绑定来源与 freshness；无法求值 → Unknown，不执行。

### 3.2 首批新增谓词（按实现顺序）

| 谓词 | 证据来源 | 用途 |
|---|---|---|
| `file:<p>.exists/content` | 现有 LocalProbe | 已有 |
| `file:<p>.sha256` / `.size` | 文件读取 | 内容稳定性/大文件 |
| `dir:<p>.exists` | 目录探测 | 脚手架验证 |
| `process.exit_code:<step_id>` | Tier2 exec 结果 | 测试/构建是否通过 |
| `process.stdout_contains:<step_id>` | exec 输出（脱敏后） | 关键输出断言 |
| `git.dirty` / `git.branch` / `git.last_commit` | git 只读命令 | 仓库类任务 |
| `http.status:<url>` / `http.body_sha256:<url>` | 受控 HTTP 探针（需 scope 允许） | 服务/接口验证 |
| `port.open:<n>` | 本地端口探测 | dev server 就绪 |

### 3.3 与 L2 五级门的关系

- Gate 3（前提可观测）：新谓词必须在 Registry 中可绑定；
- Gate 4（dry-run）：文件操作在 scratch 重放；exec 只做“解析 + 策略校验
  + 参数形状”，不执行；HTTP 探针 dry-run 只解析；
- Gate 5（完成判据可观察）：判据导出限制在新的可观察谓词集合；
  不可观察 → 停留 CANDIDATE（保持诚实拒绝）。

## 4. 数据与接口变化（兼容性）

- `WorkflowStep.action` 增加 `exec`、`append_file`、`mkdir`、`copy_file`、
  `move_file`、`delete_file`、`read_file`（全部走 capability policy）；
- `scope_policy` 为 additive 元数据（store envelope + 会话可覆盖收窄）；
- 新增只读端点：`GET /api/policy`、`GET /api/state/sources`；
  写策略端点 `PUT /api/scopes/{scope}/policy`（actor 审计）；
- 所有新字段 additive + serde default，旧 store/会话不受影响。

## 5. 里程碑与验收

### S0 状态：✅ 完成（2026-09-12）

- `experience-core::safety::safe_join`：绝对/盘符/UNC/`..`/ADS/保留设备名/
  symlink 逃逸全部拒绝；L3（server `l3_execute_step`）与 L4（controller
  `LocalRunner`）均改走围栏；
- `experience-core::redact`：固有规则（≤6 类）+ 槽位策略（secret/identity/
  locator/payload/free-text）；默认披露级别 **structure**；HMAC-SHA256
  “可比不可还原”（每 store 一个 `redaction.key`）；脱敏幂等；
- 全链路接线：trace args 提取（agent-codex innate）、ledger/audit、
  ingestion parse/package/run-notes（返回 `redacted/redactions`，
  预览确认式）、参考注入文本、LLM compiler 输入；
- 验收：核心 154 tests（含 leak corpus fixture 与 5 组 redact/safety 单测）、
  controller 路径逃逸单测、`scripts/accept-p0-security.ps1` PASS（泄露语料
  在 parse/run-notes/store/ledger 全链路 0 命中，7 处脱敏）。

| 阶段 | 交付 | 验收 |
|---|---|---|
| S0 | `safe_join` + `redact` 全链路 + 单测/fixture | 攻击用例全部拒绝；密钥样例不出现在 trace/ledger/prompt |
| S1 | capability policy（scope/会话强制执行）+ Tier1 文件操作 + 备份/回滚 | 未授权步骤 invalid；备份可回滚；真机场景 A/B 复跑通过 |
| S2 | Tier2 exec（默认关 + 白名单/超时/deny） | 允许的程序可跑并产出 exit_code 证据；危险命令拒绝；超时击杀 |
| S3 | State Source Registry + 新谓词 + L2 门接线 | 新谓词 mock 五级门；至少一个“exec + exit_code 判据”的真机经验闭环 |
| S4 | 设置页（compiler/injection/policy）+ 撤销 UI | UI 可配策略；旧 env 仍兼容 |

S0 是硬门槛（P0 安全），S1–S3 是“能干活”的核心，S4 是产品化体验。

## 6. 明确不做（本规划外）

- 不实现无沙箱的任意 shell 执行；
- 不做跨机器/多用户权限体系（先用本地单用户 + actor 审计）；
- 不把 computer-use 作为默认执行器（通道保留）；
- 不引入 embedding/RAG（规模索引另见 C5）。

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
  "fs_read": { "enabled": true, "workspace_only": true, "max_bytes": 20480, "special_grant_bytes": null },
  "fs_write": "workspace_only",
  "fs_delete": "deny",
  "exec": { "mode": "off", "allow": ["pwsh","python","node","git","npm"], "timeout_secs": 60, "output_cap": 20000, "allow_legacy_shell": false },
  "network": { "mode": "off", "allow": [] }
}
```

- 默认：`fs_write=workspace_only`、`exec=off`、`network=off`；
- 会话级 `capability_allowlist` 只能**收窄** scope policy，不能放宽；
- 未授权步骤 → 步骤失败（invalid），失败即停并如实反馈；
- 危险模式 denylist（`format`、`del /f /s`、注册表、`shutdown` 等）
  即使 exec 开启也拒绝，并写审计。

**落地（S1-a，2026-09-12 ✅）**

- `experience-core::policy`：`CapabilityPolicy { fs_read, fs_write, fs_delete,
  exec, network }`，默认 `fs_write=workspace_only`、`fs_delete=deny`、
  `exec=off`、`network=off`；
- store envelope 新增 additive `scope_policies`（`__global__` 为全局默认，
  scope 未配置时继承），旧 store 不受影响；
- 强制点：L3 预检（`l3_policy_blocks` → 候选记 `policy_denied:*` 并跳过）与
  逐步执行（未授权步骤 → `invalid`、失败即停、detail 保留归因）；
- 会话 `capabilities` 只能**收窄**：请求超出 scope policy 时在
  `/api/sessions` 直接 400（legacy `exec_command` 即使 `exec=allowlist` 也需
  `allow_legacy_shell` 显式开启）；
- API：`GET /api/policy[?scope=]`（effective + 已存策略列表）、
  `PUT /api/policy`（`scope` 缺省 `__global__`，`clear:true` 恢复继承），
  写入前 `validate_policy`；每次变更写 ledger `policy_updated`（actor）。

**读取上限与“特殊许可”（回应 20KB 是否武断）**

- 默认 `fs_read.max_bytes = 20480`（20 KiB）只是**保守基线**，不是硬编码：
  每个 scope 可调 `max_bytes`；
- 单例例外走 `fs_read.special_grant_bytes`（显式授权、上限
  `8 MiB`、必须 ≥ `max_bytes`），有效上限 = `max(max_bytes, grant)`——
  S1-b 的 `read_file` 直接消费该字段，用户不需要为一次大文件读取改全局策略；
- 读取仍受工作区围栏与脱敏约束（`special_grant` 只放宽长度，不放宽范围）。

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
| S1 | capability policy（scope/会话强制执行，**S1-a ✅**）+ Tier1 文件操作（**S1-b ✅**）+ 备份/回滚（**S1-c ✅**） | 未授权步骤 invalid；备份可回滚；真机场景 A/B 复跑通过 |
| S2 | Tier2 exec（默认关 + 白名单/超时/deny） | 允许的程序可跑并产出 exit_code 证据；危险命令拒绝；超时击杀 |
| S3 | State Source Registry + 新谓词 + L2 门接线 | 新谓词 mock 五级门；至少一个“exec + exit_code 判据”的真机经验闭环 |
| S4 | 设置页（compiler/injection/policy）+ 撤销 UI | UI 可配策略；旧 env 仍兼容 |

### S1-b / S1-c 状态：✅ 完成（2026-09-12）

- **共享执行器** `experience-core::exec::execute_step`：server（L3）与
  controller（Gate `LocalRunner`）共用一份实现，杜绝两处漂移；每次执行
  依次做 策略校验 → 工作区围栏 → 备份 → 执行，未授权步骤在任何副作用前
  失败（`invalid`），且带 `policy denied step '<action>': <family> (<reason>)`；
- **Tier1 动作**：`read_file`（默认上限 `fs_read.max_bytes=20480`，可被
  `special_grant_bytes` 显式放宽，读取结果脱敏）、`append_file`、`mkdir`、
  `copy_file`、`move_file`、`delete_file`（默认 `fs_delete=deny`，显式开启才允许）；
- **备份/回滚**：每次 L3 运行在 `<home>/backups/<session>/run-<ts>/` 下
  按需复制被改动文件并写 `manifest.json`（含 workspace、session、条目、
  是否原存在、是否目录）；`restore_from_manifest` 逆序回滚——
  原有文件恢复、本不存在的路径（文件/目录）删除；
- **API**：`GET /api/backups?session=`（列出该会话快照 + manifest）、
  `POST /api/backups/{session}/restore {snapshot,actor}`（回滚并写 ledger
  `backup_restored`）；
- **验收**：`scripts/accept-s1-exec.ps1`（无 LLM）PASS——write/append/mkdir/
  copy/move/read 真实落盘、`delete_file` 被策略拒绝且文件保留、备份快照可列出、
  回滚后 `existing.txt` 还原且新建文件被移除；`cargo test --workspace` 全绿。

### S1-d 真机验收：✅ 完成（2026-09-13）

目的：证明 S1-b/S1-c 的 Tier1 执行面在**真实模型委派链路**里成立——
进程内执行发生在委派之前，委派文本不会诱导 agent 重做或破坏产物。
复用 `accept-scenes-ab.ps1` 的 server + session-channel harness（6 个会话，
每会话一次 codex 运行；凭据取 `~/.codex/config.toml` 的 deepseek token）。

| 场景 | 压测点 | 通过判据 |
|---|---|---|
| D1 多动作组合 | write/append/mkdir/copy/move/read 一次跑完 | 六个动作产物正确；ledger `experience_execution=success`；delegation 存在 |
| D2 删除默认拒绝 | `delete_file` 默认 `fs_delete=deny` | 目标文件仍在且内容不变；ledger 该经验为 `invalid`；委派继续完成 |
| D3 显式授权删除 | scope policy 打开 `fs_delete` | 目标被删；备份快照含该文件；回滚可恢复 |
| D4 覆盖 + 回滚 | 已存在文件被覆盖后一键撤销 | `existing.txt` 回滚为原内容；ledger `backup_restored` |
| D5 读取大文件 | 默认上限触发拒绝，授权后通过 | 未授权时 `read_file` 记 `invalid`；授权后读到全文、委派 prompt 含其内容 |
| D6 半自动编辑 | 内嵌写入 + agent 补齐未知段 | 产物同时含内嵌内容与 agent 追加内容；无重复写入迹象 |

**执行结果（DeepSeek 版 codex 真机，7 场 + 3 场回归，全部 PASS）**

- D1 多动作组合 PASS：`out/new.txt`/`existing.txt`/`made/sub`/`made/moved.txt`
  全部正确，`made/copied.txt` 不存在；ledger 一条 `experience_execution=success`；
  delegation 存在；
- D2 删除默认拒绝 PASS：`keep.txt` 原样保留，ledger 该经验 `invalid` 且
  reason 含 `fs_delete=deny`，委派仍收尾完成；
- D3 显式授权删除 PASS：`victim.txt` 被删且快照 manifest 含该条目；
- D4 覆盖 + 回滚 PASS：先 `OVERWRITTEN`，`POST /api/backups/{sid}/restore`
  后回滚为 `original-content`，ledger `backup_restored` 存在；
- D5a 读取大文件（special grant）PASS：读成功（success）；
  D5b 未授权读取 PASS：`invalid` 且 reason 含 `above the read cap`；
- D6 半自动编辑 PASS：`notes.md` 同时含内嵌标题与 agent 追加的 `AGENT-ADDED`。

回归：`accept-scenes-ab.ps1 -Only A1/A3`、`accept-scenes-d.ps1 -Only D1`
全 PASS，证明 S1-b/S1-c 重构未破坏 L3 与 Gate 两条执行路径。
脚本：`scripts/accept-s1d-real.ps1`；fixture：`experiences/scenes/s1d_presets.json`。

**由 D1/D3/D4 首次失败暴露并修掉的产品问题**：委派文本只写了“请勿重复执行”，
真实模型仍会重写/回滚已完成的文件改动。现在 `delegation_text` 明确追加
“不要重写、覆盖、回滚这些文件改动”，并在验收用例中加入 observer 约束；
修复后三场均 PASS（`delegation.rs`）。

S0 是硬门槛（P0 安全），S1–S3 是“能干活”的核心，S4 是产品化体验。

### S2 状态：✅ 完成（2026-09-13）

**执行器**（`experience-core::exec`，server 与 controller 共用）

- 新步骤形态 `exec{program, args[], cwd?, env?}`：**不走 shell 拼接**，
  program 必须在 `exec.allow` 白名单（按可执行体 stem 匹配，大小写不敏感）；
- 旧形态 `exec_command{cmd}`：仅当 scope policy 显式
  `exec.allow_legacy_shell=true` 才放行，默认拒绝；
- **硬 denylist**（即使被白名单点名也拒绝）：`cmd/powershell/pwsh/reg/format/
  diskpart/shutdown/taskkill/netsh/schtasks/vssadmin/wbadmin/takeown/icacls/
  robocopy/...`，以及参数级危险模式（`del /f /s`、`rm -rf`、`reg delete`、
  `-EncodedCommand`、`DownloadString`、`curl | sh` 等）；
- 运行约束：cwd 必须落在工作区内；可执行体只能来自工作区或 `<home>/run`
  运行根（绝对路径越界直接拒绝）；超时（policy `timeout_secs`）到点击杀；
  输出按 `output_cap` 截断并脱敏；继承环境会剔除 `*_KEY/*_TOKEN/*_SECRET/
  PASSWORD/CREDENTIAL/API_KEY` 等敏感变量；
- 证据：`StepExecution{exit_code, stdout, stderr}`；`process.exit_code:<step_id>`
  与 `process.stdout_contains:<step_id>` 已可作后置条件（S3 谓词注册表的基础）。

**验收**：`scripts/accept-s2-exec.ps1`（无 LLM，真原生可执行体）PASS——
白名单程序退出码 0 且 stdout 进入 ledger、denylist 拒绝 cmd、legacy 未授权拒绝、
默认 `exec=off` 拒绝、超时 1s 击杀并报 TIMEOUT；`cargo test --workspace`
**267 passed / 0 failed**（含 `apps/experience-server/tests/s2_exec.rs` 两个
HTTP 端到端用例与 core `exec::tests` 7 个 Tier2 单测）。

**S2-d 真机验收：✅ 完成（2026-09-13，1 次会话）**

`scripts/accept-s2d-real.ps1`（DeepSeek 版 codex）：白名单原生程序在 L3 进程内
执行，`process.exit_code:exec#0` 判据成立 → `experience_execution=success`，
stdout（`PROBE_OK`）进入 ledger，delegation 正常且未重做。这一场同时是
S3 的“exec + exit_code 判据”真机雏形（完整 S3 仍需谓词注册表与五级门接线）。

## 5.5 S3 合理性分析：什么任务适合 Experience（2026-09-13）

S3 不是“再实现几个谓词”，而是**把分工讲清楚并让系统按分工执行**。以下结论
同时是产品边界与验收口径。

### 5.5.1 第一性原则

> Experience 的产物不是“把活干完”，而是**一组可验证、可归因、可回滚的状态
> 转移**。因此它只对“能观察、能证明、能负责”的工作负责。

由此得到三条硬性适配条件（缺一不可）：

1. **可判定**：完成后存在客观证据（文件/进程/仓库/端口/接口的观测值），
   而不是“看起来做完了”；
2. **可界定**：步骤集合有限、边界明确（单一目录/单一命令），不会因外部
   条件爆炸；
3. **可归因**：失败能说清是“经验前提不成立（不扣分）”还是“执行环境出错”
   （misfire 与 ToolError 已在 L1 分层）。

### 5.5.2 适合 / 不适合（判断表）

| 维度 | 适合 Experience 直接执行 | 不适合（应交由 agent / 用户） |
|---|---|---|
| 结果形态 | 确定性产物：脚手架、配置、清单、`.editorconfig`、CI 片段 | 需要判断审美/架构权衡的设计决策 |
| 可验证性 | 有可观察判据（文件哈希/大小、退出码、stdout 关键行、端口就绪） | 结论无法观测（“代码更清晰”“体验更好”） |
| 变量范围 | 输入稳定、路径固定、命令白名单内 | 需要探索未知代码库、失败原因不明 |
| 风险 | 工作区内、可备份、可回滚 | 不可逆/越界（删除、发布、外发、密钥操作） |
| 频率 | 高频重复、每次都一样的机械段 | 一次性、每次都要重新判断 |
| 责任 | Experience 写得清“为什么这样做” | 需要用户显式授权的领域选择 |

**结论（分工三档）**

- **经验全自动（auto）**：经验覆盖任务的**全部**可判定部分，且判据成立；
  agent 只做收尾确认，不重写、不回滚（S1-d 已验证该语义）。
- **经验 + agent（semi）**：经验负责“已知骨架/已知修复”，agent 负责未知段；
  判据只对经验自己的部分成立，`delegation_text` 明确“不要重做已完成部分”。
- **仅参考（reference）**：判据不可观测、环境不可控、或需要人工判断；
  Experience 只提供方法与证据，绝不动手。

### 5.5.3 如何兼容更多场景（S3 的扩展方向）

兼容性来自**判据面变宽**，而不是执行面变野：

| 场景 | 现在为什么不能做 | S3 之后靠什么做 |
|---|---|---|
| 构建/测试类任务 | 只有 `file.*` 判据，看不到退出码 | `process.exit_code` / `stdout_contains`（S2 已铺） |
| 仓库类任务（提交前检查） | 无 git 判据 | `git.dirty/branch/last_commit/sha` |
| 服务类任务（dev server 就绪） | 无端口/接口判据 | `port.open` / `http.status` / `http.body_sha256` |
| 大文件/内容稳定性 | 只能比内容字符串 | `file.sha256/size`（无需读全文） |
| 多步骤半自动 | 判据无法表达“前一步的输出” | `process.*:<step_id>` 绑定到具体步骤 |

同时**收紧**：所有判据必须来自注册表（有来源、有 fresh 语义、有 TTL）；
未注册的判据 → `Unknown` → **不允许执行**（诚实拒绝，而不是猜）。

### 5.5.4 分工落地（S3 设计）

- `experience-core::state_source`：`StateSourceRegistry` + 谓词族探测；
  `Unknown` 与 `False` 严格区分；网络/进程类探测受 capability policy 约束；
- L2 五级门接线（Gate 3/4/5）：
  - Gate 3（前提可观测）：precondition 必须能绑定到注册表来源；
  - Gate 4（dry-run）：文件动作在 scratch 重放，exec 只做“策略 + 形状”校验
    （不真正运行），HTTP 探测只解析；
  - Gate 5（完成判据可观察）：判据导出必须落在可观察谓词集合，否则停在
    CANDIDATE（保持诚实拒绝）；
- **任务适配合成**：经验命中后，若判据不可观测 → 降级为“仅参考/委派”，
  并把理由写进审计（`unsuitable:unobservable_verdict` 等）。

### S3 状态：✅ 完成（2026-09-13）

**State Source Registry**（`experience-core::state_source`）

- 每个谓词键必须由注册来源产出（`family_of`）；未注册 → `Unknown`
  （`observable()` 为 false），**绝不当作“已满足”**；
- 谓词族与新鲜度：`fs`（exists/content/size/sha256、`dir:*.exists`，读取时刻）、
  `exec`（`process.exit_code/stdout_contains:<step_id>`，运行时刻）、
  `git`（dirty/branch/last_commit，TTL 30s；无仓库时为 Unknown）、
  `http`（status/body_sha256，TTL 15s，**需网络策略**）、
  `net`（`port.open:<n>`，TTL 10s，本地回环探测）；
- `ProbeResult{value, source, observed_at, ttl}`：值 + 出处 + 新鲜度；
- 探测策略：文件/exec 恒可用；git 默认可用；网络探测需显式授权（默认 Unknown）。

**L2 五级门接线（L3）**

- Gate 3（前提可观测）：precondition 未注册 → 视为 Unknown → 不执行；
- Gate 4（dry-run）：`StepContext::with_scratch` 在临时目录重放文件动作，
  exec 只校验策略与形状**绝不 spawn**；dry-run 失败 → 候选跳过并审计
  `dryrun_failed:*`；策略拒绝的步骤不在 dry-run 中失败（由真实运行记 invalid）；
- Gate 5（完成判据可观察）：未注册判据 → 候选跳过，审计
  `unobservable_verdict:<key>`，经验保持 CANDIDATE（诚实拒绝）。

**验收**：`scripts/accept-s3-state.ps1`（无 LLM）PASS——sha256/size/dir 判据
成立并 success、未注册判据被拒（委派且不改工作区）、dry-run 不泄漏、
git 判据在真仓库中绑定且假设不成立时诚实 misfire；
`cargo test --workspace` **278 passed / 0 failed**（core 190 + server 43）。
S2-d 真机已含“exec + exit_code 判据”闭环。

### S3.5 可声明项：学习来源与相似性（2026-09-13）

不当作“缺陷”，而是**可移植的方法论卖点**，已固化为
`docs/learning-reuse-contract.md`（学习/复用契约 + 移植检查清单 + 已知妥协表）：

- **来源标注**：store envelope 新增 additive `candidate_origins`
  （`task_signature / session_id / distiller / recorded_at`），自动学习产物可
  与手工/吸收产物区分；`GET /api/experiences` 每项带 `origin`；
- **只读相似视图**：`experience-core::similarity`（结构指纹 = 工具 + 模式词 +
  动作骨架，Jaccard 打分）；`GET /api/similarity?scope=&threshold=&cap=`
  返回 `clusters`（同族，含 `max_similarity`）与 `pairs`（排序、限量）；
  **永不自动合并**，由人或编译器显式决定。

### S4 状态：✅ 完成（2026-09-13）

- **设置持久化**：`<home>/settings.json`（`llm_compiler` / `injection_policy`
  / `undo_keep`）；`GET|PUT /api/settings`；**环境变量仍然优先**（显式设置时
  覆盖文件并在响应里标注 `env_override`）；旧部署行为不变（默认全关）；
- **撤销面**：`GET /api/undo?limit=` 把最近的 `experience_execution` 与
  其备份快照合并返回（含 manifest），一键调用既有
  `POST /api/backups/{session}/restore`，并写 ledger `backup_restored`；
- **设置页**：`ui/www/settings.html` + `js/pages/settings.js` +
  `css/pages/settings.css`；含能力策略编辑（含特殊读取许可）、学习/复用开关、
  撤销列表（按快照撤销）、similarity 只读视图入口；全部页面导航新增「设置」；
- **验收**：`scripts/accept-s4-settings.ps1` PASS（默认值、持久化重启、
  env 覆盖、审计 `settings_updated`、undo 形状、similarity 形状、页面与脚本
  可服务）；`apps/experience-server/tests/s4_settings.rs` 2 个 HTTP 用例；
  `cargo test --workspace` **285 passed / 0 failed**。

## 6. 明确不做（本规划外）

- 不实现无沙箱的任意 shell 执行；
- 不做跨机器/多用户权限体系（先用本地单用户 + actor 审计）；
- 不把 computer-use 作为默认执行器（通道保留）；
- 不引入 embedding/RAG（规模索引另见 C5）。

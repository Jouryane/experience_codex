# L2 资格形成链设计（2026-09-09 定稿 v2，审查裁定已并入）

## 实现状态（2026-09-09）

- 已落地：Decaying + 单一转移矩阵（无 CANDIDATE→ACTIVE）、canonical 词汇
  单源、name 规范化、unstable_shape 保守拒收、五级门 3–5（ProbeSource/
  freshness、scratch dry-run、占位判据拒收）、Evidence/Activity 正交 +
  Qualification 输出、store transition 单一入口、API 收口 + force_activate
  reason 审计、learning-l1.json 信封、fixture 无 token 回归；
- 冻结待接线（如实登记）：Decaying 的 decay/tick 生产者与 pin 豁免（等
  L3 usage 反馈事件）；confidence/activity 落 usage 持久化；VALIDATED 对
  真实候选生效的条件 = L3 State 源接线 + 判据导出（当前服务端 resolver
  恒 None，诚实拒绝）；write_file/read_file 蒸馏参数形状缺口已通过“非
  exec 工具确定性蒸馏拒收”闭合，LLM compiler 落地后恢复。

> 更新（2026-09-09，WP1–WP3，experience-main 108b688）：前三项已接线——
> usage.json 持久化 ConfidenceRecord 并在 experience_execution 后写回；
> 负证据触发 Decay（ACTIVE→DECAYING，pin 豁免）；服务端 validate/
> revalidate 改用 fs State 源 resolver。write/read 的 LLM compiler 与
> pin API/UI 管理面仍待后续。

> 更新2（2026-09-09，WP4–WP7 收口）：pin 的 HTTP 入口与 ledger 审计已补
> （e041161，record_type pinned/unpinned + actor），UI 仍待接；usage
> 可经 GET /api/usage 观测，真机两轮 successes 1→2 且无 decay
> （accept-l3-loop PASS）。剩余 seam：pin UI、LLM compiler，列入 Stage S。

## 0. 目的与定位

L1 已把真实会话沉淀为 **CANDIDATE**（`CandidateWriter` 权力边界、necessity、
确定性蒸馏、validate 前两级、服务端 sink、learning-l1.json 审计；P1-1
Dirty 拒收与 P1-2 工具名规范化已在 `d6ccc47` 闭环）。L2 回答唯一问题：

> 这个候选经验什么时候获得执行资格？

资格 = 验证、证据强化与生命周期管理（L1 审查裁定：L2 不拆）。本设计稿
按 C1–C4 子块切分（validate 五级 → lifecycle 转移 → confidence → 管理面/
审计），并逐条关闭 L1 审查的 P2 账目。

## 1. L2 输入与输出

- 输入：L1 产出的 CANDIDATE（domain store，provenance 在
  learning-l1.json：session/thread、trace_version、outcome）+ 来源轮
  的 typed v1 trace（workspace/task）；
- 输出：VALIDATED / ACTIVE / DECAYING / DISABLED 状态与资格变更审计；
- 红线（架构不变量延续）：**L1 管道永不拥有激活权**；**自动进入执行
  路径（ACTIVE）只来自 VALIDATED**；用户显式 override 允许但必须审计
  并记录 reason（P2-1 收口）。

## 2. C1：canonical 工具契约 + 状态机补齐（设计先行）

### 2.1 canonical 工具契约

- producer 名 → canonical 能力名映射（已在 learning_l1::canonical_tool
  落地）：`commandExecution/exec_command → exec_command`，
  `write_file`、`read_file` 直通；未知工具在 L1 蒸馏层拒收；
- L2 约束：**入库/激活的 workflow 与 trigger 只允许 canonical 名**；
  原始 producer 名只留在 trace/审计，不进可执行体；
- `TOOL_BOUNDARY` 与 LocalRunner/Gate 能力词汇共用同一常量源（抽到
  domain 或 capability 模块，避免三处漂移）。

### 2.2 domain 状态枚举补齐（P2-4）

domain `ExperienceStatus` 补 `Decaying`（序列化 snake_case），全枚举：

```text
Draft · Candidate · Validated · Active · Decaying · Disabled
```

（`Draft` 与代码库 rich 模型的 `New` 语义对齐：L1 蒸馏失败的草稿也可
落 Draft，不进执行路径。）

### 2.3 全转移矩阵（含 pin 豁免）

| 当前 | 动作 | 结果 | 条件/审计 |
|---|---|---|---|
| Candidate | validate（门 3–5 全过） | Validated | 每轮资格证据写入 usage |
| Candidate | invalid / validate 失败 | Disabled 或 Draft | 审计 reason=invalid_gateN |
| Validated | activate / force_activate | Active | 激活唯一合法源 = Validated；override 可覆盖“自动激活决策”，不能覆盖“最低资格证明” |
| Active | tick 衰减 / usage 负证据 | Decaying | pin 豁免：pinned Active 永不衰减 |
| Decaying | activate（revalidate 成功） | Validated→Active | 证据恢复路径 |
| Decaying | disable / 继续衰减 | Disabled | |
| 任意态 | disable | Disabled | 审计操作者 |
| Disabled | revalidate | Validated | 需要新证据 |

实现：状态转移表做成单一函数（domain/lifecycle），API 与管理面都调用它，
不允许 remove+insert 直改 status（P2-1）。**CANDIDATE → ACTIVE 的任何
通道（含 override）都不存在**：override 与自动激活共享“必须 VALIDATED”
的前置；未来如确需专家强行启用，另设计更高权限 unsafe_activate（当前
不做）。

### 2.4 name 规范化（P2-2）

候选/经验名统一：`cand_<task_signature>` 中的空白 → `_`；name 只允许
`[A-Za-z0-9_-]`（迁移存量：读时兼容，写时规范化）。`unstable_shape` 落
最简**保守拒收启发**：args_summary 中出现长度 ≥ 16 的 hex/uuid 形态片段
→ `likely_ephemeral_identifier` → 拒（reason=unstable_shape）。这是
“当前 L1/L2 拒收规则”，不是“随机性判定”——commit hash/UUID/数据库 id
等合法业务参数同样被保守拒收，且**只拒候选、不 assert invalid**。

## 3. C2：validate 五级完整接线

| 级 | 内容 | L1 现状 | L2 动作 |
|---|---|---|---|
| 1 | Schema valid | ✅ store 强制 | 复用 |
| 2 | Tool boundary valid | ✅ canonical + 非空参数 | 复用 |
| 3 | State prerequisite observable | — | 每个 predicate 绑定一个可验证的 **State Source / Probe**（binding + type + observable + freshness）；observable 回答“此谓词现在能否被可靠求值（含新鲜度）”，不是“key 是否在官方字典”。L2 不建封闭 State Ontology，只**冻结 StateProbe/StateSource 接口语义**给 L3 |
| 4 | Dry-run replayable | — | workflow 每步经 ToolRouter 解析（canonical 工具 + 参数形状），write_file 类步骤在隔离 scratch 执行一次以**验证 ToolRouter/runner 行为**（path/content 可解析、runner 可接受、scratch 可执行），exec_command 只解析不执行（v1 边界）。scratch 成功**不推断**真实 workspace 有效——那是 Gate 5 的职责 |
| 5 | Completion criterion observable | — | postconditions 不再是 `candidate.pending_validation` 占位：资格证据来自真实轮/隔离重放，产出可观测谓词（绑定 State Source，可求值且带 freshness）；verification 步骤解析通过；无法导出真实判据 → 停留 CANDIDATE（诚实）。Gate 4 验证“runner 行为”，Gate 5 验证“完成判据可观察”，二者不重叠 |

### 3.1 State 接口语义（本子块冻结，不实现完整 Registry）

```text
StateFact {
    key, value, source, observed_at, freshness_ttl, confidence
}
predicate { key = expected }
   └── binding: StateSource/Probe（可求当前值 + freshness）
```

State 源可自然扩展（network/browser/docker/git/gpu…），不需要预建
“完整状态宇宙”；新增源只需实现 StateSource 接口，不改 ontology。

验收口径（L1 审查：尽早钉死，避免被 L3 卡住）：

- “命中”= store 索引级匹配（`active_by_tool` / `candidates_for(tool)`），
  不要求 L3 runtime 已接线；
- “disable 后不再命中”= `candidates_for(tool)` 不含该经验；
- mock 五级门必须拒绝“schema 合法但谓词不可观测 / 步骤不可解析 / 判据
  不可观察”的垃圾候选。

## 4. C3：confidence 规格（先写后写码）

### 4.1 语义（L1 审查裁定）

`confidence` 是**证据变量**：历史证据对经验的支持程度，不是 Runtime 的
信任分、不投票决定真理。证据增/减永不改变“某个谓词事实是否成立”。

### 4.2 两个正交变量（审查裁定：activity ≠ 质量）

```text
Evidence  { successes, misfires, invalid }            # 证据支持程度
Activity  { last_used_at, usage_count }               # 近期使用程度
```

`confidence = evidence quality`；`recency = recent activity`；
`lifecycle = confidence + evidence + policy`（**不是 inactivity**）。
低频 ≠ 低质：罕见但正确的经验保持 ACTIVE，仅降低优先级；禁止
“未使用 → Decaying”的隐式推导，避免高频经验垄断生态。

### 4.3 字段与存储

```text
EvidenceCounters { successes, misfires, invalid, last_used_at }
ConfidenceRecord { counters, score: 0..1, updated_at }
```

- 存储：usage 侧（usage/audit），**不进 Experience 可执行体**（轻量）；
- 来源：命中反馈与生命周期事件写 usage，聚合出 score；
- baseline：与现有 rich 模型常量一致（CANDIDATE 初始值沿用
  `INITIAL_CANDIDATE_CONFIDENCE`），迁移不二义。

### 4.4 更新函数

- success（命中且验证通过）→ successes+1，score 上调；
- misfire（MissingSource/ParamMismatch，非经验之过）→ misfires+1，score
  **不变**；
- invalid（ToolError/Semantic）→ invalid+1，score 下调；
- tick → 未用经验按半衰衰减 → 触发 Decaying（pin 豁免）。

### 4.5 给 L3 的输出：Qualification（权力边界）

```text
ExperienceQualification {
    valid: bool,
    confidence: f32,
    lifecycle: ExperienceStatus,
    evidence: Evidence,
    recency: Activity,
}
```

L2 只回答“这个经验有资格”（输出 Qualification），**不持有最终执行决策
权**。L3 才做：`State + Qualification + Cost + Priority + Conflict +
Intent → Decision`；L4 回答“LLM 要执行该 Action 时是否直接接管”。三个
问题分属 L2/L3/L4，互不混淆。

## 5. C4：管理面 / 审计 / 可复跑

- API 状态变更统一走转移表；`force_activate` 需显式 reason，写审计；
- UI：状态显示、候选“申请验证”/“验证”→VALIDATED、显式 override 需
  理由输入；隐藏“任意态直改 ACTIVE”入口；
- **P2-3 可复跑验收**：新增 `scripts/accept-l1-sink.ps1`（冷沉淀 +
  同任务重跑 + no-action 轮三段断言），并把一份 ledger + store fixture
  入库（`fixtures/learning/`），作为 L2 回归基线；
- **P2-5 敏感参数策略**：args_summary 脱敏规则与 evidence 策略在 C3 定稿
  （默认不存 env/密钥类键值；L3 注入前强制生效）；
- 审计记录沿用 learning-l1.json 信封扩展（记录类型
  written/rejected/promoted/activated/decayed/disabled/override）。

## 6. 非目标（L2 明确不做）

- 不接 runtime 委派/注入（L3）；不接 Action Gate（L4/M5）；
- 不做 embedding/RAG；
- 不做跨 workspace 泛化判定（保留给后续）；
- L1 的 LLM compiler（脏轨迹蒸馏）不在 L2 范围，除非必要性上升。

## 7. 子块计划与验收

| 子块 | 交付 | 验收 |
|---|---|---|
| C1 | canonical 契约收口（常量同源）、Decaying + 转移矩阵、force_activate 审计、name 规范化、unstable_shape 决策 | 转移矩阵单测（含非法转移拒、override 审计）、Decaying 序列化回归 |
| C2 | 五级门完整实现 + scratch dry-run + 判据导出 | mock 五级拒垃圾；candidates_for 命中/disable 断言 |
| C3 | ConfidenceRecord + 更新函数 + 联合决策签名 | 单测钉“证据增/减 与 真值无关”（counter 变化不翻转任何 predicate） |
| C4 | 管理 API/UI 收口 + 审计扩展 + accept-l1-sink.ps1 + fixture | 脚本三段真机 PASS；override 审计记录可查 |

## 8. 协作节奏

- 本设计稿：L2 开工前轻审（C1/C3 语义点）；
- C1–C4 按块推进并提交，自查账本 + mock 回归；
- L2 完成 + 真机（复用 accept 脚本）后整体审查一次。

## 9. 审查裁定（2026-09-09 并入）

1. **force_activate 仅限 VALIDATED → ACTIVE**：override 覆盖“自动激活
   决策”，不覆盖“最低资格证明”；CANDIDATE → ACTIVE 任何通道不存在
   （§2.3）；
2. **Gate 3 语义修正**：predicate 绑定 State Source/Probe 并回答
   “当前能否可靠求值（含 freshness）”，不建封闭 State Ontology；
   StateFact 含 key/value/source/observed_at/freshness_ttl/confidence
   （§3.1）；
3. **Gate 4 scratch = 验证 ToolRouter 行为**，不推断真实 workspace
   有效性，与 Gate 5 不重叠；
4. **confidence 与 activity 分离**：低频不衰减为 DECAYING，只降优先级；
5. **L2 输出 Qualification**，不定义最终 Decision（L3 联合决策，
   L4 接管）；
6. **unstable_shape = 保守拒收启发**（likely_ephemeral_identifier →
   拒候选），不是随机性判定，不 assert invalid。

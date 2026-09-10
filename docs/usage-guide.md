# Experience 使用说明（第一性问答）

> 更新：2026-09-09（Stage S 收口后）。本文回答四条第一性问题；标注
> 【现状】= 已在代码/真机验证中落地，【讨论/规划】= 设计推演，尚未成为
> 默认实现。项目总账见 README「阶段状态与下一阶段」。

## Q1 一万条经验、分布不均匀：如何快速唤醒（仅讨论）

一万条“不均匀”意味着经验不是相互独立的随机文本：它们共享任务形态、
工具面、场景、State 谓词与参数模式。快速唤醒的本质不是“对一万条做相似
度”，而是**先分桶、再短路、最后校验**：

1. **两级路由**：入口先做廉价确定性分桶（任务签名/trigger.tool/场景域/
   workspace 域/agent 域），只把桶内候选送入精排；桶外直接 MISS。
   现状的 `active_by_tool` 索引就是这一思想的雏形（按 tool 键控）。
2. **共性归纳成“家族/模板”**：分布不均匀 = 少数高频形态覆盖大多数任务。
   把经验按 trigger 模式、workflow 形状、postcondition 键聚类成家族；
   家族级索引（如 10 个家族 × 每族 1000 条参数化实例）使唤醒宽度从一万
   降到十级。参数（文件名/路径/内容）留在 State/参数槽，不写死进本体。
3. **冷热分层**：usage（hits/last_used）只决定**优先级与缓存驻留**，
   不决定质量（L2 裁定：低频 ≠ 低质）。热桶驻留内存/前缀树，冷桶延迟
   打开；每次唤醒先查热路径。
4. **倒排/前缀 + State 谓词预筛**：对任务词元建倒排；对 `file:*`/
   `cwd.*` 等可求值谓词按“现在能评估且为真”预筛，缩小到可直接执行的
   候选集。
5. **唤醒终点仍是确定性 Select Plan**：召回只负责“别漏”，执行决定仍由
   L3 规则（覆盖→资格→confidence→冲突不自动执行）给出；被召回的
   参考/未命中经验进入 skipped/omitted 审计，绝不静默执行。

结论：一万条不均匀 → 真正要解决的是“路由 + 家族抽象 + 冷热”，不是把
一万条都算一遍相似度。此节为讨论口径，暂不改变默认实现。

## Q2 一百万条：快速匹配难题、为什么现在不这样建、用户如何自行调整

### 2.1 一百万条的匹配难题

线性/按 tool 小索引在百万级必然失效。工程化路径（按依赖顺序）：

1. **物理分区**：按场景域/workspace/agent 分库分表（多 home 或多
   store），每条经验带显式路由键；入口先路由到分区，而不是全量。
2. **倒排 + 前缀索引**：任务签名词元倒排、trigger 模式前缀树，把候选
   收敛到百级。
3. **向量粗召回 + 确定性精排（可选层）**：embedding 只做“候选生成”，
   命中后仍走 State probe + 确定性 Select Plan；语义层永远不拥有控制权。
4. **状态分层缓存**：ACTIVE 常驻子集 + usage 热度驱逐；冷数据按需加载。
5. **生命周期治理**：decay/disable/归档由 usage 证据驱动；百万条里真正
   ACTIVE 的应远小于总量，索引只服务 ACTIVE 面。

### 2.2 为什么项目现在不这么建设

- 当前经验量是数十条级（真机 fixture 数十条、仓库内手工+沉淀 < 20），
  机械匹配 + tool 索引 + 顺序校验在数量级内正确且可证；
- 架构红线是“确定性优先、不建封闭 State Ontology、不做默认 embedding/
  RAG”（p1/l1 非目标）；在证据不足时引入向量层 = 用模糊召回换掉已被
  真机验证的确定性语义，属于过早抽象；
- 经验本体 schema、State 谓词、L3 控制权语义与规模无关——未来加索引层
  不需要改执行语义，这正是现在“不加”仍然正确的理由（YAGNI +
  可演进性）。

### 2.3 用户拓展到百万条时如何自行调整

1. **先分区**：把 store 按域拆成多 home（EXPERIENCE_HOME），或给经验名
   加域前缀，入口按域路由（现有 trigger/name 体系兼容）；
2. **只给 ACTIVE 建索引**：候选索引面向 ACTIVE 面，CANDIDATE/VALIDATED
   归档离线；这是 schema 层就支持的（匹配面索引本就只含 ACTIVE）；
3. **开启可选元数据列**：在不改 Experience 本体的前提下，在索引后端加
   usage 热度、场景域、embedding（用户自选向量库），作为候选生成器；
4. **缓存与衰减**：热 ACTIVE 子集常驻；用 usage 证据驱动 decay/disable，
   定期把零命中经验移出热区；
5. **查询/审计面**：`GET /api/usage`、`GET /api/experiences`、UI 与
   learning-l1/usage.json 审计文件已提供计数与证据入口；百万级再加分页
   与过滤查询面即可，不改变数据文件格式（additive 字段兼容）。

## Q3 十条经验、五种类型：执行过程与“Experience 如何操作计算机”

先明确五类的语义（同一 Experience schema，差别在“意图与使用位”）：

| 类型 | 含义 | 执行形态 |
|---|---|---|
| 独立结果类 | 任务=问结果，不要求副作用 | 查表/复用已验证结论，直接产出最终结果（可零 LLM） |
| 独立状态类 | 任务=确认/断言环境状态 | 只探测、不修改：真实 State 谓词求值（cwd/file），产出证据供后续判据 |
| 独立工作流类 | 任务=做出一串副作用并验证 | 预编译步骤本地执行（write_file）→ postcondition 验证 → 剩余 delegate |
| 参考类 | 任务=给 agent 提供上下文 | 不执行；policy_on 时注入“仅参考、可质疑”文本并审计 injected/omitted |
| 嵌入计划的结果/状态类 | 大任务中间产物 | 作为 Select Plan 的子结论/证据被引用：结果类给下一步输入，状态类做前置/后置谓词 |

一条任务进入后的执行链（现状 L3 闭环）：

```text
Task + workspace
  → 任务文本机械匹配 ACTIVE（command_pattern 子串 + 否定守卫）
  → State probe（fs：cwd/file，真实求值，三值：真/假/不可观测）
  → Select Plan（确定性：覆盖→confidence→冲突不自动执行）
  → 执行（本地 runner 做 write_file 等副作用）
  → 验证（postcondition 由 verification/read_file 取真实 evidence）
  → 已完成步骤/已验证状态写回委派文本（勿重复执行 notice）
  → 剩余 delegate（session channel）→ usage 分层写回
```

以 10 条为例说明“Experience 怎么操作计算机”：

- 任务“确认 probe.txt 内容”命中**独立状态类**：只读文件、产出
  `file:probe.txt.content=EXPERIENCE_GATE_SUCCESS` 证据，无副作用；
- 任务“创建 probe 文件”命中**独立工作流类**：本地执行 write_file，
  读回验证，返回“已完成+已验证”给委派方；
- 任务“生成报告（先建 probe 再总结）”命中**嵌入计划的结果/状态类**：
  状态类先证明前置成立，结果类/工作流类产出中间文件，委派文本携带
  completed step ids + verified state，agent 拿到的不是“让它猜”，而是
  “已经发生的、可验证的中间世界”；
- 任务形态已知但证据不足 → **参考类**注入 agent 上下文（默认 off，
  开启即审计）；
- 完全未知 → 纯委派，零事件零注入（v1 语义）。

关键点：Experience 对计算机的“操作”分三类——probe（只读观察）、
executor（受控副作用）、delegate（把不可本地执行/未知部分外包给 agent），
且全程遵守“不伪造状态（验证后才声称完成）、失败即停、不吞错误”。

## Q4 一条经验是如何形成与存储的

形成链（L1→L2）：

```text
真实会话完成（typed trace v1，单写者）
  → trace_reader 选合法轮（completed + turn_completed）
  → L1 necessity 门（终态/无动作/repeat/unstable 拒）
  → distill：干净短轨迹 → DeterministicDistiller；
           脏轨迹（>3 步）→ LLM compiler（env 门控，JSON-only，成功路径摘要）
  → validate 前两级（schema valid + tool boundary，干跑不执行）
  → CandidateWriter 强制写入 CANDIDATE（结构上无 L1→ACTIVE 通道）
  → learning-l1.json 审计（written/rejected + reason）
  → L2 五级门（fs State 源可观测 + dry-run + 完成判据可观察）
  → VALIDATED → Activate/ForceActivate（审计 reason）→ ACTIVE
  → 执行后 usage.json 写回 ConfidenceRecord（evidence/activity/score）
```

存储位置与内容（现状）：

| 文件 | 内容 | 与本体关系 |
|---|---|---|
| `<home>/store.json` | Experience 本体：name/trigger/pre/postconditions/workflow/verification/status + pinned | 唯一可执行体来源 |
| `<home>/learning-l1.json` | 审计账本：written/rejected/promoted/activated/decayed/delegate/injection… | 不进本体、不进 prompt |
| `<home>/usage.json` | ConfidenceRecord（evidence/activity/score，2 位小数） | 元数据，与本体分离 |
| `<home>/sessions.json` | 会话与 typed trace v1 | L1 唯一合法输入 |

本体 JSON 样例（创建 probe 文件的 ACTIVE 经验）：

```json
{
  "name": "create_probe_file",
  "trigger": { "tool": "exec_command", "command_pattern": "create probe file" },
  "preconditions": [ { "key": "cwd.exists", "expected": true } ],
  "workflow": [ { "action": "write_file",
                  "args": { "path": "probe.txt", "content": "EXPERIENCE_GATE_SUCCESS" } } ],
  "postconditions": [
    { "key": "file:probe.txt.exists", "expected": true },
    { "key": "file:probe.txt.content", "expected": "EXPERIENCE_GATE_SUCCESS" }
  ],
  "verification": [ { "kind": "read_file", "path": "probe.txt",
                      "expect_content": "EXPERIENCE_GATE_SUCCESS" } ],
  "failure_policy": "stop_and_report",
  "undo": "unsupported",
  "status": "active"
}
```

存储原则：Experience 本体只存“可执行转移”，不存日志/叙事/推理；
confidence/activity 不进本体（轻量原则）；格式变更 additive 用 serde
default 兼容，破坏性变更才升 schema 版本。

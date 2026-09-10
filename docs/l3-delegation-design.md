# L3 委派闭环与先动手设计（2026-09-09 定稿 v2，审查裁定已并入）

## 0. 定位与权力边界

L2 冻结了“资格”（CANDIDATE→VALIDATED→ACTIVE，证据变量、单一转移入口）。
L3 回答：

> 在当前 State 和当前 Task 下，哪些已获资格的 Experience 可以先替
> 人类/LLM 完成哪些事情？

**核心定义（裁定）**：L3 = State/Experience 匹配后的**任务级编排器**，
决定“执行哪些已知 Transition、执行后还剩什么”，不是决定“LLM 应该怎么
做”。L3 的产物是 **Select Plan**，delegate/注入/执行都不是内核。

三个问题分属三级（l2-qualification-design §4.5）：L2 “有没有资格”；
**L3 “现在该不该用、用哪一部分”**；L4 “LLM 要执行该 Action 时是否直接
接管”。L3 不做 Action 级 Gate（那是 L4/M5）；不做多 Agent 适配（保持
冻结）。

硬顺序来自 core-principle §2.5：**先动手（兑现已知 State→State'）→
只委派剩余 → 参考经验第二位、可质疑、可省略**。

## 1. 输入与输出

- 输入：任务（task + workspace）、L2 输出（store ACTIVE 匹配面 +
  Qualification）、首批 State 源实现（cwd/file 探针，L2 接口已冻结）、
  用户策略（注入开关/预算）；
- 输出：执行报告（done/partial/delegated/failed + 已兑现副作用清单）、
  delegate 记录（进 trace/audit）、usage 反馈（驱动 L2 confidence 与
  decay——闭合 L2 seam 2/5）。

## 2. 控制流（外层编排）

```text
Task ─────────→ Task Segments（A/B/C/D）
State ─────────┐
               ↓
          Match ACTIVE（candidates_for）
               ↓
          Select Plan（确定性）
               ↓
          Experience execution
               ↓
          State verify
               ↓
        ┌──────┴──────┐
     all covered   uncovered
        ↓              ↓
      DONE          DELEGATE → LLM
```

**Remaining = 原始任务结构中尚未被已验证 Transition 覆盖的 Task
Segment**（不是文本扣除）。**Select Plan 是 L3 内核**：

```text
ExperiencePlan {
    selected: [E17, E23],
    skipped: [E31],
    covered_segments: [A, B],
    remaining_segments: [C, D],
    reason: ...,
}
```

确定性冲突规则（无 LLM 决策）：1) State predicate 完整匹配 → 2) Task
coverage 最大 → 3) Qualification 更高 → 4) confidence 更高 → 5) cost 更低
→ 6) priority 更高 → 7) 冲突 → 不自动执行。

## 3. 子块拆分（沿用 C 式粒度）

### D1 delegate 一等操作 + trace 事件

- delegate 与 write_file/exec_command 平级（core-principle §3.5），需要
  新的 typed trace 事件 kind **`delegate { agent, task_summary }`**
  （裁定：不复用现有 kind）；已落地 TraceEvent::Delegate；
- delegate 进 trace = necessity/usage 能回答“这次为何外包、能否蒸馏”。

### D2 首批 State 源 + 真实资格生效

- 实现 L2 已冻结的 StateSource/Probe 接口的首批绑定：`cwd.exists`、
  `file:<path>.exists/content`、`runtime.exec_allowed`；
- 作用：让 VALIDATED/ACTIVE 的真实资格链首次可走通（L2 seam 4 闭合
  的前提），并支撑兑现后的 postcondition 验证。

### D3 兑现编排（先动手）

- match → identify covered task segment → execute Experience → verify
  resulting State → **mark segment completed** → delegate uncovered
  segments（裁定：结构化步骤结果驱动，L3 内不允许 LLM 摘要式扣文本）；
- 步骤结果流：`StepResult[]`（✓/✗/not_run）→ completed/remaining；
- **v1 收窄回退**：暂无 Task Segment 结构时，partial → delegate 原任务 +
  附加 completed step IDs / verified state（仍不让 LLM 重写剩余文本）；
- 执行复用 controller runner 同步语义：失败即停、partial 带
  executed_side_effects、不吞失败；
- 完成判据来自 L2 五级门导出的可观测谓词，此处真实 probe 求值；
- done 不进 LLM。

### D4 注入策略与 usage 回流

- 参考经验注入：只来自蒸馏后的 Experience 本体（不注入 trace），
  标注“仅参考、可质疑”；策略 = 注入开关 + token 预算上限 + 命中数上限
  （**默认不注入**：仅 user policy=enabled 才提供 reference，并记录
   injected / omitted + reason）；
- usage **分层**（裁定：不把 delegate 失败算到 Experience 头上）：

  ```text
  experience_execution: success | misfire | invalid | execution_error
  delegation:           delegated | completed | failed
  task_outcome:         success | partial | failed
  ```

  Experience 成功执行后 delegate 失败 → Experience 仍是 success，不污染
  confidence（L2 “证据变量、非真值投票”在 L3 的第一次真实落地）；
- Activity 与 confidence 写回 L2 ConfidenceRecord（ledger 信封扩展），
  低频不衰减；
- 审计：delegate/注入决策与省略原因都记 ledger。

## 4. 验收（真机对照三档 + 回测）

1. 全已知：真实任务命中 ACTIVE 经验 → 零/少 LLM 完成 + postconditions
   验证通过；
2. 部分已知：任务先兑现可做部分，剩余最小文本 delegate → 成功；
3. 全未知：纯委派（注入按策略）；
4. 回测：复用/命中后 ledger 出现 usage 反馈，confidence/activity 变化
   可观测（L2 C3 验收口径在 L3 接线后闭合）。

## 5. 非目标（L3 明确不做）

- Action 级同步 Gate（L4 / codex-main M5）；
- 多 Agent 适配（仍只接 codex）；
- 自动注入一切经验（策略默认轻量、省略有审计）；
- 完整 State Registry（只落首批探针，接口语义已冻结）。

## 6. 依赖与并行

- L3 依赖 L2 store/qualification/state 接口（已具备）；M5（codex-main）
  验证内层拦截，与本块验证外层编排互不等待，可并行推进；
- LLM compiler（脏轨迹蒸馏）不在 L3 范围，除非必要性上升。

## 7. 审查裁定（2026-09-09 并入）

1. **delegate trace**：新增独立 `delegate { agent, task_summary }` kind，
   不复用（TraceEvent::Delegate 已随本稿落地）；
2. **注入默认**：纯委派默认不注入；user policy=enabled 才提供 reference，
   并记录 injected/omitted + reason；
3. **partial 剩余任务**：结构化步骤结果驱动；L3 内禁止 LLM 摘要式扣文本；
   无 Task Segment 结构的 v1 收窄为“delegate 原任务 + completed step
   IDs / verified state”；
4. **Select Plan 是 L3 内核**（任务级编排器；产出 ExperiencePlan；确定性
   冲突规则；冲突不自动执行）；delegate/注入/执行都不是内核。

## 8. 范围标签与实现状态（2026-09-09 L3 v2）

- **L3 v1**（f659bf9，验收 PASS）：任务入口检测 ACTIVE 经验 → 计划性委派
  （原任务 + plan 名单进入会话通道；delegate 事件与 delegate 决策 /
  delegation_completed|failed 分层入 ledger；无 ACTIVE 纯委派零事件）。
- **L3 v2（本批）**：任务文本机械匹配 ACTIVE（trigger.command_pattern
  非空且大小写不敏感命中；无 pattern 的经验无 Task Segment 时绝不自动
  执行；P1-1 守卫：命中位置前 ≤32 字符窗口出现否定/回避词
  not/never/don't/avoid/skip/without/except/no 时不自动执行——子串匹配是
  占位语义，不是自动执行的安全判据）→ write_file-only workflow 先本地
  执行 + 真实验证（LocalRunner
  契约；唯一最优覆盖者执行，等覆盖 = 冲突不自动执行）→ v1 fallback
  委派原任务 + 已完成步骤/已验证状态（delegation_text，禁 LLM 摘要式扣
  文本）→ `experience_execution`（success/misfire/invalid/execution_error）
  分层入 ledger；委托与任务结果永不污染 Experience confidence（D4）。
- 范围边界（仍非目标）：exec_command 步骤无进程内执行器（照旧委派）；
  Task Segment 结构未做（v1 fallback 收窄）；policy_on 参考注入未接线；
  L4/M5 内层 Action Gate 不属本批。

## 9. 裁定：delegate 结构化 plan 与截断口径（2026-09-09）

- `task_summary` 维持 ≤200 字符短标签语义，原任务过长允许截断；
- **plan 以结构化字段落在 `TraceEvent::Delegate`（`plan: Vec<String>`）**，
  截断不可能再吞掉 selected 名单；v1 产物缺该字段时读回空列表（serde
  default，向后兼容）；
- 长任务标签即使失去 `[plan: ...]` 后缀，审计仍可从结构化 plan 与
  ledger `candidate_name` 还原完整计划。

## 10. WP4 范围裁定与实现账本（2026-09-09）

- **Task Segment / “全已知零 LLM” = 本阶段非目标**：任务入口只有文本，
  没有上游结构化段；L3 禁止 LLM 摘要式切分文本（红线），因此不实现
  自动段拆分，也不宣称“全已知”。覆盖判定以“本地可执行部分已兑现 +
  v1 fallback 委派原任务”的诚实形态推进。
- **resume 语义裁定**：resume = 同线程继续委派，不重算 plan、不重执行
  本地 workflow；delegate 事件只在初始任务入口写入（代码自
  0be587e 起即为此语义，本轮文档化）。
- **usage 存储**：`<home>/usage.json`（schema_version=1，entries 按经验名
  存 L2 ConfidenceRecord），与 learning-l1.json 审计分开；经验本体不含
  confidence/activity。
- **WP1–WP3 已落地（108b688）**：usage 写回 + Decay 生产者（pin 豁免经
  store envelope `pinned` 名单接线）+ validate/revalidate fs 资格源 +
  EXPERIENCE_INJECTION_POLICY（默认 off）与 injection 审计。
- 范围边界（仍非目标/未接线）：pin 的 API/UI 管理面（C4）；policy 的
  token/命中上限参数化；exec_command 进程内执行器；L4/M5 内层 Gate。

## 11. 阶段收口（2026-09-09 WP1–WP7）

- L3 外层闭环已闭合（v1→v2→WP1–WP4，真机 accept-l3-loop PASS）；L4/M5
  内层 Gate 真机四问 PASS 且 GateHitResult→usage 写回完成（codex-main
  `7db612899`/`57df9f1d9`）；L0–L4 文档终点达成。
- 审查修复（e041161）已并入：否定句守卫、audit 串行写、execution_error
  独立归因、score 取整、pin HTTP+审计。
- **P2-4 待裁定（不阻塞完成声明）**：execute-first 后委派文本仍含原任务
  全文，真机证据显示 codex 会重跑已知段（Set-Content probe.txt）；修复
  依赖“剩余任务”语义（Task Segment 或结构化委托），选项与建议见
  docs/next-stage-plan.md S1。

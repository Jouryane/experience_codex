# L1 学习回流设计 + L1–L4 分级口径（2026-09-09 定稿 v2，审查裁定已并入）

## 0. 目的与定位

P2 冻结了 Learning 的输入契约（typed trace v1，docs/p2-trace-v1.md）：
会话完成轮 → 语义里程碑事件 → 轮次/任务/终态/失败语义。L1 是把这个
契约接到产品路径上的第一段：**真实会话自动沉淀出可重放的经验候选**。

本设计稿先冻结 L1–L4 的**分级口径**（谁是审查对象、每级的定义/交付/
验收），再给 L1 内部规格。逐级定义此前在文档中只有一句“v1 冻结后
接入”，本稿补上这层账。

## 1. L1–L4 分级口径（审查核心）

Ladder 回答一个问题：经验如何从“真实执行结果”一步步变成“反过来接管
执行的能力”。L0–L4 定义如下：

| 级 | 名称 | 定义（做什么） | 交付物 | 验收（最小） | 前置 |
|---|---|---|---|---|---|
| L0 | 契约探针（已做） | 只读 trace → distill 示例 → schema 合法 Draft；不落库 | learning_smoke + trace_reader | typed v1 → Draft schema 合法 | typed v1 冻结 |
| **L1** | **学习（自动沉淀）** | 真实 session 完成后自动触发 necessity → distill → validate（结构+工具边界）→ 经 **CandidateWriter** 写入 store 为 **CANDIDATE**。**架构不变量：L1 最高权限 = CANDIDATE，不拥有 ACTIVATE 权限** | l1 回流入口、necessity 判定表、distiller 接口（LLM compiler + 无 LLM 降级）、validate 前两级、CandidateWriter、入库+来源 usage | 真机冷沉淀：任务成功 → store 出现 CANDIDATE 且校验全过；失败/取消/重复任务不沉淀；**类型/API 上不存在 L1→ACTIVE 路径** | 本稿 |
| L2 | 资格：验证、证据强化与生命周期管理 | 完整资格形成链：validate 后三级（前置谓词可观测 / dry-run 可重放 / 完成判据可观察）→ VALIDATED → evidence/usage → confidence → ACTIVE；DECAYING/DISABLED 自动管理。**confidence 是证据变量，不是信任分/真值投票** | validate 五级完整接线、feedback→lifecycle、管理面（confirm/pin/revalidate/disable） | mock：五级拒“格式正确但不可执行/不可验证”的垃圾；真机：复用命中后 confidence/usage 变化可观测，disable 后不再命中；confidence 语义测试 | L1 |
| L3 | 委派闭环与先动手 | 外层 Decision 用 ACTIVE 经验**先兑现已知转移**，只委派剩余；delegate 进 trace；参考经验注入 executor（第二位、可质疑、可按策略省略） | runtime decide→delegate 接线、注入策略（默认轻量） | 真机对照：经验可做的部分直接做掉 + 剩余委派；注入 token 受控可测 | L2 |
| L4 | 接管：内层 Gate + 全生命周期闭环 | Action Gate 在“Action Proposal 后、副作用前”同步接管：`(State, Intent/Action) → Known Transition → State'`；GateHitResult 回 usage；衰减/遗忘/审计闭环 | Gate 接线（模型 A 内嵌；模型 B 契约保留）、审计查询面 | 真实 Codex loop：HIT 不 dispatch 原工具、真实兑现+验证、合法输出注入、loop 继续（M5 四问） | **验收依赖** codex-main M5；**工程开发与 L3 并行** |

### 1.1 权力等级（本 ladder 的语义内核）

L0–L4 描述的是 Experience 如何从“数据”逐步获得“执行权”：

| Level | Experience 获得的能力 | 一句话 |
|---|---|---|
| L0 | 被观察 | “我能读懂发生了什么” |
| L1 | 被产生 | “我能从发生过的事情产生候选” |
| L2 | 被证明 | “这个候选被证明可以执行” |
| L3 | 被调用 | “经验可以替代一部分 LLM 工作” |
| L4 | 能够接管 | “经验可以在副作用前直接拦截/兑现 Action” |

因此 L1 与 L2 是**权力边界不同**，不是成熟度差别：L1 = 学习，
L2 = 证据验证 + 获得执行资格。“刚学到就相信它”的因果跳跃被结构排除。

**口径要点**：

- L1 只到“合格候选”，**不拥有 ACTIVATE 权限（架构不变量）**。代码层用
  `L1 learner → CandidateWriter → CANDIDATE` 表达：L1 不持有可任意写
  ACTIVE 的 store API；
- L2 的 validate 是 L1 的“结构+工具边界”的延伸，不是新引擎；lifecycle/
  confidence/usage 在 experience-core 已有雏形（自 codex-main 迁移），
  本阶段是接线与行为校准，不是重写；**L2 不拆**——后三级门 + lifecycle/
  confidence 同属“资格形成链”（一个已 VALIDATED 但无 lifecycle 管理
  的经验不该悬在“可否执行”之间）；
- **confidence = 证据变量**（历史证据对经验的支持程度），不是 Runtime
  的信任分，更不是“投票决定真理”。进入执行的最终判定是共同决策：
  `State match + Experience validity + confidence + cost + priority +
  conflict resolution`；高 confidence/高 usage 不天然豁免审查与调改；
- L3 的顺序硬约束来自 core-principle §2.5：**先动手、后委派、参考第二**；
- L4 与 codex-main M5 是**验收绑定**而非开发绑定：M5 验证内层副作用
  拦截语义（LLM proposes Action → Gate → HIT/MISS → verify → loop），
  L3 验证外层编排语义（先兑现 → delegate → 参考注入），两者无相互
  等待关系，可并行推进；L4 最终验收在 M5 完成后进行；
- 多 Agent / 模型 B（外部 Runtime + IPC）不进入 L1–L4 的执行依赖，
  保持冻结口径（compatibility §2.5/§3）。

### 1.2 状态（2026-09-09 WP1–WP7 收口）

- L0 契约探针 ✅ / L1 学习 ✅ / L2 资格 ✅ / L3 委派闭环 ✅（v1+v2+
  WP1–WP4，真机 accept-l3-loop PASS）/ L4 接管 ✅（M5 真机四问 PASS +
  GateHitResult→usage 写回，codex-main `7db612899`）；
- 剩余非阻塞：P2-4 剩余任务语义裁定（README「阶段状态」）、pin UI、
  L1 脏轨迹 LLM compiler（write/read 蒸馏恢复）——列入 Stage S。

## 2. L1 范围（本阶段做/不做）

**做**：

1. 回流入口：真实 session 终态（completed）→ `trace_reader` 取出合法轮
   （task + typed v1 events）→ L1 流水线；
2. necessity gate（判定表，见 §4）；
3. distiller：trait + 一种 LLM compiler（单次小调用，遵循最小成功路径
   压缩）+ 无 LLM 时的确定性降级（只对“干净短轨迹”直接产出草稿）；
4. validate 前两级：schema valid + tool boundary valid（干跑不执行）；
5. 入库：CANDIDATE（复用 Experience schema/版本化/store），附来源
   round_id + source_trace_version=1；
6. usage 侧记录：哪些 session/round 触发/被拒（审计按需查询，不进
   prompt）。

**不做**：

- 不自动 ACTIVATE / 不进执行路径（属 L2）；
- 不做 validate 后三级（L2）；
- 不做 delegate 进 trace 与注入策略（L3）；
- 不做 Gate（L4 / codex-main M5）；
- 不做 RAG/向量/embedding（保持机械匹配口径）；
- 不把 trace 原文、双写历史、legacy 会话当输入（p2 冻结）。

## 3. 输入契约（复用，不改）

- 输入：`trace_reader::select_round(session, RoundSelection::Last)` 的
  结果（已完成会话的最后一轮；含 legacy 的会话整体拒绝；无
  turn_completed 拒绝；task 取 round_tasks[round]）；
- 来源：`session_id + round_index + thread_id` 写入候选元数据（审计）；
- 附加证据：真实 workspace 副作用验证属于完成判据（L2 五级门第 5 级），
  L1 不伪造完成声明。

## 4. necessity gate 判定表（L1）

输入：`RoundTrace + session 终态 + 候选去重查询（同 task 签名/同经验名）`。

| 条件 | 动作 |
|---|---|
| 终态非 completed / 轮内 failed / 用户取消 | 拒（不入库；usage 记 reason=terminal_not_success） |
| 无 tool_call（纯对话/只读无动作） | 拒（reason=no_action_evidence） |
| 仅 1 步且动作形态不稳定（命名即随机参数） | 拒（reason=unstable_shape） |
| 同 task 签名已有等价 CANDIDATE/VALIDATED 且无改进 | 拒（reason=repeat_no_improvement） |
| 干净短轨迹（≤ N 步）且动作形态稳定 | 直接 distill（确定性草稿，可无 LLM） |
| 脏轨迹（步骤多/含试探失败后修正） | 剪枝后触发一次 LLM compiler，压缩为最小可重放路径 |
| distill 失败 / 产物过 validate 前两级 | 停留 CANDIDATE 原样或丢弃（usage 记失败原因，不半激活） |

## 5. distiller 契约（接口先定）

```text
trait L1Distiller {
    fn distill(&self, round: RoundTrace) -> Option<ExperienceDraft>;
}
```

- 实现 1：**LLM compiler**——一次小模型调用，输入 = 剪枝后的成功路径
  摘要（不是原始 trace），输出 = 预编译 workflow + trigger 面 + 完成
  判据草案；参考 codex-main `experience_distiller` 的“模型只输出
  JSON、解析/兜底确定性”做法；
- 实现 2：**确定性降级**——干净短轨迹直接映射（单/少步 exec_command →
  workflow），无 LLM 时仍可沉淀；
- 轻量约束（沿用 p2 原则）：蒸馏只在值得的轮次触发一次；失败/审计进
  usage/audit，不进 Experience 本体；绝不把原始 trace 注入任何 prompt。

## 6. validate 前两级（L1 落地；后三级 L2）

1. **Schema valid**：Experience 结构/版本/触发面合法（已有
   `schema_issues`/store 强制校验）；
2. **Tool boundary valid**：每一步可解析到真实工具且参数形状正确
   （干跑不执行）。

## 7. 入库与生命周期（CandidateWriter 权力边界）

- L1 经由 **CandidateWriter** 写入 `CANDIDATE`：writer 类型只暴露
  candidate 写入能力，**不暴露 ACTIVATE/执行资格变更**——权力边界在
  API 形状上成立，而非调用纪律；
- 元数据必带：`source_trace_version=1`、来源 session/round/thread_id、
  distill 模式（llm|deterministic）、necessity 结论；
- 重复任务命中已有候选时走 necessity 的 repeat 分支，不叠库。

## 8. 接线点与测试（L1 验收）

接线：experience-server 会话终态钩子 →（service 侧）读 session JSON →
`trace_reader` → L1 流水线 → store。learning_smoke 降级为只做契约演示，
不复用为产品路径。

测试：

- 单测（deterministic）：necessity 各分支、干净/脏轨迹蒸馏降级、validate
  前两级拒垃圾、入库元数据完整；
- store 回归：CANDIDATE 可读、不 ACTIVATE；
- 真机验收（fork codex + 真实 API）：一次成功任务 → store 出现合格
  CANDIDATE；同任务重跑 → repeat 拒；失败轮 → 不沉淀。

## 9. 协作节奏（配合加速）

- 本设计稿：正式审查一次（当前）；
- L1 实现 + 真机冷/暖对照：完成后整体审查一次（中间只做自查账本 +
  mock 回归，不再每步停）；
- L2–L4：只在语义边界（validate 激活规则、注入策略、Gate 接线）做
  小范围审查；阶段内按批推进并在 README 双主线账本记账。

## 10. 分级口径裁定（2026-09-09 并入）

1. **L1 不自动 ACTIVATE = 架构不变量，已采纳**：L1 对生命周期的最高权限
   固定为 CANDIDATE；任何进入执行路径的 Experience 必须经过 L2 的验证
   与激活机制。代码层以 CandidateWriter 表达（§7）。
2. **L2 为完整“资格”单元，不拆**：五级门后三级 + evidence/usage →
   confidence → ACTIVE + DECAYING/DISABLED 管理同属一条资格形成链；
   L2 命名定为“验证、证据强化与生命周期管理”。confidence 定义为
   证据变量，不作为真值投票（§1.1 口径要点）。
3. **L4 与 M5 = 验收绑定，工程并行**：L3 与 M5 验证不同语义（外层编排
   语义 vs 内层副作用拦截语义），可同时推进；L4 最终验收在 codex-main
   M5 完成后执行。

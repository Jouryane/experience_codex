# Step / Action Gate：Experience 的同步执行接管权

> 2026-09-06。本文回答：复杂任务中，Experience 何时介入、介入多深、
> 成本多少。

## 1. 介入点：不是“下一步决策前”，是“下一步副作用前”

典型 Agent 时序：

```text
LLM → 计划/决定 → Tool Call → 执行 → 结果 → LLM → 决定 → Tool Call → …
```

若 Experience 等 Tool Call **执行完**才发现“这步有经验”，LLM 资源已经
消耗——晚了。正确介入点：

```text
LLM
 ↓
Action Proposal（工具调用已生成、参数已确定）
 ↓
【Action Gate】← Experience 在这里获得一次同步决策窗口
 ├─ HIT  → Experience 执行（可接管单个 Action，或一整条状态转移链）
 └─ MISS → 正常 Tool 执行
 ↓
Result → Agent 继续生命周期（LLM 观察新环境）
```

这是 Agent 本身的天然时序（proposal→副作用之间本来就存在的真空），
不是伪平行时滞。Experience 不负责“暂停 Agent”，只负责在 Action 已确定、
副作用尚未发生时行使一次**执行接管权**。

## 2. 为什么不能是“每一步问 Experience”

错误模型（100 次 Action 查 100 次、命中 3 次，成本反而更高）：

```text
Agent → 每步问 Experience → Agent → 每步再问 Experience → …
```

约束：

- Gate 成本必须接近**一次本地函数判断**（结构化匹配，不做每步语义/全文
  检索）；
- 未命中时的开销应趋近于零，绝不能高于“直接执行”本身；
- Gate 命中后不是替代“一个动作”这么简单，它可以替代**一条状态转移链**
  （见下），把多次 LLM 往返折叠成一次 Experience 执行。

## 3. Hit 的语义：接管动作或状态转移链

普通 Agent 路径：

```text
Action → 发现环境问题 → LLM 分析 → Action(改配置) → Result → LLM 继续
```

Experience 路径：

```text
Action(proposal)
 ↓ Experience 命中
 ↓ 恢复已知环境状态 + 执行已知处理
 ↓ Result
 ↓ Agent 继续
```

效果：`LLM→Action→Result→LLM→Action→Result→LLM` 折叠为
`LLM→Action→Experience→Result→LLM`。

边界（重要）：

- Experience **可以偷偷把世界往前推进**（真实 Environment 改变）；
- Experience **不能偷偷替 Agent 思考**（不注入决策、不控制规划）——LLM
  仍负责验证与下一步；
- Experience Hit 后必须返回可观察 Result，让 LLM 继续自己的生命周期。

## 4. 与 Executor / Runtime 的关系

```text
Task → Experience Runtime（外层：委派决策）
         ↓
       Agent（Executor，例：Codex）
         ↓
       LLM 产生 Action Proposal
         ↓
       Action Gate（嵌入运行时/适配器的工具边界）
         ├─ MISS → 正常 Tool 执行
         └─ HIT  → Experience 执行（同步接管）→ Result
```

- 外层 Runtime：决定“这个任务/子任务要不要委派 Agent”；
- 内层 Action Gate：决定“Agent 已确定要做的这个动作，我是否已知怎么做
  得更好/更快”；
- Gate 依赖 Adapter 提供“proposal 可见性”。Codex 原生具备该边界
  （FunctionCall 已输出、尚未 dispatch 到 ToolCallRuntime 之间），是首个
  落地点；其它 Agent 若无此边界，则只能退回到外层任务级 Gate（能力降级，
  但架构不变）。

## 5. 实现要点（第一版）

- Gate 输入：`(tool, args, cwd, state 快照)`——结构化，不全文语义；
- Gate 存储：只登记“可接管状态转移”的经验（gate 快表），与完整经验库
  分离或加索引，保证判断是本地哈希/查表级；
- Hit 输出：合成 Result（等价于工具输出），走正常结果通道回给 LLM；
- 防抖/防死循环：每 turn 记录 Gate 命中数上限；HIT 后仍交回 LLM 验证，
  不静默吞掉控制权；
- 验收指标：100 次 Action 场景下，Gate 总开销应明显低于 3 次命中节省的
  LLM 往返，未命中开销接近 0。

## 6. 同步 Gate 是唯一形态（外部监听不是 Gate）

```text
✗ 错误（异步外部监控）：
   Codex → 事件流出 → Experience App 监听 → 判断 → 再回来
   这适合观察/回流，不能用于“副作用前接管”。

✓ 正确（执行路径上的同步点）：
   Codex 内部：LLM → Action Proposal → Experience Gate（同步）
     ├─ MISS → Tool Executor
     └─ HIT  → Experience 直接兑现 → Environment
```

红线：Gate 必须位于执行路径内、工具 dispatch 之前，是同步调用；任何
“Experience App 订阅 Codex 事件再回调”的实现都不叫 Action Gate。因此
Executor 契约必须显式声明是否支持同步 Gate：

- 支持（Level 2）：executor 的 dispatch 前有同步钩子（我们构建的 Codex
  在 `build_tool_call` 前预留该点）；
- 不支持（Level 1）：只能做任务级委派 + 观察回流，动作级接管不可用——
  这是能力降级，不是架构变体。

## 7. Experience 不是外挂大脑，而是并列行为路径

语义边界（必须分开）：

- Memory：告诉 LLM 过去发生过什么；
- Skill：告诉 LLM 怎么做；
- **Experience：直接把已经学会的行为兑现到现实世界（State → State'）**。

```text
Task → Codex ──┬── LLM → Reason → Action（探索未知）
               └── Experience → Known Transition（兑现已知）
                              ↓
                          Environment
                              ↓
                            State'
```

LLM 负责探索未知；Experience 负责兑现已知。

匹配模型（这也是 State↔Experience 为什么关键）：

```text
(State, Intent/Action)
         ↓
   Known Transition（经验已掌握的局部状态转移）
         ↓
        State'
```

不是简单 `State → Experience`，而是“当前状态 + 意图/动作”共同决定是否
命中一个已知状态转移。Gate 接管的也从来不是“大步骤规划权”，而是
“这个动作/目标恰好落入我的已知转移范围，因此我一次兑现这个局部过程”。

## 8. Gate 执行语义（四档能力）

“兑现已知转移”至少包含四种不同的执行能力：

| 档 | 能力 | 语义 |
|---|---|---|
| A | 替换一个 Tool Call | 用经验动作替换本 proposal 动作，返回等价 Result |
| B | 替换一个 Tool Call + 验证 | A + 执行后验证（本动作后置谓词为 true，附证据） |
| C | 替换连续多个 Tool Call | 本 proposal 是预编译转移链入口，链内动作全部在经验内、无现场决策，一次执行到底并返回等价 Result |
| D | 替换一整段探索转移 | Agent 原需多轮 LLM 探索才能完成的 S→S'，由经验整段兑现（环境恢复/已知处理/后置验证） |

档位由“经验的预编译范围”决定；Gate 只执行经验内预编译内容，永不现场规划。

### 8.1 D 档（替换整段探索转移）门槛

D 是最有价值也最危险的一档。激活门槛：

```text
高置信 + 强 State 条件 + 完整 postcondition
+ 可验证 + 有成功历史 + 副作用边界明确
```

不满足则**主动降级**：

```text
D → C → B → A → MISS
```

禁止“D → 赌一把”：一个经验若只是在过去特定环境“看起来会”，不得进入 D。

## 8.2 接管语义 = 控制权事务（control-flow transaction）

对 C/D 的链式转移（A→B→C→D），先定语义，再回答失败四问。

### 同步边界是 Level 2 的常驻契约

Gate HIT 后，execute + verify + return 必须在**同一同步调用**内完成：

```text
decide(action, ctx)
      ↓ HIT
同一同步调用内：Execute → Verify → Return
```

**Gate call 返回 ⇔ 接管已终结**：completed / partial / failed 三态已定，
控制权已归还 Agent，Agent 不需要、也没有第二个信号。这是动作级 Gate
（Level 2）的常驻契约，不是 P1 的临时简化——P1 之所以“看起来像事务”，
只是因为 workflow 短、验证即时；契约本身不会因经验变长而改变。

### 控制权事务 ≠ 世界状态原子事务

这里的“事务”只指**控制权事务（control-flow transaction）**，不是数据库式
**世界状态原子事务（world-state atomicity）**：

- 承诺：接管期间 Experience 独占执行权（Agent 处于工具等待中，天然无并发
  观察/写入）；唯一出口是 `GateHitResult`；不静默续跑、不隐式重试、不把
  partial 冒充 completed；
- 不承诺：所有副作用可整体回滚、世界状态“全成或全无”。

世界状态的一致性靠**诚实报告**保证，而非原子性保证：completed 只有在
postconditions 经 verification 成立后才可声明；partial/failed 是合法出口，
必须如实携带实际 State 与 executed_side_effects。

未来的长任务（100 个文件操作 → 编译 → 测试 → 部署）不改变上述契约：仍是
同一同步边界内的控制权事务，变化的只是时长、执行预算/中断策略，以及“若
经验显式声明 undo，则以补偿步骤形式执行”。补偿是经验声明的动作，不是
运行时的原子性承诺。

同步接管以阻塞 Agent turn 为代价：长任务必须有显式执行预算与工具级中断
出口（沿用 codex 对长 tool call 的超时/中断能力），避免“接管”退化成
“劫持”。异步只存在于**任务级委派**（Level 1，`run(task)` 后另行回流）；
动作级 Gate（Level 2）没有异步变体，二者不得混用（见 §6）。

### 失败四问（第一版语义）

对链式转移失败，回答：

```text
step 失败后：
  1. Experience 是否继续？         → 否，立即停止
  2. 是否 rollback（已成功的 A/B）？ → 仅当经验显式声明 undo 且副作用边界内
                                        可回滚；否则不做隐式回滚
  3. State 如何更新？             → 无论成败写 State'（成功=后置已验证；
                                        失败=记录 partial/failed 实际谓词）
  4. 如何交还 Agent？             → GateHitResult（completion_status +
                                        verification_status）；partial/failed 由
                                        LLM 接管排障，禁止 Experience 自行重试接管
```

失败只归因、不扣错账：缺源/参数不匹配 → misfire；逻辑/语义错误 →
invalid。

## 9. Result 语义（闭合 Result → LLM）

Result 不能是裸 `{success:true}`：

```text
GateHitResult {
  proposed_action
  state_before
  executed
  state_after
  execution_evidence
  completion_status      // completed / partial / failed
  verification_status    // 后置谓词 verified / unverified / failed
}
```

LLM 看到“B 已完成 + 证据 + 后置已验证”，据此观察 S' 并继续/验证。

## 10. Executor Capability（形式化）

```text
Executor Capability
  ├── TaskExecution      // run(task) -> RunReport
  └── ActionInterception // 同步 Gate：proposal 拦截 + result injection
```

仅 TaskExecution = Level 1；同时具备 ActionInterception = Level 2；
声明式能力，禁止行为猜测。

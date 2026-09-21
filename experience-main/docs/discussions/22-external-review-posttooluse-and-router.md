# 22 · 外部评审：PostToolUse 对称性、近替代路径、以及"router"建议

> 2026-09-18。来源：openai/codex 讨论区一位开发者的回复（TonyDzi / Palo Alto AI
> Research Lab，21 个 PreToolUse hook、六机 fleet 的日常运营者）。
> 他读的是 `main@3e581eb`（当天），比我们的基线 `6478a751f`（2026-08-29）新。

## 一、他补上的新事实（我们之前只看了 Pre 一侧）

1. **边界是对称的，而且写在类型里。**
   - `PreToolUseOutcome`：`should_block` / `block_reason` / `additional_contexts` /
     `updated_input`——**没有 result**；
   - `PostToolUseOutcome`：`hook_events` / `should_block` / `additional_contexts` /
     `feedback_message`——**也没有 result**。hook 在请求里拿到 `tool_response`，
     但**不能还回一个不同的**。
2. **存在一条近替代路径**（`core/src/tools/registry.rs:741` 附近，附源码注释
   "A PostToolUse block rejects the result, not the already-completed tool execution"）：
   PostToolUse 阻断时走 `FunctionCallError::RespondToModel(message)`——
   **扩展可以把一段自己的字符串放到原本结果的位置**。
   三个致命caveat：① 原工具**已经跑完**，副作用已发生；② 替代内容以**失败调用**
   的形式到达，不是已验证的成功；③ 它是**字符串，不是结构化结果**。
3. **他的判读**：这个面是**策略/否决层，不是执行层**——
   "hook 决定*要不要*、*用什么输入*，从不决定*发生了什么*"。
   并且他明确说：**两侧都一致，不像一个未完成的分支**（比我原来的
   "有意留白 vs 未探索"更稳的措辞，因为他有 Pre/Post 对称作为证据）。
4. **Claude Code 侧他有一手运营经验**：边界相同（`allow`/`deny`/`ask` + 改输入，
   无结果替代）。他举了当次会话的实例：一个 PreToolUse hook 拦了他的 Bash 调用，
   回来的是 block reason，**不是"我替你做完了，这是验证过的结果"**，
   工作还得模型重做。

## 二、与我们数据的交叉验证（这一条最有意思）

他的 caveat ② —— "替代内容以**失败调用**的形式到达" —— 恰好是我们
[round 19](19-handoff-declare-vs-silent.md) 测到的形状：

| 形状 | 我们的实测 |
|---|---|
| 世界变了但**没有解释**（模型的处境等价于"出现了一个我没要求的异常"） | **18 / 15 次请求** |
| 世界变了且**有解释** | 6 / 6 次请求 |

**他的直觉对了，而且我们那边有数字：约 3 倍。**
一个失败形状的替代，会迫使模型先调查"为什么与预期不符"，再做别的事。

## 三、他的建议：把两半合起来用

> 把已验证的动作放进**你自己的工具**（dynamic tool 或 MCP server），
> 把 `PreToolUse` 留作 **router**：拦住原生调用，并在 `block_reason` 里点名你的工具。
> **代价是一个额外的模型回合**；换来的是 hook 面在任何价位都给不了的结构化成功结果。

### 我的评估：他说对了一半，而那一半正好落在我们 pivot 后的主张上

| 维度 | router 的作用 |
|---|---|
| 解决 pull 的不稳定（F5）？ | **是**——模型决定不了"要不要用"，因为原生路被拦了，它被指向你的工具 |
| 降低模型回合数？ | **不**——他明确说要多一个回合，而我们的基线只有 5–7 次 |
| 让经验**被可靠使用**？ | **是**，而且是确定性的 |
| 让经验**更便宜**？ | **否**（按 [21 §六](21-consolidated-findings.md) 的结论，成本主张会随模型变强而变弱） |

**所以 router 是对的机制，但服务的是"可靠 + 可审计"，不是"省成本"。**
这恰好是我们在 round 21 做的 pivot：价值锚点从"省平均成本"搬到
"把已知部分从模型路径上移除 + 可审计 + 可回滚"。
**router 是那个新锚点下的正确实现。**

## 四、他给的最有价值的一条：失败模式预警

> "a hook that can only veto slowly becomes a hook that vetoes a lot,
> and a model that gets vetoed a lot **starts negotiating with it**
> instead of doing the work."

这是一个**退化的均衡**：否决覆盖面越大 → 模型越倾向于绕开 → 需要更多否决。
**这是本轮外部输入里唯一全新的、我们没有考虑过的风险**，而且它是可测的：
装上 router 之后，观察模型是否开始改变命令写法以躲避拦截。

## 五、需要核实的（不要直接引用）

他给的**行号**（`registry.rs:741` / `:758`）来自**比我们新的 main**。
我们的基线在 2026-08-29，中间有 12+ 个提交。
**引用前必须在新版本上核对这两处**，否则会出现"引用了自己仓库里不存在的行"。

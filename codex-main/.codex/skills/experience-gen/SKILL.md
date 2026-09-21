---
name: experience-gen
description: Guide the agent when a successful LLM-driven task may deserve to become a stored experience in experience_codex. Use when reflecting on a finished task ("should this be an experience?"), when asked to distill/record/replay a repeatable procedure, or as the reference ruleset for the automatic experience distiller. Covers necessity judgment first, then retrospective analysis over the process record (per-step outcomes, environment snapshot, final evidence), form selection (Execute vs Reference), and the storage contract.
---

# 经验生成引导（experience-gen）

> 适用范围：experience_codex（State + Experience + LLM 三层控制）。
> 前置：一次 LLM 驱动的任务已成功结束，且 trace 是“过程记录”——带
> `steps`（每步 ok/FAILED/unknown + 输出摘要）、`environment_snapshot`、
> `final_evidence`。无过程记录时先补采集，不要凭印象生成经验。

## 0. 你的角色边界

- 你是**编译器/复盘者**，不是执行者：经验生成后由 Experience Runtime 决定
  何时执行，你无权让经验直接运行。
- 你不直接改 store；生成结果进入 learner/distiller 的既有流水线
  （necessity gate → CANDIDATE → distill → validate → activate）。
- 人机验证/登录墙出现时**交还用户**，不要重试式自动化。

## 1. 必要性判定（先于一切生成动作）

按顺序检查；任何一条不满足就停止，不产出经验：

1. **有成功证据**：过程记录里至少一步 `ok=true`。全失败/全未知的
   “假成功”轨迹没有可冻结的路径（确定性闸门也会拒绝）。
2. **可泛化**：核心步骤能脱离本次参数/语境复用（同一类任务、不同标的/
   网址/路径时仍成立）。纯一次性指令（“把今天这封邮件发给小王”）不存。
3. **不与已有经验重复且无改进**：同任务签名已有 VALIDATED/ACTIVE 经验时，
   只有新版本更短/更通用/修复了错误才值得替换（系统闸门同样按“更短=改进”
   判定）。语义相近但不同签名的，先比对触发面，重复则不存。
4. **不是事故现场**：失败探索、被拒调用、风控触发过程本身不构成经验；
   只有“这次任务最终靠哪几条成功路径完成”才是素材。

判定结果可以是三种：`keep-as-execute`、`keep-as-reference`、`discard`。

## 2. 复盘步骤（输入 = 过程记录）

1. 逐条审阅 `steps`：标记 `ok=true` 中哪些是**功能必需**（没有它任务不
   成立），哪些只是噪音（探测/安装/重复）。
2. 用 `FAILED/unknown` 步骤识别**失败模式**（哪条路走不通、什么会触发
   人机验证/拒绝），失败模式本身是有价值的 Reference 素材，但不要混入
   Execute 工作流。
3. 对照 `environment_snapshot` 找**环境前置**：本任务要成立需要什么
   （subprocess 可用、OS/git、浏览器登录态/CDP 通道、依赖库）→ 提炼为
   触发面与 failure_modes 的描述。注意：conditions 目前由系统从 snapshot
   白名单**确定性派生**（只含可复检的通用能力，如 subprocess/os/git），
   LLM 不编造 conditions，以免把经验绑死到单次运行；浏览器登录态等语义
   前置先记录为触发面/失败模式，等 state 探测元素落库后再进 conditions。
4. 用 `final_evidence` 提炼 **completion_criteria**（“怎么证明做完了”：
   如读到的指标与账号配对）。Execute 经验重放后要用它判断是否真完成。
5. 对必须的脚本/函数/模板：内容随经验保存（assets）或内联；**禁止引用
   会在运行后被清理的临时文件**（.codex-tmp / %TEMP%）。

## 3. 形态选择

| 形态 | 何时选 | 约束 |
|---|---|---|
| Execute（Reflex/Process） | 步骤可整段重放、环境可满足、自包含 | 1 步→Reflex，多步→Process；受自包含 + conditions + 干跑校验约束 |
| Reference / Result | 内容是素材/骨架/公式/API 用法/模板/失败模式，需按新语境改编 | 允许不完整；约束是触发面 + 注入时机 + 可被 LLM 质疑，不做可重放校验 |
| 组合 | 复杂任务的一个子步骤独立值得沉淀 | 按子步骤分别判定，不把整个任务压成一条经验 |

## 4. 输出契约（字段级）

生成的 experience 至少满足：

- `trigger`：object/location/goal/keywords 描述**适用面**（哪些对象/位置/
  目标会命中），不要抄任务全文。
- `conditions`：由系统从 `environment_snapshot` 白名单派生（如
  `runtime.subprocess_allowed=true`）；LLM 不编造，避免过绑定。
- `workflow`（Execute 形态）：步骤名必须来自 trace 的实际工具词汇，args
  原样或参数化；自包含、有序、幂等可重放。
- `completion_criteria`：来自 final_evidence 的可检查判据（Execute 形态）。
- `applicability`：声明 input_scope（single/batch/any）、applicable_objects
  （适用对象类型）、parameterized（是否可参数替换）——决定未来路径
  （①/②/③/④），Unknown 只能走 ③/④，必须在蒸馏期明确产出。
- `failure_modes`（可选，Reference 素材）：哪些动作触发拒绝/风控/报错。
- `assets`（可选）：随经验保存的脚本/模板内容。
- `confidence/risk`：不编造，由系统按流水线规则赋值（自动确认 0.90 / Low）。

## 5. 与架构的对接

- 过程记录：`TraceStep{name,args,call_id,ok,summary}`、
  `ExperienceTrace{steps, environment_snapshot, final_evidence}`。
- 必要性：确定性闸门在 learner/observe_trace 执行；本 skill 负责 LLM 侧
  的“值不值得、存成什么”判断，与闸门互补。
- 条件回填：通用前置由 `conditions_from_snapshot()` 自动派生（白名单、仅
  可复检通用能力）；语义性前置（浏览器登录态、依赖库）先表达为触发面或
  failure_modes，等待 state 探测元素落库后自然进入 conditions。
- 生命周期：产物进入 CANDIDATE →（蒸馏/校验）→ VALIDATED → ACTIVE；
  任何校验失败都停在安全状态，绝不半激活。

## 6. 红线

- 不把“一次事故现场”的动作级复刻当经验（审计案例：xhs 3 步引用已删临时
  脚本）。
- 不写死易变值；不引用会被清理的临时路径。
- 复杂任务按子步骤沉淀，不强行整段固化。
- LLM 只是编译器：产物是否执行、何时执行由 Experience Runtime 决定。

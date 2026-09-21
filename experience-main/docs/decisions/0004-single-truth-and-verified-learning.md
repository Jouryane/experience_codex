# 0004 · 单一真相与"经验从何处来"

> 2026-09-21。承接 [0002](0002-canonical-runtime-two-seams.md) 与
> [0003](0003-experience-template-binding.md)。本文件只写当前采用的决策。

## 一、单一真相：一个解析器，一个文件，一个写入者

**问题。** 执行面读 `EXPERIENCE_GATE_STORE`，管理面（`codex experience`、
app-server、UI）固定读 `<codex_home>/experience/store.json`。两个答案意味着
两个 Store，意味着"操作员 pin 的那个"不一定是"agent 执行的那个"。

**决策。**

1. 只保留一个解析器：`codex-core::experience_paths::resolve_store_path`。
   优先级是显式 env 覆盖、其次 `<codex_home>/experience/store.json`。
   env 从"启用前提"降级为"覆盖项"——没有 env 时 Gate 也读默认路径。
2. 写入者由**文件自身的格式**决定，不由配置决定：
   - `schema_version` 信封 → canonical 写入者（`experience-core`）；
   - 旧信封 → legacy 写入者。
   legacy 写入者在 canonical 文件上被禁止落盘，这条路径原本可以覆盖
   canonical Store（数据丢失），现在不可能。
3. 管理面是 canonical Store 的**投影 + 操作者**：`list/detail/pin/unpin/
   activate/revalidate/disable/delete/update_meta/update_control/export/import`
   直接作用于同一个文件。legacy 独有的两个操作（`edit_as_draft`、
   `adopt_draft`）在 canonical Store 上明确报错，而不是悄悄降级。
4. `codex experience doctor` 把这件事变成可观测的：解析路径、格式、条数，
   以及 override 与 home 同时存在时的漂移报告。

**为什么不是"两边各写各的、再同步"。** 同步会引入第二个真相和第二个控制器。
宁可让一个操作报错，也不能让两个 Store 各自"看起来正常"。

## 二、经验从真实成功轨迹中来，且必须自证

**问题。** 之前的经验体是手写 fixture。手写体可以证明执行面能跑，不能证明
系统会学习。

**决策。** 学习链路固定为：

```text
真实 LLM 执行（记录 tool call）
  → 生产者名确定性翻译（shell/apply_patch → canonical 能力）
  → 双轨迹归纳（差异 → 捕获规则）
  → Candidate（永不自动 ACTIVE）
  → 人工/资格链激活
  → 下次同类任务由 Gate 接管
```

四条硬约束：

1. **只有成功的执行才是观察**。`succeeded` 由外部证据决定，不由代码假设。
2. **参数是"被验证过的捕获规则"，不是猜测**。一个候选参数只有在
   `prefix/suffix` 规则能把两个观察值**同时**还原出来时才成立，而且用的是
   Gate 绑定时同一个 `capture_parameter`。还原不出来就是不成立，整个归纳失败。
3. **翻译表 + 显式拒绝**。`shell` 的命令形状只有有限几种可翻译（`git`、
   `mv`、`mkdir`、`echo > file`、`apply_patch` 的 Add File…）；管道、命令替换、
   通配、`cd`、向活会话喂输入一律拒绝。拒绝是**整体拒绝**：缺一步的 workflow
   绝不能落盘，否则它会接管一个它做不完的任务。
4. **没有可观测后置条件的 body 不是模板**。归纳会同时生成 `verification`
   探针：只有断言而没有办法观测，等于永远无法证明完成。

## 三、回放是"重放已验证的转移"，不是"重放命令"

**决策。** 进程步骤（`exec` / `exec_command`）默认要求退出码 0；观测时成功的
命令在回放时失败，说明前提已经改变，接管必须失败并把控制权交回模型，而不是
继续往下跑、再拿无关的后置条件声称完成。需要非零期望的 body 用
`"expect_exit": n` 显式声明。

## 四、模板绑定必须留痕

**决策。** `TemplateHit` 携带 `bindings` 与稳定指纹，落进 usage 的
`template_audit`。任一被模板接管的调用都可回溯到"哪个模板、绑定了哪些值"。
没有任何一个字段来自模型。

## 五、世界状态触发是一个座位，不是一个控制器

**决策。** `EXPERIENCE_STATE_GATE` 开启后，Turn 开始时会询问：是否存在
trigger tool 为 `state`、且前置条件此刻成立的 ACTIVE 经验。它：

- 只由宿主循环调用，不自己找活干；
- 单 Turn 最多 2 次接管，`Satisfied`（已经做完）立即停止，不循环；
- 失败立刻把控制权交回；
- 与其它命中走同一条记录/审计路径。

这是"记忆"目标的正位：过去在不被想起时也能改变当前路径，且不要求模型先提案。

## 六、仍然已知、尚未关闭的缺口

| 缺口 | 现状 | 为什么先记下来 |
|---|---|---|
| 进程步骤的**网络出口**未中介 | `network` 策略存在于 Store，但 exec 子进程不经过它 | 执行面能发起网络请求；归因依赖 exec allowlist |
| 双轨迹归纳只在"结构完全一致"时成立 | 步骤数/动作名/参数形状必须相同 | 真实轨迹有轮询噪声（已按"空输入轮询不是步骤"处理），但更多形状差异会直接拒绝 |
| legacy 的 `edit_as_draft`/`adopt_draft` 在 canonical 上不可用 | 明确报错 | 要把编辑流搬到 canonical 的 body-replacement 语义上 |
| 状态触发的 seat 是 Turn 级 | 只有 turn 开始这一个触发点 | 事件驱动（文件变化、外部信号）需要新的宿主接口 |

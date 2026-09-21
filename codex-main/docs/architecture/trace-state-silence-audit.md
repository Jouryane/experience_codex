# State / Runtime 沉默审计：经验是否记载了“上一个任务是怎么完成的”

> 日期：2026-09-04
> 方法：反向验证——以小红书冷启动任务（`xhs-cold`，产物
> `cand-51ba94d08be86c16`，Active/0.90/3 步）为样本，对照“理想经验应记载”
> 的信息维度，逐项检查 state update 与 experience runtime 是否捕获、在哪一
> 环丢失。

## 1. 为什么做这次审计

简单任务（E1 移动 PNG）已验证：可复用单元 = 动作序列，经验=触发词+几步
exec_command 即可有效。小红书这类任务暴露：复杂任务的可复用单元不是动作
序列，而是“环境依赖 + 关键指令/素材 + 成功操作 + 流程顺序 + 失败判据 +
完成判据”。用户提出核心怀疑：**state 本质解决环境问题、runtime 解决内容
问题，但它们都没有真正指向“上一个任务是怎么完成的”，而只是在尝试。**

本审计验证该怀疑是否成立、沉默发生在哪里。

## 2. 信息维度核对表（反向验证框架）

| 理想应记载的维度 | state（A1/元素注册表） | trace/learner | 经验 store（schema） | 结论 |
|---|---|---|---|---|
| 环境依赖（完成本任务需要什么环境/登录态/依赖/通道） | 仅 cwd/sandbox/os/git/subprocess_allowed；无浏览器/登录/CDP/依赖探测 | 不采集环境快照 | 无字段（conditions 字段存在但从不回填） | **缺失** |
| 关键指令/素材（真正解决问题的脚本、命令、函数） | 不采集 | 只记动作名+args；脚本文件内容未入库 | workflow steps 可引用路径，无 assets | **缺失（且引用已删路径）** |
| 成功的操作（哪几条是最终成功路径） | last_action/last_result 仅保留最后一步 | 无每步成败；剪枝靠 markers 猜测 | 蒸馏后为“动作序列”，无成败标注 | **半缺失** |
| 流程与顺序（先做什么后做什么） | executed_steps 只存名称，turn 内 | 动作有序，但含失败/探测噪声 | workflow 有序，但可能含坏引用/探测片段 | **部分（可信度低）** |
| 失败模式与原因（哪些路走不通、为什么） | 不采集 | 失败调用与成功调用同表，无结果字段 | 无字段 | **缺失** |
| 完成判据（怎么知道任务真的完成了） | task.status 是经验执行器自己设的 | 整段 trace 标 Success | 无字段 | **缺失** |
| 决策理由（为什么选这条路） | 不采集 | 不采集 | reference_text 只有动作 | **缺失** |

## 3. 沉默点定位（代码级）

### 3.1 state update 的沉默

`session/experience_state_builder.rs`：
- `build_experience_state`：`current_goal`/`intent` = 用户输入全文；
  `environment` = env_id/cwd/sandbox；`detections` = os + git_repo；
  `tools` = capability roots 的 id；`network.policy`（reachability 恒 None）；
  `elements` = 仅 `runtime.subprocess_allowed`。
- `update_experience_state`：每次采样前只做 `refresh_state_elements`，保留
  task/executed_steps/last_action/last_result 作 continuity。

沉默点：
1. **只投影“此刻环境”，不投影“本任务完成需要什么环境”**——没有
   “任务→所需环境条件”的关联，也没有把成功任务的最终环境状态回填成经验
   的 conditions（`Experience.conditions` 全流程从未被写入）。
2. **探测面极窄**：无浏览器 profile/登录态/CDP、无依赖目录/进程、无网络
   可达性；`environment_key` 实为 environment id，非注释所说“os+cwd+工具集”。
3. state 与 learner 之间无通道：`observe_trace` 只拿到 trace，state 里的
   process_log / executed_steps / deployed / elements 从未进入经验生成。

### 3.2 experience runtime / learner 的沉默

`experience_learner.rs`：
- `TraceAction { name, args }`、`ExperienceTrace { task, actions, outcome,
  llm_used }`：**没有每步成败、没有步骤输出摘要、没有环境快照**。
- `build_draft` 把 task 分词成 trigger keywords、把 actions 直接变成
  workflow steps——经验 = “当时动作表的子集”。

`experience_runtime.rs`：
- `observe_trace` 输入即 trace；提交后 `commit_session` 只把 ExperienceState
  存为下轮基线，learner/蒸馏不消费它。
- `session/turn.rs` 调用点：成功 LLM turn 结束后
  `into_trace(task_text, Success, true)` —— task_text 只是用户输入原文，
  不带 agent 对任务的理解、不带中间判断、不带最终验证。

`session/experience_distiller.rs`：
- `build_distill_prompt` 只给 LLM：任务文本 + 动作列表（name+args）。不给
  失败步骤、不给每步结果、不给环境状态、不给完成判据。
- 蒸馏器无从判断“哪几步是本质、哪几步是一次性事故”，只能靠模型猜测。

### 3.3 store schema 的沉默

`experience.rs`：Experience = id/name/kind/trigger/conditions/workflow/
confidence/risk/status/version。`conditions` 存在但从不回填；
**无 assets（素材容器）、无 source_process（过程记载）、无 failure_modes、
无 completion_criteria、无 environment_requirements**。

## 4. 小红书样本对照（cand-51ba94d08be86c16）

实际 store 中的经验：
```text
[1] exec_command  mkdir %TEMP%\xhs-automation
[2] exec_command  Copy-Item .codex-tmp\read-xhs.mjs → %TEMP% 并运行   ← 源文件已被清理
[3] exec_command  node -e CDP 9222 查找 creator.xiaohongshu.com 页并检查“曝光数”文本存在
```

应该记载而实际没有的：
- 环境依赖：已登录 Edge/目标账号、CDP 9222 通道、playwright-core；
- 关键素材：真正读出数值的 `read-xhs.mjs`/`retry-xhs.mjs` 逻辑
  （TreeWalker 找“曝光数/观看数”标签→取父级文本配对）；
- 成功判据：页面含账号“晚时笙歌🎶”，曝光 1,163 / 观看 176；
- 失败模式：9222 首启 refused、taskkill 已登录进程触发小红书人机验证；
- 流程顺序：附着已开浏览器 → 连接 CDP → 定位页面 → 取值 → 报告；
  store 中的顺序是“建目录→复制坏引用→探测”，不是该顺序。

结论：**该经验是对“一次事故现场”的动作级复刻，不是对“这类任务怎么完成”
的记载。**

## 5. 审计结论

用户的怀疑成立：

1. **state update 的沉默**：state 回答“此刻环境是什么”，从不回答“完成这类
   任务需要什么环境、上次是靠什么环境完成的”。环境探测面过窄，且成功任务
   的环境状态没有回填为经验条件。
2. **experience runtime 的沉默**：经验生成的唯一输入是动作序列；每步成败、
   失败原因、过程日志、最终验证判据、素材内容全部在 observe_trace 边界外
   被丢弃。store schema 没有承载“上一个任务是怎么完成的”的字段。
3. **“上一个任务怎么完成的”目前只存在于**：LLM 会话文本、turn 内
   process_log、日志文件——三者都不进入经验，因此复杂任务经验必然“存了也
   没用”。

## 6. 候选修复方向（未实施，仅记录）

1. **扩展 trace 为“过程记录”**：`ExperienceTrace` 增加可选
   `steps: [{action, args, ok, summary}]`、`environment_snapshot`、
   `final_evidence`（完成判据），让 learner/distiller 的输入是“过程”而不是
   “动作表”。
2. **state 参与学习**：`observe_trace` 增加入参 state；成功任务的
   process_log / deployed / elements 作为经验生成上下文，环境的“所需条件”
   提炼为 conditions。
3. **store 增加复杂任务形态字段**：assets（素材）、source_process（过程）、
   failure_modes、completion_criteria（见 experience-runtime-design.md §13
   Reference 层），Execute 语义仍走自包含校验。
4. **经验生成参考 skill**（用户建议，候选）：一份给 LLM 的“如何从 trace 提炼
   经验”的生成规范，保证提炼维度一致，而不是每次靠模型临场发挥。

## 7. 修复进展（2026-09-04，实施中）

**候选 1（过程记录）已实现主体**（提交 `57b8c5c25`）：
- `TraceRecorder` 新增 `record_call(name,args,call_id)` 与
  `record_outcome(call_id, ok, summary)`：工具结果经
  `drain_in_flight`（`FunctionCallOutput.success`）按 call_id 回填每步成败
  与输出摘要（截断 800 字符）；
- `ExperienceTrace` 新增 `steps: Vec<TraceStep>`（name/args/call_id/ok/
  summary）、`environment_snapshot`、`final_evidence`；采集点（run_turn 收尾）
  用 `ExperienceState::snapshot_for_learning()` + agent 最终回复填充；
- 蒸馏提示词改为基于**过程记录**（每步标 ok/FAILED/unknown + 输出 + 环境 +
  最终证据），并要求不得引用会被清理的临时文件；
- 单测：call_id 关联/成败回填/过程 trace 构造（86/86 通过）。

尚未完成：候选 2 的环境知识“转正为 Reference/知识条目”通道、候选 4 的
经验生成参考 skill 深度整合、生成期必要性闸门（后续版本可扩展过滤面）。

**候选 2（state ↔ 经验闭环）第一刀已实现**（提交 `fa7239b35`）：
- `conditions_from_snapshot()`：从学习快照提炼通用、可复用的前置条件
  （`runtime.subprocess_allowed`、os/git 存在性），易变值（cwd/URL）刻意
  排除，避免经验被单次运行绑死；
- `observe_trace` 对每个入库草稿合并派生条件——**即使工作流被蒸馏拒绝、
  留下的 CANDIDATE 也保留环境知识**（用户关切：内容被毙但 state 有意义
  时不再全部丢失）；
- `decide()` 命中前本来就会用 state 校验 conditions——state→经验→state
  由此形成双向闭环；
- 残余：CANDIDATE 不参与匹配，环境知识仍无“转正为 Reference/知识条目”
  的独立通道（候选 3 的 source_process/failure_modes 部分）。

**问题 2 查证（语义错误 vs 干跑校验）已证实**（`fa7239b35` 同提交）：
- 新增固定测试 `replayable_check_cannot_detect_semantic_errors`：任务要求读
  小红书曝光/观看，蒸馏 workflow 却去抖音查“播放量”并回填“0/0（猜测）”，
  `validate_replayable` 仍返回 Ok——工具名可解析、参数非空即可放行；
- 结论：干跑校验只能证明“形状可执行”，不能证明“语义正确”；
  completion-criteria 的证据源已在 trace（final_evidence），缺的是把它
  固化进 store 字段并纳入校验/重放判据（候选 3）。

**生成期治理（必要性门槛 + 引导 skill）——架构微调判断（用户提案）**：
- 用户建议：经验生成环节让 agent 走一个 skill，引导复盘、协助生成与存储；
  但先判定“这条经验有没有存储的必要性”。
- 判断：这是架构微调，落点是 **learner 与 store 之间的必要性闸门
  （necessity gate，确定性）+ 生成期引导 skill（供 LLM 复盘/提炼参考）**，
  与候选 4 同源；建议先做确定性闸门（可执行性/重复度/一次性/低价值过滤），
  skill 次之并复用过程记录（steps/environment/final_evidence）作为输入。

**必要性闸门（确定性）已实现**（提交 `c6541171c`）：
- learner 侧：`trace_has_success_evidence()` —— 声称 Success 但每一步都被
  观测为失败/未知的“假成功”轨迹直接拒绝（不再累计成功证据、不生成草稿）；
  无每步细节的旧路径（legacy/测试）保持放行，不破坏旧行为；
- store 侧（`observe_trace`）：同一签名下若新草稿不比已存
  VALIDATED/ACTIVE 经验更短（更蒸馏），则跳过存储——重复运行不再产生无意义
  的版本 bump；更短的蒸馏结果仍会替换；
- 测试：假成功拒绝 / 无改进跳过 / 更短放行（91/91 通过）。

**生成期引导 skill（experience-gen）已落地**（提交 `1f0b6ec51`）：
- 位置：`.codex/skills/experience-gen/SKILL.md`；
- 内容：必要性判定 → 过程记录复盘 → 形态选择（Execute/Reference/组合）→
  存储契约（触发面 / conditions 系统派生 / workflow 自包含 / completion
  criteria / failure_modes / assets / 不写易变值）；
- 接线：自动蒸馏 `llm_distill` 的 prompt 引用并同步该 skill 规则；
- 契约已落库（候选 3，提交 `ebdb746f8`）：Experience 新增 completion_criteria
  （默认取 final_evidence）/ failure_modes / assets 字段；蒸馏解析接受三者；
  reference_text 展示；94/94 测试通过。

本阶段（沉默审计修复链）至此收尾：过程记录 → 条件回填 → 必要性闸门 →
用户放行通道 → experience-gen skill → schema 落库 全部完成，等候下一阶段
指令（测试）。

**长期记忆（2026-09-05，docx 批量测试后固化）**：任务-经验对应关系的路径
判定是**先验决策**，不是试错——完整逻辑见 experience-runtime-design.md §14
（SEM/ISO/COV/TRU 判定表 + 伪代码 + misfire/invalid 归因分离 + 适用元数据）。
禁止再犯：用 kind 标签替代场景判断、把 misfire 当 invalid 扣置信度。

**生成门槛与用户放行通道已确认/补齐**（提交 `e322e61ba`）：
- 门槛（代码事实，非每轮生成）：仅“LLM 采样过 + 有工具调用”的成功 turn
  进入 learner；随后依次拦截假成功（无成功步骤证据）、不可执行（剪枝后
  空）、无改进重复（store 侧）；messy 轨迹还需蒸馏/校验才激活。
- 此前缺口：`user_confirmed` 唯一来源是全局 `EXPERIENCE_AUTO_CONFIRM`，
  没有“用户这次明确要求记录”的通道。
- 补齐：`force_record`（只绕过 no-improvement 闸门，质量闸门仍生效）+
  窄词面意图检测（“记住这个流程/以后都这样做/沉淀为经验/save this as an
  experience…”），turn 层命中即走放行通道。

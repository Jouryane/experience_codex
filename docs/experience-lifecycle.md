# 经验：结果 / 内容 / 调用 / 人工修改 / 自动留存

## 1. 经验是什么（结果）

经验是一个 **已学会的、可兑现到现实世界的状态转移**：

```text
(State, Intent/Action) → Known Transition → State'
```

语义边界：Memory 告诉 LLM 过去发生了什么；Skill 告诉 LLM 怎么做；
Experience 直接把行为兑现（State→State'），不是“给 LLM 的提示”。

## 2. 经验的内容（schema）

```text
id / version
name            # 内容/来源任务（只读溯源）
title           # 短展示名（可重命名）
trigger         # 适用面（object/location/goal/keywords）
conditions      # state 依赖（经验成立所需状态）
workflow        # 动作序列或状态转移链（自包含、可参数化）
completion_criteria  # 完成判据（来自最终证据）
failure_modes   # 已知失败方式（misfire 记录）
assets          # 随经验持久化的素材
applicability   # input_scope / applicable_objects / parameterized
note            # 用户备注
confidence / risk / status / created_at
```

## 3. 调用

- 调用时机 1（外层）：任务/子任务 Decision 判定“能直接做”；
- 调用时机 2（内层）：Action Gate 在 Action Proposal 后、副作用前同步
  判定 `(State, Intent/Action)` 是否命中已知转移；
- 命中执行：恢复环境 → 执行已知处理 → 返回 Result → Agent 继续；
- 每次调用记录 usage：decision 档位、效果（success/misfire/invalid）、
  引用日志（线程/会话/进程/任务）。

## 4. 人工修改

管理操作（Experience UI / 管理接口）：

- 重命名（title）；直接赋值 confidence；编辑 note；
- 修改 state 作用（conditions）与 runtime 作用（applicability）；
- pin（永不遗忘）/ activate / revalidate / disable / delete（可回收）；
- edit-as-draft（复制为 CANDIDATE 草稿）→ adopt（启用草稿并停用旧版）；
- 导入 / 导出 JSON；查看引用日志与统计。

人工修改必须写审计并保持 schema 与 Runtime 一致，确保修改后仍可被
Experience 再次使用。

## 5. 自动留存

```text
Agent RunReport → trace 转换
  → necessity gate（假成功/不可执行/重复无改进 拦截）
  → distill（LLM 编译器，遵循 experience-gen 规范；产出 title/触发面/
    自包含 workflow/完成判据/applicability）
  → validate（结构 + 工具边界干跑）
  → activate（VALIDATED → ACTIVE）
  → 使用统计 / 衰减 / 遗忘（DECAYING→DISABLED，pin 豁免）
```

### 5.1 validate 阶梯（防“格式正确的垃圾经验”）

validate 是递进的五级门：

```text
1. Schema valid
2. Tool boundary valid        # 每步可解析到真实工具（干跑不执行）
3. State prerequisite valid   # 前置谓词声明存在且可验证
4. Execution dry-run valid    # 安全边界内试运行/等价验证
5. Completion criterion observable  # 后置谓词/完成判据可观察
```

任何一级不过都不激活；LLM compiler 产出“看起来像经验”但不可执行/不可
验证的内容，会在 2/3/4/5 被拒绝。

- 沉淀主责在 Experience 应用，不由 Agent 自行生成；
- `EXPERIENCE_ENABLED=0` 时自动留存关闭；
- 数据文件：`store.json`（本体）、`usage.json`（统计/引用/审计）、
  `trash.json`（删除回收）——唯一事实源，Agent 永不私藏。

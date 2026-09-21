# 业务逻辑文档

## 1. 一句话业务

用户在 Experience 应用里对话；Experience 先尝试“自己会做就直接做
（兑现已知状态转移）”，不会做的才外包给用户选定的 Agent（默认 Codex），
并把 Agent 的结果回流为经验，越用越快。

## 2. 主流程

```text
1. 用户任务进入 Experience State（构建环境/目标/上下文）
2. Decision：
   - 命中外层已知转移 → Experience 直接执行 → 完成/部分完成
   - 未命中/需推理 → 委派 Agent（Agent Manager 选 executor）
3. Agent 执行中（LLM 探索未知）：
   - 每个 Action Proposal 经过同步 Action Gate
     - HIT：Experience 兑现已知转移（单动作或状态链），返回 Result
     - MISS：正常 Tool 执行
   - 每次执行改变真实 Environment
4. Agent 完成 → RunReport 返回 Experience
5. 回流：necessity gate → 蒸馏（LLM 编译器遵循 experience-gen）→
   validate → activate → 入库（store/usage）
```

## 3. 经验参与的三档

- 整任务可做：不外包，Experience 全程兑现；
- 部分步骤可做：任务委派 Agent；执行中 Action Gate 逐点/逐链接管；
- 全程无经验：纯委派，回流后由 necessity gate 决定是否值得沉淀。

## 4. 学习回流（自动留存）

```text
RunReport（task/raw/actions 摘要/结果）
  → trace 转换
  → necessity gate（假成功/不可执行/无改进重复 拦截）
  → distill（experience-gen skill 规范：短 title/触发面/自包含/完成判据/
    适用性 metadata）
  → validate（结构 + 工具边界干跑）
  → activate（VALIDATED→ACTIVE）
```

执行失败归因：MissingSource/ParamMismatch → misfire（不扣质量置信度）；
ToolError/Semantic → invalid（扣质量、驱动生命周期）。

## 5. 状态机

NEW → CANDIDATE → VALIDATED → ACTIVE → DECAYING → DISABLED；
用户可显式 pin（永不遗忘）/activate/revalidate/disable。

## 6. 主开关

`EXPERIENCE_ENABLED=0/false/no/off`：关闭后匹配与学习全停，Agent 按原生
路径运行（兼容“接入更强 LLM 时不用经验”）。

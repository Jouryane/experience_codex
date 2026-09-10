# 资产梳理与迁移清单（codex-main → experience-main）

## 原则

- 只迁移**纯逻辑、与 codex 会话/传输解耦**的部分；
- turn.rs / session 接线等耦合内容**不迁移**，作为 codex-main 内嵌参考
  实现保留（受 `EXPERIENCE_ENABLED` 总开关控制，可双轨并存）；
- 迁移不是复制粘贴：先搬运，再按独立 crate 适配，逐模块编译验证。

## A. 直接搬运（已复制，待适配）

| 文件 | 内容 | 适配项 |
|---|---|---|
| experience.rs | Experience schema + validate + reference_text | serde derive 可用；`applicability` 等新字段保留 |
| experience_state.rs | State/StateElement/conditions/snapshot | 无 codex 依赖，主要适配 mod 可见性 |
| experience_matcher.rs | 匹配算法 | 无依赖 |
| experience_learner.rs | trace/learner/necessity/prune/build_draft | `experience_management` 相关仅 runtime 用，先裁剪 |
| experience_lifecycle.rs | 状态机 | 无依赖 |
| experience_confidence.rs | 置信度 | 无依赖 |
| experience_executor.rs | workflow 执行（trait 化） | futures BoxFuture；runner 由 controller 提供 |
| experience_store.rs | store + json | serde_json |
| process_log.rs | 过程日志 | 无依赖 |
| experience_runtime.rs | 总控制器/decide/usage | 需抽掉 `crate::experience_management` 引用（usage 自洽实现） |
| mod.rs | 模块导出 | `pub(crate)` → controller 需 `pub`；经验管理 facade 独立 |

## B. 提炼后迁移（后续）

- `experience-gen` 引导规范（skill 思路 → 控制器蒸馏提示词/配置）；
- necessity gate / misfire-invalid 归因逻辑（在 learner/runtime 中已有，
  随 A 迁移后统一）；
- usage/audit 结构（抽离 experience_management 的 UsageEntry/UsageLog）。

## C. 保留在 codex-main（参考实现，不迁移）

- session/turn.rs 决策接线、experience_action_runner、state builder、
  app-server experience/* 方法、HTML 管理页、launcher——它们证明
  “内嵌路径”可运行，供对照与验收。

## 下一步（M1）

1. experience-core 独立编译（改可见性 + 抽 usage）；
2. agent-runtime: `trait AgentRuntime { async fn run(&self, task, ctx) -> RunReport }`;
3. agent-codex: CodexExecAdapter 包装 `codex exec`，stdout→RunReport；
4. experience-controller：decide→(经验执行|delegate)，用现有经验库
   （复制或链接同一 store 路径）跑通冷/暖对照。

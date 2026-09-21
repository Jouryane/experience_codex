# Stage S：收口与演进（2026-09-09 起）

## 0. 判定

WP1–WP7 已完成并声明收口（双仓提交、测试全绿、真机 PASS）。本阶段处理
**非阻塞剩余**与**一个待裁定语义**；模型 B / 多 Agent / Task Segment
自动拆分 / policy 上限参数化维持冻结。

## 状态（2026-09-09，S1–S4 推进）

- **S1 ✅**（44a22d7）：delegation 文本加“勿重复执行”notice；真机两轮
  redo_writes=0（tool_calls=2 均为只读验证），usage 2 成功、repeat 拒；
- **S2 ✅**（9a05594）：UI 固定/取消固定 + pinned 显示 + usage score/
  successes 可见（HTTP/审计已有，e041161）；
- **S3 ✅**（82ccbdb + 真机 23:27:57）：learning_l1 dirty 路由 + 服务端
  env 门控 `EXPERIENCE_LLM_COMPILER=on`/`EXPERIENCE_LLM_CODEX` codex
  子进程编译 + JSON 解析；真机 dirty 轮（5 次 toolCall）→ LLM compiler
  → CANDIDATE（create_seq_a_d_files，5 步 exec_command），ledger written；
  no-token fixture `fixtures/l3/accept-llm/` + 测试
  `fixture_llm_dirty_round_writes_candidate`；
- **S4 ✅ 文档**：本文件 + docs/rerun-matrix.md。
- **S2 UI 部分**：转交后续 UI 专属模块（HTTP/审计已就绪，9a05594）。

## S1 P2-4 剩余任务语义裁定（唯一需用户裁定项）

问题：execute-first 后委派文本 = 原任务全文 + 已完成步骤/已验证状态；
真机显示 codex 会重跑已知段（Set-Content probe.txt）。

- 选项 A（建议）：维持 v1 fallback 文本，但委派时显式加
  “已完成/已验证段落，勿重复执行；只完成剩余或确认收尾”，并真机验证
  不再出现重跑 toolCall（≤2 次运行）；
- 选项 B：TaskSegment v0——由上游显式提供段结构（非 LLM 切分），L3 按
  段兑现并只委派剩余段；
- 选项 C：维持现状，仅文档标注“已知段可能被重复执行，副作用幂等性由
  经验 workflow 保证”。

## S2 L2 管理面收口

- pin/unpin UI 接入（HTTP + ledger 已就绪：POST
  /api/experiences/{name}/pin|unpin，actor 审计）；
- 生命周期/usage 可视化（GET /api/usage 已有，UI 补齐）。

## S3 L1 脏轨迹 LLM compiler

- 接口已冻结（L1Distiller/necessity 表）；补一次小模型调用编译
  write/read 脏轨迹；失败/审计不进本体；确定性降级不变。

## S4 跨仓一致性 + 复跑矩阵

- 双轨（rich vs domain）usage/confidence 口径对照与文档化；
- schema 升版策略（additive serde-default 不升版已文档化）；
- accept-*.ps1 一键复跑清单与产物归档规范；
- 评估 codex-core 预存在 Windows 集成用例栈溢出（与 Gate 无关）。

## 纪律

每项：脚本/fixture 先提交 → 实现 → cargo test 全绿贴原始计数 → 真机
≤2 → 产物与 fixture 同步 → README 账本更新。

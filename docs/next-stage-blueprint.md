# 下一阶段蓝图：能力判断 × 规模分层 × 树状管理 × UI/状态/编辑 × 场景通道

> 2026-09-10。本文把 Q1/Q2（万级/百万级唤醒）落到架构分层，并给出
> computer-use 判断、树状分布、状态层、用户编辑与场景选择权力的设计；
> 具身智能训练只留通道，不展开。

## 0. Computer Use 插件判断（先定边界）

事实核查：

- computer-use 是宿主级 UI 自动化能力（cua runtime + node 依赖），随
  Codex 桌面插件分发；它不在 fork codex 的 `.codex-exp-home` 插件目录
  中，也不是可嵌入 experience-main Rust 运行时的库；
- 项目现有执行链已能“把 UI 型任务 delegate 给具备该能力的 agent”，
  与“Experience 只接可声明的 executor、模型 B 冻结”的边界一致。

判断：**不把 computer-use 内嵌为 Experience 的默认执行器**，理由：

1. 副作用强度与审批边界（UI 自动化可点击/输入/移动文件，强于
   write_file），默认放开会破坏“先动手”的安全护栏；
2. 依赖宿主/插件环境，违背 fork 自持运行与多 Agent 冻结口径；
3. 内嵌不解决任何当前已证伪的语义问题，只增加一个“第二个执行器”的
   维护面。

采纳形态：**预留 capability 通道，实现冻结**——`capabilities:
["computer_use"]` + adapter 契约（动作词表 / UI StateProbe / verification /
审批策略）。未来如需接入，以可选 adapter 落位（与 MCP adapter 同层），
L3 控制权语义不变。该通道同时就是“具身智能经验训练”需要的唯一接口。

## 1. 规模与使用分层（Q1/Q2 → 架构）

把“万级不均匀 / 百万级”的讨论收敛为五层：

```text
L0 数据分区   scope(域/专项/workspace/agent) 分 store；embedding 仅作
              可选元数据列（用户自选，默认关闭）
L1 索引面     ACTIVE tool 索引 → 任务签名倒排/前缀树 → 可选向量粗召回
              （只负责“别漏”，不拥有决策权）
L2 候选精排   确定性 Select Plan（覆盖→confidence→冲突不自动执行）
L3 使用规则   per-scope 注入/执行预算（policy_off 默认；上限参数化冻结）
L4 治理       usage 冷热/decay/disable/归档（低频不降质，只降驻留）
```

规模分层不改变 Experience 本体 schema 与执行语义：索引层可插拔，缺省
保持现状（数十条级线性+tool 索引已证正确）。

## 2. Experience 树状分布管理

树是“组织/导航视图”，不是新的匹配语义：

```text
Root（域/项目）
└── Scene（场景/专项，scope）
    └── Family（家族/子任务模板，共性归纳）
        └── Experience 叶子（具体实例：结果/状态/工作流/参考）
```

- 节点元数据：status 聚合、usage 热度、scope、家族参数槽；
- 匹配仍按“叶子 ACTIVE + scope 过滤”执行；树只负责呈现与归属；
- 落地方案：store envelope 增加 additive `scope`（serde default）；
  树可由 `scope/家族前缀` 约定推导，或显式 metadata；提供
  `GET /api/experience-tree`（按 scope 展开，叶子含 usage/status）。

## 3. UI 层（转交 UI 专属模块的规格）

- Tree 导航：域/专项/家族/经验树 + 搜索过滤（scope/status/usage）；
- 详情：本体只读 + status/pinned/usage 面板；
- 编辑：进入 draft → 编辑 → revalidate（写操作全走 actor 审计）；
- 场景选择：经验“挂在哪些 scope”多选；使用入口选 scope；
- 审计：learning-l1 查询面（分页/过滤），pin/unpin/编辑/状态变更可查。

## 4. 经验状态层管理

- 状态面：Draft/Candidate/Validated/Active/Decaying/Disabled + pinned；
  转移单一入口（domain transition table），全部审计；
- 证据面：usage.json 的 ConfidenceRecord（evidence/activity/score）；
- 生命周期：usage 负证据 → Decay（pin 豁免）；低频只降驻留不降质；
- 视图：状态按 scope 过滤后的“当前可用执行面”，与全局管理面分离。

## 5. 编辑权利与场景选择权力

- 编辑权利：用户可创建/草稿化/编辑/替换/删除已有经验；每次写操作带
  actor + ledger（现有 pin/status/删除已有审计骨架，edit/replace 需补
  server API，语义参考 codex-main rich 的 editAsDraft/adoptDraft）；
- 场景选择权力：“在专项下使用 experience”= 会话/任务入口携带 scope，
  唤醒只查该 scope 的 ACTIVE 面；经验可被多个 scope 引用，可配置默认
  scope；
- **具身智能经验训练：只留通道，不展开**——通道契约 = scope +
  capability allowlist + 经验 import/export + State 快照；具体训练系统
  通过该通道喂入/取用经验，本项目不实现训练闭环。

## 6. 落地顺序（Stage C）

- C0 本文档（判断与总设计）✅；
- C1 scope 元数据 + 树/过滤 API（脚本+fixture 先行）；
- C2 编辑权利 API（draft/edit/replace/delete，actor 审计）；
- C3 UI 专属模块（Tree/详情/编辑/场景选择/审计）；
- C4 computer-use adapter 契约 + 具身通道 stub（仅接口，不实现执行）；
- C5 规模索引设计稿 + 基准（桶内不 miss、确定性兜底）。

纪律不变：脚本/fixture 先提交 → 实现 → cargo test 全绿 → 真机 ≤2 →
产物与 fixture 同步 → README 账本更新。

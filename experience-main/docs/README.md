# Experience 文档地图

> 目的：让每一份文档只有一个角色。当前结论在 `decisions/` 和少量 canonical
> 文档里；`discussions/` 是历史证据与思考过程，不用来承载“现在是什么”。

## 一、按问题找文档

| 你要回答的问题 | 先看 |
|---|---|
| 我们为什么做 Experience，主目标是什么 | [decisions/0001](decisions/0001-primary-goals-and-drift-guard.md) · [why-3-5-layer](why-3-5-layer.md) |
| 当前执行面采用什么架构 | [decisions/0002](decisions/0002-canonical-runtime-two-seams.md) · codex-main `docs/architecture/m6-canonical-runtime.md` |
| 经验如何从个案走向参数化 | [decisions/0003](decisions/0003-experience-template-binding.md) |
| 经验从哪来、Store 以谁为准 | [decisions/0004](decisions/0004-single-truth-and-verified-learning.md) · codex-main `docs/architecture/m10-v2-path.md` |
| 当前架构的硬边界是什么 | [core-principle](core-principle.md) · [engineering-architecture](engineering-architecture.md) · [step-gate](step-gate.md) |
| 哪些承诺已经可执行、可验收 | [learning-reuse-contract](learning-reuse-contract.md) · [productization-security-execution-plan](productization-security-execution-plan.md) · [rerun-matrix](rerun-matrix.md) |
| 产品怎么用、怎么对外讲 | [product-overview](product-overview.md) · [usage-guide](usage-guide.md) · [experience-app](experience-app.md) |
| 当前做到哪一步、下一步做什么 | 根目录 [README](../README.md) · [delivery-status](delivery-status.md) · [discussions/25](discussions/25-first-principles-review-and-next-research.md) |
| 某个判断是怎么被证伪或修正的 | [discussions/README](discussions/README.md) |

## 二、文档分层（唯一角色）

| 层 | 位置 | 回答什么 | 纪律 |
|---|---|---|---|
| **决策** | `decisions/` | 当前采用的原则、目标、取舍 | 短、可替换、带日期；不写过程 |
| **契约 / 架构** | `docs/*.md` | 可执行约束、接口、验收口径 | 改实现必须同步改契约 |
| **产品 / 使用** | `product-overview.md` 等 | 对外价值和实际用法 | 不承载内部实验过程 |
| **计划 / 状态** | `next-stage-*` / `delivery-status` | 当前工作安排和完成度 | 容易被取代；过期要归档 |
| **讨论 / 证据** | `discussions/` | 为什么这样判断、实验如何失败或成立 | 按时间 append，不回头重写历史 |
| **归档** | `archive/`（后续建立） | 已被替代但仍需审计的材料 | 不从主阅读路径引用 |

## 三、为什么暂时不把文件全部搬进新目录

当前问题主要是**入口不清**，不是文件位置不对：

- `discussions/01–25` 之间已有大量相对链接，表示的是判断如何演化；
- 大规模移动会同时制造链接修复、历史语义丢失和复跑脚本引用断裂；
- 物理搬家应该在 canonical 集稳定后做，而不是先做。

所以先执行低成本的三步：

1. 新增 `docs/README.md` 作为唯一入口；
2. 新增 `docs/decisions/` 承载当前决策，替代“从 25 篇讨论里拼答案”；
3. `discussions/` 保持按时间不可变，只在其 `README` 增加状态和入口。

当同一份稳定文档连续两三周没有因讨论更新时，再把它归入
`architecture/`、`contracts/`、`product/` 或 `delivery/`，并一次性更新引用。

## 四、第二阶段目标结构（已定，暂不搬）

| 目标目录 | 收纳内容 | 当前文件示例 |
|---|---|---|
| `charter/` | 第一原则、长期定位、主目标 | `core-principle.md`、`why-3-5-layer.md`、`business-logic.md` |
| `architecture/` | 机制、状态、Gate、兼容边界 | `engineering-architecture.md`、`state-model.md`、`step-gate.md`、`compatibility.md`、`session-channel.md`、`experience-lifecycle.md` |
| `contracts/` | 可执行承诺、验收口径、外部适配接口 | `learning-reuse-contract.md`、`computer-use-adapter-contract.md`、`scale-index-design.md`、`productization-security-execution-plan.md` |
| `product/` | 对外价值、使用方式、UI | `product-overview.md`、`usage-guide.md`、`experience-app.md`、`preset-scenes.md`、`ui-*.md`、`frontend-redesign-plan.html` |
| `delivery/` | 设计、计划、状态、复跑 | `l1-*.md`、`l2-*.md`、`l3-*.md`、`p1-*.md`、`p2-*.md`、`next-stage-*.md`、`stage-*.md`、`delivery-status.md`、`rerun-matrix.md` |
| `archive/` | 已被替代但需审计的过程材料 | 先收 `run-notes.md` 等过程账本；再由 delivery 文档的替代关系决定 |
| `discussions/` | 不可变讨论与实验账本 | 保持编号和相对链接，不参与搬迁 |

搬迁规则：先更新 `docs/README.md` 的 canonical 链接，再移动文件，最后用链接
检查全量验证；一次只搬一个目标目录。

## 五、阅读纪律

- 讨论文档不是当前状态；看到结论先找 `decisions/` 或契约。
- 实验结论必须保留反例、撤回和 `n`，不要只摘成功数字。
- 计划文档过期后不再补写历史，改为移动到 `archive/` 并在入口留一行替代链接。

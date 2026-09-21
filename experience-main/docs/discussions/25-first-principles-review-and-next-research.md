# 25 · 第一性回检：router 之后，四个主目标还差什么

> 2026-09-19。承接 [24](24-router-experiment-results.md) 与外审回复。
> 本文件记录讨论结论和下一阶段研究计划；当前采用的决策已收敛到
> [decisions/0001](../decisions/0001-primary-goals-and-drift-guard.md)。
>
> **执行状态（2026-09-20）：** 计划中的 P0 机制部分已完成并转为
> [decisions/0002](../decisions/0002-canonical-runtime-two-seams.md)；
> M6 四场景验收 PASS。`updated_input` 扩展、frequency 曲线与高 veto
> 压力实验仍是后续研究。

## 一、这次测试带来的真正变化

router 实验确认了两件事，也暴露了一件更重要的事：

1. stock Codex 上可以做到“hook 拦 + dynamic tool 执行”，且 block reason 能
   到达模型（3/3 经验调用、3/3 正确）；
2. 外审“多一个模型回合”的预测是**局部事实**：拦截点确实多一次交接，但工具
   可能把后续多步原生路径压缩，所以净回合数取决于前缀长度和可批处理性；
3. router 仍然要求模型先提出动作，所以它没有把“已知前缀”从模型决策路径里
   移除。它改善的是可靠性，不是记忆或成本上限。

## 二、四条主目标的第一性重述

- **记忆**：不是“把更多历史放进上下文”，而是“过去在不被想起时也能改变
  当前路径”。router 是动作触发；pre / gate 才是状态触发。
- **省 token**：只有移除模型决策往返才成立。pull 只替换执行，判断成本仍在；
  router 降低执行浪费，但不稳定地增加/减少净往返；pre / gate 对覆盖部分
  是构造性的零模型轮次。
- **加快响应**：同理，只有当模型不在关键路径上时才成立。hook 本身的耗时
  不是主项，模型往返才是。
- **熟悉领域效率**：熟悉意味着已知前缀密集，正是最该把决策权移出模型的地方。
  如果仍要求模型先提案，效率上限就被锁住。

## 三、新的、未测的可能性：`updated_input` 同回合改写

PreToolUse 除了 `block`，还有 `updated_input`。一个 hook 可以把 Bash /
`exec_command` 的输入改写为调用 Experience runner 的命令：

```text
模型提议 Bash("New-Item ... ")
  → hook 匹配已知前缀
  → updated_input = Bash("python experience_runner.py ...")
  → 同一个工具调用执行 runner
  → runner 执行 + 验证 + 打印结构化结果
  → 结果在同一模型回合返回
```

它不等于 hook 返回结果：结果仍由工具执行产生。但它可能消除“额外交接回合”，
成为 stock Codex 上最接近 native gate 的近似。

边界同样明确：

- 只适用于可被命令重写的工具；`apply_patch` 等不覆盖；
- 返回是 stdout / exit code，不是 structured `contentItems`；
- 匹配错误会直接劫持模型原意，治理要求高于普通 veto；
- 仍要求模型先提出动作，所以不解决世界状态触发的记忆目标。

## 四、下一阶段研究计划

| 优先级 | 问题 | 实验 | 主指标 | 通过判据 |
|---|---|---|---|---|
| P0 | 同回合改写是否可行、是否真省 | Route C：`updated_input` → Experience runner | 净模型请求、wall、正确性、误改写率 | 与 router 相比净请求下降且正确不降 |
| P0 | 中途浮现的已知前缀有多常见 | 任务文本不直接给状态，先探索再触发 | `P` 的频率、触发位置 | 能区分 pre 覆盖不到 / router 才接管 |
| P1 | 可移除的模型决策轮次有多长 | 非批处理前缀曲线（1/5/20 个决策轮） | `N`、净 token、p95 wall | 找到 router / rewrite / gate 的收益拐点 |
| P1 | veto 压力下的退化均衡 | 高覆盖 hook + 多次误匹配 | 改写法次数、重试、谈判迹象 | 确认何时出现 negotiation |
| P2 | native gate 的实际增量 | M5 参考实现 vs stock adapter | 零模型轮次覆盖、结构化结果 | 量化“内置”相对 stock 的净增量 |

## 五、决策规则

任何新实验开工前先声明它测的是 `P`、`N`、机制保真度还是治理成本。
不能再用“经验被调用了”作为主目标达成的证据；那只是适配器工作的证据。

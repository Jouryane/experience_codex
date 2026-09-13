# 规模索引面设计（Stage C5）

> 2026-09-10。默认实现不变；本稿说明 10 万–100 万经验量级的索引分层与
> 确定性兜底，配套 10k 正确性测试与 100k `#[ignore]` 基准。

## 1. 现状与边界

- 现状：`ExperienceStore` 维护 `active_by_tool`（tool → ACTIVE names），
  任务入口在 scope/usage 过滤后的 ACTIVE 子集上做机械匹配；
- 数十条量级：全量扫描与桶查找无差别，正确性优先；
- 规模红线：索引层只做“候选生成”，Select Plan（确定性）永远保有最终
  执行决定；默认不引入 embedding/RAG。

## 2. 五层设计

```text
L0 数据分区      scope/workspace/agent 分 store（C1 已提供 scope 键）
L1 ACTIVE 索引面  tool 桶 → scope 桶 → usage 白名单（现有 active_by_tool
                 扩展为 BTreeMap<scope?, tool, Vec<name>>）
L2 词元/前缀     任务签名倒排 + trigger 前缀树（只在 ACTIVE 面建）
L3 可选粗召回    embedding 仅候选生成；命中后回 L4 精排（默认关闭）
L4 确定性精排     Select Plan：State 谓词 + 覆盖 + confidence/用户偏好
L5 治理           usage 冷热缓存/decay/disable/归档（只 ACTIVE 进索引）
```

操作数说明：设 ACTIVE=N，tool 桶平均 M，scope 桶平均 K。桶化后每任务
唤醒 ≈ O(K_scene + 桶内精排)，不再 O(N)；`active_by_tool` 已经是
O(1) 取桶，扩展点是“桶内再按 scope/usage 过滤”和“桶太多时倒排”。

## 3. 正确性契约

- 索引化结果必须与线性过滤**同输入同输出**（命中集合逐名相等）；
- 缺省/索引未启用时行为与今天完全一致（MISS 语义不变）；
- 用户偏好 deny 与 scope 过滤在索引前后结果一致（C1/C2.2 已实现过滤，
  索引只是同一谓词的更快执行）。

## 4. 基准与测试

- `store.rs`：`synthetic_scale_scope_lookup_matches_linear_filter`（10k
  合成经验，正确性，CI 常跑）；
- `#[ignore]`：`synthetic_scale_100k_lookup_reports_ns`（100k 量级计时，
  手动 `cargo test -- --ignored` 运行，无硬断言）；
- 时机成熟再引入真正的 scope+tool 双键索引；本批不改变默认数据结构。

## 5. 落地清单（后续可选）

1. `active_by_scope_tool: HashMap<scope, HashMap<tool, Vec<name>>>`；
2. 任务签名词元倒排（HashMap<&str token, Vec<name>>）；
3. usage 热度 LRU 驻留热桶；
4. 可选 embedding 列/粗召回 adapter（用户自选向量库）。

## 6. 为什么现在不建：两个量级的声明（可移植）

我们的默认实现是**单机、数十到数百条经验**；索引分层是“需要时才启用”的
可选面，而不是默认复杂度。以下两个问题的答案本身就是给使用者的声明。

### 6.1 一万条经验（分布不均，存在共性）

- **现状够用**：`active_by_tool` 已经把候选面压到“同一工具”的桶；
  一万条里真正同工具、同 scope、ACTIVE 的通常只有几十~几百条，
  全量扫描这个子集是毫秒级；
- **不平衡恰恰是助力**：经验天然聚集在少数“能力族”（工具/动作骨架），
  桶分布越偏，桶化收益越大；
- **升级路径（无需改语义）**：在 5 的清单 1/2 上加一层
  `scope → tool → names`，把桶内过滤也变成索引查；正确性契约见 §3
  （索引结果必须与线性过滤逐名相等）。

### 6.2 一百万条经验（快速匹配）

- **不能靠线性扫描**：100 万条下“每条都算一次相似度”是不可接受的成本；
- **正确做法是两层**：
  1. **粗召回**（可禁用）：任务签名词元倒排（零依赖）或 embedding 召回
     （用户自选向量库）→ 候选 100~1000 条；
  2. **精排**（始终确定性）：State 谓词 + 动作覆盖 + confidence + 用户偏好，
     与今天同一条 `Select Plan`；
- **索引只生成候选，不做决定**：这是我们的红线——召回可以近似，
  执行决定必须确定性、可复现、可审计；
- **治理先行**：只让 ACTIVE 进索引；decay/disable/归档把“活经验”控制在
  可维护量级，比“全量索引 100 万”更划算。

### 6.3 为什么当前不这么做

1. 我们没有 100 万条经验，建了也没有真实数据可验证；
2. 索引是**可选加速层**，一旦引入就带来一致性/迁移/调试成本；
3. 现阶段更重要的是把**契约**写清楚（本文 + `learning-reuse-contract.md`），
   让使用者按自己的规模选择 5 中的第 1–4 项。

使用者想扩展时的最小改动：

```text
1. 复制 active_by_tool 的维护逻辑，加 scope 维度；
2. 在 candidates_for() 里先走索引，再用同一套 Predicate/偏好过滤；
3. 用 store.rs 现有的 10k 正确性测试与 100k #[ignore] 基准做回归。
```

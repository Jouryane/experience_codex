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

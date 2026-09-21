# 0003 · 三契约与 ExperienceTemplate 绑定层

> 状态：accepted · 2026-09-21
> 关联：[0002](0002-canonical-runtime-two-seams.md)、
> codex-main `docs/architecture/m9-template-validation.md`。

## 决策

Experience 分为三种类型级契约，不能混成一个模糊的“经验”容器：

| 契约 | 是否可执行 | 进入 Gate 的条件 |
|---|---|---|
| Exact Experience | 是 | trigger + preconditions 成立 |
| ExperienceTemplate | 绑定后才可执行 | 所有 required 参数确定性绑定成功 |
| ReferenceEntry | 否 | 只作为 LLM 参考，不拥有执行权 |

## 参数化规则

- 模板本身永远不执行；
- 参数只能来自确定性来源：task capture / action args / cwd；
- 缺少 required 参数 → 绑定失败 → Miss，禁止猜测或部分执行；
- 绑定后生成完整 `Experience`，并随 `GateDecision::TemplateHit` 传入执行；
- 当前 capture 是 `prefix/suffix` 确定性抽取，不引入正则或 LLM 填空；
- 如果参数必须由 LLM 推断，应降级为 Reference/Delegation，不能宣称零 LLM。

## 存储

`store.json` 在同一版本信封中增加 additive `templates[]`：

- Exact experiences 仍在 `experiences[]`；
- Templates 在 `templates[]`；
- Reference 仍在 `references{}`；
- 旧文件不需要迁移。

## 验收

M9：同一模板连续绑定：

```text
inbox/alpha.pdf -> library/alpha.pdf
inbox/beta.pdf  -> library/beta.pdf
```

两轮 fake provider 请求均为 0，产物正确，模板 usage `hits=2`。

# M9：ExperienceTemplate 参数化复验

> 2026-09-21。验证经验不再写死“移动某个文件”，而是可以绑定不同参数后
> 确定性执行。

## 1. 模板

fixture：`fixtures/m9/parameterized-move-store.json`

```text
trigger: task + "M9 move"
parameters:
  source <- [source=...]
  target <- [target=...]
workflow:
  move_file ${source} -> ${target}
preconditions:
  file:${source}.exists == true
postconditions:
  file:${source}.exists == false
  file:${target}.exists == true
```

模板不会进入执行表；命中后先绑定成具体 `Experience`，再交给同一个 P1
Runner 执行。

## 2. 两轮真机

| run | prompt 参数 | exit | fake requests | 结果 |
|---|---|---|---:|---|
| alpha | `[source=inbox/alpha.pdf] [target=library/alpha.pdf]` | 0 | **0** | 文件移动成功 |
| beta | `[source=inbox/beta.pdf] [target=library/beta.pdf]` | 0 | **0** | 文件移动成功 |

两次使用同一个模板 `m9_move_any_inbox_file`：

- 第一轮执行参数绑定为 `alpha`；
- 第二轮执行参数绑定为 `beta`；
- 两轮 stdout 都显示 `EXPERIENCE TASK GATE: completed ... without LLM`；
- usage：`hits=2`。

## 3. 这证明了什么

1. 经验可以保存“动作结构 + 参数槽”，不必写死文件名；
2. 绑定是确定性的，不需要 LLM 推断；
3. 绑定后的实例仍走同一执行/验证/备份/Gate 通道；
4. 缺少参数时会绑定失败，不会猜测执行。

## 4. 当前边界

- capture 目前是 `prefix/suffix`，不支持正则、嵌套结构或 LLM 绑定；
- 参数类型目前以字符串为主；
- binding provenance 已随 `TemplateHit` 进入内存决策，但尚未写入 usage 日志；
- 多实例、条件分支和工作流子链模板仍未实现。

## 5. 产物

- `validate-m9-template.ps1`
- `fixtures/m9/parameterized-move-store.json`
- `m9-template-artifacts/alpha/stdout.txt`
- `m9-template-artifacts/beta/stdout.txt`
- `m9-template-artifacts/store/usage.json`

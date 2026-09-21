# M10 · V2 路径验收：学习 → 资格 → 执行（单一 Store）

> 2026-09-21。复跑：`pwsh -File .\validate-m10-v2-path.ps1`
> （`-ReuseCold` 只复用已记录观测，不产生新的冷启动）。
> 决策依据：[decisions/0004](../../../experience-main/docs/decisions/0004-single-truth-and-verified-learning.md)。

## 为什么做这次验收

移动文件的测试只比探针高一点：它证明"执行面能跑"，不能证明"系统会学习"，
也不能证明"管理面和执行面看的是同一个 Store"。M10 同时回答这两件事，并且
故意使用一条**三步、含终端与网络**的任务族，而不是单步文件操作。

## 任务族

请求文本（三次运行只有参数不同）：

```text
V2 package transfer: [source=inbox/<name>.zip] [target=archive/<name>.zip]
  [repo=https://gitee.com/mirrors/rust]
1. git ls-remote <repo> HEAD      （终端 + 网络）
2. mkdir -p archive               （文件系统）
3. mv inbox/<name>.zip archive/   （文件系统）
```

## 验收链路与实测结果

| 阶段 | 做了什么 | 结果 |
|---|---|---|
| cold-a | 真实 LLM 执行 alpha 包（记录 tool call） | 成功；观测 3 步 |
| cold-b | 真实 LLM 执行 beta 包（同族、不同工作区） | 成功；观测 3 步（含一次空输入轮询，被翻译层判为非步骤） |
| induce | 双轨迹确定性归纳 | 生成 `Candidate` 模板：2 个参数、3 步 workflow（含 `exec`）、3 条 verification 探针 |
| qualify | **产品 CLI** `codex experience list / doctor / activate` | 先看到 `status=Candidate`，激活为 `Active`；`doctor` 报告 `format: canonical`、`drift: none` |
| warm | 第三个参数（gamma），fake provider 精确计数 | **模型请求 0**；Gate 完成任务；模板审计 `bindings={source:inbox/gamma.zip,target:archive/gamma.zip}`、指纹 `253c213513a206ed` |

冷启动两臂合计 16 次模型请求；热执行 0 次。

## 这次验收暴露并修掉的四个真问题

这四条都是"跑真实任务才看得见"的，前三个在最小验证里永远不会出现：

1. **归纳出的 body 没有 verification**。runtime 通过 `verification` 观测世界、
   `postconditions` 是断言；归纳器只写断言不写观测 → 永远 `Partial`。修法：
   归纳时按后置条件生成探针。
2. **`exec` 步失败不影响结论**。`git ls-remote` 退出 128 时工作流继续跑，
   单靠文件后置条件会"成功"。修法：进程步默认要求退出码 0（可用
   `expect_exit` 覆盖），不符合即判定接管失败并交回模型。
3. **真实轨迹有噪声**。模型会 `write_stdin` 轮询会话；翻译层把"空输入轮询"
   判为非步骤，"向活会话喂输入"判为拒绝。
4. **同族不同工作区**。两条观测在两台/两个目录里跑是常态；守卫从"必须等于
   某个 workspace"改为"单次观测内部一致"，因为 canonical 步骤是相对路径。

## 产物

- `m10-v2-artifacts/cold-a|cold-b/{observation.jsonl,stdout.txt,stderr.txt}`
  —— 真实观测（可复算归纳输入）
- `m10-v2-artifacts/induce.txt` —— 归纳判定与生成物
- `m10-v2-artifacts/cli-{list-before,doctor,activate,list-after}.txt`
  —— 管理面读到同一个文件
- `m10-v2-artifacts/warm/{stdout.txt,provider.jsonl}` —— 热执行（provider.jsonl 为空）
- `m10-v2-artifacts/store/{store.json,usage.json}` —— 单一 Store + 使用/审计
- `fixtures/m10/v2-policy-store.json` —— 起始策略（exec allowlist: git/python）

## 仍然不成立的推论

- **不能**说"复杂度 n 步、经验覆盖 m 步，就必然省 m 次请求"：真实省下的次数
  取决于前缀是否连续、是否可批处理（M8 已记录同样结论与长尾方差）。
- **不能**说网络出口已被治理：`exec` 白名单生效，网络策略仍只是 Store 里的
  声明，未中介到子进程（decision 0004 §6）。
- 归纳要求两条观测的结构完全一致（动作名、参数形状、步骤数）；不一致就是
  拒绝，不猜。

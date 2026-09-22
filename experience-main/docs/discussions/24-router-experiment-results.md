# 24 · Router 实验结果：路线 B 只能拦，路线 A 能拦并指路

> 2026-09-18。承接 [23](23-router-experiment-design.md)。
> 本文件只记录**已经跑出的数据**；没有测到的维度在 §六明确标出，不拿设计
> 当结果。

## 一、结论先说

1. **路线 A（真 PreToolUse hooks）在 stock Codex 上跑通，而且是有因果对照的。**
   隔离 `CODEX_HOME`、同一份已信任 `hooks.json`、同一任务、`allow` 与
   `block` 各 3 轮：
   - `allow`：原生路径完成 **3/3**，经验工具调用 **0/3**，正确 **3/3**；
   - `block`：hook 拦截后经验工具调用 **3/3**，正确 **3/3**；
   - `block` 的 3 轮里，模型可见文字 **3/3** 都复述了"原生写操作重复了
     已验证经验"并点名 `experience_feature_module`。

2. **路线 B（审批缝）能拦，但不能保真地"拦 + 点名工具"。**
   `CommandExecutionRequestApprovalResponse` 只有 `decision`，没有 reason 字段。
   用 JSON-RPC error 旁路塞 hint，没有在模型文字里留下我们写的那句话；模型
   看到的是通用的 `approval request failed`。所以路线 B 只能测"被拒绝后是否
   改用经验"，不能实现外审建议的 `block_reason` 语义。

3. **外审预测的"+1 模型回合"没有被 n=3 支持。**
   `block` 与 `allow` 的 `model_requests` 中位数都是 **4**；三轮配对差是
   **0, +1, 0**（`block − allow`）。wall 中位数 `block` 15.1s、`allow` 18.7s，
   方向与"router 更贵"相反，但同样只能视为噪声。**当前能立住的是确定性，
   不是成本。**

4. **没有看到"改写法躲避拦截"的退化行为。**
   `block` 臂里模型没有在第一个写操作被拦后换一种 shell/patch 写法继续硬做；
   它转向了经验工具。唯一一次额外 veto 是**我们自己 hook 的误拦**：第二轮
   一条只读验证命令因为字符串里含 `>`，被简单的写操作启发式当成了写命令。
   这是一条实现警示：**拦截面必须按结构化的工具/动作类型判定，不能按命令
   子串判定**，否则 veto 会开始吃掉只读探索。

5. **"回合中途才发现"的频率问题仍未回答。**
   自然条件下（不强制先走原生工具）的两次 scaffold 运行都直接调用了
   dynamic tool，审批请求为 0。也就是说 router 在这类任务里根本没有进入
   价值域。要回答"这种场合有多常见"，必须换一个任务：任务文本不直接包含
   已知前缀，模型先探索、后才发现它。

## 二、路线 A：真 hooks（机制实验）

### 配置

- host：stock `codex-cli 0.155.0-alpha.9`
  （`%LOCALAPPDATA%\OpenAI\Codex\bin\<version-hash>\codex.exe`）
- 隔离 home：`active_leg/router_home`，内含 `config.toml` + `hooks.json`
- hook 信任：先用 `hooks/list` 读 `currentHash`，再把 `trusted_hash` 写回
  `config.toml`。**未信任时 hook 被发现但不会执行**，这一点单独就有产品含义。
- 任务：创建精确的 `feature_module` scaffold
- 探针约束：明确要求"第一次工具调用必须是原生 shell/apply_patch"，用于把
  router 的机制打出来。这不是频率实验。

| rep | arm | wall_s | model_requests | hook calls | hook blocks | block reason seen | exp_calls | correct |
|---|---|---|---|---|---|---|---|---|
| 1 | block | 15.1 | 4 | 2 | 1 | True | 1 | True |
| 1 | allow | 18.7 | 4 | 3 | 0 | False | 0 | True |
| 2 | allow | 32.6 | 6 | 5 | 0 | False | 0 | True |
| 2 | block | 29.8 | 7 | 6 | 2 | True | 1 | True |
| 3 | block | 14.5 | 4 | 2 | 1 | True | 1 | True |
| 3 | allow | 16.3 | 4 | 3 | 0 | False | 0 | True |

按臂汇总：

| arm | n | wall 中位 | model_requests 中位 | hook blocks 中位 | 经验使用率 | 全部正确 |
|---|---|---|---|---|---|---|
| allow | 3 | 18.7 | 4 | 0 | 0/3 | 3/3 |
| block | 3 | 15.1 | 4 | 1 | 3/3 | 3/3 |

### 这一组对照证明了什么

- **因果成立**：同一 home、同一任务，只切换 hook 的 `allow/block` 行为，
  经验使用率从 0/3 变成 3/3。
- **消息送达**：`block` 臂的 assistant message 明确写了 hook 的原因和
  `experience_feature_module`；这不是我们从臂名推断的。
- **正确性不靠牺牲换**：6/6 最终判据通过。
- **成本未证实**：模型回合数没有稳定的 +1 位移。更诚实的说法是：
  "hook 拦截能把执行权稳定地交到经验工具，但它对总回合数的代价在当前
  样本里测不出稳定方向。"

## 三、路线 B：审批缝（为什么不能当保真 router）

已核对：

```text
CommandExecutionRequestApprovalResponse = { decision: accept|decline|cancel|... }
```

没有承载 `block_reason` 的字段。实验中尝试把 hint 放进 JSON-RPC error；
host 没有把它变成模型可见的理由。强制先走原生工具的 n=2 数据：

| rep | arm | model_requests | native approvals | exp_calls | 最终正确 |
|---|---|---|---|---|---|
| 1 | hint | 5 | 1 | 1 | True |
| 1 | decline | 6 | 1 | 1 | True |
| 2 | decline | 7 | 1 | 1 | True |
| 2 | hint | 6 | 1 | 1 | True |

两臂都能在原生路失败后落到经验工具，但模型文字里出现的是
`approval request failed`，不是我们构造的指路句。因此：

- 路线 B 可以验证"拒绝会促成回退"；
- 它**不能**验证"拒绝 + 点名工具"的保真形状；
- 这个形状目前只有路线 A 做到了。

## 四、外审建议的哪一半被证实了

外审建议：

> 把已验证的动作放进自己的工具；把 `PreToolUse` 留作 router，拦住原生调用，
> 在 `block_reason` 里点名你的工具。代价是一个额外的模型回合。

| 建议部分 | 本轮结论 |
|---|---|
| 用 dynamic tool 承载已验证动作 | 早就成立；本轮再次 3/3 正确执行 |
| 用 PreToolUse 拦原生调用 | **成立**：3/3 触发，且是有对照的 |
| 在 block_reason 里点名工具 | **成立**：3/3 到达模型文字 |
| 代价是一个额外模型回合 | **未证实**：n=3 中位数差异为 0 |
| veto 会导致模型"谈判/躲避" | **本轮未观察到**；但误拦一条只读命令暴露了实现风险 |

还有一个产品边界比 hook 语义更早出现：**hook 的信任状态不是自动的。** 未信任
hook 会被发现、显示 `enabled=true`，但不会执行（本轮因此先得到一批
`hook_invocations=0` 的无效数据）。要让 app-server 自动化真正使用 hook，
必须处理持久信任或受管 hook 来源。

## 五、对 Experience 论证的影响

1. **"router = 用现有 hook 面实现的 Gate"在机制上成立。** 不需要 fork；
   stock 上就能做到"拦原生 + 转向经验 + 返回结构化成功结果"。
2. **它服务的目标仍然不是省成本，而是可靠、可审计。** 这和数据里看到的
   一致：经验使用从"模型是否想起来"变成 **3/3 确定发生**，但回合数没有
   出现稳定的 +1 位移。
3. **"hook 只能否决，不能替代结果"这条边界继续成立。** `block_reason`
   是文字；真正的结构化成功结果仍由 dynamic tool 返回。
4. **最大未决问题缩小了。** 剩下要回答的不是"router 能不能做"，而是
   **"`pre` 覆盖不到的中途浮现前缀有多常见"**。如果它罕见，router 是
   可靠的保险；如果它常见，router 才是有规模的增量。

## 六、未测 / 下一步

- **U6｜中途浮现频率**：设计一个任务文本不直接给出已知前缀、模型必须先
  读世界才发现它的任务；测 `pre` 臂够不到、router 才能接管的次数。
- **U7｜精确动作分类**：把当前"命令子串含 `>` 就算写"的启发式替换为
  结构化判定（工具名 + 解析后的 action 类型）。这是本轮误拦的直接修复。
- **U8｜成本 n≥5**：若要把"+1 回合"从无结论变成结论，至少扩到每臂 5 次，
  并保持 allow/block 交替。
- **U9｜hook 信任的产品形态**：用户级 hook 的信任确认、受管 hook 和自动化
  场景的信任持久化，需要在 Experience 侧形成明确口径。

## 七、产物与复跑

实验脚本与原始数据在 `D:\test\_only\experience-segment-mode-20260916\active_leg\`：

- `router_hooks_experiment.py` + `router_hook.ps1`：路线 A
- `router_experiment.py`：路线 B
- `out/router_hooks_summary.md`：路线 A 表格汇总
- `out/router_hooks.jsonl`：路线 A 每轮记录
- `router_hooks/r{rep}-{arm}-hook.jsonl`：每次 hook 调用的原始 stdin 摘要
- `out/router_summary.md`：路线 B 表格汇总
- `out/router.jsonl`：路线 B 每轮记录

复跑路线 A：

```powershell
cd D:\test\_only\experience-segment-mode-20260916\active_leg
$env:PYTHONIOENCODING='utf-8'
python router_hooks_experiment.py --arms allow,block --reps 3 --reset
```

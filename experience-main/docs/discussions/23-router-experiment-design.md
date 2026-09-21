# 23 · Router 实验：设计、已核对的事实、以及为什么它还没跑

> 2026-09-18。承接 [22](22-external-review-posttooluse-and-router.md)。
> 本文件是**设计稿 + 已核对的接口事实**，供下一次直接执行。
>
> **已执行（2026-09-18）：** 路线 B 与路线 A 都跑了；结果、原始数据位置和
> 仍未回答的问题见 [24](24-router-experiment-results.md)。

## 一、为什么这个实验比前面几轮重

前面所有实验都只用了 **app-server 客户端能控制的东西**（`thread/start` 的
`dynamicTools`、`turn/start` 的输入、审批应答）。

而 hook 是**配置驱动**的，不在线程级接口里：

```
配置来源：codex_config::HookEventsToml
配置键形如：file:/tmp/hooks.json:pre_tool_use:0:0
```

也就是说，router 要落到**测试用的 `CODEX_HOME` 里的一份配置 + 一个 shell 脚本**，
而不是客户端的一个参数。这是它比前几轮重的原因。

## 二、已核对的接口事实（本地 checkout）

`codex-rs/hooks/src/schema.rs`：

```rust
enum PreToolUseDecisionWire        { approve, block }
enum PreToolUsePermissionDecisionWire { allow, deny, ask }
```

hook 通过 stdin 拿 JSON、stdout 回 JSON，决策上限就是**放行 / 阻断**，
以及允许/拒绝/询问——**没有"我来做并返回结果"**。这与
[22](22-external-review-posttooluse-and-router.md) 的类型学结论一致。

## 三、router 的精确价值域（这一节比实现更重要）

**先问：有没有一个场合，经验必须在"模型发起之后"才可用？**

有——**当触发不是"回合开始时任务文本"，而是"模型在回合中途发现要做什么"的时候。**
例如模型读了输出、跑了诊断，才决定"该修复这个值"。此时客户端在
`turn/start` 之前**没有机会**预先匹配（[round 17](17-three-arm-result.md) 的 `pre`
臂只能匹配回合开始时的任务文本）。

> **router 的价值域 = `pre` 覆盖不到的那部分：中途才浮现的已知前缀。**

如果不存在这种场合，router 相对 `pre` 没有增量——`pre` 在整任务覆盖时已经是 0 回合。
**所以这个实验的第一个产出应当是"这个场合有多常见"，而不是"router 省不省"。**

## 四、两条实现路线

| 路线 | 做法 | 保真度 | 成本 |
|---|---|---|---|
| **A｜真 hooks** | 往测试 `CODEX_HOME` 写 `HookEventsToml` 配置 + 一个脚本：读 stdin JSON，命中则输出 `{"decision":"block","reason":"用 experience_X"}` | 高（就是 review 建议的原形） | 要处理 shell 协议 + 隔离 home |
| **B｜审批缝** | 把 `approvalPolicy` 调到命令需审批，客户端在 `item/commandExecution/requestApproval` 上返回**拒绝 + 提示改用经验工具** | 中（拦+指路的作用相同，载体不同） | 低（纯客户端） |

**建议先 B 再 A**：B 能用已验证的客户端骨架快速拿到"拦+指路"的效用数据，
若效果成立再上 A 验证保真版本。

## 五、测量项

| 指标 | 用途 |
|---|---|
| 模型回合数 / 请求数（对比 plain pull 与本轮基线） | 他的预测是 **+1 回合**，验证这个代价 |
| **模型是否改变命令写法以躲避拦截** | **他预警的退化均衡**——本轮唯一全新的风险，且只有我们能量化 |
| 经验是否被调用（二值） | router 的主要收益：**确定性**，对比 pull 的掷硬币 |
| 正确性 | 一如既往 |

## 六、判读纪律（沿用本轮建立的）

1. 每格 **n≥2**，臂序按 rep 轮转（round 12 的教训）；
2. 单一计数器先**交叉验证**再用（round 16 / 20 两次教训）；
3. 单次差异一律视为噪声；
4. `notifications` 是未控变量，若随效应同步变化，先怀疑它。

## 七、为什么这次没跑

本次会话的剩余空间不足以完成"配置 hooks → 跑 → 复查 → 记录"这条链，
**半成品比设计稿更没有价值**，所以停在这里，把已核对的事实和设计留给下一次。
需要新增的东西只有两件：一个测试用 `CODEX_HOME` 的 hook 配置，和一个处理
stdin/stdout JSON 的脚本（约 30 行）。

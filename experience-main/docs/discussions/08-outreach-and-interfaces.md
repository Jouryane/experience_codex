# 08 · 对外沟通记录与宿主接口核对

> 本文件是**操作记录**，不是结论。结论在 01–07。
> 单开一文件的原因：07 曾经把定义、外联、接口三件事装在一起，长到 20 KB 且互相干扰。

## 一、沟通记录

| 时间 | 对象 | 渠道 | 结果 |
|---|---|---|---|
| 2026-09-16 | @tianyi（DeepSeek Harness 团队） | X 私信 | 发出，尚无回音。**不追。**若回，按 §六第 4 条处理 |
| 2026-09-16/17 | openai/codex Discussions | 公开帖 | **收到高质量回复**，见 §三 |

> 已发出的私信正文不在此留存——它和当初的草稿不一样，留草稿会让人误以为是发出去的版本。
> 唯一的记录是：**DSH 那条线保持静默。**

## 二、Codex 侧帖子的落点

标题（问题前置 + 一条可验证的事实）：

```text
PreToolUse can block or rewrite a call, but not produce its result — is that a
deliberate boundary?
```

正文的骨架（全文见 git 历史）：

1. 具体观察：`PreToolUseHookResult` 只有 `Continue { updated_input }` 与 `Blocked`，
   没有携带结果的变体；并注明 payload 形状像 Claude Code 的 hook 协议。
2. 一个问题：这个留白是**有意**的，还是只是未探索的缝隙？
3. 经验是什么（不是 RAG / MCP / skill / 压缩上下文）。
4. 已经做了什么、测到什么（**并明确标注合成域与无模型在环**）。
5. 为什么这变成一个架构问题（fork 撞的墙）。

## 三、回复带来的纠正与创造

### 3.1 纠正（三条，全部集中在“它能住在哪里”这个域）

| # | 原来的判断 | 被纠正为 |
|---|---|---|
| N1' | “没有 LLM 之前的先动手座位” | **下得太宽**：主动调用那条腿**有座位，而且是有文档的公开接口**（`thread/start.dynamicTools`）。是紧迫感的纠正，不是定义的纠正 |
| N2' | “接管返回值必须伪装成被接管工具的 canonical value” | **只对拦截模式成立**；动态工具有自己的内容契约（`content_items` + `success`）。印证了“N2 本质上是 N1 的延伸” |
| N3' | “接管要成为 durable 事实”被列为**最贵的一条** | **注册模式下平台已提供**：`TurnItem::DynamicToolCall` 是 v2 一等 item，`thread_history.rs` 可从 request+response 重建 |

### 3.2 创造（三条，都是“回复 + 代码核对”共同产生）

1. **“拦截 vs 注册”是一根我们在整个讨论里从未考虑过的轴。**
   此前所有架构推理都默认经验必须**拦下模型的调用**。注册模式的性质完全不同：
   不冒充 schema、不改会话格式、不碰不变量、今天就能跑。
2. **`deferLoading` 把成本论证掀翻了。** [04](04-token-economy-and-media.md) 花了
   一整节论证“经验不能以知识形态存在，否则每请求付费”——而协议里已经给了
   “注册但不加载”的旋钮。**“把已知配方注册成工具”从“要吃上下文预算”变成“近似免费”。**
3. **九个矛盾收缩成一个。** 从“没有座位”到“**有一个座位，还缺第二个座位**”——
   剩下那一句是：**经验能不能在模型没有问的时候起作用？**

### 3.3 理论部分一条都没被推翻

C45（时间维度）、C46（两条腿）、C15–C19（段化）、C39–C42（介质与成本）、
C43/C44（投影与硬约束）全部原样成立。被纠正的**全部是接口假设**，
不是理论。这是个好信号：**理论对，是我们对接口的想象错了。**

## 四、Codex 侧接口核对结果（本地 checkout，基线 `6478a751f`，2026-08-29）

| 事实 | 位置 | 意义 |
|---|---|---|
| `DynamicToolSpec = Function \| Namespace`，且 `DynamicToolCallRequest.namespace` 存在 | `codex-rs/protocol/src/dynamic_tools.rs` | 经验可以注册成**命名空间**下的多个工具；namespace 是现成的粗粒度身份键（对应 C12–C14） |
| `DynamicToolFunctionSpec.defer_loading` | 同上 | **注册的工具可以延迟加载**——大量经验不必立刻占用 prompt |
| `DynamicToolResponse { content_items, success }` | 同上 | 动态工具有**自己的内容契约**，不需要冒充内建工具的 schema |
| `TurnItem::DynamicToolCall` 是 v2 一等 item，带 `duration_ms`；`thread_history.rs` 有 `reconstructs_dynamic_tool_items_from_request_and_response_events` | `app-server-protocol/src/protocol/v2/item.rs`、`thread_history.rs` | **动态工具调用是可重建的 thread history**，不只是生命周期事件 |
| `HOOK_EVENT_NAMES` 有 12 个事件，含 `PreToolUse` / `PostToolUse` / `PermissionRequest` / `PreCompact` / `SubagentStart` 等；由 plugin 声明 | `codex-rs/hooks/src/lib.rs` | Codex 已有 seat，而且是**插件分发**的 |
| `PreToolUseOutcome` 有 `should_block` / `block_reason` / `additional_contexts` / **`updated_input`** | `codex-rs/hooks/src/events/pre_tool_use.rs` | Codex 的座位比 DSH 的更宽：**支持改写参数**；但仍**没有结果字段** |

**结论**：两个独立架构、同一个空缺——**都能拦，都不能代做，也都不主动触发。**
这不是某一个产品的疏漏，是 agent runtime 这一类系统的共同盲区。

### 4.1 已实测：动态工具注册可用（2026-09-17）

用 `codex app-server generate-json-schema` 抓下完整协议 schema（约 250 个文件）
之后，写了一个最小客户端（`D:\test\_only\experience-segment-mode-20260916\active_leg\`）
并**实跑通过握手**：

```text
[initialize] ok                       # initialize + capabilities.experimentalApi = true
[thread/start] ok, thread=01a0ade6-...
```

即：

1. **stdio 分帧确实是换行分隔 JSON（JSONL）**——之前的假设成立；
2. **`thread/start` 接受 `dynamicTools: [{type:"function", name, description, inputSchema, deferLoading}]`**，
   把工具注册进去**没有任何报错**；
3. 剩下的唯一阻塞是 `DEEPSEEK_API_KEY` 未设置（stderr 明确报
   `startup auth prewarm failed: Missing environment variable`），
   也就是**只有"模型是否会主动调用"这一步还没有被观测**。

这条实测把"注册模式可行"从文档推论变成了本地事实。

### 4.2 已实测：Pull 腿在 **stock Codex** 上端到端跑通（2026-09-17）

宿主：`C:\Users\someuser\AppData\Local\OpenAI\Codex\bin\eab8377aebac6c07\codex.exe`
（**官方安装，`codex-cli 0.155.0-alpha.2.6`，不是我们的 fork**）。
工具面**由 `registry.json` 生成**（2 条经验 → 2 个动态工具），不是手写。
客户端：`D:\test\_only\experience-segment-mode-20260916\active_leg\explicit_experience.py`。

| 轮次 | 任务 | 模型是否调用 | 验证 | wall | totalTokens |
|---|---|---|---|---|---|
| 1 | 开工：新建功能模块 | **是** `experience_feature_module` | **True**（后置判据通过） | 21.7s | 87,665 |
| 2 | 再帮我开工一次（同一 workspace） | **是**（模型不知道已完成） | **False**（前提 `file.absent(src)` 不成立，**未重复执行**） | 59.8s | 316,607 |

轮次 1 真实落盘：`README.md` / `src/__init__.py` / `src/feature/index.py`（内容正确）。

**三条结论：**

1. **Pull 腿不需要 fork**——官方二进制接受 registry 生成的动态工具，模型会主动调用；
2. **C44 在真实回路里成立**——前提被自身动作证伪，第二次触发被拦下，没有双执行；
3. **但"拒绝"的代价很高**：轮次 2 的 token 是轮次 1 的 3.6 倍、耗时 2.8 倍，通知数
   668 → 8332。因为模型拿到的只是"前提不成立"，它必须自己搞清楚为什么、再重新规划。

**推论（新）**：门控只保证"不伤害"，不保证"不昂贵"。
**拒绝必须携带足够的信息让模型立刻接受并停止**——例如
"已经做过了：`src/` 存在，其中是 X、Y、Z"，而不是"前提不成立"。
这是一条可立刻验证的改进，也是 Push 腿必须一起解决的问题
（Push 没有接手方时，代价只会更高）。

## 五、仍待答复的问题（比 N1–N9 窄得多）

1. `deferLoading` 是否真意味着可以注册很大的工具面而 prompt 成本近似为零？
2. 动态工具**能不能**占用内建保留命名空间的名字？文档说避免，
   但在 `normalize_dynamic_tool_specs` 里没看到强制检查——是文档约定还是运行时约束？
3. 如果 2 的答案是不可能，那么“模型提出 `Bash`、由客户端代做”这条路彻底关闭，
   只剩“不经模型调用的触发机制”这一条请求。

## 六、冷启动外联的五条纪律（通用，不针对某个对象）

1. **不要附文档或链接**——第一封信附材料等于要求对方做功课；
2. **不要说“我们做了 X，你们要不要集成”**——那是推销；
3. **不要用“你们应该”**——换成“我想验证一个判断”；
4. **不要一次问多个问题**——问一个，剩下的等回信；
5. **不要三连免责**。一句是礼貌，三句是**自我判决**——等于给对方一个同意并结束
   对话的许可。

另外两条渠道层面的经验：

- **问题要前置**。要求对方滚动才能看到问题，回复率会明显下降；
- 私信在大号账号上大概率落进 message requests，**公开讨论区是更可靠的并行渠道**：
  公开、可搜索、不欠人情，而且没人回也不算失礼。

# 参考实现借用清单（prior-art experience_runtime）

> 来源：作者自有的一个私有 Python 参考实现（模块名 `experience_runtime`，
> v0.2.1，2026-08-31 仍在更新）。路径与项目名不进入本仓库。
> 版本：v0.2.1（Python）
> 性质：**设计蓝本，不是代码移植**。借语义与结构，不复制 Python 实现。

## 为什么可以借用

该参考实现与本项目的 10 阶段规划同构：宿主在每次潜在 LLM 调用之前先询问
Runtime，命中且成熟则确定性执行（零 LLM），不成熟则 assist，未命中则
delegate；事后反馈驱动置信度升降与经验演化。这正是我们要在 Rust 里重写的
核心语义。

## 文件映射

| 参考文件 | 职责 | 我们的对应 | 阶段 |
|---|---|---|---|
| `__init__.py` | ExperienceRuntime 入口 | `experience/mod.rs` | - |
| `runtime.py` | 编排 request→encode/match/gate/run→feedback；`before_llm` 门面；目标记忆 | `experience_runtime.rs` | 4/5 |
| `state.py` | 状态编码（分词/任务分类/实体/哈希 n-gram 向量） | `experience_state.rs` + 未来 `experience_matcher.rs` 的编码侧 | 3/4 |
| `matcher.py` | 三元匹配（规则/标签/向量 + BM25）、类型分化权重 | `experience_matcher.rs` | 4 |
| `controller.py` | 激活门控（置信度×相似度×安全门）、拒绝链 | 并入 `experience_runtime.rs` | 4/6 |
| `executor.py` | 确定性执行器 + 安全表达式求值（非 eval/exec） | `experience_executor.rs` | 4 |
| `feedback.py` | 三路径反馈 → 置信度闭环 | `experience_confidence.rs` | 4/7 |
| `store.py` | 独立 SQLite（不碰宿主数据库） | `experience_store.rs` | 4 |
| `schema.py` | 经验 Schema 版本化 + 归一化 + 校验 | `experience.rs` | 8 |
| `mcp_server.py` | MCP 外壳（40 工具，标准帧协议） | 阶段 10 的 MCP adapter | 10 |
| `assoc_query.py` | 联想记忆查询 | 后期可选 | 后期 |

## 关键语义借鉴

1. **决策链**：`execute`（conf≥0.75 且 sim≥0.50，绕过 LLM）→ `assist`
   （conf≥0.35 且 sim≥0.30，注入 prefill/origin 供 LLM 修正）→ `delegate`
   （正常 LLM 流程 + 保存 episode 供编译）。我们的 `ControlDecision`
   （ExecuteExperience / InvokeLLM / AbortOrAsk）按用户规划保持三态；
   `assist` 可未来作为第四个变体引入。
2. **匹配排序原则**：候选按匹配质量排序，置信度只决定"信任到什么程度"，
   不参与排序——避免高置信弱匹配挤掉低置信强匹配。
3. **拒绝链公平化**：执行型（result/reflex）严格门槛；reference/process 型
   失败时降级为 hint 参考注入，不硬拒。
4. **置信度公式**：`score = PA × (base + UF×w_uf + UA×w_ua + R×w_r) × DS`，
   多维计数（success/failure/streak/frequency/positive_fb/negative_fb/last_used）。
5. **类型分化**：reference/process/result/reflex 各有匹配权重。
6. **安全执行**：安全表达式求值器替代 eval/exec。
7. **项目级工作区**：`.experience/` 独立经验库 + 管理仪表盘 + 经验卡片。

## 本项目对参考实现的扩展（用户第一性原理要求）

参考实现的 state 是"输入编码"（文本特征/任务类/实体），而用户明确要求
Experience State 承担另一种职责：**记录经验所依赖的状态元素**——环境配置、
环境检测、网络配置、工具配置等，让命中后能快速唤起/跳过这些状态的建立，
避免浪费 LLM 资源。这些状态信息同时是可被 LLM 检查的信息源：条件通过则
LLM 不必读取；条件不通过、LLM 介入时再读取。

这部分的 Rust 设计（`EnvironmentState` / `NetworkState` / `ToolState` /
`ConfigState` + 统一 `lookup`）在参考实现中没有直接对应，按本项目
`experience_state.rs` 独立实现。

## 借用边界

- 只借设计与语义，不复制 Python 代码。
- 该参考实现所在的宿主应用（七层架构、金融业务）与本项目无关，不采用。
- 经验 DB、.experience/、测试数据等本地产物不进入本仓库。

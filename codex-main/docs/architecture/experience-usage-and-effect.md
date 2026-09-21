# Experience Codex 使用与效果边界说明

> 日期：2026-09-06
> 范围：回答三个问题——(1) 能否停用 experience；(2) 与 Codex 固有功能
> 同时使用时如何启动；(3) 官方路径与改造路径的关系、改造如何生效。

## 0. 一句话结论

experience 是 **codex-core 内部、挂在官方采样循环缝隙上的伪平行层**，不是
另一个应用。它只在我们编译并运行的二进制里存在；官方二进制里不存在。

## 1. 能否停用 experience？（Q1）

### 现状（代码事实）

- 决策点逻辑：`decide_for_state` → store 为空或全部 DISABLED → 恒 MISS →
  `Delegate`，行为与基线一致（LLM 照常、skills/MCP/工具/审批/沙箱全保留）。
- 学习闭环：只有“成功 LLM 采样 + 工具调用”后才进入 learner；但**没有
  显式总开关**，若开着自动确认（`EXPERIENCE_AUTO_CONFIRM`），新轨迹仍会
  重新沉淀候选。

### 结论

- 支持“**等效停用**”：清空/禁用经验库 + 不设自动确认 → 运行与原生几乎
  一致（决策点多一次空匹配，毫秒级）；
- ✅ 显式硬停用已实现（2026-09-06）：`EXPERIENCE_ENABLED=0/false/no/off`
  关闭时，matching/execution/learning 全部跳过，运行 turn 每次同步开关，
  行为与原始 Codex 循环一致；默认开启（不设即启用）。管理页仍可查看库。

## 2. 与固有功能同时使用：如何启动（Q2）

不需要“两个系统分别启动”。experience 内嵌在 agent 循环里，用户只用
**一个我们编译的二进制**，固有功能天然并存。

### 启动步骤

1. **准备二进制**：在 `codex-rs` 下
   `cargo build -p codex-cli`（GNU 工具链），产物
   `codex-rs/target/debug/codex.exe`。
2. **配置家目录**：设 `CODEX_HOME`（例如实验隔离
   `D:\experience_codex\codex-main\.codex-exp-home`），该目录下 `config.toml`
   选择模型与供应商（DeepSeek / OpenAI 均可——架构与模型无关）；
   `models.json` 提供模型 schema。
3. **运行任务**：
   ```
   codex.exe exec --skip-git-repo-check   # 交互/脚本 stdin 均可
   ```
   运行期间：无匹配经验 → 走固有 LLM 流程（工具/技能/MCP 不变）；
   命中高置信同构经验 → 零 LLM 直接执行；命中参考 → 经验内容注入、LLM 主导。
4. **管理与观察**（不影响 agent 运行）：
   - `codex.exe experience list/pin/unpin/disable`
   - `codex.exe experience ui --port 8765`（HTML 管理页）
   - `experience-ui-launcher.exe`（双击入口：自动定位 codex.exe 与
     `.codex-exp-home`、启动服务并打开页面）
5. **经验数据位置**：`<CODEX_HOME>/experience/store.json`（本体）、
   `usage.json`（命中/档位/引用日志/审计）、`trash.json`（删除回收）。

### 与固有功能的关系

- 工具/技能/MCP/审批/沙箱：保留官方行为；经验执行也走同一 ToolCallRuntime
  通道（审批、沙箱、工具记录）；
- 经验本身不是工具、不是 MCP、不是技能（架构不变量）；
- “同时使用”= 运行同一个二进制，让经验在缝隙中先试、固有功能兜底。

## 3. 官方 UI / 官方路径与改造路径（Q3）

### 谁走哪条路径

- **官方路径**：用户安装/登录 OpenAI 官方 Codex（官方 Desktop/官方
  app-server）。运行的是官方二进制，代码里没有 experience——改造零效果。
- **改造路径**：用户运行我们编译的 `codex.exe` 或其 app-server。经验决策点
  在 run_turn 内，一定生效。

### 官方 UI 能显示 experience 吗

不能直接显示。经验 UI 是我们自有的 HTML 管理页（启动器/experience ui），
官方 UI 不识别它；官方 UI 能看到的只是经验执行留下的对话痕迹/工具调用。

### 如果强制走官方默认路径，改造怎么产生效果

**不会产生效果**——因为改造在“后端 core 的采样循环内”，UI/前端无法插入该
决策点。要让改造生效，必须满足下列任一：

1. **直接用改造二进制**：用户跑我们构建的 codex（CLI 或 app-server）——
   当前唯一可靠路径；
2. **官方 UI 当观察器 + 我们的后端**：把官方 UI 接向我们启动的 app-server
   （此前已验证官方 UI 通过 app-server 协议驱动线程；若未来官方允许指定
   本地后端/自托管运行时，即可达成），此时决策在改造后端内；
3. **等官方放权**：官方若开放“可替换/扩展 core 决策点”或成熟插件/面板渠道，
   届时把同一套 experience core 以官方支持的方式挂入——这是远期路径，
   当前不依赖。

### 对用户的直接建议

- 若目标是“体验 experience_codex”：**不要用官方安装目录的 codex**，用本
  仓库构建的 `codex.exe`（模型可自配）；
- 若目标是“官方 UI 外观 + 改造行为”：等待/争取官方 app-server 自定义后端
  或 core 扩展放权，UI 侧只做观察。

## 4. 待补清单

- config 字段形态的总开关（目前为 env `EXPERIENCE_ENABLED`；如需
  `config.toml` 字段可后续接入）；
- 官方 app-server 自定义后端/插件渠道成熟度跟进（此前勘察：当前不可用）；
- “等效停用 vs 显式停用”的验收测试（停用后与基线行为/开销一致）。

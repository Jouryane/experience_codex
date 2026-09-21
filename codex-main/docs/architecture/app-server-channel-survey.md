# App Server 渠道成熟度勘察（experience 管理 UI 载体）

> 日期：2026-09-05
> 目的：在决定如何把 experience 管理页接到 `codex app-server` 之前，先评估
> “官方渠道”（Apps / io.modelcontextprotocol/ui webview / 本地 Plugin UI）
> 是否成熟可用，避免选一条会被官方放权/更新随时打断的路。

## 1. 三条候选渠道的取证结论

### 1.1 Apps（`app/*`，chatgpt.com/apps）

证据：README §Apps；`app/list` 返回 accessible + directory apps；元数据带
`installUrl: https://chatgpt.com/apps/...`；`app/read` 缺失时向后端
`POST /ps/apxinitialize.capabilities.extensions["io.modelcontextprotocol/ui"]`
出现（透传声明，如 `mimeTypes: ["text/html;profile=mcp-app"]`）；app-server
侧只是“keeps the complete value”（保留透传），服务端代码中没有任何
webview 宿主实现（grep 无命中，仅协议 schema/序列化测试）；README 明确
websocket transport 为 experimental/unsupported。

结论：该能力描述的是**客户端（官方 UI / VS Code 扩展）支持 mcp-app HTML**，
服务端不负责宿主呈现；同时它面向“会话内 webview/表单”，不是独立管理面板。
在我们能控制的 codex 仓库里没有可用的宿主实现，且依赖官方客户端放权。
**成熟度：低，不依赖。**

### 1.3 本地 Plugin（`.codex-plugin` / `plugin://` mention）

证据：plugin 是真实本地机制（marketplace 安装、`plugin/list`、
`plugin/read`、`plugin/install`、skills/commands/MCP 注入）；对话中可用
`mention`（`plugin://<name>@<marketplace>`）唤起（README “Invoke a plugin”）。
插件 manifest 的 interface 目前只有 `defaultPrompt` 之类的说明，**没有独立
页面/面板宿主**。

结论：本地插件是受支持的“对话内入口”（如 `@experience` 触发管理命令），
但不是“独立管理页”的宿主。

## 2. 综合判断

| 渠道 | 可注册本地页面 | 是否受官方放权 | 当前成熟度 | 对本项目 |
|---|---|---|---|---|
| Apps（云端 connector） | 否 | 是 | 账号级可用 | 不可用 |
| io.modelcontextprotocol/ui | 服务端无宿主 | 是（客户端） | 低/试验 | 不可依赖 |
| 本地 Plugin | 无页面宿主 | 否 | 中（对话入口） | 可作临时入口 |
| **扩展 app-server JSON-RPC** | 是（我们自建） | 否（本地协议） | 高（官方同款传输） | **推荐** |

推荐路线：**A）扩展 app-server 协议（`experience/*` 方法族）+ 自有轻量
前端页面**。管理页通过 app-server 同款 JSON-RPC transport 取数；未来若
官方开放 plugin UI 面板或 io.modelcontextprotocol/ui 宿主，我们只换前端壳，
后端复用。

可选混合：做一个本地 Plugin（`@experience`），把“打开管理页 / 常用管理
命令”做成官方原生的对话内入口，作为临时低成本通道；完整页面仍走自有
前端。

## 3. 决策请求

1. 采纳路线 A（协议扩展 + 自有前端）？
2. 是否顺带做 `@experience` 本地 Plugin 对话入口（低成本的官方原生入口）？
   还是先只做路线 A，插件后置？

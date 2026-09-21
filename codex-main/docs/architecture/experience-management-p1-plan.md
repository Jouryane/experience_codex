# Experience 管理 · P1 专项实施计划（app-server 协议扩展）

> 日期：2026-09-05
> 前提：渠道勘察结论（app-server-channel-survey.md）→ 采纳路线 A；
> P0 域层已完成（4f4bde17d）。

## 1. 为什么是“专项”

`codex app-server` 的请求/响应均为 `codex_app_server_protocol` 宏生成的
强类型枚举（`ClientRequest` / `ClientResponsePayload` / `ClientResponse`），
并联动 TS/JSON-schema 导出、实验 API 门控与 schema fixtures 测试。
`experience/*` 想成为一等方法，必须按官方模式完整接入——这要求在一段
专注的连续工作单元里按序修改并反复全量编译（GNU，单轮数分钟），不适合
在长会话尾段穿插完成。

## 2. 目标范围（最小方法集先行）

第一批只打通“看得到”：

- `experience/list` → 全部经验行（名称/状态/置信度/scope/对象/pinned/
  最近使用/命中/misfire/invalid）
- `experience/detail { id }` → 完整详情（experience 全字段 + usage + 审计）

第二批（同一专项内，机制跑通后追加）：

- `experience/pin` / `experience/unpin`
- `experience/activate` / `experience/revalidate` / `experience/disable`
- `experience/delete`
- `experience/export` / `experience/import`
- `experience/editAsDraft` / `experience/adoptDraft`

## 3. 改动文件清单（按序）

1. `codex-rs/app-server-protocol/src/protocol/common.rs`
   - `ClientRequest` 变体 + 参数类型（`ExperienceListParams` /
     `ExperienceDetailParams` 等，可先空/最小）；
   - `ClientResponsePayload` 变体 + 响应类型（复用 core 的
     `ManagedRow`/`ExperienceDetail` 会引入依赖方向问题——DTO 放在
     protocol 侧或 app-server 侧转 JSON，按现有 processor 模式定）。
2. `codex-rs/app-server-protocol/.../v1.rs`（或对应 v2 文件）：
   方法/参数/响应类型归属与序列化 rename。
3. `codex-rs/app-server/src/request_serialization.rs`：
   method 字符串 ↔ `ClientRequest` 映射。
4. `codex-rs/app-server/src/request_processors/experience_processor.rs`（新增）：
   持有 `ExperienceManagementService`，实现各方法；
   `request_processors` 目录模块声明按现有模式补全。
5. `codex-rs/app-server/src/message_processor.rs`：
   `handle_client_request` match 新分支 + processor 装配（构造/依赖注入，
   参考 ThreadGoal/Apps processor 的 new 与字段）。
6. protocol schema fixtures / TS 生成测试：补齐新方法条目。
7. 单测：processor 直接调用 + 一次 JSON-RPC 级端到端（可选 stdio 子进程
   测试沿用现有 transport_tests 模式）。

## 4. 约束（红线继承）

- 只读/写同一 `<codex_home>/experience/store.json`；usage.json 相邻；
- processor 每次操作前 reload（乐观、不独占）；
- 写操作全部走 ManagerService（生命周期/校验），格式与 agent 完全兼容；
- 管理渠道不进决策路径；前端是观察器。

## 5. 风险与回滚

- 协议宏/枚举改动可能波及其他客户端（官方 UI、VS Code 扩展）：以
  向后兼容方式新增（默认参数/新变体只增不改），官方客户端不受影响；
- schema fixtures/TS 不匹配会挂 CI：改协议同时同步 fixtures；
- 回滚：单独提交，`git revert` 即可（P0 域层与协议改动分离）。

## 6. @experience 插件试点（可选混合）

- 范围：本地 `.codex-plugin`，对话内 `@experience` 唤起，提供
  “打开管理页/列出经验/禁用指定经验”等命令；
- 前置调研：本地插件注册/市场路径（`plugin/install` 对本地目录的支持、
  `.codex-plugin/plugin.json` manifest 结构），插件试点单独小步；
- 时序：协议 P1 完成后进行（先让后端方法稳定，插件只是对话入口壳）。

## 7. 状态（2026-09-05）

- ✅ P1 协议扩展完成（提交 `e2f3b4860`）：protocol v2/experience.rs +
  common.rs 12 条 `experience/*` 方法；app-server experience_processor +
  message_processor 装配；`cargo check -p codex-app-server` 与
  `-p codex-cli` 均 exit 0。
- ✅ stdio 冒烟：initialize 握手成功（返回 codexHome 正确指向
  `.codex-exp-home`）。`experience/list` 未在冒烟中回显——原因是 stdin
  文件 EOF 使连接先断开（脚本局限），需真实客户端（P2 前端）或保持
  连接的双工客户端做最终端到端确认。
- ⏳ 待 P2：扁平列表/详情前端（经 app-server JSON-RPC 取数）；
  `@experience` 插件试点；runtime 决策统计写入 usage.json。

## 8. P2 状态（2026-09-05）

- ✅ `codex experience ui` 落地：spawn 自身 `app-server --stdio` 为后端，
  官方 JSON-RPC 2.0（initialize 握手 + `experience/*`），tiny_http 提供
  单页（列表 + 扁平详情 + activate/revalidate/disable/pin/unpin/delete）；
  浏览器只是观察器。
- ✅ 端到端验证：`/` 页面可访问；`/api/list` 经 app-server 返回 2 条经验；
  `/api/detail` 返回完整详情。
- ⏳ 剩余：`@experience` 插件试点；runtime 决策统计写入 usage.json；
  树状关系视图（第二阶段）。

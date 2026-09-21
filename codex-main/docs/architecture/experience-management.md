# Experience 管理渠道设计（前端展示 / 后端排布 / 管理逻辑）

> 日期：2026-09-05
> 现状基线：`codex experience` CLI + stdio MCP（list/pin/unpin/disable/path），
> 数据源 `<codex_home>/experience/store.json`；App Server 无 experience 端点；
> runtime 的 counters/last_used 仅存内存，管理行里 last_used 恒为 None。

## 1. 目标与原则

1. **管理渠道 ≠ 决策路径**：管理页/API 只读写 experience 数据与元数据，
   永不进入 `run_turn` 的 matcher/decide（不变量 §0 保持）。
2. **单一事实源**：`store.json`（经验本体）+ 新增 `usage.json`
   （统计/审计/遗忘），由同一个后端服务持有写权限，避免 CLI 与运行中
   agent 双写互相覆盖。
3. **可解释**：每个状态变化（pin/disable/activate/delete/redistill）都有
   审计记录（谁、何时、为什么），前端可回查。
4. **管理动作语义化**：所有操作走生命周期/域规则，不直接改 JSON。

## 1.5 数据主权红线（用户 2026-09-05 声明，禁止违反）

管理应用只是 experience_codex 的**子应用/视图**：

1. **经验不是管理应用的私产**：它读写的是 `<codex_home>/experience/store.json`
   这一同一事实源；usage/trash 是相邻元数据文件，绝不迁移经验本体。
2. **不要求管理应用常驻**：agent 不依赖管理应用是否启动；管理服务空闲不
   持有锁，写前 reload 最新文件（乐观合并），绝不独占文件。
3. **管理修改必须可被 agent 使用**：所有写操作经 ExperienceStore 校验与
   ExperienceLifecycle 迁移，schema 与 agent 完全一致；编辑以“草稿副本 +
   采纳”进行，不破坏在产经验。

## 2. 前端：展示的问题（信息架构）

### 2.1 列表视图（默认页）

每行一个经验，展示：

- 标识：名称（可读）、kind（Reflex/Process/Reference/Result）、id；
- 状态：NEW/CANDIDATE/VALIDATED/ACTIVE/DECAYING/DISABLED + pinned 徽标；
- 信任：confidence、risk、applicability（scope/对象/parameterized）；
- 使用：last_used、命中次数、misfire/invalid 计数；
- 遗忘：按生命周期规则算出的“剩余可用天数/已过期”提示；
- 操作入口：查看详情、pin/unpin、activate/revalidate、disable、delete、
  重新蒸馏、导出。

过滤与搜索：按状态/kind/scope/适用对象过滤；按名称/关键词/触发面搜索；
排序（最近使用/置信度/创建时间）。

### 2.2 详情视图

- 完整字段：trigger（object/location/goal/keywords）、conditions、
  workflow（步骤 name/args）、completion_criteria、failure_modes、
  assets（可预览/下载）、applicability；
- 过程与证据：source 任务摘要、final_evidence（已入 criteria）、
  failure_modes 历史（misfire 模式）、最近 5 次使用结果（来自 usage）；
- 统计：hits/misfires/invalid/confidence 变化（若有历史点）；
- 操作：全部管理动作 + “复制为草稿编辑”（编辑字段 → 存为 CANDIDATE v+1）。

### 2.3 监控/观察视图

- 经验健康度面板：哪些在 DECAYING、哪些 misfire 频发、哪些 pin 中；
- 决策摘要：最近命中走了 ①/②/③/④ 的次数（usage 记录 decision 档位），
  用于回答“经验到底帮 LLM 省了多少”。

## 3. 后端：如何排布

```text
前端（管理页面 / TUI / 外部工具）
        │  JSON / stdio
        ▼
ExperienceManagementApi（命令+查询，DTO 层）
        │
        ▼
ExperienceManagerService（域逻辑：生命周期/统计/审计/redistill 编排）
        │
        ├── ExperienceStore（store.json：经验本体）
        └── ExperienceUsageStore（usage.json：hits/misfires/last_used/audit/decision 统计）
```

传输形态（三选一或组合，见 §6 待确认）：

- **App Server 端点**：codex app-server 增加 `experience.*` JSON 方法，
  与运行中的 SessionServices.experience_runtime **同进程同 Mutex** 访问，
  天然解决双写冲突——推荐；
- **stdio MCP**：已有雏形（experience_list/pin/unpin/disable），扩展为
  全量管理工具，面向外部 agent；
- **CLI/TUI**：离线管理（agent 未运行时直连 store.json），复用同一
  ManagerService，靠文件存在性检查提醒“agent 正在运行，建议用 API”。

写冲突策略：

1. agent 运行中 → 只允许同进程 API/MCP 写（经 runtime Mutex）；
2. agent 未运行 → CLI 可直写；
3. `persist` 前校验文件 mtime，若被外部改动则先 reload 再合并写
   （乐观并发，避免整文件覆盖）。

## 4. 管理逻辑（域规则）

### 4.1 状态操作（复用并扩展 ExperienceLifecycle）

| 操作 | 语义 | 允许从 |
|---|---|---|
| pin / unpin | 永不遗忘特权 / 撤销 | 任意（除 disabled 需先 activate） |
| activate | 手动启用 | VALIDATED / DECAYING |
| revalidate | 手动复审通过 | DISABLED → VALIDATED |
| disable | 停止参与匹配（pin 不豁免 disable） | 任意 |
| delete | 物理删除（先归档到 trash.json 可恢复） | 任意 |
| edit-as-draft | 复制当前内容为 CANDIDATE v+1 供编辑 | ACTIVE/VALIDATED |

### 4.2 重新蒸馏（redistill）

把“语义不佳/失效”的经验送回生成管线：以现 workflow + completion_criteria
+ failure_modes 为输入，跑一次 experience-gen 蒸馏（小 LLM 调用），产出
新版本 CANDIDATE；**不覆盖旧版本**，用户对比后决定采纳（采纳=bump 替换，
拒绝=丢弃候选）。旧版本在候选期间保持可用。

### 4.3 统计与遗忘

- `usage.json` 记录每次决策档位与结果：{ experience_id, decision: ①/②/③/④,
  outcome: success/misfire/invalid, at }；
- last_used / hits 由 runtime 写入（从内存计数器落盘），管理端只读；
- 遗忘预览：用 ExperienceLifecycle 常量（30 天 stale / 60 天 forget）计算
  每行剩余天数，前端提示“即将降级”而不自动改状态。

### 4.4 审计

每次管理写操作追加 { at, action, id, by: "user"/"cli"/"mcp", note }；
审计保存在 usage.json，前端详情页展示变更历史。

## 5. 实施阶段

- **P0 后端域服务与持久化**：ExperienceUsageStore + ManagerService
  （detail/activate/revalidate/delete/export/import/edit-as-draft），
  runtime 把 counters/last_used/decision 档位写入 usage.json；补单测。
  **状态：已完成主体（提交 `……`）**——`ExperienceUsageStore`
  （usage.json：命中/decision 档位/misfire/审计）+ `ExperienceManagementService`
  （list/detail/pin/unpin/activate/revalidate/disable/delete→trash/export/
  import/edit-as-draft/adopt）；DECAYING 可由用户激活/复审；写前 reload；
  单测 2/2，experience 全量 99/99。剩余：runtime 决策统计写入 usage.json
  （归入 P1）。
- **P1 API 面**：按待确认的形态接 App Server 或 CLI 子命令扩展；
- **P2 前端**：列表/详情/监控三视图（形态待确认）；
- **P3 增强**：redistill、编辑草稿对比、遗忘预览落地。

## 6. 待确认的关键决策

1. **前端形态**：A) 独立本地 Web 管理页（推荐：最可控，不依赖官方 UI）；
   B) 扩展 `codex experience` 为交互 TUI（最快）；C) 等官方
   app-server/Desktop 插件渠道成熟再嵌 UI。
2. **写路径**：同意“同进程 API（app-server/MCP）+ 离线 CLI + mtime 乐观
   合并”的三层策略吗？
3. **usage/审计持久化**：是否现在就引入 usage.json（影响 P0 范围），
   还是先只做管理操作、统计留到 P3？

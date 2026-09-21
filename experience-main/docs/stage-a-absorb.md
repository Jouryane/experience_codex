# Stage A：外部 agent 材料吸收（A1–A3 完成，A4 转交 Trae）

> 2026-09-10。本质：Experience 可被注入“某个 agent 在解决问题时、对最终
> 结果有用的成功过程与结论”；材料可来自 agent 自我声明（主动生产日志）
> 或工作区证据。红线不变：agent 声明只能成为**参考类经验**；只有工作区
> 可验证证据才能支撑可执行 candidate，且永不自动 ACTIVE。

## 1. 参考层数据模型（A1，store additive `references`）

```json
{
  "id": "ref_trae_...", "title": "任务/方法名",
  "scope": "scene-frontend", "tags": ["frontend"],
  "body": "run-notes markdown",
  "steps": ["脚手架", "装依赖", "改组件", "验证"],
  "tools_used": ["file-edit", "terminal"],
  "plugins_used": ["ui-kit"],
  "source_agent": "trae", "declared": true,
  "trust_level": "declared | workspace_verified",
  "evidence_summary": "git diff --stat ...", "created_at": 0
}
```

参考条目**永不进匹配/执行路径**；只在 injection policy_on 时作为“仅参考、
可质疑”的上下文注入，并带 `[ref:id][trust]` 标注。

## 2. 通道（A2）

| 端点 | 用途 |
|---|---|
| `GET /api/ingestion/guide?agent=trae` | 采集清单 + 可复制给 agent 的 run-notes 提示词模板 + workspace 步骤 |
| `POST /api/ingestion/package` | 提交材料包 → 落 reference（trust 判定：有 diff/verified_files ⇒ workspace_verified；否则 declared），ledger `ingested` |
| `GET /api/references?scope=&tag=` | 浏览参考条目 |
| `POST /api/references/{id}/promote` | **放行（不再要求 env 开关）**：用户点击即视为同意，用指定 agent / `EXPERIENCE_LLM_CODEX` / 自动发现的 codex 抽取可执行体 → CANDIDATE；返回补充说明（可能的问题点） |
| `POST /api/ingestion/parse` | **上传/粘贴 run-notes.md → 自动解析**（task/steps/tools/plugins/artifacts/evidence 建议）供 UI 自动填充，不落库 |
| `POST /api/ingestion/run-notes` | **一步式**：解析 run-notes.md → 直接落 reference + ledger（trust=declared，除非调用方另附 evidence） |

### run-notes.md 解析约定（自动填充）

- 标题 = `#` 一级标题（或首个非空行）；
- 章节按关键词识别（中英）：目标/约束、工具/插件、步骤/子步骤、失败/修正、
  产物/验证、模板/参数；
- 步骤支持有序/无序/嵌套 bullet，统一扁平为 step 列表（用户不再逐行填写）；
- 工具 vs 插件：含“插件/plugin”前缀的行归 plugins，其余归 tools；
- 产物/验证章节里的 `git diff`/`sha`/“验证”行会成为 evidence 建议；
- 无章节时降级：标题取首行，所有 bullet 行作为 steps。

### promote 放行口径与可能的问题点

- 用户点击 promote = 同意一次模型调用；**不需要**预先设置
  `EXPERIENCE_LLM_COMPILER`；
- 助手选择：body 传 `agent`（推荐）→ 否则 `EXPERIENCE_LLM_CODEX` →
  否则自动发现的 codex（此时返回额外 warning 建议显式指定）；
- 响应/错误均携带 `warnings`：
  1. 会调用一次助手模型（耗时/token）；
  2. 产物只到 CANDIDATE，仍需 L2 验证（可观察判据）才能激活；
  3. 材料缺少可复现步骤/可观察判据时可能抽取失败或停留在参考经验；
  4. 使用自动发现助手时会提示显式指定；
- 编译子进程 120s 超时（超时击杀，UI 不会挂住）；连可执行文件都找不到时
  返回结构化 `{code: "compiler_unavailable", warnings}`。

## 3. 注入接线（A3）

- 默认 `policy_off`（不注入）；
- `policy_on` 时：匹配 ACTIVE 经验摘要 + 当前 scope 可见 references 合并注入
  （各 600 字符上限），reference 行格式 `- [ref:id][trust] title: steps`；
- injection 审计记录 `refs=<count>;ids=<前3个>`，便于追溯“这次注入了哪些参考”。

## 4. A4（转交 Trae）：吸收向导页

Trae 实现 UI 时请对齐：

1. 引导区：调用 `/api/ingestion/guide` 展示采集清单 + 一键复制提示词模板；
2. 提交区：**上传/粘贴 run-notes.md → 调 `/api/ingestion/parse` 自动填充
   表单字段**（步骤/工具/插件/产物/evidence 建议），用户可微调后
   一步提交 `POST /api/ingestion/run-notes`（无需再逐行手填）；
3. 参考列表：`GET /api/references`（按 scope/tag 过滤）展示 title/trust/
   steps/plugins，`workspace_verified` 与 `declared` 用不同徽章；
4. “提升为候选”按钮：`POST /api/references/{id}/promote`，展示诚实拒绝
   原因（未启用 compiler / 抽不出可执行体）；
5. 验收交互：参考条目可见、注入策略开关状态可见、trust 标签不可混淆。

## 5. A5（验收，使用 Trae 的 A4 过程作为材料）

> 状态：✅ 材料已入库（2026-09-12）：`docs/run-notes.md`（Trae A4 过程）
> 提交入库，派生 fixture `tests/fixtures/a5/trae-a4-reference.json` +
> `a5_fixture.rs`；run-notes 解析/入库/参考注入链路此前已真机验证。

1. 让 Trae 在受控 git workspace 完成 A4 开发，并要求其输出 run-notes.md
   （可用 A4 页面的模板）；
2. `git diff --stat` + run-notes + 产物清单 → 通过 A4 页面/API 提交；
3. 期望落一条 `workspace_verified` reference（本次材料即“待留存的经验”）；
4. 若需可执行化：开启 `EXPERIENCE_LLM_COMPILER=on` +
   `EXPERIENCE_LLM_CODEX`，执行 promote → CANDIDATE → validate/activate；
   抽不出可验证判据时保持 reference（诚实拒绝）；
5. 回放：一次暖任务确认 experience_execution/delegate/注入审计形态。

真机预算：A5 最多 1 次暖任务（若只验 reference 入库则为 0）。

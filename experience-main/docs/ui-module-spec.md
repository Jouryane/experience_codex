# C3 UI 模块规格（转交 UI 专属专项）

> 2026-09-10。后端契约已就绪（C1 scope/树、C2 编辑、C2.1 别名、
> C2.2 用户偏好、C4 export/import/snapshot）。本文件只定义前端模块规格与
> 数据契约；实现由独立 UI 专项承接。

## 1. 页面/视图

1. **Tree（主视图）**：Root → Scene → Family → Experience；支持
   scope/status/usage_min 过滤与搜索；树是组织导航，不承担匹配语义。
2. **详情**：Experience 本体只读展示 + 状态/pinned/scope 徽章 +
   evidence score（usage.json）与 user score 并列 + 审计最近记录。
3. **编辑**：从任意非 draft 经验发起“草稿化”进入编辑页；仅 Draft 可
   编辑体；采纳/放弃草稿；原版本审计可见。
4. **场景选择**：经验挂载/移除 scope；会话/任务入口选择 scope（专项）。
5. **管理动作**：pin/unpin、状态转移（validate/activate/disable/
   revalidate）、删除——全部要求 reason/actor 输入并展示审计结果。

## 2. 数据契约（与后端一致）

### 经验摘要（GET /api/experiences?scope=&status=&usage_min=）

```json
{
  "name": "create_probe_a",
  "status": "active",
  "trigger_tool": "exec_command",
  "workflow_steps": 1,
  "postconditions": 2,
  "pinned": false,
  "scope": "scene-a",
  "display_name": "A 场景探针经验",
  "user_usage": "allow",
  "user_confidence": 0.85
}
```

### 详情（GET /api/experiences/{name}）

Experience 全字段 + `scope/display_name/user_usage/user_confidence`
（同摘要键；evidence 从 GET /api/usage 的
`entries[name].evidence/activity/score` 合并展示）。

### 树（GET /api/experience-tree?scope=&status=）

```json
{ "kind": "root", "name": "experience", "children": [
  { "kind": "scene", "name": "scene-a", "children": [
    { "kind": "family", "name": "create", "children": [
      { "kind": "experience", "name": "create_probe_a", "status": "active",
        "pinned": false, "scope": "scene-a" } ] } ] } ] }
```

### 编辑流

- `POST /{name}/draft`（body actor）→ 201；`{name}__draft` 出现
- `PUT /{draft}/body`（完整 Experience JSON）→ 200；仅 Draft
- `POST /{draft}/adopt`（body actor）→ 200；原经验身份/状态/scope/pin
  保留，draft 删除

### 场景/别名/偏好

- `POST /{name}/scope` body：`{scope|null, actor}`
- `POST /{name}/display_name` body：`{display_name|null, actor}`
- `POST /{name}/user-preference` body：`{usage, confidence|null, reason, actor}`

## 3. 交互规则

- 别名/用户评分是“个性化层”：UI 明确标注“仅我的视图/信任偏好，不改
  证据”；deny 显示“已从唤醒面移除”；
- 状态按钮按 domain 转移表显隐（VALIDATED 才有“激活”）；reason 必填
  的动作为 force_activate/disable/delete/adopt；
- 编辑入口：非 draft 显示“草稿化编辑”，draft 显示“编辑/采纳/放弃”；
- 树节点计数含 children 数量；hover 展示内部 name（身份）与 display_name。

## 4. 验收清单（UI 专项）

1. 树可展开/折叠，过滤结果与后端列表一致（同参数同集合）；
2. 草稿化→编辑→采纳全流程可用且 ledger 出现 drafted/edited/adopted；
3. scope/别名/偏好修改即时反映到摘要与详情；
4. deny 经验在入口 scope 视图可见但标注停用，不参与唤醒；
5. 所有写操作有 actor 输入并展示审计结果；删除有二次确认。

## 5. 边界

- 不实现后端逻辑；只消费 C1/C2/C4 HTTP 契约；
- 视觉/交互框架由 UI 专项自选（HTML/TS/组件库），契约为本文件 + swagger
  式示例即可；
- 具身/computer-use 通道不在 UI 展示（保留 API 通道）。

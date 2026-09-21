# 交付状态（2026-09-12）

## 结论

- **技术预览交付：符合**；核心闭环、四类经验利用、12 场景真机验证、
  多页面 UI、吸收链路与复跑矩阵齐备。
- **正式发布交付：符合**（P0/P1 缺口已在本轮关闭；P2 为已声明的设计边界）。

## 缺口关闭记录

| 缺口 | 状态 | 证据 |
|---|---|---|
| P0-1 UI A4 未入库/远端缺吸收页 | ✅ 关闭 | commit `596a0ad` 提交 5 页 UI（含 absorb）；GitHub main 已同步 |
| P0-2 U7 验收 | ✅ 关闭（自动化） | `accept-ui-u7.ps1` PASS：5 页 HTTP 200 + DOM 关键元素 + 动态数据 + 指南加载 |
| P1-1 A5 材料未入库 | ✅ 关闭 | `docs/run-notes.md` 入库；`fixtures/a5/trae-a4-reference.json` + `a5_fixture.rs` |
| P1-2 codex fork 分支未发布 | ✅ 免除 | 采用 DeepSeek 版本 Codex 本地复现 M5/L4，不要求发布 fork |
| P2 Trae 中间产物 | ✅ 清理 | `.gitignore` 忽略 `.trae-html-share-packages/`、`docs/frontend-redesign-plan.html` |
| P0 安全（后续增强） | ✅ 关闭 | P0 工作完成：工作区围栏 + 全链路脱敏（structure/HMAC/预览确认）；`accept-p0-security.ps1` PASS |

## 已声明的设计边界（非缺陷）

- 规模索引：设计稿 + 确定性基准，未实现运行时索引（Stage C5 规划）；
- Result 类经验暂无“返回文本/缓存结果”动作（当前以 write_file 或参考注入表达）；
- 参考注入相关性过滤未做（按 scope + 600 字符预算控制，默认 policy_off）；
- 多 Agent/模型 B 保持冻结。

## 复跑入口

- 场景：`scripts/accept-scenes-ab.ps1` / `-c.ps1` / `-d.ps1`
- UI：`scripts/accept-ui-u7.ps1`
- 吸收：`scripts/accept-a-absorb.ps1` / `accept-a-runnotes.ps1`
- 其它矩阵：docs/rerun-matrix.md

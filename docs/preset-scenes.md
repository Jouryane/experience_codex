# 预设经验 12 场景：利用流程与真机结果（2026-09-11）

目标：反向测试——先定经验，再看“是否被正确唤醒/协作/利用得当”。

## A 全自动（经验本地兑现，agent 收尾）— 3/3 PASS

| 场景 | 经验 | 任务 | 结果 |
|---|---|---|---|
| A1 | create_manifest | create project manifest…then stop | PASS：manifest.json 在委派前生成；exec=1/delegate=1 |
| A2 | create_editorconfig | create editorconfig…then stop | PASS：.editorconfig 精确匹配 |
| A3 | scaffold_skeleton | scaffold project skeleton…then stop | PASS：src/index.ts + docs/README.md 两步全成 |

## B 半自动（经验做已知段，agent 补剩余）— 3/3 PASS

| 场景 | 经验 | agent 动作 | 结果 |
|---|---|---|---|
| B1 | readme_skeleton | 在 README 加入 `ACMEUI: AcmeUI` | PASS（首轮 agent 只读未改 → 任务措辞改为显式编辑后通过） |
| B2 | gitignore_base | 追加 Python 规则（__pycache__/） | PASS |
| B3 | smoke_test_scaffold | 为 add() 补用例 | PASS |

## C 仅参考（policy_on 注入，agent 主导）— 3/3 PASS

| 场景 | 参考 | 产物断言 | 注入审计 |
|---|---|---|---|
| C1 | theme tokens(--bg/--bg-panel/--text/--accent) | theme.css 含四个 token | injected;refs=1 |
| C2 | error convention(AppError + retry once) | fetch.js 含 AppError/retry | injected;refs=1 |
| C3 | commit template(feat(scope): + Added/Changed) | commit.txt 含 feat( 与 Changed | injected;refs=1 |

对照：policy_off 下注入记录为 0（同任务仍可运行）。

## D 参与 agent 中间流程（内层 Gate 接管）— 3/3 PASS

| 场景 | 经验 | 判别证据 |
|---|---|---|
| D1 | create_probe_file | GATE HIT；probe.txt 由经验写；tool-ran.txt 不存在（原命令未 dispatch） |
| D2 | create_lock_marker | GATE HIT；lock-marker.json 由经验写；marker 不存在 |
| D3 | create_license_header | GATE HIT；LICENSE 由经验写；marker 不存在 |

## 本轮修复与观测

1. **纯参考会话此前不会注入**：`l3_entry` 只在实际存在 ACTIVE 经验时才写
   injection 审计；已改为“无 ACTIVE 但有可见 references + policy_on”时
   也注入并审计（仍不发 delegate 事件，保持 v1 纯委派语义）。
2. **参考段位置**：参考文本现追加在任务之后（任务在前，参考在后），避免
   模型把参考当成“要回答的内容”而不调用工具。
3. **验收脚本 bug**：C 脚本漏设 `DEEPSEEK_API_KEY`，导致会话空转；
   修复后 C 组 3/3 PASS。
4. **半自动的委派措辞**：B1 说明“勿重复执行 + 只完成剩余”需要给出
   明确的可执行剩余指令（例如编辑 README 加入某行），否则 agent 可能只确认。

产物：`scripts/accept-artifacts/scenes-ab-*`、`scenes-c-*`、`scenes-d-*`；
预设定义：`experiences/scenes/presets.json` + `experiences/scenes/references/`。

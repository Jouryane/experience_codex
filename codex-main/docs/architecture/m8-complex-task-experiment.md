# M8：复杂任务下的 Experience + LLM 分工实验

> 2026-09-21。目的：验证同一复杂任务在“无经验”和“已知前缀经验”两臂下，
> LLM 请求数如何变化，并检查经验表示是否能跨文件复用。

## 1. 任务设计

工作区：`m8-complex-workspace`

初始状态：`inbox/` 中有两份 PDF。

最终要求：

| # | 步骤 | 归属 |
|---|---|---|
| 1 | 创建 `docs/index.md`（精确内容） | Experience |
| 2 | 创建 `config/settings.json`（精确内容） | Experience |
| 3 | 移动 `inbox/2407.09450v3.pdf` → `library/` | Experience |
| 4 | 移动 `inbox/2603.07670v1.pdf` → `library/` | Experience |
| 5 | 执行 `git --version` / `python --version` → `env/versions.txt` | LLM |
| 6 | 执行 `git ls-remote github...` → `env/github.txt` | LLM（网络） |
| 7 | 计算四项 SHA256 → `manifest.json` | LLM |

逻辑步骤 `n=7`，Experience 覆盖 `m=4`，理论上剩余 LLM 逻辑步骤为 3。

## 2. 测量方法

使用 Codex 自带 `codex-responses-api-proxy`：

- 上游：`https://api.deepseek.com/v1/responses`
- dump：每个模型请求一对 request/response JSON
- 计数：`*-request.json` 数量

两臂使用同一 prompt、同一工作区初始状态、同一验收函数。

## 3. 结果

| arm | 模型请求数 | exit | 全部产物正确 |
|---|---|---|---|
| baseline（无经验） | **10** | 0 | 是 |
| experience（4 步前缀经验） | **6** | 0 | 是 |

净减少：**4 次模型请求**。

**复跑注记：** 2026-09-21 晚些复跑时，baseline 臂出现一次超过 38 次请求的
长尾且未在 240s 内收敛，实验被主动中止。这不影响首次 M8 成功运行的
“10 vs 6”原始记录，但说明该指标仍有很高方差；M8 的单次差值不能当作
稳定均值使用。

这个数字有意思：Experience 覆盖了 4 个逻辑步骤，模型请求也正好减少 4。
但它**不等于** `n-m=3`。原因是 baseline 和 experience 臂都包含探索、复核和
验证请求，实际请求数不是逻辑步骤数。

更准确的结论：

```text
Δ模型请求 = 被 Experience 移除的模型决策/验证往返
          ≠ 逻辑操作数 n-m
```

## 4. 三个重要问题的核验

### 4.1 经验存的是“移动这个文件”还是“移动文件的规则”？

**当前存的是已实例化动作，不是可参数化规则。**

`WorkflowStep.args` 是 concrete `serde_json::Value`；`ActionPattern` 只做
tool + substring 匹配，不捕获变量。因此 M8 store 里写死的是：

```text
inbox/2407.09450v3.pdf -> library/2407.09450v3.pdf
inbox/2603.07670v1.pdf -> library/2603.07670v1.pdf
```

它不能自动变成“移动任意指定 PDF”。要跨文件复用，需要新增：

1. trigger captures（从 task/action args 提取变量）；
2. workflow placeholder（如 `${source}` / `${target}`）；
3. 执行前变量绑定与校验；
4. provenance/适用域记录，防止错误泛化。

**2026-09-21 补记：** M9 已实现最小模板层（参数捕获 + 占位符绑定 +
绑定后实例化），并在同一模板上连续移动两个不同文件、两轮 0 LLM。当前
仍未实现结构化参数、条件分支和模板 binding 的 usage 审计。

### 4.2 任务已完成时为什么还做大量校验？

修复前的“大量校验”不是本地 probe，而是 **30 次 LLM 请求**：Gate 只有
Hit/Miss，第二次移动时前置不存在 → Miss → 任务交给 LLM 重新调查。

现在新增 `Satisfied`：

- `decide` 一次 probe 后置；
- 后置全部成立 → 成功 no-op；
- `state_after` 直接携带到结果，不重复 probe；
- 不执行 workflow，不唤醒 LLM。

M7 复验：重复移动任务 fake provider 请求 **0**，输出
`already satisfied; no action needed`。

### 4.3 复杂任务能否实现 `n-m`？

方向成立：Experience 覆盖的已知前缀确实从模型路径移除了。

但不能把“逻辑步骤数”直接当成“模型请求数”。模型仍会为剩余任务进行探索、
网络调用和结果验证；baseline 也会把多个操作批处理。因此应报告：

- `m`：被 Experience 执行的逻辑步骤数；
- `ΔR`：实际减少的模型请求数；
- 两者不保证相等，应按真实任务形状测量。

## 5. 产物

- `validate-m8-complex.ps1`
- `fixtures/m8/known-prefix-store.json`
- `m8-complex-artifacts/summary.json`
- `m8-complex-artifacts/baseline/proxy-dump/`（10 个请求）
- `m8-complex-artifacts/experience/proxy-dump/`（6 个请求）
- `m8-complex-artifacts/experience/store/usage.json`
- `m8-complex-artifacts/experience/backup/`（两个 PDF 的移动前备份）

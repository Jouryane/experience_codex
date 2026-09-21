# M7：桌面论文迁移现场逻辑校验

> 2026-09-21。目标：用真实桌面文件验证“冷启动 → 经验赋值 → 快速复用 →
> 重复重入”的完整业务链，并观察改造后的 Agent 控制流。

## 1. 冷启动：没有经验，走原生 LLM

- 对象：`20_Hierarchical_Episodic_Memor.pdf` → `papers-archive/`
- 配置：`EXPERIENCE_ENABLED=0`，真实 DeepSeek
- 结果：exit 0；源文件消失，目标文件存在；agent 先核对源/目标，再移动并复核
- 产物：`m7-live-artifacts/cold.stdout.txt` / `cold.stderr.txt`

这一轮证明：未命中经验时，改造后的 agent 仍然保留原生 LLM + Tool 能力。

## 2. 经验赋值

按冷启动观察到的成功路径建立 canonical ACTIVE 经验：

- name：`move_desktop_paper_2407`
- trigger：`task` + 包含 `2407.09450v3`
- workflow：`move_file(source=2407.09450v3.pdf, target=papers-archive/2407.09450v3.pdf)`
- 后置：源不存在 + 目标存在
- store 元数据：`pinned=true`、`user_confidence=1.0`

fixture：`fixtures/m7/move-2407-store.json`

## 3. 第一次复用：0 LLM

- 输入：`请把桌面论文 2407.09450v3 移动到 papers-archive 文件夹。`
- fake provider 请求数：**0**
- exit：0
- 文件：源消失、目标存在
- stdout：出现 `EXPERIENCE TASK GATE: completed ... without LLM`，并列出
  `move_file` 执行、证据、side effects、state_after
- usage：`hits += 1`，band `experience_only`

这一轮证明：完整已知任务可以在模型采样之前由 Experience 完成。

## 4. 第二次复用：重入缺口与修复

第二次输入相同任务，但源文件已经不存在、目标已经存在。

修复前：

- `decide()` 只检查前置，源不存在 → `Miss`
- 任务落回 LLM
- fake provider 被请求 **30 次**

修复后：

- 新增 `GateDecision::Satisfied`
- 前置不成立但全部后置成立 → 成功 no-op
- `decide` 观测到的 state facts 直接传给结果，不重复 probe
- fake provider 请求数：**0**
- exit：0
- stdout：`EXPERIENCE GATE: already satisfied; no action needed; original tool NOT dispatched`
- usage：再次 `hits += 1`，但不执行 workflow

这一轮证明：重复调用同一经验时，控制流不会因为“已经做完”而退化成 LLM
重新调查。

## 5. 业务流结论

```text
未知任务（20_Hierarchical）
  → LLM 探索 → Tool 执行 → 验证 → 记录成功路径

已知完整任务（2407，首次）
  → turn Gate 命中 → Experience 执行 → 验证 → 返回；LLM = 0

已知完整任务（2407，再次）
  → 前置不成立 + 后置成立 → Satisfied → 成功 no-op；LLM = 0

未知任务（无命中）
  → 原生 LLM loop（M6 unknown 基线）
```

因此新的 Agent 控制权不是“Experience 取代 LLM”，而是：

```text
LLM：未知路径的探索权
Experience：已验证转移的执行权
Gate：在副作用前决定这次控制权归谁
```

## 6. 产物

- `validate-m7-warm.ps1`
- `m7-live-artifacts/cold.stdout.txt` / `cold.stderr.txt`
- `m7-live-artifacts/warm1-final2.stdout.txt` / `warm1-final2.stderr.txt`
- `m7-live-artifacts/warm2-final2.stdout.txt` / `warm2-final2.stderr.txt`
- `m7-live-artifacts/store/usage.json`

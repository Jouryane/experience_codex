# DSH guard 席位：出处与原文

> 用途：供核对“DSH 已经承认存在一类决定模型不能覆盖，但只用在 deny”这一判断的
> 一手出处。内容为第三方（MIT）文档的**逐字摘录 + 定位**，不是复述。

## 定位

| 项 | 值 |
|---|---|
| 仓库 | `deepseek-ai/deepseek-harness` |
| 版本 | tag `dsh-v0.1.5-rc.2` |
| commit | `fb2c4b9e698e30edb738bca4cf0618587db7d203` |
| 抓取日期 | 2026-09-16 |
| 许可 | MIT |

> 注意：仓库处于 developer preview，官方明写会有破坏性变更
> （`README.md`: “THERE WILL BE COMPATIBILITY-BREAKING CHANGES.”）。
> 而且 `dsh-v0.1.6-alpha.1` 已存在。引用时务必带 tag。

## 1. 类型定义（源码）

`packages/core/tools/src/index.ts`，`ToolGuard` 类型上方注释原文：

```text
/**
 * A monotonic execution guard evaluated after every `tools/pre-execute`
 * listener and before the tool body. Returning a reason denies the call;
 * returning `undefined` leaves it unchanged. Because guards have no allow
 * result, listener ordering cannot turn a denial back into permission.
 * @param execution - the identity-protected call after extensible pre-execute policy completed.
 * @returns a final denial reason, or `undefined` to leave the call allowed.
 */
export type ToolGuard = (execution: Readonly<ToolExecution>) => string | undefined
```

## 2. 注册入口（源码）

同文件 `ToolRuntime.guard()` 方法注释原文：

```text
  /**
   * Register a monotonic guard after the extensible `tools/pre-execute`
   * waterfall. A plain-context guard applies globally; one registered through
   * `agent.ctx` applies only to that agent. Any matching guard may deny by
   * returning a reason, while no guard can force-allow a call another guard
   * denied. The exact effect disposer is returned for ordered ownership and
   * HMR cleanup.
   * @param guard - synchronous check; a returned string denies the execution.
   * @returns the exact disposer that unregisters the guard.
   */
  guard(guard: ToolGuard): () => void
```

同文件私有求值顺序：全局 guard 先求值，再按 scope 链“由远及近”求值，
第一个 denial 即生效。

## 3. 管线位置（文档）

`docs/subsystems/tools.md`，章节 “Execution: extensible waterfalls plus monotonic policy”：

```text
`ctx.tools.execute()` accepts a caller-owned `ToolExecutionInput` with a required
readonly `signal`, materializes its parsed JSON arguments once into a
pipeline-owned `ToolExecution`, and runs that call through `tools/pre-execute`
(the reorderable allow/deny/ask waterfall) → registered monotonic guards →
`tools/execute` (around-dispatch wrappers) → `tools/post-execute`
(inspect/replace the result) → optional definition-owned `finalizeContent` →
`tools/result` (the immutable authoritative outcome).
```

```text
A `ToolGuard` is scope-aware final pre-dispatch policy. Its return type
deliberately has no allow result: `undefined` preserves the waterfall decision,
while a returned reason can only reduce permission, so a later listener cannot
undo it.
```

## 4. 设计理由（Agent Note）

`.agents/notes/implemented/feature/2026-06-30-interception-extension-points.md`：

```text
- **`ctx.tools.guard()`** installs synchronous scope-aware policy after the whole
  pre-execute waterfall. A guard may deny or abstain, never force-allow, so
  listener ordering cannot resurrect an operation that a final invariant forbids.
```

## 5. 目前这个席位被用在哪儿

`packages/guard/README.md`：

```text
The `guard/` group keeps the agent loop productive by watching for two common
failure patterns. `repeat-tool-reminder` notices when the model repeats the exact
same tool call and reminds it to change approach or finish... `timeout-policy`
puts a time limit on tool calls that declare one...
```

即：**两个现役 guard 都是 loop 卫生，不是策略。**

## 6. 复核命令

```powershell
$tag = 'dsh-v0.1.5-rc.2'
$base = "https://raw.githubusercontent.com/deepseek-ai/deepseek-harness/$tag"

# guard 类型与注册入口
(Invoke-WebRequest "$base/packages/core/tools/src/index.ts" -UseBasicParsing).Content |
  Select-String -Pattern 'ToolGuard' -Context 12,2

# 管线位置
(Invoke-WebRequest "$base/docs/subsystems/tools.md" -UseBasicParsing).Content |
  Select-String -Pattern 'monotonic' -Context 3,3

# 设计理由
(Invoke-WebRequest "$base/.agents/notes/implemented/feature/2026-06-30-interception-extension-points.md" -UseBasicParsing).Content |
  Select-String -Pattern 'guard' -Context 2,2

# 现役用法
(Invoke-WebRequest "$base/packages/guard/README.md" -UseBasicParsing).Content
```

## 7. 相邻座位（同一管线的其他 extension point，供对照）

| 阶段 | 机制 | Experience 想做的事 |
|---|---|---|
| 判定 | `tools/pre-execute` waterfall（allow / deny / ask） | 命中判定 |
| 强制 | `ctx.tools.guard()`（单调 deny，无法 force-allow） | 策略在副作用前拒绝 |
| 询问 | `ctx.approval` seam | 不可逆动作二次确认 |
| 接管 | `tools/execute` waterfall（wrapper 可自行产出成功、短路 dispatch） | 命中后原 Tool 不 dispatch |
| 富化 | `tools/post-execute`（accept / block / replace / `additionalContexts`） | 参考注入、结果改写 |
| 观测 | `tools/result`（冻结的权威结果，监听器失败被隔离） | 学习回流证据 |

> “wrapper 自行产出成功即短路 dispatch”这一条来自
> `.agents/notes/implemented/feature/2026-06-30-interception-extension-points.md`
> 的正文表述（“a wrapper-authored success short-circuits dispatch and is
> re-normalized through the resolved output declaration”）。
> 它与 `tools/execute` 的 JSDoc（强调 “wrappers may change only `exec.signal`”）
> 措辞侧重不同，**属于需要契约探针钉住的一条**，不宜直接当作稳定承诺使用。

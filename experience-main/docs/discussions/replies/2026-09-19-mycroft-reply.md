# Reply draft · 2026-09-19

> 回复对象：TonyDzi / Mycroft（openai/codex 讨论区外审）。
> 状态：draft，可直接发送；如需要可在文末附实验脚本或汇总表。

```text
Thanks — this was unusually useful. Your type-level split held up in a small
controlled test, and it changed how we model the cost.

We built the composition you proposed on stock Codex (0.155.0-alpha.9): the
verified action lives in a dynamic tool, and PreToolUse acts as the router that
blocks the native call and names that tool in block_reason. Same isolated
CODEX_HOME, same task, same hook loaded in both arms:

- allow control (hook records but never blocks): native path completed,
  experience tool used 0/3
- block: hook blocked 3/3, the model read the reason, called the experience
  tool 3/3; all 6 runs correct

So "hook can veto and point, tool must execute and return the verified result"
is confirmed in practice, not just in the types. We also checked the cheaper
approval seam: it can decline, but its response has only a decision and no
reason field; a hint placed in a JSON-RPC error did not surface as model-visible
text. PreToolUse is the right router for this purpose.

Two things did not match the simple cost story:

1. We did not see a net +1 model turn at n=3. block vs allow model_requests
   medians were 4 vs 4; paired deltas were 0, +1, 0. Our interpretation is that
   the +1 is local: the blocked call consumes the request that would otherwise
   have executed natively, and the tool call arrives on the next request. But
   the tool also collapses a multi-step native path into one verified call, so
   net cost should depend on the length of the prefix being replaced. Have you
   measured that split in your fleet, or do you mostly see the local +1?

2. We did not see the negotiation/degeneration you warned about in three
   blocked runs. We did, however, produce one false-positive veto with a naive
   write matcher: a read-only verification command containing ">" was
   classified as a write. That made your governance point concrete. We are
   treating structured action classification as a requirement, not a
   refinement.

One boundary we hit that was not in your note: user hooks are discovered and
enabled but not executed until trusted. That is a product-level gate before
hook semantics even matter.

Where we would value your operational read: in high-veto regimes, what signal
tells you the model has started negotiating rather than doing the work —
command-shape drift, retry count, something else? And have you seen any
extension surface that avoids the extra model turn without a host change?

Our current claim is deliberately narrow: the router does not save cost by
default; it converts probabilistic pull into deterministic use, and its net
cost should scale with the length of the prefix it replaces. We are measuring
prevalence next.
```

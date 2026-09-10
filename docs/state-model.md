# State 模型（v2 约束）

## 1. 构成

State = 一组**可验证原子谓词** `(key, value, evidence, verified_at, ttl)`。

P1 注：facts 目前是 runtime 一次性观测（每次从零 probe，state_after 仅存
在 GateHitResult 返回值）；持久化 State 注册表（唯一写主：Experience
Runtime）属后续形态，P1 接受并在实现中不伪造持久化。

## 2. 粒度约束（v2 明确工程约束，黄灯项）

Predicate 的 value 必须达到**足以决定该 Transition 是否成立**的粒度。
“向量相同 ⇒ 状态等价”只在谓词粒度足够时成立。

反例：

```text
file:a exists = true        # 两个环境都满足，但 file:a 内容完全不同
```

正例（由 Transition 的前置/后置声明决定粒度）：

```text
file:pom.xml dependency(postgresql) = 42.7
file:pom.xml hash = H
```

原则：能区分“该转移是否可执行/已完成”的事实才成为谓词；不足粒度的
观测不得当作等价依据。evidence / verified_at / ttl 用于支持与保鲜，不能
替代粒度本身。

## 3. 更新与验证

- 更新来源：探测（probe）、Experience 兑现副作用、TTL/外部失效；
- State 唯一写主：Experience Runtime；
- State' 成立判据：后置谓词逐条验证为 true（含证据）。

## 4. 同一性

投影等价（相关谓词三值向量相同）为方向；叠加 §2 粒度约束后，向量相同
才可视为同一可复用 State。

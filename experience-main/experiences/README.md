# 手工 Experience 库（M2）

此目录存放 P1 人工写入、可直接加载进 Store 的 Experience 文件。每个文件
是一个单独的 Experience 对象；Store 的持久化信封（`schema_version` +
`experiences`）由运行时负责组装（见 `crates/experience-core/src/store.rs`）。

当前文件：

- `create_probe_file.json` — P1 验收用经验：命中 `exec_command` 且命令含
  “create probe file” 的动作，用 `write_file` 写 `probe.txt` 并验证内容，
  返回等价于工具执行的 Result。

约束（与 p1-development-plan.md v1.3 一致）：

- 只接受 schema 合法的经验（`Experience::schema_issues()` 为空）；
- `workflow` 必须是预编译步骤，直接映射真实 Tool capability；
- 只有 `status: active` 的经验进入匹配面索引；
- `undo` 在 P1 一律为 `unsupported`，失败策略为 `stop_and_report`。

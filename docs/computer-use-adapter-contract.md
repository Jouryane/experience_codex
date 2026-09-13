# Computer Use 适配器契约（预留通道，实现冻结）

> 状态：**契约开放，实现冻结**。`computer_use` 已在能力词表里登记为
> reserved capability（可声明、可配置、不可执行），本文定义“当你想接入
> 时应该满足什么”，而不是“我们现在就驱动桌面”。

## 0. 为什么冻结而不是现在做

1. **证据链要求高**：桌面操作的结果需要截图/可访问性树作为证据，判据面
   比文件/进程复杂一个量级；
2. **不可回滚**：点击、拖拽、提交表单都可能不可逆，与“工作区内可回滚”的
   默认安全模型冲突；
3. **维护成本**：分辨率、DPI、主题、窗口管理都会漂移，属于“每台机器都要
   重新校准”的能力；
4. **当前无阻塞**：S0–S4 的闭环（文件/进程/仓库/接口）已经覆盖可声明范围，
   computer-use 属于“通道保留、按需实现”。

因此我们**保留词表与策略位**，让使用者可以声明“我允许这条经验使用
computer_use”，但执行器返回明确的 `unsupported`（而不是静默跳过）。

## 1. 适配器接口（建议形状）

```text
trait ComputerUseAdapter {
    /// 声明它能做什么（应用名、动作类型、是否需要前台窗口）
    fn describe(&self) -> AdapterCapabilities;
    /// 解析请求：把经验步骤翻译成具体动作，不执行
    fn plan(&self, step: &ComputerUseStep) -> Result<ActionPlan, AdapterError>;
    /// 执行并返回证据
    fn run(&self, plan: &ActionPlan, policy: &CapabilityPolicy) -> Result<AdapterOutcome, AdapterError>;
}

ComputerUseStep {
    app: String,          // 目标应用（可执行名或窗口类）
    action: String,       // click | type | key | wait_for | screenshot
    target: Option<String>, // 语义选择器优先（可访问性路径），坐标是最后手段
    args: serde_json::Value,
}

AdapterOutcome {
    evidence: String,     // 人类可读描述
    artifacts: Vec<Artifact>, // 截图/可访问性树/文本片段，落盘并脱敏
    state_after: Vec<StateFact>, // 供判据使用（S3 注册表）
}
```

## 2. 与现有分层的关系

| 层 | 接入点 | 要求 |
|---|---|---|
| 能力策略 | `policy.computer_use`（新增族） | 默认 `deny`；开启需显式授权 |
| 执行器 | `experience-core::exec` 的适配器分支 | 与文件动作共用“策略 → 围栏 → 备份 → 执行”顺序 |
| 证据 | `StateFact` / 谓词注册表 | 截图哈希、可访问性节点值可作判据；坐标不可作判据 |
| L3 | 命中与委派语义不变 | 命中仍要先过 Gate 3/4/5 |

## 3. 强制要求（不接受放宽）

- **默认拒绝**：没有显式策略时绝不驱动输入设备；
- **语义选择器优先**：坐标必须由选择器解析而来，不能来自经验里的硬编码；
- **每一步都要证据**：动作前后至少一张截图（或其哈希）+ 目标节点的可访问性值；
- **不可逆动作二次确认**：提交/发送/删除/支付类动作需要用户确认（或策略显式放行）；
- **失败即停**：任一步失败立刻停止，不做“猜着继续”；
- **审计**：动作类型、目标选择器、证据路径、决策理由进 ledger（脱敏后）。

## 4. 判据建议

| 判据 | 证据 | 说明 |
|---|---|---|
| `ui.node_value:<path>` | 可访问性树 | 语义稳定，推荐 |
| `ui.screenshot_sha256:<name>` | 截图哈希 | 用于“界面已到达该状态” |
| `ui.text_present:<text>` | 可访问性树/OCR | 谨慎使用（OCR 有噪声） |
| `proc.focused_window:<app>` | 系统查询 | 轻量前提检查 |

坐标、像素差、OCR 片段都**不建议**作为唯一判据。

## 5. 移植清单

- [ ] 词表里登记为 reserved，且执行器对未实现动作返回 `unsupported`
- [ ] 策略族存在且默认 deny
- [ ] 证据落盘 + 脱敏 + 审计
- [ ] 不可逆动作有确认路径
- [ ] 至少一条端到端经验（前提 → 动作 → 判据）在真实环境跑通
- [ ] 失败归因能区分“环境（窗口没出现）”与“语义（选择器错了）”

# Experience App：launcher + API server + 磁盘 UI

> 2026-09-07。产品定位：**Experience 是本体，Agent（含 codex）是“外包
> executor”**。不通过改造/重建 codex 来交付产品；codex-main 的 fork 探针
> 与 M4 内嵌 Gate 属于“参考/验证资产”，不是产品路径。

> 2026-09-07（桌面化）：UI 升级为应用壳（侧栏：会话 / 经验 / Agents），
> 会话页真实委派 executor；`scripts/build-app.ps1` 产出
> `dist/Experience/`（Experience.exe + experience-server.exe + ui/）。

## 1. 形态（VE5 仅参考其启动逻辑）

```text
experience.exe（薄启动器，一次打包不再动）
        │ spawn + 随机端口 + ready 探测
        ▼
experience-server（本地 API 进程，代码在磁盘）
        │ 静态文件 + JSON API
        ▼
ui/www（HTML/JS/CSS，刷新即生效）
```

- 改前端：直接改 `ui/www/*`，浏览器刷新；
- 改后端：`cargo build -p experience-server`（或直接 `cargo run -p
  experience-server`）后重启；
- 启动器无需重打包——测试密集期的核心诉求。

## 2. 启动流程（launcher）

1. 探测 `EXPERIENCE_SERVER`，缺省取 exe 同目录 `experience-server(.exe)`；
2. 绑定 `127.0.0.1:0` 预留空闲端口；
3. 若端口已在服务 → 只开浏览器并退出（单实例）；
4. spawn server，注入 `EXPERIENCE_PORT` / `EXPERIENCE_HOME`；
5. TCP 轮询等待服务就绪（20s 上限）；
6. `cmd /C start` 打开默认浏览器；`--no-open` 供自动化测试；
7. server 退出后 launcher 退出。

## 3. server API（当前最小面）

- `GET  /api/health`
- `GET  /api/agents`（executor 列表：config + 连接状态）
- `GET  /api/agents/{id}`
- `POST /api/agents`（注册/配置 executor：`directory`（agent 应用目录，
  自动发现可执行文件）或 `executable`（覆盖）；codex_cli 仅支持 managed；
  不接收密钥/env——那是 agent 应用自身的配置边界）
- `POST /api/agents/{id}/connect`（managed 就绪检查：`<command> --version`，
  15s 超时，失败显式落 Error）
- `POST /api/agents/{id}/disconnect`（CLI executor：无常驻进程，任务时
  按需拉起）
- `DELETE /api/agents/{id}`
- `GET  /api/fs/roots`（本地盘符/根）
- `GET  /api/fs/browse?path=…`（列目录：子目录 + 可选启动文件
  .exe/.lnk/.cmd/.bat/.com——用户在 UI 里打开本地文件选择 agent）
- `GET  /api/sessions`（历史会话摘要）
- `POST /api/sessions`（`{"agent_id","task","cwd"?}`：agent 未就绪时自动
  执行 managed 连接检查；error 态拒绝委派——失败有声；后台线程运行
  `<command> exec --skip-git-repo-check`，stdin 喂任务；stdout/stderr 实时
  写入会话 output；agent `channel=session` 时改走真实 SessionHost：
  app-server → thread/start → queue → 事件 trace 实时写回）
- `GET  /api/sessions/{id}`（含完整输出，UI 轮询至终态）
- `POST /api/sessions/{id}/cancel`（终止正在运行的 executor 并标记
  “已取消”，绝不静默放弃）
- `POST /api/sessions/{id}/resume`（session-channel：同一 threadId 追加
  一轮任务；trace 延续、历史不回滚）
- `GET  /api/experiences`（摘要列表）
- `GET  /api/experiences/{name}`（完整详情）
- `POST /api/experiences`（注入一条 schema 合法的 Experience）
- `PATCH /api/experiences/{name}/status`（`{"status":"active|draft|disabled"}`）
- `DELETE /api/experiences/{name}`
- 其余路径：从 `ui/www` 提供静态文件（路径穿越已拦截）

数据落在 `<home>/store.json`（P1 信封格式）。home 由 `EXPERIENCE_HOME`
或 `--home` 指定，缺省为 exe 上一级目录的 `.experience-home`。

Agent 配置落在 `<home>/agents.json`；状态机
`configured → checking → ready / error / disconnected` 在 UI 常驻可见
（失败有声：probe 失败时 message 携带确切原因，委派不得在 error 态进行）。

会话运行保障（“断路”免疫）：

- UI 实时显示运行秒数、已输出字符数与输出尾部；
- 可随时“终止执行”（kill executor → 会话标记 error“已取消”）；
- **不设自动超时**：真实 codex 任务常远超任何墙钟限制，终态只有“进程
  真实退出”或“用户主动终止”；仅显式设置
  `EXPERIENCE_SESSION_TIMEOUT_SECS` 才启用 opt-in kill；
- 应用重启时把遗留 “running” 会话标记为“上次运行中断”，杜绝永远转圈。

委派 = Experience 的一等操作（core-principle §3.5）：携带剩余任务、
workspace（项目根）、可选参考经验；写权限/审批/密钥由 executor 自身
配置决定，Experience 不设沙箱。

## 5. 打包（dist）与迭代

```powershell
powershell -ExecutionPolicy Bypass -File scripts/build-app.ps1
# 产物：dist/Experience/{Experience.exe, experience-server.exe, ui/, README.txt}
```

- `Experience.exe` 是薄启动器（预留端口 + 单实例 .port 文件 + 拉起 server
  + 打开浏览器）；启动逻辑不变则无需重打包；
- 后端逻辑改动：`cargo build -p experience-server` 后替换
  dist 中的 exe；
- 前端改动：直接编辑 `ui/www/` 或 `dist/Experience/ui/`，浏览器刷新。

会话委派真实调用 codex exec：需要 executor 可访问其模型 API（如 DeepSeek
密钥/登录态在 agent 应用内已配置，Experience 不读取）。无网络/agent 未
配置模型时，会话以 error 态结束并显示原因——这是有意的失败有声。

Agent 定位与 label：

- Agent 配置可指向 **agent 应用目录**（Experience 按 kind 发现可执行文件）
  或用户在**本地文件浏览**中选择的 .exe / .lnk（快捷方式会解析为目标
  exe；带启动参数的快捷方式会显式报错，提示直接选目标 exe）；
- 默认种子只在本机真实安装中查找（官方安装目录 / PATH），绝不猜测；
- label 只是显示名（列表识别），与模型无关；模型/密钥是 agent 应用内部
  属性。

## 4. 边界（与既有决策一致）

- Executor 连接语义（托管/附着、“不静默 ≠ Gate 阻塞”）见
  compatibility.md §1.1；
- Agent 配置只指向“在哪、拉哪个 agent 应用”（directory/executable）；
  密钥/登录/模型由 agent 应用自持（compatibility.md §4）——Experience
  不采集密钥、不把填密钥当作功能；
- 本 app 只管理 store / 后续会话委派，不私藏经验、不要求只有本 app 启动
  才能用经验（经验文件即 store.json）。

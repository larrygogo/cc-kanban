# 后台安装 agent + 进度 + 自动刷新 — 设计

日期：2026-07-07
分支：feat/new-session-20260706
状态：已通过头脑风暴，待实现

## 背景与动机

账号页（About「模型」）每张 agent 卡在未安装时提供「安装」按钮。现有实现（`install_agent`
→ `spawn_install`）会**弹出一个终端窗口**在里面跑官方安装脚本（`irm … | iex` /
`curl … | bash`）。本次改为：

- **后台安装，不弹终端窗口**；
- 在卡片内**就地显示进度**（转圈 + 最新步骤行）；
- 安装成功后**自动重查检测**，卡片直接变「已装/已登录」。

### 明确不做重启

先前设想「装完需重启 meowo-app 才生效」。经核实已不成立：检测（`CodexAgent::is_installed` 等）与
启动会话（`launch_args`/`codex_launch_prefix`）均走 agent 的**绝对路径**，安装脚本落盘后立即可
识别；且 `ccsetup::apply` 启动时只给 Claude Code 配 hooks、与新装 agent 无关。故**去掉一切重启
提示与一键重启**，装完自动刷新即可。

## 目标 / 非目标

**目标**
- claude / codex / kimi 三家统一的后台安装体验。
- 后台跑安装脚本，实时把有效步骤行推送到对应卡片。
- 成功→自动刷新检测转「已装」；失败→显示错误 + 「重试」。
- 跨平台同构（Windows 用 pwsh/powershell，macOS 用 bash）。

**非目标（YAGNI）**
- 安装中「取消」按钮（后台安装通常 10–30s，加取消是额外复杂度）。
- 跨设置面板的实时进度续传（中途关面板→重开靠重查检测显示最终态，不重放进度）。
- 任何重启相关 UI。

## 架构总览

```
[安装按钮] ── invoke install_agent(provider)
                    │
   Rust: 写临时脚本(pwsh 优先, 复用 -File/临时文件逻辑)
                    │ 后台 spawn: CREATE_NO_WINDOW + stdout/stderr 管道 + stdin=null(非交互)
                    │
后台线程逐行读输出 ──emit──▶ "install-progress" {provider, line} → 卡片: ⟳ + 最新 ==> 行
                    │
        进程退出 ──emit──▶ "install-done" {provider, ok, code}
                    │
              ok=true ──▶ 前端重查 availableAgents → 卡片变「已装/已登录」
              ok=false ─▶ 卡片显示错误行 + 「重试」按钮
```

后台**不弹窗口** → 不用 wt、不碰 Win11 DefTerm 交接（那套只服务于「给用户看窗口」）。但保留两点：
**pwsh 优先**（避 PSModulePath 被 PS7 污染导致 5.1 丢 `Get-FileHash`）与**临时脚本文件**（避引号/管道
在命令行被破坏）。

## 后端设计（`app/src-tauri/src/lib.rs`）

### `install_agent` 命令（重写）

签名保持前端可调用形态，新增 `app` 句柄用于 emit：

```rust
#[tauri::command]
async fn install_agent(app: tauri::AppHandle, provider: String) -> Result<(), String>
```

流程：
1. 解析 `ProviderKey` → `agent::for_provider(key).install_script(cfg!(windows))`，无脚本则返回 Err
   （沿用现有「该 agent 没有可用的一键安装命令」）。
2. 写临时脚本到 `%TEMP%/meowo-install-<provider>.ps1`（Windows）/ `.../…-<provider>.sh`（macOS），
   内容为一层薄包装：`try { <script> } catch { 输出 "Installation failed: …" 一行 }`。
   按 provider 命名，允许不同 provider 并行安装、互不覆盖临时文件。
3. 在 blocking 线程里 spawn shell 跑该脚本：
   - Windows：`<shell> -NoProfile -ExecutionPolicy Bypass -File <ps1>`，
     `shell = if pwsh_available() { "pwsh" } else { "powershell" }`（复用已有 `pwsh_available`）。
     `creation_flags(CREATE_NO_WINDOW=0x0800_0000)`。
   - macOS：`bash <sh>`。
   - 三端统一：`stdin(Stdio::null())`、`stdout(Stdio::piped())`、`stderr(Stdio::piped())`，
     env 追加 `CODEX_NON_INTERACTIVE=1`（belt-and-suspenders；脚本本也靠 `IsInputRedirected` 判非交互）。
4. spawn 成功后起后台线程：逐行读 stdout+stderr，对每行调 `is_progress_line` 过滤，命中就
   `app.emit("install-progress", InstallProgress { provider, line })`。
5. 进程 `wait()` 结束：`app.emit("install-done", InstallDone { provider, ok: status.success(), code })`。
6. 命令本身在 spawn 成功后即 `Ok(())` 返回（不 await 整个安装）；spawn 失败返回 Err（前端立即显示错误）。

### 纯函数 `is_progress_line`（新增，可单测）

```rust
/// 判定安装脚本输出的一行是否是「有效步骤行」，用于过滤 CLIXML/进度噪声后推给卡片。
fn is_progress_line(line: &str) -> bool
```

规则（保守白名单）：
- `trim` 后以 `==>` 开头 → true（各家脚本的步骤前缀）；
- 以 `Installing`（含首行 `Installing, please wait...`）开头 → true；
- 以 `Installation failed:` 开头 → true（失败信息，需展示）；
- 以 `#< CLIXML` 开头、或含 `<Objs `/`</Objs>`（PowerShell 序列化进度噪声）→ false；
- 空行 → false；
- 其余 → false（默认不展示，宁缺毋滥，避免噪声刷屏）。

### 事件与 payload

- `install-progress`：`{ provider: string, line: string }`
- `install-done`：`{ provider: string, ok: bool, code: i32 | null }`

（事件名用现有 kebab-case 约定，同 `board-changed`/`settings-changed`。）

### 删除

- 旧的开窗 `spawn_install`（Windows/macOS/其它三份）不再需要，删除。
- 相关「装完在窗口重新聚焦/手动刷新时重检」注释同步更新。

## 前端设计（`app/src/views/About.tsx` 的 `ProviderCard`）

### 卡片状态机

`idle → installing → (installed | error)`，用一个本地 `installState` 表示。

- **idle（未装）**：现状——显示「安装」按钮 + 安装提示文案。
- **installing**：按钮位替换为 `⟳ + 最新步骤行`（转圈动画 + `step` 文本）。
- **installed**：不单独渲染——安装成功后调用 `onRefresh()` 重查 `availableAgents`，父层 `installed`
  集合更新，卡片按已有三态（已装/未登录/已登录）自然重渲染。
- **error**：显示最后一条错误行（红）+ 「重试」按钮（重跑 `installAgent`）。

### 事件订阅

每张 `ProviderCard` 各自在 `useEffect` 里 `listen`（多卡监听同一全局事件、各按自己的 `provider`
过滤，简单且互不耦合）：
- `install-progress` → 若 `payload.provider === provider` 则 `setStep(payload.line)`。
- `install-done` → 若本 provider：`ok` 则 `setInstallState("idle")` 并 `onRefresh()`；否则
  `setInstallState("error")`（错误文案取最近一条 `Installation failed:` 行，缺则通用文案）。

卸载时取消 listen（`un.then(f => f())` 现有约定）。

### api（`app/src/api.ts`）

`installAgent(provider)` 签名不变（`invoke("install_agent", { provider })`）；事件类型可加轻量
`InstallProgress`/`InstallDone` type 便于前端消费。

### i18n

新增文案键（en/zh）：安装中、重试、安装失败通用文案。步骤行文本直接透传脚本英文输出，不翻译。

## 错误处理与边界

- **spawn 失败**（如 pwsh/powershell/bash 缺失）：`install_agent` 返回 Err → 卡片直接进 error。
- **脚本内失败**：薄包装 try/catch 输出 `Installation failed: …` 行 → `is_progress_line` 命中展示 →
  进程非零退出 → `install-done{ok:false}` → error 态。
- **非交互挂起**：`stdin=null` + `CODEX_NON_INTERACTIVE=1`，脚本 `IsInputRedirected` 判定为非交互、
  自动跳过 Y/N 提示。
- **并行安装**：临时脚本按 provider 命名、事件按 provider 过滤，三家可并行互不干扰。
- **中途关面板**：Rust 后台线程继续；重开面板 `availableAgents` 重查显示最终态（不重放进度）。

## 测试

- **Rust 单测**：`is_progress_line` — 覆盖 `==>` 步骤、`Installing…`、`Installation failed:`、
  `#< CLIXML`/`<Objs>` 噪声、空行、普通噪声行。
- **前端测试**（仿 `About.account.test.tsx`）：mock `installAgent` 与 `@tauri-apps/api/event` 的
  `listen`，驱动 progress/done 事件，断言卡片 installing（转圈+步骤行）/ error（错误+重试）/ 成功后
  触发 `onRefresh` 四条路径。
- **手动验证**：真实点 codex 安装，观察卡片转圈→步骤行→装完自动变「已装」，且**不弹终端窗口**。

## 影响文件

- `app/src-tauri/src/lib.rs`：重写 `install_agent`、新增 `is_progress_line`、删旧 `spawn_install`、
  两个事件 payload struct。
- `app/src/views/About.tsx`：`ProviderCard` 状态机 + 事件订阅 + 重试。
- `app/src/api.ts`：事件 type（可选）。
- `app/src/i18n/en.ts`、`zh.ts`：新增文案键。
- 测试：`lib.rs` 单测；`About.*.test.tsx`。

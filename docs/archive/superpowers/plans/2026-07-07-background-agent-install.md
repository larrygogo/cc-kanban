# 后台安装 agent + 进度 + 自动刷新 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把账号页 agent 一键安装从「弹终端窗口」改为「后台跑、卡片内显示进度、装完自动刷新检测」。

**Architecture:** 后端 `install_agent` 命令后台 spawn 安装脚本（不弹窗口、管道捕获输出、非交互 stdin），逐行把有效步骤 `emit("install-progress")`，退出时 `emit("install-done")`；前端 `ProviderCard` 订阅这两个事件，显示「转圈 + 最新步骤行」，成功后重查 `availableAgents` 令卡片转「已装」，失败显示错误 + 重试。

**Tech Stack:** Rust + Tauri 2（`app.emit`、`spawn_blocking`、`std::process`）；React + TypeScript（`@tauri-apps/api/event` `listen`）；vitest + @testing-library/react；cargo test。

## Global Constraints

- 代码注释、commit message 用中文；代码本身英文（用户规则）。
- Rust：错误串沿用现有中文风格；事件名用现有 kebab-case 约定（同 `board-changed`/`settings-changed`）。
- 平台：主目标 Windows；macOS 同构（`bash` 后台跑），macOS 编译靠 CI，本地不验证。
- pwsh 优先于 powershell（避 PSModulePath 被 PS7 污染丢 `Get-FileHash`）——复用已有 `pwsh_available()`。
- 不做重启相关任何 UI；不做「取消」；不做跨面板进度续传（YAGNI）。
- 前端测试文件已有 `vi.hoisted` 提升约定（避免 TDZ），新 mock 沿用。

---

## 文件结构

- `app/src-tauri/src/lib.rs`
  - 新增纯函数 `is_progress_line`（+ 单测）。
  - 新增事件 payload struct `InstallProgress` / `InstallDone`。
  - 新增 cfg 助手 `write_install_script` / `build_install_command`。
  - 重写 `install_agent` 命令（后台 spawn + 读输出 + emit）。
  - 删除旧的 `spawn_install`（Windows/macOS/其它三份）及其常量。
- `app/src-tauri/src/macos/terminal.rs`
  - 删除 `run_install_mac`（删 spawn_install 后仅它无引用；macOS-only，CI 验证）。
- `app/src/api.ts`：新增 `InstallProgress` / `InstallDone` TS 类型。
- `app/src/i18n/zh.ts`、`app/src/i18n/en.ts`：account 段新增 `installing` / `installRetry` / `installFailed`。
- `app/src/styles.css`：新增 `.agent-install-progress` / `.agent-install-step` / `.agent-install-error` 样式（转圈复用已有 `RefreshIcon spinning`）。
- `app/src/views/About.tsx`：`ProviderCard` 状态机 + 事件订阅 + 重试；`AccountSection` 抽 `refreshInstalled` 并作为 `onInstalled` 传入。
- `app/src/views/About.account.test.tsx`：新增 mock `@tauri-apps/api/event` + 三条事件路径测试。

---

## Task 1: 后端 `is_progress_line` 纯函数 + 单测

把安装脚本输出过滤成「有效步骤行」的判定抽成纯函数，先 TDD。

**Files:**
- Modify: `app/src-tauri/src/lib.rs`（在 `install_agent` 上方新增函数；在文件末尾 `#[cfg(test)] mod tests` 内加测试并把函数加入 `use super::{…}`）

**Interfaces:**
- Produces: `fn is_progress_line(line: &str) -> bool`（供 Task 2 的读输出线程调用过滤）。

- [ ] **Step 1: 写失败测试**

在 `app/src-tauri/src/lib.rs` 末尾的 `#[cfg(test)] mod tests { … }` 内追加：

```rust
    #[test]
    fn is_progress_line_keeps_steps_filters_noise() {
        // 有效步骤/失败行放行
        assert!(is_progress_line("==> Installing Codex CLI"));
        assert!(is_progress_line("  ==> Downloading Codex CLI")); // 前导空白也算
        assert!(is_progress_line("Installing, please wait..."));
        assert!(is_progress_line("Installation failed: something broke"));
        // 噪声/空行滤掉
        assert!(!is_progress_line(""));
        assert!(!is_progress_line("   "));
        assert!(!is_progress_line("#< CLIXML"));
        assert!(!is_progress_line(
            "<Objs Version=\"1.1.0.1\" xmlns=\"http://schemas.microsoft.com/powershell/2004/04\">"
        ));
        assert!(!is_progress_line("random chatter"));
        assert!(!is_progress_line("PS C:\\Users\\larry>"));
    }
```

并把 `is_progress_line` 加入该模块顶部的 `use super::{…}` 导入列表（与 `path_has_exe` 等并列）。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p meowo-app is_progress_line_keeps_steps_filters_noise`
Expected: 编译失败 `cannot find function is_progress_line`。

- [ ] **Step 3: 实现函数**

在 `app/src-tauri/src/lib.rs` 中 `async fn install_agent` 定义**上方**新增：

```rust
/// 判定安装脚本输出的一行是否是「有效步骤行」，用于过滤噪声后推给账号页卡片。
/// 保守白名单：只放行明确的步骤/失败行；PowerShell 把进度/流对象序列化成的 CLIXML
/// （`#< CLIXML` / `<Objs …>`）与空行一律不展示，避免刷屏。
fn is_progress_line(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return false;
    }
    if t.starts_with("#< CLIXML") || t.contains("<Objs ") || t.contains("</Objs>") {
        return false;
    }
    t.starts_with("==>") || t.starts_with("Installing") || t.starts_with("Installation failed:")
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p meowo-app is_progress_line_keeps_steps_filters_noise`
Expected: PASS。

- [ ] **Step 5: 提交**

```bash
git add app/src-tauri/src/lib.rs
git commit -m "feat(agent-install): 抽 is_progress_line 过滤安装输出噪声(纯函数+单测)"
```

---

## Task 2: 后端 `install_agent` 改后台安装 + 事件

重写命令为后台 spawn、管道读输出、emit 进度/结果；删旧的开窗逻辑。此任务无自动化测试（副作用重），闸口为编译 + clippy 通过 + 手动冒烟。

**Files:**
- Modify: `app/src-tauri/src/lib.rs`（新增 payload struct 与 cfg 助手；重写 `install_agent`；删除三份 `spawn_install` 及 `CREATE_NEW_CONSOLE` 常量）
- Modify: `app/src-tauri/src/macos/terminal.rs`（删除 `run_install_mac`）

**Interfaces:**
- Consumes: `is_progress_line`（Task 1）；已有 `pwsh_available()`；`meowo_reporter::agent::for_provider(key).install_script(windows)`。
- Produces: Tauri 事件
  - `install-progress` payload `{ provider: String, line: String }`
  - `install-done` payload `{ provider: String, ok: bool, code: Option<i32> }`
  - 命令签名 `async fn install_agent(app: tauri::AppHandle, provider: String) -> Result<(), String>`（前端仍 `invoke("install_agent", { provider })`，`app` 由 Tauri 注入）。

- [ ] **Step 1: 新增事件 payload struct**

在 `app/src-tauri/src/lib.rs` 中 `install_agent` 上方（`is_progress_line` 附近）新增：

```rust
/// 后台安装的进度事件：一行有效步骤文本，按 provider 分发给对应卡片。
#[derive(Clone, serde::Serialize)]
struct InstallProgress {
    provider: String,
    line: String,
}

/// 后台安装结束事件：ok=true 表示进程 0 退出；code 为退出码（无法取得时 None）。
#[derive(Clone, serde::Serialize)]
struct InstallDone {
    provider: String,
    ok: bool,
    code: Option<i32>,
}
```

- [ ] **Step 2: 新增 cfg 助手 `write_install_script` / `build_install_command`**

在同文件（`install_agent` 上方）新增。Windows 版包一层 try/catch，出错时打印统一失败行**并 `exit 1`**（否则 `-File` 吞掉异常后仍 0 退出、被误判成功）；bash 版子 shell 失败时打印并透传退出码：

```rust
/// 把安装脚本写进临时文件（按 provider 命名，允许并行安装互不覆盖），返回其路径。
/// Windows：try/catch 捕获终止错误，打印 `Installation failed: …` 并 exit 1。
#[cfg(target_os = "windows")]
fn write_install_script(provider: &str, script: &str) -> std::io::Result<String> {
    let body = format!(
        "Write-Host 'Installing, please wait...'\r\n\
         try {{ {script} }} catch {{ Write-Host ('Installation failed: ' + $_.ToString()); exit 1 }}\r\n"
    );
    let p = std::env::temp_dir().join(format!("meowo-install-{provider}.ps1"));
    std::fs::write(&p, body)?;
    Ok(p.to_string_lossy().into_owned())
}

/// macOS/Linux：子 shell 跑安装串，失败打印统一行并以原退出码退出。
#[cfg(not(target_os = "windows"))]
fn write_install_script(provider: &str, script: &str) -> std::io::Result<String> {
    let body = format!(
        "echo 'Installing, please wait...'\n\
         ( {script} ) || {{ rc=$?; echo \"Installation failed: exit code $rc\"; exit $rc; }}\n"
    );
    let p = std::env::temp_dir().join(format!("meowo-install-{provider}.sh"));
    std::fs::write(&p, body)?;
    Ok(p.to_string_lossy().into_owned())
}

/// 构造后台安装子进程（不弹窗口）。平台差异只在此：Windows 用 pwsh(优先)/powershell + CREATE_NO_WINDOW，
/// 其它平台用 bash。stdin/stdout/stderr 由调用方统一设。
#[cfg(target_os = "windows")]
fn build_install_command(script_path: &str) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let shell = if pwsh_available() { "pwsh" } else { "powershell" };
    let mut c = std::process::Command::new(shell);
    c.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", script_path])
        .creation_flags(CREATE_NO_WINDOW);
    c
}

#[cfg(not(target_os = "windows"))]
fn build_install_command(script_path: &str) -> std::process::Command {
    let mut c = std::process::Command::new("bash");
    c.arg(script_path);
    c
}

/// 过滤噪声后把一行安装输出作为进度事件 emit（stdout/stderr 两处读取共用，避免重复逻辑）。
fn emit_install_progress(app: &tauri::AppHandle, provider: &str, line: String) {
    use tauri::Emitter;
    if is_progress_line(&line) {
        let _ = app.emit(
            "install-progress",
            InstallProgress { provider: provider.to_string(), line },
        );
    }
}
```

- [ ] **Step 3: 重写 `install_agent`**

把现有 `async fn install_agent(provider: String) -> Result<(), String> { … }`（含其上方 `///` 注释）整段替换为：

```rust
/// 一键安装某 agent：后台跑其官方安装脚本（不弹终端窗口），逐行把有效步骤 emit 给账号页卡片，
/// 装完 emit install-done、前端重查检测转「已装」。安装命令是受信硬编码串（Agent::install_script），非用户输入。
#[tauri::command]
async fn install_agent(app: tauri::AppHandle, provider: String) -> Result<(), String> {
    let key = meowo_store::ProviderKey::parse(Some(&provider));
    let script = meowo_reporter::agent::for_provider(key)
        .install_script(cfg!(target_os = "windows"))
        .ok_or("该 agent 没有可用的一键安装命令")?;
    let path = write_install_script(&provider, &script).map_err(|e| e.to_string())?;

    // spawn 放 blocking 线程：GUI 进程首次 spawn 子进程可能被杀软扫描拖慢，勿堵事件循环。
    // spawn 成功即返回 Ok；进度/结果全走事件。spawn 失败回传 Err，前端立即显示错误。
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        use std::process::Stdio;
        let mut child = build_install_command(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("CODEX_NON_INTERACTIVE", "1")
            .spawn()
            .map_err(|e| format!("启动安装失败：{e}"))?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        // 读输出 + 等退出 + emit done 放独立线程，让 spawn_blocking 尽快归还线程池。
        std::thread::spawn(move || {
            use std::io::{BufRead, BufReader};
            use tauri::Emitter; // 供本闭包末尾的 install-done emit（install-progress 走 emit_install_progress helper）
            // stderr 另起线程并行读，避免两个管道任一写满后互相阻塞。
            let err_handle = stderr.map(|e| {
                let app = app.clone();
                let provider = provider.clone();
                std::thread::spawn(move || {
                    for line in BufReader::new(e).lines().map_while(Result::ok) {
                        emit_install_progress(&app, &provider, line);
                    }
                })
            });
            if let Some(o) = stdout {
                for line in BufReader::new(o).lines().map_while(Result::ok) {
                    emit_install_progress(&app, &provider, line);
                }
            }
            if let Some(h) = err_handle {
                let _ = h.join();
            }
            let code = child.wait().ok().and_then(|s| s.code());
            let _ = app.emit(
                "install-done",
                InstallDone { provider, ok: code == Some(0), code },
            );
        });
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}
```

注：`app.emit` 需 `tauri::Emitter` trait；本仓是**函数级局部 use**（非文件顶部）。install-progress 走 `emit_install_progress` helper（内含 `use tauri::Emitter;`）；install-done 的 emit 在线程闭包里，故该闭包首行也有 `use tauri::Emitter;`。无需改文件顶部导入。

- [ ] **Step 4: 删除旧 `spawn_install` 三份 + 常量**

删除 `app/src-tauri/src/lib.rs` 中：
- `#[cfg(target_os = "windows")] fn spawn_install(script: &str, _terminal: &str) -> bool { … }` 整段（含其上方长注释与 `CREATE_NEW_CONSOLE` 常量）。
- `#[cfg(target_os = "macos")] fn spawn_install(script: &str, terminal: &str) -> bool { … }` 整段。
- `#[cfg(not(any(target_os = "windows", target_os = "macos")))] fn spawn_install(_script: &str, _terminal: &str) -> bool { false }` 整段。

- [ ] **Step 5: 删除 macOS `run_install_mac`**

删除 `app/src-tauri/src/macos/terminal.rs` 中 `pub fn run_install_mac(cmd: &str, kind: TermKind) -> bool { … }` 整段（删 spawn_install 后已无引用）。若 `TermKind` 因此在该文件变为未使用，保留（别处仍用）——只删函数本体。

- [ ] **Step 6: 编译检查**

Run: `cargo check -p meowo-app`
Expected: `Finished`，无错误、无 `unused` 警告（若报 `run_install_mac`/`spawn_install` 仍被引用，回查漏删处）。

- [ ] **Step 7: clippy**

Run: `cargo clippy -p meowo-app`
Expected: `Finished`，无新 warning。

- [ ] **Step 8: 提交**

```bash
git add app/src-tauri/src/lib.rs app/src-tauri/src/macos/terminal.rs
git commit -m "feat(agent-install): install_agent 改后台安装+进度事件, 删开窗 spawn_install"
```

---

## Task 3: 前端卡片后台安装态 + 事件订阅 + 测试

`ProviderCard` 加 `idle/installing/error` 状态机与事件订阅；`AccountSection` 抽 `refreshInstalled` 作为成功回调；补 i18n / 类型 / 样式；更新测试。

**Files:**
- Modify: `app/src/i18n/zh.ts`、`app/src/i18n/en.ts`（account 段加键）
- Modify: `app/src/api.ts`（加事件类型）
- Modify: `app/src/styles.css`（加进度样式）
- Modify: `app/src/views/About.tsx`（导入 `listen`；`ProviderCard` 状态机+订阅+重试；`AccountSection` 抽 `refreshInstalled` 传 `onInstalled`）
- Modify: `app/src/views/About.account.test.tsx`（mock 事件 + 三条路径测试）

**Interfaces:**
- Consumes: 事件 `install-progress` / `install-done`（Task 2 的 payload 形状）。
- Produces: 无（终端 UI）。

- [ ] **Step 1: 加 i18n 键**

`app/src/i18n/zh.ts` 的 `account:` 段内、`installHint` 之后加：

```ts
    installing: "安装中…",
    installRetry: "重试",
    installFailed: "安装失败",
```

`app/src/i18n/en.ts` 对应 account 段同位置加：

```ts
    installing: "Installing…",
    installRetry: "Retry",
    installFailed: "Installation failed",
```

- [ ] **Step 2: 加事件 TS 类型**

`app/src/api.ts` 末尾（`installAgent` 附近）新增：

```ts
/** 后台安装进度事件 payload（对应后端 install-progress）。 */
export type InstallProgress = { provider: ProviderKey; line: string };
/** 后台安装结束事件 payload（对应后端 install-done）。 */
export type InstallDone = { provider: ProviderKey; ok: boolean; code: number | null };
```

- [ ] **Step 3: 加样式**

`app/src/styles.css` 末尾新增（转圈复用 `RefreshIcon spinning`，这里只排版与配色）：

```css
/* 账号页 agent 卡：后台安装进度（转圈 + 最新步骤行） */
.agent-install-progress {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  color: var(--cc-text-dim);
  max-width: 60%;
}
.agent-install-step {
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}
.agent-install-error {
  color: var(--cc-err-vivid);
}
```

（变量已核实存在：`--cc-text-dim` 次要文字色、`--cc-err-vivid` 错误色，与 `.usage-stale`/`.cstrip-error` 同源。）

- [ ] **Step 4: 写失败测试**

`app/src/views/About.account.test.tsx`：在顶部 `vi.mock("../api", …)` 之后加事件 mock（用 `vi.hoisted` 收集所有卡片的 listener，模拟 Tauri 向所有监听者广播），并加三条测试。

在文件顶部 import 区确保有 `act`：把第 2 行改为
`import { render, screen, fireEvent, cleanup, waitFor, act } from "@testing-library/react";`

在 `vi.mock("../api", …)` 之后加：

```tsx
// 收集所有 ProviderCard 注册的事件回调，测试里手动广播（模拟 Tauri emit 到全部监听者）
const ev = vi.hoisted(() => ({
  progressCbs: [] as Array<(e: unknown) => void>,
  doneCbs: [] as Array<(e: unknown) => void>,
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: (event: string, cb: (e: unknown) => void) => {
    if (event === "install-progress") ev.progressCbs.push(cb);
    if (event === "install-done") ev.doneCbs.push(cb);
    return Promise.resolve(() => {});
  },
}));
const fireProgress = (provider: string, line: string) =>
  act(() => ev.progressCbs.forEach((cb) => cb({ payload: { provider, line } })));
const fireDone = (provider: string, ok: boolean) =>
  act(() => ev.doneCbs.forEach((cb) => cb({ payload: { provider, ok, code: ok ? 0 : 1 } })));
```

在 `beforeEach` 末尾加重置：

```tsx
  ev.progressCbs.length = 0;
  ev.doneCbs.length = 0;
```

在 `describe("AccountSection agent 卡", …)` 内追加：

```tsx
  it("点安装进入安装中：转圈 + 最新步骤行", async () => {
    api.installAgent.mockResolvedValue(undefined);
    render(<AccountSection />);
    fireEvent.click(await screen.findByTestId("agent-install-kimi"));
    await waitFor(() => expect(screen.getByTestId("agent-installing-kimi")).toBeTruthy());
    fireProgress("kimi", "==> Downloading Kimi Code");
    expect(screen.getByTestId("agent-installing-kimi").textContent).toContain("Downloading Kimi Code");
    // 只更新本 provider：codex 的进度不影响 kimi
    fireProgress("codex", "==> other");
    expect(screen.getByTestId("agent-installing-kimi").textContent).toContain("Downloading Kimi Code");
  });

  it("install-done 成功后重查检测、卡片转已装", async () => {
    api.installAgent.mockResolvedValue(undefined);
    // 初次未装 kimi；装完重查返回含 kimi
    api.availableAgents.mockResolvedValueOnce(["claude", "codex"]).mockResolvedValue(["claude", "codex", "kimi"]);
    render(<AccountSection />);
    fireEvent.click(await screen.findByTestId("agent-install-kimi"));
    await waitFor(() => expect(screen.getByTestId("agent-installing-kimi")).toBeTruthy());
    fireDone("kimi", true);
    await waitFor(() => expect(screen.queryByTestId("agent-install-kimi")).toBeNull());
    expect(screen.queryByTestId("agent-installing-kimi")).toBeNull();
  });

  it("install-done 失败：退出安装中、显示重试按钮", async () => {
    api.installAgent.mockResolvedValue(undefined);
    render(<AccountSection />);
    fireEvent.click(await screen.findByTestId("agent-install-kimi"));
    await waitFor(() => expect(screen.getByTestId("agent-installing-kimi")).toBeTruthy());
    fireDone("kimi", false);
    await waitFor(() => expect(screen.queryByTestId("agent-installing-kimi")).toBeNull());
    // 仍未装 → 按钮回来（文案为重试），testid 不变
    expect(screen.getByTestId("agent-install-kimi")).toBeTruthy();
  });
```

- [ ] **Step 5: 跑测试确认失败**

Run: `cd app && bun run test -- About.account`（或 `bunx vitest run src/views/About.account.test.tsx`）
Expected: 新 3 条 FAIL（`agent-installing-kimi` 不存在等）。

- [ ] **Step 6: 前端实现 — 导入 listen + 类型**

`app/src/views/About.tsx` 顶部：
- 第 1 行 `import { useEffect, useRef, useState } from "react";` 保持（已含 useRef）。
- 加 `import { listen } from "@tauri-apps/api/event";`。
- 第 5/6 行的 `../api` 导入里追加 `type InstallProgress, type InstallDone`。

- [ ] **Step 7: 前端实现 — `ProviderCard` 状态机 + 订阅**

在 `ProviderCard` 函数体内、`const acc = …` 之后加状态与逻辑。先在参数解构里加 `onInstalled`：

把签名首行
`function ProviderCard({ provider, installed, payload, usage, err, onRefresh, refreshing, settings, onToggleQuota }: {`
改为
`function ProviderCard({ provider, installed, payload, usage, err, onRefresh, onInstalled, refreshing, settings, onToggleQuota }: {`
并在其后的 props 类型块内、`onRefresh: () => void;` 下一行加：
`  /** 后台安装成功后重查安装检测（令卡片转「已装」）。 */`
`  onInstalled: () => void;`

在 `const acc = payload?.account ?? null;` 之后加：

```tsx
  // 后台安装态：idle=未装可点 / installing=转圈+步骤行 / error=失败可重试。
  const [installState, setInstallState] = useState<"idle" | "installing" | "error">("idle");
  const [step, setStep] = useState("");
  // onInstalled 每次渲染新建，用 ref 存最新，事件订阅只依赖 provider、不反复重订。
  const onInstalledRef = useRef(onInstalled);
  onInstalledRef.current = onInstalled;

  const startInstall = () => {
    setStep("");
    setInstallState("installing");
    installAgent(provider).catch((e) => {
      setStep(String(e));
      setInstallState("error");
    });
  };

  useEffect(() => {
    const unP = listen<InstallProgress>("install-progress", (e) => {
      if (e.payload.provider === provider) setStep(e.payload.line);
    });
    const unD = listen<InstallDone>("install-done", (e) => {
      if (e.payload.provider !== provider) return;
      if (e.payload.ok) {
        setInstallState("idle");
        setStep("");
        onInstalledRef.current();
      } else {
        setInstallState("error");
      }
    });
    return () => {
      unP.then((f) => f());
      unD.then((f) => f());
    };
  }, [provider]);
```

- [ ] **Step 8: 前端实现 — 渲染安装中/重试**

把 head 里现有的安装按钮块

```tsx
        {installed === false && (
          <button
            type="button"
            className="provider-card-action provider-card-action-primary"
            data-testid={"agent-install-" + provider}
            onClick={() => installAgent(provider).catch(() => {})}
          >
            <IconDownload />
            {t.account.install}
          </button>
        )}
```

替换为：

```tsx
        {installed === false &&
          (installState === "installing" ? (
            <div className="agent-install-progress" data-testid={"agent-installing-" + provider}>
              <RefreshIcon spinning />
              <span className="agent-install-step">{step || t.account.installing}</span>
            </div>
          ) : (
            <button
              type="button"
              className="provider-card-action provider-card-action-primary"
              data-testid={"agent-install-" + provider}
              onClick={startInstall}
            >
              <IconDownload />
              {installState === "error" ? t.account.installRetry : t.account.install}
            </button>
          ))}
```

并在 head 结束 `</div>`（`provider-card-head` 闭合）之后、`{desc && …}` 之前，加失败信息行：

```tsx
      {installed === false && installState === "error" && step && (
        <div className="provider-card-body agent-install-error">{step}</div>
      )}
```

- [ ] **Step 9: 前端实现 — `AccountSection` 抽 `refreshInstalled` 并传入**

在 `AccountSection` 内，把现有

```tsx
  useEffect(() => { availableAgents().then((a) => setInstalled(new Set(a))).catch(() => {}); }, []);
  // 窗口重新聚焦时重检安装状态（一键安装装完回来即更新）。
  useEffect(() => {
    const onFocus = () => availableAgents().then((a) => setInstalled(new Set(a))).catch(() => {});
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, []);
```

替换为：

```tsx
  // 重查本机已装 agent 集合。挂载、窗口聚焦、后台安装成功各处复用。
  const refreshInstalled = () => {
    availableAgents().then((a) => setInstalled(new Set(a))).catch(() => {});
  };
  useEffect(() => { refreshInstalled(); }, []);
  useEffect(() => {
    const onFocus = () => refreshInstalled();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, []);
```

在 `return` 的 `<ProviderCard … />` 里，`onRefresh={() => doRefresh(p)}` 下一行加：
`            onInstalled={refreshInstalled}`

- [ ] **Step 10: 跑测试确认通过**

Run: `cd app && bunx vitest run src/views/About.account.test.tsx`
Expected: 全部 PASS（含原 2 条 + 新 3 条）。

- [ ] **Step 11: 类型检查 + 全量前端测试**

Run: `cd app && bun run build`（tsc + vite；或至少 `bunx tsc --noEmit`）后 `bunx vitest run`
Expected: 类型无误、测试全绿。

- [ ] **Step 12: 提交**

```bash
git add app/src/views/About.tsx app/src/views/About.account.test.tsx app/src/api.ts app/src/i18n/zh.ts app/src/i18n/en.ts app/src/styles.css
git commit -m "feat(agent-install): 账号页卡片后台安装态(转圈/步骤行/重试)+装完自动刷新"
```

---

## 手动验证（实现完成后，需重启 dev app）

1. 重启 dev app 加载新代码。
2. 账号页点 Codex「安装」：**不弹终端窗口**；卡片出现转圈 + `==> …` 步骤行滚动。
3. 装完卡片自动转「已登录/未登录」（不需重启、不需手动刷新）。
4. 断网或改坏脚本模拟失败：卡片显示错误行 + 「重试」，点重试可重来。

---

## Self-Review 结论

- **Spec 覆盖**：后台安装(Task2)、进度转圈+步骤行(Task3 Step7-8)、`is_progress_line` 过滤(Task1)、成功自动刷新(Task3 Step9 `onInstalled`→`refreshInstalled`)、失败+重试(Task3)、pwsh 优先(Task2 `build_install_command`)、不弹窗口 `CREATE_NO_WINDOW`(Task2)、跨平台 bash(Task2)、无重启 UI —— 均有对应任务。
- **无占位符**：所有步骤含实际代码/命令/预期。
- **类型一致**：`InstallProgress{provider,line}`、`InstallDone{provider,ok,code}` 前后端字段一致；`onInstalled`/`refreshInstalled` 命名前后一致；测试用 `agent-installing-<provider>` / `agent-install-<provider>` 与渲染一致。

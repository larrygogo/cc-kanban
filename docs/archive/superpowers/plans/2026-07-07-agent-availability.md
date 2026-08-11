# 按实际安装展示 agent（Agent Availability + 一键安装）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 所有选/展示 agent 的地方按用户设备实际安装情况展示——选 agent 处只列已装、账号页重构为 agent 卡显示安装/账号/用量状态、未装卡一键在终端跑官方安装脚本。

**Architecture:** 后端加 `Agent::is_installed()`（可执行存在口径）与 `available_agents()` 命令（仿 `available_terminals`）；`Agent::install_script()` 存各 agent 官方一句话安装命令，`install_agent` 命令在终端跑它（Windows `powershell -Command`，macOS AppleScript `do script`）。前端各展示点按 `availableAgents()` 过滤；账号页 `AccountSection` 重构为遍历全部 agent 的卡。

**Tech Stack:** Rust（tauri 2、meowo-reporter Agent trait）+ React 18 + TypeScript（vitest）。基于 `feat/new-session-20260706`（PR #28）。

## Global Constraints

- 判定口径 = **可执行存在**：claude → 在 PATH 或 `~/.local/bin`；codex → `codex_launch_prefix().is_some()` 或在 PATH；kimi → `~/.kimi-code/bin/kimi[.exe]` 存在或在 PATH。
- `available_agents` 是**实时命令**（检测廉价），前端打开新建面板/设置/agent 页时调，不缓存。仿 `available_terminals`（`lib.rs:1968`）。
- 一键安装命令（verbatim，官方核实 2026-07；硬编码在 `install_script` 单点）：
  - claude：Win `irm https://claude.ai/install.ps1 | iex` / mac `curl -fsSL https://claude.ai/install.sh | bash`
  - codex：Win `irm https://chatgpt.com/codex/install.ps1 | iex` / mac `curl -fsSL https://chatgpt.com/codex/install.sh | sh`
  - kimi：Win `irm https://code.kimi.com/install.ps1 | iex` / mac `curl -LsSf https://code.kimi.com/install.sh | bash`
- 安装命令是**受信硬编码串**（非用户输入）→ macOS AppleScript `do script` 直接跑（不 quoted，让 shell 解析管道）。安装终端沿用 `settings.resume_terminal`（默认终端）。
- 装完 app 不轮询；面板/agent 页在**窗口重新聚焦**时或用户手动刷新时重调 `available_agents`。
- `PROVIDER_KEYS`（`api.ts:13`）仍是全集不动；过滤基于 `availableAgents` 结果，不改 `PROVIDER_KEYS`。
- 已有会话卡片的 agent 图标（`Sticker.tsx:939`）不动。
- `Agent` trait 新方法必须 claude/kimi/codex 三 impl 全部实现。
- 代码英文，注释与 commit message 中文。分支 `feat/new-session-20260706`。

---

## File Structure

**修改：**
- `crates/meowo-reporter/src/agent.rs` — `Agent` trait 增 `is_installed()`、`install_script(windows)`；`exe_on_path` helper；三 impl；测试。
- `crates/meowo-reporter/src/kimi.rs` — 增 `kimi_installed()`（检查可执行路径存在）。
- `app/src-tauri/src/term_script.rs` — 增 `install_script_mac(kind)` AppleScript。
- `app/src-tauri/src/lib.rs` — `available_agents` 命令、`install_agent` 命令 + Windows/macOS 安装 spawn、注册。
- `app/src/api.ts` — `availableAgents()`、`installAgent()` wrapper。
- `app/src/views/NewSessionPanel.tsx` — agent 选择按 availableAgents 过滤 + default 未装退首个 + 空态提示。
- `app/src/views/About.tsx` — 默认 agent 下拉过滤；`AccountSection`/`ProviderCard` 重构为 agent 卡（安装状态 + 未装安装按钮）。
- `app/src/views/Sticker.tsx` — 底栏配额只显示已装 provider。
- `app/src/i18n/zh.ts` + `en.ts` — `account.*`/`agent.*` 新文案。

---

## Task 1: Agent::is_installed（可执行存在检测）

**Files:**
- Modify: `crates/meowo-reporter/src/agent.rs`（trait + 三 impl + helper + 测试）
- Modify: `crates/meowo-reporter/src/kimi.rs`（`kimi_installed`）

**Interfaces:**
- Produces: `Agent::is_installed(&self) -> bool`；`meowo_reporter::kimi::kimi_installed() -> bool`。

- [ ] **Step 1: 写失败测试**（`agent.rs` 的 `mod tests` 末尾）

```rust
    #[test]
    fn is_installed_reflects_executable_presence() {
        // 在临时 PATH 下放一个假 claude 可执行，claude 应判已装；清空 PATH 后应判未装。
        let dir = std::env::temp_dir().join(format!("cckb-agent-inst-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join(if cfg!(windows) { "claude.exe" } else { "claude" });
        std::fs::write(&exe, b"").unwrap();
        let saved = std::env::var_os("PATH");
        std::env::set_var("PATH", &dir);
        assert!(exe_on_path(if cfg!(windows) { "claude.exe" } else { "claude" }));
        std::env::set_var("PATH", ""); // 空 PATH
        assert!(!exe_on_path(if cfg!(windows) { "claude.exe" } else { "claude" }));
        if let Some(p) = saved { std::env::set_var("PATH", p); }
        let _ = std::fs::remove_dir_all(&dir);
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p meowo-reporter is_installed_reflects`
Expected: 编译失败 `cannot find function 'exe_on_path'`。

- [ ] **Step 3: 加 helper + kimi_installed + trait 方法 + 三 impl**

在 `agent.rs`（`is_agent_process` 附近）加：

```rust
/// 可执行 `name`（Windows 传含 .exe 的名）是否能在 PATH 各目录找到。纯查文件存在，不 spawn。
pub fn exe_on_path(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| dir.join(name).is_file())
    })
}

/// USERPROFILE/HOME 下的 `~/.local/bin/<name>` 是否存在（claude native installer 默认装这里并加 PATH，
/// 但 PATH 可能尚未在当前进程环境刷新，故兜底直查）。
fn in_local_bin(name: &str) -> bool {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()
        .map(|h| std::path::Path::new(&h).join(".local").join("bin").join(name).is_file())
        .unwrap_or(false)
}
```

在 `Agent` trait 里（`launch_args` 声明附近）加：

```rust
    /// 该 agent 的可执行是否装在本机（决定各处是否列出/可选它）。
    fn is_installed(&self) -> bool;
    /// 官方一句话安装命令串（None = 无一键方案）。`windows` 决定返回 PowerShell 还是 curl 版。
    /// 命令是受信硬编码串，调用方在终端里跑（Windows powershell -Command / macOS do script）。
    fn install_script(&self, windows: bool) -> Option<String>;
```

`ClaudeAgent`（`launch_args` 后）：

```rust
    fn is_installed(&self) -> bool {
        let bin = if cfg!(windows) { "claude.exe" } else { "claude" };
        exe_on_path(bin) || in_local_bin(bin)
    }
    fn install_script(&self, windows: bool) -> Option<String> {
        Some(if windows {
            "irm https://claude.ai/install.ps1 | iex".into()
        } else {
            "curl -fsSL https://claude.ai/install.sh | bash".into()
        })
    }
```

`KimiAgent`：

```rust
    fn is_installed(&self) -> bool {
        let bin = if cfg!(windows) { "kimi.exe" } else { "kimi" };
        crate::kimi::kimi_installed() || exe_on_path(bin)
    }
    fn install_script(&self, windows: bool) -> Option<String> {
        Some(if windows {
            "irm https://code.kimi.com/install.ps1 | iex".into()
        } else {
            "curl -LsSf https://code.kimi.com/install.sh | bash".into()
        })
    }
```

`CodexAgent`：

```rust
    fn is_installed(&self) -> bool {
        let bin = if cfg!(windows) { "codex.exe" } else { "codex" };
        crate::codex::codex_launch_prefix().is_some() || exe_on_path(bin)
    }
    fn install_script(&self, windows: bool) -> Option<String> {
        Some(if windows {
            "irm https://chatgpt.com/codex/install.ps1 | iex".into()
        } else {
            "curl -fsSL https://chatgpt.com/codex/install.sh | sh".into()
        })
    }
```

在 `kimi.rs`（`kimi_exe` 附近）加：

```rust
/// kimi 可执行是否真实存在于 `~/.kimi-code/bin`（区别于 `kimi_exe` 找不到时回退裸名 "kimi"）。
pub fn kimi_installed() -> bool {
    let bin = if cfg!(windows) { "kimi.exe" } else { "kimi" };
    kimi_share_dir().map(|d| d.join("bin").join(bin).is_file()).unwrap_or(false)
}
```

- [ ] **Step 4: 跑测试确认通过 + 编译**

Run: `cargo test -p meowo-reporter && cargo build -p meowo-reporter`
Expected: `is_installed_reflects_executable_presence` PASS；既有测试（`every_provider_key_has_agent` 等）全绿。

- [ ] **Step 5: Commit**

```bash
git add crates/meowo-reporter/src/agent.rs crates/meowo-reporter/src/kimi.rs
git commit -m "feat(agent-availability): Agent 增 is_installed / install_script（可执行存在口径 + 官方安装命令）"
```

---

## Task 2: available_agents 命令 + 前端 availableAgents

**Files:**
- Modify: `app/src-tauri/src/lib.rs`（命令 + 注册 `:2244` 区）
- Modify: `app/src/api.ts`（wrapper）

**Interfaces:**
- Consumes: `Agent::is_installed`（Task 1）、`agent::all()`（`agent.rs:215`）。
- Produces: 命令 `available_agents() -> Vec<String>`（已装 provider key）；前端 `availableAgents(): Promise<ProviderKey[]>`。

- [ ] **Step 1: 加命令**（`lib.rs`，仿 `available_terminals` 放其附近）

```rust
/// 本机实际已安装的 agent（provider key 列表），供各处按安装过滤展示。仿 available_terminals：
/// 检测廉价（PATH/文件查询），仍放 blocking 池避免任何意外阻塞事件循环。
#[tauri::command]
async fn available_agents() -> Vec<String> {
    tauri::async_runtime::spawn_blocking(|| {
        meowo_reporter::agent::all()
            .iter()
            .filter(|a| a.is_installed())
            .map(|a| a.key().as_str().to_string())
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default()
}
```

在 `generate_handler!`（`available_terminals` 附近）加 `available_agents`。

- [ ] **Step 2: 前端 wrapper**（`api.ts`，`availableTerminals` 附近）

```ts
/** 本机实际已安装的 agent（provider key）；各处选/展示 agent 按此过滤。 */
export function availableAgents(): Promise<ProviderKey[]> {
  return invoke("available_agents");
}
```

- [ ] **Step 3: 编译 + 类型检查**

Run: `cargo build -p meowo-app && (cd app && bunx tsc --noEmit)`
Expected: 通过。

- [ ] **Step 4: Commit**

```bash
git add app/src-tauri/src/lib.rs app/src/api.ts
git commit -m "feat(agent-availability): available_agents 命令 + 前端 wrapper"
```

---

## Task 3: install_agent 命令（终端跑官方安装脚本）

**Files:**
- Modify: `app/src-tauri/src/term_script.rs`（`install_script_mac`）
- Modify: `app/src-tauri/src/lib.rs`（`install_agent` 命令 + spawn + 注册）
- Modify: `app/src/api.ts`（`installAgent` wrapper）

**Interfaces:**
- Consumes: `Agent::install_script`（Task 1）、`load_settings().resume_terminal`。
- Produces: 命令 `install_agent(provider: String) -> Result<(), String>`；前端 `installAgent(provider: ProviderKey): Promise<void>`。

- [ ] **Step 1: macOS AppleScript（跑受信命令串，do script 不 quoted）**（`term_script.rs`，`resume_script_cwdless` 后）

```rust
/// 在 Terminal.app / iTerm2 新窗口里跑一条**受信命令串**（一键安装用；argv item 1 = 完整命令，
/// 直接 do script 不做 quoted form —— 命令是硬编码官方安装脚本、含管道，需 shell 解析）。
pub fn install_script_mac(kind: TermKind) -> &'static str {
    match kind {
        TermKind::ITerm2 => {
            r#"on run argv
  set theCmd to item 1 of argv
  tell application "iTerm2"
    activate
    set newWindow to (create window with default profile)
    tell current session of newWindow to write text theCmd
  end tell
end run"#
        }
        _ => {
            r#"on run argv
  set theCmd to item 1 of argv
  tell application "Terminal"
    activate
    do script theCmd
  end tell
end run"#
        }
    }
}
```

其对应测试（`term_script.rs` tests）：

```rust
    #[test]
    fn install_script_mac_runs_raw_command() {
        for kind in [TermKind::Terminal, TermKind::ITerm2, TermKind::Other] {
            let s = install_script_mac(kind);
            assert!(s.contains("item 1 of argv"));
            // 不得对命令做 quoted form（含管道要 shell 解析）。
            assert!(!s.contains("quoted form"));
        }
    }
```

- [ ] **Step 2: install_agent 命令 + 平台 spawn**（`lib.rs`）

```rust
/// 一键安装某 agent：在一个终端窗口里跑其官方安装脚本，用户看进度、装完关终端、面板刷新即变已装。
/// 安装命令是受信硬编码串（Agent::install_script），非用户输入。
#[tauri::command]
async fn install_agent(provider: String) -> Result<(), String> {
    let key = meowo_store::ProviderKey::parse(Some(&provider));
    let windows = cfg!(target_os = "windows");
    let script = meowo_reporter::agent::for_provider(key)
        .install_script(windows)
        .ok_or("该 agent 没有可用的一键安装命令")?;
    let terminal = load_settings().resume_terminal;
    tauri::async_runtime::spawn_blocking(move || spawn_install(&script, &terminal))
        .await
        .map_err(|e| e.to_string())?
        .then_some(())
        .ok_or_else(|| "启动安装终端失败".into())
}

/// 在终端里跑安装命令串。Windows：powershell -NoExit -ExecutionPolicy Bypass -Command "<script>"
/// （-NoExit 保留窗口看结果）；用户默认终端若是 wt 则在 wt 新标签跑，否则独立 PowerShell 窗口。
#[cfg(target_os = "windows")]
fn spawn_install(script: &str, terminal: &str) -> bool {
    use std::os::windows::process::CommandExt;
    use std::process::Command;
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    let ps_args = ["-NoExit", "-ExecutionPolicy", "Bypass", "-Command", script];
    let use_wt = terminal != "wezterm" && wt_available();
    let spawned: std::io::Result<()> = if use_wt {
        let mut args: Vec<String> = vec!["-w".into(), "0".into(), "nt".into(), "powershell".into()];
        args.extend(ps_args.iter().map(|s| s.to_string()));
        Command::new("wt").args(&args).spawn().map(|_| ())
    } else {
        Command::new("powershell").args(ps_args).creation_flags(CREATE_NEW_CONSOLE).spawn().map(|_| ())
    };
    match spawned {
        Ok(()) => true,
        Err(e) => { eprintln!("一键安装启动失败：{e}"); false }
    }
}

/// macOS：Terminal.app / iTerm2 新窗口 do script 跑安装命令串（按 resume_terminal 选宿主）。
#[cfg(target_os = "macos")]
fn spawn_install(script: &str, terminal: &str) -> bool {
    use crate::term_script::TermKind;
    let kind = match crate::term_script::resume_kind_from_setting(terminal) {
        TermKind::ITerm2 if iterm_installed() => TermKind::ITerm2,
        TermKind::ITerm2 => TermKind::Terminal,
        other => other,
    };
    crate::macos::terminal::run_install_mac(script, kind)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn spawn_install(_script: &str, _terminal: &str) -> bool { false }
```

在 `macos/terminal.rs` 加（复用 `run_osascript`）：

```rust
/// 一键安装用：新窗口跑受信安装命令串（osascript 从 stdin 读脚本，命令经 argv 传入防脚本注入）。
pub fn run_install_mac(cmd: &str, kind: TermKind) -> bool {
    run_osascript(crate::term_script::install_script_mac(kind), &[cmd]).is_ok()
}
```

在 `generate_handler!` 加 `install_agent`。

- [ ] **Step 3: 前端 wrapper**（`api.ts`）

```ts
/** 一键安装某 agent（在终端跑官方安装脚本）。装完在窗口重新聚焦/手动刷新时重检安装状态。 */
export function installAgent(provider: ProviderKey): Promise<void> {
  return invoke("install_agent", { provider });
}
```

- [ ] **Step 4: 编译 + term_script 测试**

Run: `cargo test -p meowo-app install_script_mac && cargo build -p meowo-app && (cd app && bunx tsc --noEmit)`
Expected: 测试 PASS；编译通过。（Windows 若 meowo-app.exe 在跑致 exe 链接锁，退用 `cargo build -p meowo-app --lib`，report 注明。）

- [ ] **Step 5: Commit**

```bash
git add app/src-tauri/src/term_script.rs app/src-tauri/src/macos/terminal.rs app/src-tauri/src/lib.rs app/src/api.ts
git commit -m "feat(agent-availability): install_agent 命令——终端跑官方安装脚本（Windows powershell / macOS do script）"
```

---

## Task 4: 新建面板 + 设置默认下拉按已装过滤

**Files:**
- Modify: `app/src/views/NewSessionPanel.tsx`
- Modify: `app/src/views/About.tsx`（默认 agent 下拉）
- Modify: `app/src/i18n/zh.ts` + `en.ts`（`newSession.noAgents`）

**Interfaces:**
- Consumes: `availableAgents`（Task 2）。

- [ ] **Step 1: 写失败测试**（`NewSessionPanel.test.tsx`，新增；mock availableAgents 只返回 claude/codex，验证 kimi 卡不渲染 + 一都没装的提示）

```tsx
  it("agent 选择只列已装的", async () => {
    api.availableAgents.mockResolvedValue(["claude", "codex"]);
    render(<NewSessionPanel />);
    await screen.findByTestId("ns-launch");
    expect(screen.queryByTestId("ns-agent-claude")).toBeTruthy();
    expect(screen.queryByTestId("ns-agent-codex")).toBeTruthy();
    expect(screen.queryByTestId("ns-agent-kimi")).toBeNull();
  });

  it("一个都没装时提示 + 启动禁用", async () => {
    api.availableAgents.mockResolvedValue([]);
    render(<NewSessionPanel />);
    expect(await screen.findByTestId("ns-no-agents")).toBeTruthy();
    expect((screen.getByTestId("ns-launch") as HTMLButtonElement).disabled).toBe(true);
  });
```

（在文件顶部 mock 里给 `availableAgents: vi.fn()`，`beforeEach` 默认 `mockResolvedValue(["claude","codex","kimi"])`。）

- [ ] **Step 2: 跑测试确认失败**

Run: `cd app && bunx vitest run src/views/NewSessionPanel.test.tsx`
Expected: 失败（无 `ns-agent-*` / `ns-no-agents` testid、无过滤）。

- [ ] **Step 3: 改 NewSessionPanel**

import 加 `availableAgents`。state 加 `const [avail, setAvail] = useState<ProviderKey[] | null>(null);`（null=未加载）。effect 里加 `availableAgents().then(setAvail).catch(() => setAvail([]));`。

default_agent 选中改为「已装才选，否则退首个已装」——在 avail 加载后校正：

```tsx
  useEffect(() => {
    if (avail && avail.length > 0 && !avail.includes(provider)) setProvider(avail[0]);
  }, [avail, provider]);
```

agent 列表渲染改用 `avail`（未加载时用 PROVIDER_KEYS 占位或空）：把 `{PROVIDER_KEYS.map((p) => {` 改为 `{(avail ?? []).map((p) => {`，并给每张卡 `data-testid={"ns-agent-" + p}`。

`.ns-agents` 块外层加空态：

```tsx
        <div className="ns-field">
          <span className="ns-label">{t.newSession.agent}</span>
          {avail && avail.length === 0 ? (
            <div className="ns-warn" data-testid="ns-no-agents">{t.newSession.noAgents}</div>
          ) : (
            <div className="ns-agents">{/* 已装 agent 卡，每张 data-testid={"ns-agent-"+p} */}</div>
          )}
          {avail && avail.length > 0 && warn && (/* 现有 hooks 警告，不变 */)}
        </div>
```

启动禁用条件加"无可用 agent"：`disabled={!cwd.trim() || busy || (avail?.length ?? 0) === 0}`。

- [ ] **Step 4: i18n**（zh.ts / en.ts 的 `newSession` 段加）

```ts
    noAgents: "未检测到已安装的 AI CLI（claude / codex / kimi），请先安装",   // zh
```
```ts
    noAgents: "No installed AI CLI detected (claude / codex / kimi). Please install one first.",   // en
```

- [ ] **Step 5: 设置页默认 agent 下拉过滤**（`About.tsx`）

`GeneralSection` 里加 `const [availAgents, setAvailAgents] = useState<ProviderKey[]>([]);` + `useEffect(() => { availableAgents().then(setAvailAgents).catch(() => {}); }, []);`（import `availableAgents`）。默认 agent 下拉 options 从 `PROVIDER_KEYS` 改为 `availAgents`：

```tsx
            options={availAgents.map((p) => ({ value: p, label: providerConfig(p).label(t) }))}
```

- [ ] **Step 6: 跑测试 + 全量 + tsc**

Run: `cd app && bunx vitest run src/views/NewSessionPanel.test.tsx && bunx vitest run && bunx tsc --noEmit`
Expected: 新增两测试 PASS；全量不回归。

- [ ] **Step 7: Commit**

```bash
git add app/src/views/NewSessionPanel.tsx app/src/views/About.tsx app/src/i18n/zh.ts app/src/i18n/en.ts
git commit -m "feat(agent-availability): 新建面板与设置默认下拉只列已装 agent + 空态提示"
```

---

## Task 5: 贴纸配额只显示已装 provider

**Files:**
- Modify: `app/src/views/Sticker.tsx`（底栏配额屏 `UsageScreen` / `quotaProviders`）

**Interfaces:**
- Consumes: `availableAgents`（Task 2）。

- [ ] **Step 1: 定位现有配额 provider 来源**

Run: `cd app && grep -n "quotaProviders\|sticker_quota" src/views/Sticker.tsx`
（现有 `quotaProviders` 来自 `settings.sticker_quota_providers`。）

- [ ] **Step 2: 加 availableAgents 过滤**

在 Sticker 主组件加 `const [availAgents, setAvailAgents] = useState<string[]>([]);` + effect `availableAgents().then(setAvailAgents).catch(() => {});`（import `availableAgents`）。把传给底栏配额的 `quotaProviders` 交叉过滤为**既在配额设置里、又已装**：

```tsx
  const shownQuota = quotaProviders.filter((p) => availAgents.length === 0 || availAgents.includes(p));
```

用 `shownQuota` 替换传给 `<UsageScreen quotaProviders={...}>` 的值（`availAgents` 为空=未加载时不过滤，避免闪空）。

- [ ] **Step 3: tsc + 全量测试**

Run: `cd app && bunx tsc --noEmit && bunx vitest run`
Expected: 通过、不回归。

- [ ] **Step 4: Commit**

```bash
git add app/src/views/Sticker.tsx
git commit -m "feat(agent-availability): 贴纸底栏配额只显示已装 provider"
```

---

## Task 6: 账号页 → agent 卡（显示全部 + 安装/账号/用量状态 + 一键安装）

**Files:**
- Modify: `app/src/views/About.tsx`（`AccountSection` / `ProviderCard`）
- Modify: `app/src/i18n/zh.ts` + `en.ts`（`account.installed`/`notInstalled`/`notLoggedIn`/`install`）

**Interfaces:**
- Consumes: `availableAgents`、`installAgent`（Task 2/3）、`getAccounts`、`PROVIDER_KEYS`、`providerConfig`。

- [ ] **Step 1: 写失败测试**（`About.tsx` 若无测试则新建 `About.account.test.tsx`；mock getAccounts 返回仅 claude 有账号、availableAgents 返回 claude+codex，验证：三卡全渲染、kimi 标未安装且有安装按钮、codex 已装但未登录）

```tsx
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup, waitFor } from "@testing-library/react";

const api = { getAccounts: vi.fn(), availableAgents: vi.fn(), installAgent: vi.fn(), refreshUsage: vi.fn(), getSettings: vi.fn(), setSettings: vi.fn() };
vi.mock("../api", async (o) => ({ ...(await o<typeof import("../api")>()), ...api }));

import { AccountSection } from "./About";

beforeEach(() => {
  Object.values(api).forEach((m) => m.mockReset());
  api.getAccounts.mockResolvedValue([{ provider: "claude", account: { email: "a@b.c" }, usage: null, usage_supported: true }]);
  api.availableAgents.mockResolvedValue(["claude", "codex"]);
  api.refreshUsage.mockResolvedValue({ lanes: [], note: null });
  api.getSettings.mockResolvedValue({ sticker_quota_providers: [] });
});
afterEach(() => cleanup());

describe("AccountSection agent 卡", () => {
  it("三个 agent 都渲染，未装的标未安装 + 安装按钮", async () => {
    render(<AccountSection />);
    await waitFor(() => expect(screen.getByTestId("agent-card-kimi")).toBeTruthy());
    expect(screen.getByTestId("agent-card-claude")).toBeTruthy();
    expect(screen.getByTestId("agent-card-codex")).toBeTruthy();
    // kimi 未装：有安装按钮
    expect(screen.getByTestId("agent-install-kimi")).toBeTruthy();
    // 已装的（claude/codex）无安装按钮
    expect(screen.queryByTestId("agent-install-claude")).toBeNull();
  });

  it("点安装调 installAgent", async () => {
    api.installAgent.mockResolvedValue(undefined);
    render(<AccountSection />);
    fireEvent.click(await screen.findByTestId("agent-install-kimi"));
    await waitFor(() => expect(api.installAgent).toHaveBeenCalledWith("kimi"));
  });
});
```

（需要 `export function AccountSection`——若当前非 export，本 task 顺手加 `export`。）

- [ ] **Step 2: 跑测试确认失败**

Run: `cd app && bunx vitest run src/views/About.account.test.tsx`
Expected: 失败（AccountSection 未 export / 无 agent-card 骨架 / 无安装按钮）。

- [ ] **Step 3: 重构 AccountSection 为「遍历全部 agent」**

`AccountSection` 改为：以 `PROVIDER_KEYS` 为骨架（而非只 `getAccounts` 返回的），拿 `availableAgents()` 存 `installed: Set`，每个 provider 渲染一张 `ProviderCard`，传入 `installed`（是否已装）+ 该 provider 的 payload（`getAccounts` 里匹配，可能无）。

`AccountSection` 内加：

```tsx
  const [installed, setInstalled] = useState<Set<string>>(new Set());
  useEffect(() => { availableAgents().then((a) => setInstalled(new Set(a))).catch(() => {}); }, []);
  // 窗口重新聚焦时重检安装状态（一键安装装完回来即更新）。
  useEffect(() => {
    const onFocus = () => availableAgents().then((a) => setInstalled(new Set(a))).catch(() => {});
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, []);
```

渲染改为遍历 `PROVIDER_KEYS`（import 之）：每个 `p` 找 `payloads.find((x) => x.provider === p)`，渲染 `<ProviderCard key={p} provider={p} installed={installed.has(p)} payload={...} .../>`。

- [ ] **Step 4: 改 ProviderCard 支持未装/未登录三态**

`ProviderCard` 签名加 `provider: ProviderKey; installed: boolean;`，`payload` 改为可空 `payload: ProviderAccountPayload | null`。删掉 `if (!acc) return null`。顶部按状态渲染：

```tsx
  const cfg = providerConfig(provider);
  const acc = payload?.account ?? null;
  return (
    <div className="row-card provider-card" data-testid={"agent-card-" + provider}>
      <div className="provider-head">
        <span className="provider-icon"><cfg.Icon /></span>
        <span className="provider-name">{cfg.label(t)}</span>
        {!installed && <span className="agent-badge agent-badge-off">{t.account.notInstalled}</span>}
        {installed && !acc && <span className="agent-badge">{t.account.notLoggedIn}</span>}
      </div>
      {!installed ? (
        <div className="agent-install-row">
          <button type="button" className="ns-btn is-primary" data-testid={"agent-install-" + provider}
            onClick={() => installAgent(provider).catch(() => {})}>
            {t.account.install}
          </button>
        </div>
      ) : acc ? (
        <>{/* 现有 acc-block + provider-usage（账号+用量+配额开关），不变 */}</>
      ) : (
        <div className="agent-install-row">{t.account.notLoggedInHint}</div>
      )}
    </div>
  );
```

（已装且登录的分支 = 现有 `acc-block`/`provider-usage` 原样搬进 `acc ?` 分支。）

- [ ] **Step 5: i18n**（`account` 段加）

```ts
    // zh
    installed: "已安装", notInstalled: "未安装", notLoggedIn: "未登录",
    notLoggedInHint: "已安装，未登录——在终端运行该 CLI 登录后这里会显示账号",
    install: "安装",
```
```ts
    // en
    installed: "Installed", notInstalled: "Not installed", notLoggedIn: "Not signed in",
    notLoggedInHint: "Installed but not signed in — run the CLI in a terminal to log in",
    install: "Install",
```

- [ ] **Step 6: 样式**（`styles.css` 加）

```css
.agent-badge { font-size: 10px; padding: 1px 7px; border-radius: 999px; background: var(--cc-surface-hover); color: var(--cc-text-dim); margin-left: auto; }
.agent-badge-off { color: var(--cc-warn); }
.agent-install-row { padding: 10px 0 2px; font-size: 12px; color: var(--cc-text-dim); }
```

- [ ] **Step 7: 跑测试 + 全量 + tsc**

Run: `cd app && bunx vitest run src/views/About.account.test.tsx && bunx vitest run && bunx tsc --noEmit`
Expected: 两测试 PASS；全量不回归。

- [ ] **Step 8: Commit**

```bash
git add app/src/views/About.tsx app/src/i18n/zh.ts app/src/i18n/en.ts app/src/styles.css
git commit -m "feat(agent-availability): 账号页重构为 agent 卡（安装/登录/未装三态 + 一键安装按钮）"
```

---

## 最终验收（全 task 完成后）

- [ ] **Rust：** `cargo test` 三 crate + `cargo clippy --all-targets -- -D warnings`
- [ ] **前端：** `cd app && bunx vitest run && bunx tsc --noEmit`
- [ ] **手动（真机）：**
  - 卸载某个 CLI → 新建面板、设置默认下拉、配额都不再列它;账号页该卡标「未安装」+ 安装按钮。
  - 点「安装」→ 终端开出跑官方脚本;装完切回窗口(focus)→ 卡片变已装、各处重新列出。
  - 全部卸载 → 新建面板显示「未检测到 AI CLI」+ 启动禁用。
  - 已装未登录的 agent → 卡片标「未登录」。
- [ ] **回归：** 新建会话、resume、已有会话卡片图标不受影响。

# 从看板新建会话（New Session）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在看板底部栏/空状态加「新建会话」入口，弹迷你面板选目录 + agent + 终端，一键 spawn 终端启动全新 claude/codex/kimi 会话，卡片随后经各自 hook 自然出现。

**Architecture:** 复用 `resume_session` 的终端 spawn 机制——把「选终端 + spawn argv」抽成共享 `spawn_in_terminal`，resume 传 resume 命令、new 传新的 `launch_args`（裸启动命令，无 session_id）。会话入库仍走各 provider 的 hook（app 不预插占位卡）；spawn 后前端给瞬态 toast。codex/kimi 的 hooks 只检测（`check_provider_hooks`）+ 面板引导，不自动安装。目录选择接入 `tauri-plugin-dialog`，最近目录来自 board.db 已有会话 cwd。

**Tech Stack:** Rust（tauri 2、rusqlite、serde_json）+ React 18 + Vite + TypeScript（vitest）。三 crate workspace：`meowo-store`（DB）、`meowo-reporter`（Agent 抽象）、`meowo-app`（Tauri 后端）。

## Global Constraints

- provider 范围：claude / codex / kimi 全支持；新建只负责 spawn，**不自动安装 hooks**，只检测 + 引导。
- 卡片出现依赖各 provider 的 hook：claude/kimi 启动即 `SessionStart`（秒级出卡）；**codex 要到首条消息才出卡**——文案如实告知，不做占位卡。
- 终端 spawn 仅 Windows + macOS 有实现；其它平台 `spawn_in_terminal` 返回 `false`（new_session 报「当前平台不支持」类错误）。
- ProviderKey 前后端单一事实源同步点：`api.ts` 的 `ProviderKey` 联合类型、`providers.tsx` 的 `PROVIDERS`、Rust `meowo_store::ProviderKey::ALL`（本计划不新增 provider，仅使用）。
- Rust 错误：meowo-store 用 `thiserror`（`StoreError`，`?` 直接传播）；异步用 `tauri::async_runtime`。
- 前端：包管理 `bun`，测试 `vitest`；命令 `invoke(name, {camelCaseArgs})`，Tauri 自动转 snake_case 参数。
- `tauri-plugin-dialog` 的 `open` 需在 `app/src-tauri/capabilities/default.json` 授权。
- 代码用英文，注释与 commit message 用中文。
- 分支 `feat/new-session-20260706`（已创建，spec 已提交 `f09cb34`）；每个 task 结尾 commit。
- Agent trait 新方法必须在 `ClaudeAgent`/`KimiAgent`/`CodexAgent` 三个 impl 全部实现（否则不编译）。

---

## File Structure

**修改：**
- `crates/meowo-reporter/src/agent.rs` — `Agent` trait 增 `launch_args()` + 三 impl + 测试。
- `crates/meowo-store/src/store.rs` — 增 `recent_cwds()` + 测试。
- `app/src-tauri/src/settings.rs` — `Settings` 增 `default_agent` 字段。
- `app/src-tauri/src/ccsetup.rs` — 增 `pub fn claude_hooks_installed()`，`claude_settings_path`/`parse_settings` 提 `pub(crate)`。
- `app/src-tauri/src/lib.rs` — 抽 `spawn_in_terminal`；`resume_session` 改调它；新增 `new_session` / `recent_cwds` / `check_provider_hooks` 命令 + 纯函数 + 注册 + dialog plugin。
- `app/src-tauri/Cargo.toml` — 加 `tauri-plugin-dialog`。
- `app/src-tauri/capabilities/default.json` — 加 dialog 权限。
- `app/package.json` — 加 `@tauri-apps/plugin-dialog`。
- `app/src/api.ts` — 类型 + wrapper（`newSession`/`recentCwds`/`checkProviderHooks`/`HooksStatus`/`PROVIDER_KEYS`/`Settings.default_agent`）。
- `app/src/i18n/zh.ts` + `app/src/i18n/en.ts` — 加 `newSession.*` 文案。
- `app/src/views/Sticker.tsx` — 底部栏「新建」按钮 + 空状态 CTA + toast + 挂载面板。
- `app/src/styles.css` — 面板 / toast / CTA 样式。
- `app/src/views/About.tsx` — （可选）默认 agent 下拉。

**新建：**
- `app/src/views/NewSessionPanel.tsx` — 新建会话迷你面板组件。
- `app/src/views/NewSessionPanel.test.tsx` — 组件 vitest。

---

## Task 1: Agent::launch_args（裸启动动词）

**Files:**
- Modify: `crates/meowo-reporter/src/agent.rs`（trait 定义 `:36` 附近；三 impl `:48-159`；测试 mod `:212`）

**Interfaces:**
- Produces: `Agent::launch_args(&self) -> Vec<String>` — 裸启动 argv（无 resume/id）。claude=`["claude"]`、kimi=`[kimi_exe()]`、codex=`codex_launch_prefix()` 或回退 `["codex"]`。

- [ ] **Step 1: 写失败测试**

在 `crates/meowo-reporter/src/agent.rs` 的 `mod tests` 末尾（`resume_args_per_provider` 之后）加：

```rust
    #[test]
    fn launch_args_per_provider() {
        // claude：裸命令，无 resume/id。
        assert_eq!(for_provider(ProviderKey::Claude).launch_args(), vec!["claude"]);
        // codex：不含 resume/id；末元素不是 "resume"；某元素含 "codex"。
        let codex = for_provider(ProviderKey::Codex).launch_args();
        assert!(codex.iter().all(|a| a != "resume"));
        assert!(codex.iter().any(|a| a.to_ascii_lowercase().contains("codex")));
        // kimi：单元素可执行（绝对路径或回退裸名），无参数。
        let kimi = for_provider(ProviderKey::Kimi).launch_args();
        assert_eq!(kimi.len(), 1);
        assert!(kimi[0].to_ascii_lowercase().contains("kimi"));
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p meowo-reporter launch_args_per_provider`
Expected: 编译失败 `no method named 'launch_args' found`。

- [ ] **Step 3: 实现**

在 trait `Agent` 里 `resume_args` 声明（`:36`）下方加方法声明：

```rust
    /// 裸启动一个全新会话的命令 argv（[可执行名, 参数...]），不含 resume/id。
    /// 如 ["claude"] / [kimi_exe()] / codex 启动前缀。看板「新建会话」用。
    fn launch_args(&self) -> Vec<String>;
```

`ClaudeAgent`（在 `resume_args` 后，`:68` 下方）：

```rust
    fn launch_args(&self) -> Vec<String> {
        vec!["claude".into()]
    }
```

`KimiAgent`（`:105` 下方）：

```rust
    fn launch_args(&self) -> Vec<String> {
        // 绝对路径优先（spawned 终端 PATH 未必含 kimi），与 resume_args 同源。
        vec![crate::kimi::kimi_exe()]
    }
```

`CodexAgent`（`:154` 下方）：

```rust
    fn launch_args(&self) -> Vec<String> {
        // 与 resume_args 同款可执行解析，仅去掉 `resume <id>`：进入 codex TUI 新会话。
        crate::codex::codex_launch_prefix().unwrap_or_else(|| vec!["codex".into()])
    }
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p meowo-reporter`
Expected: 全绿（含新 `launch_args_per_provider` 与既有 `resume_args_per_provider`）。

- [ ] **Step 5: Commit**

```bash
git add crates/meowo-reporter/src/agent.rs
git commit -m "feat(new-session): Agent 增 launch_args 裸启动动词（claude/codex/kimi）"
```

---

## Task 2: Settings.default_agent

**Files:**
- Modify: `app/src-tauri/src/settings.rs`（`Settings` struct `:51-92`；`Default` impl `:94-112`；default fn 区 `:8-47`）
- Modify: `app/src/api.ts`（`Settings` type `:117-144`）

**Interfaces:**
- Produces: `Settings.default_agent: String`（默认 `"claude"`）；前端 `Settings.default_agent: ProviderKey`。

- [ ] **Step 1: 写失败测试**

在 `settings.rs` 底部新增（文件当前无 `mod tests`，整段追加到文件末尾）：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_agent_defaults_to_claude() {
        assert_eq!(Settings::default().default_agent, "claude");
    }

    #[test]
    fn old_settings_json_without_default_agent_deserializes() {
        // 旧 settings.json 无 default_agent 字段：serde default 兜底 claude，不 panic。
        let v: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(v.default_agent, "claude");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p meowo-app default_agent`
Expected: 编译失败 `no field 'default_agent' on type 'Settings'`。

- [ ] **Step 3: 实现**

在 default fn 区（`:47` `default_sticker_quota_providers` 之后）加：

```rust
/// 新建会话默认选中的 agent（provider key）。缺省 claude。
fn default_default_agent() -> String {
    "claude".to_string()
}
```

在 `Settings` struct 末尾字段（`sticker_quota_providers` 之后，`:91` 下方）加：

```rust
    /// 「新建会话」面板默认选中的 agent（claude/kimi/codex）。缺省 claude，兼容老 settings.json。
    #[serde(default = "default_default_agent")]
    pub(crate) default_agent: String,
```

在 `Default for Settings`（`:109` `sticker_quota_providers` 行之后）加：

```rust
            default_agent: default_default_agent(),
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p meowo-app default_agent`
Expected: 两个测试 PASS。

- [ ] **Step 5: 前端类型同步**

在 `app/src/api.ts` 的 `Settings` type 内（`sticker_quota_providers` 行 `:143` 之后）加：

```ts
  /** 「新建会话」面板默认选中的 agent。缺省 "claude"。 */
  default_agent: ProviderKey;
```

- [ ] **Step 6: 前端类型检查**

Run: `cd app && bunx tsc --noEmit`
Expected: 无类型错误（此步仅验证类型，消费点在后续 task 接入）。

- [ ] **Step 7: Commit**

```bash
git add app/src-tauri/src/settings.rs app/src/api.ts
git commit -m "feat(new-session): Settings 增 default_agent 字段（默认 claude）"
```

---

## Task 3: Store::recent_cwds

**Files:**
- Modify: `crates/meowo-store/src/store.rs`（在 `live_session_liveness` `:689` 附近加方法；测试进文件内 `#[cfg(test)]` 或新增）

**Interfaces:**
- Produces: `Store::recent_cwds(&self, limit: usize) -> Result<Vec<String>, StoreError>` — 去重非空 cwd，按各 cwd 的最近 `last_event_at` 倒序，取前 `limit`。

- [ ] **Step 1: 写失败测试**

在 `store.rs` 文件末尾新增（`start_session(project_id, cc_session_id, now_ms) -> (sid, tid)` 见 `:199`；`set_session_cwd` 会把 `last_event_at` 刷成传入时间，用它控制各目录“最近活跃时刻”）：

```rust
#[cfg(test)]
mod recent_cwds_tests {
    use super::*;

    #[test]
    fn recent_cwds_dedups_orders_and_limits() {
        let store = Store::open_in_memory().unwrap();
        let pid = store.upsert_project_by_root("C:/root", "root", 100).unwrap();
        let (id1, _) = store.start_session(pid, "s1", 100).unwrap();
        store.set_session_cwd(id1, "C:/projA", 100).unwrap();
        let (id2, _) = store.start_session(pid, "s2", 200).unwrap();
        store.set_session_cwd(id2, "C:/projB", 300).unwrap();
        let (id3, _) = store.start_session(pid, "s3", 400).unwrap();
        store.set_session_cwd(id3, "C:/projA", 500).unwrap(); // projA 再次活跃至 500

        // projA 最近活跃 500 > projB 300；projA 去重仅一条。
        assert_eq!(
            store.recent_cwds(10).unwrap(),
            vec!["C:/projA".to_string(), "C:/projB".to_string()]
        );
        // limit 生效。
        assert_eq!(store.recent_cwds(1).unwrap(), vec!["C:/projA".to_string()]);
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p meowo-store recent_cwds`
Expected: 编译失败 `no method named 'recent_cwds'`。

- [ ] **Step 3: 实现**

在 `store.rs` 的 `live_session_liveness`（`:689-699`）后加：

```rust
    /// 「新建会话」面板的最近目录：去重非空 cwd，按各目录最近一次 last_event_at 倒序取前 limit。
    pub fn recent_cwds(&self, limit: usize) -> Result<Vec<String>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT cwd FROM sessions \
             WHERE cwd IS NOT NULL AND cwd <> '' \
             GROUP BY cwd \
             ORDER BY MAX(last_event_at) DESC \
             LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p meowo-store recent_cwds`
Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add crates/meowo-store/src/store.rs
git commit -m "feat(new-session): Store 增 recent_cwds（去重+倒序+limit 的最近目录）"
```

---

## Task 4: 抽 spawn_in_terminal + resume_session 改用（重构）

本 task 无新增单测（spawn 是纯副作用），验证靠**编译通过 + resume 手动回归**。抽取是机械代码移动，逻辑不变。

**Files:**
- Modify: `app/src-tauri/src/lib.rs`（新增 `spawn_in_terminal`；改 `resume_session` `:1044-1160` 的 Windows/macOS 分支主体）

**Interfaces:**
- Produces: `spawn_in_terminal(argv: &[String], cwd: Option<&str>, terminal: &str) -> bool` — 按 `terminal` 选终端在 `cwd` spawn `argv`；成功 true。Task 5 复用。

- [ ] **Step 1: 加 spawn_in_terminal（三平台）**

在 `lib.rs` 的 `resume_session`（`:1044`）**之前**插入：

```rust
/// 在 `cwd` 打开一个终端并运行 `argv`，终端类型由 `terminal`（同 settings.resume_terminal 取值）决定。
/// resume（`claude --resume <id>`）与 new（裸 `claude`）共用——唯一区别是传入的 argv。成功返回 true。
/// Windows：powershell/cmd/wezterm/wt，缺失回退链同 resume 旧逻辑；wt 分支独立传 argv 不拼 shell 串。
#[cfg(target_os = "windows")]
fn spawn_in_terminal(argv: &[String], cwd: Option<&str>, terminal: &str) -> bool {
    use std::os::windows::process::CommandExt;
    use std::process::Command;
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

    let dir = safe_cwd(cwd);
    // 选了 wt/默认但没装 wt → 回退 PowerShell；选了 wezterm 但已卸载 → 落回 wt/powershell。
    let eff = match terminal {
        "powershell" => "powershell",
        "cmd" => "cmd",
        "wezterm" if wezterm::available() => "wezterm",
        _ if wt_available() => "wt",
        _ => "powershell",
    };
    let spawned: std::io::Result<()> = match eff {
        "powershell" => {
            let mut c = Command::new("powershell");
            c.args(["-NoExit", "-Command", &shell_join_for_windows(argv, true)]);
            if let Some(d) = &dir {
                c.current_dir(d);
            }
            c.creation_flags(CREATE_NEW_CONSOLE).spawn().map(|_| ())
        }
        "cmd" => {
            let mut c = Command::new("cmd");
            c.raw_arg("/k").raw_arg(shell_join_for_windows(argv, false));
            if let Some(d) = &dir {
                c.current_dir(d);
            }
            c.creation_flags(CREATE_NEW_CONSOLE).spawn().map(|_| ())
        }
        "wezterm" => wezterm::resume(dir.as_deref(), argv),
        _ => {
            let mut args: Vec<String> = vec!["-w".into(), "0".into(), "nt".into()];
            if let Some(p) = wt_default_profile() {
                args.push("-p".into());
                args.push(p);
            }
            if let Some(d) = &dir {
                args.push("-d".into());
                args.push(d.clone());
            }
            args.extend(argv.iter().cloned());
            Command::new("wt").args(&args).spawn().map(|_| ())
        }
    };
    match spawned {
        Ok(()) => true,
        Err(e) => {
            eprintln!("打开终端 {eff} 失败：{e}");
            false
        }
    }
}

/// macOS 版：按 terminal 选 Terminal.app/iTerm2（iTerm2 未装回退 Terminal），走 AppleScript。成功 true。
#[cfg(target_os = "macos")]
fn spawn_in_terminal(argv: &[String], cwd: Option<&str>, terminal: &str) -> bool {
    use crate::term_script::TermKind;
    let kind = match crate::term_script::resume_kind_from_setting(terminal) {
        TermKind::ITerm2 if iterm_installed() => TermKind::ITerm2,
        TermKind::ITerm2 => TermKind::Terminal,
        other => other,
    };
    crate::macos::terminal::resume_session_mac(cwd, argv, kind)
}

/// 其它平台无终端集成。
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn spawn_in_terminal(_argv: &[String], _cwd: Option<&str>, _terminal: &str) -> bool {
    false
}
```

- [ ] **Step 2: resume_session 的 Windows 分支改调 helper**

把 `resume_session` Windows 分支线程体（`:1063-1126`，从 `let (revived, ...) = prepare_resume` 到 spawn/回滚那段）替换为：

```rust
            let (revived, resolved_cwd, resume) =
                prepare_resume(&app, &session_id, cwd.as_deref(), &provider);
            let ok = spawn_in_terminal(&resume, resolved_cwd.as_deref(), &load_settings().resume_terminal);
            if !ok {
                // GUI 构建 stderr 不可见：回滚乐观复活，卡片立即回落「已断开」而非假连接 120s。
                if let Some(sid) = revived {
                    rollback_failed_resume(sid);
                }
                let _ = app.emit("board-changed", ());
            }
```

（删除该分支内原有的 `use std::os::windows::process::CommandExt; use std::process::Command; const CREATE_NEW_CONSOLE...; let dir = safe_cwd(...); let eff = ...; let spawned = match eff {...};` 整段——已移入 `spawn_in_terminal`。保留外层 `std::thread::spawn(move || { ... })` 与末尾 `Ok(())`。）

- [ ] **Step 3: resume_session 的 macOS 分支改调 helper**

把 macOS 分支线程体（`:1135-1151`）替换为：

```rust
            let (revived, resolved, resume) =
                prepare_resume(&app, &session_id, cwd.as_deref(), &provider);
            let ok = spawn_in_terminal(&resume, resolved.as_deref(), &load_settings().resume_terminal);
            if !ok {
                eprintln!("恢复会话：终端启动失败");
                if let Some(sid) = revived {
                    rollback_failed_resume(sid);
                }
                let _ = app.emit("board-changed", ());
            }
```

（原 `resume_session_mac(...)` 调用被 `spawn_in_terminal` 取代——后者内部按 terminal 选 kind 并调 `resume_session_mac`，等价。`resume_terminal_kind()` 保留供 `focus_session` 用，不动。）

- [ ] **Step 4: 编译 + 既有测试**

Run: `cargo build -p meowo-app && cargo test -p meowo-app`
Expected: 编译通过；既有纯函数测试（`shell_join_for_windows`/`safe_cwd`/`wt_default_profile` 等）全绿。

- [ ] **Step 5: 手动回归 resume（Windows）**

启动 app（`cd app && bun run tauri dev`），对一张已断开的 claude 卡片点击「恢复」→ 应照常在设置的终端里开出 `claude --resume <id>`，卡片乐观显示已连接。确认与改动前行为一致。

- [ ] **Step 6: Commit**

```bash
git add app/src-tauri/src/lib.rs
git commit -m "refactor(new-session): 抽 spawn_in_terminal 共享 helper，resume_session 改用（不改行为）"
```

---

## Task 5: new_session 命令

**Files:**
- Modify: `app/src-tauri/src/lib.rs`（新增纯函数 + 命令；注册进 `generate_handler!` `:2271`）

**Interfaces:**
- Consumes: `spawn_in_terminal`（Task 4）、`Agent::launch_args`（Task 1）、`load_settings`。
- Produces: 命令 `new_session(cwd: String, provider: String, terminal: Option<String>) -> Result<(), String>`；纯函数 `validate_new_session_cwd(cwd: &str) -> Result<String, String>`。

- [ ] **Step 1: 写失败测试（纯函数）**

在 `lib.rs` 现有测试 mod 里（若无则在文件末尾新增 `#[cfg(test)] mod new_session_tests`）加：

```rust
#[cfg(test)]
mod new_session_tests {
    use super::*;

    #[test]
    fn validate_cwd_rejects_empty_and_missing() {
        assert!(validate_new_session_cwd("").is_err());
        assert!(validate_new_session_cwd("   ").is_err());
        assert!(validate_new_session_cwd("C:/definitely/not/a/real/dir/xyz123").is_err());
    }

    #[test]
    fn validate_cwd_accepts_existing_dir() {
        let tmp = std::env::temp_dir();
        let got = validate_new_session_cwd(tmp.to_str().unwrap()).unwrap();
        assert_eq!(got, tmp.to_str().unwrap().trim());
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p meowo-app validate_cwd`
Expected: 编译失败 `cannot find function 'validate_new_session_cwd'`。

- [ ] **Step 3: 实现纯函数 + 命令**

在 `lib.rs`（`spawn_in_terminal` 之后、`resume_session` 之前的位置为宜）加：

```rust
/// 校验并归一「新建会话」的工作目录：非空、真实存在的目录。返回 trim 后的路径。
fn validate_new_session_cwd(cwd: &str) -> Result<String, String> {
    let d = cwd.trim();
    if d.is_empty() {
        return Err("请选择工作目录".into());
    }
    if !std::path::Path::new(d).is_dir() {
        return Err("目录不存在".into());
    }
    Ok(d.to_string())
}

/// 新建一个全新会话：在 `cwd` 打开终端裸启动指定 provider 的 CLI（无 session_id）。
/// 会话入库仍靠该 CLI 自己的 hook（claude/kimi 秒级，codex 首条消息后）——本命令只负责 spawn。
/// terminal 缺省用 settings.resume_terminal。spawn 放 blocking 线程池并 await，失败回传前端面板。
#[tauri::command]
async fn new_session(
    cwd: String,
    provider: String,
    terminal: Option<String>,
) -> Result<(), String> {
    let dir = validate_new_session_cwd(&cwd)?;
    let key = meowo_store::ProviderKey::parse(Some(&provider));
    let argv = meowo_reporter::agent::for_provider(key).launch_args();
    let term = terminal
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| load_settings().resume_terminal);
    // 冷启动首次 spawn 控制台子进程可达数秒；放 blocking 池不挡事件循环，同时能 await 结果回传。
    let ok = tauri::async_runtime::spawn_blocking(move || spawn_in_terminal(&argv, Some(&dir), &term))
        .await
        .map_err(|e| e.to_string())?;
    if ok {
        Ok(())
    } else {
        Err("启动终端失败：请确认所选 agent 已安装并在 PATH 中".into())
    }
}
```

- [ ] **Step 4: 注册命令**

在 `generate_handler!` 列表（`:2271` `available_terminals` 行）末尾加 `,` 与新命令：

```rust
            available_terminals,
            new_session
```

- [ ] **Step 5: 跑测试 + 编译**

Run: `cargo test -p meowo-app validate_cwd && cargo build -p meowo-app`
Expected: 测试 PASS；编译通过。

- [ ] **Step 6: Commit**

```bash
git add app/src-tauri/src/lib.rs
git commit -m "feat(new-session): new_session 命令（校验 cwd → launch_args → spawn_in_terminal）"
```

---

## Task 6: check_provider_hooks 命令

**Files:**
- Modify: `app/src-tauri/src/ccsetup.rs`（`claude_settings_path`/`parse_settings` 提 `pub(crate)`；加 `claude_hooks_installed`）
- Modify: `app/src-tauri/src/lib.rs`（`HooksStatus` + 纯函数 + 命令 + 注册）

**Interfaces:**
- Consumes: `ccsetup::reporter_path_from_hooks`（已 pub）、`meowo_reporter::codex::codex_home`、`meowo_reporter::kimi::kimi_share_dir`（均 pub）。
- Produces: 命令 `check_provider_hooks(provider: String) -> HooksStatus`；`HooksStatus { installed | missing | unknown }`（serde lowercase）；纯函数 `hooks_json_has_reporter(&Value, provider) -> bool`、`toml_text_has_reporter(text, provider) -> bool`。

- [ ] **Step 1: 写失败测试（纯函数）**

在 `lib.rs` 测试区加：

```rust
#[cfg(test)]
mod hooks_check_tests {
    use super::*;

    #[test]
    fn hooks_json_detects_reporter_with_provider() {
        let v: serde_json::Value = serde_json::from_str(r#"{
          "hooks": { "SessionStart": [
            { "matcher": "*", "hooks": [
              { "type": "command", "command": "\"C:/x/meowo-reporter.exe\" --provider codex", "timeout": 5 }
            ]}
          ]}
        }"#).unwrap();
        assert!(hooks_json_has_reporter(&v, "codex"));
        assert!(!hooks_json_has_reporter(&v, "kimi")); // provider 不符
    }

    #[test]
    fn hooks_json_ignores_foreign_hooks() {
        let v: serde_json::Value = serde_json::from_str(r#"{
          "hooks": { "Stop": [
            { "hooks": [{ "type": "command", "command": "node other.js" }] }
          ]}
        }"#).unwrap();
        assert!(!hooks_json_has_reporter(&v, "codex"));
        // 无 hooks 键。
        let empty: serde_json::Value = serde_json::from_str("{}").unwrap();
        assert!(!hooks_json_has_reporter(&empty, "codex"));
    }

    #[test]
    fn toml_text_detects_reporter_line() {
        let toml = "\
[[hooks]]\n\
event = \"SessionStart\"\n\
command = \"/home/u/.local/meowo-reporter --provider kimi\"\n\
timeout = 5\n";
        assert!(toml_text_has_reporter(toml, "kimi"));
        assert!(!toml_text_has_reporter(toml, "codex"));
        assert!(!toml_text_has_reporter("event = \"x\"\ncommand = \"node a.js\"", "kimi"));
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p meowo-app hooks_check`
Expected: 编译失败（函数未定义）。

- [ ] **Step 3: ccsetup 暴露 claude 检测**

在 `ccsetup.rs`：把 `claude_settings_path`（`:211`）与 `parse_settings`（`:206`）的 `fn` 改为 `pub(crate) fn`。在文件内（`resolve_reporter_native` 之后）加：

```rust
/// claude 的 meowo-reporter hooks 是否已接入 ~/.claude/settings.json。读/解析失败即视为未装。
pub fn claude_hooks_installed() -> bool {
    let Ok(text) = std::fs::read_to_string(claude_settings_path()) else {
        return false;
    };
    match parse_settings(&text) {
        Some(v) => reporter_path_from_hooks(&v).is_some(),
        None => false,
    }
}
```

- [ ] **Step 4: lib.rs 加 HooksStatus + 检测逻辑 + 命令**

在 `lib.rs` 加：

```rust
/// provider 的 meowo-reporter hooks 接入状态（供「新建会话」面板引导）。
#[derive(serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum HooksStatus {
    Installed,
    Missing,
    Unknown,
}

/// claude/codex 同款 hooks JSON 里是否存在指向 meowo-reporter 且带 `--provider <p>` 的 command。纯函数。
/// 启发式：命令含 "meowo-reporter"（basename）+ "--provider <p>"——该组合不会误配用户自有 hook。
fn hooks_json_has_reporter(v: &serde_json::Value, provider: &str) -> bool {
    let Some(hooks) = v.get("hooks").and_then(|h| h.as_object()) else {
        return false;
    };
    let want = format!("--provider {provider}");
    for (_event, arr) in hooks {
        for entry in arr.as_array().into_iter().flatten() {
            for h in entry.get("hooks").and_then(|x| x.as_array()).into_iter().flatten() {
                if let Some(cmd) = h.get("command").and_then(|x| x.as_str()) {
                    if cmd.to_ascii_lowercase().contains("meowo-reporter") && cmd.contains(&want) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// kimi config.toml 文本里是否有指向 meowo-reporter 且带 `--provider kimi` 的 hook 命令行。纯函数。
/// 无 TOML 解析库，按行文本启发式（meowo-reporter + --provider kimi + command 键，足够可靠）。
fn toml_text_has_reporter(text: &str, provider: &str) -> bool {
    let want = format!("--provider {provider}");
    text.lines().any(|l| {
        let low = l.to_ascii_lowercase();
        low.contains("meowo-reporter") && low.contains("command") && l.contains(&want)
    })
}

/// codex hooks 接入状态：读 ~/.codex/hooks.json。文件不存在=Missing；读/解析失败=Unknown（不误报）。
fn codex_hooks_status() -> HooksStatus {
    let Some(home) = meowo_reporter::codex::codex_home() else {
        return HooksStatus::Unknown;
    };
    let path = home.join("hooks.json");
    if !path.exists() {
        return HooksStatus::Missing;
    }
    let Ok(text) = std::fs::read_to_string(&path) else {
        return HooksStatus::Unknown;
    };
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(v) if hooks_json_has_reporter(&v, "codex") => HooksStatus::Installed,
        Ok(_) => HooksStatus::Missing,
        Err(_) => HooksStatus::Unknown,
    }
}

/// kimi hooks 接入状态：读 ~/.kimi-code/config.toml。文件不存在=Missing；读失败=Unknown。
fn kimi_hooks_status() -> HooksStatus {
    let Some(dir) = meowo_reporter::kimi::kimi_share_dir() else {
        return HooksStatus::Unknown;
    };
    let path = dir.join("config.toml");
    if !path.exists() {
        return HooksStatus::Missing;
    }
    match std::fs::read_to_string(&path) {
        Ok(text) if toml_text_has_reporter(&text, "kimi") => HooksStatus::Installed,
        Ok(_) => HooksStatus::Missing,
        Err(_) => HooksStatus::Unknown,
    }
}

/// 检测某 provider 的 meowo-reporter hooks 是否已接入（新建会话面板据此提示是否会入库）。
#[tauri::command]
fn check_provider_hooks(provider: String) -> HooksStatus {
    match meowo_store::ProviderKey::parse(Some(&provider)) {
        meowo_store::ProviderKey::Claude => {
            if ccsetup::claude_hooks_installed() {
                HooksStatus::Installed
            } else {
                HooksStatus::Missing
            }
        }
        meowo_store::ProviderKey::Codex => codex_hooks_status(),
        meowo_store::ProviderKey::Kimi => kimi_hooks_status(),
    }
}
```

- [ ] **Step 5: 注册命令**

在 `generate_handler!` 里 `new_session` 后加 `, check_provider_hooks`。

- [ ] **Step 6: 跑测试 + 编译**

Run: `cargo test -p meowo-app hooks_check && cargo build -p meowo-app`
Expected: 三个测试 PASS；编译通过。

- [ ] **Step 7: Commit**

```bash
git add app/src-tauri/src/ccsetup.rs app/src-tauri/src/lib.rs
git commit -m "feat(new-session): check_provider_hooks 检测三 provider 的 meowo-reporter hooks 接入"
```

---

## Task 7: recent_cwds 命令 + tauri-plugin-dialog 接入

**Files:**
- Modify: `app/src-tauri/src/lib.rs`（`recent_cwds` 命令 + 注册 + dialog plugin 注册 `:2239` 附近）
- Modify: `app/src-tauri/Cargo.toml`（`:21` 依赖区）
- Modify: `app/src-tauri/capabilities/default.json`
- Modify: `app/package.json`

**Interfaces:**
- Produces: 命令 `recent_cwds(limit: usize) -> Result<Vec<String>, String>`；前端可 `@tauri-apps/plugin-dialog` 的 `open`。

- [ ] **Step 1: recent_cwds 命令**

在 `lib.rs`（`get_overview` 附近，仿其 `async + spawn_blocking` 范式）加：

```rust
/// 「新建会话」面板的最近目录（去重+倒序）。
#[tauri::command]
async fn recent_cwds(state: State<'_, AppState>, limit: usize) -> Result<Vec<String>, String> {
    let db_path = state.db_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        open_store(&db_path)?.recent_cwds(limit).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
```

- [ ] **Step 2: 后端依赖**

在 `app/src-tauri/Cargo.toml` 的 `[dependencies]`（`tauri-plugin-process` 行 `:21` 下方）加：

```toml
tauri-plugin-dialog = "2"
```

- [ ] **Step 3: 注册 plugin + 命令**

在 `lib.rs` 的 builder plugin 链（`:2239` `.plugin(tauri_plugin_positioner::init())` 之后）加：

```rust
        .plugin(tauri_plugin_dialog::init())
```

在 `generate_handler!` 里 `check_provider_hooks` 后加 `, recent_cwds`。

- [ ] **Step 4: 授权 dialog**

在 `app/src-tauri/capabilities/default.json` 的 `permissions` 数组（`"process:default"` 行后）加：

```json
    "dialog:default"
```

（记得给前一行补逗号。`dialog:default` 含 `allow-open`，覆盖目录选择器。）

- [ ] **Step 5: 前端依赖**

Run: `cd app && bun add @tauri-apps/plugin-dialog`
Expected: `package.json` 的 `dependencies` 出现 `@tauri-apps/plugin-dialog`。

- [ ] **Step 6: 编译验证**

Run: `cargo build -p meowo-app`
Expected: 编译通过（dialog plugin 与新命令链接成功）。

- [ ] **Step 7: Commit**

```bash
git add app/src-tauri/Cargo.toml app/src-tauri/capabilities/default.json app/src-tauri/src/lib.rs app/package.json app/bun.lockb
git commit -m "feat(new-session): 接入 tauri-plugin-dialog + recent_cwds 命令（目录选择器 & 最近目录）"
```

---

## Task 8: 前端 api.ts wrappers 与类型

**Files:**
- Modify: `app/src/api.ts`（`ProviderKey` `:8` 后加 `PROVIDER_KEYS`；末尾加 `HooksStatus` + wrappers）

**Interfaces:**
- Produces: `PROVIDER_KEYS: ProviderKey[]`；`type HooksStatus = "installed" | "missing" | "unknown"`；`newSession(cwd, provider, terminal?)`、`recentCwds(limit)`、`checkProviderHooks(provider)`。

- [ ] **Step 1: 写失败测试**

新建 `app/src/api.newsession.test.ts`：

```ts
import { describe, it, expect, vi, beforeEach } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invokeMock(...a) }));

import { PROVIDER_KEYS, newSession, recentCwds, checkProviderHooks } from "./api";

beforeEach(() => invokeMock.mockReset());

describe("new-session api", () => {
  it("PROVIDER_KEYS 覆盖三个 provider", () => {
    expect([...PROVIDER_KEYS].sort()).toEqual(["claude", "codex", "kimi"]);
  });

  it("newSession 透传参数", () => {
    invokeMock.mockResolvedValue(undefined);
    newSession("C:/p", "claude", "wt");
    expect(invokeMock).toHaveBeenCalledWith("new_session", { cwd: "C:/p", provider: "claude", terminal: "wt" });
  });

  it("checkProviderHooks 传 provider", () => {
    invokeMock.mockResolvedValue("missing");
    checkProviderHooks("codex");
    expect(invokeMock).toHaveBeenCalledWith("check_provider_hooks", { provider: "codex" });
  });

  it("recentCwds 传 limit", () => {
    invokeMock.mockResolvedValue([]);
    recentCwds(8);
    expect(invokeMock).toHaveBeenCalledWith("recent_cwds", { limit: 8 });
  });
});
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd app && bunx vitest run src/api.newsession.test.ts`
Expected: 失败（`PROVIDER_KEYS`/函数未导出）。

- [ ] **Step 3: 实现**

在 `app/src/api.ts` 的 `DEFAULT_PROVIDER`（`:10`）之后加：

```ts
/** 所有 provider key（渲染新建面板 agent 选项用；与 ProviderKey 联合类型同步）。 */
export const PROVIDER_KEYS: ProviderKey[] = ["claude", "codex", "kimi"];
```

在文件末尾加：

```ts
/** 某 provider 的 meowo-reporter hooks 接入状态。unknown = 无法确认（读取失败/位置未知）。 */
export type HooksStatus = "installed" | "missing" | "unknown";

/** 新建一个全新会话：在 cwd 打开终端裸启动该 provider。terminal 省略则用设置里的默认终端。 */
export function newSession(cwd: string, provider: ProviderKey, terminal?: string): Promise<void> {
  return invoke("new_session", { cwd, provider, terminal });
}

/** 最近使用过的工作目录（新建面板快捷选择）。 */
export function recentCwds(limit: number): Promise<string[]> {
  return invoke("recent_cwds", { limit });
}

/** 检测某 provider 的 meowo-reporter hooks 是否已接入（决定新建后会不会入库）。 */
export function checkProviderHooks(provider: ProviderKey): Promise<HooksStatus> {
  return invoke("check_provider_hooks", { provider });
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cd app && bunx vitest run src/api.newsession.test.ts`
Expected: 4 个测试 PASS。

- [ ] **Step 5: Commit**

```bash
git add app/src/api.ts app/src/api.newsession.test.ts
git commit -m "feat(new-session): 前端 api wrappers（newSession/recentCwds/checkProviderHooks）"
```

---

## Task 9: i18n newSession 文案

**Files:**
- Modify: `app/src/i18n/zh.ts`（`newSession` 段）
- Modify: `app/src/i18n/en.ts`（对应，`typeof zh` 约束）

**Interfaces:**
- Produces: `t.newSession.*` 文案键（title/dir/browse/recent/agent/terminal/cancel/launch/launching/hooksMissing/codexDelay/errorTitle/newButton/emptyCta）。

- [ ] **Step 1: zh.ts 加段**

在 `app/src/i18n/zh.ts` 的 `sticker: {...}` 段之后（同级）插入：

```ts
  newSession: {
    title: "新建会话",
    dir: "工作目录",
    dirPlaceholder: "选择或输入项目目录",
    browse: "浏览…",
    recent: "最近",
    agent: "Agent",
    terminal: "终端",
    cancel: "取消",
    launch: "启动",
    launching: "正在启动…",
    launchedToast: (agent: string) => `正在启动 ${agent} 会话，卡片稍后出现`,
    launchedCodexToast: (agent: string) => `已启动 ${agent}，发送首条消息后卡片才会出现`,
    hooksMissing: "未检测到该 agent 的 hooks，新建后可能不会出现在看板",
    hooksUnknown: "无法确认该 agent 的 hooks 状态",
    hooksHelp: "如何接入",
    newButton: "新建会话",
    emptyCta: "新建会话",
  },
```

- [ ] **Step 2: en.ts 加对应段**

在 `app/src/i18n/en.ts` 相同位置插入（键与函数签名须与 zh 完全一致，否则 `typeof zh` 编译报错）：

```ts
  newSession: {
    title: "New session",
    dir: "Working directory",
    dirPlaceholder: "Pick or type a project directory",
    browse: "Browse…",
    recent: "Recent",
    agent: "Agent",
    terminal: "Terminal",
    cancel: "Cancel",
    launch: "Launch",
    launching: "Launching…",
    launchedToast: (agent: string) => `Launching ${agent} session; card will appear shortly`,
    launchedCodexToast: (agent: string) => `Launched ${agent}; the card appears after the first message`,
    hooksMissing: "No hooks detected for this agent; the session may not appear on the board",
    hooksUnknown: "Can't confirm this agent's hooks status",
    hooksHelp: "How to set up",
    newButton: "New session",
    emptyCta: "New session",
  },
```

- [ ] **Step 3: 类型检查**

Run: `cd app && bunx tsc --noEmit`
Expected: 无错误（zh/en key 对齐）。

- [ ] **Step 4: Commit**

```bash
git add app/src/i18n/zh.ts app/src/i18n/en.ts
git commit -m "feat(new-session): 新建会话面板 i18n 文案（中英）"
```

---

## Task 10: NewSessionPanel 组件

**Files:**
- Create: `app/src/views/NewSessionPanel.tsx`
- Create: `app/src/views/NewSessionPanel.test.tsx`
- Modify: `app/src/styles.css`（面板样式）

**Interfaces:**
- Consumes: `newSession`/`recentCwds`/`checkProviderHooks`/`availableTerminals`/`getSettings`/`PROVIDER_KEYS`/`HooksStatus`/`ResumeTerminal`（api.ts）、`providerConfig`（providers.tsx）、`useT`（i18n）、`@tauri-apps/plugin-dialog` 的 `open`。
- Produces: `export function NewSessionPanel({ onClose, onLaunched }: { onClose: () => void; onLaunched: (msg: string) => void }): ReactElement`。

- [ ] **Step 1: 写失败测试**

新建 `app/src/views/NewSessionPanel.test.tsx`：

```tsx
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

const api = {
  newSession: vi.fn(),
  recentCwds: vi.fn(),
  checkProviderHooks: vi.fn(),
  availableTerminals: vi.fn(),
  getSettings: vi.fn(),
};
vi.mock("../api", async (orig) => ({ ...(await orig<typeof import("../api")>()), ...api }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

import { NewSessionPanel } from "./NewSessionPanel";

beforeEach(() => {
  Object.values(api).forEach((m) => m.mockReset());
  api.recentCwds.mockResolvedValue([]);
  api.checkProviderHooks.mockResolvedValue("installed");
  api.availableTerminals.mockResolvedValue(["wt"]);
  api.getSettings.mockResolvedValue({ default_agent: "claude", resume_terminal: "wt" });
});

describe("NewSessionPanel", () => {
  it("目录为空时启动禁用", async () => {
    render(<NewSessionPanel onClose={() => {}} onLaunched={() => {}} />);
    const launch = await screen.findByTestId("ns-launch");
    expect(launch).toBeDisabled();
  });

  it("填目录后启动调 newSession 并回调", async () => {
    api.newSession.mockResolvedValue(undefined);
    const onLaunched = vi.fn();
    render(<NewSessionPanel onClose={() => {}} onLaunched={onLaunched} />);
    fireEvent.change(await screen.findByTestId("ns-dir"), { target: { value: "C:/proj" } });
    fireEvent.click(screen.getByTestId("ns-launch"));
    await waitFor(() => expect(api.newSession).toHaveBeenCalledWith("C:/proj", "claude", "wt"));
    await waitFor(() => expect(onLaunched).toHaveBeenCalled());
  });

  it("hooks 未装显示警告", async () => {
    api.checkProviderHooks.mockResolvedValue("missing");
    render(<NewSessionPanel onClose={() => {}} onLaunched={() => {}} />);
    expect(await screen.findByTestId("ns-hooks-warn")).toBeInTheDocument();
  });

  it("启动失败显示错误、不回调", async () => {
    api.newSession.mockRejectedValue("启动终端失败");
    const onLaunched = vi.fn();
    render(<NewSessionPanel onClose={() => {}} onLaunched={onLaunched} />);
    fireEvent.change(await screen.findByTestId("ns-dir"), { target: { value: "C:/proj" } });
    fireEvent.click(screen.getByTestId("ns-launch"));
    expect(await screen.findByTestId("ns-error")).toHaveTextContent("启动终端失败");
    expect(onLaunched).not.toHaveBeenCalled();
  });
});
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd app && bunx vitest run src/views/NewSessionPanel.test.tsx`
Expected: 失败（模块不存在）。

- [ ] **Step 3: 实现组件**

新建 `app/src/views/NewSessionPanel.tsx`：

```tsx
import { type ReactElement, useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  type ProviderKey,
  type HooksStatus,
  type ResumeTerminal,
  PROVIDER_KEYS,
  newSession,
  recentCwds,
  checkProviderHooks,
  availableTerminals,
  getSettings,
} from "../api";
import { providerConfig } from "../providers";
import { useT } from "../i18n";

export function NewSessionPanel({
  onClose,
  onLaunched,
}: {
  onClose: () => void;
  onLaunched: (msg: string) => void;
}): ReactElement {
  const t = useT();
  const [cwd, setCwd] = useState("");
  const [provider, setProvider] = useState<ProviderKey>("claude");
  const [terminal, setTerminal] = useState<ResumeTerminal | "">("");
  const [terms, setTerms] = useState<ResumeTerminal[]>([]);
  const [recent, setRecent] = useState<string[]>([]);
  const [hooks, setHooks] = useState<Record<string, HooksStatus>>({});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // 初始：默认 agent/终端来自设置；最近目录、可用终端、各 provider hooks 状态。
  useEffect(() => {
    getSettings().then((s) => {
      setProvider(s.default_agent);
      setTerminal(s.resume_terminal);
    });
    recentCwds(8).then(setRecent).catch(() => {});
    availableTerminals().then(setTerms).catch(() => {});
    PROVIDER_KEYS.forEach((p) =>
      checkProviderHooks(p)
        .then((st) => setHooks((h) => ({ ...h, [p]: st })))
        .catch(() => {}),
    );
  }, []);

  async function pickDir() {
    const picked = await open({ directory: true });
    if (typeof picked === "string") setCwd(picked);
  }

  async function launch() {
    if (!cwd.trim() || busy) return;
    setBusy(true);
    setError(null);
    try {
      await newSession(cwd.trim(), provider, terminal || undefined);
      const label = providerConfig(provider).label(t);
      const msg =
        provider === "codex"
          ? t.newSession.launchedCodexToast(label)
          : t.newSession.launchedToast(label);
      onLaunched(msg);
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  }

  const warn = hooks[provider] === "missing" || hooks[provider] === "unknown";

  return (
    <div className="ns-overlay" onMouseDown={onClose}>
      <div className="ns-modal" onMouseDown={(e) => e.stopPropagation()}>
        <div className="ns-title">{t.newSession.title}</div>

        <label className="ns-field">
          <span className="ns-label">{t.newSession.dir}</span>
          <div className="ns-dir-row">
            <input
              className="ns-input"
              data-testid="ns-dir"
              value={cwd}
              placeholder={t.newSession.dirPlaceholder}
              onChange={(e) => setCwd(e.target.value)}
            />
            <button type="button" className="ns-btn" onClick={pickDir}>
              {t.newSession.browse}
            </button>
          </div>
          {recent.length > 0 && (
            <div className="ns-recent">
              <span className="ns-recent-lbl">{t.newSession.recent}</span>
              {recent.map((r) => (
                <button key={r} type="button" className="ns-chip" title={r} onClick={() => setCwd(r)}>
                  {r.split(/[\\/]/).filter(Boolean).pop() ?? r}
                </button>
              ))}
            </div>
          )}
        </label>

        <div className="ns-field">
          <span className="ns-label">{t.newSession.agent}</span>
          <div className="ns-agents">
            {PROVIDER_KEYS.map((p) => {
              const cfg = providerConfig(p);
              return (
                <button
                  key={p}
                  type="button"
                  className={"ns-agent" + (provider === p ? " is-on" : "")}
                  onClick={() => setProvider(p)}
                >
                  <cfg.Icon />
                  <span>{cfg.label(t)}</span>
                </button>
              );
            })}
          </div>
          {warn && (
            <div className="ns-warn" data-testid="ns-hooks-warn">
              {hooks[provider] === "unknown" ? t.newSession.hooksUnknown : t.newSession.hooksMissing}
            </div>
          )}
        </div>

        {terms.length >= 2 && (
          <label className="ns-field">
            <span className="ns-label">{t.newSession.terminal}</span>
            <select
              className="ns-input"
              value={terminal}
              onChange={(e) => setTerminal(e.target.value as ResumeTerminal)}
            >
              {terms.map((tm) => (
                <option key={tm} value={tm}>
                  {tm}
                </option>
              ))}
            </select>
          </label>
        )}

        {error && (
          <div className="ns-error" data-testid="ns-error">
            {error}
          </div>
        )}

        <div className="ns-actions">
          <button type="button" className="ns-btn" onClick={onClose}>
            {t.newSession.cancel}
          </button>
          <button
            type="button"
            className="ns-btn is-primary"
            data-testid="ns-launch"
            disabled={!cwd.trim() || busy}
            onClick={launch}
          >
            {busy ? t.newSession.launching : t.newSession.launch}
          </button>
        </div>
      </div>
    </div>
  );
}
```

- [ ] **Step 4: 加样式**

在 `app/src/styles.css` 末尾追加（配色沿用主题的中性半透明，必要时对齐已有 `--` 变量）：

```css
/* 新建会话面板 */
.ns-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.45);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 50;
}
.ns-modal {
  width: 320px;
  max-width: calc(100vw - 24px);
  background: var(--card-bg, #1e1e22);
  color: var(--fg, #e8e8ea);
  border-radius: 12px;
  padding: 16px;
  box-shadow: 0 12px 40px rgba(0, 0, 0, 0.5);
  display: flex;
  flex-direction: column;
  gap: 12px;
}
.ns-title { font-weight: 600; font-size: 14px; }
.ns-field { display: flex; flex-direction: column; gap: 6px; }
.ns-label { font-size: 11px; opacity: 0.7; }
.ns-dir-row { display: flex; gap: 6px; }
.ns-input {
  flex: 1;
  min-width: 0;
  padding: 6px 8px;
  border-radius: 8px;
  border: 1px solid rgba(255, 255, 255, 0.14);
  background: rgba(255, 255, 255, 0.05);
  color: inherit;
  font-size: 12px;
}
.ns-recent { display: flex; flex-wrap: wrap; align-items: center; gap: 4px; }
.ns-recent-lbl { font-size: 10px; opacity: 0.5; }
.ns-chip, .ns-btn, .ns-agent {
  border: 1px solid rgba(255, 255, 255, 0.14);
  background: rgba(255, 255, 255, 0.05);
  color: inherit;
  border-radius: 8px;
  cursor: pointer;
  font-size: 12px;
}
.ns-chip { padding: 3px 8px; max-width: 120px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.ns-btn { padding: 6px 12px; }
.ns-btn.is-primary { background: var(--accent, #c96442); border-color: transparent; color: #fff; }
.ns-btn:disabled { opacity: 0.45; cursor: not-allowed; }
.ns-agents { display: flex; gap: 6px; }
.ns-agent { display: flex; align-items: center; gap: 6px; padding: 6px 10px; flex: 1; justify-content: center; }
.ns-agent.is-on { border-color: var(--accent, #c96442); }
.ns-warn { font-size: 11px; color: #e0a33e; }
.ns-error { font-size: 11px; color: #e5534b; }
.ns-actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 4px; }
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cd app && bunx vitest run src/views/NewSessionPanel.test.tsx`
Expected: 4 个测试全 PASS。

- [ ] **Step 6: Commit**

```bash
git add app/src/views/NewSessionPanel.tsx app/src/views/NewSessionPanel.test.tsx app/src/styles.css
git commit -m "feat(new-session): NewSessionPanel 迷你面板（目录/agent/终端/hooks 引导）"
```

---

## Task 11: Sticker 接线（入口 + toast）

**Files:**
- Modify: `app/src/views/Sticker.tsx`（import；主组件 state；底部栏按钮 `:1142`；空状态 `EmptyState` `:453` + 渲染点；toast + 面板挂载）

**Interfaces:**
- Consumes: `NewSessionPanel`（Task 10）。
- Produces: `EmptyState` 增可选 `onNew?: () => void`。

- [ ] **Step 1: 写失败测试（入口打开面板）**

新建 `app/src/views/Sticker.newsession.test.tsx`（只验证底部栏「新建」按钮打开面板；对 Sticker 现有数据依赖按需 mock）：

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { EmptyState } from "./Sticker";

// EmptyState 的 onNew CTA 打开新建（最小、稳定的接线单元测试）。
describe("EmptyState 新建 CTA", () => {
  it("有 onNew 时渲染 CTA 并可点击", () => {
    const onNew = vi.fn();
    render(<EmptyState tab="all" onNew={onNew} />);
    fireEvent.click(screen.getByTestId("empty-new-cta"));
    expect(onNew).toHaveBeenCalled();
  });

  it("无 onNew 时不渲染 CTA", () => {
    render(<EmptyState tab="all" />);
    expect(screen.queryByTestId("empty-new-cta")).toBeNull();
  });
});
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd app && bunx vitest run src/views/Sticker.newsession.test.tsx`
Expected: 失败（`EmptyState` 无 `onNew` prop / CTA 不存在）。

- [ ] **Step 3: EmptyState 加 onNew CTA**

改 `Sticker.tsx` 的 `EmptyState`（`:453-463`）：

```tsx
export function EmptyState({ tab, onNew }: { tab: Tab; onNew?: () => void }) {
  const t = useT();
  const { title, hint } = emptyCopy(tab, t);
  return (
    <div className="stk-empty">
      <span className="stk-empty-icon"><EmptyIcon tab={tab} /></span>
      <div className="stk-empty-title">{title}</div>
      {hint && <div className="stk-empty-hint">{hint}</div>}
      {onNew && (
        <button type="button" className="stk-empty-cta" data-testid="empty-new-cta" onClick={onNew}>
          {t.newSession.emptyCta}
        </button>
      )}
    </div>
  );
}
```

- [ ] **Step 4: import + 主组件 state + 面板/toast 挂载**

在 `Sticker.tsx` 顶部 import 区（`:25` `providerConfig` 附近）加：

```tsx
import { NewSessionPanel } from "./NewSessionPanel";
```

在主 Sticker 组件（渲染底栏 `.stk-bar` 的那个组件，`return` 前）加 state：

```tsx
  const [newOpen, setNewOpen] = useState(false);
  const [toast, setToast] = useState<string | null>(null);
  function showToast(msg: string) {
    setToast(msg);
    window.setTimeout(() => setToast(null), 4000);
  }
```

在该组件根 `<div>` 末尾（`:1169` `</div>` 底栏之后、组件最外层 `</div>` 之前）挂面板与 toast：

```tsx
      {newOpen && (
        <NewSessionPanel
          onClose={() => setNewOpen(false)}
          onLaunched={(msg) => {
            setNewOpen(false);
            showToast(msg);
          }}
        />
      )}
      {toast && <div className="stk-toast" role="status">{toast}</div>}
```

- [ ] **Step 5: 底部栏「新建」按钮**

在底栏 `.stk-bar-actions`（`:1142`）里、搜索按钮（`:1143`）之前加：

```tsx
              <span
                className="stk-act"
                data-tip={t.newSession.newButton}
                aria-label={t.newSession.newButton}
                data-testid="bar-new"
                onClick={() => setNewOpen(true)}
              >
                <PlusIcon />
              </span>
```

在 Sticker.tsx 的图标区（`PencilIcon` 等旁，`:70` 附近）加 `PlusIcon`：

```tsx
function PlusIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
      strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M12 5v14M5 12h14" />
    </svg>
  );
}
```

- [ ] **Step 6: 空状态渲染点传 onNew**

Run: `cd app && grep -n "<EmptyState" src/views/Sticker.tsx`

在其渲染处把 `<EmptyState tab={...} />` 改为 `<EmptyState tab={...} onNew={() => setNewOpen(true)} />`。

- [ ] **Step 7: toast/CTA 样式**

在 `app/src/styles.css` 末尾加：

```css
.stk-empty-cta {
  margin-top: 10px;
  padding: 6px 14px;
  border-radius: 8px;
  border: 1px solid var(--accent, #c96442);
  background: transparent;
  color: inherit;
  cursor: pointer;
  font-size: 12px;
}
.stk-toast {
  position: fixed;
  left: 50%;
  bottom: 16px;
  transform: translateX(-50%);
  background: rgba(0, 0, 0, 0.82);
  color: #fff;
  padding: 8px 14px;
  border-radius: 8px;
  font-size: 12px;
  z-index: 60;
  max-width: calc(100vw - 24px);
  text-align: center;
}
```

- [ ] **Step 8: 跑测试 + 全量前端测试 + 构建**

Run: `cd app && bunx vitest run src/views/Sticker.newsession.test.tsx && bunx tsc --noEmit`
Expected: EmptyState 两个测试 PASS；类型检查通过。

- [ ] **Step 9: Commit**

```bash
git add app/src/views/Sticker.tsx app/src/styles.css
git commit -m "feat(new-session): 底部栏新建按钮 + 空状态 CTA + 瞬态 toast，挂载面板"
```

---

## Task 12（可选）: 设置页默认 agent 下拉

spec 标注为增强项，可跳过。若做：

**Files:**
- Modify: `app/src/views/About.tsx`（`GeneralSection` 的下拉区）

- [ ] **Step 1: 加下拉**

在 `About.tsx` 的 `GeneralSection` 里、终端下拉（`resume_terminal`）附近，仿其结构加一个 `default_agent` 下拉：`PROVIDER_KEYS.map` 渲染选项，`value={settings.default_agent}`，`onChange` 调 `setSettings({ ...settings, default_agent: e.target.value })`。文案用 `t.newSession.agent` 或新增 `t.settings.defaultAgent`（如新增，zh/en 同步）。

- [ ] **Step 2: 类型检查 + 手动**

Run: `cd app && bunx tsc --noEmit`
Expected: 通过。手动：改默认 agent 后打开新建面板，默认选中项随之变化。

- [ ] **Step 3: Commit**

```bash
git add app/src/views/About.tsx app/src/i18n/zh.ts app/src/i18n/en.ts
git commit -m "feat(new-session): 设置页默认 agent 下拉（可选增强）"
```

---

## 最终验收（全 task 完成后）

- [ ] **Rust 全绿：** `cargo test`（三 crate）+ `cargo clippy --all-targets -- -D warnings`
- [ ] **前端全绿：** `cd app && bunx vitest run && bunx tsc --noEmit`
- [ ] **手动三 provider：**
  - claude：新建 → 秒级出卡。
  - kimi：新建 → 秒级出卡（hooks 已装时）；未装 hooks 时面板 ⚠ + toast 引导。
  - codex：新建 → toast 提示「发首条消息后出现」；发一条消息后出卡。
- [ ] **回归：** resume（恢复会话）三 provider 仍正常（Task 4 未回归）。
- [ ] **错误路径：** 目录留空启动禁用；填不存在目录 → 面板报错；PATH 无该 agent → toast/面板报「未找到」。

---

# 修订 R1：改为独立窗口（替代 overlay）

> 用户执行中途（T1–T10 完成后）决定把面板从「看板内模态弹层」改为**独立 WebviewWindow**。见 spec 修订 R1。
> 原 Task 10 的 `NewSessionPanel`（overlay 版）由 Task 13 改造；原 Task 11（overlay 接线）作废，由 Task 14 替代。后端命令/api/i18n（Task 1–9）零改动。原 Task 12（设置页默认 agent）仍可选，编号顺延到最后。

## Task 13: 独立窗口——开窗命令 + 面板改造

**Files:**
- Modify: `app/src-tauri/src/lib.rs`（`open_new_session_window` 命令 + impl + 注册）
- Modify: `app/src-tauri/src/settings.rs`（`tr()` 加 `window.newSession` 中英）
- Modify: `app/src-tauri/capabilities/default.json`（`windows` 加 `"new-session"`）
- Modify: `app/src/main.tsx`（label 路由）
- Modify: `app/src/views/NewSessionPanel.tsx`（overlay → 独立窗口页）
- Modify: `app/src/views/NewSessionPanel.test.tsx`（改 mock：emit + getCurrentWindow().close）
- Modify: `app/src/styles.css`（删 `.ns-overlay`/`.ns-modal`，加 `.ns-window`/`.ns-titlebar`/`.ns-close`/`.ns-body`；表单内部样式不动）

**Interfaces:**
- Produces: 命令 `open_new_session_window()`；窗口 label `"new-session"`；成功事件 `emit("new-session-launched", msg: string)`（Task 14 监听）。

- [ ] **Step 1: 后端开窗命令**（`lib.rs`，仿 `open_settings`/`open_update_window`，放其附近）

```rust
/// 前端调用：打开「新建会话」窗口（贴纸底栏 + 按钮 / 空状态 CTA）。
/// 与 open_settings 同理由走子线程创建：同步 command 在主线程 build 会阻塞消息泵致白屏。
#[tauri::command]
fn open_new_session_window(app: tauri::AppHandle) {
    std::thread::spawn(move || open_new_session_window_impl(&app));
}

/// 打开（或聚焦）新建会话窗口。label 为 "new-session"（main.tsx 按此 label 路由到面板页）。
fn open_new_session_window_impl(app: &tauri::AppHandle) {
    // macOS：纯托盘 App 的窗口需临时切 Regular 激活策略才能获焦（同设置窗口）。
    #[cfg(target_os = "macos")]
    crate::macos::menubar::settings_window_will_open(app);

    if let Some(w) = app.get_webview_window("new-session") {
        let _ = w.set_focus();
        return;
    }
    let builder = tauri::WebviewWindowBuilder::new(
        app,
        "new-session",
        tauri::WebviewUrl::App("index.html".into()),
    )
    .title(tr(ui_lang(&load_settings()), "window.newSession"))
    .inner_size(440.0, 380.0)
    .min_inner_size(440.0, 380.0)
    .resizable(false)
    .decorations(false)
    .center();
    // macOS：无边框窗口不自动圆角，设透明由前端 .ns-window 的 border-radius 呈现（同设置窗口）。
    #[cfg(target_os = "macos")]
    let builder = builder.transparent(true);
    match builder.build() {
        Ok(_win) => {
            #[cfg(target_os = "macos")]
            {
                let app_handle = app.clone();
                _win.on_window_event(move |e| {
                    if matches!(
                        e,
                        tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Destroyed
                    ) {
                        crate::macos::menubar::settings_window_did_close(&app_handle);
                    }
                });
            }
        }
        Err(e) => eprintln!("创建新建会话窗口失败: {e}"),
    }
}
```

在 `generate_handler!` 里加 `open_new_session_window`（放 `recent_cwds` 后）。

- [ ] **Step 2: 窗口标题文案**（`settings.rs` 的 `tr()`）

在 `window.updater` 的中英两行附近各加一行：

```rust
        ("en", "window.newSession") => "New Session",
```
（英文块内，与 `("en", "window.updater") => ...` 同处）

```rust
        (_, "window.newSession") => "新建会话",
```
（默认块内，与 `(_, "window.updater") => ...` 同处）

- [ ] **Step 3: 授权新窗口**（`capabilities/default.json`）

`windows` 数组从 `["main", "about", "updater"]` 改为：

```json
  "windows": ["main", "about", "updater", "new-session"],
```

- [ ] **Step 4: 前端路由**（`main.tsx`）

顶部 import：

```tsx
import { NewSessionPanel } from "./views/NewSessionPanel";
```

渲染分支（`label === "updater" ? <Updater /> :` 之后加一支）：

```tsx
        {label === "about" ? (
          <About />
        ) : label === "updater" ? (
          <Updater />
        ) : label === "new-session" ? (
          <NewSessionPanel />
        ) : (
          <App />
        )}
```

- [ ] **Step 5: 面板改造为独立窗口页**（`NewSessionPanel.tsx` 整体替换）

```tsx
import { type ReactElement, useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { emit } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  type ProviderKey,
  type HooksStatus,
  type ResumeTerminal,
  PROVIDER_KEYS,
  newSession,
  recentCwds,
  checkProviderHooks,
  availableTerminals,
  getSettings,
} from "../api";
import { providerConfig } from "../providers";
import { useT } from "../i18n";

/** 独立窗口页（label="new-session"）：新建一个全新会话。成功后 emit 通知主看板弹 toast 并自关。 */
export function NewSessionPanel(): ReactElement {
  const t = useT();
  const [cwd, setCwd] = useState("");
  const [provider, setProvider] = useState<ProviderKey>("claude");
  const [terminal, setTerminal] = useState<ResumeTerminal | "">("");
  const [terms, setTerms] = useState<ResumeTerminal[]>([]);
  const [recent, setRecent] = useState<string[]>([]);
  const [hooks, setHooks] = useState<Record<string, HooksStatus>>({});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    getSettings()
      .then((s) => {
        setProvider(s.default_agent);
        setTerminal(s.resume_terminal);
      })
      .catch(() => {});
    recentCwds(8).then(setRecent).catch(() => {});
    availableTerminals().then(setTerms).catch(() => {});
    PROVIDER_KEYS.forEach((p) =>
      checkProviderHooks(p)
        .then((st) => setHooks((h) => ({ ...h, [p]: st })))
        .catch(() => {}),
    );
  }, []);

  function closeWin() {
    getCurrentWindow().close();
  }

  async function pickDir() {
    const picked = await open({ directory: true });
    if (typeof picked === "string") setCwd(picked);
  }

  async function launch() {
    if (!cwd.trim() || busy) return;
    setBusy(true);
    setError(null);
    try {
      await newSession(cwd.trim(), provider, terminal || undefined);
      const label = providerConfig(provider).label(t);
      const msg =
        provider === "codex"
          ? t.newSession.launchedCodexToast(label)
          : t.newSession.launchedToast(label);
      await emit("new-session-launched", msg);
      closeWin();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  }

  const warn = hooks[provider] === "missing" || hooks[provider] === "unknown";

  return (
    <div className="ns-window">
      <div className="ns-titlebar" data-tauri-drag-region>
        <span className="ns-title">{t.newSession.title}</span>
        <button type="button" className="ns-close" aria-label={t.newSession.cancel} onClick={closeWin}>
          ×
        </button>
      </div>

      <div className="ns-body">
        <label className="ns-field">
          <span className="ns-label">{t.newSession.dir}</span>
          <div className="ns-dir-row">
            <input
              className="ns-input"
              data-testid="ns-dir"
              value={cwd}
              placeholder={t.newSession.dirPlaceholder}
              onChange={(e) => setCwd(e.target.value)}
            />
            <button type="button" className="ns-btn" onClick={pickDir}>
              {t.newSession.browse}
            </button>
          </div>
          {recent.length > 0 && (
            <div className="ns-recent">
              <span className="ns-recent-lbl">{t.newSession.recent}</span>
              {recent.map((r) => (
                <button key={r} type="button" className="ns-chip" title={r} onClick={() => setCwd(r)}>
                  {r.split(/[\\/]/).filter(Boolean).pop() ?? r}
                </button>
              ))}
            </div>
          )}
        </label>

        <div className="ns-field">
          <span className="ns-label">{t.newSession.agent}</span>
          <div className="ns-agents">
            {PROVIDER_KEYS.map((p) => {
              const cfg = providerConfig(p);
              return (
                <button
                  key={p}
                  type="button"
                  className={"ns-agent" + (provider === p ? " is-on" : "")}
                  onClick={() => setProvider(p)}
                >
                  <cfg.Icon />
                  <span>{cfg.label(t)}</span>
                </button>
              );
            })}
          </div>
          {warn && (
            <div className="ns-warn" data-testid="ns-hooks-warn">
              {hooks[provider] === "unknown" ? t.newSession.hooksUnknown : t.newSession.hooksMissing}
            </div>
          )}
        </div>

        {terms.length >= 2 && (
          <label className="ns-field">
            <span className="ns-label">{t.newSession.terminal}</span>
            <select
              className="ns-input"
              value={terminal}
              onChange={(e) => setTerminal(e.target.value as ResumeTerminal)}
            >
              {terms.map((tm) => (
                <option key={tm} value={tm}>
                  {tm}
                </option>
              ))}
            </select>
          </label>
        )}

        {error && (
          <div className="ns-error" data-testid="ns-error">
            {error}
          </div>
        )}
      </div>

      <div className="ns-actions">
        <button type="button" className="ns-btn" onClick={closeWin}>
          {t.newSession.cancel}
        </button>
        <button
          type="button"
          className="ns-btn is-primary"
          data-testid="ns-launch"
          disabled={!cwd.trim() || busy}
          onClick={launch}
        >
          {busy ? t.newSession.launching : t.newSession.launch}
        </button>
      </div>
    </div>
  );
}
```

- [ ] **Step 6: 测试改造**（`NewSessionPanel.test.tsx`）

面板不再有 props；改为断言 `emit` + `getCurrentWindow().close`。保留 api mock（沿用 T10 已修好的 `vi.hoisted` + 原生断言 + `afterEach(cleanup)` 写法），新增 window/event mock：

```tsx
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor, cleanup } from "@testing-library/react";

const api = {
  newSession: vi.fn(),
  recentCwds: vi.fn(),
  checkProviderHooks: vi.fn(),
  availableTerminals: vi.fn(),
  getSettings: vi.fn(),
};
vi.mock("../api", async (orig) => ({ ...(await orig<typeof import("../api")>()), ...api }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
const closeMock = vi.fn();
const emitMock = vi.fn();
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ close: closeMock }) }));
vi.mock("@tauri-apps/api/event", () => ({ emit: (...a: unknown[]) => emitMock(...a) }));

import { NewSessionPanel } from "./NewSessionPanel";

beforeEach(() => {
  Object.values(api).forEach((m) => m.mockReset());
  closeMock.mockReset();
  emitMock.mockReset();
  api.recentCwds.mockResolvedValue([]);
  api.checkProviderHooks.mockResolvedValue("installed");
  api.availableTerminals.mockResolvedValue(["wt"]);
  api.getSettings.mockResolvedValue({ default_agent: "claude", resume_terminal: "wt" });
});
afterEach(() => cleanup());

describe("NewSessionPanel (独立窗口)", () => {
  it("目录为空时启动禁用", async () => {
    render(<NewSessionPanel />);
    const launch = await screen.findByTestId("ns-launch");
    expect((launch as HTMLButtonElement).disabled).toBe(true);
  });

  it("填目录后启动调 newSession → emit → 关窗", async () => {
    api.newSession.mockResolvedValue(undefined);
    render(<NewSessionPanel />);
    fireEvent.change(await screen.findByTestId("ns-dir"), { target: { value: "C:/proj" } });
    fireEvent.click(screen.getByTestId("ns-launch"));
    await waitFor(() => expect(api.newSession).toHaveBeenCalledWith("C:/proj", "claude", "wt"));
    await waitFor(() => expect(emitMock).toHaveBeenCalledWith("new-session-launched", expect.any(String)));
    await waitFor(() => expect(closeMock).toHaveBeenCalled());
  });

  it("hooks 未装显示警告", async () => {
    api.checkProviderHooks.mockResolvedValue("missing");
    render(<NewSessionPanel />);
    expect(await screen.findByTestId("ns-hooks-warn")).toBeTruthy();
  });

  it("启动失败显示错误，不 emit、不关窗", async () => {
    api.newSession.mockRejectedValue("启动终端失败");
    render(<NewSessionPanel />);
    fireEvent.change(await screen.findByTestId("ns-dir"), { target: { value: "C:/proj" } });
    fireEvent.click(screen.getByTestId("ns-launch"));
    expect((await screen.findByTestId("ns-error")).textContent).toContain("启动终端失败");
    expect(emitMock).not.toHaveBeenCalled();
    expect(closeMock).not.toHaveBeenCalled();
  });
});
```

- [ ] **Step 7: 样式**（`styles.css`）

删除 `.ns-overlay` 与 `.ns-modal` 两条规则；新增（表单内部 `.ns-field`/`.ns-input`/`.ns-recent`/`.ns-chip`/`.ns-btn`/`.ns-agents`/`.ns-agent`/`.ns-warn`/`.ns-error` **保持不变**）：

```css
/* 新建会话独立窗口 */
.ns-window {
  width: 100vw;
  height: 100vh;
  display: flex;
  flex-direction: column;
  background: var(--cc-surface, #2e2e2c);
  color: var(--cc-text, #e8e8ea);
  border-radius: var(--r-lg, 12px);
  overflow: hidden;
}
.ns-titlebar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 8px 12px;
  user-select: none;
  -webkit-user-select: none;
}
.ns-title {
  font-weight: 600;
  font-size: 13px;
}
.ns-close {
  border: none;
  background: transparent;
  color: inherit;
  cursor: pointer;
  font-size: 18px;
  line-height: 1;
  padding: 2px 8px;
  border-radius: var(--r-md, 8px);
}
.ns-close:hover {
  background: var(--cc-surface-hover, rgba(255, 255, 255, 0.08));
}
.ns-body {
  flex: 1;
  overflow-y: auto;
  padding: 4px 16px 12px;
  display: flex;
  flex-direction: column;
  gap: 12px;
}
.ns-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  padding: 10px 16px;
}
```

- [ ] **Step 8: 验证**

Run: `cargo build -p meowo-app`（若本机 meowo-app.exe 在运行导致 exe 链接 os error 5，退而用 `cargo check -p meowo-app` + `cargo build -p meowo-app --lib`，report 注明）
Run: `cd app && bunx vitest run src/views/NewSessionPanel.test.tsx && bunx tsc --noEmit`
Expected: 后端编译通过；4 测试 PASS；tsc 无错误。

- [ ] **Step 9: Commit**

```bash
git add app/src-tauri/src/lib.rs app/src-tauri/src/settings.rs app/src-tauri/capabilities/default.json app/src/main.tsx app/src/views/NewSessionPanel.tsx app/src/views/NewSessionPanel.test.tsx app/src/styles.css
git commit -m "feat(new-session): 新建会话改为独立窗口（开窗命令 + 面板页 + emit 反馈）"
```

---

## Task 14: 主看板接线（入口 + toast）

**Files:**
- Modify: `app/src/views/Sticker.tsx`（EmptyState CTA + 底部栏按钮 + PlusIcon + toast 监听）
- Create: `app/src/views/Sticker.newsession.test.tsx`
- Modify: `app/src/styles.css`（`.stk-empty-cta` + `.stk-toast`）

**Interfaces:**
- Consumes: 命令 `open_new_session_window`（Task 13）；事件 `new-session-launched`（Task 13 emit）。

- [ ] **Step 1: 写失败测试**（`Sticker.newsession.test.tsx`）

```tsx
import { describe, it, expect, vi, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup } from "@testing-library/react";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invokeMock(...a) }));

import { EmptyState } from "./Sticker";

afterEach(() => cleanup());

describe("EmptyState 新建 CTA", () => {
  it("有 onNew 时渲染 CTA 且点击触发", () => {
    const onNew = vi.fn();
    render(<EmptyState tab="all" onNew={onNew} />);
    fireEvent.click(screen.getByTestId("empty-new-cta"));
    expect(onNew).toHaveBeenCalled();
  });

  it("无 onNew 时不渲染 CTA", () => {
    render(<EmptyState tab="all" />);
    expect(screen.queryByTestId("empty-new-cta")).toBeNull();
  });
});
```

Run: `cd app && bunx vitest run src/views/Sticker.newsession.test.tsx` → 应失败（`EmptyState` 无 `onNew`）。

- [ ] **Step 2: EmptyState 加 onNew CTA**（`Sticker.tsx`，`EmptyState` 组件）

```tsx
export function EmptyState({ tab, onNew }: { tab: Tab; onNew?: () => void }) {
  const t = useT();
  const { title, hint } = emptyCopy(tab, t);
  return (
    <div className="stk-empty">
      <span className="stk-empty-icon"><EmptyIcon tab={tab} /></span>
      <div className="stk-empty-title">{title}</div>
      {hint && <div className="stk-empty-hint">{hint}</div>}
      {onNew && (
        <button type="button" className="stk-empty-cta" data-testid="empty-new-cta" onClick={onNew}>
          {t.newSession.emptyCta}
        </button>
      )}
    </div>
  );
}
```

- [ ] **Step 3: PlusIcon + 底部栏按钮 + toast**（`Sticker.tsx`）

加 `PlusIcon`（图标区，如 `PencilIcon` 旁）：

```tsx
function PlusIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
      strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M12 5v14M5 12h14" />
    </svg>
  );
}
```

主 Sticker 组件（渲染 `.stk-bar` 的那个）内，`return` 前加 toast state + 监听（`listen`/`invoke` 已在文件顶部 import）：

```tsx
  const [toast, setToast] = useState<string | null>(null);
  useEffect(() => {
    const un = listen<string>("new-session-launched", (e) => {
      setToast(e.payload);
      window.setTimeout(() => setToast(null), 4000);
    });
    return () => {
      un.then((f) => f());
    };
  }, []);
```

底栏 `.stk-bar-actions` 内、搜索按钮之前，加「新建」按钮：

```tsx
              <span
                className="stk-act"
                data-tip={t.newSession.newButton}
                aria-label={t.newSession.newButton}
                data-testid="bar-new"
                onClick={() => invoke("open_new_session_window").catch(() => {})}
              >
                <PlusIcon />
              </span>
```

组件根 `<div>` 末尾（底栏 `.stk-bar` 之后）加 toast：

```tsx
      {toast && <div className="stk-toast" role="status">{toast}</div>}
```

- [ ] **Step 4: 空状态渲染点传 onNew**

Run: `cd app && grep -n "<EmptyState" src/views/Sticker.tsx`

把渲染处 `<EmptyState tab={...} />` 改为：

```tsx
<EmptyState tab={...} onNew={() => invoke("open_new_session_window").catch(() => {})} />
```

- [ ] **Step 5: 样式**（`styles.css`）

```css
.stk-empty-cta {
  margin-top: 10px;
  padding: 6px 14px;
  border-radius: var(--r-md, 8px);
  border: 1px solid var(--cc-accent, #c96442);
  background: transparent;
  color: inherit;
  cursor: pointer;
  font-size: 12px;
}
.stk-toast {
  position: fixed;
  left: 50%;
  bottom: 16px;
  transform: translateX(-50%);
  background: rgba(0, 0, 0, 0.82);
  color: #fff;
  padding: 8px 14px;
  border-radius: var(--r-md, 8px);
  font-size: 12px;
  z-index: 200;
  max-width: calc(100vw - 24px);
  text-align: center;
}
```

- [ ] **Step 6: 验证**

Run: `cd app && bunx vitest run src/views/Sticker.newsession.test.tsx && bunx vitest run && bunx tsc --noEmit`
Expected: EmptyState 两测试 PASS；全量前端测试不回归；tsc 无错误。

- [ ] **Step 7: Commit**

```bash
git add app/src/views/Sticker.tsx app/src/views/Sticker.newsession.test.tsx app/src/styles.css
git commit -m "feat(new-session): 底部栏新建按钮 + 空状态 CTA → 开独立窗口，监听事件弹 toast"
```
```

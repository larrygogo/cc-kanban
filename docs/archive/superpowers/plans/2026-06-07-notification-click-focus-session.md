# 点击通知跳转到对应会话终端 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 点击桌面通知直接聚焦对应会话的 Windows Terminal 标签页（等同点击贴纸卡片），通过把 Windows 通知从 `tauri-plugin-notification` 换成支持点击回调的 `tauri-winrt-notification` 直发实现。

**Architecture:** 抽出共享的 `focus_session_terminal` 供命令与通知回调复用；新增 `show_session_notification` helper，在主线程用 `tauri-winrt-notification` 构建 toast 并挂 `on_activated` 回调（回调里聚焦会话终端）；`spawn_liveness_watch` 改调该 helper；移除 `tauri-plugin-notification`。不动前端、不改 DB。

**Tech Stack:** Rust（tauri v2、tauri-winrt-notification 0.7、windows-only）。

> 说明：本特性以重构 + 系统级通知/窗口聚焦为主，无新增可单测纯逻辑（去重/指纹已有测试，不动）。每个任务以「编译干净 + clippy 无警告 + 现有测试不回归」为验收；点击跳转为 UI 行为且仅安装版生效，需安装版手测。

---

## 文件结构

- `app/src-tauri/src/lib.rs`（改）：抽出 `focus_session_terminal`；瘦身 `focus_session` 命令；新增 `show_session_notification`（windows 实现 + 非 windows 空实现）；`spawn_liveness_watch` 改调 helper、去掉 `NotificationExt`；`run()` 移除通知插件注册。
- `app/src-tauri/Cargo.toml`（改）：移除 `tauri-plugin-notification`，windows target 增 `tauri-winrt-notification`。
- `Cargo.toml`（根，改）：`[workspace.dependencies]` 移除 `tauri-plugin-notification`。
- `app/src-tauri/capabilities/default.json`（改）：移除 `notification:default`。
- `README.md`（改）：桌面通知特性补「点击跳转」。

---

## Task 1: 抽出 `focus_session_terminal` 共享聚焦逻辑

**Files:**
- Modify: `app/src-tauri/src/lib.rs`（`focus_session` 命令，约 599-633 行）

- [ ] **Step 1: 在 `focus_session` 命令上方新增自由函数**

在 `#[tauri::command] fn focus_session(...)` 定义的**正上方**插入：

```rust
/// 聚焦某会话的终端：优先按标题用 UIA 精确切到对应 WT 标签页，否则按进程组找窗口置前。
/// 放后台线程 fire-and-forget（保证干净 COM apartment + 不阻塞调用方）。仅 Windows 有实际行为。
/// 供 focus_session 命令与「点击通知」回调共用。
fn focus_session_terminal(pid: i64, title: Option<String>) {
    #[cfg(target_os = "windows")]
    std::thread::spawn(move || {
        // 首选：按标题用 UIA 精确切到对应 WT 标签页（解决单进程多标签/多窗口下按 PID 对应不上）。
        if let Some(t) = title.as_deref() {
            if focus_terminal_tab(pid as u32, t) {
                return;
            }
        }
        // 兜底：传统 conhost（每窗口独立进程）等场景，扫进程组按 PID 找顶层窗口置前。
        let targets = console_group_pids(pid as u32);
        if let Some(hwnd) = find_window_for_pids(&targets) {
            force_foreground(hwnd);
        }
    });
    #[cfg(not(target_os = "windows"))]
    let _ = (pid, title);
}
```

- [ ] **Step 2: 把 `focus_session` 命令改为薄封装**

把现有命令体（约 599-633 行）整体替换为：

```rust
#[tauri::command]
fn focus_session(pid: i64, title: Option<String>) -> Result<(), String> {
    if pid <= 0 {
        return Err("无效 pid".into());
    }
    #[cfg(target_os = "windows")]
    {
        focus_session_terminal(pid, title);
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (pid, title);
        Err("仅支持 Windows".into())
    }
}
```

- [ ] **Step 3: 编译 + 测试 + clippy**

Run: `cargo build -p meowo-app && cargo test -p meowo-app && cargo clippy -p meowo-app -- -D warnings`
Expected: 全部通过，无警告（纯重构，行为不变）。

- [ ] **Step 4: 提交**

```bash
git add app/src-tauri/src/lib.rs
git commit -m "refactor(app): 抽出 focus_session_terminal 供命令与后续复用"
```

---

## Task 2: 通知后端换成 winrt 直发 + 点击聚焦

**Files:**
- Modify: `Cargo.toml`（根）
- Modify: `app/src-tauri/Cargo.toml`
- Modify: `app/src-tauri/capabilities/default.json`
- Modify: `app/src-tauri/src/lib.rs`（新增 helper；`spawn_liveness_watch`；`run()`）

- [ ] **Step 1: 切换依赖**

根 `Cargo.toml` 的 `[workspace.dependencies]` 里**删除**这一行：

```toml
tauri-plugin-notification = "2"
```

`app/src-tauri/Cargo.toml` 的 `[dependencies]` 里**删除**：

```toml
tauri-plugin-notification = { workspace = true }
```

`app/src-tauri/Cargo.toml` 的 `[target.'cfg(target_os = "windows")'.dependencies]` 段里**新增**（与现有 `windows-sys`、`uiautomation` 同段）：

```toml
tauri-winrt-notification = "0.7"
```

- [ ] **Step 2: 移除通知插件注册**

`app/src-tauri/src/lib.rs` 的 `run()` 里**删除**这一行：

```rust
        .plugin(tauri_plugin_notification::init())
```

- [ ] **Step 3: 移除 capability 权限**

`app/src-tauri/capabilities/default.json` 的 `permissions` 数组里**删除** `"notification:default"` 这一项（注意删掉后保持 JSON 逗号合法——它原本在 `"process:default"` 之后、是最后一项，需把 `"process:default"` 行尾的逗号去掉）。

- [ ] **Step 4: 新增 `show_session_notification` helper**

在 `app/src-tauri/src/lib.rs` 的 `spawn_liveness_watch` 函数**正上方**插入（两个 cfg 变体）：

```rust
/// 弹一条「点击即聚焦该会话终端」的桌面通知。构建+show 放主线程（winrt toast 的 on_activated
/// 回调需要有消息泵的 COM apartment 才能可靠投递，Tauri 主线程最稳）；回调里调
/// focus_session_terminal（它自己 spawn 干净线程做 UIA，不阻塞主线程）。app 仅 Windows。
#[cfg(target_os = "windows")]
fn show_session_notification(
    app: &tauri::AppHandle,
    title: String,
    body: String,
    pid: i64,
    focus_title: String,
) {
    use tauri_winrt_notification::Toast;
    // 安装版用 bundle identifier（解析到开始菜单快捷方式 → 显示 Meowo+图标 + 点击可激活）；
    // dev 下 AUMID 未注册，退回 PowerShell 的 AUMID 仅保证 toast 能弹出（dev 点击不跳转）。
    let app_id = if tauri::is_dev() {
        Toast::POWERSHELL_APP_ID.to_string()
    } else {
        app.config().identifier.clone()
    };
    let _ = app.run_on_main_thread(move || {
        let _ = Toast::new(&app_id)
            .title(&title)
            .text1(&body)
            .on_activated(move |_| {
                focus_session_terminal(pid, Some(focus_title.clone()));
                Ok(())
            })
            .show();
    });
}

#[cfg(not(target_os = "windows"))]
fn show_session_notification(
    _app: &tauri::AppHandle,
    _title: String,
    _body: String,
    _pid: i64,
    _focus_title: String,
) {
}
```

- [ ] **Step 5: `spawn_liveness_watch` 改调 helper**

(a) 删除函数顶部的这一行：

```rust
    use tauri_plugin_notification::NotificationExt;
```

(b) 把会话循环里「错误通知 + 待交互通知」那段（当前约 961-1002 行，从 `let meowo_store::TranscriptInfo { title, error } =` 到待交互的 `match ... { None => { notified_waiting.remove(&sid); } }` 结束）整体替换为：

```rust
                    let meowo_store::TranscriptInfo { title, error } =
                        meowo_store::title::resolve_transcript_path(None, s.cwd.as_deref(), &sid)
                            .and_then(|p| p.to_str().map(meowo_store::analyze_transcript))
                            .unwrap_or_default();
                    // 会话标题：通知正文用，也作点击聚焦时匹配 WT 标签页的标题。transcript 标题优先，否则 DB 标题。
                    let display_title = title
                        .filter(|t| !t.trim().is_empty())
                        .unwrap_or_else(|| s.task_title.clone());
                    let pid = s.pid.unwrap_or(0); // 连接中必为有效 pid

                    // 错误通知（优先）。
                    if let Some(e) = &error {
                        let prev = notified.get(&sid).map(|s| s.as_str());
                        if seeded && notify_on && should_notify(prev, Some(&e.fingerprint)) {
                            show_session_notification(
                                &app,
                                "会话出错".into(),
                                format!("{} · {}", s.project_name, e.label),
                                pid,
                                display_title.clone(),
                            );
                        }
                        notified.insert(sid.clone(), e.fingerprint.clone());
                    } else {
                        notified.remove(&sid); // 错误消失：下次再错会重新通知
                    }

                    // 待交互通知（errored 时 waiting_fingerprint 返回 None，自动让位给错误）。
                    match waiting_fingerprint(error.is_some(), &s.session.status, s.session.last_event_at) {
                        Some(fp) => {
                            let prev = notified_waiting.get(&sid).map(|s| s.as_str());
                            if seeded && notify_on && should_notify(prev, Some(&fp)) {
                                show_session_notification(
                                    &app,
                                    "等待你回复".into(),
                                    format!("{} · {}", s.project_name, display_title),
                                    pid,
                                    display_title.clone(),
                                );
                            }
                            notified_waiting.insert(sid.clone(), fp);
                        }
                        None => {
                            notified_waiting.remove(&sid);
                        }
                    }
```

- [ ] **Step 6: 编译 + 测试 + clippy**

Run: `cargo build -p meowo-app && cargo test -p meowo-app && cargo clippy -p meowo-app -- -D warnings`
Expected: 通过，无警告。

> 若 `tauri-winrt-notification` 的方法名与本计划不符（如 `title`/`text1`/`on_activated`/`POWERSHELL_APP_ID`），以 0.7.x 实际 API 为准微调；若 `on_activated` 闭包因 `FnMut` 不能 move `focus_title` 而报错，保持 `Some(focus_title.clone())`（已是 clone）。如遇 winrt/COM 相关编译或运行问题，STOP 并报告具体错误，不要擅自改成别的机制。

- [ ] **Step 7: 提交**

```bash
git add Cargo.toml app/src-tauri/Cargo.toml app/src-tauri/capabilities/default.json app/src-tauri/src/lib.rs
git commit -m "feat(app): 通知改用 winrt 直发，点击通知聚焦对应会话终端"
```

---

## Task 3: 整体验证 + 文档

**Files:**
- Modify: `README.md`

- [ ] **Step 1: 全量验证**

Run（仓库根）:

```bash
cargo test --workspace && cargo clippy --workspace -- -D warnings
```

Expected: 通过，无警告。

Run（前端，确认无回归）:

```bash
cd app && bunx tsc --noEmit && bunx vitest run
```

Expected: 通过（前端未改，应原样通过）。

- [ ] **Step 2: 确认插件已彻底移除**

Run: `grep -rn "tauri-plugin-notification\|tauri_plugin_notification\|notification:default\|NotificationExt" Cargo.toml app/src-tauri`
Expected: 无任何输出（插件引用已清干净）。

- [ ] **Step 3: README 补「点击跳转」**

`README.md` 的「桌面通知」特性那条：

```markdown
- **桌面通知**：会话需要你回复（待交互）或出错时弹一条去重的系统通知（同一情形只弹一次）；可在设置里用总开关统一开关，默认开启。
```

改为：

```markdown
- **桌面通知**：会话需要你回复（待交互）或出错时弹一条去重的系统通知（同一情形只弹一次），点击通知直接切到该会话的终端；可在设置里用总开关统一开关，默认开启。
```

- [ ] **Step 4: 手动冒烟（需安装版，可选）**

Run: `cd app && bun run tauri build`，安装产物后验证：通知显示「Meowo」+ 图标；点击「等待你回复」/「会话出错」通知能切到该会话的 Windows Terminal 标签页并置前。

> dev 模式（`bun run tauri dev`）下通知仍归在 powershell 且点击不跳转，属 AUMID 未注册的固有限制，以安装版为准。

- [ ] **Step 5: 提交**

```bash
git add README.md
git commit -m "docs: README 补通知点击跳转说明"
```

---

## 自查记录

- **Spec 覆盖**：抽 `focus_session_terminal` → Task 1；winrt 直发 + `on_activated` 聚焦 + 主线程构建 + dev 用 POWERSHELL_APP_ID → Task 2 helper；`spawn_liveness_watch` 改调（保留去重/门控/播种）→ Task 2 Step 5；移除插件依赖/capability/注册 → Task 2 Step 1-3 + Task 3 Step 2 校验；README → Task 3。✅
- **占位符**：无 TBD/TODO；所有代码步骤含完整代码。✅
- **类型一致**：`focus_session_terminal(pid: i64, title: Option<String>)`、`show_session_notification(&AppHandle, String, String, i64, String)`、`display_title`/`pid`、`Toast::{new,title,text1,on_activated,show,POWERSHELL_APP_ID}` 全程一致；helper 与回调共用 `focus_session_terminal`。✅

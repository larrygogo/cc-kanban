# 点击通知跳转到对应会话终端 — 设计

> 日期：2026-06-07
> 状态：已通过设计评审，待写实现计划
> 前置：建立在已合并的「错误通知」「待交互通知 + 总开关」之上。

## 背景与问题

两个观察：

1. **通知显示成 powershell（图标/名字）** —— 经查 `tauri-plugin-notification` 源码（`desktop.rs:195-206`），它**只在非 `target/debug`、非 `target/release` 目录运行时**才给 toast 设 AUMID。`tauri dev` 跑在 `target/debug` → 不设 AUMID → Windows 归给启动进程（powershell）。**安装版（NSIS）会正常显示 Meowo + 图标。** 属 dev 固有限制，本身无需改代码。
2. **点击通知无反应** —— `tauri-plugin-notification` 桌面端 `show()` 是 fire-and-forget，**不暴露任何点击/激活回调**，无法实现点击跳转。

目标：**点击通知 = 跳到该会话的终端**（等同点击贴纸里的会话卡片：精确切到它的 Windows Terminal 标签页并置前）。

## 关键技术结论

- 底层 crate `tauri-winrt-notification`（0.7.2，已是传递依赖）**支持 `.on_activated(F)`**（`F: FnMut(Option<String>) -> Result<()> + Send + 'static`）。
- 因此放弃 `tauri-plugin-notification`，在 Windows 上**直接用 `tauri-winrt-notification`** 构建 toast 并挂 `on_activated`。app 仅支持 Windows，故无跨平台损失。
- 我们的通知只对**连接中**会话发，点击目标恒为"聚焦该会话终端"，复用现有 `focus_session` 命令的聚焦逻辑。

## 设计决策（已与用户确认）

1. 点击通知 → 聚焦对应会话的 Windows Terminal 标签页（与点击贴纸卡片一致），**不切贴纸 tab、不改前端**。
2. 移除 `tauri-plugin-notification`，全面改用 `tauri-winrt-notification` 直发。
3. `#1`（powershell 名字）当作 dev 限制，不单独处理（winrt 路径在安装版会设正确 AUMID，顺带正常显示）；dev 下用 `POWERSHELL_APP_ID` 兜底保证 toast 仍能弹出（便于测试），但 dev 下点击不跳转（AUMID 未注册）。

## 组件与改动点（全在 `app/src-tauri`）

### 1. 抽出共享聚焦函数 `focus_session_terminal`

把现有 `focus_session` 命令（`lib.rs:599-633`）的 Windows 实现体抽成自由函数，命令与通知回调共用，**零行为变化**：

```rust
/// 聚焦某会话的终端：优先按标题 UIA 精确切到对应 WT 标签页，否则按进程组找窗口置前。
/// 放后台线程 fire-and-forget（干净 COM apartment + 不阻塞调用方）。仅 Windows 有实际行为。
fn focus_session_terminal(pid: i64, title: Option<String>) {
    #[cfg(target_os = "windows")]
    std::thread::spawn(move || {
        if let Some(t) = title.as_deref() {
            if focus_terminal_tab(pid as u32, t) {
                return;
            }
        }
        let targets = console_group_pids(pid as u32);
        if let Some(hwnd) = find_window_for_pids(&targets) {
            force_foreground(hwnd);
        }
    });
    #[cfg(not(target_os = "windows"))]
    let _ = (pid, title);
}
```

`focus_session` 命令改为薄封装：

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

### 2. 新增通知 helper `show_session_notification`

构建 + show 放到**主线程**（`run_on_main_thread`）执行：winrt toast 的 `on_activated` 回调需要一个有消息泵的 COM apartment 才能可靠投递，Tauri 主线程（STA、持续泵消息）最稳；回调里再调 `focus_session_terminal`（它自己 spawn 干净线程做 UIA，不阻塞主线程）。

```rust
/// 弹一条「点击即聚焦该会话终端」的桌面通知。app 仅 Windows，故非 Windows 为 no-op。
#[cfg(target_os = "windows")]
fn show_session_notification(
    app: &tauri::AppHandle,
    title: String,
    body: String,
    pid: i64,
    focus_title: String,
) {
    use tauri_winrt_notification::Toast;
    // 安装版用 bundle identifier（解析到开始菜单快捷方式 → 显示 Meowo + 图标 + 点击可激活）；
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

### 3. `spawn_liveness_watch` 改用新 helper

- 删除 `use tauri_plugin_notification::NotificationExt;`。
- 循环里每个连接中会话：算出 `display_title`（`info.title` 非空优先，否则 `s.task_title`）与 `pid`（连接中必为有效）。
- 错误通知：`show_session_notification(&app, "会话出错".into(), format!("{} · {}", s.project_name, e.label), pid, display_title.clone())`。
- 待交互通知：body 用 `display_title`，`show_session_notification(&app, "等待你回复".into(), format!("{} · {}", s.project_name, display_title), pid, display_title.clone())`。
- **去重、总开关 `notify_on` 门控、首扫播种、retain 全部不变**——只把 `.show()` 的实现从插件换成 helper。

### 4. 依赖与配置

- `app/src-tauri/Cargo.toml`：
  - `[dependencies]` 移除 `tauri-plugin-notification = { workspace = true }`。
  - `[target.'cfg(target_os = "windows")'.dependencies]` 增 `tauri-winrt-notification = "0.7"`。
- 根 `Cargo.toml`：`[workspace.dependencies]` 移除 `tauri-plugin-notification = "2"`。
- `app/src-tauri/capabilities/default.json`：移除 `"notification:default"`。
- `run()`：移除 `.plugin(tauri_plugin_notification::init())`。

## 错误处理

- `run_on_main_thread` / `Toast::show()` 失败 → `let _ =` 吞掉（best-effort，沿用现状）。
- 点击时会话可能已断开 → `focus_terminal_tab` 找不到标签页，静默无操作（不做 resume）。
- 非 Windows 平台 → helper 为 no-op；`focus_session_terminal` 无实际行为（保证 workspace 在非 Windows 也能编译）。

## 测试计划

- `focus_session_terminal` 是对现有逻辑的纯抽取（命令复用它），靠编译 + 现有路径保证；通知点击为 UI 行为且仅安装版生效，无法单测。
- 验证：`cargo build`/`clippy` 干净、现有测试不回归；**安装版手测**：弹通知显示 Meowo + 图标；点击通知切到该会话 WT 标签页并置前；待交互/出错两类均可点。
- 无新增可单测纯逻辑（去重/指纹已有测试，不动）。

## 非目标（YAGNI）

- 不切贴纸 tab、不动前端。
- dev 下点击跳转不保证（AUMID 限制）；dev 仅保证 toast 能弹。
- 断开会话点击不做 resume。
- 不改 DB schema、不加 hook。

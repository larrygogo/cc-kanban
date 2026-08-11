# macOS 构建支持 & 状态栏面板 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 Meowo 增加 macOS（≥14 Sonoma）构建产物（签名公证的 .dmg + 自动更新），并用顶部状态栏（Menu Bar）面板取代 Windows 上的浮窗+吸边交互，保留终端跳转/恢复与桌面通知。

**Architecture:** 单一 Tauri v2 `main` 窗口在 macOS 上经 `tauri-nspanel` 原地转成 `NonactivatingPanel`，由托盘图标左键开/关、失焦自动隐藏、`tauri-plugin-positioner` 定位到图标下方；`ActivationPolicy::Accessory` + Info.plist `LSUIElement` 隐藏 Dock。终端跳转/恢复改用 `osascript` 控制 Terminal.app/iTerm2（按 tty 匹配），通知改用 `mac-notification-sys` 串行线程拿点击回调。所有平台相关副作用走 `#[cfg(target_os="macos")]`，可测纯逻辑抽成跨平台函数加单测。Windows 行为保持不变。

**Tech Stack:** Rust / Tauri v2（tray-icon + macos-private-api）、tauri-nspanel(git v2)、tauri-plugin-positioner、mac-notification-sys、React + TS、osascript/AppleScript、GitHub Actions + tauri-action（签名公证）。

**对应设计文档：** `docs/superpowers/specs/2026-06-09-macos-menubar-support-design.md`

---

## 贯穿原则（每个任务都遵守）

1. **不回归 Windows**：所有 macOS 逻辑走 `#[cfg(target_os = "macos")]` 或前端平台分流；Windows 路径不改行为。每个任务完成后在 Windows 上跑 `cargo clippy --workspace -- -D warnings` 与前端 `bunx tsc --noEmit` 必须仍绿。
2. **纯逻辑跨平台可测**：tty 规范化、终端类型判定、AppleScript 脚本文本等抽成**不带 cfg** 的纯函数放 `term_script.rs`，配单测，让现有 Windows CI 的 `cargo test` 真实覆盖（开发期无 Mac 的主要验证手段）。
3. **macOS 编译验证靠 CI**：Phase 1 即把 `macos-latest` 加入 CI 构建矩阵，后续每个 phase 推上去都有 macOS 编译/打包反馈。
4. **频繁提交**：每个 Task 末尾提交，message 用中文。
5. 当前分支：`feat/macos-menubar-support-20260609`（已创建）。

## 文件结构（新增/修改）

**新增 Rust（`app/src-tauri/src/`）**
- `term_script.rs` — 跨平台纯逻辑：`normalize_tty`、`TermKind`、`detect_term_kind`、AppleScript 脚本文本常量/构造、转义判定。**含 `#[cfg(test)]` 单测。**
- `macos/mod.rs` — macOS 模块入口，`#[cfg(target_os = "macos")]` 在 lib.rs 引入。
- `macos/panel.rs` — nspanel 转换、定位显示、失焦隐藏、`toggle_panel`。
- `macos/menubar.rs` — macOS 托盘（左键开/关面板，右键菜单 设置/退出）+ 激活策略切换。
- `macos/terminal.rs` — 副作用：`tty_for_pid`、`run_osascript`、`focus_session_terminal`、`resume_session_mac`。
- `macos/notify.rs` — `mac-notification-sys` 串行通知线程 + `OnceLock<Sender>`。

**修改 Rust**
- `app/src-tauri/Cargo.toml` — 加 macOS 依赖、tauri `macos-private-api` 特性。
- `app/src-tauri/src/lib.rs` — cfg 门控 Windows-only 调用点；setup 里按平台分流托盘/面板/通知；`focus_session`/`resume_session`/`show_session_notification` 加 macOS 实体；新增 `host_os` 命令；引入新模块。
- `app/src-tauri/tauri.conf.json` — `app.macOSPrivateApi: true`、`bundle.targets` 加 dmg/app、`bundle.macOS` 段。
- `app/src-tauri/Info.plist`（新增）— `LSUIElement`、`NSAppleEventsUsageDescription`。

**修改前端（`app/src/`）**
- `platform.ts`（新增）— `hostOs()` / `isMacPanel()`。
- `App.tsx` — macOS 下不挂吸边状态机/`snap-changed`/`CollapsedStrip`，直接渲染卡片列表。
- `views/Sticker.tsx` — 平台分流隐藏拖拽区、pin、resize 手柄。

**修改 CI/发布**
- `.github/workflows/ci.yml` — 构建矩阵加 `macos-latest`。
- `.github/workflows/release.yml` — 矩阵 + 签名公证 env + universal target。
- `README.md` — 移除「仅 Windows」、补 macOS 下载/权限说明。

---

# Phase 1 — 打通 macOS 编译 + 面板骨架

> 目标：macOS CI 能编译并打出 dmg；状态栏图标左键弹面板（复用卡片）、右键菜单设置/退出、失焦隐藏、Dock 不显示。

### Task 1.1：cfg 门控 Windows-only 调用点，让 macOS 可编译

**Files:**
- Modify: `app/src-tauri/src/lib.rs`（setup 回调里的 Windows-only 块，约 1395-1405；及任何无 cfg 引用 `pull_on_screen` / `win_constrain` 的位置）

- [ ] **Step 1：定位所有 Windows-only 调用点**

Run: `rg -n "pull_on_screen|win_constrain|\.hwnd\(\)" app/src-tauri/src/lib.rs`
预期：列出 setup 中调用 `pull_on_screen(...)` 与 `win_constrain::install(...)`、`window.hwnd()` 的行（函数定义本身已带 `#[cfg(target_os="windows")]`，问题在调用点未门控）。

- [ ] **Step 2：把 setup 里的调用点包进 cfg 块**

把 setup 中类似下面的调用整体包进 `#[cfg(target_os = "windows")]`（保留原逻辑不变）：

```rust
// setup() 内，原本无条件调用的 Windows-only 收尾
#[cfg(target_os = "windows")]
{
    pull_on_screen(&window, false);
    if let Ok(hwnd) = window.hwnd() {
        win_constrain::install(hwnd);
    }
}
```

同理，`on_window_event` 的 `Moved` 分支里若调用了 `pull_on_screen`，对该调用加 `#[cfg(target_os = "windows")]`（`edge_for_rect` 等纯几何函数跨平台保留）。

- [ ] **Step 3：Windows 编译仍绿**

Run: `cargo clippy -p meowo-app --target x86_64-pc-windows-msvc -- -D warnings`（或当前 Windows 默认 target：`cargo clippy --workspace -- -D warnings`）
预期：PASS，无新告警。

- [ ] **Step 4：静态确认 macOS 无未定义引用**

Run: `rg -n "pull_on_screen|win_constrain" app/src-tauri/src/lib.rs`
预期：所有调用点均处于 `#[cfg(target_os = "windows")]` 块内（函数定义也是）。macOS 编译验证留待 Task 1.7 的 CI。

- [ ] **Step 5：提交**

```bash
git add app/src-tauri/src/lib.rs
git commit -m "fix(macos): cfg 门控 Windows-only 调用点，解除 macOS 编译阻塞"
```

---

### Task 1.2：新增 macOS 依赖与 Tauri 特性

**Files:**
- Modify: `app/src-tauri/Cargo.toml`

- [ ] **Step 1：给 tauri 开 macos-private-api，加跨平台 positioner + macOS 依赖**

`[dependencies]` 中把 tauri 行改为带 `macos-private-api`（保留 tray-icon），并加 positioner（跨平台 crate，仅 macOS 调用）：

```toml
tauri = { workspace = true, features = ["tray-icon", "macos-private-api"] }
tauri-plugin-positioner = { version = "2", features = ["tray-icon"] }
```

文件末尾新增 macOS 专属依赖块：

```toml
[target.'cfg(target_os = "macos")'.dependencies]
tauri-nspanel = { git = "https://github.com/ahkohd/tauri-nspanel", branch = "v2" }
mac-notification-sys = "0.6"
```

- [ ] **Step 2：Windows 仍能解析依赖并编译**

Run: `cargo build -p meowo-app`（Windows 上；首次会拉 positioner）
预期：PASS。`tauri-nspanel`/`mac-notification-sys` 因 cfg target 不在 Windows 解析，不影响。

- [ ] **Step 3：提交**

```bash
git add app/src-tauri/Cargo.toml Cargo.lock
git commit -m "build(macos): 引入 nspanel/positioner/mac-notification-sys 与 macos-private-api"
```

---

### Task 1.3：tauri.conf.json 与 Info.plist 的 macOS 配置

**Files:**
- Modify: `app/src-tauri/tauri.conf.json`
- Create: `app/src-tauri/Info.plist`

- [ ] **Step 1：开启 macOSPrivateApi、扩展 bundle targets、加 macOS bundle 段**

`app` 对象内新增（与 `windows`/`security` 同级）：

```json
"macOSPrivateApi": true
```

`bundle` 对象内：`targets` 改为 `["nsis", "dmg", "app"]`；新增 `macOS` 段：

```json
"macOS": {
  "minimumSystemVersion": "14.0",
  "hardenedRuntime": true,
  "signingIdentity": null
}
```

（`createUpdaterArtifacts: true` 已存在，保留——它让 macOS 产 `.app.tar.gz(.sig)`。）

- [ ] **Step 2：新增 Info.plist（Tauri 构建时自动与生成值合并）**

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>LSUIElement</key>
  <true/>
  <key>NSAppleEventsUsageDescription</key>
  <string>Meowo 需要发送 Apple Event 来切换到你的 Claude Code 会话所在终端。</string>
</dict>
</plist>
```

- [ ] **Step 3：校验 JSON 合法**

Run: `bunx --bun tsc --version >NUL 2>&1; node -e "JSON.parse(require('fs').readFileSync('app/src-tauri/tauri.conf.json','utf8'))" && echo OK`
预期：输出 `OK`（JSON 无语法错误）。

- [ ] **Step 4：提交**

```bash
git add app/src-tauri/tauri.conf.json app/src-tauri/Info.plist
git commit -m "feat(macos): tauri.conf 开启 private api + dmg/app 打包 + Info.plist(LSUIElement)"
```

---

### Task 1.4：macOS 面板模块（nspanel 转换 + 定位显示 + 失焦隐藏）

**Files:**
- Create: `app/src-tauri/src/macos/mod.rs`
- Create: `app/src-tauri/src/macos/panel.rs`

- [ ] **Step 1：创建 macOS 模块入口**

`app/src-tauri/src/macos/mod.rs`：

```rust
//! macOS 专属：状态栏面板、托盘、终端跳转、通知。仅在 target_os = "macos" 编译。
pub mod menubar;
pub mod notify;
pub mod panel;
pub mod terminal;
```

- [ ] **Step 2：实现面板转换与显隐（照搬官方 menubar 示例 v2 分支 API）**

`app/src-tauri/src/macos/panel.rs`：

```rust
use tauri::{AppHandle, Emitter, Listener, Manager};
use tauri_nspanel::{
    cocoa::appkit::{NSMainMenuWindowLevel, NSWindowCollectionBehavior},
    panel_delegate, ManagerExt, WebviewWindowExt,
};
use tauri_plugin_positioner::{Position, WindowExt};

#[allow(non_upper_case_globals)]
const NS_NONACTIVATING_PANEL: i32 = 1 << 7; // NSWindowStyleMaskNonActivatingPanel

const RESIGN_EVENT: &str = "menubar_panel_did_resign_key";

/// 把已存在的 main 窗口原地转成 NonactivatingPanel，并接好失焦 -> emit 事件。
pub fn convert_main_to_panel(app: &AppHandle) {
    let window = match app.get_webview_window("main") {
        Some(w) => w,
        None => return,
    };
    let panel = match window.to_panel() {
        Ok(p) => p,
        Err(_) => return,
    };

    let delegate = panel_delegate!(CcPanelDelegate { window_did_resign_key });
    let handle = app.clone();
    delegate.set_listener(Box::new(move |name: String| {
        if name == "window_did_resign_key" {
            let _ = handle.emit(RESIGN_EVENT, ());
        }
    }));

    panel.set_level(NSMainMenuWindowLevel + 1);
    panel.set_style_mask(NS_NONACTIVATING_PANEL);
    panel.set_collection_behaviour(
        NSWindowCollectionBehavior::NSWindowCollectionBehaviorCanJoinAllSpaces
            | NSWindowCollectionBehavior::NSWindowCollectionBehaviorStationary
            | NSWindowCollectionBehavior::NSWindowCollectionBehaviorFullScreenAuxiliary,
    );
    panel.set_delegate(delegate);

    // 启动即隐藏，等托盘点击再显示。
    panel.order_out(None);
}

/// 失焦自动隐藏的监听器（在 setup 里调用一次）。
pub fn setup_resign_listener(app: &AppHandle) {
    let handle = app.clone();
    app.listen_any(RESIGN_EVENT, move |_| {
        if let Ok(panel) = handle.get_webview_panel("main") {
            panel.order_out(None);
        }
    });
}

/// 托盘点击：可见则收起，不可见则定位到图标下方再显示。
pub fn toggle_panel(app: &AppHandle) {
    let panel = match app.get_webview_panel("main") {
        Ok(p) => p,
        Err(_) => return,
    };
    if panel.is_visible() {
        panel.order_out(None);
        return;
    }
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.move_window(Position::TrayCenter); // 先定位
    }
    panel.show(); // 后显示
}
```

- [ ] **Step 3：（暂不可在 Windows 编译该模块，验证留 CI）确认无语法笔误**

Run: `rg -n "fn convert_main_to_panel|fn toggle_panel|fn setup_resign_listener" app/src-tauri/src/macos/panel.rs`
预期：三个函数都在。macOS 编译验证在 Task 1.7。

- [ ] **Step 4：提交**

```bash
git add app/src-tauri/src/macos/mod.rs app/src-tauri/src/macos/panel.rs
git commit -m "feat(macos): 新增状态栏面板模块(nspanel 转换/定位显示/失焦隐藏)"
```

---

### Task 1.5：macOS 托盘 + 激活策略（左键开面板，右键菜单）

**Files:**
- Create: `app/src-tauri/src/macos/menubar.rs`

- [ ] **Step 1：实现 macOS 托盘与设置窗口获焦策略**

`app/src-tauri/src/macos/menubar.rs`：

```rust
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

use crate::macos::panel;

/// 创建 macOS 状态栏托盘：左键切换面板，右键弹「设置 / 退出」菜单。
pub fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let settings = MenuItemBuilder::with_id("settings", "设置").build(app)?;
    let quit = MenuItemBuilder::with_id("quit", "退出").build(app)?;
    let menu = MenuBuilder::new(app).items(&[&settings, &quit]).build()?;

    TrayIconBuilder::with_id("meowo-tray")
        .icon(app.default_window_icon().unwrap().clone())
        .tooltip("Meowo")
        .menu(&menu)
        .show_menu_on_left_click(false) // 左键不弹菜单 => 留给右键
        .on_menu_event(|app, event| match event.id().as_ref() {
            "settings" => crate::open_settings_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            let app = tray.app_handle();
            // positioner 需要每次托盘事件记录图标坐标
            tauri_plugin_positioner::on_tray_event(app, &event);
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                panel::toggle_panel(app);
            }
        })
        .build(app)?;
    Ok(())
}

/// 打开设置窗口前临时切到 Regular 以便获焦；关闭时切回 Accessory（挂在窗口事件里）。
pub fn settings_window_will_open(app: &AppHandle) {
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
}

pub fn settings_window_did_close(app: &AppHandle) {
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
}
```

> 注：`crate::open_settings_window` 现为 `fn open_settings_window(app: &AppHandle)`（lib.rs:1148）。若它当前是私有，Task 1.6 会改为 `pub(crate)`。

- [ ] **Step 2：确认引用的现有符号存在**

Run: `rg -n "fn open_settings_window" app/src-tauri/src/lib.rs`
预期：找到定义；Task 1.6 把它设为 `pub(crate)`。

- [ ] **Step 3：提交**

```bash
git add app/src-tauri/src/macos/menubar.rs
git commit -m "feat(macos): 新增状态栏托盘(左键开面板/右键菜单)与激活策略切换"
```

---

### Task 1.6：在 lib.rs 接线 macOS 启动路径 + host_os 命令

**Files:**
- Modify: `app/src-tauri/src/lib.rs`

- [ ] **Step 1：引入模块、注册插件、新增 host_os 命令**

lib.rs 顶部模块声明处加：

```rust
mod term_script;
#[cfg(target_os = "macos")]
mod macos;
```

新增命令（放在其它 `#[tauri::command]` 旁）：

```rust
#[tauri::command]
fn host_os() -> String {
    #[cfg(target_os = "macos")]
    { "macos".into() }
    #[cfg(target_os = "windows")]
    { "windows".into() }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    { "other".into() }
}
```

把 `host_os` 加进 `tauri::generate_handler![...]` 列表。

在 `tauri::Builder` 链上注册 positioner（跨平台 init，仅 macOS 实际用到）：

```rust
.plugin(tauri_plugin_positioner::init())
```

- [ ] **Step 2：open_settings_window 设为 pub(crate)，并在 macOS 接入激活策略切换**

把 `fn open_settings_window` 改为 `pub(crate) fn open_settings_window`。在其创建/显示 about 窗口前后，macOS 切换策略：

```rust
pub(crate) fn open_settings_window(app: &tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    crate::macos::menubar::settings_window_will_open(app);

    if let Some(w) = app.get_webview_window("about") {
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    // ...原有 WebviewWindowBuilder 创建逻辑...
    // 在创建后给 about 窗口挂 CloseRequested 还原策略：
    // #[cfg(target_os = "macos")]
    // about.on_window_event(move |e| if matches!(e, tauri::WindowEvent::CloseRequested { .. }) {
    //     crate::macos::menubar::settings_window_did_close(&app_handle);
    // });
}
```

- [ ] **Step 3：setup 内按平台分流托盘/面板**

把现有 `setup_tray(...)`（Windows 托盘：左键开设置）调用改为平台分流：

```rust
// setup() 内
#[cfg(target_os = "macos")]
{
    app.handle().set_activation_policy(tauri::ActivationPolicy::Accessory)?;
    crate::macos::panel::convert_main_to_panel(app.handle());
    crate::macos::panel::setup_resign_listener(app.handle());
    crate::macos::menubar::setup_tray(app.handle())?;
}
#[cfg(target_os = "windows")]
{
    setup_tray(app)?; // 现有 Windows 托盘逻辑保持不变
}
```

> 说明：`main` 窗口在 `tauri.conf.json` 仍 `visible: true`；macOS 上 `convert_main_to_panel` 内 `order_out(None)` 立即隐藏，等左键再显示（可能有一帧闪烁，真机验收确认；如明显再改 per-platform visible:false）。

- [ ] **Step 4：Windows 编译与 clippy 仍绿**

Run: `cargo clippy --workspace -- -D warnings`
预期：PASS。`term_script` 模块（Task 1.8 创建，含 #[cfg(test)]）此时若尚未建，先建空壳或调整顺序——本计划中 Task 1.8 在 1.7 之前可先做；执行时若 `mod term_script;` 找不到文件，先完成 1.8。

- [ ] **Step 5：提交**

```bash
git add app/src-tauri/src/lib.rs
git commit -m "feat(macos): lib.rs 接线 macOS 启动(面板/托盘/激活策略) 与 host_os 命令"
```

---

### Task 1.7：CI 加 macOS 编译矩阵（关键验证机制）

**Files:**
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1：把 check job 改成 windows + macOS 矩阵**

```yaml
name: CI
on:
  push:
    branches: [main]
  pull_request:
jobs:
  check:
    strategy:
      fail-fast: false
      matrix:
        platform: [windows-latest, macos-latest]
    runs-on: ${{ matrix.platform }}
    steps:
      - uses: actions/checkout@v4
      - name: 安装 Bun
        uses: oven-sh/setup-bun@v2
      - name: 安装 Rust(含 clippy)
        uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy
      - name: Rust 构建缓存
        uses: Swatinem/rust-cache@v2
      - name: 安装前端依赖
        working-directory: app
        run: bun install --frozen-lockfile
      - name: 前端单测
        working-directory: app
        run: bunx vitest run
      - name: 构建前端(tsc + vite build)
        working-directory: app
        run: bun run build
      - name: Rust 测试
        run: cargo test --workspace
      - name: Rust clippy
        run: cargo clippy --workspace -- -D warnings
```

- [ ] **Step 2：推分支触发 CI，确认 macOS job 编译通过**

```bash
git add .github/workflows/ci.yml
git commit -m "ci(macos): CI 构建矩阵加入 macos-latest"
git push -u origin feat/macos-menubar-support-20260609
```

Run（观察）：`gh run watch` 或 `gh run list --branch feat/macos-menubar-support-20260609 -L 1`
预期：macOS 与 Windows 两个 job 均 PASS（这是 macOS 代码首次真实编译验证；nspanel/positioner/面板模块若有错会在此暴露）。

> 若 macOS job 因 nspanel API 报错，按研究结论核对 v2 分支 API（`set_collection_behaviour` 英式拼写、`to_panel()` 无泛型、需 `macos-private-api`），修正后重跑。

---

### Task 1.8：跨平台纯逻辑模块 term_script（tty/终端类型/脚本文本，含单测）

> 顺序提示：本 Task 与 `mod term_script;` 声明相关，建议在 Task 1.6 Step 4 编译前完成。

**Files:**
- Create: `app/src-tauri/src/term_script.rs`

- [ ] **Step 1：先写失败的单测**

`app/src-tauri/src/term_script.rs` 顶部先放测试（实现暂空）：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_tty_variants() {
        assert_eq!(normalize_tty("ttys003"), Some("/dev/ttys003".into()));
        assert_eq!(normalize_tty("s003"), Some("/dev/ttys003".into()));
        assert_eq!(normalize_tty("/dev/ttys012"), Some("/dev/ttys012".into()));
        assert_eq!(normalize_tty("  ttys004  "), Some("/dev/ttys004".into()));
        assert_eq!(normalize_tty("??"), None); // 无控制终端
        assert_eq!(normalize_tty(""), None);
    }

    #[test]
    fn detect_term_kind_picks_nearest_known_host() {
        // 进程树从 claude 向祖先：claude -> zsh -> login -> iTerm2 -> launchd
        let names = vec![
            "claude".to_string(),
            "zsh".to_string(),
            "login".to_string(),
            "iTerm2".to_string(),
            "launchd".to_string(),
        ];
        assert_eq!(detect_term_kind(&names), TermKind::ITerm2);

        let names2 = vec!["claude".into(), "zsh".into(), "Terminal".into()];
        assert_eq!(detect_term_kind(&names2), TermKind::Terminal);

        let names3 = vec!["claude".into(), "zsh".into(), "WezTerm".into()];
        assert_eq!(detect_term_kind(&names3), TermKind::Other);
    }

    #[test]
    fn focus_script_present_for_known_hosts_only() {
        assert!(focus_script(TermKind::Terminal).unwrap().contains("tty of t"));
        assert!(focus_script(TermKind::ITerm2).unwrap().contains("tty of s"));
        assert!(focus_script(TermKind::Other).is_none());
    }

    #[test]
    fn resume_script_uses_argv_and_quoted_form() {
        let s = resume_script(TermKind::Terminal);
        assert!(s.contains("on run argv"));
        assert!(s.contains("quoted form of"));
        assert!(s.contains("claude --resume"));
        // cwdless 变体：只 resume 不 cd
        let c = resume_script_cwdless(TermKind::Terminal);
        assert!(c.contains("on run argv"));
        assert!(c.contains("claude --resume"));
        assert!(!c.contains("cd "));
    }
}
```

- [ ] **Step 2：跑测试确认失败**

Run: `cargo test -p meowo-app term_script`
预期：FAIL（`normalize_tty` 等未定义 / 编译错误）。

- [ ] **Step 3：实现纯逻辑**

在测试模块上方实现：

```rust
//! 跨平台纯逻辑：供 macOS 终端跳转使用，但不依赖 macOS API，便于在任意平台单测。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TermKind {
    Terminal,
    ITerm2,
    Other,
}

/// 把 `ps -o tty=` 的输出规范化成 `/dev/ttysNNN`；无控制终端返回 None。
pub fn normalize_tty(raw: &str) -> Option<String> {
    let t = raw.trim();
    if t.is_empty() || t == "??" || t == "?" {
        return None;
    }
    if let Some(rest) = t.strip_prefix("/dev/") {
        return if rest.is_empty() { None } else { Some(format!("/dev/{rest}")) };
    }
    if let Some(num) = t.strip_prefix("ttys") {
        return Some(format!("/dev/ttys{num}"));
    }
    if let Some(num) = t.strip_prefix('s') {
        // ps 偶尔返回 's003'
        return Some(format!("/dev/ttys{num}"));
    }
    Some(format!("/dev/{t}"))
}

/// 进程名按「从 claude 自身向祖先」顺序传入，返回最近的已知终端宿主。
pub fn detect_term_kind(ancestor_names_root_first: &[String]) -> TermKind {
    for name in ancestor_names_root_first {
        let n = name.to_ascii_lowercase();
        if n.contains("iterm") {
            return TermKind::ITerm2;
        }
        if n == "terminal" || n.contains("terminal.app") {
            return TermKind::Terminal;
        }
    }
    TermKind::Other
}

/// 返回按 tty 定位并置前的 AppleScript（tty 通过 osascript argv 传入）。未知宿主返回 None。
pub fn focus_script(kind: TermKind) -> Option<&'static str> {
    match kind {
        TermKind::Terminal => Some(
            r#"on run argv
  set targetTTY to item 1 of argv
  tell application "Terminal"
    repeat with w in windows
      repeat with t in tabs of w
        if (tty of t) is targetTTY then
          set selected of t to true
          set frontmost of w to true
          activate
          return "FOUND"
        end if
      end repeat
    end repeat
  end tell
  return "NOT_FOUND"
end run"#,
        ),
        TermKind::ITerm2 => Some(
            r#"on run argv
  set targetTTY to item 1 of argv
  tell application "iTerm2"
    repeat with w in windows
      repeat with t in tabs of w
        repeat with s in sessions of t
          if (tty of s) is targetTTY then
            select w
            select t
            select s
            activate
            return "FOUND"
          end if
        end repeat
      end repeat
    end repeat
  end tell
  return "NOT_FOUND"
end run"#,
        ),
        TermKind::Other => None,
    }
}

/// 返回新开终端执行 `cd <cwd> && claude --resume <id>` 的 AppleScript（cwd/id 通过 argv 传入，用 quoted form 防注入）。
pub fn resume_script(kind: TermKind) -> &'static str {
    match kind {
        TermKind::ITerm2 => {
            r#"on run argv
  set targetDir to item 1 of argv
  set sessionId to item 2 of argv
  set theCmd to "cd " & quoted form of targetDir & " && claude --resume " & quoted form of sessionId
  tell application "iTerm2"
    activate
    set newWindow to (create window with default profile)
    tell current session of newWindow to write text theCmd
  end tell
end run"#
        }
        // Terminal 与 Other(回退到 Terminal) 共用
        _ => {
            r#"on run argv
  set targetDir to item 1 of argv
  set sessionId to item 2 of argv
  set theCmd to "cd " & quoted form of targetDir & " && claude --resume " & quoted form of sessionId
  tell application "Terminal"
    activate
    do script theCmd
  end tell
end run"#
        }
    }
}

/// 无 cwd 时的恢复脚本（仅 `claude --resume <id>`，不 cd），镜像 Windows 在 cwd 缺失时不带 -d 的行为。
pub fn resume_script_cwdless(kind: TermKind) -> &'static str {
    match kind {
        TermKind::ITerm2 => {
            r#"on run argv
  set sessionId to item 1 of argv
  set theCmd to "claude --resume " & quoted form of sessionId
  tell application "iTerm2"
    activate
    set newWindow to (create window with default profile)
    tell current session of newWindow to write text theCmd
  end tell
end run"#
        }
        _ => {
            r#"on run argv
  set sessionId to item 1 of argv
  set theCmd to "claude --resume " & quoted form of sessionId
  tell application "Terminal"
    activate
    do script theCmd
  end tell
end run"#
        }
    }
}
```

- [ ] **Step 4：跑测试确认通过**

Run: `cargo test -p meowo-app term_script`
预期：PASS（4 个测试全过）。

- [ ] **Step 5：clippy + 提交**

Run: `cargo clippy --workspace -- -D warnings`
预期：PASS。

```bash
git add app/src-tauri/src/term_script.rs
git commit -m "feat(macos): 新增 term_script 跨平台纯逻辑(tty 规范化/终端判定/脚本文本)+单测"
```

---

### Task 1.9：前端平台分流（macOS 渲染纯卡片列表，剥离窗口控件）

**Files:**
- Create: `app/src/platform.ts`
- Modify: `app/src/App.tsx`
- Modify: `app/src/views/Sticker.tsx`
- Test: `app/src/platform.test.ts`

- [ ] **Step 1：先写 platform 单测**

`app/src/platform.test.ts`：

```ts
import { describe, it, expect, vi, beforeEach } from "vitest";

describe("platform", () => {
  beforeEach(() => vi.resetModules());

  it("isMac true 当 host_os 返回 macos", async () => {
    vi.doMock("@tauri-apps/api/core", () => ({
      invoke: vi.fn().mockResolvedValue("macos"),
    }));
    const { detectHostOs, isMac } = await import("./platform");
    await detectHostOs();
    expect(isMac()).toBe(true);
  });

  it("isMac false 当 host_os 返回 windows", async () => {
    vi.doMock("@tauri-apps/api/core", () => ({
      invoke: vi.fn().mockResolvedValue("windows"),
    }));
    const { detectHostOs, isMac } = await import("./platform");
    await detectHostOs();
    expect(isMac()).toBe(false);
  });
});
```

- [ ] **Step 2：跑测试确认失败**

Run: `cd app && bunx vitest run platform`
预期：FAIL（`./platform` 不存在）。

- [ ] **Step 3：实现 platform.ts**

```ts
import { invoke } from "@tauri-apps/api/core";

let hostOs: "macos" | "windows" | "other" | null = null;

export async function detectHostOs(): Promise<void> {
  try {
    hostOs = (await invoke<string>("host_os")) as typeof hostOs;
  } catch {
    hostOs = "other";
  }
}

export function isMac(): boolean {
  return hostOs === "macos";
}

/** macOS 上以菜单栏面板形态运行（无独立浮窗/吸边）。 */
export function isMacPanel(): boolean {
  return hostOs === "macos";
}
```

- [ ] **Step 4：跑测试确认通过**

Run: `cd app && bunx vitest run platform`
预期：PASS。

- [ ] **Step 5：App.tsx 启动检测平台 + macOS 跳过吸边状态机**

在 `App.tsx` 的初始化处（mount effect 最前）`await detectHostOs()` 后再 fetch；并把吸边相关逻辑用 `isMacPanel()` 短路：

```tsx
import { detectHostOs, isMacPanel } from "./platform";

// mount effect 内最前：
useEffect(() => {
  let alive = true;
  (async () => {
    await detectHostOs();
    if (!alive) return;
    // ...原有 initial fetch / 订阅 board-changed...
  })();
  return () => { alive = false; };
}, []);
```

把 `snap-changed` 监听、`snap_collapse/expand/restore` 调用、`CollapsedStrip` 渲染、拖拽释放处理全部包一层：`if (isMacPanel()) return;` 或在 JSX 里 `{!isMacPanel() && <CollapsedStrip .../>}`。`mode` 在 macOS 固定为 `"normal"`，不读 `meowo-snap-edge`/`meowo-normal-size`。

- [ ] **Step 6：Sticker.tsx 平台分流隐藏拖拽区/pin/resize**

`Sticker.tsx` 中：
- 拖拽区 `<div className="drag" data-tauri-drag-region />`（约 276 行）→ `{!isMacPanel() && <div className="drag" data-tauri-drag-region />}`
- pin 按钮（约 292-298）→ `{!isMacPanel() && <button className="stk-pin" .../>}`
- resize 手柄（App.tsx:401-407 的 `.resize-grip`）→ `{!isMacPanel() && <div className="resize-grip" .../>}`

`import { isMacPanel } from "../platform";`

- [ ] **Step 7：前端测试 + tsc 全绿**

Run: `cd app && bunx tsc --noEmit && bunx vitest run`
预期：PASS（含既有 Sticker/App 测试不回归；若既有测试断言 pin/drag 存在，调整为在默认非 mac 下断言）。

- [ ] **Step 8：提交**

```bash
git add app/src/platform.ts app/src/platform.test.ts app/src/App.tsx app/src/views/Sticker.tsx
git commit -m "feat(macos): 前端平台分流(面板模式渲染纯卡片，剥离拖拽/pin/resize/吸边)"
```

---

# Phase 2 — 终端跳转 / 恢复（macOS）

> 目标：点连接中的卡片切到 Terminal.app/iTerm2 对应 tab；点已断开的卡片新开 Terminal 并 `claude --resume`。

### Task 2.1：macOS 终端副作用模块（tty 取得 + osascript 执行 + 进程树宿主判定）

**Files:**
- Create: `app/src-tauri/src/macos/terminal.rs`

- [ ] **Step 1：实现 tty 取得、osascript 执行、宿主判定、聚焦与恢复**

```rust
use std::process::Command;

use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, System};

use crate::term_script::{
    detect_term_kind, focus_script, normalize_tty, resume_script, resume_script_cwdless, TermKind,
};

/// 由 PID 取控制终端 tty，规范化为 /dev/ttysNNN。
fn tty_for_pid(pid: i64) -> Option<String> {
    let out = Command::new("ps")
        .args(["-o", "tty=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let raw = String::from_utf8_lossy(&out.stdout);
    normalize_tty(raw.trim())
}

/// 从 claude PID 向祖先收集进程名（root-first），用于判定终端宿主。
fn ancestor_names(pid: i64) -> Vec<String> {
    let sys = System::new_with_specifics(
        RefreshKind::new().with_processes(ProcessRefreshKind::everything()),
    );
    let mut names = Vec::new();
    let mut cur = Some(Pid::from(pid as usize));
    let mut guard = 0;
    while let Some(p) = cur {
        guard += 1;
        if guard > 32 {
            break;
        }
        match sys.process(p) {
            Some(proc_) => {
                names.push(proc_.name().to_string_lossy().to_string());
                cur = proc_.parent();
            }
            None => break,
        }
    }
    names
}

/// 用 stdin 传脚本、argv 传参数地运行 osascript（防注入）。返回 stdout trim。
fn run_osascript(script: &str, args: &[&str]) -> std::io::Result<String> {
    use std::io::Write;
    let mut child = Command::new("osascript")
        .arg("-") // 从 stdin 读脚本
        .args(args) // 作为 on run argv 的参数
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(script.as_bytes())?;
    }
    let out = child.wait_with_output()?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// 点连接中的卡片：切到该 claude 进程所在的终端 tab；未知宿主或未命中则回退新开 Terminal resume。
pub fn focus_session_terminal(pid: i64, cwd: Option<&str>, session_id: Option<&str>) {
    let kind = detect_term_kind(&ancestor_names(pid));
    if let (Some(tty), Some(script)) = (tty_for_pid(pid), focus_script(kind)) {
        if let Ok(res) = run_osascript(script, &[&tty]) {
            if res == "FOUND" {
                return;
            }
        }
    }
    // 回退（含未知宿主终端）：若有 session_id 就新开 Terminal resume，否则放弃。
    if let Some(id) = session_id {
        resume_session_mac(cwd, id);
    }
}

/// 点已断开的卡片（或跳转回退）：默认在 Terminal.app 新开窗口 claude --resume；有 cwd 则先 cd。
pub fn resume_session_mac(cwd: Option<&str>, session_id: &str) {
    match cwd {
        Some(dir) if !dir.trim().is_empty() => {
            let _ = run_osascript(resume_script(TermKind::Terminal), &[dir, session_id]);
        }
        _ => {
            let _ = run_osascript(resume_script_cwdless(TermKind::Terminal), &[session_id]);
        }
    }
}
```

- [ ] **Step 2：提交**

```bash
git add app/src-tauri/src/macos/terminal.rs
git commit -m "feat(macos): 终端跳转/恢复副作用模块(ps 取 tty + osascript + 进程树宿主判定)"
```

---

### Task 2.2：把 focus_session / resume_session 命令接到 macOS 实体

> 既有签名（已核对）：`fn focus_session(pid: i64, title: Option<String>) -> Result<(), String>`（lib.rs:631）；`fn resume_session(cwd: Option<String>, session_id: String) -> Result<(), String>`（lib.rs:670）。前端 `Sticker.tsx:331/335` 分别调用，且卡片 `l` 同时持有 `l.cwd` 与 `l.session.cc_session_id`。

**Files:**
- Modify: `app/src-tauri/src/lib.rs`（`focus_session` 631-645、`resume_session` 670-703）
- Modify: `app/src/views/Sticker.tsx`（focus_session 调用，331 行）

- [ ] **Step 1：focus_session 增量加 cwd/session_id 参数（Windows 忽略）并加 macOS 分支**

把 focus_session 改为（Windows 逻辑只用 pid/title 不变）：

```rust
#[tauri::command]
fn focus_session(
    pid: i64,
    title: Option<String>,
    cwd: Option<String>,
    session_id: Option<String>,
) -> Result<(), String> {
    if pid <= 0 {
        return Err("无效 pid".into());
    }
    #[cfg(target_os = "windows")]
    {
        let _ = (cwd, session_id); // Windows 按标题/进程组定位，不需要这两个
        focus_session_terminal(pid, title);
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        let _ = title; // macOS 按 pid->tty 定位
        crate::macos::terminal::focus_session_terminal(pid, cwd.as_deref(), session_id.as_deref());
        Ok(())
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = (pid, title, cwd, session_id);
        Err("当前平台不支持".into())
    }
}
```

- [ ] **Step 2：前端 focus_session 调用补传 cwd/sessionId**

`app/src/views/Sticker.tsx:331`：

```tsx
if (l.pid)
  invoke("focus_session", {
    pid: l.pid,
    title: l.task_title,
    cwd: l.cwd,
    sessionId: l.session.cc_session_id,
  }).catch(() => {});
```

- [ ] **Step 3：resume_session 加 macOS 分支（复用 resolve_cwd）**

在 `resume_session` 的 `#[cfg(target_os = "windows")]` 块后、`#[cfg(not(target_os = "windows"))]` stub 处改为：

```rust
#[cfg(target_os = "macos")]
{
    // 与 Windows 一致：DB 的 cwd 可能为空，用 resolve_cwd 从 transcript 兜底解析。
    let resolved = meowo_store::title::resolve_cwd(cwd.as_deref(), &session_id);
    crate::macos::terminal::resume_session_mac(resolved.as_deref(), &session_id);
    return Ok(());
}
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
{
    let _ = cwd;
    return Err("当前平台不支持".into());
}
```

`is_session_id` UUID 校验保持在函数最前（两平台共用，防注入）。

- [ ] **Step 4：Windows clippy + 前端 tsc 仍绿**

Run: `cargo clippy --workspace -- -D warnings`
Run: `cd app && bunx tsc --noEmit`
预期：均 PASS（Windows focus_session 多出的两个参数通过 `let _` 消费，无 unused 告警）。

- [ ] **Step 5：推 CI 验证 macOS 编译**

```bash
git add app/src-tauri/src/lib.rs app/src/views/Sticker.tsx
git commit -m "feat(macos): focus_session/resume_session 接入 macOS osascript 实体"
git push
```

Run: `gh run list --branch feat/macos-menubar-support-20260609 -L 1`
预期：macOS + Windows CI 均 PASS。

---

# Phase 3 — 桌面通知（macOS）

> 目标：会话待交互/出错时弹系统通知（复用现有去重与总开关），点击通知切到对应终端。

### Task 3.1：macOS 通知线程（mac-notification-sys 串行线程 + 点击回调）

**Files:**
- Create: `app/src-tauri/src/macos/notify.rs`

- [ ] **Step 1：实现串行通知线程与投递接口**

```rust
use std::sync::mpsc::{self, Sender};
use std::sync::OnceLock;

use mac_notification_sys::{get_bundle_identifier_or_default, send_notification, set_application,
    NotificationResponse};
use tauri::AppHandle;

/// 一条待弹通知；点击后用 pid->tty 切到对应终端（通知场景无需 resume，故不带 cwd/id）。
pub struct NotifyJob {
    pub title: String,
    pub body: String,
    pub pid: i64,
}

static TX: OnceLock<Sender<NotifyJob>> = OnceLock::new();

/// 启动一次：设应用归属 + 起串行通知线程。5s 轮询线程只投递、绝不阻塞。
pub fn init(_app: &AppHandle) {
    let bundle = get_bundle_identifier_or_default("Meowo");
    let _ = set_application(&bundle);

    let (tx, rx) = mpsc::channel::<NotifyJob>();
    std::thread::spawn(move || {
        for job in rx {
            if let Ok(NotificationResponse::Click) =
                send_notification(&job.title, None, &job.body, None)
            {
                // 点通知正文 -> 按 pid->tty 切到该会话所在终端。
                crate::macos::terminal::focus_session_terminal(job.pid, None, None);
            }
        }
    });
    let _ = TX.set(tx);
}

/// 投递一条通知任务（非阻塞）。在 OnceLock 未初始化时静默丢弃。
pub fn post(job: NotifyJob) {
    if let Some(tx) = TX.get() {
        let _ = tx.send(job);
    }
}
```

- [ ] **Step 2：提交**

```bash
git add app/src-tauri/src/macos/notify.rs
git commit -m "feat(macos): 通知串行线程(mac-notification-sys)+点击回调聚焦终端"
```

---

### Task 3.2：show_session_notification 的 macOS 实体 + 启动初始化

**Files:**
- Modify: `app/src-tauri/src/lib.rs`（`show_session_notification` 非 Windows stub 约 995-1003；setup）

- [ ] **Step 1：setup 里初始化通知线程（macOS）**

在 Task 1.6 的 `#[cfg(target_os = "macos")]` setup 块末尾追加：

```rust
crate::macos::notify::init(app.handle());
```

- [ ] **Step 2：填充 macOS 的 show_session_notification（按既有签名，不改调用处）**

既有签名（已核对，lib.rs:967/996）：`fn show_session_notification(app: &tauri::AppHandle, title: String, body: String, pid: i64, focus_title: String)`。当前非 Windows 是空 stub。把它拆成 macOS 实体 + 其它平台 no-op：

```rust
#[cfg(target_os = "macos")]
fn show_session_notification(
    _app: &tauri::AppHandle,
    title: String,
    body: String,
    pid: i64,
    _focus_title: String, // macOS 按 pid->tty 定位，标题用不上
) {
    crate::macos::notify::post(crate::macos::notify::NotifyJob { title, body, pid });
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn show_session_notification(
    _app: &tauri::AppHandle,
    _title: String,
    _body: String,
    _pid: i64,
    _focus_title: String,
) {
}
```

> `spawn_liveness_watch`（lib.rs:1009-1103）已在所有平台用 `(app, title, body, pid, focus_title)` 调用 `show_session_notification`，去重指纹（`should_notify`/`waiting_fingerprint`）与 `notifications_enabled` 门控全平台共用——**调用处与签名都不改**，只把 macOS 实体填上。

- [ ] **Step 3：Windows clippy + 测试仍绿**

Run: `cargo clippy --workspace -- -D warnings && cargo test --workspace`
预期：PASS。

- [ ] **Step 4：推 CI 验证 macOS 编译**

```bash
git add app/src-tauri/src/lib.rs
git commit -m "feat(macos): show_session_notification 接入 macOS 通知投递 + 启动初始化"
git push
```

Run: `gh run list --branch feat/macos-menubar-support-20260609 -L 1`
预期：两平台 CI PASS。

---

# Phase 4 — 发布管线（签名 / 公证 / 自动更新）+ 文档

> 目标：tag 触发后在 macОS runner 上构建签名公证的 universal dmg，并产出自动更新产物；README 更新。

### Task 4.1：release.yml 改矩阵 + 签名公证 + universal

**Files:**
- Modify: `.github/workflows/release.yml`

- [ ] **Step 1：改成 windows + macOS 矩阵，注入签名公证 env**

```yaml
name: Release
on:
  push:
    tags: ['v*']
permissions:
  contents: write
jobs:
  release:
    strategy:
      fail-fast: false
      matrix:
        include:
          - platform: 'macos-latest'
            args: '--target universal-apple-darwin'
          - platform: 'windows-latest'
            args: ''
    runs-on: ${{ matrix.platform }}
    steps:
      - uses: actions/checkout@v4
      - name: 安装 Bun
        uses: oven-sh/setup-bun@v2
      - name: 安装 Rust
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.platform == 'macos-latest' && 'aarch64-apple-darwin,x86_64-apple-darwin' || '' }}
      - name: Rust 构建缓存
        uses: Swatinem/rust-cache@v2
      - name: 安装前端依赖
        working-directory: app
        run: bun install --frozen-lockfile
      - name: 构建并发布
        uses: tauri-apps/tauri-action@v0
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
          TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}
          TAURI_SIGNING_PRIVATE_KEY_PASSWORD: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}
          # macOS 代码签名
          APPLE_CERTIFICATE: ${{ secrets.APPLE_CERTIFICATE }}
          APPLE_CERTIFICATE_PASSWORD: ${{ secrets.APPLE_CERTIFICATE_PASSWORD }}
          APPLE_SIGNING_IDENTITY: ${{ secrets.APPLE_SIGNING_IDENTITY }}
          KEYCHAIN_PASSWORD: ${{ secrets.KEYCHAIN_PASSWORD }}
          # 公证（App Store Connect API key，推荐）
          APPLE_API_ISSUER: ${{ secrets.APPLE_API_ISSUER }}
          APPLE_API_KEY: ${{ secrets.APPLE_API_KEY }}
          APPLE_API_KEY_PATH: ${{ secrets.APPLE_API_KEY_PATH }}
        with:
          projectPath: app
          tagName: ${{ github.ref_name }}
          releaseName: Meowo ${{ github.ref_name }}
          releaseDraft: true
          prerelease: false
          args: ${{ matrix.args }}
```

- [ ] **Step 2：YAML 合法性检查**

Run: `node -e "require('js-yaml')" 2>NUL || npx -y js-yaml .github/workflows/release.yml >NUL && echo OK`（或用 `gh workflow view` 在推送后确认被识别）
预期：被 GitHub 识别为合法 workflow（推 tag 后在 Actions 出现）。

- [ ] **Step 3：提交**

```bash
git add .github/workflows/release.yml
git commit -m "ci(macos): release 矩阵化 + 签名公证 + universal dmg + 更新产物"
```

---

### Task 4.2：列出所需 GitHub Secrets（交用户配置）

**Files:**
- Create: `docs/macos-release-secrets.md`

- [ ] **Step 1：写 secret 清单文档**

```markdown
# macOS 发布所需 GitHub Secrets

在仓库 Settings → Secrets and variables → Actions 添加：

## 代码签名（Developer ID Application 证书）
- `APPLE_CERTIFICATE`：Developer ID Application 证书导出的 .p12，base64 编码（`base64 -i cert.p12`）
- `APPLE_CERTIFICATE_PASSWORD`：导出 .p12 时设置的密码
- `APPLE_SIGNING_IDENTITY`：如 `Developer ID Application: Your Name (TEAMID)`（`security find-identity -v -p codesigning`）
- `KEYCHAIN_PASSWORD`：任意强随机串（CI 临时钥匙串用）

## 公证（App Store Connect API key，二选一推荐此项）
- `APPLE_API_ISSUER`：Issuer ID
- `APPLE_API_KEY`：Key ID
- `APPLE_API_KEY_PATH`：.p8 私钥在 runner 上的路径（需在 workflow 里先写出文件，或改存内容并落盘）

> 备选公证方式（Apple ID）：`APPLE_ID` / `APPLE_PASSWORD`(App 专用密码) / `APPLE_TEAM_ID`。

## 自动更新（已存在，复用）
- `TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`

## 注意
- 证书必须是 **Developer ID Application**（站外分发），不是 Apple Development / Mac App Distribution。
- `hardenedRuntime` 已在 tauri.conf 开启（公证必需）。
- `APPLE_API_KEY_PATH` 若用 secret 存 .p8 内容，需在 workflow 加一步把内容写到文件并把该路径传给 env。
```

- [ ] **Step 2：提交**

```bash
git add docs/macos-release-secrets.md
git commit -m "docs(macos): 发布所需 GitHub Secrets 清单"
```

---

### Task 4.3：README 更新（去掉「仅 Windows」+ macOS 下载/权限说明）

**Files:**
- Modify: `README.md`

- [ ] **Step 1：更新平台措辞与下载段**

- 删除/改写 `README.md:10` 的「目前面向 Windows（macOS/Linux 打包暂未做）」。
- 下载段补 macOS：`.dmg`（universal，≥ macOS 14 Sonoma），双击安装；已签名公证，直接打开。
- 平台差异说明：macOS 为状态栏菜单栏 App（无浮窗/吸边/pin），左键图标开面板、右键设置/退出。
- 权限说明：首次点击「跳转/恢复终端」会弹 macOS「自动化」授权（系统设置 → 隐私与安全性 → 自动化），需允许 Meowo 控制 Terminal/iTerm2；首次通知会请求通知权限。
- 路线图把 `- [ ] macOS / Linux 打包` 的 macOS 勾上。

- [ ] **Step 2：提交**

```bash
git add README.md
git commit -m "docs(macos): README 增加 macOS 下载/交互差异/权限说明"
```

---

### Task 4.4：真机验收清单（交用户在 Mac 上回归）

**Files:**
- Create: `docs/macos-acceptance-checklist.md`

- [ ] **Step 1：写验收清单**

```markdown
# macOS 真机验收清单

## 安装与启动
- [ ] dmg 双击安装，拖入 Applications；首次打开无 Gatekeeper 拦截（签名公证生效）
- [ ] Dock 不出现 Meowo 图标（LSUIElement）
- [ ] 顶部状态栏出现 Meowo 图标

## 面板交互
- [ ] 左键图标弹出面板，定位在图标正下方
- [ ] 面板内容与 Windows 贴纸卡片一致（项目名/状态/标题/todo 进度/连接状态/tab）
- [ ] 面板无拖拽区/pin/resize 手柄
- [ ] 点面板外部（失焦）面板自动收起；再次左键重新弹出
- [ ] 右键图标弹出「设置 / 退出」菜单；点设置打开设置窗口并获焦；关闭后 Dock 不残留图标
- [ ] 跨 Space / 全屏下面板仍可正常弹出

## 终端跳转 / 恢复
- [ ] 连接中会话（运行在 Terminal.app）点卡片 → 切到对应 tab 并置前
- [ ] 连接中会话（运行在 iTerm2）点卡片 → 切到对应 session/tab 并置前
- [ ] 已断开会话点卡片 → 新开 Terminal.app，cd 到原目录并 claude --resume
- [ ] 运行在非 Terminal/iTerm2（如 Warp）的会话点卡片 → 回退新开 Terminal resume
- [ ] 首次跳转弹「自动化」授权，允许后生效；路径含空格/特殊字符不出错（防注入）

## 通知
- [ ] 会话待交互/出错弹系统通知；同一情形不重复弹（去重）
- [ ] 总开关关闭后不再弹
- [ ] 点击通知切到对应终端
- [ ] 弹通知时不卡顿、CPU 正常（无 100% 飙升）

## 自动更新
- [ ] 发布新版本后，应用内/后台检查到更新并能升级（latest.json 含 darwin-universal）
```

- [ ] **Step 2：提交**

```bash
git add docs/macos-acceptance-checklist.md
git commit -m "docs(macos): 真机验收清单"
```

---

### Task 4.5：打 tag 验证发布管线（用户执行/确认）

- [ ] **Step 1：合并分支到 main（PR 评审后）**
- [ ] **Step 2：按现有发版流程打 tag（版本号取自 Cargo.toml，如 v0.1.5）**
- [ ] **Step 3：确认 Actions 上 macOS job 完成签名+公证，Release 草稿含 `*_universal.dmg`、`*.app.tar.gz(.sig)`，`latest.json` 含 `darwin-universal`**
- [ ] **Step 4：用户在 Mac 上跑「真机验收清单」**

---

## 自检（写计划后对照 spec）

- **Spec §4 面板架构** → Task 1.2/1.4（nspanel v2 + macos-private-api）✓
- **Spec §5 菜单栏交互** → Task 1.5/1.6（左键面板、右键菜单、设置窗口获焦）✓
- **Spec §6 前端复用分流** → Task 1.9 ✓
- **Spec §7 终端跳转/恢复** → Task 1.8（纯逻辑+测试）/2.1/2.2 ✓
- **Spec §8 通知** → Task 3.1/3.2（mac-notification-sys 串行线程，复用去重）✓
- **Spec §9 构建/CI/发布** → Task 1.2/1.3/1.7/4.1/4.2 ✓
- **Spec §10 验证策略** → 纯逻辑单测(1.8/1.9) + macOS CI(1.7) + 验收清单(4.4) ✓
- **Spec §1 隐藏 Dock(LSUIElement)** → Task 1.3(Info.plist) + 1.6(Accessory) ✓
- **类型/命名一致性**：`term_script::{normalize_tty, TermKind, detect_term_kind, focus_script, resume_script}`、`macos::panel::{convert_main_to_panel, setup_resign_listener, toggle_panel}`、`macos::terminal::{focus_session_terminal, resume_session_mac}`、`macos::notify::{init, post, NotifyJob}`、前端 `platform::{detectHostOs, isMac, isMacPanel}` 在各 Task 中引用一致 ✓
- **占位符扫描**：无 TBD/TODO；标注「以实际签名为准」处均给出明确核对动作与现有锚点（focus_session/resume_session/show_session_notification 的现签名需在执行时核对，因这些是既有函数）。

## 已知执行期需真机/CI 确认的点（非占位，属外部依赖）
1. nspanel `panel.show()` 后能否可靠收到 `window_did_resign_key`（官方示例同款模式，真机确认；若不触发，补「托盘外全局点击监听」或仅靠左键 toggle）。
2. macOS 托盘图标用彩色 default icon（`icon_as_template(false)`，默认）；若要随明暗菜单栏反色，后续补单色 template 图标再开 `icon_as_template(true)`。
3. 公证用 App Store Connect API key 时 `APPLE_API_KEY_PATH` 的落盘步骤（Task 4.1 可能需补一步把 .p8 内容写成文件再把路径传给 env）。
4. macOS 上 `main` 窗口 `convert_main_to_panel` 内 `order_out` 前是否有可见闪烁（config 仍 `visible:true`）；若明显，改 per-platform `visible:false` 并在 Windows setup 显式 `show()`。

> 三个既有函数签名已核对并写实：`focus_session(pid,title)`→增量加 `cwd,session_id`；`resume_session(cwd,session_id)`；`show_session_notification(app,title,body,pid,focus_title)` 签名不变只填 macOS 实体。

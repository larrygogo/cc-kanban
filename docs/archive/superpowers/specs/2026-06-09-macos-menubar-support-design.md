# macOS 构建支持 & 状态栏面板 — 设计文档

- 日期：2026-06-09
- 分支：`feat/macos-menubar-support-20260609`
- 状态：设计定稿，待落实施计划

## 1. 背景与目标

Meowo 目前仅面向 Windows：一个半透明、可吸边的浮动「贴纸」窗口实时展示所有 Claude Code 会话进度。本需求在新分支上为 **macOS（最低 14 Sonoma）** 添加构建产物（`.dmg`）与平台适配，并**用顶部状态栏（Menu Bar）面板取代** Windows 上的浮窗 + 吸边交互，作为 macOS 上唯一主入口。

### 平台行为差异（相对 Windows）

| 功能 | Windows | macOS |
|------|---------|-------|
| 吸边缩略 | ✅ | ❌ 移除 |
| 主悬浮贴纸窗口 | ✅ | ❌ 移除（由状态栏面板取代） |
| 状态栏图标 + 点击展开面板 | ❌ | ✅ 唯一主入口 |
| 点击跳转 / 恢复终端 | ✅ | ✅ 保留（Terminal.app + iTerm2） |
| 桌面通知 | ✅ | ✅ 保留 |
| 窗口置顶 pin | ✅ | ❌ 不适用 |

### 已确认的关键决策（2026-06-09 与用户确认）

1. **构建/测试**：用户有 Mac 但需另启。**开发先行，用户后续真机测试**。开发期验证靠：`cargo test` / `clippy` / 前端 `tsc`+`vitest` 全绿 + macOS CI job 编译打包成功；真机交互验收由用户在 Mac 完成。
2. **签名分发**：**完整签名 + 公证**（Developer ID + notarization）。
3. **自动更新**：**启用**（产 `.app.tar.gz` + minisign 签名，`latest.json` 增 macOS 条目）。
4. **设置/退出入口**：**右键菜单栏图标弹原生菜单**（左键开面板）。

## 2. 现状摸底（实现锚点）

> 以下 file:line 为现状参考，实现期以实际代码为准。

**可直接复用（平台无关）**
- 会话卡片 UI 内容结构：`app/src/views/Sticker.tsx`（卡片正文、状态指示、todo 进度）
- 数据流：`app/src/api.ts` `getLiveSessions()` → `app/src-tauri/src/lib.rs:293-364` `get_live_sessions`；`board-changed` 事件刷新
- tab 过滤、空态、外观主题（明暗/不透明度/密度）：`app/src/appearance.ts`
- 通知去重与门控：`lib.rs:945-960` `should_notify` / `waiting_fingerprint`，`settings.notifications_enabled`
- 活性轮询：`lib.rs:1009-1103` `spawn_liveness_watch`（全平台跑）
- 会话路径解析：`crates/meowo-store/src/title.rs`（`resolve_cwd` / transcript 定位，已跨平台）
- 进程树遍历：`lib.rs:366-411` `console_group_pids`（基于 `sysinfo`，跨平台）

**Windows 强耦合（macOS 需移除或替换）**
- 浮窗 + 吸边：`App.tsx` 状态机（normal/collapsed/expanded）、`CollapsedStrip.tsx`、`lib.rs:148-235` `snap_collapse/expand/restore`、`lib.rs:1200-1305` `win_constrain`（Win32 子类化）、`lib.rs:90-133` `pull_on_screen`
- 终端跳转：`lib.rs:488-565` `focus_terminal_tab`（UIAutomation 定位 WT 标签页）、`lib.rs:568-608` `force_foreground`（AttachThreadInput）、`lib.rs:413-443` `find_window_for_pids`
- 恢复会话：`lib.rs:655-703` `resume_session`（`wt.exe`）
- 通知：`lib.rs:967-1003` `show_session_notification`（WinRT Toast + on_activated）
- 窗口控件：`Sticker.tsx:243` pin（`setAlwaysOnTop`）、`App.tsx:405` resize 手柄、`App.tsx:276` 拖拽区
- 打包：`tauri.conf.json:38` `targets:["nsis"]`
- 依赖：`app/src-tauri/Cargo.toml:28-31`（windows-sys / uiautomation / tauri-winrt-notification，已 cfg 隔离）

**关键编译风险**：部分 Windows-only 函数（`pull_on_screen`、`win_constrain::install`）的**调用点未做 cfg 门控**（`lib.rs` setup 回调约 1398-1405），macOS 编译会报未定义函数。打通编译是第一步。

**依赖现状**：Tauri v2（`tray-icon` feature）；插件 window-state/autostart/updater/process；`sysinfo 0.32`、`ureq`（跨平台）。**updater 的 minisign 私钥 `TAURI_SIGNING_PRIVATE_KEY` 已在 `release.yml` 配置**，macOS 自动更新直接复用。

## 3. 总原则

- **不回归 Windows**：所有 macOS 逻辑走 `#[cfg(target_os = "macos")]` / 前端平台分流；Windows 路径行为保持不变。
- **复用优先**：卡片 UI、数据流、去重逻辑、路径解析尽量原样复用，仅在窗口/交互层做平台分流。
- **可测优先**：把可单测的纯逻辑（TTY 匹配、AppleScript 拼装与转义、终端宿主判定、通知去重）抽成纯函数并加单测，弥补开发期无真机。

## 4. 状态栏面板架构

**选型：tauri-nspanel + tauri-plugin-positioner（方案 A，已采纳）**

- 把主 WebviewWindow 转成 `NonactivatingPanel` 型 NSPanel：点图标弹出不抢焦点（不打断终端输入）、悬浮最上、失焦（resignKey）自动收起、不进 Cmd+Tab / Dock。
- `tauri-plugin-positioner` 把面板居中定位到托盘图标正下方（TrayCenter / TrayBottomCenter）。
- 隐藏 Dock 图标：`app.set_activation_policy(ActivationPolicy::Accessory)` + Info.plist `LSUIElement=true`（双保险，避免启动瞬间闪 Dock 图标）。
- **依赖风险记录**：`tauri-nspanel` 为社区 crate（`ahkohd/tauri-nspanel`，含少量 objc unsafe），是 macOS 菜单栏 App 的事实标准做法；实现期需确认其与当前 Tauri 2 版本兼容。

被否方案 B（纯 WebviewWindow + positioner + 手动 hide）：依赖更少，但弹出会激活 App、可能抢焦点、出现在窗口循环，失焦自动收起在面板内交互时不稳，体验不够「菜单栏」。

## 5. 菜单栏交互模型

- **左键托盘图标** → 切换面板显隐（再点或失焦收起）。
- **右键托盘图标** → 原生菜单 `设置` / `退出`（复用现有 `open_settings_window` 与 quit）。
- **面板内容** = 复用会话卡片列表（项目名/状态/标题/todo 进度/连接状态/tab 过滤），剥掉拖拽区、pin、resize 手柄。
- **设置窗口**（label `about`）打开时临时把 activation policy 切回 `Regular` 使其能正常获焦，关闭后切回 `Accessory`。

## 6. 前端重构（复用卡片，平台分流）

- 抽出纯卡片列表 UI（如 `StickerList`），Windows 与 macOS 共用。
- 启动检测平台（`@tauri-apps/plugin-os` `platform()`）：
  - **macOS**：渲染纯列表；**不挂** snap 状态机、`snap-changed` 监听、`CollapsedStrip`、pin/drag/resize。
  - **Windows**：保持现状（窗口 chrome + 吸边）完全不变。
- `snap_*` 命令在 macOS 保留注册但 body 为 no-op/Err，保持 invoke handler 名单一致（前端不调用即可）。
- 吸边相关测试在 macOS 分流下不受影响（Windows 测试保留）。

## 7. 终端跳转 / 恢复（macOS）— 最高风险项

**识别会话所在终端 tab（核心）**
- 从 DB 中 claude 进程 PID → 取控制终端 TTY（`ps -o tty= -p <pid>` 或 libproc）。
- Terminal.app 每个 tab、iTerm2 每个 session 都有 `tty` 属性；用 AppleScript 按 TTY 精确匹配，再 `select` tab + `activate`（比 Windows 标题匹配更可靠）。
- 判断终端类型：沿进程树上溯找终端宿主进程名（`Terminal` / `iTerm2`），决定用哪套 AppleScript（`sysinfo` 进程树遍历复用 `console_group_pids` 思路）。

**跳转（连接中）**：命中 → 选中 tab 并置前；宿主非 Terminal/iTerm2（Warp/kitty 等）→ fallback 到新开 Terminal.app `claude --resume`。

**恢复（已断开）**：复用 `title.rs` cwd 解析，**默认在 Terminal.app** `do script "cd <cwd> && claude --resume <id>"`（已采纳：默认 Terminal.app，不记忆 iTerm2 偏好）。

**安全**：session_id 仍走 UUID 校验（`is_session_id`）；cwd 做 AppleScript 字符串转义 + shell 引用，防注入。命令参数尽量以 argv 数组传递。

**权限（TCC）**：控制 Terminal/iTerm2 触发 macOS「自动化」授权弹窗 → Info.plist 加 `NSAppleEventsUsageDescription`；首次跳转弹一次系统授权，属预期行为，需在验收清单与 README 说明。

## 8. 桌面通知（macOS）

- **复用不变**：去重指纹（`should_notify` / `waiting_fingerprint`）、总开关 `notifications_enabled`、`spawn_liveness_watch` 轮询——只需填上 macOS 的 `show_session_notification` 实体。
- **主方案**：`UNUserNotificationCenter`（原生）+ 启动时注册 delegate，点击通知路由到现有 `focus_session`，实现「点击切到对应终端」。
- **退路**（点击路由原生接线成本过高时）：`tauri-plugin-notification` 弹通知 + 点击仅激活 App/面板（仍保留去重与展示）。具体 crate/API 实现期验证后定稿——次高风险点。
- 首次通知触发系统权限请求，Info.plist 加用途说明。

## 9. 构建 / CI / 发布

**tauri.conf.json**
- `bundle.targets` 改为 union（如 `["nsis","app","dmg"]`，Tauri 按宿主 OS 取交集）。
- 新增 `bundle.macOS`：`minimumSystemVersion: "14.0"`；签名 identity 由 CI env 注入。
- `updater` 端点不变（同一 `latest.json`，tauri-action 按 OS 写入 macOS 平台条目）；pubkey 跨平台复用。

**Info.plist（src-tauri 合并）**：`LSUIElement=true`、`NSAppleEventsUsageDescription`、通知用途说明。

**Cargo.toml**：新增 `[target.'cfg(target_os = "macos")'.dependencies]`（tauri-nspanel、tauri-plugin-positioner、objc2/cocoa 等按需）。

**CI（ci.yml）**：`runs-on` 改矩阵 `[windows-latest, macos-14]`；macOS 加 `universal-apple-darwin` 双架构 target（兼容 Intel + Apple Silicon），CI 装两个 rust target。

**release.yml**：加 macOS job/矩阵；tauri-action 注入签名+公证 env。需用户在 GitHub 配置的 secret 清单（实现期在 README/spec 落实）：
- `APPLE_CERTIFICATE`（base64 P12）、`APPLE_CERTIFICATE_PASSWORD`
- `APPLE_SIGNING_IDENTITY`（Developer ID Application: …）
- `APPLE_ID`、`APPLE_PASSWORD`（app 专用密码）、`APPLE_TEAM_ID`
- `KEYCHAIN_PASSWORD`（CI 临时钥匙串）
- 复用：`TAURI_SIGNING_PRIVATE_KEY`、`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`

**自动更新**：tauri-action 自动产 `.app.tar.gz` + minisign 签名并写 `latest.json` macOS 条目。

**图标**：`icon.icns` 已存在，校验尺寸（≥512²）。

## 10. 验证策略（开发先行，用户后测）

开发期可保证：
- `cargo test --workspace`、`cargo clippy --workspace -- -D warnings`、前端 `bunx tsc --noEmit` + `bunx vitest run` 全绿。
- **macOS CI job 编译 + 打包 dmg 成功**（「能在 Mac 跑」的最强离线证据）。
- 纯逻辑单测：TTY 解析、AppleScript 命令拼装与转义、终端宿主判定、通知去重指纹。

交付时提供 **Mac 真机验收清单**：面板弹出/收起、左右键、跳转 Terminal/iTerm2、未支持终端 fallback、恢复会话、通知点击聚焦、Dock 隐藏、签名公证后双击直开、自动更新链路。

## 11. 建议分期（实施计划阶段细化）

1. **打通 macOS 编译 + 面板骨架**：cfg 门控调用点、引入 nspanel/positioner、Accessory 策略、卡片 UI 抽取与复用、左键面板/右键菜单、平台分流。目标：macOS CI 出 dmg。
2. **终端跳转 / 恢复**：AppleScript + TTY 匹配 + 终端宿主判定 + 纯逻辑单测。
3. **通知**：UNUserNotificationCenter（或退路）+ 点击路由 `focus_session`。
4. **发布管线**：签名/公证/updater 接线 + secret 文档 + README 更新（移除「仅 Windows」措辞、补 macOS 下载与权限说明）。

## 12. 风险登记

| 风险 | 等级 | 缓解 |
|------|------|------|
| 终端 tab 精确定位（TTY/AppleScript） | 高 | TTY 匹配比标题更稳；纯逻辑单测；fallback 新开 Terminal |
| 通知点击→聚焦的原生 delegate 接线 | 中高 | 主用 UNUserNotificationCenter，退路降级为仅激活面板 |
| tauri-nspanel 与当前 Tauri 版本兼容 | 中 | 实现期先验证版本；不兼容则回退方案 B |
| 签名/公证 CI 配置繁琐 | 中 | tauri-action 内建支持；列清 secret 清单；先验证 ad-hoc 编译再加公证 |
| macOS 多屏坐标系（y 轴）与 positioner 定位 | 低中 | 面板由 positioner 相对托盘定位，规避手算坐标 |
| 开发期无真机，交互类 bug 滞后暴露 | 中 | 逻辑单测 + CI 打包 + 详尽真机验收清单 |

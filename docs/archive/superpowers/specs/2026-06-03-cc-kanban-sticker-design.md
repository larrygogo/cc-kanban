# Meowo 桌面贴纸 设计文档（计划 3）

> 日期：2026-06-03。计划 1（数据管线）、计划 2（当前活跃看板）已合并 main 并通过审计修复。

## 1. 背景与目标

计划 2 把 App 做成了「当前活跃」单窗口。但要真正解决「并行多任务、随时瞄一眼又不干扰视线」的核心痛点，需要一个**常驻桌面、置顶、不抢焦点**的轻量贴纸。计划 3 把产品形态从「单窗 App」转为「**系统托盘常驻 + 桌面贴纸**」。

**目标**：托盘图标常驻；一个透明、置顶、无边框的极简贴纸悬浮桌面角落，实时显示当前活跃会话；可拖动、记住位置；纯展示不干扰操作。

**非目标（YAGNI）**：多张贴纸、把会话拆成独立卡、点击行触发动作、鼠标穿透、保留普通大窗口、看板/总览视图入口。

## 2. 产品形态

- **启动**：程序起来 → 托盘图标常驻 + 贴纸显示在上次保存的位置（首次默认右上角）。
- **托盘菜单**：
  - 显示/隐藏贴纸（toggle）
  - 开机自启（勾选开关，**默认关**）
  - 退出
- **贴纸窗口**：透明背景、无边框（`decorations:false`）、置顶（`alwaysOnTop:true`）、不进任务栏与 Alt-Tab（`skipTaskbar:true`）、不抢焦点；可拖动；窗口位置+大小持久化。
- **内容**：极简紧凑，每行一个活跃会话——状态点（running=绿/waiting=黄/stale=灰）+ 项目名 + 一句话当前动作（截断）。顶部一条细「拖动手柄」。无活跃会话时显示一行「无活跃会话」（保持可见，提示程序在运行）。
- **实时**：复用计划 2 的 `get_live_sessions` 命令 + `listen("board-changed")` + stale 巡检线程，数据一变贴纸自动刷新。

### 布局示意

```
┌─────────────────────────┐  ← 细拖动手柄(data-tauri-drag-region)
│ ● reverse-bot-rs   内化端点│
│ ● clawmo-relay     集成推送│      透明 · 置顶 · 桌面角落
│ ◐ jsgraph          等待输入│      （灰点=可能已结束）
└─────────────────────────┘
```

## 3. 架构与改动（方案 A：单窗口即贴纸）

把现有那个窗口直接配置成贴纸，托盘控制其显隐。复用现成数据层（meowo-store 只读 + notify），前端把渲染换成极简贴纸视图。

数据流不变：
```
Claude Code hooks → meowo-reporter → SQLite(board.db, WAL)
                                      ▲
        Tauri 后端(按需开短连接读) ──┘  notify 监听 → emit board-changed
                                      → 贴纸前端 listen 后 get_live_sessions 重渲染
```

### 组件 / 文件改动

1. **`app/src-tauri/tauri.conf.json`**：主窗口改为贴纸——`transparent:true`、`decorations:false`、`alwaysOnTop:true`、`skipTaskbar:true`、`focus:false`、小初始尺寸（约 260 宽、高自适应内容/可调）、`visible:true`。
2. **`app/src-tauri/src/lib.rs`**：
   - 加 **TrayIcon + 菜单**（Tauri v2 `tray-icon` feature）：显示/隐藏贴纸、开机自启开关、退出。点击菜单项操作窗口 `show()/hide()`、`app.exit(0)`。
   - **窗口位置/大小持久化**：用官方 `tauri-plugin-window-state`（自动保存与恢复，免手写）。
   - **开机自启**：用官方 `tauri-plugin-autostart`（菜单开关调用 enable/disable）。
   - 保留 `spawn_db_watcher`、`spawn_stale_sweeper`、`get_live_sessions` 命令、按需开短连接（计划2+审计修复后的形态）。
3. **前端**：
   - 新建 `app/src/views/Sticker.tsx`：极简紧凑行视图（状态点 + 项目名 + 当前动作截断），顶部 `data-tauri-drag-region` 拖动手柄，空态「无活跃会话」。
   - `App.tsx` 渲染改为 `<Sticker>`（复用 `getLiveSessions` + `listen("board-changed")` 的拉取/刷新逻辑）。
   - `LiveView.tsx` / `Overview.tsx` / `ProjectBoard.tsx` 文件**保留不删**（备用），不再被路由。
   - 透明窗样式：`body` 背景透明，贴纸卡用半透明深色圆角（沿用既有暗色调）。
4. **依赖**：Tauri 开 `tray-icon` feature；加 `tauri-plugin-window-state`、`tauri-plugin-autostart`（及对应前端 npm 包，如开机自启需前端调用则装 `@tauri-apps/plugin-autostart`，否则纯 Rust 侧操作）。

## 4. 错误处理

- 沿用审计修复后的「按需开短连接」：库暂时不可用时该次刷新返回错误、贴纸显示空/上次内容，不崩。
- 托盘/插件初始化失败：降级处理（如 window-state 恢复失败用默认位置），不阻断启动。

## 5. 测试

- **前端**：`Sticker.tsx` 组件测试（vitest + testing-library）——空态文案、running/waiting/stale 三种状态点、当前动作渲染、拖动手柄存在。沿用计划 2 的测试模式。
- **Rust**：无新查询（复用 `live_sessions`，已测），故 meowo-store 无新单测。托盘菜单/窗口显隐/置顶/持久化属 GUI 行为，靠**手动验证**（启动→托盘可见→贴纸置顶→拖动后重启位置恢复→托盘隐藏/显示/退出生效）。

## 6. 交付范围

单机本地版的桌面贴纸形态。手机端、多端、多贴纸、看板拖拽等仍不在范围。

## 7. 关键默认值（可在 review 时调整）

- 托盘菜单三项：显示/隐藏、开机自启（默认关）、退出。
- 空态：显示「无活跃会话」一行（不自动隐藏窗口）。
- 贴纸内容含 stale（灰点「可能已结束」）。
- 密度固定极简紧凑（不在贴纸上做三档切换）。
- 初始位置：右上角；之后由 window-state 记住。

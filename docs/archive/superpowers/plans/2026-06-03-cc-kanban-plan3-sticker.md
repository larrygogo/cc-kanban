# Meowo 计划 3：托盘 + 桌面贴纸 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 Meowo 从单窗 App 变成「系统托盘常驻 + 透明置顶桌面贴纸」：托盘控制显隐/退出/开机自启，贴纸极简紧凑实时显示当前活跃会话、可拖动且位置持久化。

**Architecture:** 方案 A——现有主窗口直接配置成贴纸（transparent/decorations:false/alwaysOnTop/skipTaskbar）。Rust 侧加 Tauri v2 TrayIcon + 菜单、`tauri-plugin-window-state`（位置持久化）、`tauri-plugin-autostart`（开机自启）；复用既有 `get_live_sessions` 命令、notify watcher、stale 巡检、按需开短连接。前端把渲染换成极简 `Sticker` 视图，复用 `getLiveSessions` + `listen("board-changed")`。

**Tech Stack:** Tauri v2（feature `tray-icon` + 两个官方插件）、Rust、React 18 + Vite + TS、bun、vitest + @testing-library/react。

**前置：** 计划 1/2 + 审计修复已合并 main。`app/src-tauri/src/lib.rs` 现状：`AppState{db_path}`、命令 `get_overview`/`get_project_tasks`/`get_live_sessions`、`spawn_db_watcher`、`spawn_stale_sweeper`、按需开短连接、`run()` 用 `.setup()` 启动 watcher+sweeper。前端 `App.tsx` 渲染 `LiveView`（单视图）；`api.ts` 有 `getLiveSessions()`/类型 `LiveSession`。窗口当前是 1100×720 普通窗（label 默认 "main"）。

**开始前：** 开分支 `feat/sticker-20260603`（从 main 切出）。

---

## 文件结构

```
app/src-tauri/Cargo.toml                 # tauri 加 tray-icon feature + 两个插件依赖
app/src-tauri/tauri.conf.json            # 主窗口改贴纸配置
app/src-tauri/capabilities/default.json  # 加窗口拖动权限
app/src-tauri/src/lib.rs                 # 注册插件 + TrayIcon/菜单（显隐/自启/退出）
app/src/views/Sticker.tsx                # 新：极简紧凑贴纸视图（拖动手柄 + 空态）
app/src/views/Sticker.test.tsx           # 新：组件测试
app/src/App.tsx                          # 改为渲染 <Sticker>
app/src/styles.css                       # 透明 body + 贴纸/拖动手柄样式
```

`LiveView.tsx`/`Overview.tsx`/`ProjectBoard.tsx` 保留不删（备用，不再路由）。

---

## Task 1: 依赖 + 窗口贴纸配置

**Files:** Modify `Cargo.toml`(根)、`app/src-tauri/Cargo.toml`、`app/src-tauri/tauri.conf.json`、`app/src-tauri/capabilities/default.json`

- [ ] **Step 1: 根 workspace 加插件依赖**

在根 `Cargo.toml` 的 `[workspace.dependencies]` 追加：
```toml
tauri-plugin-window-state = "2"
tauri-plugin-autostart = "2"
```

- [ ] **Step 2: meowo-app 启用 tray-icon feature + 引入插件**

把 `app/src-tauri/Cargo.toml` 的 `[dependencies]` 里 tauri 那行改为带 feature，并加两个插件：
```toml
tauri = { workspace = true, features = ["tray-icon"] }
tauri-plugin-window-state = { workspace = true }
tauri-plugin-autostart = { workspace = true }
```
（其余依赖 meowo-store/serde/serde_json/notify 不动。）

- [ ] **Step 3: 窗口改贴纸配置**

把 `app/src-tauri/tauri.conf.json` 的 `app.windows` 改为：
```json
    "windows": [
      {
        "label": "main",
        "title": "Meowo",
        "width": 280,
        "height": 220,
        "minWidth": 200,
        "minHeight": 80,
        "transparent": true,
        "decorations": false,
        "alwaysOnTop": true,
        "skipTaskbar": true,
        "resizable": true,
        "focus": false,
        "shadow": false,
        "visible": true
      }
    ]
```
（其余 build/bundle/identifier 不动。）

- [ ] **Step 4: capabilities 加拖动权限**

把 `app/src-tauri/capabilities/default.json` 的 `permissions` 数组改为（加 start-dragging，供 `data-tauri-drag-region` 用）：
```json
  "permissions": [
    "core:default",
    "core:event:default",
    "core:window:default",
    "core:window:allow-start-dragging"
  ]
```

- [ ] **Step 5: 验证编译（拉插件依赖）**

Run: `cargo build -p meowo-app`
Expected: 拉取 tray-icon/两个插件依赖后编译通过（首次较慢）。**此时还没注册插件/托盘，只验证依赖与配置能编译。**

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml app/src-tauri/Cargo.toml app/src-tauri/tauri.conf.json app/src-tauri/capabilities/default.json
git commit -m "feat(app): 贴纸窗口配置(透明/无边框/置顶/skipTaskbar) + tray/插件依赖"
```

---

## Task 2: lib.rs —— 注册插件 + 托盘菜单（显隐/自启/退出）

**Files:** Modify `app/src-tauri/src/lib.rs`

> 说明：Tauri 托盘/窗口属 GUI 行为，难单测；本任务以「编译通过 + 后续 Task 5 手动验证」为准。先 Read 当前 lib.rs 确认 `run()`/`spawn_db_watcher`/`spawn_stale_sweeper`/命令 都在。

- [ ] **Step 1: 顶部 use 追加托盘/菜单/插件所需**

在 `lib.rs` 现有 `use` 区追加：
```rust
use tauri::menu::{CheckMenuItemBuilder, MenuBuilder, MenuItemBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::Manager;
use tauri_plugin_autostart::ManagerExt;
```
（若 `tauri::Manager` 已导入则不重复。）

- [ ] **Step 2: 加构建托盘的辅助函数**

在 `lib.rs` 加一个函数（放在 `run()` 之前）：
```rust
/// 构建系统托盘：显示/隐藏贴纸、开机自启开关、退出。
fn setup_tray(app: &tauri::App) -> tauri::Result<()> {
    let toggle = MenuItemBuilder::with_id("toggle", "显示/隐藏贴纸").build(app)?;
    let autostart_on = app.autolaunch().is_enabled().unwrap_or(false);
    let autostart = CheckMenuItemBuilder::with_id("autostart", "开机自启")
        .checked(autostart_on)
        .build(app)?;
    let quit = MenuItemBuilder::with_id("quit", "退出").build(app)?;
    let menu = MenuBuilder::new(app)
        .items(&[&toggle, &autostart, &quit])
        .build()?;

    let autostart_item = autostart.clone();
    TrayIconBuilder::with_id("meowo-tray")
        .icon(app.default_window_icon().unwrap().clone())
        .tooltip("Meowo")
        .menu(&menu)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "toggle" => {
                if let Some(w) = app.get_webview_window("main") {
                    if w.is_visible().unwrap_or(false) {
                        let _ = w.hide();
                    } else {
                        let _ = w.show();
                    }
                }
            }
            "autostart" => {
                let mgr = app.autolaunch();
                let now_on = if mgr.is_enabled().unwrap_or(false) {
                    let _ = mgr.disable();
                    false
                } else {
                    let _ = mgr.enable();
                    true
                };
                let _ = autostart_item.set_checked(now_on);
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(())
}
```

- [ ] **Step 3: 在 run() 注册插件 + 调 setup_tray**

把 `run()` 改为（在现有 `.setup` 里加 `setup_tray(app)?;`，并在 builder 上加两个 `.plugin(...)`）：
```rust
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let path = db_path();
    tauri::Builder::default()
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(AppState { db_path: path.clone() })
        .invoke_handler(tauri::generate_handler![
            get_overview,
            get_project_tasks,
            get_live_sessions
        ])
        .setup(move |app| {
            setup_tray(app)?;
            spawn_db_watcher(app.handle().clone(), path.clone());
            spawn_stale_sweeper(app.handle().clone(), path.clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

- [ ] **Step 4: 验证编译**

Run: `cargo build -p meowo-app`
Expected: 编译通过。若某 Tauri v2 API 名对不上（如菜单/托盘 builder 方法），**Read 已编译依赖的源或查 Tauri v2 文档（tray/menu/autostart）按真实 API 调整**，保持行为不变（显隐/自启/退出）。

- [ ] **Step 5: clippy**

Run: `cargo clippy -p meowo-app`
Expected: 无新 warning（清理未用 import）。

- [ ] **Step 6: Commit**

```bash
git add app/src-tauri/src/lib.rs
git commit -m "feat(app): 系统托盘菜单(显隐/开机自启/退出) + window-state/autostart 插件"
```

---

## Task 3: 前端 Sticker 视图 + 透明样式

**Files:** Create `app/src/views/Sticker.tsx`；Modify `app/src/App.tsx`、`app/src/styles.css`

- [ ] **Step 1: 写 Sticker.tsx**

```tsx
// app/src/views/Sticker.tsx
import { LiveSession } from "../api";

const DOT: Record<string, string> = {
  running: "dot-run",
  waiting: "dot-wait",
  stale: "dot-stale",
};

export function Sticker({ data }: { data: LiveSession[] }) {
  return (
    <div className="sticker">
      <div className="drag" data-tauri-drag-region />
      {data.length === 0 ? (
        <div className="stk-empty">无活跃会话</div>
      ) : (
        data.map((l) => {
          const unnamed = !l.task_title || l.task_title === "(未命名会话)";
          const activity = l.current_activity ?? (unnamed ? "等待首次输入" : "");
          return (
            <div className="stk-row" key={l.session.id}>
              <span className={"dot " + (DOT[l.session.status] ?? "dot-idle")} />
              <span className="stk-proj">{l.project_name}</span>
              <span className="stk-act">{activity}</span>
            </div>
          );
        })
      )}
    </div>
  );
}
```

- [ ] **Step 2: 改 App.tsx 渲染 Sticker**

```tsx
// app/src/App.tsx
import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getLiveSessions, LiveSession } from "./api";
import { Sticker } from "./views/Sticker";

export function App() {
  const [live, setLive] = useState<LiveSession[]>([]);

  const refresh = useCallback(async () => {
    setLive(await getLiveSessions());
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  useEffect(() => {
    const un = listen("board-changed", () => refresh());
    return () => {
      un.then((f) => f());
    };
  }, [refresh]);

  return <Sticker data={live} />;
}
```

- [ ] **Step 3: 透明 body + 贴纸样式**

把 `app/src/styles.css` 顶部 `body` 规则的 `background` 改为透明，并在文件末尾追加贴纸样式。先把：
```css
body {
  margin: 0;
  font-family: -apple-system, "Segoe UI", sans-serif;
  background: #0e0e12;
  color: #e8e8ea;
}
```
改成 `background: transparent;`（其余保留）。然后末尾追加：
```css
.sticker {
  background: rgba(18, 18, 22, 0.9);
  border: 1px solid rgba(255, 255, 255, 0.08);
  border-radius: 12px;
  padding: 4px 10px 8px;
  margin: 4px;
  -webkit-user-select: none;
  user-select: none;
  backdrop-filter: blur(6px);
}
.drag {
  height: 12px;
  cursor: move;
  border-radius: 6px;
}
.drag:hover {
  background: rgba(255, 255, 255, 0.06);
}
.stk-row {
  display: flex;
  align-items: center;
  gap: 7px;
  font-size: 12px;
  padding: 3px 0;
}
.stk-proj {
  font-weight: 600;
  flex: none;
  max-width: 110px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.stk-act {
  color: #8a8a92;
  font-size: 11px;
  margin-left: auto;
  max-width: 130px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.stk-empty {
  font-size: 12px;
  color: #5b5b63;
  padding: 6px 2px;
}
```
（`.dot`/`.dot-run`/`.dot-wait`/`.dot-stale` 已存在，沿用。）

- [ ] **Step 4: 类型检查 + 构建**

Run: `cd app && bunx tsc --noEmit && bun run build`
Expected: tsc 无错；build 成功。

- [ ] **Step 5: Commit**

```bash
git add app/src
git commit -m "feat(app): 极简贴纸视图(拖动手柄+空态) + 透明窗样式，App 渲染 Sticker"
```

---

## Task 4: Sticker 组件测试

**Files:** Create `app/src/views/Sticker.test.tsx`

- [ ] **Step 1: 写测试**

```tsx
// app/src/views/Sticker.test.tsx
import { describe, it, expect, afterEach } from "vitest";
import { render, screen, cleanup } from "@testing-library/react";
import { Sticker } from "./Sticker";
import type { LiveSession } from "../api";

function mk(over: Partial<LiveSession> = {}): LiveSession {
  return {
    session: { id: 1, project_id: 1, cc_session_id: "s", status: "running", started_at: 0, last_event_at: 0, ended_at: null },
    project_name: "proj",
    task_title: "做点事",
    current_activity: "正在做点事",
    column: "doing",
    todo_done: 0,
    todo_total: 0,
    todos: [],
    ...over,
  };
}

afterEach(() => cleanup());

describe("Sticker", () => {
  it("空数据显示无活跃会话", () => {
    const { container } = render(<Sticker data={[]} />);
    expect(screen.getByText("无活跃会话")).toBeTruthy();
    // 拖动手柄始终存在
    expect(container.querySelector("[data-tauri-drag-region]")).toBeTruthy();
  });

  it("渲染会话行：项目名 + 当前动作", () => {
    render(<Sticker data={[mk()]} />);
    expect(screen.getByText("proj")).toBeTruthy();
    expect(screen.getByText("正在做点事")).toBeTruthy();
  });

  it("unnamed 会话且无动作时显示等待首次输入", () => {
    render(<Sticker data={[mk({ task_title: "(未命名会话)", current_activity: null })]} />);
    expect(screen.getByText("等待首次输入")).toBeTruthy();
  });

  it("stale 会话用灰点", () => {
    const { container } = render(<Sticker data={[mk({ session: { id: 2, project_id: 1, cc_session_id: "x", status: "stale", started_at: 0, last_event_at: 0, ended_at: null } })]} />);
    expect(container.querySelector(".dot-stale")).toBeTruthy();
  });
});
```

- [ ] **Step 2: 跑测试**

Run: `cd app && bunx vitest run`
Expected: 全过（原 api.test 3 + LiveView 5 + App 2 + Sticker 4 = 14）。

- [ ] **Step 3: Commit**

```bash
git add app/src/views/Sticker.test.tsx
git commit -m "test(app): Sticker 视图组件测试"
```

---

## Task 5: 端到端手动验证（GUI）

**Files:** 无（验证）

- [ ] **Step 1: 用种子库启动 dev**

Run（bash；先确认无 meowo-app.exe 占用）：
```bash
cd app && MEOWO_DB="C:/Users/larry/Desktop/workspace/meowo/demo-board.db" bunx tauri dev
```
（demo-board.db 可用计划2验证时的种子方式灌几条；或指向真实 board.db。）

- [ ] **Step 2: 人工核对清单**

- 任务栏**不出现** Meowo（skipTaskbar 生效），但**系统托盘有图标**。
- 贴纸窗口**透明圆角、置顶**（盖在其它窗口之上）、显示活跃会话极简行（状态点颜色对：绿/黄/灰）。
- 拖动顶部手柄能移动贴纸到任意角落。
- 托盘菜单：「显示/隐藏贴纸」能切显隐；「开机自启」勾选状态可切换（勾上后查 Windows 启动项有 Meowo，取消则移除）；「退出」真的退出进程。
- **位置持久化**：拖到某处 → 退出 → 重开 → 贴纸回到上次位置。
- **实时**：另开终端往同一 `MEOWO_DB` 灌一条事件，贴纸约 300ms 内自动更新。

- [ ] **Step 3: 标记交付**

清单全过即计划 3 交付。`rm -f demo-board.db*`（如用了种子库）。

---

## 自检备忘（已核对）

- **Spec 覆盖**：托盘+菜单(spec §2)→Task 2；贴纸窗透明/置顶/skipTaskbar(§2/§3)→Task 1；极简紧凑行+状态点+空态(§2)→Task 3；位置持久化(§3 window-state)→Task 1/2 插件；开机自启(§2)→Task 2 autostart；实时(§2)→复用 listen，Task 3；运行/等待/stale 全显(§7)→Sticker 渲染所有 data（后端 live_sessions 已含三态）。
- **类型一致**：`Sticker({data: LiveSession[]})` 与 `App` 传入一致；`LiveSession.session.status`/`project_name`/`current_activity`/`task_title` 字段名与 api.ts 类型一致；命令 `get_live_sessions` 复用不变。
- **YAGNI**：不做多贴纸/穿透/点击动作/拆出卡；LiveView/Overview/ProjectBoard 保留不删但不路由。
- **GUI 测试边界**：托盘/置顶/透明/持久化无法单测，明列入 Task 5 手动清单。

# Meowo 计划 2：Tauri 只读看板 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 一个 Tauri v2 桌面窗口，读取计划 1 落地的 `board.db`，展示「项目总览 + 项目看板（待办/进行中/完成三列）」，并在 Claude Code 上报新进度时通过文件监听自动实时刷新。

**Architecture:** 三层。(A) 在 `meowo-store` 加只读聚合查询（纯 Rust，可 TDD），返回 `#[derive(Serialize)]` 的 DTO。(B) `app/src-tauri` 是 Tauri v2 后端：用 `Mutex<Store>` 作托管状态暴露 `#[tauri::command]`，并用 `notify` 监听 `board.db` 变更后 `emit("board-changed")`。(C) React+Vite+TS 前端通过 `invoke` 调命令、`listen("board-changed")` 触发重新拉取，在总览与项目看板间切换。

**Tech Stack:** Rust（rusqlite、serde、notify 6、tauri 2、tauri-build 2），前端 React 18 + Vite 5 + TypeScript + `@tauri-apps/api` 2，包管理 bun，测试 `cargo test` + `vitest`。

**前置：** 计划 1 已合并 main。`meowo-store` 导出 `Store` 及模型 `Project/Session/Task/Todo`（均 `#[derive(Serialize)]`）、枚举 `SessionStatus/TaskColumn/TodoStatus`。`Store` 持有 `pub(crate) conn: Connection`，已有 `open`/`open_in_memory`/`list_projects`/`get_task`/`get_session`/`list_todos` 等。模型字段：`Task { id, project_id, session_id: Option<i64>, title, column: String, column_locked: bool, current_activity: Option<String>, created_at, updated_at }`；SQL 表 `tasks` 里列名是 `column_name`。`Session { id, project_id, cc_session_id, status: String, started_at, last_event_at, ended_at: Option<i64> }`。

**开始前：** 先开一个功能分支 `feat/tauri-board-20260603`（`git checkout -b feat/tauri-board-20260603`，从 main 切出）。所有 commit 在该分支上。

---

## 文件结构

```
meowo/
  crates/meowo-store/
    src/query.rs               # 新增：只读聚合查询 + DTO（ProjectOverview/TaskCard）
    src/lib.rs                 # 导出 query 的 DTO
    tests/query_test.rs        # 新增：聚合查询测试
  app/                         # 新增：Tauri v2 应用
    package.json
    vite.config.ts
    tsconfig.json
    tsconfig.node.json
    index.html
    src/
      main.tsx
      App.tsx
      api.ts                   # invoke 包装 + TS 类型 + 纯聚合辅助
      api.test.ts              # vitest：纯逻辑测试（进度计算等）
      views/Overview.tsx
      views/ProjectBoard.tsx
      styles.css               # 暗色主题
    src-tauri/
      Cargo.toml
      build.rs
      tauri.conf.json
      capabilities/default.json
      icons/                   # 占位图标（用 tauri 默认）
      src/
        main.rs
        lib.rs                 # AppState + commands + notify 监听
  Cargo.toml                   # 根 workspace：members 增加 "app/src-tauri"
```

职责：`query.rs` 只读聚合不掺写逻辑；Tauri `lib.rs` 只做「命令薄封装 + 文件监听 emit」；React 每个 view 单一视图。

---

## 阶段 A：meowo-store 只读聚合查询（纯 Rust，TDD）

### Task A1: ProjectOverview 聚合查询

**Files:**
- Create: `crates/meowo-store/src/query.rs`
- Modify: `crates/meowo-store/src/lib.rs`
- Create: `crates/meowo-store/tests/query_test.rs`

- [ ] **Step 1: 在 lib.rs 挂上 query 模块并导出 DTO**

把 `crates/meowo-store/src/lib.rs` 改为（在现有基础上加 `pub mod query;` 与 re-export）：
```rust
pub mod error;
pub mod migrations;
pub mod models;
pub mod query;
pub mod store;

pub use error::StoreError;
pub use models::*;
pub use query::{ProjectOverview, TaskCard};
pub use store::Store;
```

- [ ] **Step 2: 写失败测试**

`crates/meowo-store/tests/query_test.rs`：
```rust
use meowo_store::{Store, TodoInput, TodoStatus};

#[test]
fn overview_aggregates_counts_and_active_sessions() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();

    // 会话1：有 in_progress todo -> doing 列；会话 running（活跃）
    let (s1, _t1) = store.start_session(pid, "s1", 200).unwrap();
    store.on_user_prompt(s1, "任务一", 210).unwrap();
    store.sync_todos(s1, &[TodoInput { content: "a".into(), status: TodoStatus::InProgress }], 220).unwrap();

    // 会话2：全 completed -> done 列；会话 ended（不活跃）
    let (s2, _t2) = store.start_session(pid, "s2", 300).unwrap();
    store.on_user_prompt(s2, "任务二", 310).unwrap();
    store.sync_todos(s2, &[TodoInput { content: "b".into(), status: TodoStatus::Completed }], 320).unwrap();
    store.end_session(s2, 330).unwrap();

    let ov = store.overview().unwrap();
    assert_eq!(ov.len(), 1);
    let o = &ov[0];
    assert_eq!(o.project.name, "p");
    assert_eq!(o.active_sessions, 1); // 只有 s1 是 running/waiting
    assert_eq!(o.doing_count, 1);
    assert_eq!(o.done_count, 1);
    assert_eq!(o.todo_count, 0);
    assert_eq!(o.last_activity_at, 330); // 最大 last_event_at
}

#[test]
fn overview_empty_when_no_projects() {
    let store = Store::open_in_memory().unwrap();
    assert_eq!(store.overview().unwrap().len(), 0);
}
```

- [ ] **Step 3: 运行确认失败**

Run: `cargo test -p meowo-store overview_aggregates_counts_and_active_sessions`
Expected: FAIL，缺 `overview`。

- [ ] **Step 4: 实现 query.rs**

```rust
// crates/meowo-store/src/query.rs
use crate::error::StoreError;
use crate::models::{Project, Task, Todo};
use crate::store::Store;
use serde::Serialize;

/// 总览里每个项目一行的聚合。
#[derive(Debug, Clone, Serialize)]
pub struct ProjectOverview {
    pub project: Project,
    pub active_sessions: i64,
    pub todo_count: i64,
    pub doing_count: i64,
    pub done_count: i64,
    pub last_activity_at: i64,
}

/// 项目看板里一张任务卡：任务 + 子清单 + 关联会话状态。
#[derive(Debug, Clone, Serialize)]
pub struct TaskCard {
    pub task: Task,
    pub todos: Vec<Todo>,
    pub session_status: Option<String>,
}

impl Store {
    /// 所有项目的总览聚合，按 last_activity_at 倒序（最近活跃在前）。
    pub fn overview(&self) -> Result<Vec<ProjectOverview>, StoreError> {
        let projects = self.list_projects()?;
        let mut out = Vec::with_capacity(projects.len());
        for project in projects {
            let pid = project.id;
            let active_sessions: i64 = self.conn.query_row(
                "SELECT count(*) FROM sessions WHERE project_id = ?1 AND status IN ('running','waiting')",
                [pid],
                |r| r.get(0),
            )?;
            let col_count = |col: &str| -> Result<i64, StoreError> {
                let n: i64 = self.conn.query_row(
                    "SELECT count(*) FROM tasks WHERE project_id = ?1 AND column_name = ?2",
                    rusqlite::params![pid, col],
                    |r| r.get(0),
                )?;
                Ok(n)
            };
            let todo_count = col_count("todo")?;
            let doing_count = col_count("doing")?;
            let done_count = col_count("done")?;
            let last_activity_at: i64 = self.conn.query_row(
                "SELECT COALESCE(MAX(last_event_at), ?2) FROM sessions WHERE project_id = ?1",
                rusqlite::params![pid, project.updated_at],
                |r| r.get(0),
            )?;
            out.push(ProjectOverview {
                project,
                active_sessions,
                todo_count,
                doing_count,
                done_count,
                last_activity_at,
            });
        }
        out.sort_by(|a, b| b.last_activity_at.cmp(&a.last_activity_at));
        Ok(out)
    }
}
```

- [ ] **Step 5: 运行确认通过**

Run: `cargo test -p meowo-store overview`
Expected: 两个 overview 测试 PASS。

- [ ] **Step 6: Commit**

```bash
git add crates/meowo-store
git commit -m "feat(store): overview 项目总览聚合查询"
```

---

### Task A2: project_tasks 看板卡片查询

**Files:**
- Modify: `crates/meowo-store/src/query.rs`
- Modify: `crates/meowo-store/tests/query_test.rs`

- [ ] **Step 1: 写失败测试**

追加到 `crates/meowo-store/tests/query_test.rs`：
```rust
#[test]
fn project_tasks_returns_cards_with_todos_and_session_status() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (s1, t1) = store.start_session(pid, "s1", 200).unwrap();
    store.on_user_prompt(s1, "卡一", 210).unwrap();
    store.sync_todos(s1, &[
        meowo_store::TodoInput { content: "x".into(), status: meowo_store::TodoStatus::InProgress },
        meowo_store::TodoInput { content: "y".into(), status: meowo_store::TodoStatus::Pending },
    ], 220).unwrap();

    let cards = store.project_tasks(pid).unwrap();
    assert_eq!(cards.len(), 1);
    let c = &cards[0];
    assert_eq!(c.task.id, t1);
    assert_eq!(c.task.title, "卡一");
    assert_eq!(c.task.column, "doing");
    assert_eq!(c.todos.len(), 2);
    assert_eq!(c.todos[0].content, "x");
    assert_eq!(c.session_status.as_deref(), Some("running"));
}

#[test]
fn project_tasks_empty_for_unknown_project() {
    let store = Store::open_in_memory().unwrap();
    assert_eq!(store.project_tasks(999).unwrap().len(), 0);
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p meowo-store project_tasks`
Expected: FAIL，缺 `project_tasks`。

- [ ] **Step 3: 实现（追加到 query.rs 的 `impl Store`）**

```rust
impl Store {
    /// 某项目的所有任务卡，按 updated_at 倒序（最近更新在前）。
    pub fn project_tasks(&self, project_id: i64) -> Result<Vec<TaskCard>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project_id, session_id, title, column_name, column_locked, current_activity, created_at, updated_at
             FROM tasks WHERE project_id = ?1 ORDER BY updated_at DESC, id DESC",
        )?;
        let tasks = stmt
            .query_map([project_id], |r| {
                Ok(Task {
                    id: r.get(0)?,
                    project_id: r.get(1)?,
                    session_id: r.get(2)?,
                    title: r.get(3)?,
                    column: r.get(4)?,
                    column_locked: r.get::<_, i64>(5)? != 0,
                    current_activity: r.get(6)?,
                    created_at: r.get(7)?,
                    updated_at: r.get(8)?,
                })
            })?
            .collect::<Result<Vec<Task>, _>>()?;

        let mut out = Vec::with_capacity(tasks.len());
        for task in tasks {
            let todos = self.list_todos(task.id)?;
            let session_status = match task.session_id {
                Some(sid) => self
                    .conn
                    .query_row("SELECT status FROM sessions WHERE id = ?1", [sid], |r| r.get(0))
                    .ok(),
                None => None,
            };
            out.push(TaskCard { task, todos, session_status });
        }
        Ok(out)
    }
}
```
> 注意：`Task` 已在 `crate::models` 导入到本文件顶部（`use crate::models::{Project, Task, Todo};`）。

- [ ] **Step 4: 运行确认通过**

Run: `cargo test -p meowo-store`
Expected: 全部 PASS（含新 2 个）。

- [ ] **Step 5: Commit**

```bash
git add crates/meowo-store
git commit -m "feat(store): project_tasks 看板卡片查询（任务+子清单+会话状态）"
```

---

## 阶段 B：Tauri v2 后端（命令 + 文件监听）

> 说明：Tauri 部分难以纯 TDD，改为「构建通过 + 命令冒烟 + 手动验证」。务必每步跑 `cargo build` 确认编译。

### Task B1: 把 Tauri app 加入 workspace 并能空跑

**Files:**
- Modify: `Cargo.toml`（根）
- Create: `app/src-tauri/Cargo.toml`、`app/src-tauri/build.rs`、`app/src-tauri/tauri.conf.json`、`app/src-tauri/capabilities/default.json`、`app/src-tauri/src/main.rs`、`app/src-tauri/src/lib.rs`
- Create: 前端最小占位 `app/package.json`、`app/index.html`、`app/vite.config.ts`、`app/tsconfig.json`、`app/tsconfig.node.json`、`app/src/main.tsx`

- [ ] **Step 1: 根 workspace 加入成员**

把根 `Cargo.toml` 的 `members` 改为：
```toml
members = ["crates/meowo-store", "crates/meowo-reporter", "app/src-tauri"]
```
并在 `[workspace.dependencies]` 追加：
```toml
tauri = { version = "2", features = [] }
tauri-build = { version = "2", features = [] }
notify = "6"
```

- [ ] **Step 2: src-tauri Cargo.toml**

```toml
# app/src-tauri/Cargo.toml
[package]
name = "meowo-app"
version = "0.1.0"
edition = "2021"

[lib]
name = "meowo_app_lib"
crate-type = ["staticlib", "cdylib", "rlib"]

[build-dependencies]
tauri-build = { workspace = true }

[dependencies]
meowo-store = { path = "../../crates/meowo-store" }
tauri = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
notify = { workspace = true }
```

- [ ] **Step 3: build.rs**

```rust
// app/src-tauri/build.rs
fn main() {
    tauri_build::build();
}
```

- [ ] **Step 4: tauri.conf.json（Tauri v2 schema）**

```json
{
  "$schema": "https://schema.tauri.app/config/2",
  "productName": "Meowo",
  "version": "0.1.0",
  "identifier": "com.larrygogo.meowo",
  "build": {
    "frontendDist": "../dist",
    "devUrl": "http://localhost:1420",
    "beforeDevCommand": "bun run dev",
    "beforeBuildCommand": "bun run build"
  },
  "app": {
    "windows": [
      {
        "title": "Meowo",
        "width": 1100,
        "height": 720,
        "resizable": true
      }
    ],
    "security": {
      "csp": null
    }
  },
  "bundle": {
    "active": true,
    "targets": "all",
    "icon": ["icons/icon.png"]
  }
}
```

- [ ] **Step 5: capabilities/default.json（允许前端调用 core 与事件）**

```json
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "default",
  "description": "默认窗口能力",
  "windows": ["main"],
  "permissions": [
    "core:default",
    "core:event:default",
    "core:window:default"
  ]
}
```

- [ ] **Step 6: 占位图标**

Run（生成 Tauri 默认图标，需先有任意 PNG；若无则从一个纯色 PNG 生成）：
```bash
cd app/src-tauri
# 若已安装 tauri-cli 可用：bunx @tauri-apps/cli icon path/to/any.png
# 简化：先放一个 32x32 占位 png 到 icons/icon.png（用任意工具/下载占位）
mkdir -p icons
```
> 若没有现成 PNG，最省事：`bunx @tauri-apps/cli@latest icon`（无参时它会找默认 app-icon.png）。实在缺图标先放一个 1x1 png 也能过 dev（bundle 才严格要求）。本计划 dev 阶段不依赖图标完整性。

- [ ] **Step 7: src-tauri/src/lib.rs（先空运行）**

```rust
// app/src-tauri/src/lib.rs
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

- [ ] **Step 8: src-tauri/src/main.rs**

```rust
// app/src-tauri/src/main.rs
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    meowo_app_lib::run();
}
```

- [ ] **Step 9: 前端最小占位**

```json
// app/package.json
{
  "name": "meowo-frontend",
  "private": true,
  "version": "0.1.0",
  "type": "module",
  "scripts": {
    "dev": "vite",
    "build": "tsc && vite build",
    "preview": "vite preview",
    "test": "vitest run"
  },
  "dependencies": {
    "@tauri-apps/api": "^2",
    "react": "^18",
    "react-dom": "^18"
  },
  "devDependencies": {
    "@types/react": "^18",
    "@types/react-dom": "^18",
    "@vitejs/plugin-react": "^4",
    "typescript": "^5",
    "vite": "^5",
    "vitest": "^2"
  }
}
```
```html
<!-- app/index.html -->
<!doctype html>
<html lang="zh">
  <head><meta charset="UTF-8" /><title>Meowo</title></head>
  <body>
    <div id="root"></div>
    <script type="module" src="/src/main.tsx"></script>
  </body>
</html>
```
```typescript
// app/vite.config.ts
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
});
```
```json
// app/tsconfig.json
{
  "compilerOptions": {
    "target": "ES2020",
    "useDefineForClassFields": true,
    "lib": ["ES2020", "DOM", "DOM.Iterable"],
    "module": "ESNext",
    "skipLibCheck": true,
    "moduleResolution": "bundler",
    "resolveJsonModule": true,
    "isolatedModules": true,
    "noEmit": true,
    "jsx": "react-jsx",
    "strict": true
  },
  "include": ["src"],
  "references": [{ "path": "./tsconfig.node.json" }]
}
```
```json
// app/tsconfig.node.json
{
  "compilerOptions": {
    "composite": true,
    "skipLibCheck": true,
    "module": "ESNext",
    "moduleResolution": "bundler",
    "allowSyntheticDefaultImports": true
  },
  "include": ["vite.config.ts"]
}
```
```tsx
// app/src/main.tsx
import React from "react";
import ReactDOM from "react-dom/client";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <div>Meowo</div>
  </React.StrictMode>,
);
```

- [ ] **Step 10: 安装前端依赖并验证构建**

Run:
```bash
cd app && bun install
cd .. && cargo build -p meowo-app
```
Expected: 前端依赖装好；`cargo build -p meowo-app` 编译通过（Tauri 首次会拉不少依赖，耐心等）。

- [ ] **Step 11: Commit**

```bash
git add Cargo.toml app
git commit -m "feat(app): Tauri v2 应用骨架（可空运行）"
```

---

### Task B2: AppState + 三个只读命令

**Files:**
- Modify: `app/src-tauri/src/lib.rs`

- [ ] **Step 1: 写带 AppState 与命令的 lib.rs**

```rust
// app/src-tauri/src/lib.rs
use meowo_store::{ProjectOverview, Store, TaskCard};
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{Manager, State};

/// 托管状态：持有打开的 Store（单连接，命令间用 Mutex 串行）。
struct AppState {
    store: Mutex<Store>,
}

/// 库路径：环境变量 MEOWO_DB 优先，否则 ~/.meowo/board.db。
fn db_path() -> PathBuf {
    if let Ok(p) = std::env::var("MEOWO_DB") {
        return PathBuf::from(p);
    }
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".meowo").join("board.db")
}

#[tauri::command]
fn get_overview(state: State<AppState>) -> Result<Vec<ProjectOverview>, String> {
    let store = state.store.lock().map_err(|e| e.to_string())?;
    store.overview().map_err(|e| e.to_string())
}

#[tauri::command]
fn get_project_tasks(state: State<AppState>, project_id: i64) -> Result<Vec<TaskCard>, String> {
    let store = state.store.lock().map_err(|e| e.to_string())?;
    store.project_tasks(project_id).map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let store = Store::open(db_path()).expect("打开 board.db 失败");
    tauri::Builder::default()
        .manage(AppState { store: Mutex::new(store) })
        .invoke_handler(tauri::generate_handler![get_overview, get_project_tasks])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```
> 说明：`Mutex<Store>` 让状态满足 `Send + Sync`（`rusqlite::Connection` 是 `Send`）。命令串行执行，对只读看板足够。`db_path` 与 reporter 保持同一默认路径，所以读的就是上报器写的库。

- [ ] **Step 2: 验证编译**

Run: `cargo build -p meowo-app`
Expected: 编译通过。

- [ ] **Step 3: Commit**

```bash
git add app/src-tauri/src/lib.rs
git commit -m "feat(app): AppState + get_overview/get_project_tasks 命令"
```

---

### Task B3: notify 文件监听 → emit("board-changed")

**Files:**
- Modify: `app/src-tauri/src/lib.rs`

- [ ] **Step 1: 在 run() 的 setup 里启动 watcher**

把 `run()` 改为带 `.setup(...)`，并加监听函数：
```rust
// 在文件顶部 use 区追加：
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};
use tauri::Emitter;

// 替换原 run()：
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let path = db_path();
    let store = Store::open(&path).expect("打开 board.db 失败");
    tauri::Builder::default()
        .manage(AppState { store: Mutex::new(store) })
        .invoke_handler(tauri::generate_handler![get_overview, get_project_tasks])
        .setup(move |app| {
            spawn_db_watcher(app.handle().clone(), path.clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// 监听 board.db 所在目录的变更，去抖后向前端发 "board-changed"。
fn spawn_db_watcher(app: tauri::AppHandle, db_path: PathBuf) {
    let watch_dir = db_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    std::thread::spawn(move || {
        let (tx, rx) = channel();
        let mut watcher: RecommendedWatcher = match notify::recommended_watcher(tx) {
            Ok(w) => w,
            Err(_) => return,
        };
        if watcher.watch(&watch_dir, RecursiveMode::NonRecursive).is_err() {
            return;
        }
        let debounce = Duration::from_millis(300);
        let mut last_emit = Instant::now() - debounce;
        for res in rx {
            if res.is_err() {
                continue;
            }
            // 去抖：300ms 内多次变更只发一次
            if last_emit.elapsed() >= debounce {
                let _ = app.emit("board-changed", ());
                last_emit = Instant::now();
            }
        }
    });
}
```
> 注意：WAL 模式下写会落到 `board.db-wal`，监听整个目录（NonRecursive）能捕获 `board.db` / `-wal` / `-shm` 的变化。去抖避免一次提交触发多次刷新。

- [ ] **Step 2: 验证编译**

Run: `cargo build -p meowo-app`
Expected: 编译通过。

- [ ] **Step 3: Commit**

```bash
git add app/src-tauri/src/lib.rs
git commit -m "feat(app): notify 监听 board.db 变更并 emit board-changed"
```

---

## 阶段 C：React 前端

### Task C1: api.ts —— invoke 包装 + 类型 + 纯逻辑（vitest）

**Files:**
- Create: `app/src/api.ts`
- Create: `app/src/api.test.ts`

- [ ] **Step 1: 写失败测试（纯逻辑：进度计算）**

```typescript
// app/src/api.test.ts
import { describe, it, expect } from "vitest";
import { todoProgress } from "./api";

describe("todoProgress", () => {
  it("counts completed over total", () => {
    expect(todoProgress([
      { id: 1, task_id: 1, content: "a", status: "completed", order_idx: 0 },
      { id: 2, task_id: 1, content: "b", status: "in_progress", order_idx: 1 },
    ])).toEqual({ done: 1, total: 2, percent: 50 });
  });
  it("zero todos -> 0% and total 0", () => {
    expect(todoProgress([])).toEqual({ done: 0, total: 0, percent: 0 });
  });
  it("all done -> 100%", () => {
    expect(todoProgress([
      { id: 1, task_id: 1, content: "a", status: "completed", order_idx: 0 },
    ])).toEqual({ done: 1, total: 1, percent: 100 });
  });
});
```

- [ ] **Step 2: 运行确认失败**

Run: `cd app && bunx vitest run src/api.test.ts`
Expected: FAIL（找不到 `todoProgress`）。

- [ ] **Step 3: 实现 api.ts**

```typescript
// app/src/api.ts
import { invoke } from "@tauri-apps/api/core";

export type Todo = {
  id: number;
  task_id: number;
  content: string;
  status: "pending" | "in_progress" | "completed";
  order_idx: number;
};

export type Task = {
  id: number;
  project_id: number;
  session_id: number | null;
  title: string;
  column: "todo" | "doing" | "done";
  column_locked: boolean;
  current_activity: string | null;
  created_at: number;
  updated_at: number;
};

export type Project = {
  id: number;
  root_path: string;
  name: string;
  created_at: number;
  updated_at: number;
};

export type ProjectOverview = {
  project: Project;
  active_sessions: number;
  todo_count: number;
  doing_count: number;
  done_count: number;
  last_activity_at: number;
};

export type TaskCard = {
  task: Task;
  todos: Todo[];
  session_status: string | null;
};

export function getOverview(): Promise<ProjectOverview[]> {
  return invoke("get_overview");
}

export function getProjectTasks(projectId: number): Promise<TaskCard[]> {
  return invoke("get_project_tasks", { projectId });
}

/// 纯函数：根据 todo 列表算完成度。
export function todoProgress(todos: Todo[]): { done: number; total: number; percent: number } {
  const total = todos.length;
  const done = todos.filter((t) => t.status === "completed").length;
  const percent = total === 0 ? 0 : Math.round((done / total) * 100);
  return { done, total, percent };
}
```
> 注意：Tauri 命令参数名 Rust 端是 `project_id`，JS 端 invoke 传 `projectId`（Tauri 自动 camelCase↔snake_case 转换）。

- [ ] **Step 4: 运行确认通过**

Run: `cd app && bunx vitest run src/api.test.ts`
Expected: 3 个测试 PASS。

- [ ] **Step 5: Commit**

```bash
git add app/src/api.ts app/src/api.test.ts
git commit -m "feat(app): api 封装 invoke + 类型 + todoProgress（vitest 覆盖）"
```

---

### Task C2: 视图组件 + App 路由 + 实时刷新

**Files:**
- Create: `app/src/views/Overview.tsx`、`app/src/views/ProjectBoard.tsx`、`app/src/styles.css`
- Modify: `app/src/App.tsx`（新建）、`app/src/main.tsx`

- [ ] **Step 1: 暗色主题样式**

```css
/* app/src/styles.css */
:root { color-scheme: dark; }
* { box-sizing: border-box; }
body {
  margin: 0;
  font-family: -apple-system, "Segoe UI", sans-serif;
  background: #0e0e12;
  color: #e8e8ea;
}
.app { padding: 18px 22px; }
.h1 { font-size: 18px; font-weight: 600; margin: 0 0 14px; }
.back { cursor: pointer; color: #8a8a92; font-size: 13px; margin-bottom: 12px; display: inline-block; }
.back:hover { color: #e8e8ea; }

.proj-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(260px, 1fr)); gap: 12px; }
.proj-card {
  background: rgba(24,24,28,0.92); border: 1px solid rgba(255,255,255,0.06);
  border-radius: 12px; padding: 14px 16px; cursor: pointer;
}
.proj-card:hover { border-color: rgba(255,255,255,0.18); }
.proj-name { font-weight: 600; display: flex; align-items: center; gap: 7px; }
.dot { width: 8px; height: 8px; border-radius: 50%; flex: none; }
.dot-active { background: #34d399; box-shadow: 0 0 8px #34d399; }
.dot-idle { background: #5b5b63; }
.proj-meta { font-size: 12px; color: #8a8a92; margin-top: 8px; display: flex; gap: 12px; }

.board { display: grid; grid-template-columns: repeat(3, 1fr); gap: 14px; }
.col-title { font-size: 13px; color: #b9b9c0; margin-bottom: 8px; }
.task-card {
  background: rgba(24,24,28,0.92); border: 1px solid rgba(255,255,255,0.06);
  border-radius: 10px; padding: 11px 13px; margin-bottom: 10px;
}
.task-title { font-size: 13px; font-weight: 600; }
.task-act { font-size: 11.5px; color: #8a8a92; margin-top: 5px; }
.bar { height: 5px; border-radius: 3px; background: rgba(255,255,255,0.1); margin-top: 9px; overflow: hidden; }
.bar > i { display: block; height: 100%; background: linear-gradient(90deg,#34d399,#22d3ee); }
.empty { color: #5b5b63; font-size: 12px; }
```

- [ ] **Step 2: Overview.tsx**

```tsx
// app/src/views/Overview.tsx
import { ProjectOverview } from "../api";

export function Overview({
  data,
  onOpen,
}: {
  data: ProjectOverview[];
  onOpen: (projectId: number, name: string) => void;
}) {
  if (data.length === 0) {
    return <div className="empty">还没有任何项目。打开一个 Claude Code 会话试试。</div>;
  }
  return (
    <div className="proj-grid">
      {data.map((o) => (
        <div className="proj-card" key={o.project.id} onClick={() => onOpen(o.project.id, o.project.name)}>
          <div className="proj-name">
            <span className={"dot " + (o.active_sessions > 0 ? "dot-active" : "dot-idle")} />
            {o.project.name}
          </div>
          <div className="proj-meta">
            <span>{o.active_sessions} 活跃</span>
            <span>{o.todo_count} 待办</span>
            <span>{o.doing_count} 进行</span>
            <span>{o.done_count} 完成</span>
          </div>
        </div>
      ))}
    </div>
  );
}
```

- [ ] **Step 3: ProjectBoard.tsx**

```tsx
// app/src/views/ProjectBoard.tsx
import { TaskCard, todoProgress } from "../api";

const COLUMNS: { key: "todo" | "doing" | "done"; label: string }[] = [
  { key: "todo", label: "待办" },
  { key: "doing", label: "进行中" },
  { key: "done", label: "完成" },
];

function Card({ card }: { card: TaskCard }) {
  const { done, total, percent } = todoProgress(card.todos);
  return (
    <div className="task-card">
      <div className="task-title">{card.task.title}</div>
      {card.task.current_activity && <div className="task-act">{card.task.current_activity}</div>}
      {total > 0 && (
        <>
          <div className="bar">
            <i style={{ width: `${percent}%` }} />
          </div>
          <div className="task-act">
            {done}/{total} · {percent}%
          </div>
        </>
      )}
    </div>
  );
}

export function ProjectBoard({ cards }: { cards: TaskCard[] }) {
  return (
    <div className="board">
      {COLUMNS.map((col) => {
        const inCol = cards.filter((c) => c.task.column === col.key);
        return (
          <div key={col.key}>
            <div className="col-title">
              {col.label}（{inCol.length}）
            </div>
            {inCol.length === 0 ? (
              <div className="empty">—</div>
            ) : (
              inCol.map((c) => <Card key={c.task.id} card={c} />)
            )}
          </div>
        );
      })}
    </div>
  );
}
```

- [ ] **Step 4: App.tsx（路由 + 拉数据 + 监听刷新）**

```tsx
// app/src/App.tsx
import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getOverview, getProjectTasks, ProjectOverview, TaskCard } from "./api";
import { Overview } from "./views/Overview";
import { ProjectBoard } from "./views/ProjectBoard";

type View = { kind: "overview" } | { kind: "board"; projectId: number; name: string };

export function App() {
  const [view, setView] = useState<View>({ kind: "overview" });
  const [overview, setOverview] = useState<ProjectOverview[]>([]);
  const [cards, setCards] = useState<TaskCard[]>([]);

  const refresh = useCallback(async (v: View) => {
    if (v.kind === "overview") {
      setOverview(await getOverview());
    } else {
      setCards(await getProjectTasks(v.projectId));
    }
  }, []);

  // 视图变化时拉一次
  useEffect(() => {
    refresh(view);
  }, [view, refresh]);

  // 监听 board-changed 实时刷新当前视图
  useEffect(() => {
    const un = listen("board-changed", () => refresh(view));
    return () => {
      un.then((f) => f());
    };
  }, [view, refresh]);

  if (view.kind === "overview") {
    return (
      <div className="app">
        <div className="h1">项目总览</div>
        <Overview data={overview} onOpen={(projectId, name) => setView({ kind: "board", projectId, name })} />
      </div>
    );
  }
  return (
    <div className="app">
      <span className="back" onClick={() => setView({ kind: "overview" })}>
        ← 返回总览
      </span>
      <div className="h1">{view.name}</div>
      <ProjectBoard cards={cards} />
    </div>
  );
}
```

- [ ] **Step 5: 更新 main.tsx 挂载 App + 样式**

```tsx
// app/src/main.tsx
import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";
import "./styles.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
```

- [ ] **Step 6: 类型检查 + 测试**

Run:
```bash
cd app && bunx tsc --noEmit && bunx vitest run
```
Expected: tsc 无报错；vitest 3 个测试通过。

- [ ] **Step 7: Commit**

```bash
git add app/src
git commit -m "feat(app): 总览/项目看板视图 + board-changed 实时刷新"
```

---

### Task C3: 端到端手动验证

**Files:** 无（验证）

- [ ] **Step 1: 准备种子数据**

用计划 1 的 reporter 往一个临时库灌点数据，或直接用真实 `~/.meowo/board.db`（里面已有真实会话）。临时库示例（bash）：
```bash
export MEOWO_DB="$(pwd)/dev-board.db"
rm -f dev-board.db*
BIN=./target/release/meowo-reporter.exe
echo '{"hook_event_name":"SessionStart","session_id":"d1","cwd":"/tmp/projA"}' | $BIN
echo '{"hook_event_name":"UserPromptSubmit","session_id":"d1","prompt":"实现登录页"}' | $BIN
echo '{"hook_event_name":"PostToolUse","session_id":"d1","tool_name":"TodoWrite","tool_input":{"todos":[{"content":"画 UI","status":"completed"},{"content":"接接口","status":"in_progress"}]}}' | $BIN
```

- [ ] **Step 2: 启动 dev（指向该库）**

Run（保持上面的 `MEOWO_DB` 环境变量）：
```bash
cd app/src-tauri && cargo tauri dev
```
> 若没装 tauri-cli：`bun add -d @tauri-apps/cli` 后用 `bunx tauri dev`（在 `app/` 下）。`beforeDevCommand` 会自动起 Vite。

- [ ] **Step 3: 人工核对**

窗口应显示总览有 `projA`（1 活跃 / 1 进行）；点进去，看板「进行中」列有一张「实现登录页」卡，进度 1/2 · 50%，当前动作显示。

- [ ] **Step 4: 验证实时刷新**

dev 运行时，另开终端再灌一条（同一个 `MEOWO_DB`）：
```bash
echo '{"hook_event_name":"PostToolUse","session_id":"d1","tool_name":"TodoWrite","tool_input":{"todos":[{"content":"画 UI","status":"completed"},{"content":"接接口","status":"completed"}]}}' | ./target/release/meowo-reporter.exe
```
界面应在约 300ms 内自动把卡片移到「完成」列、进度变 2/2 · 100%——**无需手动刷新**。这验证了 notify→emit→listen 全链路。

- [ ] **Step 5: 清理 + Commit（如有 dev 配置微调）**

```bash
rm -f dev-board.db*
git add -A && git commit -m "chore(app): 端到端验证通过" --allow-empty
```

---

## 自检备忘（已核对）

- **Spec 覆盖**：总览（design §2）→ Task A1 + C2 Overview；项目看板三列（§2）→ A2 + C2 ProjectBoard；实时刷新（§3「notify 文件监听」）→ B3 + C2 listen；读共享 SQLite（§3 方案 A）→ B2 同一 `db_path`；进度=完成 todo/总 todo（§2 进度卡）→ C1 `todoProgress`。
- **类型一致**：Rust DTO `ProjectOverview { project, active_sessions, todo_count, doing_count, done_count, last_activity_at }` 与 TS 同名同字段；`TaskCard { task, todos, session_status }` 一致；命令名 `get_overview`/`get_project_tasks` 在 B2 定义、C1 调用一致；参数 `project_id`(Rust)↔`projectId`(JS) 由 Tauri 转换。
- **桌面贴纸 / 拖拽改列**：不在本计划，属计划 3。
- **YAGNI**：本计划只读，不做拖拽、不做贴纸、不做手机端。

---

## 实现调整记录（2026-06-03，验收反馈后）

计划执行中根据真实使用反馈做了如下调整（均已实现并验证）：

1. **视图收敛为单一「当前活跃」**：去掉了顶部导航、项目总览、看板钻入的路由。`Overview.tsx` / `ProjectBoard.tsx` 与 `overview()` / `project_tasks()` / `get_overview` / `get_project_tasks` 代码**保留备用**，只是前端不再路由进去。App 即「当前活跃」会话列表。
2. **新增活跃区查询 `live_sessions()`**：返回 running/waiting/stale 会话 + 项目名 + 任务标题 + 当前动作 + 进度 + todo 列表（`LiveSession` DTO）。
3. **卡片密度切换（极简 / 进度卡 / 信息丰富）**：localStorage 持久化。三档即使无 todo 也明显区分——极简只有标题；进度卡加状态/当前动作/进度条（无 todo 显示「暂无子任务」）；信息丰富再加 todo 勾选清单 + 最近活跃时间。
4. **隐藏未命名空占位卡**：`project_tasks()` 与 `overview()` 计数过滤 `title='(未命名会话)' 且无 todo` 的噪音卡。
5. **stale 巡检线程**：Tauri app 每 60s 调 `mark_stale`，把 10 分钟无事件的 running 会话标为 stale，让活跃状态诚实（终端被强杀收不到 SessionEnd 的场景）。

**端到端验证**：真实 reporter 二进制 → 临时 SQLite → notify → 界面，进度条从 50% 自动跳 100%（约 300ms，无需手动刷新），实时闭环通过。

**留待计划 3**：桌面贴纸（透明置顶悬浮窗）、看板拖拽改列、（如需）重新启用总览/看板视图入口。

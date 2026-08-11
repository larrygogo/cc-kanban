# Meowo 计划 1：数据管线 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让任意 Claude Code 会话通过 hooks 自动把进度事件写入共享 SQLite，供后续看板读取——交付后可直接查库验证数据正确落地。

**Architecture:** 全 Rust workspace。`meowo-store` 封装 SQLite（schema/迁移/查询，WAL 并发写）；`meowo-reporter` 是被 hooks 调用的 CLI，从 stdin 读取 Claude Code 的 hook JSON，按事件类型把状态 upsert 进库。出错绝不阻塞 Claude Code（静默退出码 0）。所有写库方法接收显式 `now_ms` 参数以便确定性测试。

**Tech Stack:** Rust（`rusqlite` bundled、`serde`/`serde_json`、`thiserror`、`clap`），`cargo test`。

---

## 文件结构

```
meowo/
  Cargo.toml                      # workspace 根
  crates/
    meowo-store/
      Cargo.toml
      src/lib.rs                  # 导出 Store / 模型 / 错误
      src/error.rs                # StoreError (thiserror)
      src/models.rs               # Project/Session/Task/Todo 结构与枚举
      src/store.rs                # Store：open + 所有读写方法
      src/migrations.rs           # 建表 SQL
      tests/store_test.rs         # 集成测试
    meowo-reporter/
      Cargo.toml
      src/main.rs                 # 入口：读 stdin、分发、永不 panic
      src/hook.rs                 # HookEvent 解析（serde）
      src/dispatch.rs             # HookEvent -> Store 调用
      tests/dispatch_test.rs
  scripts/
    install-hooks.mjs             # 把 hook 配置写进 ~/.claude/settings.json（bun 运行）
```

每个文件单一职责：`meowo-store` 只管持久化，`meowo-reporter` 只管「解析 hook → 调 store」，hook 安装独立成脚本。

---

## Task 1: 初始化 workspace 与 meowo-store 骨架

**Files:**
- Create: `Cargo.toml`（workspace 根）
- Create: `crates/meowo-store/Cargo.toml`
- Create: `crates/meowo-store/src/lib.rs`

- [ ] **Step 1: 写 workspace 根 Cargo.toml**

```toml
# Cargo.toml
[workspace]
members = ["crates/meowo-store", "crates/meowo-reporter"]
resolver = "2"

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "1"
rusqlite = { version = "0.31", features = ["bundled"] }
```

- [ ] **Step 2: 写 meowo-store 的 Cargo.toml**

```toml
# crates/meowo-store/Cargo.toml
[package]
name = "meowo-store"
version = "0.1.0"
edition = "2021"

[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
rusqlite = { workspace = true }
```

- [ ] **Step 3: 写最小 lib.rs 占位**

```rust
// crates/meowo-store/src/lib.rs
pub mod error;
pub mod migrations;
pub mod models;
pub mod store;

pub use error::StoreError;
pub use models::*;
pub use store::Store;
```

- [ ] **Step 4: 建空模块文件让其能编译**

创建以下四个文件，先放占位内容：

```rust
// crates/meowo-store/src/error.rs
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}
```

```rust
// crates/meowo-store/src/migrations.rs
pub const SCHEMA: &str = "";
```

```rust
// crates/meowo-store/src/models.rs
```

```rust
// crates/meowo-store/src/store.rs
pub struct Store;
```

- [ ] **Step 5: 验证编译**

Run: `cargo build -p meowo-store`
Expected: 编译通过（可能有 unused 警告，可忽略）。

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/meowo-store
git commit -m "chore: 初始化 workspace 与 meowo-store 骨架"
```

---

## Task 2: 定义模型与枚举

**Files:**
- Modify: `crates/meowo-store/src/models.rs`

- [ ] **Step 1: 写模型与枚举**

```rust
// crates/meowo-store/src/models.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Running,
    Waiting,
    Ended,
    Stale,
}

impl SessionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            SessionStatus::Running => "running",
            SessionStatus::Waiting => "waiting",
            SessionStatus::Ended => "ended",
            SessionStatus::Stale => "stale",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskColumn {
    Todo,
    Doing,
    Done,
}

impl TaskColumn {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskColumn::Todo => "todo",
            TaskColumn::Doing => "doing",
            TaskColumn::Done => "done",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

impl TodoStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            TodoStatus::Pending => "pending",
            TodoStatus::InProgress => "in_progress",
            TodoStatus::Completed => "completed",
        }
    }
    pub fn from_str(s: &str) -> TodoStatus {
        match s {
            "in_progress" => TodoStatus::InProgress,
            "completed" => TodoStatus::Completed,
            _ => TodoStatus::Pending,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub id: i64,
    pub root_path: String,
    pub name: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub id: i64,
    pub project_id: i64,
    pub cc_session_id: String,
    pub status: String,
    pub started_at: i64,
    pub last_event_at: i64,
    pub ended_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: i64,
    pub project_id: i64,
    pub session_id: Option<i64>,
    pub title: String,
    pub column: String,
    pub column_locked: bool,
    pub current_activity: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Todo {
    pub id: i64,
    pub task_id: i64,
    pub content: String,
    pub status: String,
    pub order_idx: i64,
}

/// 上报器同步 todo 时的输入项。
#[derive(Debug, Clone, PartialEq)]
pub struct TodoInput {
    pub content: String,
    pub status: TodoStatus,
}
```

- [ ] **Step 2: 验证编译**

Run: `cargo build -p meowo-store`
Expected: 编译通过。

- [ ] **Step 3: Commit**

```bash
git add crates/meowo-store/src/models.rs
git commit -m "feat(store): 定义项目/会话/任务/todo 模型与枚举"
```

---

## Task 3: 建表 SQL 与 Store::open（迁移 + WAL）

**Files:**
- Modify: `crates/meowo-store/src/migrations.rs`
- Modify: `crates/meowo-store/src/store.rs`
- Create: `crates/meowo-store/tests/store_test.rs`

- [ ] **Step 1: 写建表 SQL**

```rust
// crates/meowo-store/src/migrations.rs
pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS projects (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    root_path   TEXT NOT NULL UNIQUE,
    name        TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS sessions (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id    INTEGER NOT NULL REFERENCES projects(id),
    cc_session_id TEXT NOT NULL UNIQUE,
    status        TEXT NOT NULL,
    started_at    INTEGER NOT NULL,
    last_event_at INTEGER NOT NULL,
    ended_at      INTEGER
);

CREATE TABLE IF NOT EXISTS tasks (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id       INTEGER NOT NULL REFERENCES projects(id),
    session_id       INTEGER REFERENCES sessions(id),
    title            TEXT NOT NULL,
    column_name      TEXT NOT NULL,
    column_locked    INTEGER NOT NULL DEFAULT 0,
    current_activity TEXT,
    created_at       INTEGER NOT NULL,
    updated_at       INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS todos (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id   INTEGER NOT NULL REFERENCES tasks(id),
    content   TEXT NOT NULL,
    status    TEXT NOT NULL,
    order_idx INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS events (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id INTEGER REFERENCES sessions(id),
    kind       TEXT NOT NULL,
    payload    TEXT,
    created_at INTEGER NOT NULL
);
"#;
```

> 注：列名用 `column_name` 而非 `column`，因为 `column` 不是保留字但易混淆；模型里仍叫 `column`，在 SQL 映射时对应 `column_name`。

- [ ] **Step 2: 写 Store::open**

```rust
// crates/meowo-store/src/store.rs
use crate::error::StoreError;
use crate::migrations::SCHEMA;
use rusqlite::Connection;
use std::path::Path;

pub struct Store {
    pub(crate) conn: Connection,
}

impl Store {
    /// 打开（或新建）数据库，开启 WAL，执行建表。
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Store, StoreError> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Store { conn })
    }

    /// 仅用于测试：内存库。
    pub fn open_in_memory() -> Result<Store, StoreError> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Store { conn })
    }
}
```

- [ ] **Step 3: 写失败测试——open 后表存在**

```rust
// crates/meowo-store/tests/store_test.rs
use meowo_store::Store;

#[test]
fn open_in_memory_creates_tables() {
    let store = Store::open_in_memory().expect("open");
    // 通过查询 sqlite_master 确认 5 张表都建了
    let count: i64 = store
        .raw_table_count()
        .expect("count tables");
    assert_eq!(count, 5);
}
```

- [ ] **Step 4: 运行测试确认失败**

Run: `cargo test -p meowo-store open_in_memory_creates_tables`
Expected: FAIL，编译错误「no method named `raw_table_count`」。

- [ ] **Step 5: 加 raw_table_count 辅助方法**

```rust
// 追加到 crates/meowo-store/src/store.rs 的 impl Store 内
impl Store {
    /// 测试辅助：统计用户表数量。
    pub fn raw_table_count(&self) -> Result<i64, StoreError> {
        let n: i64 = self.conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            [],
            |r| r.get(0),
        )?;
        Ok(n)
    }
}
```

- [ ] **Step 6: 运行测试确认通过**

Run: `cargo test -p meowo-store open_in_memory_creates_tables`
Expected: PASS。

- [ ] **Step 7: Commit**

```bash
git add crates/meowo-store
git commit -m "feat(store): 建表 SQL 与 Store::open（WAL + 迁移）"
```

---

## Task 4: upsert_project_by_root

**Files:**
- Modify: `crates/meowo-store/src/store.rs`
- Modify: `crates/meowo-store/tests/store_test.rs`

- [ ] **Step 1: 写失败测试**

```rust
// 追加到 tests/store_test.rs
use meowo_store::Project;

#[test]
fn upsert_project_is_idempotent_by_root() {
    let store = Store::open_in_memory().unwrap();
    let id1 = store.upsert_project_by_root("/home/me/proj", "proj", 1000).unwrap();
    // 同 root 第二次：返回同一 id，不新建
    let id2 = store.upsert_project_by_root("/home/me/proj", "proj", 2000).unwrap();
    assert_eq!(id1, id2);

    let projects: Vec<Project> = store.list_projects().unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].name, "proj");
    assert_eq!(projects[0].updated_at, 2000); // 第二次更新了 updated_at
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p meowo-store upsert_project_is_idempotent_by_root`
Expected: FAIL，「no method named `upsert_project_by_root`」。

- [ ] **Step 3: 实现方法**

```rust
// 追加到 impl Store
use crate::models::Project;

impl Store {
    /// 按 root_path upsert 项目，返回 project id。已存在则更新 updated_at。
    pub fn upsert_project_by_root(
        &self,
        root_path: &str,
        name: &str,
        now_ms: i64,
    ) -> Result<i64, StoreError> {
        self.conn.execute(
            "INSERT INTO projects (root_path, name, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?3)
             ON CONFLICT(root_path) DO UPDATE SET updated_at = ?3",
            rusqlite::params![root_path, name, now_ms],
        )?;
        let id: i64 = self.conn.query_row(
            "SELECT id FROM projects WHERE root_path = ?1",
            [root_path],
            |r| r.get(0),
        )?;
        Ok(id)
    }

    pub fn list_projects(&self) -> Result<Vec<Project>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, root_path, name, created_at, updated_at FROM projects ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Project {
                id: r.get(0)?,
                root_path: r.get(1)?,
                name: r.get(2)?,
                created_at: r.get(3)?,
                updated_at: r.get(4)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }
}
```

- [ ] **Step 4: 运行确认通过**

Run: `cargo test -p meowo-store upsert_project_is_idempotent_by_root`
Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add crates/meowo-store
git commit -m "feat(store): upsert_project_by_root + list_projects"
```

---

## Task 5: start_session（建会话 + 同时建占位任务）

**Files:**
- Modify: `crates/meowo-store/src/store.rs`
- Modify: `crates/meowo-store/tests/store_test.rs`

- [ ] **Step 1: 写失败测试**

```rust
// 追加到 tests/store_test.rs
use meowo_store::Task;

#[test]
fn start_session_creates_session_and_placeholder_task() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, tid) = store.start_session(pid, "cc-abc", 200).unwrap();
    assert!(sid > 0 && tid > 0);

    // 同 cc_session_id 再次调用应幂等返回同一对 id
    let (sid2, tid2) = store.start_session(pid, "cc-abc", 300).unwrap();
    assert_eq!(sid, sid2);
    assert_eq!(tid, tid2);

    let task: Task = store.get_task(tid).unwrap();
    assert_eq!(task.title, "(未命名会话)");
    assert_eq!(task.column, "todo");
    assert_eq!(task.session_id, Some(sid));
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p meowo-store start_session_creates_session_and_placeholder_task`
Expected: FAIL，缺 `start_session` / `get_task`。

- [ ] **Step 3: 实现 start_session 与 get_task**

```rust
// 追加到 impl Store
use crate::models::Task;

impl Store {
    /// 开始一个会话；若 cc_session_id 已存在则幂等返回既有 (session_id, task_id)。
    /// 新会话会同时建一张占位任务卡。
    pub fn start_session(
        &self,
        project_id: i64,
        cc_session_id: &str,
        now_ms: i64,
    ) -> Result<(i64, i64), StoreError> {
        if let Some(sid) = self.find_session_id(cc_session_id)? {
            let tid = self.task_id_of_session(sid)?;
            return Ok((sid, tid));
        }
        self.conn.execute(
            "INSERT INTO sessions (project_id, cc_session_id, status, started_at, last_event_at)
             VALUES (?1, ?2, 'running', ?3, ?3)",
            rusqlite::params![project_id, cc_session_id, now_ms],
        )?;
        let sid = self.conn.last_insert_rowid();
        self.conn.execute(
            "INSERT INTO tasks (project_id, session_id, title, column_name, column_locked, created_at, updated_at)
             VALUES (?1, ?2, '(未命名会话)', 'todo', 0, ?3, ?3)",
            rusqlite::params![project_id, sid, now_ms],
        )?;
        let tid = self.conn.last_insert_rowid();
        Ok((sid, tid))
    }

    pub(crate) fn find_session_id(&self, cc_session_id: &str) -> Result<Option<i64>, StoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM sessions WHERE cc_session_id = ?1")?;
        let mut rows = stmt.query([cc_session_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    pub(crate) fn task_id_of_session(&self, session_id: i64) -> Result<i64, StoreError> {
        let id: i64 = self.conn.query_row(
            "SELECT id FROM tasks WHERE session_id = ?1 ORDER BY id LIMIT 1",
            [session_id],
            |r| r.get(0),
        )?;
        Ok(id)
    }

    pub fn get_task(&self, task_id: i64) -> Result<Task, StoreError> {
        let task = self.conn.query_row(
            "SELECT id, project_id, session_id, title, column_name, column_locked, current_activity, created_at, updated_at
             FROM tasks WHERE id = ?1",
            [task_id],
            |r| {
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
            },
        )?;
        Ok(task)
    }
}
```

- [ ] **Step 4: 运行确认通过**

Run: `cargo test -p meowo-store start_session_creates_session_and_placeholder_task`
Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add crates/meowo-store
git commit -m "feat(store): start_session 建会话+占位任务（幂等）"
```

---

## Task 6: set_task_title_from_prompt 与 set_current_activity

**Files:**
- Modify: `crates/meowo-store/src/store.rs`
- Modify: `crates/meowo-store/tests/store_test.rs`

- [ ] **Step 1: 写失败测试**

```rust
// 追加到 tests/store_test.rs
#[test]
fn first_prompt_sets_title_then_later_prompts_only_update_activity() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, tid) = store.start_session(pid, "cc-1", 200).unwrap();

    // 首条 prompt：替换占位标题 + 设当前动作
    store.on_user_prompt(sid, "实现登录功能并写测试", 300).unwrap();
    let t = store.get_task(tid).unwrap();
    assert_eq!(t.title, "实现登录功能并写测试");
    assert_eq!(t.current_activity.as_deref(), Some("实现登录功能并写测试"));

    // 第二条 prompt：标题不再变，只更新当前动作
    store.on_user_prompt(sid, "再加个登出按钮", 400).unwrap();
    let t2 = store.get_task(tid).unwrap();
    assert_eq!(t2.title, "实现登录功能并写测试");
    assert_eq!(t2.current_activity.as_deref(), Some("再加个登出按钮"));
}

#[test]
fn long_prompt_title_is_truncated_to_60_chars() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, tid) = store.start_session(pid, "cc-2", 200).unwrap();
    let long = "字".repeat(80);
    store.on_user_prompt(sid, &long, 300).unwrap();
    let t = store.get_task(tid).unwrap();
    assert_eq!(t.title.chars().count(), 60);
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p meowo-store on_user_prompt`
Expected: FAIL，缺 `on_user_prompt`。

- [ ] **Step 3: 实现**

```rust
// 追加到 impl Store
impl Store {
    /// 收到用户 prompt：占位标题则替换为截断后的 prompt；当前动作总是更新为该 prompt。
    pub fn on_user_prompt(
        &self,
        session_id: i64,
        prompt: &str,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        let tid = self.task_id_of_session(session_id)?;
        let title: String = self.conn.query_row(
            "SELECT title FROM tasks WHERE id = ?1",
            [tid],
            |r| r.get(0),
        )?;
        let activity = truncate_chars(prompt.trim(), 60);
        if title == "(未命名会话)" {
            let new_title = truncate_chars(prompt.trim(), 60);
            self.conn.execute(
                "UPDATE tasks SET title = ?1, current_activity = ?2, updated_at = ?3 WHERE id = ?4",
                rusqlite::params![new_title, activity, now_ms, tid],
            )?;
        } else {
            self.conn.execute(
                "UPDATE tasks SET current_activity = ?1, updated_at = ?2 WHERE id = ?3",
                rusqlite::params![activity, now_ms, tid],
            )?;
        }
        self.touch_session(session_id, now_ms)?;
        Ok(())
    }

    /// 更新会话 last_event_at；若处于 waiting/stale 恢复为 running。
    pub fn touch_session(&self, session_id: i64, now_ms: i64) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE sessions
             SET last_event_at = ?1,
                 status = CASE WHEN status IN ('waiting','stale') THEN 'running' ELSE status END
             WHERE id = ?2",
            rusqlite::params![now_ms, session_id],
        )?;
        Ok(())
    }
}

/// 按字符（非字节）截断，避免切坏多字节中文。
fn truncate_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}
```

- [ ] **Step 4: 运行确认通过**

Run: `cargo test -p meowo-store`
Expected: 全部 PASS（含两条新测试）。

- [ ] **Step 5: Commit**

```bash
git add crates/meowo-store
git commit -m "feat(store): on_user_prompt 设标题/当前动作 + touch_session"
```

---

## Task 7: sync_todos（同步子清单 + 按 todo 推导列）

**Files:**
- Modify: `crates/meowo-store/src/store.rs`
- Modify: `crates/meowo-store/tests/store_test.rs`

- [ ] **Step 1: 写失败测试**

```rust
// 追加到 tests/store_test.rs
use meowo_store::{Todo, TodoInput, TodoStatus};

#[test]
fn sync_todos_replaces_list_and_derives_column() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, tid) = store.start_session(pid, "cc-1", 200).unwrap();

    // 有 in_progress -> 列变 doing
    store.sync_todos(sid, &[
        TodoInput { content: "解析".into(), status: TodoStatus::Completed },
        TodoInput { content: "建图".into(), status: TodoStatus::InProgress },
        TodoInput { content: "测试".into(), status: TodoStatus::Pending },
    ], 300).unwrap();

    let todos: Vec<Todo> = store.list_todos(tid).unwrap();
    assert_eq!(todos.len(), 3);
    assert_eq!(todos[0].content, "解析");
    assert_eq!(store.get_task(tid).unwrap().column, "doing");

    // 再同步：全部 completed -> 列变 done，且旧 todo 被替换不累积
    store.sync_todos(sid, &[
        TodoInput { content: "解析".into(), status: TodoStatus::Completed },
        TodoInput { content: "建图".into(), status: TodoStatus::Completed },
    ], 400).unwrap();
    assert_eq!(store.list_todos(tid).unwrap().len(), 2);
    assert_eq!(store.get_task(tid).unwrap().column, "done");
}

#[test]
fn sync_todos_does_not_override_locked_column() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, tid) = store.start_session(pid, "cc-1", 200).unwrap();
    store.set_task_column(tid, meowo_store::TaskColumn::Done, true, 250).unwrap(); // 手动锁定到 done

    store.sync_todos(sid, &[
        TodoInput { content: "x".into(), status: TodoStatus::InProgress },
    ], 300).unwrap();
    // 锁定后不被自动推导覆盖
    assert_eq!(store.get_task(tid).unwrap().column, "done");
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p meowo-store sync_todos`
Expected: FAIL，缺 `sync_todos` / `list_todos` / `set_task_column`。

- [ ] **Step 3: 实现**

```rust
// 追加到 impl Store
use crate::models::{TaskColumn, Todo, TodoInput, TodoStatus};

impl Store {
    /// 用新列表整体替换某会话任务的 todos；未锁定时按 todo 推导列。
    pub fn sync_todos(
        &self,
        session_id: i64,
        todos: &[TodoInput],
        now_ms: i64,
    ) -> Result<(), StoreError> {
        let tid = self.task_id_of_session(session_id)?;
        self.conn.execute("DELETE FROM todos WHERE task_id = ?1", [tid])?;
        for (i, t) in todos.iter().enumerate() {
            self.conn.execute(
                "INSERT INTO todos (task_id, content, status, order_idx) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![tid, t.content, t.status.as_str(), i as i64],
            )?;
        }

        let locked: bool = self.conn.query_row(
            "SELECT column_locked FROM tasks WHERE id = ?1",
            [tid],
            |r| Ok(r.get::<_, i64>(0)? != 0),
        )?;
        if !locked {
            let col = derive_column(todos);
            self.conn.execute(
                "UPDATE tasks SET column_name = ?1, updated_at = ?2 WHERE id = ?3",
                rusqlite::params![col.as_str(), now_ms, tid],
            )?;
        } else {
            self.conn.execute(
                "UPDATE tasks SET updated_at = ?1 WHERE id = ?2",
                rusqlite::params![now_ms, tid],
            )?;
        }
        self.touch_session(session_id, now_ms)?;
        Ok(())
    }

    pub fn list_todos(&self, task_id: i64) -> Result<Vec<Todo>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, task_id, content, status, order_idx FROM todos WHERE task_id = ?1 ORDER BY order_idx",
        )?;
        let rows = stmt.query_map([task_id], |r| {
            Ok(Todo {
                id: r.get(0)?,
                task_id: r.get(1)?,
                content: r.get(2)?,
                status: r.get(3)?,
                order_idx: r.get(4)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 设置任务列；locked=true 表示手动覆盖，之后自动推导不再生效。
    pub fn set_task_column(
        &self,
        task_id: i64,
        column: TaskColumn,
        locked: bool,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE tasks SET column_name = ?1, column_locked = ?2, updated_at = ?3 WHERE id = ?4",
            rusqlite::params![column.as_str(), locked as i64, now_ms, task_id],
        )?;
        Ok(())
    }
}

/// 无 todo -> todo；有 in_progress 或部分完成 -> doing；全 completed -> done。
fn derive_column(todos: &[TodoInput]) -> TaskColumn {
    if todos.is_empty() {
        return TaskColumn::Todo;
    }
    if todos.iter().all(|t| t.status == TodoStatus::Completed) {
        return TaskColumn::Done;
    }
    if todos.iter().any(|t| t.status != TodoStatus::Pending) {
        return TaskColumn::Doing;
    }
    TaskColumn::Todo
}
```

- [ ] **Step 4: 运行确认通过**

Run: `cargo test -p meowo-store`
Expected: 全部 PASS。

- [ ] **Step 5: Commit**

```bash
git add crates/meowo-store
git commit -m "feat(store): sync_todos 同步子清单 + 推导列（尊重锁定）"
```

---

## Task 8: 会话状态变更（waiting/ended）与 stale 标记

**Files:**
- Modify: `crates/meowo-store/src/store.rs`
- Modify: `crates/meowo-store/tests/store_test.rs`

- [ ] **Step 1: 写失败测试**

```rust
// 追加到 tests/store_test.rs
use meowo_store::{Session, SessionStatus};

#[test]
fn stop_sets_waiting_and_end_sets_ended() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, _tid) = store.start_session(pid, "cc-1", 200).unwrap();

    store.set_session_status(sid, SessionStatus::Waiting, 300).unwrap();
    assert_eq!(store.get_session(sid).unwrap().status, "waiting");

    store.end_session(sid, 400).unwrap();
    let s = store.get_session(sid).unwrap();
    assert_eq!(s.status, "ended");
    assert_eq!(s.ended_at, Some(400));
}

#[test]
fn mark_stale_flags_idle_running_sessions() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid_old, _) = store.start_session(pid, "old", 1000).unwrap();
    let (sid_new, _) = store.start_session(pid, "new", 1000).unwrap();
    store.touch_session(sid_new, 9000).unwrap(); // new 最近有事件

    // now=10000，阈值=2000ms：old(last=1000) 超时 -> stale；new(last=9000) 不变
    let n = store.mark_stale(2000, 10000).unwrap();
    assert_eq!(n, 1);
    assert_eq!(store.get_session(sid_old).unwrap().status, "stale");
    assert_eq!(store.get_session(sid_new).unwrap().status, "running");
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p meowo-store session`
Expected: FAIL，缺 `set_session_status` / `end_session` / `get_session` / `mark_stale`。

- [ ] **Step 3: 实现**

```rust
// 追加到 impl Store
use crate::models::{Session, SessionStatus};

impl Store {
    pub fn set_session_status(
        &self,
        session_id: i64,
        status: SessionStatus,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE sessions SET status = ?1, last_event_at = ?2 WHERE id = ?3",
            rusqlite::params![status.as_str(), now_ms, session_id],
        )?;
        Ok(())
    }

    pub fn end_session(&self, session_id: i64, now_ms: i64) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE sessions SET status = 'ended', ended_at = ?1, last_event_at = ?1 WHERE id = ?2",
            rusqlite::params![now_ms, session_id],
        )?;
        Ok(())
    }

    pub fn get_session(&self, session_id: i64) -> Result<Session, StoreError> {
        let s = self.conn.query_row(
            "SELECT id, project_id, cc_session_id, status, started_at, last_event_at, ended_at
             FROM sessions WHERE id = ?1",
            [session_id],
            |r| {
                Ok(Session {
                    id: r.get(0)?,
                    project_id: r.get(1)?,
                    cc_session_id: r.get(2)?,
                    status: r.get(3)?,
                    started_at: r.get(4)?,
                    last_event_at: r.get(5)?,
                    ended_at: r.get(6)?,
                })
            },
        )?;
        Ok(s)
    }

    /// 把 running 且 (now - last_event_at) > threshold_ms 的会话标记为 stale，返回受影响行数。
    pub fn mark_stale(&self, threshold_ms: i64, now_ms: i64) -> Result<usize, StoreError> {
        let n = self.conn.execute(
            "UPDATE sessions SET status = 'stale'
             WHERE status = 'running' AND (?1 - last_event_at) > ?2",
            rusqlite::params![now_ms, threshold_ms],
        )?;
        Ok(n)
    }
}
```

- [ ] **Step 4: 运行确认通过**

Run: `cargo test -p meowo-store`
Expected: 全部 PASS。

- [ ] **Step 5: Commit**

```bash
git add crates/meowo-store
git commit -m "feat(store): 会话状态变更与 stale 标记"
```

---

## Task 9: meowo-reporter 骨架与 HookEvent 解析

**Files:**
- Create: `crates/meowo-reporter/Cargo.toml`
- Create: `crates/meowo-reporter/src/main.rs`
- Create: `crates/meowo-reporter/src/hook.rs`
- Create: `crates/meowo-reporter/tests/dispatch_test.rs`

> **背景说明（务必先读）：** Claude Code 的 hook 通过 stdin 传入一个 JSON 对象，包含 `hook_event_name`、`session_id`、`cwd` 等字段；不同事件附带不同字段（如 `UserPromptSubmit` 带 `prompt`，`PostToolUse` 带 `tool_name`/`tool_input`）。本任务用「宽容解析」：未知字段忽略，缺字段降级。Task 13 会先用调试 hook 抓真实 payload 校验字段名，若与此处假设不符，在 `hook.rs` 调整 `#[serde(rename)]` 即可，下游 dispatch 不受影响。

- [ ] **Step 1: 写 meowo-reporter 的 Cargo.toml**

```toml
# crates/meowo-reporter/Cargo.toml
[package]
name = "meowo-reporter"
version = "0.1.0"
edition = "2021"

[dependencies]
meowo-store = { path = "../meowo-store" }
serde = { workspace = true }
serde_json = { workspace = true }
```

- [ ] **Step 2: 写 HookEvent 解析（失败测试先行）**

```rust
// crates/meowo-reporter/tests/dispatch_test.rs
use meowo_reporter::hook::HookEvent;

#[test]
fn parse_user_prompt_event() {
    let json = r#"{
        "hook_event_name": "UserPromptSubmit",
        "session_id": "abc",
        "cwd": "/home/me/proj",
        "prompt": "写个登录"
    }"#;
    let ev = HookEvent::parse(json).expect("parse");
    assert_eq!(ev.session_id, "abc");
    assert_eq!(ev.cwd.as_deref(), Some("/home/me/proj"));
    assert_eq!(ev.hook_event_name, "UserPromptSubmit");
    assert_eq!(ev.prompt.as_deref(), Some("写个登录"));
}

#[test]
fn parse_posttooluse_todowrite() {
    let json = r#"{
        "hook_event_name": "PostToolUse",
        "session_id": "abc",
        "cwd": "/p",
        "tool_name": "TodoWrite",
        "tool_input": { "todos": [
            {"content":"a","status":"completed"},
            {"content":"b","status":"in_progress"}
        ]}
    }"#;
    let ev = HookEvent::parse(json).expect("parse");
    assert_eq!(ev.tool_name.as_deref(), Some("TodoWrite"));
    let todos = ev.todo_items();
    assert_eq!(todos.len(), 2);
    assert_eq!(todos[1].content, "b");
}

#[test]
fn parse_tolerates_unknown_fields() {
    let json = r#"{"hook_event_name":"Stop","session_id":"z","extra":123}"#;
    let ev = HookEvent::parse(json).expect("parse");
    assert_eq!(ev.hook_event_name, "Stop");
}
```

- [ ] **Step 3: 运行确认失败**

Run: `cargo test -p meowo-reporter`
Expected: FAIL（crate/模块不存在）。

- [ ] **Step 4: 实现 hook.rs**

```rust
// crates/meowo-reporter/src/hook.rs
use meowo_store::{TodoInput, TodoStatus};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct HookEvent {
    pub hook_event_name: String,
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub tool_input: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct RawTodo {
    content: String,
    #[serde(default)]
    status: String,
}

impl HookEvent {
    pub fn parse(s: &str) -> Result<HookEvent, serde_json::Error> {
        serde_json::from_str(s)
    }

    /// 从 tool_input.todos 提取 TodoInput 列表（非 TodoWrite 或无 todos 时返回空）。
    pub fn todo_items(&self) -> Vec<TodoInput> {
        let Some(input) = &self.tool_input else { return Vec::new() };
        let Some(arr) = input.get("todos").and_then(|v| v.as_array()) else {
            return Vec::new();
        };
        arr.iter()
            .filter_map(|v| serde_json::from_value::<RawTodo>(v.clone()).ok())
            .map(|t| TodoInput {
                content: t.content,
                status: TodoStatus::from_str(&t.status),
            })
            .collect()
    }

    /// 取 Bash 工具的 command 字段（用于「当前动作」显示）。
    pub fn bash_command(&self) -> Option<String> {
        self.tool_input
            .as_ref()?
            .get("command")?
            .as_str()
            .map(|s| s.to_string())
    }
}
```

- [ ] **Step 5: 写 lib.rs 与最小 main.rs**

```rust
// crates/meowo-reporter/src/lib.rs
pub mod dispatch;
pub mod hook;
```

```rust
// crates/meowo-reporter/src/main.rs
fn main() {
    // 真实逻辑在 Task 11 接入；先占位保证可构建。
    std::process::exit(0);
}
```

> 注：为让测试能 `use meowo_reporter::...`，crate 需同时是 lib + bin。在 `crates/meowo-reporter/Cargo.toml` 末尾加：
> ```toml
> [lib]
> path = "src/lib.rs"
> [[bin]]
> name = "meowo-reporter"
> path = "src/main.rs"
> ```

- [ ] **Step 6: 建占位 dispatch.rs 让其可编译**

```rust
// crates/meowo-reporter/src/dispatch.rs
// 占位，Task 10 实现。
```

- [ ] **Step 7: 运行确认通过**

Run: `cargo test -p meowo-reporter`
Expected: 三条解析测试 PASS。

- [ ] **Step 8: Commit**

```bash
git add crates/meowo-reporter
git commit -m "feat(reporter): HookEvent 宽容解析（prompt/todos/bash）"
```

---

## Task 10: dispatch —— HookEvent 映射到 Store 调用

**Files:**
- Modify: `crates/meowo-reporter/src/dispatch.rs`
- Modify: `crates/meowo-reporter/tests/dispatch_test.rs`

- [ ] **Step 1: 写失败测试（端到端：事件 → 库状态）**

```rust
// 追加到 crates/meowo-reporter/tests/dispatch_test.rs
use meowo_reporter::dispatch::dispatch;
use meowo_store::Store;

fn ev(json: &str) -> HookEvent { HookEvent::parse(json).unwrap() }

#[test]
fn session_start_then_prompt_then_todos_flow() {
    let store = Store::open_in_memory().unwrap();

    dispatch(&store, &ev(r#"{"hook_event_name":"SessionStart","session_id":"s1","cwd":"/home/me/proj"}"#), 100).unwrap();
    // 项目按 cwd 建出来；占位任务存在
    let projects = store.list_projects().unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].name, "proj");

    dispatch(&store, &ev(r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt":"实现登录"}"#), 200).unwrap();
    dispatch(&store, &ev(r#"{"hook_event_name":"PostToolUse","session_id":"s1","tool_name":"TodoWrite","tool_input":{"todos":[{"content":"a","status":"in_progress"}]}}"#), 300).unwrap();

    // 取该会话的任务，标题已设、列为 doing、子清单 1 条
    let sid = store.find_session_id_pub("s1").unwrap().unwrap();
    let tid = store.task_id_of_session_pub(sid).unwrap();
    let t = store.get_task(tid).unwrap();
    assert_eq!(t.title, "实现登录");
    assert_eq!(t.column, "doing");
    assert_eq!(store.list_todos(tid).unwrap().len(), 1);
}

#[test]
fn stop_then_end_updates_session_status() {
    let store = Store::open_in_memory().unwrap();
    dispatch(&store, &ev(r#"{"hook_event_name":"SessionStart","session_id":"s2","cwd":"/p"}"#), 100).unwrap();
    dispatch(&store, &ev(r#"{"hook_event_name":"Stop","session_id":"s2"}"#), 200).unwrap();
    let sid = store.find_session_id_pub("s2").unwrap().unwrap();
    assert_eq!(store.get_session(sid).unwrap().status, "waiting");

    dispatch(&store, &ev(r#"{"hook_event_name":"SessionEnd","session_id":"s2"}"#), 300).unwrap();
    assert_eq!(store.get_session(sid).unwrap().status, "ended");
}

#[test]
fn unknown_session_for_prompt_is_ignored_gracefully() {
    let store = Store::open_in_memory().unwrap();
    // 没有 SessionStart 直接来 prompt：不应 panic，也不应报错
    let r = dispatch(&store, &ev(r#"{"hook_event_name":"UserPromptSubmit","session_id":"ghost","prompt":"x"}"#), 100);
    assert!(r.is_ok());
}
```

> 测试用到 `find_session_id_pub` / `task_id_of_session_pub`：在 `meowo-store` 暴露两个公开包装方法（包内 `find_session_id`/`task_id_of_session` 是 `pub(crate)`）。

- [ ] **Step 2: 在 meowo-store 加公开包装（供测试/上报器用）**

```rust
// 追加到 crates/meowo-store/src/store.rs 的 impl Store
impl Store {
    pub fn find_session_id_pub(&self, cc_session_id: &str) -> Result<Option<i64>, StoreError> {
        self.find_session_id(cc_session_id)
    }
    pub fn task_id_of_session_pub(&self, session_id: i64) -> Result<i64, StoreError> {
        self.task_id_of_session(session_id)
    }
}
```

- [ ] **Step 3: 运行确认失败**

Run: `cargo test -p meowo-reporter session_start_then_prompt_then_todos_flow`
Expected: FAIL，缺 `dispatch`。

- [ ] **Step 4: 实现 dispatch**

```rust
// crates/meowo-reporter/src/dispatch.rs
use meowo_store::{SessionStatus, Store, StoreError};
use crate::hook::HookEvent;
use std::path::Path;

/// 把一个 hook 事件落到库。未知/缺字段一律降级为「无操作」，绝不报错冒泡到会阻塞 CC 的层级。
pub fn dispatch(store: &Store, ev: &HookEvent, now_ms: i64) -> Result<(), StoreError> {
    match ev.hook_event_name.as_str() {
        "SessionStart" => {
            let Some(cwd) = ev.cwd.as_deref() else { return Ok(()) };
            if ev.session_id.is_empty() { return Ok(()); }
            let (root, name) = project_root_and_name(cwd);
            let pid = store.upsert_project_by_root(&root, &name, now_ms)?;
            store.start_session(pid, &ev.session_id, now_ms)?;
        }
        "UserPromptSubmit" => {
            if let Some(sid) = lookup_session(store, ev)? {
                if let Some(prompt) = ev.prompt.as_deref() {
                    store.on_user_prompt(sid, prompt, now_ms)?;
                }
            }
        }
        "PostToolUse" => {
            if let Some(sid) = lookup_session(store, ev)? {
                match ev.tool_name.as_deref() {
                    Some("TodoWrite") => {
                        store.sync_todos(sid, &ev.todo_items(), now_ms)?;
                    }
                    Some("Bash") => {
                        if let Some(cmd) = ev.bash_command() {
                            store.set_current_activity(sid, &format!("› {cmd}"), now_ms)?;
                        }
                    }
                    _ => { store.touch_session(sid, now_ms)?; }
                }
            }
        }
        "Stop" => {
            if let Some(sid) = lookup_session(store, ev)? {
                store.set_session_status(sid, SessionStatus::Waiting, now_ms)?;
            }
        }
        "SessionEnd" => {
            if let Some(sid) = lookup_session(store, ev)? {
                store.end_session(sid, now_ms)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn lookup_session(store: &Store, ev: &HookEvent) -> Result<Option<i64>, StoreError> {
    if ev.session_id.is_empty() {
        return Ok(None);
    }
    store.find_session_id_pub(&ev.session_id)
}

/// cwd 的 git 根（向上找 .git）作为项目 root；无 git 则用 cwd 本身。name = 末段目录名。
fn project_root_and_name(cwd: &str) -> (String, String) {
    let root = find_git_root(cwd).unwrap_or_else(|| cwd.to_string());
    let name = Path::new(&root)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&root)
        .to_string();
    (root, name)
}

fn find_git_root(start: &str) -> Option<String> {
    let mut dir = Path::new(start);
    loop {
        if dir.join(".git").exists() {
            return Some(dir.to_string_lossy().to_string());
        }
        dir = dir.parent()?;
    }
}
```

- [ ] **Step 5: 在 meowo-store 补 set_current_activity**

```rust
// 追加到 crates/meowo-store/src/store.rs 的 impl Store
impl Store {
    pub fn set_current_activity(
        &self,
        session_id: i64,
        activity: &str,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        let tid = self.task_id_of_session(session_id)?;
        self.conn.execute(
            "UPDATE tasks SET current_activity = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![activity, now_ms, tid],
        )?;
        self.touch_session(session_id, now_ms)?;
        Ok(())
    }
}
```

- [ ] **Step 6: 运行确认通过**

Run: `cargo test`
Expected: meowo-store 与 meowo-reporter 全部 PASS。

- [ ] **Step 7: Commit**

```bash
git add crates
git commit -m "feat(reporter): dispatch 把 hook 事件映射到 store"
```

---

## Task 11: main.rs —— 读 stdin、定位库、永不阻塞 CC

**Files:**
- Modify: `crates/meowo-reporter/src/main.rs`
- Modify: `crates/meowo-reporter/src/lib.rs`

- [ ] **Step 1: 实现 db 路径解析（纯函数 + 测试）**

在 `crates/meowo-reporter/src/lib.rs` 追加：

```rust
// crates/meowo-reporter/src/lib.rs
pub mod dispatch;
pub mod hook;

use std::path::PathBuf;

/// 库路径：环境变量 MEOWO_DB 优先，否则 ~/.meowo/board.db。
pub fn db_path() -> PathBuf {
    if let Ok(p) = std::env::var("MEOWO_DB") {
        return PathBuf::from(p);
    }
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".meowo").join("board.db")
}
```

- [ ] **Step 2: 写 main.rs**

```rust
// crates/meowo-reporter/src/main.rs
use meowo_reporter::{db_path, dispatch::dispatch, hook::HookEvent};
use meowo_store::Store;
use std::io::Read;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    // 任何错误都吞掉并以 0 退出——绝不阻塞 Claude Code。
    let _ = run();
    std::process::exit(0);
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf)?;
    let ev = HookEvent::parse(&buf)?;
    let store = Store::open(db_path())?;
    let now = now_ms();
    dispatch(&store, &ev, now)?;
    Ok(())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
```

- [ ] **Step 3: 验证整体构建**

Run: `cargo build --release`
Expected: 生成 `target/release/meowo-reporter`（Windows 为 `meowo-reporter.exe`）。

- [ ] **Step 4: 手工冒烟测试（模拟一次 hook 调用）**

Run（PowerShell）：
```powershell
$env:MEOWO_DB="$PWD\smoke.db"
'{"hook_event_name":"SessionStart","session_id":"smoke","cwd":"' + ($PWD -replace '\\','/') + '"}' | .\target\release\meowo-reporter.exe
echo "exit=$LASTEXITCODE"
```
Expected: `exit=0`，且当前目录生成 `smoke.db`。

- [ ] **Step 5: 验证库内容**

Run:
```powershell
'{"hook_event_name":"UserPromptSubmit","session_id":"smoke","prompt":"冒烟测试 prompt"}' | .\target\release\meowo-reporter.exe
```
然后用任意 sqlite 工具或下一行 Rust 一次性脚本确认 tasks 表里 title = "冒烟测试 prompt"。最简单：临时加一个 `cargo test -p meowo-store` 已覆盖逻辑，这里只需确认 exit=0 且 db 文件增大。

- [ ] **Step 6: 清理并 Commit**

```bash
rm -f smoke.db smoke.db-wal smoke.db-shm
git add crates/meowo-reporter
git commit -m "feat(reporter): main 读 stdin 落库，错误静默退出 0"
```

---

## Task 12: hook 安装脚本（写入 ~/.claude/settings.json）

**Files:**
- Create: `scripts/install-hooks.mjs`

- [ ] **Step 1: 写安装脚本**

```javascript
// scripts/install-hooks.mjs
// 用法：bun scripts/install-hooks.mjs /abs/path/to/meowo-reporter(.exe)
// 把 Meowo 的 hooks 合并进 ~/.claude/settings.json，不破坏已有配置。
import { readFileSync, writeFileSync, existsSync, mkdirSync } from "node:fs";
import { homedir } from "node:os";
import { join, dirname } from "node:path";

const reporter = process.argv[2];
if (!reporter) {
  console.error("用法: bun scripts/install-hooks.mjs <meowo-reporter 可执行文件绝对路径>");
  process.exit(1);
}

const settingsPath = join(homedir(), ".claude", "settings.json");
mkdirSync(dirname(settingsPath), { recursive: true });

let settings = {};
if (existsSync(settingsPath)) {
  settings = JSON.parse(readFileSync(settingsPath, "utf8"));
}
settings.hooks ??= {};

// 我们要挂的事件。command 统一调用同一个 reporter，事件类型由 stdin 的 hook_event_name 区分。
const EVENTS = ["SessionStart", "UserPromptSubmit", "PostToolUse", "Stop", "SessionEnd"];
const TAG = "Meowo"; // 用于幂等：识别并替换我们自己加的条目

for (const event of EVENTS) {
  settings.hooks[event] ??= [];
  // 移除旧的 Meowo 条目（幂等重装）
  settings.hooks[event] = settings.hooks[event].filter(
    (entry) => !(entry.hooks ?? []).some((h) => h.command?.includes(TAG) || h._tag === TAG),
  );
  settings.hooks[event].push({
    matcher: "*",
    hooks: [{ type: "command", command: `"${reporter}"`, _tag: TAG }],
  });
}

writeFileSync(settingsPath, JSON.stringify(settings, null, 2));
console.log(`已写入 ${settingsPath}，挂载事件: ${EVENTS.join(", ")}`);
```

> 说明：`matcher: "*"` 表示对所有工具/来源生效。`PostToolUse` 我们在 reporter 内部只对 TodoWrite/Bash 做事，其余 touch 一下；如担心 PostToolUse 触发过频，可在 Task 13 校验后把 matcher 收窄为 `"TodoWrite|Bash"`。

- [ ] **Step 2: 试运行（dry check）**

Run:
```powershell
bun scripts/install-hooks.mjs "$PWD\target\release\meowo-reporter.exe"
```
Expected: 打印「已写入 …settings.json」。打开 `~/.claude/settings.json` 确认 5 个事件下各有一条 `_tag: "Meowo"` 的 command。

- [ ] **Step 3: 验证幂等（重复运行不重复挂载）**

再运行一次同样命令，确认每个事件下仍只有一条 Meowo 条目（不累积）。

- [ ] **Step 4: Commit**

```bash
git add scripts/install-hooks.mjs
git commit -m "feat: hook 安装脚本（幂等合并进 settings.json）"
```

---

## Task 13: 真实 payload 校验 + 端到端验收

**Files:**
- 无新增（验证 + 必要时微调 `crates/meowo-reporter/src/hook.rs`）

> 目的：用真实的 Claude Code hook 触发，确认我们假设的字段名（`prompt` / `tool_input.todos[].status` / `cwd` / `session_id` / `hook_event_name`）与实际一致。

- [ ] **Step 1: 指向一个临时库，避免污染真实数据**

在你的 shell 配置或当前会话设 `MEOWO_DB` 指向临时文件（reporter 已支持该环境变量）。

- [ ] **Step 2: 在一个真实 git 项目里启动一个 Claude Code 会话**

随便发一句 prompt，让它用一下 TodoWrite（例如让它「列个 3 步计划」），再让它跑一条 Bash 命令。

- [ ] **Step 3: 查库确认**

用 sqlite 工具打开临时库，逐项确认：
- `projects` 有该 git 项目，name 是目录名。
- `sessions` 有一条 status=running/waiting。
- `tasks` 的 title = 你的首条 prompt（截断 ≤60 字），column 随 TodoWrite 变化。
- `todos` 同步出你的待办项。

- [ ] **Step 4: 若字段名不符则修正**

若某字段没填上（如 prompt 为空），说明实际 JSON 字段名不同。临时加一个 dump hook 抓真实 payload：
```powershell
# 临时：把某事件原样 dump 到文件查看真实结构
# 在 settings.json 里临时把某事件 command 换成： powershell -c "$input | Out-File -Append $env:USERPROFILE\hook-dump.txt"
```
对照 dump 出的真实键名，调整 `hook.rs` 里对应字段的 `#[serde(rename = "...")]`，重跑 `cargo test -p meowo-reporter` 确保解析测试仍过，再重新验收。

- [ ] **Step 5: 全量测试 + 标记交付**

Run: `cargo test`
Expected: 全绿。

至此计划 1 交付：任意 Claude Code 会话的进度已自动落进共享 SQLite，可被计划 2 的看板读取。

- [ ] **Step 6: Commit（如有 hook.rs 调整）**

```bash
git add crates/meowo-reporter/src/hook.rs
git commit -m "fix(reporter): 按真实 hook payload 校正字段名"
```

---

## 自检备忘（已核对）

- **Spec 覆盖**：事件→状态映射（spec §5）逐条对应 Task 5–8、10；列推导与锁定（§6）见 Task 7；stale（§6）见 Task 8；项目分组 git 根（§6）见 Task 10 `project_root_and_name`；不阻塞 CC（§7）见 Task 11；WAL/自动建库（§4.2）见 Task 3。
- **类型一致**：`sync_todos`/`on_user_prompt`/`set_current_activity`/`set_task_column` 等签名在定义任务与调用任务间一致；SQL 列 `column_name` 对应模型字段 `column` 已在 Task 3 注明。
- **贴纸/看板 UI**：不在本计划，属计划 2、3。
- **进度百分比**：由 `todos` 完成比例算，本计划已把数据备齐，展示在计划 2。

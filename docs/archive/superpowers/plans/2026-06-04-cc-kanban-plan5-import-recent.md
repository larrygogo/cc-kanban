# 模块 C：首次导入近期会话 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 首次启动 meowo-app 时，把 `~/.claude/projects` 下近 7 天、最多 30 条历史会话以 `ended` 状态导入看板，让用户初次试用即有内容；用标记文件保证只导一次，绝不覆盖真实会话。

**Architecture:** 三层：(1) meowo-store 新增幂等 `import_session` 写入方法；(2) meowo-reporter 新增 `import` 模块，扫描 transcript 目录、复用既有标题/项目命名逻辑，提供可注入目录的 `import_from_dir` 与从 HOME 解析的 `import_recent`；(3) meowo-app 启动后台线程调用 `import_recent`，写 `imported.json` 标记并发 `board-changed` 刷新。

**Tech Stack:** Rust（workspace：crates/meowo-store、crates/meowo-reporter、app/src-tauri），rusqlite + serde_json，测试用 cargo test + tempfile + filetime。

设计来源：`docs/superpowers/specs/2026-06-04-release-and-polish-design.md` 模块 C。

> **与设计文档的有意偏离（已确认更稳健）：**
> 1. `import_recent` 返回 `Result<usize, StoreError>` 而非新建 `ImportError`——扫描期的 IO 错误按文件粒度吞掉跳过，唯一会冒泡的就是 StoreError，无需额外错误类型与 thiserror 依赖。
> 2. 新增**可注入目录**的 `import_from_dir(projects_dir, ...)` 作为测试接缝，`import_recent` 仅是解析 `~/.claude/projects` 的薄包装。测试调用 `import_from_dir` 传 tempdir，避免改 `USERPROFILE`/`HOME` 环境变量（Rust 并行测试下共享进程环境不安全）。
> 3. `import_session` 返回 `bool`（true=新插入）以便上层计数。

---

### Task 1: meowo-store 新增 `import_session` 写入方法

**Files:**
- Modify: `crates/meowo-store/src/store.rs`（在 `impl Store` 内、`end_session` 之后新增方法）
- Test: `crates/meowo-store/tests/store_test.rs`（追加两个测试）

> 背景：`sessions.cc_session_id` 有 `UNIQUE` 约束（`migrations.rs`），`tasks(session_id)` 有唯一索引。`import_session` 用 `ON CONFLICT(cc_session_id) DO NOTHING` 保证绝不覆盖真实会话；任务卡用 `INSERT OR IGNORE`。`truncate_chars` 是 `store.rs` 内已有私有函数，可直接复用做标题截断。`started_at = ended_at = last_event_at = mtime`，状态固定 `ended`，列固定 `done`（历史已结束会话）。

- [ ] **Step 1: 追加失败测试**

在 `crates/meowo-store/tests/store_test.rs` 文件末尾追加：

```rust
#[test]
fn import_session_inserts_ended_and_skips_existing() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 1000).unwrap();

    // 首次导入：以 ended 写入，建 done 列任务卡
    let inserted = store
        .import_session("hist1", pid, "历史标题", Some("/p"), 5000)
        .unwrap();
    assert!(inserted);

    let sid = store.find_session_id_pub("hist1").unwrap().unwrap();
    let s = store.get_session(sid).unwrap();
    assert_eq!(s.status, "ended");
    assert_eq!(s.started_at, 5000);
    assert_eq!(s.last_event_at, 5000);
    assert_eq!(s.ended_at, Some(5000));
    assert_eq!(store.session_cwd(sid).unwrap(), Some("/p".to_string()));

    let tid = store.task_id_of_session_pub(sid).unwrap();
    let t = store.get_task(tid).unwrap();
    assert_eq!(t.title, "历史标题");
    assert_eq!(t.column, "done");

    // 再次导入同一 cc_session_id：跳过、不覆盖
    let again = store
        .import_session("hist1", pid, "改标题", Some("/p"), 9000)
        .unwrap();
    assert!(!again);
    let s2 = store.get_session(sid).unwrap();
    assert_eq!(s2.last_event_at, 5000); // 未被改
    let t2 = store.get_task(tid).unwrap();
    assert_eq!(t2.title, "历史标题"); // 未被改
}

#[test]
fn import_session_does_not_resurrect_real_session() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 1000).unwrap();
    // 预置真实 running 会话
    let (sid, _) = store.start_session(pid, "live1", 2000).unwrap();

    let inserted = store
        .import_session("live1", pid, "x", None, 8000)
        .unwrap();
    assert!(!inserted);
    assert_eq!(store.get_session(sid).unwrap().status, "running");
}
```

确认 `store_test.rs` 顶部已 `use meowo_store::Store;`（若用的是 `meowo_store::*` 亦可）。若缺失则补 `use meowo_store::Store;`。

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test -p meowo-store import_session`
Expected: FAIL —— `no method named import_session found for struct Store`（编译错误）。

- [ ] **Step 3: 实现 `import_session`**

在 `crates/meowo-store/src/store.rs` 的 `impl Store` 块内，紧接 `end_session`（约 355-361 行）之后插入：

```rust
    /// 导入一条历史会话：以 ended 状态写入，started_at=ended_at=last_event_at=mtime。
    /// 用 ON CONFLICT(cc_session_id) DO NOTHING 保证绝不覆盖已存在的真实会话。
    /// 返回 true 表示新插入；false 表示 cc_session_id 已存在被跳过。
    pub fn import_session(
        &self,
        cc_session_id: &str,
        project_id: i64,
        title: &str,
        cwd: Option<&str>,
        last_event_at: i64,
    ) -> Result<bool, StoreError> {
        let n = self.conn.execute(
            "INSERT INTO sessions
                (project_id, cc_session_id, status, started_at, last_event_at, ended_at, cwd)
             VALUES (?1, ?2, 'ended', ?3, ?3, ?3, ?4)
             ON CONFLICT(cc_session_id) DO NOTHING",
            rusqlite::params![project_id, cc_session_id, last_event_at, cwd],
        )?;
        if n == 0 {
            return Ok(false); // 已存在，绝不覆盖
        }
        let sid = self
            .find_session_id(cc_session_id)?
            .ok_or(StoreError::Sqlite(rusqlite::Error::QueryReturnedNoRows))?;
        let mut t = truncate_chars(title.trim(), 80);
        if t.is_empty() {
            t = "(未命名会话)".to_string();
        }
        // 历史已结束会话的任务卡固定放 done 列，不导入 todo。
        self.conn.execute(
            "INSERT OR IGNORE INTO tasks
                (project_id, session_id, title, column_name, column_locked, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'done', 0, ?4, ?4)",
            rusqlite::params![project_id, sid, t, last_event_at],
        )?;
        Ok(true)
    }
```

- [ ] **Step 4: 运行测试，确认通过**

Run: `cargo test -p meowo-store import_session`
Expected: PASS（2 个新测试 + 既有测试不受影响）。

- [ ] **Step 5: 提交**

```bash
git add crates/meowo-store/src/store.rs crates/meowo-store/tests/store_test.rs
git commit -m "feat(store): 新增 import_session 幂等导入历史会话(ended/done,不覆盖真实会话)"
```

---

### Task 2: meowo-reporter 新增 `import` 模块

**Files:**
- Modify: `crates/meowo-reporter/src/dispatch.rs:89`（`fn project_root_and_name` → `pub(crate) fn`）
- Create: `crates/meowo-reporter/src/import.rs`
- Modify: `crates/meowo-reporter/src/lib.rs`（新增 `pub mod import;`）
- Modify: `crates/meowo-reporter/Cargo.toml`（新增 `[dev-dependencies]`）
- Test: `crates/meowo-reporter/tests/import_test.rs`（新建）

> 背景：`project_root_and_name(cwd) -> (String, String)`（root, name）现为 `dispatch.rs` 私有；import 模块需复用它，故提升为 `pub(crate)`。标题解析复用 `meowo_store::title::title_from_transcript(path)`（已 pub）。cwd 需自行从 transcript 行解析（无现成函数）。集成测试 `tests/import_test.rs` 只能访问 pub API，故 `import_from_dir` 与 `ImportOpts` 必须 `pub`。

- [ ] **Step 1: 提升 `project_root_and_name` 可见性**

在 `crates/meowo-reporter/src/dispatch.rs` 第 89 行，将：

```rust
fn project_root_and_name(cwd: &str) -> (String, String) {
```

改为：

```rust
pub(crate) fn project_root_and_name(cwd: &str) -> (String, String) {
```

- [ ] **Step 2: 注册模块**

在 `crates/meowo-reporter/src/lib.rs` 顶部模块声明区（现有 `pub mod dispatch;` 等之后）追加一行：

```rust
pub mod import;
```

- [ ] **Step 3: 加测试用 dev 依赖**

在 `crates/meowo-reporter/Cargo.toml` 末尾（`[[bin]]` 块之后）追加：

```toml
[dev-dependencies]
tempfile = "3"
filetime = "0.2"
```

- [ ] **Step 4: 写失败测试**

新建 `crates/meowo-reporter/tests/import_test.rs`：

```rust
use meowo_reporter::import::{import_from_dir, ImportOpts};
use meowo_store::Store;
use filetime::{set_file_mtime, FileTime};
use std::fs;
use std::path::Path;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// 在 projects_dir/<dir>/<session>.jsonl 写一行可选 cwd + 可选 ai-title 的 transcript，并设置 mtime。
/// cwd 用 unix 风格路径避免反斜杠转义。
fn write_transcript(
    projects_dir: &Path,
    dir: &str,
    session: &str,
    cwd: Option<&str>,
    ai_title: Option<&str>,
    mtime_secs: i64,
) {
    let d = projects_dir.join(dir);
    fs::create_dir_all(&d).unwrap();
    let path = d.join(format!("{session}.jsonl"));
    let mut lines: Vec<String> = Vec::new();
    if let Some(c) = cwd {
        lines.push(format!(r#"{{"type":"user","cwd":"{c}"}}"#));
    }
    if let Some(t) = ai_title {
        lines.push(format!(r#"{{"type":"ai-title","aiTitle":"{t}"}}"#));
    }
    if lines.is_empty() {
        lines.push("{}".to_string());
    }
    fs::write(&path, lines.join("\n")).unwrap();
    set_file_mtime(&path, FileTime::from_unix_time(mtime_secs, 0)).unwrap();
}

#[test]
fn imports_only_recent_and_marks_ended() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path();
    let now_s = now_secs();
    let now_ms = now_s * 1000;
    // 近期（1 小时前），带 cwd + ai-title
    write_transcript(proj, "dirA", "recent", Some("/home/me/foo"), Some("我的标题"), now_s - 3600);
    // 过期（10 天前）
    write_transcript(proj, "dirB", "old", Some("/home/me/bar"), None, now_s - 10 * 24 * 3600);

    let store = Store::open_in_memory().unwrap();
    let n = import_from_dir(proj, &store, now_ms, ImportOpts::default()).unwrap();
    assert_eq!(n, 1);

    let sid = store.find_session_id_pub("recent").unwrap().unwrap();
    let s = store.get_session(sid).unwrap();
    assert_eq!(s.status, "ended");
    assert_eq!(store.session_cwd(sid).unwrap(), Some("/home/me/foo".to_string()));

    let tid = store.task_id_of_session_pub(sid).unwrap();
    let t = store.get_task(tid).unwrap();
    assert_eq!(t.title, "我的标题");
    assert_eq!(t.column, "done");

    let names: Vec<String> = store.list_projects().unwrap().into_iter().map(|p| p.name).collect();
    assert!(names.contains(&"foo".to_string()));

    // 过期的不导入
    assert!(store.find_session_id_pub("old").unwrap().is_none());
}

#[test]
fn respects_max_count_newest_first() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path();
    let now_s = now_secs();
    // 5 个近期，mtime 递减：s0 最新
    for i in 0..5 {
        write_transcript(proj, &format!("d{i}"), &format!("s{i}"), Some("/home/me/p"), None, now_s - (i as i64) * 60);
    }
    let store = Store::open_in_memory().unwrap();
    let opts = ImportOpts { within_ms: 7 * DAY_MS, max_count: 3 };
    let n = import_from_dir(proj, &store, now_s * 1000, opts).unwrap();
    assert_eq!(n, 3);
    for i in 0..3 {
        assert!(store.find_session_id_pub(&format!("s{i}")).unwrap().is_some(), "s{i} 应被导入");
    }
    for i in 3..5 {
        assert!(store.find_session_id_pub(&format!("s{i}")).unwrap().is_none(), "s{i} 不应被导入");
    }
}

#[test]
fn does_not_overwrite_existing_session() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path();
    let now_s = now_secs();
    write_transcript(proj, "dirX", "dup", Some("/home/me/x"), Some("导入标题"), now_s - 100);

    let store = Store::open_in_memory().unwrap();
    let p = store.upsert_project_by_root("/home/me/x", "x", now_s * 1000).unwrap();
    let (sid, _) = store.start_session(p, "dup", now_s * 1000).unwrap(); // running

    let n = import_from_dir(proj, &store, now_s * 1000, ImportOpts::default()).unwrap();
    assert_eq!(n, 0);
    assert_eq!(store.get_session(sid).unwrap().status, "running");
}

#[test]
fn fallback_project_name_without_cwd() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path();
    let now_s = now_secs();
    // 无 cwd 行；目录名形如编码后的 cwd
    write_transcript(proj, "C--Users-me-myproj", "nocwd", None, Some("标题X"), now_s - 50);

    let store = Store::open_in_memory().unwrap();
    let n = import_from_dir(proj, &store, now_s * 1000, ImportOpts::default()).unwrap();
    assert_eq!(n, 1);
    let names: Vec<String> = store.list_projects().unwrap().into_iter().map(|p| p.name).collect();
    assert!(names.contains(&"myproj".to_string()));
}
```

- [ ] **Step 5: 运行测试，确认失败**

Run: `cargo test -p meowo-reporter --test import_test`
Expected: FAIL —— `unresolved import meowo_reporter::import`（`import.rs` 尚未创建）。

- [ ] **Step 6: 实现 `import.rs`**

新建 `crates/meowo-reporter/src/import.rs`：

```rust
//! 首次启动时导入 ~/.claude/projects 下近期的历史会话（标记为 ended）。
//! 复用 meowo-store 的标题解析与本 crate 的项目命名逻辑。

use crate::dispatch::project_root_and_name;
use meowo_store::{Store, StoreError};
use std::path::Path;

/// 导入参数。
#[derive(Debug, Clone, Copy)]
pub struct ImportOpts {
    /// 仅导入 mtime 距 now 不超过该毫秒数的会话。
    pub within_ms: i64,
    /// 最多导入条数（按 mtime 倒序取最新）。
    pub max_count: usize,
}

impl Default for ImportOpts {
    fn default() -> Self {
        ImportOpts {
            within_ms: 7 * 24 * 60 * 60 * 1000, // 7 天
            max_count: 30,
        }
    }
}

/// 从 ~/.claude/projects 导入近期历史会话。返回新导入条数。
/// HOME 不可解析或目录不存在时返回 Ok(0)。
pub fn import_recent(store: &Store, now_ms: i64, opts: ImportOpts) -> Result<usize, StoreError> {
    let Some(dir) = claude_projects_dir() else {
        return Ok(0);
    };
    import_from_dir(&dir, store, now_ms, opts)
}

/// 从指定 projects 目录导入（测试可注入 tempdir）。
pub fn import_from_dir(
    projects_dir: &Path,
    store: &Store,
    now_ms: i64,
    opts: ImportOpts,
) -> Result<usize, StoreError> {
    // 收集 (mtime_ms, cc_session_id, transcript_path, 编码目录名)
    let mut found: Vec<(i64, String, std::path::PathBuf, String)> = Vec::new();
    let Ok(dirs) = std::fs::read_dir(projects_dir) else {
        return Ok(0);
    };
    for dir in dirs.flatten() {
        if !dir.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let dir_name = dir.file_name().to_string_lossy().to_string();
        let Ok(files) = std::fs::read_dir(dir.path()) else {
            continue;
        };
        for f in files.flatten() {
            let path = f.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let Some(mtime) = mtime_ms(&path) else {
                continue;
            };
            if now_ms - mtime > opts.within_ms {
                continue;
            }
            found.push((mtime, stem.to_string(), path.clone(), dir_name.clone()));
        }
    }
    // 最新优先，取上限。
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.truncate(opts.max_count);

    let mut imported = 0usize;
    for (mtime, cc_session_id, path, dir_name) in found {
        let title = path
            .to_str()
            .and_then(meowo_store::title::title_from_transcript)
            .unwrap_or_else(|| "(未命名会话)".to_string());
        let cwd = cwd_from_transcript(&path);
        let (root, name) = match cwd.as_deref() {
            Some(c) => project_root_and_name(c),
            None => fallback_project(&dir_name),
        };
        let project_id = store.upsert_project_by_root(&root, &name, mtime)?;
        if store.import_session(&cc_session_id, project_id, &title, cwd.as_deref(), mtime)? {
            imported += 1;
        }
    }
    Ok(imported)
}

fn claude_projects_dir() -> Option<std::path::PathBuf> {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()?;
    Some(Path::new(&home).join(".claude").join("projects"))
}

/// 文件 mtime 转 Unix 毫秒。
fn mtime_ms(path: &Path) -> Option<i64> {
    let mt = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(mt.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as i64)
}

/// 逐行找含顶层 "cwd" 字段的条目，取最后一个非空值。
fn cwd_from_transcript(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let mut cwd: Option<String> = None;
    for line in content.lines() {
        if !line.contains("\"cwd\"") {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if let Some(s) = v.get("cwd").and_then(|x| x.as_str()) {
            if !s.is_empty() {
                cwd = Some(s.to_string());
            }
        }
    }
    cwd
}

/// 无 cwd 兜底：root 用编码目录名本身，name 取其 '-' 分隔的末段非空片段。
fn fallback_project(dir_name: &str) -> (String, String) {
    let name = dir_name.rsplit('-').find(|s| !s.is_empty()).unwrap_or(dir_name);
    (dir_name.to_string(), name.to_string())
}
```

- [ ] **Step 7: 运行测试，确认通过**

Run: `cargo test -p meowo-reporter`
Expected: PASS（4 个 import 测试 + 既有 dispatch/transcript 测试全绿）。

- [ ] **Step 8: clippy 把关**

Run: `cargo clippy -p meowo-reporter -- -D warnings`
Expected: 无警告。（若 `found.sort_by` 被建议改 `sort_by_key`，按建议改为 `found.sort_by_key(|e| std::cmp::Reverse(e.0));`）

- [ ] **Step 9: 提交**

```bash
git add crates/meowo-reporter/src/import.rs crates/meowo-reporter/src/lib.rs crates/meowo-reporter/src/dispatch.rs crates/meowo-reporter/Cargo.toml crates/meowo-reporter/tests/import_test.rs
git commit -m "feat(reporter): 新增 import 模块,扫描并导入近期历史会话(可注入目录,4 测试)"
```

---

### Task 3: meowo-app 启动钩子触发导入

**Files:**
- Modify: `app/src-tauri/Cargo.toml`（dependencies 增加 meowo-reporter）
- Modify: `app/src-tauri/src/lib.rs`（新增 `spawn_first_import`，在 `setup` 闭包中调用）

> 背景：meowo-app 当前仅依赖 meowo-store，需新增对 meowo-reporter 的路径依赖。已有 `db_path()`、`now_ms()`、`open_store`/`Store`、`spawn_db_watcher`/`spawn_stale_sweeper` 后台线程范式，以及 `app.emit("board-changed", ())` 刷新事件，全部直接复用。此 hook 是薄胶水层，导入逻辑已在 Task 2 充分测试，故本任务以 `cargo check` + clippy 编译验证为主，运行行为人工验证。

- [ ] **Step 1: 加 meowo-reporter 依赖**

在 `app/src-tauri/Cargo.toml` 的 `[dependencies]` 中，`meowo-store` 那一行下方追加：

```toml
meowo-reporter = { path = "../../crates/meowo-reporter" }
```

- [ ] **Step 2: 实现 `spawn_first_import` 并接入 setup**

在 `app/src-tauri/src/lib.rs` 中，于 `spawn_stale_sweeper` 函数（约 287-299 行）之后新增：

```rust
/// 首次启动：~/.meowo/imported.json 不存在时，后台导入近 7 天历史会话并写标记文件。
/// 出错仅静默（下次启动重试），绝不阻塞窗口创建。
fn spawn_first_import(app: tauri::AppHandle, db_path: PathBuf) {
    std::thread::spawn(move || {
        let Some(dir) = db_path.parent().map(|p| p.to_path_buf()) else {
            return;
        };
        let marker = dir.join("imported.json");
        if marker.exists() {
            return; // 已导入过，跳过
        }
        let store = match Store::open(&db_path) {
            Ok(s) => s,
            Err(_) => return,
        };
        let now = now_ms();
        if let Ok(count) =
            meowo_reporter::import::import_recent(&store, now, meowo_reporter::import::ImportOpts::default())
        {
            let body = format!("{{\"imported\":{count},\"at\":{now}}}");
            let _ = std::fs::write(&marker, body);
            if count > 0 {
                let _ = app.emit("board-changed", ());
            }
        }
    });
}
```

然后在 `run()` 的 `.setup(...)` 闭包中（现有 `spawn_stale_sweeper(...)` 那一行之后）追加：

```rust
            spawn_first_import(app.handle().clone(), path.clone());
```

- [ ] **Step 3: 编译与静态检查**

Run: `cargo check -p meowo-app && cargo clippy -p meowo-app -- -D warnings`
Expected: 编译通过、无 clippy 警告。

- [ ] **Step 4: 全 workspace 测试回归**

Run: `cargo test --workspace`
Expected: 全绿（meowo-store + meowo-reporter 全部测试）。

- [ ] **Step 5: 提交**

```bash
git add app/src-tauri/Cargo.toml app/src-tauri/src/lib.rs
git commit -m "feat(app): 首次启动后台导入近期历史会话(imported.json 标记,完成后刷新)"
```

---

## 自检（Self-Review）

- **Spec 覆盖**：
  - 扫描 `~/.claude/projects/*/*.jsonl`、文件名即 cc_session_id → Task 2 `import_from_dir`。
  - mtime 过滤近 7 天 + 上限 30 + 倒序 → Task 2 `within_ms`/`max_count`/`sort_by`。
  - cwd 逐行解析取最后一个、读不到走兜底 → Task 2 `cwd_from_transcript` + `fallback_project`。
  - title 复用 `title_from_transcript`、读不到 `(未命名会话)` → Task 2。
  - project 有 cwd 用 `project_root_and_name`、无 cwd 用编码目录名末段 → Task 2。
  - `import_session` 以 ended/pid=NULL/started=ended=last=mtime 写入 + `ON CONFLICT DO NOTHING` + 建 task 不导入 todo → Task 1。
  - 启动检查 `imported.json` 标记、后台线程、出错仅日志、完成 emit 刷新 → Task 3。
  - 测试：近 7 天过滤 / 上限 / 不覆盖已存在 / title+cwd+project 正确 → Task 2 四个测试 + Task 1 两个测试。
- **占位扫描**：无 TBD/“适当处理”，每个代码步骤均含完整代码与确切路径行号。
- **类型一致性**：
  - `import_session(&str, i64, &str, Option<&str>, i64) -> Result<bool, StoreError>` 在 Task 1 定义、Task 2 调用一致。
  - `import_from_dir(&Path, &Store, i64, ImportOpts) -> Result<usize, StoreError>` 与 `import_recent` 同签名风格；Task 3 调 `import_recent`。
  - `ImportOpts { within_ms: i64, max_count: usize }` 字段在测试与实现一致；`Default` 提供 7天/30。
  - `Session.status`/`Task.column` 均为 `String`（已核对 models.rs），断言用 `"ended"`/`"done"`/`"running"` 字符串。
  - `project_root_and_name` 返回 `(root, name)`，import 中按此顺序解构。
- **可见性**：`project_root_and_name` 提升为 `pub(crate)`（import.rs 同 crate 可见）；`import` 模块 `pub`，`import_from_dir`/`ImportOpts` `pub`（集成测试可见）。

## 非目标

- 不导入历史会话的 todo 明细（仅标题）。
- 不做增量/重复导入（仅首次，靠 `imported.json` 标记）。
- 不解析 transcript 的消息内容/进度（只取标题、cwd、mtime）。

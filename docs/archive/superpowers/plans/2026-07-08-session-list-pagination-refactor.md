# 会话列表虚拟列表+分页重构 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把会话列表的 filter/搜索/排序/分页下沉后端，board-changed 刷新保留已加载窗口，消除「看不全 / 搜不全 / loadMore 卡死 / counts 口径不一致」。

**Architecture:** 后端 `live_sessions` 加 `search` 参数并按 filter 定排序/游标方向；前端分页改由 `reachedEnd` 驱动、board-changed 刷新重查 `max(PAGE_SIZE, 已加载数)` 窗口（节流）、搜索词从 Sticker 提升到 App 走后端。

**Tech Stack:** Rust + rusqlite（keyset 分页）；Tauri command；React + TypeScript；vitest；cargo test。

## Global Constraints

- 代码注释、commit message 用中文；代码本身英文。
- 搜索作用域 = 当前 tab 内（filter 条件 AND 搜索词）。
- 分页由 `reachedEnd` 驱动（`page.length < PAGE_SIZE → reachedEnd`）；counts 只作角标显示，不参与 loadMore 判定。
- 排序：waiting tab `last_event_at ASC, id ASC`（等最久优先）；其它 tab `DESC, DESC`；游标方向随排序翻转。
- board-changed refresh 重查 `W = max(PAGE_SIZE, items.length)` 窗口，前端节流 ~400ms。
- 提交只精确 `git add` 本任务文件，绝不 `-A/-u`（仓库有无关 untracked `docs/superpowers/specs/2026-07-03-chat-window-design.md` + 用户 WIP，均不得扫入）。
- PAGE_SIZE = 100（App.tsx 既有常量）。

---

## 文件结构

- `crates/meowo-store/src/query.rs`：`live_sessions` 加 search + 排序/游标按 filter；新增 `escape_like`。
- `crates/meowo-store/tests/query_test.rs`：14 处 `live_sessions(` 调用补 `None`；新增 search/排序单测。
- `app/src-tauri/src/lib.rs`：`get_live_sessions_page` 命令 + `live_sessions_blocking` 加 search；补 1954 内部调用。
- `app/src/api.ts`：`getLiveSessionsPage` 加 search。
- `app/src/App.tsx`：分页改 reachedEnd 驱动 + W 窗口 refresh + 节流（Task 3）；search 状态 + 传 props（Task 4）。
- `app/src/views/Sticker.tsx`：搜索框走 props、`shown` 去客户端搜索、counts.all 口径、去 waiting 客户端重排（Task 4）。
- 测试：`App.test.tsx` / `Sticker.test.tsx`。

---

## Task 1: 后端 `live_sessions` 加 search + 按 filter 排序/游标

**Files:**
- Modify: `crates/meowo-store/src/query.rs`（`live_sessions` 函数；新增 `escape_like`）
- Modify: `crates/meowo-store/tests/query_test.rs`（补 `None` + 新测试）

**Interfaces:**
- Produces: `live_sessions(filter: Option<&str>, search: Option<&str>, before_last_event_at: Option<i64>, before_id: Option<i64>, limit: usize) -> Result<Vec<LiveSession>, StoreError>`（供 Task 2 的 `live_sessions_blocking` 调用）。

- [ ] **Step 1: 补齐现有调用点（先让改签名后仍编译），写新测试**

在 `crates/meowo-store/tests/query_test.rs` 中，把所有 `live_sessions(` 调用补入 `None` search（在 filter 之后）。共 14 处（行号约 116/137/157/161/165/169/209/232/242/251/275/346/347/380），例如：
- `store.live_sessions(None, None, None, 1000)` → `store.live_sessions(None, None, None, None, 1000)`
- `store.live_sessions(Some("all"), None, None, 100)` → `store.live_sessions(Some("all"), None, None, None, 100)`
- `store.live_sessions(Some("all"), Some(last.session.last_event_at), Some(last.session.id), 100)` → `store.live_sessions(Some("all"), None, Some(last.session.last_event_at), Some(last.session.id), 100)`
- `store.live_sessions(Some("all"), Some(100), Some(b), 100)` → `store.live_sessions(Some("all"), None, Some(100), Some(b), 100)`
- `store.live_sessions(Some("waiting"), None, None, 100)` → `store.live_sessions(Some("waiting"), None, None, None, 100)`
（逐一处理，凡 `live_sessions(<filter>,` 后紧跟 cursor 的都在 filter 与 cursor 之间插 `None,`。）

在文件末尾追加 4 个新测试：

```rust
#[test]
fn live_sessions_search_scoped_to_tab() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 1).unwrap();
    // running: 标题含 "login"
    let (r, _) = store.start_session(pid, "r", 100).unwrap();
    store.on_user_prompt(r, "实现 login 登录", 110).unwrap();
    store.set_session_status(r, SessionStatus::Running, 110).unwrap();
    // waiting: 标题含 "login"
    let (w, _) = store.start_session(pid, "w", 200).unwrap();
    store.on_user_prompt(w, "login 待回复", 210).unwrap();
    store.set_session_status(w, SessionStatus::Waiting, 210).unwrap();
    // running: 标题不含 "login"
    let (r2, _) = store.start_session(pid, "r2", 300).unwrap();
    store.on_user_prompt(r2, "别的任务", 310).unwrap();
    store.set_session_status(r2, SessionStatus::Running, 310).unwrap();

    // 全部 tab 搜 login：命中 r + w，不含 r2
    let all = store.live_sessions(Some("all"), Some("login"), None, None, 100).unwrap();
    assert_eq!(all.len(), 2);
    // running tab 搜 login：只命中 r（w 是 waiting，被 filter 排除）
    let run = store.live_sessions(Some("running"), Some("login"), None, None, 100).unwrap();
    assert_eq!(run.len(), 1);
    assert_eq!(run[0].session.cc_session_id, "r");
    // waiting tab 搜 login：只命中 w
    let wait = store.live_sessions(Some("waiting"), Some("login"), None, None, 100).unwrap();
    assert_eq!(wait.len(), 1);
    assert_eq!(wait[0].session.cc_session_id, "w");
}

#[test]
fn live_sessions_search_matches_cwd_and_escapes_wildcards() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 1).unwrap();
    let (a, _) = store.start_session(pid, "a", 100).unwrap();
    store.on_user_prompt(a, "无关标题", 110).unwrap();
    store.set_session_cwd(a, "C:/work/my_proj", 120).unwrap();
    let (b, _) = store.start_session(pid, "b", 200).unwrap();
    store.on_user_prompt(b, "无关", 210).unwrap();
    store.set_session_cwd(b, "C:/work/myXproj", 220).unwrap();

    // 搜 cwd 片段命中 a
    let hit = store.live_sessions(Some("all"), Some("my_proj"), None, None, 100).unwrap();
    // `_` 被转义为字面下划线：只命中 my_proj，不命中 myXproj
    assert!(hit.iter().any(|l| l.session.cc_session_id == "a"));
    assert!(!hit.iter().any(|l| l.session.cc_session_id == "b"), "`_` 应作字面量、不作通配");
}

#[test]
fn live_sessions_waiting_sorted_ascending_and_paginates() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 1).unwrap();
    // 3 条 waiting，last_event_at 递增：等最久(最小 last_event_at)应先出
    for i in 0..3 {
        let (s, _) = store.start_session(pid, &format!("w{i}"), 100 + i).unwrap();
        store.on_user_prompt(s, &format!("t{i}"), 100 + i).unwrap();
        store.set_session_status(s, SessionStatus::Waiting, 100 + i).unwrap();
    }
    let page = store.live_sessions(Some("waiting"), None, None, None, 2).unwrap();
    assert_eq!(page.len(), 2);
    // ASC：w0(最久) 在最前
    assert_eq!(page[0].session.cc_session_id, "w0");
    assert_eq!(page[1].session.cc_session_id, "w1");
    // 用末条 cursor 取下一页（ASC 游标方向）
    let last = page.last().unwrap();
    let next = store
        .live_sessions(Some("waiting"), None, Some(last.session.last_event_at), Some(last.session.id), 2)
        .unwrap();
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].session.cc_session_id, "w2");
}

#[test]
fn live_sessions_search_none_is_backcompat() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 1).unwrap();
    let (s, _) = store.start_session(pid, "s", 100).unwrap();
    store.on_user_prompt(s, "任务", 110).unwrap();
    // search=None 与不搜一致：返回该会话
    let r = store.live_sessions(Some("all"), None, None, None, 100).unwrap();
    assert_eq!(r.len(), 1);
    // search=Some("") 空串按不搜处理
    let r2 = store.live_sessions(Some("all"), Some("  "), None, None, 100).unwrap();
    assert_eq!(r2.len(), 1);
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p meowo-store live_sessions_search`
Expected: 编译失败（`live_sessions` 参数个数不匹配 / `escape_like` 未定义），或新测试断言失败。

- [ ] **Step 3: 实现 `live_sessions` 新签名 + `escape_like`**

在 `crates/meowo-store/src/query.rs` 中，把整个 `pub fn live_sessions(...) { ... }`（从 `pub fn live_sessions` 到其对应的收尾 `Ok(out)\n    }`）替换为：

```rust
    /// 活跃区：按 filter（+ 可选 search，作用于当前 tab 内）取会话，附项目名、任务标题、进度。
    /// waiting tab 按 last_event_at ASC（等最久优先）、其它按 DESC；游标方向随排序翻转，cursor 为 null 取首页。
    /// filter: "all" | "running" | "waiting" | "archived"；其它值按 "all"。search 去空白后非空才生效。
    pub fn live_sessions(
        &self,
        filter: Option<&str>,
        search: Option<&str>,
        before_last_event_at: Option<i64>,
        before_id: Option<i64>,
        limit: usize,
    ) -> Result<Vec<LiveSession>, StoreError> {
        use rusqlite::types::Value;
        const SELECT: &str = "SELECT s.id, s.project_id, s.cc_session_id, s.status, s.started_at, s.last_event_at, s.ended_at,
                p.name, t.id, t.title, t.current_activity, t.column_name, s.pid, s.archived, s.cwd, s.archived_at,
                sc.used_pct, sc.window_size, sc.model, sn.note,
                s.pending_review, s.last_ai_text, s.last_user_text, s.provider
         FROM sessions s
         JOIN projects p ON p.id = s.project_id
         LEFT JOIN tasks t ON t.session_id = s.id
         LEFT JOIN session_context sc ON sc.cc_session_id = s.cc_session_id
         LEFT JOIN session_notes sn ON sn.cc_session_id = s.cc_session_id";

        let mut conditions: Vec<String> = Vec::new();
        let mut params: Vec<Value> = Vec::new();

        match filter {
            Some("all") => conditions.push("s.archived = 0".into()),
            Some("running") => conditions.push("s.status = 'running' AND s.pending_review IS NULL AND s.archived = 0".into()),
            Some("waiting") => conditions.push("(s.status = 'waiting' OR s.pending_review IS NOT NULL) AND s.archived = 0".into()),
            Some("archived") => conditions.push("s.archived = 1".into()),
            _ => {} // None 不过滤
        }

        // 搜索（当前 tab 内 AND 搜索词）：title / cwd / project 名任一命中。%/_/\ 转义成字面量。
        if let Some(q) = search.map(str::trim).filter(|s| !s.is_empty()) {
            let pat = format!("%{}%", escape_like(q));
            conditions.push(
                "(t.title LIKE ? ESCAPE '\\' OR s.cwd LIKE ? ESCAPE '\\' OR p.name LIKE ? ESCAPE '\\')".into(),
            );
            params.push(Value::Text(pat.clone()));
            params.push(Value::Text(pat.clone()));
            params.push(Value::Text(pat));
        }

        // waiting 等最久优先（ASC），游标取「更大」的；其它 DESC，游标取「更小」的。
        let asc = matches!(filter, Some("waiting"));
        if let (Some(ts), Some(id)) = (before_last_event_at, before_id) {
            // 整体括起：AND 优先级高于 OR，不加括号第二个 OR 分支会绕过 filter（4035ec5 回归）。
            if asc {
                conditions.push("((s.last_event_at > ?) OR (s.last_event_at = ? AND s.id > ?))".into());
            } else {
                conditions.push("((s.last_event_at < ?) OR (s.last_event_at = ? AND s.id < ?))".into());
            }
            params.push(Value::Integer(ts));
            params.push(Value::Integer(ts));
            params.push(Value::Integer(id));
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };
        let order = if asc {
            "s.last_event_at ASC, s.id ASC"
        } else {
            "s.last_event_at DESC, s.id DESC"
        };
        let sql = format!("{} {} ORDER BY {} LIMIT ?", SELECT, where_clause, order);
        params.push(Value::Integer(limit as i64));

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(params.iter()), |r| {
                let session = Session {
                    id: r.get(0)?,
                    project_id: r.get(1)?,
                    cc_session_id: r.get(2)?,
                    status: r.get(3)?,
                    started_at: r.get(4)?,
                    last_event_at: r.get(5)?,
                    ended_at: r.get(6)?,
                };
                let project_name: String = r.get(7)?;
                let task_id: Option<i64> = r.get(8)?;
                let task_title: Option<String> = r.get(9)?;
                let current_activity: Option<String> = r.get(10)?;
                let column: Option<String> = r.get(11)?;
                let pid: Option<i64> = r.get(12)?;
                let archived: i64 = r.get(13)?;
                let cwd: Option<String> = r.get(14)?;
                let archived_at: Option<i64> = r.get(15)?;
                let context_pct: Option<i64> = r.get(16)?;
                let context_window: Option<i64> = r.get(17)?;
                let model: Option<String> = r.get(18)?;
                let note: Option<String> = r.get(19)?;
                let pending_review: Option<String> = r.get(20)?;
                let last_ai_text: Option<String> = r.get(21)?;
                let last_user_text: Option<String> = r.get(22)?;
                let provider: String = r.get(23)?;
                Ok((session, project_name, task_id, task_title, current_activity, column, pid, archived, cwd, archived_at, context_pct, context_window, model, note, pending_review, last_ai_text, last_user_text, provider))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let task_ids: Vec<i64> = rows.iter().filter_map(|r| r.2).collect();
        let mut todos_map = self.todos_by_task(&task_ids)?;
        let mut out = Vec::with_capacity(rows.len());
        for (session, project_name, task_id, task_title, current_activity, column, pid, archived, cwd, archived_at, context_pct, context_window, model, note, pending_review, last_ai_text, last_user_text, provider) in rows {
            let todos = task_id
                .and_then(|tid| todos_map.remove(&tid))
                .unwrap_or_default();
            let todo_total = todos.len() as i64;
            let todo_done = todos.iter().filter(|t| t.status == "completed").count() as i64;
            out.push(LiveSession {
                session,
                project_name,
                task_title: task_title.unwrap_or_default(),
                current_activity,
                column: column.unwrap_or_else(|| "todo".to_string()),
                todo_done,
                todo_total,
                todos,
                pid,
                archived: archived != 0,
                archived_at,
                cwd,
                context_pct,
                context_window,
                model,
                note,
                pending_review,
                last_ai_text,
                last_user_text,
                provider,
            });
        }
        Ok(out)
    }
```

并在 `live_sessions` 函数**上方**（或文件内合适的自由函数位置，如 `impl` 块之外文件末尾）新增：

```rust
/// 转义 SQL LIKE 通配符，使用户输入里的 `%` `_` `\` 作字面量匹配（配合 `LIKE … ESCAPE '\'`）。
fn escape_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '%' || c == '_' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p meowo-store`
Expected: 全绿（新 4 测试 + 既有测试补 None 后照常通过）。

- [ ] **Step 5: 提交**

```bash
git add crates/meowo-store/src/query.rs crates/meowo-store/tests/query_test.rs
git commit -m "feat(session-list): live_sessions 加 search(当前tab内) + waiting ASC 排序/游标下沉后端"
```

---

## Task 2: 后端命令 + api 线程 search

**Files:**
- Modify: `app/src-tauri/src/lib.rs`（`get_live_sessions_page` 命令、`live_sessions_blocking`、1954 内部调用）
- Modify: `app/src/api.ts`（`getLiveSessionsPage`）

**Interfaces:**
- Consumes: `live_sessions(filter, search, cursor.., limit)`（Task 1）。
- Produces: 命令 `get_live_sessions_page(filter, search, before_last_event_at, before_id, limit)`；`getLiveSessionsPage(filter, search, cursor, limit)`。

- [ ] **Step 1: `live_sessions_blocking` 加 search 参数**

在 `app/src-tauri/src/lib.rs` 把 `live_sessions_blocking` 签名与内部调用改为：

```rust
fn live_sessions_blocking(
    db_path: &PathBuf,
    tx_cache: &Mutex<meowo_store::TranscriptCache>,
    filter: &str,
    search: Option<&str>,
    before_last_event_at: Option<i64>,
    before_id: Option<i64>,
    limit: usize,
) -> Result<Vec<LiveItem>, String> {
    let store = open_store(db_path)?;
    let sessions = store
        .live_sessions(Some(filter), search, before_last_event_at, before_id, limit)
        .map_err(|e| e.to_string())?;
```

（仅改签名 + 这一处 `live_sessions(...)` 调用，函数其余不动。）

- [ ] **Step 2: `get_live_sessions_page` 命令加 search**

把该命令改为：

```rust
#[tauri::command]
async fn get_live_sessions_page(
    state: State<'_, AppState>,
    filter: String,
    search: Option<String>,
    before_last_event_at: Option<i64>,
    before_id: Option<i64>,
    limit: usize,
) -> Result<Vec<LiveItem>, String> {
    let db_path = state.db_path.clone();
    let tx_cache = state.tx_cache.clone();
    let filter = if ["all", "running", "waiting", "archived"].contains(&filter.as_str()) {
        filter
    } else {
        "all".into()
    };
    tauri::async_runtime::spawn_blocking(move || {
        live_sessions_blocking(&db_path, &tx_cache, &filter, search.as_deref(), before_last_event_at, before_id, limit)
    })
    .await
    .map_err(|e| e.to_string())?
}
```

- [ ] **Step 3: 补内部调用（约 lib.rs:1954）**

把 `store.live_sessions(Some("all"), None, None, 1000)` 改为 `store.live_sessions(Some("all"), None, None, None, 1000)`。
（用 `grep -n 'live_sessions(Some("all"), None, None, 1000)' app/src-tauri/src/lib.rs` 定位。）

- [ ] **Step 4: `api.ts` getLiveSessionsPage 加 search**

把 `app/src/api.ts` 的 `getLiveSessionsPage` 改为：

```ts
export function getLiveSessionsPage(
  filter: StickerFilter,
  search: string | null,
  cursor: { last_event_at: number; id: number } | null,
  limit: number
): Promise<LiveSession[]> {
  return invoke("get_live_sessions_page", {
    filter,
    search: search && search.trim() ? search : null,
    before_last_event_at: cursor?.last_event_at ?? null,
    before_id: cursor?.id ?? null,
    limit,
  });
}
```

- [ ] **Step 5: 编译检查**

Run: `cargo check -p meowo-app`
Expected: `Finished`，无错误（注意：本机若有 meowo-app.exe 运行，`cargo build` 链接会 os error 5，`cargo check` 不链接、应过）。
Run: `cd app && bunx tsc --noEmit`
Expected: 报错——App.tsx 仍以旧 2 参调用 `getLiveSessionsPage`（Task 3 修）。**本步只需 `cargo check` 过**；tsc 待 Task 3。

- [ ] **Step 6: clippy**

Run: `cargo clippy -p meowo-app`
Expected: `Finished` 无 warning。

- [ ] **Step 7: 提交**

```bash
git add app/src-tauri/src/lib.rs app/src/api.ts
git commit -m "feat(session-list): get_live_sessions_page 命令 + api 线程 search 参数"
```

---

## Task 3: App.tsx 分页改 reachedEnd 驱动 + W 窗口 refresh + 节流

**Files:**
- Modify: `app/src/App.tsx`
- Test: `app/src/App.test.tsx`

**Interfaces:**
- Consumes: `getLiveSessionsPage(filter, search, cursor, limit)`（Task 2）。
- Produces: 传给 Sticker 的 `hasMore = !reachedEnd`（不再依赖 counts）。本任务 search 先固定传 `null`（Task 4 再接搜索词）。

- [ ] **Step 1: 记录基线（本任务不新增自动化测试）**

本任务改的是 App 的分页回调（`loadMore` 由 `reachedEnd` 驱动、`refresh` 重查 W 窗口 + 节流）。`loadMore` 由 Sticker 虚拟列表滚动触发，属组件交互/集成行为，在纯 App 单测里难以稳定驱动（jsdom + 虚拟列表 + 定时器）。故本任务**不新增自动化测试**，闸口为：既有 `App.test.tsx` 不回归 + `tsc` 通过 + 末尾「手动验证」。这是刻意选择，非遗漏。

Run: `cd app && bunx vitest run src/App.test.tsx`
Expected: 通过。记下通过数作为 Step 5 不回归对照。

- [ ] **Step 3: 改 App.tsx —— reachedEnd 驱动 + W 窗口 refresh + 节流 + loadPage 加 search 形参（先传 null）**

在 `app/src/App.tsx` 做以下改动：

(a) `loadPage` 增加 `search` 形参并透传，且**支持 refresh 传更大的 limit**。把 `loadPage` 签名与调用 `getLiveSessionsPage` 改为：

```tsx
  const loadPage = useCallback(
    async (
      filter: StickerFilter,
      cursor: { last_event_at: number; id: number } | null,
      limit: number = PAGE_SIZE
    ): Promise<{ page: Item[]; applied: boolean }> => {
      const seq = ++refreshSeqRef.current;
      const needCounts = cursor === null;
      try {
        const [countsRes, page] = await Promise.all([
          needCounts ? getLiveSessionsCounts() : Promise.resolve(null),
          getLiveSessionsPage(filter, null, cursor, limit),
        ]);
```

（`getLiveSessionsPage` 第二参 search 本任务固定 `null`；Task 4 换成真实搜索词。其余 loadPage 主体—— applied 守卫、setItems 首页替换/loadMore 合并——不变。**注意**：首页替换分支 `return (page as Item[]).slice();` 保持不变，它天然支持「W 窗口整体替换」。）

(b) `refresh` 用 W 窗口 + 节流。把 `refresh` 替换为：

```tsx
  // board-changed 频繁触发：节流刷新，且重查「已加载窗口」大小（max(PAGE_SIZE, 当前条数)），
  // 保住用户已滚动加载的会话、同时反映最新排序/状态，避免被打回第一页（P0）。
  const itemsLenRef = useRef(0);
  itemsLenRef.current = items.length;
  const refreshThrottleRef = useRef<number | undefined>(undefined);
  const refresh = useCallback(() => {
    const run = () => {
      setReachedEnd(false);
      const w = Math.max(PAGE_SIZE, itemsLenRef.current);
      loadPage(filter, null, w).then(({ page, applied }) => {
        if (applied && page.length < w) setReachedEnd(true);
      }).catch(() => {});
    };
    // 400ms trailing 节流：连续 board-changed 只在安静后跑一次。
    window.clearTimeout(refreshThrottleRef.current);
    refreshThrottleRef.current = window.setTimeout(run, 400);
  }, [filter, loadPage]);
```

（board-changed 监听 `listen("board-changed", () => refresh())` 不变。卸载时清理见下。）

(c) 在 board-changed 的 useEffect 里补节流定时器清理。把该 effect 改为：

```tsx
  useEffect(() => {
    const un = listen("board-changed", () => refresh());
    return () => {
      un.then((f) => f());
      window.clearTimeout(refreshThrottleRef.current);
    };
  }, [refresh]);
```

(d) `loadMore` 守卫改 reachedEnd 驱动（去掉 `items.length >= totalFor(...)`）。把 `loadMore` 的首行守卫改为：

```tsx
  const loadMore = useCallback(async () => {
    if (loadingMore || reachedEnd) return;
    const last = items[items.length - 1];
    if (!last) return;
    setLoadingMore(true);
    try {
      const { page, applied } = await loadPage(filter, {
        last_event_at: last.session.last_event_at,
        id: last.session.id,
      });
      if (applied && page.length < PAGE_SIZE) {
        setReachedEnd(true);
      }
    } catch (err) {
      console.error("[loadMore] 加载失败：", err);
    } finally {
      setLoadingMore(false);
    }
  }, [filter, loadingMore, reachedEnd, items, loadPage]);
```

（`loadMore` 依赖去掉 `counts`。）

(e) 传给 Sticker 的 `hasMore` 改为 `!reachedEnd`（不再 `&& items.length < totalFor(...)`）。把 `<Sticker>` 的 `hasMore` prop 改为：

```tsx
        hasMore={!reachedEnd}
```

（`total`/`counts` props 仍传，Sticker 角标用；`totalFor` 若变为未使用则一并删除其定义与 import 以免 tsc noUnusedLocals 报错——先搜索确认 `totalFor` 是否还有其它使用点，无则删。）

- [ ] **Step 4: 处理 totalFor 去留**

Run: `cd app && grep -n "totalFor" src/App.tsx`
- 若 `totalFor` 仅剩定义无调用 → 删除其定义（`function totalFor(...) {...}`）与相关不再使用的 import。
- 若仍被用（如 CollapsedStrip 或别处）→ 保留。
以 `bunx tsc --noEmit` 无 unused 报错为准。

- [ ] **Step 5: 类型 + 测试不回归**

Run: `cd app && bunx tsc --noEmit`
Expected: 无错误。
Run: `cd app && bunx vitest run src/App.test.tsx`
Expected: 通过数 ≥ Step 1 基线（无回归）。

- [ ] **Step 6: 提交**

```bash
git add app/src/App.tsx app/src/App.test.tsx
git commit -m "fix(session-list): 分页改 reachedEnd 驱动 + board-changed 重查已加载窗口(节流)，不再打回第一页"
```

---

## Task 4: 搜索下沉 —— App search 状态 + Sticker 走 props

**Files:**
- Modify: `app/src/App.tsx`（search 状态 + 传 loadPage/props）
- Modify: `app/src/views/Sticker.tsx`（搜索框走 props、shown 去客户端搜索、counts.all、去 waiting 客户端重排）
- Test: `app/src/views/Sticker.test.tsx`

**Interfaces:**
- Consumes: Task 3 的 loadPage/refresh；Task 2 的 `getLiveSessionsPage(filter, search, ...)`。
- Produces: Sticker props `search: string`、`onSearchChange: (q: string) => void`。

- [ ] **Step 1: App 加 search 状态并接入 loadPage**

在 `app/src/App.tsx`：

(a) 加状态（放在 `filter` 附近）：
```tsx
  const [search, setSearch] = useState("");
```

(b) `loadPage` 的 `getLiveSessionsPage(filter, null, cursor, limit)` 改为传真实 search。因 `loadPage` 是 useCallback，需让它读到最新 search——把 search 作为 loadPage 参数最稳。改 `loadPage` 签名加 `search` 参数：
```tsx
  const loadPage = useCallback(
    async (
      filter: StickerFilter,
      search: string,
      cursor: { last_event_at: number; id: number } | null,
      limit: number = PAGE_SIZE
    ): Promise<{ page: Item[]; applied: boolean }> => {
      ...
        getLiveSessionsPage(filter, search, cursor, limit),
      ...
    },
    []
  );
```
并更新所有 `loadPage(` 调用点传 search：
- 首页/挂载 effect、tab 切换 effect：`loadPage(filter, search, null)`。
- `refresh`：`loadPage(filter, search, null, w)`（并把 `search` 加进 refresh 的 useCallback 依赖）。
- `loadMore`：`loadPage(filter, search, { ... })`（并把 `search` 加进 loadMore 依赖）。

(c) search 变化时重置并重载（debounce 300ms），且 tab 切换 effect 依赖已含 filter；把「filter 变化重载」的 effect 依赖加上 `search`，并 debounce：
```tsx
  // filter / search 变化：重置到首页（search 变化去抖 300ms）。
  useEffect(() => {
    const t = window.setTimeout(() => {
      setReachedEnd(false);
      loadPage(filter, search, null)
        .then(({ page, applied }) => {
          if (applied && page.length < PAGE_SIZE) setReachedEnd(true);
        })
        .catch(() => {});
    }, 300);
    return () => window.clearTimeout(t);
  }, [filter, search, loadPage]);
```
（删除原先仅 `[filter, loadPage]` 的挂载/切 tab effect，用本 effect 统一。首帧也会在 300ms 后加载——若嫌首帧延迟，可在此 effect 内对「首次」立即执行；简单起见统一 300ms 可接受，或用 `search ? 300 : 0` 延迟。采用 `search ? 300 : 0`：）
```tsx
    const t = window.setTimeout(() => { ... }, search ? 300 : 0);
```

(d) 传 props 给 Sticker：
```tsx
        search={search}
        onSearchChange={setSearch}
```

- [ ] **Step 2: Sticker 写失败测试（搜索走后端 + 不客户端过滤）**

在 `app/src/views/Sticker.test.tsx` 的 `describe("Sticker", ...)` 内追加（沿用其 `render`/`mk`/`zh` 工具；`.stk-vitem` 是卡片、`.stk-search-in` 是搜索输入、`getByLabelText(zh.sticker.search)` 打开搜索）：

```tsx
  it("搜索走后端：输入调用 onSearchChange，且不客户端过滤已加载数据", () => {
    const onSearchChange = vi.fn();
    const { container } = render(
      <Sticker filter="all" data={[mk({ task_title: "任务甲" })]} search="" onSearchChange={onSearchChange} />
    );
    const before = container.querySelectorAll(".stk-vitem").length;
    expect(before).toBeGreaterThan(0);
    // 打开搜索框并输入一个不匹配已加载标题的词
    fireEvent.click(screen.getByLabelText(zh.sticker.search));
    const input = container.querySelector(".stk-search-in") as HTMLInputElement;
    fireEvent.change(input, { target: { value: "不匹配任何标题zzz" } });
    // 搜索词经回调交后端处理
    expect(onSearchChange).toHaveBeenCalledWith("不匹配任何标题zzz");
    // 前端不再按搜索词过滤已加载数据（过滤由后端负责）→ 卡片数不变
    expect(container.querySelectorAll(".stk-vitem").length).toBe(before);
  });
```

该测试对**旧代码会失败**（旧代码本地 `setQuery` → `shown` 客户端过滤 → 卡片消失、且无 `onSearchChange`），对新代码通过——是有效的 RED→GREEN。

`counts.all = total - archived` 是**潜伏修正**（all/archived tab 不显示数字角标，Sticker.tsx:974），无可观察出口，故不单独单测，靠 Step 3(d) 的改动 + 评审核对（一行、口径对齐 App `totalFor`）。

Run: `cd app && bunx vitest run src/views/Sticker.test.tsx`
Expected: 新测试 FAIL（旧代码卡片因客户端过滤消失 / onSearchChange 未接线）。

- [ ] **Step 3: Sticker 实现**

在 `app/src/views/Sticker.tsx`：

(a) props：在组件参数与类型里加 `search`、`onSearchChange`：
```tsx
  search,
  onSearchChange,
  ...
  search?: string;
  onSearchChange?: (q: string) => void;
```
（若 Sticker 有明确 props interface，则加到该 interface。默认值：`const q = search ?? "";`）

(b) 搜索框接线：删除本地 `const [query, setQuery] = useState("")`，改用 props；`closeSearch` 调 `onSearchChange?.("")`：
```tsx
  const [searchOpen, setSearchOpen] = useState(false);
  const q = search ?? "";
  const closeSearch = () => {
    setSearchOpen(false);
    onSearchChange?.("");
  };
```
搜索输入框（约 1259 行）`value={q}`、`onChange={(e) => onSearchChange?.(e.target.value)}`。

(c) `shown`：去掉客户端搜索过滤分支（后端做）。把 `shown` 的 useMemo 改为——**保留** `match(tab)` 安全网与 starred 浮顶，**去掉** `if (q) {...}` 搜索分支与 waiting 的客户端 ASC 重排：
```tsx
  const shown = useMemo(() => {
    return data
      .filter((l) => match(tab, l, hideDays))
      .sort(
        (a, b) =>
          Number(starred.has(b.session.cc_session_id)) -
          Number(starred.has(a.session.cc_session_id))
      );
  }, [data, tab, hideDays, starred]);
```
（依赖去掉 `query`。waiting 的「等最久优先」现由后端 ASC 保证，客户端只做 starred 浮顶。）

(d) `counts.all` 口径：把 `all: countsProp.total` 改为 `all: countsProp.total - countsProp.archived`：
```tsx
        all: countsProp.total - countsProp.archived,
```

- [ ] **Step 4: 跑通新测试**

Run: `cd app && bunx vitest run src/views/Sticker.test.tsx`
Expected: Step 2 的新测试通过（且既有 Sticker 测试不回归）。

- [ ] **Step 5: 全量类型 + 测试**

Run: `cd app && bunx tsc --noEmit`（无错误）
Run: `cd app && bunx vitest run`（全绿）

- [ ] **Step 6: 提交**

```bash
git add app/src/App.tsx app/src/views/Sticker.tsx app/src/views/Sticker.test.tsx
git commit -m "feat(session-list): 搜索下沉后端(当前tab内全库搜) + counts.all 口径 + 去客户端搜索/waiting重排"
```

---

## 手动验证（实现完成后，重启 dev app）

1. 「全部」滚到底能加载到全部会话（不再卡在 ~100）。
2. 有活动会话时（持续写库），滚动加载后**不被打回第一页**。
3. 搜索能命中**未加载**的会话；运行中 tab 搜索只在运行中范围内。
4. 待交互 tab 按**等最久优先**排序。
5. 关闭搜索框恢复完整列表。

## Self-Review 结论

- **Spec 覆盖**：搜索下沉+当前tab内(T1/T2/T4)、reachedEnd 分页(T3)、W 窗口 refresh+节流(T3)、waiting ASC 下沉(T1)、counts.all 口径(T4)、去客户端搜索/waiting重排(T4)、starred 保留(T4)、P3 不含 —— 均有对应任务。
- **无占位测试**：T3 不新增自动化测试（loadMore 属滚动触发的集成行为，闸口=不回归+tsc+手动，已显式说明）；T4 用真实的 onSearchChange RED→GREEN 测试；counts.all 是无展示出口的潜伏一行，评审核对而非单测。均非占位。
- **类型一致**：`live_sessions(filter, search, cursor.., limit)`、`getLiveSessionsPage(filter, search, cursor, limit)`、`get_live_sessions_page(filter, search, ...)`、Sticker `search`/`onSearchChange` 前后一致。
- **编译顺序**：T1 改签名后 meowo-store 独立编译+测试通过；meowo-app 待 T2 接；前端 tsc 待 T3/T4。各任务闸口已相应说明。

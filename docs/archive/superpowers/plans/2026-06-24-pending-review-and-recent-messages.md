# 待审批子态 + 最近消息字段 合并实现 Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把「待审批/待权限会话子态」(spec A) 与「卡片最近一条 AI/用户消息改用 hook 现成字段」(spec B) 两份设计合并为一条 TDD 流水线落地。

**Architecture:** 两份 spec 共享数据层(sessions 表加 3 列、`LiveSession` 加 3 字段、`live_sessions()` 回带)、meowo-reporter 的 `dispatch`、meowo-app 的 `lib.rs`、前端 `Sticker.tsx` / `api.ts` / `i18n`。为避免两特性各自重复触碰同一文件(尤其 schema 数组、SELECT、dispatch 分支)造成冲突,本 plan **先建共享数据层地基(Task 1–3),再分特性推进逻辑层**。A = `pending_review TEXT` 正交子态(NULL/approval/question/plan),在 hook 进入时置位、下一个事件清除,通知/前端按 error > pending > waiting 三级表达;B = `last_ai_text` / `last_user_text` 两个独立稳定字段,Stop / UserPromptSubmit 时刻落库,transcript 解析降为兜底。

**Tech Stack:** Rust (rusqlite + serde + thiserror, workspace 三 crate: `meowo-store` / `meowo-reporter` / `meowo-app`)、Tauri 2、React + Vite + TypeScript、vitest、Node (`scripts/install-hooks.mjs`)。

## Global Constraints

以下为两份 spec 的项目级约束,每个 task 隐含适用,逐字照抄自勘察事实:

- **三个新 sessions 列均为可空 `TEXT`**:`pending_review`、`last_ai_text`、`last_user_text`。新库写进 `crates/meowo-store/src/migrations.rs` 的 `CREATE TABLE sessions`;旧库靠 `Store::init`(**注意:不存在 `Store::migrate`,实际函数名是 `init`,`crates/meowo-store/src/store.rs:43`**)里的 `ALTERS` 数组 `ADD COLUMN` 补列;**必须把 `USER_VERSION` 从 `2` bump 到 `3`**,否则旧库提前返回、ALTER 永不执行。
- **新派生字段落在 `LiveSession` 顶层**(与 `pid` / `cwd` / `archived_at` 一致——它们同为 sessions 表列却放顶层),**不放进内嵌 `Session` struct**,以免波及 `get_session` 与 dispatch 测试的 `Session{}` 构造点。
- **`live_sessions()` 的 SELECT 是显式列举(非 `SELECT s.*`),且 `row.get(idx)` 用手写位置常量**:新列必须追加到 SELECT **末尾**(现有 idx 0..=18 不能动位),否则全部错位。
- **Rust 枚举落库模式**:复制 `TodoStatus`(`models.rs:41`)整套——`#[serde(rename_all = "lowercase")]` + 手写 `as_str(self) -> &'static str`;DB 用 `as_str`,JSON 用 serde。
- **rusqlite 全用位置参数 `?1/?2/?3` + `rusqlite::params![]`**,无命名参数。
- **dispatch 全程 best-effort**:未知 `tool_name` / 缺 session 一律无操作,`_ => {}` 兜底,绝不冒泡。
- **通知文案在 Rust 侧**:`app/src-tauri/src/settings.rs` 的 `pub(crate) fn tr(lang: &str, key: &str) -> &'static str`(`settings.rs:99`)。前端 i18n **没有** notification segment。新增 key 要写 `("en", key)` + `(_, key)` 两组(zh 走 `_` 兜底)。
- **前端 i18n 已落地**:`app/src/i18n/{index.tsx,zh.ts,en.ts}`,`en: Dict` 编译期约束 + `i18n.test.ts` 运行时 key 集合/arity 校验。**任何新 key 必须 zh + en 同步且 arity 一致**,否则 `tsc` 与 `i18n.test.ts` 双失败。
- **图标统一已落地**:`TabIcon`/`EmptyIcon` 的 waiting=举手、running=循环箭头。本 plan 不动图标。
- **测试位置**:`meowo-store` → `crates/meowo-store/tests/*.rs`(集成,走 public API);`meowo-reporter` → `crates/meowo-reporter/tests/dispatch_test.rs`;`ccsetup` → `app/src-tauri/src/ccsetup.rs` 文件尾 `#[cfg(test)] mod tests`;`lib.rs` 纯函数 → `lib.rs` 内联测试(`lib.rs:2013` 附近);前端 → `*.test.tsx` / `*.test.ts` 跑 vitest。
- **in-memory store 构造**:`Store::open_in_memory().unwrap()`(**不是** `Store::open(":memory:")`)。
- **测试命令**:`cargo test -p meowo-store` / `cargo test -p meowo-reporter` / `cargo test -p meowo-app`;前端 `cd app && bun run test`(= `vitest run`)、类型检查 `cd app && bunx tsc --noEmit`。
- **commit message 用中文**;Rust 改动 commit 前本地可跑 `cargo clippy --all-targets`(CI 会卡 clippy)。

---

## 任务总览

| # | 子系统 | 内容 | 特性 |
|---|--------|------|------|
| 1 | meowo-store | schema 加 3 列 + bump version + `LiveSession` 回带 | A+B 地基 |
| 2 | meowo-store | `PendingReview` 枚举 + `set_pending_review` / `clear_pending_review` | A |
| 3 | meowo-store | `set_last_ai_text` / `set_last_user_text` | B |
| 4 | meowo-reporter | `HookEvent` 加 `last_assistant_message`(serde alias) | B |
| 5 | meowo-reporter | dispatch:Stop 落 `last_ai_text`、UserPromptSubmit 落 `last_user_text` + `on_user_prompt` 去 `current_activity` | B |
| 6 | meowo-reporter | dispatch:新增 `PermissionRequest` + `PreToolUse` 置 `pending_review` | A |
| 7 | meowo-reporter | dispatch:四分支加 `clear_pending_review` | A |
| 8 | meowo-store | `analyze.rs` `fold_line` 改拼接一条 assistant 的所有 text 块 | B 兜底 |
| 9 | meowo-app | `ccsetup.rs` matcher 感知 + `HOOK_SPECS`(8 条) | A |
| 10 | scripts | `install-hooks.mjs` 同步 (event,matcher) 维度 | A |
| 11 | meowo-app | `settings.rs` `tr()` 加 3 条 pending 通知文案 | A |
| 12 | meowo-app | `lib.rs` `pending_fingerprint` + 改 `waiting_fingerprint` + `spawn_liveness_watch` pending 通知 + 计数口径 | A |
| 13 | 前端 | `api.ts` `LiveSession` 加 3 字段 | A+B |
| 14 | 前端 | `i18n` zh/en 加 pending 标签 + 你/AI 前缀 | A+B |
| 15 | 前端 | `Sticker.tsx` `match()` 纳入 pending + 排序置顶 + 计数 | A |
| 16 | 前端 | `Sticker.tsx` indicator pending 徽标 + 醒目 pill + CSS | A |
| 17 | 前端 | `Sticker.tsx` 活动行加用户消息行 + AI 行优先 `last_ai_text` | B |

依赖顺序:1 → 2,3(并列) → 4 → 5 → 6 → 7;8 独立(可任意时点);9 → 10;11 → 12;13 → 14 → 15 → 16,17。后端(1–12)与前端(13–17)之间靠 snake_case 字段名对齐。

---

## Task 1: meowo-store schema 加 3 列 + bump version + LiveSession 回带

**Files:**
- Modify: `crates/meowo-store/src/migrations.rs:10-23`(`CREATE TABLE sessions`)
- Modify: `crates/meowo-store/src/store.rs:37`(`USER_VERSION`)、`:52-56`(`ALTERS` 数组)
- Modify: `crates/meowo-store/src/query.rs:28-49`(`LiveSession` struct)、`:193-264`(`live_sessions()`)
- Test: `crates/meowo-store/tests/query_test.rs`(新增用例)

**Interfaces:**
- Produces: `LiveSession` 新增三个公开字段 `pub pending_review: Option<String>`、`pub last_ai_text: Option<String>`、`pub last_user_text: Option<String>`(后续 Task 2/3/12 与前端依赖)。

- [ ] **Step 1: 写失败测试**

在 `crates/meowo-store/tests/query_test.rs` 末尾追加(若文件无 `use`,参照文件首部已有 `use meowo_store::*;` 风格;建会话用 public API):

```rust
#[test]
fn live_sessions_returns_new_columns_as_none_by_default() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, _) = store.start_session(pid, "cc1", 100).unwrap();
    let _ = sid;

    let live = store.live_sessions().unwrap();
    let s = live.iter().find(|l| l.session.cc_session_id == "cc1").unwrap();
    assert_eq!(s.pending_review, None);
    assert_eq!(s.last_ai_text, None);
    assert_eq!(s.last_user_text, None);
}
```

- [ ] **Step 2: 运行,确认编译失败**

Run: `cargo test -p meowo-store live_sessions_returns_new_columns_as_none_by_default`
Expected: 编译错误 `no field 'pending_review' on type '&LiveSession'`(字段尚未存在)。

- [ ] **Step 3: 加列到 SCHEMA**

`crates/meowo-store/src/migrations.rs` 把 `CREATE TABLE sessions` 末列 `archived_at INTEGER` 后补三列(在 `);` 之前):

```sql
    archived      INTEGER NOT NULL DEFAULT 0,
    archived_at   INTEGER,
    pending_review TEXT,
    last_ai_text   TEXT,
    last_user_text TEXT
);
```

- [ ] **Step 4: 改 ALTERS 数组 + bump version**

`crates/meowo-store/src/store.rs:37` 把 `const USER_VERSION: i64 = 2;` 改为:

```rust
    /// v3: sessions 加 pending_review / last_ai_text / last_user_text 三列。
    const USER_VERSION: i64 = 3;
```

`store.rs:52-56` 的 `ALTERS` 数组(原 `[&str; 4]`)改为 `[&str; 7]`:

```rust
        const ALTERS: [&str; 7] = [
            "ALTER TABLE sessions ADD COLUMN pid INTEGER",
            "ALTER TABLE sessions ADD COLUMN cwd TEXT",
            "ALTER TABLE sessions ADD COLUMN archived INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE sessions ADD COLUMN archived_at INTEGER",
            "ALTER TABLE sessions ADD COLUMN pending_review TEXT",
            "ALTER TABLE sessions ADD COLUMN last_ai_text TEXT",
            "ALTER TABLE sessions ADD COLUMN last_user_text TEXT",
        ];
```

- [ ] **Step 5: 给 LiveSession 加三字段**

`crates/meowo-store/src/query.rs`,在 `LiveSession` struct 的 `note` 字段(`:48`)之后、`}` 之前追加:

```rust
    /// 待审批子态：NULL/approval/question/plan(回合中途等用户介入)。
    pub pending_review: Option<String>,
    /// 最近一条 AI 正文(锚 Stop hook 的 last_assistant_message)；无则 None,前端回退 transcript preview。
    pub last_ai_text: Option<String>,
    /// 最近一条用户消息(锚 UserPromptSubmit.prompt)；独立字段,不被工具活动覆盖。
    pub last_user_text: Option<String>,
```

- [ ] **Step 6: 改 live_sessions() 的 SELECT + 回填(三处同步)**

`query.rs:197-199` 的 SELECT 文本,在 `sn.note` 之后追加三列(新 idx 19/20/21):

```rust
            "SELECT s.id, s.project_id, s.cc_session_id, s.status, s.started_at, s.last_event_at, s.ended_at,
                    p.name, t.id, t.title, t.current_activity, t.column_name, s.pid, s.archived, s.cwd, s.archived_at,
                    sc.used_pct, sc.window_size, sn.note,
                    s.pending_review, s.last_ai_text, s.last_user_text
             FROM sessions s
```

在 `query_map` 闭包内 `let note: Option<String> = r.get(18)?;`(`:230`)之后追加:

```rust
                let pending_review: Option<String> = r.get(19)?;
                let last_ai_text: Option<String> = r.get(20)?;
                let last_user_text: Option<String> = r.get(21)?;
```

把闭包返回的元组(`:231`)末尾扩展(在 `note` 后加三项):

```rust
                Ok((session, project_name, task_id, task_title, current_activity, column, pid, archived, cwd, archived_at, context_pct, context_window, note, pending_review, last_ai_text, last_user_text))
```

`for ... in rows`(`:239`)解构模式同步加三项(在 `note` 后):

```rust
        for (session, project_name, task_id, task_title, current_activity, column, pid, archived, cwd, archived_at, context_pct, context_window, note, pending_review, last_ai_text, last_user_text) in rows {
```

`LiveSession { ... }` 构造块(`:262`)在 `note,` 之后追加:

```rust
                note,
                pending_review,
                last_ai_text,
                last_user_text,
```

- [ ] **Step 7: 运行测试,确认通过**

Run: `cargo test -p meowo-store live_sessions_returns_new_columns_as_none_by_default`
Expected: PASS。再跑 `cargo test -p meowo-store` 全量,确认 `store_test.rs:10` 的 `assert_eq!(count, 7)`(表数)等既有用例不回归。

- [ ] **Step 8: Commit**

```bash
git add crates/meowo-store/src/migrations.rs crates/meowo-store/src/store.rs crates/meowo-store/src/query.rs crates/meowo-store/tests/query_test.rs
git commit -m "feat(store): sessions 加 pending_review/last_ai_text/last_user_text 三列并由 live_sessions 回带"
```

---

## Task 2: PendingReview 枚举 + set_pending_review / clear_pending_review

**Files:**
- Modify: `crates/meowo-store/src/models.rs`(新增 `PendingReview` 枚举,紧随 `TodoStatus` 之后,`:67` 附近)
- Modify: `crates/meowo-store/src/store.rs`(新增两个方法,放在 `set_session_status` 之后,`:442` 附近)
- Test: `crates/meowo-store/tests/store_test.rs`

**Interfaces:**
- Consumes: Task 1 的 `LiveSession.pending_review`(测试读回断言)。
- Produces:
  - `pub enum PendingReview { Approval, Question, Plan }` + `pub fn as_str(self) -> &'static str`(Task 6 dispatch 用)。
  - `pub fn set_pending_review(&self, session_id: i64, kind: PendingReview, now_ms: i64) -> Result<(), StoreError>`(刷新 `last_event_at`)。
  - `pub fn clear_pending_review(&self, session_id: i64) -> Result<(), StoreError>`(不动 `last_event_at`)。

- [ ] **Step 1: 写失败测试**

`crates/meowo-store/tests/store_test.rs` 末尾追加:

```rust
#[test]
fn pending_review_set_and_clear() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, _) = store.start_session(pid, "cc1", 100).unwrap();

    // set:写入子态并刷新 last_event_at。
    store.set_pending_review(sid, PendingReview::Approval, 500).unwrap();
    let live = store.live_sessions().unwrap();
    let s = live.iter().find(|l| l.session.cc_session_id == "cc1").unwrap();
    assert_eq!(s.pending_review.as_deref(), Some("approval"));
    assert_eq!(s.session.last_event_at, 500);

    // clear:置 NULL,且不改 last_event_at。
    store.clear_pending_review(sid).unwrap();
    let live = store.live_sessions().unwrap();
    let s = live.iter().find(|l| l.session.cc_session_id == "cc1").unwrap();
    assert_eq!(s.pending_review, None);
    assert_eq!(s.session.last_event_at, 500);
}
```

确认 `store_test.rs` 顶部 `use meowo_store::*;` 能带出 `PendingReview`(`lib.rs:11` 的 `pub use models::*;` 会自动导出新枚举);若用的是具名 `use`,补 `PendingReview`。

- [ ] **Step 2: 运行,确认失败**

Run: `cargo test -p meowo-store pending_review_set_and_clear`
Expected: 编译错误 `cannot find type 'PendingReview'` / `no method named 'set_pending_review'`。

- [ ] **Step 3: 加 PendingReview 枚举**

`crates/meowo-store/src/models.rs`,在 `TodoStatus` 的 `impl` 块结束(`:67`)之后追加(复制 `TodoStatus` 模式):

```rust
/// 待审批子态:回合中途等用户介入的三种情形。NULL 态在 store 层用 Option 表达,枚举不含 None 变体。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PendingReview {
    Approval,
    Question,
    Plan,
}

impl PendingReview {
    pub fn as_str(self) -> &'static str {
        match self {
            PendingReview::Approval => "approval",
            PendingReview::Question => "question",
            PendingReview::Plan => "plan",
        }
    }
}
```

- [ ] **Step 4: 加两个 store 方法**

`crates/meowo-store/src/store.rs`,在 `set_session_status`(结束于 `:442`)之后追加。注意 `PendingReview` 需在 store.rs 顶部 `use` 中可见——store.rs 已 `use crate::models::*;` 或类似(若是具名 `use crate::models::{...}` 则补 `PendingReview`):

```rust
    /// 设置待审批子态,同时刷新 last_event_at(让卡片排到最近活跃,并作为去重指纹)。
    pub fn set_pending_review(
        &self,
        session_id: i64,
        kind: PendingReview,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE sessions SET pending_review = ?1, last_event_at = ?2 WHERE id = ?3",
            rusqlite::params![kind.as_str(), now_ms, session_id],
        )?;
        Ok(())
    }

    /// 清除待审批子态(置 NULL)。不动 last_event_at——由同回合的兄弟调用负责时间戳。
    pub fn clear_pending_review(&self, session_id: i64) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE sessions SET pending_review = NULL WHERE id = ?1",
            rusqlite::params![session_id],
        )?;
        Ok(())
    }
```

- [ ] **Step 5: 运行测试,确认通过**

Run: `cargo test -p meowo-store pending_review_set_and_clear`
Expected: PASS。

- [ ] **Step 6: Commit**

```bash
git add crates/meowo-store/src/models.rs crates/meowo-store/src/store.rs crates/meowo-store/tests/store_test.rs
git commit -m "feat(store): 新增 PendingReview 枚举与 set/clear_pending_review"
```

---

## Task 3: set_last_ai_text / set_last_user_text

**Files:**
- Modify: `crates/meowo-store/src/store.rs`(两个新方法,放在 `clear_pending_review` 之后)
- Test: `crates/meowo-store/tests/store_test.rs`

**Interfaces:**
- Consumes: Task 1 的 `LiveSession.last_ai_text` / `last_user_text`;`store.rs` 模块私有 `sanitize_prompt`(`:607`)与 `truncate_chars`。
- Produces:
  - `pub fn set_last_ai_text(&self, session_id: i64, text: &str) -> Result<(), StoreError>`(清洗+截断 200,空串不覆盖,不动 `last_event_at`)。
  - `pub fn set_last_user_text(&self, session_id: i64, text: &str) -> Result<(), StoreError>`(复用 `sanitize_prompt` + 截断 200,空串不覆盖,不动 `last_event_at`)。

- [ ] **Step 1: 写失败测试**

`crates/meowo-store/tests/store_test.rs` 末尾追加:

```rust
#[test]
fn last_ai_and_user_text_set_with_cleaning() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, _) = store.start_session(pid, "cc1", 100).unwrap();

    // 折叠空白;不动 last_event_at(仍是建会话时的 100)。
    store.set_last_ai_text(sid, "  调研   完成。\n结论更微妙  ").unwrap();
    store.set_last_user_text(sid, "切到这个 [Image #1] 任务").unwrap();
    let live = store.live_sessions().unwrap();
    let s = live.iter().find(|l| l.session.cc_session_id == "cc1").unwrap();
    assert_eq!(s.last_ai_text.as_deref(), Some("调研 完成。 结论更微妙"));
    assert_eq!(s.last_user_text.as_deref(), Some("切到这个 任务")); // [Image #1] 被 sanitize 剥除
    assert_eq!(s.session.last_event_at, 100);

    // 空串/全空白不覆盖旧值。
    store.set_last_ai_text(sid, "   ").unwrap();
    let live = store.live_sessions().unwrap();
    let s = live.iter().find(|l| l.session.cc_session_id == "cc1").unwrap();
    assert_eq!(s.last_ai_text.as_deref(), Some("调研 完成。 结论更微妙"));
}
```

- [ ] **Step 2: 运行,确认失败**

Run: `cargo test -p meowo-store last_ai_and_user_text_set_with_cleaning`
Expected: 编译错误 `no method named 'set_last_ai_text'`。

- [ ] **Step 3: 实现两个方法**

`crates/meowo-store/src/store.rs`,在 `clear_pending_review` 之后追加(`sanitize_prompt` 与 `truncate_chars` 是同模块函数,直接调用):

```rust
    /// 落最近一条 AI 正文:折叠空白 + 截断 200 字符;空/全空白不覆盖旧值。
    /// 不动 last_event_at——Stop 的兄弟 set_session_status 已刷新它。
    pub fn set_last_ai_text(&self, session_id: i64, text: &str) -> Result<(), StoreError> {
        let cleaned = truncate_chars(&sanitize_prompt(text), 200);
        if cleaned.is_empty() {
            return Ok(());
        }
        self.conn.execute(
            "UPDATE sessions SET last_ai_text = ?1 WHERE id = ?2",
            rusqlite::params![cleaned, session_id],
        )?;
        Ok(())
    }

    /// 落最近一条用户消息:复用 sanitize_prompt(剥图片标记 + 折叠空白) + 截断 200;空不覆盖。
    /// 不动 last_event_at——UserPromptSubmit 的 on_user_prompt(touch_session) 已刷新它。
    pub fn set_last_user_text(&self, session_id: i64, text: &str) -> Result<(), StoreError> {
        let cleaned = truncate_chars(&sanitize_prompt(text), 200);
        if cleaned.is_empty() {
            return Ok(());
        }
        self.conn.execute(
            "UPDATE sessions SET last_user_text = ?1 WHERE id = ?2",
            rusqlite::params![cleaned, session_id],
        )?;
        Ok(())
    }
```

- [ ] **Step 4: 运行测试,确认通过**

Run: `cargo test -p meowo-store last_ai_and_user_text_set_with_cleaning`
Expected: PASS。(若 `truncate_chars` 不在 store.rs 作用域,确认其定义位置并按现状调用方式引用——`on_user_prompt`/`set_current_activity` 已在同文件使用它。)

- [ ] **Step 5: Commit**

```bash
git add crates/meowo-store/src/store.rs crates/meowo-store/tests/store_test.rs
git commit -m "feat(store): 新增 set_last_ai_text/set_last_user_text(清洗截断、空不覆盖)"
```

---

## Task 4: HookEvent 加 last_assistant_message(serde alias)

**Files:**
- Modify: `crates/meowo-reporter/src/hook.rs:4-19`(`HookEvent` struct)
- Test: `crates/meowo-reporter/tests/dispatch_test.rs`

**Interfaces:**
- Produces: `HookEvent` 新增 `pub last_assistant_message: Option<String>`,带 `#[serde(default, alias = "assistant_message")]`(对冲官方字段名不确定性)。Task 5 dispatch 的 Stop 分支消费。

- [ ] **Step 1: 写失败测试**

`crates/meowo-reporter/tests/dispatch_test.rs` 末尾追加(文件已有 `fn ev(json: &str) -> HookEvent`):

```rust
#[test]
fn hookevent_parses_last_assistant_message_and_alias() {
    let a = ev(r#"{"hook_event_name":"Stop","session_id":"s","last_assistant_message":"结论更微妙"}"#);
    assert_eq!(a.last_assistant_message.as_deref(), Some("结论更微妙"));
    // 官方文档另称 assistant_message,alias 也要能接住。
    let b = ev(r#"{"hook_event_name":"Stop","session_id":"s","assistant_message":"另一种字段名"}"#);
    assert_eq!(b.last_assistant_message.as_deref(), Some("另一种字段名"));
}
```

- [ ] **Step 2: 运行,确认失败**

Run: `cargo test -p meowo-reporter hookevent_parses_last_assistant_message_and_alias`
Expected: 编译错误 `no field 'last_assistant_message'`。

- [ ] **Step 3: 加字段**

`crates/meowo-reporter/src/hook.rs`,在 `tool_input` 字段(`:18`)之后、`}` 之前追加:

```rust
    #[serde(default, alias = "assistant_message")]
    pub last_assistant_message: Option<String>,
```

- [ ] **Step 4: 运行测试,确认通过**

Run: `cargo test -p meowo-reporter hookevent_parses_last_assistant_message_and_alias`
Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add crates/meowo-reporter/src/hook.rs crates/meowo-reporter/tests/dispatch_test.rs
git commit -m "feat(reporter): HookEvent 加 last_assistant_message(serde alias assistant_message)"
```

---

## Task 5: dispatch — Stop 落 last_ai_text、UserPromptSubmit 落 last_user_text + on_user_prompt 去 current_activity

**Files:**
- Modify: `crates/meowo-store/src/store.rs:303-331`(`on_user_prompt` 移除 `current_activity` 写入)
- Modify: `crates/meowo-reporter/src/dispatch.rs`(`UserPromptSubmit` 分支 `:21-32`、`Stop` 分支 `:48-53`)
- Test: `crates/meowo-reporter/tests/dispatch_test.rs`、`crates/meowo-store/tests/store_test.rs`

**Interfaces:**
- Consumes: Task 3 的 `set_last_ai_text` / `set_last_user_text`;Task 4 的 `HookEvent.last_assistant_message`。
- Produces: 行为变化——`current_activity` 不再被用户 prompt 写入(只剩 Bash 命令写它);`last_user_text` / `last_ai_text` 在对应 hook 落库。

- [ ] **Step 1: 写失败测试(store + dispatch 两处)**

`crates/meowo-store/tests/store_test.rs` 末尾追加(验证 on_user_prompt 不再写 current_activity,但仍替换占位标题):

```rust
#[test]
fn on_user_prompt_no_longer_writes_current_activity() {
    let store = Store::open_in_memory().unwrap();
    let pid = store.upsert_project_by_root("/p", "p", 100).unwrap();
    let (sid, _) = store.start_session(pid, "cc1", 100).unwrap();
    let tid = store.task_id_of_session_pub(sid).unwrap();

    store.on_user_prompt(sid, "实现登录功能", 200).unwrap();
    let t = store.get_task(tid).unwrap();
    assert_eq!(t.title, "实现登录功能");          // 占位标题被首句替换(保留)
    assert_eq!(t.current_activity, None);          // 不再把 prompt 写进 current_activity
}
```

`crates/meowo-reporter/tests/dispatch_test.rs` 末尾追加(验证落库):

```rust
#[test]
fn stop_sets_last_ai_text_and_prompt_sets_last_user_text() {
    let store = Store::open_in_memory().unwrap();
    dispatch(&store, &ev(r#"{"hook_event_name":"SessionStart","session_id":"m1","cwd":"/p"}"#), 100).unwrap();
    dispatch(&store, &ev(r#"{"hook_event_name":"UserPromptSubmit","session_id":"m1","prompt":"切到这个任务"}"#), 200).unwrap();
    dispatch(&store, &ev(r#"{"hook_event_name":"Stop","session_id":"m1","last_assistant_message":"调研完成,结论更微妙"}"#), 300).unwrap();

    let live = store.live_sessions().unwrap();
    let s = live.iter().find(|l| l.session.cc_session_id == "m1").unwrap();
    assert_eq!(s.last_user_text.as_deref(), Some("切到这个任务"));
    assert_eq!(s.last_ai_text.as_deref(), Some("调研完成,结论更微妙"));
}
```

- [ ] **Step 2: 运行,确认失败**

Run: `cargo test -p meowo-store on_user_prompt_no_longer_writes_current_activity && cargo test -p meowo-reporter stop_sets_last_ai_text_and_prompt_sets_last_user_text`
Expected: 第一个断言 `t.current_activity` 不为 None(现状仍写)而 FAIL;第二个因 `last_user_text`/`last_ai_text` 仍为 None 而 FAIL。

- [ ] **Step 3: 改 on_user_prompt 移除 current_activity 写入**

`crates/meowo-store/src/store.rs:317-327`,把占位/非占位两分支替换为(占位分支只更新 title,非占位分支无操作):

```rust
            if title == "(未命名会话)" {
                self.conn.execute(
                    "UPDATE tasks SET title = ?1, updated_at = ?2 WHERE id = ?3",
                    rusqlite::params![cleaned, now_ms, tid],
                )?;
            }
            // 非占位标题:不再把 prompt 写进 current_activity(改由 last_user_text 承担)。
```

(注:删除原 `current_activity = ?2` 的占位分支赋值与整个 `else { UPDATE ... current_activity ... }` 分支;`touch_session` 与函数其余不变。)

同时改 `on_user_prompt` 的函数级注释(`store.rs:302`):把 `/// 收到用户 prompt：占位标题则替换为截断后的 prompt；当前动作总是更新为该 prompt。` 改为 `/// 收到用户 prompt：仅当占位标题时替换为截断后的 prompt(不再写 current_activity，那已由 last_user_text 承担)。`

**⚠️ 同步修正两个既有测试(否则 Step 5 全量 `cargo test -p meowo-store` 必红——它们断言 prompt 被写进 `current_activity`)**:
- `crates/meowo-store/tests/store_test.rs` 的 `first_prompt_sets_title_then_later_prompts_only_update_activity`(`:55-70`):删除第 `:64` 的 `assert_eq!(t.current_activity.as_deref(), Some("实现登录功能并写测试"));` 与第 `:69` 的 `assert_eq!(t2.current_activity.as_deref(), Some("再加个登出按钮"));` 两行(改后均为 None);保留其 title 断言。建议把该用例重命名为 `first_prompt_sets_title_then_later_prompts_keep_title` 以反映新语义。
- `crates/meowo-store/tests/store_test.rs` 的 `prompt_with_image_marker_is_cleaned_for_title`(`:184-193`):删除第 `:192` 的 `assert_eq!(t.current_activity.as_deref(), Some("把路径放在最前面"));`(改后为 None);保留 title 断言。
- 无需动 `image_only_prompt_keeps_placeholder_title`(`:204-213`):它断言 `current_activity == None`,改后仍 PASS。

- [ ] **Step 4: 改 dispatch UserPromptSubmit + Stop 分支**

`crates/meowo-reporter/src/dispatch.rs` 的 `UserPromptSubmit` 分支,在 `store.on_user_prompt(sid, prompt, now_ms)?;` 之后补一行:

```rust
                if let Some(prompt) = ev.prompt.as_deref() {
                    store.on_user_prompt(sid, prompt, now_ms)?;
                    store.set_last_user_text(sid, prompt)?;
                }
```

`Stop` 分支(`:48-53`)改为:

```rust
        "Stop" => {
            if let Some(sid) = lookup_session(store, ev)? {
                store.set_session_status(sid, SessionStatus::Waiting, now_ms)?;
                if let Some(msg) = ev.last_assistant_message.as_deref() {
                    store.set_last_ai_text(sid, msg)?;
                }
                apply_title(store, ev, sid, now_ms)?;
            }
        }
```

- [ ] **Step 5: 运行测试,确认通过**

Run: `cargo test -p meowo-store on_user_prompt_no_longer_writes_current_activity && cargo test -p meowo-reporter stop_sets_last_ai_text_and_prompt_sets_last_user_text`
Expected: 均 PASS。再跑 `cargo test -p meowo-store && cargo test -p meowo-reporter` 全量,确认无回归(尤其 `posttooluse_bash_sets_current_activity` 仍 PASS——Bash 写 current_activity 不受影响;若有旧用例断言 prompt→current_activity,删除/改正该断言)。

- [ ] **Step 6: Commit**

```bash
git add crates/meowo-store/src/store.rs crates/meowo-reporter/src/dispatch.rs crates/meowo-reporter/tests/dispatch_test.rs crates/meowo-store/tests/store_test.rs
git commit -m "feat(reporter): Stop/UserPromptSubmit 落 last_ai_text/last_user_text,current_activity 回归只表示在跑什么"
```

---

## Task 6: dispatch — 新增 PermissionRequest + PreToolUse 置 pending_review

**Files:**
- Modify: `crates/meowo-reporter/src/dispatch.rs`(`dispatch` 的 `match` 新增两个 arm)
- Test: `crates/meowo-reporter/tests/dispatch_test.rs`

**Interfaces:**
- Consumes: Task 2 的 `store.set_pending_review` + `PendingReview`(`meowo_store::PendingReview`)。
- Produces: `PermissionRequest` 与 `PreToolUse(AskUserQuestion|ExitPlanMode)` 事件置 `pending_review`。

- [ ] **Step 1: 写失败测试**

`crates/meowo-reporter/tests/dispatch_test.rs` 末尾追加:

```rust
#[test]
fn permission_and_pretooluse_set_pending_review() {
    let store = Store::open_in_memory().unwrap();
    dispatch(&store, &ev(r#"{"hook_event_name":"SessionStart","session_id":"p1","cwd":"/p"}"#), 100).unwrap();

    let kind = |cc: &str| {
        store.live_sessions().unwrap().into_iter()
            .find(|l| l.session.cc_session_id == cc).unwrap().pending_review
    };

    // PermissionRequest:无 tool_name/普通工具 → approval。
    dispatch(&store, &ev(r#"{"hook_event_name":"PermissionRequest","session_id":"p1","tool_name":"Bash"}"#), 200).unwrap();
    assert_eq!(kind("p1").as_deref(), Some("approval"));
    // PermissionRequest:ExitPlanMode → plan。
    dispatch(&store, &ev(r#"{"hook_event_name":"PermissionRequest","session_id":"p1","tool_name":"ExitPlanMode"}"#), 210).unwrap();
    assert_eq!(kind("p1").as_deref(), Some("plan"));
    // PreToolUse:AskUserQuestion → question。
    dispatch(&store, &ev(r#"{"hook_event_name":"PreToolUse","session_id":"p1","tool_name":"AskUserQuestion"}"#), 220).unwrap();
    assert_eq!(kind("p1").as_deref(), Some("question"));
    // PreToolUse:其它工具 → 无操作(保持上一个 question)。
    dispatch(&store, &ev(r#"{"hook_event_name":"PreToolUse","session_id":"p1","tool_name":"Read"}"#), 230).unwrap();
    assert_eq!(kind("p1").as_deref(), Some("question"));
}
```

- [ ] **Step 2: 运行,确认失败**

Run: `cargo test -p meowo-reporter permission_and_pretooluse_set_pending_review`
Expected: FAIL——`pending_review` 始终为 None(两个 arm 落入 `_ => {}`)。

- [ ] **Step 3: 加两个 match arm**

`crates/meowo-reporter/src/dispatch.rs`,在 `"SessionEnd" => { ... }` 之后、`_ => {}` 之前插入。顶部确保 `use meowo_store::PendingReview;`(文件已 `use meowo_store::{Store, ...}`,补 `PendingReview`):

```rust
        "PermissionRequest" => {
            if let Some(sid) = lookup_session(store, ev)? {
                let kind = match ev.tool_name.as_deref() {
                    Some("ExitPlanMode") => PendingReview::Plan,
                    Some("AskUserQuestion") => PendingReview::Question,
                    _ => PendingReview::Approval,
                };
                store.set_pending_review(sid, kind, now_ms)?;
            }
        }
        "PreToolUse" => {
            if let Some(sid) = lookup_session(store, ev)? {
                let kind = match ev.tool_name.as_deref() {
                    Some("AskUserQuestion") => Some(PendingReview::Question),
                    Some("ExitPlanMode") => Some(PendingReview::Plan),
                    _ => None, // 安装侧已用 matcher 限定;这里再兜一层防御
                };
                if let Some(kind) = kind {
                    store.set_pending_review(sid, kind, now_ms)?;
                }
            }
        }
```

- [ ] **Step 4: 运行测试,确认通过**

Run: `cargo test -p meowo-reporter permission_and_pretooluse_set_pending_review`
Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add crates/meowo-reporter/src/dispatch.rs crates/meowo-reporter/tests/dispatch_test.rs
git commit -m "feat(reporter): dispatch 新增 PermissionRequest/PreToolUse 检测置 pending_review"
```

---

## Task 7: dispatch — 四分支加 clear_pending_review(下一个事件即清)

**Files:**
- Modify: `crates/meowo-reporter/src/dispatch.rs`(`PostToolUse`、`UserPromptSubmit`、`Stop`、`SessionEnd` 四分支)
- Test: `crates/meowo-reporter/tests/dispatch_test.rs`

**Interfaces:**
- Consumes: Task 2 的 `store.clear_pending_review`;Task 6 的置位逻辑。
- Produces: 置 pending 后,下一个 `PostToolUse`/`UserPromptSubmit`/`Stop`/`SessionEnd` 任一即把 `pending_review` 清回 NULL。

- [ ] **Step 1: 写失败测试**

`crates/meowo-reporter/tests/dispatch_test.rs` 末尾追加(参数化四种清除事件):

```rust
#[test]
fn pending_review_cleared_by_next_event() {
    for (i, clear_ev) in [
        r#"{"hook_event_name":"PostToolUse","session_id":"c1","tool_name":"Read"}"#,
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"c1","prompt":"继续"}"#,
        r#"{"hook_event_name":"Stop","session_id":"c1"}"#,
        r#"{"hook_event_name":"SessionEnd","session_id":"c1"}"#,
    ].iter().enumerate() {
        let store = Store::open_in_memory().unwrap();
        dispatch(&store, &ev(r#"{"hook_event_name":"SessionStart","session_id":"c1","cwd":"/p"}"#), 100).unwrap();
        dispatch(&store, &ev(r#"{"hook_event_name":"PermissionRequest","session_id":"c1","tool_name":"Bash"}"#), 200).unwrap();
        // 置位后确认非空。
        let pending = store.live_sessions().unwrap().into_iter()
            .find(|l| l.session.cc_session_id == "c1").unwrap().pending_review;
        assert_eq!(pending.as_deref(), Some("approval"), "case {i} 置位前提");
        // 下一个事件清除。
        dispatch(&store, &ev(clear_ev), 300).unwrap();
        let pending = store.live_sessions().unwrap().into_iter()
            .find(|l| l.session.cc_session_id == "c1").unwrap().pending_review;
        assert_eq!(pending, None, "case {i} 应被清除");
    }
}
```

- [ ] **Step 2: 运行,确认失败**

Run: `cargo test -p meowo-reporter pending_review_cleared_by_next_event`
Expected: FAIL(清除事件后 `pending_review` 仍为 `Some("approval")`)。

- [ ] **Step 3: 在四分支顶部插入 clear**

`crates/meowo-reporter/src/dispatch.rs`,在以下四个分支的 `if let Some(sid) = lookup_session(store, ev)? {` 之后**第一行**各插入 `store.clear_pending_review(sid)?;`:

`UserPromptSubmit`:
```rust
        "UserPromptSubmit" => {
            if let Some(sid) = lookup_session(store, ev)? {
                store.clear_pending_review(sid)?;
                if let Some(prompt) = ev.prompt.as_deref() {
```

`PostToolUse`:
```rust
        "PostToolUse" => {
            if let Some(sid) = lookup_session(store, ev)? {
                store.clear_pending_review(sid)?;
                match ev.tool_name.as_deref() {
```

`Stop`:
```rust
        "Stop" => {
            if let Some(sid) = lookup_session(store, ev)? {
                store.clear_pending_review(sid)?;
                store.set_session_status(sid, SessionStatus::Waiting, now_ms)?;
```

`SessionEnd`:
```rust
        "SessionEnd" => {
            if let Some(sid) = lookup_session(store, ev)? {
                store.clear_pending_review(sid)?;
                store.end_session(sid, now_ms)?;
            }
        }
```

注意:`PreToolUse`(Task 6 新增)**不**加 clear——它本身是置位事件;`PermissionRequest` 同理不加。

- [ ] **Step 4: 运行测试,确认通过**

Run: `cargo test -p meowo-reporter pending_review_cleared_by_next_event`
Expected: PASS。再跑 `cargo test -p meowo-reporter` 全量确认无回归。

- [ ] **Step 5: Commit**

```bash
git add crates/meowo-reporter/src/dispatch.rs crates/meowo-reporter/tests/dispatch_test.rs
git commit -m "feat(reporter): PostToolUse/UserPromptSubmit/Stop/SessionEnd 统一清除 pending_review"
```

---

## Task 8: analyze.rs fold_line 改拼接一条 assistant 的所有 text 块(B 兜底对齐)

**Files:**
- Modify: `crates/meowo-store/src/analyze.rs:122-152`(`fold_line` assistant 分支的 text 提取)
- Test: `crates/meowo-store/src/analyze.rs`(文件内 `#[cfg(test)] mod tests`,`:282` 起)

**Interfaces:**
- Produces: `analyze_transcript` 的 `preview`/`last_text` 在一条 assistant 含多个 text 块时,从「取第一个」改为「拼接全部(空格连接)」,对齐 moshi 与 spec B 主源。

> ⚠️ 风险说明(执行者须知):`last_text` 同时喂 `preview` 与 `classify_error`(错误检测)。spec B 非目标声明「不改错误检测」指不改其**机制**(仍 transcript classify_error),拼接后错误句仍在文本内,正常不影响匹配。Step 5 必须跑 `analyze.rs` 全部既有测试确认错误检测用例不回归;若某 error 用例因拼接而漂移,在该 task 内修正 `classify_error` 的匹配或测试预期,不要扩大范围。

- [ ] **Step 1: 写失败测试**

`crates/meowo-store/src/analyze.rs` 的 `mod tests` 内追加(紧邻 `analyze_exposes_last_assistant_preview`):

```rust
    #[test]
    fn analyze_concatenates_multiple_text_blocks_in_one_assistant() {
        let content = concat!(
            r#"{"type":"assistant","uuid":"u1","message":{"role":"assistant","content":[{"type":"text","text":"先说开场白"},{"type":"tool_use","id":"t","name":"Bash","input":{}},{"type":"text","text":"再说结论"}]}}"#, "\n",
        );
        let p = write_tmp("concat", content);
        let info = analyze_transcript(p.to_str().unwrap());
        std::fs::remove_file(&p).ok();
        assert_eq!(info.preview.as_deref(), Some("先说开场白 再说结论"));
    }
```

- [ ] **Step 2: 运行,确认失败**

Run: `cargo test -p meowo-store analyze_concatenates_multiple_text_blocks_in_one_assistant`
Expected: FAIL——现状取第一个 text 块,得 `Some("先说开场白")`。

- [ ] **Step 3: 改 text 提取为拼接**

`crates/meowo-store/src/analyze.rs:139-147`,把 `find_map(...)`(取第一个 text 块)替换为收集所有 text 块并以空格拼接:

```rust
                // 取该 assistant 消息 content 数组里所有 text 块,空格拼接(对齐 moshi);无 text 块则 None(如纯 tool_use)。
                let text = v
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                    .and_then(|arr| {
                        let joined = arr
                            .iter()
                            .filter(|x| x.get("type").and_then(|t| t.as_str()) == Some("text"))
                            .filter_map(|x| x.get("text").and_then(|t| t.as_str()))
                            .collect::<Vec<_>>()
                            .join(" ");
                        if joined.is_empty() { None } else { Some(joined) }
                    });
```

(其余 `if let Some(text) = text { ... self.last_text = Some((text, uuid)); }` 不变——仍只保留最后一条 assistant。)

- [ ] **Step 4: 运行新测试,确认通过**

Run: `cargo test -p meowo-store analyze_concatenates_multiple_text_blocks_in_one_assistant`
Expected: PASS。

- [ ] **Step 5: 跑 analyze 全量防回归**

Run: `cargo test -p meowo-store`
Expected: 全 PASS,特别确认 `analyze_exposes_last_assistant_preview`(单块场景拼接结果不变)、`analyze_skips_tooluse_only_assistant`、以及任何 `classify_error` 相关用例不回归。

- [ ] **Step 6: Commit**

```bash
git add crates/meowo-store/src/analyze.rs
git commit -m "fix(store): transcript 兜底取一条 assistant 的全部 text 块拼接,对齐主源"
```

---

## Task 9: ccsetup.rs matcher 感知 + HOOK_SPECS(8 条)

**Files:**
- Modify: `app/src-tauri/src/ccsetup.rs:9-16`(`HOOK_EVENTS` → `HOOK_SPECS`)、`:65-81`(`find_reporter_hook`)、`:83-123`(`ensure_hooks`)、`:270-430`(测试)
- Test: `app/src-tauri/src/ccsetup.rs` 文件尾 `#[cfg(test)] mod tests`

**Interfaces:**
- Produces: `ensure_hooks` 按 `(event, matcher)` 定位/追加 meowo-reporter 条目,新增 `PermissionRequest`(matcher `*`)与 `PreToolUse`(matcher `AskUserQuestion` / `ExitPlanMode`)三条,保留用户自有 `PreToolUse:Bash` 等非 meowo-reporter 条目。

- [ ] **Step 1: 写失败测试**

`app/src-tauri/src/ccsetup.rs` 的 `mod tests` 内追加两个用例:

```rust
    #[test]
    fn ensure_hooks_adds_all_specs_including_pretooluse_matchers() {
        let mut v = json!({});
        assert!(ensure_hooks(&mut v, "C:/x/meowo-reporter.exe"));
        // 5 个老事件 + PermissionRequest:matcher "*"。
        for e in ["SessionStart", "UserPromptSubmit", "PostToolUse", "Stop", "SessionEnd", "PermissionRequest"] {
            assert_eq!(v["hooks"][e][0]["matcher"], "*", "{e} matcher");
            assert_eq!(v["hooks"][e][0]["hooks"][0]["command"], "\"C:/x/meowo-reporter.exe\"");
        }
        // PreToolUse:两条,matcher 分别 AskUserQuestion / ExitPlanMode。
        let pre = v["hooks"]["PreToolUse"].as_array().unwrap();
        let matchers: Vec<&str> = pre.iter().map(|e| e["matcher"].as_str().unwrap()).collect();
        assert!(matchers.contains(&"AskUserQuestion"));
        assert!(matchers.contains(&"ExitPlanMode"));
        // 幂等。
        assert!(!ensure_hooks(&mut v, "C:/x/meowo-reporter.exe"));
    }

    #[test]
    fn ensure_hooks_preserves_user_pretooluse_bash() {
        // 用户自有 PreToolUse:Bash node 预检,不是 meowo-reporter。
        let mut v = json!({
            "hooks": { "PreToolUse": [
                { "matcher": "Bash", "hooks": [{ "type": "command", "command": "node \"x/pre-check.cjs\"" }] }
            ]}
        });
        ensure_hooks(&mut v, "C:/x/meowo-reporter.exe");
        let pre = v["hooks"]["PreToolUse"].as_array().unwrap();
        // 原 Bash 条目原封保留。
        let bash = pre.iter().find(|e| e["matcher"] == "Bash").unwrap();
        assert_eq!(bash["hooks"][0]["command"], "node \"x/pre-check.cjs\"");
        // 且新增了 AskUserQuestion / ExitPlanMode 两条 meowo-reporter。
        assert!(pre.iter().any(|e| e["matcher"] == "AskUserQuestion"));
        assert!(pre.iter().any(|e| e["matcher"] == "ExitPlanMode"));
    }
```

- [ ] **Step 2: 运行,确认失败**

Run: `cargo test -p meowo-app ensure_hooks_adds_all_specs_including_pretooluse_matchers`
Expected: FAIL/编译错误(`PermissionRequest`/`PreToolUse` 未被挂、`HOOK_SPECS` 不存在)。

- [ ] **Step 3: 替换 HOOK_EVENTS 为 HOOK_SPECS**

`app/src-tauri/src/ccsetup.rs:9-16`:

```rust
/// meowo-reporter 负责的 hook 事件 + matcher。PreToolUse 用 matcher 限定只在两种工具触发,
/// 与用户自有 PreToolUse(如 Bash 预检)按 matcher 区分共存。
const HOOK_SPECS: [(&str, &str); 8] = [
    ("SessionStart", "*"),
    ("UserPromptSubmit", "*"),
    ("PostToolUse", "*"),
    ("Stop", "*"),
    ("SessionEnd", "*"),
    ("PermissionRequest", "*"),
    ("PreToolUse", "AskUserQuestion"),
    ("PreToolUse", "ExitPlanMode"),
];
```

- [ ] **Step 4: 改 find_reporter_hook 为 matcher 感知(返回 entry)**

把 `find_reporter_hook`(`:65-81`)整体替换为:

```rust
/// 在某 hook 事件数组里找「matcher 等于 target_matcher 且含 meowo-reporter 命令」的 entry。
/// 返回整个 entry 的可变引用(用于更新其内部 hook 的路径)。matcher 感知:
/// 同一事件下可有多条按 matcher 区分的条目(如 PreToolUse 的 Bash 预检与本程序的 AskUserQuestion)。
fn find_reporter_entry_with_matcher<'a>(
    event_arr: &'a mut [Value],
    target_matcher: &str,
) -> Option<&'a mut Value> {
    for entry in event_arr.iter_mut() {
        if entry.get("matcher").and_then(|m| m.as_str()) != Some(target_matcher) {
            continue;
        }
        let has_reporter = entry
            .get("hooks")
            .and_then(|x| x.as_array())
            .into_iter()
            .flatten()
            .any(|h| {
                h.get("command")
                    .and_then(|x| x.as_str())
                    .and_then(reporter_exe_path)
                    .is_some()
            });
        if has_reporter {
            return Some(entry);
        }
    }
    None
}
```

- [ ] **Step 5: 改 ensure_hooks 遍历 HOOK_SPECS**

把 `ensure_hooks`(`:83-123`)的 `for event in HOOK_EVENTS { ... }` 循环体替换为按 `(event, matcher)` 处理:

```rust
    for (event, matcher) in HOOK_SPECS {
        let arr = settings["hooks"]
            .as_object_mut()
            .unwrap()
            .entry(event.to_string())
            .or_insert_with(|| json!([]));
        let arr = match arr.as_array_mut() {
            Some(a) => a,
            None => {
                *arr = json!([]);
                arr.as_array_mut().unwrap()
            }
        };
        match find_reporter_entry_with_matcher(arr, matcher) {
            Some(entry) => {
                // 升级该 entry 内 meowo-reporter hook 的路径(matcher 不动)。
                if let Some(hs) = entry.get_mut("hooks").and_then(|x| x.as_array_mut()) {
                    for h in hs.iter_mut() {
                        let is_reporter = h
                            .get("command")
                            .and_then(|x| x.as_str())
                            .and_then(reporter_exe_path)
                            .is_some();
                        if is_reporter
                            && h.get("command").and_then(|x| x.as_str()) != Some(desired_cmd.as_str())
                        {
                            h["command"] = json!(desired_cmd);
                            changed = true;
                        }
                    }
                }
            }
            None => {
                arr.push(json!({
                    "matcher": matcher,
                    "hooks": [{ "type": "command", "command": desired_cmd, "timeout": 5 }]
                }));
                changed = true;
            }
        }
    }
```

(`ensure_hooks` 签名、`desired_cmd`/`changed` 初始化、`hooks` 对象兜底等其余不变。)

- [ ] **Step 6: 修旧测试引用**

`ccsetup.rs` 的 `mod tests` 里:
- `ensure_hooks_adds_all_events_when_empty`(`:294`)的 `for e in HOOK_EVENTS` 改为 `for e in ["SessionStart","UserPromptSubmit","PostToolUse","Stop","SessionEnd"]`(只断言这 5 个老事件的 `[0]` 条目),或直接删除该用例(已被 Step 1 新用例覆盖)。
- `reporter_exe_path_strict_matches_only_our_exe`(`:274`)末尾的 `find_reporter_hook(&mut arr)` 改为 `find_reporter_entry_with_matcher(&mut arr, "*")`(arr 内 entry 无 matcher 字段 → 不匹配 `*` → 仍返回 `None`,断言不变)。
- `real_shape_user_settings_merge`(`:381` 起):**首要**改其幂等断言——该用例首行 `assert!(!ensure_hooks(&mut v, ccr));`(`:398`,fixture 只含 PreToolUse(Bash) + 5 个 meowo-reporter 事件,无 PermissionRequest / PreToolUse matcher 条目)在 Task 9 后会因新增 3 条而返回 `true`,故 `!ensure_hooks` 必 FAIL。把该行从 `assert!(!ensure_hooks(&mut v, ccr));` 翻转为 `assert!(ensure_hooks(&mut v, ccr));`(首次调用追加 PermissionRequest + AskUserQuestion + ExitPlanMode 三条,返回 true);随后断言 PreToolUse 下原 Bash 条目原封保留、AskUserQuestion/ExitPlanMode 两条已追加;末尾可再 `assert!(!ensure_hooks(&mut v, ccr));` 验证此时才幂等。
- `dryrun_against_copy`(`:366`,带 `#[ignore]`,正常 `cargo test` 不跑):第 `:375-376` 行打印注释 `=== PreToolUse(应原封不动)===` 语义已过时(现会追加 meowo-reporter matcher 条目)。可选:把注释改为「Bash 条目保留 + 新增 meowo-reporter matcher 条目」,或因 `#[ignore]` 不影响 CI 而保留不动。

- [ ] **Step 7: 运行测试,确认通过**

Run: `cargo test -p meowo-app ensure_hooks_adds_all_specs_including_pretooluse_matchers ensure_hooks_preserves_user_pretooluse_bash`
Expected: PASS。再跑 `cargo test -p meowo-app ccsetup`(模块全部用例)确认无回归。

- [ ] **Step 8: Commit**

```bash
git add app/src-tauri/src/ccsetup.rs
git commit -m "feat(ccsetup): hook 接线改 matcher 感知,新增 PermissionRequest/PreToolUse(AskUserQuestion|ExitPlanMode)"
```

---

## Task 10: install-hooks.mjs 同步 (event,matcher) 维度

**Files:**
- Modify: `scripts/install-hooks.mjs:33`(`EVENTS`)、`:34-49`(去重 + 追加)
- Test: 该脚本无单测;Step 4 用 `node --check` + 手动 dry-run 校验(见下)

**Interfaces:**
- Produces: node 安装脚本与 `ccsetup.rs` 的 `HOOK_SPECS` 一致,去重按 `(command, matcher)` 双键。

- [ ] **Step 1: 替换 EVENTS 为 SPECS**

`scripts/install-hooks.mjs:33`:

```javascript
const SPECS = [
  ["SessionStart", "*"],
  ["UserPromptSubmit", "*"],
  ["PostToolUse", "*"],
  ["Stop", "*"],
  ["SessionEnd", "*"],
  ["PermissionRequest", "*"],
  ["PreToolUse", "AskUserQuestion"],
  ["PreToolUse", "ExitPlanMode"],
];
```

- [ ] **Step 2: 改循环为 (event,matcher) + 双键去重**

`scripts/install-hooks.mjs:36-49` 的 `for (const event of EVENTS) { ... }` 替换为:

```javascript
for (const [event, matcher] of SPECS) {
  settings.hooks[event] ??= [];
  // 幂等识别:只移除「command 完全相同 且 matcher 相同」的旧条目,
  // 避免同事件多 matcher 条目互相误删(如 PreToolUse 的 AskUserQuestion 与 ExitPlanMode)。
  settings.hooks[event] = settings.hooks[event].filter(
    (entry) =>
      !(entry.matcher === matcher && (entry.hooks ?? []).some((h) => h.command === command)),
  );
  settings.hooks[event].push({
    matcher,
    hooks: [{ type: "command", command, timeout: 5 }],
  });
}
```

并改收尾日志(`scripts/install-hooks.mjs:52`)——它也引用了被改名的 `EVENTS`,否则脚本**运行期**抛 `ReferenceError: EVENTS is not defined`(而 Step 3 的 `node --check` 只查语法、抓不到)。把:

```javascript
console.log(`已写入 ${settingsPath}，挂载事件: ${EVENTS.join(", ")}`);
```

改为:

```javascript
console.log(`已写入 ${settingsPath}，挂载: ${SPECS.map(([e, m]) => `${e}:${m}`).join(", ")}`);
```

- [ ] **Step 3: 语法检查**

Run: `node --check scripts/install-hooks.mjs`
Expected: 无输出(语法正确)。

- [ ] **Step 4: 手动 dry-run 校验(可选但推荐)**

若脚本支持指向临时 settings(查看脚本是否读 `CLAUDE_CONFIG_DIR` 或参数),用一份临时 `settings.json` 跑一次,确认产出含 8 条 spec、PreToolUse 下两条 matcher 条目、且重复跑幂等。无 dry-run 入口则跳过,依赖 Task 9 的 Rust 侧 `ensure_hooks` 测试作为等价逻辑保证。

- [ ] **Step 5: Commit**

```bash
git add scripts/install-hooks.mjs
git commit -m "feat(scripts): install-hooks 同步 PermissionRequest/PreToolUse,去重加 matcher 维度"
```

---

## Task 11: settings.rs tr() 加 3 条 pending 通知文案

**Files:**
- Modify: `app/src-tauri/src/settings.rs:99-113`(`tr` 函数 match 臂)
- Test: `app/src-tauri/src/settings.rs`(若文件有 `#[cfg(test)] mod tests` 则加;否则靠 Task 12 间接覆盖 + 编译保证)

**Interfaces:**
- Produces: `tr(lang, key)` 支持 `notify.pending.approval` / `notify.pending.question` / `notify.pending.plan`(en + zh 兜底)。Task 12 消费。

- [ ] **Step 1: 加 match 臂**

`app/src-tauri/src/settings.rs:99-113`,在 `tr` 的 `match (lang, key)` 中,`("en", "notify.waiting")` 之后加三条 en,`(_, "notify.waiting")` 之后加三条 zh:

```rust
        ("en", "notify.error") => "Session error",
        ("en", "notify.waiting") => "Waiting for your reply",
        ("en", "notify.pending.approval") => "Approve a tool call?",
        ("en", "notify.pending.question") => "Claude is asking you a question",
        ("en", "notify.pending.plan") => "Plan awaiting approval",
        ("en", "tray.settings") => "Settings",
```

```rust
        (_, "notify.error") => "会话出错",
        (_, "notify.waiting") => "等待你回复",
        (_, "notify.pending.approval") => "需要你批准工具调用",
        (_, "notify.pending.question") => "Claude 在问你问题",
        (_, "notify.pending.plan") => "计划待批准",
        (_, "tray.settings") => "设置",
```

(把新条目插在已有对应 en/zh 块内即可,保持其余臂不变;末尾 `_ => ""` 兜底不变。)

- [ ] **Step 2: 编译确认**

Run: `cargo build -p meowo-app`
Expected: 编译通过(新增 match 臂不破坏穷尽性——`tr` 用字符串 match + `_` 兜底)。

- [ ] **Step 3: Commit**

```bash
git add app/src-tauri/src/settings.rs
git commit -m "feat(i18n): tr 增加待审批/待问题/待计划三条通知文案(中英)"
```

---

## Task 12: lib.rs pending_fingerprint + waiting_fingerprint 改签名 + spawn_liveness_watch pending 通知 + 计数口径

**Files:**
- Modify: `app/src-tauri/src/lib.rs:1002-1019`(`waiting_fingerprint` 加 `has_pending` 参数 + 新增 `pending_fingerprint`)、`:1079-1201`(`spawn_liveness_watch`)、`:1134`(计数口径)、`:2013-2034`(纯函数测试)
- Test: `app/src-tauri/src/lib.rs` 内联测试

**Interfaces:**
- Consumes: Task 1 的 `LiveSession.pending_review`;Task 11 的 `tr` 三条 key;现有 `should_notify` / `show_session_notification`。
- Produces:
  - `fn pending_fingerprint(errored: bool, pending_review: Option<&str>, last_event_at: i64) -> Option<String>`。
  - `fn waiting_fingerprint(errored: bool, has_pending: bool, status: &str, last_event_at: i64) -> Option<String>`(签名变更)。
  - `spawn_liveness_watch` 第三张 map `notified_pending`,判定优先级 error > pending > waiting,计数口径含 pending。

- [ ] **Step 1: 写/改失败测试**

`app/src-tauri/src/lib.rs` 内联测试(`:2013` 附近),新增 `pending_fingerprint` 测试,并更新既有 `waiting_fingerprint_rules`:

```rust
    #[test]
    fn pending_fingerprint_rules() {
        // errored 优先 → None(让位错误)。
        assert_eq!(pending_fingerprint(true, Some("approval"), 100), None);
        // pending 为 Some 且未出错 → Some("{kind}:{last_event_at}")。
        assert_eq!(pending_fingerprint(false, Some("question"), 100).as_deref(), Some("question:100"));
        // 无 pending → None。
        assert_eq!(pending_fingerprint(false, None, 100), None);
        // 指纹随 last_event_at 变化(新回合新指纹)。
        assert_ne!(pending_fingerprint(false, Some("approval"), 100), pending_fingerprint(false, Some("approval"), 200));
    }

    #[test]
    fn waiting_fingerprint_rules() {
        // 错误优先:无指纹。
        assert_eq!(waiting_fingerprint(true, false, "waiting", 100), None);
        // pending 优先:无 waiting 指纹(让位 pending)。
        assert_eq!(waiting_fingerprint(false, true, "waiting", 100), None);
        // 纯 waiting:用 last_event_at 作指纹。
        assert_eq!(waiting_fingerprint(false, false, "waiting", 100).as_deref(), Some("100"));
        // 非 waiting 状态:None。
        assert_eq!(waiting_fingerprint(false, false, "running", 100), None);
    }
```

- [ ] **Step 2: 运行,确认失败**

Run: `cargo test -p meowo-app pending_fingerprint_rules waiting_fingerprint_rules`
Expected: 编译错误(`pending_fingerprint` 未定义;`waiting_fingerprint` 参数数不符)。

- [ ] **Step 3: 改 waiting_fingerprint + 加 pending_fingerprint**

`app/src-tauri/src/lib.rs:1013-1019`,把 `waiting_fingerprint` 替换并在其后新增 `pending_fingerprint`:

```rust
/// 待交互通知指纹:errored 或 has_pending 时不发(None,让位错误/待审批);
/// status==waiting 且无错无 pending 时用 last_event_at 作指纹;其它状态 None。纯函数。
fn waiting_fingerprint(errored: bool, has_pending: bool, status: &str, last_event_at: i64) -> Option<String> {
    if errored || has_pending || status != "waiting" {
        None
    } else {
        Some(last_event_at.to_string())
    }
}

/// 待审批通知指纹:errored 时 None(错误优先);pending 为 Some(kind) 时 "{kind}:{last_event_at}";
/// 否则 None。纯函数,便于单测。
fn pending_fingerprint(errored: bool, pending_review: Option<&str>, last_event_at: i64) -> Option<String> {
    if errored {
        return None;
    }
    pending_review.map(|kind| format!("{kind}:{last_event_at}"))
}
```

并把新函数加进内联测试模块的具名导入(**否则 Step 5 的 `cargo test` 编译失败 `cannot find function 'pending_fingerprint' in this scope`**):lib.rs 的 `#[cfg(test)] mod tests` 用的是显式 `use super::{...}`(现状 `lib.rs:1757-1760`,内含 `... should_notify, strip_jsonc_comments, tab_match_score, waiting_fingerprint };`,只导入了 `waiting_fingerprint`)。在该列表中追加 `pending_fingerprint`(如改为 `... pending_fingerprint, should_notify, strip_jsonc_comments, tab_match_score, waiting_fingerprint };`)。`waiting_fingerprint` 已在列表中,改 4 参签名无需动导入。

- [ ] **Step 4: 改 spawn_liveness_watch**

(a) 在 map 声明处(`:1088` 附近,`notified_waiting` 之后)加第三张 map:

```rust
        let mut notified_pending: HashMap<String, String> = HashMap::new(); // cc_session_id -> 上次待审批指纹
```

(b) 计数口径(`:1134`)扩展:

```rust
                    // 菜单栏摘要计数:出错/待交互/待审批 → 需关注(●),运行中 → ○;在线空闲不计入。
                    if error.is_some() || s.session.status == "waiting" || s.pending_review.is_some() {
                        tray_waiting += 1;
                    } else if s.session.status == "running" {
                        tray_running += 1;
                    }
```

(c) 在错误通知块之后、待交互通知块之前,插入 pending 通知块:

```rust
                    // 待审批通知(错误之后、待交互之前;errored 时 pending_fingerprint 返回 None 自动让位)。
                    match pending_fingerprint(error.is_some(), s.pending_review.as_deref(), s.session.last_event_at) {
                        Some(fp) => {
                            let prev = notified_pending.get(&sid).map(|s| s.as_str());
                            if seeded && notify_on && should_notify(prev, Some(&fp)) {
                                let key = match s.pending_review.as_deref() {
                                    Some("question") => "notify.pending.question",
                                    Some("plan") => "notify.pending.plan",
                                    _ => "notify.pending.approval",
                                };
                                show_session_notification(
                                    &app,
                                    tr(lang, key).into(),
                                    format!("{} · {}", s.project_name, display_title),
                                    pid,
                                    display_title.clone(),
                                );
                            }
                            notified_pending.insert(sid.clone(), fp);
                        }
                        None => {
                            notified_pending.remove(&sid);
                        }
                    }
```

(d) 待交互通知块的 `waiting_fingerprint(...)` 调用(`:1158` 附近)加 `has_pending` 实参:

```rust
                    match waiting_fingerprint(error.is_some(), s.pending_review.is_some(), &s.session.status, s.session.last_event_at) {
```

(e) retain 处(`:1180` 附近)加一行:

```rust
                notified_pending.retain(|k, _| present.contains_key(k));
```

- [ ] **Step 5: 运行测试,确认通过**

Run: `cargo test -p meowo-app pending_fingerprint_rules waiting_fingerprint_rules`
Expected: PASS。再跑 `cargo test -p meowo-app` 全量 + `cargo build -p meowo-app` 确认 `spawn_liveness_watch` 编译通过、无回归。

- [ ] **Step 6: Commit**

```bash
git add app/src-tauri/src/lib.rs
git commit -m "feat(app): liveness 通知加待审批级别(error>pending>waiting)与计数口径"
```

---

## Task 13: api.ts LiveSession 加 3 字段

**Files:**
- Modify: `app/src/api.ts:65-90`(`LiveSession` 类型)
- Test: `cd app && bunx tsc --noEmit`(类型即测试)

**Interfaces:**
- Produces: TS `LiveSession` 新增 `pending_review`、`last_ai_text`、`last_user_text`(后续前端 task 消费)。

- [ ] **Step 1: 加字段**

`app/src/api.ts`,在 `LiveSession` 类型的 `context_window` 字段之后、`}` 之前追加:

```ts
  /** 待审批子态:回合中途等用户介入(批准工具/回答提问/批准计划);无则 null。 */
  pending_review: "approval" | "question" | "plan" | null;
  /** 最近一条 AI 正文(锚 Stop hook);无则 null,卡片回退 preview。 */
  last_ai_text: string | null;
  /** 最近一条用户消息(锚 UserPromptSubmit);独立字段,不被工具活动覆盖。 */
  last_user_text: string | null;
```

- [ ] **Step 2: 类型检查通过**

Run: `cd app && bunx tsc --noEmit`
Expected: 通过(新增可空字段不破坏现有代码;`Sticker.test.tsx` 的 `mk()` 用 `as Item` 强转,不报缺字段)。

- [ ] **Step 3: Commit**

```bash
git add app/src/api.ts
git commit -m "feat(web): LiveSession 类型加 pending_review/last_ai_text/last_user_text"
```

---

## Task 14: i18n zh/en 加 pending 标签 + 你/AI 前缀

**Files:**
- Modify: `app/src/i18n/zh.ts`、`app/src/i18n/en.ts`
- Test: `app/src/i18n/i18n.test.ts`(运行时 key 一致 + arity)+ `bunx tsc --noEmit`

**Interfaces:**
- Produces: 字典新增 `pending: { approval, question, plan }`(纯字符串)、`sticker.youPrefix`、`sticker.aiPrefix`。Task 15/16/17 消费。

- [ ] **Step 1: 给 zh.ts 加 key**

`app/src/i18n/zh.ts`,在 `sticker` 段内(`previewMark` 附近)加两条前缀,并在顶层(`badge` 之后)加 `pending` 段:

```ts
  pending: { approval: "待批准", question: "待回答", plan: "待批计划" },
```

`sticker` 段内追加:

```ts
    youPrefix: "你",
    aiPrefix: "AI",
```

- [ ] **Step 2: 给 en.ts 加对应 key(同结构)**

`app/src/i18n/en.ts` 同位置:

```ts
  pending: { approval: "Approve", question: "Question", plan: "Review plan" },
```

```ts
    youPrefix: "You",
    aiPrefix: "AI",
```

- [ ] **Step 3: 运行 i18n 测试 + 类型检查**

Run: `cd app && bun run test i18n && bunx tsc --noEmit`
Expected: `i18n.test.ts` PASS(zh/en key 集合一致、arity 一致),tsc 通过(`Dict = typeof zh`,en 缺/多 key 会编译报错)。

- [ ] **Step 4: Commit**

```bash
git add app/src/i18n/zh.ts app/src/i18n/en.ts
git commit -m "feat(i18n): 字典加待审批子态标签与你/AI 前缀(中英)"
```

---

## Task 15: Sticker.tsx match() 纳入 pending + 排序置顶 + 计数

**Files:**
- Modify: `app/src/views/Sticker.tsx:231-245`(`match`)、`:485-517`(`shown` 排序)
- Test: `app/src/views/Sticker.test.tsx`

**Interfaces:**
- Consumes: Task 13 的 `pending_review`。
- Produces: pending 会话(`status==="running"` 但 `pending_review!=null`)归入「待交互」tab 并整组置顶,组内仍按 `last_event_at` 升序。

- [ ] **Step 1: 写失败测试**

`app/src/views/Sticker.test.tsx` 末尾追加:

```tsx
  it("pending_review 会话归入待交互并置顶", () => {
    localStorage.setItem("meowo-tab", "waiting");
    const sess = (id: number, cc: string, status: "running" | "waiting", last: number) =>
      ({ id, project_id: 1, cc_session_id: cc, status, started_at: 0, last_event_at: last, ended_at: null });
    const now = Date.now();
    const items = [
      mk({ task_title: "等待最久的纯waiting", connected: true, session: sess(1, "w1", "waiting", now - 600_000) }),
      mk({ task_title: "待批准", connected: true, pending_review: "approval", session: sess(2, "p1", "running", now - 60_000) }),
    ];
    const { container } = render(<Sticker data={items} />);
    // 待交互 tab 计数含 pending(2)。
    const waitingTab = screen.getByText(zh.tabs.waiting).closest(".stab")!;
    expect(waitingTab.querySelector(".stab-n")!.textContent).toBe("2");
    // pending 组置顶:第一张卡是「待批准」,即便它 last_event_at 更晚。
    const cards = container.querySelectorAll(".stk-card");
    expect(cards[0].querySelector(".stk-title")?.textContent).toBe("待批准");
  });
```

- [ ] **Step 2: 运行,确认失败**

Run: `cd app && bun run test Sticker`
Expected: 新用例 FAIL(pending 会话当前 `status==="running"` 不进 waiting tab;计数为 1,首卡非「待批准」)。

- [ ] **Step 3: 改 match() 待交互归类**

`app/src/views/Sticker.tsx:242`,把 waiting 分支改为也纳入 pending:

```tsx
  if (tab === "waiting") return l.connected && (l.session.status === "waiting" || l.errored || l.pending_review != null);
  if (tab === "running") return l.connected && l.session.status === "running" && !l.errored && l.pending_review == null;
```

(running 分支同步排除 pending——pending 会话不再算"纯运行中"。)

- [ ] **Step 4: 改 shown 排序加置顶键**

`app/src/views/Sticker.tsx:513-515`(`if (star !== 0) return star;` → waiting 单行 → `return 0;`),把 waiting tab 的排序回调扩展为「pending 组优先 → 组内 last_event_at 升序」:

```tsx
        if (star !== 0) return star;
        if (tab === "waiting") {
          const ap = a.pending_review != null ? 0 : 1;
          const bp = b.pending_review != null ? 0 : 1;
          if (ap !== bp) return ap - bp; // pending 整组置顶
          return a.session.last_event_at - b.session.last_event_at; // 组内等最久优先
        }
        return 0;
```

- [ ] **Step 5: 运行测试,确认通过**

Run: `cd app && bun run test Sticker`
Expected: 新用例 PASS;既有用例(`待交互标签页按等待最久优先排序`、`errored 会话归入待交互`)不回归。

- [ ] **Step 6: Commit**

```bash
git add app/src/views/Sticker.tsx app/src/views/Sticker.test.tsx
git commit -m "feat(web): pending 会话归入待交互 tab 并整组置顶"
```

---

## Task 16: Sticker.tsx indicator pending 徽标 + 醒目 pill + CSS

**Files:**
- Modify: `app/src/views/Sticker.tsx:186-214`(`RunBadge` tone)、`:648-658`(`indicator`)、`:710-711`(line1 渲染区 `<span className="stk-title">` 与 `<span className="stk-time">` 之间,插 pill)
- Modify: `app/src/styles.css:475`(加 `.run-badge--pending`)、`:593`(加 `.pending-pill`)
- Test: `app/src/views/Sticker.test.tsx`

**Interfaces:**
- Consumes: Task 13 `pending_review`、Task 14 `t.pending[kind]`。
- Produces: pending 会话 indicator 显示琥珀 `RunBadge`,标题行显示琥珀脉冲文字 pill(待批准/待回答/待批计划)。

- [ ] **Step 1: 写失败测试**

`app/src/views/Sticker.test.tsx` 末尾追加:

```tsx
  it("pending 会话显示琥珀 pill 与 pending 徽标", () => {
    const item = mk({
      task_title: "审批中",
      connected: true,
      pending_review: "approval",
      context_pct: 30,
      session: { id: 5, project_id: 1, cc_session_id: "pp", status: "running", started_at: 0, last_event_at: Date.now(), ended_at: null },
    });
    const { container } = render(<Sticker data={[item]} />);
    expect(screen.getByText(zh.pending.approval)).toBeTruthy();     // pill 文字「待批准」
    expect(container.querySelector(".pending-pill")).toBeTruthy();  // pill 元素
    expect(container.querySelector(".run-badge--pending")).toBeTruthy(); // 琥珀徽标
  });
```

- [ ] **Step 2: 运行,确认失败**

Run: `cd app && bun run test Sticker`
Expected: 新用例 FAIL(无 `.pending-pill` / `.run-badge--pending`)。

- [ ] **Step 3: RunBadge 加 pending tone**

`app/src/views/Sticker.tsx:191-196`,把 `tone` 类型与 className 扩展:

```tsx
function RunBadge({
  pct,
  tone = "running",
}: {
  pct: number | null;
  tone?: "running" | "waiting" | "pending";
}) {
```

className 行(`:206`)改为按 tone 拼:

```tsx
      className={"run-badge" + (tone === "waiting" ? " run-badge--waiting" : tone === "pending" ? " run-badge--pending" : "")}
```

(`what`/`label` 取 `tone === "waiting" ? t.badge.waiting : t.badge.running` 可保留——pending 的中心圆仍显示 pct,语义文字非关键;无需新增 badge 文案。)

- [ ] **Step 4: indicator 加 pending 分支**

`app/src/views/Sticker.tsx:648-658`,在 `errored` 分支之后、`running` 分支之前插入 pending:

```tsx
            const indicator = !l.connected ? (
              <span className="ring-stop" title={t.sticker.stopped} />
            ) : l.errored ? (
              <span className="needs-error" title={l.error_raw ?? t.sticker.sessionError} />
            ) : l.pending_review ? (
              <RunBadge pct={l.context_pct} tone="pending" />
            ) : l.session.status === "running" ? (
              <RunBadge pct={l.context_pct} />
            ) : l.session.status === "waiting" ? (
              <RunBadge pct={l.context_pct} tone="waiting" />
            ) : (
              <span className="sdot sdot-on" title={t.sticker.online} />
            );
```

- [ ] **Step 5: 标题行插入 pill**

`app/src/views/Sticker.tsx`,在 `.stk-line1` 的标题 span 与时间 span 之间(`:710-711`,即 `<span className="stk-title">{title}</span>` 与 `<span className="stk-time">…</span>` 之间;注意 `:565-566` 是 styles.css 行号、`Sticker.tsx:565` 实为 syncSb 函数,勿在那里找)插入 pending pill(仅 pending 时渲染):

```tsx
                {l.pending_review && (
                  <span className={"pending-pill pending-" + l.pending_review}>
                    {t.pending[l.pending_review]}
                  </span>
                )}
```

(`t.pending[l.pending_review]`:`l.pending_review` 为 `"approval"|"question"|"plan"`,与 Task 14 字典 key 对齐。)

- [ ] **Step 6: 加 CSS**

`app/src/styles.css`,在 `.run-badge--waiting { ... }`(`:480`)之后加 pending 徽标色:

```css
.run-badge--pending {
  --bc: #f0883e; /* 琥珀:区别于运行绿/待交互黄/错误红 */
  --bc-track: rgba(240, 136, 62, 0.22);
  --bc-fade: rgba(240, 136, 62, 0);
  --bc-text: #2a1402;
}
```

在 `.stk-sub-err`(`:593`)之后加 pill 样式:

```css
/* 待审批醒目 pill:琥珀脉冲,标题行内 */
.pending-pill {
  flex: none;
  font-size: calc(9.5px * var(--cc-ui));
  font-weight: 700;
  padding: 1px 7px;
  border-radius: 999px;
  color: #2a1402;
  background: #f0883e;
  animation: needs-pulse-pending 1.6s ease-out infinite;
}
@keyframes needs-pulse-pending {
  0% { box-shadow: 0 0 0 0 rgba(240, 136, 62, 0.5); }
  70% { box-shadow: 0 0 0 5px rgba(240, 136, 62, 0); }
  100% { box-shadow: 0 0 0 0 rgba(240, 136, 62, 0); }
}
```

- [ ] **Step 7: 运行测试,确认通过**

Run: `cd app && bun run test Sticker && bunx tsc --noEmit`
Expected: PASS。

- [ ] **Step 8: Commit**

```bash
git add app/src/views/Sticker.tsx app/src/styles.css app/src/views/Sticker.test.tsx
git commit -m "feat(web): pending 会话显示琥珀徽标与醒目 pill(待批准/待回答/待批计划)"
```

---

## Task 17: Sticker.tsx 活动行加用户消息行 + AI 行优先 last_ai_text

**Files:**
- Modify: `app/src/views/Sticker.tsx:638-647`(`sub` 计算)、`:787-801`(活动行渲染)
- Modify: `app/src/styles.css:576`(加 `.stk-userrow` / `.stk-msg-tag`)
- Test: `app/src/views/Sticker.test.tsx`

**Interfaces:**
- Consumes: Task 13 `last_ai_text` / `last_user_text`、Task 14 `t.sticker.youPrefix`。
- Produces: 卡片 AI 活动行优先 `last_ai_text ?? preview`;新增用户消息行(带「你」前缀),`last_user_text` 存在时显示。

- [ ] **Step 1: 写失败测试**

`app/src/views/Sticker.test.tsx` 末尾追加:

```tsx
  it("卡片优先显示 last_ai_text,并显示用户消息行", () => {
    const item = mk({
      connected: true,
      preview: "transcript 兜底的旧预览",
      last_ai_text: "调研完成,结论更微妙",
      last_user_text: "切到这个任务",
      session: { id: 7, project_id: 1, cc_session_id: "uai", status: "waiting", started_at: 0, last_event_at: Date.now(), ended_at: null },
    });
    render(<Sticker data={[item]} />);
    expect(screen.getByText("调研完成,结论更微妙")).toBeTruthy(); // AI 行用 last_ai_text 而非 preview
    expect(screen.queryByText("transcript 兜底的旧预览")).toBeNull();
    expect(screen.getByText("切到这个任务")).toBeTruthy();         // 用户消息行
    expect(screen.getByText(zh.sticker.youPrefix)).toBeTruthy();   // 「你」前缀
  });
```

- [ ] **Step 2: 运行,确认失败**

Run: `cd app && bun run test Sticker`
Expected: FAIL(当前 AI 行用 `preview`,无用户行)。

- [ ] **Step 3: 改 sub 数据源为 last_ai_text 优先**

`app/src/views/Sticker.tsx:642-644`,把 `sub` 的 preview 源改为 `last_ai_text ?? preview`:

```tsx
            const sub = l.errored && l.error_label
              ? t.errorLabels[l.error_label] ?? l.error_label
              : previewEnabled && (l.last_ai_text ?? l.preview)
              ? (l.last_ai_text ?? l.preview)
              : null;
```

- [ ] **Step 4: 在活动行区插入用户消息行**

`app/src/views/Sticker.tsx`,在 `.stk-subrow`(AI/错误行,`:788` 附近)之前插入用户消息行(仅 `last_user_text` 存在时):

```tsx
                {l.last_user_text && (
                  <div className="stk-subrow stk-userrow">
                    <span className="stk-msg-tag">{t.sticker.youPrefix}</span>
                    <span className="stk-sub" title={l.last_user_text}>{l.last_user_text}</span>
                  </div>
                )}
```

(注意外层渲染条件 `(sub || (buttonMode && canOpen(l)))` 只控制 AI 行;用户行独立条件 `l.last_user_text`,放在该块之前同级位置。)

- [ ] **Step 5: 加 CSS**

`app/src/styles.css`,在 `.stk-sub`(`:576`)之后加:

```css
.stk-userrow { margin-top: 5px; }
.stk-msg-tag { flex: none; font-size: calc(9.5px * var(--cc-ui)); font-weight: 600; color: var(--cc-text-dim); }
```

- [ ] **Step 6: 运行测试,确认通过**

Run: `cd app && bun run test Sticker && bunx tsc --noEmit`
Expected: PASS;既有活动行用例不回归。

- [ ] **Step 7: Commit**

```bash
git add app/src/views/Sticker.tsx app/src/styles.css app/src/views/Sticker.test.tsx
git commit -m "feat(web): 卡片优先显示 last_ai_text 并新增用户消息行"
```

---

## 收尾验证(全部 task 完成后)

- [ ] **Rust 全量 + clippy**

```bash
cargo test -p meowo-store && cargo test -p meowo-reporter && cargo test -p meowo-app
cargo clippy --all-targets
```
Expected: 测试全 PASS,clippy 无 error(CI 会卡 clippy)。

- [ ] **前端全量 + 类型 + 构建**

```bash
cd app && bun run test && bunx tsc --noEmit && bun run build
```
Expected: vitest 全 PASS、tsc 无错、build 成功(产物不含 demo 入口)。

- [ ] **端到端冒烟(可选,本机)**

启动 app,在终端跑一个会触发 `AskUserQuestion` / 计划批准 / 工具批准的 Claude Code 会话,确认:卡片出现琥珀 pill、排到「待交互」tab 顶部、弹一条去重通知;用户回答/批准后 pill 消失;卡片「你/AI」两行显示正确。注:macOS 编译与菜单栏计数仅能靠 CI 验(本机 Windows),提 PR 触发 CI。

---

## 实现说明与已知风险

1. **新列落点**:三列加在 `LiveSession` 顶层而非内嵌 `Session`,与现状 `pid`/`cwd`/`archived_at` 一致,避免波及 `get_session` 与 dispatch 测试的 `Session{}` 构造。前端对应 `l.pending_review`(非 `l.session.pending_review`)。
2. **`LiveItem` 无需改**:两份 spec 字面都说"`LiveItem` 加字段透传",但 `LiveItem`(`lib.rs:96`)用 `#[serde(flatten)] inner: LiveSession`——Task 1 给 `LiveSession` 加的三字段会**自动 flatten 序列化到前端**,不必给 `LiveItem` struct 新增字段。`spawn_liveness_watch` 直接读 `s.pending_review`(`s` 即 `LiveSession`)。**AI 展示优先级 `last_ai_text ?? preview` 在前端做**(Task 17):`LiveItem` 已有独立 `preview` 字段(由 `analyze` 计算),前端同时拿到 `last_ai_text`(flatten)与 `preview`,做 `??` 即可,后端不改优先级逻辑。
3. **Task 8 的 error 输入变化**:`fold_line` 改拼接后,`classify_error` 收到的是拼接文本(错误句仍在内)。已在 Task 8 风险框标注,Step 5 用全量测试守护;若回归在该 task 内最小化修正。
4. **通知文案在 Rust 侧**:前端 i18n 无 notification segment,故 pending 通知文案走 `settings.rs::tr`(Task 11),前端字典只加 pill 标签(Task 14)。
5. **macOS 盲区**:本机 Windows,macOS 菜单栏计数(`update_tray_status`)与编译只能靠 CI;特性分支须开 PR 才触发 macOS 编译。计数口径改动(Task 12)同时覆盖 mac/win,逻辑在 `lib.rs`,menubar.rs 不需改。
6. **交付**:两特性合并在一条流水线,但 commit 粒度按 task 切分,可分段 review。建议整体走一个 PR(CI 验 macOS 编译 + clippy);发版不在本 plan 范围,由用户另行决定。

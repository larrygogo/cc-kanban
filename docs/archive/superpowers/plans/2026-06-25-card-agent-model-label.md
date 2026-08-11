# 会话卡片标注 agent 类型 + 模型 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在会话卡片元信息行右侧标注 agent 类型（Claude Code，图标 + hover）与模型（statusline 来的 `model.display_name`，胶囊）。

**Architecture:** 模型沿用现有 statusline → `session_context` 表通道（新增 `model` 列），随 `live_sessions()` 经 `LiveItem` 的 `serde(flatten)` 自动带到前端；agent 类型恒为 Claude Code，纯前端常量（图标 + i18n hover 文案）。

**Tech Stack:** Rust（meowo-store / meowo-reporter，rusqlite，workspace）、React + TypeScript（Vite，vitest）。

## Global Constraints

- 代码注释/commit 用中文，代码标识符用英文（遵循目标语言惯例）。
- i18n：`en.ts` 由 `Dict = typeof zh` 约束，**zh/en 必须同步加同名 key**，否则 `tsc` 报错。
- 卡片文字尺寸用密度乘子：`calc(Npx * var(--cc-ui))`，与既有卡片一致。
- 旧库迁移走 `store.rs::init` 的 ALTER 模式（已存在列忽略 "duplicate column name"），改 schema 须 bump `USER_VERSION`。
- model 写入用 `COALESCE`，缺失不覆盖已有值（与 used_pct/window_size 一致）。
- 验证：`cargo test`、`bunx tsc --noEmit`、`bun run test` 全绿；最终本地 `bun run tauri dev` 目测。

---

## File Structure

- `crates/meowo-store/src/migrations.rs` — `session_context` 表加 `model TEXT` 列。
- `crates/meowo-store/src/store.rs` — `USER_VERSION` 3→4；ALTER 补 `model` 列；`set_session_context` 增 `model` 参数。
- `crates/meowo-store/src/query.rs` — `LiveSession.model` 字段；`live_sessions()` SELECT 带出 `sc.model`。
- `crates/meowo-reporter/src/statusline.rs` — `record` 解析 `model.display_name` 并传入；新增往返测试。
- `app/src/api.ts` — `LiveSession` 加 `model: string | null`。
- `app/src/i18n/zh.ts` + `en.ts` — `sticker.agentClaudeCode`。
- `app/src/views/Sticker.tsx` — `AgentMark` 组件 + `.stk-line2` 渲染。
- `app/src/views/Sticker.test.tsx` — model 有/无两态用例。
- `app/src/styles.css` — `.stk-agentmodel` / `.stk-agent` / `.stk-model` + `.stk-repo` 收缩。

`app/src-tauri/src/lib.rs` **不改**：`LiveItem` 用 `#[serde(flatten)] inner: LiveSession`，新字段自动带出。

---

## Task 1: 后端 — model 采集、存储、带出（meowo-store + meowo-reporter）

**Files:**
- Modify: `crates/meowo-reporter/src/statusline.rs:8-25`（record）、测试模块（文件末尾 `mod tests`）
- Modify: `crates/meowo-store/src/migrations.rs:61-66`（session_context 表）
- Modify: `crates/meowo-store/src/store.rs:38`（USER_VERSION）、`:53-61`（ALTERS）、`:121-138`（set_session_context）
- Modify: `crates/meowo-store/src/query.rs:28-55`（LiveSession）、`:201-277`（live_sessions）

**Interfaces:**
- Produces:
  - `Store::set_session_context(&self, cc_session_id: &str, used_pct: Option<i64>, window_size: Option<i64>, model: Option<&str>, now_ms: i64) -> Result<(), StoreError>`
  - `LiveSession.model: Option<String>`

- [ ] **Step 1: 写失败测试（statusline.rs 测试模块内新增）**

在 `crates/meowo-reporter/src/statusline.rs` 的 `mod tests` 内追加：

```rust
    #[test]
    fn record_writes_model_for_session() {
        let store = Store::open_in_memory().unwrap();
        let pid = store.upsert_project_by_root("/p", "p", 1).unwrap();
        let _ = store.start_session(pid, "sm-1", 1).unwrap();
        let json = r#"{"session_id":"sm-1","model":{"display_name":"Opus"},"context_window":{"used_percentage":10,"context_window_size":200000}}"#;
        record(&store, json, 100);
        let live = store.live_sessions().unwrap();
        let s = live.iter().find(|l| l.session.cc_session_id == "sm-1").unwrap();
        assert_eq!(s.model.as_deref(), Some("Opus"));
    }

    #[test]
    fn record_missing_model_keeps_previous() {
        let store = Store::open_in_memory().unwrap();
        let pid = store.upsert_project_by_root("/p", "p", 1).unwrap();
        let _ = store.start_session(pid, "sm-2", 1).unwrap();
        record(&store, r#"{"session_id":"sm-2","model":{"display_name":"Sonnet"}}"#, 1);
        // 后续 statusline 不带 model（仅上下文）→ 不应抹掉已存的模型
        record(&store, r#"{"session_id":"sm-2","context_window":{"used_percentage":20}}"#, 2);
        let live = store.live_sessions().unwrap();
        let s = live.iter().find(|l| l.session.cc_session_id == "sm-2").unwrap();
        assert_eq!(s.model.as_deref(), Some("Sonnet"));
    }
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p meowo-reporter`
Expected: 编译失败 —— `LiveSession` 无 `model` 字段、`record` 内 `set_session_context` 参数不匹配（实现后才有）。

- [ ] **Step 3: migrations.rs 加列**

把 `session_context` 表（行 61-66）改为：

```sql
CREATE TABLE IF NOT EXISTS session_context (
    cc_session_id TEXT PRIMARY KEY,
    used_pct      INTEGER,
    window_size   INTEGER,
    model         TEXT,
    updated_at    INTEGER NOT NULL
);
```

- [ ] **Step 4: store.rs 迁移 + setter**

4a. 版本注释与常量（行 37-38）：

```rust
    /// v3: sessions 加 pending_review / last_ai_text / last_user_text 三列。
    /// v4: session_context 加 model 列（statusline 的模型展示名）。
    const USER_VERSION: i64 = 4;
```

4b. `ALTERS` 数组（行 53-61）尺寸 7→8 并加一条：

```rust
        const ALTERS: [&str; 8] = [
            "ALTER TABLE sessions ADD COLUMN pid INTEGER",
            "ALTER TABLE sessions ADD COLUMN cwd TEXT",
            "ALTER TABLE sessions ADD COLUMN archived INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE sessions ADD COLUMN archived_at INTEGER",
            "ALTER TABLE sessions ADD COLUMN pending_review TEXT",
            "ALTER TABLE sessions ADD COLUMN last_ai_text TEXT",
            "ALTER TABLE sessions ADD COLUMN last_user_text TEXT",
            "ALTER TABLE session_context ADD COLUMN model TEXT",
        ];
```

4c. `set_session_context`（行 121-138）整体替换为：

```rust
    pub fn set_session_context(
        &self,
        cc_session_id: &str,
        used_pct: Option<i64>,
        window_size: Option<i64>,
        model: Option<&str>,
        now_ms: i64,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO session_context (cc_session_id, used_pct, window_size, model, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(cc_session_id) DO UPDATE SET
                 used_pct = COALESCE(excluded.used_pct, used_pct),
                 window_size = COALESCE(excluded.window_size, window_size),
                 model = COALESCE(excluded.model, model),
                 updated_at = excluded.updated_at",
            rusqlite::params![cc_session_id, used_pct, window_size, model, now_ms],
        )?;
        Ok(())
    }
```

- [ ] **Step 5: query.rs 带出 model**

5a. `LiveSession` 结构体（行 44-46 附近，`context_window` 之后）加字段：

```rust
    /// 上下文窗口大小（200000 或 1000000）；无 statusline 数据为 None。
    pub context_window: Option<i64>,
    /// 模型展示名（来自 Claude Code statusline 的 model.display_name，如 "Opus"）；无则 None。
    pub model: Option<String>,
```

5b. `live_sessions()` SELECT（行 205）在 `sc.window_size` 后插入 `sc.model`：

```rust
                    sc.used_pct, sc.window_size, sc.model, sn.note,
                    s.pending_review, s.last_ai_text, s.last_user_text
```

5c. 行解析闭包（行 235-241）—— `sc.model` 占 index 18，其后各列 +1：

```rust
                let context_pct: Option<i64> = r.get(16)?;
                let context_window: Option<i64> = r.get(17)?;
                let model: Option<String> = r.get(18)?;
                let note: Option<String> = r.get(19)?;
                let pending_review: Option<String> = r.get(20)?;
                let last_ai_text: Option<String> = r.get(21)?;
                let last_user_text: Option<String> = r.get(22)?;
                Ok((session, project_name, task_id, task_title, current_activity, column, pid, archived, cwd, archived_at, context_pct, context_window, model, note, pending_review, last_ai_text, last_user_text))
```

5d. 解构与构造（行 249、255-274）加入 `model`：

```rust
        for (session, project_name, task_id, task_title, current_activity, column, pid, archived, cwd, archived_at, context_pct, context_window, model, note, pending_review, last_ai_text, last_user_text) in rows {
```

并在 `out.push(LiveSession { ... })` 中 `context_window,` 之后加一行 `model,`：

```rust
                context_pct,
                context_window,
                model,
                note,
```

- [ ] **Step 6: statusline.rs record 解析并传入 model**

把 `record`（行 8-25）整体替换为：

```rust
pub fn record(store: &Store, input: &str, now_ms: i64) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(input) else {
        return;
    };
    let Some(sid) = v.get("session_id").and_then(|x| x.as_str()) else {
        return;
    };
    let cw = v.get("context_window");
    // used_percentage 可能是整数或小数（如 23.5），统一四舍五入为整数。
    let used_pct = cw
        .and_then(|c| c.get("used_percentage"))
        .and_then(|x| x.as_f64())
        .map(|f| f.round() as i64);
    let window = cw
        .and_then(|c| c.get("context_window_size"))
        .and_then(|x| x.as_i64());
    let model = v
        .get("model")
        .and_then(|m| m.get("display_name"))
        .and_then(|x| x.as_str());
    let _ = store.set_session_context(sid, used_pct, window, model, now_ms);
}
```

- [ ] **Step 7: 运行后端测试确认通过**

Run: `cargo test -p meowo-store -p meowo-reporter`
Expected: PASS（含新增两个 model 用例 + 既有 context 用例不回归）。

- [ ] **Step 8: 提交**

```bash
git add crates/meowo-store/src/migrations.rs crates/meowo-store/src/store.rs crates/meowo-store/src/query.rs crates/meowo-reporter/src/statusline.rs
git commit -m "feat(store): session_context 存模型展示名并随 live_sessions 带出"
```

---

## Task 2: 前端 — 卡片 agent 图标 + 模型胶囊

**Files:**
- Modify: `app/src/api.ts:65-96`（LiveSession 类型）
- Modify: `app/src/i18n/zh.ts`、`app/src/i18n/en.ts`（sticker 段）
- Modify: `app/src/views/Sticker.tsx`（新增 `AgentMark` + `.stk-line2` 渲染，行 754-757）
- Test: `app/src/views/Sticker.test.tsx`
- Modify: `app/src/styles.css:572,577`（.stk-line2 / .stk-repo）+ 新增样式

**Interfaces:**
- Consumes: `LiveSession.model`（Task 1 产出，经 flatten 到前端）。
- Produces: 卡片 `.stk-agent`（恒在）、`.stk-model`（model 存在才有，文本=model）。

- [ ] **Step 1: api.ts 加类型字段**

`LiveSession`（`context_window` 之后）加：

```ts
  /** 上下文窗口大小（200000 或 1000000）；无 statusline 数据为 null。 */
  context_window: number | null;
  /** 模型展示名（Claude Code statusline 的 model.display_name，如 "Opus"）；无则 null。 */
  model: string | null;
```

- [ ] **Step 2: i18n 加 agent 文案（zh + en 同步）**

`app/src/i18n/zh.ts` 的 `sticker` 段内（如 `aiPrefix` 旁）加：

```ts
    youPrefix: "你",
    aiPrefix: "AI",
    agentClaudeCode: "Claude Code",
```

`app/src/i18n/en.ts` 的 `sticker` 段对应加：

```ts
    youPrefix: "You",
    aiPrefix: "AI",
    agentClaudeCode: "Claude Code",
```

- [ ] **Step 3: 写失败测试（Sticker.test.tsx）**

在 `describe("Sticker", ...)` 内追加：

```tsx
  it("有 model 时渲染模型胶囊与 agent 图标", () => {
    const { container } = render(<Sticker data={[mk({ model: "Opus" })]} />);
    expect(container.querySelector(".stk-model")?.textContent).toBe("Opus");
    expect(container.querySelector(".stk-agent")).toBeTruthy();
  });

  it("无 model 时只渲染 agent 图标、不渲染模型胶囊", () => {
    const { container } = render(<Sticker data={[mk({ model: null })]} />);
    expect(container.querySelector(".stk-agent")).toBeTruthy();
    expect(container.querySelector(".stk-model")).toBeNull();
  });
```

- [ ] **Step 4: 运行测试确认失败**

Run: `cd app && bun run test`
Expected: 上两个用例 FAIL（`.stk-agent` / `.stk-model` 尚未渲染）。

- [ ] **Step 5: Sticker.tsx 加 AgentMark 与渲染**

5a. 在文件图标组件区（如 `StarIcon` 之后）加组件：

```tsx
function AgentMark() {
  // 通用 AI 标记（4 角 spark），accent 色；未来按 provider 换此图标。
  return (
    <svg width="11" height="11" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
      <path d="M12 2c.5 4.5 2.5 6.5 7 7-4.5.5-6.5 2.5-7 7-.5-4.5-2.5-6.5-7-7 4.5-.5 6.5-2.5 7-7z" />
    </svg>
  );
}
```

5b. `.stk-line2`（行 754-757）改为：

```tsx
                    <div className="stk-line2">
                      <ConnBadge connected={l.connected} />
                      <span className="stk-repo">{l.project_name}</span>
                      <span className="stk-agentmodel">
                        <span className="stk-agent" title={t.sticker.agentClaudeCode} aria-label={t.sticker.agentClaudeCode}>
                          <AgentMark />
                        </span>
                        {l.model && <span className="stk-model">{l.model}</span>}
                      </span>
                    </div>
```

- [ ] **Step 6: styles.css 加样式**

6a. `.stk-repo`（行 577）加 `min-width: 0;` 使其在拥挤时收缩（避免把右侧组挤出）：

```css
.stk-repo { font-size: calc(10.5px * var(--cc-ui)); color: var(--cc-text-dim); min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
```

6b. 在 `.stk-repo` 之后新增：

```css
/* 元信息行右侧：agent 图标（恒显，hover 标明 Claude Code）+ 模型胶囊（有 model 才显）。 */
.stk-agentmodel { margin-left: auto; display: inline-flex; align-items: center; gap: 5px; flex: none; }
.stk-agent { display: inline-flex; align-items: center; color: var(--cc-accent-text); }
.stk-model { font-size: calc(10px * var(--cc-ui)); line-height: 1.5; padding: 0 6px; border-radius: 999px; background: var(--cc-surface-hover); color: var(--cc-text-dim); white-space: nowrap; }
```

- [ ] **Step 7: 运行测试与类型检查确认通过**

Run: `cd app && bunx tsc --noEmit && bun run test`
Expected: `tsc` 退出 0；vitest 全过（含新增两个用例 + 既有不回归）。

- [ ] **Step 8: 提交**

```bash
git add app/src/api.ts app/src/i18n/zh.ts app/src/i18n/en.ts app/src/views/Sticker.tsx app/src/views/Sticker.test.tsx app/src/styles.css
git commit -m "feat(web): 卡片元信息行标注 agent 类型(Claude Code)与模型"
```

---

## 收尾验证（非提交步骤）

- [ ] 运行 `cargo test`（全工作区）+ `cd app && bunx tsc --noEmit && bun run test` 全绿。
- [ ] 本地 `bun run tauri dev` 起应用，真实会话卡片目测：元信息行右侧 `◆ Opus`、hover 图标显示 "Claude Code"、深/浅主题正常、仓库名过长时截断不挤压右侧组、无 model 的会话只显示图标。
- [ ] 确认 OK 后推分支、开 PR（中文标题/描述含变更摘要 + 测试计划）。

## Self-Review 记录

- **Spec 覆盖**：模型存储(Task1 Step3-6)、agent 常量+图标(Task2 Step2/5)、卡片右对齐呈现(Task2 Step5-6)、model 缺省只显图标(Task2 Step3/5b)、测试(两任务 Step1/3)、迁移(Task1 Step4)均有对应任务。lib.rs 经核实无需改（flatten）。
- **占位符扫描**：无 TBD/TODO，所有代码步骤含完整代码与确切命令。
- **类型一致**：`set_session_context(..., model: Option<&str>, now_ms)` 在 store 定义与 statusline 调用一致；`LiveSession.model: Option<String>`(Rust) ↔ `model: string | null`(TS)；CSS 类名 `.stk-agent`/`.stk-model` 在测试、JSX、样式三处一致。
- **列索引**：live_sessions SELECT 插入 `sc.model` 于 index 18，其后 note/pending_review/last_ai_text/last_user_text 顺移到 19/20/21/22，闭包 `r.get` 已同步。

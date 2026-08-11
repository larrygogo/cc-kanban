# 会话错误状态检测 + 提醒 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 harness 致命错误（工具调用解析失败 / 需要登录 / 认证失败）的会话在贴纸上以红色露出并归入「待交互」，且出错时弹一次去重的桌面通知。

**Architecture:** 错误状态走"实时计算"——不写 DB、不改 schema、不加 hook。meowo-store 新增纯函数 `analyze_transcript`，单次扫 transcript 同时产出标题与错误；meowo-app 的 `get_live_sessions` 用它给每个展示会话算 `errored`，`spawn_liveness_watch`（5s 轮询）用它对连接中会话做去重通知。

**Tech Stack:** Rust（rusqlite、serde_json、tauri v2 + tauri-plugin-notification）、React 18 + TypeScript（vitest）。

---

## 文件结构

- `crates/meowo-store/src/analyze.rs`（新建）：`TurnError` / `TranscriptInfo` 类型、`classify_error` 纯函数、`analyze_transcript` 单次扫描。
- `crates/meowo-store/src/title.rs`（改）：抽出 `resolve_transcript_path`，`resolve_title` 复用它（行为不变）。
- `crates/meowo-store/src/lib.rs`（改）：注册 `mod analyze` 并重导出。
- `app/src-tauri/src/lib.rs`（改）：`LiveItem` 增错误字段；`get_live_sessions` 改用 `analyze_transcript`；`spawn_liveness_watch` 加通知；新增 `should_notify` 纯函数；注册 notification 插件。
- `app/src-tauri/Cargo.toml` + 根 `Cargo.toml`（改）：加 `tauri-plugin-notification`。
- `app/src-tauri/capabilities/default.json`（改）：加 `notification:default` 权限。
- `app/src/api.ts`（改）：`LiveSession` 增 `errored` / `error_label` / `error_raw`。
- `app/src/views/Sticker.tsx`（改）：indicator 优先级、`match()`、sub 行。
- `app/src/views/CollapsedStrip.tsx`（改）：errored → 红点。
- `app/src/styles.css`（改）：`--cc-err` 变量、`.needs-error`、`.cstrip-error`。
- `app/src/views/Sticker.test.tsx`（改）：errored 渲染/分类测试 + 更新 `mk()` 默认值。

---

## Task 1: meowo-store 错误检测纯函数 `analyze_transcript`

**Files:**
- Create: `crates/meowo-store/src/analyze.rs`
- Modify: `crates/meowo-store/src/lib.rs`
- Modify: `crates/meowo-store/src/title.rs`
- Test: `crates/meowo-store/src/analyze.rs`（`#[cfg(test)]` 内联）

- [ ] **Step 1: 写失败测试（classify_error + analyze_transcript）**

新建 `crates/meowo-store/src/analyze.rs`，先只放类型骨架与测试：

```rust
//! 从 Claude Code transcript 检测「致命卡死错误」并与标题解析共用一次文件读取。
use serde::Serialize;

/// 检测到的回合错误：短中文标签 + 原始文案 + 去重指纹（出错 assistant 消息的 uuid）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TurnError {
    pub label: String,
    pub raw: String,
    pub fingerprint: String,
}

/// 单次扫 transcript 的产物：标题与错误。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TranscriptInfo {
    pub title: Option<String>,
    pub error: Option<TurnError>,
}

/// 把 assistant 正文归类为「卡死错误」短标签；非卡死返回 None。
/// 刻意排除 529/500/ECONNRESET 等临时错误（多数自愈，标红会误报）。
pub fn classify_error(text: &str) -> Option<&'static str> {
    let t = text.trim();
    if t.contains("could not be parsed (retry also failed)") {
        return Some("工具调用解析失败");
    }
    if t.starts_with("Please run /login") || t.contains("API Error: 403") {
        return Some("需要重新登录");
    }
    if t.starts_with("Failed to authenticate") || t.contains("API Error: 401") {
        return Some("认证失败");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_matches_stuck_errors() {
        assert_eq!(
            classify_error("The model's tool call could not be parsed (retry also failed)."),
            Some("工具调用解析失败")
        );
        assert_eq!(
            classify_error("Please run /login · API Error: 403 Request not allowed"),
            Some("需要重新登录")
        );
        assert_eq!(classify_error("API Error: 403 Request not allowed"), Some("需要重新登录"));
        assert_eq!(
            classify_error("Failed to authenticate. API Error: 401 Invalid authentication credentials"),
            Some("认证失败")
        );
        assert_eq!(classify_error("API Error: 401 Invalid authentication credentials"), Some("认证失败"));
    }

    #[test]
    fn classify_ignores_transient_and_normal() {
        assert_eq!(classify_error("API Error: 529 Overloaded. This is a server-side issue"), None);
        assert_eq!(classify_error("API Error: 500 status code (no body)"), None);
        assert_eq!(classify_error("Unable to connect to API (ECONNRESET)"), None);
        assert_eq!(classify_error("这是一段正常的助手回答。"), None);
    }

    fn write_tmp(name: &str, content: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("cc_analyze_{}_{}.jsonl", std::process::id(), name));
        std::fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn analyze_detects_parse_abort_and_title() {
        // 末尾：ai-title + 出错 assistant（带 uuid）+ turn_duration
        let content = concat!(
            r#"{"type":"ai-title","aiTitle":"做某功能"}"#, "\n",
            r#"{"type":"assistant","uuid":"u-err-1","message":{"role":"assistant","content":[{"type":"thinking","thinking":""},{"type":"text","text":"The model's tool call could not be parsed (retry also failed)."}]}}"#, "\n",
            r#"{"type":"system","subtype":"turn_duration","durationMs":1000}"#, "\n",
        );
        let p = write_tmp("parse", content);
        let info = analyze_transcript(p.to_str().unwrap());
        std::fs::remove_file(&p).ok();
        assert_eq!(info.title.as_deref(), Some("做某功能"));
        let e = info.error.expect("应检测到错误");
        assert_eq!(e.label, "工具调用解析失败");
        assert_eq!(e.fingerprint, "u-err-1");
    }

    #[test]
    fn analyze_no_error_on_normal_ending() {
        let content = concat!(
            r#"{"type":"assistant","uuid":"u1","message":{"role":"assistant","content":[{"type":"text","text":"已完成，结果如下。"}]}}"#, "\n",
            r#"{"type":"system","subtype":"turn_duration","durationMs":500}"#, "\n",
        );
        let p = write_tmp("normal", content);
        let info = analyze_transcript(p.to_str().unwrap());
        std::fs::remove_file(&p).ok();
        assert_eq!(info.error, None);
    }

    #[test]
    fn analyze_recovered_after_error_not_flagged() {
        // 出错回合之后又有成功回合 → 最后一条 assistant 正文是成功回答 → 不报
        let content = concat!(
            r#"{"type":"assistant","uuid":"u-err","message":{"role":"assistant","content":[{"type":"text","text":"The model's tool call could not be parsed (retry also failed)."}]}}"#, "\n",
            r#"{"type":"system","subtype":"turn_duration","durationMs":100}"#, "\n",
            r#"{"type":"user","message":{"role":"user","content":"继续"}}"#, "\n",
            r#"{"type":"assistant","uuid":"u-ok","message":{"role":"assistant","content":[{"type":"text","text":"好的，已经修好了。"}]}}"#, "\n",
            r#"{"type":"system","subtype":"turn_duration","durationMs":200}"#, "\n",
        );
        let p = write_tmp("recover", content);
        let info = analyze_transcript(p.to_str().unwrap());
        std::fs::remove_file(&p).ok();
        assert_eq!(info.error, None);
    }

    #[test]
    fn analyze_skips_tooluse_only_assistant() {
        // 最后一条 assistant 仅 tool_use（无 text）→ 不应被当成错误判据来源；
        // 取「最后一条带 text 的 assistant」= 出错文案 → 仍报错。
        let content = concat!(
            r#"{"type":"assistant","uuid":"u-err","message":{"role":"assistant","content":[{"type":"text","text":"Please run /login · API Error: 403 Request not allowed"}]}}"#, "\n",
            r#"{"type":"assistant","uuid":"u-tool","message":{"role":"assistant","content":[{"type":"tool_use","name":"Bash","input":{}}]}}"#, "\n",
        );
        let p = write_tmp("toolonly", content);
        let info = analyze_transcript(p.to_str().unwrap());
        std::fs::remove_file(&p).ok();
        assert_eq!(info.error.map(|e| e.label), Some("需要重新登录".to_string()));
    }

    #[test]
    fn analyze_missing_file_is_empty() {
        let info = analyze_transcript("C:/no/such/file-xyz.jsonl");
        assert_eq!(info, TranscriptInfo::default());
    }
}
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test -p meowo-store analyze`
Expected: 编译失败 —— `analyze_transcript` 未定义、`mod analyze` 未注册。

- [ ] **Step 3: 实现 `analyze_transcript` 并注册模块**

在 `crates/meowo-store/src/analyze.rs` 的类型与测试之间加实现：

```rust
/// 单次遍历 transcript：同时解析标题（custom-title 优先于 ai-title）与
/// 「最后一条带 text 的 assistant 正文」，对该正文做卡死归类。读不到/空 → 全 None。
pub fn analyze_transcript(path: &str) -> TranscriptInfo {
    let Ok(content) = std::fs::read_to_string(path) else {
        return TranscriptInfo::default();
    };
    let mut custom: Option<String> = None;
    let mut ai: Option<String> = None;
    let mut last_text: Option<(String, String)> = None; // (正文, uuid)

    for line in content.lines() {
        let has_title = line.contains("-title");
        let has_assistant = line.contains("\"assistant\"");
        if !has_title && !has_assistant {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("custom-title") => {
                if let Some(s) = v.get("customTitle").and_then(|x| x.as_str()) {
                    if !s.trim().is_empty() {
                        custom = Some(s.to_string());
                    }
                }
            }
            Some("ai-title") => {
                if let Some(s) = v.get("aiTitle").and_then(|x| x.as_str()) {
                    if !s.trim().is_empty() {
                        ai = Some(s.to_string());
                    }
                }
            }
            Some("assistant") => {
                // 取该 assistant 消息 content 数组里第一个 text 块；无 text 块则跳过（如纯 tool_use）。
                let text = v
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                    .and_then(|arr| {
                        arr.iter().find_map(|x| {
                            if x.get("type").and_then(|t| t.as_str()) == Some("text") {
                                x.get("text").and_then(|t| t.as_str()).map(|s| s.to_string())
                            } else {
                                None
                            }
                        })
                    });
                if let Some(text) = text {
                    let uuid = v.get("uuid").and_then(|u| u.as_str()).unwrap_or("").to_string();
                    last_text = Some((text, uuid));
                }
            }
            _ => {}
        }
    }

    let error = last_text.and_then(|(text, uuid)| {
        classify_error(&text).map(|label| TurnError {
            label: label.to_string(),
            raw: text,
            fingerprint: uuid,
        })
    });
    TranscriptInfo { title: custom.or(ai), error }
}
```

在 `crates/meowo-store/src/lib.rs` 注册模块（与现有 `pub mod title;` 等同处）：

```rust
pub mod analyze;
pub use analyze::{analyze_transcript, TranscriptInfo, TurnError};
```

- [ ] **Step 4: 运行测试，确认通过**

Run: `cargo test -p meowo-store analyze`
Expected: PASS（全部 analyze 测试通过）。

- [ ] **Step 5: 抽出 `resolve_transcript_path` 供后续复用**

在 `crates/meowo-store/src/title.rs` 新增（放在 `resolve_title` 上方）：

```rust
/// 解析 transcript 文件路径，依次尝试：1) hook 给的 path；2) cwd+session_id 重建；
/// 3) 按 session_id 全局查找。供「同时要标题+错误」的调用方先拿路径再 analyze。
pub fn resolve_transcript_path(
    transcript_path: Option<&str>,
    cwd: Option<&str>,
    session_id: &str,
) -> Option<std::path::PathBuf> {
    if let Some(p) = transcript_path {
        let pb = std::path::PathBuf::from(p);
        if pb.exists() {
            return Some(pb);
        }
    }
    if let Some(cwd) = cwd {
        if let Some(p) = reconstruct_transcript_path(cwd, session_id) {
            if p.exists() {
                return Some(p);
            }
        }
    }
    find_transcript_by_session(session_id)
}
```

> `resolve_title` 保持原样不动（仍可用）；本函数仅供 meowo-app 新流程。

- [ ] **Step 6: 运行全 store 测试 + clippy**

Run: `cargo test -p meowo-store && cargo clippy -p meowo-store -- -D warnings`
Expected: PASS，无 clippy 警告。

- [ ] **Step 7: 提交**

```bash
git add crates/meowo-store/src/analyze.rs crates/meowo-store/src/lib.rs crates/meowo-store/src/title.rs
git commit -m "feat(store): transcript 错误检测 analyze_transcript + 路径解析复用"
```

---

## Task 2: meowo-app 后端 `get_live_sessions` 接入错误字段

**Files:**
- Modify: `app/src-tauri/src/lib.rs:272-334`（`LiveItem` 与 `get_live_sessions`）

- [ ] **Step 1: 给 `LiveItem` 加错误字段**

把 `LiveItem`（约 272-277 行）改为：

```rust
#[derive(serde::Serialize)]
struct LiveItem {
    #[serde(flatten)]
    inner: LiveSession,
    connected: bool,
    errored: bool,
    error_label: Option<String>,
    error_raw: Option<String>,
}
```

- [ ] **Step 2: 在标题解析处改用 `analyze_transcript`**

在 `get_live_sessions` 循环里（约 317-331 行），把现有的：

```rust
        if let Some(t) =
            meowo_store::title::resolve_title(None, s.cwd.as_deref(), &s.session.cc_session_id)
        {
            s.task_title = t;
        }
```

替换为：

```rust
        // 一次读 transcript 同时拿标题与错误（断开/历史会话不触发 hook，DB 可能是旧值）。
        let mut errored = false;
        let mut error_label: Option<String> = None;
        let mut error_raw: Option<String> = None;
        if let Some(path) = meowo_store::title::resolve_transcript_path(
            None,
            s.cwd.as_deref(),
            &s.session.cc_session_id,
        ) {
            if let Some(p) = path.to_str() {
                let info = meowo_store::analyze_transcript(p);
                if let Some(t) = info.title {
                    s.task_title = t;
                }
                if let Some(e) = info.error {
                    errored = true;
                    error_label = Some(e.label);
                    error_raw = Some(e.raw);
                }
            }
        }
```

并把循环末尾的 `items.push(LiveItem { inner: s, connected });`（约 331 行）改为：

```rust
        items.push(LiveItem { inner: s, connected, errored, error_label, error_raw });
```

- [ ] **Step 3: 编译 + 现有测试 + clippy**

Run: `cargo test -p meowo-app && cargo clippy -p meowo-app -- -D warnings`
Expected: PASS（现有窗口/吸边等单测不受影响，新增字段编译通过）。

- [ ] **Step 4: 提交**

```bash
git add app/src-tauri/src/lib.rs
git commit -m "feat(app): get_live_sessions 输出会话错误状态字段"
```

---

## Task 3: 桌面通知（依赖 + 去重 + 接入轮询）

**Files:**
- Modify: `Cargo.toml`（根，workspace 依赖）
- Modify: `app/src-tauri/Cargo.toml`
- Modify: `app/src-tauri/capabilities/default.json`
- Modify: `app/src-tauri/src/lib.rs`（`should_notify` + `run()` 注册插件 + `spawn_liveness_watch`）

- [ ] **Step 1: 写 `should_notify` 失败测试**

在 `app/src-tauri/src/lib.rs` 末尾 `#[cfg(test)] mod tests` 的 `use super::{...}` 里加入 `should_notify`，并新增测试：

```rust
    #[test]
    fn should_notify_only_on_new_error() {
        assert!(!should_notify(None, None));            // 无错 → 不弹
        assert!(should_notify(None, Some("a")));        // 新错 → 弹
        assert!(!should_notify(Some("a"), Some("a")));  // 同一错误 → 不弹
        assert!(should_notify(Some("a"), Some("b")));   // 换了新错误 → 弹
        assert!(!should_notify(Some("a"), None));       // 错误消失 → 不弹（由清除处理）
    }
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p meowo-app should_notify`
Expected: 编译失败 —— `should_notify` 未定义。

- [ ] **Step 3: 实现 `should_notify`**

在 `app/src-tauri/src/lib.rs` 加入纯函数（放在 `spawn_liveness_watch` 上方）：

```rust
/// 是否应为「当前错误指纹」弹通知：仅当当前有错误且指纹与上次通知过的不同。
/// 同一错误不反复弹；错误消失（cur=None）不弹（清除条目交给调用方）。纯函数，便于单测。
fn should_notify(prev: Option<&str>, cur: Option<&str>) -> bool {
    match cur {
        None => false,
        Some(c) => prev != Some(c),
    }
}
```

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p meowo-app should_notify`
Expected: PASS。

- [ ] **Step 5: 提交（纯函数先落地）**

```bash
git add app/src-tauri/src/lib.rs
git commit -m "feat(app): 通知去重纯函数 should_notify"
```

- [ ] **Step 6: 加 notification 插件依赖**

根 `Cargo.toml` 的 `[workspace.dependencies]` 末尾加：

```toml
tauri-plugin-notification = "2"
```

`app/src-tauri/Cargo.toml` 的 `[dependencies]` 加（紧跟其它 tauri-plugin 行）：

```toml
tauri-plugin-notification = { workspace = true }
```

- [ ] **Step 7: 加 capability 权限**

`app/src-tauri/capabilities/default.json` 的 `permissions` 数组里，在 `"process:default"` 后加一项：

```json
    "notification:default"
```

- [ ] **Step 8: 注册插件**

`app/src-tauri/src/lib.rs` 的 `run()` 里，在 `.plugin(tauri_plugin_process::init())` 之后加一行：

```rust
        .plugin(tauri_plugin_notification::init())
```

- [ ] **Step 9: 在 5s 轮询里接入去重通知**

把 `spawn_liveness_watch`（约 877-894 行）整体替换为：

```rust
/// 周期轮询：收尾进程已死的卡住会话；存活集合变化或有收尾时发 board-changed 让前端刷新。
/// 同时对「连接中且出错」的会话做去重桌面通知（同一错误只弹一次，启动首扫只播种不弹）。
fn spawn_liveness_watch(app: tauri::AppHandle, db_path: PathBuf) {
    use std::collections::HashMap;
    use tauri_plugin_notification::NotificationExt;
    std::thread::spawn(move || {
        let mut last: Vec<i64> = Vec::new();
        let mut notified: HashMap<String, String> = HashMap::new(); // cc_session_id -> 上次通知指纹
        let mut seeded = false;
        loop {
            if let Ok(store) = Store::open(&db_path) {
                let orphaned = store.end_orphaned_idle(ORPHAN_IDLE_MS, now_ms()).unwrap_or(0);
                let (alive, reaped) = reap_and_alive_ids(&store, now_ms());
                if alive != last || reaped > 0 || orphaned > 0 {
                    let _ = app.emit("board-changed", ());
                    last = alive;
                }

                // 错误检测 + 去重通知：仅扫连接中的会话（活跃，数量少）。
                let sys = System::new_with_specifics(
                    RefreshKind::new().with_processes(ProcessRefreshKind::new()),
                );
                let mut present: HashMap<String, String> = HashMap::new();
                for s in store.live_sessions().unwrap_or_default() {
                    if s.session.status == "ended" || !pid_is_claude(&sys, s.pid.unwrap_or(0)) {
                        continue;
                    }
                    let sid = s.session.cc_session_id.clone();
                    let err = meowo_store::title::resolve_transcript_path(
                        None, s.cwd.as_deref(), &sid,
                    )
                    .and_then(|p| p.to_str().map(meowo_store::analyze_transcript))
                    .and_then(|info| info.error);

                    match err {
                        Some(e) => {
                            present.insert(sid.clone(), e.fingerprint.clone());
                            let prev = notified.get(&sid).map(|s| s.as_str());
                            if seeded && should_notify(prev, Some(&e.fingerprint)) {
                                let _ = app
                                    .notification()
                                    .builder()
                                    .title("会话出错")
                                    .body(format!("{} · {}", s.project_name, e.label))
                                    .show();
                            }
                            notified.insert(sid, e.fingerprint);
                        }
                        None => {
                            notified.remove(&sid); // 错误消失：下次再错会重新通知
                        }
                    }
                }
                // 清掉已不在连接中集合里的残留条目，防止 map 无限增长。
                notified.retain(|k, _| present.contains_key(k));
                seeded = true;
            }
            std::thread::sleep(Duration::from_secs(5));
        }
    });
}
```

- [ ] **Step 10: 编译 + 全 app 测试 + clippy**

Run: `cargo test -p meowo-app && cargo clippy -p meowo-app -- -D warnings`
Expected: PASS。若 clippy 报 `tauri_plugin_notification` 未用导入，确认 Step 6 依赖已加。

- [ ] **Step 11: 提交**

```bash
git add Cargo.toml app/src-tauri/Cargo.toml app/src-tauri/capabilities/default.json app/src-tauri/src/lib.rs
git commit -m "feat(app): 会话出错时去重桌面通知"
```

---

## Task 4: 前端渲染（类型 + 卡片 + 缩略条 + 样式 + 测试）

**Files:**
- Modify: `app/src/api.ts:65-79`（`LiveSession` 类型）
- Modify: `app/src/views/Sticker.tsx`（`match` / indicator / sub 行）
- Modify: `app/src/views/CollapsedStrip.tsx:66-72`
- Modify: `app/src/styles.css`
- Test: `app/src/views/Sticker.test.tsx`

- [ ] **Step 1: 扩展 `LiveSession` 类型**

`app/src/api.ts` 的 `LiveSession` 类型（约 65-79 行）末尾、`cwd` 之后加三个字段：

```ts
  cwd: string | null;
  errored: boolean;
  error_label: string | null;
  error_raw: string | null;
```

- [ ] **Step 2: 写失败测试（先更新 mk 默认值，再加断言）**

`app/src/views/Sticker.test.tsx` 的 `mk()` 默认对象里（`cwd: null,` 之后）加：

```ts
    cwd: null, errored: false, error_label: null, error_raw: null,
```

并在 `describe("Sticker", ...)` 内追加测试：

```ts
  it("errored 会话归入待交互、显示红点与错误文案", () => {
    const item = mk({
      session: { id: 9, project_id: 1, cc_session_id: "s9", status: "running", started_at: 0, last_event_at: Date.now(), ended_at: null },
      errored: true, error_label: "工具调用解析失败", error_raw: "The model's tool call could not be parsed (retry also failed).",
    });
    const { container } = render(<Sticker data={[item]} />);
    // 待交互 tab 计数为 1
    const waitingTab = screen.getByText("待交互").closest(".stab")!;
    expect(waitingTab.querySelector(".stab-n")!.textContent).toBe("1");
    // 运行中 tab 计数为 0（出错从运行中挪走）
    const runningTab = screen.getByText("运行中").closest(".stab")!;
    expect(runningTab.querySelector(".stab-n")!.textContent).toBe("0");
    // 红点存在、错误文案存在
    expect(container.querySelector(".needs-error")).toBeTruthy();
    expect(screen.getByText("工具调用解析失败")).toBeTruthy();
  });

  it("断开优先于 errored：只显示断开环", () => {
    const item = mk({ connected: false, errored: true, error_label: "认证失败" });
    const { container } = render(<Sticker data={[item]} />);
    expect(container.querySelector(".ring-stop")).toBeTruthy();
    expect(container.querySelector(".needs-error")).toBeFalsy();
  });
```

- [ ] **Step 3: 运行，确认失败**

Run: `cd app && bunx vitest run src/views/Sticker.test.tsx`
Expected: FAIL —— `.needs-error` 不存在、运行中计数为 1（尚未排除 errored）。

- [ ] **Step 4: 改 `match()` 分类逻辑**

`app/src/views/Sticker.tsx` 的 `match()`（约 120-122 行），把 waiting / running 两行改为：

```ts
  if (tab === "waiting") return l.connected && (l.session.status === "waiting" || l.errored);
  if (tab === "running") return l.connected && l.session.status === "running" && !l.errored;
```

- [ ] **Step 5: 改 indicator 优先级 + sub 行**

`app/src/views/Sticker.tsx` 的 `indicator`（约 280-288 行）改为（断开 > errored > running > waiting > 在线）：

```tsx
            const indicator = !l.connected ? (
              <span className="ring-stop" title="已断开/已停止" />
            ) : l.errored ? (
              <span className="needs-error" title={l.error_raw ?? "会话出错"} />
            ) : l.session.status === "running" ? (
              <span className="spinner" />
            ) : l.session.status === "waiting" ? (
              <span className="needs" title="等待输入" />
            ) : (
              <span className="sdot sdot-on" title="在线" />
            );
```

并把 sub 行计算（约 278 行 `const sub = ...`）与渲染（约 350 行 `{sub && ...}`）改为优先显示错误文案：

约 278 行改为：

```tsx
            const sub = l.errored && l.error_label
              ? l.error_label
              : l.current_activity && l.current_activity !== title
              ? l.current_activity
              : null;
```

约 350 行 `{sub && <div className="stk-sub">{sub}</div>}` 改为：

```tsx
                {sub && <div className={"stk-sub" + (l.errored ? " stk-sub-err" : "")} title={l.errored ? l.error_raw ?? undefined : undefined}>{sub}</div>}
```

- [ ] **Step 6: 加样式**

`app/src/styles.css` 的 `:root` 里（`--cc-warn: #e0a23c;` 之后，约 16 行）加：

```css
  --cc-err: #e0584c;
```

`app/src/styles.css` 的 `.needs` 规则之后（约 242 行）加：

```css
/* 会话出错：红色脉冲点 */
.needs-error {
  width: 9px;
  height: 9px;
  flex: none;
  border-radius: 50%;
  background: var(--cc-err);
  animation: needs-pulse-err 1.6s ease-out infinite;
}
@keyframes needs-pulse-err {
  0% { box-shadow: 0 0 0 0 rgba(224, 88, 76, 0.5); }
  70% { box-shadow: 0 0 0 5px rgba(224, 88, 76, 0); }
  100% { box-shadow: 0 0 0 0 rgba(224, 88, 76, 0); }
}
/* 出错时的 sub 行文案用红色 */
.stk-sub-err { color: var(--cc-err); }
```

`app/src/styles.css` 的 `.cstrip-waiting`（约 311 行）之后加：

```css
.cstrip-error { background: var(--cc-err); animation: cstrip-pulse 1.2s ease-in-out infinite; }
```

- [ ] **Step 7: 缩略条同步红点**

`app/src/views/CollapsedStrip.tsx` 的 `cls` 计算（约 66-71 行）改为（errored 优先）：

```tsx
            const cls = l.errored
              ? "cstrip-error"
              : l.session.status === "running"
              ? "cstrip-running"
              : l.session.status === "waiting"
              ? "cstrip-waiting"
              : "cstrip-on";
```

- [ ] **Step 8: 运行前端测试 + 类型检查**

Run: `cd app && bunx vitest run && bunx tsc --noEmit`
Expected: PASS（新增两测试通过，其余不回归，无类型错误）。

- [ ] **Step 9: 提交**

```bash
git add app/src/api.ts app/src/views/Sticker.tsx app/src/views/CollapsedStrip.tsx app/src/styles.css app/src/views/Sticker.test.tsx
git commit -m "feat(app): 贴纸渲染会话错误状态（红点+文案+缩略条+归入待交互）"
```

---

## Task 5: 整体验证 + 文档

**Files:**
- Modify: `README.md`（特性列表）

- [ ] **Step 1: 全量测试 + lint**

Run（仓库根）：

```bash
cargo test --workspace && cargo clippy --workspace -- -D warnings
```

Expected: PASS，无警告。

Run（前端）：

```bash
cd app && bunx tsc --noEmit && bunx vitest run
```

Expected: PASS。

- [ ] **Step 2: README 补一条特性**

`README.md` 的「特性」列表里（`- **状态指示**：...` 那条之后）加：

```markdown
- **错误提醒**：会话因工具调用解析失败 / 需要重新登录 / 认证失败而卡死时，卡片转红并归入「待交互」，同时弹一条去重的桌面通知（同一错误只弹一次）。
```

- [ ] **Step 3: 手动冒烟（可选，需真实环境）**

Run: `cd app && bun run tauri dev`
验证：制造一次（或用历史出错会话）→ 卡片红点 + 红色文案 + 归入待交互 tab + 弹一次通知；继续会话跑通后红色消失。

- [ ] **Step 4: 提交**

```bash
git add README.md
git commit -m "docs: README 补会话错误提醒特性"
```

---

## 自查记录

- **Spec 覆盖**：检测判据 → Task 1；不写 DB 的实时计算 → Task 1/2；待交互+红点 UI → Task 4；桌面通知+去重+首扫播种 → Task 3；短中文标签+tooltip → Task 1(label)/Task 4(raw)；断开优先 → Task 4 indicator；测试计划三项 → Task 1/3/4 各自覆盖。✅
- **占位符**：无 TBD/TODO，所有步骤含完整代码与命令。✅
- **类型一致**：`TurnError{label,raw,fingerprint}`、`analyze_transcript`、`should_notify`、`resolve_transcript_path`、`LiveItem{errored,error_label,error_raw}`、前端 `errored/error_label/error_raw` 全程一致。✅

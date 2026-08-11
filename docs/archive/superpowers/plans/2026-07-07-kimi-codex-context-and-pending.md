# kimi/codex 待交互 + 上下文百分比 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 kimi + codex 卡片显示上下文百分比（块C），并让 kimi 会话在等待用户审批时显示「待交互」（块B）。

**Architecture:** 块C 给 `Agent` trait 加 `read_context` 方法，kimi 从 `wire.jsonl` 的 `usage.record`、codex 从 rollout 的 `token_count` 读最近上下文占用；dispatch 在 `PostToolUse`/`Stop` 分支调用它写 `session_context`。块B 给 `setup/kimi.rs` 的 `KIMI_EVENTS` 加 `PermissionRequest`，dispatch 既有分支复用。provider 差异全封在 `read_context` 实现里，dispatch 保持 provider 无关。

**Tech Stack:** Rust；serde_json（已有）；无新依赖（config.toml 用逐行启发式解析，同 `account/kimi.rs` 既有范式）。

**Spec:** `docs/superpowers/specs/2026-07-07-kimi-codex-context-and-pending-design.md`

## Global Constraints

- 代码注释与 commit message 用中文；代码本身英文。
- `cargo clippy --workspace -- -D warnings` 与 `cargo test --workspace` 必须全绿后才 commit。
- best-effort：任何定位/读取/解析失败 → `read_context` 返回 `None`，绝不 panic、绝不阻断 hook。
- claude 零行为变更：`read_context` 用 trait 默认实现返回 `None`，statusLine 链路不动。
- 长文件（wire.jsonl / rollout 可达数 MB）一律尾部有界读，只取最后一条用量事件。
- **块B 前置**：块A「provider-setup」旧 plan（`docs/superpowers/plans/2026-07-03-provider-setup.md`）已执行完毕，`app/src-tauri/src/setup/kimi.rs` 及其 `KIMI_EVENTS` / `KIMI_EVENT_WHITELIST` 已存在。块C（Task 1–4）不依赖块A，可独立先做。
- 执行顺序：块A（旧 plan）→ 本 plan 的 Task 1–4（块C）→ Task 5（块B）。

---

### Task 1: `ContextUsage` 结构 + `Agent::read_context` trait 方法（默认 None）

**Files:**
- Modify: `crates/meowo-reporter/src/agent.rs`（加结构 + trait 方法，claude 用默认）

**Interfaces:**
- Produces: `pub struct ContextUsage { pub used_pct: i64, pub window: i64 }`；`Agent::read_context(&self, ev: &HookEvent) -> Option<ContextUsage>`（默认 `None`）。Task 2/3 覆写，Task 4 消费。

- [ ] **Step 1: 写失败测试**

在 `agent.rs` 的 `mod tests` 内追加：

```rust
    #[test]
    fn claude_read_context_defaults_none() {
        let ev = HookEvent::parse(r#"{"hook_event_name":"Stop","session_id":"x"}"#).unwrap();
        assert!(for_provider(ProviderKey::Claude).read_context(&ev).is_none());
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p meowo-reporter agent::tests::claude_read_context_defaults_none`
Expected: FAIL（`read_context` 未定义，编译错误）

- [ ] **Step 3: 加结构 + trait 方法**

在 `agent.rs` 的 `StopOutputs` 结构之后追加：

```rust
/// 会话上下文占用快照。kimi/codex 从会话日志读；claude 走 statusline，返回 None。
#[derive(Debug, Default, PartialEq)]
pub struct ContextUsage {
    /// 已用百分比（0–100，已 clamp）。
    pub used_pct: i64,
    /// 上下文窗口大小（token）。
    pub window: i64,
}
```

在 `trait Agent` 内（`stop_outputs` 附近）追加带默认实现的方法：

```rust
    /// 从会话日志读最近一次上下文占用。claude 返回 None（走 statusline）；
    /// kimi 读 wire.jsonl 的 usage.record，codex 读 rollout 的 token_count（各自覆写）。
    fn read_context(&self, _ev: &HookEvent) -> Option<ContextUsage> {
        None
    }
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p meowo-reporter agent:: && cargo clippy -p meowo-reporter -- -D warnings`
Expected: PASS，clippy 零警告

- [ ] **Step 5: Commit**

```bash
git add crates/meowo-reporter/src/agent.rs
git commit -m "feat(agent): 加 ContextUsage + Agent::read_context trait 方法（默认 None）"
```

---

### Task 2: kimi 上下文解析 + read_context

**Files:**
- Modify: `crates/meowo-reporter/src/kimi.rs`（加 `parse_context` / `context_window` / `read_context`）
- Modify: `crates/meowo-reporter/src/agent.rs`（`KimiAgent` 覆写 `read_context`）

**Interfaces:**
- Consumes: Task 1 的 `ContextUsage`；本文件既有 `session_dir`、`read_range`、`FULL_READ_CAP`、`TAIL_BYTES`。
- Produces: `kimi::parse_context(&str) -> Option<(i64, String)>`（used, model_alias）；`kimi::context_window(&str) -> i64`；`kimi::read_context(&str) -> Option<crate::agent::ContextUsage>`。

- [ ] **Step 1: 写失败测试**

在 `kimi.rs` 的 `mod tests` 内追加：

```rust
    #[test]
    fn parse_context_takes_last_usage_record_and_sums_inputs() {
        let wire = r#"
{"type":"usage.record","model":"kimi-code/kimi-for-coding","usage":{"inputOther":100,"output":5,"inputCacheRead":200,"inputCacheCreation":0}}
{"type":"context.append_loop_event","event":{"type":"content.part","part":{"type":"text","text":"hi"}}}
{"type":"usage.record","model":"kimi-code/kimi-for-coding","usage":{"inputOther":727,"output":815,"inputCacheRead":20480,"inputCacheCreation":13}}
"#;
        // 取最后一条：727 + 20480 + 13 = 21220；output 不计。
        assert_eq!(parse_context(wire), Some((21220, "kimi-code/kimi-for-coding".to_string())));
    }

    #[test]
    fn parse_context_none_when_no_usage_record() {
        let wire = r#"{"type":"turn.prompt","input":"hi"}"#;
        assert_eq!(parse_context(wire), None);
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p meowo-reporter kimi::tests::parse_context`
Expected: FAIL（`parse_context` 未定义）

- [ ] **Step 3: 实现三个函数**

在 `kimi.rs` 追加（`read_summary` 之后）：

```rust
/// 从 wire.jsonl 文本取**最后一条** usage.record 的 (used_input_tokens, model_alias)。
/// used = inputOther + inputCacheRead + inputCacheCreation（≈ 该回合请求发送时的 context 输入量，
/// 每次请求都把整个 context 作为 input 发送）；output 不计（本轮新生成，尚未进 context）。
pub fn parse_context(content: &str) -> Option<(i64, String)> {
    let mut last: Option<(i64, String)> = None;
    for line in content.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("usage.record") {
            continue;
        }
        let Some(u) = v.get("usage") else { continue };
        let field = |k: &str| u.get(k).and_then(|x| x.as_i64()).unwrap_or(0);
        let used = field("inputOther") + field("inputCacheRead") + field("inputCacheCreation");
        let model = v.get("model").and_then(|m| m.as_str()).unwrap_or("").to_string();
        last = Some((used, model));
    }
    last
}

/// 读 config.toml 里 `[models."<alias>"]` 的 `max_context_size`；找不到回退 262144。
/// 逐行启发式解析（不引 toml 依赖，同 account/kimi.rs 既有范式）。
pub fn context_window(model_alias: &str) -> i64 {
    const FALLBACK: i64 = 262_144;
    let Some(dir) = kimi_share_dir() else { return FALLBACK };
    let Ok(content) = std::fs::read_to_string(dir.join("config.toml")) else {
        return FALLBACK;
    };
    let want = format!("[models.\"{model_alias}\"]");
    let mut in_section = false;
    for line in content.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_section = t == want;
            continue;
        }
        if !in_section || t.starts_with('#') {
            continue;
        }
        if let Some(rest) = t.strip_prefix("max_context_size") {
            if let Some(after) = rest.trim_start().strip_prefix('=') {
                if let Ok(n) = after.trim().parse::<i64>() {
                    if n > 0 {
                        return n;
                    }
                }
            }
        }
    }
    FALLBACK
}

/// 读某 kimi 会话最近的上下文占用：wire.jsonl 尾部取最后一条 usage.record，used/window 算百分比。
/// 定位/读/解析失败返回 None。大文件尾部有界读（与 read_summary 同款）。
pub fn read_context(session_id: &str) -> Option<crate::agent::ContextUsage> {
    let wire = session_dir(session_id)?
        .join("agents")
        .join("main")
        .join("wire.jsonl");
    let size = std::fs::metadata(&wire).ok()?.len();
    let text = if size <= FULL_READ_CAP {
        std::fs::read_to_string(&wire).ok()?
    } else {
        read_range(&wire, size.saturating_sub(TAIL_BYTES), TAIL_BYTES)?
    };
    let (used, model) = parse_context(&text)?;
    let window = context_window(&model);
    if window <= 0 {
        return None;
    }
    let pct = (used * 100 / window).clamp(0, 100);
    Some(crate::agent::ContextUsage { used_pct: pct, window })
}
```

在 `agent.rs` 的 `impl Agent for KimiAgent` 内追加：

```rust
    fn read_context(&self, ev: &HookEvent) -> Option<ContextUsage> {
        crate::kimi::read_context(&ev.session_id)
    }
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p meowo-reporter kimi:: && cargo clippy -p meowo-reporter -- -D warnings`
Expected: PASS，clippy 零警告

- [ ] **Step 5: Commit**

```bash
git add crates/meowo-reporter/src/kimi.rs crates/meowo-reporter/src/agent.rs
git commit -m "feat(kimi): 从 wire.jsonl 的 usage.record 读上下文占用（parse_context + read_context）"
```

---

### Task 3: codex 上下文解析 + read_context

**Files:**
- Modify: `crates/meowo-reporter/src/codex.rs`（加 `parse_context` / `read_tail` / `read_context`）
- Modify: `crates/meowo-reporter/src/agent.rs`（`CodexAgent` 覆写 `read_context`）

**Interfaces:**
- Consumes: Task 1 的 `ContextUsage`；本文件既有 `find_rollout`。
- Produces: `codex::parse_context(&str) -> Option<(i64, i64)>`（input_tokens, window）；`codex::read_context(Option<&str>, &str) -> Option<crate::agent::ContextUsage>`。

- [ ] **Step 1: 写失败测试**

在 `codex.rs` 的 `mod tests` 内追加：

```rust
    #[test]
    fn parse_context_takes_last_nonnull_token_count() {
        let rollout = r#"
{"type":"event_msg","payload":{"type":"token_count","info":null,"rate_limits":{"primary":{"used_percent":7.0}}}}
{"type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":6766,"cached_input_tokens":4480},"model_context_window":258400}}}
"#;
        // 跳过 info=null 那条；取最后一条 info 非 null 的：input_tokens=6766, window=258400。
        assert_eq!(parse_context(rollout), Some((6766, 258400)));
    }

    #[test]
    fn parse_context_none_when_no_token_count() {
        assert_eq!(parse_context(r#"{"type":"turn_context","payload":{"model":"gpt-5.5"}}"#), None);
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p meowo-reporter codex::tests::parse_context`
Expected: FAIL（`parse_context` 未定义）

- [ ] **Step 3: 实现**

在 `codex.rs` 追加（`read_model` 之后）：

```rust
/// 从 rollout 文本取**最后一条 info 非 null** 的 token_count 的 (input_tokens, model_context_window)。
/// codex 会话开头的 token_count `info` 为 null（只有 rate_limits），跳过。used 取 last_token_usage.input_tokens
/// （最近一次请求的 context 输入量，已含 cached_input_tokens）。
pub fn parse_context(content: &str) -> Option<(i64, i64)> {
    let mut last: Option<(i64, i64)> = None;
    for line in content.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let payload = v.get("payload");
        if payload.and_then(|p| p.get("type")).and_then(|t| t.as_str()) != Some("token_count") {
            continue;
        }
        let Some(info) = payload.and_then(|p| p.get("info")).filter(|i| !i.is_null()) else {
            continue;
        };
        let used = info
            .get("last_token_usage")
            .and_then(|l| l.get("input_tokens"))
            .and_then(|x| x.as_i64());
        let window = info.get("model_context_window").and_then(|x| x.as_i64());
        if let (Some(u), Some(w)) = (used, window) {
            last = Some((u, w));
        }
    }
    last
}

/// 读文件尾部最多 max_bytes 字节为 lossy UTF-8（首个半截行交给 parse_context 跳过）。
fn read_tail(path: &Path, max_bytes: u64) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let size = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(size.saturating_sub(max_bytes))).ok()?;
    let mut buf = Vec::new();
    f.take(max_bytes).read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// codex 会话最近上下文占用：定位 rollout（hook 的 transcript_path 优先，否则按 id 找），
/// 尾部读取最后一条 token_count。定位/解析失败返回 None。
pub fn read_context(transcript_path: Option<&str>, session_id: &str) -> Option<crate::agent::ContextUsage> {
    let path = transcript_path
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .or_else(|| find_rollout(session_id))?;
    const TAIL_BYTES: u64 = 256 * 1024;
    let text = read_tail(&path, TAIL_BYTES)?;
    let (used, window) = parse_context(&text)?;
    if window <= 0 {
        return None;
    }
    let pct = (used * 100 / window).clamp(0, 100);
    Some(crate::agent::ContextUsage { used_pct: pct, window })
}
```

在 `agent.rs` 的 `impl Agent for CodexAgent` 内追加：

```rust
    fn read_context(&self, ev: &HookEvent) -> Option<ContextUsage> {
        crate::codex::read_context(ev.transcript_path.as_deref(), &ev.session_id)
    }
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p meowo-reporter codex:: && cargo clippy -p meowo-reporter -- -D warnings`
Expected: PASS，clippy 零警告

- [ ] **Step 5: Commit**

```bash
git add crates/meowo-reporter/src/codex.rs crates/meowo-reporter/src/agent.rs
git commit -m "feat(codex): 从 rollout 的 token_count 读上下文占用（parse_context + read_context）"
```

---

### Task 4: dispatch 挂载 read_context（PostToolUse + Stop）+ 真机校准

**Files:**
- Modify: `crates/meowo-reporter/src/dispatch.rs`（`PostToolUse` 与 `Stop` 分支各加一段写 context）

**Interfaces:**
- Consumes: Task 1–3 的 `for_provider(provider).read_context(ev)`、`store.set_session_context`。

- [ ] **Step 1: 挂载到两个分支**

`dispatch.rs` 的 `PostToolUse` 分支：在既有 `match ev.tool_name.as_deref() { ... }` 之后、闭合 `if let Some(sid)` 之前，追加：

```rust
                if let Some(c) = crate::agent::for_provider(provider).read_context(ev) {
                    store.set_session_context(&ev.session_id, Some(c.used_pct), Some(c.window), None, now_ms)?;
                }
```

`Stop` 分支：在既有 `apply_title(...)` / `write_tab_token(...)` 之后，闭合 `if let Some(sid)` 之前，追加同一段。

（claude 的 `read_context` 返回 None → 该段对 claude 是 no-op，statusLine 链路不受影响。）

- [ ] **Step 2: 全量编译 + 测试 + clippy**

Run: `cargo test --workspace && cargo clippy --workspace -- -D warnings`
Expected: 全绿（dispatch 既有测试不受影响；claude 分支 None 不写）

- [ ] **Step 3: 真机校准（需用户配合，块C 实测前置）**

先 `bun scripts/prepare-sidecar.mjs`，再 `cd app && bun run tauri dev`。分别起一个 kimi 会话和一个 codex 会话，跑几轮让其产生 token 用量，核对：

1. kimi 卡片显示的 context% 与 kimi TUI 自身显示的百分比是否一致；
2. codex 卡片显示的 context% 与 codex TUI 自身显示的百分比是否一致。

**若偏差明显**（说明分母口径不同）：
- kimi：把 `kimi::read_context` 的分母从 `window` 改为 `window - reserved`，其中 reserved 读 config.toml `[loop_control] reserved_context_size`（同 `context_window` 的启发式解析追加一个键）。
- codex：codex TUI 常按「可用窗口」扣一个系统 baseline；若偏差稳定，在 `codex::read_context` 里对 `window` 减去实测得到的 baseline 常量（注释标注来源）。

仅调整分母常量/来源，不动结构。校准后重跑本 task 的解析单测确保仍通过。

- [ ] **Step 4: Commit**

```bash
git add crates/meowo-reporter/src/dispatch.rs crates/meowo-reporter/src/kimi.rs crates/meowo-reporter/src/codex.rs
git commit -m "feat(dispatch): PostToolUse/Stop 写 kimi/codex 上下文百分比（含真机校准）"
```

---

### Task 5: kimi 待交互（KIMI_EVENTS 加 PermissionRequest）

**前置**：块A 已执行，`app/src-tauri/src/setup/kimi.rs` 存在（含 `KIMI_EVENTS: [&str; 5]` 与 `KIMI_EVENT_WHITELIST`）。

**Files:**
- Modify: `app/src-tauri/src/setup/kimi.rs`（`KIMI_EVENTS` 5→6）

**Interfaces:**
- Consumes: 块A 的 `KIMI_EVENT_WHITELIST`（16 事件，含 `PermissionRequest`）、既有防连坐白名单绊线测试。
- dispatch 无需改动：`PermissionRequest` 分支既有（`dispatch.rs`），kimi 无 `ExitPlanMode`/`AskUserQuestion` 工具 → `tool_name` 读不到 → 缺省落 `Approval`；stdin 顶层带 `session_id`，`lookup_session` 天然满足（见 spec 技术事实）。

- [ ] **Step 1: 写失败测试**

在 `setup/kimi.rs` 的 `mod tests` 内追加：

```rust
    #[test]
    fn kimi_events_include_permission_request_all_whitelisted() {
        assert!(KIMI_EVENTS.contains(&"PermissionRequest"));
        for ev in KIMI_EVENTS {
            assert!(KIMI_EVENT_WHITELIST.contains(&ev), "{ev} 不在 kimi 16 事件白名单");
        }
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p meowo-app setup::kimi::tests::kimi_events_include_permission_request_all_whitelisted`
Expected: FAIL（`PermissionRequest` 不在 5 事件的 `KIMI_EVENTS` 里）

- [ ] **Step 3: 加事件**

`setup/kimi.rs` 的 `KIMI_EVENTS` 改为 6 项：

```rust
/// 接线事件集。PermissionRequest = kimi 交互式等待用户审批前触发（官方源码确认，observation-only），
/// 用于卡片「待交互」显示；dispatch 既有 PermissionRequest 分支复用（kimi 落 Approval）。
pub const KIMI_EVENTS: [&str; 6] = [
    "SessionStart",
    "UserPromptSubmit",
    "PostToolUse",
    "Stop",
    "SessionEnd",
    "PermissionRequest",
];
```

（若块A 的既有测试里硬编码了 `KIMI_EVENTS.len() == 5` 或断言逐条命中，同步更新为 6 项。）

- [ ] **Step 4: 跑测试与 clippy**

Run: `cargo test -p meowo-app setup::kimi && cargo clippy -p meowo-app -- -D warnings`
Expected: PASS（含白名单绊线仍绿）

- [ ] **Step 5: 真机端到端确认（块B 实测前置）**

重启 app（会自动把 `PermissionRequest` hook 幂等写入 `~/.kimi-code/config.toml`）。起一个 kimi 会话，让它执行一个需审批的命令（触发 "Run this command? Approve/Reject"）：

1. 卡片应即刻进入「待交互」（琥珀徽标）；批准/拒绝后退出（随后的 PostToolUse/UserPromptSubmit 清 `pending_review`）。
2. 同时用一个临时 dump 脚本（或看 meowo-reporter 日志）记录 PermissionRequest 收到的**最终 stdin JSON** 形态（`inputData` 嵌套 vs 展开），更新 spec 的技术事实一节备查。

若卡片无反应：确认 config.toml 已写入 PermissionRequest hook，且 stdin 顶层确有 `session_id`（不符则给 `HookEvent` 加 serde `alias`——但源码已确认顶层字段，一般无需）。

- [ ] **Step 6: Commit**

```bash
git add app/src-tauri/src/setup/kimi.rs
git commit -m "feat(setup): kimi 接线加 PermissionRequest 事件（卡片待交互显示）"
```

---

## Self-Review

- **Spec 覆盖**：块C 的 `read_context` 抽象（Task 1）、kimi 数据源（Task 2）、codex 数据源（Task 3）、dispatch 挂 PostToolUse+Stop（Task 4）、算法校准（Task 4 Step 3）；块B 的 KIMI_EVENTS+PermissionRequest（Task 5）、dispatch 复用（Task 5 说明）、端到端确认（Task 5 Step 5）——spec「决议记录」5 条全覆盖。codex 待交互属块A、claude context% 属范围外，均未纳入（与 spec 一致）。
- **类型一致性**：`ContextUsage { used_pct: i64, window: i64 }` 在 Task 1 定义，Task 2/3 构造、Task 4 消费一致；`set_session_context(&str, Option<i64>, Option<i64>, Option<&str>, i64)` 与 Task 4 调用一致；`read_context` 签名（kimi 单参 session_id、codex 双参 transcript_path+session_id）与 agent.rs 委托一致。
- **无占位符**：每个 code step 均有完整可编译代码；校准/端到端为标注清楚的真机步骤（依赖运行环境，非代码占位）。
- **已知不确定点显式处理**：context% 分母校准（Task 4 Step 3 给出 kimi/codex 各自调整点）；PermissionRequest 最终 stdin 形态（Task 5 Step 5 dump 记录，且论证不影响 Approval 路径）。

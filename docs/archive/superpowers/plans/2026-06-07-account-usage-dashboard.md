# 设置页「账号」分区 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在设置窗口新增「账号」分区，展示 Claude Code 账号、实时用量配额（5h/7d %）、每日用量历史；用量走「先缓存后请求」，token 过期自动刷新。

**Architecture:** 新增 `account` 模块（Windows 实现 + 非 Windows 桩），含可单测纯函数（解析 usage/daily、判过期、合并凭据）与 I/O（读 `~/.claude.json`/`.credentials.json`/`stats-cache.json`、调 `/api/oauth/usage`、刷新 token 原子写回）。两个 Tauri 命令 `get_account`（瞬时缓存）/`refresh_usage`（联网）。设置页加第三个分区。

**Tech Stack:** Rust（serde_json、ureq 阻塞 HTTP）、Tauri v2、React + TypeScript。Windows-only。

> 说明：纯解析/判定/合并逻辑用 TDD 单测覆盖（Task 1）；文件 I/O、网络、token 刷新写回是外部依赖，靠编译 + clippy + 手测（Task 2/4）。

---

## 文件结构

- `app/src-tauri/src/account.rs`（新建）：类型 + 纯函数（Task 1）+ I/O/网络/命令辅助（Task 2）。
- `app/src-tauri/src/lib.rs`（改）：`mod account;`、注册 `get_account`/`refresh_usage` 命令。
- `app/src-tauri/Cargo.toml`（改）：windows target 增 `ureq`。
- `app/src/api.ts`（改）：类型 + `getAccount`/`refreshUsage`。
- `app/src/views/About.tsx`（改）：新增「账号」分区 + `AccountSection`。
- `app/src/styles.css`（改）：账号卡 / 用量条 / 每日列表样式。

---

## Task 1: account.rs 类型 + 纯解析函数（TDD）

**Files:**
- Create: `app/src-tauri/src/account.rs`
- Modify: `app/src-tauri/src/lib.rs`（加 `mod account;`）

- [ ] **Step 1: 新建 `account.rs`，写类型 + 纯函数 + 测试**

```rust
//! Claude Code 账号、实时用量、每日用量的读取与解析。
//! 纯解析/判定/合并函数在此可单测；I/O 与网络见同文件后半部分。
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Account {
    pub email: String,
    pub display_name: String,
    pub organization: Option<String>,
    pub plan: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UsageWindow {
    pub utilization: f64,
    pub resets_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Usage {
    pub five_hour: Option<UsageWindow>,
    pub seven_day: Option<UsageWindow>,
    pub seven_day_opus: Option<UsageWindow>,
    pub seven_day_sonnet: Option<UsageWindow>,
    pub extra_usage_enabled: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DailyEntry {
    pub date: String,
    pub message_count: i64,
    pub session_count: i64,
    pub tokens: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DailyStats {
    pub days: Vec<DailyEntry>,
    pub last_computed_date: String,
}

/// 从 ~/.claude.json 的根 JSON 解析账号（取 oauthAccount）。无则 None。
pub fn parse_account(root: &serde_json::Value) -> Option<Account> {
    let a = root.get("oauthAccount")?;
    let email = a.get("emailAddress").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if email.is_empty() {
        return None;
    }
    let display_name = a
        .get("displayName")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(&email)
        .to_string();
    let organization = a
        .get("organizationName")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    // 套餐标签：优先 seatTier，否则 billingType。
    let plan = ["seatTier", "billingType"]
        .iter()
        .find_map(|k| a.get(*k).and_then(|v| v.as_str()).filter(|s| !s.is_empty()))
        .map(|s| s.to_string());
    Some(Account { email, display_name, organization, plan })
}

/// 解析 /api/oauth/usage 响应。各 bucket 可能为 null/缺失 → Option。
pub fn parse_usage(v: &serde_json::Value) -> Usage {
    fn win(v: &serde_json::Value, key: &str) -> Option<UsageWindow> {
        let w = v.get(key)?;
        if w.is_null() {
            return None;
        }
        let utilization = w.get("utilization").and_then(|x| x.as_f64())?;
        let resets_at = w.get("resets_at").and_then(|x| x.as_str()).unwrap_or("").to_string();
        Some(UsageWindow { utilization, resets_at })
    }
    let extra_usage_enabled = v
        .get("extra_usage")
        .and_then(|e| e.get("is_enabled"))
        .and_then(|x| x.as_bool())
        .unwrap_or(false);
    Usage {
        five_hour: win(v, "five_hour"),
        seven_day: win(v, "seven_day"),
        seven_day_opus: win(v, "seven_day_opus"),
        seven_day_sonnet: win(v, "seven_day_sonnet"),
        extra_usage_enabled,
    }
}

/// 从 stats-cache.json 根 JSON 取最近 max_days 天（dailyActivity ⨝ dailyModelTokens by date）。
pub fn parse_daily(root: &serde_json::Value, max_days: usize) -> Option<DailyStats> {
    let activity = root.get("dailyActivity")?.as_array()?;
    let last_computed_date = root
        .get("lastComputedDate")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    // date -> 该日各模型 token 求和
    let mut tokens_by_date: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    if let Some(dmt) = root.get("dailyModelTokens").and_then(|v| v.as_array()) {
        for e in dmt {
            let date = e.get("date").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let sum: i64 = e
                .get("tokensByModel")
                .and_then(|m| m.as_object())
                .map(|m| m.values().filter_map(|x| x.as_i64()).sum())
                .unwrap_or(0);
            *tokens_by_date.entry(date).or_insert(0) += sum;
        }
    }
    let mut days: Vec<DailyEntry> = activity
        .iter()
        .map(|e| {
            let date = e.get("date").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let tokens = tokens_by_date.get(&date).copied().unwrap_or(0);
            DailyEntry {
                message_count: e.get("messageCount").and_then(|v| v.as_i64()).unwrap_or(0),
                session_count: e.get("sessionCount").and_then(|v| v.as_i64()).unwrap_or(0),
                tokens,
                date,
            }
        })
        .collect();
    // 取最近 max_days（数组按日期升序，取末尾）。
    if days.len() > max_days {
        days = days.split_off(days.len() - max_days);
    }
    Some(DailyStats { days, last_computed_date })
}

/// token 是否需要刷新：到期前留 60s 余量。
pub fn is_token_expired(expires_at_ms: i64, now_ms: i64) -> bool {
    now_ms >= expires_at_ms - 60_000
}

/// 把新 token 合并进原 credentials JSON：只改 claudeAiOauth.{accessToken,refreshToken,expiresAt}，
/// 保留 mcpOAuth 等其它所有字段。返回新 JSON。
pub fn merge_credentials(
    original: &serde_json::Value,
    access_token: &str,
    refresh_token: &str,
    expires_at_ms: i64,
) -> serde_json::Value {
    let mut out = original.clone();
    let obj = out.as_object_mut();
    if let Some(obj) = obj {
        let oauth = obj
            .entry("claudeAiOauth")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(o) = oauth.as_object_mut() {
            o.insert("accessToken".into(), serde_json::json!(access_token));
            o.insert("refreshToken".into(), serde_json::json!(refresh_token));
            o.insert("expiresAt".into(), serde_json::json!(expires_at_ms));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_account_extracts_fields() {
        let root = json!({"oauthAccount":{"emailAddress":"a@b.com","displayName":"Larry","organizationName":"Acme","seatTier":"max","billingType":"subscription"}});
        let a = parse_account(&root).unwrap();
        assert_eq!(a.email, "a@b.com");
        assert_eq!(a.display_name, "Larry");
        assert_eq!(a.organization.as_deref(), Some("Acme"));
        assert_eq!(a.plan.as_deref(), Some("max"));
    }

    #[test]
    fn parse_account_none_without_email() {
        assert!(parse_account(&json!({"oauthAccount":{}})).is_none());
        assert!(parse_account(&json!({})).is_none());
    }

    #[test]
    fn parse_usage_full_and_nulls() {
        let v = json!({
            "five_hour":{"utilization":13.0,"resets_at":"2026-06-07T08:50:01Z"},
            "seven_day":{"utilization":19.0,"resets_at":"2026-06-11T12:00:00Z"},
            "seven_day_opus":null,
            "seven_day_sonnet":{"utilization":2.0,"resets_at":"x"},
            "extra_usage":{"is_enabled":false}
        });
        let u = parse_usage(&v);
        assert_eq!(u.five_hour.as_ref().unwrap().utilization, 13.0);
        assert_eq!(u.five_hour.as_ref().unwrap().resets_at, "2026-06-07T08:50:01Z");
        assert!(u.seven_day_opus.is_none());
        assert_eq!(u.seven_day_sonnet.as_ref().unwrap().utilization, 2.0);
        assert!(!u.extra_usage_enabled);
    }

    #[test]
    fn parse_usage_missing_fields_degrade() {
        let u = parse_usage(&json!({}));
        assert!(u.five_hour.is_none() && u.seven_day.is_none() && !u.extra_usage_enabled);
    }

    #[test]
    fn parse_daily_joins_and_limits() {
        let root = json!({
            "lastComputedDate":"2026-05-17",
            "dailyActivity":[
                {"date":"2026-05-15","messageCount":10,"sessionCount":1,"toolCallCount":5},
                {"date":"2026-05-16","messageCount":20,"sessionCount":2,"toolCallCount":8},
                {"date":"2026-05-17","messageCount":40,"sessionCount":4,"toolCallCount":34}
            ],
            "dailyModelTokens":[
                {"date":"2026-05-16","tokensByModel":{"opus":100,"sonnet":50}},
                {"date":"2026-05-17","tokensByModel":{"opus":8568}}
            ]
        });
        let d = parse_daily(&root, 2).unwrap();
        assert_eq!(d.last_computed_date, "2026-05-17");
        assert_eq!(d.days.len(), 2); // 取最近 2 天 → 05-16, 05-17
        assert_eq!(d.days[0].date, "2026-05-16");
        assert_eq!(d.days[0].tokens, 150);
        assert_eq!(d.days[1].date, "2026-05-17");
        assert_eq!(d.days[1].tokens, 8568);
        assert_eq!(d.days[1].message_count, 40);
    }

    #[test]
    fn is_token_expired_margin() {
        let now = 1_000_000_000_000i64;
        assert!(is_token_expired(now, now)); // 已过期
        assert!(is_token_expired(now + 30_000, now)); // 30s 内 → 视为需刷新
        assert!(!is_token_expired(now + 120_000, now)); // 还有 2 分钟 → 不刷
    }

    #[test]
    fn merge_credentials_preserves_other_fields() {
        let original = json!({
            "mcpOAuth":{"some":"value"},
            "claudeAiOauth":{"accessToken":"old","refreshToken":"oldr","expiresAt":1,"scopes":["a"],"subscriptionType":"max"}
        });
        let merged = merge_credentials(&original, "newA", "newR", 999);
        assert_eq!(merged["mcpOAuth"]["some"], "value"); // 其它字段保留
        assert_eq!(merged["claudeAiOauth"]["accessToken"], "newA");
        assert_eq!(merged["claudeAiOauth"]["refreshToken"], "newR");
        assert_eq!(merged["claudeAiOauth"]["expiresAt"], 999);
        assert_eq!(merged["claudeAiOauth"]["scopes"][0], "a"); // claudeAiOauth 内其它字段保留
        assert_eq!(merged["claudeAiOauth"]["subscriptionType"], "max");
    }
}
```

- [ ] **Step 2: 在 lib.rs 注册模块**

`app/src-tauri/src/lib.rs` 顶部（与其它 `use`/模块声明同处）加：

```rust
mod account;
```

- [ ] **Step 3: 运行测试，确认通过**

Run: `cargo test -p meowo-app account`
Expected: 7 个 account 测试通过（首次会因 `mod account` 引入而编译）。

- [ ] **Step 4: clippy**

Run: `cargo clippy -p meowo-app -- -D warnings`
Expected: 无警告。

- [ ] **Step 5: 提交**

```bash
git add app/src-tauri/src/account.rs app/src-tauri/src/lib.rs
git commit -m "feat(app): account 模块——账号/用量/每日 纯解析函数 + 单测"
```

---

## Task 2: account.rs I/O + 网络 + token 刷新 + 命令

**Files:**
- Modify: `app/src-tauri/Cargo.toml`（加 ureq）
- Modify: `app/src-tauri/src/account.rs`（I/O + 网络）
- Modify: `app/src-tauri/src/lib.rs`（命令 + 注册）

- [ ] **Step 1: 加 ureq 依赖**

`app/src-tauri/Cargo.toml` 的 `[target.'cfg(target_os = "windows")'.dependencies]` 段增：

```toml
ureq = { version = "2", features = ["json"] }
```

- [ ] **Step 2: 在 account.rs 追加 I/O / 网络 / 命令辅助**

在 `account.rs`（测试模块之前）追加：

```rust
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Claude Code 公开 OAuth client id。
const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const OAUTH_BETA: &str = "oauth-2025-04-20";
const HTTP_TIMEOUT: Duration = Duration::from_secs(6);

/// 联网拉取并写缓存后返回的整体载荷（命令返回给前端）。
#[derive(Debug, Clone, Serialize)]
pub struct AccountPayload {
    pub account: Option<Account>,
    pub daily: Option<DailyStats>,
    pub usage: Option<Usage>,
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn home_dir() -> Option<PathBuf> {
    std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).ok().map(PathBuf::from)
}

fn claude_json_path() -> Option<PathBuf> { home_dir().map(|h| h.join(".claude.json")) }
fn credentials_path() -> Option<PathBuf> { home_dir().map(|h| h.join(".claude").join(".credentials.json")) }
fn stats_cache_path() -> Option<PathBuf> { home_dir().map(|h| h.join(".claude").join("stats-cache.json")) }
fn usage_cache_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("MEOWO_DB") {
        return Some(PathBuf::from(p).with_file_name("usage-cache.json"));
    }
    home_dir().map(|h| h.join(".meowo").join("usage-cache.json"))
}

fn read_json(path: &PathBuf) -> Option<serde_json::Value> {
    let s = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&s).ok()
}

/// 读账号（~/.claude.json）。
pub fn read_account() -> Option<Account> {
    parse_account(&read_json(&claude_json_path()?)?)
}

/// 读每日用量（stats-cache.json，最近 14 天）。
pub fn read_daily() -> Option<DailyStats> {
    parse_daily(&read_json(&stats_cache_path()?)?, 14)
}

/// 读上次缓存的用量快照（~/.meowo/usage-cache.json 的 `usage` 字段）。
pub fn read_cached_usage() -> Option<Usage> {
    let v = read_json(&usage_cache_path()?)?;
    serde_json::from_value(v.get("usage")?.clone()).ok()
}

fn write_cached_usage(usage: &Usage) {
    if let Some(p) = usage_cache_path() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let body = serde_json::json!({ "usage": usage, "fetched_at": now_ms() });
        if let Ok(s) = serde_json::to_string(&body) {
            let _ = std::fs::write(&p, s);
        }
    }
}

/// 原子写回 credentials 文件（临时文件 + rename）。
fn write_credentials_atomic(path: &PathBuf, value: &serde_json::Value) -> Result<(), String> {
    let body = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    Ok(())
}

/// 确保有有效 access token：未过期直接返回；过期则刷新并原子写回，再返回新 token。
fn ensure_valid_token() -> Result<String, String> {
    let path = credentials_path().ok_or("无 HOME")?;
    let root = read_json(&path).ok_or("读不到 .credentials.json（未登录 Claude Code？）")?;
    let oauth = root.get("claudeAiOauth").ok_or("凭据缺 claudeAiOauth")?;
    let access = oauth.get("accessToken").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let refresh = oauth.get("refreshToken").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let expires_at = oauth.get("expiresAt").and_then(|v| v.as_i64()).unwrap_or(0);

    if !access.is_empty() && !is_token_expired(expires_at, now_ms()) {
        return Ok(access);
    }
    // 刷新
    if refresh.is_empty() {
        return Err("token 已过期且无 refreshToken".into());
    }
    let resp = ureq::post(TOKEN_URL)
        .timeout(HTTP_TIMEOUT)
        .send_json(serde_json::json!({
            "grant_type": "refresh_token",
            "refresh_token": refresh,
            "client_id": CLIENT_ID,
        }))
        .map_err(|e| format!("刷新 token 失败：{e}"))?;
    let body: serde_json::Value = resp.into_json().map_err(|e| e.to_string())?;
    let new_access = body.get("access_token").and_then(|v| v.as_str()).ok_or("刷新响应缺 access_token")?.to_string();
    let new_refresh = body
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .unwrap_or(&refresh) // 有的实现不轮换 refresh，沿用旧的
        .to_string();
    let expires_in = body.get("expires_in").and_then(|v| v.as_i64()).unwrap_or(3600);
    let new_expires_at = now_ms() + expires_in * 1000;

    let merged = merge_credentials(&root, &new_access, &new_refresh, new_expires_at);
    write_credentials_atomic(&path, &merged)?;
    Ok(new_access)
}

/// 联网拉实时用量（含按需刷新 token），成功则写缓存。
pub fn fetch_usage_live() -> Result<Usage, String> {
    let token = ensure_valid_token()?;
    let resp = ureq::get(USAGE_URL)
        .timeout(HTTP_TIMEOUT)
        .set("Authorization", &format!("Bearer {token}"))
        .set("anthropic-beta", OAUTH_BETA)
        .call()
        .map_err(|e| format!("请求用量失败：{e}"))?;
    let v: serde_json::Value = resp.into_json().map_err(|e| e.to_string())?;
    let usage = parse_usage(&v);
    write_cached_usage(&usage);
    Ok(usage)
}

/// 距上次缓存写入是否在 fresh_ms 内（用于 60s 限频）。
fn cache_is_fresh(fresh_ms: i64) -> bool {
    usage_cache_path()
        .and_then(|p| read_json(&p))
        .and_then(|v| v.get("fetched_at").and_then(|x| x.as_i64()))
        .map(|t| now_ms() - t < fresh_ms)
        .unwrap_or(false)
}

/// get_account：账号 + 每日 + 缓存用量（瞬时，不联网）。
pub fn get_account_payload() -> AccountPayload {
    AccountPayload { account: read_account(), daily: read_daily(), usage: read_cached_usage() }
}

/// refresh_usage：60s 内有新鲜缓存则直接返回缓存，否则联网拉取。
pub fn refresh_usage_payload() -> Result<Usage, String> {
    if cache_is_fresh(60_000) {
        if let Some(u) = read_cached_usage() {
            return Ok(u);
        }
    }
    fetch_usage_live()
}
```

> 上述 I/O/网络代码整体放在 `#[cfg(target_os = "windows")]` 之外亦可编译（ureq 跨平台），但本项目仅 Windows。为简洁不再加平台桩——若将来需非 Windows 编译，把 ureq 调用处 cfg-gate。当前 CI 为 windows-latest，无需处理。

- [ ] **Step 3: 在 lib.rs 加两个命令并注册**

`app/src-tauri/src/lib.rs` 加命令（放在其它 `#[tauri::command]` 附近）：

```rust
#[tauri::command]
fn get_account() -> account::AccountPayload {
    account::get_account_payload()
}

#[tauri::command]
async fn refresh_usage() -> Result<account::Usage, String> {
    // 阻塞 HTTP 放到 blocking 线程，避免占用异步运行时。
    tauri::async_runtime::spawn_blocking(account::refresh_usage_payload)
        .await
        .map_err(|e| e.to_string())?
}
```

在 `tauri::generate_handler![ ... ]` 列表里加入 `get_account, refresh_usage`（注意已有项末尾逗号）。

- [ ] **Step 4: 编译 + 测试 + clippy**

Run: `cargo build -p meowo-app && cargo test -p meowo-app && cargo clippy -p meowo-app -- -D warnings`
Expected: 通过，无警告（首次会拉取 ureq 及其依赖）。

- [ ] **Step 5: 提交**

```bash
git add app/src-tauri/Cargo.toml app/src-tauri/src/account.rs app/src-tauri/src/lib.rs
git commit -m "feat(app): account 用量读取/联网/token 刷新 + get_account/refresh_usage 命令"
```

---

## Task 3: 前端「账号」分区

**Files:**
- Modify: `app/src/api.ts`
- Modify: `app/src/views/About.tsx`
- Modify: `app/src/styles.css`

- [ ] **Step 1: api.ts 加类型与命令**

`app/src/api.ts` 末尾追加：

```ts
export type UsageWindow = { utilization: number; resets_at: string };
export type Usage = {
  five_hour: UsageWindow | null;
  seven_day: UsageWindow | null;
  seven_day_opus: UsageWindow | null;
  seven_day_sonnet: UsageWindow | null;
  extra_usage_enabled: boolean;
};
export type Account = {
  email: string;
  display_name: string;
  organization: string | null;
  plan: string | null;
};
export type DailyEntry = { date: string; message_count: number; session_count: number; tokens: number };
export type DailyStats = { days: DailyEntry[]; last_computed_date: string };
export type AccountPayload = { account: Account | null; daily: DailyStats | null; usage: Usage | null };

export function getAccount(): Promise<AccountPayload> {
  return invoke("get_account");
}
export function refreshUsage(): Promise<Usage> {
  return invoke("refresh_usage");
}
```

- [ ] **Step 2: About.tsx 加「账号」分区**

(a) 把 `type Section = "general" | "about";` 改为：

```tsx
type Section = "general" | "account" | "about";
```

(b) 在 `import { getSettings, setSettings, type Settings } from "../api";` 这行追加导入：

```tsx
import { getAccount, refreshUsage, type AccountPayload, type Usage } from "../api";
```

(c) 在 `IconInfo` 函数后面新增一个用户图标 + `AccountSection` 组件：

```tsx
function IconUser() {
  return (
    <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
      <circle cx="12" cy="8" r="4" />
      <path d="M4 21v-1a6 6 0 0 1 6-6h4a6 6 0 0 1 6 6v1" />
    </svg>
  );
}

function fmtResetIn(iso: string): string {
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return "";
  const min = Math.round((t - Date.now()) / 60000);
  if (min <= 0) return "即将重置";
  if (min < 60) return `${min} 分钟后重置`;
  return `${Math.floor(min / 60)} 小时 ${min % 60} 分后重置`;
}

function UsageBar({ label, win }: { label: string; win: { utilization: number; resets_at: string } | null }) {
  if (!win) return null;
  const pct = Math.max(0, Math.min(100, win.utilization));
  return (
    <div className="usage-row">
      <div className="usage-head">
        <span className="usage-label">{label}</span>
        <span className="usage-pct">{pct.toFixed(0)}%</span>
      </div>
      <div className="usage-track"><i style={{ width: `${pct}%` }} /></div>
      <div className="usage-reset">{fmtResetIn(win.resets_at)}</div>
    </div>
  );
}

function AccountSection() {
  const [data, setData] = useState<AccountPayload | null>(null);
  const [usage, setUsage] = useState<Usage | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [usageErr, setUsageErr] = useState(false);

  const doRefresh = () => {
    setRefreshing(true);
    setUsageErr(false);
    refreshUsage()
      .then((u) => setUsage(u))
      .catch(() => setUsageErr(true))
      .finally(() => setRefreshing(false));
  };

  useEffect(() => {
    // 先缓存后请求：getAccount 立即给账号/每日/缓存用量，再 refreshUsage 联网刷新。
    getAccount()
      .then((d) => { setData(d); setUsage(d.usage); })
      .catch(() => {});
    doRefresh();
  }, []);

  const acc = data?.account ?? null;
  const daily = data?.daily ?? null;
  const maxTok = daily ? Math.max(1, ...daily.days.map((d) => d.tokens)) : 1;

  return (
    <>
      <div className="sec-title">账号</div>
      {acc ? (
        <div className="row-card">
          <div className="row">
            <div className="row-icon"><div className="acc-avatar">{(acc.display_name || acc.email).slice(0, 1).toUpperCase()}</div></div>
            <div className="row-text">
              <div className="row-label">{acc.display_name}</div>
              <div className="row-desc">{acc.email}{acc.plan ? ` · ${acc.plan}` : ""}{acc.organization ? ` · ${acc.organization}` : ""}</div>
            </div>
          </div>
        </div>
      ) : (
        <div className="row-card"><div className="row"><div className="row-text"><div className="row-desc">未检测到 Claude Code 登录信息</div></div></div></div>
      )}

      <div className="sec-title">用量</div>
      <div className="row-card usage-card">
        <div className="usage-bar-head">
          <span className="usage-card-title">配额</span>
          <button className="sbtn" disabled={refreshing} onClick={doRefresh}>{refreshing ? "刷新中…" : "刷新"}</button>
        </div>
        {usage ? (
          <>
            <UsageBar label="5 小时窗口" win={usage.five_hour} />
            <UsageBar label="7 天窗口" win={usage.seven_day} />
            <UsageBar label="Opus · 7 天" win={usage.seven_day_opus} />
            <UsageBar label="Sonnet · 7 天" win={usage.seven_day_sonnet} />
            {usage.extra_usage_enabled && <div className="usage-extra">已开启超额用量</div>}
            {usageErr && <div className="usage-stale">最新数据刷新失败，显示的是缓存值</div>}
          </>
        ) : usageErr ? (
          <div className="usage-stale">用量暂不可用（需在终端用一次 Claude Code 或检查网络）</div>
        ) : (
          <div className="usage-stale">加载中…</div>
        )}
      </div>

      {daily && daily.days.length > 0 && (
        <>
          <div className="sec-title">每日用量</div>
          <div className="row-card">
            <div className="daily-list">
              {daily.days.map((d) => (
                <div className="daily-row" key={d.date}>
                  <span className="daily-date">{d.date.slice(5)}</span>
                  <div className="daily-track"><i style={{ width: `${Math.round((d.tokens / maxTok) * 100)}%` }} /></div>
                  <span className="daily-val">{(d.tokens / 1000).toFixed(0)}k · {d.message_count} 条</span>
                </div>
              ))}
            </div>
            <div className="sec-hint">数据截至 {daily.last_computed_date || "—"}，在终端运行 /stats 可刷新</div>
          </div>
        </>
      )}
    </>
  );
}
```

(d) 在 nav 里「通用」和「关于」之间插入「账号」按钮（在 `sec === "general"` 的 button 之后）：

```tsx
          <button className={"nav-item" + (sec === "account" ? " on" : "")} onClick={() => setSec("account")}>
            <IconUser />
            <span>账号</span>
          </button>
```

(e) 把 main-body 的三元渲染（当前 `{sec === "general" ? <GeneralSection/> : <AboutSection .../>}`）改为支持三段：

```tsx
          {sec === "general" ? (
            <GeneralSection />
          ) : sec === "account" ? (
            <AccountSection />
          ) : (
            <AboutSection status={status} newVersion={newVersion} recheck={recheck} />
          )}
```

- [ ] **Step 3: styles.css 加样式**

`app/src/styles.css` 末尾追加：

```css
/* 账号分区 */
.acc-avatar { width: 34px; height: 34px; border-radius: 50%; background: var(--cc-accent, #6b8afd); color: #fff; display: flex; align-items: center; justify-content: center; font-weight: 600; font-size: 15px; }
.usage-card { display: block; }
.usage-bar-head { display: flex; align-items: center; justify-content: space-between; margin-bottom: 8px; }
.usage-card-title { font-size: 12.5px; font-weight: 600; }
.usage-row { margin: 10px 0; }
.usage-head { display: flex; justify-content: space-between; font-size: 11.5px; }
.usage-label { color: var(--cc-text-dim); }
.usage-pct { font-weight: 600; }
.usage-track { height: 6px; border-radius: 3px; background: rgba(255,255,255,0.1); overflow: hidden; margin: 4px 0 2px; }
.usage-track > i { display: block; height: 100%; background: var(--cc-accent, #6b8afd); border-radius: 3px; }
.usage-reset { font-size: 10px; color: var(--cc-text-faint); }
.usage-extra { font-size: 11px; color: var(--cc-warn); margin-top: 6px; }
.usage-stale { font-size: 11px; color: var(--cc-text-faint); padding: 6px 0; }
.daily-list { display: flex; flex-direction: column; gap: 6px; }
.daily-row { display: flex; align-items: center; gap: 8px; font-size: 11px; }
.daily-date { width: 40px; color: var(--cc-text-dim); flex: none; }
.daily-track { flex: 1; height: 5px; border-radius: 3px; background: rgba(255,255,255,0.08); overflow: hidden; }
.daily-track > i { display: block; height: 100%; background: var(--cc-accent, #6b8afd); }
.daily-val { width: 96px; text-align: right; color: var(--cc-text-faint); flex: none; }
```

- [ ] **Step 4: 类型检查 + 前端测试**

Run（从 `app/`）: `cd app && bunx tsc --noEmit && bunx vitest run`
Expected: 无类型错误；现有测试不回归。

- [ ] **Step 5: 提交**

```bash
git add app/src/api.ts app/src/views/About.tsx app/src/styles.css
git commit -m "feat(app): 设置页新增账号分区（账号+实时用量+每日用量）"
```

---

## Task 4: 整体验证 + 文档

**Files:**
- Modify: `README.md`

- [ ] **Step 1: 全量验证**

Run（根）: `cargo test --workspace && cargo clippy --workspace -- -D warnings`
Expected: 通过，无警告。

Run（前端）: `cd app && bunx tsc --noEmit && bunx vitest run`
Expected: 通过。

- [ ] **Step 2: README 补特性**

`README.md` 的「特性」列表末尾（`- **位置/尺寸记忆**` 那条之后）加：

```markdown
- **账号与用量**：设置页「账号」分区显示当前 Claude Code 账号、实时配额（5 小时 / 7 天窗口用量 % 与重置时间）、以及每日用量；用量先显示缓存再后台刷新，登录 token 过期自动续期。
```

- [ ] **Step 3: 手动冒烟（需真实环境）**

Run: `cd app && bun run tauri dev`
验证：打开设置 → 「账号」分区显示邮箱/套餐；用量秒显缓存后刷新出 5h/7d 百分比 + 重置倒计时；每日列表显示近 14 天 + 截止日期标注；点「刷新」能重新拉取；断网/未登录时优雅降级不崩。

- [ ] **Step 4: 提交**

```bash
git add README.md
git commit -m "docs: README 补账号与用量特性"
```

---

## 自查记录

- **Spec 覆盖**：账号 read+parse → Task 1/2；实时用量端点+解析 → Task 1(parse_usage)/Task 2(fetch_usage_live)；token 过期判定+刷新+原子写回 → Task 1(is_token_expired/merge_credentials)/Task 2(ensure_valid_token/write_credentials_atomic)；每日用量 stats-cache+标注 → Task 1(parse_daily)/Task 3(UI+截止日期)；先缓存后请求 → Task 2(get_account/refresh_usage)+Task 3(useEffect 先 getAccount 再 doRefresh)；60s 限频 → Task 2(cache_is_fresh)；降级不影响其它 → Task 2(Result/Option)+Task 3(占位)；UI 分区 → Task 3。✅
- **占位符**：无 TBD/TODO；所有步骤含完整代码与命令。✅
- **类型一致**：Rust `Account/UsageWindow/Usage/DailyEntry/DailyStats/AccountPayload` 与 TS 镜像一致；`get_account`/`refresh_usage` 命令名与前端 `getAccount`/`refreshUsage` 对应；纯函数 `parse_account/parse_usage/parse_daily/is_token_expired/merge_credentials` 在 Task 2 被 I/O 层正确调用。✅

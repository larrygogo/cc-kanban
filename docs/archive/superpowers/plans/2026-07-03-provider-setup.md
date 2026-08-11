# 多 provider 自动接线（ProviderSetup）实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** meowo-app 启动时对检测到已安装的 claude/codex/kimi 幂等自动接线（hooks 指向 meowo-reporter），消除「开箱即用只对 claude 成立」的缺口。

**Architecture:** 新建 `app/src-tauri/src/setup/` 模块（模仿 `account/` 的 trait+注册表组织）：`ProviderSetup` trait + `apply_all()` 遍历。claude = 现 `ccsetup.rs` 原样平移（零行为变更）；codex = 幂等合并 `~/.codex/hooks.json` + 复算 trusted_hash 写 `config.toml [hooks.state]` 预信任；kimi = toml_edit 结构保持地合并 `~/.kimi-code/config.toml [[hooks]]`。全程 best-effort：任何读取/解析失败即放弃该 provider，绝不写坏文件、绝不影响启动。

**Tech Stack:** Rust（Tauri v2）、serde_json（已有）、新增 `toml_edit = "0.22"`（结构保持 TOML 编辑）与 `sha2 = "0.10"`（trusted_hash）。

**Spec:** `docs/superpowers/specs/2026-07-03-provider-setup-design.md`

## Global Constraints

- 代码注释与 commit message 用中文；代码本身英文。
- `cargo clippy --workspace -- -D warnings` 与 `cargo test --workspace` 必须全绿后才 commit。
- claude 接线**零行为变更**：平移后原测试原样通过，真机 `~/.claude/settings.json` 不产生任何 diff。
- 所有写入：备份一次（`<文件名>.cckb-bak`，仅当备份不存在时）→ 原子写（`fsutil::write_atomic`）。claude 保留其现有 tmp+rename 写法不改。
- 认领判定严格（解析命令、精确匹配 `--provider <x>` 余参），绝不裸 contains，绝不动用户自有 hook 条目。
- 合并逻辑保持纯函数（输入 `&mut Value`/`&mut DocumentMut` + 路径字符串，不依赖 Tauri/app 状态）——为后续项 `meowo-reporter setup` 跨 crate 迁移铺路。
- 本计划不改 `scripts/install-hooks.mjs` 功能（仅更新其内指向 ccsetup.rs 的同步注释路径）。

---

### Task 1: setup/ 模块骨架 + claude 平移（纯重构，零行为变更）

**Files:**
- Create: `app/src-tauri/src/setup/mod.rs`
- Create: `app/src-tauri/src/setup/claude.rs`（内容 = 现 `ccsetup.rs` 全文 + trait 壳）
- Delete: `app/src-tauri/src/ccsetup.rs`（git mv）
- Modify: `app/src-tauri/src/lib.rs:30`（`pub mod ccsetup;` → `pub mod setup;`）、`lib.rs:2363`（spawn 调用）
- Modify: `scripts/install-hooks.mjs`（同步注释里的 ccsetup.rs 路径 → setup/claude.rs）

**Interfaces:**
- Produces: `setup::apply_all()`；trait `ProviderSetup { key/detect/apply }`；共享工具 `setup::sibling_reporter() -> Option<String>`、`setup::backup_once(&Path)`、`setup::parse_hook_command(&str) -> Option<(String, Vec<String>)>`、`setup::claim_provider_cmd(&str, provider: &str) -> Option<String>`（后续 Task 2-5 消费）
- Consumes: `meowo_store::ProviderKey`、现有 `ccsetup.rs` 全部逻辑

- [ ] **Step 1: git mv 平移文件**

```bash
cd C:/Users/larry/Desktop/workspace/meowo
mkdir -p app/src-tauri/src/setup
git mv app/src-tauri/src/ccsetup.rs app/src-tauri/src/setup/claude.rs
```

- [ ] **Step 2: 写 setup/mod.rs（trait + 注册表 + 共享工具）**

```rust
//! provider 自动接线：启动时对检测到已安装的 AI CLI 幂等挂上 meowo-reporter hooks。
//! 组织仿 account/（trait + 静态注册表）。合并逻辑保持纯函数（不依赖 Tauri/app 状态），
//! 为后续 `meowo-reporter setup` 子命令跨 crate 迁移铺路。
pub mod claude;

/// Provider 接线抽象。Sync：以 &'static dyn 静态注册表共享。
pub trait ProviderSetup: Sync {
    fn key(&self) -> meowo_store::ProviderKey;
    /// 数据目录存在即视为已安装（各自尊重 env 覆盖）。不存在 → apply_all 跳过。
    fn detect(&self) -> bool;
    /// 幂等接线。全程 best-effort：读不到/解析失败/找不到 reporter 均静默返回，绝不 panic。
    fn apply(&self);
}

static CLAUDE_SETUP: claude::ClaudeSetup = claude::ClaudeSetup;
static ALL_SETUP: &[&dyn ProviderSetup] = &[&CLAUDE_SETUP];

/// 启动后台线程入口：逐 provider 独立 best-effort，一家失败不影响他家。
pub fn apply_all() {
    for s in ALL_SETUP {
        if s.detect() {
            s.apply();
        }
    }
}

/// app 可执行同目录的 meowo-reporter（打包态 sidecar 与 app 放一起）。
pub(crate) fn sibling_reporter() -> Option<String> {
    let bin = if cfg!(windows) { "meowo-reporter.exe" } else { "meowo-reporter" };
    let exe = std::env::current_exe().ok()?;
    let sib = exe.with_file_name(bin);
    sib.exists().then(|| sib.to_string_lossy().into_owned())
}

/// 备份一次：`<文件名>.cckb-bak` 不存在时 copy。保留最初的用户原始配置。
pub(crate) fn backup_once(path: &std::path::Path) {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return;
    };
    let bak = path.with_file_name(format!("{name}.cckb-bak"));
    if !bak.exists() {
        let _ = std::fs::copy(path, &bak);
    }
}

/// 解析 hook command 为（可执行路径, 余参）。首 token 支持带双引号或裸路径。
pub(crate) fn parse_hook_command(cmd: &str) -> Option<(String, Vec<String>)> {
    let c = cmd.trim();
    let (path, rest) = if let Some(r) = c.strip_prefix('"') {
        let end = r.find('"')?;
        (r[..end].to_string(), r[end + 1..].trim())
    } else {
        match c.split_once(char::is_whitespace) {
            Some((p, r)) => (p.to_string(), r.trim()),
            None => (c.to_string(), ""),
        }
    };
    let args = rest.split_whitespace().map(str::to_string).collect();
    Some((path, args))
}

/// 严格认领带 provider 参数的命令（codex/kimi 形态）：可执行文件名恰为 meowo-reporter[.exe]
/// 且余参恰为 ["--provider", provider]。返回可执行路径。不裸 contains，不误伤用户 hook。
pub(crate) fn claim_provider_cmd(cmd: &str, provider: &str) -> Option<String> {
    let (path, args) = parse_hook_command(cmd)?;
    let name = std::path::Path::new(&path).file_name()?.to_str()?;
    let is_reporter =
        name.eq_ignore_ascii_case("meowo-reporter") || name.eq_ignore_ascii_case("meowo-reporter.exe");
    (is_reporter && args == ["--provider", provider]).then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_provider_cmd_strict() {
        // 认领：带引号/裸路径两种形态。
        assert_eq!(
            claim_provider_cmd("\"C:/x/meowo-reporter.exe\" --provider codex", "codex").as_deref(),
            Some("C:/x/meowo-reporter.exe")
        );
        assert_eq!(
            claim_provider_cmd("C:/x/meowo-reporter.exe --provider kimi", "kimi").as_deref(),
            Some("C:/x/meowo-reporter.exe")
        );
        // 拒绝：provider 不符 / 无参数 / 多余参数 / 别的可执行 / 子串陷阱。
        assert!(claim_provider_cmd("C:/x/meowo-reporter.exe --provider codex", "kimi").is_none());
        assert!(claim_provider_cmd("\"C:/x/meowo-reporter.exe\"", "codex").is_none());
        assert!(claim_provider_cmd("C:/x/meowo-reporter.exe --provider codex --v", "codex").is_none());
        assert!(claim_provider_cmd("node meowo-reporter-notify.js --provider codex", "codex").is_none());
    }
}
```

- [ ] **Step 3: claude.rs 套 trait 壳**

在 `setup/claude.rs` 文件头部（模块注释后）追加：

```rust
/// Claude Code 的 ProviderSetup 实现：委托本文件原有 apply()（hooks + statusLine），零行为变更。
pub struct ClaudeSetup;

impl super::ProviderSetup for ClaudeSetup {
    fn key(&self) -> meowo_store::ProviderKey {
        meowo_store::ProviderKey::Claude
    }
    fn detect(&self) -> bool {
        // 与 apply() 内部「~/.claude 目录存在才创建 settings」的守卫同源。
        claude_settings_path().parent().is_some_and(|p| p.is_dir())
    }
    fn apply(&self) {
        apply();
    }
}
```

其余内容一字不动（`claude_settings_path` 已是本文件私有 fn，可直接引用）。

- [ ] **Step 4: 接线 lib.rs 与 mjs 注释**

`lib.rs:30` 改为 `pub mod setup;`；`lib.rs:2363` 改为：

```rust
            // 无感适配：幂等把 meowo-reporter 接入各 AI CLI（claude: hooks+statusLine）。后台跑，失败不影响启动。
            std::thread::spawn(setup::apply_all);
```

`scripts/install-hooks.mjs` 中指向 `ccsetup.rs` 的同步注释改指 `app/src-tauri/src/setup/claude.rs`。

- [ ] **Step 5: 全量测试验证零回归**

Run: `cargo test -p meowo-app && cargo clippy --workspace -- -D warnings`
Expected: 原 ccsetup 全部测试在 `setup::claude::tests` 下 PASS；clippy 零警告。

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "refactor(setup): ccsetup 平移为 setup/ 模块 + ProviderSetup trait（claude 零行为变更）"
```

---

### Task 2: codex hooks.json 幂等合并（纯函数，TDD）

**Files:**
- Create: `app/src-tauri/src/setup/codex.rs`（本 task 只有纯函数与测试，apply 在 Task 4）
- Modify: `app/src-tauri/src/setup/mod.rs`（`pub mod codex;`）

**Interfaces:**
- Consumes: `super::claim_provider_cmd`
- Produces: `codex::CODEX_EVENTS: [&str; 5]`、`codex::ensure_codex_hooks(root: &mut serde_json::Value, reporter_native: &str) -> bool`、`codex::reporter_path_from_codex(root: &Value) -> Option<String>`、`codex::claimed_codex_entries(root: &Value) -> Vec<(String, String, u64)>`（Task 3/4 消费）

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ensure_codex_hooks_adds_all_events_when_empty() {
        let mut v = json!({});
        assert!(ensure_codex_hooks(&mut v, "C:/x/meowo-reporter.exe"));
        for ev in CODEX_EVENTS {
            let cmd = v["hooks"][ev][0]["hooks"][0]["command"].as_str().unwrap();
            assert_eq!(cmd, "\"C:/x/meowo-reporter.exe\" --provider codex");
            assert_eq!(v["hooks"][ev][0]["hooks"][0]["timeout"], 5);
        }
        // 幂等：二跑无改动。
        assert!(!ensure_codex_hooks(&mut v, "C:/x/meowo-reporter.exe"));
    }

    #[test]
    fn ensure_codex_hooks_adopts_manual_wiring_and_fills_missing() {
        // 精确复刻本机手工接线形态：裸路径命令、3 事件、Stop timeout=10。
        let dev = "C:/Users/larry/Desktop/workspace/meowo/target/release/meowo-reporter.exe";
        let entry = |t: u64| json!({ "hooks": [{ "type": "command", "command": format!("{dev} --provider codex"), "timeout": t }] });
        let mut v = json!({ "hooks": {
            "SessionStart": [entry(5)], "UserPromptSubmit": [entry(5)], "Stop": [entry(10)]
        }});
        assert!(ensure_codex_hooks(&mut v, dev)); // 补 PostToolUse/PermissionRequest → 有改动
        // 既有条目原样保留（裸路径不被改写为引号形态、timeout 10 不动）——幂等按解析后内容判定。
        assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["command"], format!("{dev} --provider codex"));
        assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["timeout"], 10);
        // 新事件已补齐。
        assert!(v["hooks"]["PostToolUse"][0]["hooks"][0]["command"].as_str().unwrap().contains("--provider codex"));
        assert!(v["hooks"]["PermissionRequest"].is_array());
        assert!(!ensure_codex_hooks(&mut v, dev));
    }

    #[test]
    fn ensure_codex_hooks_updates_stale_path_keeps_user_hooks() {
        let mut v = json!({ "hooks": { "Stop": [
            { "hooks": [{ "type": "command", "command": "node my-notify.js" }] },
            { "hooks": [{ "type": "command", "command": "\"C:/old/meowo-reporter.exe\" --provider codex", "timeout": 5 }] }
        ]}});
        assert!(ensure_codex_hooks(&mut v, "C:/new/meowo-reporter.exe"));
        assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["command"], "node my-notify.js"); // 用户 hook 不动
        assert_eq!(v["hooks"]["Stop"][1]["hooks"][0]["command"], "\"C:/new/meowo-reporter.exe\" --provider codex");
        assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 2); // 不重复追加
    }

    #[test]
    fn reporter_path_and_claimed_entries_extraction() {
        let mut v = json!({});
        ensure_codex_hooks(&mut v, "C:/x/meowo-reporter.exe");
        assert_eq!(reporter_path_from_codex(&v).as_deref(), Some("C:/x/meowo-reporter.exe"));
        let entries = claimed_codex_entries(&v);
        assert_eq!(entries.len(), 5);
        assert!(entries.iter().all(|(_, cmd, t)| cmd.contains("--provider codex") && *t == 5));
        assert!(entries.iter().any(|(ev, _, _)| ev == "PermissionRequest"));
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p meowo-app setup::codex -- --nocapture`
Expected: FAIL（`ensure_codex_hooks` 未定义，编译错误）

- [ ] **Step 3: 最小实现**

```rust
//! codex（OpenAI Codex CLI）自动接线：幂等合并 `~/.codex/hooks.json`。
//! codex hooks 格式 Claude 同款但顶层只允许 {"hooks": {...}}（deny_unknown_fields），
//! 条目无 matcher 键；信任机制（trusted_hash）见本模块 hash/apply 部分（Task 3/4）。
use serde_json::{json, Value};

/// 接线事件集：dispatch 消化面 ∩ codex 0.142 支持面。无 SessionEnd（codex 不支持，
/// 会话收尾靠 Stop + liveness）；不配 PreToolUse（其 matcher 目标是 claude 专属工具）。
pub const CODEX_EVENTS: [&str; 5] =
    ["SessionStart", "UserPromptSubmit", "PostToolUse", "Stop", "PermissionRequest"];

/// 从 hooks.json 找出已配置的 meowo-reporter 路径（复用现有路径优先的解析源）。
pub fn reporter_path_from_codex(root: &Value) -> Option<String> {
    let hooks = root.get("hooks")?.as_object()?;
    for (_ev, arr) in hooks {
        for entry in arr.as_array().into_iter().flatten() {
            for h in entry.get("hooks").and_then(|x| x.as_array()).into_iter().flatten() {
                if let Some(p) = h
                    .get("command")
                    .and_then(|c| c.as_str())
                    .and_then(|c| super::claim_provider_cmd(c, "codex"))
                {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// 幂等合并：CODEX_EVENTS 逐事件确保挂上 meowo-reporter。已有认领条目 → 仅路径不符时更新
/// （解析后内容判定，裸路径/引号形态视为等价，不无谓改写用户在用的配置）；缺 → 追加。
/// 返回是否有改动。
pub fn ensure_codex_hooks(root: &mut Value, reporter_native: &str) -> bool {
    let desired_cmd = format!("\"{reporter_native}\" --provider codex");
    let mut changed = false;
    if !root.get("hooks").map(|h| h.is_object()).unwrap_or(false) {
        root["hooks"] = json!({});
        changed = true;
    }
    for ev in CODEX_EVENTS {
        let arr = root["hooks"]
            .as_object_mut()
            .unwrap()
            .entry(ev.to_string())
            .or_insert_with(|| json!([]));
        let arr = match arr.as_array_mut() {
            Some(a) => a,
            None => {
                *arr = json!([]);
                arr.as_array_mut().unwrap()
            }
        };
        let mut found = false;
        for entry in arr.iter_mut() {
            let Some(hs) = entry.get_mut("hooks").and_then(|x| x.as_array_mut()) else {
                continue;
            };
            for h in hs.iter_mut() {
                let claimed = h
                    .get("command")
                    .and_then(|c| c.as_str())
                    .and_then(|c| super::claim_provider_cmd(c, "codex"));
                if let Some(path) = claimed {
                    found = true;
                    if path != reporter_native {
                        h["command"] = json!(desired_cmd);
                        changed = true;
                    }
                }
            }
        }
        if !found {
            arr.push(json!({ "hooks": [{ "type": "command", "command": desired_cmd, "timeout": 5 }] }));
            changed = true;
        }
    }
    changed
}

/// 提取合并后全部我方认领条目 (CamelCase 事件, command 原串, timeout)——
/// 供 trusted_hash 按**实际写出的内容**计算（含既有条目的原样 timeout，如本机 Stop=10）。
pub fn claimed_codex_entries(root: &Value) -> Vec<(String, String, u64)> {
    let mut out = Vec::new();
    let Some(hooks) = root.get("hooks").and_then(|h| h.as_object()) else {
        return out;
    };
    for (ev, arr) in hooks {
        for entry in arr.as_array().into_iter().flatten() {
            for h in entry.get("hooks").and_then(|x| x.as_array()).into_iter().flatten() {
                let Some(cmd) = h.get("command").and_then(|c| c.as_str()) else {
                    continue;
                };
                if super::claim_provider_cmd(cmd, "codex").is_some() {
                    let t = h.get("timeout").and_then(|t| t.as_u64()).unwrap_or(600);
                    out.push((ev.clone(), cmd.to_string(), t));
                }
            }
        }
    }
    out
}
```

`mod.rs` 加 `pub mod codex;`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p meowo-app setup::codex && cargo clippy --workspace -- -D warnings`
Expected: 4 个测试 PASS，clippy 零警告

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(setup): codex hooks.json 幂等合并纯函数（认领/接管/补齐/幂等）"
```

---

### Task 3: codex trusted_hash 计算（TDD，以本机真实哈希为测试向量）

**Files:**
- Modify: `app/src-tauri/src/setup/codex.rs`
- Modify: `app/src-tauri/Cargo.toml`（`[dependencies]` 加 `sha2 = "0.10"`）

**Interfaces:**
- Produces: `codex::snake_event(&str) -> &'static str`、`codex::codex_hook_hash(event_snake: &str, command: &str, timeout: u64) -> String`（返回 `"sha256:<hex>"`）（Task 4 消费）

- [ ] **Step 1: 写失败测试（本机 2026-07-03 实测三向量，算法漂移时此测试即绊线）**

```rust
    #[test]
    fn codex_hook_hash_matches_real_machine_vectors() {
        // 三条向量取自本机 ~/.codex/config.toml 的真实 trusted_hash（codex-cli 0.142.3），
        // 算法：canonical JSON（key 字母序、紧凑）SHA-256。codex 升级若改算法，此测试变红。
        let cmd = "C:/Users/larry/Desktop/workspace/meowo/target/release/meowo-reporter.exe --provider codex";
        assert_eq!(
            codex_hook_hash("session_start", cmd, 5),
            "sha256:6a10ea73fc05fb9000b03c3a8d6f54375278aca7cb375d0915be8294fa29c95b"
        );
        assert_eq!(
            codex_hook_hash("user_prompt_submit", cmd, 5),
            "sha256:b304e91ff6e3b6baf2b37c64498582b9ea12d5847d773bc5a632da317ffb8564"
        );
        assert_eq!(
            codex_hook_hash("stop", cmd, 10),
            "sha256:e91f37fe561ddc6784ec8c3fe559f90590a708429a86aab477fb456dec9738d7"
        );
    }

    #[test]
    fn snake_event_covers_all_codex_events() {
        for ev in CODEX_EVENTS {
            assert!(!snake_event(ev).is_empty(), "{ev} 缺 snake_case 映射");
        }
        assert_eq!(snake_event("PermissionRequest"), "permission_request");
        assert_eq!(snake_event("Unknown"), "");
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p meowo-app setup::codex::tests::codex_hook_hash`
Expected: FAIL（函数未定义）

- [ ] **Step 3: 实现（手工拼 canonical 串——serde_json Map 键序受 preserve_order 特性统一影响，不可依赖）**

```rust
/// CamelCase 事件名 → codex hooks.state 键用的 snake_case 标签。未知返回 ""。
pub(crate) fn snake_event(ev: &str) -> &'static str {
    match ev {
        "SessionStart" => "session_start",
        "UserPromptSubmit" => "user_prompt_submit",
        "PostToolUse" => "post_tool_use",
        "Stop" => "stop",
        "PermissionRequest" => "permission_request",
        _ => "",
    }
}

/// codex trusted_hash：对归一化身份对象的 canonical JSON（key 字母序、紧凑）做 SHA-256。
/// 算法为 codex 内部实现（源码 fingerprint.rs），本机 0.142.3 三真实向量验证命中；
/// 漂移时向量测试变红，运行期兜底 = codex TUI 一次 Trust all，无损。
/// 手工 format! 拼串而非 serde_json 对象：杜绝 preserve_order 特性统一导致的键序漂移。
pub(crate) fn codex_hook_hash(event_snake: &str, command: &str, timeout: u64) -> String {
    use sha2::{Digest, Sha256};
    let canon = format!(
        r#"{{"event_name":{ev},"hooks":[{{"async":false,"command":{cmd},"timeout":{timeout},"type":"command"}}]}}"#,
        ev = serde_json::to_string(event_snake).unwrap_or_default(),
        cmd = serde_json::to_string(command).unwrap_or_default(),
    );
    format!("sha256:{:x}", Sha256::digest(canon.as_bytes()))
}
```

Cargo.toml `[dependencies]` 追加：

```toml
# codex hook 预信任的 trusted_hash 计算（SHA-256）。
sha2 = "0.10"
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p meowo-app setup::codex && cargo clippy --workspace -- -D warnings`
Expected: 全 PASS（含三向量命中）

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(setup): codex trusted_hash 复算（本机三真实向量钉死算法）"
```

---

### Task 4: codex 预信任写入 + apply 组装 + 注册

**Files:**
- Modify: `app/src-tauri/src/setup/codex.rs`（`ensure_trusted_hashes` + `CodexSetup`）
- Modify: `app/src-tauri/src/setup/mod.rs`（注册表加 codex）
- Modify: `app/src-tauri/Cargo.toml`（`[dependencies]` 加 `toml_edit = "0.22"`）

**Interfaces:**
- Consumes: Task 2 的 `ensure_codex_hooks`/`claimed_codex_entries`/`reporter_path_from_codex`、Task 3 的 `codex_hook_hash`/`snake_event`、Task 1 的 `sibling_reporter`/`backup_once`、`meowo_reporter::codex::codex_home()`（pub，已存在）、`crate::fsutil::write_atomic`
- Produces: `codex::CodexSetup`（注册进 `ALL_SETUP`）、`codex::ensure_trusted_hashes(doc: &mut toml_edit::DocumentMut, hooks_path_display: &str, entries: &[(String, String, u64)]) -> bool`

- [ ] **Step 1: 写失败测试**

```rust
    #[test]
    fn ensure_trusted_hashes_writes_and_is_idempotent() {
        // 从空 config.toml 起：写入 [hooks.state] 各键；已有等值哈希则不动；不碰无关内容。
        let mut doc: toml_edit::DocumentMut = "default_model = \"x\"\n".parse().unwrap();
        let cmd = "C:/Users/larry/Desktop/workspace/meowo/target/release/meowo-reporter.exe --provider codex";
        let entries = vec![("SessionStart".to_string(), cmd.to_string(), 5u64)];
        let hooks_path = r"C:\Users\larry\.codex\hooks.json";
        assert!(ensure_trusted_hashes(&mut doc, hooks_path, &entries));
        let out = doc.to_string();
        assert!(out.contains("default_model = \"x\"")); // 无关内容原样
        // 键 = <display路径>:<snake事件>:0:0，值 = 本机验证过的真实哈希。
        assert!(out.contains(r"C:\Users\larry\.codex\hooks.json:session_start:0:0"));
        assert!(out.contains("sha256:6a10ea73fc05fb9000b03c3a8d6f54375278aca7cb375d0915be8294fa29c95b"));
        // 幂等：二跑无改动。
        assert!(!ensure_trusted_hashes(&mut doc, hooks_path, &entries));
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p meowo-app setup::codex::tests::ensure_trusted`
Expected: FAIL（函数未定义）

- [ ] **Step 3: 实现 ensure_trusted_hashes + CodexSetup**

Cargo.toml `[dependencies]` 追加：

```toml
# kimi config.toml / codex config.toml 的结构保持编辑（保注释与键序）。
toml_edit = "0.22"
```

codex.rs 追加：

```rust
/// 向 config.toml 的 [hooks.state] 写入/更新各认领条目的 trusted_hash（只动该子树）。
/// 键格式：'<hooks.json 绝对路径 display 串>:<snake_case 事件>:0:0'（每组恒单 handler、
/// 索引恒 0:0——ensure_codex_hooks 只追加单 handler 组，永不重排）。返回是否有改动。
pub fn ensure_trusted_hashes(
    doc: &mut toml_edit::DocumentMut,
    hooks_path_display: &str,
    entries: &[(String, String, u64)],
) -> bool {
    let mut changed = false;
    // 逐层确保 hooks / hooks.state 为隐式 table（不打扰同级既有内容）。
    if doc.get("hooks").is_none() {
        let mut t = toml_edit::Table::new();
        t.set_implicit(true);
        doc.insert("hooks", toml_edit::Item::Table(t));
    }
    let hooks = doc["hooks"].as_table_mut().expect("hooks 应为 table");
    if hooks.get("state").is_none() {
        let mut t = toml_edit::Table::new();
        t.set_implicit(true);
        hooks.insert("state", toml_edit::Item::Table(t));
    }
    let state = hooks["state"].as_table_mut().expect("hooks.state 应为 table");
    for (ev, cmd, timeout) in entries {
        let snake = snake_event(ev);
        if snake.is_empty() {
            continue; // 未知事件不写信任（不该发生：entries 来自我方认领条目）
        }
        let key = format!("{hooks_path_display}:{snake}:0:0");
        let hash = codex_hook_hash(snake, cmd, *timeout);
        let cur = state
            .get(&key)
            .and_then(|it| it.get("trusted_hash"))
            .and_then(|v| v.as_str());
        if cur != Some(hash.as_str()) {
            let mut t = toml_edit::Table::new();
            t.insert("trusted_hash", toml_edit::value(hash));
            state.insert(&key, toml_edit::Item::Table(t));
            changed = true;
        }
    }
    changed
}

/// codex 的 ProviderSetup：先 hooks.json 落盘成功，再写 config.toml 预信任（反序会留下
/// 指向不存在配置的信任残渣；正序失败的最坏情形 = codex 弹一次审查提示，无损）。
pub struct CodexSetup;

impl super::ProviderSetup for CodexSetup {
    fn key(&self) -> meowo_store::ProviderKey {
        meowo_store::ProviderKey::Codex
    }
    fn detect(&self) -> bool {
        meowo_reporter::codex::codex_home().is_some_and(|d| d.is_dir())
    }
    fn apply(&self) {
        let Some(home) = meowo_reporter::codex::codex_home() else {
            return;
        };
        let hooks_path = home.join("hooks.json");
        // 1) hooks.json：不存在从空起；存在但读不了/解析失败 → 放弃（绝不覆盖用户文件）。
        let root_text = match std::fs::read_to_string(&hooks_path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => "{\"hooks\":{}}".to_string(),
            Err(_) => return,
        };
        let Ok(mut root) = serde_json::from_str::<serde_json::Value>(root_text.trim_start_matches('\u{feff}')) else {
            return;
        };
        if !root.is_object() {
            return;
        }
        // reporter 路径：复用已配置且存在的 → 否则 app 同目录 sidecar。
        let reporter = reporter_path_from_codex(&root)
            .filter(|p| std::path::Path::new(p).exists())
            .or_else(super::sibling_reporter);
        let Some(reporter) = reporter else {
            return;
        };
        if ensure_codex_hooks(&mut root, &reporter) {
            let Ok(pretty) = serde_json::to_string_pretty(&root) else {
                return;
            };
            if hooks_path.exists() {
                super::backup_once(&hooks_path);
            }
            if crate::fsutil::write_atomic(&hooks_path, &format!("{pretty}\n")).is_err() {
                return; // hooks.json 没写成，信任步骤跳过（下次启动整段重试）
            }
        }
        // 2) config.toml 预信任：解析失败只跳过信任（hooks 已接上，退化 = codex 弹一次 Trust all）。
        let cfg_path = home.join("config.toml");
        let cfg_text = match std::fs::read_to_string(&cfg_path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(_) => return,
        };
        let Ok(mut doc) = cfg_text.parse::<toml_edit::DocumentMut>() else {
            return;
        };
        let entries = claimed_codex_entries(&root);
        if ensure_trusted_hashes(&mut doc, &hooks_path.display().to_string(), &entries) {
            if cfg_path.exists() {
                super::backup_once(&cfg_path);
            }
            let _ = crate::fsutil::write_atomic(&cfg_path, &doc.to_string());
        }
    }
}
```

`mod.rs` 注册：

```rust
static CODEX_SETUP: codex::CodexSetup = codex::CodexSetup;
static ALL_SETUP: &[&dyn ProviderSetup] = &[&CLAUDE_SETUP, &CODEX_SETUP];
```

- [ ] **Step 4: 跑测试与 clippy**

Run: `cargo test -p meowo-app setup:: && cargo clippy --workspace -- -D warnings`
Expected: 全 PASS

- [ ] **Step 5: dry-run 真机验证（对副本）**

```bash
# 复制真实 ~/.codex 到临时目录，设 CODEX_HOME 指向副本后手动调 apply（写个 ignored 测试跑）
cargo test -p meowo-app setup::codex::tests::dryrun_codex -- --ignored --nocapture
```

ignored 测试（加入 codex.rs tests）：

```rust
    /// dry-run：CODEX_HOME=<真实 ~/.codex 的副本> 时跑 CodexSetup::apply，人工核对副本产物。
    /// 用法：复制 ~/.codex 到临时目录，CODEX_HOME=<副本> cargo test ... -- --ignored --nocapture
    #[test]
    #[ignore]
    fn dryrun_codex() {
        use crate::setup::ProviderSetup;
        CodexSetup.apply();
        let home = meowo_reporter::codex::codex_home().unwrap();
        eprintln!("=== hooks.json ===\n{}", std::fs::read_to_string(home.join("hooks.json")).unwrap());
        eprintln!("=== config.toml [hooks.state] ===\n{}", std::fs::read_to_string(home.join("config.toml")).unwrap());
    }
```

人工核对：既有 3 事件条目一字不动、新增 PostToolUse/PermissionRequest（引号形态 + timeout 5）、config.toml 新增两条 state 键（路径反斜杠、snake_case）、原有三条 state 与无关内容不动。

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "feat(setup): codex 自动接线（hooks.json 合并 + trusted_hash 预信任）"
```

---

### Task 5: kimi config.toml 接线（TDD）

**Files:**
- Create: `app/src-tauri/src/setup/kimi.rs`
- Modify: `app/src-tauri/src/setup/mod.rs`（`pub mod kimi;` + 注册）

**Interfaces:**
- Consumes: `super::claim_provider_cmd`/`sibling_reporter`/`backup_once`、`meowo_reporter::kimi::kimi_share_dir()`（pub，已存在）、`crate::fsutil::write_atomic`
- Produces: `kimi::KIMI_EVENTS: [&str; 5]`、`kimi::ensure_kimi_hooks(doc: &mut toml_edit::DocumentMut, reporter_native: &str) -> bool`、`kimi::KimiSetup`

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kimi_events_all_in_upstream_whitelist() {
        // 防连坐绊线：一条非法 event 会让 kimi 静默禁用全部 hooks（源码 salvageConfigData）。
        for ev in KIMI_EVENTS {
            assert!(KIMI_EVENT_WHITELIST.contains(&ev), "{ev} 不在 kimi 0.20 事件白名单");
        }
    }

    #[test]
    fn ensure_kimi_hooks_adds_all_when_absent_and_preserves_content() {
        let src = "default_model = \"kimi-code/kimi-for-coding\"\n# 用户注释\n[loop_control]\nmax_steps_per_turn = 100\n";
        let mut doc: toml_edit::DocumentMut = src.parse().unwrap();
        assert!(ensure_kimi_hooks(&mut doc, "C:/x/meowo-reporter.exe"));
        let out = doc.to_string();
        assert!(out.contains("# 用户注释")); // 结构保持：注释仍在
        assert!(out.contains("max_steps_per_turn = 100"));
        for ev in KIMI_EVENTS {
            assert!(out.contains(&format!("event = \"{ev}\"")), "{ev} 未写入");
        }
        assert!(out.contains(r#""C:/x/meowo-reporter.exe" --provider kimi"#));
        assert!(!ensure_kimi_hooks(&mut doc, "C:/x/meowo-reporter.exe")); // 幂等
    }

    #[test]
    fn ensure_kimi_hooks_adopts_manual_and_updates_stale_path() {
        // 复刻本机手工接线形态：裸路径命令、5 事件、timeout 5。
        let dev = "C:/Users/larry/Desktop/workspace/meowo/target/release/meowo-reporter.exe";
        let mut src = String::from("theme = \"light\"\n");
        for ev in KIMI_EVENTS {
            src.push_str(&format!("[[hooks]]\nevent = \"{ev}\"\ncommand = \"{dev} --provider kimi\"\ntimeout = 5\n\n"));
        }
        let mut doc: toml_edit::DocumentMut = src.parse().unwrap();
        // 路径仍存在时（解析等价）：无改动。
        assert!(!ensure_kimi_hooks(&mut doc, dev));
        // 路径失效换 sidecar：5 条 command 全部更新，用户键 theme 不动。
        assert!(ensure_kimi_hooks(&mut doc, "C:/app/meowo-reporter.exe"));
        let out = doc.to_string();
        assert_eq!(out.matches(r#""C:/app/meowo-reporter.exe" --provider kimi"#).count(), 5);
        assert!(out.contains("theme = \"light\""));
    }

    #[test]
    fn ensure_kimi_hooks_keeps_user_hook_entries() {
        let src = "[[hooks]]\nevent = \"Notification\"\ncommand = \"my-notify --ding\"\ntimeout = 3\n";
        let mut doc: toml_edit::DocumentMut = src.parse().unwrap();
        assert!(ensure_kimi_hooks(&mut doc, "C:/x/meowo-reporter.exe"));
        let out = doc.to_string();
        assert!(out.contains("my-notify --ding")); // 用户 hook 原样
        assert_eq!(out.matches("--provider kimi").count(), 5); // 我方 5 条已加
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p meowo-app setup::kimi`
Expected: FAIL（模块/函数未定义）

- [ ] **Step 3: 实现**

```rust
//! kimi（kimi-code CLI）自动接线：结构保持地合并 `~/.kimi-code/config.toml` 顶层 [[hooks]]。
//! 纪律（源码调研 kimi-code 0.20）：kimi 自身会全量重写此文件（注释全丢）——幂等判定只按
//! (event, command) 内容匹配，绝不依赖注释标记；一条非法 hook 会让 kimi 静默禁用全部
//! hooks——事件名有白名单绊线测试；文件解析失败即放弃（与 kimi 自身写保护一致）。
use toml_edit::{value, ArrayOfTables, DocumentMut, Item, Table};

/// 接线事件集（与本机手工验证过的组合一致）。PermissionRequest 待真实 payload 验证后再议。
pub const KIMI_EVENTS: [&str; 5] =
    ["SessionStart", "UserPromptSubmit", "PostToolUse", "Stop", "SessionEnd"];

/// kimi-code 0.20 支持的全部 hook 事件（HOOK_EVENT_TYPES，白名单绊线用）。
pub const KIMI_EVENT_WHITELIST: [&str; 16] = [
    "PreToolUse", "PostToolUse", "PostToolUseFailure", "PermissionRequest", "PermissionResult",
    "UserPromptSubmit", "Stop", "StopFailure", "Interrupt", "SessionStart", "SessionEnd",
    "SubagentStart", "SubagentStop", "PreCompact", "PostCompact", "Notification",
];

/// 幂等合并 [[hooks]]：逐 KIMI_EVENTS 找认领条目（event 相符 + command 可认领），
/// 路径不符则更新 command；缺则追加 {event, command, timeout=5}。用户条目一概不动。
pub fn ensure_kimi_hooks(doc: &mut DocumentMut, reporter_native: &str) -> bool {
    let desired_cmd = format!("\"{reporter_native}\" --provider kimi");
    let mut changed = false;
    if doc.get("hooks").and_then(|it| it.as_array_of_tables()).is_none() {
        // 不存在或非 array-of-tables（如 hooks = [] 的旧写法残留在新文件：实测 kimi-code
        // 默认无顶层 hooks 键；inline array 形态无法结构保持地转换，直接放弃不写坏）。
        if doc.get("hooks").is_some() {
            return false;
        }
        doc.insert("hooks", Item::ArrayOfTables(ArrayOfTables::new()));
        changed = true;
    }
    let arr = doc["hooks"].as_array_of_tables_mut().expect("hooks 应为 array-of-tables");
    for ev in KIMI_EVENTS {
        let mut found = false;
        for t in arr.iter_mut() {
            if t.get("event").and_then(|v| v.as_str()) != Some(ev) {
                continue;
            }
            let Some(path) = t
                .get("command")
                .and_then(|v| v.as_str())
                .and_then(|c| super::claim_provider_cmd(c, "kimi"))
            else {
                continue; // 该事件上的用户自有 hook，不动
            };
            found = true;
            if path != reporter_native {
                t.insert("command", value(desired_cmd.clone()));
                changed = true;
            }
        }
        if !found {
            let mut t = Table::new();
            t.insert("event", value(ev));
            t.insert("command", value(desired_cmd.clone()));
            t.insert("timeout", value(5));
            arr.push(t);
            changed = true;
        }
    }
    changed
}

/// kimi 的 ProviderSetup。config.toml 由 kimi login 生成，缺失 → 视为未完成安装，跳过不创建。
pub struct KimiSetup;

impl super::ProviderSetup for KimiSetup {
    fn key(&self) -> meowo_store::ProviderKey {
        meowo_store::ProviderKey::Kimi
    }
    fn detect(&self) -> bool {
        meowo_reporter::kimi::kimi_share_dir().is_some_and(|d| d.is_dir())
    }
    fn apply(&self) {
        let Some(dir) = meowo_reporter::kimi::kimi_share_dir() else {
            return;
        };
        let cfg = dir.join("config.toml");
        let Ok(text) = std::fs::read_to_string(&cfg) else {
            return;
        };
        let Ok(mut doc) = text.parse::<DocumentMut>() else {
            return; // 解析失败绝不写（kimi 自身对坏文件同样拒写）
        };
        // reporter 路径：复用 [[hooks]] 里已认领且存在的 → 否则 sidecar。
        let existing = doc
            .get("hooks")
            .and_then(|it| it.as_array_of_tables())
            .into_iter()
            .flat_map(|a| a.iter())
            .find_map(|t| {
                t.get("command")
                    .and_then(|v| v.as_str())
                    .and_then(|c| super::claim_provider_cmd(c, "kimi"))
            })
            .filter(|p| std::path::Path::new(p).exists());
        let Some(reporter) = existing.or_else(super::sibling_reporter) else {
            return;
        };
        if ensure_kimi_hooks(&mut doc, &reporter) {
            super::backup_once(&cfg);
            let _ = crate::fsutil::write_atomic(&cfg, &doc.to_string());
        }
    }
}
```

`mod.rs`：`pub mod kimi;` + 注册 `static KIMI_SETUP: kimi::KimiSetup = kimi::KimiSetup;`、`ALL_SETUP = &[&CLAUDE_SETUP, &CODEX_SETUP, &KIMI_SETUP]`。

- [ ] **Step 4: 跑测试与 clippy**

Run: `cargo test -p meowo-app setup:: && cargo clippy --workspace -- -D warnings`
Expected: 全 PASS

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(setup): kimi config.toml [[hooks]] 结构保持自动接线（防连坐白名单绊线）"
```

---

### Task 6: README 手动接线文档 + 真机验收

**Files:**
- Modify: `README.md`（「接入 Claude Code」章节扩为多 provider；手动接线 details 补 codex/kimi 片段）

**Interfaces:**
- Consumes: 前五个 task 的成品行为

- [ ] **Step 1: README 更新**

「零配置接入」特性段与「接入 Claude Code」章节改为三 provider 口径（自动接线覆盖 claude hooks+statusLine、codex hooks+预信任、kimi hooks）。手动接线 details 内补充：

````markdown
<details>
<summary>codex / kimi 手动接线（可选：不启动 app 时）</summary>

**codex**：编辑 `~/.codex/hooks.json`（没有则创建），对 SessionStart / UserPromptSubmit / PostToolUse / Stop / PermissionRequest 各加一条：

```json
{ "hooks": { "SessionStart": [ { "hooks": [ { "type": "command", "command": "\"<meowo-reporter绝对路径>\" --provider codex", "timeout": 5 } ] } ] } }
```

首次启动 codex 会提示审查新 hooks，选 **Trust all and continue** 即可（meowo-app 自动接线则无此提示——会一并写入信任记录）。

**kimi**：编辑 `~/.kimi-code/config.toml`，对 SessionStart / UserPromptSubmit / PostToolUse / Stop / SessionEnd 各加一段：

```toml
[[hooks]]
event = "SessionStart"
command = '"<meowo-reporter绝对路径>" --provider kimi'
timeout = 5
```

</details>
````

- [ ] **Step 2: 前端/Rust 全量回归**

Run: `cargo test --workspace && cargo clippy --workspace -- -D warnings && cd app && bunx tsc --noEmit && bunx vitest run`
Expected: 全绿（本特性不动前端，纯确认无意外牵连）

- [ ] **Step 3: 真机验收（本机三家全有，一次覆盖）**

先跑 `bun ../scripts/prepare-sidecar.mjs`（sidecar 编译前置），再 `cd app && bun run tauri dev`，核对：

1. `~/.claude/settings.json` **零 diff**（`git diff` 无从查，用启动前 copy 对比）；
2. `~/.codex/hooks.json` 新增 PostToolUse/PermissionRequest 两条（既有 3 条一字不动）；`~/.codex/config.toml` 的 `[hooks.state]` 新增两条对应 trusted_hash；
3. **启动 codex 新会话：无 hooks 审查弹窗**（预信任生效的判定性证据），会话上板、跑一个工具后卡片显示工具活动（PostToolUse payload 兼容性验证）；触发一次权限确认，卡片进入待交互（PermissionRequest 验证）——若 payload 不兼容（卡片无反应/reporter 报错），把该事件从 `CODEX_EVENTS` 移除并同步 README（设计已预留此收缩路径）；
4. `~/.kimi-code/config.toml` 无 diff（手工配置已是目标态）；起 kimi 新会话正常上板；
5. 二次重启 app：三家配置文件均零改动（幂等终态）。

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "docs(readme): 多 provider 自动接线口径 + codex/kimi 手动接线片段"
```

---

## Self-Review 记录

- **Spec 覆盖**：模块组织（Task 1）、codex hooks+预信任+顺序纪律（Task 2-4）、kimi 结构保持+防连坐（Task 5）、README 补偿（Task 6）、claude 零变更（Task 1 + Task 6 验收 1）、幂等/备份/原子写（各 task）、纯函数约束（Global Constraints）——spec「决议记录」四条全部落实。
- **类型一致性**：`claim_provider_cmd` 签名在 Task 1 定义、Task 2/5 消费一致；`claimed_codex_entries` 返回 `Vec<(String, String, u64)>` 与 `ensure_trusted_hashes` 参数一致。
- **已知不确定点已显式处理**：codex PostToolUse/PermissionRequest payload 兼容性（Task 6 验收 3 的收缩路径）、生成命令引号形态在 codex/kimi 下的执行（Task 6 验收 3/4 覆盖：新增条目为引号形态，真实会话触发即验证）、trusted_hash 算法漂移（Task 3 向量测试绊线 + TUI Trust all 兜底）。

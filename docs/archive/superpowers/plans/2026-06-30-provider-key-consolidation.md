# ProviderKey 强类型收敛 实施计划（Phase 0）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把全项目散落的「provider 裸字符串 + claude 隐式默认」收敛成一个定义在 `meowo-store` 的 `ProviderKey` 强类型 + 单一 `DEFAULT_PROVIDER` 常量 + 单一事实源注册表派发，并加守护测试防回归——零行为变更。

**Architecture:** `ProviderKey` 仿 `meowo-store/models.rs` 既有的 `SessionStatus`/`TodoStatus` 惯例（`as_str` + 无副作用 `from_str`，未知/空降级默认）。它必须定义在最底层 crate `meowo-store`，被 `meowo-reporter`（`Agent::key`/`for_provider`/`dispatch`）和 `meowo-app`（Tauri 调用点）共享。`for_provider` 改为遍历 `ALL` 注册表（删除与之重复的 `match`），并用一个 enum↔registry 配对测试保证两者不漂移。前端加一个 vitest 守护测试锁定 provider 注册表与 i18n 文案齐全。

**Tech Stack:** Rust（workspace：`meowo-store`/`meowo-reporter`/`meowo-app`，rusqlite，cargo test）+ React/TypeScript（vitest，bun）。

## Global Constraints

- **零行为变更**：claude 路径的运行结果（写库、resume 命令、focus/rename 行为）与改造前逐字节一致；非默认 provider（kimi/codex）的写库与派发行为也不变。本计划只换类型与归一点，不改任何业务逻辑。
- **`ProviderKey` 必须定义在 `crates/meowo-store`**（最底层 crate，零依赖 meowo-reporter）。**严禁**放 `meowo-reporter` —— 依赖方向是 `meowo-reporter → meowo-store`、`meowo-app → meowo-store`，反向会形成循环依赖、Cargo 拒编。
- **遵循 `meowo-store/src/models.rs` 既有 enum 惯例**：`#[derive(... Serialize, Deserialize)]` + `#[serde(rename_all = "lowercase")]` + `as_str(self) -> &'static str` + 无副作用 `from_str(&str) -> Self`（未知值降级默认，带 `#[allow(clippy::should_implement_trait)]`）。
- **代码用英文，注释/commit message 用中文。**
- **DRY / YAGNI**：本计划**不**引入 `session_provider` getter、`Caps` 结构、`HookInstaller`/`ProviderAccount` trait、前端能力位字段、跨语言 codegen —— 这些经对抗评审属价值有限或 YAGNI，留作后续独立计划（见文末「本计划之外」）。
- **测试 / 编译命令（每个任务末尾据此验证）**：
  - Rust 库与上报器：`cargo test -p meowo-store -p meowo-reporter` 与 `cargo clippy -p meowo-store -p meowo-reporter --all-targets -- -D warnings`
  - Tauri crate 仅做编译检查（含 build.rs 的 sidecar 前置，见 [[sidecar-build-prereq]]）：`node scripts/prepare-sidecar.mjs && cargo check -p meowo-app`
  - 前端：`cd app && bun run test`
- **分支**：在 `refactor/provider-key-consolidation-20260630` 上执行（git-workflow 规则：type=refactor）。

---

## File Structure

- `crates/meowo-store/src/models.rs` —— 新增 `ProviderKey` enum + `DEFAULT_PROVIDER` 常量 + 守护单测（与 `SessionStatus` 等同处，经 `lib.rs` 的 `pub use models::*` 自动导出为 `meowo_store::ProviderKey`）。
- `crates/meowo-store/src/store.rs` —— `set_session_provider` 入参由 `&str` 改 `ProviderKey`。
- `crates/meowo-reporter/src/agent.rs` —— `Agent::key()` 返回 `ProviderKey`；`for_provider` 入参改 `ProviderKey` 并遍历 `ALL`（删 `match`）；更新单测 + 新增 enum↔registry 配对测试。
- `crates/meowo-reporter/src/dispatch.rs` —— `dispatch`/`create_session`/`lookup_or_create` 入参 `&str` 改 `ProviderKey`；`!= "claude"` 改 `!provider.is_default()`。
- `crates/meowo-reporter/src/main.rs` —— `parse_provider` 返回 `ProviderKey`。
- `crates/meowo-reporter/tests/dispatch_test.rs` —— 测试里的 provider 字面量改 `ProviderKey` 变体。
- `app/src-tauri/src/lib.rs` —— 4 处 `for_provider(...)` 调用点改用 `meowo_store::ProviderKey::parse(...)`，消除 `unwrap_or("claude")`。
- `app/src/providers.tsx` —— 导出 `PROVIDERS`（供测试断言）。
- `app/src/providers.test.tsx` —— 新增 vitest 守护测试（注册表 key 集合 + i18n 文案齐全）。

---

### Task 1: 在 meowo-store 定义 ProviderKey + DEFAULT_PROVIDER（纯增量）

**Files:**
- Modify: `crates/meowo-store/src/models.rs`（在文件末尾、`TodoInput` 之后追加）
- Test: `crates/meowo-store/src/models.rs`（同文件 `#[cfg(test)]` 模块）

**Interfaces:**
- Produces:
  - `pub enum meowo_store::ProviderKey { Claude, Kimi, Codex }`（`Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize`）
  - `pub const meowo_store::DEFAULT_PROVIDER: &str = "claude"`
  - `ProviderKey::ALL: &'static [ProviderKey]`
  - `ProviderKey::as_str(self) -> &'static str`
  - `ProviderKey::from_str(s: &str) -> ProviderKey`（无副作用，未知→Claude）
  - `ProviderKey::parse(s: Option<&str>) -> ProviderKey`（None/未知→Claude，唯一归一点）
  - `ProviderKey::is_default(self) -> bool`

- [ ] **Step 1: 写失败测试**

在 `crates/meowo-store/src/models.rs` 末尾追加测试模块：

```rust
#[cfg(test)]
mod provider_key_tests {
    use super::*;

    #[test]
    fn as_str_roundtrips_known_keys() {
        assert_eq!(ProviderKey::Claude.as_str(), "claude");
        assert_eq!(ProviderKey::Kimi.as_str(), "kimi");
        assert_eq!(ProviderKey::Codex.as_str(), "codex");
    }

    #[test]
    fn from_str_falls_back_to_claude_on_unknown() {
        assert_eq!(ProviderKey::from_str("kimi"), ProviderKey::Kimi);
        assert_eq!(ProviderKey::from_str("codex"), ProviderKey::Codex);
        assert_eq!(ProviderKey::from_str("claude"), ProviderKey::Claude);
        assert_eq!(ProviderKey::from_str("nonsense"), ProviderKey::Claude);
        assert_eq!(ProviderKey::from_str(""), ProviderKey::Claude);
    }

    #[test]
    fn parse_normalizes_none_and_unknown_to_default() {
        // 唯一归一点：替代散落的 unwrap_or("claude")。
        assert_eq!(ProviderKey::parse(None), ProviderKey::Claude);
        assert_eq!(ProviderKey::parse(Some("kimi")), ProviderKey::Kimi);
        assert_eq!(ProviderKey::parse(Some("zzz")), ProviderKey::Claude);
    }

    #[test]
    fn is_default_only_for_claude() {
        assert!(ProviderKey::Claude.is_default());
        assert!(!ProviderKey::Kimi.is_default());
        assert!(!ProviderKey::Codex.is_default());
    }

    #[test]
    fn default_const_matches_claude_variant_and_schema() {
        // DEFAULT_PROVIDER 必须与 ProviderKey::Claude 及 sessions.provider 列的
        // SQL DEFAULT 'claude'（migrations.rs / store.rs ALTER）保持一致。
        // 改默认 provider 时此断言守护三处不漂移。
        assert_eq!(DEFAULT_PROVIDER, "claude");
        assert_eq!(ProviderKey::Claude.as_str(), DEFAULT_PROVIDER);
    }

    #[test]
    fn all_lists_every_variant_once() {
        assert_eq!(ProviderKey::ALL.len(), 3);
        for v in [ProviderKey::Claude, ProviderKey::Kimi, ProviderKey::Codex] {
            assert!(ProviderKey::ALL.contains(&v));
        }
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p meowo-store provider_key_tests`
Expected: 编译失败（`cannot find type ProviderKey` / `cannot find value DEFAULT_PROVIDER`）。

- [ ] **Step 3: 写最小实现**

在 `crates/meowo-store/src/models.rs` 末尾（测试模块**之前**）追加：

```rust
/// 默认 agent provider 名。与 sessions.provider 列的 SQL DEFAULT 'claude'
/// （migrations.rs 建表 + store.rs ALTER）必须一致——由 models 测试
/// default_const_matches_claude_variant_and_schema 守护。
pub const DEFAULT_PROVIDER: &str = "claude";

/// agent 提供方（CLI）。与 sessions.provider 列、前端 ProviderConfig key 对齐。
/// 仿 SessionStatus/TodoStatus：as_str + 无副作用 from_str（未知/空降级默认），
/// 作为全项目「provider 名」的单一强类型，取代散落的裸 &str 比较与 unwrap_or("claude")。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKey {
    Claude,
    Kimi,
    Codex,
}

impl ProviderKey {
    /// 全部已知 provider。新增 variant 必在此登记；meowo-reporter 的 enum↔registry
    /// 配对测试据此校验每个 key 都有对应 Agent 实现。
    pub const ALL: &'static [ProviderKey] = &[ProviderKey::Claude, ProviderKey::Kimi, ProviderKey::Codex];

    pub fn as_str(self) -> &'static str {
        match self {
            ProviderKey::Claude => "claude",
            ProviderKey::Kimi => "kimi",
            ProviderKey::Codex => "codex",
        }
    }

    /// 无副作用解析：未知 → 默认（Claude）。仿 TodoStatus::from_str。
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> ProviderKey {
        match s {
            "kimi" => ProviderKey::Kimi,
            "codex" => ProviderKey::Codex,
            _ => ProviderKey::Claude,
        }
    }

    /// 唯一归一点：替代散落的 unwrap_or("claude") / != "claude"。None/未知 → 默认。
    pub fn parse(s: Option<&str>) -> ProviderKey {
        match s {
            Some(v) => ProviderKey::from_str(v),
            None => ProviderKey::Claude,
        }
    }

    /// 是否为默认 provider（claude）。DB 把 NULL/缺省视作 claude，故默认 provider 不写库。
    pub fn is_default(self) -> bool {
        matches!(self, ProviderKey::Claude)
    }
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p meowo-store provider_key_tests`
Expected: PASS（6 个测试全绿）。

- [ ] **Step 5: clippy 全绿**

Run: `cargo clippy -p meowo-store --all-targets -- -D warnings`
Expected: 无警告（`should_implement_trait` 已被 `#[allow]` 抑制）。

- [ ] **Step 6: 提交**

```bash
git add crates/meowo-store/src/models.rs
git commit -m "feat(provider): meowo-store 新增 ProviderKey 强类型与 DEFAULT_PROVIDER 常量"
```

---

### Task 2: 把 for_provider / key / dispatch / set_session_provider 切到 ProviderKey（原子类型迁移）

> 这是一次跨 crate 的原子签名迁移：`for_provider` 的入参类型一改，所有调用方必须同步，否则 workspace 编不过。因此本任务一次性覆盖 meowo-store / meowo-reporter / meowo-app 的全部调用点，提交一次保持全绿。各步骤本身很小。

**Files:**
- Modify: `crates/meowo-store/src/store.rs:625-631`
- Modify: `crates/meowo-reporter/src/agent.rs`（trait + 3 impl + for_provider + tests）
- Modify: `crates/meowo-reporter/src/dispatch.rs:1-8, 156-189`
- Modify: `crates/meowo-reporter/src/main.rs:30-50`
- Modify: `crates/meowo-reporter/tests/dispatch_test.rs:75-83` 及 kimi 调用点
- Modify: `app/src-tauri/src/lib.rs:709, 915, 1013-1014, 1349`

**Interfaces:**
- Consumes: `meowo_store::ProviderKey`（Task 1）
- Produces:
  - `meowo_store::Store::set_session_provider(&self, session_id: i64, provider: ProviderKey) -> Result<(), StoreError>`
  - `meowo_reporter::agent::Agent::key(&self) -> ProviderKey`
  - `meowo_reporter::agent::for_provider(key: ProviderKey) -> &'static dyn Agent`
  - `meowo_reporter::dispatch::dispatch(store, ev, now_ms, provider: ProviderKey) -> Result<(), StoreError>`

- [ ] **Step 1: meowo-store —— set_session_provider 改收 ProviderKey**

替换 `crates/meowo-store/src/store.rs:623-631`：

```rust
    /// 设置会话所属 agent provider（claude/kimi…）。仅在 SessionStart 由 reporter 写一次；
    /// 不动 last_event_at（同回合的 set_session_cwd 等已刷新）。入参为强类型，写入端归一。
    pub fn set_session_provider(&self, session_id: i64, provider: crate::ProviderKey) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE sessions SET provider = ?1 WHERE id = ?2",
            rusqlite::params![provider.as_str(), session_id],
        )?;
        Ok(())
    }
```

- [ ] **Step 2: meowo-reporter agent.rs —— 引入 ProviderKey、key() 返回 ProviderKey、for_provider 遍历 ALL**

在 `crates/meowo-reporter/src/agent.rs` 顶部 `use crate::hook::HookEvent;` 下加：

```rust
use meowo_store::ProviderKey;
```

把 trait 里的 `key` 方法签名（`agent.rs:15-16`）改为：

```rust
    /// provider key（与 DB sessions.provider / 前端一致）。
    fn key(&self) -> ProviderKey;
```

三个 impl 的 `key` 方法分别改为（`ClaudeAgent`/`KimiAgent`/`CodexAgent`）：

```rust
    fn key(&self) -> ProviderKey {
        ProviderKey::Claude
    }
```
```rust
    fn key(&self) -> ProviderKey {
        ProviderKey::Kimi
    }
```
```rust
    fn key(&self) -> ProviderKey {
        ProviderKey::Codex
    }
```

把 `for_provider`（`agent.rs:180-187`）整体替换为遍历注册表、删除重复 match：

```rust
/// 按 provider key 取 agent；遍历 ALL 注册表（单一事实源）。未知不会发生（入参已是强类型），
/// find 失败时回退 claude 兜底。
pub fn for_provider(key: ProviderKey) -> &'static dyn Agent {
    ALL.iter().copied().find(|a| a.key() == key).unwrap_or(&CLAUDE)
}
```

- [ ] **Step 3: meowo-reporter agent.rs —— 更新单测 + 新增 enum↔registry 配对测试**

把 `for_provider_falls_back_to_claude` 测试（`agent.rs:205-212`）替换为：

```rust
    #[test]
    fn for_provider_returns_matching_agent() {
        assert_eq!(for_provider(ProviderKey::Kimi).key(), ProviderKey::Kimi);
        assert_eq!(for_provider(ProviderKey::Codex).key(), ProviderKey::Codex);
        assert_eq!(for_provider(ProviderKey::Claude).key(), ProviderKey::Claude);
    }

    #[test]
    fn every_provider_key_has_agent_and_vice_versa() {
        // enum↔registry 单一事实源守护：ProviderKey 每个 variant 必有一个 ALL 中的 Agent，
        // 反之亦然；二者数量相等。加新 CLI 漏注册任一侧即在此处失败。
        for &k in ProviderKey::ALL {
            assert!(ALL.iter().any(|a| a.key() == k), "ProviderKey {k:?} 无对应 Agent");
        }
        for a in ALL {
            assert!(ProviderKey::ALL.contains(&a.key()), "Agent {:?} 不在 ProviderKey::ALL", a.key());
        }
        assert_eq!(ALL.len(), ProviderKey::ALL.len());
    }
```

把 `resume_args_per_provider` 测试里的 `for_provider("claude")` / `for_provider("codex")` / `for_provider("kimi")`（`agent.rs:233/235/239`）分别改成 `for_provider(ProviderKey::Claude)` / `for_provider(ProviderKey::Codex)` / `for_provider(ProviderKey::Kimi)`。

- [ ] **Step 4: meowo-reporter dispatch.rs —— 签名与归一点**

`dispatch.rs:1` 的 import 改为引入 ProviderKey：

```rust
use meowo_store::{PendingReview, ProviderKey, SessionStatus, Store, StoreError};
```

`dispatch.rs:8` 函数签名改为：

```rust
pub fn dispatch(store: &Store, ev: &HookEvent, now_ms: i64, provider: ProviderKey) -> Result<(), StoreError> {
```

`create_session`（`dispatch.rs:152`）签名的 `provider: &str` 改为 `provider: ProviderKey`；其内部 `dispatch.rs:156-158` 改为：

```rust
    if !provider.is_default() {
        store.set_session_provider(sid, provider)?;
    }
```

`lookup_or_create`（`dispatch.rs:175`）签名的 `provider: &str` 改为 `provider: ProviderKey`（它把 provider 透传给 create_session，无其它改动；ProviderKey 是 Copy，按值传递）。

> 说明：`apply_title`（`dispatch.rs:101`，`provider: &str`）与 `write_tab_token`（`dispatch.rs:126`，`provider: &str`）内部调用 `for_provider(provider)`。它们的 `provider` 参数同样改为 `ProviderKey`，调用处（`dispatch.rs:17/31/64` 等的 `apply_title(..., provider)`、`write_tab_token(..., provider)`）因 provider 已是 ProviderKey 且 Copy，无需改写实参。

把 `apply_title` 与 `write_tab_token` 的签名里 `provider: &str` 改为 `provider: ProviderKey`：

```rust
fn apply_title(store: &Store, ev: &HookEvent, sid: i64, now_ms: i64, provider: ProviderKey) -> Result<(), StoreError> {
```
```rust
fn write_tab_token(store: &Store, sid: i64, ev: &HookEvent, provider: ProviderKey) {
```

- [ ] **Step 5: meowo-reporter main.rs —— parse_provider 返回 ProviderKey**

`main.rs:1-4` 的 import 已有 `use meowo_store::Store;`，在其下加：

```rust
use meowo_store::ProviderKey;
```

`main.rs:36-50` 的 `parse_provider` 替换为：

```rust
/// 从命令行解析 `--provider <name>` / `--provider=<name>`，缺省 claude。
fn parse_provider() -> ProviderKey {
    let args: Vec<String> = std::env::args().collect();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--provider" {
            if let Some(v) = it.next() {
                return ProviderKey::from_str(v);
            }
        } else if let Some(v) = a.strip_prefix("--provider=") {
            return ProviderKey::from_str(v);
        }
    }
    ProviderKey::Claude
}
```

`main.rs:31-32` 不变（`let provider = parse_provider();` 后 `dispatch(&store, &ev, now, provider)?;`）—— provider 现为 `ProviderKey`（Copy），按值传入 dispatch，正确。

- [ ] **Step 6: meowo-reporter dispatch_test.rs —— 测试 provider 字面量改枚举**

`dispatch_test.rs:76` 改为：

```rust
use meowo_store::{ProviderKey, Store};
```

`dispatch_test.rs:81-83` 的 helper 改为：

```rust
/// 测试默认走 claude provider；provider 行为单独在 kimi_session_tagged_with_provider 覆盖。
fn disp(store: &Store, ev: &HookEvent, now_ms: i64) -> Result<(), meowo_store::StoreError> {
    dispatch(store, ev, now_ms, ProviderKey::Claude)
}
```

把 dispatch_test.rs 中其余四处直接调用 `dispatch(..., "kimi")`（约在第 319、332、335、345 行的 `dispatch(...)` 调用，末位实参为字符串 `"kimi"`）的 `"kimi"` 实参改为 `ProviderKey::Kimi`。

- [ ] **Step 7: meowo-app lib.rs —— 4 处调用点改归一点，消除 unwrap_or("claude")**

`app/src-tauri/src/lib.rs:709`（focus 路径，`provider` 为 `Option<String>`）替换为：

```rust
        let title_based = meowo_reporter::agent::for_provider(meowo_store::ProviderKey::parse(provider.as_deref()))
```

`app/src-tauri/src/lib.rs:915`（resume_session，`provider` 为 `String`）替换为：

```rust
            let resume = meowo_reporter::agent::for_provider(meowo_store::ProviderKey::parse(Some(&provider))).resume_args(&session_id);
```

`app/src-tauri/src/lib.rs:1013-1014`（rename_session，`provider` 为 `Option<String>`）替换为：

```rust
    // 落到 agent 自己的持久层（best-effort）。provider 缺省 claude（兼容旧调用方）。
    let provider = meowo_store::ProviderKey::parse(provider.as_deref());
    let _ = meowo_reporter::agent::for_provider(provider).write_rename(&session_id, cwd.as_deref(), &title);
```

`app/src-tauri/src/lib.rs:1349`（通知点击聚焦，`s.provider` 为 `String`）替换为：

```rust
                        meowo_reporter::agent::for_provider(meowo_store::ProviderKey::parse(Some(&s.provider))).sets_terminal_tab_title();
```

- [ ] **Step 8: 运行 Rust 库与上报器测试，确认全绿**

Run: `cargo test -p meowo-store -p meowo-reporter`
Expected: PASS（含新增的 `every_provider_key_has_agent_and_vice_versa`、改写后的 dispatch_test）。

- [ ] **Step 9: clippy 全绿**

Run: `cargo clippy -p meowo-store -p meowo-reporter --all-targets -- -D warnings`
Expected: 无警告。

- [ ] **Step 10: meowo-app 编译检查（含 sidecar 前置）**

Run: `node scripts/prepare-sidecar.mjs && cargo check -p meowo-app`
Expected: 编译通过（4 处调用点类型对齐；`unwrap_or("claude")` 已消除）。
> 见 [[sidecar-build-prereq]]：编译 meowo-app 前必须先跑 prepare-sidecar.mjs，否则 tauri_build 编译期报错。

- [ ] **Step 11: 提交**

```bash
git add crates/meowo-store/src/store.rs crates/meowo-reporter/src/agent.rs crates/meowo-reporter/src/dispatch.rs crates/meowo-reporter/src/main.rs crates/meowo-reporter/tests/dispatch_test.rs app/src-tauri/src/lib.rs
git commit -m "refactor(provider): for_provider/key/dispatch 切到 ProviderKey 强类型，归一 claude 默认"
```

---

### Task 3: 前端 provider 注册表守护测试（vitest）

> 前端不依赖 Rust 类型，故本任务独立于 Task 2。它锁定「provider 注册表 key 集合」与「每个 provider 的 i18n 文案在 zh/en 均可解析」，防止加新 CLI 时漏配注册表或 i18n。跨语言（Rust `ProviderKey::ALL` ↔ TS 注册表）的强一致此处用一份「期望 key 集合」字面量 + 注释镜像锁定（不引入 codegen，属本计划之外）。

**Files:**
- Modify: `app/src/providers.tsx:50`（导出 PROVIDERS）
- Create: `app/src/providers.test.tsx`

**Interfaces:**
- Consumes: `app/src/providers.tsx` 的 `PROVIDERS`、`providerConfig`；`app/src/i18n/zh.ts` 的 `zh`；`app/src/i18n/en.ts` 的 `en`。
- Produces: 无（仅测试 + 一个导出）。

- [ ] **Step 1: 写失败测试**

新建 `app/src/providers.test.tsx`：

```tsx
import { describe, it, expect } from "vitest";
import { PROVIDERS, providerConfig } from "./providers";
import { zh } from "./i18n/zh";
import { en } from "./i18n/en";

// 期望的 provider key 集合，必须与 Rust 侧 meowo_store::ProviderKey::ALL 保持一致
// （加新 CLI：此处、providers.tsx 的 PROVIDERS、Rust ProviderKey 三处同步）。
const EXPECTED_KEYS = ["claude", "codex", "kimi"];

describe("provider 注册表守护", () => {
  it("PROVIDERS 的 key 集合恰好等于期望集合", () => {
    expect(Object.keys(PROVIDERS).sort()).toEqual(EXPECTED_KEYS);
  });

  it("每个 provider 在 zh/en 都有非空展示名", () => {
    for (const key of EXPECTED_KEYS) {
      const cfg = PROVIDERS[key];
      expect(cfg, `缺少 provider 注册项: ${key}`).toBeTruthy();
      expect(cfg.label(zh).length, `zh 文案为空: ${key}`).toBeGreaterThan(0);
      expect(cfg.label(en).length, `en 文案为空: ${key}`).toBeGreaterThan(0);
    }
  });

  it("未知 provider 回退到 claude 配置", () => {
    expect(providerConfig("__nope__")).toBe(PROVIDERS.claude);
  });
});
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd app && bun run test providers`
Expected: FAIL（`PROVIDERS` 未导出 —— `does not provide an export named 'PROVIDERS'`）。

- [ ] **Step 3: 导出 PROVIDERS**

`app/src/providers.tsx:50` 把 `const PROVIDERS` 改为导出：

```tsx
export const PROVIDERS: Record<string, ProviderConfig> = {
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cd app && bun run test providers`
Expected: PASS（3 个测试全绿）。

- [ ] **Step 5: 前端类型检查不回归**

Run: `cd app && bun run build`
Expected: `tsc` 通过、vite build 成功（导出 PROVIDERS 不破坏类型）。

- [ ] **Step 6: 提交**

```bash
git add app/src/providers.tsx app/src/providers.test.tsx
git commit -m "test(provider): 前端 provider 注册表与 i18n 文案守护测试"
```

---

## Self-Review

**1. Spec coverage（对照审查报告的 Phase 0 目标）：**
- ProviderKey 强类型 + 单一 DEFAULT_PROVIDER → Task 1 ✅
- `for_provider` 遍历 ALL、删重复 match → Task 2 Step 2 ✅
- 消除散落的 `unwrap_or("claude")`/`!= "claude"`（main/dispatch/lib.rs）→ Task 2 Step 4/5/7 ✅
- `set_session_provider` 写入端强类型归一 → Task 2 Step 1 ✅
- 两处 `DEFAULT 'claude'` SQL 字面量的单源守护 → Task 1 的 `default_const_matches_claude_variant_and_schema`（保留 SQL 字面量 + 断言守护，不强行 const 插值 —— 改默认值会被测试拦截）✅
- enum↔registry 不漂移守护 → Task 2 Step 3 配对测试 ✅
- 跨语言 key 齐全守护 → Task 3 ✅
- **零行为变更**：claude 默认仍跳过写库（`is_default()` 等价 `!= "claude"`）；resume/focus/rename 派发逻辑不变，仅入参归一 ✅

**2. Placeholder scan：** 无 TBD/TODO/「类似上文」；每个改动步骤都给了完整可粘贴代码与确切命令、预期输出。✅

**3. Type consistency：** `ProviderKey`/`DEFAULT_PROVIDER`/`ALL`/`as_str`/`from_str`/`parse`/`is_default` 在 Task 1 定义，Task 2 全部按此签名消费；`set_session_provider(_, ProviderKey)`、`key()->ProviderKey`、`for_provider(ProviderKey)`、`dispatch(..., ProviderKey)` 前后一致；前端 `PROVIDERS`/`providerConfig` 名称与 providers.tsx 现状一致。✅

---

## 本计划之外（后续独立计划，经对抗评审取舍）

- **Phase 1（Caps 合并 + model_via_statusline）**：把三个 bool 方法并进 `Caps` 结构。评审认为多数新标志（`has_statusline` 在 main.rs 拦截点拿不到 provider、`supports_resume` 三家全真、`supports_account` 三处重复）属死标志，仅 `model_via_statusline` 有判别价值 —— 价值有限，单独评估。
- **Phase 2（macOS resume 参数化）**：`term_script.rs`/`macos/terminal.rs` 接 `resume_args`，注意 AppleScript+shell 双层转义。
- **Phase 3（前端类型收紧 + 能力位）**：`LiveSession.provider` 收紧为联合类型、品牌色入注册表 —— 仅加真正能判别的字段（如 codex `canResume:false`）。
- **Phase 5（TranscriptSpec）**：用**增量解析单元**（非 one-shot），`context_window` 不进 spec（走 statusline per-session），`Analysis` 不带 model。
- **暂缓（YAGNI）**：`HookInstaller`、`ProviderAccount` —— 仅单一实现，待第二个实例出现再按真实形状抽象。
- **顺手清理**：`scripts/install-hooks.mjs` 与 `ccsetup.rs` 共享 SPECS、README/docs 中性化、底栏 `UsageScreen` 多 provider 语义决策。

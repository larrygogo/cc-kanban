# 多 provider 账号 + 用量（ProviderAccount 抽象）实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** 把当前 claude 独占的账号/用量（`account.rs` + 前端 AccountSection/UsageScreen）抽成 `ProviderAccount` 抽象，claude/codex/kimi 三套真实实现：claude 重构（零回归）、codex 纯本地（账号 id_token + 用量 session JSONL，已实地确认）、kimi best-effort（用量 `GET /usages` 容错解析 + 账号降级「已登录」）。前端按 provider 显示账号卡 + 用量。

**Architecture:** 镜像 `meowo-reporter::agent::Agent` 注册表范式：`ProviderAccount` trait + `ALL` 注册表 + `for_provider(ProviderKey)` + enum↔registry 配对单测。通用 `Account`（全 Option + login_label）+ 通用「用量泳道」`ProviderUsage{lanes: Vec<UsageLane>, note}`（`UsageLane.kind` 为 **enum** `UsageKind`，遵循本仓 enum 单一事实源约定）。三家映射进同一泳道结构；查不到一律降级 None，绝不编造、绝不让整卡报错。

**Tech Stack:** Rust（meowo-app `account` 模块、meowo-reporter 复用 codex/kimi 路径解析）+ React/TS（AccountSection/UsageScreen/api.ts/i18n）。

## Global Constraints

- **claude 零回归（最高优先）**：claude 账号卡与底栏在重构后视觉/行为与今日一致；ClaudeProviderAccount 仅**包装**现有 `read_account`/`ensure_valid_token`/`fetch_usage_live` 并映射成 lanes，不改其逻辑。
- **命令签名切换不破坏中间态**：新增 `get_accounts()` / `refresh_usage(provider)` 时，**先 additive 保留**旧 `get_account`/`refresh_usage()`（Task 1），待前端切换到新命令后（Task 2）再删旧命令。任一任务结束 workspace 可编译可运行。
- **`UsageLane.kind` 用 enum `UsageKind`**（`FiveHour|SevenDay|Opus|Weekly|Balance|Other`，含 `as_str`/`from_str`/`ALL` + 配对单测），不用裸 String。
- **codex 全程纯本地、只读、不联网、不写 token**：账号解 `~/.codex/auth.json` 的 `id_token`（JWT，base64url 解中段，**不验签、仅展示**）；用量解最新 `~/.codex/sessions/.../rollout-*.jsonl` 的最后一条 `token_count` 事件。**已实地确认**字段：`payload.rate_limits.{primary,secondary}{used_percent, window_minutes, resets_at(unix秒)}`、`plan_type`。
- **kimi 不刷新 token**（跨进程一次性轮转会让正在跑的 kimi CLI 失效）：直接用 `credentials/kimi-code.json` 的 `access_token` 调 `GET {base}/usages`，遇 401/任何失败 → 降级 None，**绝不写回 kimi 凭据**。用量解析容错（`used↔remaining`、`resetAt↔reset_at↔reset_in/ttl`、单位 token/请求）。设备头（`X-Msh-*` from `~/.kimi-code/device_id`）best-effort 带上。kimi 账号 best-effort：尝试解 access_token JWT 取 email（只读 claim、绝不打印 token），无则 `login_label="已登录 · managed:kimi-code"`。
- **JWT 解码**：新建纯函数 `decode_jwt_payload(token) -> Option<serde_json::Value>`（按 `.` 切三段、base64url-no-pad 解中段、serde_json 解析）。**自写 base64url 解码器**（~20 行纯函数 + 单测），不加新依赖。严禁打印/落盘 token 原文；畸形 token 优雅降级 None。
- **路径复用**：codex/kimi 数据根复用 meowo-reporter 现有 `codex::codex_home()`（私有→提 pub）与 `kimi::kimi_share_dir()`（私有→提 pub，env `KIMI_SHARE_DIR`），**不在 account 模块重写**（避免 agent.rs 注释告诫的口径漂移）。
- **用量缓存**：`~/.meowo/usage-cache.json` 改 provider 分键 `{claude:{usage,fetched_at}, codex:{...}, kimi:{...}}`；旧扁平格式容错（当 claude 或忽略重拉）。
- 降级语义：`usage_supported()` 区分「不支持」（如 codex apikey 模式 / 第三方 claude 登录）vs「失败」。
- 代码英文、注释/commit 中文。
- 分支：`feat/provider-account-usage-20260630`（从 feat/kimi-code-cli-adapter-20260626 当前 HEAD 切出；type=feat，因这是新功能）。
- 验证：`cargo test -p meowo-app`（account 纯函数单测）、`node scripts/prepare-sidecar.mjs && cargo check -p meowo-app`、`cd app && bun run build && bun run test`。

## File Structure

- `app/src-tauri/src/account/mod.rs`（或保留 `account.rs` 改造）—— ProviderAccount trait + UsageKind/UsageLane/ProviderUsage/Account/ProviderAccountPayload + ALL 注册表 + for_provider + 通用缓存（provider 分键）+ JWT/base64url 纯函数。
- `app/src-tauri/src/account/claude.rs` / `codex.rs` / `kimi.rs` —— 三个 impl（或同文件分节，按实现者判断保持文件聚焦）。
- `crates/meowo-reporter/src/codex.rs` / `kimi.rs` —— `codex_home`/`kimi_share_dir` 提 pub。
- `app/src-tauri/src/lib.rs` —— 命令 get_accounts/refresh_usage(provider) 注册（先 additive）。
- `app/src/api.ts` —— 新类型 + getAccounts/refreshUsage(provider)。
- `app/src/views/About.tsx`（AccountSection 多卡）、`app/src/views/Sticker.tsx`（UsageScreen rows）、`app/src/demo/mock.ts`、`app/src/i18n/{zh,en}.ts`。

---

### Task 1 (P1): 后端 ProviderAccount trait + 通用类型 + Claude impl + 新命令（additive，零回归）

**Files:** Create `app/src-tauri/src/account/`（mod + claude.rs；把现有 account.rs 内容迁入 claude.rs 并在 mod.rs 定义 trait/类型/注册表/缓存）；Modify `app/src-tauri/src/lib.rs`（mod 路径 + additive 注册新命令）。

**Interfaces — Produces（契约，后续任务依赖）:**
```rust
// account/mod.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageKind { FiveHour, SevenDay, Opus, Weekly, Balance, Other }
impl UsageKind { pub fn as_str(self)->&'static str; pub fn from_str(&str)->UsageKind; pub const ALL: &'static [UsageKind]; }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UsageLane {
    pub kind: UsageKind,
    pub used_pct: Option<f64>,   // None = 非百分比型(余额)→前端显数值不画条
    pub used: Option<f64>,
    pub limit: Option<f64>,
    pub unit: Option<String>,    // "percent"|"tokens"|"requests"|"usd"
    pub resets_at: Option<String>, // ISO8601；codex unix 秒在 Rust 侧转 ISO
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ProviderUsage { pub lanes: Vec<UsageLane>, pub note: Option<String> }

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Account {
    pub email: Option<String>, pub display_name: Option<String>,
    pub organization: Option<String>, pub plan: Option<String>,
    pub login_label: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct ProviderAccountPayload {
    pub provider: String, pub account: Option<Account>,
    pub usage: Option<ProviderUsage>, pub usage_supported: bool,
}
pub trait ProviderAccount: Sync {
    fn key(&self) -> meowo_store::ProviderKey;
    fn account(&self) -> Option<Account>;
    fn usage(&self, force: bool) -> Option<ProviderUsage>;
    fn usage_supported(&self) -> bool { true }
}
pub fn for_provider(k: meowo_store::ProviderKey) -> &'static dyn ProviderAccount; // 遍历 ALL
pub fn read_cached_usage(k: meowo_store::ProviderKey) -> Option<ProviderUsage>;
```

- [ ] Step 1: 建 `account/mod.rs`，定义上述 UsageKind(+as_str/from_str/ALL)/UsageLane/ProviderUsage/Account/ProviderAccountPayload/ProviderAccount trait/ALL/for_provider。把现有 `account.rs` 迁入 `account/claude.rs` 作为 `ClaudeProviderAccount` 的实现细节（原自由函数 read_account/parse_account/parse_usage/ensure_valid_token/fetch_usage_live/缓存/Keychain 等保留）。
- [ ] Step 2: 写 UsageKind 单测（as_str/from_str roundtrip + 未知→Other）+ enum↔registry 配对单测（照抄 agent.rs `every_provider_key_has_agent_and_vice_versa`：每个 `ProviderKey::ALL` 有 ProviderAccount、反之、len 相等）。现有 account 纯函数单测（parse_account/parse_usage/...）全部迁移保留、保持绿。
- [ ] Step 3: ClaudeProviderAccount impl：`account()` 包 read_account 映射进通用 Account（email/display_name → Some）；`usage(force)` 调现有 refresh_usage_payload/fetch_usage_live 得旧 Usage，映射成 lanes：five_hour→`{kind:FiveHour,used_pct:utilization,unit:"percent",resets_at}`、seven_day→SevenDay、seven_day_opus→Opus（sonnet 作额外 lane 或忽略保持现视觉），extra_usage_enabled→note；`usage_supported()`=`!oauth_credentials_missing(...)`。
- [ ] Step 4: 缓存改 provider 分键：`read_cached_usage(key)`/`write_cached_usage(key,&ProviderUsage)`/`cache_is_fresh(key,ms)`，旧扁平 JSON 容错。
- [ ] Step 5: lib.rs **additive** 注册新命令 `get_accounts()->Vec<ProviderAccountPayload>`（遍历 ALL，usage 取缓存不联网）与 `async refresh_usage_v2(provider:String)->Result<ProviderUsage,String>`（spawn_blocking 调 for_provider(parse(provider)).usage(true)，None 时按 usage_supported 返回 UNAVAILABLE/USAGE_UNSUPPORTED）；**保留旧 `get_account`/`refresh_usage` 不动**（Task 2 删）。
- [ ] Step 6: `cargo test -p meowo-app` 全绿（含迁移的 claude 单测 + 新 enum/registry 测）；`node scripts/prepare-sidecar.mjs && cargo check -p meowo-app` 通过。
- [ ] Step 7: 提交 `feat(account): ProviderAccount trait + 通用用量泳道类型 + Claude impl（additive 新命令，零回归）`。

---

### Task 2 (P2): 前端迁移到通用形状 + 多卡骨架（仅 claude 数据）+ 删旧命令

**Files:** Modify `app/src/api.ts`、`app/src/views/About.tsx`、`app/src/views/Sticker.tsx`、`app/src/demo/mock.ts`、`app/src/i18n/{zh,en}.ts`、`app/src-tauri/src/lib.rs`（删旧命令）。

- [ ] Step 1: api.ts 新类型 `UsageKind`（联合 `"five_hour"|"seven_day"|"opus"|"weekly"|"balance"|"other"`）、`UsageLane`、`ProviderUsage`、`Account`（字段全 `string|null` + `login_label`）、`ProviderAccountPayload`；新函数 `getAccounts()`、`refreshUsage(provider)`；删旧 `Usage`/`UsageWindow`/`AccountPayload`/`getAccount`/旧 `refreshUsage()`。
- [ ] Step 2: mock.ts：`get_account`→`get_accounts`（返回 `[{provider:"claude",account,usage,usage_supported:true}]`）、`refresh_usage`→`refresh_usage_v2(provider)` 返回 ProviderUsage。
- [ ] Step 3: About.tsx AccountSection：`getAccounts()` 拿全部；遍历 `account!=null` 的 provider 各渲一张同结构卡（卡顶复用 `providers.tsx` 的 `PROVIDERS[provider].label/Icon`）；一张都没有→现有「未登录」占位。`UsageBar` 入参从 `{utilization,resets_at}` 改 `{used_pct,resets_at,label}`；遍历 `usage.lanes`：used_pct!=null 画条、==null 显数值（余额）。每卡独立刷新/不支持/失败三态。lane 标签按 kind→i18n（新增 laneFiveHour/laneSevenDay/laneOpus/laneWeekly/laneBalance，未知回退 kind）。
- [ ] Step 4: Sticker.tsx 底栏 UsageScreen：入参从固定 `wins` 改为 `rows:{provider,label,pct|null,amount?}[]`；本任务先只喂 claude（一行主泳道 five_hour），视觉与今日一致。
- [ ] Step 5: 删 lib.rs 旧 `get_account`/`refresh_usage` 命令 + 注册；新命令 `refresh_usage_v2` 可改名回 `refresh_usage`（此时无旧冲突）。
- [ ] Step 6: `cd app && bun run build && bun run test` 全绿；`cargo check -p meowo-app` 通过。真机视觉确认 claude 账号卡 + 底栏无回归。
- [ ] Step 7: 提交 `refactor(account): 前端迁移到通用 ProviderUsage 形状 + 多卡骨架，删旧单 provider 命令`。

---

### Task 3 (P3): Codex impl（纯本地、只读、已实地确认）

**Files:** Modify `crates/meowo-reporter/src/codex.rs`（`codex_home` 提 pub）；Create/Modify `app/src-tauri/src/account/codex.rs` + mod 注册。

- [ ] Step 1: meowo-reporter `codex.rs`：`codex_home()` 改 `pub`。account/mod.rs 加 `decode_jwt_payload(&str)->Option<serde_json::Value>` + 自写 `base64url_decode_nopad(&str)->Option<Vec<u8>>`，各带单测（畸形/缺段→None）。
- [ ] Step 2: `parse_codex_account(auth_json:&Value)->Option<Account>`（纯函数）：`auth_mode=="chatgpt"` 时解 `tokens.id_token` → email（顶层 claim）、plan（`["https://api.openai.com/auth"]["chatgpt_plan_type"]`）、org（organization_id）；claim 缺失→对应 None。`apikey`→`login_label:"API Key"` 余 None。单测用合成 JWT（自造 payload 编码）。
- [ ] Step 3: `parse_codex_usage(token_count_payload:&Value)->ProviderUsage`（纯函数）：`rate_limits.primary{used_percent,window_minutes,resets_at}`→lane FiveHour（used_pct,unit"percent",resets_at unix→ISO）；`secondary`→Weekly；兼容 `resets_at` 缺失时用 `记录ts+resets_in_seconds`（若无 ts 则 None）；`plan_type`/`credits`→note。单测用确认过的真实结构（primary window_minutes:300、secondary:10080、resets_at unix 秒、plan_type)。
- [ ] Step 4: CodexProviderAccount impl：`account()` 读 `codex_home()/auth.json`→parse_codex_account；`usage(_)` 在 `codex_home()/sessions` 与 `archived_sessions` 下按 mtime 取最新 rollout-*.jsonl、倒序找最后一条 `{"type":"event_msg"...}` 或顶层 `payload.type=="token_count"` 行→parse_codex_usage（**新读取逻辑**：mtime-latest + tail-scan，非复用 walk_find）；`usage_supported()`=`auth_mode=="chatgpt"`。注册进 ALL。
- [ ] Step 5: `cargo test -p meowo-app -p meowo-reporter` 全绿；`cargo check -p meowo-app` 通过。真机（已登录 codex）`bun run tauri dev` 或装机验证 codex 账号卡（email/plan）+ 用量卡（5h/周窗）出现。
- [ ] Step 6: 提交 `feat(account): Codex 账号(id_token)+用量(session JSONL) 纯本地实现`。

---

### Task 4 (P4): Kimi impl（用量联网 best-effort + 账号降级）

**Files:** Modify `crates/meowo-reporter/src/kimi.rs`（`kimi_share_dir` 提 pub）；Create/Modify `app/src-tauri/src/account/kimi.rs` + mod 注册。

- [ ] Step 1: meowo-reporter `kimi.rs`：`kimi_share_dir()` 改 `pub`。
- [ ] Step 2: `parse_kimi_usage(json:&Value)->Option<ProviderUsage>`（纯函数，**容错**）：`usage{name,used,limit,resetAt}`→lane Weekly（used_pct=used/limit*100 若 limit>0、used/limit 绝对、unit"tokens"、resetAt→ISO）；`limits[]{detail,window}`→按 window.timeUnit/duration 派生 lane（5h/7d）；兼容字段漂移 `used↔remaining`(remaining 时 used=limit-remaining)、`resetAt↔reset_at↔reset_in/ttl(秒)`；open-platform `data.available_balance`→lane Balance(unit"usd",used_pct:None,resets_at:None)。解析不出→None。单测覆盖几种推断 schema + 漂移 + 畸形→None。
- [ ] Step 3: KimiProviderAccount impl：
  - `usage(_)`：读 `kimi_share_dir()/credentials/kimi-code.json` 的 access_token；base_url 取 config.toml `[providers."managed:kimi-code"].base_url`（缺省 `https://api.kimi.com/coding/v1`，去尾斜杠）；`GET {base}/usages`，header `Authorization: Bearer <access_token>` + `Accept: application/json` + best-effort `User-Agent`/`X-Msh-*`（device_id 取 `kimi_share_dir()/device_id`）；8s 超时；**不刷新 token、不写回**；任何非 200/网络错/解析失败→None。成功→parse_kimi_usage + 写缓存。
  - `account()`：best-effort 解 access_token JWT 取 email（decode_jwt_payload，只读 claim、不打印 token）；无→`login_label:"已登录 · managed:kimi-code"`（+ expires_at 过期时间作 note 可选）。凭据文件不存在→None。
  - `usage_supported()`：凭据文件存在即 true。
  注册进 ALL。
- [ ] Step 4: `cargo test -p meowo-app -p meowo-reporter` 全绿；`cargo check -p meowo-app` 通过。真机（已登录 kimi）验证用量卡（best-effort：能出最好；出不来也不崩、不影响 claude/codex）。
- [ ] Step 5: 提交 `feat(account): Kimi 用量(/usages best-effort 容错)+账号降级实现`。

---

### Task 5 (P5): 底栏多 provider + i18n(zh/en) 收尾

**Files:** Modify `app/src/views/Sticker.tsx`（UsageScreen 多行）、`app/src/i18n/{zh,en}.ts`（补齐全部新文案）。

- [ ] Step 1: 底栏 UsageScreen：每个有可显示主泳道的 provider 一行（行首迷你品牌图标复用 PROVIDERS[].Icon + 主泳道 label + 条/金额），上限 3 行；claude/codex 主泳道取 FiveHour、kimi 取 Weekly 或 Balance；used_pct!=null 画液柱（沿用 usageSev 绿/黄/红）、==null 显金额。底栏数据 effect 从 getAccounts() 拿缓存秒显 + 对 present provider 定时 refreshUsage。仅 claude 登录时=一行，与今日基本一致。
- [ ] Step 2: i18n zh+en 补齐：lane 标签（laneFiveHour/laneSevenDay/laneOpus/laneWeekly/laneBalance）、kimi `login_label`/「已登录」、balance 单位、codex/kimi 的「未登录/不支持用量」文案。en.ts 受 `Dict=typeof zh` 约束，键齐全。
- [ ] Step 3: `cd app && bun run build && bun run test` 全绿。真机 e2e：claude/codex/kimi 账号卡 + 底栏。
- [ ] Step 4: 提交 `feat(account): 底栏多 provider 主泳道 + i18n(zh/en) 收尾`。

---

## Self-Review / 已纳入 critique 的修正

- kimi **不刷新 token**（401→None，不写回凭据）——避开跨进程一次性轮转危害。
- `UsageKind` 用 **enum + 配对单测**（不用裸 String）——遵循本仓 enum 单一事实源约定。
- claude 命令切换 **additive**（Task 1 加新、Task 2 删旧）——避免 P1→P2 中间态坏。
- codex 用量是**新读取逻辑**（mtime-latest + tail-scan），不假装复用 walk_find/read_head_lines；字段已**实地确认**。
- kimi 用量标为 **best-effort 容错**：研究推断的 /usages 字段未真机验证，解析失败整卡降级 None、不崩、不影响 claude/codex。
- 账号字段全 Option + login_label 兜底，**不编造**；JWT 只读 claim、不验签、不打印/落盘 token。
- 路径复用 meowo-reporter 的 codex_home/kimi_share_dir（提 pub），不重写。

## 本计划之外 / 待真机校准

- kimi /usages 真实字段/单位/设备头：best-effort 实现后，若你跑过验证命令拿到真实 JSON，可据此收紧 `parse_kimi_usage`（容错解析已留好扩展点）。
- codex 联网 wham/usage 实时路径：默认不实现（本地 JSONL 够用，且联网刷 token 与 codex CLI 并发冲突）。
- kimi 账号 email：若 access_token 非 JWT/无 email claim，则只显「已登录」（诚实降级）。

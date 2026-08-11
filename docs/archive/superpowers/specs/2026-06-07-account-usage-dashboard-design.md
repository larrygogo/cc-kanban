# 设置页「账号」分区：账号 + 实时用量 + 每日用量 — 设计

> 日期：2026-06-07
> 状态：已通过设计评审，待写实现计划
> 平台：Windows-only（与项目其余部分一致）

## 背景与目标

在设置窗口新增「账号」分区，展示当前 Claude Code 账号、实时用量配额、以及每日用量历史。

调研确认：实时配额（状态栏那种「Usage X% · resets in」）**只能**通过带 OAuth token 的 `GET /api/oauth/usage` 拿到，不落本地缓存。逐日历史 live 端点不提供，改读 CC 自己的 `stats-cache.json`。

## 已验证的数据契约

| 区块 | 来源 | 说明 |
|------|------|------|
| 账号 | `~/.claude.json` → `oauthAccount` | 字段：`emailAddress`、`displayName`、`organizationName`、`billingType`、`seatTier`、`subscriptionType` |
| 凭据 | `~/.claude/.credentials.json` → `claudeAiOauth` | 字段：`accessToken`、`refreshToken`、`expiresAt`(ms)、`scopes`、`subscriptionType`、`rateLimitTier`。明文 JSON，顶层还有 `mcpOAuth` 必须保留 |
| 实时用量 | `GET https://api.anthropic.com/api/oauth/usage` | 头：`Authorization: Bearer <accessToken>` + `anthropic-beta: oauth-2025-04-20`。验证返回 HTTP 200 |
| token 刷新 | `POST https://platform.claude.com/v1/oauth/token` | body：`grant_type=refresh_token` + `refresh_token` + `client_id=9d1c250a-e61b-44d9-88ed-5944d1962f5e`（Claude Code 公开 OAuth client） |
| 每日用量 | `~/.claude/stats-cache.json` | `dailyActivity`(date/messageCount/sessionCount/toolCallCount) + `dailyModelTokens`(date/tokensByModel) + `lastComputedDate`（**可能过期**，仅 CC 跑 /stats 等时重算） |

### `/api/oauth/usage` 响应结构（实测）

```json
{
  "five_hour":  {"utilization": 13.0, "resets_at": "2026-06-07T08:50:01.112515+00:00"},
  "seven_day":  {"utilization": 19.0, "resets_at": "2026-06-11T12:00:00+00:00"},
  "seven_day_opus":   null,
  "seven_day_sonnet": {"utilization": 2.0, "resets_at": "..."},
  "extra_usage": {"is_enabled": false, "monthly_limit": null, "used_credits": null, "utilization": null, "currency": null, "disabled_reason": null}
}
```
各 bucket 可能为 `null`（该档位不适用）。`utilization` 是百分比（0-100），`resets_at` 是 ISO8601。

## 设计决策（已与用户确认）

1. **走 C（实时端点）** 拿真实配额%。
2. **先缓存后请求**（stale-while-revalidate）：打开分区先显示上次缓存的用量快照，再后台联网刷新。
3. **token 自动刷新**：过期则刷新并**原子写回** `.credentials.json`。
4. **每日用量**用 `stats-cache.json`（带"截至 X 日"标注），不做重型 transcript 聚合。
5. **接受已知风险**：刷新写回凭据与 CC 并发时的低概率竞争（缓解：仅真过期才刷 + 原子写）。

## 架构

```
account 模块（app/src-tauri/src/account.rs，Windows 实现 + 非 Windows 桩）
  ├─ read_account() -> Option<Account>          读 ~/.claude.json oauthAccount
  ├─ read_daily() -> Option<DailyStats>         读 stats-cache.json（近 N 天 + lastComputedDate）
  ├─ read_cached_usage() -> Option<Usage>       读 ~/.meowo/usage-cache.json
  ├─ fetch_usage_live() -> Result<Usage>        ensure_token → GET /api/oauth/usage → 写缓存
  │    └─ ensure_valid_token() -> Result<String>  过期则刷新 + 原子写回 .credentials.json
  └─ 纯函数（可单测）：parse_usage / parse_daily / is_expired / merge_credentials
命令（app/src-tauri/src/lib.rs）：
  get_account()    -> AccountPayload   账号 + 每日 + 缓存用量快照（瞬时、不联网）
  refresh_usage()  -> Result<Usage>    联网拉最新（含按需刷新 token），更新缓存
前端（app/src/views/About.tsx）：新增「账号」分区 + api.ts 类型 + 命令封装
```

### 模块边界与职责

- `account.rs` 独立承担「读 CC 账号/凭据/统计 + 调用量端点 + 刷新 token」，不与现有 sticker/通知逻辑耦合。HTTP 用轻量阻塞客户端 `ureq`（rustls），在 `tauri::async_runtime::spawn_blocking` 或独立线程里跑，3-5s 超时。
- 用量缓存文件：`~/.meowo/usage-cache.json`（与 board.db / settings.json 同目录），结构 = `{ usage: Usage, fetched_at: i64 }`。

### 类型（serde）

```
Account { email, display_name, organization, plan }     // plan 由 billingType/seatTier/subscriptionType 归一
UsageWindow { utilization: f64, resets_at: String }
Usage { five_hour: Option<UsageWindow>, seven_day: Option<UsageWindow>,
        seven_day_opus: Option<UsageWindow>, seven_day_sonnet: Option<UsageWindow>,
        extra_usage_enabled: bool }
DailyEntry { date: String, message_count, session_count, tokens: i64 }
DailyStats { days: Vec<DailyEntry>, last_computed_date: String }
AccountPayload { account: Option<Account>, daily: Option<DailyStats>, usage: Option<Usage> }
```

## 数据流

1. 设置页切到「账号」→ 调 `get_account()`：立即渲染账号卡 + 每日列表 + **缓存的用量**（秒开，可能略旧）。
2. 紧接着调 `refresh_usage()`：后台联网（必要时刷新 token）→ 成功则替换用量数字 + 写缓存；失败则保留缓存值并在用量区显示"刷新失败/暂不可用"。
3. 分区内提供「刷新」按钮手动触发 `refresh_usage()`。
4. `refresh_usage()` 内 60s 限频：距上次成功拉取不足 60s 直接返回缓存，不打端点。

## token 刷新细节

- `ensure_valid_token()`：读 `.credentials.json`；若 `now_ms <= expiresAt - 60_000`（留 1 分钟余量）直接用 accessToken；否则：
  - `POST /v1/oauth/token`（JSON：`grant_type/refresh_token/client_id`）→ 解析 `access_token` / `refresh_token` / `expires_in`
  - `merge_credentials`：读原文件 → 只改 `claudeAiOauth.{accessToken,refreshToken,expiresAt}`（`expiresAt = now + expires_in*1000`）→ 保留 `mcpOAuth` 等所有其它字段 → 写临时文件 → rename 原子替换
  - 刷新失败 → 返回错误，用量区降级
- 写回是唯一会修改 CC 文件的操作，必须原子且字段保留完整。

## 错误处理（全程 best-effort，互不影响）

- `~/.claude.json` / `.credentials.json` / `stats-cache.json` 任一缺失或解析失败 → 对应区块返回 `None`，UI 显示占位（"未登录 Claude Code" / "暂无每日数据"）。
- 端点 401 / 网络失败 / 超时 / 端点结构变更 → 用量区显示"用量暂不可用"，其余区块照常。
- 整个「账号」分区任何失败都**不影响**设置页其它分区与贴纸主功能。

## 测试计划

- **纯函数单测（Rust）**：
  - `parse_usage`：完整 JSON / 含 null bucket / 缺字段 → 正确 Option 化。
  - `parse_daily`：从 stats-cache 片段取近 N 天 + lastComputedDate；空/缺失降级。
  - `is_token_expired`：基于 expiresAt + 余量边界。
  - `merge_credentials`：给定原 JSON（含 mcpOAuth）+ 新 token 三元组 → 输出只改三字段、其余原样保留。
- **网络 / 刷新写回 / 命令**：I/O 与外部依赖，靠编译 + 手测（含 token 过期场景）。
- **前端**：类型经 `tsc`；分区渲染靠手测（设置页无既有测试）。

## 非目标（YAGNI）

- 不做重型 transcript 全量聚合（1115 会话太重）；每日用量以 stats-cache 为准 + 标注。
- 不做用量历史图表的复杂可视化（先用简单柱/列表）。
- 不主动后台轮询用量（仅打开分区 / 手动刷新时拉）。
- 不支持非 Windows（与项目一致）。
- 不改 DB schema。

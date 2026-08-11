# 底栏配额标签式 + 账号卡贴纸开关 + kimi 已登录 + logo 间距 实施计划

> **For agentic workers:** 用 superpowers:subagent-driven-development 执行。已过 brainstorming 设计批准。

**Goal:** 账号卡加「在贴纸显示配额」开关（存 Settings）；底栏配额从「每 provider 一行」改为「每开启 provider 一个标签、点选显示该 provider 5h+7d」；kimi 账号只显「已登录」；修账号卡 logo 与 provider 名间距过近。

**Architecture:** 后端 `Settings` 加 `sticker_quota_providers: Vec<String>`（开启贴纸配额的 provider key，默认 `["claude"]`）；前端账号卡开关读写它，底栏据它渲染标签式用量。复用现有 `getAccounts`/`refreshUsage`/`UsageLane`/`PROVIDERS`。

## Global Constraints
- 代码英文、注释中文。claude 之外零回归（claude 默认在列表里，底栏默认仍显 claude）。
- Rust `Settings` 与 TS `Settings` 类型、demo mock 三者字段一致。
- 分支：直接在 feat/kimi-code-cli-adapter-20260626（延续未推送的账号+用量特性）。
- 验证：`node scripts/prepare-sidecar.mjs && cargo test -p meowo-app`、`cargo clippy -p meowo-app --all-targets -- -D warnings`、`cd app && bun run build && bun run test`。

## 改动

### 1. 后端 Settings + kimi label
- `app/src-tauri/src/settings.rs`：`Settings` 加 `sticker_quota_providers: Vec<String>`，默认 `vec!["claude".into()]`；序列化/反序列化缺省时给默认（`#[serde(default = "...")]`）；get_settings/set_settings 透传（若 Settings 是整体读写则自动带上）。
- `app/src-tauri/src/account/kimi.rs`：`account()` 的降级 `login_label` 从 `"已登录 · managed:kimi-code"` 改为 `"已登录"`。

### 2. 前端类型 + mock
- `app/src/api.ts`：`Settings` type 加 `sticker_quota_providers: string[]`。
- `app/src/demo/mock.ts`：`get_settings` 返回含 `sticker_quota_providers: ["claude"]`；`set_settings` 照常。

### 3. 账号卡：开关 + kimi 显示 + logo 间距
- `app/src/views/About.tsx` ProviderCard：
  - 加一个开关（toggle/checkbox，复用现有 toggle 组件/样式）「在贴纸显示配额」，勾选态 = 该 provider 在 `settings.sticker_quota_providers` 中；切换时读当前 settings、增删该 provider key、`setSettings(next)`。需要 AccountSection 拿到 settings（`getSettings()`）并能刷新。
  - kimi 账号显示：`login_label` 现为「已登录」，现有渲染 `!acc.email && acc.login_label` 已能显示——确认 kimi 卡显示「已登录」而非旧后缀。
- CSS（`app/src/styles.css`）：账号卡卡顶 provider **logo 与名字间距过近** → 给 logo 与名字之间加 gap（如 `.provider-card` 卡头的图标容器 `margin-right` 或 flex `gap`，加到 ~8px 视觉舒适）。定位现有卡头结构后最小化调整，只加间距不改布局。

### 4. 底栏标签式（Sticker.tsx UsageScreen 重做）
- 数据：从 `getSettings()` 拿 `sticker_quota_providers`；只对**在该列表且有用量**的 provider 显示。底栏用量 effect 仍 `getAccounts()` + 对这些 provider `refreshUsage`，维护 `Map<provider, ProviderUsage>`。
- 渲染：一行**标签**（每个符合条件 provider 一个小标签 = 品牌图标 `PROVIDERS[p].Icon`）；`selected` provider（局部 state，默认第一个）高亮。选中标签下方显示该 provider 的：
  - **5h**：其 `kind==="five_hour"` 的 lane（进度条，沿用 usageSev 颜色）。
  - **7d**：其 `kind==="seven_day"`（claude）或 `kind==="weekly"`（codex/kimi）的 lane。
  - lane 缺失则该条不显示。used_pct==null（余额型）显数值。
- 标签点击切换 selected。符合条件 provider 为空 → 底栏配额区不渲染。
- 保持底栏紧凑；标签行 + ≤2 条用量条。i18n：5h/7d 标签复用 laneFiveHour/laneSevenDay/laneWeekly；「在贴纸显示配额」开关文案新增 zh+en（如 `showQuotaOnSticker`）。

## Self-Review 要点
- Settings 三处（Rust/TS/mock）字段一致，默认 `["claude"]`，旧配置无此字段时反序列化给默认（不 panic）。
- claude 默认在列表 → 底栏默认显 claude 标签（≈今日），零回归。
- 底栏 selected state 切换正确、无陈旧闭包；符合条件为空时不渲染。
- CSS 只加间距不破坏卡头布局。

## 本计划之外
- 用量单位精确文案（token/请求数）待真机进一步确认。

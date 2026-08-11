# 待交互通知 + 通知总开关 — 设计

> 日期：2026-06-07
> 状态：已通过设计评审，待写实现计划
> 前置：本特性建立在已合并的「会话错误状态检测 + 桌面通知」之上（见 `2026-06-07-session-error-detection-design.md`）。错误通知与 5s 轮询 `spawn_liveness_watch` 已存在。

## 背景与目标

错误通知已上线，但 Meowo 的核心承诺是「无需切来切去」——**会话需要你回复（待交互）时也该主动提醒**。同时，既然开始往系统推通知，用户需要一个**总开关**来控制。

本特性：
1. 会话进入「待交互」时弹一条桌面通知（立即弹、去重）。
2. 新增**单个总开关**，统一控制所有桌面通知（待交互 + 错误）。

## 设计决策（已与用户确认）

1. **单个总开关**：一个开关管住全部通知，不做分类（错误/待交互）子开关。
2. **立即弹**：会话一进入 waiting 即弹，不做延迟。
3. **默认 ON**：保持现有错误通知行为不变，待交互直接生效；老 `settings.json` 缺字段时也默认 ON。

## 组件与改动点

### 1. 总开关（设置项）

- `Settings` 结构（`app/src-tauri/src/lib.rs`）增字段 `notifications_enabled: bool`。
- **向后兼容默认 ON**：
  - 字段加 `#[serde(default = "default_true")]`（老文件缺字段时填 `true`）。
  - 手动 `impl Default for Settings`（`notifications_enabled: true`、`archive_hide_days: 0`），替代当前的 `#[derive(Default)]`——因为 `derive(Default)` 会给 bool 填 `false`，而整文件缺失/解析失败时用的是 `Default`，必须为 ON。
- 设置页「通用」（`app/src/views/About.tsx` 的 `GeneralSection`）加一个 `Switch` 行：
  - 标题「桌面通知」，描述「会话需要你回复或出错时弹系统通知」。
  - 沿用现有 `getSettings`/`setSettings` 乐观更新模式（与「开机自启」「归档自动隐藏」一致）。
- 前端 `Settings` 类型（`app/src/api.ts`）增 `notifications_enabled: boolean`。

### 2. 待交互通知（后端轮询）

在已有的 `spawn_liveness_watch`（`app/src-tauri/src/lib.rs`，5s 轮询）内，对**连接中**会话（已有 `status != "ended" && pid_is_claude` 过滤）：

- **每轮 `load_settings()`** 读总开关（文件读极廉价；设置改动 5s 内生效，无需在循环里监听事件）。
- 复用本轮已有的 `analyze_transcript` 调用——现在保留完整 `TranscriptInfo`（`title` + `error`），而非只取 `error`。
- **优先级**：
  - 若 `info.error` 为 Some（errored）→ 走**现有错误通知**路径，**跳过**该会话的待交互通知（同一会话不双重打扰）。
  - 否则若该会话 DB `status == "waiting"` → 走**待交互通知**路径。
- **待交互去重**：新增 `notified_waiting: HashMap<String, String>`（cc_session_id → 上次通知指纹），复用 `should_notify`。
  - 指纹 = 该会话的 `last_event_at`（字符串化）。每次 Stop hook 都会刷新 `last_event_at`，故每个新的「等待回合」是新指纹 → 弹一次；用户回复后再次进入 waiting → 新指纹 → 再弹。
  - 状态离开 waiting（running/ended）→ 从 map 移除该会话。
  - 轮询末尾 `notified_waiting.retain(|k,_| present.contains_key(k))`，与错误通知一致防止 map 无限增长（`present` 已含本轮所有被扫描的连接中会话）。
- **待交互文案**：标题「等待你回复」，正文「{项目名} · {标题}」。标题优先用 `info.title`，兜底用 DB `s.task_title`。

### 3. 总开关门控（统一）

- 错误通知与待交互通知的 `.show()` 调用都包在 `if notifications_enabled` 内。
- **关键**：总开关 OFF 时，**仍照常更新去重 map**（`notified` / `notified_waiting`），只是不 `.show()`。这样中途打开开关不会把积压的旧错误/待交互一次性炸出来——与「首扫只播种」同理。
- 沿用现有 `seeded` 标志：启动首扫对待交互也只播种不弹。

## 架构图

```
spawn_liveness_watch（5s 轮询，已存在）
  ├─ load_settings() → notifications_enabled（每轮读）
  ├─ reap/orphan/board-changed（已存在，不变）
  ├─ 对每个连接中会话调 analyze_transcript（已存在；现取完整 TranscriptInfo）
  │   ├─ errored → 错误通知（已存在）；门控 .show() by 总开关；跳过待交互
  │   └─ status==waiting（且非 errored）→ 待交互通知（新）；门控 .show() by 总开关
  ├─ notified（错误，已存在）+ notified_waiting（待交互，新）各自去重
  └─ 两个 map 均 retain 防增长；seeded 首扫播种不弹
```

## 错误处理

- `load_settings()` 失败 → `unwrap_or_default()` → `Default`（ON），不影响轮询。
- 通知发送失败 → `let _ =` 吞掉（沿用现状）。
- DB / transcript 读失败 → 沿用现有 best-effort（`unwrap_or_default`、`TranscriptInfo::default`）。

## 测试计划

- **Rust — Settings 向后兼容**：反序列化 `{}` 与 `{"archive_hide_days":7}` → `notifications_enabled == true`；显式 `{"notifications_enabled":false}` → `false`。
- **Rust — 待交互判定纯函数**（若抽出）：给定 `(errored: bool, status: &str, last_event_at)` → 返回待交互指纹 `Option<String>`（errored 时 None；status==waiting 且非 errored 时 Some(ts)；其它 None）。单测覆盖：错误优先、waiting 出指纹、指纹随 last_event_at 变化。
- **复用**：`should_notify` 已有测试覆盖去重四种情形。
- **前端**：`Settings` 加字段后靠 `tsc` 保证类型一致；设置页无现成测试，不新增重型测试。

## 非目标（YAGNI）

- 不做分类开关（错误/待交互分别开关）。
- 不做声音、免打扰时段、延迟弹。
- 不做通知点击跳转。
- 不改 DB schema、不加 hook、不动 meowo-reporter。

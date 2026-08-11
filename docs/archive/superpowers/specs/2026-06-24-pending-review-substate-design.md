# 「待审批/待权限」会话子态 — 设计

> 日期：2026-06-24
> 状态：已通过设计评审，待用户审阅 spec
> 前置：建立在已上线的「待交互通知 + 总开关」（`2026-06-07-waiting-notification-and-toggle-design.md`）与「会话错误检测」（`2026-06-07-session-error-detection-design.md`）之上。`spawn_liveness_watch`（5s 轮询）、`should_notify` 去重、`notified` / `notified_waiting` 两张指纹 map、`seeded` 首扫播种、总开关门控均已存在。

## 背景与目标

现状：Meowo 的 `waiting` 仅由 `Stop` hook 写入 = **回合边界**（Claude 说完一轮、等你下一句）。但 Claude 真正**卡在回合中途等你介入**的三种情形——等你批准工具调用、等你回答提问（`AskUserQuestion`）、等你批准计划（`ExitPlanMode`）——此刻会话仍是 `running`，Meowo 完全无标记，而这恰恰是最该立刻处理的高价值信号。

目标：新增一个**正交子态** `pending_review`，把这三种「回合中途阻塞、等你介入」识别出来，在卡片上醒目标记、排到最顶、弹一条去重通知。这是对标 moshi-hook 的 `pendingPermissionAt` / `lastToolName=AskUserQuestion` 设计、Meowo 此前缺失的一环。

## 真实数据核实（非凭文档）

- 本机现役 moshi（Go 版）在 `~/.claude/settings.json` 实际挂的就是：`PermissionRequest`（全量）+ `PreToolUse`/`PostToolUse` 限定 matcher `AskUserQuestion`、`ExitPlanMode`——经过实战验证的配置。
- Claude Code hook 事实：
  - `PermissionRequest`：权限弹窗出现时触发，带 `tool_name` / `tool_input`，支持 matcher。**没有配对的「已解除」事件**——用户批准/拒绝后不再触发 hook。
  - `AskUserQuestion` / `ExitPlanMode` 是**工具名**，非事件：`PreToolUse(tool=AskUserQuestion)` = Claude 在问你；对应 `PostToolUse` = 你已答（干净解除）。`ExitPlanMode` 批准走 `PermissionRequest(tool=ExitPlanMode)`。
- 结论：进入态可精确捕获；解除态对 `PermissionRequest` 类只能靠**下一个事件兜底**（这正是设计采用「下一个事件即清」的原因）。

## 设计决策（已与用户确认）

1. **正交标记，不动状态机**：在 `sessions` 表加 `pending_review TEXT`（NULL/`approval`/`question`/`plan`），不新增 `SessionStatus` 变体。会话保持 `running`，子态叠加表达「running 但正卡住等你」。
2. **全套三类检测**：`PermissionRequest` + `PreToolUse(AskUserQuestion)` + `PreToolUse(ExitPlanMode)`，用 matcher 限定只在这几种情形触发，零额外开销，且能在卡片上分辨「待批准 / 待回答 / 待批计划」。
3. **并入「待交互」tab + 醒目徽标置顶 + 去重通知**，不新开 tab。

## 组件与改动点

### 1. 数据层（`crates/meowo-store`）

- **schema**：`migrations.rs` 的 `SCHEMA` 在 `sessions` 表加 `pending_review TEXT`（默认 NULL）。
- **旧库迁移**：`Store::migrate` 沿用现有 pid/cwd/archived 的 `ALTER TABLE sessions ADD COLUMN pending_review TEXT` 补列模式（`ADD COLUMN` 若已存在则忽略错误，与现状一致）。
- **models.rs**：可选新增 `PendingReview { Approval, Question, Plan }` 枚举 + `as_str`/`from_str`（与 `TodoStatus` 同模式）。子态值在库中以小写字符串存。
- **store.rs**：新增两个方法：
  - `set_pending_review(sid, kind, now_ms)`：`UPDATE sessions SET pending_review=?, last_event_at=? WHERE id=?`（**刷新 last_event_at**：让卡片排到最近活跃、并作为去重指纹）。
  - `clear_pending_review(sid)`：`UPDATE sessions SET pending_review=NULL WHERE id=?`（不动 last_event_at——由同回合的兄弟调用负责时间戳）。
- **query.rs**：`LiveSession` 加 `pending_review: Option<String>`；`live_sessions()` 的 SELECT 增 `s.pending_review` 并回填。`overview()` 的 `active_sessions`（`status IN ('running','waiting')`）无需改——pending 会话本就是 running，已计入。

### 2. 事件流转（`crates/meowo-reporter/src/dispatch.rs`）

`HookEvent` 已有 `tool_name`，**不改解析**。新增/调整分支：

- **新增 `"PermissionRequest"`**：`lookup_session` → 按 `tool_name` 映射 kind（`Some("ExitPlanMode")`→`plan`、`Some("AskUserQuestion")`→`question`、其余/None→`approval`）→ `set_pending_review`。
- **新增 `"PreToolUse"`**：仅 `AskUserQuestion`→`question`、`ExitPlanMode`→`plan`；其它 tool_name 一律无操作（安装侧已用 matcher 限定，这里再兜一层防御）。
- **清除（统一「下一个事件即清」）**：在以下既有分支顶部、`lookup_session` 拿到 `sid` 后各加一句 `store.clear_pending_review(sid)?;`：
  - `"PostToolUse"`（覆盖 AskUserQuestion 已答、批准后工具开跑、及任意后续工具）
  - `"UserPromptSubmit"`、`"Stop"`、`"SessionEnd"`
- 顺序：清除在该分支原有 store 调用之前；这些兄弟调用（`touch_session`/`on_user_prompt`/`set_session_status`/`end_session`）照常刷新 `last_event_at`。

### 3. Hook 接线（`app/src-tauri/src/ccsetup.rs` + `scripts/install-hooks.mjs`）

当前 `ensure_hooks` 把 5 个事件各按 `matcher:"*"` 挂一条、且 `find_reporter_hook` 忽略 matcher。改为 **matcher 感知**以支持「同一事件下多条按 matcher 区分的 meowo-reporter 条目」：

- `HOOK_EVENTS:[&str;5]` → `HOOK_SPECS:&[(event, matcher)]`：
  ```
  ("SessionStart","*"), ("UserPromptSubmit","*"), ("PostToolUse","*"),
  ("Stop","*"), ("SessionEnd","*"),
  ("PermissionRequest","*"),
  ("PreToolUse","AskUserQuestion"), ("PreToolUse","ExitPlanMode"),
  ```
- `find_reporter_hook` → `find_reporter_hook_with_matcher(event_arr, matcher)`：匹配「`entry["matcher"] == matcher` **且** 该 entry 的某 hook 命令是 meowo-reporter（`reporter_exe_path` 命中）」。命中则更新路径，未命中则按该 matcher 追加 `{ matcher, hooks:[{type:command, command, timeout:5}] }`。
- **幂等/保留性**：
  - 老库 5 个 `*` 事件仍命中升级（matcher 同为 `*`）。
  - 用户自有 `PreToolUse:Bash`（如 node 预检）matcher 不同且非 meowo-reporter → 原封不动。
  - `reporter_exe_path` 严格判定不变（不误伤 `node tools/meowo-reporter-notify.js` 等）。
- `install-hooks.mjs` 同步改：`EVENTS` → 同样的 `(event,matcher)` 列表；幂等过滤键从「`command` 完全相等」改为「`command` 相等 **且** `matcher` 相等」，避免同事件多 matcher 条目互相误删。

### 4. 应用层（`app/src-tauri/src/lib.rs`）

- `LiveItem`（flatten 自 `LiveSession`）加 `pending_review: Option<String>` 透传到前端。
- `spawn_liveness_watch`：加第三张去重 map `notified_pending`。每会话判定优先级 **error > pending > waiting**：
  - `errored` → 错误通知（现状），跳过 pending/waiting。
  - 否则 `pending_review` 为 Some(kind) → **pending 通知**（新），跳过 waiting。
  - 否则 `status=="waiting"` → 待交互通知（现状）。
  - 抽 `pending_fingerprint(errored, pending_review, last_event_at) -> Option<String>`：errored 时 None；pending 为 Some 时 `Some("{kind}:{last_event_at}")`；否则 None（纯函数，便于单测，与 `waiting_fingerprint` 同模式）。
  - 文案按 kind：标题 `需要你批准工具调用` / `Claude 在问你问题` / `计划待批准`，正文 `{项目名} · {标题}`。点击跳终端复用现有逻辑。
  - 总开关只门控 `.show()`、map 始终更新；`seeded` 首扫只播种；`notified_pending.retain(...)` 防增长——全沿用现有模式。
- **计数**：macOS 菜单栏 / Windows 托盘的「待交互」计数口径从现有 `errored || status=="waiting"` 扩为 `errored || status=="waiting" || pending_review.is_some()`。

### 5. 前端（`app/src/views/Sticker.tsx` + 类型 + i18n）

- `LiveItem`/session 类型加 `pending_review?: 'approval'|'question'|'plan' | null`。
- **「待交互」tab 归类**：`status=='waiting' || !!pending_review`（不放宽则漏——pending 会话是 running）。各 tab 计数同步。
- **排序**：pending 组整体置顶（组内沿用「等最久优先」= `last_event_at` 升序），其后才是普通 waiting 组。
- **徽标 + 状态色**：卡片显醒目 pill —— `待批准` / `待回答` / `待批计划`，用区别于「运行中橙」「待交互黄」「错误红」的更跳颜色（如琥珀脉冲）；具体取色实现时定。
- **i18n**：`app/src/i18n` 中英各补三条子态标签 + 三条通知标题。

## 架构图

```
Claude Code 会话（回合中途阻塞）
  │ PermissionRequest / PreToolUse(AskUserQuestion|ExitPlanMode)
  ▼
meowo-reporter dispatch → set_pending_review(kind)  ──┐
  │ 下一个 PostToolUse/UserPromptSubmit/Stop/End   │  写 sessions.pending_review
  ▼ clear_pending_review                           ▼
                                          ~/.meowo/board.db
                                                    ▲ live_sessions() 回带 pending_review
meowo-app spawn_liveness_watch（5s 轮询，已存在）       │
  ├─ error > pending > waiting 三级判定 + 各自去重 map（notified / notified_pending / notified_waiting）
  ├─ 总开关只门控 .show()，map 始终更新，seeded 首扫播种，retain 防增长
  └─ 前端：待交互 tab 纳入 pending、置顶、醒目徽标、点击跳终端
```

## 错误处理

- dispatch 全程 best-effort：未知 tool_name / 缺 session 一律无操作（沿用 `dispatch` 现状，绝不冒泡）。
- `set/clear_pending_review` 失败 → 沿用 reporter 的 `?` 冒泡到 `let _ = run()` 被吞、exit 0。
- 安装侧解析失败/找不到二进制 → 沿用 `apply()` 的「绝不覆盖用户文件、原子写、先备份」。
- 旧库已存在该列时 `ADD COLUMN` 报错 → 沿用现有 migrate 的忽略策略。

## 测试计划

- **dispatch（meowo-reporter，in-memory store）**：
  - 进入：`PermissionRequest`（tool=Bash→approval、tool=ExitPlanMode→plan）、`PreToolUse(AskUserQuestion)`→question、`PreToolUse(ExitPlanMode)`→plan 各置对应 `pending_review`。
  - 清除：置 pending 后，`PostToolUse` / `UserPromptSubmit` / `Stop` / `SessionEnd` 各能清回 NULL。
- **store**：`set_pending_review` 刷新 last_event_at；`clear_pending_review` 置 NULL 不动 last_event_at；`live_sessions()` 回带 pending_review。
- **ccsetup（纯函数）**：
  - 空配置 → 8 条 spec 全挂；二次幂等无改动。
  - 用户已有 `PreToolUse:Bash` node 预检 → 原封不动，且 PreToolUse 下新增两条带 matcher 的 meowo-reporter。
  - 老库 5 个 `*` 事件 + 路径变更 → 仅更新路径、新增 PermissionRequest/PreToolUse 三条。
- **lib.rs 纯函数**：`pending_fingerprint` 覆盖 errored 优先、pending 出指纹（含 kind）、指纹随 last_event_at 变化。
- **复用**：`should_notify` 去重四情形已有测试。
- **前端**：类型加字段靠 `tsc` 保证；tab 归类/排序若有现成测试位则补，否则不新增重型测试。

## 非目标（YAGNI）

- 不做 `Notification`（`idle_prompt`/`permission_prompt`）hook —— 用更精确的 `PermissionRequest`+`PreToolUse` 三件套替代。
- 不新开独立 tab、不做声音/免打扰时段/延迟弹。
- 不区分「待批准」子开关——复用现有单一通知总开关。
- 不改 meowo-reporter 的网络/状态文件模型（仍无状态、单 SQLite）。
- 不为 `pending_review` 单列 `pending_review_at` 列——`last_event_at` 已足够承担排序与去重指纹。

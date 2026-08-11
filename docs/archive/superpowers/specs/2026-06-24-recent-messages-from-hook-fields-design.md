# 卡片「最近一条 AI / 用户消息」改用 CC hook 现成字段 — 设计

> 日期：2026-06-24
> 状态：已通过设计评审，待用户审阅 spec
> 前置：与「待审批/待权限子态」（`2026-06-24-pending-review-substate-design.md`）相互独立，可单独实现。错误检测仍走现有 transcript 路径（`analyze.rs` classify_error），本特性不动它。

## 背景与目标

Meowo 卡片展示的「最近一条 AI 正文 / 用户那句话」不如 moshi 准。**根因不是数据，是取数方式**：Meowo 持续猜 transcript + 把用户那句话挤进一个会被覆盖的字段；moshi 在 hook 触发时直接取 Claude Code 给的现成字段，存成互不覆盖的稳定字段。

目标：把「AI 最近一条完整回答」「用户最近一条」改为在对应 hook 时刻用 CC payload 的现成字段落库展示，transcript 解析降为兜底。

## 真实数据核实（非凭文档）

- **AI 侧**：
  - Meowo（`crates/meowo-store/src/analyze.rs:122-151`）每 5s 从 transcript 取「最新 text 块」，且每条 assistant 只取 `find_map` 命中的**第一个** text 块。
  - moshi（`index.ts:682`）主源是 `input.last_assistant_message`（CC 在 Stop 时给的完整最终答复），transcript 解析仅兜底，且兜底时 `.filter(text).map(text).join(" ")` **拼接所有** text 块（`index.ts:178-184`）。
  - 实证：抓取本会话进行中的一轮，Meowo 取到的是发工具前的**开场白**「切到这个。先按你的规矩…」；moshi 存的 `lastEventTitle` 是上一轮 Stop 的**完整答复**「调研完成。结论比表面更微妙…」。即 Meowo 回合中途显示半句中间话。
- **用户侧**：
  - Meowo（`crates/meowo-store/src/store.rs:303` `on_user_prompt`）把用户那句话截到 60 字写进 `current_activity`，但该字段会被下一个 Bash 工具的 `「› 命令」` **覆盖**（`crates/meowo-reporter/src/dispatch.rs:39-42`）。回合一开跑工具，「人说的话」就被命令盖掉。
  - moshi 用独立稳定的 `lastUserPrompt`，不被工具活动覆盖。
- **CC hook 字段**（待落地前以真实 payload 复核，见「诚实保留」）：
  - `Stop` hook payload 含本回合最终助手消息正文（moshi 实测字段名 `last_assistant_message`；官方文档另称 `assistant_message`——两者存疑）。
  - `UserPromptSubmit` 的用户文本在 `prompt`（Meowo 已在读）。

## 设计决策（已与用户确认）

1. **两个独立稳定字段**：`sessions` 表加 `last_ai_text TEXT`、`last_user_text TEXT`，互不覆盖，分别在 Stop / UserPromptSubmit 时刻落定。
2. **主源 = hook payload**：`last_ai_text` 取自 Stop payload 的助手消息字段；`last_user_text` 取自 `UserPromptSubmit.prompt`。
3. **transcript 解析降为兜底**：仅当 DB 字段为 NULL（首次导入的历史会话、或本机第一次 Stop 之前）时，app 用**修正后**的 transcript 提取兜底——取**最后**一条 assistant、**拼接所有** text 块（对齐 moshi）。
4. **`current_activity` 回归本职**：只表示「正在跑什么」（Bash 命令），不再兼任「用户那句话」的展示源。

## 组件与改动点

### 1. hook 解析（`crates/meowo-reporter/src/hook.rs`）

- `HookEvent` 加字段，**用 serde alias 同时接受两种可能的字段名**以对冲不确定性：
  ```rust
  #[serde(default, alias = "assistant_message")]
  pub last_assistant_message: Option<String>,
  ```
- `prompt` 已存在，无需改。

### 2. 数据层（`crates/meowo-store`）

- **schema**：`migrations.rs` 的 `SCHEMA` 在 `sessions` 加 `last_ai_text TEXT`、`last_user_text TEXT`。
- **旧库迁移**：`Store::migrate` 沿用现有 ALTER 补列模式各加一列（已存在则忽略）。
- **store.rs**：
  - `set_last_ai_text(sid, text)`：清洗（折叠空白）+ 截断 ~200 字符后 `UPDATE sessions SET last_ai_text=? WHERE id=?`。空串/全空白 → 不写（保留旧值）。**不动 `last_event_at`**——Stop / UserPromptSubmit 的兄弟调用已刷新它。
  - `set_last_user_text(sid, text)`：同上写 `last_user_text`，复用现有 `sanitize_prompt`（`store.rs:607`，剔图片标记+折叠空白）。
- **query.rs**：`LiveSession` 加 `last_ai_text: Option<String>`、`last_user_text: Option<String>`；`live_sessions()` SELECT 增这两列并回填。

### 3. 事件流转（`crates/meowo-reporter/src/dispatch.rs`）

- **`"Stop"` 分支**：在现有 `set_session_status(Waiting)` + `apply_title` 旁，若 `ev.last_assistant_message` 为 Some 则 `store.set_last_ai_text(sid, msg, now)`。为 None（旧 CC / 字段名不符）→ 不写，留给 app 的 transcript 兜底。
- **`"UserPromptSubmit"` 分支**：在现有 `on_user_prompt` 旁，`store.set_last_user_text(sid, prompt, now)`。
- **`on_user_prompt` 调整**（`store.rs:303`）：保留「无标题时用首句当标题」；**移除**「把 prompt 写进 `current_activity`」那段——`current_activity` 改由工具活动独占，「用户那句话」由 `last_user_text` 承担。（占位标题逻辑不变，避免影响未命名会话展示。）

### 4. 应用层与前端

- `app/src-tauri/src/lib.rs`：`LiveItem` 加 `last_ai_text` / `last_user_text` 透传。AI 展示优先级：**`last_ai_text`（DB，锚 Stop）→ 回退 live `preview`（transcript 兜底）**。`preview` 仍由 `analyze.rs` 计算（错误检测共用同一次解析，不变）。
- transcript 兜底修正（`analyze.rs` `fold_line` 的 assistant 分支）：把「`find_map` 取第一个 text 块」改为「**收集所有 text 块并以空格拼接**」，使兜底与主源、与 moshi 一致。（`last_text` 仍只保留最后一条 assistant。）
- 前端（`app/src/views/Sticker.tsx` + 类型 + i18n）：卡片分别显示「你：…」（`last_user_text`）与「AI：…」（`last_ai_text ?? preview`）。`current_activity` 仍单独显示「正在跑什么」。类型与 i18n 各补对应字段/标签。

## 架构图

```
UserPromptSubmit ──prompt──▶ dispatch ─ set_last_user_text ─┐
Stop ──last_assistant_message──▶ dispatch ─ set_last_ai_text ┤  写 sessions.{last_user_text,last_ai_text}
                                                            ▼
                                                  ~/.meowo/board.db
                                                            ▲ live_sessions() 回带两字段
meowo-app 文件监听 ─▶ LiveItem ─▶ 前端卡片
   AI:  last_ai_text(DB, 锚Stop) ?? preview(live transcript 兜底, 已修为拼接所有 text 块)
   你:  last_user_text(DB, 独立, 不被工具覆盖)
   活动: current_activity(只表示在跑什么)
```

## 错误处理

- Stop 无助手消息字段 / 字段名不符 → `last_ai_text` 不写，app 回退 live transcript 兜底，行为不劣于现状。
- `set_last_*` 写库失败 → 沿用 reporter `?` 冒泡被 `let _ = run()` 吞、exit 0。
- 首次导入的历史会话无 hook 触发 → 两字段 NULL，靠 transcript 兜底（AI 侧）；用户侧无兜底时卡片不显示「你：」行（可接受）。

## 测试计划

- **dispatch（in-memory store）**：`Stop{last_assistant_message}` → `last_ai_text` 落库；`Stop{assistant_message}`（alias）同样落库；`UserPromptSubmit{prompt}` → `last_user_text` 落库。
- **store**：`set_last_ai_text`/`set_last_user_text` 截断+折叠空白+空串不覆盖；`live_sessions()` 回带两字段。
- **analyze.rs 兜底修正**：单条 assistant 多 text 块 → 拼接（新增/改 `analyze_exposes_last_assistant_preview` 类用例）；多条 assistant → 取最后一条。
- **on_user_prompt 调整**：无标题会话仍用首句当标题；`current_activity` 不再被 prompt 写（断言）。
- **前端**：类型加字段靠 `tsc`；AI 优先取 `last_ai_text`、缺时回退 `preview`（若有测试位则补）。

## 非目标（YAGNI）

- 不存完整对话历史，只各留「最近一条」。
- 不改错误检测（仍走 live transcript classify_error）。
- 不动 context% / 用量。
- 不做卡片内多条消息滚动/展开。

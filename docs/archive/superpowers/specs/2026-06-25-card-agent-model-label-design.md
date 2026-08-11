# 会话卡片标注 agent 类型 + 模型 — 设计

> 日期：2026-06-25
> 状态：已通过设计评审，待用户审阅 spec
> 前置：独立特性，不依赖其它进行中改动。复用 statusline → session_context 现有数据通道（与 `context_pct` 同源）。

## 背景与目标

会话卡片目前不显示「这个会话跑的是哪个 AI、哪个模型」。用户希望在卡片上**标明 agent 类型与模型**，并为后续接入其它 AI（呼应账号页「为多 AI 做准备」）预留结构。

- **agent 类型**：指会话归属的 AI 工具/提供方。当前全部是 **Claude Code**（meowo-reporter 本就是 CC 的 hook）；现在显示是前瞻性恒定标注，未来真接入 Codex/Gemini 等时再按数据来源区分。
- **模型**：会话当前使用的模型展示名（如 `Opus` / `Sonnet`）。

## 真实数据核实（非凭文档）

- **模型数据已现成、只是没存**：Claude Code 每次渲染状态栏会把会话 JSON 通过 stdin 传给 statusline 命令，负载含 `model.display_name`（如 `"Opus"`）。`crates/meowo-reporter/src/statusline.rs`：
  - `minimal_line`（行 29-48）已经在读 `v["model"]["display_name"]`，仅用于自渲染终端状态行字符串。
  - `record`（行 8-25）只把 `context_window.used_percentage` / `context_window_size` 写进 `session_context` 表，**没存 model**。
  - 测试样例证实字段路径：`{"model":{"display_name":"Opus"},...}`（statusline.rs:84）。
- **agent 类型无现成来源、也无需后端**：全代码库无 model/agent 持久化字段；hook 负载（`hook.rs` HookEvent）不带 model/agent。当前 agent 恒为 Claude Code，做成前端常量即可，不引后端字段。
- **承载位置**：`session_context` 表已按 `cc_session_id` 主键 upsert、随 statusline 刷新（`migrations.rs:61-66`），与 model 的生命周期一致，是 model 的天然归宿（优于在 `sessions` 加列）。

## 设计决策（已与用户确认）

1. **agent 类型 = 前端常量 "Claude Code"**，不加后端字段（YAGNI）。卡片用一个 **AgentMark 图标**表达，`title`/`aria-label` 给 "Claude Code" → hover 即「标明」agent 类型；未来按 provider 换图标。
2. **模型存进 `session_context.model`**（statusline 驱动，一次 upsert 与 context 一起写），随 `live_sessions` 带出到前端。
3. **呈现**：放在卡片元信息行 `.stk-line2` 末尾、右对齐 —— `<AgentMark/> + 模型胶囊`。`model` 为空时只显示 agent 图标（statusline 尚未到达），到达后出现模型胶囊。
4. **范围**：连接中 / 已断开 / 归档会话只要有 `model` 值就显示（展示最近已知模型，对查看历史有意义）。

## 组件与改动点

### 1. 数据层（`crates/meowo-store`）

- **schema**（`migrations.rs`）：`session_context` 表 `CREATE TABLE` 加 `model TEXT`。
- **旧库迁移**（`store.rs` `init` 的 `ALTERS`，行 53-61）：加一条 `ALTER TABLE session_context ADD COLUMN model TEXT`（已存在则忽略 "duplicate column name"）；`USER_VERSION` 3 → 4（注释补 v4 说明）。
- **store.rs `set_session_context`**（行 121-138）：增形参 `model: Option<&str>`；upsert 列加 `model`，`ON CONFLICT DO UPDATE SET model = COALESCE(excluded.model, model)`（模型缺失不覆盖已有值，与 used_pct/window_size 一致）。
- **query.rs**：`LiveSession` 加 `model: Option<String>`（行 28-55 结构体）；`live_sessions()` SELECT 加 `sc.model`（行 205），row 解析与回填新增一列（行 235-241、249-274）。

### 2. meowo-reporter（`crates/meowo-reporter/src/statusline.rs`）

- `record`：解析 `v["model"]["display_name"]`（沿用 `minimal_line` 的取法），把 `Option<&str>` 传给 `set_session_context`。原 context 解析不变。

### 3. 应用层（`app/src-tauri`）

- **无需改动**：`lib.rs` 的 `LiveItem` 用 `#[serde(flatten)] inner: LiveSession`（lib.rs:97-99），`LiveSession` 新增的 `model` 会随 flatten 自动输出到前端（与 `context_pct` 同机制）。

### 4. 前端（`app/src`）

- **api.ts**：`LiveSession` 加 `model: string | null`（行 65-96 类型）。
- **Sticker.tsx**：`.stk-line2`（行 754-757，现为 `ConnBadge + .stk-repo`）末尾加右对齐组：
  - `AgentMark`：小号 spark SVG（accent 色），包一层 `<span className="stk-agent" title={t.sticker.agentClaudeCode} aria-label={...}>`。纯展示、随卡片点击冒泡（点哪都开终端，符合现状）。
  - 模型胶囊：`{l.model && <span className="stk-model">{l.model}</span>}`，中性低调样式。
- **styles.css**：`.stk-line2` 改 flex（`align-items:center; gap`），让 agent+模型组 `margin-left:auto` 右对齐、`.stk-repo` 可截断（`min-width:0; text-overflow:ellipsis`）；新增 `.stk-agent`（图标尺寸/色）、`.stk-model`（小号、faint 底、dim 文字、圆角，不抢状态色）。
- **i18n**：`sticker` 加 `agentClaudeCode: "Claude Code"`（zh/en 一致，供 hover/aria）。

## 架构图

```
CC statusline JSON ── model.display_name ──▶ meowo-reporter statusline::record
                                                   │ set_session_context(..., model)
                                                   ▼
                                    session_context.model (新列)  ~/.meowo/board.db
                                                   ▲ live_sessions() LEFT JOIN sc.model
meowo-app 文件监听 ─▶ LiveSession.model ─▶ 前端卡片 .stk-line2（右对齐）
        ◆(AgentMark, title=Claude Code)  +  [模型胶囊 Opus]
agent 类型：前端常量（图标 + hover 文案），无后端字段
```

## 错误处理

- statusline 负载无 `model` / 解析失败 → `record` 传 `None`，COALESCE 保留旧值；前端 `model` 为 null → 只显示 agent 图标，不显示胶囊。
- 旧库 ALTER 已存在列 → 忽略 "duplicate column name"（与现有迁移一致）；迁移遇 BUSY/IO → 不 bump 版本，下次 open 重试（现有机制）。
- 写库失败 → 沿用 reporter `?` 冒泡被吞、exit 0，绝不影响 statusline 透传。

## 测试计划

- **Rust / statusline**：`record` 解析含 `model` 的负载并写库；回读 `live_sessions()` 得到该 model；负载缺 model 时不覆盖已有 model（COALESCE）。
- **Rust / store**：`set_session_context` 带 model 的 upsert；旧库迁移加 `model` 列不丢原有 context 数据。
- **Rust / query**：`live_sessions()` 带出 `model` 字段。
- **前端**：`LiveSession` 加字段靠 `tsc`；`Sticker` 渲染 model 有/无两态（沿用现有 vitest mock 模式，断言模型胶囊出现/缺席、agent 图标恒在）。
- 全量 `cargo test` + `vitest run` + `tsc --noEmit` 绿。

## 验证

本地 `bun run tauri dev` 起应用，真实会话卡片目测：`◆ Opus` 右对齐于元信息行、hover 图标显示 "Claude Code"、深/浅主题正常、仓库名长时截断不挤压。

## 非目标（YAGNI）

- 不加后端 agent/provider 字段（当前恒 Claude Code，前端常量即可）。
- 不做模型历史/切换记录，只显示最近已知模型。
- 不动 context% / 用量 / 错误检测等既有数据。
- 不做真 Claude 官方 logo（用通用 spark 标记，规避商标且便于多 provider 换图标）。

# Meowo 设计文档

> 工作名 `Meowo`，可改。日期：2026-06-03。

## 1. 背景与目标

重度并行使用 Claude Code 时，常出现：

- 任务开太多，做着做着忘了某个会话在干什么。
- 不小心关掉终端，丢失某项目的进度上下文。

**目标**：一个专供 Claude Code 使用者的桌面任务看板，在 Claude Code 运行时**自动**把进度上报到看板，并提供随时一瞥的桌面贴纸，做到「关了终端 / 关了 App 也不丢进度」。

**非目标（YAGNI，本期不做）**：手机端、多端同步、云存储、多用户协作。本期只交付单机本地版。

## 2. 产品形态

Tauri v2 桌面应用，三类界面：

1. **总览**：所有项目一屏，每项目一张卡，显示状态、活跃会话数、未完成任务数。
2. **项目看板**：点进项目 → 待办 / 进行中 / 完成 三列，卡片 = 任务，可拖拽。
3. **桌面贴纸**：把「活跃会话」或「关注的项目」钉成悬浮窗——透明、置顶、可拖动、密度可切换。

### 桌面贴纸密度

三档密度，每张贴纸可独立切换，亦支持「平时极简、悬停展开变丰富」：

- **极简**：项目/会话名 + 一句话当前动作 + 状态点。
- **进度卡**：增加进度条 + 步骤计数。进度 = 已完成 todo 数 / 总 todo 数（无 todo 时进度条隐藏）。
- **信息丰富**：状态 + 当前事项 + 迷你 todo 勾选 + 最近一条日志。

### 布局示意

```
┌─ 总览 ───────────────────────────┐     ┌─ 桌面贴纸（悬浮在桌面角落）─┐
│ ▣ reverse-bot-rs   ● 1活跃 3待办 │     │ ● clawmo-relay            │
│ ▣ clawmo-relay     ● 1活跃 2进行 │     │   Phase2 集成推送 ▓▓▓░ 64% │
│ ▣ jsgraph          ○ 等待输入    │     └───────────────────────────┘
└──────────────────────────────────┘
        │ 点进某项目
        ▼
┌─ clawmo-relay 看板 ──────────────────────────┐
│  待办          进行中            完成          │
│ ┌────┐      ┌──────────┐     ┌────────┐      │
│ │... │      │集成推送通道│     │Ed25519 │      │
│ └────┘      │☑解析 ☑通道 │     │认证    │      │
│             │☐ 写测试   │     └────────┘      │
│             │● 会话进行中│                     │
│             └──────────┘                      │
└───────────────────────────────────────────────┘
```

## 3. 架构与数据流（共享 SQLite 方案）

```
Claude Code 会话
  └─(hooks, 全局 ~/.claude/settings.json)
       SessionStart / UserPromptSubmit / PostToolUse / Stop / SessionEnd
          └─► meowo-reporter (Rust 单文件 exe，解析 hook 的 stdin JSON)
                 └─► SQLite (~/.meowo/board.db, WAL 模式)
                        ▲
                        └── Tauri 后端 read + notify 文件监听 → 实时刷新 UI
```

**为什么选共享 SQLite（而非 App 内嵌服务 / 独立守护进程）**：

- App 没开时事件照样被记录，开了自动补上——直接满足「不丢进度」硬需求。
- 无网络、无常驻守护进程，活动部件最少。
- 全 Rust（上报器 + Tauri 后端），契合技术栈。
- 实时性为「事件级」（每次工具调用 / 回合结束刷新），对本场景足够。

## 4. 核心组件

### 4.1 meowo-reporter（Rust CLI）

- 被 hooks 调用，从 stdin 读取 Claude Code 传入的 hook JSON，upsert 进 SQLite。
- 单文件静态 exe，轻、快、零运行时依赖。
- **永不阻塞 Claude Code**：任何错误都静默处理、以退出码 0 返回，绝不拖慢会话。
- 库不存在时自动建库 + 建表。

### 4.2 SQLite Schema

```
projects(id, root_path, name, created_at, updated_at)
sessions(id, project_id, cc_session_id, status, started_at, last_event_at, ended_at)
tasks(id, project_id, session_id, title, column, column_locked, created_at, updated_at)
todos(id, task_id, content, status, order_idx)
events(id, session_id, kind, payload, created_at)
```

- `status`（session）：`running` / `waiting` / `ended` / `stale`。
- `column`（task）：`todo` / `doing` / `done`。
- `column_locked`：手动拖拽后置 true，自动推导不再覆盖。

### 4.3 Tauri Rust core

- 读库；用 `notify` crate 监听 db 变更（去抖）→ 通过 Tauri event 推到前端。
- 管理贴纸窗口：透明背景、置顶、无边框、可拖动、记忆位置。

### 4.4 React + Vite 前端（bun）

- 三套视图：总览 / 项目看板（拖拽）/ 贴纸。
- 订阅 Tauri event 实时刷新。

## 5. 事件 → 状态映射（全自动）

| Hook | 行为 |
|------|------|
| **SessionStart** | 按 **cwd 的 git 根**（无 git 则用 cwd）归到项目；新建一条活跃会话 = 一个任务卡，标题暂为「(未命名会话)」占位。 |
| **UserPromptSubmit** | 首条 prompt 自动替换占位标题（截断 ≤60 字，可后改）；刷新「当前在做什么」。 |
| **PostToolUse(TodoWrite)** | 把 todo 列表同步成任务子清单；`in_progress` 那条 = 当前动作。 |
| **PostToolUse(Bash)** | 当前动作显示 `› <命令>`。 |
| **Stop** | 会话转「等待输入」。 |
| **SessionEnd** | 会话标记结束；任务**不**自动判完成，留原列等续，避免误判。 |

## 6. 关键设计规则与默认值

- **列归属推导**：无 todo = 待办；有 `in_progress` = 进行中；全 `completed` = 完成。**手动拖拽锁定**（`column_locked`）后不再被自动覆盖。
- **活跃 vs 结束**：优先用 SessionEnd；终端被强杀收不到时，**10 分钟无事件** → 标记 `stale`（可配置）。
- **项目分组**：git 根目录；同目录多终端 = 多会话卡，归同一项目。
- **会话↔任务**：1 会话 = 1 任务卡。

## 7. 测试与错误处理

- **meowo-reporter**：hook JSON 解析、并发写 SQLite（WAL）、自动建库——单元测试覆盖。出错静默退出码 0。
- **Tauri core**：文件监听去抖、库读取容错（库损坏 / 缺失时降级提示而非崩溃）。
- **前端**：拖拽改列写回库、乐观更新 + 失败回滚。

## 8. 技术栈

- 桌面框架：Tauri v2。
- 后端 / 上报器：Rust（SQLite 用 `rusqlite`，文件监听用 `notify`，错误类型用 `thiserror`）。
- 前端：React + Vite + TypeScript，包管理 bun。
- 测试：Rust `cargo test`，前端 `vitest`。

## 9. 交付范围

本 spec 为第一个可独立交付子项目：单机本地版，含 meowo-reporter + SQLite + Tauri App（总览 / 项目看板 / 桌面贴纸）。手机端与多端同步留待后续独立立项。

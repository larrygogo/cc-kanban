# 会话对话窗口（Chat Window）— 设计

> 日期：2026-07-03
> 状态：**已废弃——未按本设计实施**（2026-08-11 标注）。本文选定的第一期方案
> （每轮 spawn `claude -p --resume` headless 子进程 + 分状态混合）**从未落地**；
> 实际实现走的是本文列为"二期备选"的 **PTY 托管路线**（`pty.rs` + `managed_terminal.rs` +
> xterm.js，五家 agent 均进对话窗）。理解对话窗架构请读
> `docs/architecture/overview.md` 的「对话窗架构」一节。本文仅存档其调研结论（§关键技术事实）。
> 原状态：草稿——待用户批准（设计呈现两轮，用户离席期间按调研与实测结论整理成文，未 commit）
> 前置：无硬依赖。与多 provider 接口化路线（`2026-06-30-provider-key-consolidation.md`）方向一致：本特性第一期 claude-only，经 `TranscriptSpec`/`Agent` 既有接口留 codex/kimi 入口。

## 背景与目标

看板卡片只显示会话摘要（标题、状态、最近一条消息）。用户希望：点开一个会话能看到**完整对话流**（用户消息、AI 正文、bash/websearch 等全部工具调用与结果），并能**直接在 UI 里和 AI 继续对话**，与 CLI 双向同步——CLI 侧的进展实时出现在 UI，UI 发的消息也进入同一个会话。

## 关键技术事实（实测 + 官方文档核实，2026-07-03）

以下均在本机（claude v2.1.199, Windows 11）实测验证，非仅凭文档：

1. **transcript jsonl 是完整对话的实时事实源**：`~/.claude/projects/<slug>/<session-id>.jsonl` 按消息级逐行追加（user 行 / assistant 行含 text 与 tool_use 块 / user 行含 tool_result 块）。粒度为消息级而非 token 级。
2. **`claude -p --resume <id>` 续聊：session ID 不变、上下文连续、追加写入原 transcript 文件**（实测三连：单轮拿 ID → resume 问前文 → 检查同一 jsonl 增长）。
3. **stream-json 双向长驻可行**：`claude -p --input-format stream-json --output-format stream-json --verbose` 进程跨轮存活、多轮同 session、assistant 消息实时流出；与 `--resume` 组合同样保持 session ID。（早前一轮文档调研称"不能长驻"，被实测推翻——VS Code 官方插件即此架构：GUI 前端 + headless 子进程。）
4. **headless 会话照常触发 hooks**：实测两个探针会话经 meowo-reporter 正常写入 board.db（status=ended）。⇒ UI 续聊时看板状态同步免费获得。
5. **无官方机制向"普通启动的、正在终端运行的"交互式会话注入用户消息**：hooks 只能在会话自身活动时刻注入 context（非用户回合）；IDE 集成 WebSocket 只做上下文工具不做消息提交；remote control 走 CLI 内建的 Anthropic 云端通道，无本地入口。
6. **Channels（v2.1.80+，research preview）是唯一官方"注入运行中会话"机制**：channel = MCP server，会话以 `claude --channels plugin:<name>` 启动后可接收推送消息并经 channel `reply` 工具回话，支持权限中继。官方 demo 插件 fakechat（localhost 聊天 UI ↔ 运行中会话）即本特性终态形态。硬约束：①必须启动时带 flag，覆盖不了随手 `claude` 起的会话；②preview 期 `--channels` 只认 Anthropic 官方 allowlist，自建 channel 需 `--dangerously-load-development-channels`，不适合面向公众发布；③官方声明 flag 语法与协议契约可能变。

## 方案取舍

| 方案 | 说明 | 结论 |
|------|------|------|
| 只读查看器 | 仅展示 + 跳终端 | 不满足"UI 上对话"核心诉求，弃 |
| **分状态混合（选定，第一期）** | 已断开/空闲 → UI 直接 resume 续聊（官方路径、session 不变）；运行中/待交互 → 实时只读镜像 + 一键跳终端 | 全程官方支持路径，零启动要求，开箱即用 |
| 终端按键注入 | 聚焦终端 + SendInput 模拟键盘触达运行中会话 | hacky（抢焦点/输入法/时序），被 Channels 取代，弃 |
| Channels 真双向 | 自建 Meowo channel 插件，运行中会话真双向 + 权限中继 | **二期**：等 preview 毕业或进 allowlist；架构本期预留插槽 |

## 第一期设计

### 窗口形态

- 独立**会话详情窗口**，从看板卡片点开。入口 = 卡片悬停操作按钮排新增「对话」图标按钮（与星标/便签/改名/归档同排）；**点击卡片本体的既有行为（跳终端/打开按钮模式）不变**。
- 沿用 updater/settings 多窗口先例：`WebviewWindowBuilder` + label 路由（`main.tsx` 按 label 分发页面）。label 固定 `"chat"`，**单例**：已开时点其他卡片则切换会话并聚焦。
- 尺寸可调、记忆上次尺寸位置；主题/密度/不透明度跟随看板设置。

### 组件与改动点

#### 1. meowo-store：消息级 transcript 解析（新模块 `chat.rs`）

- `ChatItem` 枚举（serde 序列化直达前端）：
  - `UserText { text }`（过滤图片/粘贴占位标记，复用 sanitize 思路）
  - `AssistantText { text }`（同一 assistant 行内**拼接全部 text 块**）
  - `ToolUse { name, input_summary }`（input 按工具类型提取要点：Bash→command、WebSearch→query 等；未知工具→紧凑 JSON 截断）
  - `ToolResult { summary, is_error }`（截断预览，保留展开用的全文可后续加）
  - `Meta { kind }`（compact/clear 等边界标记，第一期识别不细化）
- 每项带 `uuid`、`timestamp`。`isSidechain: true` 的行跳过（子代理内部流不进主线；Task 工具本身的 tool_use/tool_result 在主线正常显示）。
- 增量解析：`ChatParser` 记文件字节偏移，新行只 fold 新增部分——与既有 `TranscriptCache` 的 fold 模式同构。畸形行静默跳过。
- 挂接 `TranscriptSpec`：trait 增 `chat_parser()` 入口，claude 返回实现，codex/kimi 返回 None（前端据此隐藏对话入口）——与既定多 provider 路线一致。

#### 2. Tauri 后端（`app/src-tauri`）

- **`open_chat_window(session_id)`**：建/聚焦单例窗口，URL 带 session_id。
- **`get_chat_history(session_id)`**：经 `TranscriptSpec::resolve_transcript_path` 定位文件 → 全量解析 → 返回 `Vec<ChatItem>` + 当前字节偏移。MB 级 jsonl Rust 解析为毫秒级，不分页；渲染端控制显示窗口。
- **transcript watcher**：chat 窗口打开期间对该 transcript 的**父目录**起 notify 监听（Windows 单文件监听不可靠），按文件名过滤、去抖 ~150ms，增量解析新行 → `emit("chat-items", …)` 推给窗口。切换会话/关窗即注销。
- **`send_chat_message(session_id, text)`**：
  1. 校验会话 provider=claude 且**未连接**——复用既有 resume 守卫语义（status 非 running/waiting，且 `pid_alive_agent_quick` 判活失败；`lib.rs` 现有 `session_connected`/resume 前奏同款），闭合「stale 但 CLI 进程还活着」的双开风险；UI 层输入区随同一判定禁用/启用；
  2. 从 DB 取 cwd，spawn `claude -p --resume <id> --input-format stream-json --output-format stream-json --verbose`（cwd=会话原目录），**stdin 写入单条 user 消息后关闭**——每轮一进程：避开 Windows 命令行长度/转义问题，轮间无常驻进程、与终端侧 resume 的竞态窗口最小；
  3. stdout 流式读取：`stream_event` 驱动"AI 正在回复"活动指示（token 级预览为增强项），`result` 事件即轮次结束；
  4. 同一会话同时只允许一条 in-flight（后端持 `HashMap<session_id, Child>` 守卫）；窗口关闭时终止 in-flight 子进程。
- **渲染源单一化**：对话内容**只以 transcript watcher 为渲染源**（CLI 侧与 UI 侧的消息走同一条管道，天然无双源分叉）；send 的 stdout 仅用于临时活动指示、错误捕获与完成信号，transcript 行到达即替换临时态。

#### 3. 前端（`app/src`）

- `ChatWindow` 页面（label="chat" 路由）：
  - 消息列表：默认渲染最近 ~100 条 +「加载更早」，自动滚底（用户上翻时暂停跟随）；
  - 工具调用渲染为**可折叠卡片**（图标 + 工具名 + input 摘要，展开见 result 预览）；用户/AI 消息为气泡/段落，md 简渲染可后续加；
  - 底部输入区按会话状态切换：`ended/stale` → 输入框 + 发送；`running/waiting` → 只读提示 +「跳转到终端」按钮（复用现有 focus/resume 通道）；状态经既有 board.db 监听实时切换；
  - 顶栏：会话标题、项目名、状态点，与卡片视觉语言一致。
- i18n：新增 `chat.*` 文案条目（中英）。

### 数据流

```
CLI 侧输入/输出 ──┐
                  ├──▶ transcript.jsonl ──▶ watcher(增量解析) ──▶ ChatWindow 渲染
UI send ──▶ claude -p --resume(子进程) ──┘                          ▲
              │ stdout: 活动指示/错误/结束 ────────────────────────────┘
              └ hooks 照常触发 ──▶ board.db ──▶ 卡片状态 & 输入区状态切换
```

### 错误处理

- transcript 缺失/无法解析 → 窗口空态提示（会话过旧/文件被清理）。
- send 子进程失败（PATH 无 claude、cwd 已删除、exit≠0、stderr 有货）→ 对话流内插入错误条 + 可重试。
- 竞态：发送前二次校验状态；若发送期间终端侧恢复了该会话（双开），以失败/提示兜底，不做强锁。
- 已知限制（如实展示，不隐藏）：headless 续聊沿用 settings.json 权限，需要交互批准的工具在 `-p` 模式被禁用/拒绝——AI 会在回复中自然体现；权限中继属二期 Channels 能力。

### 测试

- Rust 单测：`chat.rs` 解析（文本/多 text 块拼接/tool_use-result 配对/sidechain 跳过/畸形行/增量偏移续解）；send 状态门控与 in-flight 单飞。
- 前端 vitest：ChatItem 渲染分支、输入区状态机。
- 手动验收：真实会话续聊（session 不变、看板同步）、CLI 侧输出实时镜像、终端/UI 竞态提示、大 transcript 首开性能。

## 二期路线：运行中会话真双向（预留，不在本期）

行业规律（VS Code 插件 / remote control / paseo 等同类调研结论）：不存在 attach「别人启动的裸交互会话」的路径，真双向必须占据会话进程的出生链路。两条候选，可并行评估：

- **Channels 插件**：自建 `Meowo` channel（MCP server，参照官方 fakechat/telegram 源码），UI ↔ 运行中会话真双向 + 权限中继（在 UI 里批准权限提示）。触发条件：Channels 走出 research preview / 进入 allowlist（当前自建插件需 `--dangerously-load-development-channels`，不宜面向公众发布）。
- **PTY wrapper（备选）**：`cck` 包装命令在用户终端与 claude 之间加一层伪终端（Windows ConPTY / Unix pty），终端照常用、UI 输入写入同一 PTY。渐进采纳点：看板「恢复已断开会话」的终端本来就由 app 拉起，该路径可先行替换为 wrapper——「由看板恢复的会话」即获终端+UI 真双向，不依赖 preview 协议；用户裸起的会话再由 Channels 覆盖。代价：ConPTY 转发工程量、结构化内容仍靠 transcript。
- 本期预留：发送路径在前后端各收敛为单点（`send_chat_message` / 输入区状态机），二期无论走哪条只加分支不动骨架。

## 待用户确认的假设（按推荐值先行）

1. 窗口形态 = 会话详情窗口（单例、从卡片点开），非全局聊天客户端。
2. 双向策略 = 分状态混合；终端按键注入不做。
3. Channels 放二期（若希望第一期就上 dev-flag 版本，请明示）。
4. provider 范围 = 第一期 claude-only，codex/kimi 留接口入口。
5. waiting（CLI 正等回复）状态：UI 引导跳终端，不代答。

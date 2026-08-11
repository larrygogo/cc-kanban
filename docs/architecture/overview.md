# Meowo 架构总览

> 本文是理解 Meowo 当前实现的入口文档（基线 0.5.12，2026-08-11）。
> 专题文档：[agent-plugin.md](agent-plugin.md)（插件架构与迁移史）、
> [refactor-roadmap.md](refactor-roadmap.md)（协议收敛路线）、
> [consistency-backlog.md](consistency-backlog.md)(已知不一致清单)。
> 线程纪律、平台验证、屏幕检测规则维护三条硬纪律见 `app/src-tauri/CLAUDE.md`。

## 产品形态

Meowo 把各 AI CLI（Claude Code / Codex / Kimi / Gemini / OpenCode）的会话事件收集到本地
SQLite，再用桌面贴纸实时展示，并提供原生对话窗直接驱动会话。三个进程角色：

- **meowo-reporter**：hook 子进程二进制。各 CLI 的 hook 被接线成调用 reporter，由它写库、
  上报 statusline、写终端标签标题。
- **meowo-app**（Tauri GUI）：贴纸看板、对话窗、托管 PTY、审批 broker、设置/账号页。
- **各 AI CLI 本体**：Meowo 不持有它们的会话逻辑，只观察（hook/transcript）与驱动（PTY）。

## Crate 分层

```
meowo-protocol   ← 跨 crate/跨语言 DTO 与纯编解码（零 tauri/db/agent 依赖）
   ↑
meowo-store      ← SQLite 持久层（不认识任何具体 agent）
meowo-agent      ← Agent 插件层（纯声明+纯函数，IO 走注入端口）
   ↑
meowo-reporter   ← hook 子进程
meowo-app        ← Tauri GUI 宿主（workspace 根，app/src-tauri/src）
```

依赖方向单向无环。全部 Rust 住在 `app/src-tauri` 下。

### meowo-protocol：契约的唯一来源

- `ipc.rs`：GUI 边界 DTO。`#[cfg_attr(test, derive(ts_rs::TS))]` 在 `cargo test -p
  meowo-protocol` 时导出 TypeScript 到 `app/src/generated/contracts/`；CI 用
  `git diff --exit-code` 保证生成物不漂移（`.github/workflows/ci.yml`）。
  DTO 统一 `rename_all = "camelCase"`。
- `broker.rs`：app↔reporter 的本地 broker 线协议。v2 为 `MWO2` 魔数 + 长度帧 JSON
  envelope，v1 按行文本，`read_handshake()` 是单一解码入口，两者统一成 `BrokerRequest`。
  协议版本改动有绊线测试把关。发现文件为 `approval-broker.json`。

**纪律：凡过 Tauri IPC 的结构体应走 protocol DTO + ts-rs 生成，而非裸 `Serialize` +
前端手写类型。**（现状尚有一半类型未迁，见 consistency-backlog D6。）

### meowo-agent：插件能力槽

「meowo 提供能力，agent 声明自己用哪些」。原则与迁移史见 [agent-plugin.md](agent-plugin.md)；
接入新 agent 的实操指南在 `crates/meowo-agent/src/plugins/mod.rs` 的 rustdoc（3 步接入、
必填四项、能力槽逐条说明、安全默认论证）。要点：

- 身份是字符串 `AgentId`，未知 id 绝不降级；`registry.rs::resolve()` 是身份解析唯一入口。
- 必填仅 4 项（`id`/`display_name`/`variants`/`process_names`），其余 30+ 能力槽全部
  带最保守默认值（`None`/空表/`false`），不支持就不声明，框架降级。
- IO 通过宿主注入的端口（`HttpPort`/`KeychainPort`）完成；`fsutil::write_atomic` 刻意
  不做成端口（纯 std，可直接测）。
- 前端零 agent 逻辑分支：能力经 `list_agents()` / `agent_chat_ui(provider, cwd)` 下发，
  图标与品牌色是前端资产（`providers.tsx`）不是下发数据。守卫测试：前端
  `app/src/architecture.test.ts`，宿主 `host_code_does_not_branch_on_agent_identity`。

### meowo-store

只剩 `error/migrations/models/query/store` 五个模块，provider 列存原样字符串。
判定「会话此刻还连着」需要进程表，只有 app 层做得到——store 只供料不判定
（`LiveCandidate` 的注释即此约定）。

## meowo-app 子系统

| 子系统 | 模块 | 说明 |
|---|---|---|
| PTY broker | `pty.rs` | 托管 PTY 生命周期、attach TCP 服务、审批 broker、交互式提问、屏幕检测线程、claim/rebind |
| 终端集成 | `terminal.rs`、`wezterm.rs` | 定位/聚焦外部终端标签（UIA/Win32、AppleScript）、resume 计划、spawn |
| 屏幕检测 | `detect.rs` | working/idle/blocked 引擎，规则由各插件的 `screen_rules()` 声明；取证走 `screen_detect_explain` |
| 会话查询 | `session_query.rs`、`session_command.rs` | 看板分页/角标/查询服务与写命令 |
| 对话窗数据 | `chat.rs` | transcript 增量 → `ChatHistoryDto`；`ChatReducer` 在前端（`chat/reducer.ts`） |
| 后台线程 | `watch.rs` | board-changed 合流、DB 监听、存活轮询、通知去重 |
| 登录/安装 | `install.rs` | 登录 operationId 状态机（epoch+operation_id 双匹配收尾，唯一出口 `login-done` 事件） |
| 多账号 | `profile/` | per-agent profile、跨账号会话文件迁移 |
| 网络 | `proxy.rs`、`relay.rs` | 代理声明解析、API 中转注入 |
| 后台会话 | `bgpty.rs` | Claude FleetView 派生的后台会话接入 |

### 对话窗架构（PTY 路线）

> 历史注意：`docs/archive/superpowers/specs/2026-07-03-chat-window-design.md` 选定的
> 「每轮 spawn `claude -p --resume`」方案**从未实施**，实际落地的是它当年列为二期备选的
> PTY 托管路线。理解对话窗以本节与代码为准。

会话由 Meowo 的 PTY 持有（ConPTY / openpty），对话窗与外部终端只是同一 PTY 的两种视图
（attach 客户端镜像）。数据流：

```
CLI transcript(JSONL) → 插件 TranscriptSpec::parse_transcript_line()
  → Vec<TranscriptEvent>（领域事件，agent/transcript.rs）
  → From<TranscriptEvent> for ChatItem（边界适配，改名即契约）
  → ChatDelta / read_chat_delta()（按 offset+mtime 增量）
  → chat.rs::load_chat_history → ChatHistoryDto（ts-rs 契约）
  → 前端 chat/reducer.ts 纯函数归并
```

审批走 broker：reporter 的 PermissionRequest hook → `pty.rs` 审批队列 →
`ApprovalConsumer` 租约（对话窗注册消费；无消费者时先拉起对话窗、等不到租约立即 Pass，
绝不盲等）→ 决策经 `ApprovalDecision` 线协议回传，保留 agent 原生 permission_suggestions。

## 前端（app/src）

React 18 + Vite + TypeScript，无全局状态库（刻意，见 refactor-roadmap「不做」）。范式：

- **subscribe-first**：先订 `settings-changed` 再 fetch，`eventApplied` 防旧读覆盖新事件
  （参考实现 `appearance.ts`）。
- **统一事件订阅**：`hooks/useTauriEvent.ts`（handlerRef + disposed 竞态处理）。存量
  手写 `listen()` 正在迁移。
- **异步守卫**：state 供渲染 / ref 供同步判定的双写模式（`useLoginOperations` 母本）。
- **弹层菜单唯一实现**：`views/menu.tsx` 的 `useMenuPopup`（曾一式三份，已收敛）。
- **Tooltip 单例**：`Tooltip.tsx` 事件委托 `data-tip`。
- **确认对话框**：`confirm.tsx::appConfirm`（应用主题原生小窗）；系统 MessageBox 已弃用。
- **i18n**：zh 为基准，en 用 `Dict = typeof zh` 编译期对齐 + 运行时 key/arity 比对测试。
  产品名不翻译；数据库 sentinel（如未命名会话占位）不是 i18n 文案。
- **设计 token**：`styles.css:19-111`，「≥3 处复用才立档」；暗色默认，浅色
  `data-theme="light"` 覆盖；贴纸另有 `data-sticker-style="flat"` 正交风格轴。

## 测试与 CI 守卫（架构的可执行形式）

| 守卫 | 钉住什么 |
|---|---|
| `cargo test -p meowo-protocol` + CI diff | TS 契约与 Rust DTO 不漂移 |
| `app/src/architecture.test.ts` | 前端不按 agent 身份分支 |
| `host_code_does_not_branch_on_agent_identity` | 宿主代码同上（覆盖名单待扩，见 backlog D3） |
| `i18n.test.ts` | zh/en key 集合与函数 arity 一致 |
| broker `protocol_version_change_is_deliberate` | 协议版本改动是深思后的 |
| `providers.test.tsx` 等三件套 | 未知 agent 中性兜底，不顶 Claude 徽标 |
| clippy `--all-targets -D warnings` + macOS 矩阵 | `src/macos/**` 的唯一编译验证在 CI |

## 文档地图

- `docs/architecture/` —— 现行架构（本文 + agent-plugin + roadmap + backlog）。
- `docs/research/` —— 真机取证记录（活文档，被代码注释引用，勿移动）。
- `docs/macos-*.md` —— 发布 secrets 清单与验收清单（仍在用）。
- `docs/archive/superpowers/` —— 历史 specs/plans（已实现并被后续重构改写，仅供考古）。
- `app/src-tauri/CLAUDE.md` —— 三条硬纪律（线程/平台/检测规则）。
- `app/e2e/README.md` —— E2E 特制构建说明。

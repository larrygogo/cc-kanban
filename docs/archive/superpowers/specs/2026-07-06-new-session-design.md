# 从看板新建会话（New Session）— 设计

> 日期：2026-07-06
> 状态：**草稿——待用户 review**
> 前置：无硬编码依赖，但对 codex/kimi 的「入库」软依赖其 hooks 已安装（本期只检测+引导，不自动安装；真正的自动安装是独立的 `2026-07-03-provider-setup-design.md`）。

## 背景与目标

看板目前只能**发现/导入**已经存在的会话——一个会话必须先由 CLI 跑起来、触发 hook 上报，才会出现在看板上。用户希望能**从看板直接新建一个全新会话**：选一个工作目录 + 选一个 agent（claude / codex / kimi）→ app 打开一个终端并启动该 CLI → 新会话的卡片随后自动出现在看板。

这是「恢复会话」的对偶操作：恢复是对**已有** session 续跑 `claude --resume <id>`，新建是在**新目录**里裸起 `claude`（无 id）。二者共用同一套「在指定 cwd 打开终端并跑一条命令」的机制，只是命令不同。

## 关键技术事实（2026-07-03 探索核实，file:line 为实现锚点）

1. **终端 spawn 机制已完备且可直接复用**：`resume_session`（`app/src-tauri/src/lib.rs:1044-1160`，注册于 `lib.rs:2244-2272`）已实现全平台终端选择 + spawn：
   - Windows 终端选择 `lib.rs:1067-1074`：读 `settings.resume_terminal`，映射 `powershell`/`cmd`/`wezterm`/`wt`，含可用性回退。
   - 各分支：`wt` `lib.rs:1102-1115`（`wt -w 0 nt [-p profile] [-d cwd] argv`）、`powershell` `lib.rs:1079-1086`、`cmd` `lib.rs:1090-1097`、`wezterm` → `wezterm::resume`（`wezterm.rs:154-174`）。
   - 辅助：`shell_join_for_windows` `lib.rs:938-968`、`safe_cwd` `lib.rs:918-926`、`wt_default_profile` `lib.rs:905-916`。
   - macOS `lib.rs:1130-1154`：`macos::terminal::resume_session_mac`，AppleScript 模板 `term_script.rs:99-165`，终端类型 `resume_terminal_kind` `lib.rs:691-699`。
   - ⇒ 新建会话与之只差**启动命令**，可把这段抽成共享 helper。
2. **`session_id` 由 CLI 启动那刻生成，app 事先拿不到**：因此**无法**像 `resume_session` 那样预改 DB 行来立刻显示卡片（`prepare_resume` 的乐观 revive `lib.rs:997-1025` 依赖已知 `session_id`）。新卡片只能等真实 hook 上报后自然出现。
3. **会话入库唯一的实时路径是 hooks（meowo-reporter）**：`SessionStart` → `dispatch::create_session`（`meowo-reporter/src/dispatch.rs:155-168`）→ `store.start_session`（`meowo-store/src/store.rs:199-231`，插入占位 task，卡片由此产生）。任何带 cwd 的活动事件也会 `lookup_or_create`（`dispatch.rs:179-193`）兜底创建。
4. **出卡时机因 provider 而异**：claude/kimi 启动即 `SessionStart`（秒级出卡）；**codex 的 `SessionStart` 只在首个回合才 fire**（`lib.rs:126-128`），即 codex 卡片要等用户发第一条消息才出现。
5. **hooks 自动安装目前仅覆盖 claude**：`ccsetup::apply`（`app/src-tauri/src/ccsetup.rs:244-319`，启动时 spawn `lib.rs:2377`）只写 `~/.claude/settings.json`。codex/kimi 的 hooks 不会被 app 自动安装（多 provider 自动装是未实现的 `provider-setup` spec）。⇒ 新建的 codex/kimi 会话若 hooks 未装，**永远不会入库**。
6. **Agent 抽象已就位，但只有 resume 动词**：`Agent` trait（`meowo-reporter/src/agent.rs:15-46`）有 `resume_args(id)`（claude `["claude","--resume",id]` `:66-68`、kimi `[kimi_exe(),"-r",id]` `:102-105`、codex `[prefix..., "resume", id]` `:143-154`），但**没有裸启动动词**。可执行文件解析器：`codex_launch_prefix`（`codex.rs:28-42`）、`kimi_exe`（`kimi.rs:29-44`）、claude 走 PATH。`for_provider`/`all` `:196-203`。
7. **前端现成件**：`resume`/`focus` 调用样板 `Sticker.tsx:698-711`；空状态 `Sticker.tsx:455-461`；底部栏 `Sticker.tsx:1118-1169`；provider 图标/标签 `PROVIDERS`/`providerConfig`（`providers.tsx:51-60`）；终端下拉取数 `available_terminals`（`api.ts:153-155`、`About.tsx:503,517-525`）；账号/安装检测 `get_accounts`/`account::all`（`lib.rs:1735-1750`、`account/mod.rs:157-159`）。
8. **无目录选择器**：项目当前未引入 `tauri-plugin-dialog`，从未打开过系统文件夹选择器。
9. **设置存储**：`Settings`（`settings.rs:49-92`）持久化到 `~/.meowo/settings.json`；已有 `resume_terminal`（`:68-69`）；**无 `default_agent` 字段**。

## 方案取舍（关键决策，均已与用户确认）

| 决策点 | 取舍 | 定案 |
|--------|------|------|
| provider 范围 | 仅 claude / +kimi / 全部 | **全部三个 agent** |
| codex/kimi hooks 依赖 | 本期实现自动装 / 检测+引导 / 不管 | **检测 + 引导**，自动装留给 `provider-setup` |
| 新卡片如何出现 | 即时占位卡 / spawn+toast / 纯等待 | **spawn + 瞬态 toast**，不引入占位卡 |
| 交互形态 | 迷你面板 / split 按钮 / 纯一步 | **迷你面板**（集中选目录+agent+终端） |
| 入口位置 | 底部栏 / 空状态 / 两者 | **底部栏 + 空状态两处** |
| hooks 未装时 | 允许启动+提示 / 禁用启动 | **允许启动 + 提示引导**，不阻断 |

**为什么不做即时占位卡**：占位卡与真实 hook 的关联只能靠 `cwd + provider + 时间窗口`猜测（无 `session_id` 可锚定），用户在同一目录快速多开时极易错配；收益仅是填补 claude/kimi 的几秒空窗，复杂度与 bug 面不成正比。瞬态 toast 以最小代价覆盖「点击后有反馈」的核心诉求。

## 第一期设计

### 用户流程

1. **入口**：看板底部栏新增 `+ 新建` 按钮（常驻）；看板为空时空状态额外呈现一个大 CTA。两者打开**同一个** `NewSessionPanel`。
2. **面板**（看板主窗口内的**模态弹层**，非独立 WebviewWindow，与 rename/note 的 inline 编辑同风格）：
   - **目录**：文本框 + `浏览`（系统原生目录选择器）+ 最近目录快捷条（来自已有会话 cwd 去重）。
   - **Agent**：claude / codex / kimi 单选，图标来自 `PROVIDERS`。每个选项旁按 `check_provider_hooks` 结果标注状态：hooks 未装 → ⚠ + 「该会话可能不会出现在看板，点此了解如何安装」引导。
   - **终端**：下拉，复用 `available_terminals`，仅当可选项 ≥ 2 时显示；默认取 `settings.resume_terminal`。
   - `取消` / `启动`。
3. **启动**：点 `启动` → `invoke("new_session", {cwd, provider, terminal?})` → 后端在所选 cwd 打开终端跑该 agent 裸启动命令 → 关闭面板 → toast。
   - claude/kimi：toast「正在启动 <agent> 会话，卡片稍后出现」。
   - codex：toast 额外说明「codex 会话在你发送第一条消息后才会出现在看板」。

### 组件与改动点

#### 1. meowo-reporter：Agent 新增裸启动动词

- `Agent` trait 增 `launch_args(&self) -> Vec<String>`（与 `resume_args` 对称，不含 resume/id）：
  - claude → `["claude"]`
  - kimi → `[kimi_exe()]`
  - codex → `codex_launch_prefix()`（进入 codex TUI，无 `resume <id>`）
- 复用现有 `codex_launch_prefix`/`kimi_exe` 解析器，不新增可执行文件查找逻辑。

#### 2. app 后端：抽共享 spawn helper + 新命令

- **抽取 `spawn_in_terminal(app, argv, cwd, terminal_pref)`**：把 `resume_session`（`lib.rs:1044-1160`）中「终端选择 + spawn」那段（Windows 各分支、wezterm、macOS 分支）抽成独立 helper。`resume_session` 与 `new_session` 都调它——消除重复，保证 resume 行为不回归。resume 特有的乐观 revive / rollback（`prepare_resume`、`rollback_failed_resume`）**留在** `resume_session`，新建不涉及。
- **新命令 `new_session(app, cwd, provider, terminal: Option<String>)`**：
  1. 校验 cwd 存在且是目录（不存在/无权限 → 返回 Err）。
  2. `let argv = meowo_reporter::agent::for_provider(provider).launch_args();`
  3. `spawn_in_terminal(app, argv, cwd, terminal.unwrap_or(settings.resume_terminal))`。
  4. 无 `session_id`、不预改 DB、不做 rollback；spawn 失败直接返回 Err（前端面板展示）。
  注册进 invoke handler（`lib.rs:2244-2272` 同处）。
- **`check_provider_hooks(provider) -> HooksStatus`**（轻量只读）：判断该 provider 的 meowo-reporter hook 是否已登记。
  - claude：读 `~/.claude/settings.json` 的 `hooks` 段，比对 `ccsetup` 写入的条目是否存在（复用 `ccsetup.rs` 的读取/比对口径）。
  - codex/kimi：读各自 hook 配置（配置文件的精确位置在 writing-plans 阶段依 `scripts/install-hooks.mjs` 与 `provider-setup` spec 定位），判断是否存在指向 meowo-reporter 且带正确 `--provider` 的条目。
  - 返回枚举：`Installed` / `Missing` / `Unknown`（读取失败时降级为 `Unknown`，不误报）。
- **`recent_cwds(limit) -> Vec<String>`**：从 board.db `sessions` 取 distinct 非空 `cwd`，按 `last_event_at` 倒序，取前 `limit`（默认 8）。
- **引入 `tauri-plugin-dialog`**：加入依赖并在 builder 注册；前端直接用其 JS API `open({ directory: true })`，不新增后端目录命令。

#### 3. 前端

- **`NewSessionPanel`** 组件（模态弹层）：
  - 表单状态：`{ cwd, provider, terminal }`；provider 默认 `settings.default_agent`，terminal 默认 `settings.resume_terminal`。
  - 目录：`浏览` 调 dialog plugin；最近目录条调 `recent_cwds`。
  - agent 选项渲染 `check_provider_hooks` 状态（⚠ + 引导）。
  - `启动` → `invoke("new_session", …)`；成功关面板 + toast；失败面板内红字报错 + 可重试。
- **入口接线**：`Sticker.tsx` 底部栏（`:1118-1169`）加 `+ 新建` 按钮；空状态（`:455-461`）加 CTA。二者打开同一面板。
- **toast**：复用现有轻量提示机制；若无则新增一个 auto-dismiss 的极简 toast 组件。
- **i18n**：新增 `newSession.*` 文案（中英）。

#### 4. 设置

- `settings.rs` 的 `Settings` 增 `default_agent: String`（默认 `"claude"`），随 `get_settings`/`set_settings` 读写；前端 `api.ts` 的 `Settings` 类型同步。
- 终端**不新开字段**，复用 `resume_terminal`。
- 设置页（`About.tsx`）可选加「默认新建 agent」下拉（增强项，非必须——面板本身可切）。

### 数据流

```
点 [+ 新建] / 空状态 CTA
   └─▶ NewSessionPanel（选 cwd / agent / terminal）
          │  浏览 → tauri-plugin-dialog                最近目录 → recent_cwds(board.db)
          │  agent 状态 → check_provider_hooks
          └─▶ invoke new_session(cwd, provider, terminal)
                 └─▶ launch_args(provider) ─▶ spawn_in_terminal(argv, cwd, terminal)
                        └─▶ 终端里跑 `claude` / `kimi` / `codex`
                               └─▶ CLI 生成 session_id ─▶ hooks 上报 ─▶ board.db ─▶ 卡片出现
   （面板关闭 + toast：claude/kimi 秒级出卡；codex 首条消息后出卡）
```

### 错误处理

- **cwd 无效**（不存在 / 非目录 / 无权限）：面板内报错，不 spawn。
- **agent 可执行文件缺失**（PATH 无 `claude`、codex/kimi 解析失败或 spawn 失败）：捕获 spawn 错误 → 面板报错「未找到 <agent>，请确认已安装」+ 可重试。
- **hooks 未装**：**不阻断启动**。面板在该 agent 旁 ⚠ 预警；启动后 toast 明示「会话可能不会出现在看板，需先安装 hooks」+ 引导入口（指向后续 provider-setup / 文档）。
- **codex 固有延迟**：如实文案说明「发送首条消息后才出现」，不隐藏、不假装即时。
- 无 rollback 需求：新建不预改 DB，失败即失败，卡片从不误显。

### 测试

- **Rust 单测**：各 provider `launch_args` 正确（含 codex prefix、kimi exe 解析）；`new_session` cwd 校验分支；`spawn_in_terminal` 抽取后 `resume_session` 行为不回归（覆盖 wt/powershell/cmd 命令拼装）；`check_provider_hooks` 三态（Installed/Missing/Unknown）；`recent_cwds` 去重+排序+limit。
- **前端 vitest**：`NewSessionPanel` 表单状态机（默认值来自 settings、启动禁用条件）；agent 切换与 hooks 未装分支渲染；成功/失败后的面板与 toast 行为。
- **手动验收**：claude/kimi/codex 各新建一次，验证出卡时机（claude/kimi 秒级、codex 首条消息后）；hooks 未装时的引导；cwd 无效与 agent 缺失的报错;终端类型切换生效。

## 明确不做（第一期范围外）

- **不实现 codex/kimi 的 hooks 自动安装**——那是 `provider-setup` spec 的工作；本期仅检测 + 引导。
- **不做即时占位卡 / pending session 概念**——见上文取舍。
- **不做新建时的模板/参数**（如预填首条 prompt、选模型、传自定义 flag）——裸启动即可，后续可加。
- **不改 codex 首个回合才 fire `SessionStart` 的固有行为**——只在 UI 文案层面如实告知。

## 待确认假设（按定案先行，如需调整请在 review 时提出）

1. 面板为看板主窗口内的模态弹层，非独立窗口。
2. 目录选择用 `tauri-plugin-dialog` 原生选择器；「最近目录」数据源为已有会话的 cwd。
3. 终端复用 `resume_terminal`，不新增 `default_new_terminal`。
4. 新增 `default_agent` 设置字段，默认 `claude`。
5. hooks 检测对 claude 精确（复用 ccsetup 口径）；codex/kimi 的 hook 配置精确位置在 writing-plans 阶段定位，读取失败降级为 `Unknown` 不误报。

## 修订 R1：改为独立窗口（2026-07-06，用户执行中途决定）

原设计的「看板内模态弹层」改为**独立 WebviewWindow**（与设置/更新窗口同款多窗口机制）。表单内容、字段、hooks 检测、启动逻辑不变，仅**窗口形态**与**反馈流**调整。

**变更点：**
- **窗口**：新增后端命令 `open_new_session_window`（仿 `open_settings_window`：子线程建窗防消息泵白屏、单例 `get_webview_window("new-session")` 已开则聚焦、`WebviewUrl::App("index.html")` + label `"new-session"`、`inner_size` ~440×380、`resizable(false)`、`decorations(false)`、`center()`；macOS `transparent(true)` + `settings_window_will_open`/`did_close` 激活策略）。标题走 `tr(lang, "window.newSession")`。
- **路由 & 权限**：`main.tsx` 加 `label === "new-session"` 分支渲染 `NewSessionPanel` 整页；`capabilities/default.json` 的 `windows` 加 `"new-session"`。
- **面板**：`NewSessionPanel` 去掉 `onClose`/`onLaunched` props，改为独立窗口页——无边框故加可拖动标题区（`data-tauri-drag-region`）+ 关闭 X；`取消`/关闭 → `getCurrentWindow().close()`；启动**成功** → `emit("new-session-launched", msg)` 再 `close()`；**失败** → 窗内报错留窗重试。外壳从 `.ns-overlay`/`.ns-modal` 改为整页 `.ns-window`（表单内部样式不变）。
- **入口 & 反馈**：底部栏 `+ 新建` 按钮 + 空状态 CTA 点击 → `invoke("open_new_session_window")`（不再显示 overlay）。主看板 `Sticker` 监听 `new-session-launched` 事件 → 弹 toast（claude/kimi 秒级出卡；codex 提示发首条消息），toast 停留主看板窗口。
- **影响**：后端命令 `new_session`/`recent_cwds`/`check_provider_hooks`、前端 api、i18n 文案（原 Task 1–9）零改动；仅面板外壳 + 入口接线改，外加开窗基建。

**取舍理由**：用户希望「新建会话」是可独立摆放、不遮挡看板的窗口，而非临时遮罩弹层；独立窗口也更贴合「离开看板、专注配置一个新会话」这一动作。

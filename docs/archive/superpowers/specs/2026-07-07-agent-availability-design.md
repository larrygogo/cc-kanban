# 按实际安装展示 agent（Agent Availability）— 设计

> 日期：2026-07-07
> 状态：**草稿——待用户 review**
> 前置：基于 `feat/new-session-20260706`（PR #28，新建会话功能）——本特性改动其新建面板 agent 选择、设置页默认 agent 下拉，以及账号卡。实现分支从 new-session 延续或其 merge 后新起。

## 背景与目标

看板现在支持 claude / codex / kimi 三个 agent，但**所有让用户选/看 agent 的地方都硬编码列出全部三个**（`PROVIDER_KEYS = ["claude","codex","kimi"]`，`api.ts:13`），不管用户设备上实际装没装。结果：用户可能在新建面板选中一个没装的 agent、启动后 spawn 失败；设置里也列着用不上的 provider。

目标：**所有关于 agent 的展示，都依据用户设备上的实际安装情况**。选 agent 的地方只列已装的；账号页重构成「agent 管理中心」，显示每个 agent 的安装 + 账号 + 用量状态。

## 判定口径（已与用户确认）

**「装了」= 可执行存在**（最贴合"能不能真的新建会话"——`new_session` 要 spawn 它）：
- **claude**：`claude` 在 PATH 上（复用 `path_has_exe` 的思路）。
- **codex**：`codex_launch_prefix().is_some()`（`codex.rs:28`，即 bun 全局 `codex.exe` 或 npm 的 node 包装存在）。
- **kimi**：`kimi_exe()`（`kimi.rs:29`）解析到的绝对路径**真实存在**（注意 `kimi_exe` 找不到时回退裸名 `"kimi"`，判定要看解析路径是否 exists，而非是否返回字符串）。

不采用「数据目录存在」（CLI 卸载后 `~/.claude` 等可能残留 → 误判已装）、「账号已登录」（装了没登录会被漏掉）。

## 关键技术事实（探索核实，file:line 为实现锚点）

- 前端 agent 展示点：新建面板 agent 选择（`NewSessionPanel.tsx:159`）、设置页默认 agent 下拉（`About.tsx:601`）、贴纸配额 provider（`Sticker.tsx:539` + 设置页配额选择）、账号卡（`About.tsx:225` 渲染 `getAccounts()` 的 provider）。已有会话卡片的 agent 图标（`Sticker.tsx:939`）**不受影响**（会话存在即已装）。
- 检测零件已就位但无统一入口：`codex_home`（`codex.rs:13`）、`kimi_share_dir`（`kimi.rs:14`）、`kimi_exe`、`codex_launch_prefix`、`Agent::all()`（`agent.rs:215`）；claude 靠 PATH（`path_has_exe` 在 `lib.rs`）。
- 现成范式：`available_terminals` 命令（检测本机可用终端 → 前端过滤下拉），agent 照此加 `available_agents`。
- 账号：`get_accounts` → `account::all()` 遍历 provider 返回账号 payload；前端 `About.tsx` 渲染 provider-card。

## 设计

### 1. 检测（后端）

- **`Agent::is_installed(&self) -> bool`**（`meowo-reporter/agent.rs`）：每个 agent 自检可执行（口径见上）。claude 的 PATH 查找在 meowo-reporter 加一个小 helper（或复用现有 which 逻辑）；codex/kimi 用各自已有的解析器判存在。
- **`available_agents()` 命令**（`app/src-tauri`）：遍历 `agent::all()`，返回 `is_installed()` 为真的 `ProviderKey[]`。轻量、无副作用，仿 `available_terminals`。
- 前端 `availableAgents(): Promise<ProviderKey[]>` wrapper（`api.ts`）。

### 2. A —「选 agent」的地方 → 只列已装

- **新建面板 agent 选择**（`NewSessionPanel`）：agent 卡只渲染 `availableAgents` 里的；默认选中 `settings.default_agent`，**若它未装则退到首个已装的**。
- **设置页默认 agent 下拉**（`About`）：`options` = 已装。
- **贴纸配额 provider**（设置页选择 + 底栏配额屏）：只列/显示已装。
- **边缘——一个都没装**：新建面板显示「未检测到已安装的 AI CLI（claude / codex / kimi），请先安装」+ 启动按钮禁用；其他展示点空。

### 3. B — 账号页 → agent 卡（显示全部 + 状态，不过滤）

现「账号」卡重构为 **agent 卡**——不再只显示 `getAccounts` 返回的、也不按安装隐藏，而是**遍历全部三个 agent**，每张卡显示：
- agent 图标 + 名（`providerConfig`）。
- **安装状态**：已装 / 未安装（来自 `availableAgents`）。
- 账号 + 用量（来自 `getAccounts`）：已装且登录 → 正常账号 + 用量；已装未登录 → 「未登录」；未安装 → 「未安装」+ **「安装」按钮**（见 §C 一键安装）。
- 页面/文案「账号」→「Agent」，成为 agent 管理中心。

数据来源合并：前端拿 `availableAgents()` + `getAccounts()`，以「全部三个 agent」为骨架，各卡按两者拼装状态。`account::all()` 后端不改（前端按 provider 匹配 payload）。

### C. 一键安装（未装卡的「安装」按钮）

未装 agent 卡上的「安装」按钮 → 后端 `install_agent(provider)` 命令在一个终端窗口里跑该 agent **官方的一句话安装脚本**，用户实时看进度、装完关终端、面板重新检测显示已装。

`Agent::install_script(windows: bool) -> Option<String>`（meowo-reporter），命令经 GitHub / 官方文档核实（2026-07）：

| agent | Windows (PowerShell) | macOS / Linux |
|-------|----------------------|---------------|
| claude | `irm https://claude.ai/install.ps1 \| iex` | `curl -fsSL https://claude.ai/install.sh \| bash` |
| codex | `irm https://chatgpt.com/codex/install.ps1 \| iex` | `curl -fsSL https://chatgpt.com/codex/install.sh \| sh` |
| kimi | `irm https://code.kimi.com/install.ps1 \| iex` | `curl -LsSf https://code.kimi.com/install.sh \| bash` |

（三家两平台都有官方一句话；codex Windows 的 `install.ps1` 在 GitHub README，官方 CLI docs 页漏列。）

- **执行**：安装命令是平台 shell **命令串**（含管道），非简单 argv。Windows 开终端跑 `powershell -NoExit -ExecutionPolicy Bypass -Command "<script>"`（`-NoExit` 保留窗口看结果）；macOS 在 Terminal/iTerm 跑 `<script>`。终端类型沿用 `settings.resume_terminal`（默认终端）。为此在 `spawn_in_terminal` 旁加一个"跑命令串"的入口（或 helper），与新建会话的 argv spawn 并列。
- **装完刷新**：安装在独立终端异步进行，app 无法可靠得知何时完成 → 面板/agent 页在**窗口重新聚焦时**（或用户手动「刷新」）重新调 `available_agents` 更新状态。不阻塞、不轮询。
- **维护注意**：安装 URL 硬编码在 `install_script` 单点（官方改地址只需改一处）；失败时用户在终端直接看到错误，不是静默坑。

### 4. 刷新时机

`available_agents` 做成**实时命令**（可执行检测廉价：几次 PATH / 文件 stat），前端在打开新建面板 / 设置页 / agent 页时调，不缓存——与 `available_terminals` 完全一致。

### 5. 错误处理

- `available_agents` 命令失败（罕见）→ 前端兜底当作"全部未知/全部列出"，不阻断（宁可多列也不空）；或退回 `PROVIDER_KEYS`。以不让 UI 卡死为准。
- 单个 agent 检测抛错 → 视为未装（保守）。

## 测试

- **Rust**：各 agent `is_installed`（用 env 覆盖 PATH / CODEX_HOME / KIMI_SHARE_DIR 指向临时目录，构造装/未装两态）；`available_agents` 遍历（全装 / 部分 / 全无）。
- **前端 vitest**：新建面板 agent 选择按 `availableAgents` 过滤、default_agent 未装时退首个、一都没装的提示；设置默认 agent 下拉与配额 provider 过滤；agent 卡三态渲染（已装登录 / 已装未登录 / 未安装）。
- **手动**：本机装/卸某个 CLI，验证各处即时反映。

## 不做（YAGNI）

- **自定义安装逻辑 / 进度条 UI**——一键安装只在终端里跑官方一句话脚本，不自己实现下载 / 进度 / 重试（官方脚本自理）；也不自动 `--login`（装完用户自己在终端登录，或用现有账号流程）。
- 已有会话卡片的 agent 图标不动（会话存在即已装）。
- `account::all()` 后端过滤（前端拼装即可，改动更小）。

## 待确认假设（按定案先行）

1. 判定口径 = 可执行存在（非数据目录 / 账号）。
2. 「选 agent」的地方（新建 / 默认下拉 / 配额）只列已装；一都没装则新建面板提示 + 禁用。
3. 账号页重构为 agent 卡，显示全部三个 + 安装/账号/用量状态，未装标「未安装」不隐藏；文案「账号」→「Agent」。
4. `available_agents` 实时命令，不缓存。
5. 未装卡提供**「安装」按钮**：一键在终端跑官方安装脚本（三家 × Windows/macOS 命令见 §C 表），复用默认终端；装完在窗口重新聚焦 / 手动刷新时重检状态。兼容 Windows + macOS。

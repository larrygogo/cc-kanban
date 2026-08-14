<div align="center">
  <img src="docs/images/logo.png" width="104" alt="Meowo logo" />
  <h1>Meowo / 喵呜</h1>
  <p><b>桌面贴纸，集中查看 Claude Code、Codex、Kimi、Gemini CLI、OpenCode 等 AI 编程会话的状态。</b></p>
  <p>
    <a href="https://github.com/larrygogo/meowo/actions/workflows/ci.yml"><img src="https://github.com/larrygogo/meowo/actions/workflows/ci.yml/badge.svg" alt="CI" /></a>
    <a href="https://github.com/larrygogo/meowo/releases/latest"><img src="https://img.shields.io/github/v/release/larrygogo/meowo?label=release&color=d97757" alt="Release" /></a>
    <a href="https://github.com/larrygogo/meowo/releases"><img src="https://img.shields.io/github/downloads/larrygogo/meowo/total?color=4ec9a5" alt="Downloads" /></a>
    <img src="https://img.shields.io/badge/platform-Windows%20%7C%20macOS-555" alt="Platform: Windows | macOS" />
    <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT" /></a>
  </p>
  <p><b>中文</b> · <a href="README.en.md">English</a></p>
  <p>Meowo 把各个 AI CLI 的会话事件收集到本地数据库，再用一个小窗口实时展示。<br/>不用在终端之间来回切，也能知道谁在跑、谁在等你、进行到哪一步。</p>
  <img src="docs/images/demo.webp" alt="Meowo 演示：实时会话贴纸、待交互提醒、会话搜索、重命名归档、底栏用量、吸边缩略" width="760" />
</div>

## 下载

官网：**[meowo.io](https://meowo.io)** —— 点进去会按你的系统直接给对应安装包。

| 平台 | 安装包 | 说明 |
|------|--------|------|
| **Windows** | [最新版 x64 安装包](https://github.com/larrygogo/meowo/releases/latest)（`Meowo_x.y.z_x64-installer.exe`，老版本为 `-setup.exe`） | 一键安装；无 WebView2 环境自动回退经典安装界面 |
| **macOS** | [最新版 universal DMG](https://github.com/larrygogo/meowo/releases/latest)（`Meowo_x.y.z_universal.dmg`） | Intel / Apple Silicon 通用，需 macOS ≥ 14 Sonoma；已签名公证 |

下载对应安装包，双击安装即可。应用内支持检查更新。

> **Windows 首次运行会提示「Windows 已保护你的电脑」**：Meowo 暂未做代码签名，点
> **「更多信息」→「仍要运行」** 即可继续。安装包由 GitHub Actions 从公开源码构建
> （见 [release.yml](.github/workflows/release.yml)），自动更新另有 minisign 签名校验。
> 正在申请开源项目的免费代码签名，后续版本会去掉这个提示。

## 能做什么

### 实时会话看板

- 每个 AI CLI 会话一张卡片，显示项目名、会话标题、最近一条 AI 正文和连接状态。
- Claude Code 会话会显示准确的 **Context 已用百分比**（来自 statusline）。
- 顶部 tab 分类：全部 / 待交互 / 运行中 / 已归档，并显示各自数量。
- 状态用颜色区分：运行中（橙色转圈）、待交互（黄）、在线/闲置（绿）、已断开（虚线环）。
- 「待交互」按等待时间排序，等得最久的排在最前面。
- 底栏搜索可按会话标题或仓库名过滤。
- 第一次启动会自动导入最近 7 天的历史会话（最多 30 条），不会从空白开始。

### 点击直达终端

- 点**连接中**的会话，直接跳到它所在的终端标签页。Windows 精确切到 Windows Terminal 对应标签；macOS 聚焦 Terminal 或 iTerm2 对应标签。
- 点**已断开**的会话，在原项目目录新开终端并按对应 CLI 的方式续接会话（claude `--resume`、kimi `-r`、codex `resume` 等，由各 agent 插件声明）。
- 打开方式可以在设置里改：默认点卡片跳转，也可以改成只通过卡片上的「打开」按钮跳转。

### 内置对话窗与托管终端

- 会话可以直接在 Meowo 的原生对话窗里继续：完整对话流（用户消息、AI 正文、工具调用与结果）实时渲染，输入框直接发消息，斜杠命令与模型切换有补全菜单。
- 会话由 Meowo 托管的 PTY 持有，对话窗与外部终端只是同一会话的两种视图，可随时互切。
- 权限审批、AskUserQuestion 提问、计划确认渲染成可点的卡片；终端里的信任确认、模型菜单等弹窗也被识别成按钮。
- 从看板可直接**新建会话**：选 agent、工作目录与启动选项（模型、权限模式等，按各 CLI 支持情况下发）。
- 托管会话带屏幕状态角标（运行中 / 空闲 / 等待输入），来自终端画面的实时检测。

### 待交互与出错提醒

- 会话需要你回复，或者因为工具调用失败、认证失败等原因卡住时，会归类到「待交互」。
- 同一情况只弹一次系统通知，点击通知即可跳转到对应终端。
- 通知可在设置里关闭，默认开启。

### 卡片管理

- **星标置顶**：重要会话加星后始终排在最前面。
- **便签**：给会话挂一段本地备忘，纯本地，和会话内容无关。
- **改名**：在卡片上直接改名，写入与 Claude Code `/rename` 一致的记录。
- **归档**：把会话收进「已归档」，可随时还原；归档条目可在 1 / 7 / 30 天后自动隐藏。
- 操作按钮默认悬停才出现，保持列表简洁。

### 吸边与窗口（Windows）

- 把窗口拖到屏幕**左 / 右 / 顶**边缘，松手后缩成一条缩略条。左右为竖条，顶部为横条。
- 缩略条用状态色点表示会话；悬停展开，移开收回；拖离边缘恢复普通窗口。
- 可 pin 置顶，重启后保留上次的位置、尺寸和吸附边。
- 托盘悬停时显示待交互 / 运行中会话数。

### 菜单栏面板（macOS）

macOS 上是状态栏应用：无独立浮窗，不显示在 Dock。

- 左键点击菜单栏图标弹出贴纸面板，不抢焦点，失焦自动收起。
- 右键点击图标打开「设置 / 退出」菜单。
- 图标实时显示运行中与待交互会话数。

<details>
<summary>macOS 首次使用的权限说明</summary>

首次点击「跳转 / 恢复终端」会触发 macOS「自动化」授权（系统设置 → 隐私与安全性 → 自动化），需允许 Meowo 控制 Terminal / iTerm2；首次通知会请求通知权限。授权弹窗期间应用保持响应。

</details>

### 外观与系统集成

- 深色 / 浅色 / 跟随系统三种主题。
- 窗口不透明度（60%–100%）与界面密度（紧凑 / 标准 / 宽松）可调。
- Windows 托盘 / macOS 菜单栏均可快速打开设置或退出。
- 支持开机自启。

### 账号与用量

- 底栏常显 5 小时 / 7 天配额利用率。
- 设置页显示各家 CLI 的账号、用量与配额重置时间；支持 agent 一键安装、登录与多账号（profile）切换。
- 用量先显示缓存，再后台刷新；token 过期会自动续期。

### 接入 Claude Code

启动时，Meowo 会自动把 `meowo-reporter` 接入 Claude Code 的 hooks 和 statusLine，先备份、再原子写入，不会破坏已有配置。前提是 `~/.claude/settings.json` 已存在（运行过一次 Claude Code 就会生成）。

## 为什么叫 Meowo？

名字来自猫叫 **meow**，中文译作「喵呜」。

## 工作原理

> 以 Claude Code 为例；Codex / Kimi / Gemini CLI 走各自 CLI 的 hook 机制，OpenCode 没有 hook、改由一份自动生成的桥接插件转发事件。数据最终都落到同一份本地数据库。

```
 Claude Code 会话
   │  触发 hooks（SessionStart / UserPromptSubmit / PostToolUse / Stop / SessionEnd …）
   │  渲染 statusline（包装脚本把数据喂给 meowo-reporter statusline）
   ▼
 meowo-reporter（命令行，读 stdin 的事件 JSON）
   │  解析事件、标题、项目、todo、Context 用量
   ▼
 ~/.meowo/board.db（SQLite，WAL）
   ▲
   │  文件监听 + 去抖刷新
 meowo-app（Tauri 贴纸，React 前端）
```

- **meowo-reporter** 是无状态的一次性进程：Claude Code 每次触发 hook 都会启动它，读取事件、写库后立即退出，不会阻塞会话。
- **meowo-app** 启动时监听 `~/.meowo/` 目录变化，库一变就刷新 UI；同时跑后台任务标记空闲会话、首次导入历史会话。
- 两端只通过 SQLite 通信，运行时不直接依赖。

## 项目结构

```
meowo/
├── app/
│   ├── src/                # React 前端（贴纸看板、对话窗、吸边状态机、设置页）
│   └── src-tauri/          # Tauri 桌面壳（窗口、托盘、PTY 托管、审批、账号用量）；Rust workspace 根
│       └── crates/
│           ├── meowo-protocol/  # 跨进程契约（IPC DTO 生成 TypeScript + broker 线协议）
│           ├── meowo-store/     # SQLite 读写
│           ├── meowo-agent/     # Agent 插件层（身份、变体、transcript 解析、屏幕规则）
│           └── meowo-reporter/  # AI CLI hooks 上报器 + statusline + 首次导入
├── scripts/
│   └── install-hooks.mjs   # 把 meowo-reporter 接入 Claude Code settings.json
└── docs/                   # 架构文档（architecture/）与历史设计存档（archive/）
```

**技术栈**：Rust（Tauri v2 + rusqlite）、React 18 + TypeScript + Vite（xterm.js、react-markdown、TanStack Virtual）、Bun。

## 环境要求

- [Rust](https://rustup.rs/)（stable）
- [Bun](https://bun.sh/)
- Windows 上的 Tauri 前置依赖：**WebView2 Runtime**（Win11 自带）、**MSVC 构建工具**（Visual Studio Build Tools，含 C++ 桌面开发）。详见 [Tauri 前置依赖](https://tauri.app/start/prerequisites/)。
- macOS 上的前置依赖：**Xcode 命令行工具**（`xcode-select --install`）；如需本地构建 universal 包，另需 `rustup target add aarch64-apple-darwin x86_64-apple-darwin`。
- 已安装的 AI 编程 CLI（[Claude Code](https://docs.claude.com/en/docs/claude-code) / Codex / Kimi / [Gemini CLI](https://github.com/google-gemini/gemini-cli) / [OpenCode](https://opencode.ai)，用于产生会话事件）。

## 快速开始

```bash
# 1. 安装前端依赖
cd app
bun install

# 2. 开发模式运行（含热更新；首次会编译 Rust，稍慢）
bun run tauri dev
```

构建发布版安装包：

```bash
cd app
bun run tauri build
# 产物在 app/src-tauri/target/release/bundle/ 下（Windows 为 NSIS 安装包，macOS 为 dmg/app）
```

## 接入 Claude Code

Meowo 启动时会自动接入。如果你不想启动 app 就先挂 hooks，或要写入自定义 settings 路径，可以手动操作：

<details>
<summary>手动挂 hooks（可选）</summary>

```bash
# 1. 编译 meowo-reporter（Rust workspace 根在 app/src-tauri）
cd app/src-tauri && cargo build --release -p meowo-reporter
# 产物：app/src-tauri/target/release/meowo-reporter.exe

# 2. 把它接入 ~/.claude/settings.json 的 hooks（用绝对路径）
bun scripts/install-hooks.mjs "<仓库绝对路径>/app/src-tauri/target/release/meowo-reporter.exe"
```

脚本会把 meowo-reporter 挂到所需的 hook 事件上（SessionStart / UserPromptSubmit / PostToolUse / Stop / SessionEnd / PermissionRequest，以及 PreToolUse 的 AskUserQuestion / ExitPlanMode；普通 hook 带 5s 超时上限，PermissionRequest 为 310s——要等你审批）。用同一路径重复运行不会重复追加，也不会破坏你已有的其它 hooks。若更换了 reporter 路径（如 debug 换 release、换了安装目录），再跑一次脚本即可——它按可执行文件名认领旧条目并原地更新为新路径，不会留下重复条目。

> 此脚本仅用于 Claude Code（写入 `~/.claude/settings.json`）。其余几家不经本脚本，由 app 启动时自动接线：codex / kimi / gemini 写各自 CLI 的原生 hook 配置（hook 命令带 `--provider <id>`）；opencode 没有 hook 机制，改为在 `~/.config/opencode/plugin/` 下生成一份桥接插件，由它把事件转发给 meowo-reporter。

也可指定写入别的 settings 文件：`bun scripts/install-hooks.mjs <reporter路径> <settings路径>`，或用环境变量 `MEOWO_SETTINGS`。

</details>

挂好后，新开的 Claude Code 会话就会实时出现在贴纸里。

## 数据与配置

<details>
<summary>数据与配置文件位置</summary>

- **数据库**：`~/.meowo/board.db`（SQLite，WAL 模式）。可用环境变量 `MEOWO_DB` 覆盖路径。
- **应用设置**：`~/.meowo/settings.json`（通知、主题、不透明度、界面密度、语言、终端字号/行高、打开方式等全部设置项）。
- **用量缓存**：`~/.meowo/usage-cache.json`。
- **statusLine 包装脚本**：`~/.meowo/statusline.sh`（由 app 自动生成与维护，无需手改）。
- **首次导入标记**：`~/.meowo/imported.json`（存在即跳过再次导入）。删掉它可让下次启动重新导入近期历史会话。
- **前端本地状态**（localStorage）：当前 tab、吸附边、记忆的正常窗口尺寸、置顶偏好、会话星标。

</details>

## 测试

```bash
# Rust（全 workspace）
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings

# 前端
cd app
bunx tsc --noEmit
bunx vitest run
```

> 演示动图可以重新生成：`cd app && bun run demo:webp`（Playwright 逐帧录制 `demo.html`，再用 sharp 合成动画 WebP，产物写到 `docs/images/demo.webp`）。

## 路线

- [x] CI（GitHub Actions：cargo test/clippy + IPC 契约生成校验 + 前端 tsc/vitest + cargo/bun audit，windows-latest + macos-latest）
- [x] 在线更新（`tauri-plugin-updater` + tag 触发的 GitHub Releases）
- [x] macOS 打包（universal dmg，签名公证 + 自动更新）
- [ ] Linux 打包

现行架构见 [`docs/architecture/`](docs/architecture/)；历史设计与实现计划存档在 [`docs/archive/superpowers/`](docs/archive/superpowers/)。

## License

[MIT](LICENSE) © larrygogo

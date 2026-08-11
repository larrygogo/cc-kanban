<div align="center">
  <img src="docs/images/logo.png" width="104" alt="Meowo logo" />
  <h1>Meowo / 喵呜</h1>
  <p><b>A desktop sticker that keeps an eye on your Claude Code, Codex, Kimi, Gemini CLI, and OpenCode coding sessions — all in one place.</b></p>
  <p>
    <a href="https://github.com/larrygogo/meowo/actions/workflows/ci.yml"><img src="https://github.com/larrygogo/meowo/actions/workflows/ci.yml/badge.svg" alt="CI" /></a>
    <a href="https://github.com/larrygogo/meowo/releases/latest"><img src="https://img.shields.io/github/v/release/larrygogo/meowo?label=release&color=d97757" alt="Release" /></a>
    <a href="https://github.com/larrygogo/meowo/releases"><img src="https://img.shields.io/github/downloads/larrygogo/meowo/total?color=4ec9a5" alt="Downloads" /></a>
    <img src="https://img.shields.io/badge/platform-Windows%20%7C%20macOS-555" alt="Platform: Windows | macOS" />
    <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT" /></a>
  </p>
  <p><a href="README.md">中文</a> · <b>English</b></p>
  <p>Meowo collects session events from your AI CLIs into a local database and shows them live in a small window.<br/>No more switching between terminals to see what's running, what's waiting on you, and how far along each one is.</p>
  <img src="docs/images/demo.webp" alt="Meowo demo: live session stickers with the latest AI message, waiting-for-you alerts, card menu (rename / note / archive), session search, quota readout, edge-snap strip" width="760" />
</div>

## Download

Website: **[meowo.io](https://meowo.io)** — it hands you the installer for your OS directly.

| Platform | Installer | Notes |
|----------|-----------|-------|
| **Windows** | [Latest x64 installer](https://github.com/larrygogo/meowo/releases/latest) (`Meowo_x.y.z_x64-setup.exe`) | NSIS installer |
| **macOS** | [Latest universal DMG](https://github.com/larrygogo/meowo/releases/latest) (`Meowo_x.y.z_universal.dmg`) | Universal (Intel / Apple Silicon), requires macOS ≥ 14 Sonoma; signed & notarized |

Download the installer for your platform and double-click to install. The app supports in-app update checks.

> **Windows shows "Windows protected your PC" on first run**: Meowo isn't code-signed yet.
> Choose **"More info" → "Run anyway"** to continue. Installers are built by GitHub Actions
> from public source ([release.yml](.github/workflows/release.yml)), and auto-updates carry a
> separate minisign signature. We're applying for free open-source code signing; a future
> release will drop this prompt.

## What it does

### Live session board

- One card per AI CLI session, showing the project name, session title, the latest AI message, and connection status.
- Claude Code sessions show an accurate **Context usage percentage** (sourced from the statusline).
- Tabs at the top: All / Waiting / Running / Archived, each with a count.
- Status is color-coded: running (orange spinner), waiting (yellow), online/idle (green), disconnected (dashed ring).
- The "Waiting" tab is sorted by wait time, so the session that's been waiting longest comes first.
- The search box in the bottom bar filters by session title or repo name.
- On first launch it imports your recent sessions from the last 7 days (up to 30), so you don't start from an empty board.

### Jump straight to the terminal

- Click a **connected** session to jump to its terminal tab. On Windows it switches to the matching Windows Terminal tab; on macOS it focuses the matching Terminal or iTerm2 tab.
- Click a **disconnected** session to open a new terminal in its project directory and resume it the way that CLI expects (claude `--resume`, kimi `-r`, codex `resume`, etc. — declared by each agent plugin).
- The open behavior is configurable: by default clicking the card jumps; you can switch it to jump only via a dedicated "Open" button on the card.

### Built-in chat window & managed terminal

- Continue a session right inside Meowo's native chat window: the full conversation (user messages, AI text, tool calls and results) renders in real time, with a composer to send messages and completion menus for slash commands and model switching.
- The session is owned by a Meowo-managed PTY; the chat window and an external terminal are just two views of the same session, switchable at any time.
- Permission approvals, AskUserQuestion prompts, and plan confirmations become clickable cards; terminal popups like trust confirmations and model menus are recognized as buttons.
- Create **new sessions** straight from the board: pick the agent, working directory, and launch options (model, permission mode, etc., offered per CLI support).
- Managed sessions carry a screen-state badge (working / idle / blocked), detected live from the terminal screen.

### Waiting & error alerts

- When a session needs your reply, or gets stuck (failed tool call, auth failure, etc.), it's grouped under "Waiting".
- A deduplicated system notification fires once per situation; click it to jump to the terminal.
- Notifications can be turned off in settings; they're on by default.

### Card management

- **Star to pin**: starred sessions always stay at the top of the list.
- **Notes**: attach a local memo to a session — purely local, unrelated to the session content.
- **Rename**: rename a session right on the card, writing a record consistent with Claude Code's `/rename`.
- **Archive**: move a session into "Archived" (restorable anytime); archived items can auto-hide after 1 / 7 / 30 days.
- Action buttons appear on hover by default, keeping the list clean.

### Edge-snap & window (Windows)

- Drag the window to the **left / right / top** edge of the screen and release — it collapses into a thin strip (vertical on the sides, horizontal on top).
- The strip shows sessions as status-colored dots; hover to peek, move away to collapse, drag off the edge to restore the full window.
- You can pin it on top, and it remembers its position, size, and snap edge across restarts.
- Hovering the tray icon shows the waiting / running session counts.

### Menu bar panel (macOS)

On macOS it's a status bar app: no floating window, not shown in the Dock.

- Left-click the menu bar icon to pop up the sticker panel — it doesn't steal focus and collapses when it loses focus.
- Right-click the icon for the "Settings / Quit" menu.
- The icon shows the running and waiting session counts in real time.

<details>
<summary>First-run permissions on macOS</summary>

The first time you click "jump to / resume terminal", macOS asks for "Automation" permission (System Settings → Privacy & Security → Automation) — allow Meowo to control Terminal / iTerm2. The first notification requests notification permission. The app stays responsive during these prompts.

</details>

### Appearance & system integration

- Dark / light / follow-system themes.
- Adjustable window opacity (60%–100%) and UI density (compact / standard / comfortable).
- Quick access to settings or quit from the Windows tray / macOS menu bar.
- Supports launch at login.

### Account & usage

- The bottom bar always shows your 5-hour / 7-day quota utilization.
- The settings page shows accounts, usage, and quota reset times for each CLI; agents can be installed with one click, logged in, and switched between multiple accounts (profiles).
- Usage shows cached values first, then refreshes in the background; expired tokens auto-renew.

### Connecting to Claude Code

On startup, Meowo automatically wires `meowo-reporter` into Claude Code's hooks and statusLine — backing up first, then writing atomically, without breaking your existing config. This requires `~/.claude/settings.json` to already exist (running Claude Code once creates it).

## Why "Meowo"?

The name comes from the sound a cat makes — **meow** — rendered in Chinese as 喵呜 ("miāo-wū").

## How it works

> Claude Code is used as the example here; Codex / Kimi / Gemini CLI use their own CLI hook mechanisms, while OpenCode has no hooks at all and instead gets an auto-generated bridge plugin that forwards its events. The data all lands in the same local database.

```
 Claude Code session
   │  fires hooks (SessionStart / UserPromptSubmit / PostToolUse / Stop / SessionEnd …)
   │  renders statusline (a wrapper feeds the data to meowo-reporter statusline)
   ▼
 meowo-reporter (CLI, reads event JSON from stdin)
   │  parses events, title, project, todos, Context usage
   ▼
 ~/.meowo/board.db (SQLite, WAL)
   ▲
   │  file watching + debounced refresh
 meowo-app (Tauri sticker, React frontend)
```

- **meowo-reporter** is a stateless one-shot process: Claude Code launches it on every hook, it reads the event, writes to the DB, and exits immediately — never blocking the session.
- **meowo-app** watches the `~/.meowo/` directory on startup and refreshes the UI whenever the DB changes; it also runs background tasks to mark idle sessions and import history on first run.
- The two sides communicate only through SQLite, with no direct runtime dependency.

## Project structure

```
meowo/
├── app/
│   ├── src/                # React frontend (sticker board, chat window, edge-snap state machine, settings)
│   └── src-tauri/          # Tauri desktop shell (window, tray, PTY hosting, approvals, account usage); Rust workspace root
│       └── crates/
│           ├── meowo-protocol/  # cross-process contracts (IPC DTOs generating TypeScript + broker wire protocol)
│           ├── meowo-store/     # SQLite read/write
│           ├── meowo-agent/     # agent plugin layer (identity, variants, transcript parsing, screen rules)
│           └── meowo-reporter/  # AI CLI hooks reporter + statusline + first-run import
├── scripts/
│   └── install-hooks.mjs   # wires meowo-reporter into Claude Code's settings.json
└── docs/                   # architecture docs (architecture/) & archived designs (archive/)
```

**Stack**: Rust (Tauri v2 + rusqlite), React 18 + TypeScript + Vite (xterm.js, react-markdown, TanStack Virtual), Bun.

## Requirements

- [Rust](https://rustup.rs/) (stable)
- [Bun](https://bun.sh/)
- Tauri prerequisites on Windows: **WebView2 Runtime** (bundled with Win11) and the **MSVC build tools** (Visual Studio Build Tools, incl. C++ desktop development). See [Tauri prerequisites](https://tauri.app/start/prerequisites/).
- Prerequisites on macOS: **Xcode Command Line Tools** (`xcode-select --install`); to build a universal package locally you also need `rustup target add aarch64-apple-darwin x86_64-apple-darwin`.
- An installed AI coding CLI ([Claude Code](https://docs.claude.com/en/docs/claude-code) / Codex / Kimi / [Gemini CLI](https://github.com/google-gemini/gemini-cli) / [OpenCode](https://opencode.ai)) to produce session events.

## Quick start

```bash
# 1. Install frontend dependencies
cd app
bun install

# 2. Run in dev mode (with hot reload; the first run compiles Rust and is slower)
bun run tauri dev
```

Build a release installer:

```bash
cd app
bun run tauri build
# Output goes to app/src-tauri/target/release/bundle/ (NSIS installer on Windows, dmg/app on macOS)
```

## Connecting to Claude Code

Meowo wires itself in automatically on startup. If you'd rather set up the hooks without launching the app, or write to a custom settings path, you can do it manually:

<details>
<summary>Wire up hooks manually (optional)</summary>

```bash
# 1. Build meowo-reporter (the Rust workspace root is app/src-tauri)
cd app/src-tauri && cargo build --release -p meowo-reporter
# Output: app/src-tauri/target/release/meowo-reporter.exe

# 2. Wire it into ~/.claude/settings.json hooks (use an absolute path)
bun scripts/install-hooks.mjs "<absolute-repo-path>/app/src-tauri/target/release/meowo-reporter.exe"
```

The script wires meowo-reporter into the required hook events (SessionStart / UserPromptSubmit / PostToolUse / Stop / SessionEnd / PermissionRequest, plus PreToolUse's AskUserQuestion / ExitPlanMode; normal hooks get a 5s timeout cap, PermissionRequest 310s so it can wait for your approval). Running it again with the same path won't duplicate entries or break your other hooks. If you change the reporter path (e.g. debug → release, or a new install location), just run the script again — it claims its existing entries by executable name and updates them to the new path in place, leaving no duplicates.

> This script is for Claude Code only (it writes to `~/.claude/settings.json`). The others don't go through it — the app wires them on startup: Codex / Kimi / Gemini get entries in their own CLIs' native hook config (hook commands carry `--provider <id>`); OpenCode has no hook mechanism, so a bridge plugin is generated under `~/.config/opencode/plugin/` to forward events to meowo-reporter.

You can also write to a different settings file: `bun scripts/install-hooks.mjs <reporter-path> <settings-path>`, or use the `MEOWO_SETTINGS` environment variable.

</details>

Once wired up, new Claude Code sessions show up in the sticker in real time.

## Data & configuration

<details>
<summary>Data & config file locations</summary>

- **Database**: `~/.meowo/board.db` (SQLite, WAL mode). Override the path with the `MEOWO_DB` environment variable.
- **App settings**: `~/.meowo/settings.json` (notifications, theme, opacity, UI density, language, terminal font size / line height, open behavior, and every other setting).
- **Usage cache**: `~/.meowo/usage-cache.json`.
- **statusLine wrapper script**: `~/.meowo/statusline.sh` (generated and maintained by the app; no manual edits needed).
- **First-import marker**: `~/.meowo/imported.json` (skips re-importing if present). Delete it to re-import recent history on the next launch.
- **Frontend local state** (localStorage): current tab, snap edge, remembered normal window size, pin preference, session stars.

</details>

## Testing

```bash
# Rust (whole workspace)
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings

# Frontend
cd app
bunx tsc --noEmit
bunx vitest run
```

> The demo animation can be regenerated: `cd app && bun run demo:webp` (Playwright captures `demo.html` frame-by-frame at 2×, then sharp composes a high-fidelity animated WebP written to `docs/images/demo.webp`).

## Roadmap

- [x] CI (GitHub Actions: cargo test/clippy + IPC contract generation check + frontend tsc/vitest + cargo/bun audit, windows-latest + macos-latest)
- [x] Online updates (`tauri-plugin-updater` + tag-triggered GitHub Releases)
- [x] macOS packaging (universal dmg, signed & notarized + auto-update)
- [ ] Linux packaging

See [`docs/architecture/`](docs/architecture/) for the current architecture; historical designs and implementation plans are archived under [`docs/archive/superpowers/`](docs/archive/superpowers/).

## License

[MIT](LICENSE) © larrygogo

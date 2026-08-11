# 状态图标统一 + 界面多语言 + demo.gif 重录 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 待交互图标统一为举手；界面（前端 + Rust 通知/托盘）支持中英双语；最后重录 demo.gif。

**Architecture:** 任务 1 是 Sticker.tsx 两处 SVG 替换。任务 2 自研轻量字典（`app/src/i18n/` 的 zh/en 嵌套对象 + React context，`en: typeof zh` 编译期对齐），Settings 加 `language` 字段（auto/zh/en），Rust 侧 11 条文案用静态函数 + sys-locale 检测；切语言经 settings-changed 实时生效，托盘菜单在 set_settings 时重建。任务 3 在前两者合并 main 后跑现有录制管线。

**Tech Stack:** React 18 + TS、Tauri v2、sys-locale crate、vitest、Playwright+gifenc 管线（现成）。

**对应 spec：** `docs/superpowers/specs/2026-06-10-icons-i18n-demo-regen-design.md`

---

## 任务 1：待交互图标 气泡 → 举手（分支 `style/unify-waiting-icon-20260610`）

**Files:**
- Modify: `app/src/views/Sticker.tsx:67-72`（TabIcon）、`app/src/views/Sticker.tsx:181-186`（EmptyIcon）

- [ ] **Step 1.1: 建分支**：`git checkout main && git pull && git checkout -b style/unify-waiting-icon-20260610`

- [ ] **Step 1.2: TabIcon waiting 换举手**。`Sticker.tsx:67-72` 的 case "waiting" 整体替换为（lucide `hand` 路径，与 running case 同走 24 viewBox）：

```tsx
    case "waiting": // 举手（待交互）——与 macOS 菜单栏 hand.raised 同隐喻；lucide hand
      return (
        <svg {...common} viewBox="0 0 24 24" fill="none" stroke="currentColor"
          strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
          <path d="M18 11V6a2 2 0 0 0-2-2a2 2 0 0 0-2 2" />
          <path d="M14 10V4a2 2 0 0 0-2-2a2 2 0 0 0-2 2v2" />
          <path d="M10 10.5V6a2 2 0 0 0-2-2a2 2 0 0 0-2 2v8" />
          <path d="M18 8a2 2 0 1 1 4 0v6a8 8 0 0 1-8 8h-2c-2.8 0-4.5-.86-5.99-2.34l-3.6-3.6a2 2 0 0 1 2.83-2.82L7 15" />
        </svg>
      );
```

- [ ] **Step 1.3: EmptyIcon waiting 换举手**。`Sticker.tsx:181-186` 的 case "waiting" 替换为（common 已是 24 viewBox + stroke 属性，只给 path）：

```tsx
    case "waiting": // 举手（待交互）
      return (
        <svg {...common}>
          <path d="M18 11V6a2 2 0 0 0-2-2a2 2 0 0 0-2 2" />
          <path d="M14 10V4a2 2 0 0 0-2-2a2 2 0 0 0-2 2v2" />
          <path d="M10 10.5V6a2 2 0 0 0-2-2a2 2 0 0 0-2 2v8" />
          <path d="M18 8a2 2 0 1 1 4 0v6a8 8 0 0 1-8 8h-2c-2.8 0-4.5-.86-5.99-2.34l-3.6-3.6a2 2 0 0 1 2.83-2.82L7 15" />
        </svg>
      );
```

- [ ] **Step 1.4: 跑测试**：`cd app && bun run test`。预期全过（现有测试不断言气泡 path）。
- [ ] **Step 1.5: 视觉抽查**：`node scripts/snap-frame.mjs`（或起 dev）确认 tab 图标渲染正常。
- [ ] **Step 1.6: Commit + PR**：`git add app/src/views/Sticker.tsx && git commit -m "style(ui): 待交互图标统一为举手，与 macOS 菜单栏隐喻一致"`；推分支开 PR，CI 绿后合并。

---

## 任务 2：界面多语言（分支 `feat/i18n-zh-en-20260610`，基于任务 1 合并后的 main）

### 2A. i18n 基建（前端）

**Files:**
- Create: `app/src/i18n/zh.ts`、`app/src/i18n/en.ts`、`app/src/i18n/index.tsx`
- Test: `app/src/i18n/i18n.test.ts`

- [ ] **Step 2A.1: 写 `app/src/i18n/zh.ts`**（基准字典，完整内容）：

```ts
// 中文字典（基准）。en.ts 用 `typeof zh` 约束 key 与函数签名编译期对齐。
// 注意:「(未命名会话)」是数据库 sentinel 不在此处——展示层用 sticker.waitingFirstInput 映射。
export const zh = {
  tabs: { all: "全部", waiting: "待交互", running: "运行中", archived: "已归档" },
  time: {
    now: "刚刚",
    minAgo: (m: number) => `${m} 分钟前`,
    hourAgo: (h: number) => `${h} 小时前`,
    dayAgo: (d: number) => `${d} 天前`,
  },
  conn: { on: "已连接", off: "已断开" },
  badge: {
    waiting: "等待输入",
    running: "运行中",
    full: (what: string, pct: number) => `${what} · Context 已用 ${pct}%`,
  },
  empty: {
    allTitle: "还没有会话",
    allHint: "在终端运行 Claude Code，进度会自动出现在这里",
    waitingTitle: "没有等待交互的会话",
    waitingHint: "有会话需要你回复时会出现在这里",
    runningTitle: "当前没有运行中的会话",
    archivedTitle: "没有归档的会话",
    archivedHint: "点卡片右上角按钮可收纳会话",
  },
  sticker: {
    pinOn: "已置顶：点击取消",
    pinOff: "置顶窗口",
    waitingFirstInput: "等待首次输入",
    stopped: "已断开/已停止",
    sessionError: "会话出错",
    online: "在线",
    jumpToTerminal: "点击跳转到该会话的终端",
    resumeInTerminal: "点击新建终端恢复该会话",
    renameTitle: "重命名（同步到 Claude）",
    renamePlaceholder: "输入名称，回车保存",
    renameHint: "运行中：改名后需在该终端 /resume 才生效",
    archive: "归档",
    unarchive: "取消归档",
  },
  update: {
    clickToInstall: "点击下载并安装新版本",
    downloading: (pct: number) => `下载更新中 ${pct}%`,
    newVersion: (v: string) => `有新版本 v${v} · 点击更新`,
  },
  errorLabels: {
    // key 为 meowo-store 写库的中文 sentinel（数据值，不改库）；zh 原样、en 翻译。
    "工具调用解析失败": "工具调用解析失败",
    "需要重新登录": "需要重新登录",
    "认证失败": "认证失败",
  } as Record<string, string>,
  settings: {
    nav: { general: "通用", appearance: "外观", account: "账号", about: "关于" },
    close: "关闭",
    autostart: "开机自启",
    autostartDesc: "登录系统后自动启动 Meowo",
    notify: "桌面通知",
    notifyDesc: "会话需要你回复或出错时弹系统通知",
    archiveHide: "归档自动隐藏",
    archiveHideDesc: "归档超过所选时长后，自动从「已归档」中隐藏",
    hideNever: "永不",
    hideDays: (d: number) => `${d} 天`,
    resumeTerm: "未连接会话打开终端",
    resumeTermDesc: "点开已断开的会话时，用哪个终端运行 claude --resume",
    cmdPrompt: "命令提示符",
    language: "语言",
    languageDesc: "界面与系统通知的显示语言",
    langAuto: "跟随系统",
    moreSoon: "更多设置项陆续补充中…",
    theme: "外观模式",
    themeDesc: "深色、浅色，或跟随系统",
    themeDark: "深色",
    themeLight: "浅色",
    themeSystem: "跟随系统",
    density: "界面密度",
    densityDesc: "调整贴纸卡片的字号与间距",
    densityCompact: "紧凑",
    densityNormal: "标准",
    densityLoose: "宽松",
    opacity: "贴纸不透明度",
    opacityDesc: "调整桌面贴纸的背景透明度",
    appearanceHint: "外观更改即时生效，并保存到本地。",
  },
  account: {
    notLoggedIn: "未登录 Claude Code",
    notLoggedInDesc: "在终端运行 claude 登录后即可查看账号与用量",
    quota: "配额",
    refresh: "刷新",
    quota5h: "5 小时配额",
    quota7d: "7 天配额",
    quotaOpus: "Opus · 7 天",
    quotaSonnet: "Sonnet · 7 天",
    extraUsage: "已开启超额用量",
    refreshFailed: "最新数据刷新失败，显示的是缓存值",
    usageUnavailable: "用量暂不可用，请确认已登录 Claude Code（终端运行 claude）或检查网络",
    loading: "加载中…",
    dailyUsage: "每日用量",
    less: "少",
    more: "多",
    cellTitle: (date: string, kTokens: string, msgs: number) => `${date} · ${kTokens}k token · ${msgs} 条`,
    dataAsOf: (date: string) => `数据截至 ${date}，在终端运行 /stats 可刷新`,
    resetSoon: "即将重置",
    resetInMin: (m: number) => `${m} 分钟后重置`,
    resetInHour: (h: number) => `${h} 小时后重置`,
    resetInHourMin: (h: number, m: number) => `${h} 小时 ${m} 分后重置`,
    resetTomorrow: (clock: string) => `明天 ${clock} 重置`,
    resetDayAfter: (clock: string) => `后天 ${clock} 重置`,
    resetOnDate: (mo: number, d: number, clock: string) => `${mo} 月 ${d} 日 ${clock} 重置`,
  },
  about: {
    updating: "更新中…",
    updateTo: (v: string) => `更新到 v${v}`,
    checking: "检查中…",
    checkUpdate: "检查更新",
    foundNew: (v: string) => `发现新版本 v${v}`,
    upToDate: "已是最新版本",
    versionInfo: "版本信息",
    homepage: "项目主页",
    open: "打开",
    blurb: "常驻桌面贴纸，实时显示所有 Claude Code 会话的进度。",
    feedback: "意见反馈",
    changelog: "更新日志",
  },
};
export type Dict = typeof zh;
```

- [ ] **Step 2A.2: 写 `app/src/i18n/en.ts`**（`const en: Dict` 全量英文翻译；与 zh 同结构，逐 key 翻译。代表性条目如下，其余照译）：

```ts
import type { Dict } from "./zh";

export const en: Dict = {
  tabs: { all: "All", waiting: "Waiting", running: "Running", archived: "Archived" },
  time: {
    now: "now",
    minAgo: (m) => `${m} min ago`,
    hourAgo: (h) => `${h} hr ago`,
    dayAgo: (d) => `${d} days ago`,
  },
  conn: { on: "Connected", off: "Disconnected" },
  badge: {
    waiting: "Waiting for input",
    running: "Running",
    full: (what, pct) => `${what} · Context ${pct}% used`,
  },
  empty: {
    allTitle: "No sessions yet",
    allHint: "Run Claude Code in a terminal and progress shows up here",
    waitingTitle: "No sessions waiting for you",
    waitingHint: "Sessions needing your reply appear here",
    runningTitle: "No running sessions",
    archivedTitle: "No archived sessions",
    archivedHint: "Use the button on a card's top-right to archive it",
  },
  sticker: {
    pinOn: "Pinned: click to unpin",
    pinOff: "Pin window",
    waitingFirstInput: "Waiting for first input",
    stopped: "Disconnected / stopped",
    sessionError: "Session error",
    online: "Online",
    jumpToTerminal: "Click to jump to this session's terminal",
    resumeInTerminal: "Click to resume in a new terminal",
    renameTitle: "Rename (syncs to Claude)",
    renamePlaceholder: "Type a name, Enter to save",
    renameHint: "Running: rename takes effect after /resume in that terminal",
    archive: "Archive",
    unarchive: "Unarchive",
  },
  update: {
    clickToInstall: "Click to download and install",
    downloading: (pct) => `Downloading update ${pct}%`,
    newVersion: (v) => `v${v} available · click to update`,
  },
  errorLabels: {
    "工具调用解析失败": "Tool call parse failed",
    "需要重新登录": "Re-login required",
    "认证失败": "Authentication failed",
  },
  settings: { /* …全量照译，nav: General/Appearance/Account/About、Language、Follow system 等… */ },
  account: { /* …全量照译… */ },
  about: { /* …全量照译… */ },
};
```

（执行时 settings/account/about 三段必须写全——`Dict` 类型保证漏一个 key 编译即红，无遗漏风险。）

- [ ] **Step 2A.3: 写 `app/src/i18n/index.tsx`**（完整内容）：

```tsx
// 轻量 i18n：嵌套字典 + context。语言来源 Settings.language（auto/zh/en，auto 按
// navigator.language 解析）；仿 appearance.ts——localStorage 缓存防首屏闪错语言，
// settings-changed 实时切换并消除 fetch-vs-subscribe 竞态。
import { createContext, useContext, useEffect, useState, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import { getSettings, type Settings } from "../api";
import { zh, type Dict } from "./zh";
import { en } from "./en";

export type LangSetting = "auto" | "zh" | "en";
export type Lang = "zh" | "en";

const CACHE_KEY = "meowo-lang";

export function resolveLang(setting: string | undefined): Lang {
  if (setting === "zh" || setting === "en") return setting;
  return /^zh\b|^zh-/i.test(navigator.language) ? "zh" : "en";
}

function readCache(): Lang {
  const c = localStorage.getItem(CACHE_KEY);
  return c === "en" ? "en" : c === "zh" ? "zh" : resolveLang(undefined);
}

const DICTS: Record<Lang, Dict> = { zh, en };
const I18nCtx = createContext<Dict>(zh);

/** 取当前语言字典：const t = useT(); t.tabs.all */
export function useT(): Dict {
  return useContext(I18nCtx);
}

export function I18nProvider({ children, initial }: { children: ReactNode; initial?: Lang }) {
  const [lang, setLang] = useState<Lang>(() => initial ?? readCache());
  useEffect(() => {
    if (initial) return; // 测试注入固定语言时不订阅
    let eventApplied = false;
    const apply = (s: Partial<Settings>) => {
      const l = resolveLang((s as Settings).language);
      setLang(l);
      try { localStorage.setItem(CACHE_KEY, l); } catch { /* ignore */ }
    };
    try {
      listen<Settings>("settings-changed", (e) => { eventApplied = true; apply(e.payload); }).catch(() => {});
    } catch { /* 非 Tauri 环境 */ }
    getSettings().then((s) => { if (!eventApplied) apply(s); }).catch(() => {});
  }, [initial]);
  return <I18nCtx.Provider value={DICTS[lang]}>{children}</I18nCtx.Provider>;
}
```

- [ ] **Step 2A.4: 写字典对齐测试 `app/src/i18n/i18n.test.ts`**：

```ts
import { describe, expect, it } from "vitest";
import { zh } from "./zh";
import { en } from "./en";
import { resolveLang } from "./index";

// en: Dict 已由编译期保证 key 对齐；这里补运行时校验函数 key 的参数个数一致 + resolveLang。
function keys(o: object, prefix = ""): string[] {
  return Object.entries(o).flatMap(([k, v]) =>
    v !== null && typeof v === "object" ? keys(v, `${prefix}${k}.`) : [`${prefix}${k}`],
  );
}

describe("i18n dicts", () => {
  it("zh/en key sets identical", () => {
    expect(keys(en).sort()).toEqual(keys(zh).sort());
  });
});

describe("resolveLang", () => {
  it("explicit wins", () => {
    expect(resolveLang("zh")).toBe("zh");
    expect(resolveLang("en")).toBe("en");
  });
});
```

- [ ] **Step 2A.5: 跑测试**：`cd app && bun run test`，预期新测试过、`tsc` 无错（`bun run build` 或 vitest 即可暴露类型错）。
- [ ] **Step 2A.6: Commit**：`feat(i18n): 中英字典与 I18nProvider 基建（en 由 typeof zh 编译期对齐）`

### 2B. Settings 增加 language（前后端打通）

**Files:**
- Modify: `app/src-tauri/src/lib.rs:907-945`（Settings struct/Default）、`app/src-tauri/Cargo.toml`、`app/src/api.ts:94-109`、`app/src/views/About.tsx`（SETTINGS_DEFAULTS + GeneralSection 加语言行）

- [ ] **Step 2B.1: Rust Settings 加字段**。`lib.rs` Settings struct 末尾加：

```rust
    /// 界面/通知语言：auto（跟随系统）/ zh / en。缺省 auto，兼容老 settings.json。
    #[serde(default = "default_language")]
    language: String,
```

`default_resume_terminal` 旁加 `fn default_language() -> String { "auto".to_string() }`；`Default` impl 加 `language: default_language(),`。

- [ ] **Step 2B.2: 加 sys-locale 依赖**：`app/src-tauri/Cargo.toml` dependencies 加 `sys-locale = "0.3"`。lib.rs 加语言解析（放 Settings 附近）：

```rust
/// 解析生效语言：settings.language 为 zh/en 用之；auto 按系统 locale（zh* → zh，其余 en）。
fn ui_lang(settings: &Settings) -> &'static str {
    match settings.language.as_str() {
        "zh" => "zh",
        "en" => "en",
        _ => {
            if sys_locale::get_locale().map(|l| l.starts_with("zh")).unwrap_or(false) { "zh" } else { "en" }
        }
    }
}

/// Rust 侧用户可见文案（仅 11 条，不引 i18n 库）。
fn tr(lang: &str, key: &str) -> &'static str {
    match (lang, key) {
        ("en", "notify.error") => "Session error",
        ("en", "notify.waiting") => "Waiting for your reply",
        ("en", "tray.settings") => "Settings",
        ("en", "tray.quit") => "Quit",
        ("en", "window.settings") => "Settings",
        (_, "notify.error") => "会话出错",
        (_, "notify.waiting") => "等待你回复",
        (_, "tray.settings") => "设置",
        (_, "tray.quit") => "退出",
        (_, "window.settings") => "设置",
        _ => "",
    }
}
```

- [ ] **Step 2B.3: api.ts Settings 类型加 `language: LangSetting`**（`export type LangSetting = "auto" | "zh" | "en";` 放 api.ts，i18n/index.tsx 改为 re-export 以免循环依赖——执行时二选一保持单一定义）。About.tsx `SETTINGS_DEFAULTS` 加 `language: "auto"`。

- [ ] **Step 2B.4: GeneralSection 加语言行**（About.tsx，紧跟「桌面通知」行后）：

```tsx
        <div className="row">
          <div className="row-text">
            <div className="row-label">{t.settings.language}</div>
            <div className="row-desc">{t.settings.languageDesc}</div>
          </div>
          <Dropdown
            value={settings?.language ?? "auto"}
            options={[
              { value: "auto" as const, label: t.settings.langAuto },
              { value: "zh" as const, label: "中文" },
              { value: "en" as const, label: "English" },
            ]}
            onChange={(v) => patch({ language: v })}
          />
        </div>
```

（语言名"中文/English"用各自语言原文呈现，业界惯例，不进字典。）

- [ ] **Step 2B.5: 验证**：`cargo test --workspace` + `cd app && bun run test`。Commit：`feat(i18n): Settings 增加 language 字段（auto/zh/en）+ 设置页语言下拉`

### 2C. 前端文案全量替换

**Files:**
- Modify: `app/src/main.tsx`（Provider 包根）、`app/src/views/Sticker.tsx`、`app/src/views/About.tsx`、`app/src/App.tsx`
- Delete: `app/src/views/LiveView.tsx`、`LiveView.test.tsx`、`Overview.tsx`、`ProjectBoard.tsx`（及它们的测试，glob 确认）
- Modify: `app/src/views/Sticker.test.tsx`、`app/src/App.test.tsx` 等（断言改引字典）

- [ ] **Step 2C.1: main.tsx 用 `<I18nProvider>` 包住 `<About />` / `<App />`**。
- [ ] **Step 2C.2: Sticker.tsx 替换**：组件内 `const t = useT()`；TABS/EMPTY 常量移入组件或改造为接收 t 的函数；fmtAgo 改为 `fmtAgo(ms, t)`；ConnBadge/RunBadge 接 t；错误标签展示处用 `t.errorLabels[l.error_label] ?? l.error_label` 映射；`"(未命名会话)"` 比较保持原样（sentinel）。
- [ ] **Step 2C.3: About.tsx 替换**：HIDE_OPTIONS/THEME_OPTIONS/DENSITY_OPTIONS 等模块级常量改为组件内由 t 构造；fmtResetIn 改 `fmtResetIn(iso, t)`；全部 label/desc/title/aria 走 t。
- [ ] **Step 2C.4: App.tsx 更新条 3 条走 t**。
- [ ] **Step 2C.5: 删除遗留视图**及其测试、styles.css 中 `.needs` 死样式（styles.css:351-364）顺带删。api.ts 的 getOverview/getProjectTasks 保留（Rust 命令仍在）。
- [ ] **Step 2C.6: 测试更新**：测试 render 包 `<I18nProvider initial="zh">`；断言从硬编码中文改为 `zh.xxx` 引用（如 `expect(screen.getByText(zh.tabs.all))`）。注意 jsdom 的 navigator.language 是 en-US，不包 Provider 的测试会渲染英文——必须显式 initial="zh"。
- [ ] **Step 2C.7: 全量验证**：`bun run test` + `bun run build`（tsc 严格暴露漏改）。grep 自查：`rg "[一-龥]" app/src --glob '!**/*.test.*' --glob '!src/demo/**' --glob '!src/i18n/**'` 结果应只剩注释与 sentinel 比较。
- [ ] **Step 2C.8: Commit**：`feat(i18n): 前端文案全量接入字典；删除遗留死代码视图`

### 2D. Rust 侧消费 + 托盘重建

**Files:**
- Modify: `app/src-tauri/src/lib.rs`（通知 :1241,:1259、窗口标题 :1395、托盘 :1430-1433、set_settings :963-974）、`app/src-tauri/src/macos/menubar.rs:94-97`

- [ ] **Step 2D.1: 通知标题走 tr**。liveness 轮询里（lib.rs:1203 已 load_settings）改为取整个 settings：`let settings = load_settings(); let notify_on = settings.notifications_enabled; let lang = ui_lang(&settings);`；两处 `"会话出错".into()` / `"等待你回复".into()` 改 `tr(lang, "notify.error").into()` / `tr(lang, "notify.waiting").into()`。
- [ ] **Step 2D.2: 设置窗口标题**：`.title("设置")` 改 `.title(tr(ui_lang(&load_settings()), "window.settings"))`。
- [ ] **Step 2D.3: 托盘菜单文案 + 切语言重建**。两处 setup_tray 的 `"设置"`/`"退出"` 改走 tr；抽出 `fn build_tray_menu(app, lang) -> tauri::Result<Menu>`（Win/macOS 各自文件内）；`set_settings` 在 emit 后调用 `rebuild_tray_menu(&app, ui_lang(&settings))`——用 `app.tray_by_id("meowo-tray")` 拿托盘 `set_menu`。语言未变时跳过（比较旧值或无条件重建均可，菜单仅两项，无条件重建最简单）。
- [ ] **Step 2D.4: 验证**：`cargo clippy --workspace -- -D warnings` + `cargo test --workspace`。Windows 实机：切语言 → 托盘右键菜单立即变英文；通知在 5s 轮询后用新语言。
- [ ] **Step 2D.5: Commit**：`feat(i18n): Rust 侧通知/托盘/窗口标题双语，切语言实时重建托盘菜单`

### 2E. 收尾

- [ ] **Step 2E.1: 全套本地验证**：`node scripts/prepare-sidecar.mjs && cargo clippy --workspace -- -D warnings && cargo test --workspace && cd app && bun run test && bun run build`
- [ ] **Step 2E.2: 推分支开 PR**（标题 `feat: 界面与系统通知中英双语`，正文含变更摘要/测试计划），CI 绿后合并。

---

## 任务 3：重录 demo.gif（前两者合并 main 后，直接在 main）

- [ ] **Step 3.1**：`git checkout main && git pull`，确认含任务 1、2 的合并。
- [ ] **Step 3.2**：`cd app && bun run demo:gif`（内部 node 跑 Playwright，自起 vite:14210；记忆坑：必须 node、图像预览暗部发白是假象、RGB→RGBA 管线已内置）。
- [ ] **Step 3.3**：`node scripts/check-gif.mjs` 抽帧检查：收尾图标应为橙色；待交互 tab 应为举手；文案应为「已连接/刚刚」等统一中文（demo 固定中文环境——jsdom 之外真浏览器 navigator.language 取决于 CI/本机，若录出英文需在 demo mock 固定 zh：`app/src/demo/main.tsx` 的 Provider 传 `initial="zh"`，执行时确认）。
- [ ] **Step 3.4**：确认 GIF 体积量级不变（~2MB），`git add docs/images/demo.gif && git commit -m "docs(demo): 品牌色/举手图标/统一文案后重录演示 GIF" && git push origin main`。

---

## Self-Review 记录

- spec 覆盖：图标两处 ✓、i18n（前端 105 条 + Rust 11 条 + 语言设置 + 实时生效 + 三个坑）✓、GIF ✓
- 占位符：en.ts 的 settings/account/about 三段以「全量照译 + Dict 编译期保证无漏」交代，执行时必须写全——非 TBD，是体量裁剪
- 类型一致：`LangSetting` 单一定义（2B.3 注明二选一）；`useT()/t` 命名贯穿 2C/2D 一致
- demo 语言风险（Step 3.3）已显式标注处理方案

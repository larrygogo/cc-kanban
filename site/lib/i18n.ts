// 双语（中文默认在根路径 / ，英文在 /en）。静态导出，不用 Next 内置 i18n 路由。
// 页面正文各自内联对应语言；此处集中放导航/页脚/共享组件的字符串与路径助手。

export type Lang = "zh" | "en";
export const LOCALES: Lang[] = ["zh", "en"];
export const DEFAULT_LOCALE: Lang = "zh";

/** 从路径判断语言：/en 或 /en/... 为英文，其余为中文。 */
export function langFromPath(pathname: string): Lang {
  return pathname === "/en" || pathname.startsWith("/en/") ? "en" : "zh";
}

/** 给一个「中文根路径」（如 "/features"、"/"）套上语言前缀。 */
export function withLang(lang: Lang, path: string): string {
  const clean = path === "/" ? "" : path.replace(/\/$/, "");
  if (lang === "en") return `/en${clean}` || "/en";
  return clean || "/";
}

/** 把当前路径切换到目标语言的等价路径（保持子路径）。 */
export function switchLangPath(pathname: string, target: Lang): string {
  const current = langFromPath(pathname);
  if (current === target) return pathname;
  if (target === "en") {
    const rest = pathname === "/" ? "" : pathname.replace(/\/$/, "");
    return `/en${rest}` || "/en";
  }
  // en -> zh：去掉 /en 前缀
  const rest = pathname.replace(/^\/en/, "");
  return rest || "/";
}

type NavDict = {
  links: { path: string; label: string }[];
  download: string;
  github: string;
  menu: string;
  switchTo: string; // 语言切换按钮的 aria/title
};

type FooterDict = {
  tagline: string;
  cols: { title: string; links: { label: string; path?: string; href?: string }[] }[];
  license: string;
  tip: string;
};

type CtaDict = { title: string; subtitle: string; download: string; star: string };

type Feature = { title: string; body: string };

type ThemeDict = {
  eyebrowHome: string;
  headingHome: string;
  subHome: string;
  eyebrowFeat: string;
  headingFeat: string;
  subFeat: string;
  extra: string;
  color: string;
  style: string;
  theme: string;
  flat: string;
  emboss: string;
  dark: string;
  light: string;
  hint: string;
  swatches: Record<string, string>;
};

export type Dict = {
  htmlLang: string;
  nav: NavDict;
  footer: FooterDict;
  cta: CtaDict;
  // 与 components/FeatureGrid.tsx 的 ICONS 一一对应，共 8 张卡。
  features: Feature[];
  theme: ThemeDict;
  sticker: {
    tabs: { all: string; waiting: string; running: string; archived: string };
    quota5h: string;
    quota7d: string;
    ai: string;
    you: string;
    justNow: string;
    menu: { star: string; note: string; rename: string; archive: string; newSession: string; openDir: string };
  };
  featuresMore: string; // 首页「查看全部功能」
};

const ZH: Dict = {
  htmlLang: "zh-CN",
  nav: {
    links: [
      { path: "/features", label: "功能" },
      { path: "/download", label: "下载" },
      { path: "/docs", label: "文档" },
      { path: "/changelog", label: "更新日志" },
      { path: "/faq", label: "FAQ" },
    ],
    download: "下载",
    github: "GitHub",
    menu: "菜单",
    switchTo: "English",
  },
  footer: {
    tagline: "本地优先的 AI 编程代理桌面工作台。展开是桌面贴纸，收起是电子红绿灯。少切终端，少输命令。",
    cols: [
      {
        title: "产品",
        links: [
          { label: "功能", path: "/features" },
          { label: "下载", path: "/download" },
          { label: "更新日志", path: "/changelog" },
        ],
      },
      {
        title: "资源",
        links: [
          { label: "文档", path: "/docs" },
          { label: "FAQ", path: "/faq" },
          { label: "Releases", href: "releases" },
        ],
      },
      {
        title: "项目",
        links: [
          { label: "GitHub", href: "repo" },
          { label: "License", href: "license" },
        ],
      },
    ],
    license: "MIT © larrygogo",
    tip: "名字来自猫叫 meow，中文译作「喵呜」🐱",
  },
  cta: {
    title: "把多开 AI 编程，收进桌面一角",
    subtitle: "少切终端，少输命令。每个会话的状态、配额与待办，一切尽在计划之中。",
    download: "下载最新版",
    star: "在 GitHub 上 Star",
  },
  features: [
    { title: "会话看板", body: "项目名、会话标题、最近一条 AI 输出、连接状态，一张卡看完。能读 Context 的 Agent 还会带上上下文已用百分比——哪个会话快撑爆上下文，扫一眼卡片就知道。" },
    { title: "点击直达终端 tab", body: "点卡片就切到会话所在的终端标签页：Windows 上是 Windows Terminal，macOS 上是 Terminal 或 iTerm2。会话已断开时自动回到原目录、按各工具自己的方式续接——不用查会话 ID，也不用记续接参数。" },
    { title: "Agent 待办清单", body: "Agent 拆解任务后，对话窗口实时显示它的待办清单：正在做哪一条、4 条里完成了几条，随 hook 落库跟着刷新——不用翻终端输出找进度。" },
    { title: "审批与菜单按钮化", body: "等批准的命令、弹出的选择菜单，在对话窗口里直接渲染成按钮：允许 / 拒绝、信任确认、长会话恢复、模型选择，点一下就把对应按键发回终端——不用敲键盘应答。" },
    { title: "多账号 + API 中转", body: "同一个工具保存多个官方账号，一键切换，凭据与会话历史各自独立；也可以按模型填入中转地址与密钥接入 API 中转。两种接入互斥，配置中转期间仍走官方账号。" },
    { title: "本地优先", body: "会话与设置只写进本机 ~/.meowo/ 的 SQLite，reporter 与 app 只通过这个本地文件通信。没有自己的服务器，不上传会话内容，断网也照常工作。" },
    { title: "一键安装、登录与接入", body: "Claude Code、Codex、Kimi、Gemini CLI、OpenCode 直接在应用内一键安装并发起登录，所需 hooks 自动接入；检测到连接缺失时，再点一下就修好。" },
    { title: "用量与上下文监控", body: "贴纸底栏实时显示 5 小时 / 7 天配额的使用比例，越接近上限颜色越偏红；每张卡片显示会话上下文用量。限额将至你有预判，不会被突然中断打个措手不及。" },
  ],
  sticker: {
    tabs: { all: "全部", waiting: "待交互", running: "运行中", archived: "已归档" },
    quota5h: "5 小时配额",
    quota7d: "7 天配额",
    ai: "AI",
    you: "你",
    justNow: "刚刚",
    menu: { star: "置顶", note: "添加便签", rename: "重命名", archive: "归档", newSession: "新建会话", openDir: "打开项目目录" },
  },
  theme: {
    eyebrowHome: "你的桌面，你说了算",
    headingHome: "多种风格与配色，随手切换",
    subHome: "下面这块就是活的，点点看它怎么变。",
    eyebrowFeat: "外观",
    headingFeat: "多种风格与配色，随手切换",
    subFeat: "7 种贴纸配色、扁平与立体两种风格、深浅主题——点点下面这块试试。",
    extra: "另外还能调整不透明度（配合系统毛玻璃透出桌面）与界面密度。",
    color: "配色",
    style: "风格",
    theme: "明暗",
    flat: "扁平",
    emboss: "立体",
    dark: "深色",
    light: "浅色",
    hint: "7 种配色 · 扁平 / 立体 · 深 / 浅 · 还能调透明度与界面密度，随手换一套。",
    swatches: { neutral: "无色", classic: "经典", slate: "石青", moss: "苔绿", plum: "暮紫", rose: "玫粉", amber: "琥珀" },
  },
  featuresMore: "查看全部功能",
};

const EN: Dict = {
  htmlLang: "en",
  nav: {
    links: [
      { path: "/features", label: "Features" },
      { path: "/download", label: "Download" },
      { path: "/docs", label: "Docs" },
      { path: "/changelog", label: "Changelog" },
      { path: "/faq", label: "FAQ" },
    ],
    download: "Download",
    github: "GitHub",
    menu: "Menu",
    switchTo: "中文",
  },
  footer: {
    tagline: "A local-first desktop workbench for AI coding agents. A sticker when expanded, an electronic traffic light when collapsed. Fewer terminal switches, fewer commands.",
    cols: [
      {
        title: "Product",
        links: [
          { label: "Features", path: "/features" },
          { label: "Download", path: "/download" },
          { label: "Changelog", path: "/changelog" },
        ],
      },
      {
        title: "Resources",
        links: [
          { label: "Docs", path: "/docs" },
          { label: "FAQ", path: "/faq" },
          { label: "Releases", href: "releases" },
        ],
      },
      {
        title: "Project",
        links: [
          { label: "GitHub", href: "repo" },
          { label: "License", href: "license" },
        ],
      },
    ],
    license: "MIT © larrygogo",
    tip: "Named after a cat's “meow”, written 喵呜 in Chinese 🐱",
  },
  cta: {
    title: "Tuck your parallel AI coding into a corner of your desktop",
    subtitle: "Fewer terminal switches, fewer commands. Every session's status, quota, and to-dos — all under control.",
    download: "Download latest",
    star: "Star on GitHub",
  },
  features: [
    { title: "Session board", body: "Project, session title, the latest AI output, connection state — one card tells it all. Agents that expose Context also show how full the context window is, so you know which session is about to burst." },
    { title: "Click to jump to the terminal tab", body: "Click a card and you land in its terminal tab — Windows Terminal on Windows, Terminal or iTerm2 on macOS. Disconnected? Meowo returns to the project directory and resumes the session the tool's own way — no IDs or resume flags to remember." },
    { title: "Agent todo list", body: "When the agent breaks a task into steps, the chat window shows its todo list live: the item in progress and how many of the list are done, refreshed as hooks land. No scrolling terminal output to find the progress." },
    { title: "Approvals & menus as buttons", body: "Commands awaiting approval and popup menus render as buttons in the chat window: allow / deny, trust prompts, long-session resume, model pickers. One click sends the matching keys back to the terminal — no keyboard replies." },
    { title: "Multiple accounts + API relay", body: "Keep several official accounts per tool, switch in one click, each with its own credentials and history. Or fill in a relay address and key per model to use an API relay — mutually exclusive, and the official account keeps serving while you configure." },
    { title: "Local-first", body: "Sessions and settings live only in ~/.meowo/ as local SQLite; the reporter and the app talk only through that file. No Meowo server, no uploaded conversations — works just fine offline." },
    { title: "One-click install, sign-in & connect", body: "Install Claude Code, Codex, Kimi, Gemini CLI, or OpenCode right in the app and start the login; the required hooks are wired automatically. Detect a broken connection? One more click repairs it." },
    { title: "Quota & context monitoring", body: "The sticker's bottom bar tracks 5-hour / 7-day quota live, turning redder near the cap; every card shows context usage. You see limits coming — no sudden interruption catching you off guard." },
  ],
  sticker: {
    tabs: { all: "All", waiting: "Needs you", running: "Running", archived: "Archived" },
    quota5h: "5-hr",
    quota7d: "7-day",
    ai: "AI",
    you: "You",
    justNow: "just now",
    menu: { star: "Star to pin", note: "Add note", rename: "Rename", archive: "Archive", newSession: "New session", openDir: "Open project dir" },
  },
  theme: {
    eyebrowHome: "Your desktop, your call",
    headingHome: "Styles and colors, switched on a whim",
    subHome: "The block below is live — click around and watch it change.",
    eyebrowFeat: "Appearance",
    headingFeat: "Styles and colors, switched on a whim",
    subFeat: "7 sticker colors, flat or dimensional styles, dark or light — try the live block below.",
    extra: "You can also tune opacity (to let the frosted desktop show through) and UI density.",
    color: "Color",
    style: "Style",
    theme: "Theme",
    flat: "Flat",
    emboss: "3D",
    dark: "Dark",
    light: "Light",
    hint: "7 colors · Flat / 3D · Dark / Light · plus opacity and UI density — swap a whole set anytime.",
    swatches: { neutral: "None", classic: "Classic", slate: "Slate", moss: "Moss", plum: "Plum", rose: "Rose", amber: "Amber" },
  },
  featuresMore: "See all features",
};

const DICTS: Record<Lang, Dict> = { zh: ZH, en: EN };

export function getDict(lang: Lang): Dict {
  return DICTS[lang] ?? ZH;
}

import Link from "next/link";
import { REPO } from "@/lib/site";
import { getLatestRelease } from "@/lib/release";
import { getDict, withLang, type Lang } from "@/lib/i18n";
import { ArrowRightIcon, GitHubIcon } from "@/components/icons";
import DownloadButton from "@/components/DownloadButton";
import CheckItem from "@/components/CheckItem";
import FeatureGrid from "@/components/FeatureGrid";
import Reveal from "@/components/Reveal";
import CtaBand from "@/components/CtaBand";
import DemoFrame from "@/components/DemoFrame";
import ThemeShowcase from "@/components/ThemeShowcase";
import { ChatWindowMock } from "@/components/screenshots";

type SemTone = "err" | "warn" | "ok";

type Content = {
  hero: { eyebrow: string; h1a: string; accent: string; lead: string; github: string; note: string };
  sem: { tone: SemTone; label: string; text: string }[]; // 三色状态语义条：红 / 黄 / 绿
  chat: { eyebrow: string; title: string; body: string; checks: string[] };
  featGridHead: { eyebrow: string; title: string };
  platHead: { eyebrow: string; title: string };
  win: { title: string; body: string; checks: string[] };
  mac: { title: string; body: string; checks: string[] };
};

const CONTENT: Record<Lang, Content> = {
  zh: {
    hero: {
      eyebrow: "// AI 编程代理工作台",
      h1a: "多开 AI 编程，",
      accent: "一切尽在计划之中",
      lead: "本地优先的 AI 编程代理工作台。展开是桌面贴纸，收起是电子红绿灯，点一下直达对应终端。",
      github: "GitHub",
      note: "开源 · MIT · Windows 与 macOS · 无需预装 AI CLI",
    },
    sem: [
      { tone: "err", label: "ERROR · 报错", text: "有会话出错停住了，红色第一时间跳出来，不会被其他窗口盖过去。" },
      { tone: "warn", label: "WAITING · 待交互", text: "有会话在等你确认或回复，黄色提醒，按等待时长排好序。" },
      { tone: "ok", label: "RUNNING · 运行中", text: "其余会话正常推进，绿色表示不用管，安心做手头的事。" },
    ],
    chat: {
      eyebrow: "对话窗口",
      title: "打开卡片，是完整的结构化对话",
      body: "点开任意一张会话卡片，就进入这个会话的对话窗口：消息、工具调用、子任务各就各位，不用在终端滚屏里翻找。等批准的命令、弹出的选择菜单，都变成可以直接点的按钮。",
      checks: [
        "工具调用折叠成一行摘要，展开看完整输入输出；子任务有独立时间线",
        "Agent 拆解任务后，待办清单实时显示在对话里，进度跟着刷新",
        "信任确认、长会话恢复、模型选择等终端菜单，直接渲染成按钮",
        "命令审批卡在输入区上方，批准 / 拒绝一点即发",
        "斜杠命令带补全，模型与协作模式在输入区直接切换",
        "对话与终端双视图并排，随时切到底层终端亲自操作",
      ],
    },
    featGridHead: { eyebrow: "完整工作流", title: "从会话到环境，一处管理" },
    platHead: { eyebrow: "平台", title: "Windows 和 macOS，各自顺手" },
    win: {
      title: "Windows · 桌面贴纸",
      body: "钉在桌面一角的常驻贴纸，拖到屏幕边缘就收成一条红绿灯。",
      checks: [
        "拖到屏幕左 / 右 / 顶边缩成状态条，鼠标悬停展开",
        "可以置顶；重启后沿用上次的窗口位置和吸附边",
        "鼠标停在托盘图标上，能看到待交互和运行中的会话数",
      ],
    },
    mac: {
      title: "macOS · 菜单栏面板",
      body: "菜单栏图标弹出的原生面板，失焦自动收起，不占 Dock。",
      checks: [
        "左键点图标弹出面板，失焦自动收起",
        "菜单栏图标上显示运行中和待交互的会话数",
        "Apple 芯片与 Intel 各自独立包，已签名公证，双击打开",
      ],
    },
  },
  en: {
    hero: {
      eyebrow: "// AI coding agent workbench",
      h1a: "Run AI coding in parallel,",
      accent: "all under control",
      lead: "A local-first workbench for AI coding agents. A sticker when expanded, an electronic traffic light when collapsed — one click jumps to the terminal.",
      github: "GitHub",
      note: "Open source · MIT · Windows & macOS · no AI CLI needed upfront",
    },
    sem: [
      { tone: "err", label: "ERROR", text: "A session errored out and stopped — red jumps out first, never buried under other windows." },
      { tone: "warn", label: "WAITING", text: "A session is waiting for your confirmation or reply — amber reminds you, sorted by wait time." },
      { tone: "ok", label: "RUNNING", text: "Everything else is making progress — green means leave it alone and focus on your work." },
    ],
    chat: {
      eyebrow: "Chat window",
      title: "Open a card for the full structured conversation",
      body: "Open any session card and you're in that session's chat window: messages, tool calls and subagents all in place — no scrolling terminal output. Commands awaiting approval and popup menus become buttons you can just click.",
      checks: [
        "Tool calls fold into one-line summaries; expand for full I/O — subagents get their own timeline",
        "When the agent breaks a task down, its todo list shows live in the conversation",
        "Trust prompts, long-session resume and model pickers render as buttons",
        "Approval cards sit above the composer — approve or deny in one click",
        "Slash commands come with completion; model and mode switch right in the composer",
        "Chat and terminal views side by side — drop into the raw terminal anytime",
      ],
    },
    featGridHead: { eyebrow: "Full workflow", title: "From sessions to environment, managed in one place" },
    platHead: { eyebrow: "Platforms", title: "Feels native on Windows and macOS" },
    win: {
      title: "Windows · Desktop sticker",
      body: "A resident sticker pinned to a desktop corner; drag it to a screen edge and it folds into a traffic light.",
      checks: [
        "Drag to the left / right / top edge to shrink into a status strip; hover to expand",
        "Pin on top; remembers last position and snap edge across restarts",
        "Hover the tray icon to see needs-you and running counts",
      ],
    },
    mac: {
      title: "macOS · Menu-bar panel",
      body: "A native panel from the menu-bar icon that auto-hides on blur — no Dock space taken.",
      checks: [
        "Left-click the icon for the panel; it hides when it loses focus",
        "The menu-bar icon shows running and needs-you counts",
        "Separate Apple silicon and Intel builds, signed & notarized, double-click to open",
      ],
    },
  },
};

export default async function HomeContent({ lang }: { lang: Lang }) {
  const release = await getLatestRelease();
  const d = getDict(lang);
  const c = CONTENT[lang];

  return (
    <main>
      {/* Hero：mono eyebrow + display + 双 CTA + 贴纸舞台 */}
      <section className="hero">
        <div className="container">
          <span className="eyebrow">{c.hero.eyebrow}</span>
          <h1 className="h-display">
            {c.hero.h1a}
            <br />
            <span className="acc">{c.hero.accent}</span>
          </h1>
          <p className="lead">{c.hero.lead}</p>
          <div className="hero-cta">
            <DownloadButton
              lang={lang}
              windows={release?.windows ?? null}
              macosArm={release?.macosArm ?? null}
              macosIntel={release?.macosIntel ?? null}
              fallbackHref={withLang(lang, "/download")}
              className="btn btn-primary btn-lg"
            />
            <a className="btn btn-ghost btn-lg" href={REPO} target="_blank" rel="noopener noreferrer">
              <GitHubIcon />
              {c.hero.github}
            </a>
          </div>
          <p className="hero-note">
            {c.hero.note}
            {release ? ` · ${release.tag}` : ""}
          </p>
          <div className="hero-stage">
            <div className="stage">
              {/* 实时演示：iframe 里是真实贴纸 UI 的录屏动画（见 DemoFrame） */}
              <DemoFrame lang={lang} />
            </div>
          </div>
        </div>
      </section>

      {/* 三色状态语义条 */}
      <section className="section-sm section-sunken">
        <div className="container">
          <div className="sem-strip">
            {c.sem.map((s) => (
              <Reveal key={s.label}>
                <div className={`sem sem-${s.tone}`}>
                  <span className="sem-tag">
                    <span className="sem-dot" />
                    {s.label}
                  </span>
                  <p>{s.text}</p>
                </div>
              </Reveal>
            ))}
          </div>
        </div>
      </section>

      {/* 对话窗口 */}
      <section className="section">
        <div className="container">
          <div className="split">
            <div className="split-text">
              <span className="eyebrow">{c.chat.eyebrow}</span>
              <h2 className="h2">{c.chat.title}</h2>
              <p className="lead">{c.chat.body}</p>
              <ul className="checklist">
                {c.chat.checks.map((x) => (
                  <CheckItem key={x}>{x}</CheckItem>
                ))}
              </ul>
            </div>
            <div className="split-media">
              <ChatWindowMock lang={lang} />
            </div>
          </div>
        </div>
      </section>

      {/* 特性网格 */}
      <section className="section section-sunken">
        <div className="container">
          <div className="section-head">
            <span className="eyebrow">{c.featGridHead.eyebrow}</span>
            <h2 className="h1">{c.featGridHead.title}</h2>
          </div>
          <FeatureGrid lang={lang} />
          <div className="center-cta">
            <Link className="btn btn-ghost" href={withLang(lang, "/features")}>
              {d.featuresMore} <ArrowRightIcon />
            </Link>
          </div>
        </div>
      </section>

      {/* 主题互动块：配色/风格/明暗实时切换的活贴纸 */}
      <section className="section">
        <div className="container">
          <div className="section-head">
            <span className="eyebrow">{d.theme.eyebrowHome}</span>
            <h2 className="h1">{d.theme.headingHome}</h2>
            <p className="lead">{d.theme.subHome}</p>
          </div>
          <ThemeShowcase lang={lang} />
          <p className="note-center">{d.theme.extra}</p>
        </div>
      </section>

      {/* 平台 */}
      <section className="section">
        <div className="container">
          <div className="section-head">
            <span className="eyebrow">{c.platHead.eyebrow}</span>
            <h2 className="h1">{c.platHead.title}</h2>
          </div>
          <div className="grid grid-2">
            {[c.win, c.mac].map((pl) => (
              <Reveal key={pl.title}>
                <div className="fcard plat-card">
                  <h3>{pl.title}</h3>
                  <p>{pl.body}</p>
                  <ul className="checklist">
                    {pl.checks.map((x) => (
                      <CheckItem key={x}>{x}</CheckItem>
                    ))}
                  </ul>
                </div>
              </Reveal>
            ))}
          </div>
        </div>
      </section>

      <CtaBand lang={lang} />
    </main>
  );
}

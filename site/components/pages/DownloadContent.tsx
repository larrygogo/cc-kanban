import { RELEASE_LATEST, RELEASES } from "@/lib/site";
import { getLatestRelease, formatSize, type Asset } from "@/lib/release";
import { AppleIcon, WindowsIcon, DownloadIcon } from "@/components/icons";
import Reveal from "@/components/Reveal";
import { type Lang } from "@/lib/i18n";

type Content = {
  eyebrow: string;
  title: string;
  leadVersion: (tag: string) => string;
  leadRest: string;
  win: { meta: string; btn: string; fallback: string };
  mac: { meta: string; btn: string; fallback: string };
  allVersions: string;
  stepsEyebrow: string;
  stepsTitle: string;
  steps: { title: string; body: string }[];
  reqsEyebrow: string;
  reqsTitle: string;
  reqs: { k: string; v: string }[];
  macNote: string;
  winNote: string;
};

const CONTENT: Record<Lang, Content> = {
  zh: {
    eyebrow: "下载",
    title: "下载 Meowo",
    leadVersion: (tag) => `最新版本 ${tag}。`,
    leadRest: "开源，MIT 许可。无需提前配置 AI CLI：安装 Meowo 后，可直接在应用内一键安装、登录并自动接入。",
    win: { meta: "x64 NSIS 安装包 · Windows 10 / 11", btn: "下载 .exe", fallback: "在 GitHub Releases 里选 x64-setup.exe" },
    mac: { meta: "universal DMG · Intel / Apple Silicon 通用 · ≥ Sonoma", btn: "下载 .dmg", fallback: "在 GitHub Releases 里选 universal.dmg" },
    allVersions: "查看全部历史版本",
    stepsEyebrow: "安装",
    stepsTitle: "安装步骤",
    steps: [
      { title: "下载安装包", body: "在 GitHub Releases 里选择对应平台的安装包。" },
      { title: "安装并打开", body: "Windows 双击 setup.exe。macOS 把应用拖进「应用程序」，双击打开。" },
      { title: "一键安装并登录 AI CLI", body: "没有 Claude Code、Codex、Kimi、Gemini CLI 或 OpenCode 也没关系：在设置里选择工具，一键安装并发起账号登录。已有工具会自动检测。" },
      { title: "自动接入并开始使用", body: "Meowo 自动接入所需 hooks。选择项目目录和 AI 工具，点「启动」即可新建会话。会话状态就会出现在桌面一角的贴纸上。" },
    ],
    reqsEyebrow: "环境",
    reqsTitle: "环境要求",
    reqs: [
      { k: "操作系统", v: "Windows 10 / 11，或 macOS 14 Sonoma 及以上" },
      { k: "AI 编程 CLI", v: "无需预装；可在 Meowo 内一键安装并登录 Claude Code / Codex / Kimi / Gemini CLI / OpenCode 其一，用于产生会话事件" },
      { k: "Windows 依赖", v: "WebView2 Runtime（Windows 11 自带），安装包会按需安装" },
      { k: "磁盘占用", v: "几十 MB。数据存在 ~/.meowo，本地 SQLite。" },
    ],
    macNote: "macOS 上首次点「跳转 / 恢复终端」会触发系统的「自动化」授权，允许 Meowo 控制 Terminal / iTerm2 即可。",
    winNote: "Windows 首次运行安装包会提示「Windows 已保护你的电脑」——Meowo 暂未做代码签名，点「更多信息 → 仍要运行」即可继续。我们已向 SignPath Foundation 申请面向开源项目的免费代码签名，通过后 Windows 安装包将由 SignPath Foundation 签名，这个提示会消失。",
  },
  en: {
    eyebrow: "Download",
    title: "Download Meowo",
    leadVersion: (tag) => `Latest version ${tag}. `,
    leadRest: "Open source, MIT. No need to set up an AI CLI in advance: after installing Meowo, install, sign in, and connect right in the app.",
    win: { meta: "x64 NSIS installer · Windows 10 / 11", btn: "Download .exe", fallback: "Pick x64-setup.exe in GitHub Releases" },
    mac: { meta: "Universal DMG · Intel / Apple Silicon · ≥ Sonoma", btn: "Download .dmg", fallback: "Pick universal.dmg in GitHub Releases" },
    allVersions: "See all past releases",
    stepsEyebrow: "Install",
    stepsTitle: "Install steps",
    steps: [
      { title: "Download the installer", body: "Pick the installer for your platform in GitHub Releases." },
      { title: "Install and open", body: "On Windows, double-click setup.exe. On macOS, drag the app to Applications and double-click." },
      { title: "One-click install & sign in to an AI CLI", body: "No Claude Code, Codex, Kimi, Gemini CLI, or OpenCode yet? In Settings, pick a tool and install & sign in with one click. Existing tools are detected automatically." },
      { title: "Auto-connect and start", body: "Meowo wires up the required hooks. Pick a project directory and AI tool, click Launch to start a session — its status shows up on the sticker in a corner of your desktop." },
    ],
    reqsEyebrow: "Requirements",
    reqsTitle: "System requirements",
    reqs: [
      { k: "Operating system", v: "Windows 10 / 11, or macOS 14 Sonoma and later" },
      { k: "AI coding CLI", v: "None required upfront; install & sign in to one of Claude Code / Codex / Kimi / Gemini CLI / OpenCode inside Meowo to produce session events" },
      { k: "Windows dependency", v: "WebView2 Runtime (bundled on Windows 11); the installer adds it as needed" },
      { k: "Disk", v: "Tens of MB. Data lives in ~/.meowo, a local SQLite database." },
    ],
    macNote: "On macOS, the first “jump / resume terminal” triggers the system Automation permission — just allow Meowo to control Terminal / iTerm2.",
    winNote: "On Windows the installer shows “Windows protected your PC” — Meowo isn't code-signed yet. Choose “More info → Run anyway” to continue. We have applied to the SignPath Foundation for free code signing for open source projects; once approved, Windows installers will be signed by the SignPath Foundation and this prompt will go away.",
  },
};

function assetSub(asset: Asset | null, fallback: string) {
  return asset ? `${asset.name} · ${formatSize(asset.size)}` : fallback;
}

export default async function DownloadContent({ lang }: { lang: Lang }) {
  const release = await getLatestRelease();
  const c = CONTENT[lang];

  return (
    <main>
      <section className="pagehead">
        <div className="container">
          <span className="eyebrow">{c.eyebrow}</span>
          <h1 className="h1">{c.title}</h1>
          <p className="lead">
            {release ? c.leadVersion(release.tag) : ""}
            {c.leadRest}
          </p>
        </div>
      </section>

      <section className="section-sm">
        <div className="container">
          <div className="platforms">
            <Reveal>
              <div className="platform">
                <div className="platform-top">
                  <WindowsIcon />
                  <h3>Windows</h3>
                </div>
                <p className="meta">{c.win.meta}</p>
                <a className="btn btn-primary btn-lg" href={release?.windows?.url ?? RELEASE_LATEST}>
                  <DownloadIcon />
                  {c.win.btn}
                </a>
                <p className="sub">{assetSub(release?.windows ?? null, c.win.fallback)}</p>
              </div>
            </Reveal>
            <Reveal>
              <div className="platform">
                <div className="platform-top">
                  <AppleIcon />
                  <h3>macOS</h3>
                </div>
                <p className="meta">{c.mac.meta}</p>
                <a className="btn btn-primary btn-lg" href={release?.macos?.url ?? RELEASE_LATEST}>
                  <DownloadIcon />
                  {c.mac.btn}
                </a>
                <p className="sub">{assetSub(release?.macos ?? null, c.mac.fallback)}</p>
              </div>
            </Reveal>
          </div>
          <p className="more-row">
            <a className="btn btn-ghost" href={RELEASES} target="_blank" rel="noopener noreferrer">
              {c.allVersions}
            </a>
          </p>
        </div>
      </section>

      <section className="section">
        <div className="container">
          <div className="section-head">
            <span className="eyebrow">{c.stepsEyebrow}</span>
            <h2 className="h1">{c.stepsTitle}</h2>
          </div>
          <div className="steps">
            {c.steps.map((s, i) => (
              <Reveal key={s.title}>
                <div className="step">
                  <span className="sn">{i + 1}</span>
                  <h3>{s.title}</h3>
                  <p>{s.body}</p>
                </div>
              </Reveal>
            ))}
          </div>
        </div>
      </section>

      <section className="section section-sunken">
        <div className="container container-narrow">
          <div className="section-head">
            <span className="eyebrow">{c.reqsEyebrow}</span>
            <h2 className="h1">{c.reqsTitle}</h2>
          </div>
          <div className="rows">
            {c.reqs.map((r) => (
              <div className="row" key={r.k}>
                <div className="k">{r.k}</div>
                <div className="v">{r.v}</div>
              </div>
            ))}
          </div>
          <p className="faint note-center">{c.winNote}</p>
          <p className="faint note-center">{c.macNote}</p>
        </div>
      </section>
    </main>
  );
}

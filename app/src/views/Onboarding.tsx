import { Fragment, useEffect, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useShowWhenReady } from "../useShowWhenReady";
import { languageOptions, useT } from "../i18n";
import { availableTerminals, type ResumeTerminal, type StickerStyle, type CardMenuMode, type ThemeMode } from "../api";
import { Segmented } from "./settings/widgets";
import { Dropdown } from "./menu";
import { useSettingsState } from "./settings/state";
import logoUrl from "../../src-tauri/icons/128x128.png";
import { useEscClose } from "../hooks/useEscClose";

type Dict = ReturnType<typeof useT>;

// 平台判定与真实设置页一致（WKWebView 的 UA 含 "Mac"）。默认终端选项按平台给，再用后端探测过滤。
const IS_MAC = typeof navigator !== "undefined" && /Mac/i.test(navigator.userAgent);
const IS_WIN = typeof navigator !== "undefined" && /Win/i.test(navigator.userAgent);
const RESUME_TERM_OPTIONS_MAC: { value: ResumeTerminal; label: string }[] = [
  { value: "terminal", label: "Terminal" },
  { value: "iterm", label: "iTerm2" },
  { value: "ghostty", label: "Ghostty" },
];
const resumeTermOptionsWin = (t: Dict): { value: ResumeTerminal; label: string }[] => [
  { value: "wt", label: "Windows Terminal" },
  { value: "wezterm", label: "WezTerm" },
  { value: "powershell", label: "PowerShell" },
  { value: "cmd", label: t.settings.cmdPrompt },
];

// ── 每步的「迷你界面示意图」：用 div 复刻贴纸真实观感（状态色、卡片、tab、底栏、菜单）。

function MiniCard({ variant, wide }: { variant: "run" | "wait" | "active" | "off"; wide?: boolean }) {
  return (
    <div className="obm-card">
      <span className={"obm-dot obm-" + variant} />
      <div className="obm-lines">
        <i className="obm-l1" />
        <i className={"obm-l2" + (wide ? " wide" : "")} />
      </div>
    </div>
  );
}

function HeroWelcome() {
  return (
    <div className="obm-panel">
      <div className="obm-bar">
        <img src={logoUrl} width={20} height={20} alt="" className="obm-logo" />
        <b>Meowo</b>
      </div>
      <MiniCard variant="run" wide />
      <MiniCard variant="active" />
    </div>
  );
}

function HeroBoard({ t }: { t: Dict }) {
  return (
    <div className="obm-panel">
      <div className="obm-tabs">
        <b className="on">{t.tabs.all}</b>
        <b>{t.tabs.waiting}</b>
        <b>{t.tabs.running}</b>
      </div>
      <div className="obm-card">
        <span className="obm-dot obm-run" />
        <div className="obm-lines">
          <i className="obm-l1" />
          <div className="obm-bar-ctx">
            <span style={{ width: "62%" }} />
          </div>
        </div>
      </div>
      <MiniCard variant="wait" />
      <MiniCard variant="off" />
    </div>
  );
}

// 卡片菜单：一张卡片右上角「⋯」按钮高亮 + 展开的操作菜单。项与图标对齐真实 CardContextMenu：
// 置顶 / 便签 / 改名 / 归档 —分隔线— 新建会话 / 打开目录，共 6 项。
const CARD_MENU_ICONS: ReactNode[] = [
  // 置顶：与 CardContextMenu 的 TopIcon 同形（arrow-up-to-line）。这里画星星的话，
  // 新手在引导里认的图标和真实菜单里的对不上。
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M5 3h14" /><path d="m18 13-6-6-6 6" /><path d="M12 7v14" /></svg>,
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M16 3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h11l5-5V5a2 2 0 0 0-2-2z" /><path d="M15 21v-5a1 1 0 0 1 1-1h5" /></svg>,
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M12 20h9" /><path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4Z" /></svg>,
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><rect width="20" height="5" x="2" y="3" rx="1" /><path d="M4 8v11a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8" /><path d="M10 12h4" /></svg>,
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M12 5v14M5 12h14" /></svg>,
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="m6 14 1.5-2.9A2 2 0 0 1 9.24 10H20a2 2 0 0 1 1.94 2.5l-1.54 6a2 2 0 0 1-1.95 1.5H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3.9a2 2 0 0 1 1.69.9l.81 1.2a2 2 0 0 0 1.67.9H18a2 2 0 0 1 2 2v2" /></svg>,
];

function HeroCardMenu({ t }: { t: Dict }) {
  const labels = [
    t.sticker.star,
    t.sticker.noteAdd,
    t.sticker.renameTitle,
    t.sticker.archive,
    t.sticker.newSession,
    t.sticker.openProjectDir,
  ];
  return (
    <div className="obm-cardmenu">
      <div className="obm-card obm-card-menu">
        <span className="obm-dot obm-active" />
        <div className="obm-lines">
          <i className="obm-l1" />
          <i className="obm-l2" />
        </div>
        <span className="obm-menubtn" aria-hidden>
          <svg width="13" height="13" viewBox="0 0 24 24" fill="currentColor"><circle cx="5" cy="12" r="1.7" /><circle cx="12" cy="12" r="1.7" /><circle cx="19" cy="12" r="1.7" /></svg>
        </span>
      </div>
      <div className="obm-pop">
        {labels.map((label, i) => (
          <Fragment key={i}>
            {/* 真实菜单里「新建会话/打开目录」上方有分隔线 */}
            {i === 4 && <div className="obm-pop-sep" />}
            <span className="obm-pop-item">
              <span className="obm-pop-ico">{CARD_MENU_ICONS[i]}</span>
              {label}
            </span>
          </Fragment>
        ))}
      </div>
    </div>
  );
}

function HeroTerminal() {
  return (
    <div className="obm-jump">
      <div className="obm-toast">
        <span className="obm-dot obm-wait" />
        <i />
      </div>
      <div className="obm-jump-row">
        <div className="obm-card obm-card-click">
          <span className="obm-dot obm-active" />
          <div className="obm-lines">
            <i className="obm-l1" />
            <i className="obm-l2" />
          </div>
          <svg className="obm-cursor" width="16" height="16" viewBox="0 0 24 24" fill="currentColor" aria-hidden>
            <path d="M5 3l14 8-6 1.5L9.5 18z" />
          </svg>
        </div>
        <svg className="obm-arrow" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
          <line x1="4" y1="12" x2="18" y2="12" />
          <polyline points="12 6 18 12 12 18" />
        </svg>
        <div className="obm-term">
          <span className="obm-term-dots"><i /><i /><i /></span>
          <code>$ claude --resume</code>
        </div>
      </div>
    </div>
  );
}

function HeroWindow() {
  return (
    <div className="obm-desktop">
      <div className="obm-tray">
        <span className="obm-tray-ico">
          <img src={logoUrl} width={16} height={16} alt="" />
        </span>
        <span className="obm-tray-badge">2</span>
      </div>
      <div className="obm-panel obm-panel-pinned">
        <MiniCard variant="run" wide />
        <MiniCard variant="wait" />
        {/* 底栏操作条：左侧用量读数 + 右侧动作图标（新建/搜索/设置/置顶），与真实贴纸一致；
            置顶图钉在最右、用高亮底片突出——它在底栏右下角，不在顶栏 */}
        <div className="obm-footbar">
          <span className="obm-usage">
            <i className="obm-uchip" />
            <i className="obm-uchip" />
            <span className="obm-ubar"><span /></span>
          </span>
          <span className="obm-acts">
            <i className="obm-act">+</i>
            <i className="obm-act obm-act-search" />
            {/* 第三个是设置齿轮：与真实贴纸同一枚 lucide settings 图标 */}
            <span className="obm-act" aria-hidden>
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.9" strokeLinecap="round" strokeLinejoin="round">
                <circle cx="12" cy="12" r="3" />
                <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
              </svg>
            </span>
            <span className="obm-act obm-act-pin" aria-hidden>
              <svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <path d="M12 17v5" />
                <path d="M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z" />
              </svg>
            </span>
          </span>
        </div>
      </div>
    </div>
  );
}

// 设置行前置图标：一眼分辨每项在调什么（尤其语言）。lucide 风格、17px、描边。
const SI = {
  lang: (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round"><circle cx="12" cy="12" r="9" /><line x1="3" y1="12" x2="21" y2="12" /><path d="M12 3a15 15 0 0 1 0 18a15 15 0 0 1 0-18z" /></svg>
  ),
  theme: (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round"><circle cx="12" cy="12" r="9" /><path d="M12 3a9 9 0 0 0 0 18z" fill="currentColor" stroke="none" /></svg>
  ),
  style: (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round"><path d="M12 3 3 8l9 5 9-5-9-5z" /><path d="m3 12 9 5 9-5" /><path d="m3 16 9 5 9-5" /></svg>
  ),
  menu: (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round"><line x1="9" y1="6" x2="20" y2="6" /><line x1="9" y1="12" x2="20" y2="12" /><line x1="9" y1="18" x2="20" y2="18" /><circle cx="4.5" cy="6" r="1" /><circle cx="4.5" cy="12" r="1" /><circle cx="4.5" cy="18" r="1" /></svg>
  ),
  term: (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round"><rect x="3" y="4" width="18" height="16" rx="2" /><path d="m7 9 3 3-3 3" /><line x1="12.5" y1="15" x2="17" y2="15" /></svg>
  ),
};

// 会话菜单打开方式（卡片按钮 / 右键菜单）——留在「卡片菜单」步就地配置（那里正好解释菜单是什么）。
function CardMenuControl({ t }: { t: Dict }) {
  const [settings, patch] = useSettingsState();
  const mode = (settings?.card_menu_mode ?? "button") as CardMenuMode;
  return (
    <div className="ob-set">
      <div className="ob-set-row">
        <span className="ob-set-ico">{SI.menu}</span>
        <div className="ob-set-text">
          <div className="ob-set-label">{t.settings.cardMenu}</div>
          <div className="ob-set-desc">{t.onboarding.cardmenu.mode}</div>
        </div>
        <Segmented
          value={mode}
          options={[
            { value: "button" as const, label: t.settings.cardMenuButton },
            { value: "context" as const, label: t.settings.cardMenuContext },
          ]}
          onChange={(v: CardMenuMode) => patch({ card_menu_mode: v })}
          label={t.settings.cardMenu}
        />
      </div>
    </div>
  );
}

// 快速设置：语言 / 贴纸风格 / 默认终端，即时写入并生效（复用设置页的读写 hook 与控件）。
// 刻意前置到欢迎之后——偏配置的项若压到最后，用户很可能一路 Next 跳过。
function ConfigBody({ t }: { t: Dict }) {
  const [settings, patch] = useSettingsState();
  const [availTerms, setAvailTerms] = useState<ResumeTerminal[] | null>(null);
  useEffect(() => {
    availableTerminals().then(setAvailTerms).catch(() => setAvailTerms([]));
  }, []);

  const language = settings?.language ?? "auto";
  const theme = (settings?.theme ?? "dark") as ThemeMode;
  const stickerStyle = (settings?.sticker_style ?? "elevated") as StickerStyle;
  const platformOpts = IS_MAC ? RESUME_TERM_OPTIONS_MAC : resumeTermOptionsWin(t);
  const termOptions = platformOpts.filter((o) => (availTerms ?? []).includes(o.value));
  const storedTerm = (settings?.resume_terminal ?? "terminal") as ResumeTerminal;
  const resumeTerm = termOptions.some((o) => o.value === storedTerm) ? storedTerm : termOptions[0]?.value ?? "terminal";
  const showTermRow = (IS_MAC || IS_WIN) && termOptions.length >= 2;

  return (
    <div className="ob-set">
      <div className="ob-set-row">
        <span className="ob-set-ico">{SI.lang}</span>
        <div className="ob-set-text">
          <div className="ob-set-label">{t.settings.language}</div>
        </div>
        <Dropdown
          value={language}
          options={languageOptions(t)}
          onChange={(v) => patch({ language: v })}
        />
      </div>
      <div className="ob-set-row">
        <span className="ob-set-ico">{SI.theme}</span>
        <div className="ob-set-text">
          <div className="ob-set-label">{t.settings.theme}</div>
        </div>
        <Segmented
          value={theme}
          options={[
            { value: "dark" as const, label: t.settings.themeDark },
            { value: "light" as const, label: t.settings.themeLight },
            { value: "system" as const, label: t.settings.themeSystem },
          ]}
          onChange={(v: ThemeMode) => patch({ theme: v })}
          label={t.settings.theme}
        />
      </div>
      <div className="ob-set-row">
        <span className="ob-set-ico">{SI.style}</span>
        <div className="ob-set-text">
          <div className="ob-set-label">{t.settings.stickerStyle}</div>
        </div>
        <Segmented
          value={stickerStyle}
          options={[
            { value: "elevated" as const, label: t.settings.styleElevated },
            { value: "flat" as const, label: t.settings.styleFlat },
          ]}
          onChange={(v: StickerStyle) => patch({ sticker_style: v })}
          label={t.settings.stickerStyle}
        />
      </div>
      {showTermRow && (
        <div className="ob-set-row">
          <span className="ob-set-ico">{SI.term}</span>
          <div className="ob-set-text">
            <div className="ob-set-label">{t.settings.resumeTerm}</div>
            <div className="ob-set-desc">{t.onboarding.setup.terminalHint}</div>
          </div>
          <Dropdown value={resumeTerm} options={termOptions} onChange={(v) => patch({ resume_terminal: v })} />
        </div>
      )}
    </div>
  );
}

type Step = {
  hero?: ReactNode;
  title: string;
  desc?: string;
  points?: string[];
  body?: ReactNode;
};

/**
 * 使用引导窗口（label "onboarding"）。首次启动由 Rust 侧 setup 自动弹出，之后可从托盘/菜单栏
 * 图标或设置页手动打开。任一方式关闭（完成 / 跳过 / 关闭按钮）都会把 onboarding_seen 落盘，
 * 保证首次弹出只弹一次。既介绍核心用法（每步配迷你界面示意图），也在最后让用户顺手做基础配置。
 */
export function Onboarding() {
  const t = useT();
  // 窗口以 visible:false 创建（window.rs），首帧渲染后再显示，消除打开瞬间的白框闪烁。
  useShowWhenReady();
  const [step, setStep] = useState(0);

  // 顺序刻意把两个「偏配置」的步骤（快速设置、卡片菜单）放到欢迎之后的前面：
  // 压到最后用户很可能一路 Next 跳过。会话菜单的打开方式就地配置在「卡片菜单」步（那里正好解释菜单是什么）。
  const steps: Step[] = [
    { hero: <HeroWelcome />, title: t.onboarding.welcome.title, desc: t.onboarding.welcome.desc },
    { title: t.onboarding.setup.title, desc: t.onboarding.setup.desc, body: <ConfigBody t={t} /> },
    { hero: <HeroCardMenu t={t} />, title: t.onboarding.cardmenu.title, desc: t.onboarding.cardmenu.desc, body: <CardMenuControl t={t} /> },
    { hero: <HeroBoard t={t} />, title: t.onboarding.board.title, points: t.onboarding.board.points },
    { hero: <HeroTerminal />, title: t.onboarding.terminal.title, points: t.onboarding.terminal.points },
    // 吸边仅 Windows 有（macOS 是菜单栏面板），要点按平台拼接——不教的话用户误吸附后
    // 只会以为「窗口不见了」。
    { hero: <HeroWindow />, title: t.onboarding.window.title, points: IS_MAC ? t.onboarding.window.points : [...t.onboarding.window.points, t.onboarding.window.snapPoint] },
  ];
  const total = steps.length;
  const isFirst = step === 0;
  const isLast = step === total - 1;
  const cur = steps[step];

  // 完成/跳过/关闭都走这里：先落盘「已看过」，再关窗口。invoke 失败也照常关，别把用户卡在引导里。
  const dismiss = () => {
    invoke("mark_onboarding_seen").catch(() => {});
    getCurrentWindow().close().catch(() => {});
  };
  // Esc = 跳过（与点 ✕ 同语义，一样落盘「已看过」）。
  useEscClose(dismiss);
  const next = () => (isLast ? dismiss() : setStep((s) => Math.min(s + 1, total - 1)));
  const back = () => setStep((s) => Math.max(s - 1, 0));

  return (
    <div className="onboarding">
      <div className="ob-bar" data-tauri-drag-region>
        {!isLast && (
          <button className="ob-skip" onClick={dismiss}>
            {t.onboarding.skip}
          </button>
        )}
        <button className="winclose" data-tip={t.settings.close} aria-label={t.settings.close} onClick={dismiss}>
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round">
            <line x1="6" y1="6" x2="18" y2="18" />
            <line x1="18" y1="6" x2="6" y2="18" />
          </svg>
        </button>
      </div>

      {/* key=step 触发每步淡入，弱化切换的生硬感 */}
      <div className="ob-body" key={step}>
        {cur.hero && <div className="ob-hero">{cur.hero}</div>}
        <h1 className="ob-title">{cur.title}</h1>
        {cur.desc && <p className="ob-desc">{cur.desc}</p>}
        {cur.points && (
          <ul className="ob-points">
            {cur.points.map((p, i) => (
              <li key={i}>
                <span className="ob-tick" aria-hidden>
                  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round">
                    <path d="M5 12.5l4.5 4.5L19 7" />
                  </svg>
                </span>
                <span>{p}</span>
              </li>
            ))}
          </ul>
        )}
        {cur.body}
        {isLast && <p className="ob-reopen">{t.onboarding.window.reopenHint}</p>}
      </div>

      <div className="ob-foot">
        {/* 步骤圆点是导航，不是 tab（无对应 tabpanel）：用 aria-current="step" 表达当前步。 */}
        <div className="ob-dots" role="group" aria-label={t.onboarding.stepsLabel}>
          {steps.map((_, i) => (
            <button
              key={i}
              className={"ob-dot" + (i === step ? " on" : "")}
              aria-label={t.onboarding.stepOf(i + 1, total)}
              aria-current={i === step ? "step" : undefined}
              onClick={() => setStep(i)}
            />
          ))}
        </div>
        <div className="ob-actions">
          {!isFirst && (
            <button className="sbtn" onClick={back}>
              {t.onboarding.back}
            </button>
          )}
          <button className="sbtn primary" onClick={next}>
            {isLast ? t.onboarding.done : t.onboarding.next}
          </button>
        </div>
      </div>
    </div>
  );
}

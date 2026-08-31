import { useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useShowWhenReady } from "../useShowWhenReady";
import { languageOptions, useT } from "../i18n";
import { type ThemeMode } from "../api";
import { Segmented } from "./settings/widgets";
import { Dropdown } from "./menu";
import { useSettingsState } from "./settings/state";
import logoUrl from "../../src-tauri/icons/128x128.png";
import { useEscClose } from "../hooks/useEscClose";

type Dict = ReturnType<typeof useT>;

// 平台判定与真实设置页一致（WKWebView 的 UA 含 "Mac"）。吸边要点仅 Windows 拼接。
const IS_MAC = typeof navigator !== "undefined" && /Mac/i.test(navigator.userAgent);

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

function HeroWindow() {
  return (
    <div className="obm-desktop">
      <div className="obm-tray">
        <span className="obm-tray-ico">
          <img src={logoUrl} width={16} height={16} alt="" />
        </span>
        {/* 数字角标仅 macOS 菜单栏有（W-10：Windows 托盘不实现角标），Windows 上画了是在教不存在的东西。 */}
        {IS_MAC && <span className="obm-tray-badge">2</span>}
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
};

// 欢迎步就地设置:只留最关键的语言与外观两项(其余项全用默认值,设置页随时可改)。
// 引导从 6 步压到 3 步:配置项与设置页重复的步骤删除,是「操作更简」的一部分。
function WelcomeConfig({ t }: { t: Dict }) {
  const [settings, patch] = useSettingsState();
  const language = settings?.language ?? "auto";
  const theme = (settings?.theme ?? "dark") as ThemeMode;
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

  // 5 步:欢迎(含语言/外观两项就地设置) → 连接 AI CLI → Meowo 如何读到进度 → 看板与卡片 → 窗口行为。
  // 其余配置项全用默认值,设置页随时可改——配置型步骤与设置页重复,已删。
  // 「连接 AI CLI」不能省：此前引导只字未提装 agent/登录，用户走完面对一块空看板。
  // 「如何读到进度」同理（S-12）：hooks 改写 CLI 配置曾全程零告知，必须明写备份与移除方式。
  const steps: Step[] = [
    { hero: <HeroWelcome />, title: t.onboarding.welcome.title, desc: t.onboarding.welcome.desc, body: <WelcomeConfig t={t} /> },
    { title: t.onboarding.connect.title, points: t.onboarding.connect.points },
    { title: t.onboarding.progress.title, points: t.onboarding.progress.points },
    { hero: <HeroBoard t={t} />, title: t.onboarding.board.title, points: t.onboarding.board.points },
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
        {/* reopen 提示贴跳过路径：页尾那句只有走完最后一步的人才看得到，跳过者只经过这里。 */}
        {!isLast && <span className="ob-reopen ob-reopen-bar">{t.onboarding.window.reopenHint}</span>}
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

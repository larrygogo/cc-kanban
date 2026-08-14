import { useCallback, useEffect, useRef, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useAgentListRefresh } from "../useAgents";
import { availableTerminals, getLiveSessionsPage, listAgents, agentName, setArchived, type AgentId, type AgentDescriptor, type LiveSession, type PageCursor, type ThemeMode, type ResumeTerminal, type TerminalOpenMode, type SessionOpenIn, type CardMenuMode, type StickerStyle, type TerminalLineHeight } from "../api";
import { folderName } from "../paths";
import { fmtAgo } from "./sticker/helpers";
import { useUpdate, type UpdateStatus } from "../useUpdate";
import { useShowWhenReady } from "../useShowWhenReady";
import { languageOptions, useT } from "../i18n";
import logoUrl from "../../src-tauri/icons/128x128.png";
import type { Dict } from "../i18n/zh";
import { SETTINGS_DEFAULTS, useSettingsState } from "./settings/state";
import { Switch, Segmented, SwatchPicker, FontSizeSlider } from "./settings/widgets";
import { Dropdown } from "./menu";
import { AccountSection } from "./settings/AccountSection";
import { NetworkSection } from "./settings/NetworkSection";
import { useEscClose } from "../hooks/useEscClose";

const REPO = "github.com/larrygogo/meowo";
const REPO_URL = "https://github.com/larrygogo/meowo";
const SITE = "meowo.io";
const SITE_URL = "https://meowo.io";
const openExt = (url: string) => invoke("open_url", { url }).catch(() => {});

type Section = "general" | "sessions" | "appearance" | "network" | "account" | "about";


// 打开未连接会话用的终端：按平台给不同选项。WKWebView 的 UA 含 "Mac"/"Win"，与 main.tsx 同步判定一致。
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


function IconGear() {
  return (
    <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
    </svg>
  );
}
function IconInfo() {
  return (
    <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
      <circle cx="12" cy="12" r="9" />
      <line x1="12" y1="11" x2="12" y2="16" />
      <line x1="12" y1="8" x2="12" y2="8" />
    </svg>
  );
}

// 机器人徽标：这一分区管的是各家 AI Agent，比人像更贴切。
function IconAgent() {
  return (
    <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
      <path d="M12 8V4H8" />
      <rect x="4" y="8" width="16" height="12" rx="2.5" />
      <path d="M2 14h2" />
      <path d="M20 14h2" />
      <path d="M9 13.5v2" />
      <path d="M15 13.5v2" />
    </svg>
  );
}

// 叠放的圆角卡片：会话分区管的是会话与看板卡片。
function IconCards() {
  return (
    <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
      <rect x="3" y="3" width="13" height="13" rx="2.5" />
      <path d="M8 21h10.5a2.5 2.5 0 0 0 2.5-2.5V8" />
    </svg>
  );
}

// 半填充对比圆：外观/主题的经典图标。
function IconAppearance() {
  return (
    <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
      <circle cx="12" cy="12" r="9" />
      <path d="M12 3a9 9 0 0 0 0 18z" fill="currentColor" stroke="none" />
    </svg>
  );
}

// 地球：网络/代理。
function IconGlobe() {
  return (
    <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
      <circle cx="12" cy="12" r="9" />
      <line x1="3" y1="12" x2="21" y2="12" />
      <path d="M12 3a15 15 0 0 1 0 18a15 15 0 0 1 0-18z" />
    </svg>
  );
}



/** 设置保存失败的统一错误行：patch 失败时开关会被回读「弹回去」，没有这一行用户
 *  只能看到界面自己变回去、零解释（S-3——曾只有网络分区把错误显示了出来）。 */
function SettingsError({ error }: { error: string | null }) {
  if (!error) return null;
  return <div className="sec-hint proxy-err" role="alert">{error}</div>;
}

// 通用：应用级设置（语言、自启、更新、通知）。会话与卡片的行为在 SessionsSection。
function GeneralSection() {
  const t = useT();
  const [autostart, setAutostart] = useState(false);
  const [settings, patch, patchError] = useSettingsState();
  // dev 下开机自启会注册调试二进制(开机连不上 dev server → 白屏)，故禁用此开关，仅安装版可用。
  const autostartDisabled = import.meta.env.DEV;
  useEffect(() => {
    if (!autostartDisabled) invoke<boolean>("get_autostart").then(setAutostart).catch(() => {});
  }, [autostartDisabled]);
  const toggleAutostart = () => {
    if (autostartDisabled) return;
    const next = !autostart;
    setAutostart(next);
    invoke("set_autostart", { enabled: next }).catch(() => setAutostart(!next));
  };
  const notifyOn = settings?.notifications_enabled ?? true;
  const flashOn = settings?.attention_flash_enabled ?? true;
  const autoUpdateOn = settings?.auto_update_enabled ?? true;
  const chatOn = settings?.chat_enabled ?? true;
  const toggleNotify = () => patch({ notifications_enabled: !notifyOn });
  const toggleFlash = () => patch({ attention_flash_enabled: !flashOn });
  const toggleAutoUpdate = () => patch({ auto_update_enabled: !autoUpdateOn });
  const toggleChat = () => patch({ chat_enabled: !chatOn });
  return (
    <>
      <div className="row-card">
        <div className="row">
          <div className="row-text">
            <div className="row-label">{t.settings.language}</div>
            <div className="row-desc">{t.settings.languageDesc}</div>
          </div>
          <Dropdown
            value={settings?.language ?? "auto"}
            options={languageOptions(t)}
            onChange={(v) => patch({ language: v })}
          />
        </div>
        <div className="row">
          <div className="row-text">
            <div className="row-label">{t.settings.autostart}</div>
            <div className="row-desc">{t.settings.autostartDesc}</div>
          </div>
          <Switch checked={autostart} onChange={toggleAutostart} disabled={autostartDisabled} label={t.settings.autostart} />
        </div>
        {/* 对话窗口功能总开关（轻量模式）。放 General：轻量模式下点任何 chat 入口都会
            落到设置窗，这里是用户找回完整功能的地方，必须在首屏分区就看得见。 */}
        <div className="row">
          <div className="row-text">
            <div className="row-label">{t.settings.chatFeature}</div>
            <div className="row-desc">{t.settings.chatFeatureDesc}</div>
          </div>
          <Switch checked={chatOn} onChange={toggleChat} label={t.settings.chatFeature} />
        </div>
        <div className="row">
          <div className="row-text">
            <div className="row-label">{t.settings.autoUpdate}</div>
            <div className="row-desc">{t.settings.autoUpdateDesc}</div>
          </div>
          <Switch checked={autoUpdateOn} onChange={toggleAutoUpdate} label={t.settings.autoUpdate} />
        </div>
        <div className="row">
          <div className="row-text">
            <div className="row-label">{t.settings.notify}</div>
            <div className="row-desc">{t.settings.notifyDesc}</div>
          </div>
          <Switch checked={notifyOn} onChange={toggleNotify} label={t.settings.notify} />
        </div>
        {/* 任务栏闪烁仅 Windows 有效(macOS 由菜单栏徽章承担同一职责),别的平台不显示无效开关。 */}
        {IS_WIN && <div className="row">
          <div className="row-text">
            <div className="row-label">{t.settings.attentionFlash}</div>
            <div className="row-desc">{t.settings.attentionFlashDesc}</div>
          </div>
          <Switch checked={flashOn} onChange={toggleFlash} label={t.settings.attentionFlash} />
        </div>}
      </div>
      <SettingsError error={patchError} />
    </>
  );
}

// 会话：新建/打开/归档等会话行为，外加看板卡片的展示与交互（独立成第二张卡）。
function SessionsSection() {
  const t = useT();
  const [settings, patch, patchError] = useSettingsState();
  const [availTerms, setAvailTerms] = useState<ResumeTerminal[] | null>(null);
  const [agents, setAgents] = useState<AgentDescriptor[]>([]);
  const availAgents = agents.filter((a) => a.installed).map((a) => a.id);
  useEffect(() => {
    availableTerminals().then(setAvailTerms).catch(() => setAvailTerms([]));
  }, []);
  const reloadAgents = () => {
    listAgents().then(setAgents).catch(() => {});
  };
  useEffect(reloadAgents, []);
  useAgentListRefresh(reloadAgents); // 装完新 agent 立刻反映
  const previewOn = settings?.preview_enabled ?? true;
  const togglePreview = () => patch({ preview_enabled: !previewOn });
  const chatOn = settings?.chat_enabled ?? true;
  // 终端选项按平台给，再用后端探测到的「本机实际可用」列表过滤（未装的不列出）。
  const platformOpts = IS_MAC ? RESUME_TERM_OPTIONS_MAC : resumeTermOptionsWin(t);
  const termOptions = platformOpts.filter((o) => (availTerms ?? []).includes(o.value));
  // 保存值若不在可用项内（如未装 iTerm 仍存着 "iterm"，或 Windows 上残留 macOS 默认 "terminal"），显示退回首项。
  const storedTerm = settings?.resume_terminal ?? "terminal";
  const resumeTerm = termOptions.some((o) => o.value === storedTerm) ? storedTerm : (termOptions[0]?.value ?? "terminal");
  const changeResumeTerm = (v: ResumeTerminal) => patch({ resume_terminal: v });
  // 至少两个可用终端才有选择意义；只有一个（如 macOS 没装 iTerm）就不显示这一行。
  const showTermRow = (IS_MAC || IS_WIN) && termOptions.length >= 2;
  // 默认 Agent 下拉：选项以已装 agent 为主；若保存值不在已装列表里（未装/尚未探测完成），
  // 在最前面补一项，避免 Dropdown 内部 find 不到导致按钮标签空白。
  const defaultAgent = settings?.default_agent ?? SETTINGS_DEFAULTS.default_agent;
  const opt = (p: AgentId) => ({ value: p, label: agentName(agents, p) });
  const defaultAgentOptions = availAgents.includes(defaultAgent)
    ? availAgents.map(opt)
    : [opt(defaultAgent), ...availAgents.map(opt)];
  return (
    <>
      <div className="row-card">
        <div className="row">
          <div className="row-text">
            <div className="row-label">{t.settings.defaultAgent}</div>
            <div className="row-desc">{t.settings.defaultAgentDesc}</div>
          </div>
          <Dropdown
            value={defaultAgent}
            options={defaultAgentOptions}
            onChange={(v) => patch({ default_agent: v })}
          />
        </div>
        {/* 轻量模式（chat_enabled=off）下后端强制走外部终端：行保留但置灰、原因写进
            row-desc——整行隐藏会让用户以为设置项消失了。 */}
        <div className="row">
          <div className="row-text">
            <div className="row-label">{t.settings.sessionOpenIn}</div>
            <div className="row-desc">{chatOn ? t.settings.sessionOpenInDesc : t.settings.sessionOpenInLite}</div>
          </div>
          <Dropdown
            value={chatOn ? (settings?.session_open_in ?? "terminal") : ("terminal" as SessionOpenIn)}
            disabled={!chatOn}
            options={[
              { value: "chat" as const, label: t.settings.sessionOpenInChat },
              { value: "terminal" as const, label: t.settings.sessionOpenInTerminal },
            ]}
            onChange={(v: SessionOpenIn) => patch({ session_open_in: v })}
          />
        </div>
        {showTermRow && (
          <div className="row">
            <div className="row-text">
              <div className="row-label">{t.settings.resumeTerm}</div>
              <div className="row-desc">{t.settings.resumeTermDesc}</div>
            </div>
            <Dropdown value={resumeTerm} options={termOptions} onChange={changeResumeTerm} />
          </div>
        )}
      </div>

      <div className="sec-caption">{t.settings.cardsGroup}</div>
      <div className="row-card">
        <div className="row">
          <div className="row-text">
            <div className="row-label">{t.settings.preview}</div>
            <div className="row-desc">{t.settings.previewDesc}</div>
          </div>
          <Switch checked={previewOn} onChange={togglePreview} label={t.settings.preview} />
        </div>
        <div className="row">
          <div className="row-text">
            <div className="row-label">{t.settings.terminalOpen}</div>
            <div className="row-desc">{t.settings.terminalOpenDesc}</div>
          </div>
          <Dropdown
            value={settings?.terminal_open_mode ?? "card"}
            options={[
              { value: "card" as const, label: t.settings.openModeCard },
              { value: "button" as const, label: t.settings.openModeButton },
            ]}
            onChange={(v: TerminalOpenMode) => patch({ terminal_open_mode: v })}
          />
        </div>
        <div className="row">
          <div className="row-text">
            <div className="row-label">{t.settings.cardMenu}</div>
            <div className="row-desc">{t.settings.cardMenuDesc}</div>
          </div>
          <Dropdown
            value={settings?.card_menu_mode ?? "button"}
            options={[
              { value: "context" as const, label: t.settings.cardMenuContext },
              { value: "button" as const, label: t.settings.cardMenuButton },
            ]}
            onChange={(v: CardMenuMode) => patch({ card_menu_mode: v })}
          />
        </div>
      </div>
      <SettingsError error={patchError} />
      <ArchivedSessions />
    </>
  );
}

/** 已归档会话每页条数。设置页是普通滚动列，用「加载更多」按钮翻页而不是滚动加载——
 *  归档区只是设置页的一段，滚动到底不代表用户想看更多归档。 */
const ARCHIVED_PAGE = 30;

/** 已归档会话管理。看板不再有「已归档」tab（归档 = 从看板彻底收纳走），这里是唯一入口：
 *  游标分页动态加载，唯一操作是放回看板。 */
function ArchivedSessions() {
  const t = useT();
  // null = 尚未加载完成，与「真空」区分。
  const [items, setItems] = useState<LiveSession[] | null>(null);
  const [cursor, setCursor] = useState<PageCursor | null>(null);
  const [loadingMore, setLoadingMore] = useState(false);
  const loadPage = useCallback((after: PageCursor | null) => {
    setLoadingMore(true);
    getLiveSessionsPage("archived", null, after, ARCHIVED_PAGE)
      .then((page) => {
        setCursor(page.next_cursor);
        setItems((prev) => {
          // 首页替换、翻页按 id 去重追加（翻页间归档集可能被并发改动）。
          if (after === null || prev === null) return page.items;
          const seen = new Set(prev.map((s) => s.session.id));
          return [...prev, ...page.items.filter((s) => !seen.has(s.session.id))];
        });
      })
      .catch(() => setItems((prev) => prev ?? []))
      .finally(() => setLoadingMore(false));
  }, []);
  useEffect(() => loadPage(null), [loadPage]);
  const unarchive = (id: number) => {
    // 乐观移除；失败重取首页拉回（与看板同款语义）。看板经 board-changed 自行刷新。
    setItems((prev) => prev?.filter((s) => s.session.id !== id) ?? prev);
    void setArchived(id, false).catch(() => loadPage(null));
  };
  return (
    <>
      {/* 标题在卡片外——放卡片里像一个可交互的设置项（用户实拍反馈），这里是列表的分组标题。 */}
      <div className="sec-caption">{t.settings.archivedSessions}</div>
      <div className="row-card">
        {items !== null && items.length === 0 && (
          <div className="archived-empty">{t.settings.archivedEmpty}</div>
        )}
        {/* 限高内滚：归档可能积上百条，撑满整个设置页会把后面的设置项挤没。 */}
        <div className="archived-scroll">
          {(items ?? []).map((item) => (
            <div className="row archived-row" key={item.session.id}>
              <div className="row-text">
                <div className="row-label archived-title">{item.task_title || t.sticker.waitingFirstInput}</div>
                <div className="row-desc">
                  {[folderName(item.cwd), fmtAgo(item.session.last_event_at, t)].filter(Boolean).join(" · ")}
                </div>
              </div>
              <div className="archived-actions">
                <button type="button" className="sbtn" onClick={() => unarchive(item.session.id)}>
                  {t.sticker.unarchive}
                </button>
              </div>
            </div>
          ))}
          {cursor !== null && (
            <div className="row archived-more-row">
              <button type="button" className="sbtn" disabled={loadingMore} onClick={() => loadPage(cursor)}>
                {loadingMore ? t.chat.sidebarLoading : t.settings.archivedLoadMore}
              </button>
            </div>
          )}
        </div>
      </div>
    </>
  );
}


const themeOptions = (t: Dict): { value: ThemeMode; label: string }[] => [
  { value: "dark", label: t.settings.themeDark },
  { value: "light", label: t.settings.themeLight },
  { value: "system", label: t.settings.themeSystem },
];
const stickerStyleOptions = (t: Dict): { value: StickerStyle; label: string }[] => [
  { value: "elevated", label: t.settings.styleElevated },
  { value: "flat", label: t.settings.styleFlat },
];
const fontSizeOptions = (t: Dict): { value: number; label: string }[] => [
  { value: 90, label: t.settings.fontSizeSmall },
  { value: 100, label: t.settings.fontSizeNormal },
  { value: 112, label: t.settings.fontSizeLarge },
];

const OPACITY_MIN = 25;
const OPACITY_MAX = 100;

const TERM_FONT_MIN = 10;
const TERM_FONT_MAX = 18;
const lineHeightOptions = (t: Dict): { value: TerminalLineHeight; label: string }[] => [
  { value: "compact", label: t.settings.lineCompact },
  { value: "normal", label: t.settings.lineNormal },
  { value: "relaxed", label: t.settings.lineRelaxed },
];

/** 滑杆草稿：拖动期间只更新本地显示，松手（pointerup）或键盘调节静默 240ms 后才提交。
 *  逐像素 onChange 直发 patch = 每 px 一轮完整的 set_settings 链（全量写盘 + 读写 claude
 *  配置文件 + proxy-applied/settings-changed 双广播），拖一次滑杆等于几十轮流程，低端机
 *  与杀软环境下可感知卡顿。 */
function useSliderDraft(commit: (v: number) => void) {
  const [draft, setDraft] = useState<number | null>(null);
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  const change = (v: number) => {
    setDraft(v);
    window.clearTimeout(timer.current);
    // 键盘方向键调节没有 pointerup：静默 240ms 后兜底提交。
    timer.current = window.setTimeout(() => { commit(v); setDraft(null); }, 240);
  };
  const release = (v: number) => {
    window.clearTimeout(timer.current);
    commit(v);
    setDraft(null);
  };
  return { draft, change, release };
}

function AppearanceSection() {
  const t = useT();
  const [settings, patch, patchError] = useSettingsState();
  // 占位一律读 SETTINGS_DEFAULTS：曾在此手写 `?? 94` / `?? "elevated"` 字面量，与真实默认
  // （settings.rs：100 / "flat"）不符——设置页每次打开都先高亮错误档位再跳正确值。
  const theme = settings?.theme ?? SETTINGS_DEFAULTS.theme;
  const opacity = settings?.opacity ?? SETTINGS_DEFAULTS.opacity;
  const uiScale = settings?.ui_scale ?? SETTINGS_DEFAULTS.ui_scale;
  const stickerStyle = settings?.sticker_style ?? SETTINGS_DEFAULTS.sticker_style;
  const stickerColor = settings?.sticker_color ?? SETTINGS_DEFAULTS.sticker_color;
  const termFont = settings?.terminal_font_size ?? SETTINGS_DEFAULTS.terminal_font_size;
  const termLine = settings?.terminal_line_height ?? SETTINGS_DEFAULTS.terminal_line_height;
  // 滑杆草稿（拖动中本地显示，松手才提交）：见 useSliderDraft。
  const opacityDraft = useSliderDraft((v) => void patch({ opacity: v }));
  const termFontDraft = useSliderDraft((v) => void patch({ terminal_font_size: v }));
  const opacityShown = opacityDraft.draft ?? opacity;
  const termFontShown = termFontDraft.draft ?? termFont;
  // 钳到 [0,100]：手改 settings.json 为越界值时，避免算出负/超界的 linear-gradient 填充宽度。
  const fill = Math.max(0, Math.min(100, ((opacityShown - OPACITY_MIN) / (OPACITY_MAX - OPACITY_MIN)) * 100));
  const termFill = Math.max(0, Math.min(100, ((termFontShown - TERM_FONT_MIN) / (TERM_FONT_MAX - TERM_FONT_MIN)) * 100));
  return (
    <>
      <div className="row-card">
        <div className="row">
          <div className="row-text">
            <div className="row-label">{t.settings.theme}</div>
            <div className="row-desc">{t.settings.themeDesc}</div>
          </div>
          <Segmented value={theme} options={themeOptions(t)} onChange={(v) => patch({ theme: v })} label={t.settings.theme} />
        </div>
        <div className="row">
          <div className="row-text">
            <div className="row-label">{t.settings.fontSize}</div>
            <div className="row-desc">{t.settings.fontSizeDesc}</div>
          </div>
          <FontSizeSlider value={uiScale} options={fontSizeOptions(t)} onChange={(v) => patch({ ui_scale: v })} label={t.settings.fontSize} />
        </div>
        <div className="row">
          <div className="row-text">
            <div className="row-label">{t.settings.stickerStyle}</div>
            <div className="row-desc">{t.settings.stickerStyleDesc}</div>
          </div>
          <Segmented value={stickerStyle} options={stickerStyleOptions(t)} onChange={(v) => patch({ sticker_style: v })} label={t.settings.stickerStyle} />
        </div>
        <div className="row">
          <div className="row-text">
            <div className="row-label">{t.settings.stickerColor}</div>
            <div className="row-desc">{t.settings.stickerColorDesc}</div>
          </div>
          <SwatchPicker value={stickerColor} onChange={(v) => patch({ sticker_color: v })} label={t.settings.stickerColor} names={t.settings.colorNames} />
        </div>
        <div className="row row-col">
          <div className="row-head">
            <div className="row-text">
              <div className="row-label">{t.settings.opacity}</div>
              <div className="row-desc">{t.settings.opacityDesc}</div>
            </div>
            <span className="row-val">{opacityShown}%</span>
          </div>
          <input
            type="range"
            className="slider"
            min={OPACITY_MIN}
            max={OPACITY_MAX}
            value={opacityShown}
            style={{ background: `linear-gradient(90deg, var(--cc-accent) ${fill}%, var(--cc-border) ${fill}%)` }}
            onChange={(e) => opacityDraft.change(Number(e.target.value))}
            onPointerUp={(e) => opacityDraft.release(Number((e.target as HTMLInputElement).value))}
            aria-label={t.settings.opacity}
          />
        </div>
      </div>

      <div className="sec-caption">{t.settings.terminalGroup}</div>
      <div className="row-card">
        <div className="row row-col">
          <div className="row-head">
            <div className="row-text">
              <div className="row-label">{t.settings.termFontSize}</div>
              <div className="row-desc">{t.settings.termFontSizeDesc}</div>
            </div>
            <span className="row-val">{termFontShown}px</span>
          </div>
          <input
            type="range"
            className="slider"
            min={TERM_FONT_MIN}
            max={TERM_FONT_MAX}
            value={termFontShown}
            style={{ background: `linear-gradient(90deg, var(--cc-accent) ${termFill}%, var(--cc-border) ${termFill}%)` }}
            onChange={(e) => termFontDraft.change(Number(e.target.value))}
            onPointerUp={(e) => termFontDraft.release(Number((e.target as HTMLInputElement).value))}
            aria-label={t.settings.termFontSize}
          />
        </div>
        <div className="row">
          <div className="row-text">
            <div className="row-label">{t.settings.termLineHeight}</div>
            <div className="row-desc">{t.settings.termLineHeightDesc}</div>
          </div>
          <Segmented value={termLine} options={lineHeightOptions(t)} onChange={(v) => patch({ terminal_line_height: v })} label={t.settings.termLineHeight} />
        </div>
      </div>
      <SettingsError error={patchError} />
      <div className="sec-hint">{t.settings.appearanceHint}</div>
    </>
  );
}

function AboutSection({
  status,
  newVersion,
}: {
  status: UpdateStatus;
  newVersion: string | null;
}) {
  const t = useT();
  const [version, setVersion] = useState("");

  useEffect(() => {
    getVersion().then(setVersion).catch(() => {});
  }, []);

  // 「检查更新」与「更新到 vX」都直接打开更新窗口——检查/下载/安装全在那边完成并可视反馈
  // （内联 recheck 在检查失败时界面毫无动静）。本节的后台检查只驱动按钮文案与导航角标。
  // 旧的 trigger-update/update-failed 跨窗口协议已废除：曾因两窗状态分歧把按钮锁死在「更新中…」。
  const openUpdater = () => invoke("open_update_window").catch(() => {});
  const hasUpdate = status === "available" || status === "downloading" || status === "ready";
  const updateBtn =
    hasUpdate
      ? { label: t.about.updateTo(newVersion ?? ""), primary: true }
      : { label: t.about.checkUpdate, primary: false };

  const verText = `v${version || "—"}`;
  // checking/error 也要有话说：曾经两态都落空串——打开「关于」十秒空窗、检查失败一片安静。
  // unknown（关了自动更新、从未检查）只显示版本号，不谎报「已是最新」。
  const verStatus =
    hasUpdate ? t.about.foundNew(newVersion ?? "")
      : status === "latest" ? t.about.upToDate
      : status === "checking" ? t.about.checking
      : status === "error" ? t.about.checkFailed
      : "";
  const verSub = verStatus ? `${verText} · ${verStatus}` : verText;

  return (
    <>
      <div className="row-card">
        <div className="row">
          <div className="row-icon"><img className="pmark" src={logoUrl} width={38} height={38} alt="" /></div>
          <div className="row-text">
            <div className="row-label">{t.about.versionInfo}</div>
            <div className="row-desc">{verSub}</div>
          </div>
          <button className={"sbtn" + (updateBtn.primary ? " primary" : "")} onClick={openUpdater}>
            {updateBtn.label}
          </button>
        </div>
        <div className="row">
          <div className="row-text">
            <div className="row-label">{t.about.website}</div>
            <div className="row-desc">{SITE}</div>
          </div>
          <button className="sbtn primary" onClick={() => openExt(SITE_URL)}>
            {t.about.open}
          </button>
        </div>
        <div className="row">
          <div className="row-text">
            <div className="row-label">{t.about.source}</div>
            <div className="row-desc">{REPO}</div>
          </div>
          <button className="sbtn" onClick={() => openExt(REPO_URL)}>
            {t.about.open}
          </button>
        </div>
        <div className="row">
          <div className="row-text">
            <div className="row-label">{t.about.guide}</div>
            <div className="row-desc">{t.onboarding.welcome.title}</div>
          </div>
          <button className="sbtn" onClick={() => void invoke("open_onboarding").catch(() => {})}>
            {t.about.open}
          </button>
        </div>
      </div>

      <p className="about-blurb">{t.about.blurb}</p>

      <div className="about-foot">
        {/* 样式原本挂在 `.about-foot a` 元素选择器上（styles.css 本轮不可改），
            这里用内联 style 复刻外观（accent 色 + 手型 + 去掉按钮默认样式）。 */}
        <button
          type="button"
          style={{ appearance: "none", background: "none", border: "none", padding: 0, font: "inherit", color: "var(--cc-accent-text)", cursor: "pointer" }}
          onClick={() => openExt(REPO_URL + "/issues")}
        >
          {t.about.feedback}
        </button>
        <span className="dot">·</span>
        <button
          type="button"
          style={{ appearance: "none", background: "none", border: "none", padding: 0, font: "inherit", color: "var(--cc-accent-text)", cursor: "pointer" }}
          onClick={() => openExt(REPO_URL + "/releases")}
        >
          {t.about.changelog}
        </button>
        <div className="copy">MIT License · © 2026 larrygogo</div>
      </div>
    </>
  );
}

export function About() {
  const t = useT();
  // 窗口以 visible:false 创建（window.rs），首帧渲染后再显示，消除打开瞬间的白框闪烁。
  useShowWhenReady();
  // 分区记忆：每次开窗回 general 让最常改的分区（外观/Agent）永远多点一次。
  const [sec, setSec] = useState<Section>(() => {
    const s = localStorage.getItem("meowo-settings-section");
    return s === "general" || s === "sessions" || s === "appearance" || s === "network" || s === "account" || s === "about"
      ? s
      : "general";
  });
  const pickSec = (next: Section) => {
    setSec(next);
    try { localStorage.setItem("meowo-settings-section", next); } catch { /* 隐私模式禁写 */ }
  };
  const close = () => getCurrentWindow().close().catch(() => {});
  useEscClose(close); // 弹出式任务窗的基本预期：Esc 关窗（输入框内让位）
  // 设置窗口也服从自动更新开关；关闭时不做后台检查，用户仍可从「关于」手动打开更新窗口检查。
  const { status, version: newVersion } = useUpdate({ automatic: true });

  return (
    <div className="settings">
      <aside className="side">
        <div className="side-top" data-tauri-drag-region />
        <nav className="side-nav">
          <button className={"nav-item" + (sec === "general" ? " on" : "")} aria-current={sec === "general" ? "page" : undefined} onClick={() => pickSec("general")}>
            <IconGear />
            <span>{t.settings.nav.general}</span>
          </button>
          <button className={"nav-item" + (sec === "sessions" ? " on" : "")} aria-current={sec === "sessions" ? "page" : undefined} onClick={() => pickSec("sessions")}>
            <IconCards />
            <span>{t.settings.nav.sessions}</span>
          </button>
          <button className={"nav-item" + (sec === "appearance" ? " on" : "")} aria-current={sec === "appearance" ? "page" : undefined} onClick={() => pickSec("appearance")}>
            <IconAppearance />
            <span>{t.settings.nav.appearance}</span>
          </button>
          <button className={"nav-item" + (sec === "network" ? " on" : "")} aria-current={sec === "network" ? "page" : undefined} onClick={() => pickSec("network")}>
            <IconGlobe />
            <span>{t.settings.nav.network}</span>
          </button>
          <button className={"nav-item" + (sec === "account" ? " on" : "")} aria-current={sec === "account" ? "page" : undefined} onClick={() => pickSec("account")}>
            <IconAgent />
            <span>{t.settings.nav.account}</span>
          </button>
          <button className={"nav-item" + (sec === "about" ? " on" : "")} aria-current={sec === "about" ? "page" : undefined} onClick={() => pickSec("about")}>
            <IconInfo />
            <span>{t.settings.nav.about}</span>
            {(status === "available" || status === "downloading" || status === "ready") && (
              <span className="nav-tag">{t.settings.updateTag}</span>
            )}
          </button>
        </nav>
      </aside>

      <main className="main">
        <div className="main-bar" data-tauri-drag-region>
          <button className="winclose" data-tip={t.settings.close} onClick={close} aria-label={t.settings.close}>
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round">
              <line x1="6" y1="6" x2="18" y2="18" />
              <line x1="18" y1="6" x2="6" y2="18" />
            </svg>
          </button>
        </div>
        <div className="main-body" key={sec}>
          {sec === "general" ? (
            <GeneralSection />
          ) : sec === "sessions" ? (
            <SessionsSection />
          ) : sec === "appearance" ? (
            <AppearanceSection />
          ) : sec === "network" ? (
            <NetworkSection />
          ) : sec === "account" ? (
            <AccountSection />
          ) : (
            <AboutSection status={status} newVersion={newVersion} />
          )}
        </div>
      </main>
    </div>
  );
}

// 设置窗口「账号」页：每个 provider 的安装 / 登录 / 用量三态卡片。
// 从 About.tsx 抽出（体量最大、最内聚，且已被 About.account.test.tsx 单独覆盖）。
import { useCallback, useEffect, useRef, useState } from "react";
import { appConfirm } from "../../confirm";
import {
  listAgents,
  listProfiles,
  createProfile,
  setActiveProfile,
  renameProfile,
  deleteProfile,
  mergeProfileIntoDefault,
  type AgentId,
  type AgentDescriptor,
  type ProfileView,
  type Settings,
} from "../../api";
import {
  getAccounts,
  refreshUsage,
  checkProviderHooks,
  repairProviderHooks,
  logoutAgent,
  apiKeyLogin,
  agentPathGap,
  addAgentToUserPath,
  openInstallLog,
  checkAgentUpdates,
  getLiveSessionsPage,
  type AgentUpdateInfo,
  type ProviderAccountPayload,
  type ProviderUsage,
  type UsageLane,
  type HooksStatus,
} from "../../api";
import { agentAssets } from "../../providers";
import { useT, repairFailMessage } from "../../i18n";
import { formatBackendError } from "../../i18n/errors";
import type { Dict } from "../../i18n/zh";
import { Switch } from "./widgets";
import { ActionMenu, Dropdown } from "../menu";
import { useSettingsState } from "./state";
import { RelayAccess } from "./RelayAccess";
import { useTauriEvent } from "../../hooks/useTauriEvent";
import { useLoginOperations, type LoginOperationState } from "../../hooks/useLoginOperations";
import { useInstallOperations, type InstallOperationState } from "../../hooks/useInstallOperations";

function RefreshIcon({ spinning }: { spinning?: boolean }) {
  return (
    <svg className={spinning ? "spin" : undefined} width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M3 12a9 9 0 0 1 15-6.7L21 8" />
      <path d="M21 3v5h-5" />
      <path d="M21 12a9 9 0 0 1-15 6.7L3 16" />
      <path d="M3 21v-5h5" />
    </svg>
  );
}

function IconDownload() {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
      <polyline points="7 10 12 15 17 10" />
      <line x1="12" y1="15" x2="12" y2="3" />
    </svg>
  );
}

function fmtResetIn(iso: string, t: Dict): string {
  const ts = Date.parse(iso);
  if (Number.isNaN(ts)) return "";
  const now = Date.now();
  const diffMs = ts - now;
  if (diffMs <= 0) return t.account.resetSoon;
  // 按自然日差判断：今天显示剩余小时/分钟，跨天则用相对词/日期并精确到钟点。
  const startOf = (ms: number) => {
    const d = new Date(ms);
    return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  };
  const dayDiff = Math.round((startOf(ts) - startOf(now)) / 86_400_000);
  if (dayDiff <= 0) {
    const min = Math.round(diffMs / 60000);
    if (min < 1) return t.account.resetSoon; // 剩余不足半分钟时 round 得 0，归入「即将重置」
    if (min < 60) return t.account.resetInMin(min);
    const h = Math.floor(min / 60);
    const m = min % 60;
    return m > 0 ? t.account.resetInHourMin(h, m) : t.account.resetInHour(h);
  }
  // 时刻/日期走 Intl：手拼 `pad(getHours())` 是硬编码 24 小时制，英文用户看到的是
  // "Resets tomorrow 14:30" 而非 "2:30 PM"；月/日顺序同理随 locale。
  const r = new Date(ts);
  const clock = new Intl.DateTimeFormat(t.locale, { hour: "numeric", minute: "2-digit" }).format(r);
  if (dayDiff === 1) return t.account.resetTomorrow(clock);
  if (dayDiff === 2) return t.account.resetDayAfter(clock);
  const date = new Intl.DateTimeFormat(t.locale, { month: "short", day: "numeric" }).format(r);
  return t.account.resetOnDate(date, clock);
}

function laneLabel(lane: UsageLane, t: Dict): string {
  switch (lane.kind) {
    case "five_hour": return t.account.laneFiveHour;
    case "seven_day": return t.account.laneSevenDay;
    case "opus": return t.account.laneOpus;
    case "weekly": return t.account.laneWeekly;
    // 模型专属周限(claude 的 Fable/Opus 周配额):模型名由 API 下发,缺失时退回通用周配额文案。
    case "model_weekly": return lane.label ? t.account.laneModelWeekly(lane.label) : t.account.laneWeekly;
    case "balance": return t.account.laneBalance;
    default: return lane.kind;
  }
}

// note 是后端机器哨兵串（claude 发 "extra_usage_enabled"、codex 发 "credits:45.5"），
// 映射为本地化文案；未知格式原样显示以向后兼容。
function renderNote(note: string, t: Dict): string {
  if (note === "extra_usage_enabled") return t.account.extraUsage;
  if (note.startsWith("credits:")) return t.account.credits(note.slice("credits:".length));
  return note;
}

function UsageBar({ lane, label }: { lane: UsageLane; label: string }) {
  const t = useT();
  if (lane.used_pct != null) {
    const pct = Math.max(0, Math.min(100, lane.used_pct));
    return (
      <div className="usage-row">
        <div className="usage-head">
          <span className="usage-label">{label}</span>
          <span className="usage-pct">{pct.toFixed(0)}%</span>
        </div>
        <div className="usage-track" role="progressbar" aria-label={label} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(pct)}>
          <i style={{ width: `${pct}%` }} />
        </div>
        {lane.resets_at && <div className="usage-reset">{fmtResetIn(lane.resets_at, t)}</div>}
      </div>
    );
  }
  // 余额型：显数值，不画进度条
  const valText = lane.used != null ? `${lane.used}${lane.unit ? ` ${lane.unit}` : ""}` : "—";
  return (
    <div className="usage-row">
      <div className="usage-head">
        <span className="usage-label">{label}</span>
        <span className="usage-pct">{valText}</span>
      </div>
    </div>
  );
}

// 单个 provider 卡片：安装/登录/用量三态。已装且登录 = 现有账号信息 + 用量泳道 + 刷新按钮 + 贴纸显示开关；
// 已装未登录 = 提示语；未装 = 一键安装按钮。
function ProviderCard({ provider, name, installed, supportsAccount, supportsApiKeyLogin, supportsProfiles, supportsContext, relay, payload, usage, err, onRefresh, installOp, onStartInstall, onCancelInstall, onLoggedIn, loginState, onStartLogin, onCancelLogin, refreshing, settings, patchSettings, onToggleQuota, usageRefreshedAt, updateInfo }: {
  provider: AgentId;
  /** 展示名，来自后端 list_agents()（产品名，不翻译）。 */
  name: string;
  /** null = 安装状态检测中（listAgents() 尚未 resolve），此时不渲染未安装/已安装的判定分支。 */
  installed: boolean | null;
  /**
   * 该 agent 有没有账号概念。false → 不显示登录态、不给登录入口。
   *
   * 不能靠 `payload == null` 推断：那既可能是「没有账号能力」，也可能是「账号还没加载出来」
   * 或「真的没登录」。三者混在一起的后果就是给没有登录入口的 agent 亮出登录按钮——它的
   * `login_argv()` 是 None，点下去只会得到一句「拉起登录失败」。
   */
  supportsAccount: boolean;
  /**
   * 该 agent 能否用 API Key 登录（gemini：OAuth 被官方停用，key 是唯一活路）。
   * true → 未登录时在「登录」旁边多一个「API Key 登录」入口。
   */
  supportsApiKeyLogin: boolean;
  /**
   * 该 agent 能否有多个账号。false（gemini：数据目录不可被环境变量覆盖）→ 不显示账号列表。
   *
   * 不能靠「列表只有一条」推断——那与「只建了默认账号」长得一模一样。
   */
  supportsProfiles: boolean;
  /** meowo 能否显示该 agent 的上下文占用。false（gemini/opencode）→ 卡片显式标注「不支持」。 */
  supportsContext: boolean;
  relay: AgentDescriptor["relay"];
  payload: ProviderAccountPayload | null;
  usage: ProviderUsage | null;
  err: "unsupported" | "error" | null;
  onRefresh: () => void;
  /** 页面级安装状态（useInstallOperations）；切换 provider 导致卡片重挂载时仍会保留。 */
  installOp: InstallOperationState | undefined;
  /** 发起后台安装（状态机在页面级）。 */
  onStartInstall: () => void;
  /** 取消进行中的安装（S-8：安装曾是不可中断的黑盒）。 */
  onCancelInstall: () => void;
  /** 登录成功后重查账号（令卡片转「已登录」并显示身份/用量）。 */
  onLoggedIn: () => void;
  /** 页面级登录状态；切换 provider 导致卡片重挂载时仍会保留。 */
  loginState: LoginOperationState | undefined;
  onStartLogin: (profile?: string | null) => Promise<boolean>;
  onCancelLogin: () => Promise<boolean>;
  refreshing: boolean;
  /** 当前应用设置，用于读取 sticker_quota_providers 开关态。 */
  settings: Settings | null;
  /** 保存模型接入方式及中转元数据。 */
  patchSettings: (p: Partial<Settings>) => Promise<string | null>;
  /** 切换本 provider 的贴纸配额显示开关。 */
  onToggleQuota: () => void;
  /** 最近一次联网刷新配额的时刻（epoch ms）；null = 本次会话尚未刷新过（只显示缓存）。 */
  usageRefreshedAt?: number | null;
  /** 页面级探测的版本/更新信息（check_agent_updates）。null = 尚未探测或探测失败——整块不渲染。 */
  updateInfo: AgentUpdateInfo | null;
}) {
  const t = useT();
  const assets = agentAssets(provider);
  const acc = payload?.account ?? null;

  // 后台安装态从页面级 useInstallOperations 派生（installOp prop）：状态曾住在本卡片里，
  // 安装中切换下拉 = 卡片卸载 + install-done 监听注销，回来显示「安装」按钮诱导二次安装，
  // 失败原因永久丢失。idle=未装可点 / installing=转圈 / error=失败可重试。
  // 不透传安装脚本的英文原始输出，进度只用 i18n 文案（随界面语言）。
  const installState: "idle" | "installing" | "error" =
    installOp?.phase === "installing"
      ? "installing"
      : installOp?.phase === "error" || (installOp?.phase === "done" && !installOp.ok)
        ? "error"
        : "idle";
  // 安装失败时的日志落点（后端把脚本输出重定向到该文件）。只展示路径，不透传英文原文。
  const installLog = installOp?.phase === "done" ? installOp.logPath : null;
  // 「打开安装日志」的失败就地提示（S-8：日志路径从纯文本改可点按钮）。
  const [logOpenMsg, setLogOpenMsg] = useState<string | null>(null);
  const openLog = () => {
    setLogOpenMsg(null);
    openInstallLog(provider)
      .catch((e) => setLogOpenMsg(t.account.installLogOpenFailed(formatBackendError(e, t.locale))));
  };
  // 后端在**跑脚本之前**就失败时的诊断（如引导脚本被 Cloudflare 人机校验拦截）。这是我们自己写的
  // 中文诊断，不是脚本的英文输出，故直接展示——此时还没有日志文件，不给这句话用户就一点线索都没有。
  const installMsg = installOp?.phase === "error" ? installOp.message : null;
  // 装好了但 bin 目录不在持久 PATH 上时，这里是那个目录——终端里敲命令会找不到。
  // 官方安装器不保证写 PATH（claude 在 Windows 上只打印一行提示就 exit 0），而 meowo 启动
  // agent 走绝对路径、察觉不到，故须显式检测并给用户一键写入。
  const [pathGapDir, setPathGapDir] = useState<string | null>(null);
  const [addingPath, setAddingPath] = useState(false);
  const [pathMsg, setPathMsg] = useState<string | null>(null);
  const [hooksStatus, setHooksStatus] = useState<HooksStatus | null>(null);
  const [repairingHooks, setRepairingHooks] = useState(false);
  const [repairMsg, setRepairMsg] = useState<string | null>(null);
  const loginBusy = loginState?.phase === "pending";
  // 登录窗口开在哪个终端由后端按 settings.resume_terminal 决定（install.rs login_agent）。
  // pending 文案必须说清「窗口在哪、最多等多久」——一句裸「等待登录…」让人干等没抓手。
  const loginTermLabels: Record<string, string> = {
    terminal: "Terminal", iterm: "iTerm2", ghostty: "Ghostty",
    wt: "Windows Terminal", wezterm: "WezTerm", powershell: "PowerShell",
    cmd: t.settings.cmdPrompt,
  };
  const loginTerm = settings?.resume_terminal ? loginTermLabels[settings.resume_terminal] ?? null : null;
  const loginMsg = loginState?.phase === "pending"
    ? loginTerm
      ? t.account.loginWaiting(loginTerm)
      : t.account.loggingIn
    : loginState?.phase === "error"
      ? loginState.action === "start" ? t.account.loginFailed : t.account.loginCancelled
      : loginState?.phase === "done" && loginState.outcome !== "success"
        ? loginState.outcome === "cancelled" ? t.account.loginCancelled : t.account.loginTimeout
        : null;
  const [logoutBusy, setLogoutBusy] = useState(false);
  const [logoutMsg, setLogoutMsg] = useState<string | null>(null);
  // 成功/失败共用 logoutMsg：成功语义不能渲染在错误红（.agent-install-error）上。
  const [logoutOk, setLogoutOk] = useState(false);
  // API Key 登录：输入区开合 + 输入值 + 保存中 + 错误。key 不落任何前端持久化，提交即清。
  const [apiKeyOpen, setApiKeyOpen] = useState(false);
  const [apiKeyValue, setApiKeyValue] = useState("");
  const [apiKeyBusy, setApiKeyBusy] = useState(false);
  const [apiKeyMsg, setApiKeyMsg] = useState<string | null>(null);
  // 刚装完（本次会话内）→ 把「登录」按钮标为下一步，把「装完 → 登录」串成一条链路。
  const [justInstalled, setJustInstalled] = useState(false);
  // 安装耗时 tick：无进度事件的一两分钟里，秒数是「还活着」的唯一证据。仅安装中运转。
  const [installNow, setInstallNow] = useState(0);
  useEffect(() => {
    if (installOp?.phase !== "installing") return;
    setInstallNow(Date.now());
    const timer = window.setInterval(() => setInstallNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [installOp]);
  const installElapsed = installOp?.phase === "installing"
    ? Math.max(0, Math.floor((installNow - installOp.startedAt) / 1000))
    : 0;

  useEffect(() => {
    let cancelled = false;
    checkProviderHooks(provider)
      .then((st) => { if (!cancelled) setHooksStatus(st); })
      .catch(() => {});
    // 挂载即查：早就装好、但从来没进过 PATH 的用户（本 bug 的多数受害者）也要看到提示，
    // 不能只在「本次装完」时查。后端对未安装的 agent 返回 null，无需先判 installed。
    agentPathGap(provider)
      .then((d) => { if (!cancelled) setPathGapDir(d); })
      .catch(() => {});
    return () => { cancelled = true; };
  }, [provider]);

  /** 把 agent 的 bin 目录写进用户级 PATH。成功后清掉提示条，并告知需重开终端。 */
  const addPath = () => {
    if (addingPath) return;
    setAddingPath(true);
    setPathMsg(null);
    addAgentToUserPath(provider)
      .then(() => {
        setPathGapDir(null);
        setPathMsg(t.account.pathAdded);
      })
      .catch(() => setPathMsg(t.account.pathAddFailed))
      .finally(() => setAddingPath(false));
  };

  const repairHooks = () => {
    if (repairingHooks) return;
    setRepairingHooks(true);
    setRepairMsg(null);
    repairProviderHooks(provider)
      .then((res) => {
        setHooksStatus(res.status);
        // 修复后仍非 installed → 接线没真正生效；按后端回传的 reason 给出精准提示
        // （如 kimi 未登录 → 「请先登录」），不再静默吞掉。
        setRepairMsg(res.status === "installed" ? null : repairFailMessage(t, res.reason));
      })
      .catch(() => setRepairMsg(repairFailMessage(t, null)))
      .finally(() => setRepairingHooks(false));
  };

  // 发起安装：状态机在页面级（useInstallOperations），卡片只是触发器。
  const startInstall = () => onStartInstall();

  // 「更新」= 重跑幂等安装（五条安装通道装的都是 latest）。仿 U0-12：重装前先看该 provider
  // 有没有活跃托管会话——更新会重拉 agent 本体，正在跑的会话可能当场出事，n>0 必须过危险
  // 确认（主按钮说后果）；查询失败按 0 处理，不挡流程。busy 态复用安装中的转圈+取消，不另造。
  const startUpdate = async () => {
    if (installState === "installing") return;
    let managed = 0;
    try {
      const page = await getLiveSessionsPage("all", null, null, 200);
      managed = page.items.filter((l) => l.connected && l.pty_managed && l.provider === provider).length;
    } catch {
      /* 查询失败按 0 处理，不挡更新 */
    }
    if (managed > 0) {
      const ok = await appConfirm(t.account.updateConfirm(managed), {
        title: t.account.update,
        danger: true,
        confirmLabel: t.account.update,
      });
      if (!ok) return;
    }
    onStartInstall();
  };

  /** 拉起交互式登录。成功 spawn 后不清 busy——等 login-done（或 5 分钟超时 / 用户取消）才落回。 */
  const startLogin = () => {
    if (loginBusy) return;
    void onStartLogin().catch(() => {});
  };

  /**
   * 取消等待。终端可能已经被关掉（用户手动关、崩溃、agent 自己退出），而后端只轮询账号文件，
   * 要 5 分钟才超时——这五分钟里按钮一直不可点。
   *
   * 不检测「终端还活着吗」：`wt.exe` 拉起窗口后自身立即退出，真正跑登录的是它的孙进程；
   * 而 `powershell -NoExit` 又会一直活着。三种终端行为不一致，靠监视进程只会时灵时不灵。
   *
   * 收尾由后端 emit 带 operationId 的 `login-done`，故这里不抢先清 busy。
   */
  const cancelLoginWait = () => {
    if (!loginBusy) return;
    void onCancelLogin().catch(() => {});
  };

  // 装完（成功）后的卡片侧后续动作。install-done 事件由页面级 useInstallOperations 消费
  // （名单重查也在那边）——卡片对 installOp 的**状态变迁**响应，安装中切走再切回也不漏：
  // installOp 在重挂后仍是 done，effect 照常触发一次。
  const installDoneHandledRef = useRef<InstallOperationState | undefined>(undefined);
  useEffect(() => {
    if (installDoneHandledRef.current === installOp) return;
    installDoneHandledRef.current = installOp;
    if (installOp?.phase !== "done" || !installOp.ok) return;
    setJustInstalled(true); // 装完通常尚未登录 → 高亮「登录」作为下一步
    // 「退出码 0」不等于「装好就能用」：claude 的安装器不写 PATH 也照样 exit 0。
    agentPathGap(provider).then(setPathGapDir).catch(() => {});
    // 后端装完顺手接了 hooks（best-effort）——重查一下，接上了就让「未接入」提示条自动消失。
    // 装完常常还没登录（kimi 的 config.toml 尚不存在），此时多半仍未接上，那就等登录后再接。
    checkProviderHooks(provider).then(setHooksStatus).catch(() => {});
  }, [installOp, provider]);

  // 成功事件由页面级状态机匹配；卡片只处理与展示相关的后续动作。
  useEffect(() => {
    if (loginState?.phase !== "done" || loginState.outcome !== "success") return;
    setJustInstalled(false);
    // 登录后配置文件才生成，是三家都接得上 hooks 的时机——后端已顺手接了，这里重查让提示条消失。
    checkProviderHooks(provider).then(setHooksStatus).catch(() => {});
  }, [loginState, provider]);

  /** 保存 API Key。后端同步落盘（写进 agent 自己的配置），成功即已登录，无需轮询等待。 */
  const saveApiKey = () => {
    const key = apiKeyValue.trim();
    if (!key || apiKeyBusy) return;
    setApiKeyBusy(true);
    setApiKeyMsg(null);
    apiKeyLogin(provider, key)
      .then(() => {
        setApiKeyOpen(false);
        setApiKeyValue("");
        onLoggedIn();
        // 后端保存时顺手接了 hooks（settings.json 此刻必然存在）——重查让「未接入」提示条消失。
        checkProviderHooks(provider).then(setHooksStatus).catch(() => {});
      })
      .catch((e) => setApiKeyMsg(t.account.apiKeyFailed(formatBackendError(e, t.locale))))
      .finally(() => setApiKeyBusy(false));
  };

  // 当前 provider 是否在贴纸配额列表中
  const inQuota = settings?.sticker_quota_providers?.includes(provider) ?? false;

  // 退出登录可逆(重新登录即可,数据不删),不弹确认——直接执行,结果就地提示。
  const startLogout = async () => {
    setLogoutBusy(true);
    setLogoutMsg(null);
    setLogoutOk(false);
    try {
      await logoutAgent(provider);
      setLogoutMsg(t.account.loggedOut);
      setLogoutOk(true);
      onLoggedIn();
    } catch (e) {
      setLogoutMsg(t.account.logoutFailed(formatBackendError(e, t.locale)));
    } finally {
      setLogoutBusy(false);
    }
  };

  // 安装态优先：未安装时一律按未安装展示（即使本地缓存了旧账号信息），
  // 只有「已安装且账号存在」才展示登录身份与用量。
  const isInstalled = installed === true;
  // 设置页切换接入方式后应立即反映，不等待账号接口下一次刷新；老后端 payload 仍作为加载前回退。
  const relayEnabled = settings?.relay?.per_agent[provider]?.enabled ?? payload?.relay_enabled ?? false;
  const isLoggedIn = isInstalled && (acc != null || relayEnabled);
  // 无账号概念的 agent：已装即到此为止——它没有「登录」这回事，所以不报「未登录」、也不给登录按钮。
  const statusBadge = relayEnabled
    ? t.account.relayBadge
    : !isInstalled
    ? installed === false
      ? t.account.notInstalled
      : null
    : acc || !supportsAccount
    ? null
    : t.account.notLoggedIn;
  // 只显示邮箱：显示名 + 邮箱 + 组织三段拼起来又长又重复（个人账号的组织名就是
  // 「<邮箱>'s Organization」）。邮箱本身已足够标识「登录的是哪个账号」。
  // 回退链兜住没有邮箱的登录方式（如 codex 的 API key，只有 login_label）。
  const desc = relayEnabled
    ? t.account.relayActive
    : isLoggedIn
    ? acc?.email ?? acc?.display_name ?? acc?.login_label ?? ""
    : installed === false
    ? t.account.installHint
    : isInstalled && supportsAccount
    ? t.account.notLoggedInHint
    : "";

  return (
    <div className="row-card provider-card" data-testid={"agent-card-" + provider}>
      <div className="provider-card-head">
        <div
          className={"provider-card-icon" + (assets.needsTile ? " provider-card-icon-tile" : "")}
          data-agent={provider}
          // claude 是 currentColor 绘制的裸 logomark，无方框时得由容器给品牌橙（--cc-claude），
          // 否则会继承文字色变灰。有方框的（若日后有）由 .provider-card-icon-tile 自己给白。
          style={!assets.needsTile && assets.tint ? { color: `var(${assets.tint})` } : undefined}
        >
          <assets.Icon />
        </div>
        <div className="provider-card-title">
          <div className="provider-card-title-row">
            <span className="provider-name">{name}</span>
            {!relayEnabled && isLoggedIn && acc?.plan && <span className="provider-badge provider-badge-plan">{acc.plan}</span>}
            {/* 活跃账号持续可见：切到过自定义账号就一直挂着这个徽章，防「切过一次就忘了」。
                默认账号（active_profile_name 为空）不显示——没建过账号的用户零感知。 */}
            {!relayEnabled && payload?.active_profile_name && (
              <span className="provider-badge" data-testid={"agent-profile-" + provider}>
                {t.account.activeProfileBadge(payload.active_profile_name)}
              </span>
            )}
            {statusBadge && <span className={"provider-badge" + (installed === false ? " provider-badge-off" : "")}>{statusBadge}</span>}
          </div>
          {/* 版本与更新入口紧贴标题行：低调一行小字。探测失败的降级全在这里收口——
              installed_version 为 null 整块不渲染；latest_version 为 null（unknown）只显当前版本、
              不给更新入口；check_agent_updates 整体失败时 updateInfo 为 null，同样什么都不显示。 */}
          {isInstalled && updateInfo?.installed_version && (
            <div className="provider-card-version" data-testid={"agent-version-" + provider}>
              <span className="provider-card-version-current">{t.account.versionCurrent(updateInfo.installed_version)}</span>
              {updateInfo.update_available && updateInfo.latest_version != null && (
                installState === "installing" ? (
                  <div className="agent-install-progress" data-testid={"agent-installing-" + provider}>
                    <RefreshIcon spinning />
                    <span className="agent-install-step">
                      {installElapsed >= 5 ? t.account.installingFor(installElapsed) : t.account.installing}
                    </span>
                    <button
                      type="button"
                      className="provider-card-action provider-card-action-sm"
                      data-testid={"agent-install-cancel-" + provider}
                      onClick={onCancelInstall}
                    >
                      {t.account.cancelInstall}
                    </button>
                  </div>
                ) : (
                  <>
                    <span className="provider-card-version-new" data-testid={"agent-update-available-" + provider}>
                      {t.account.updateAvailable(updateInfo.latest_version)}
                    </span>
                    <button
                      type="button"
                      className="provider-card-action provider-card-action-sm"
                      data-testid={"agent-update-" + provider}
                      onClick={() => void startUpdate()}
                    >
                      {t.account.update}
                    </button>
                  </>
                )
              )}
            </div>
          )}
          {/* 账号信息紧贴标题下方。邮箱可能很长，单行省略；data-tip 兜住完整值。 */}
          {desc && (
            <div className="provider-card-desc" data-tip={desc} data-testid={"agent-desc-" + provider}>
              {desc}
            </div>
          )}
        </div>
        {installed === false &&
          (installState === "installing" ? (
            <div className="agent-install-progress" data-testid={"agent-installing-" + provider}>
              <RefreshIcon spinning />
              {/* 带耗时：官方安装脚本动辄一两分钟且无进度事件，静止的「安装中…」看不出死活。 */}
              <span className="agent-install-step">
                {installElapsed >= 5 ? t.account.installingFor(installElapsed) : t.account.installing}
              </span>
              {/* 取消安装（S-8）：不可中断的黑盒必须给出口。后端杀脚本进程树/丢弃直下结果，
                  cancelled 的 install-done 到达后卡片回到「安装」按钮。 */}
              <button
                type="button"
                className="provider-card-action"
                data-testid={"agent-install-cancel-" + provider}
                onClick={onCancelInstall}
              >
                {t.account.cancelInstall}
              </button>
            </div>
          ) : (
            <button
              type="button"
              className="provider-card-action provider-card-action-primary"
              data-testid={"agent-install-" + provider}
              onClick={startInstall}
            >
              <IconDownload />
              {installState === "error" ? t.account.installRetry : t.account.install}
            </button>
          ))}
        {/* 顶部这两个按钮作用于**当前活跃账号**（卡片头部显示的正是它的信息）。下面账号列表里的
            同名按钮则针对具体某一行——两者并存不是冗余：一个是「当前账号」的快捷方式，
            一个是「哪一个账号」的精确操作。 */}
        {isInstalled && !isLoggedIn && !relayEnabled && supportsAccount && (
          <button
            type="button"
            className={"provider-card-action" + (justInstalled ? " provider-card-action-primary" : "")}
            data-testid={"agent-login-" + provider}
            // 等待中不再是死按钮：终端可能已被关掉（用户手动关/崩溃），而后端只轮询账号文件，
            // 要 5 分钟才超时。点它即取消等待，立刻落回可点状态。
            onClick={loginBusy ? cancelLoginWait : startLogin}
            data-tip={loginBusy ? t.account.cancelLogin : undefined}
          >
            {loginBusy ? t.account.cancelLogin : t.account.login}
          </button>
        )}
        {/* API Key 登录：交互式 OAuth 之外的第二入口（gemini 的 OAuth 已被官方停用，这是
            唯一走得通的一条）。点击开合输入区，不打断旁边的交互式登录等待。 */}
        {isInstalled && !isLoggedIn && !relayEnabled && supportsAccount && supportsApiKeyLogin && (
          <button
            type="button"
            className="provider-card-action"
            data-testid={"agent-api-key-login-" + provider}
            onClick={() => {
              setApiKeyMsg(null);
              setApiKeyOpen((o) => !o);
            }}
          >
            {t.account.apiKeyLogin}
          </button>
        )}
        {isInstalled && acc && !relayEnabled && (
          <button
            type="button"
            className="provider-card-action"
            data-testid={"agent-logout-" + provider}
            onClick={startLogout}
            disabled={logoutBusy}
          >
            {logoutBusy ? t.account.loggingOut : t.account.logout}
          </button>
        )}
        {isInstalled && hooksStatus && (hooksStatus === "missing" || hooksStatus === "unknown") && (
          <button
            type="button"
            className="provider-card-action"
            data-testid={"agent-repair-hooks-" + provider}
            onClick={repairHooks}
            disabled={repairingHooks}
            // S-8：按钮凭空出现零解释——tooltip 说清它做什么、何时需要点。
            data-tip={t.newSession.repairHooksTip}
          >
            {repairingHooks ? t.newSession.repairingHooks : t.newSession.repairHooks}
          </button>
        )}
      </div>

      {/* API Key 输入区：type=password——key 是机密，屏幕上不明示。Enter 提交、Esc 收起。 */}
      {apiKeyOpen && isInstalled && !isLoggedIn && (
        <div className="provider-card-body" data-testid={"agent-api-key-form-" + provider}>
          <div className="profile-add-row">
            <input
              type="password"
              className="profile-add-input"
              autoFocus
              placeholder={t.account.apiKeyPlaceholder}
              value={apiKeyValue}
              data-testid={"agent-api-key-input-" + provider}
              onChange={(e) => setApiKeyValue(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") saveApiKey();
                if (e.key === "Escape") setApiKeyOpen(false);
              }}
            />
            <button
              type="button"
              className="provider-card-action provider-card-action-primary"
              data-testid={"agent-api-key-save-" + provider}
              disabled={apiKeyBusy || !apiKeyValue.trim()}
              onClick={saveApiKey}
            >
              {apiKeyBusy ? t.account.apiKeySaving : t.account.apiKeySave}
            </button>
            <button
              type="button"
              className="provider-card-action"
              disabled={apiKeyBusy}
              onClick={() => {
                setApiKeyOpen(false);
                setApiKeyValue("");
              }}
            >
              {t.account.cancelEdit}
            </button>
          </div>
          <div className="profile-hint">{t.account.apiKeyHint}</div>
          {apiKeyMsg && (
            <div className="agent-install-error" data-testid={"agent-api-key-error-" + provider}>
              {apiKeyMsg}
            </div>
          )}
        </div>
      )}

      {/* 未安装时不给「接入方式」——还没有可运行的 agent，配官方账号还是中转都无从谈起，
          先把它装上。装好后这一段才出现。 */}
      {isInstalled && relay && (
        <RelayAccess
          agent={{ id: provider, display_name: name, relay }}
          settings={settings}
          patch={patchSettings}
        />
      )}

      {/* 安装/更新失败就地报错（无 toast 纪律）：未安装时是「安装」失败，已安装时是「更新」失败，
          同一呈现。更新入口在版本行里，失败后按钮回落可重试，错误原因必须有处可看。 */}
      {installState === "error" && (
        <div className="provider-card-body agent-install-error" data-testid={"agent-install-error-" + provider}>
          {/* 有后端诊断（被 CF 拦、取不到脚本）就显示它；否则脚本跑失败了，给通用文案 + 日志路径。 */}
          {installMsg ?? t.account.installFailed}
          {installLog && (
            <div className="agent-install-log" data-testid={"agent-install-log-" + provider}>
              {/* 可点按钮打开日志（S-8：纯文本路径没法点）；完整路径进 tooltip。 */}
              <button
                type="button"
                className="agent-path-gap-btn"
                data-testid={"agent-open-log-" + provider}
                data-tip={installLog}
                onClick={openLog}
              >
                {t.account.installLogOpen}
              </button>
              {logOpenMsg && <span className="agent-path-gap-text">{logOpenMsg}</span>}
            </div>
          )}
        </div>
      )}

      {/* 装好了却不在 PATH 上：终端里敲不出来。
          **刻意低调**：对多数人这是背景噪音（装完就在 PATH 上），横一条长提示喧宾夺主。
          正文只留一句「为什么该点」，完整路径与后果进 tooltip；按钮做成文字链接的样子。 */}
      {isInstalled && pathGapDir && (
        <div
          className="provider-card-body agent-path-gap"
          data-testid={"agent-path-gap-" + provider}
          data-tip={t.account.pathGapDetail(pathGapDir)}
        >
          <span className="agent-path-gap-text">{t.account.pathGap}</span>
          <button
            type="button"
            className="agent-path-gap-btn"
            data-testid={"agent-add-path-" + provider}
            onClick={addPath}
            disabled={addingPath}
          >
            {addingPath ? t.account.addingToPath : t.account.addToPath}
          </button>
        </div>
      )}

      {pathMsg && (
        <div className="provider-card-body agent-path-msg" data-testid={"agent-path-msg-" + provider}>
          {pathMsg}
        </div>
      )}

      {loginMsg && (
        <div className="provider-card-body agent-install-error" data-testid={"agent-login-error-" + provider}>
          {loginMsg}
        </div>
      )}

      {logoutMsg && (
        <div className={"provider-card-body " + (logoutOk ? "agent-path-msg" : "agent-install-error")} data-testid={"agent-logout-error-" + provider}>
          {logoutMsg}
        </div>
      )}

      {repairMsg && (
        <div className="provider-card-body agent-install-error" data-testid={"agent-repair-failed-" + provider}>
          {repairMsg}
        </div>
      )}


      {isLoggedIn && !relayEnabled && (
        <div className="provider-usage">
          <div className="usage-bar-head">
            <span className="usage-card-title">{t.account.quota}</span>
            {/* 数据时刻：没有它，开着设置页越久显示的越陈旧且毫无提示——用户判断
                「要不要现在跑大任务」依据的可能是十几分钟前的额度。 */}
            {usageRefreshedAt != null && (
              <span className="usage-updated">
                {t.account.usageUpdatedAt(new Intl.DateTimeFormat(t.locale, { hour: "numeric", minute: "2-digit" }).format(usageRefreshedAt))}
              </span>
            )}
            <button className="icon-btn" data-tip={t.account.refresh} aria-label={t.account.refresh} disabled={refreshing || err === "unsupported" || (!(payload?.usage_supported ?? false) && !usage)} onClick={onRefresh}>
              <RefreshIcon spinning={refreshing} />
            </button>
          </div>
          {usage ? (
            <>
              {usage.lanes.map((lane, i) => (
                <UsageBar key={`${lane.kind}-${i}`} lane={lane} label={laneLabel(lane, t)} />
              ))}
              {usage.note && <div className="usage-extra">{renderNote(usage.note, t)}</div>}
              {err === "error" && <div className="usage-stale">{t.account.refreshFailed}</div>}
            </>
          ) : !(payload?.usage_supported ?? false) || err === "unsupported" ? (
            <div className="usage-stale">{t.account.usageUnsupported}</div>
          ) : err === "error" ? (
            <div className="usage-stale">{t.account.usageUnavailable}</div>
          ) : (
            <div className="usage-stale">{t.account.loading}</div>
          )}
          {/* 贴纸配额显示开关 */}
          <div className="usage-sticker-row">
            <span className="usage-sticker-label">{t.settings.showQuotaOnSticker}</span>
            <Switch checked={inQuota} onChange={onToggleQuota} label={t.settings.showQuotaOnSticker} />
          </div>
        </div>
      )}

      {/* 能力如实告知：上下文占用不支持时明写「不支持」，不留空白让用户以为是 bug。
          只在已装时显示——没装谈不上能力。 */}
      {isInstalled && !supportsContext && (
        <div className="provider-cap-note" data-testid={"agent-context-unsupported-" + provider}>
          <svg className="provider-cap-note-ico" width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            <circle cx="12" cy="12" r="9.5" />
            <path d="M12 11v5" />
            <path d="M12 7.5h.01" />
          </svg>
          <span>{t.account.contextUnsupported}</span>
        </div>
      )}


      {/* 多账号：已装且支持时才给。不支持的（gemini）连列表都不显示——它只有一个账号，
          列一个孤零零的「默认账号」除了占地方没有任何信息。
          中转模式下也不给：账号列表管的是官方登录身份，中转与之互斥，列出来只会误导。 */}
      {isInstalled && supportsProfiles && !relayEnabled && (
        <ProfileList
          provider={provider}
          onChanged={onLoggedIn}
          loginState={loginState}
          onStartLogin={onStartLogin}
          onCancelLogin={onCancelLogin}
        />
      )}
    </div>
  );
}

/**
 * 某个 agent 的账号列表：默认账号 + 自定义账号，可切换 / 登录 / 删除 / 添加。
 *
 * 「默认账号」是隐式的（`id === null`）——它就是 agent 自己的目录（`~/.claude`），不可删除。
 * 没建过任何自定义账号的用户，这里只会看到它一条 + 一个「添加账号」按钮。
 */
/** 默认账号在前端的行 key —— 后端给的 id 是 `null`（它不在 settings.profiles 里）。 */
const DEFAULT_KEY = "__default__";

function ProfileList({ provider, onChanged, loginState, onStartLogin, onCancelLogin }: {
  provider: AgentId;
  onChanged: () => void;
  loginState: LoginOperationState | undefined;
  onStartLogin: (profile?: string | null) => Promise<boolean>;
  onCancelLogin: () => Promise<boolean>;
}) {
  const t = useT();
  const [rows, setRows] = useState<ProfileView[] | null>(null);
  const [adding, setAdding] = useState(false);
  const [name, setName] = useState("");
  // 正在改名的**行 key**（null = 没在改名）。默认账号用 DEFAULT_KEY——它没有 profile id，
  // 但照样可以改名（名字只是个显示串，不碰任何文件）。
  const [editing, setEditing] = useState<string | null>(null);
  const [editName, setEditName] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  const reload = useCallback(() => {
    listProfiles(provider)
      .then(setRows)
      .catch(() => setRows([]));
  }, [provider]);

  useEffect(reload, [reload]);

  // 登录完成 → 该账号的登录态变了，重查。
  useTauriEvent("login-done", () => reload());

  const run = async (fn: () => Promise<unknown>) => {
    if (busy) return;
    setBusy(true);
    setErr(null);
    try {
      await fn();
      reload();
      onChanged();
    } catch (e) {
      setErr(formatBackendError(e, t.locale));
    } finally {
      setBusy(false);
    }
  };

  const add = () => {
    const n = name.trim();
    if (!n) return;
    run(async () => {
      await createProfile(provider, n);
      setName("");
      setAdding(false);
    });
  };

  /**
   * 删除账号（连同它的目录）。**活跃账号也能删** —— 后端会把活跃标记落回默认账号。
   *
   * 确认框走 `appConfirm`（应用主题原生小窗，全应用统一），**不是 `window.confirm`**：
   * 后者在 Tauri 的 webview 里会被直接吞掉，返回值恒为 false。
   */
  const remove = async (p: ProfileView) => {
    if (!p.id) return; // 默认账号是 agent 自己的目录，删不得
    const label = p.name || p.id;
    const yes = await appConfirm(t.account.deleteProfileConfirm(label), {
      title: t.account.deleteProfile,
      danger: true,
      // 主按钮说清后果（S-14）：不可逆操作不给通用「确定」。
      confirmLabel: t.account.deleteProfile,
    });
    if (!yes) return;
    run(() => deleteProfile(provider, p.id!));
  };

  /**
   * 合并进默认账号：会话与历史数据并入默认账号，账号本身从列表消失——与删除的根本区别是
   * **数据全留下**（删除是连目录一起抹掉）。有进行中的会话时后端会拒绝，错误就地显示。
   *
   * 不可逆，故与删除同规：走 appConfirm 确认。
   */
  const merge = async (p: ProfileView) => {
    if (!p.id) return; // 默认账号没有「合并进自己」
    const label = p.name || p.id;
    const yes = await appConfirm(t.account.mergeProfileConfirm(label), {
      title: t.account.mergeProfile,
      danger: true,
      confirmLabel: t.account.mergeProfile,
    });
    if (!yes) return;
    run(() => mergeProfileIntoDefault(provider, p.id!));
  };

  /**
   * 退出登录。**与删除账号不是一回事**：登出只清凭据，目录、配置、会话历史都留着，之后还能登回来。
   * 可逆操作不设确认，直接执行（失败由 run 就地显示）。
   */
  const logout = (p: ProfileView) => {
    run(() => logoutAgent(provider, p.id));
  };

  /**
   * 改名。只动展示名，**不动 id**（它是目录名，改了就等于换了个账号）。
   *
   * 默认账号也能改：它的 id 是 null，名字单独存在 settings 的 `default_profile_names` 里。
   */
  const commitRename = (p: ProfileView) => {
    const next = editName.trim();
    setEditing(null);
    if (!next || next === p.name) return;
    run(() => renameProfile(provider, p.id, next));
  };

  if (!rows) return null;

  return (
    <div className="provider-card-body profile-list" data-testid={"profiles-" + provider}>
      <div className="profile-list-head">{t.account.profiles}</div>

      {rows.map((p) => {
        const key = p.id ?? DEFAULT_KEY;
        const profileLoginPending = loginState?.phase === "pending" && loginState.profile === p.id;
        const anotherLoginPending = loginState?.phase === "pending" && !profileLoginPending;
        // 登录态：有账号信息就是登录了。邮箱 > 登录方式标签（codex 的 "API Key"、
        // opencode 的 "Anthropic, OpenAI" 一串 provider、kimi 的 userId 短码）。
        //
        // 描述行说的是**这是哪个账号**，套餐不在此列——它是徽章（见下），与卡片标题那排一致。
        const desc =
          p.account?.email ??
          p.account?.display_name ??
          p.account?.login_label ??
          t.account.notLoggedIn;
        // 正在改名的那一行：整行让位给输入框（其余按钮此时无从谈起）。
        if (editing === key) {
          return (
            <div key={key} className="profile-row profile-add-row">
              <input
                className="profile-add-input"
                autoFocus
                value={editName}
                data-testid={"profile-rename-input-" + provider + "-" + key}
                onChange={(e) => setEditName(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") commitRename(p);
                  if (e.key === "Escape") setEditing(null);
                }}
                onBlur={() => commitRename(p)}
              />
              <button
                type="button"
                className="provider-card-action"
                data-testid={"profile-rename-cancel-" + provider + "-" + key}
                // onMouseDown：抢在 input 的 onBlur（会提交）之前把改名取消掉。
                onMouseDown={(e) => {
                  e.preventDefault();
                  setEditing(null);
                }}
              >
                {t.account.cancelEdit}
              </button>
            </div>
          );
        }
        return (
          <div
            key={key}
            className={"profile-row" + (p.active ? " profile-row-active" : "")}
            data-testid={"profile-" + provider + "-" + key}
          >
            <button
              type="button"
              className="profile-row-main"
              data-tip={t.account.switchProfile}
              disabled={busy || p.active}
              onClick={() => run(() => setActiveProfile(provider, p.id))}
            >
              <span className="profile-name-row">
                <span className="profile-name">{p.name || t.account.defaultProfile}</span>
                {/* 套餐/会员等级：徽章，不写进描述行——那一行是「这是哪个账号」。
                    kimi 的等级由用量接口捎回（本地读不到），故只有活跃账号拿得到。 */}
                {p.account?.plan && (
                  <span className="profile-badge profile-badge-plan">{p.account.plan}</span>
                )}
              </span>
              <span className="profile-desc" data-tip={desc}>
                {desc}
              </span>
            </button>

            {p.active && <span className="profile-badge">{t.account.activeProfile}</span>}

            {/* 登录是未登录时的主操作，留作显眼的按钮；其余（退出/改名/删除）收进菜单。
                **必须带上这一行自己的 id**：漏了它，登录会把凭据写进默认账号（用户以为加了个账号，
                其实把原来那个覆盖了）。 */}
            {!p.account && (
              <button
                type="button"
                className="provider-card-action"
                data-testid={"profile-login-" + provider + "-" + key}
                disabled={busy || anotherLoginPending}
                onClick={() => {
                  setErr(null);
                  const request = profileLoginPending ? onCancelLogin() : onStartLogin(p.id);
                  void request.catch((e) => setErr(formatBackendError(e, t.locale)));
                }}
              >
                {profileLoginPending ? t.account.cancelLogin : t.account.login}
              </button>
            )}

            <ActionMenu
              label={t.account.profileActions}
              testId={"profile-menu-" + provider + "-" + key}
              items={[
                // 登出 ≠ 删除：它只清凭据，目录、配置、会话历史都留着，之后还能登回来。
                // 默认账号更是**只有**这一条退出路径——它是 agent 自己的目录，删不掉。
                ...(p.account
                  ? [{ key: "logout", label: t.account.logout, onSelect: () => logout(p) }]
                  : []),
                // 默认账号**也能改名**：名字只是个显示串，不碰任何文件。两个账号里有一个永远
                // 叫「默认账号」，用起来很别扭。
                {
                  key: "rename",
                  label: t.account.renameProfile,
                  onSelect: () => {
                    setEditName(p.name);
                    setEditing(p.id ?? DEFAULT_KEY);
                  },
                },
                // 删除只给自定义账号。默认账号是 agent 自己的目录（`~/.claude`）——删它等于抹掉
                // 用户的凭据、配置和**全部会话历史**，那不是 meowo 该替他做的决定。
                ...(p.id
                  ? [
                      // 合并也只给自定义账号：它是「数据已拆散」的自救出口，默认账号无需合并进自己。
                      {
                        key: "merge",
                        label: t.account.mergeProfile,
                        onSelect: () => merge(p),
                      },
                      {
                        key: "delete",
                        label: t.account.deleteProfile,
                        danger: true,
                        onSelect: () => remove(p),
                      },
                    ]
                  : []),
              ]}
            />
          </div>
        );
      })}

      {adding ? (
        <div className="profile-add-row">
          <input
            className="profile-add-input"
            autoFocus
            placeholder={t.account.newProfileName}
            value={name}
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") add();
              if (e.key === "Escape") setAdding(false);
            }}
          />
          <button
            type="button"
            className="provider-card-action provider-card-action-primary"
            data-testid={"profile-add-confirm-" + provider}
            disabled={busy || !name.trim()}
            onClick={add}
          >
            {t.account.addProfile}
          </button>
          {/* 反悔的出口。Esc 也行，但一个明摆着的按钮不该让人去猜。 */}
          <button
            type="button"
            className="provider-card-action"
            data-testid={"profile-add-cancel-" + provider}
            disabled={busy}
            onClick={() => {
              setAdding(false);
              setName("");
            }}
          >
            {t.account.cancelEdit}
          </button>
        </div>
      ) : (
        <button
          type="button"
          className="profile-add-btn"
          data-testid={"profile-add-" + provider}
          disabled={busy}
          onClick={() => setAdding(true)}
        >
          + {t.account.addProfile}
        </button>
      )}

      <div className="profile-hint">{t.account.addProfileHint}</div>
      {/* 覆盖面明示：切换只影响此后拉起的会话（后端 profile/mod.rs 的既定行为，此前 UI 零提示）。 */}
      <div className="profile-hint">{t.account.switchCoverage}</div>
      {err && <div className="agent-install-error">{err}</div>}
    </div>
  );
}

/** 顶部模型切换记住上次选择的 localStorage 键。 */
const SELECTED_AGENT_KEY = "meowo-account-agent";

export function AccountSection() {
  const t = useT();
  // 读取/写入应用设置（用于贴纸配额开关）。patchError 与其它分区同款：保存被拒时
  // 开关会被回读弹回去，必须有一行说明（S-3）。
  const [settings, patchSettings, patchError, clearPatchError] = useSettingsState();
  // 顶部下拉选中的 agent id。模型一多，全部竖排要滚半天——改为一次只看一张卡。
  // 记进 localStorage，跨次打开设置页仍停在上次那个。
  const [selectedAgent, setSelectedAgent] = useState<string | null>(() => {
    try { return localStorage.getItem(SELECTED_AGENT_KEY); } catch { return null; }
  });
  const pickAgent = (id: string) => {
    setSelectedAgent(id);
    try { localStorage.setItem(SELECTED_AGENT_KEY, id); } catch { /* 隐私模式禁写：仅失去记忆，不影响切换 */ }
  };
  const [payloads, setPayloads] = useState<ProviderAccountPayload[]>([]);
  // usageMap: provider key → 最新 ProviderUsage（缓存先填，联网值覆盖）
  const [usageMap, setUsageMap] = useState<Record<string, ProviderUsage>>({});
  const [refreshingSet, setRefreshingSet] = useState<Set<string>>(new Set());
  // errMap: provider key → 错误类型（unsupported/error/null）
  const [errMap, setErrMap] = useState<Record<string, "unsupported" | "error" | null>>({});
  // agents: 后端下发的 agent 名单（含展示名与安装态）。前端不再自己维护这份名单。
  // 初值 null = 检测中：首帧不判定任何一张卡为未安装，避免 listAgents() resolve 前误闪「未安装 + 安装按钮」。
  const [agents, setAgents] = useState<AgentDescriptor[] | null>(null);
  // 首次名单查询失败标记：null + 失败曾直接渲染整页空白，分不清「没装任何 agent」和「坏了」。
  const [agentsError, setAgentsError] = useState(false);
  // 账号数据（getAccounts）首读失败标记：失败曾静默，已登录会显示成「未登录」。
  const [accountsError, setAccountsError] = useState(false);
  const installed = agents === null ? null : new Set(agents.filter((a) => a.installed).map((a) => a.id));
  // provider → 版本/更新探测结果。探测失败静默降级为空：版本与更新入口整块不出现，
  // 不红字、不显示任何错误——版本探测是锦上添花，不该打扰账号页主流程。
  const [updateMap, setUpdateMap] = useState<Record<string, AgentUpdateInfo>>({});
  const refreshAgentUpdates = () => {
    checkAgentUpdates()
      .then((list) => {
        const next: Record<string, AgentUpdateInfo> = {};
        list.forEach((info) => { next[info.provider] = info; });
        setUpdateMap(next);
      })
      .catch(() => {});
  };
  // 重查 agent 名单（安装态会变）。挂载、窗口聚焦、后台安装成功各处复用。
  // 名单到手后一并刷新版本/更新探测（后端有 10 分钟缓存，聚焦高频不会扇出；
  // 后端在安装成功后失效版本缓存，install-done 走到这里正好拿到新值）。
  const refreshInstalled = () => {
    listAgents()
      .then((list) => {
        setAgents(list);
        setAgentsError(false);
        refreshAgentUpdates();
      })
      .catch(() => setAgentsError(true));
  };
  useEffect(() => { refreshInstalled(); }, []);
  // 窗口聚焦：重查安装态 + 账号态。用户常在外部终端完成登录后切回来——此前只查安装态，
  // 登录超时文案指向的「刷新」按钮在未登录态根本不渲染，登录完成只有重开设置窗才被看见。
  // 只回填缓存、不逐 provider 联网刷配额（那是挂载/登录成功时的事，聚焦高频不该扇出网络请求）。
  useEffect(() => {
    const onFocus = () => {
      refreshInstalled();
      getAccounts()
        .then((ps) => {
          setAccountsError(false); // 聚焦重试成功即撤掉加载失败横幅
          setPayloads(ps);
          setUsageMap((m) => {
            const next = { ...m };
            ps.forEach((p) => { if (p.usage && !next[p.provider]) next[p.provider] = p.usage; });
            return next;
          });
        })
        .catch(() => {});
    };
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, []);

  // 切换某 provider 在贴纸配额列表中的开关状态
  const toggleQuotaProvider = (provider: string) => {
    if (!settings) return;
    const list = settings.sticker_quota_providers ?? [];
    const next = list.includes(provider)
      ? list.filter((p) => p !== provider)
      : [...list, provider];
    patchSettings({ sticker_quota_providers: next });
  };

  // provider → 最近一次联网刷新成功的时刻（本次会话内）。缓存预填不算——它的年龄未知。
  const [usageRefreshedAt, setUsageRefreshedAt] = useState<Record<string, number>>({});
  const doRefresh = (provider: string) => {
    setRefreshingSet((s) => new Set([...s, provider]));
    setErrMap((m) => ({ ...m, [provider]: null }));
    const startedAt = Date.now();
    refreshUsage(provider)
      .then((u) => {
        setUsageMap((m) => ({ ...m, [provider]: u }));
        setUsageRefreshedAt((m) => ({ ...m, [provider]: Date.now() }));
      })
      .catch((e) => {
        const unsupported = String(e).includes("USAGE_UNSUPPORTED");
        setErrMap((m) => ({ ...m, [provider]: unsupported ? "unsupported" : "error" }));
      })
      .finally(() => {
        // 最短转 500ms：本地(codex)/缓存(60s 内)刷新近乎瞬时，否则 spinner 一闪即逝、看不见动画。
        const wait = Math.max(0, 500 - (Date.now() - startedAt));
        setTimeout(() => {
          setRefreshingSet((s) => { const n = new Set(s); n.delete(provider); return n; });
        }, wait);
      });
  };

  // 先从 getAccounts 拿缓存数据快速渲染，再对每个 usage_supported provider 联网刷新。
  // 挂载时与登录成功后各调一次（登录成功前该 provider 的 account 为 null，卡片显示「未登录」）。
  const loadAccounts = () => {
    getAccounts()
      .then((ps) => {
        setAccountsError(false);
        setPayloads(ps);
        // 用缓存 usage 预填
        const initial: Record<string, ProviderUsage> = {};
        ps.forEach((p) => { if (p.usage) initial[p.provider] = p.usage; });
        setUsageMap(initial);
        // 对支持用量的 provider 发起联网刷新。过滤与贴纸对齐（Sticker.tsx 同款）：
        // 未登录/走中转的 provider 发请求纯属浪费，还会把失败写进 errMap 误报。
        ps.filter((p) => p.account != null && p.usage_supported && !p.relay_enabled)
          .forEach((p) => doRefresh(p.provider));
      })
      // 失败不能静默：缓存 payloads 为空时卡片会把已登录显示成「未登录」。
      .catch(() => setAccountsError(true));
  };
  // 挂在 AccountSection 而不是 keyed ProviderCard 内：切换下拉项不会遗失仍在后端等待的 operationId。
  const loginOperations = useLoginOperations((event) => {
    if (event.outcome === "success") loadAccounts();
  });
  // 安装状态同理挂页面级：安装动辄一两分钟，切换下拉不能丢转圈态/失败原因（登录早修过同一坑）。
  const installOperations = useInstallOperations((event) => {
    if (event.ok) refreshInstalled();
  });
  useEffect(() => { loadAccounts(); }, []);
  // 配额自动刷新：与贴纸同节奏（5 分钟）。设置页开着不动时数据不再越放越陈旧。
  // 过滤条件与贴纸对齐：已登录 + 支持用量 + 未走中转（未登录/中转发请求纯属浪费还写 errMap）。
  const payloadsRef = useRef(payloads);
  payloadsRef.current = payloads;
  useEffect(() => {
    const id = window.setInterval(() => {
      payloadsRef.current
        .filter((p) => p.account != null && p.usage_supported && !p.relay_enabled)
        .forEach((p) => doRefresh(p.provider));
    }, 5 * 60_000);
    return () => window.clearInterval(id);
    // doRefresh 每次渲染新建但只用稳定 setter，按挂载一次即可。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 三态区分（曾一律 return null 整页空白）：检测中给文案占位；失败给重试；
  // 名单真空（registry 恒非空，纯保底）给空态说明。
  if (agents === null) {
    return agentsError ? (
      <div className="sec-hint" role="alert">
        {t.account.agentsLoadFailed}
        <button type="button" className="provider-card-action" style={{ marginLeft: 8 }} onClick={refreshInstalled}>
          {t.sticker.retry}
        </button>
      </div>
    ) : (
      <div className="sec-hint">{t.newSession.detectingAgents}</div>
    );
  }
  const rawList = agents;
  if (rawList.length === 0) return <div className="sec-hint">{t.newSession.noAgents}</div>;

  // 下拉里**已安装的排前面**，未安装的沉底——用户多半只装了一两个，没装的不该混在中间碍事。
  // 用分区而非 sort：分区天然稳定，同组内保持后端注册顺序，不依赖引擎的 sort 稳定性。
  const list = [
    ...rawList.filter((a) => a.installed),
    ...rawList.filter((a) => !a.installed),
  ];

  // 默认选中项：优先用记住的那个；否则选**首个已安装**的（沉底的没装的不该被默认选中）；
  // 一个都没装才退到第一个。只依赖 installed（同步可得），不掺登录态（异步）——否则账号加载完
  // 默认项会跳变，正显示的卡片会莫名切换。
  const eff =
    (selectedAgent && list.some((a) => a.id === selectedAgent) ? selectedAgent : null) ??
    list.find((a) => a.installed)?.id ??
    list[0].id;
  const cur = list.find((a) => a.id === eff)!;
  const payload = payloads.find((x) => x.provider === eff) ?? null;

  // 顶部下拉切换 + 仅渲染选中的那一张卡（每张卡按 installed/payload 自渲染未装/未登录/已登录三态）。
  return (
    <>
      {/* 账号数据首读失败的细横幅：不替换卡片内容（卡片按缓存/空态照常渲染），只给重试入口。 */}
      {accountsError && (
        <div className="sec-hint proxy-err" role="alert">
          {t.account.accountsLoadFailed}
          <button type="button" className="provider-card-action" style={{ marginLeft: 8 }} onClick={loadAccounts}>
            {t.sticker.retry}
          </button>
        </div>
      )}
      <div className="account-agent-switch">
        <Dropdown
          value={eff}
          options={list.map((a) => {
            // claude 的徽标是 currentColor 绘制的裸 logomark，得由容器给它品牌色（--cc-claude），
            // 否则在下拉里会继承文字色、变成灰白。自带固定色的（kimi/codex/gemini/opencode）tint 为空。
            const { Icon, tint } = agentAssets(a.id);
            const icon = (
              <span style={{ display: "flex", color: tint ? `var(${tint})` : undefined }}>
                <Icon />
              </span>
            );
            // 未安装的置灰：仍可选（点进去就是安装入口），但一眼看得出「还没配」。
            // 后台安装中的在标签上注明——安装态在页面级，切去看别的 agent 也不会丢。
            const installing = installOperations.states.get(a.id)?.phase === "installing";
            const label = installing ? `${a.display_name} · ${t.account.installing}` : a.display_name;
            return { value: a.id, label, icon, muted: !a.installed };
          })}
          onChange={pickAgent}
        />
      </div>
      <ProviderCard
        key={cur.id}
        provider={cur.id}
        name={cur.display_name}
        installed={installed === null ? null : installed.has(cur.id)}
        supportsAccount={cur.supports_account}
        supportsApiKeyLogin={cur.supports_api_key_login ?? false}
        supportsProfiles={cur.supports_profiles}
        supportsContext={cur.supports_context ?? true}
        relay={cur.relay}
        payload={payload}
        usage={usageMap[cur.id] ?? null}
        err={errMap[cur.id] ?? null}
        onRefresh={() => doRefresh(cur.id)}
        installOp={installOperations.states.get(cur.id)}
        onStartInstall={() => installOperations.start(cur.id)}
        onCancelInstall={() => installOperations.cancel(cur.id)}
        onLoggedIn={loadAccounts}
        loginState={loginOperations.states.get(cur.id)}
        onStartLogin={(profile) => loginOperations.start(cur.id, { profile })}
        onCancelLogin={() => loginOperations.cancel(cur.id)}
        refreshing={refreshingSet.has(cur.id)}
        settings={settings}
        patchSettings={patchSettings}
        onToggleQuota={() => toggleQuotaProvider(cur.id)}
        usageRefreshedAt={usageRefreshedAt[cur.id] ?? null}
        updateInfo={updateMap[cur.id] ?? null}
      />
      {patchError && (
        <div className="sec-hint proxy-err" role="alert">
          {patchError}
          {/* 同款内联 ×（About.tsx SettingsError）：错误不会自己消失（下一次成功才清），
              用户看完得有办法关掉；复用 useSettingsState 的 clearError 通道。 */}
          <button
            type="button"
            aria-label={t.settings.close}
            style={{ appearance: "none", background: "none", border: "none", padding: 0, marginLeft: 6, font: "inherit", color: "inherit", cursor: "pointer" }}
            onClick={clearPatchError}
          >
            ×
          </button>
        </div>
      )}
    </>
  );
}

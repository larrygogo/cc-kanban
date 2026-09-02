import { type ReactElement, useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useShowWhenReady } from "../useShowWhenReady";
import {
  type AgentId,
  type AgentDescriptor,
  type HooksStatus,
  newSession,
  recentCwds,
  checkProviderHooks,
  repairProviderHooks,
  getSettings,
  listAgents,
  agentName,
  getAccounts,
  isLoggedIn,
  listSubdirectories,
  type DirListing,
} from "../api";
import { agentAssets, tintStyle } from "../providers";
import { normalizePath, pathKey, unquotePath } from "../paths";
import { Dropdown } from "./menu";
import { useAgentListRefresh } from "../useAgents";
import { useTauriEvent } from "../hooks/useTauriEvent";
import { useLoginOperations } from "../hooks/useLoginOperations";
import { useT, repairFailMessage } from "../i18n";
import { formatBackendError } from "../i18n/errors";
import { useEscClose } from "../hooks/useEscClose";
import { remoteUi } from "../remoteMode";
import { isMac } from "../platform";

function FolderIcon() {
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z" />
    </svg>
  );
}

/** 独立窗口页（label="new-session"）：新建一个全新会话。成功后自关；
 *  「正在启动」的反馈由看板的负 id 占位卡承担（pending PTY 合成，见 session_query）。 */

const qs = new URLSearchParams(window.location.search);
const initialCwd = normalizePath(qs.get("cwd") ?? "");
const initialProvider: AgentId | null = qs.get("provider");

/** 启动选项记忆（provider → {option id → choice id}）。常用「跳过权限确认 / 指定模型」的
 *  用户每开一个会话都要重选两次下拉，且极易忘记而以默认档起会话——恢复路径早有持久化
 *  （会话存档回放），新建路径此前完全没有。localStorage 即可：这是 UI 预填偏好，
 *  未知 id 由后端声明表忽略/落默认，脏数据无害。 */
const LAUNCH_OPTS_KEY = "meowo-launch-selections";
function loadStoredOpts(): Record<string, Record<string, string>> {
  try {
    const raw = JSON.parse(localStorage.getItem(LAUNCH_OPTS_KEY) ?? "{}");
    return raw && typeof raw === "object" && !Array.isArray(raw) ? raw : {};
  } catch {
    return {};
  }
}
function saveStoredOpts(provider: string, opts: Record<string, string>) {
  try {
    localStorage.setItem(LAUNCH_OPTS_KEY, JSON.stringify({ ...loadStoredOpts(), [provider]: opts }));
  } catch {
    /* 隐私模式禁写：只失去记忆 */
  }
}

export function NewSessionPanel({ onClose, prefill, onLaunched }: {
  onClose?: () => void;
  /** 远程页内打开时的预填(桌面走 URL query / ns-prefill 事件,远程两条都不通,改走 props)。 */
  prefill?: { cwd?: string | null; provider?: string | null };
  /** 启动成功回调(临时负 id,桌面恒 null)。远程 RemoteApp 借此选中新会话;桌面不传,
   *  reveal 开窗即导航,无需此通道。 */
  onLaunched?: (tempId: number | null) => void;
} = {}): ReactElement {
  const startCwd = prefill?.cwd != null ? normalizePath(prefill.cwd) : initialCwd;
  const startProvider = (prefill?.provider ?? initialProvider) as AgentId | null;
  const t = useT();
  // 窗口以 visible:false 创建（window.rs），首帧渲染后再显示，消除打开瞬间的白框闪烁。
  useShowWhenReady();
  const [cwd, setCwd] = useState(startCwd);
  // 7T-5：面板打开时没有任何初始焦点——目录已经预填好了，回车却什么都不发生（键盘用户
  // 得先 Tab 若干次才摸到控件）。挂载即聚焦目录框；已有预填时连带全选，一次输入就能整段
  // 改写，回车直接启动（Enter 与按钮共用 canLaunch 守卫，见 launch）。
  const dirInputRef = useRef<HTMLInputElement>(null);
  useEffect(() => {
    const el = dirInputRef.current;
    if (!el) return;
    el.focus();
    if (el.value) el.select();
  }, []);
  // 首帧种子（settings.default_agent resolve 前）。真实默认值由后端给。
  const [provider, setProvider] = useState<AgentId>(startProvider ?? "claude");
  const [recent, setRecent] = useState<string[]>([]);
  const [hooks, setHooks] = useState<Record<string, HooksStatus>>({});
  const [busy, setBusy] = useState(false);
  // state 要到下一次 render 才更新；同一事件批次里的双击必须用 ref 同步挡住第二次 IPC。
  const launchPendingRef = useRef(false);
  const [repairing, setRepairing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // agents: 后端下发的名单（null = 尚未 resolve）。avail = 其中已安装的那些。
  const [agents, setAgents] = useState<AgentDescriptor[] | null>(null);
  const avail = agents === null ? null : agents.filter((a) => a.installed).map((a) => a.id);
  // 启动选项的选择（option id → choice id）。选项表由插件经 descriptor 声明；换 agent 清空
  // ——不同 agent 的选项 id 可能撞名（如都叫 approval）但语义不同，残留会串。
  const [opts, setOpts] = useState<Record<string, string>>({});
  // 附加目录(一个需求跨 N 仓 = 一个会话 + --add-dir):agent 在同一上下文里看到
  // 所有仓,跨仓改动自己协调——不必开 N 个会话再让用户当人肉消息总线。
  const [extraDirs, setExtraDirs] = useState<string[]>([]);
  // null = 尚未拿到账号（或取不到）→ 不显示未登录提示，避免误闪/误报。
  const [loggedIn, setLoggedIn] = useState<Record<string, boolean> | null>(null);
  // 各 provider 当前活跃账号的展示名（null = 默认账号）。非默认账号活跃时在启动按钮旁提示，
  // 防「切过一次账号就忘了，新会话全写进隔离账号」。
  const [activeProfiles, setActiveProfiles] = useState<Record<string, string | null>>({});
  // relay 启用的 provider → 中转固定的模型。中转的 --model 在 argv 里压轴（terminal.rs），
  // 面板选的模型静默不生效——model 档置灰并注明固定值，不留「选了却没生效」的坑。
  const [relayPins, setRelayPins] = useState<Record<string, string>>({});
  const loginOperations = useLoginOperations((event) => {
    const p = event.provider;
    // 登录成功与否是该 provider 的客观事实，与当前选中谁无关。
    // 取消时后端也会再查一次账号——用户可能已经在终端里登完了，只是嫌等得慢。
    if (event.outcome === "success") {
      setLoggedIn((m) => ({ ...(m ?? {}), [p]: true }));
    }
    // 提示只对当前看着的那个 agent 显示，免得用户莫名看到别人的报错。
    if (p !== provider) return;
    setError(event.outcome === "success"
      ? null
      : event.outcome === "cancelled" ? t.newSession.loginCancelled : t.newSession.loginTimeout);
  });

  // 窗口已开时从另一张卡片再点「新建会话」：后端发 ns-prefill 更新表单（不重开窗口）。
  useTauriEvent<{ cwd?: string | null; provider?: string | null }>("ns-prefill", (e) => {
    if (e.payload.cwd != null) setCwd(normalizePath(e.payload.cwd));
    if (e.payload.provider != null) setProvider(e.payload.provider);
  });

  // agent 名单由后端下发。拿到后再据此查 hooks 接线状态与登录态——前端不再自带一份 agent 列表。
  // 失败时保持 agents=null（未探测），UI 既不显示「未检测到已安装」也不禁用启动。
  const reloadAgents = () => {
    listAgents()
      .then((list) => {
        setAgents(list);
        for (const { id } of list) {
          checkProviderHooks(id)
            .then((st) => setHooks((h) => ({ ...h, [id]: st })))
            .catch(() => {});
        }
        // 登录态：账号能解析出来就算已登录。取不到就保持 null（不提示），宁可不打扰也不误报未登录。
        //
        // 只给**有账号能力**的 agent 记登录态。`getAccounts()` 压根不会返回没声明该能力的 agent
        // （gemini / opencode），而「查不到行」≠「未登录」——它是「无账号概念，无从谈起」。
        // 曾经把两者混为一谈：查不到 → isLoggedIn(undefined) → false → 亮出登录入口 → 点下去，
        // 后端 `login_argv()` 却是 None，只能报「拉起登录失败」。留 undefined，needLogin 即为 false。
        Promise.all([getAccounts(), getSettings()])
          .then(([rows, settings]) => {
            const m: Record<string, boolean> = {};
            const ap: Record<string, string | null> = {};
            for (const { id } of list) {
              const row = rows.find((r) => r.provider === id);
              if (row) m[id] = isLoggedIn(row);
              ap[id] = row?.active_profile_name ?? null;
            }
            setLoggedIn(m);
            setActiveProfiles(ap);
            // relay_enabled 与后端 augment_argv 同一条判定（含密钥已存）；固定值取设置的
            // 中转模型。enabled 但取不到 model 时宁可不置灰（误锁比漏提示更糟）。
            const pins: Record<string, string> = {};
            for (const row of rows) {
              const rule = settings.relay?.per_agent?.[row.provider as AgentId];
              if (row.relay_enabled && rule?.model) pins[row.provider] = rule.model;
            }
            setRelayPins(pins);
          })
          .catch(() => setLoggedIn(null));
      })
      .catch(() => {});
  };

  useEffect(() => {
    // 若从会话卡片菜单带 provider 参数打开，保留该参数；否则回退到设置里的默认 agent。
    if (!startProvider) {
      getSettings()
        .then((s) => setProvider(s.default_agent))
        .catch(() => {});
    }
    recentCwds(8)
      .then((list) => {
        // 后端按原始字符串去重；同一目录可能因历史数据斜杠方向不同而重复。
        // 前端 normalize 后再按大小写不敏感（Windows）去重一次。
        const seen = new Set<string>();
        return list
          .map(normalizePath)
          .filter((p) => {
            const key = pathKey(p);
            if (seen.has(key)) return false;
            seen.add(key);
            return true;
          });
      })
      .then((list) => {
        setRecent(list);
        // 无 prefill 时默认选中最近一个目录（保持可改）：从托盘/空态新建时表单原本是
        // 空的，必须先点一次最近项或浏览，而九成场景就是「上次那个目录」。
        // 函数式读当前值：只在用户还没填过时预填，绝不覆盖手动输入。
        if (list.length > 0) setCwd((current) => current || list[0]);
      })
      .catch(() => {});
    reloadAgents();
  }, []);
  // 装完一个 agent，这里的可选项就该多一个——不必关掉面板重开。
  useAgentListRefresh(reloadAgents);

  // default_agent 若未装，则退到首个已装 agent（avail 加载后校正）。
  // 7T-9：换过这一手此前完全静默——用户从卡片菜单带着 codex 的预填点进来，面板上
  // 选中的却是 claude，没有一个字解释。记下「本来要用谁」，在 Agent 区下说一句。
  const [swappedFrom, setSwappedFrom] = useState<string | null>(null);
  useEffect(() => {
    if (avail && avail.length > 0 && !avail.includes(provider)) {
      setSwappedFrom(provider);
      setProvider(avail[0]);
    }
  }, [avail, provider]);
  // 附加目录只对声明支持的 agent 开放(claude --add-dir);切到不支持的 agent 时清空,
  // 静默带过去会让后端如实拒绝、启动直接报错。
  const supportsExtraDirs = agents?.find((a) => a.id === provider)?.supports_extra_dirs ?? false;
  useEffect(() => {
    if (!supportsExtraDirs) setExtraDirs([]);
  }, [supportsExtraDirs]);
  const toggleExtraDir = (dir: string) => {
    const key = pathKey(dir);
    if (key === pathKey(normalizePath(cwd.trim()))) return; // 主目录不重复附加
    setExtraDirs((prev) =>
      prev.some((x) => pathKey(x) === key) ? prev.filter((x) => pathKey(x) !== key) : [...prev, dir]);
  };
  async function pickExtraDir() {
    const picked = await open({ directory: true });
    if (typeof picked === "string") toggleExtraDir(normalizePath(picked));
  }
  // 换 agent 重置选择（不同 agent 的选项 id 可能撞名但语义不同，残留会串），
  // 但优先回填本 provider 上次的选择——见 LAUNCH_OPTS_KEY。
  useEffect(() => setOpts(loadStoredOpts()[provider] ?? {}), [provider]);
  const launchOptions = agents?.find((a) => a.id === provider)?.launch_options ?? [];

  function closeWin() {
    // 桌面是独立窗口,关窗即销毁;远程是页内浮层,由 onClose 回列表(无 onClose 才退回关窗)。
    if (onClose) {
      onClose();
      return;
    }
    getCurrentWindow().close();
  }
  // Esc 关窗（输入框内让位）：填错想放弃时不必去点右上角 ✕。
  useEscClose(closeWin);

  async function pickDir() {
    const picked = await open({ directory: true });
    if (typeof picked === "string") setCwd(normalizePath(picked));
  }

  // 远程端页内目录浏览:系统目录对话框在浏览器里弹不起来,改为逐级下钻的就地列表。
  const [browse, setBrowse] = useState<DirListing | null>(null);
  // 下钻失败(权限拒绝/断链网络盘)要说出来:静默吞掉的话,点了没反应像卡死(自审 L12)。
  const [browseError, setBrowseError] = useState<string | null>(null);
  function browseTo(path?: string) {
    listSubdirectories(path)
      .then((listing) => {
        setBrowse(listing);
        setBrowseError(null);
      })
      .catch((error) => setBrowseError(formatBackendError(error, t.locale)));
  }
  async function openRemoteBrowser() {
    // 起点取当前输入的目录;不存在/为空则回退磁盘列表,列不了就保持手输。
    try {
      setBrowse(await listSubdirectories(unquotePath(cwd) || undefined));
    } catch {
      try {
        setBrowse(await listSubdirectories());
      } catch (e) {
        // 两级都失败时 browse 仍为 null、browseError 的展示位(浏览面板内)根本不会渲染,
        // 必须借主错误行说出来——否则点了「浏览」毫无反应,与断链/卡死无从区分。
        setError(formatBackendError(e, t.locale));
      }
    }
  }

  // 7T-6：Enter 与「启动」钮此前各判各的——按钮 disabled 含「一个 agent 都没装/还在
  // 检测」，Enter 路径只看目录和 busy，于是在没装 CLI 的机器上回车照样发 newSession，
  // 拿一条后端错误回来。守卫抽出来，两条路径共用。
  const canLaunch = Boolean(cwd.trim()) && !busy && (avail?.length ?? 0) > 0;

  async function launch() {
    if (!canLaunch || launchPendingRef.current) return;
    launchPendingRef.current = true;
    setBusy(true);
    setError(null);
    try {
      // 输入框里躺的是用户原样粘的串:资源管理器「复制为路径」带引号,后端按字面
      // 找不到(7T-4)。只剥引号,不动斜杠方向——发给后端的 cwd 保持用户写法。
      const dir = unquotePath(cwd);
      // 附加目录剔除与主目录重复的(归一比较):agent 对同一目录拿两份授权无意义。
      const extras = extraDirs.filter((d) => pathKey(d) !== pathKey(dir));
      const tempId = await newSession(dir, provider, opts, extras);
      saveStoredOpts(provider, opts); // 启动成功才记：失败的组合不该成为下次的默认
      // 「正在启动」的可见反馈由看板的占位卡承担（后端把 pending PTY 合成负 id 占位项
      // 合入看板查询，认领出真卡后对账撤下），面板这里启动成功直接自关即可。
      // 远程没有 reveal 这条导航：自关前把临时 id 交回 RemoteApp 选中新会话，
      // 否则用户被丢回「去侧栏选会话」空态。桌面 onLaunched 为空，行为零改变。
      onLaunched?.(tempId);
      closeWin();
    } catch (e) {
      launchPendingRef.current = false;
      setError(formatBackendError(e, t.locale));
      setBusy(false);
    }
  }

  async function repairHooks() {
    if (repairing) return;
    setRepairing(true);
    setError(null);
    try {
      const res = await repairProviderHooks(provider);
      setHooks((h) => ({ ...h, [provider]: res.status }));
      // 修复后仍非 installed → 接线没真正生效，别让用户以为「点了没反应」；
      // 按后端 reason 给出精准提示（如 kimi 未登录 → 「请先登录」）。
      if (res.status !== "installed") setError(repairFailMessage(t, res.reason));
    } catch (e) {
      setError(formatBackendError(e, t.locale));
    } finally {
      setRepairing(false);
    }
  }

  /** 拉起交互式登录。成功 spawn 后不清等待态——等 login-done 事件（或 5 分钟超时 / 用户取消）才落回。 */
  async function doLogin() {
    const target = provider; // 锁定发起时的 provider：之后用户切走了，事件仍要能对上号
    if (loginOperations.isPending(target)) return;
    setError(null);
    try {
      await loginOperations.start(target);
    } catch (e) {
      setError(formatBackendError(e, t.locale));
    }
  }

  /**
   * 取消等待。终端可能已被关掉（手动关、崩溃、agent 自己退出），而后端只轮询账号文件，
   * 要 5 分钟才超时——这期间按钮一直不可点，用户既不能重来也不知道发生了什么。
   *
   * 不检测「终端还活着吗」：`wt.exe` 拉起窗口后自身立即退出，真正跑登录的是它的孙进程；
   * 而 `powershell -NoExit` 又会一直活着。靠监视进程只会在某些终端上失灵。
   *
   * 收尾由后端 emit 带 operationId 的 `login-done`，故不在此抢先清等待态。
   */
  async function cancelLoginWait() {
    const target = provider;
    if (!loginOperations.isPending(target)) return;
    setError(null);
    try {
      await loginOperations.cancel(target);
    } catch { /* hook 已解锁该 provider，允许用户重试 */ }
  }

  // 输入框内容实时过滤最近项：空 / 已选中某项（完全匹配）时显示全部，输入片段时按 名+路径 过滤。
  // 比较前统一 normalize 斜杠方向，避免 C:/proj 与 C:\proj 因分隔符不同而无法高亮/匹配。
  const cwdNorm = normalizePath(cwd.trim());
  const q = cwdNorm.toLowerCase();
  const shownRecent =
    !q || recent.some((r) => r.toLowerCase() === q)
      ? recent
      : recent.filter((r) => r.toLowerCase().includes(q));
  const warn = hooks[provider] === "missing" || hooks[provider] === "unknown";
  // 已装但未登录才提示（loggedIn 为 null = 拿不到账号，不打扰）。
  const needLogin = loggedIn?.[provider] === false;

  return (
    <div className="ns-window">
      <div className="ns-titlebar" data-tauri-drag-region>
        <span className="ns-title">{t.newSession.title}</span>
        <button type="button" className="ns-close" aria-label={t.newSession.cancel} onClick={closeWin}>
          ×
        </button>
      </div>

      <div className="ns-body">
        <label className="ns-field">
          <span className="ns-label ns-label-row">
            {t.newSession.dir}
            {/* 附加目录:跨仓同一需求开**一个**会话(agent 在同一上下文里协调所有仓),
                每个附加目录以 --add-dir 进 argv。最近列表 Ctrl+点击可快捷附加。 */}
            {supportsExtraDirs && (
              <button
                type="button"
                className="ns-extra-add"
                data-testid="ns-extra-add"
                data-tip={t.newSession.extraDirsTip}
                onClick={(e) => { e.preventDefault(); void pickExtraDir(); }}
              >
                + {t.newSession.extraDirsAdd}
              </button>
            )}
          </span>
          <div className="ns-picker">
            <div className="ns-dir-row">
              <input
                ref={dirInputRef}
                className="ns-input"
                data-testid="ns-dir"
                value={cwd}
                placeholder={t.newSession.dirPlaceholder}
                onChange={(e) => { setCwd(e.target.value); setError(null); }}
                // Enter 直接启动（launch 内部对空目录/busy 有守卫），与账号页 API Key 输入框同规。
                // IME 合成守卫：拼音按 Enter 提交候选时先触发 keydown，放行会用半截路径误启动。
                onKeyDown={(e) => { if (e.key === "Enter" && !e.nativeEvent.isComposing) void launch(); }}
              />
              {/* 桌面走系统目录对话框(plugin-dialog);远程在浏览器里弹不起来,换页内浏览器。 */}
              {!remoteUi() ? (
                <button type="button" className="ns-browse" onClick={pickDir}>
                  {t.newSession.browse}
                </button>
              ) : (
                <button
                  type="button"
                  className="ns-browse"
                  onClick={() => (browse ? setBrowse(null) : void openRemoteBrowser())}
                >
                  {t.newSession.browse}
                </button>
              )}
            </div>
            {extraDirs.length > 0 && (
              <div className="ns-extra-list" data-testid="ns-extra-list">
                {extraDirs.map((d) => (
                  <span key={d} className="ns-extra-chip" data-tip={d}>
                    {d.split(/[\\/]/).filter(Boolean).pop() ?? d}
                    <button
                      type="button"
                      className="ns-extra-x"
                      aria-label={t.newSession.extraDirRemove}
                      onClick={(e) => { e.preventDefault(); setExtraDirs((prev) => prev.filter((x) => x !== d)); }}
                    >×</button>
                  </span>
                ))}
              </div>
            )}
            {browse && (
              <div className="ns-dirbrowse" data-testid="ns-dirbrowse">
                <div className="ns-dirbrowse-head">
                  <button
                    type="button"
                    className="ns-browse"
                    disabled={browse.parent === null}
                    onClick={() => browseTo(browse.parent || undefined)}
                  >
                    {t.newSession.up}
                  </button>
                  {/* 只显当前目录名:整条路径在窄屏必被截断,反而遮住最有信息量的末段。 */}
                  <span className="ns-dirbrowse-path" data-tip={browse.path}>
                    {browse.path
                      ? (browse.path.split(/[\\/]/).filter(Boolean).pop() ?? browse.path)
                      : t.newSession.drives}
                  </span>
                  {browse.path && (
                    <button
                      type="button"
                      className="ns-browse ns-dirbrowse-pick"
                      onClick={() => {
                        setCwd(normalizePath(browse.path));
                        setBrowse(null);
                      }}
                    >
                      {t.newSession.pickHere}
                    </button>
                  )}
                </div>
                {browseError && <div className="ns-dirbrowse-empty ns-dirbrowse-err">{browseError}</div>}
                <div className="ns-dirbrowse-list">
                  {browse.dirs.map((d) => (
                    <button
                      key={d.path}
                      type="button"
                      className="ns-recent-item"
                      onClick={() => browseTo(d.path)}
                    >
                      <FolderIcon />
                      <span className="ns-recent-name">{d.name}</span>
                    </button>
                  ))}
                  {browse.dirs.length === 0 && (
                    <div className="ns-dirbrowse-empty">{t.newSession.noSubdirs}</div>
                  )}
                </div>
              </div>
            )}
            {/* 浏览器展开时顶掉「最近」列表:两块可滚列表叠放又挤又乱。 */}
            {!browse && recent.length > 0 && shownRecent.length > 0 && (              <div className="ns-recent-list">
                {shownRecent.map((r) => (
                  <button
                    key={r}
                    type="button"
                    className={
                      "ns-recent-item"
                      + (cwdNorm === r ? " is-on" : "")
                      + (extraDirs.some((x) => pathKey(x) === pathKey(r)) ? " is-extra" : "")
                    }
                    data-tip={supportsExtraDirs ? `${r}\n${isMac() ? t.newSession.extraDirsHintMac : t.newSession.extraDirsHint}` : r}
                    onClick={(e) => {
                      // Ctrl/Cmd+点击 = 加为/移出附加目录(与侧栏多选同一手势);普通点击 = 设主目录。
                      if (supportsExtraDirs && (e.ctrlKey || e.metaKey)) {
                        toggleExtraDir(r);
                        return;
                      }
                      setCwd(r);
                    }}
                  >
                    <FolderIcon />
                    <span className="ns-recent-name">{r.split(/[\\/]/).filter(Boolean).pop() ?? r}</span>
                    <span className="ns-recent-path">{r}</span>
                  </button>
                ))}
              </div>
            )}
          </div>
        </label>

        <div className="ns-field">
          <span className="ns-label">{t.newSession.agent}</span>
          {avail === null ? (
            // listAgents() 尚未 resolve：给「检测中」占位，而不是一块猜不出含义的空白。
            <div className="ns-agents" data-testid="ns-agents-detecting">
              {t.newSession.detectingAgents}
            </div>
          ) : avail.length === 0 ? (
            // 7T-8：此前是死胡同——一句「没检测到」，没有按钮也没有去处。安装入口在
            // 设置的「账号与用量」，直接把人送过去（引导里早就这么说了，这里漏了）。
            <div className="ns-warn" data-testid="ns-no-agents">
              <span>{t.newSession.noAgents}</span>
              <button type="button" className="ns-btn" onClick={() => void invoke("open_settings").catch(() => {})}>
                {t.newSession.goInstall}
              </button>
            </div>
          ) : (
            <div className="ns-agents">
              {(avail ?? []).map((p) => {
                const { Icon } = agentAssets(p);
                return (
                  <button
                    key={p}
                    type="button"
                    data-testid={"ns-agent-" + p}
                    className={"ns-agent" + (provider === p ? " is-on" : "")}
                    onClick={() => { setProvider(p); setError(null); }}
                  >
                    {/* currentColor 绘制的徽标（claude）要由容器补品牌色，只染图标不染文字。 */}
                    <span className="ns-agent-mark" style={tintStyle(p)}>
                      <Icon />
                    </span>
                    <span>{agentName(agents ?? [], p)}</span>
                  </button>
                );
              })}
            </div>
          )}
          {/* 7T-9：预填的 agent 没装、被悄悄换成别家时说一句。用户点开面板前就选好了
              要用谁，换掉不告诉他，等于让他用错工具跑一整轮。 */}
          {swappedFrom && avail && avail.length > 0 && (
            <div className="ns-hint" data-testid="ns-agent-swapped">
              {t.newSession.agentSwapped(agentName(agents ?? [], swappedFrom), agentName(agents ?? [], provider))}
            </div>
          )}
          {launchOptions.length > 0 && (
            <div className="ns-options" data-testid="ns-options">
              {/* 启动选项由插件声明（选择 → CLI flag），未声明的 agent 没有这块。
                  choice 文案：i18n 按 `<option>.<choice>` 取，缺省回退后端的产品词 label。
                  用自绘 Dropdown 而非原生 select：WebView2 的原生下拉跟随系统主题画白底，
                  无视页面 color-scheme（终端下拉当年正因此换掉，见 styles.css 的遗留注释）。 */}
              {launchOptions.map((option) => {
                // relay 启用时 model 档由中转固定（置灰 + 注明，理由见 relayPins）。
                const pinned = option.id === "model" ? relayPins[provider] : undefined;
                return (
                <div key={option.id} className="ns-option" data-testid={"ns-option-" + option.id}>
                  <span className="ns-option-label">{t.newSession.launchOption[option.id] ?? option.id}</span>
                  <Dropdown
                    align="left"
                    disabled={pinned !== undefined}
                    value={opts[option.id] ?? option.default}
                    options={option.choices.map((choice) => ({
                      value: choice.id,
                      label: t.newSession.launchChoice[`${option.id}.${choice.id}`] ?? choice.label,
                      // 高风险档（契约 risk=true）警示色 + 副标题，与普通档拉开视觉。
                      risky: choice.risk,
                      sub: choice.risk ? t.newSession.riskyChoiceSub : undefined,
                    }))}
                    onChange={(v) => setOpts((m) => ({ ...m, [option.id]: v }))}
                  />
                  {pinned !== undefined && (
                    <span className="ns-option-note" data-testid="ns-option-pinned">{t.newSession.relayPinnedModel(pinned)}</span>
                  )}
                </div>
                );
              })}
            </div>
          )}
          {/* 修复 hook / 登录的按钮都要在桌面拉起终端进程,远程无从操作——按钮隐藏。但警告
              文字必须留:远程若整块藏掉,用户对着「启动」点下去会起一个 hook 没接、永远
              不出现在看板的哑会话且毫无线索(审查 #10)。远程把动作换成「请在桌面端处理」。 */}
          {avail && avail.length > 0 && warn && (
            <div className="ns-warn" data-testid="ns-hooks-warn">
              <span>{hooks[provider] === "unknown" ? t.newSession.hooksUnknown : t.newSession.hooksMissing}</span>
              {remoteUi() ? (
                <span className="ns-warn-remote">{t.newSession.fixOnDesktop}</span>
              ) : (
                <button
                  type="button"
                  className="ns-repair"
                  data-testid="ns-repair-hooks"
                  onClick={repairHooks}
                  disabled={repairing}
                >
                  {repairing ? t.newSession.repairingHooks : t.newSession.repairHooks}
                </button>
              )}
            </div>
          )}
          {avail && avail.length > 0 && needLogin && (
            <div className="ns-warn" data-testid="ns-login-warn">
              {/* 等待中：这行承载「正在等」，按钮则变成「取消等待」。 */}
              <span>{loginOperations.isPending(provider) ? t.newSession.loggingIn : t.newSession.notLoggedIn}</span>
              {remoteUi() ? (
                <span className="ns-warn-remote">{t.newSession.fixOnDesktop}</span>
              ) : (
                <button
                  type="button"
                  className="ns-repair"
                  data-testid="ns-login"
                  // 等待中不再是死按钮：终端可能已被关掉，而后端要 5 分钟才超时。点它即取消等待。
                  onClick={loginOperations.isPending(provider) ? cancelLoginWait : doLogin}
                >
                  {loginOperations.isPending(provider) ? t.newSession.cancelLogin : t.newSession.login}
                </button>
              )}
            </div>
          )}
        </div>

        {error && (
          <div className="ns-error" data-testid="ns-error" role="alert">
            {error}
          </div>
        )}
      </div>

      <div className="ns-actions">
        {/* 非默认账号活跃时明说：新会话将写进那个隔离账号。默认账号不显示——零感知底线。 */}
        {activeProfiles[provider] && (
          <span className="ns-profile-hint" data-testid="ns-active-profile">
            {t.newSession.activeProfile(activeProfiles[provider]!)}
          </span>
        )}
        <button type="button" className="ns-btn" onClick={closeWin}>
          {t.newSession.cancel}
        </button>
        <button
          type="button"
          className="ns-btn is-primary"
          data-testid="ns-launch"
          disabled={!canLaunch}
          onClick={launch}
        >
          {busy ? t.newSession.launching : t.newSession.launch}
        </button>
      </div>
    </div>
  );
}

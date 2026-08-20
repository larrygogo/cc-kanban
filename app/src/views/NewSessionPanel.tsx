import { type ReactElement, useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
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
import { normalizePath, pathKey } from "../paths";
import { Dropdown } from "./menu";
import { useAgentListRefresh } from "../useAgents";
import { useTauriEvent } from "../hooks/useTauriEvent";
import { useLoginOperations } from "../hooks/useLoginOperations";
import { useT, repairFailMessage } from "../i18n";
import { formatBackendError } from "../i18n/errors";
import { useEscClose } from "../hooks/useEscClose";
import { remoteUi } from "../remoteMode";

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

/** 独立窗口页（label="new-session"）：新建一个全新会话。成功后 emit 通知主看板弹 toast 并自关。 */

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

export function NewSessionPanel({ onClose, prefill }: {
  onClose?: () => void;
  /** 远程页内打开时的预填(桌面走 URL query / ns-prefill 事件,远程两条都不通,改走 props)。 */
  prefill?: { cwd?: string | null; provider?: string | null };
} = {}): ReactElement {
  const startCwd = prefill?.cwd != null ? normalizePath(prefill.cwd) : initialCwd;
  const startProvider = (prefill?.provider ?? initialProvider) as AgentId | null;
  const t = useT();
  // 窗口以 visible:false 创建（window.rs），首帧渲染后再显示，消除打开瞬间的白框闪烁。
  useShowWhenReady();
  const [cwd, setCwd] = useState(startCwd);
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
  // null = 尚未拿到账号（或取不到）→ 不显示未登录提示，避免误闪/误报。
  const [loggedIn, setLoggedIn] = useState<Record<string, boolean> | null>(null);
  // 各 provider 当前活跃账号的展示名（null = 默认账号）。非默认账号活跃时在启动按钮旁提示，
  // 防「切过一次账号就忘了，新会话全写进隔离账号」。
  const [activeProfiles, setActiveProfiles] = useState<Record<string, string | null>>({});
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
        getAccounts()
          .then((rows) => {
            const m: Record<string, boolean> = {};
            const ap: Record<string, string | null> = {};
            for (const { id } of list) {
              const row = rows.find((r) => r.provider === id);
              if (row) m[id] = isLoggedIn(row);
              ap[id] = row?.active_profile_name ?? null;
            }
            setLoggedIn(m);
            setActiveProfiles(ap);
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

  // default_agent 若未装，则退到首个已装 agent（avail 加载后校正）
  useEffect(() => {
    if (avail && avail.length > 0 && !avail.includes(provider)) setProvider(avail[0]);
  }, [avail, provider]);
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
      setBrowse(await listSubdirectories(cwd.trim() || undefined));
    } catch {
      try {
        setBrowse(await listSubdirectories());
      } catch {
        /* 保持手输 */
      }
    }
  }

  async function launch() {
    if (!cwd.trim() || busy || launchPendingRef.current) return;
    launchPendingRef.current = true;
    setBusy(true);
    setError(null);
    try {
      await newSession(cwd.trim(), provider, opts);
      saveStoredOpts(provider, opts); // 启动成功才记：失败的组合不该成为下次的默认
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
          <span className="ns-label">{t.newSession.dir}</span>
          <div className="ns-picker">
            <div className="ns-dir-row">
              <input
                className="ns-input"
                data-testid="ns-dir"
                value={cwd}
                placeholder={t.newSession.dirPlaceholder}
                onChange={(e) => setCwd(e.target.value)}
                // Enter 直接启动（launch 内部对空目录/busy 有守卫），与账号页 API Key 输入框同规。
                onKeyDown={(e) => { if (e.key === "Enter") void launch(); }}
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
                  <span className="ns-dirbrowse-path" title={browse.path}>
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
            {!browse && recent.length > 0 && shownRecent.length > 0 && (
              <div className="ns-recent-list">
                {shownRecent.map((r) => (
                  <button
                    key={r}
                    type="button"
                    className={"ns-recent-item" + (cwdNorm === r ? " is-on" : "")}
                    title={r}
                    onClick={() => setCwd(r)}
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
            <div className="ns-warn" data-testid="ns-no-agents">
              {t.newSession.noAgents}
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
                    onClick={() => setProvider(p)}
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
          {launchOptions.length > 0 && (
            <div className="ns-options" data-testid="ns-options">
              {/* 启动选项由插件声明（选择 → CLI flag），未声明的 agent 没有这块。
                  choice 文案：i18n 按 `<option>.<choice>` 取，缺省回退后端的产品词 label。
                  用自绘 Dropdown 而非原生 select：WebView2 的原生下拉跟随系统主题画白底，
                  无视页面 color-scheme（终端下拉当年正因此换掉，见 styles.css 的遗留注释）。 */}
              {launchOptions.map((option) => (
                <div key={option.id} className="ns-option" data-testid={"ns-option-" + option.id}>
                  <span className="ns-option-label">{t.newSession.launchOption[option.id] ?? option.id}</span>
                  <Dropdown
                    align="left"
                    value={opts[option.id] ?? option.default}
                    options={option.choices.map((choice) => ({
                      value: choice.id,
                      label: t.newSession.launchChoice[`${option.id}.${choice.id}`] ?? choice.label,
                    }))}
                    onChange={(v) => setOpts((m) => ({ ...m, [option.id]: v }))}
                  />
                </div>
              ))}
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
          disabled={!cwd.trim() || busy || (avail?.length ?? 0) === 0}
          onClick={launch}
        >
          {busy ? t.newSession.launching : t.newSession.launch}
        </button>
      </div>
    </div>
  );
}

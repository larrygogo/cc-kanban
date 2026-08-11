import {
  type MouseEvent as ReactMouseEvent,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  LiveSessionCounts,
  Settings,

  TerminalOpenMode,
  confirmStopSession,
  getSettings,
  getAccounts,
  openNewSessionWindow,
  openProjectDir,
  refreshUsage,
  renameSession,
  setArchived,
  setSessionNote,
  type CardMenuMode,
  type FocusSessionResult,
  type ProviderUsage,
} from "../api";
import { isMacPanel } from "../platform";
import { useTauriEvent } from "../hooks/useTauriEvent";
import { agentAssets, tintStyle } from "../providers";
import { useAgents } from "../useAgents";
import { useT } from "../i18n";
import { type Item, type Tab, TAB_KEYS, PIN_KEY, UNNAMED_SESSION_SENTINEL } from "./sticker/types";
import { editorKeyDown, fmtAgo, useStarred, match } from "./sticker/helpers";
import {
  CheckIcon,
  ChatIcon,
  CloseIcon,
  GearIcon,
  MoreIcon,
  NoteIcon,
  OpenIcon,
  PinIcon,
  PlusIcon,
  SearchIcon,
  XIcon,
} from "./sticker/icons";
import { RunBadge } from "./sticker/RunBadge";
import { CardContextMenu } from "./sticker/CardContextMenu";
import { EmptyState } from "./sticker/EmptyState";
import { UsageScreen } from "./sticker/UsageScreen";

type FocusNoticeKind = FocusSessionResult | "connecting" | "failed" | "background";

/** 卡片就地编辑框（重命名/便签共用）。草稿住在本组件的局部状态：放在 Sticker 里时
 *  每次按键 setState 都让整个看板重渲染（虚拟化把代价压到可见行，但可见行的整卡重建
 *  仍是白付；侧栏同款问题见 ChatSidebar 的 EditorInput）。提交/取消语义与原实现一致：
 *  Enter/✓ 提交、Esc/✗ 取消；无失焦提交（有显式按钮，失焦提交会和点 ✗ 打架）。 */
function EditBox({ initial, placeholder, inputClass, extraClass, icon, saveLabel, cancelLabel, onSubmit, onCancel }: {
  initial: string;
  placeholder: string;
  inputClass: string;
  extraClass?: string;
  icon?: JSX.Element;
  saveLabel: string;
  cancelLabel: string;
  onSubmit: (value: string) => void;
  onCancel: () => void;
}) {
  const [value, setValue] = useState(initial);
  return (
    <div className={"stk-editbox" + (extraClass ? ` ${extraClass}` : "")} onClick={(e) => e.stopPropagation()}>
      {icon}
      <input
        className={inputClass}
        autoFocus
        value={value}
        placeholder={placeholder}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={editorKeyDown(() => onSubmit(value), onCancel)}
      />
      {/* mousedown preventDefault：点按钮不抢走输入框焦点，避免触发其它失焦逻辑 */}
      <button
        type="button"
        className="stk-ebtn stk-ebtn-ok"
        aria-label={saveLabel}
        data-tip={saveLabel}
        onMouseDown={(e) => e.preventDefault()}
        onClick={() => onSubmit(value)}
      ><CheckIcon /></button>
      <button
        type="button"
        className="stk-ebtn"
        aria-label={cancelLabel}
        data-tip={cancelLabel}
        onMouseDown={(e) => e.preventDefault()}
        onClick={onCancel}
      ><XIcon /></button>
    </div>
  );
}

export function relayEnabledSignature(settings: Settings): string {
  return Object.entries(settings.relay?.per_agent ?? {})
    .filter(([, rule]) => rule?.enabled)
    .map(([provider]) => provider)
    .sort()
    .join(",");
}

export function Sticker({
  filter,
  onFilterChange,
  data,
  counts: countsProp,
  total,
  hasMore: hasMoreProp,
  loadMore,
  loadingMore,
  hasUpdate,
  search,
  onSearchChange,
  onArchiveOptimistic,
  onArchiveFailed,
  initialLoading,
  loadError,
  onRetry,
}: {
  filter: Tab;
  onFilterChange?: (f: Tab) => void;
  data: Item[];
  counts?: LiveSessionCounts;
  total?: number;
  hasMore?: boolean;
  loadMore?: () => void;
  loadingMore?: boolean;
  hasUpdate?: boolean;
  search?: string;
  onSearchChange?: (q: string) => void;
  /** 归档请求已发出（未确认）：父层可据此乐观摘掉卡片。 */
  onArchiveOptimistic?: (sessionId: number) => void;
  /** 归档请求失败：父层需回滚上面的乐观更新。 */
  onArchiveFailed?: () => void;
  /** 冷启动首次加载中：true 时显示加载占位而非假空态。 */
  initialLoading?: boolean;
  /** 首页加载失败：显示「加载失败 + 重试」而非「还没有会话」。 */
  loadError?: boolean;
  /** 重试回调：重新发起首页加载。 */
  onRetry?: () => void;
}) {
  // hasMore 由父组件传入；未传入时退化为 data.length < total。
  // agent 展示名来自后端下发的名单；未知 id 回退成 id 本身。
  // agent 名单一次取全：展示名 + 已装列表（配额过滤用），不再各拉一次。
  const { name: agentNameOf, installed: availAgents } = useAgents();
  const totalCount = total ?? data.length;
  const onLoadMore = loadMore ?? (() => {});
  const isLoadingMore = loadingMore ?? false;
  const hasMore = hasMoreProp ?? data.length < totalCount;
  const t = useT();
  const tab = filter;

  const pick = (t: Tab) => {
    onFilterChange?.(t);
    closeSearch(); // 切 tab 即退出搜索，避免 tab 高亮与过滤结果不一致
  };

  // 会话搜索：激活时底栏整条变成输入框；搜索词经 onSearchChange 交给父组件下沉到后端
  // （当前 tab 内全库搜，覆盖未加载数据），本组件不再持有搜索词、不做客户端过滤。
  const [searchOpen, setSearchOpen] = useState(false);
  const q = search ?? "";
  const closeSearch = () => {
    setSearchOpen(false);
    onSearchChange?.("");
  };

  // 打开终端方式 + 卡片菜单方式 + 贴纸配额 provider 列表：启动时读设置，并监听实时变更。
  const [openMode, setOpenMode] = useState<TerminalOpenMode>("card");
  // 首帧占位 context（右键，与既有单测对齐）；真实默认由后端 default_card_menu_mode=button
  // 经 getSettings 校正，缺字段时下方 apply 亦回退 button。
  const [menuMode, setMenuMode] = useState<CardMenuMode>("context");
  const [previewEnabled, setPreviewEnabled] = useState(true);
  // 空初值：settings resolve 前不渲染配额区，好过先闪一个猜出来的 agent。默认值由后端给。
  const [quotaProviders, setQuotaProviders] = useState<string[]>([]);
  // 中转启用状态改变时重建贴纸用量请求：启用后立即隐藏官方配额，关闭后立即恢复缓存并刷新。
  const [usageRevision, setUsageRevision] = useState(0);
  const relaySignatureRef = useRef<string | null>(null);

  useEffect(() => {
    const apply = (s: Settings) => {
      setOpenMode(s.terminal_open_mode);
      setMenuMode(s.card_menu_mode ?? "button");
      setPreviewEnabled(s.preview_enabled);
      setQuotaProviders(s.sticker_quota_providers ?? []);
      const signature = relayEnabledSignature(s);
      if (relaySignatureRef.current !== null && relaySignatureRef.current !== signature) {
        setUsageRevision((n) => n + 1);
      }
      relaySignatureRef.current = signature;
    };
    let receivedLiveSettings = false;
    getSettings().then((settings) => {
      // 监听事件可能比首次读取更早返回；不能让旧快照覆盖刚切换完的接入方式。
      if (!receivedLiveSettings) apply(settings);
    }).catch(() => {});
    // cleanup 可能先于 listen resolve 执行：用 cancelled 标记，resolve 后立即注销，防监听器泄漏。
    let cancelled = false;
    let un: (() => void) | undefined;
    try {
      listen<Settings>("settings-changed", (e) => {
        receivedLiveSettings = true;
        apply(e.payload);
      })
        .then((f) => {
          if (cancelled) f();
          else un = f;
        })
        .catch(() => {});
    } catch {
      /* 非 Tauri 环境（测试/浏览器） */
    }
    return () => {
      cancelled = true;
      try { un?.(); } catch { /* noop */ }
    };
  }, []);

  // 相对时间（fmtAgo）每分钟重算：递增计数触发轻量重渲染。
  const [, setTick] = useState(0);
  useEffect(() => {
    const id = window.setInterval(() => setTick((n) => n + 1), 60_000);
    return () => window.clearInterval(id);
  }, []);

  // 置顶开关：默认不置顶，激活后才把窗口设为 alwaysOnTop，状态持久化。
  const [pinned, setPinned] = useState<boolean>(() => localStorage.getItem(PIN_KEY) === "1");
  useEffect(() => {
    // 非 Tauri 环境（测试/浏览器）下 getCurrentWindow 会抛错，吞掉即可。
    try {
      getCurrentWindow().setAlwaysOnTop(pinned).catch(() => {});
    } catch {
      /* noop */
    }
  }, [pinned]);
  const togglePin = () => {
    setPinned((p) => {
      const next = !p;
      localStorage.setItem(PIN_KEY, next ? "1" : "0");
      return next;
    });
  };

  // 托盘「找回贴纸」：窗口居中/置顶由 App + 后端负责，这里把置顶按钮 UI 同步为已置顶。
  useTauriEvent("recall-sticker", () => setPinned(true));

  // 会话置顶：置顶的会话永远排到列表最前（跨重启保留）。与「置顶窗口(alwaysOnTop)」是两回事——
  // 那是窗口行为，图标也刻意分开（这里 TopIcon，窗口那边 PinIcon）。存储键沿用 meowo-starred。
  // useStarred：与对话窗侧栏/标题菜单同一份写入口，且跨窗口（storage 事件）同步——
  // 在对话窗里置顶，看板立刻跟上，反之亦然。
  const { starred, toggleStar } = useStarred();

  // 重命名：editingId 为正在编辑的会话 id。草稿文本住在 EditBox 的局部状态里，
  // 按键不触发看板整表重渲染。
  const [editingId, setEditingId] = useState<number | null>(null);
  const startRename = (l: Item) => {
    setNotingId(null); // 与便签编辑互斥，同卡只开一个编辑器
    setEditingId(l.session.id);
  };
  const submitRename = (l: Item, draft: string) => {
    // 局部变量原名 t 与 i18n 字典 t 冲突，改名 title 以便失败提示取字典文案。
    const title = draft.trim();
    if (title && title !== l.task_title) {
      // 失败不能静默：走 focusNotice 提示通道（detail 直接展示文案，4s 自动消失）。
      renameSession(l.cwd, l.session.cc_session_id, title, l.provider)
        .catch(() => setFocusNotice({ kind: "failed", item: l, detail: t.sticker.renameFailed }));
    }
    setEditingId(null);
  };

  // 卡片右键菜单：记录目标会话 id 与打开坐标。item 每次渲染按 id 从 data 现查——
  // 数据刷新后菜单内容(置顶/便签/归档态)保持最新,会话消失则菜单自然不渲染。
  const [ctxMenu, setCtxMenu] = useState<{ sid: number; x: number; y: number } | null>(null);
  const ctxItem = ctxMenu ? data.find((l) => l.session.id === ctxMenu.sid) ?? null : null;

  // 便签编辑：notingId 为正在编辑便签的会话 id。与重命名互斥（同卡只开一个）。
  const [notingId, setNotingId] = useState<number | null>(null);
  const [focusNotice, setFocusNotice] = useState<{
    kind: FocusNoticeKind;
    item: Item;
    confirming?: boolean;
    busy?: boolean;
    detail?: string;
  } | null>(null);
  useEffect(() => {
    if (!focusNotice || focusNotice.confirming || focusNotice.busy) return;
    if (["host_focused", "unsupported_terminal", "alive_but_not_found", "process_ended"].includes(focusNotice.kind)) return;
    const id = window.setTimeout(() => setFocusNotice(null), 4_000);
    return () => window.clearTimeout(id);
  }, [focusNotice]);
  const startNote = (l: Item) => {
    setEditingId(null);
    setNotingId(l.session.id);
  };
  const submitNote = (l: Item, noteDraft: string) => {
    if (noteDraft !== (l.note ?? "")) {
      // 失败不能静默：与重命名共用 focusNotice 提示通道。
      setSessionNote(l.session.cc_session_id, noteDraft)
        .catch(() => setFocusNotice({ kind: "failed", item: l, detail: t.sticker.noteFailed }));
    }
    setNotingId(null);
  };

  // 打开终端：连接中→跳转 WT 标签页；已断开未归档→新建终端 resume；归档不开。
  const buttonMode = openMode === "button";
  const canOpen = (l: Item) => l.connected || !l.archived;
  const openTerminal = (l: Item) => {
    // Agent 自己派生的后台会话：没有终端窗口可切（daemon 起的 PTY 宿主不是用户的终端），
    // 也无从恢复——恢复等于跟 supervisor 抢同一个 session id。只说明它是什么、去哪找它。
    // 注意：后端当前把这类会话整条藏掉（session_query 的 hidden_background），所以
    // `l.background` 线上恒为 false，本分支是放宽隐藏规则后的预备（测试用合成数据覆盖）。
    if (l.background) {
      setFocusNotice({ kind: "background", item: l });
      return;
    }
    if (l.connected) {
      if (!l.pid) {
        setFocusNotice({ kind: "connecting", item: l });
        return;
      }
      invoke<FocusSessionResult>("focus_session", {
          pid: l.pid,
          title: l.task_title,
          cwd: l.cwd,
          sessionId: l.session.cc_session_id,
          provider: l.provider,
        })
        .then((result) => {
          // demo/旧后端可能不返回值；保持原行为，不误弹失败提示。
          if (!result || result === "focused") setFocusNotice(null);
          else setFocusNotice({ kind: result, item: l });
        })
        .catch((err) => setFocusNotice({ kind: "failed", item: l, detail: String(err) }));
    } else if (!l.archived) {
      // 恢复现在可能因多种原因失败（恢复计划无效、PTY 起不来、attach 打不开外部终端）。
      // 静默吞掉的话，用户点了卡片只会看到「什么都没发生」，尤其在把打开方式设成外部终端后，
      // 会直接被理解成设置不生效。走与 focus 相同的提示通道。
      invoke("resume_session", { cwd: l.cwd, sessionId: l.session.cc_session_id, provider: l.provider })
        .catch((err) => setFocusNotice({ kind: "failed", item: l, detail: String(err) }));
    }
  };

  const reopenNoticeSession = () => {
    if (!focusNotice || focusNotice.busy) return;
    const l = focusNotice.item;
    if (focusNotice.kind === "process_ended") {
      setFocusNotice({ ...focusNotice, busy: true });
      invoke("resume_session", { cwd: l.cwd, sessionId: l.session.cc_session_id, provider: l.provider })
        .then(() => setFocusNotice(null))
        .catch((err) => setFocusNotice({ ...focusNotice, busy: false, kind: "failed", detail: String(err) }));
      return;
    }
    if (!focusNotice.confirming) {
      setFocusNotice({ ...focusNotice, confirming: true });
      return;
    }
    const pid = l.pid;
    if (!pid) {
      // notice 持有点击时的快照，正常不会走到这里；仍在命令边界防御可空类型，绝不把 null 交给后端。
      setFocusNotice({ ...focusNotice, confirming: false, kind: "process_ended" });
      return;
    }
    setFocusNotice({ ...focusNotice, busy: true });
    invoke("restart_session_supported", {
      pid,
      cwd: l.cwd,
      sessionId: l.session.cc_session_id,
      provider: l.provider,
    })
      .then(() => setFocusNotice(null))
      .catch((err) => setFocusNotice({ ...focusNotice, busy: false, confirming: false, kind: "failed", detail: String(err) }));
  };

  const focusNoticeText = focusNotice
    ? focusNotice.detail || (focusNotice.confirming
      ? t.sticker.reopenConfirm
      : {
          connecting: t.sticker.focusConnecting,
          host_focused: t.sticker.focusHostOnly,
          alive_but_not_found: t.sticker.focusNotFound,
          permission_denied: t.sticker.focusPermission,
          unsupported_terminal: t.sticker.focusUnsupported,
          process_ended: t.sticker.focusEnded,
          failed: t.sticker.focusFailed,
          background: t.sticker.focusBackground,
          focused: "",
        }[focusNotice.kind])
    : "";

  // 先按当前 tab 过滤，再排序：置顶恒在最前。match(tab) 是安全网（后端已按 tab/search 过滤，
  // 这里兜底防御性重过滤一遍）；搜索过滤已下沉后端（父组件按 search 请求当前 tab 内全库），
  // 本组件不再做客户端搜索过滤。waiting「等最久优先」由后端 ASC 排序保证，客户端只做 starred 浮顶。
  // useMemo 缓存：编辑便签/重命名时每次按键都会重渲染，不必每次重跑 filter+sort。
  const isStarred = (l: Item) => starred.has(l.session.cc_session_id);
  const shown = useMemo(() => {
    return data
      .filter((l) => match(tab, l))
      .sort(
        (a, b) =>
          Number(starred.has(b.session.cc_session_id)) -
          Number(starred.has(a.session.cc_session_id))
      );
  }, [data, tab, starred]);

  // 贴纸会话虚拟列表：只挂载可视区 + overscan 内的卡片，避免大量 DOM。
  // estimateSize 取常见卡片高度（无便签/preview 时约 76–84px），实际高度由 measureElement 动态测量。
  const virtualizer = useVirtualizer({
    count: shown.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => 82,
    overscan: 6,
    // 贴纸窗口初始约 460×420，减去 tabs/底栏后可视区约 400×320；给 initialRect 避免首帧
    // 没拿到 ResizeObserver 前渲染 0 条。后续真实尺寸进来会立即修正。
    initialRect: { width: 400, height: 320 },
    measureElement:
      typeof window !== "undefined" && "ResizeObserver" in window
        ? (el) => el.getBoundingClientRect().height
        : undefined,
  });
  const virtualItems = virtualizer.getVirtualItems();
  const totalSize = virtualizer.getTotalSize();

  // 无限滚动触发：当可视区底部进入「已加载列表末尾前 10 条」且仍有数据时，通知父组件加载下一页。
  useEffect(() => {
    if (isLoadingMore || !hasMore || shown.length === 0) return;
    const last = virtualItems[virtualItems.length - 1];
    if (last && last.index >= shown.length - 10) {
      onLoadMore();
    }
  }, [virtualItems, shown.length, isLoadingMore, hasMore, onLoadMore]);

  // 各标签角标计数：优先用后端返回的总数（稳定、不随懒加载闪烁）；
  // 未传入时（测试/旧调用）退化为按已加载 data 估算。
  const counts = useMemo<Record<Tab, number>>(() => {
    if (countsProp) {
      return {
        all: countsProp.total - countsProp.archived,
        running: countsProp.running,
        waiting: countsProp.waiting,
      };
    }
    const c = {} as Record<Tab, number>;
    for (const k of TAB_KEYS) c[k] = data.filter((l) => match(k, l)).length;
    return c;
  }, [countsProp, data]);

  // 自绘 overlay 滚动条：原生滚动条全程隐藏(不占布局→无抖动)，这里按滚动位置算出
  // thumb 的高度/位置，浮在内容右侧。null = 内容未超出、不需要滚动条。
  const scrollRef = useRef<HTMLDivElement>(null);
  const scrollInnerRef = useRef<HTMLDivElement>(null);
  const [sb, setSb] = useState<{ top: number; height: number } | null>(null);
  const [sbDrag, setSbDrag] = useState(false);
  // 滚动边缘淡出：仅当该方向确有被遮内容时才淡(滚到顶/底则对应边不淡，首/末卡保持清晰)。
  const [edge, setEdge] = useState({ top: false, bottom: false });

  // 底部用量：多 provider。先从 getAccounts() 拿缓存快速预填(仅在还没有联网值时填充，
  // 避免缓存晚到覆盖更新的联网值)，再对有账号且 usage_supported 的 provider 定时刷新（5 min）。
  // usageMap 只存原始 ProviderUsage，label 在渲染时取当前语言，切换即时生效。
  const [usageMap, setUsageMap] = useState<Record<string, ProviderUsage>>({});
  useEffect(() => {
    let cancelled = false;
    const timers: number[] = [];
    getAccounts()
      .then((ps) => {
        if (cancelled || !Array.isArray(ps)) return;
        // 中转启用的 provider 必须立即从贴纸移除；切回官方则用缓存快速恢复。
        setUsageMap((cur) => {
          const next: Record<string, ProviderUsage> = { ...cur };
          ps.forEach((p) => {
            if (p.relay_enabled) delete next[p.provider];
            else if (!next[p.provider] && p.usage) next[p.provider] = p.usage;
          });
          return next;
        });
        // 对有账号且支持用量的 provider：立即刷新 + 定时刷新
        ps.filter((p) => p.account != null && p.usage_supported && !p.relay_enabled).forEach(({ provider }) => {
          const doRefresh = () => {
            if (cancelled) return;
            refreshUsage(provider)
              .then((u) => { if (!cancelled) setUsageMap((m) => ({ ...m, [provider]: u })); })
              .catch(() => {}); // USAGE_UNSUPPORTED / 网络错误：保持无用量，不显示该行
          };
          doRefresh();
          timers.push(window.setInterval(doRefresh, 5 * 60_000));
        });
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      timers.forEach((id) => window.clearInterval(id));
    };
  }, [usageRevision]);
  // usageMap 与 quotaProviders 直接传入 UsageScreen，不再在父层预处理为行数组。
  // 交叉过滤：只显示既在配额设置里、又已装的 provider（availAgents 为空=未加载时不过滤，避免闪空）
  const shownQuota = quotaProviders.filter((p) => availAgents.length === 0 || availAgents.includes(p));

  const syncSb = () => {
    const el = scrollRef.current;
    if (!el) return;
    const { scrollTop, scrollHeight, clientHeight, offsetTop } = el;
    if (scrollHeight <= clientHeight + 1) {
      setSb(null);
      setEdge((p) => (p.top || p.bottom ? { top: false, bottom: false } : p));
      return;
    }
    const canUp = scrollTop > 1;
    const canDown = scrollTop + clientHeight < scrollHeight - 1;
    setEdge((p) => (p.top === canUp && p.bottom === canDown ? p : { top: canUp, bottom: canDown }));
    const thumbH = Math.max(28, (clientHeight * clientHeight) / scrollHeight);
    const top = offsetTop + (clientHeight - thumbH) * (scrollTop / (scrollHeight - clientHeight));
    // 相等守卫(与 setEdge 同款):逐卡片 observe 后 RO 触发频度到每秒级,thumb 几何没变时
    // 复用旧引用,避免每次数据刷新都强制整树重渲染。
    setSb((p) => (p && p.top === top && p.height === thumbH ? p : { top, height: thumbH }));
  };
  // 列表内容(shown)或可视尺寸变化时重算 thumb。
  useEffect(() => {
    syncSb();
    const el = scrollRef.current;
    const inner = scrollInnerRef.current;
    // 测试/非浏览器环境无 ResizeObserver：仅同步一次即可。
    if (!el || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(syncSb);
    ro.observe(el);
    // 虚拟列表下，滚动高度来自 inner 容器的总高；观察 inner 即可在 totalSize 或卡片高度变化时
    // 触发重算。同时观察可见卡片 wrapper，确保单张卡展开便签/重命名编辑器时也能及时更新 thumb。
    if (inner) ro.observe(inner);
    for (const child of Array.from(inner?.children ?? el.children)) ro.observe(child);
    return () => ro.disconnect();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [shown]);

  // 拖拽 thumb 滚动列表。
  const onSbDown = (e: ReactMouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    const el = scrollRef.current;
    if (!el) return;
    setSbDrag(true);
    const startY = e.clientY;
    const startScroll = el.scrollTop;
    const { scrollHeight, clientHeight } = el;
    const thumbH = Math.max(28, (clientHeight * clientHeight) / scrollHeight);
    const up = () => {
      setSbDrag(false);
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
      window.removeEventListener("blur", up);
    };
    const move = (ev: MouseEvent) => {
      // 拖着 thumb 移出窗口再松手时，webview 收不到这次 mouseup。靠 buttons=0 认出「键已不在」
      // 并做与 up 相同的清理——否则残留监听会把之后窗内的普通移动全当成拖拽（列表跟着鼠标乱滚）。
      if (ev.buttons === 0) { up(); return; }
      const ratio = (ev.clientY - startY) / (clientHeight - thumbH);
      el.scrollTop = startScroll + ratio * (scrollHeight - clientHeight);
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
    // 窗口失焦（拖到别的屏幕松手/切窗）同样收不到 mouseup，与 Tooltip/CardContextMenu 同款的 blur 兜底。
    window.addEventListener("blur", up);
  };

  return (
    <div className="sticker">
      {!isMacPanel() && <div className="drag" data-tauri-drag-region />}
      <div className="tabs">
        <div className="tabseg" role="tablist">
          {/* 选中态立体滑块：切换时平滑滑到目标 tab(translateX 动画) */}
          <span
            className="tabseg-slider"
            style={{ transform: `translateX(${TAB_KEYS.indexOf(tab) * 100}%)` }}
          />
          {TAB_KEYS.map((k) => {
            const n = counts[k];
            return (
              <button
                key={k}
                type="button"
                role="tab"
                aria-selected={tab === k}
                className={"stab " + (tab === k ? "stab-on" : "")}
                onClick={() => pick(k)}
              >
                {t.tabs[k]}
                {k !== "all" && <span className="stab-n">{n > 99 ? "99+" : n}</span>}
              </button>
            );
          })}
        </div>
      </div>
      <div
        className={"stk-scroll" + (edge.top ? " fade-top" : "") + (edge.bottom ? " fade-bottom" : "")}
        ref={scrollRef}
        onScroll={syncSb}
      >
        {shown.length === 0 ? (
          initialLoading ? (
            // 冷启动首次加载：先给加载占位，不闪「还没有会话」假空态。
            <div className="stk-loading">{t.sticker.loading}</div>
          ) : loadError ? (
            // 首页加载失败：如实告知并可重试，不伪装成空看板。
            <div className="stk-empty">
              <div className="stk-empty-title">{t.sticker.loadFailed}</div>
              <button type="button" className="stk-empty-cta" data-testid="empty-retry-cta" onClick={() => onRetry?.()}>
                {t.sticker.retry}
              </button>
            </div>
          ) : isLoadingMore ? (
            <div className="stk-loading">{t.sticker.loading}</div>
          ) : searchOpen && q.trim() ? (
            // 搜索有词但 0 结果：独立空态，不带「新建会话」CTA，避免误导。
            <EmptyState tab={tab} search />
          ) : (
            <EmptyState tab={tab} onNew={() => openNewSessionWindow().catch(() => {})} />
          )
        ) : (
          <div
            ref={scrollInnerRef}
            style={{ height: totalSize, width: "100%", position: "relative" }}
          >
            {virtualItems.map((virtualItem) => {
              const l = shown[virtualItem.index];
              const unnamed = !l.task_title || l.task_title === UNNAMED_SESSION_SENTINEL;
              const title = unnamed ? t.sticker.waitingFirstInput : l.task_title;
              const agentIcon = agentAssets(l.provider); // 品牌图标（视觉资产，按 id 查表）
              const agentLabel = agentNameOf(l.provider); // 展示名（后端下发；未知 id 显示 id 本身）
              // AI 活动行显示「最近一条 AI 正文」(last_ai_text，回退 transcript preview)；出错优先显示错误标签；
              // previewEnabled（对话预览开关）关闭则不显示 AI 正文（仅保留错误）；用户行同受该开关门控。
              const sub = l.errored && l.error_label
                ? t.errorLabels[l.error_label] ?? l.error_label
                : previewEnabled && (l.last_ai_text ?? l.preview)
                ? (l.last_ai_text ?? l.preview)
                : null;
              const subTitle = l.errored ? l.error_raw ?? undefined : sub ?? undefined;
              // 屏幕检测（screen_state，仅托管会话有值）优先于 DB status：DB 的 running 由
              // hook 事件驱动，agent 在屏幕上弹出审批/提问时它还停在上一个事件；屏幕状态
              // 300ms 级实时。pending_review（broker 正压着的审批）仍最优先——那是事实源。
              const indicator = !l.connected ? (
                <span className="ring-stop" data-tip={t.sticker.stopped} />
              ) : l.errored ? (
                <span className="needs-error" data-tip={l.error_raw ?? t.sticker.sessionError} />
              ) : l.pending_review || l.screen_state === "blocked" ? (
                <RunBadge pct={l.context_pct} tone="pending" />
              ) : l.screen_state === "working" ? (
                <RunBadge pct={l.context_pct} />
              ) : l.screen_state === "idle" ? (
                <RunBadge pct={l.context_pct} tone="waiting" />
              ) : l.session.status === "running" ? (
                <RunBadge pct={l.context_pct} />
              ) : l.session.status === "waiting" ? (
                <RunBadge pct={l.context_pct} tone="waiting" />
              ) : (
                <span className="sdot sdot-on" data-tip={t.sticker.online} />
              );
              return (
                <div
                  key={virtualItem.key}
                  data-index={virtualItem.index}
                  ref={virtualizer.measureElement}
                  className="stk-vitem"
                  style={{
                    position: "absolute",
                    top: 0,
                    left: 0,
                    width: "100%",
                    transform: `translateY(${virtualItem.start}px)`,
                  }}
                >
                  <div
                    className={"stk-card" + (isStarred(l) ? " is-star" : "")}
                    role="button"
                    tabIndex={0}
                    onClick={() => {
                      // 编辑态(重命名/便签)下，点击卡片仅用于关闭编辑器（失焦），绝不导航开终端。
                      // 注：编辑输入框已 stopPropagation，这里只会被「点击卡片空白处」触发。
                      if (editingId !== null || notingId !== null) {
                        setEditingId(null);
                        setNotingId(null);
                        return;
                      }
                      // 按钮模式：点击卡片不开终端，改由卡片上的打开按钮触发。
                      if (buttonMode) return;
                      openTerminal(l);
                    }}
                    onKeyDown={(e) => {
                      // 键盘可达：焦点在卡片本体时 Enter/Space 等效点击。内部按钮/输入框的
                      // 键盘事件(target 不是卡片)放行，由它们自己的 click 处理，避免双重触发。
                      if (e.target !== e.currentTarget) return;
                      if (e.key !== "Enter" && e.key !== " ") return;
                      e.preventDefault(); // 阻止 Space 触发页面滚动
                      if (editingId !== null || notingId !== null) {
                        setEditingId(null);
                        setNotingId(null);
                        return;
                      }
                      if (buttonMode) return;
                      openTerminal(l);
                    }}
                    onContextMenu={(e) => {
                      // 与卡片菜单按钮二选一：button 模式下右键只吞掉默认 webview 菜单、不弹卡片菜单。
                      e.preventDefault();
                      if (menuMode === "context") {
                        setCtxMenu({ sid: l.session.id, x: e.clientX, y: e.clientY });
                      }
                    }}
                    style={{ cursor: !buttonMode && canOpen(l) ? "pointer" : "default" }}
                    data-tip={buttonMode ? "" : l.connected ? t.sticker.openSession : l.archived ? "" : t.sticker.resumeSession}
                  >
                    <div className="stk-top">
                      <span className="stk-ind">{indicator}</span>
                      <div className="stk-top-body">
                        <div className="stk-line1">
                          {editingId === l.session.id ? (
                            <EditBox
                              initial={l.task_title && l.task_title !== UNNAMED_SESSION_SENTINEL ? l.task_title : ""}
                              placeholder={t.sticker.renamePlaceholder}
                              inputClass="stk-edit"
                              saveLabel={t.sticker.noteSave}
                              cancelLabel={t.sticker.noteCancel}
                              onSubmit={(value) => submitRename(l, value)}
                              onCancel={() => setEditingId(null)}
                            />
                          ) : (
                            <>
                              <span className="stk-title">{title}</span>
                              {/* 断开的会话不挂交互标签：进程都没了，「待批准」只会催用户去点一个
                                  点不动的东西。DB 里的 pending_review 是收尾时没清干净的残留。 */}
                              {l.connected && l.pending_review && (
                                <span className={"pending-pill pending-" + l.pending_review}>
                                  {t.pending[l.pending_review]}
                                </span>
                              )}
                              <span className="stk-time">{fmtAgo(l.session.last_event_at, t)}</span>
                              <button
                                type="button"
                                className="stk-chat-btn"
                                aria-label={t.sticker.openChat}
                                data-tip={t.sticker.openChat}
                                onClick={(e) => {
                                  e.stopPropagation();
                                  invoke("open_chat_window", { sessionId: l.session.id }).catch(() => {});
                                }}
                              ><ChatIcon /></button>
                              {/* 置顶/便签/重命名/归档操作收进卡片菜单（CardContextMenu），标题行不再挤 hover 图标。
                                  默认右键触发；card_menu_mode=button（触屏等不便右键）时改为此处的常显菜单按钮，
                                  两种触发方式二选一。置顶态由卡片金角、便签由便签块表达，收起入口不丢信息。 */}
                              {menuMode === "button" && (
                                <button
                                  type="button"
                                  className="stk-menu-btn"
                                  aria-label={t.sticker.cardMenu}
                                  data-tip={t.sticker.cardMenu}
                                  onClick={(e) => {
                                    e.stopPropagation();
                                    const r = e.currentTarget.getBoundingClientRect();
                                    setCtxMenu({ sid: l.session.id, x: r.right, y: r.bottom + 4 });
                                  }}
                                ><MoreIcon /></button>
                              )}
                            </>
                          )}
                        </div>
                        {editingId === l.session.id && l.connected && (
                          <div className="stk-edit-hint">{t.sticker.renameHint}</div>
                        )}
                        <div className="stk-line2">
                          <span
                            className={"stk-agent" + (l.connected ? "" : " stk-agent-off")}
                            style={tintStyle(l.provider, l.connected)}
                            data-tip={agentLabel}
                            role="img"
                            aria-label={agentLabel}
                          >
                            <agentIcon.Icon />
                          </span>
                          {l.cwd && (
                            <span className="stk-repo" data-tip={l.cwd}>
                              {l.cwd.split(/[\\/]/).filter(Boolean).pop() ?? l.cwd}
                            </span>
                          )}
                          {/* Agent 从这个会话派生出去的后台作业**不在卡片上留痕**：它们自己
                              不建卡（见后端 hidden_background），源会话卡上也不标数——用户既
                              管不了这些作业（终端按键无效、发消息在手动模式下不会被执行），
                              一个数字除了让人困惑并无用处。要处理请回派出它们的终端。 */}
                          {/* 所属账号徽章：默认账号（profile_name 为 null）不显示——
                              没建过 profile 的用户零感知。 */}
                          {l.profile_name && (
                            <span
                              className="stk-profile"
                              data-tip={t.account.activeProfileBadge(l.profile_name)}
                              data-testid="stk-profile-badge"
                            >
                              {l.profile_name}
                            </span>
                          )}
                          {l.model && <span className="stk-model">{l.model}</span>}
                        </div>
                      </div>
                    </div>
                    {notingId === l.session.id ? (
                      <EditBox
                        initial={l.note ?? ""}
                        placeholder={t.sticker.notePlaceholder}
                        inputClass="stk-note-edit"
                        extraClass="stk-editbox-note"
                        icon={<span className="stk-editbox-ico"><NoteIcon /></span>}
                        saveLabel={t.sticker.noteSave}
                        cancelLabel={t.sticker.noteCancel}
                        onSubmit={(value) => submitNote(l, value)}
                        onCancel={() => setNotingId(null)}
                      />
                    ) : l.note ? (
                      <div
                        className="stk-note"
                        data-tip={t.sticker.noteEdit}
                        role="button"
                        tabIndex={0}
                        onClick={(e) => { e.stopPropagation(); startNote(l); }}
                        onKeyDown={(e) => {
                          if (e.target !== e.currentTarget) return;
                          if (e.key !== "Enter" && e.key !== " ") return;
                          e.preventDefault();
                          e.stopPropagation(); // 不冒泡给卡片，避免误开终端
                          startNote(l);
                        }}
                      >
                        <span className="stk-note-icon"><NoteIcon /></span>
                        <span className="stk-note-txt">{l.note}</span>
                      </div>
                    ) : null}
                    {previewEnabled && l.last_user_text && (
                      <div className="stk-subrow stk-userrow">
                        <span className="stk-msg-tag">{t.sticker.youPrefix}</span>
                        <span className="stk-sub" data-tip={l.last_user_text}>{l.last_user_text}</span>
                      </div>
                    )}
                    {(sub || (buttonMode && canOpen(l))) && (
                      <div className="stk-subrow">
                        {/* 活动行：最近一条 AI 正文(或错误标签)，单行截断；title 给完整文本，hover 原生提示可读全文 */}
                        {sub && !l.errored && <span className="stk-msg-tag is-ai">{t.sticker.aiPrefix}</span>}
                        {sub && <span className={"stk-sub" + (l.errored ? " stk-sub-err" : "")} data-tip={subTitle}>{sub}</span>}
                        {/* 按钮模式：打开终端按钮内联在该行末尾，不突兀 */}
                        {buttonMode && canOpen(l) && (
                          <button
                            type="button"
                            className="stk-open"
                            data-tip={l.connected ? t.sticker.openSession : t.sticker.resumeSession}
                            aria-label={l.connected ? t.sticker.openSession : t.sticker.resumeSession}
                            onClick={(e) => { e.stopPropagation(); openTerminal(l); }}
                          ><OpenIcon /></button>
                        )}
                      </div>
                    )}
                  </div>
                </div>
              );
            })}
            {isLoadingMore && (
              <div className="stk-loadmore" style={{ position: "absolute", top: totalSize, left: 0, width: "100%" }}>
                <span className="stk-loadmore-dot" />
                <span className="stk-loadmore-dot" />
                <span className="stk-loadmore-dot" />
              </div>
            )}
          </div>
        )}
      </div>
      {sb && (
        <div
          className={"stk-sb" + (sbDrag ? " is-drag" : "")}
          style={{ top: sb.top, height: sb.height }}
          onMouseDown={onSbDown}
        />
      )}
      {ctxMenu && ctxItem && (
        <CardContextMenu
          x={ctxMenu.x}
          y={ctxMenu.y}
          starred={isStarred(ctxItem)}
          hasNote={!!ctxItem.note}
          archived={ctxItem.archived}
          onStar={() => toggleStar(ctxItem.session.cc_session_id)}
          onNote={() => startNote(ctxItem)}
          onRename={() => startRename(ctxItem)}
          onArchive={() => {
            const target = !ctxItem.archived;
            const sessionId = ctxItem.session.id;
            // 先乐观通知父层摘掉卡片（点完菜单即刻消失，不等 IPC 往返），失败再回滚。
            onArchiveOptimistic?.(sessionId);
            setArchived(sessionId, target).catch(() => onArchiveFailed?.());
          }}
          onNewSession={() =>
            openNewSessionWindow({ cwd: ctxItem.cwd, provider: ctxItem.provider }).catch(() => {})
          }
          onOpenDir={
            ctxItem.cwd ? () => openProjectDir(ctxItem.cwd!).catch(() => {}) : null
          }
          onEndSession={
            // 仅本 GUI 托管的 PTY 能结束（与对话窗标题栏入口同门控）；确认+停止走共用的
            // confirmStopSession。成功后不用做别的：轮询里 connected 翻 false、徽标自退。
            // 失败也由徽标兜底可见——卡片仍显「运行中」即说明没停掉，贴纸窗无独立错误槽位。
            // 后台会话恒不满足 pty_managed（PTY 在 agent 的 daemon 手上），但它有自己的
            // 结束入口——后端会连上那条旁路 socket 发 kill 控制帧，比对着 pid 下手干净：
            // 杀进程会被 supervisor 按 respawnFlags 原地拉回来。
            ctxItem.pty_managed || ctxItem.background
              ? () => {
                  void confirmStopSession(ctxItem.session.id, {
                    title: t.chat.endSession,
                    message: t.chat.endSessionConfirm,
                  }).catch(() => {});
                }
              : null
          }
          onClose={() => setCtxMenu(null)}
        />
      )}
      {focusNotice && focusNotice.kind !== "focused" && (
        <div className="stk-focus-toast" role="status" onClick={(e) => e.stopPropagation()}>
          <span className="stk-focus-mark" aria-hidden="true">!</span>
          <div className="stk-focus-body">
            <span className="stk-focus-text">{focusNoticeText}</span>
            <div className="stk-focus-actions">
              {focusNotice.confirming && (
                <button
                  type="button"
                  className="stk-focus-btn is-quiet"
                  onClick={() => setFocusNotice({ ...focusNotice, confirming: false })}
                >
                  {t.sticker.noteCancel}
                </button>
              )}
              {(["host_focused", "unsupported_terminal", "alive_but_not_found", "process_ended"] as FocusNoticeKind[]).includes(focusNotice.kind) && (
                <button type="button" className="stk-focus-btn" disabled={focusNotice.busy} onClick={reopenNoticeSession}>
                  {focusNotice.busy
                    ? t.sticker.reopening
                    : focusNotice.confirming
                    ? t.sticker.endAndReopen
                    : focusNotice.kind === "process_ended"
                    ? t.sticker.reopen
                    : t.sticker.reopenSupported}
                </button>
              )}
            </div>
          </div>
          <button
            type="button"
            className="stk-focus-close"
            aria-label={t.sticker.dismiss}
            onClick={() => setFocusNotice(null)}
          >
            <XIcon />
          </button>
        </div>
      )}
      {/* 底栏:用量(左) + 搜索/设置/固定(右)聚为一处;搜索激活时整条变输入框。 */}
      <div className="stk-bar">
        {searchOpen ? (
          <div className="stk-search">
            <span className="stk-search-ic">
              <SearchIcon />
            </span>
            <input
              className="stk-search-in"
              autoFocus
              value={q}
              placeholder={t.sticker.searchPlaceholder}
              onChange={(e) => onSearchChange?.(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Escape") closeSearch();
              }}
            />
            <button type="button" className="stk-act stk-search-x" data-tip={t.sticker.searchClose} aria-label={t.sticker.searchClose} onClick={closeSearch}>
              <CloseIcon />
            </button>
          </div>
        ) : (
          <>
            <UsageScreen quotaProviders={shownQuota} usageMap={usageMap} />
            <div className="stk-bar-actions">
              <button
                type="button"
                className="stk-act"
                data-tip={t.sticker.openChatWindow}
                aria-label={t.sticker.openChatWindow}
                data-testid="bar-chat"
                onClick={() => invoke("open_latest_chat").catch(() => {})}
              >
                <ChatIcon />
              </button>
              <button
                type="button"
                className="stk-act"
                data-tip={t.newSession.newButton}
                aria-label={t.newSession.newButton}
                data-testid="bar-new"
                onClick={() => openNewSessionWindow().catch(() => {})}
              >
                <PlusIcon />
              </button>
              <button type="button" className="stk-act" data-tip={t.sticker.search} aria-label={t.sticker.search} onClick={() => setSearchOpen(true)}>
                <SearchIcon />
              </button>
              <button
                type="button"
                className="stk-act"
                data-tip={hasUpdate ? t.sticker.updateAvailable : t.sticker.openSettings}
                aria-label={hasUpdate ? t.sticker.updateAvailable : t.sticker.openSettings}
                // 有更新时红点按钮直达更新窗口，否则照常打开设置。
                onClick={() => invoke(hasUpdate ? "open_update_window" : "open_settings").catch(() => {})}
              >
                <GearIcon />
                {hasUpdate && <span className="stk-dot" aria-hidden="true" />}
              </button>
              {!isMacPanel() && (
                <button
                  type="button"
                  className={"stk-act " + (pinned ? "stk-pin-on" : "")}
                  data-tip={pinned ? t.sticker.pinOn : t.sticker.pinOff}
                  aria-label={pinned ? t.sticker.pinOn : t.sticker.pinOff}
                  onClick={togglePin}
                >
                  <PinIcon pinned={pinned} />
                </button>
              )}
            </div>
          </>
        )}
      </div>
    </div>
  );
}

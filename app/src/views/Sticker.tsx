import {
  type MouseEvent as ReactMouseEvent,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useVirtualizer } from "@tanstack/react-virtual";
import { folderName } from "../paths";
import {
  LiveSessionCounts,
  Settings,

  TerminalOpenMode,
  confirmStopSession,
  firstImportNotice,
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
  type SessionOpenIn,
} from "../api";
import { isMacPanel } from "../platform";
import { appConfirm } from "../confirm";
import { DevBadge } from "./DevBadge";
import { useSettingsEffect } from "../hooks/useSettings";
import { agentAssets, tintStyle } from "../providers";
import { useAgents } from "../useAgents";
import { useT } from "../i18n";
import { formatBackendError } from "../i18n/errors";
import { type Item, type Tab, TAB_KEYS, PIN_KEY, UNNAMED_SESSION_SENTINEL } from "./sticker/types";
import { cardTone, editorKeyDown, fmtAgo, fmtWaited, useStarred, match, toneConfidence } from "./sticker/helpers";
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

type FocusNoticeKind = FocusSessionResult | "connecting" | "failed" | "background" | "archived" | "resuming" | "foreign" | "opened";

/** 便签字符上限：与后端 session_command.rs 的 `chars().take(500)` 截断阈值对齐。
 *  前端不设限时超长便签被后端静默截断，用户丢字无感。 */
const NOTE_MAX_CHARS = 500;

/** 卡片就地编辑框（重命名/便签共用）。草稿住在本组件的局部状态：放在 Sticker 的 state 里时
 *  每次按键 setState 都让整个看板重渲染（虚拟化把代价压到可见行，但可见行的整卡重建
 *  仍是白付；侧栏同款问题见 ChatSidebar 的 EditorInput）。局部草稿经 onDraft 同步到
 *  Sticker 的 ref（不落 state、不触发重渲染），虚拟化滚动卸载后 remount 按草稿恢复（B-9）。
 *  提交/取消语义与侧栏对齐：Enter/✓/失焦 提交、Esc/✗ 取消。按钮的 mousedown preventDefault
 *  保证点 ✗ 不先触发失焦提交;done 标志防 Enter 提交后卸载期的 blur 二次提交。 */
function EditBox({ initial, placeholder, inputClass, extraClass, icon, saveLabel, cancelLabel, maxLength, onDraft, onSubmit, onCancel }: {
  initial: string;
  placeholder: string;
  inputClass: string;
  extraClass?: string;
  icon?: JSX.Element;
  saveLabel: string;
  cancelLabel: string;
  /** 字符上限（如便签 500，与后端截断阈值对齐——不给上限时后端静默截断，用户丢字无感）。
   *  接近上限（≥80%）时显示剩余计数。 */
  maxLength?: number;
  /** 按键时把草稿同步给父层（ref，不触发重渲染）：虚拟列表滚出视口会卸载本组件，
   *  局部草稿随之丢失（B-9）；父层在 remount 时用它回填 initial。 */
  onDraft?: (value: string) => void;
  onSubmit: (value: string) => void;
  onCancel: () => void;
}) {
  const [value, setValue] = useState(initial);
  const done = useRef(false);
  const submit = (v: string) => { if (done.current) return; done.current = true; onSubmit(v); };
  const cancel = () => { if (done.current) return; done.current = true; onCancel(); };
  const nearLimit = maxLength != null && value.length >= maxLength * 0.8;
  return (
    <div className={"stk-editbox" + (extraClass ? ` ${extraClass}` : "")} onClick={(e) => e.stopPropagation()}>
      {icon}
      <input
        className={inputClass}
        autoFocus
        value={value}
        placeholder={placeholder}
        maxLength={maxLength}
        onChange={(e) => { setValue(e.target.value); onDraft?.(e.target.value); }}
        onKeyDown={editorKeyDown(() => submit(value), cancel)}
        onBlur={() => submit(value)}
      />
      {nearLimit && <span className="stk-editbox-count">{maxLength! - value.length}</span>}
      {/* mousedown preventDefault：点按钮不抢走输入框焦点，避免触发失焦提交 */}
      <button
        type="button"
        className="stk-ebtn stk-ebtn-ok"
        aria-label={saveLabel}
        data-tip={saveLabel}
        onMouseDown={(e) => e.preventDefault()}
        onClick={() => submit(value)}
      ><CheckIcon /></button>
      <button
        type="button"
        className="stk-ebtn"
        aria-label={cancelLabel}
        data-tip={cancelLabel}
        onMouseDown={(e) => e.preventDefault()}
        onClick={cancel}
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
  updateStatus,
  search,
  onSearchChange,
  onArchiveOptimistic,
  onArchiveFailed,
  onArchiveUndo,
  initialLoading,
  loadError,
  onRetry,
  snapped,
  switching,
  pinned: pinnedProp,
  onTogglePin,
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
  /// 更新阶段（available / downloading / ready）：红点按钮的提示按阶段分（7S-3）。
  updateStatus?: string;
  search?: string;
  onSearchChange?: (q: string) => void;
  /** 归档请求已发出（未确认）：父层可据此乐观摘掉卡片。 */
  onArchiveOptimistic?: (sessionId: number) => void;
  /** 归档请求失败：父层需回滚上面的乐观更新。 */
  onArchiveFailed?: () => void;
  /** 撤销归档成功：父层需整窗重查（增量刷新捞不回被摘掉的旧会话，P1-1）。 */
  onArchiveUndo?: () => void;
  /** 冷启动首次加载中：true 时显示加载占位而非假空态。 */
  initialLoading?: boolean;
  /** 首页加载失败：显示「加载失败 + 重试」而非「还没有会话」。 */
  loadError?: boolean;
  /** 重试回调：重新发起首页加载。 */
  onRetry?: () => void;
  /** 吸附展开（偷看）态：置顶归后端 snap_* 管，此时 pin 开关只记偏好、不写窗口。 */
  snapped?: boolean;
  /** 切 tab/搜索的首页请求在途（B-4）：保留旧列表灰化直到新数据就绪。 */
  switching?: boolean;
  /** 窗口置顶偏好（W-17，存 settings.json）：由 App 下发；缺席时退回旧 localStorage 自管路径
   *  （demo/poster/直挂测试）。 */
  pinned?: boolean;
  /** 切换窗口置顶：由 App 落盘 settings 并回推 pinned。 */
  onTogglePin?: () => void;
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
    // 切 tab 保留搜索词:搜索是「当前 tab 内全库搜」,与 tab 正交——搜到词后在
    // 「运行中/等待中」之间切换对比是常见动线,无声清词等于逼用户重输(实拍反馈)。
    onFilterChange?.(t);
  };

  // tab 滑块按选中按钮实测定位（effect 在 counts 声明后注册，见下方 useLayoutEffect）。
  const tabsegRef = useRef<HTMLDivElement>(null);
  const [sliderStyle, setSliderStyle] = useState<{ left: number; width: number } | undefined>(undefined);

  // 会话搜索：激活时底栏整条变成输入框；搜索词经 onSearchChange 交给父组件下沉到后端
  // （当前 tab 内全库搜，覆盖未加载数据），本组件不再持有搜索词、不做客户端过滤。
  const [searchOpen, setSearchOpen] = useState(false);
  // 搜索框实体（Ctrl+F 在「已开」时也要能把焦点抢回来，见窗口级快捷键）。
  const searchInputRef = useRef<HTMLInputElement>(null);
  const q = search ?? "";
  const closeSearch = () => {
    setSearchOpen(false);
    onSearchChange?.("");
  };

  // 打开终端方式 + 卡片菜单方式 + 贴纸配额 provider 列表：启动时读设置，并监听实时变更。
  const [openMode, setOpenMode] = useState<TerminalOpenMode>("card");
  // 首帧占位与后端真实默认（default_card_menu_mode=button）一致：曾占位 "context"，
  // 每次挂载都先渲染一批没有 ⋯ 按钮的卡片、settings 回来按钮才凭空弹入并挤压标题行。
  const [menuMode, setMenuMode] = useState<CardMenuMode>("button");
  const [previewEnabled, setPreviewEnabled] = useState(true);
  // 对话窗口功能开关（轻量模式=false）：关闭时贴纸上不渲染任何 chat 入口。
  // 首帧占位 true 与后端默认一致（default_true），避免开关用户的按钮闪现/消失。
  const [chatEnabled, setChatEnabled] = useState(true);
  // 会话打开去处（chat/terminal）：跳转/恢复成功的 toast 要说出「在哪打开了」。
  const [sessionOpenIn, setSessionOpenIn] = useState<SessionOpenIn>("chat");
  // 空初值：settings resolve 前不渲染配额区，好过先闪一个猜出来的 agent。默认值由后端给。
  const [quotaProviders, setQuotaProviders] = useState<string[]>([]);
  // 中转启用状态改变时重建贴纸用量请求：启用后立即隐藏官方配额，关闭后立即恢复缓存并刷新。
  const [usageRevision, setUsageRevision] = useState(0);
  const relaySignatureRef = useRef<string | null>(null);

  useSettingsEffect((s) => {
    setOpenMode(s.terminal_open_mode);
    setMenuMode(s.card_menu_mode ?? "button");
    setPreviewEnabled(s.preview_enabled);
    setChatEnabled(s.chat_enabled ?? true);
    setSessionOpenIn(s.session_open_in ?? "chat");
    setQuotaProviders(s.sticker_quota_providers ?? []);
    const signature = relayEnabledSignature(s);
    if (relaySignatureRef.current !== null && relaySignatureRef.current !== signature) {
      setUsageRevision((n) => n + 1);
    }
    relaySignatureRef.current = signature;
  });

  // 相对时间（fmtAgo）每分钟重算：递增计数触发轻量重渲染。
  const [, setTick] = useState(0);
  useEffect(() => {
    const id = window.setInterval(() => setTick((n) => n + 1), 60_000);
    return () => window.clearInterval(id);
  }, []);

  // 置顶开关：默认不置顶，激活后才把窗口设为 alwaysOnTop。偏好由 App 经 props 下发
  // （W-17：持久化在 settings.json）；未下发时（demo/poster/直挂测试）退回 localStorage 自管。
  const [pinnedLocal, setPinnedLocal] = useState<boolean>(() => localStorage.getItem(PIN_KEY) === "1");
  const pinned = pinnedProp ?? pinnedLocal;
  useEffect(() => {
    // macOS 面板（W-12）：面板层级本就常驻最前（NSMainMenuWindowLevel+1），always-on-top
    // 无意义；pin 的语义在这里是「失焦不自动收起」——推给后端 resign 监听器。
    if (isMacPanel()) {
      invoke("set_panel_keep_open", { open: pinned }).catch(() => {});
      return;
    }
    // 吸附展开（偷看）态不写：snap_expand 已强制置顶保证贴边窗口可见，这里若按 pinned=false
    // 覆盖回去，展开的看板会立刻沉到活动窗口之下（表现为「悬停后闪一下就没了」）。
    // 置顶偏好照常由 togglePin 落盘（settings，W-17），还原普通窗口时由 snap_restore(pinned) 统一套用。
    if (snapped) return;
    // 非 Tauri 环境（测试/浏览器）下 getCurrentWindow 会抛错，吞掉即可。
    try {
      getCurrentWindow().setAlwaysOnTop(pinned).catch(() => {});
    } catch {
      /* noop */
    }
  }, [pinned, snapped]);
  const togglePin = () => {
    if (onTogglePin) {
      onTogglePin();
      return;
    }
    setPinnedLocal((p) => {
      const next = !p;
      localStorage.setItem(PIN_KEY, next ? "1" : "0");
      return next;
    });
  };
  // macOS 面板上 pin 的语义是「保持打开（失焦不收起）」（W-12），文案随之切换。
  const pinTip = isMacPanel()
    ? pinned ? t.sticker.keepOpenOn : t.sticker.keepOpenOff
    : pinned ? t.sticker.pinOn : t.sticker.pinOff;

  // 托盘「找回贴纸」只做临时置顶（recall_center 聚焦后按 pin 偏好还原，W-6），不改写
  // 持久化的 pin 偏好、不动这里的 pinned 状态——用户的置顶偏好不被「找回」顺手改掉。

  // 会话置顶：置顶的会话永远排到列表最前（跨重启保留）。与「置顶窗口(alwaysOnTop)」是两回事——
  // 那是窗口行为，图标也刻意分开（这里 TopIcon，窗口那边 PinIcon）。存储键沿用 meowo-starred。
  // useStarred：与对话窗侧栏/标题菜单同一份写入口，且跨窗口（storage 事件）同步——
  // 在对话窗里置顶，看板立刻跟上，反之亦然。
  const { starred, toggleStar } = useStarred();

  // 星标 FLIP（B-16）：置顶切换触发「星标浮顶」重排，卡片瞬移、用户找不到刚操作的卡。
  // 切换前量出可见卡的纵向位置（翻位置要在重排触发之前——虚拟列表下只有挂载中的卡可量），
  // 重排落地后由下方 useLayoutEffect 反向补偿 transform 再过渡到 0，卡片平滑滑到新位置。
  const flipPrevRef = useRef<Map<number, number> | null>(null);
  const toggleStarFlip = (ccSessionId: string) => {
    const map = new Map<number, number>();
    scrollInnerRef.current?.querySelectorAll<HTMLElement>("[data-sid]").forEach((el) => {
      map.set(Number(el.dataset.sid), el.getBoundingClientRect().top);
    });
    flipPrevRef.current = map;
    toggleStar(ccSessionId);
  };

  // 乐观覆盖层：重命名/便签提交后立即按新值渲染（与归档的乐观摘卡同一原则——
  // 此前 Enter 后编辑框收起、标题/便签闪回旧值，要等 board-changed（节流 400ms +
  // 整页重查）才刷新，几百毫秒的「改了个寂寞」窗口让用户以为没保存而重复操作）。
  // 后端广播带回同值后条目退场（见下方 effect）；IPC 失败回滚并弹 focusNotice。
  const [optimistic, setOptimistic] = useState<Map<number, { title?: string; note?: string }>>(new Map());
  const patchOptimistic = (id: number, patch: { title?: string; note?: string }) => {
    setOptimistic((m) => new Map(m).set(id, { ...m.get(id), ...patch }));
  };
  const dropOptimistic = (id: number, key: "title" | "note") => {
    setOptimistic((m) => {
      const cur = m.get(id);
      if (!cur || cur[key] === undefined) return m;
      const rest = { ...cur };
      delete rest[key];
      const next = new Map(m);
      if (rest.title === undefined && rest.note === undefined) next.delete(id);
      else next.set(id, rest);
      return next;
    });
  };
  // 数据已带回乐观值 → 覆盖层退场。data 引用只在真实变化时更新（App 的行级结构共享），
  // 空转刷新不会进这里。会话从列表消失（归档/过滤）也一并清，防 Map 缓慢增长。
  useEffect(() => {
    setOptimistic((m) => {
      if (m.size === 0) return m;
      let changed = false;
      const next = new Map(m);
      for (const [id, o] of m) {
        const row = data.find((x) => x.session.id === id);
        if (!row) { next.delete(id); changed = true; continue; }
        const rest = { ...o };
        let entryChanged = false;
        if (rest.title !== undefined && row.task_title === rest.title) { delete rest.title; entryChanged = true; }
        if (rest.note !== undefined && (row.note ?? "") === rest.note) { delete rest.note; entryChanged = true; }
        if (!entryChanged) continue;
        changed = true;
        if (rest.title === undefined && rest.note === undefined) next.delete(id);
        else next.set(id, rest);
      }
      return changed ? next : m;
    });
  }, [data]);

  // 重命名：editingId 为正在编辑的会话 id。草稿文本住在 EditBox 的局部状态里，
  // 同时经 onDraft 同步到 editDraftsRef（B-9）：编辑中滚动列表会把卡片随虚拟行卸载，
  // 局部草稿随之静默丢失、滚回显示原文；提升到本层 ref 后 remount 时按草稿回填。
  // 存 ref 不存 state：草稿只在 EditBox 挂载时读一次，按键不需要整板重渲染。
  // 键含编辑类型前缀（r:=重命名 / n:=便签），同卡的两种草稿互不串。
  const editDraftsRef = useRef<Map<string, string>>(new Map());
  const [editingId, setEditingId] = useState<number | null>(null);
  // 失焦提交后,blur 先于 click 触发:click 到达时编辑态已清,「编辑态点卡片只关编辑器、
  // 不导航」的守卫会漏。记录编辑器关闭时刻,250ms 内的卡片点击视为同一手势不导航。
  const editCloseAtRef = useRef(0);
  const markEditClosed = () => { editCloseAtRef.current = Date.now(); };
  const startRename = (l: Item) => {
    setNotingId(null); // 与便签编辑互斥，同卡只开一个编辑器
    editDraftsRef.current.delete(`r:${l.session.id}`); // 新一轮编辑从原文开始，不回填旧草稿
    setEditingId(l.session.id);
  };
  const submitRename = (l: Item, draft: string) => {
    editDraftsRef.current.delete(`r:${l.session.id}`);
    // 局部变量原名 t 与 i18n 字典 t 冲突，改名 title 以便失败提示取字典文案。
    const title = draft.trim();
    if (title && title !== l.task_title) {
      // 乐观：立即按新标题渲染；失败回滚 + focusNotice 提示（detail 直接展示文案，4s 自动消失）。
      patchOptimistic(l.session.id, { title });
      renameSession(l.cwd, l.session.cc_session_id, title, l.provider)
        .catch(() => {
          dropOptimistic(l.session.id, "title");
          setFocusNotice({ kind: "failed", item: l, detail: t.sticker.renameFailed });
        });
    }
    markEditClosed();
    setEditingId(null);
  };

  // 卡片右键菜单：记录目标会话 id 与打开坐标。item 每次渲染按 id 从 data 现查——
  // 数据刷新后菜单内容(置顶/便签/归档态)保持最新,会话消失则菜单自然不渲染。
  const [ctxMenu, setCtxMenu] = useState<{ sid: number; x: number; y: number } | null>(null);
  const ctxItem = ctxMenu ? data.find((l) => l.session.id === ctxMenu.sid) ?? null : null;



  // 便签编辑：notingId 为正在编辑便签的会话 id。与重命名互斥（同卡只开一个）。
  const [notingId, setNotingId] = useState<number | null>(null);
  // 长便签 3 行 clamp 后的展开态：便签块常显只给 3 行（防 500 字便签撑满窗口），
  // 首次点击展开全文，再次点击才进编辑（短便签未被 clamp，一次点击直接编辑）。
  const [expandedNoteId, setExpandedNoteId] = useState<number | null>(null);
  const [focusNotice, setFocusNotice] = useState<{
    kind: FocusNoticeKind;
    item: Item;
    busy?: boolean;
    detail?: string;
  } | null>(null);
  useEffect(() => {
    if (!focusNotice || focusNotice.busy) return;
    // resuming 由 resume_session 的 promise settle 时换掉（成功换 opened 正反馈/失败换 failed），不走定时。
    // permission_denied 带着「打开系统设置」动作，4s 消失会让用户来不及点——常驻到手动关。
    if (["host_focused", "unsupported_terminal", "alive_but_not_found", "process_ended", "resuming", "permission_denied"].includes(focusNotice.kind)) return;
    const id = window.setTimeout(() => setFocusNotice(null), 4_000);
    return () => window.clearTimeout(id);
  }, [focusNotice]);

  // 归档撤销 toast 独立成数组:连续归档两张卡时,单槽 focusNotice 会让第二条直接覆盖
  // 第一条的「撤销」窗口——每条各自渲染、各自 6s 消失,撤销互不相吞。
  const [archivedToasts, setArchivedToasts] = useState<{ key: number; item: Item }[]>([]);
  const archiveToastIdRef = useRef(0);
  // 各 toast 的消失定时器：卸载时统一清掉，不让回调打在已卸载的组件上。
  const archiveToastTimersRef = useRef<Set<number>>(new Set());
  const pushArchivedToast = (item: Item) => {
    archiveToastIdRef.current += 1;
    const key = archiveToastIdRef.current;
    // 7B-9：连续归档时曾无上限堆叠，四条就盖住半张贴纸。只留最近两条——更早的那次
    // 撤销窗口本来也快到点了，超时清理由各自的定时器照旧负责（找不到即空转）。
    setArchivedToasts((cur) => [...cur, { key, item }].slice(-2));
    const timer = window.setTimeout(() => {
      archiveToastTimersRef.current.delete(timer);
      setArchivedToasts((cur) => cur.filter((toast) => toast.key !== key));
    }, 6_000);
    archiveToastTimersRef.current.add(timer);
  };
  useEffect(() => {
    const timers = archiveToastTimersRef.current;
    return () => timers.forEach((timer) => window.clearTimeout(timer));
  }, []);

  // 首启历史导入的一次性提示（S-12）：导入曾全程零说明。后端 first_import_notice
  // 取到即落「已提示」标记、保证只弹一次；拉取失败静默（提示而已，不能拖垮看板）。
  const [importNotice, setImportNotice] = useState<number | null>(null);
  useEffect(() => {
    firstImportNotice()
      .then((n) => { if (n) setImportNotice(n); })
      .catch(() => {});
  }, []);
  useEffect(() => {
    if (importNotice === null) return;
    const id = window.setTimeout(() => setImportNotice(null), 10_000);
    return () => window.clearTimeout(id);
  }, [importNotice]);

  // 窗口级快捷键：Ctrl/Cmd+F 打开搜索；Esc 按「toast → 搜索」回退（右键菜单的 Esc 由
  // CardContextMenu 自己消费并 preventDefault，先于本监听）。焦点在输入框里时 Esc 让位
  // 给各自的 onKeyDown（搜索框自己会关，编辑框是取消编辑）。此前贴纸没有任何快捷键，
  // 搜索/关 toast 都必须用鼠标——对一个常驻小窗，这是高频路径。
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && !e.altKey && e.code === "KeyF") {
        e.preventDefault();
        setSearchOpen(true);
        // 7B-7：此前只处理「未开 → 开」（靠 input 的 autoFocus 拿焦点）。搜索框已经开着、
        // 焦点在卡片上时再按 Ctrl+F 完全没反应——用户的本意是「回到搜索框改词」。
        // 已开时显式 focus + 全选：再按一次即可整段重输。
        searchInputRef.current?.focus();
        searchInputRef.current?.select();
        return;
      }
      if ((e.ctrlKey || e.metaKey) && !e.altKey && e.code === "KeyN") {
        e.preventDefault();
        openNewSessionWindow().catch(() => {});
        return;
      }
      if (e.key !== "Escape" || e.defaultPrevented) return;
      const active = document.activeElement;
      if (active instanceof HTMLElement && (active.tagName === "INPUT" || active.tagName === "TEXTAREA")) return;
      // 消费即 preventDefault：偷看展开态的「Esc=收回看板」（App）据此让位——
      // 同一按键不能既关 toast 又把看板收走。
      if (focusNotice) {
        e.preventDefault();
        setFocusNotice(null);
        return;
      }
      if (archivedToasts.length > 0) {
        // 清最新一条(与视觉栈顶一致),再按 Esc 逐条收。
        e.preventDefault();
        setArchivedToasts((cur) => cur.slice(0, -1));
        return;
      }
      if (searchOpen) {
        e.preventDefault();
        closeSearch();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // closeSearch 每次渲染新建但内部只用稳定 setter；按状态位重挂即可。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [focusNotice, archivedToasts.length, searchOpen]);
  const startNote = (l: Item) => {
    // 外库卡的便签只读:写入走本库 cc_session_id 查行,对外库会话必然落空。
    if (l.foreign) return;
    setEditingId(null);
    editDraftsRef.current.delete(`n:${l.session.id}`); // 与 startRename 同理，新一轮从原文开始
    setNotingId(l.session.id);
  };
  // 激活便签块（点击/Enter/Space）：长便签被 3 行 clamp 时先展开全文，再次激活才进编辑；
  // 未被 clamp 的短便签一次激活直接进编辑（scrollHeight 读不到的环境按未 clamp 处理）。
  const activateNote = (el: HTMLElement, l: Item) => {
    const txt = el.querySelector(".stk-note-txt");
    const clamped = txt ? txt.scrollHeight > txt.clientHeight + 1 : false;
    if (clamped && expandedNoteId !== l.session.id) {
      setExpandedNoteId(l.session.id);
      return;
    }
    startNote(l);
  };
  const submitNote = (l: Item, noteDraft: string) => {
    editDraftsRef.current.delete(`n:${l.session.id}`);
    if (noteDraft !== (l.note ?? "")) {
      // 乐观：立即按新便签渲染；失败回滚 + focusNotice 提示（与重命名同通道）。
      patchOptimistic(l.session.id, { note: noteDraft });
      setSessionNote(l.session.cc_session_id, noteDraft)
        .catch(() => {
          dropOptimistic(l.session.id, "note");
          setFocusNotice({ kind: "failed", item: l, detail: t.sticker.noteFailed });
        });
    }
    markEditClosed();
    setNotingId(null);
  };

  // 打开终端：连接中→跳转 WT 标签页；已断开未归档→新建终端 resume；归档不开。
  // 外库(安装版)会话只读观察,永不可开——openTerminal 里还有一道守卫兜漏网的调用方。
  const buttonMode = openMode === "button";
  const canOpen = (l: Item) => !l.foreign && (l.connected || !l.archived);
  // 跳转/恢复的在飞集合：一次点击到后端返回可能长达数秒（跳转要做 UIA 枚举，恢复链路含
  // 目录扫描与 1s 秒退探测），期间界面毫无变化时用户必然连点——同一会话的重复请求当场拒绝。
  // state 驱动卡片 pending 样式（.is-opening），ref 供同步判定（state 落地前的同 tick 重入）。
  const [openBusyIds, setOpenBusyIds] = useState<Set<number>>(new Set());
  const openBusyRef = useRef<Set<number>>(new Set());
  const beginOpen = (id: number): boolean => {
    if (openBusyRef.current.has(id)) return false;
    openBusyRef.current.add(id);
    setOpenBusyIds(new Set(openBusyRef.current));
    return true;
  };
  const endOpen = (id: number) => {
    openBusyRef.current.delete(id);
    setOpenBusyIds(new Set(openBusyRef.current));
  };
  // 跳转/恢复成功后的落点文案（T-6 正反馈）：与后端 prefers_external_terminal 同口径——
  // terminal 偏好或轻量模式（对话关闭）时托管会话开在外部终端，否则开对话窗口。
  // managed 由调用方给：跳转按会话当前持有方（非托管=用户自己的终端，永远跳外部终端）；
  // 恢复传 true——恢复回来的会话必归托管 PTY（外部偏好时直接落外部终端，由设置判定覆盖）。
  const openTargetText = (managed: boolean) =>
    managed && sessionOpenIn !== "terminal" && chatEnabled
      ? t.sticker.openTargetChat
      : t.sticker.openTargetTerminal;
  const openTerminal = (l: Item) => {
    // 外库(安装版)会话:本实例只有观察权——focus 跳不到安装版 GUI 内部的 PTY 视图,
    // resume 更危险(cc_session_id 是全局 UUID,恢复会真的 spawn 新进程把会话抢过来)。
    // 只说明它归谁管、去哪操作。
    if (l.foreign) {
      setFocusNotice({ kind: "foreign", item: l });
      return;
    }
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
      if (!beginOpen(l.session.id)) return;
      invoke<FocusSessionResult>("focus_session", {
          pid: l.pid,
          title: l.task_title,
          cwd: l.cwd,
          sessionId: l.session.cc_session_id,
          provider: l.provider,
        })
        .then((result) => {
          // demo/旧后端可能不返回值；保持原行为，不误弹失败提示。
          if (!result) { setFocusNotice(null); return; }
          // focused 也要给正反馈：目标窗口在别的虚拟桌面时，聚焦「成功」了用户却什么
          // 都看不到——没有这句 toast 就是「点了没反应」，必然引来连点。
          if (result === "focused") setFocusNotice({ kind: "opened", item: l, detail: t.sticker.openedIn(openTargetText(l.pty_managed)) });
          else setFocusNotice({ kind: result, item: l });
        })
        .catch((err) => setFocusNotice({ kind: "failed", item: l, detail: formatBackendError(err, t.locale) }))
        .finally(() => endOpen(l.session.id));
    } else if (!l.archived) {
      // 恢复现在可能因多种原因失败（恢复计划无效、PTY 起不来、attach 打不开外部终端）。
      // 静默吞掉的话，用户点了卡片只会看到「什么都没发生」，尤其在把打开方式设成外部终端后，
      // 会直接被理解成设置不生效。走与 focus 相同的提示通道。
      if (!beginOpen(l.session.id)) return;
      // 恢复链路串行阻塞 1~3s（目录扫描、spawn、1s 秒退探测），卡片置灰之外再给一句
      // 文字——「点了没反应」是重复点击的最大来源。成功后换成 opened 正反馈 toast
      //（board-changed 会把卡片翻绿，toast 4s 自收）。
      setFocusNotice({ kind: "resuming", item: l });
      invoke<boolean>("resume_session", { cwd: l.cwd, sessionId: l.session.cc_session_id, provider: l.provider })
        // started=false = 后端判重命中（会话已在跑）：没再起一份，只聚焦了已有视图，
        // 文案如实说「已在运行」而不是「已恢复」。
        .then((started) => setFocusNotice(started === false
          ? { kind: "opened", item: l, detail: t.sticker.resumeAlreadyRunning }
          : { kind: "opened", item: l, detail: t.sticker.openedIn(openTargetText(true)) }))
        .catch((err) => setFocusNotice({ kind: "failed", item: l, detail: formatBackendError(err, t.locale) }))
        .finally(() => endOpen(l.session.id));
    }
  };

  const reopenNoticeSession = async () => {
    if (!focusNotice || focusNotice.busy) return;
    const l = focusNotice.item;
    if (focusNotice.kind === "process_ended") {
      setFocusNotice({ ...focusNotice, busy: true });
      invoke<boolean>("resume_session", { cwd: l.cwd, sessionId: l.session.cc_session_id, provider: l.provider })
        .then((started) => setFocusNotice(started === false
          ? { kind: "opened", item: l, detail: t.sticker.resumeAlreadyRunning }
          : { kind: "opened", item: l, detail: t.sticker.openedIn(openTargetText(true)) }))
        .catch((err) => setFocusNotice({ ...focusNotice, busy: false, kind: "failed", detail: formatBackendError(err, t.locale) }));
      return;
    }
    const pid = l.pid;
    if (!pid) {
      // notice 持有点击时的快照，正常不会走到这里；仍在命令边界防御可空类型，绝不把 null 交给后端。
      setFocusNotice({ ...focusNotice, kind: "process_ended" });
      return;
    }
    // 结束并重开会杀掉原进程,属破坏性操作:走全应用统一的 appConfirm 原生小窗
    // (toast 内二次点击的旧确认范式已移除)。保留 danger；主按钮说后果（S-14）。
    const yes = await appConfirm(t.sticker.reopenConfirm, { title: t.sticker.reopen, danger: true, confirmLabel: t.sticker.reopenConfirmLabel });
    if (!yes) return;
    setFocusNotice({ ...focusNotice, busy: true });
    invoke("restart_session_supported", {
      pid,
      cwd: l.cwd,
      sessionId: l.session.cc_session_id,
      provider: l.provider,
    })
      .then(() => setFocusNotice(null))
      .catch((err) => setFocusNotice({ ...focusNotice, busy: false, kind: "failed", detail: formatBackendError(err, t.locale) }));
  };

  // 撤销归档:把会话捞回来。成功后必须让父层整窗重查（P1-1）：被撤回的会话
  // last_event_at 没变、排不进首页，归档时又被乐观摘掉不在 prev 尾部——
  // board-changed 的「首页权威 + 按 id 保留」增量刷新两条路都捞不回它。
  // toast 状态声明在窗口级快捷键 effect 之前（见 archivedToasts）。
  const undoArchiveToast = (key: number, item: Item) => {
    setArchivedToasts((cur) => cur.filter((toast) => toast.key !== key));
    setArchived(item.session.id, false)
      .then(() => onArchiveUndo?.())
      .catch(() => setFocusNotice({ kind: "failed", item, detail: t.sticker.archiveFailed }));
  };

  const focusNoticeText = focusNotice
    ? focusNotice.detail || ({
          connecting: t.sticker.focusConnecting,
          host_focused: t.sticker.focusHostOnly,
          alive_but_not_found: t.sticker.focusNotFound,
          permission_denied: t.sticker.focusPermission,
          unsupported_terminal: t.sticker.focusUnsupported,
          process_ended: t.sticker.focusEnded,
          failed: t.sticker.focusFailed,
          background: t.sticker.focusBackground,
          foreign: t.sticker.focusForeign,
          archived: t.sticker.archivedNotice,
          resuming: t.sticker.resuming,
          // 成功类文案总是带 detail（落点/已在运行），map 值只是兜底。
          opened: "",
          focused: "",
        }[focusNotice.kind])
    : "";

  // 先按当前 tab 过滤，再排序：置顶恒在最前。match(tab) 是安全网（后端已按 tab/search 过滤，
  // 这里兜底防御性重过滤一遍）；搜索过滤已下沉后端（父组件按 search 请求当前 tab 内全库），
  // 本组件不再做客户端搜索过滤。waiting「等最久优先」由后端 ASC 排序保证，客户端只做 starred 浮顶。
  // useMemo 缓存：编辑便签/重命名时每次按键都会重渲染，不必每次重跑 filter+sort。
  const isStarred = (l: Item) => starred.has(l.session.cc_session_id);
  const shownLive = useMemo(() => {
    return data
      .filter((l) => match(tab, l))
      .sort(
        (a, b) =>
          Number(starred.has(b.session.cc_session_id)) -
          Number(starred.has(a.session.cc_session_id))
      );
  }, [data, tab, starred]);
  // B-4 切换期（切 tab/搜索的首页请求在途）：保留旧列表灰化直到新数据就绪。此前中间态
  // 会把旧 data 按新 tab 过滤出一个子集先渲染（waiting 的 ASC 到达后整列表翻转），
  // 观感像列表坏了。冻结的是切换前最后一次稳定 shown；新数据落地（switching=false）
  // 当帧直接换整表，只跳变一次。effect 写 ref：切换那一帧 ref 里还是旧列表。
  const frozenShownRef = useRef(shownLive);
  useEffect(() => {
    if (!switching) frozenShownRef.current = shownLive;
  }, [shownLive, switching]);
  const shown = switching ? frozenShownRef.current : shownLive;

  // 7B-3：「全部」tab 里在线会话与历史会话之间此前没有任何分界，20+ 张卡滚到中段
  // 分不清「这一片还是活的吗」。connected-first 是全局不变量（App 的分区合并），
  // 但星标浮顶会把断开的星标卡拎到最前——所以起点取**最后一张 connected 之后**，
  // 而不是第一张断开的卡。其余 tab（运行中/待交互）本身就是同质列表，不插。
  const historyStart = useMemo(() => {
    if (tab !== "all" || q.trim()) return -1;
    let last = -1;
    for (let i = 0; i < shown.length; i += 1) if (shown[i].connected) last = i;
    return last >= 0 && last + 1 < shown.length ? last + 1 : -1;
  }, [shown, tab, q]);

  // 贴纸会话虚拟列表：只挂载可视区 + overscan 内的卡片，避免大量 DOM。
  // estimateSize 分档：普通卡约 76–84px；带便签的卡多一块 3 行 clamp 的便签块（约 +60px），
  // 统一估 82 与实测差近一倍是滚动抽搐的来源之一。实际高度由 measureElement 动态测量。
  const virtualizer = useVirtualizer({
    count: shown.length,
    getScrollElement: () => scrollRef.current,
    // key 必须是会话 id：默认按 index 复用测量高度与 DOM。列表常重排（活跃会话跳到最前、
    // 星标浮顶），按 index 会把带便签的高卡尺寸套到滑进该位置的普通卡上（高度抽搐），
    // 焦点/hover 留在「位置」而不是会话上，用户瞄准的卡在点下的瞬间可能已换成另一条会话。
    getItemKey: (i) => shown[i].session.id,
    estimateSize: (i) => (shown[i].note ? 144 : 82),
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

  // 切 tab / 搜索词变化：滚动回顶。旧 scrollTop 留着会让新列表直接落在中段
  // （或被夹到底部），观感像切换失败。用 key 比对避免其它重渲染误触发。
  // 例外（P2-6）：同 tab 清空搜索时 App 走缓存恢复搜索前的列表（unsearchedItemsRef），
  // 滚动位置随列表一起恢复而不是回顶——否则「清一下搜索」就把用户已浏览的窗口没收了。
  const preSearchScrollRef = useRef<{ tab: Tab; offset: number } | null>(null);
  const viewKeyRef = useRef(`${tab}\0${q.trim()}`);
  useEffect(() => {
    const key = `${tab}\0${q.trim()}`;
    const prevKey = viewKeyRef.current;
    if (prevKey === key) return;
    viewKeyRef.current = key;
    const sep = prevKey.indexOf("\0");
    const sameTab = prevKey.slice(0, sep) === tab;
    const prevQ = prevKey.slice(sep + 1);
    const query = q.trim();
    // 进入搜索：记下当前滚动位置，供清空搜索时恢复。
    if (sameTab && !prevQ && query) {
      preSearchScrollRef.current = { tab, offset: scrollRef.current?.scrollTop ?? 0 };
    }
    // 同 tab 清空搜索：恢复进入搜索前的滚动位置（此刻列表已是 App 缓存恢复的原列表）。
    if (sameTab && prevQ && !query && preSearchScrollRef.current?.tab === tab) {
      const offset = preSearchScrollRef.current.offset;
      preSearchScrollRef.current = null;
      virtualizer.scrollToOffset(offset);
      return;
    }
    if (!sameTab) preSearchScrollRef.current = null; // 换 tab 后旧记录失效
    virtualizer.scrollToOffset(0);
  }, [tab, q, virtualizer]);

  // 无限滚动触发：当可视区底部进入「已加载列表末尾前 10 条」且仍有数据时，通知父组件加载下一页。
  useEffect(() => {
    if (isLoadingMore || !hasMore || data.length === 0) return;
    // 客户端安全网可能把整页过滤成空（如 waiting tab：后端谓词含 DB 残留 pending_review 的
    // 断开会话，前端 match 再按 connected 丢弃）——此时没有可视行、按可视行判定的翻页永不
    // 触发，用户看到空态而下一页明明有。shown 为空但仍有下一页时主动拉（守卫看 data 不看 shown）。
    if (shown.length === 0) {
      onLoadMore();
      return;
    }
    const last = virtualItems[virtualItems.length - 1];
    if (last && last.index >= shown.length - 10) {
      onLoadMore();
    }
  }, [virtualItems, shown.length, data.length, isLoadingMore, hasMore, onLoadMore]);

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

  // tab 滑块按选中按钮实测定位：等分假设（1/3 宽 + translateX(index*100%)）在 tab 文字
  // 宽度不等或 tab 数变化时都会错位，且 CSS 里的 1/3 要靠注释与 TAB_KEYS 手工同步。
  // 计数变化（99+ 徽章出现/消失）会改按钮宽度，故依赖 counts 一并重测；窗口缩放由
  // ResizeObserver 兜住（jsdom 无该构造时静默跳过，测量值恒 0 无意义）。
  useLayoutEffect(() => {
    const seg = tabsegRef.current;
    if (!seg) return;
    const measure = () => {
      const btn = seg.querySelectorAll<HTMLElement>('[role="tab"]')[TAB_KEYS.indexOf(tab)];
      if (!btn) return;
      setSliderStyle((prev) =>
        prev?.left === btn.offsetLeft && prev?.width === btn.offsetWidth
          ? prev
          : { left: btn.offsetLeft, width: btn.offsetWidth });
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(seg);
    return () => observer.disconnect();
  }, [tab, counts]);

  // 自绘 overlay 滚动条：原生滚动条全程隐藏(不占布局→无抖动)，这里按滚动位置算出
  // thumb 的高度/位置，浮在内容右侧。null = 内容未超出、不需要滚动条。
  const scrollRef = useRef<HTMLDivElement>(null);
  const scrollInnerRef = useRef<HTMLDivElement>(null);
  const [sb, setSb] = useState<{ top: number; height: number } | null>(null);
  const [sbDrag, setSbDrag] = useState(false);
  // 滚动边缘淡出：仅当该方向确有被遮内容时才淡(滚到顶/底则对应边不淡，首/末卡保持清晰)。
  const [edge, setEdge] = useState({ top: false, bottom: false });

  // 星标重排落地后做 FLIP 的 Invert+Play（捕获在 toggleStarFlip，见上）。补偿打在卡片本体上
  // ——vitem 的 transform 归虚拟列表管，不能覆盖。只补偿有旧位置的可见卡：被虚拟化卸载、
  // 重排后才进可视区的卡没有参照，直接出现。减少动画偏好下跳过（与 cstrip-eyes 同一约定）。
  useLayoutEffect(() => {
    const prev = flipPrevRef.current;
    flipPrevRef.current = null;
    if (!prev || prev.size === 0) return;
    if (typeof window.matchMedia === "function" && window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    const inner = scrollInnerRef.current;
    if (!inner) return;
    const timers: number[] = [];
    inner.querySelectorAll<HTMLElement>("[data-sid]").forEach((el) => {
      const oldTop = prev.get(Number(el.dataset.sid));
      if (oldTop === undefined) return;
      const delta = oldTop - el.getBoundingClientRect().top;
      if (Math.abs(delta) < 2) return; // 亚像素级抖动不做动画
      const card = el.firstElementChild as HTMLElement | null;
      if (!card) return;
      card.style.transition = "none";
      card.style.transform = `translateY(${delta}px)`;
      void card.offsetHeight; // 强制 reflow：让「无过渡的旧位置」先生效，再开过渡回新位置
      card.style.transition = "transform 180ms ease";
      card.style.transform = "";
      // 过渡结束后清掉内联 transition，不留痕迹影响后续 hover/主题过渡。
      timers.push(window.setTimeout(() => { card.style.transition = ""; }, 200));
    });
    return () => timers.forEach((id) => window.clearTimeout(id));
  }, [shown]);

  // 底部用量：多 provider。先从 getAccounts() 拿缓存快速预填(仅在还没有联网值时填充，
  // 避免缓存晚到覆盖更新的联网值)，再对有账号且 usage_supported 的 provider 定时刷新（5 min）。
  // usageMap 只存原始 ProviderUsage，label 在渲染时取当前语言，切换即时生效。
  const [usageMap, setUsageMap] = useState<Record<string, ProviderUsage>>({});
  // 各 provider 上次成功刷新的时刻 + 最近一轮是否失败（7B-8 的陈旧标记）。
  const [usageMeta, setUsageMeta] = useState<Record<string, { at: number; stale: boolean }>>({});
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
        // 7B-8 补齐（复核指出）：这里预填的是**账号快照里的**用量，它本身就可能是几分钟
        // 前的。不给它写 meta 的话，首轮刷新失败时 usageMeta 空 → 陈旧标记不生效，
        // 屏上是一份不知多久以前的读数却显示得像新的。按「拿到的时刻」记，且标为待刷新。
        setUsageMeta((cur) => {
          const next = { ...cur };
          ps.forEach((p) => {
            if (p.relay_enabled) delete next[p.provider];
            else if (!next[p.provider] && p.usage) next[p.provider] = { at: Date.now(), stale: false };
          });
          return next;
        });
        // 对有账号且支持用量的 provider：立即刷新 + 定时刷新
        ps.filter((p) => p.account != null && p.usage_supported && !p.relay_enabled).forEach(({ provider }) => {
          const doRefresh = () => {
            if (cancelled) return;
            refreshUsage(provider)
              .then((u) => {
                if (cancelled) return;
                setUsageMap((m) => ({ ...m, [provider]: u }));
                setUsageMeta((m) => ({ ...m, [provider]: { at: Date.now(), stale: false } }));
              })
              // USAGE_UNSUPPORTED / 网络错误：保持无用量，不显示该行。但**已经显示过**的
              // 读数会原样留在屏上（5 分钟一轮，失败即定格），标记 stale 让它降饱和（7B-8）。
              .catch(() => {
                if (cancelled) return;
                setUsageMeta((m) => (m[provider] ? { ...m, [provider]: { ...m[provider], stale: true } } : m));
              });
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
  // 更新红点的提示按阶段分（7S-3）：available=有新版本、downloading=正在下载、ready=可安装。
  const updateLabel = updateStatus === "downloading" ? t.sticker.updateDownloading
    : updateStatus === "ready" ? t.sticker.updateReady
    : t.sticker.updateAvailable;

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
      <DevBadge />
      {!isMacPanel() && <div className="drag" data-tauri-drag-region />}
      <div className="tabs">
        <div
          className="tabseg"
          role="tablist"
          ref={tabsegRef}
          // ARIA tabs 惯例:左右方向键切换并跟随焦点(roving),不能只有鼠标一条路；
          // Home/End 跳到首/末 tab（B-12）。
          onKeyDown={(e) => {
            let next: Tab | null = null;
            if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
              const at = TAB_KEYS.indexOf(tab);
              next = TAB_KEYS[(at + (e.key === "ArrowRight" ? 1 : TAB_KEYS.length - 1)) % TAB_KEYS.length];
            } else if (e.key === "Home") next = TAB_KEYS[0];
            else if (e.key === "End") next = TAB_KEYS[TAB_KEYS.length - 1];
            if (!next) return;
            e.preventDefault();
            pick(next);
            tabsegRef.current?.querySelectorAll<HTMLButtonElement>('[role="tab"]')[TAB_KEYS.indexOf(next)]?.focus();
          }}
        >
          {/* 选中态立体滑块：切换时平滑滑到目标 tab。位置/宽度按选中按钮**实测**（见
              slider effect）——曾用 translateX(index*100%) + CSS 硬编码 1/3 宽，tab 数
              与 CSS 靠注释同步，且各 tab 文字宽度不等时滑块与视觉中心不重合。 */}
          <span className="tabseg-slider" style={sliderStyle} />
          {TAB_KEYS.map((k) => {
            const n = counts[k];
            return (
              <button
                key={k}
                type="button"
                role="tab"
                id={`stk-tab-${k}`}
                aria-selected={tab === k}
                aria-controls="stk-tabpanel"
                // roving tabindex：三个 tab 只占一个 Tab 停靠点，方向键在 tab 间搬焦点。
                tabIndex={tab === k ? 0 : -1}
                className={"stab " + (tab === k ? "stab-on" : "")}
                onClick={() => {
                  // 点击当前已选中的 tab：回顶（P2-9）。此前是静默空操作——「点了没反应」。
                  // 键盘路径（上方 Home/End/方向键）不走这里：roving 焦点与选中态可能不同步，
                  // pick 必须照常上报（B-12 回归测试钉住了这一点）。
                  if (k === tab) {
                    virtualizer.scrollToOffset(0);
                    return;
                  }
                  pick(k);
                }}
              >
                {/* 文字包一层 span：flex 容器的裸文本节点吃不到 text-overflow，
                    窄窗 + 英文长词时由 .stab-label 省略号截断（G-12）。 */}
                <span className="stab-label">{t.tabs[k]}</span>
                {k !== "all" && <span className="stab-n">{n > 99 ? "99+" : n}</span>}
              </button>
            );
          })}
        </div>
      </div>
      {/* 7B-2：刷新失败但手里还有缓存列表。此前 loadError 只在**空列表**分支上屏，非空时
          board-changed 刷新失败零提示——状态环、时间全是旧值，看不出已经不新鲜了。
          （U1-13 给了对话窗细横幅，看板这半边没跟上。） */}
      {loadError && shown.length > 0 && (
        <div className="stk-stale" role="status">
          <span>{t.sticker.staleNotice}</span>
          <button type="button" onClick={() => onRetry?.()}>{t.sticker.retry}</button>
        </div>
      )}
      <div
        className={"stk-scroll" + (switching ? " is-switching" : "") + (edge.top ? " fade-top" : "") + (edge.bottom ? " fade-bottom" : "")}
        ref={scrollRef}
        onScroll={syncSb}
        aria-busy={switching || undefined}
        // 卡片列表即三个 tab 共用的 tabpanel（B-12 的 ARIA 接线）。
        role="tabpanel"
        id="stk-tabpanel"
        aria-labelledby={`stk-tab-${tab}`}
      >
        {shown.length === 0 ? (
          initialLoading ? (
            // 冷启动首次加载：先给加载占位，不闪「还没有会话」假空态。
            <div className="stk-loading">{t.sticker.loading}</div>
          ) : switching ? (
            // 7B-1：切 tab 的首页请求在途。B-4 的冻结只保住**非空**旧列表（frozenShownRef
            // 为空时这里照样落到 EmptyState）——从空的「运行中」切「全部」，请求还没回来
            // 就先闪一屏「还没有会话 + 新建」，等真数据到了又整屏换掉。在途一律给加载占位。
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
          <>
            <div
              ref={scrollInnerRef}
              style={{ height: totalSize, width: "100%", position: "relative" }}
            >
            {virtualItems.map((virtualItem) => {
              const l = shown[virtualItem.index];
              const agentIcon = agentAssets(l.provider); // 品牌图标（视觉资产，按 id 查表）
              const agentLabel = agentNameOf(l.provider); // 展示名（后端下发；未知 id 显示 id 本身）
              // 「正在启动」占位卡（pending PTY，负 id，后端 session_query 合成）：纯展示——
              // 会话行还没落库，点击/右键/菜单/编辑全是死路，一概不挂。真实会话认领出卡后
              // 由父层对账撤下（App 的 pendingsRef，见 sticker/pending.ts）。
              if (l.session.id < 0) {
                return (
                  <div
                    key={virtualItem.key}
                    data-index={virtualItem.index}
                    data-sid={l.session.id}
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
                    <div className="stk-card is-starting" data-testid="stk-starting-card">
                      <div className="stk-top">
                        <span className="stk-ind">
                          <span className="sdot sdot-on stk-starting-dot" role="img" aria-label={t.sticker.startingTip} data-tip={t.sticker.startingTip} />
                        </span>
                        <div className="stk-top-body">
                          <div className="stk-line1">
                            <span className="stk-title">{t.sticker.startingCard}</span>
                          </div>
                          <div className="stk-line2">
                            <span
                              className="stk-agent"
                              style={tintStyle(l.provider, true)}
                              data-tip={agentLabel}
                              role="img"
                              aria-label={agentLabel}
                            >
                              <agentIcon.Icon />
                            </span>
                            {l.cwd && (
                              <span className="stk-repo" data-tip={l.cwd}>
                                {folderName(l.cwd)}
                              </span>
                            )}
                          </div>
                        </div>
                      </div>
                    </div>
                  </div>
                );
              }
              // 乐观覆盖层优先：重命名/便签刚提交、board-changed 还没带回时按新值渲染。
              const ov = optimistic.get(l.session.id);
              const effTaskTitle = ov?.title ?? l.task_title;
              const effNote = ov?.note !== undefined ? (ov.note || null) : l.note;
              const unnamed = !effTaskTitle || effTaskTitle === UNNAMED_SESSION_SENTINEL;
              const title = unnamed ? t.sticker.waitingFirstInput : effTaskTitle;
              // AI 活动行显示「最近一条 AI 正文」(last_ai_text，回退 transcript preview)；出错优先显示错误标签；
              // previewEnabled（对话预览开关）关闭则不显示 AI 正文（仅保留错误）；用户行同受该开关门控。
              const sub = l.errored && l.error_label
                ? t.errorLabels[l.error_label] ?? l.error_label
                : previewEnabled && (l.last_ai_text ?? l.preview)
                ? (l.last_ai_text ?? l.preview)
                : null;
              const subTitle = l.errored ? l.error_raw ?? undefined : sub ?? undefined;
              // 状态判定统一走 cardTone、置信度统一走 toneConfidence（与折叠缩略条同一口径，
              // 优先级与弱化语义说明见 helpers.ts）。
              const tone = cardTone(l);
              const confidence = toneConfidence(l, tone);
              const assumedBadge = confidence === "assumed";
              const fallbackIdle = confidence === "fallback";
              // assumed 分支的弱化文案（P2-11）：screen_state==null 的回落判定是「按记录
              // 推断」，悬停/读屏不能再读到内层 RunBadge 确定的「运行中/等待输入」。
              const assumedWhat = tone === "waiting" ? t.badge.waiting : t.badge.running;
              const assumedLabel = t.sticker.badgeAssumed(
                l.context_pct != null ? t.badge.full(assumedWhat, l.context_pct) : assumedWhat
              );
              const indicator = tone === "offline" ? (
                <span className="ring-stop" role="img" aria-label={t.sticker.stopped} data-tip={t.sticker.stopped} />
              ) : tone === "error" ? (
                <span className="needs-error" role="img" aria-label={l.error_raw ?? t.sticker.sessionError} data-tip={l.error_raw ?? t.sticker.sessionError} />
              ) : tone === "pending" ? (
                <RunBadge pct={l.context_pct} tone="pending" />
              ) : fallbackIdle ? (
                <span className="sdot sdot-off" role="img" aria-label={t.sticker.assumedState} data-tip={t.sticker.assumedState} />
              ) : tone === "running" ? (
                assumedBadge ? (
                  <span className="stk-badge-assumed" role="img" aria-label={assumedLabel} data-tip={assumedLabel}>
                    {/* 内层 aria-hidden + pointer-events:none（CSS）：悬停/读屏只见包层的
                        弱化文案，内层 RunBadge 的「自信」tip/aria 被压制（P2-11）。 */}
                    <span className="stk-badge-assumed-inner" aria-hidden="true"><RunBadge pct={l.context_pct} /></span>
                  </span>
                ) : (
                  <RunBadge pct={l.context_pct} />
                )
              ) : tone === "waiting" ? (
                assumedBadge ? (
                  <span className="stk-badge-assumed" role="img" aria-label={assumedLabel} data-tip={assumedLabel}>
                    <span className="stk-badge-assumed-inner" aria-hidden="true"><RunBadge pct={l.context_pct} tone="waiting" /></span>
                  </span>
                ) : (
                  <RunBadge pct={l.context_pct} tone="waiting" />
                )
              ) : (
                <span className="sdot sdot-on" role="img" aria-label={t.sticker.online} data-tip={t.sticker.online} />
              );
              return (
                <div
                  key={virtualItem.key}
                  data-index={virtualItem.index}
                  data-sid={l.session.id}
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
                  {/* 历史会话分界（7B-3）：挂在分界后第一张卡的容器里，高度随
                      measureElement 一并测到，不必给虚拟列表新增一种条目类型。 */}
                  {virtualItem.index === historyStart && (
                    <div className="stk-history-sep">{t.sticker.historyDivider(shown.length - historyStart)}</div>
                  )}
                  <div
                    className={"stk-card" + (isStarred(l) ? " is-star" : "") + (openBusyIds.has(l.session.id) ? " is-opening" : "")}
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
                      // 失焦提交刚关闭编辑器的同一次点击:只当失焦,不导航。
                      if (Date.now() - editCloseAtRef.current < 250) return;
                      // 按钮模式：点击卡片不开终端，改由卡片上的打开按钮触发。
                      if (buttonMode) return;
                      openTerminal(l);
                    }}
                    onKeyDown={(e) => {
                      // 卡内 roving（B-11）：整卡只占一个 Tab 停靠点，←/→ 在卡内控件
                      // （对话/菜单/便签/打开）间搬 DOM 焦点；编辑输入框里的左右键是光标
                      // 移动，不抢。
                      if (e.key === "ArrowRight" || e.key === "ArrowLeft") {
                        const active = document.activeElement as HTMLElement | null;
                        if (active && (active.tagName === "INPUT" || active.tagName === "TEXTAREA")) return;
                        const items = Array.from(
                          e.currentTarget.querySelectorAll<HTMLElement>('button, [role="button"]'),
                        );
                        if (items.length === 0) return;
                        e.preventDefault();
                        const at = active ? items.indexOf(active) : -1;
                        const next = at < 0
                          ? items[e.key === "ArrowRight" ? 0 : items.length - 1]
                          : items[(at + (e.key === "ArrowRight" ? 1 : items.length - 1)) % items.length];
                        next.focus();
                        return;
                      }
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
                      // 7B-4：按钮模式此前在这里直接 return——卡片仍是可聚焦的 role=button，
                      // 键盘用户 Tab 到它、按 Enter 却什么都不发生（B-11 保证了「能到达」，
                      // 没覆盖「到达之后主键有反应」）。按钮模式防的是鼠标误点，键盘上的
                      // Enter 是明确动作，照常打开。
                      openTerminal(l);
                    }}
                    onContextMenu={(e) => {
                      // 右键恒开菜单:卡片上右键出上下文菜单是通用惯例,不随设置失效——
                      // card_menu_mode 只决定「⋯」按钮是否常驻,不再关掉右键这条路(与侧栏同款)。
                      // 外库卡不弹菜单:置顶/便签/重命名/归档全是本库操作,对它一律无效。
                      e.preventDefault();
                      if (!l.foreign) {
                        setCtxMenu({ sid: l.session.id, x: e.clientX, y: e.clientY });
                      }
                    }}
                    style={{ cursor: !buttonMode && canOpen(l) ? "pointer" : "default" }}
                    // 断开卡的单击不是「查看」而是拉起 CLI 进程(1~3s):后果与普遍预期不对称,
                    // tip 必须把它说出来,让「误点」发生在点击之前而不是之后。
                    // 7B-5：外库/后台卡点下去只会弹一张「本窗口只读」的说明，tip 却写着
                    // 「打开会话」——承诺与结果对不上。这两类先于 connected 判定。
                    data-tip={buttonMode ? ""
                      : l.foreign ? t.sticker.focusForeign
                      : l.background ? t.sticker.focusBackground
                      : l.connected ? t.sticker.openSession
                      : l.archived ? "" : t.sticker.cardResumeTip}
                  >
                    <div className="stk-top">
                      <span className="stk-ind">{indicator}</span>
                      <div className="stk-top-body">
                        <div className="stk-line1">
                          {editingId === l.session.id ? (
                            <EditBox
                              initial={editDraftsRef.current.get(`r:${l.session.id}`) ?? (unnamed ? "" : effTaskTitle)}
                              placeholder={t.sticker.renamePlaceholder}
                              inputClass="stk-edit"
                              saveLabel={t.sticker.noteSave}
                              cancelLabel={t.sticker.noteCancel}
                              onDraft={(v) => editDraftsRef.current.set(`r:${l.session.id}`, v)}
                              onSubmit={(value) => submitRename(l, value)}
                              onCancel={() => {
                                editDraftsRef.current.delete(`r:${l.session.id}`);
                                setEditingId(null);
                              }}
                            />
                          ) : (
                            <>
                              {/* 长标题被省略号截断后必须能看全：标题是卡片第一识别信息，
                                  内层 data-tip 优先于卡片级的「打开会话」提示（Tooltip 取
                                  closest 匹配）。占位标题（等待首次输入）无截断意义，不挂。 */}
                              <span className="stk-title" data-tip={unnamed ? undefined : title}>{title}</span>
                              {/* 断开的会话不挂交互标签：进程都没了，「待批准」只会催用户去点一个
                                  点不动的东西。DB 里的 pending_review 是收尾时没清干净的残留。
                                  屏幕检测到的 blocked（hook 盲区的审批/提问 UI）也给 pill——
                                  此前只有琥珀环没有文字，与 waiting 黄仅差色相，读不出「需要操作」。 */}
                              {l.connected && (l.pending_review || l.screen_state === "blocked") && (
                                <span className={"pending-pill pending-" + (l.pending_review ?? "blocked")}>
                                  {l.pending_review ? t.pending[l.pending_review] : t.pending.blocked}
                                </span>
                              )}
                              <span className={"stk-time" + (tab === "waiting" ? " is-waited" : "")}>
                                {tab === "waiting" ? fmtWaited(l.session.last_event_at, t) : fmtAgo(l.session.last_event_at, t)}
                              </span>
                              {/* 对话功能关闭（轻量模式）时不渲染：入口不存在优于点了被拦。
                                  外库卡同理:对话窗按本库 id 加载历史,外库 id 必落空。 */}
                              {chatEnabled && !l.foreign && (
                                <button
                                  type="button"
                                  className="stk-chat-btn"
                                  aria-label={t.sticker.openChat}
                                  data-tip={t.sticker.openChat}
                                  // 卡内 roving：不进 Tab 序，由卡片的 ←/→ 到达（B-11）。
                                  tabIndex={-1}
                                  onClick={(e) => {
                                    e.stopPropagation();
                                    invoke("open_chat_window", { sessionId: l.session.id }).catch(() => {});
                                  }}
                                ><ChatIcon /></button>
                              )}
                              {/* 置顶/便签/重命名/归档操作收进卡片菜单（CardContextMenu），标题行不再挤 hover 图标。
                                  默认右键触发；card_menu_mode=button（触屏等不便右键）时改为此处的常显菜单按钮。
                                  context 模式下按钮仍在但平时透明、hover 卡片才极淡浮现（stk-menu-btn-ghost）——
                                  否则该模式下星标/便签/重命名/归档零可见入口、零提示（B-5）。
                                  置顶态由卡片金角、便签由便签块表达，收起入口不丢信息。 */}
                              {!l.foreign && (
                                <button
                                  type="button"
                                  className={"stk-menu-btn" + (menuMode === "button" ? "" : " stk-menu-btn-ghost")}
                                  aria-label={t.sticker.cardMenu}
                                  data-tip={t.sticker.cardMenu}
                                  // 卡内 roving：不进 Tab 序，由卡片的 ←/→ 到达（B-11）。
                                  tabIndex={-1}
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
                              {folderName(l.cwd)}
                            </span>
                          )}
                          {/* 附加目录标:一个会话跨 N 仓(--add-dir),tip 列全部目录。
                              可选链防御:旧后端/测试固件的行没有该字段。 */}
                          {(l.extra_dirs?.length ?? 0) > 0 && (
                            <span
                              className="stk-extradirs"
                              data-testid="stk-extradirs"
                              data-tip={[l.cwd, ...l.extra_dirs].filter(Boolean).join(" · ")}
                            >
                              +{l.extra_dirs.length}
                            </span>
                          )}
                          {/* Agent 从这个会话派生出去的后台作业**不在卡片上留痕**：它们自己
                              不建卡（见后端 hidden_background），源会话卡上也不标数——用户既
                              管不了这些作业（终端按键无效、发消息在手动模式下不会被执行），
                              一个数字除了让人困惑并无用处。要处理请回派出它们的终端。 */}
                          {/* 外库来源徽章:这张卡由安装版托管,本实例只读观察(dev 构建
                              聚合生产库的会话,监控视野不因数据目录隔离变窄)。 */}
                          {l.foreign && (
                            <span className="stk-foreign" data-tip={t.sticker.foreignTip}>
                              {t.sticker.foreignBadge}
                            </span>
                          )}
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
                          {l.model && <span className="stk-model" data-tip={l.model}>{l.model}</span>}
                        </div>
                      </div>
                    </div>
                    {notingId === l.session.id ? (
                      <EditBox
                        initial={editDraftsRef.current.get(`n:${l.session.id}`) ?? (effNote ?? "")}
                        placeholder={t.sticker.notePlaceholder}
                        inputClass="stk-note-edit"
                        extraClass="stk-editbox-note"
                        icon={<span className="stk-editbox-ico"><NoteIcon /></span>}
                        saveLabel={t.sticker.noteSave}
                        cancelLabel={t.sticker.noteCancel}
                        maxLength={NOTE_MAX_CHARS}
                        onDraft={(v) => editDraftsRef.current.set(`n:${l.session.id}`, v)}
                        onSubmit={(value) => submitNote(l, value)}
                        onCancel={() => {
                          editDraftsRef.current.delete(`n:${l.session.id}`);
                          setNotingId(null);
                        }}
                      />
                    ) : effNote ? (
                      <div
                        className={"stk-note" + (expandedNoteId === l.session.id ? " is-open" : "")}
                        data-tip={t.sticker.noteEdit}
                        role="button"
                        // 卡内 roving：不进 Tab 序，由卡片的 ←/→ 到达（B-11）。
                        tabIndex={-1}
                        onClick={(e) => { e.stopPropagation(); activateNote(e.currentTarget, l); }}
                        onKeyDown={(e) => {
                          if (e.target !== e.currentTarget) return;
                          if (e.key !== "Enter" && e.key !== " ") return;
                          e.preventDefault();
                          e.stopPropagation(); // 不冒泡给卡片，避免误开终端
                          activateNote(e.currentTarget, l);
                        }}
                      >
                        <span className="stk-note-icon"><NoteIcon /></span>
                        <span className="stk-note-txt">{effNote}</span>
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
                            // 卡内 roving：不进 Tab 序，由卡片的 ←/→ 到达（B-11）。
                            tabIndex={-1}
                            onClick={(e) => { e.stopPropagation(); openTerminal(l); }}
                          ><OpenIcon /></button>
                        )}
                      </div>
                    )}
                  </div>
                </div>
              );
            })}
            </div>
            {/* 加载更多 loader：内层容器之外的正常流兄弟节点——计入滚动高度（此前绝对定位在
                totalSize 之外，不算滚动高度且落在底部淡出遮罩里，滚到底也看不清）。 */}
            {isLoadingMore && (
              <div className="stk-loadmore">
                <span className="stk-loadmore-dot" />
                <span className="stk-loadmore-dot" />
                <span className="stk-loadmore-dot" />
              </div>
            )}
          </>
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
          onStar={() => toggleStarFlip(ctxItem.session.cc_session_id)}
          onNote={() => startNote(ctxItem)}
          onRename={() => startRename(ctxItem)}
          onArchive={() => {
            const target = !ctxItem.archived;
            const sessionId = ctxItem.session.id;
            const item = ctxItem;
            // 先乐观通知父层摘掉卡片（点完菜单即刻消失，不等 IPC 往返），失败再回滚。
            // 归档与重命名在菜单里上下相邻，误点后卡片瞬间消失、恢复入口又藏在设置深处——
            // 成功后给一条带「撤销」的 toast；失败此前完全静默（卡片消失又弹回来，没有一个
            // 字解释），补上失败提示，与重命名/便签同通道。
            onArchiveOptimistic?.(sessionId);
            setArchived(sessionId, target)
              .then(() => { if (target) pushArchivedToast(item); })
              .catch(() => {
                onArchiveFailed?.();
                setFocusNotice({ kind: "failed", item, detail: t.sticker.archiveFailed });
              });
          }}
          onNewSession={() =>
            openNewSessionWindow({ cwd: ctxItem.cwd, provider: ctxItem.provider }).catch(() => {})
          }
          onOpenDir={
            ctxItem.cwd ? () => openProjectDir(ctxItem.cwd!).catch(() => {}) : null
          }
          // 附加目录入口不上贴纸(用户指定):贴纸是监控视野,操作入口在对话窗。
          onAddDir={null}
          onEndSession={
            // 结束入口按 endable 口径（与对话窗标题栏同源）：本 GUI 托管的 PTY，或进程
            // 仍活着的孤儿会话（broker 已收掉 PTY 记录，后端按 pid 杀树兜底）。确认+停止走
            // 共用的 confirmStopSession。成功后不用做别的：轮询里 connected 翻 false、徽标自退。
            // 失败也由徽标兜底可见——卡片仍显「运行中」即说明没停掉，贴纸窗无独立错误槽位。
            // 后台会话恒不满足 endable（PTY 在 agent 的 daemon 手上），但它有自己的
            // 结束入口——后端会连上那条旁路 socket 发 kill 控制帧，比对着 pid 下手干净：
            // 杀进程会被 supervisor 按 respawnFlags 原地拉回来。
            ctxItem.endable || ctxItem.background
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
      {/* toast 统一纵向堆叠（P1-2）：focusNotice / 归档撤销 / 首启导入提示曾各自锚在
          同一 bottom，同屏互压（focusNotice 被压在撤销栈下不可见）。收敛进同一栈容器：
          导入/撤销叠在上，焦点提示贴底（原位）。栈内 .stk-focus-toast 由 CSS 改静态流。 */}
      {(importNotice !== null || archivedToasts.length > 0 || (focusNotice && focusNotice.kind !== "focused")) && (
        <div className="stk-archive-toasts">
          {/* 首启导入提示 toast（S-12，见 importNotice）：一次性，10s 自动消失或手动关。 */}
          {importNotice !== null && (
            <div className="stk-focus-toast is-archive" role="status" data-testid="stk-import-toast">
              <div className="stk-focus-body">
                <span className="stk-focus-text">{t.sticker.importNotice(importNotice)}</span>
              </div>
              <button
                type="button"
                className="stk-focus-close"
                aria-label={t.sticker.dismiss}
                onClick={() => setImportNotice(null)}
              >
                <XIcon />
              </button>
            </div>
          )}
          {/* 归档撤销 toast:逐条渲染(见 pushArchivedToast),连续归档时各自的撤销窗口互不相吞。 */}
          {archivedToasts.map(({ key, item }) => (
            <div className="stk-focus-toast is-archive" role="status" key={key}>
              <div className="stk-focus-body">
                {/* 带上会话名:多条并存时「已归档」×2 分不清哪条对应哪张卡。 */}
                <span className="stk-focus-text">
                  {(item.task_title && item.task_title !== UNNAMED_SESSION_SENTINEL ? item.task_title : t.sticker.waitingFirstInput)
                    + " · " + t.sticker.archivedNotice}
                </span>
                <div className="stk-focus-actions">
                  <button type="button" className="stk-focus-btn" onClick={() => undoArchiveToast(key, item)}>
                    {t.sticker.archiveUndo}
                  </button>
                </div>
              </div>
              <button
                type="button"
                className="stk-focus-close"
                aria-label={t.sticker.dismiss}
                onClick={() => setArchivedToasts((cur) => cur.filter((toast) => toast.key !== key))}
              >
                <XIcon />
              </button>
            </div>
          ))}
          {focusNotice && focusNotice.kind !== "focused" && (
            <div className="stk-focus-toast" role="status" onClick={(e) => e.stopPropagation()}>
              {/* 成功回执（opened）不带告警标记，与归档 toast 同一呈现原则。 */}
              {focusNotice.kind !== "opened" && <span className="stk-focus-mark" aria-hidden="true">!</span>}
              <div className="stk-focus-body">
                <span className="stk-focus-text">{focusNoticeText}</span>
                <div className="stk-focus-actions">
                  {(["host_focused", "unsupported_terminal", "alive_but_not_found", "process_ended"] as FocusNoticeKind[]).includes(focusNotice.kind) && (
                    <button type="button" className="stk-focus-btn" disabled={focusNotice.busy} onClick={() => void reopenNoticeSession()}>
                      {focusNotice.busy
                        ? t.sticker.reopening
                        : focusNotice.kind === "process_ended"
                        ? t.sticker.reopen
                        : t.sticker.reopenSupported}
                    </button>
                  )}
                  {/* TCC 自动化权限被拒（仅 macOS 会产生该结果）：直达系统设置对应页，
                      不再只丢一句「请允许」让用户自己翻。 */}
                  {focusNotice.kind === "permission_denied" && (
                    <button type="button" className="stk-focus-btn" onClick={() => { invoke("open_automation_settings").catch(() => {}); }}>
                      {t.sticker.openSystemSettings}
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
              ref={searchInputRef}
              className="stk-search-in"
              autoFocus
              value={q}
              placeholder={t.sticker.searchPlaceholder}
              // IME 合成守卫：拼音等罗马字中间态不上报——否则经 300ms 防抖打到后端 LIKE，
              // 候选还没上屏列表就闪成「无匹配」。合成结束一次性上报最终文本
              // （卡片编辑框的 editorKeyDown 早有同款守卫，搜索框此前漏了）。
              onChange={(e) => {
                if ((e.nativeEvent as InputEvent).isComposing) return;
                onSearchChange?.(e.target.value);
              }}
              onCompositionEnd={(e) => onSearchChange?.(e.currentTarget.value)}
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
            <UsageScreen quotaProviders={shownQuota} usageMap={usageMap} usageMeta={usageMeta} />
            <div className="stk-bar-actions">
              {/* 对话功能关闭（轻量模式）时不渲染 chat 入口。 */}
              {chatEnabled && (
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
              )}
              <button
                type="button"
                className="stk-act"
                // 快捷键零可发现性曾是盲区（P2-8）：Ctrl/Cmd+N 新建已注册（见窗口级快捷键），
                // tip 里说出来。修饰键按平台显示（mac 是 ⌘）。
                data-tip={`${t.newSession.newButton} · ${isMacPanel() ? "⌘" : "Ctrl+"}N`}
                aria-label={t.newSession.newButton}
                data-testid="bar-new"
                onClick={() => openNewSessionWindow().catch(() => {})}
              >
                <PlusIcon />
              </button>
              <button type="button" className="stk-act" data-tip={`${t.sticker.search} · ${isMacPanel() ? "⌘" : "Ctrl+"}F`} aria-label={t.sticker.search} onClick={() => setSearchOpen(true)}>
                <SearchIcon />
              </button>
              {/* 更新提示回到齿轮的右上角红点（实拍：独立成一颗按钮后底栏放不下，
                  置顶按钮被挤出可视区）。齿轮恒开设置——此前有更新时整颗按钮被改成
                  更新入口，图标是齿轮点了却弹更新窗，更新挂几天就几天进不去设置；
                  现在点击行为固定，红点只作提示，安装入口在设置 → 关于。 */}
              <button
                type="button"
                className="stk-act"
                data-tip={hasUpdate ? `${t.sticker.openSettings} · ${updateLabel}` : t.sticker.openSettings}
                aria-label={hasUpdate ? `${t.sticker.openSettings} · ${updateLabel}` : t.sticker.openSettings}
                onClick={() => invoke("open_settings").catch(() => {})}
              >
                <GearIcon />
                {hasUpdate && <span className="stk-dot" aria-hidden="true" />}
              </button>
              {/* pin 在 macOS 面板也渲染（W-12）：那边语义是「保持打开」，见 pinTip。 */}
              <button
                type="button"
                className={"stk-act " + (pinned ? "stk-pin-on" : "")}
                data-tip={pinTip}
                aria-label={pinTip}
                onClick={togglePin}
              >
                <PinIcon pinned={pinned} />
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}

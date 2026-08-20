import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type MutableRefObject, type UIEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { pickAndAddExtraDir, confirmStopSession, getLiveSessionsPage, getSessionLineage, openNewSessionWindow, openProjectDir, recentCwds, renameSession, searchChatTranscripts, sessionTone, setArchived, setSessionNote, type CardMenuMode, type LineageEntry, type LiveSession, type SessionTone, type TranscriptSearchHit } from "../api";
import { useBoardRefresh } from "../hooks/useBoardRefresh";
import { useSettingsEffect } from "../hooks/useSettings";
import { agentAssets, tintStyle } from "../providers";
import { folderName, parentSegment, pathKey } from "../paths";
import { useMenuPopup } from "./menu";
import { useAgents } from "../useAgents";
import { CardContextMenu } from "./sticker/CardContextMenu";
import { editorKeyDown, useStarred } from "./sticker/helpers";
import { UNNAMED_SESSION_SENTINEL } from "./sticker/types";
import { CheckIcon, ChevronDownIcon, MoreIcon, NoteIcon, TopIcon } from "./sticker/icons";
import { useT } from "../i18n";
import { formatBackendError } from "../i18n/errors";
import { isMac } from "../platform";
import type { Dict } from "../i18n/zh";

/** 每翻一页新增的会话数。滚到底自动加载下一页，直到后端带回 next_cursor = null 为止。 */
const PAGE_LIMIT = 60;

/** 目录下拉里最多列几个工作目录（后端按最近活跃排序，超出的靠搜索/滚动够不着的本就是冷目录）。 */
const DIR_LIMIT = 24;

/** 旧的布尔分组开关键（只有「按目录」一档时代的遗产）。只读不写：迁移到 GROUP_MODE_KEY。 */
const GROUPED_KEY = "meowo-chat-sidebar-grouped";
const GROUP_MODE_KEY = "meowo-chat-sidebar-group-mode";
const SORT_MODE_KEY = "meowo-chat-sidebar-sort-mode";
const FOLDED_KEY = "meowo-chat-sidebar-folded-dirs";

/** 分组方式（Claude Code 侧栏 Group by 同款）：不分组 / 按目录 / 按日期 / 按状态。 */
type GroupMode = "none" | "dir" | "date" | "state";

/** 排序方式：最近活跃（默认 = 后端 connected-first + 时间倒序原样透传）/ 创建时间 / 名称。
 *  非默认档在前端排**已加载的这一页**：排序不同于筛选，翻页不完整只影响顺序的全局性，
 *  不会让会话消失；改后端排序则要连 SQL 游标一起换口径，不值当。 */
type SortMode = "recent" | "created" | "name";

/** 无 cwd 的会话归到同一组（后端 cwd 可空:ping 型/早期数据）。以 NUL 开头，不可能与路径撞车。 */
const NO_DIR = String.fromCharCode(0) + "no-dir";

/** 组头汇总点的召唤强度序:出错必须先被看见 > 有明确动作要做 > 在等 > 在跑。
 *  组折起来时用户只剩这一个点可看,取组内最强的那个,否则「折叠即失明」。 */
const TONE_RANK: Record<SessionTone, number> = {
  error: 4, pending: 3, waiting: 2, running: 1, offline: 0, ended: 0,
};

type DirGroup = { key: string; cwd: string | null; label: string; items: LiveSession[] };

/** 接续链弹层：列出跨 provider 切换链上的各段（provider 图标 + 模型 + 起始日期），
 *  尾段标「当前」，点旧段回看。定位/关闭纪律与 CardContextMenu 同款：fixed + 视口钳位、
 *  点外关闭（click 捕获相拦截，防顺手触发卡片点击）、Escape 关闭。 */
function LineagePopover({ x, y, entries, onPick, onClose }: {
  x: number;
  y: number;
  entries: LineageEntry[];
  onPick: (id: number) => void;
  onClose: () => void;
}) {
  const t = useT();
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ left: x, top: y });
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const pad = 4;
    setPos({
      left: Math.max(pad, Math.min(x, window.innerWidth - el.offsetWidth - pad)),
      top: Math.max(pad, Math.min(y, window.innerHeight - el.offsetHeight - pad)),
    });
  }, [x, y]);
  useEffect(() => {
    const clickAway = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) {
        e.stopPropagation();
        onClose();
      }
    };
    const key = (e: KeyboardEvent) => { if (e.key === "Escape") onClose(); };
    document.addEventListener("click", clickAway, true);
    document.addEventListener("keydown", key);
    return () => {
      document.removeEventListener("click", clickAway, true);
      document.removeEventListener("keydown", key);
    };
  }, [onClose]);
  const last = entries[entries.length - 1];
  return (
    <div ref={ref} className="dd-menu chat-sidebar-lineage-menu" role="menu" style={{ position: "fixed", left: pos.left, top: pos.top }}>
      {entries.map((entry) => {
        const { Icon } = agentAssets(entry.provider);
        const current = entry === last;
        return (
          <button
            type="button"
            key={entry.id}
            role="menuitem"
            className="dd-item"
            onClick={() => { onClose(); onPick(entry.id); }}
          >
            <span className="chat-sidebar-lineage-ico" style={tintStyle(entry.provider)}><Icon /></span>
            <span className="dd-label">
              {entry.model ?? entry.provider}
              {" · "}
              {new Date(entry.startedAt).toLocaleDateString()}
            </span>
            {current && <span className="chat-sidebar-lineage-now">{t.chat.lineageCurrent}</span>}
          </button>
        );
      })}
    </div>
  );
}

/** 就地编辑输入框。草稿是**本组件的**局部状态：此前草稿放在 ChatSidebar 里，每次按键
 *  setState 都让整个侧栏（无虚拟化，几百条全在 DOM）重渲染一遍——重命名逐字卡顿的根因。
 *  提交/取消语义与原实现一致：Enter/失焦提交、Esc 取消（Esc 卸载组件，React 不再发 blur）。 */
function EditorInput({ initial, placeholder, onSubmit, onCancel }: {
  initial: string;
  placeholder: string;
  onSubmit: (value: string) => void;
  onCancel: () => void;
}) {
  const [value, setValue] = useState(initial);
  return (
    <input
      className="chat-sidebar-edit"
      autoFocus
      value={value}
      placeholder={placeholder}
      onClick={(event) => event.stopPropagation()}
      onChange={(event) => setValue(event.target.value)}
      onKeyDown={editorKeyDown(() => onSubmit(value), onCancel)}
      onBlur={() => onSubmit(value)}
    />
  );
}

/**
 * 按 cwd 分组,**组序由已有排序派生**:组按「组内第一条会话的位置」排,组内保持原顺序。
 * 于是后端的 connected-first + 时间倒序原样透过来——正在跑的会话所在目录自然浮到最上面,
 * 用户原来靠什么找会话,分组后还靠什么找。另起一套组排序(按目录名/会话数)会打乱这一点。
 */
function groupByDir(items: LiveSession[]): DirGroup[] {
  const groups: DirGroup[] = [];
  const index = new Map<string, DirGroup>();
  for (const item of items) {
    const key = item.cwd ? pathKey(item.cwd) : NO_DIR;
    let group = index.get(key);
    if (!group) {
      group = { key, cwd: item.cwd, label: folderName(item.cwd), items: [] };
      index.set(key, group);
      groups.push(group);
    }
    group.items.push(item);
  }
  return groups;
}

/** 按最后活跃时间分桶：今天 / 昨天 / 近 7 天 / 更早。桶序固定（新→旧）——
 *  connected-first 排序会把「老但还连着」的会话排最前，按首条位置派生组序会乱。 */
function groupByDate(items: LiveSession[], t: Dict): DirGroup[] {
  const dayStart = new Date();
  dayStart.setHours(0, 0, 0, 0);
  const today = dayStart.getTime();
  const buckets = [
    { key: "date:today", label: t.chat.dateToday, min: today },
    { key: "date:yesterday", label: t.chat.dateYesterday, min: today - 86_400_000 },
    { key: "date:week", label: t.chat.dateThisWeek, min: today - 6 * 86_400_000 },
    { key: "date:earlier", label: t.chat.dateEarlier, min: Number.NEGATIVE_INFINITY },
  ];
  const groups: DirGroup[] = buckets.map((bucket) => ({ key: bucket.key, cwd: null, label: bucket.label, items: [] }));
  for (const item of items) {
    const index = buckets.findIndex((bucket) => item.session.last_event_at >= bucket.min);
    groups[index === -1 ? buckets.length - 1 : index].items.push(item);
  }
  return groups.filter((group) => group.items.length > 0);
}

/** 按状态分桶，组序按召唤强度（出错 > 待处理 > 等待 > 运行中 > 离线 > 已结束）——
 *  状态视图的意义就是「先看要处理的」，此处不派生原有排序。组内保持原顺序。 */
const STATE_GROUP_ORDER: SessionTone[] = ["error", "pending", "waiting", "running", "offline", "ended"];
function groupByState(items: LiveSession[], t: Dict): DirGroup[] {
  const map = new Map<SessionTone, LiveSession[]>();
  for (const item of items) {
    const tone = sessionTone(item.connected, item.session.status, item.pending_review, item.errored, item.busy_subagents);
    const bucket = map.get(tone);
    if (bucket) bucket.push(item);
    else map.set(tone, [item]);
  }
  return STATE_GROUP_ORDER.filter((tone) => map.has(tone)).map((tone) => ({
    key: `state:${tone}`,
    cwd: null,
    label: t.chat.status[tone],
    items: map.get(tone)!,
  }));
}

/** 侧栏的筛选/分组弹层：单面板扁平化——「分组」「排序」的选项直接内联为 radio 行,
 *  选中即应用并收起(旧版是 根→子面板→选项 三级,改一项要 3 击);仅「目录」因列表
 *  无上限保留子面板。 */
function SidebarFilterMenu({ dirValue, dirOptions, groupMode, groupLabels, sortMode, sortLabels, archived, onDir, onGroup, onSort, onArchived, t }: {
  dirValue: string | null;
  dirOptions: { value: string; label: string }[];
  groupMode: GroupMode;
  groupLabels: Record<GroupMode, string>;
  sortMode: SortMode;
  sortLabels: Record<SortMode, string>;
  /** 已归档视图：与归档动作(卡片菜单 2 击)对称的取回入口——此前唯一入口埋在设置页深处。 */
  archived: boolean;
  onDir: (value: string | null) => void;
  onGroup: (mode: GroupMode) => void;
  onSort: (mode: SortMode) => void;
  onArchived: (value: boolean) => void;
  t: Dict;
}) {
  const { open, setOpen, pos, ref, btnRef, menuRef, toggle, onKeyDown, relayout } = useMenuPopup({});
  const [view, setView] = useState<"root" | "dir">("root");
  // 关闭即回根视图：下次打开不该停在上次翻到的子面板。
  useEffect(() => { if (!open) setView("root"); }, [open]);
  // 子视图切换后（根↔目录/分组）：
  // 1) 重测定位——首开测量只见过 2 行高的根视图，目录列表长出来会伸出窗口底边；
  // 2) 焦点接力——刚获焦的那一行随视图卸载，焦点落回 body、roving 键盘导航断线，
  //    把焦点落到新视图首项（只在视图真的切换时抢，打开菜单本身仍不抢焦点）。
  const prevViewRef = useRef(view);
  useLayoutEffect(() => {
    relayout();
    if (prevViewRef.current !== view) {
      prevViewRef.current = view;
      ref.current?.querySelector<HTMLElement>('[role="menuitem"], [role="option"]')?.focus();
    }
  }, [view, relayout, ref]);
  const dirLabel = dirValue
    ? dirOptions.find((option) => option.value === dirValue)?.label ?? folderName(dirValue)
    : t.chat.sidebarDirAll;
  const chevron = (
    <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><polyline points="9 6 15 12 9 18" /></svg>
  );
  const back = (
    <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><polyline points="15 6 9 12 15 18" /></svg>
  );
  const check = <CheckIcon className="dd-check" />;
  // 选中即应用并收起：筛选是「一次性的快速动作」，留着面板反而多一步关闭。
  const pick = (apply: () => void) => { apply(); setOpen(false); btnRef.current?.focus(); };
  return (
    <div className="dd" ref={ref} onKeyDown={onKeyDown}>
      <button
        ref={btnRef}
        type="button"
        className="chat-sidebar-filter"
        aria-label={t.chat.sidebarFilterTip}
        data-tip={t.chat.sidebarFilterTip}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={toggle}
      >
        <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" aria-hidden="true">
          <path d="M8 4v3.2M8 12.4v7.6M16 4v7.6M16 16.8V20" />
          <circle cx="8" cy="9.8" r="2.2" />
          <circle cx="16" cy="14.2" r="2.2" />
        </svg>
      </button>
      {open && (
        <div className="dd-menu sf-menu" role="menu" ref={menuRef} style={{ position: "fixed", top: pos.top, bottom: pos.bottom, right: pos.right }}>
          {view === "root" ? (
            <>
              <button type="button" role="menuitem" className="dd-item sf-row" onClick={() => setView("dir")}>
                <span>{t.chat.sidebarFilterDir}</span>
                <span className="sf-val"><span className="sf-val-text">{dirLabel}</span>{chevron}</span>
              </button>
              <div className="sf-sep" role="separator" />
              <div className="sf-caption">{t.chat.sidebarFilterGroup}</div>
              {(["none", "dir", "date", "state"] as const).map((mode) => {
                const selected = groupMode === mode;
                return (
                  <button
                    key={mode}
                    type="button"
                    role="option"
                    aria-selected={selected}
                    className={"dd-item" + (selected ? " sel" : "")}
                    onClick={() => pick(() => onGroup(mode))}
                  >
                    <span className="dd-label">{groupLabels[mode]}</span>
                    {selected && check}
                  </button>
                );
              })}
              <div className="sf-sep" role="separator" />
              <div className="sf-caption">{t.chat.sidebarFilterSort}</div>
              {(["recent", "created", "name"] as const).map((mode) => {
                const selected = sortMode === mode;
                return (
                  <button
                    key={mode}
                    type="button"
                    role="option"
                    aria-selected={selected}
                    className={"dd-item" + (selected ? " sel" : "")}
                    onClick={() => pick(() => onSort(mode))}
                  >
                    <span className="dd-label">{sortLabels[mode]}</span>
                    {selected && check}
                  </button>
                );
              })}
              {/* 非默认档只排已加载的这一页（翻页新条目追加尾部）：不说明的话,
                  乱序看起来就是「排序坏了」。 */}
              {sortMode !== "recent" && <div className="sf-note">{t.chat.sortPartialNote}</div>}
              <div className="sf-sep" role="separator" />
              <button
                type="button"
                role="option"
                aria-selected={archived}
                className={"dd-item" + (archived ? " sel" : "")}
                onClick={() => pick(() => onArchived(!archived))}
              >
                <span className="dd-label">{t.settings.archivedSessions}</span>
                {archived && check}
              </button>
            </>
          ) : (
            <>
              <button type="button" role="menuitem" className="dd-item sf-back" onClick={() => setView("root")}>
                {back}
                <span>{t.chat.sidebarFilterDir}</span>
              </button>
              <div className="sf-sep" role="separator" />
              {[{ value: "", label: t.chat.sidebarDirAll }, ...dirOptions].map((option) => {
                const selected = (dirValue ?? "") === option.value;
                return (
                  <button
                    key={option.value}
                    type="button"
                    role="option"
                    aria-selected={selected}
                    className={"dd-item" + (selected ? " sel" : "")}
                    // 菜单内刻意用原生 title：TooltipLayer 对 .dd-menu 区域静默
                    // （自绘提示会糊在菜单上，见 Tooltip.tsx tipEl 的注释）。
                    title={option.value || undefined}
                    onClick={() => pick(() => onDir(option.value || null))}
                  >
                    <span className="dd-label">{option.label}</span>
                    {selected && check}
                  </button>
                );
              })}
            </>
          )}
        </div>
      )}
    </div>
  );
}

function readFolded(): Set<string> {
  try {
    const raw = JSON.parse(localStorage.getItem(FOLDED_KEY) || "[]");
    return new Set(Array.isArray(raw) ? raw.filter((k): k is string => typeof k === "string") : []);
  } catch {
    return new Set();
  }
}

/**
 * 对话窗口左侧的会话切换列表，与右侧内容列并排、占满整窗高度。数据与看板同源
 * （get_live_sessions_page），靠 board-changed 广播刷新——它已经过后端合流去抖，
 * 这里不再自设轮询。折叠状态由 ChatWindow 持有（收起后展开入口在标题栏），
 * 本组件收起时整个卸载，数据加载随之停止。
 * memo：ChatWindow 每次击键/每秒审批倒计时都重渲染，不隔断的话上百条 item 的整棵
 * 子树跟着全量重建；父层回调已换稳定引用（resetTo/collapseSidebar 等）配合生效。
 */
export const ChatSidebar = memo(ChatSidebarImpl);
function ChatSidebarImpl({ activeId, approvalAwaitingIds, visibleOrderRef, onSelect, onCollapse }: {
  activeId: number;
  /** 有待授权请求的非当前会话：靠这里的徽标召唤用户。后端 **会** 为 broker 接管的审批把
   *  窗口切到目标会话（见 pty.rs 的 ensure_approval_window），这里是切换竞态与
   *  「消费者已注册在别的会话」时的兜底。 */
  approvalAwaitingIds: ReadonlySet<number>;
  /** 当前显示顺序的镜像（父层归档跳转用）：归档当前会话后跳「用户看到的下一条」，
   *  两个归档入口（标题菜单/侧栏菜单）取序自此一致。卸载时清空，父层退回后端现查。 */
  visibleOrderRef?: MutableRefObject<number[]>;
  onSelect: (id: number) => void;
  onCollapse: () => void;
}) {
  const t = useT();
  // null = 首次加载尚未完成：与「真空」区分，避免首帧误闪「暂无会话」。
  const [sessions, setSessions] = useState<LiveSession[] | null>(null);
  // 接续链弹层（跨 provider 切换的回看入口）：点链徽标时惰性取链上各段。
  const [lineageMenu, setLineageMenu] = useState<{ x: number; y: number; entries: LineageEntry[] } | null>(null);
  // 翻页用「从头取 limit 条」而不是游标续传：整段重取让 board-changed 刷新走同一条
  // 路径，不必维护「已加载的页」这份额外状态。到底与否看后端带回的 next_cursor。
  const [limit, setLimit] = useState(PAGE_LIMIT);
  const [reachedEnd, setReachedEnd] = useState(false);
  // 翻页请求在途：state 供 loading 行渲染；ref 同步镜像挡快速连滚——两个 scroll 事件
  // 可能落在同一次渲染提交前，state 版守卫看不出第一发已经把 limit 抬上去了。
  const [growing, setGrowing] = useState(false);
  const growingRef = useRef(false);
  const mountedRef = useRef(true);
  const limitRef = useRef(limit);
  limitRef.current = limit;
  // 目录筛选:选中的 cwd 走后端的**专用 cwd 参数**(斜杠归一后精确比较,子目录一并命中),
  // 不是 search 的子串 LIKE——理由见下面 load 里的注释。分页与排序仍全在后端做:前端过滤
  // 只能过滤「已加载的这一页」,筛出来的清单是残缺的。
  const [dirFilter, setDirFilter] = useState<string | null>(null);
  const dirFilterRef = useRef(dirFilter);
  dirFilterRef.current = dirFilter;
  // 已归档视图:数据走后端现成的 "archived" 过滤(与设置页 ArchivedSessions 同一通道),
  // 条目菜单里自然出现「取消归档」——归档(卡片菜单)与取回自此对称。不落盘:临时视图。
  const [archivedView, setArchivedView] = useState(false);
  const archivedViewRef = useRef(archivedView);
  archivedViewRef.current = archivedView;
  const [dirs, setDirs] = useState<string[]>([]);
  // 目录筛选下拉的显示名：同名目录（路径不同）带上上一级目录消歧——不消歧时用户面对
  // 一排「codebase」无从分辨（实拍反馈）。上一级仍同名的（祖父级才不同）极罕见，不再加深。
  const dirOptions = useMemo(() => {
    const counts = new Map<string, number>();
    for (const cwd of dirs) {
      const name = folderName(cwd);
      counts.set(name, (counts.get(name) ?? 0) + 1);
    }
    return dirs.map((cwd) => {
      const name = folderName(cwd);
      if ((counts.get(name) ?? 0) <= 1) return { value: cwd, label: name };
      const parent = parentSegment(cwd);
      return { value: cwd, label: parent ? `${parent}/${name}` : name };
    });
  }, [dirs]);
  // 分组方式默认不分组:不用的人一个像素都感知不到。旧布尔开关(只有按目录一档)自动迁移。
  const [groupMode, setGroupMode] = useState<GroupMode>(() => {
    const stored = localStorage.getItem(GROUP_MODE_KEY);
    if (stored === "none" || stored === "dir" || stored === "date" || stored === "state") return stored;
    return localStorage.getItem(GROUPED_KEY) === "1" ? "dir" : "none";
  });
  // 排序方式默认「最近活跃」= 现状顺序，同分组一样落盘。
  const [sortMode, setSortMode] = useState<SortMode>(() => {
    const stored = localStorage.getItem(SORT_MODE_KEY);
    return stored === "created" || stored === "name" ? stored : "recent";
  });
  // 折叠的组(按 pathKey)。侧栏收起时本组件整个卸载,不落盘的话回来全展开了。
  const [folded, setFolded] = useState<Set<string>>(readFolded);

  // 会话搜索:输入防抖 300ms 后下沉后端(searchRef 是 load 的同步读源)。
  // 不落盘——搜索是临时视图,残留的词会让下次打开的列表被静默过滤(看板踩过同款坑)。
  const [search, setSearch] = useState("");
  const searchRef = useRef("");
  // searchRef 是防抖后（300ms）才落的 load 读源;深度搜索判活性要的是**当前**词,故另用
  // 一个每渲染同步的镜像 ref(本仓 xxxRef.current = xxx 惯例),回包时与它比即可。
  const searchLiveRef = useRef(search);
  searchLiveRef.current = search;
  const searchInputRef = useRef<HTMLInputElement>(null);
  // 对话内容全文搜索(显式动作):LIKE 只覆盖标题/目录/便签/最近往来,正文要靠后端扫
  // transcript 文件——点「搜索对话内容」才发,不随击键。结果与词绑定,词一变即作废。
  const [deepHits, setDeepHits] = useState<TranscriptSearchHit[] | null>(null);
  const [deepSearching, setDeepSearching] = useState(false);
  useEffect(() => { setDeepHits(null); }, [search]);
  const runDeepSearch = () => {
    const q = search.trim();
    if (!q || deepSearching) return;
    setDeepSearching(true);
    searchChatTranscripts(q)
      // 词已变则丢弃回包(与 load() 的词校验同款):在途期间用户改了搜索词,
      // useEffect([search]) 已把 deepHits 清成 null,旧词结果落地会在新词视图下显错命中。
      .then((hits) => { if (mountedRef.current && searchLiveRef.current.trim() === q) setDeepHits(hits); })
      .catch(() => { if (mountedRef.current) setActionError(t.chat.sidebarActionFailed); })
      .finally(() => { if (mountedRef.current) setDeepSearching(false); });
  };

  // refresh = 整段重取替换（首载 / board-changed）；grow = 翻页，只把新条目**追加到尾部**。
  // 追加而非替换：后端对整页做 connected-first 排序，扩大 limit 可能把更深处的活会话
  // 顶到列表最上方——用户正停在底部等下一页，上方插行会让视口内容跳走。已加载段的
  // 重排交给下一次 refresh（那时用户多半不在翻页途中）。
  const load = useCallback((mode: "refresh" | "grow") => {
    const lim = limitRef.current;
    const dir = dirFilterRef.current;
    // 目录走专用 cwd 参数(后端斜杠归一后精确比较),不复用 search 的子串 LIKE——
    // 那会让另一种斜杠写法的会话整批消失,还把兄弟目录/标题命中漏进来。
    // 搜索走 search 参数(标题/cwd/项目名 LIKE,与看板同一条后端通道):会话上百条时
    // 靠滚动翻页找会话的路是断的,此前只能退回看板去搜。
    const q = searchRef.current.trim() || null;
    const arch = archivedViewRef.current;
    return getLiveSessionsPage(arch ? "archived" : "all", q, null, lim, dir)
      .then((page) => {
        if (!mountedRef.current) return;
        // 切目录/改搜索词/切归档视图期间发出的旧请求回来了:它装的是旧条件的会话,
        // 原样落盘会让列表与筛选状态对不上。丢弃。
        if (dirFilterRef.current !== dir) return;
        if (archivedViewRef.current !== arch) return;
        if ((searchRef.current.trim() || null) !== q) return;
        setReachedEnd(page.next_cursor === null);
        if (mode === "grow") {
          setSessions((prev) => {
            if (!prev) return page.items;
            const seen = new Set(prev.map((s) => s.session.id));
            return [...prev, ...page.items.filter((s) => !seen.has(s.session.id))];
          });
        } else {
          setSessions(page.items);
        }
      })
      .catch(() => {
        if (!mountedRef.current || dirFilterRef.current !== dir) return;
        // 翻页失败：limit 退回上一页的量。不退的话 loading 行永远挂着、重滚也发不出
        // 请求（limit 已被抬高，唯一能救场的只剩恰好路过的 board-changed）。
        if (mode === "grow") setLimit((n) => Math.max(PAGE_LIMIT, n - PAGE_LIMIT));
        // 首载失败降级为空列表（显示「暂无会话」），而不是永远停在加载占位。
        setSessions((s) => s ?? []);
      })
      .finally(() => {
        if (mode === "grow") {
          growingRef.current = false;
          if (mountedRef.current) setGrowing(false);
        }
      });
  }, []);
  const loadRef = useRef(load);
  loadRef.current = load;

  // 目录清单与会话列表同源于库。随 board-changed 一起刷（节流由 useBoardRefresh 统一做）:
  // 只在挂载取一次的话，新目录里刚建的会话在筛选下拉里找不到，直到重开侧栏。
  const refreshDirs = useCallback(() => {
    recentCwds(DIR_LIMIT)
      .then((list) => {
        if (!mountedRef.current) return;
        // 后端按原始字符串去重;同一目录可能因历史数据斜杠方向不同而重复。这里按归一
        // key 再去一次重,保留首次出现(最近活跃)的原始写法做显示;筛选走后端的 cwd
        // 参数,双方都斜杠归一后精确比较,选哪种写法都能命中两种存法的会话。
        const seen = new Set<string>();
        setDirs(list.filter((cwd) => {
          const key = pathKey(cwd);
          if (!cwd || seen.has(key)) return false;
          seen.add(key);
          return true;
        }));
      })
      .catch(() => {});
  }, []);
  const refreshDirsRef = useRef(refreshDirs);
  refreshDirsRef.current = refreshDirs;

  // board-changed 订阅 + 节流统一在 useBoardRefresh（订阅只建一次、不随 limit 重建：
  // unlisten → 新 listen 之间有异步空窗，恰落在空窗里的事件会被吞掉）。
  useBoardRefresh(() => {
    void loadRef.current("refresh");
    refreshDirsRef.current();
  });
  useEffect(() => {
    mountedRef.current = true;
    void loadRef.current("refresh");
    refreshDirsRef.current();
    return () => {
      mountedRef.current = false;
    };
  }, []);

  // limit 变大（滚动翻页）→ 拉一次 grow。失败回退会把 limit 改小，不能据此再发请求，
  // 否则「失败 → 回退 → 又触发加载」原地打转。
  const prevLimitRef = useRef(PAGE_LIMIT);
  useEffect(() => {
    const grew = limit > prevLimitRef.current;
    prevLimitRef.current = limit;
    if (grew) void loadRef.current("grow");
  }, [limit]);

  // 切目录 = 换一份数据源:整段重取,并把翻页状态清回首页(不清的话新目录一上来就
  // 顶着上一个目录翻到的 limit,一次拉几百条)。首挂载不重复触发(那次由挂载 effect 发)。
  const prevDirRef = useRef<string | null>(null);
  const dirTouchedRef = useRef(false);
  useEffect(() => {
    if (!dirTouchedRef.current) {
      dirTouchedRef.current = true;
      prevDirRef.current = dirFilter;
      return;
    }
    if (prevDirRef.current === dirFilter) return;
    prevDirRef.current = dirFilter;
    // limitRef 必须**当场**改回去:load 立刻就读它,而 setLimit 要等下一次渲染才落到
    // limitRef 上——只 setLimit 的话,切目录后的首个请求仍带着上一个目录翻到的 limit。
    prevLimitRef.current = PAGE_LIMIT;
    limitRef.current = PAGE_LIMIT;
    setLimit(PAGE_LIMIT);
    setReachedEnd(false);
    setSessions(null);
    // 在途的 grow 归属上一个目录:它的响应已被 search 守卫丢弃,这里把闸门也放开,
    // 否则新目录要等那个作废的请求 settle 才能翻页。
    growingRef.current = false;
    setGrowing(false);
    void loadRef.current("refresh");
  }, [dirFilter]);

  // 切换已归档视图 = 换数据源:与切目录同一套重置语义。
  const prevArchRef = useRef(false);
  useEffect(() => {
    if (prevArchRef.current === archivedView) return;
    prevArchRef.current = archivedView;
    prevLimitRef.current = PAGE_LIMIT;
    limitRef.current = PAGE_LIMIT;
    setLimit(PAGE_LIMIT);
    setReachedEnd(false);
    setSessions(null);
    growingRef.current = false;
    setGrowing(false);
    void loadRef.current("refresh");
  }, [archivedView]);

  // 搜索词变化:防抖 300ms 重取首页(翻页状态清回首页,与切目录同一套语义)。
  // 不清 sessions——保留旧列表到新结果落地,避免每敲一个字闪一次加载占位。
  const searchTouchedRef = useRef(false);
  useEffect(() => {
    if (!searchTouchedRef.current) {
      searchTouchedRef.current = true;
      return;
    }
    const timer = window.setTimeout(() => {
      searchRef.current = search;
      prevLimitRef.current = PAGE_LIMIT;
      limitRef.current = PAGE_LIMIT;
      setLimit(PAGE_LIMIT);
      setReachedEnd(false);
      growingRef.current = false;
      setGrowing(false);
      void loadRef.current("refresh");
    }, 300);
    return () => window.clearTimeout(timer);
  }, [search]);

  // Ctrl/Cmd+F 聚焦搜索框（终端视图内让位——焦点在 xterm 里时用户多半想找终端内容，
  // 抢过来只会打断；终端自己的搜索是后续项）。
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || e.altKey || e.code !== "KeyF") return;
      if (document.activeElement?.closest(".managed-terminal")) return;
      e.preventDefault();
      searchInputRef.current?.focus();
      searchInputRef.current?.select();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // 会话操作菜单:与看板卡片是同一个 CardContextMenu、同一批动作(置顶/便签/重命名/归档/
  // 新建会话/打开目录/结束会话)。两处共用一个组件是刻意的——菜单项一旦各写一份,迟早
  // 会一边有、另一边没有。触发方式:条目右键,或条目上的「⋯」按钮。
  const [ctxMenu, setCtxMenu] = useState<{ sid: number; x: number; y: number } | null>(null);
  // 触发方式跟随「卡片菜单」设置(card_menu_mode),与看板同一个开关、同一套语义:
  // button = 条目上的「⋯」按钮,context = 右键。**二选一**,不同时存在——两个入口并存
  // 时,改设置的人会以为没生效(另一个还在),不改的人则平白多一个不知从哪来的按钮。
  // 首帧占位与后端真实默认(button)一致,与看板同步改过——占位 context 会让 ⋯ 按钮延迟弹入。
  const [menuMode, setMenuMode] = useState<CardMenuMode>("button");
  useSettingsEffect((s) => setMenuMode(s.card_menu_mode ?? "button"));
  // 菜单内容按 id 现查:刷新后置顶/便签/归档态自动跟上,会话消失则菜单自然收起。
  const ctxItem = ctxMenu ? (sessions ?? []).find((s) => s.session.id === ctxMenu.sid) ?? null : null;
  // 置顶与看板共用同一份 localStorage(键沿用 meowo-starred,改键会丢用户已有数据),
  // 两个界面看到的是同一批置顶会话,且都排到最前(见下方 ordered)。
  // 置顶集：与看板/对话窗标题菜单共用 useStarred（同一份写入口 + 双通道同步）。
  const { starred, toggleStar } = useStarred();
  // 重命名 / 便签:与看板一样就地编辑(同一条会话上只开一个编辑器)。
  // 草稿文本不在这里——住在 EditorInput 的局部状态里,按键不触发侧栏整表重渲染。
  const [editingId, setEditingId] = useState<number | null>(null);
  const [notingId, setNotingId] = useState<number | null>(null);
  // 动作失败必须可见:静默吞掉的话,用户点了「重命名」只会看到名字没变,以为是自己没点上。
  const [actionError, setActionError] = useState<string | null>(null);
  useEffect(() => {
    if (!actionError) return;
    const timer = window.setTimeout(() => setActionError(null), 4_000);
    return () => window.clearTimeout(timer);
  }, [actionError]);

  const startRename = (item: LiveSession) => {
    setNotingId(null);
    setEditingId(item.session.id);
  };
  const submitRename = (item: LiveSession, draft: string) => {
    const title = draft.trim();
    if (title && title !== item.task_title) {
      renameSession(item.cwd, item.session.cc_session_id, title, item.provider)
        .catch(() => setActionError(t.sticker.renameFailed));
    }
    setEditingId(null);
  };
  const startNote = (item: LiveSession) => {
    setEditingId(null);
    setNotingId(item.session.id);
  };
  const submitNote = (item: LiveSession, noteDraft: string) => {
    if (noteDraft !== (item.note ?? "")) {
      setSessionNote(item.session.cc_session_id, noteDraft)
        .catch(() => setActionError(t.sticker.noteFailed));
    }
    setNotingId(null);
  };
  // 归档撤销:动作无确认、取回入口(筛选菜单→已归档)较隐蔽,归档成功后给 8s 一键撤销
  // (与看板 toast、对话窗标题菜单同一语义)。数组承载批量归档的整批撤销。
  const [archiveUndo, setArchiveUndo] = useState<number[] | null>(null);
  useEffect(() => {
    if (archiveUndo == null) return;
    const timer = window.setTimeout(() => setArchiveUndo(null), 8_000);
    return () => window.clearTimeout(timer);
  }, [archiveUndo]);
  const undoArchive = (ids: number[]) => {
    setArchiveUndo(null);
    void Promise.allSettled(ids.map((id) => setArchived(id, false))).then((results) => {
      if (results.some((entry) => entry.status === "rejected")) setActionError(t.chat.sidebarActionFailed);
      void loadRef.current("refresh");
      onSelect(ids[0]);
    });
  };
  // 批量归档:Ctrl/Cmd+点击进入多选(继续 Ctrl+点击增删),普通点击退出多选并正常切会话,
  // Esc 清空退出。选中集按 id,刷新/翻页不受影响。
  const [selectedIds, setSelectedIds] = useState<ReadonlySet<number>>(() => new Set());
  useEffect(() => {
    if (selectedIds.size === 0) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      // 截停:多选态的 Esc 只清选择,不落到窗口级「拒绝审批」。
      event.preventDefault();
      setSelectedIds(new Set());
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [selectedIds]);
  const toggleSelected = (id: number) =>
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (!next.delete(id)) next.add(id);
      return next;
    });
  const archiveSelected = () => {
    const ids = [...selectedIds];
    if (ids.length === 0) return;
    setSelectedIds(new Set());
    // 乐观整批摘除;当前打开的也在其中时切到幸存的下一条(与单条归档同语义)。
    setSessions((prev) => prev?.filter((s) => !ids.includes(s.session.id)) ?? prev);
    if (ids.includes(activeId)) {
      const following = (ordered ?? []).find((s) => !ids.includes(s.session.id));
      if (following) onSelect(following.session.id);
    }
    void Promise.allSettled(ids.map((id) => setArchived(id, true))).then((results) => {
      const ok = ids.filter((_, index) => results[index].status === "fulfilled");
      if (ok.length < ids.length) {
        setActionError(t.chat.sidebarActionFailed);
        void loadRef.current("refresh");
      }
      if (ok.length > 0) setArchiveUndo(ok);
    });
  };
  const toggleArchived = (item: LiveSession) => {
    const target = !item.archived;
    const id = item.session.id;
    // 归档 = 从 "all" 里消失,乐观摘掉(不等 IPC 往返),失败重取拉回。当前打开的那条也照摘:
    // 已归档 = 已收纳,列表里就不该再有它,哪怕右边还开着它的对话。
    setSessions((prev) => prev?.filter((s) => s.session.id !== id) ?? prev);
    // 归档的正是当前打开的这条:右边也得跟着走,否则「收起来了却还摊在桌上」。
    // 切到列表里的下一条;它是唯一一条时留在原地(空窗比自作主张关窗好)。
    if (target && id === activeId) {
      const following = (ordered ?? []).find((s) => s.session.id !== id);
      if (following) onSelect(following.session.id);
    }
    setArchived(id, target)
      .then(() => { if (target) setArchiveUndo([id]); })
      .catch(() => {
        setActionError(t.chat.sidebarActionFailed);
        void loadRef.current("refresh");
      });
  };
  const endSession = (item: LiveSession) => {
    void confirmStopSession(item.session.id, { title: t.chat.endSession, message: t.chat.endSessionConfirm })
      .catch((error) => setActionError(formatBackendError(error, t.locale)));
  };
  const openMenuAt = (item: LiveSession, x: number, y: number) => setCtxMenu({ sid: item.session.id, x, y });

  // 中途附加目录(条目菜单):命令统一负责落库 + 即时生效(有托管 PTY 时后端直写
  // /add-dir);断开会话只落库,下次恢复回放。入口按能力位显隐(与贴纸同口径)。
  const { agents } = useAgents();
  const supportsExtraDir = (p: string) => agents?.find((a) => a.id === p)?.supports_extra_dirs ?? false;
  const addExtraDir = async (item: LiveSession) => {
    try {
      await pickAndAddExtraDir(item.session.id);
    } catch (error) {
      setActionError(formatBackendError(error, t.locale));
    }
  };

  const changeGroupMode = (mode: GroupMode) => {
    setGroupMode(mode);
    localStorage.setItem(GROUP_MODE_KEY, mode);
  };
  const changeSortMode = (mode: SortMode) => {
    setSortMode(mode);
    localStorage.setItem(SORT_MODE_KEY, mode);
  };
  const toggleFolded = (key: string) => setFolded((prev) => {
    const next = new Set(prev);
    if (!next.delete(key)) next.add(key);
    localStorage.setItem(FOLDED_KEY, JSON.stringify([...next]));
    return next;
  });

  // 分组只在开关打开时算。**它只覆盖已加载的这一页**:滚动翻页会让组长大、后来的
  // 会话可能落进更靠上的组。选了目录时不存在这个问题(筛选在后端做,清单是完整的)。
  // 各分组里「未运行会话」的展开状态。刻意不持久化：它是一次浏览中的临时动作，
  // 下次打开侧栏该回到「只看在跑的」这个默认。
  const [revealed, setRevealed] = useState<ReadonlySet<string>>(() => new Set<string>());
  const toggleRevealed = (key: string) =>
    setRevealed((prev) => {
      const next = new Set(prev);
      if (!next.delete(key)) next.add(key);
      return next;
    });

  // 置顶浮顶:与看板同一条规则(置顶的排最前),否则「置顶」名不副实——标记亮着、位置没动。
  // Array.sort 稳定,「最近活跃」档非置顶的相对次序原样保留(后端 connected-first +
  // 时间倒序);「创建时间/名称」档在置顶之后按所选键重排(用户明确选了排序,不再保
  // connected-first——正在跑的会话有状态点可辨识)。
  // 分组开着时排序发生在**分组之前**:组内置顶在前,而组序由「组内第一条的位置」派生,
  // 于是含置顶会话的目录也跟着上浮——两套顺序不打架。
  const ordered = useMemo(() => {
    if (!sessions || (starred.size === 0 && sortMode === "recent")) return sessions;
    const rank = (item: LiveSession) => (starred.has(item.session.cc_session_id) ? 0 : 1);
    return [...sessions].sort((a, b) => {
      const star = rank(a) - rank(b);
      if (star !== 0) return star;
      if (sortMode === "created") return b.session.started_at - a.session.started_at;
      if (sortMode === "name") {
        // 未命名会话沉底（空串与 DB sentinel「(未命名会话)」同口径，见 EditorInput 的
        // initial）：一排未命名插在人起的名字中间只会打断扫读。
        const an = a.task_title !== UNNAMED_SESSION_SENTINEL ? a.task_title : "";
        const bn = b.task_title !== UNNAMED_SESSION_SENTINEL ? b.task_title : "";
        if (!an || !bn) return an ? -1 : bn ? 1 : 0;
        return an.localeCompare(bn);
      }
      return 0;
    });
  }, [sessions, starred, sortMode]);

  const groups = useMemo(() => {
    if (!ordered || groupMode === "none") return null;
    if (groupMode === "dir") return groupByDir(ordered);
    if (groupMode === "date") return groupByDate(ordered, t);
    return groupByState(ordered, t);
  }, [groupMode, ordered, t]);

  // 显示顺序镜像:分组模式的组序由 ordered 派生、组内亦是 ordered 序,写 ordered 即可。
  // 卸载清空——收起的侧栏没有「用户看到的顺序」,父层归档跳转退回后端现查。
  useEffect(() => {
    if (!visibleOrderRef) return;
    visibleOrderRef.current = (ordered ?? []).map((item) => item.session.id);
  }, [ordered, visibleOrderRef]);
  useEffect(() => () => { if (visibleOrderRef) visibleOrderRef.current = []; }, [visibleOrderRef]);

  // 滚到底前 120px 就预取下一页。
  const onScroll = (event: UIEvent<HTMLElement>) => {
    if (reachedEnd || sessions === null || growingRef.current) return;
    const el = event.currentTarget;
    if (el.scrollHeight - el.scrollTop - el.clientHeight > 120) return;
    growingRef.current = true;
    setGrowing(true);
    setLimit((n) => n + PAGE_LIMIT);
  };

  // 平铺与分组共用同一份条目渲染:两套 JSX 会各自漂移(状态点口径、标题回退文案),
  // 那正是「贴纸报错、侧栏亮绿点」那类不一致的来源。
  const renderItem = (item: LiveSession) => {
    const Icon = agentAssets(item.provider).Icon;
    const dir = folderName(item.cwd);
    // 与对话窗标题栏同一套口径(sessionTone,含 errored——贴纸的错误优先级由此
    // 对齐,不再出现「贴纸报错、侧栏亮绿点」)。offline/ended 不加点——图标置灰
    // (is-off)已表达「不活跃」,再叠一个灰点是噪声。
    const tone = sessionTone(item.connected, item.session.status, item.pending_review, item.errored, item.busy_subagents);
    const showDot = tone === "running" || tone === "pending" || tone === "waiting" || tone === "error";
    // 待授权徽标优先于状态点：授权在等用户决策，比「在跑/待处理」都紧急；
    // 且不依附 connected——离线会话恢复的 agent 同样可能来要权限。
    const awaitingApproval = approvalAwaitingIds.has(item.session.id);
    const editing = editingId === item.session.id;
    const noting = notingId === item.session.id;
    // 条目从 <button> 改成 role=button 的 <div>：里面要放「⋯」按钮，button 套 button 是
    // 非法 HTML（浏览器会把内层拆出去）。键盘可达性靠 tabIndex + Enter/Space 补齐。
    return (
      <div
        key={item.session.id}
        role="button"
        tabIndex={0}
        className={"chat-sidebar-item" + (item.session.id === activeId ? " is-active" : "") + (selectedIds.has(item.session.id) ? " is-checked" : "")}
        aria-current={item.session.id === activeId ? "true" : undefined}
        aria-selected={selectedIds.size > 0 ? selectedIds.has(item.session.id) : undefined}
        data-tip={item.task_title}
        onClick={(event) => {
          // 编辑态下点条目只用于收起编辑器，不切会话（输入框自身已 stopPropagation）。
          if (editing || noting) { setEditingId(null); setNotingId(null); return; }
          // Ctrl/Cmd+点击 = 多选增删(批量归档);普通点击退出多选并正常切会话。
          // 多选态下条目右侧另有勾选圈,点它增删不必按住 Ctrl。
          if (event.ctrlKey || event.metaKey) {
            toggleSelected(item.session.id);
            return;
          }
          if (selectedIds.size > 0) setSelectedIds(new Set());
          onSelect(item.session.id);
        }}
        onKeyDown={(event) => {
          if (event.target !== event.currentTarget) return; // 内部按钮/输入框自理
          if (event.key !== "Enter" && event.key !== " ") return;
          event.preventDefault(); // 空格不要滚动列表
          if (editing || noting) { setEditingId(null); setNotingId(null); return; }
          onSelect(item.session.id);
        }}
        onContextMenu={(event) => {
          // 右键恒开菜单：条目上右键出上下文菜单是通用惯例，不随设置失效——
          // card_menu_mode 只决定「⋯」按钮是否常驻，不再关掉右键这条路。
          event.preventDefault();
          openMenuAt(item, event.clientX, event.clientY);
        }}
      >
        {/* 状态指示兼 agent 标识：连接=品牌色徽标，未连接=灰（与贴纸同一套）。 */}
        <span
          className={"chat-sidebar-agent-icon" + (item.connected ? "" : " is-off")}
          style={tintStyle(item.provider, item.connected)}
          role="img"
          aria-label={item.provider}
        >
          <Icon />
          {awaitingApproval
            ? <i className="chat-sidebar-dot is-approval" role="status" aria-label={t.chat.sidebarApproval} data-tip={t.chat.sidebarApproval} />
            : showDot && <i className={`chat-sidebar-dot is-${tone}`} role="status" aria-label={t.chat.status[tone]} data-tip={t.chat.status[tone]} />}
        </span>
        <span className="chat-sidebar-text">
          {editing ? (
            <EditorInput
              initial={item.task_title && item.task_title !== UNNAMED_SESSION_SENTINEL ? item.task_title : ""}
              placeholder={t.sticker.renamePlaceholder}
              onSubmit={(value) => submitRename(item, value)}
              onCancel={() => setEditingId(null)}
            />
          ) : (
            <span className="chat-sidebar-name">
              {starred.has(item.session.cc_session_id) && (
                <span className="chat-sidebar-star" role="img" aria-label={t.sticker.star}><TopIcon /></span>
              )}
              {/* 标题必须包成自己的 span：.chat-sidebar-name 是 flex 容器（要排星标），
                  裸文本节点是匿名 flex item，容器上的 text-overflow 对它不生效——
                  长标题会溢出而不出省略号（实拍反馈）。 */}
              <span className="chat-sidebar-name-text">{item.task_title || t.sticker.waitingFirstInput}</span>
              {/* 跨 provider 接续链徽标：点开列出链上各段（回看旧引擎的对话）。 */}
              {item.predecessor_id != null && (
                <button
                  type="button"
                  className="chat-sidebar-lineage"
                  aria-label={t.chat.lineageBadge}
                  data-tip={t.chat.lineageBadge}
                  onClick={(event) => {
                    event.stopPropagation();
                    const rect = event.currentTarget.getBoundingClientRect();
                    void getSessionLineage(item.session.id)
                      .then((entries) => setLineageMenu({ x: rect.left, y: rect.bottom + 4, entries }))
                      .catch(() => {});
                  }}
                >
                  <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71" /><path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71" /></svg>
                </button>
              )}
            </span>
          )}
          {/* 按目录分组时每条再重复一遍目录名是噪声——组头已经写着了（按日期/状态分组时目录仍有用）。 */}
          {dir && groupMode !== "dir" && !noting && <span className="chat-sidebar-meta" data-tip={item.cwd ?? undefined}>{dir}</span>}
          {/* 便签写完要看得见，否则「添加便签」等于写进黑洞。编辑中的那条让位给输入框。 */}
          {!noting && item.note && (
            <span className="chat-sidebar-note" data-tip={item.note}><NoteIcon />{item.note}</span>
          )}
          {noting && (
            <EditorInput
              initial={item.note ?? ""}
              placeholder={t.sticker.notePlaceholder}
              onSubmit={(value) => submitNote(item, value)}
              onCancel={() => setNotingId(null)}
            />
          )}
        </span>
        {selectedIds.size > 0 ? (
          /* 多选态:右侧换成勾选圈(未选空心/选中实心勾),点它增删不必按住 Ctrl。
             「⋯」菜单此间让位——多选时的动作在底部操作条上。 */
          <button
            type="button"
            className="chat-sidebar-check-btn"
            role="checkbox"
            aria-checked={selectedIds.has(item.session.id)}
            aria-label={item.task_title || t.sticker.waitingFirstInput}
            onClick={(event) => {
              event.stopPropagation();
              toggleSelected(item.session.id);
            }}
          >
            <span className="chat-sidebar-check" aria-hidden="true">
              {selectedIds.has(item.session.id) && (
                <svg width="9" height="9" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3.4" strokeLinecap="round" strokeLinejoin="round"><path d="M4.5 12.5l5 5 10-11" /></svg>
              )}
            </span>
          </button>
        ) : menuMode === "button" && (
          <button
            type="button"
            className="chat-sidebar-item-menu"
            aria-label={t.sticker.cardMenu}
            onClick={(event) => {
              event.stopPropagation(); // 点菜单不切会话
              const rect = event.currentTarget.getBoundingClientRect();
              openMenuAt(item, rect.right, rect.bottom + 4);
            }}
          ><MoreIcon /></button>
        )}
      </div>
    );
  };

  return (
    <aside className="chat-sidebar">
      {/* 窗口无系统装饰，侧栏顶部也要能拖动窗口——与右列标题栏同为 drag region。
          头部即「新建会话」按钮（Kimi 式整宽 CTA），不再放「会话」标题 + 迷你加号。 */}
      <div className="chat-sidebar-head" data-tauri-drag-region>
        <button
          type="button"
          className="chat-sidebar-new"
          onClick={() => void openNewSessionWindow().catch(() => {})}
        >
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round">
            <path d="M12 5v14M5 12h14" />
          </svg>
          <span>{t.sticker.newSession}</span>
        </button>
        <SidebarFilterMenu
          dirValue={dirFilter}
          dirOptions={dirOptions}
          groupMode={groupMode}
          groupLabels={{ none: t.chat.groupNone, dir: t.chat.groupByDir, date: t.chat.groupByDate, state: t.chat.groupByState }}
          sortMode={sortMode}
          sortLabels={{ recent: t.chat.sortRecent, created: t.chat.sortCreated, name: t.chat.sortName }}
          archived={archivedView}
          onDir={setDirFilter}
          onGroup={changeGroupMode}
          onSort={changeSortMode}
          onArchived={setArchivedView}
          t={t}
        />
        {/* macOS 专用：折叠钮留在侧栏头部（无独立顶栏）。Windows/Linux 的折叠/展开
            统一在顶栏左上角（ChatWindow 的 sidebarToggleButton），这里不再重复出口。 */}
        {isMac() && (
          <button
            type="button"
            className="chat-sidebar-toggle"
            aria-label={t.chat.sidebarCollapse}
            data-tip={t.chat.sidebarCollapse}
            onClick={onCollapse}
          >
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
              <path d="M15 6l-6 6 6 6" />
            </svg>
          </button>
        )}
      </div>
      {/* 会话搜索（Ctrl/Cmd+F 聚焦）：标题/仓库名下沉后端 LIKE，与看板同一条通道。 */}
      <div className="chat-sidebar-search">
        <input
          ref={searchInputRef}
          value={search}
          placeholder={t.chat.sidebarSearch}
          aria-label={t.chat.sidebarSearch}
          // IME 合成守卫：与看板搜索框同款，拼音中间态不下发。
          onChange={(e) => {
            if ((e.nativeEvent as InputEvent).isComposing) return;
            setSearch(e.target.value);
          }}
          onCompositionEnd={(e) => setSearch(e.currentTarget.value)}
          onKeyDown={(e) => {
            if (e.key === "Escape" && search) {
              e.stopPropagation(); // 有词时 Esc 只清词，不落到窗口级「拒绝审批」
              setSearch("");
            }
          }}
        />
        {search && (
          <button
            type="button"
            className="chat-sidebar-search-x"
            aria-label={t.chat.sidebarSearchClear}
            onClick={() => setSearch("")}
          >×</button>
        )}
      </div>
      <nav className="chat-sidebar-list" aria-label={t.chat.sidebarTitle} onScroll={onScroll}>
        {sessions === null && <div className="chat-sidebar-empty">{t.chat.sidebarLoading}</div>}
        {sessions !== null && sessions.length === 0 && (
          <div className="chat-sidebar-empty">
            {search.trim() ? t.chat.sidebarEmptySearch : dirFilter ? t.chat.sidebarEmptyDir : t.chat.sidebarEmpty}
          </div>
        )}
        {groups
          ? groups.map((group) => {
              const isFolded = folded.has(group.key);
              // 组头汇总点:折起来时它是这一组唯一的状态出口,取组内最强的召唤。
              const approval = group.items.some((item) => approvalAwaitingIds.has(item.session.id));
              const tone = group.items
                .map((item) => sessionTone(item.connected, item.session.status, item.pending_review, item.errored, item.busy_subagents))
                .reduce<SessionTone | null>((best, cur) => (best && TONE_RANK[best] >= TONE_RANK[cur] ? best : cur), null);
              const showDot = !!tone && TONE_RANK[tone] > 0;
              // 「未运行」= 已断开，且不是当前打开的这条，也不是置顶的那些——正开着的会话
              // 无论死活都得留在视野里（否则用户一进来就找不到自己在哪），置顶的更是：
              // 用户刚亲手把它顶上来，转头被折进「显示 N 个未运行会话」里等于白顶。
              const live = (item: LiveSession) =>
                item.connected || item.session.id === activeId || starred.has(item.session.cc_session_id);
              const idle = group.items.filter((item) => !live(item));
              // 折叠只在组里真有在跑的会话时才启用：它的使命是保住在跑的那几条不被
              // 历史挤出视野。仅因当前打开/置顶而有「活」条目的组（典型是「已结束」组，
              // 用户刚把当前会话切进去）不收——否则选中一条,其余几十条凭空消失。
              // 有搜索词时一律不折：用户明确在找某条会话,命中了却被藏进「显示 N 个
              // 未运行」等于告诉他「搜不到」。
              const canCollapse = !search.trim() && group.items.some((item) => item.connected) && idle.length > 0;
              const shown = canCollapse && !revealed.has(group.key) ? group.items.filter(live) : group.items;
              return (
                <div className="chat-sidebar-group" key={group.key}>
                  <button
                    type="button"
                    className="chat-sidebar-group-head"
                    aria-expanded={!isFolded}
                    data-tip={group.cwd ?? undefined}
                    onClick={() => toggleFolded(group.key)}
                  >
                    <ChevronDownIcon className={"chat-sidebar-caret" + (isFolded ? " is-folded" : "")} size={10} strokeWidth={2.6} />
                    <span className="chat-sidebar-group-name">{group.label || t.chat.sidebarNoDir}</span>
                    {approval
                      ? <i className="chat-sidebar-dot is-approval" role="status" aria-label={t.chat.sidebarApproval} data-tip={t.chat.sidebarApproval} />
                      : showDot && <i className={`chat-sidebar-dot is-${tone}`} role="status" aria-label={t.chat.status[tone]} data-tip={t.chat.status[tone]} />}
                    <span className="chat-sidebar-group-count">{group.items.length}</span>
                  </button>
                  {!isFolded && shown.map(renderItem)}
                  {/* 未运行的会话默认收起：一个跑了一阵的目录里，历史会话能堆到几十条，
                      把当前真正在跑的那一两条挤出视野。折起来但不丢——点一下就回来。 */}
                  {!isFolded && canCollapse && (
                    <button
                      type="button"
                      className="chat-sidebar-more"
                      aria-expanded={revealed.has(group.key)}
                      onClick={() => toggleRevealed(group.key)}
                    >
                      {revealed.has(group.key)
                        ? t.chat.sidebarHideIdle
                        : t.chat.sidebarShowIdle(idle.length)}
                    </button>
                  )}
                </div>
              );
            })
          : (ordered ?? []).map(renderItem)}
        {/* 下一页在路上。挂在真实在途状态上：翻页失败会清掉它，不会留下一个永远
            转不完的 loading 行。 */}
        {sessions !== null && sessions.length > 0 && growing && (
          <div className="chat-sidebar-empty">{t.chat.sidebarLoading}</div>
        )}
        {/* 对话内容全文搜索:LIKE 结果之外的第二段。显式按钮触发(扫 transcript 文件,
            不随击键);结果每会话一条(标题 + 命中摘录),点击直达。 */}
        {search.trim() && sessions !== null && (
          <div className="chat-sidebar-deep">
            {deepHits === null && !deepSearching && (
              <button type="button" className="chat-sidebar-more" onClick={runDeepSearch}>
                {t.chat.searchTranscripts}
              </button>
            )}
            {deepSearching && <div className="chat-sidebar-empty">{t.chat.searchTranscriptsRunning}</div>}
            {deepHits !== null && !deepSearching && (deepHits.length === 0
              ? <div className="chat-sidebar-empty">{t.chat.searchTranscriptsEmpty}</div>
              : <>
                  <div className="chat-sidebar-deep-head">{t.chat.searchTranscriptsHits}</div>
                  {deepHits.map((hit) => (
                    <button
                      type="button"
                      key={hit.sessionId}
                      className="chat-sidebar-deep-hit"
                      onClick={() => onSelect(hit.sessionId)}
                    >
                      <span className="chat-sidebar-deep-title">
                        <span className="chat-sidebar-name-text">{hit.title || t.sticker.waitingFirstInput}</span>
                        <span className="chat-sidebar-meta">{hit.projectName}</span>
                      </span>
                      <span className="chat-sidebar-deep-excerpt">{hit.excerpt}</span>
                    </button>
                  ))}
                </>)}
          </div>
        )}
      </nav>
      {/* 动作失败的唯一出口（重命名/便签/归档）：4s 后自动消失。 */}
      {actionError && <div className="chat-sidebar-error" role="status">{actionError}</div>}
      {/* 归档撤销条:8s 内可一键取回并跳回该会话(批量归档整批撤销)。 */}
      {archiveUndo != null && !actionError && (
        <div className="chat-sidebar-error is-undo" role="status">
          <span>{archiveUndo.length > 1 ? `${t.sticker.archivedNotice} × ${archiveUndo.length}` : t.sticker.archivedNotice}</span>
          <button type="button" onClick={() => undoArchive(archiveUndo)}>{t.sticker.archiveUndo}</button>
        </div>
      )}
      {/* 批量归档操作条:多选态常驻底部(Ctrl+点击增删,Esc 退出)。 */}
      {selectedIds.size > 0 && (
        <div className="chat-sidebar-batch" role="toolbar" aria-label={t.chat.selectedCount(selectedIds.size)}>
          <span>{t.chat.selectedCount(selectedIds.size)}</span>
          <button type="button" className="is-primary" onClick={archiveSelected}>{t.chat.archiveSelected}</button>
          <button type="button" onClick={() => setSelectedIds(new Set())}>{t.chat.cancelSelect}</button>
        </div>
      )}
      {lineageMenu && (
        <LineagePopover
          x={lineageMenu.x}
          y={lineageMenu.y}
          entries={lineageMenu.entries}
          onPick={onSelect}
          onClose={() => setLineageMenu(null)}
        />
      )}
      {ctxMenu && ctxItem && (
        <CardContextMenu
          x={ctxMenu.x}
          y={ctxMenu.y}
          starred={starred.has(ctxItem.session.cc_session_id)}
          hasNote={!!ctxItem.note}
          archived={ctxItem.archived}
          onStar={() => toggleStar(ctxItem.session.cc_session_id)}
          onNote={() => startNote(ctxItem)}
          onRename={() => startRename(ctxItem)}
          onArchive={() => toggleArchived(ctxItem)}
          onNewSession={() => void openNewSessionWindow({ cwd: ctxItem.cwd, provider: ctxItem.provider }).catch(() => {})}
          onOpenDir={ctxItem.cwd ? () => void openProjectDir(ctxItem.cwd!).catch(() => {}) : null}
          onAddDir={supportsExtraDir(ctxItem.provider) ? () => void addExtraDir(ctxItem) : null}
          // 结束会话只对本 GUI 托管的 PTY 开放（与看板同一门控）：外部终端里跑的会话
          // 杀不了，后台会话杀了也会被 supervisor 拉回来。
          onEndSession={ctxItem.pty_managed && !ctxItem.background ? () => endSession(ctxItem) : null}
          onClose={() => setCtxMenu(null)}
        />
      )}
      {/* 常驻底部：会话列表可能很长并滚动，设置入口不能跟着滚走。 */}
      <div className="chat-sidebar-footer">
        <button
          type="button"
          className="chat-sidebar-settings"
          onClick={() => void invoke("open_settings").catch(() => {})}
        >
          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7">
            <circle cx="12" cy="12" r="3.2" />
            <path d="M19.4 15a1.6 1.6 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.6 1.6 0 0 0-1.8-.3 1.6 1.6 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1A1.6 1.6 0 0 0 9 19.4a1.6 1.6 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.6 1.6 0 0 0 .3-1.8 1.6 1.6 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1A1.6 1.6 0 0 0 4.6 9a1.6 1.6 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.6 1.6 0 0 0 1.8.3H9a1.6 1.6 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.6 1.6 0 0 0 1 1.5 1.6 1.6 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.6 1.6 0 0 0-.3 1.8V9a1.6 1.6 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.6 1.6 0 0 0-1.5 1z" />
          </svg>
          <span>{t.sticker.openSettings}</span>
        </button>
      </div>
    </aside>
  );
}

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  getLiveSessionsCounts,
  getLiveSessionsPage,
  LiveSession,
  LiveSessionCounts,
  PageCursor,

} from "./api";
import { Sticker } from "./views/Sticker";
import { useBoardRefresh } from "./hooks/useBoardRefresh";
import { type Item, type Tab } from "./views/sticker/types";
import { CollapsedStrip } from "./views/CollapsedStrip";
import { useUpdate } from "./useUpdate";
import { useShowWhenReady } from "./useShowWhenReady";
import { isMacPanel } from "./platform";
import { reconcileRows, sameRefs, type RowCache as SharedRowCache } from "./rowCache";
import { reconcilePendings, type PendingRegistry } from "./views/sticker/pending";
import {
  initStickerWindowState,
  normalSize,
  updateStickerWindowState,
} from "./windowState";

type Edge = "left" | "right" | "top";
type Mode = "normal" | "collapsed" | "expanded";

const TAB_KEY = "meowo-tab";
const RELEASE_POLL_MS = 90; // 拖拽中轮询鼠标左键的间隔（检测真正松手）
const PAGE_SIZE = 100; // 贴纸会话每页条数，与首屏一致

// board-changed 常是「空转」：命令写库后 db-watcher 又为同一次写入报一次、liveness 轮询重发、
// 甚至 app 自身读库触碰 board.db-wal/-shm 的 mtime 也会被 watcher 当成变更而回声。这些刷新拉回的
// 数据与当前完全一致，若照旧整表替换成新对象引用，会让整个虚拟列表无谓重渲染（视觉上「一直在更新」）。
//
// 行级 JSON 缓存 + 结构共享：每行 stringify 一次、与上次的缓存比对，没变的行**复用旧对象引用**。
// 于是（1）空转刷新整表引用不变（sameRefs 命中，跳过 setState）；（2）只有一行变化时，其余行
// 引用稳定，配合卡片层的 memo 只重渲染那一张。全字段 JSON 比较是刻意的**排除法**——按字段挑
// 白名单的签名方案会在漏字段时让 UI 静默不更新（sameHistoryMeta 的注释记过三回同类事故）。
// 缓存只增不清：键是 session.id，量级为窗口生命周期内见过的会话数，几百条小对象，不值得管理。
// 实现移到 ../rowCache 与对话窗侧栏共用（那边此前缺这层，几百条时每次空转刷新都全表重建）。
type RowCache = SharedRowCache<Item>;

// 缩略条主轴逻辑长度：按 connected 点数贴合内容（点 10px + 间距 7px = 17，两端留白 26），最小 48。
// 仅作折叠初值，CollapsedStrip 挂载后会按真实 DOM 尺寸精确校正。
// 上限 24 点（W-5）：点数随会话数线性增长会超出工作区被裁掉；真实容纳上限由 CollapsedStrip
// 按屏幕尺寸换算，这里只是别让初值先撑出一个超屏窗口。
function stripExtent(count: number): number {
  return Math.max(48, Math.min(count, 24) * 17 + 26);
}

/** 当前 tab 对应的总会话数（用于 hasMore 与加载守卫，必须与后端 filter 语义一致）。 */
function totalFor(filter: Tab, counts: LiveSessionCounts): number {
  switch (filter) {
    case "running":
      return counts.running;
    case "waiting":
      return counts.waiting;
    case "all":
      // "all" tab 后端过滤为 archived=0
      return counts.total - counts.archived;
  }
}



export function App() {
  // 启动显示闸门（W-4/W-17）：贴纸几何（尺寸/吸附边/置顶）在 settings.json，需一次 IPC 读取；
  // 读完并完成「沿用折叠 / 还原正常尺寸」之前不渲染内容、不显示窗口——否则闪一帧
  // 「默认尺寸大框里的错误内容」。后端 show_after_grace 兜底不变。
  const [bootReady, setBootReady] = useState(isMacPanel());
  // 贴纸窗口以 visible:false 创建（tauri.conf.json），首帧渲染后再显示，消除启动瞬间的白框闪烁。
  // 不抢焦点（窗口配置 focus:false，开机自启同理）；macOS 面板模式显隐归 menubar 管，这里不越权。
  useShowWhenReady({ focus: false, enabled: !isMacPanel(), ready: bootReady });
  const [items, setItems] = useState<Item[]>([]);
  const [counts, setCounts] = useState<LiveSessionCounts>({
    total: 0,
    running: 0,
    waiting: 0,
    archived: 0,
  });
  const [loadingMore, setLoadingMore] = useState<boolean>(false);
  const [reachedEnd, setReachedEnd] = useState<boolean>(false);
  // 冷启动首页加载：未落地前 initialLoading=true（Sticker 显示加载占位而非假空态）；
  // 首页/刷新失败置 loadError（Sticker 显示「加载失败 + 重试」），任一首页型加载成功后清除。
  const [initialLoading, setInitialLoading] = useState<boolean>(true);
  const [loadError, setLoadError] = useState<boolean>(false);
  // 重试：递增 nonce 重新触发下方的 filter/search 首页加载 effect。
  const [retryNonce, setRetryNonce] = useState(0);
  const retryLoad = useCallback(() => setRetryNonce((n) => n + 1), []);
  // 「已归档」不再是看板 tab（管理入口在设置 → 会话）；持久化里存过它的自动退回「全部」。
  const [filter, setFilter] = useState<Tab>(() => {
    const s = localStorage.getItem(TAB_KEY);
    return s === "waiting" || s === "running" ? s : "all";
  });
  const [search, setSearch] = useState("");
  // 搜索结果是临时视图，不能覆盖用户搜索前已经加载好的普通列表（包括它的服务端顺序）。
  // 按 tab 分开缓存，避免搜索中切 tab/清空时拿另一个 tab 的列表来恢复。
  const unsearchedItemsRef = useRef<Partial<Record<Tab, Item[]>>>({});
  // 切 tab/搜索的首页请求在途（B-4）：Sticker 据此保留旧列表灰化直到新数据就绪，
  // 不再先渲染「旧数据按新条件过滤出的子集」（waiting 的 ASC 到达后整列表翻转的来源）。
  const [switching, setSwitching] = useState(false);
  const pickFilter = useCallback((f: Tab) => {
    if (f !== filter) setSwitching(true);
    setFilter(f);
    localStorage.setItem(TAB_KEY, f);
  }, [filter]);
  const [edge, setEdge] = useState<Edge | null>(null); // 吸附边：启动时从 settings 读入（见底部启动 effect）
  // mode 初值 normal：真正的初始态在 settings 读取后确定（edge 同源）；bootReady 闸门前不渲染，
  // 不会闪现错误形态。macOS 面板模式恒 normal（无吸边）。
  const [mode, setMode] = useState<Mode>("normal");
  // 窗口置顶偏好（W-17）：存 settings.json，启动时读入；切换经 togglePin 原子落盘。
  const [pinned, setPinned] = useState(false);
  const pinnedRef = useRef(false); // 同步镜像：recall/snap_restore 等回调里取最新值不依赖闭包
  const applyPinned = useCallback((p: boolean) => {
    pinnedRef.current = p;
    setPinned(p);
  }, []);
  const togglePin = useCallback(() => {
    applyPinned(!pinnedRef.current);
    updateStickerWindowState({ pinned: pinnedRef.current });
  }, [applyPinned]);
  const [glow, setGlow] = useState<Edge | null>(null); // 拖拽中靠近边缘的发光提示
  // 展开过渡中（W-4）：为 true 时渲染 .snap-expanding 纯色占位而非看板/缩略条——
  // snap_expand/snap_restore 落地前窗口仍是 28px 细条几何，全量看板此刻渲染会被
  // 压进细条宽度造成布局抖动；尺寸落地（置回 false）后再恢复按 mode 判定。
  const [expanding, setExpanding] = useState(false);
  // 只读检查：仅驱动贴纸设置钮上的更新红点；下载/安装由更新窗口（views/Updater）全权负责。
  const { status: upStatus } = useUpdate({ automatic: true });

  // 折叠条恒显示全部「连接中」会话（running + waiting），与当前选中 tab 无关——
  // 故独立于分页 items 单独加载（按状态查，覆盖旧但仍连接的会话，不受 tab/分页窗口影响）。
  const [stripSessions, setStripSessions] = useState<Item[]>([]);
  const stripRowCacheRef = useRef<RowCache>(new Map());
  const loadStrip = useCallback(() => {
    Promise.all([
      // includeForeign:折叠条与主列表同视野——dev 构建下安装版的连接中会话也计入。
      getLiveSessionsPage("running", null, null, 200, null, true),
      getLiveSessionsPage("waiting", null, null, 200, null, true),
    ])
      .then(([r, w]) => {
        const map = new Map<number, Item>();
        [...r.items, ...w.items].forEach((s) => map.set(s.session.id, s as Item));
        const next = reconcileRows(stripRowCacheRef.current, [...map.values()]);
        setStripSessions((prev) => (sameRefs(prev, next) ? prev : next));
      })
      .catch(() => {});
  }, []);
  useEffect(() => {
    // macOS 面板模式没有吸边/折叠，缩略条永不渲染，这 2×200 条的查询是纯浪费。
    if (isMacPanel()) return;
    loadStrip();
  }, [loadStrip]);

  const connectedCount = useMemo(
    () => stripSessions.filter((l) => !l.archived && l.connected).length,
    [stripSessions]
  );

  const modeRef = useRef(mode);
  modeRef.current = mode;
  const edgeRef = useRef(edge);
  edgeRef.current = edge;
  const countRef = useRef(connectedCount);
  countRef.current = connectedCount;
  const draggingRef = useRef(false); // 是否正在拖拽窗口（mousedown 命中拖拽区）
  const dragStartPosRef = useRef<{ x: number; y: number } | null>(null); // 按下时的窗口位置（物理像素）
  const lastEdgeRef = useRef<Edge | null>(null); // 最近一次 Moved 检测到的边
  const settleRef = useRef<number | null>(null);
  const preResizeEdgeRef = useRef<Edge | null>(null); // 拖角缩放前的吸附边，缩放结束后据此恢复

  // 请求序号守卫：并发刷新时旧响应可能晚于新响应返回，仅当自己仍是最新一次请求才写入。
  const refreshSeqRef = useRef(0);
  // 列表行的结构共享缓存（与折叠条的各自独立，两边行对象不混用）。
  const itemsRowCacheRef = useRef<RowCache>(new Map());
  // 下一页游标：**只认后端响应里带回的扫描位置**。列表做过 connected-first 排序，
  // 末项不是本页时间上最旧的一条，从 items 里自己推游标会重复/漏页。
  const nextCursorRef = useRef<PageCursor | null>(null);
  // 「正在启动」占位卡（pending PTY，负 id）的登记簿：后端在首页查询里合成占位项、
  // claim 认领后摘除；这里按 provider+cwd 对账（见 sticker/pending.ts），保证
  // 「真卡出现即撤占位、启动失败不留僵尸卡」，并兜住 mergeTail 把旧占位留在尾部的缝隙。
  const pendingsRef = useRef<PendingRegistry>(new Map());
  const loadPage = useCallback(
    async (
      filter: Tab,
      search: string,
      cursor: PageCursor | null,
      limit: number = PAGE_SIZE,
      // board-changed 增量刷新（B-6）：首页权威（刚有事件变化的会话按 last_event_at
      // 必然排在前列，状态/排序迁移都在首页里反映），已滚出首页的旧行按 id 保留，
      // 不再整窗重查（500 条时每事件重查 500 行）。仅 cursor === null 时有效。
      mergeTail: boolean = false
    ): Promise<{ page: Item[]; cursor: PageCursor | null; applied: boolean }> => {
      const seq = ++refreshSeqRef.current;
      // counts 只在首页/刷新（cursor === null）时才需要；loadMore 复用已有 counts，
      // 避免频繁 loadMore 时对 counts 做纯重复的后端查询（审查发现）。
      const needCounts = cursor === null;
      try {
        const [countsRes, res] = await Promise.all([
          needCounts ? getLiveSessionsCounts() : Promise.resolve(null),
          // includeForeign:贴纸看板是监控视野的主入口,dev 构建下聚合安装版的会话
          // (外库卡只读展示,后端仅在首页附加,翻页游标不受影响)。
          // includePending:同入口附带「正在启动」占位卡(pending PTY,负 id,同样只进首页)。
          getLiveSessionsPage(filter, search, cursor, limit, null, true, true),
        ]);
        const page = res.items;
        const applied = seq === refreshSeqRef.current;
        if (!applied) return { page: page as Item[], cursor: res.next_cursor, applied };
        nextCursorRef.current = res.next_cursor;
        // 占位卡对账（仅首页响应；loadMore 的响应里没有占位项，既有登记原样保留）。
        // 对账后占位从 page 剥离、改由登记簿统一前置——否则 mergeTail 会把上一轮
        // 的占位行当「未进首页的旧行」永远留在尾部。
        const pendings = cursor === null
          // 占位行同样过行级结构共享：后端每次刷新都重新合成占位项，内容没变时
          // 复用旧引用，否则占位在场期间每次 board-changed 都整表重渲染（见 reconcileRows）。
          ? reconcileRows(
              itemsRowCacheRef.current,
              reconcilePendings(pendingsRef.current, page as Item[], Date.now())
            )
          : [];
        const pageReals = cursor === null ? page.filter((l) => l.session.id > 0) : page;
        if (countsRes) {
          setCounts((prev) =>
            prev.total === countsRes.total &&
            prev.running === countsRes.running &&
            prev.waiting === countsRes.waiting &&
            prev.archived === countsRes.archived
              ? prev
              : countsRes
          );
        }

        setItems((prev) => {
          if (cursor === null) {
            // 首页请求（切 tab / 首次加载 / board-changed 刷新）：直接按服务端顺序整体替换。
            // 服务端已按 last_event_at DESC 排序，天然反映既有会话的最新排序位置——
            // 若只按 prev 数组旧位置合并、仅将全新会话插到最前，已存在会话（如恢复的旧会话）
            // 排序键变化后不会移动，得等用户手动切 tab 才会跳到正确位置（回归 bug）。
            // 不在 page 里的会话（状态迁移出当前 filter/归档/删除）也随整体替换自然被移除。
            // 占位卡（负 id）不走 page——它由登记簿对账后统一前置（见上面 pendings）。
            const next = reconcileRows(itemsRowCacheRef.current, pageReals as Item[]);
            if (mergeTail && prev.length > next.length) {
              // 增量修补（B-6）：首页权威置顶，未进首页的旧行原样保留在尾部（这些会话
              // 近期无事件，展示状态不会变）；合并后按 connected 稳定分区恢复全局不变量
              // （与 loadMore 合并同规则）。上一轮的占位行（负 id）不属于「旧行」，
              // 必须排除——它的去留只由登记簿决定。
              const fresh = new Set(next.map((l) => l.session.id));
              const merged = [...next, ...prev.filter((l) => l.session.id > 0 && !fresh.has(l.session.id))];
              const kept = [...pendings, ...merged.filter((l) => l.connected), ...merged.filter((l) => !l.connected)];
              if (!search.trim()) unsearchedItemsRef.current[filter] = kept;
              return sameRefs(prev, kept) ? prev : kept;
            }
            const withPendings = [...pendings, ...next];
            if (!search.trim()) unsearchedItemsRef.current[filter] = withPendings;
            // 空转刷新（数据未变）保持原引用，跳过整表重渲染，消除视觉抖动（见 reconcileRows）。
            return sameRefs(prev, withPendings) ? prev : withPendings;
          }
          // loadMore（cursor 非空）：按 id 合并，保留已加载会话原有顺序，新条目追加到末尾。
          // 先过结构共享：新页里与已加载行内容相同的，合并后保持旧引用。
          const reconciled = reconcileRows(itemsRowCacheRef.current, page as Item[]);
          const map = new Map(prev.map((l) => [l.session.id, l]));
          const append: Item[] = [];
          for (const l of reconciled) {
            if (!map.has(l.session.id)) append.push(l);
            map.set(l.session.id, l);
          }
          const merged = [...prev.map((l) => map.get(l.session.id)!), ...append];
          // connected-first 是**每页内**排序（session_query 对单页做），直接追加会把第 2 页
          // 的已连接会话排到第 1 页断开会话之下——活跃会话散落在历史会话之间，越滚越乱。
          // 合并后按 connected 稳定分区恢复全局不变量（两组内部保持原有相对顺序）。
          const next = [...merged.filter((l) => l.connected), ...merged.filter((l) => !l.connected)];
          if (!search.trim()) unsearchedItemsRef.current[filter] = next;
          return next;
        });
        return { page: page as Item[], cursor: res.next_cursor, applied };
      } catch (err) {
        console.error("[loadPage] 加载失败：", err);
        throw err;
      }
    },
    []
  );

  // 一次刷新：已加载窗口超过首页时只重查首页（PAGE_SIZE）+ 增量修补尾部（B-6，不再
  // 每次事件整窗重查 500 行）。刚有事件的会话按 last_event_at 必排在首页，状态/排序
  // 变化都被首页权威覆盖；无事件的尾部旧行原样保留，用户已滚动加载的窗口不被打回
  // 第一页（P0 的诉求改由 mergeTail 保住）。窗口未超首页时首页就是整窗，整体替换
  // （filter 迁出的会话随之移除，与 loadMore 前行为一致）。
  // 例外：归档失败回滚必须整窗重查——被乐观移除的卡可能深在尾部，首页 + 增量修补
  // 捞不回来（fullRefreshOnceRef 由 onArchiveFailed 置位）。
  const itemsLenRef = useRef(0);
  itemsLenRef.current = items.length;
  const fullRefreshOnceRef = useRef(false);
  // 首页/搜索视图正在切换时，旧 items 仍会短暂留在 DOM。此时虚拟列表若触发 loadMore，
  // 会拿旧列表游标发起一个更新请求，取消首页请求并把结果按旧顺序合并（清空搜索排序错乱的根因）。
  const resettingPageRef = useRef(false);
  const pageResetSeqRef = useRef(0);
  const doRefresh = useCallback(() => {
    const resetSeq = ++pageResetSeqRef.current;
    resettingPageRef.current = true;
    const full = fullRefreshOnceRef.current;
    fullRefreshOnceRef.current = false;
    const incremental = !full && itemsLenRef.current > PAGE_SIZE;
    // 增量刷新不动 reachedEnd：尾部原样保留，此前「已到底」依然成立。
    if (!incremental) setReachedEnd(false);
    loadPage(filter, search, null, incremental ? PAGE_SIZE : Math.max(PAGE_SIZE, itemsLenRef.current), incremental)
      .then(({ cursor, applied }) => {
        if (applied) {
          setInitialLoading(false);
          setLoadError(false);
          if (resetSeq === pageResetSeqRef.current) {
            resettingPageRef.current = false;
            // 切换期的首页请求可能被这次更新的 board-changed 刷新取代（applied=false 永不
            // 落地）——切换灰化（B-4）在这里也要解除，否则列表永远灰着。
            setSwitching(false);
          }
          if (cursor === null) setReachedEnd(true);
        }
      })
      .catch(() => {
        if (resetSeq === pageResetSeqRef.current) {
          setInitialLoading(false);
          setLoadError(true);
          resettingPageRef.current = false;
          setSwitching(false);
        }
      });
    // 折叠条数据独立刷新（不随 tab）——但只在非 normal 态：正常态下缩略条不渲染，
    // 每次 board-changed 都跟着拉 2×200 条只为一个折叠时才用的计数，不值。挂载时和
    // 折叠动作发起时（doCollapse）各拉一次，保证真折叠的那一刻数据是新的。
    if (modeRef.current !== "normal") loadStrip();
  }, [filter, search, loadPage, loadStrip]);

  // board-changed 订阅 + leading/trailing 节流统一在 useBoardRefresh：每次刷新是
  // counts + 一整页 + 折叠条两查询，页大小随滚动增长，值得省。E2E 观测点也在 hook 里。
  const refresh = useBoardRefresh(doRefresh);

  // loadingMore 是 state：setLoadingMore(true) 到下次渲染落地之间，同一 tick 内 loadMore 仍可按
  // 旧闭包重入（Sticker 触底 effect 在一个渲染批内可能连发），以相同游标重复请求下一页。
  // ref 镜像同步置位，重入当场被拒（与 useLoginOperations 的 pendingRef 同一套路）。
  const loadingMoreRef = useRef(false);
  const loadMore = useCallback(async () => {
    if (resettingPageRef.current || loadingMoreRef.current || reachedEnd) return;
    // 游标必须用后端上次带回的扫描位置：从可见列表末项推会撞上 connected-first 排序
    // （末项不是时间上最旧的一条），每页都会重复返回被顶到前排的会话。
    const cursor = nextCursorRef.current;
    if (cursor === null) {
      setReachedEnd(true);
      return;
    }
    loadingMoreRef.current = true;
    setLoadingMore(true);
    try {
      const { cursor: next, applied } = await loadPage(filter, search, cursor);
      // 请求过程中若已被更新的请求（如切 tab/刷新）取代，本次结果不再代表当前 tab 的状态，
      // reachedEnd 不应据此更新，否则可能把旧 tab 的「已到底」误写到新 tab 上（审查发现的竞态）。
      if (applied && next === null) {
        setReachedEnd(true);
      }
    } catch (err) {
      console.error("[loadMore] 加载失败：", err);
    } finally {
      loadingMoreRef.current = false;
      setLoadingMore(false);
    }
  }, [filter, search, reachedEnd, loadPage]);

  // filter / search 变化：重置到首页（search 变化去抖 300ms，避免每次按键都打一次后端；
  // filter 切换无需去抖，0ms 立即加载，含首次挂载）。取代原先仅 [filter, loadPage] 的切 tab effect。
  useEffect(() => {
    const t = window.setTimeout(() => {
      const resetSeq = ++pageResetSeqRef.current;
      resettingPageRef.current = true;
      setReachedEnd(false);
      // 清空搜索后覆盖搜索前已经加载的窗口，而不是退回固定首屏；否则列表尾部会丢失，
      // 用户看到的原列表顺序/滚动窗口也会被搜索操作改变。
      const limit = search.trim()
        ? PAGE_SIZE
        : Math.max(PAGE_SIZE, unsearchedItemsRef.current[filter]?.length ?? 0);
      loadPage(filter, search, null, limit)
        .then(({ cursor, applied }) => {
          if (applied) {
            setInitialLoading(false);
            setLoadError(false);
            if (resetSeq === pageResetSeqRef.current) {
              resettingPageRef.current = false;
              setSwitching(false);
            }
            if (cursor === null) setReachedEnd(true);
          }
        })
        .catch(() => {
          if (resetSeq === pageResetSeqRef.current) {
            setInitialLoading(false);
            setLoadError(true);
            resettingPageRef.current = false;
            setSwitching(false);
          }
        });
    }, search ? 300 : 0);
    return () => window.clearTimeout(t);
  }, [filter, search, loadPage, retryNonce]);

  const changeSearch = useCallback((next: string) => {
    if (search.trim() !== next.trim()) {
      resettingPageRef.current = true;
      setSwitching(true);
    }
    // 后端的无搜索请求回来前就恢复原数组，既不让搜索结果继续占位，也完整保留原顺序。
    // 同时使仍在途的搜索请求失效，防止它在清空后短暂覆盖恢复出的列表。
    if (search.trim() && !next.trim()) {
      refreshSeqRef.current += 1;
      const cached = unsearchedItemsRef.current[filter];
      if (cached) {
        setItems(cached);
        // 缓存恢复即目标列表就位，无需切换灰化（B-4）：否则在途的后端请求不返回时，
        // 列表会一直冻在搜索结果上。
        setSwitching(false);
      }
    }
    setSearch(next);
  }, [filter, search]);

  // 折叠即卸载 Sticker，searchOpen（Sticker 局部态）随之丢失；若搜索词留在这里，
  // 再展开时列表仍被后端过滤、底栏却没有搜索框，空结果会被误读成「会话全丢了」——折叠时清空。
  // 走 changeSearch 的完整清空路径（恢复搜索前列表 + 作废在途搜索请求）。
  useEffect(() => {
    if (mode === "collapsed" && search) changeSearch("");
  }, [mode, search, changeSearch]);

  // 归档/取消归档会改变当前 tab 的可见性：乐观从列表移除该卡片并调整 counts，卡片即刻消失。
  // 这里不能顺手 refresh()——refresh 是前沿触发，会与尚未落库的 set_archived 赛跑，抢先拉回旧数据
  // 把乐观更新冲掉，卡片闪一下又回来。成功路径无需自己刷：后端 set_archived 写库后会发 board-changed，
  // 届时 counts/列表被真实数据校正。失败路径由 onArchiveFailed 显式拉回。
  const onArchiveOptimistic = useCallback(
    (sessionId: number) => {
      setItems((prev) => prev.filter((l) => l.session.id !== sessionId));
      // 看板上只可能发生「归档」（已归档的不再上板），archived 计数恒 +1。
      setCounts((prev) => ({ ...prev, archived: prev.archived + 1 }));
    },
    []
  );

  // 归档失败：乐观移除的卡片必须回来，否则用户以为归档成功了。此刻后端未改动，refresh 拉到的就是真实态。
  // 置位整窗重查：常规刷新是首页 + 增量修补，捞不回被移除后深在尾部的卡（见 doRefresh）。
  const onArchiveFailed = useCallback(() => {
    fullRefreshOnceRef.current = true;
    refresh();
  }, [refresh]);

  // 记住当前窗口位置（物理像素）：window-state 插件已不再管 main（lib.rs denylist），
  // 位置随拖拽落点/找回居中等时机经这里原子落盘进 settings（W-17）。
  const persistPosition = useCallback(() => {
    try {
      getCurrentWindow()
        .outerPosition()
        .then((p) => updateStickerWindowState({ x: p.x, y: p.y }))
        .catch(() => {});
    } catch {
      /* 非 Tauri 环境（测试/浏览器）忽略 */
    }
  }, []);

  // 托盘「找回贴纸」：把贴纸拉回主屏中央并**临时**置顶（recall_center 聚焦后按 pin 偏好还原）。
  // 折叠/吸附态先展开还原成正常窗口，再居中置顶。不改写用户的 pin 偏好（W-6）——找回只是
  // 「带到眼前」，不是「替用户决定以后都置顶」。macOS 面板模式无吸边/托盘菜单项，跳过。
  useEffect(() => {
    if (isMacPanel()) return;
    const recall = async () => {
      if (modeRef.current !== "normal") {
        const { w, h } = normalSize();
        updateStickerWindowState({ snap_edge: null });
        setEdge(null);
        // 先铺过渡占位（W-4）：窗口此刻还是细条几何，直接渲染看板会压出布局抖动。
        setExpanding(true);
        setMode("normal");
        try {
          await invoke("snap_restore", { width: w, height: h, pinned: pinnedRef.current });
        } catch (err) {
          console.error("[recall] snap_restore 失败：", err);
        } finally {
          setExpanding(false);
        }
      }
      try {
        await invoke("recall_center", { pinned: pinnedRef.current });
        persistPosition(); // 居中后的落点即最新正常位置
      } catch (err) {
        console.error("[recall] recall_center 失败：", err);
      }
    };
    const un = listen("recall-sticker", () => void recall());
    return () => {
      un.then((f) => f());
    };
  }, [persistPosition]);

  // 折叠成缩略条：厚度固定，主轴长度贴合当前点数。
  // 顺带刷一次折叠条数据：normal 态下它不随 board-changed 刷新（见 doRefresh），
  // 折叠发起的这一刻把它补新——初值尺寸用当前 countRef，落地后 CollapsedStrip 会精确校正。
  const doCollapse = useCallback(
    (d: Edge) => {
      loadStrip();
      return invoke("snap_collapse", { edge: d, extent: stripExtent(countRef.current) });
    },
    [loadStrip]
  );

  // 拖拽松手处理：靠边→折叠（从 normal 先存正常尺寸）；离边→若在吸附态则还原普通窗口。
  // macOS 面板模式：直接返回，不处理吸边逻辑。
  const handleDragRelease = useCallback(async () => {
    if (isMacPanel()) return;
    if (!draggingRef.current) return;
    draggingRef.current = false;
    document.documentElement.classList.remove("win-dragging");
    if (settleRef.current) {
      window.clearInterval(settleRef.current);
      settleRef.current = null;
    }
    setGlow(null);
    // 纯点击守卫：按住拖拽区但没真拖动（想「抓住窗口」的第一下、或单纯点了下标题区）。
    // lastEdgeRef 里可能躺着 snap_collapse/expand 自己 set_position 触发的陈旧边——
    // 不判位移的话，展开态点一下拖拽条就被当场折叠回去（实拍过的困惑）。
    // 位移 < 4 物理像素视为点击，不做任何吸附/还原判定。
    try {
      const start = dragStartPosRef.current;
      dragStartPosRef.current = null;
      if (start) {
        const p = await getCurrentWindow().outerPosition();
        if (Math.abs(p.x - start.x) + Math.abs(p.y - start.y) < 4) return;
      }
    } catch {
      /* 非 Tauri 环境：按发生过位移处理（旧行为） */
    }
    const d = lastEdgeRef.current;
    const m = modeRef.current;
    if (d) {
      // 靠边松手 → 折叠
      if (m === "normal") {
        // 从正常态首次吸附：把当前正常尺寸/位置与吸附边一并记住（单次原子落盘，W-17）。
        try {
          const win = getCurrentWindow();
          const [sz, pos, sf] = await Promise.all([
            win.outerSize(),
            win.outerPosition(),
            win.scaleFactor(),
          ]);
          updateStickerWindowState({
            normal_width: sz.width / sf,
            normal_height: sz.height / sf,
            x: pos.x,
            y: pos.y,
            snap_edge: d,
          });
        } catch {
          updateStickerWindowState({ snap_edge: d });
        }
      }
      if (m !== "collapsed") {
        // expanded 态拖回边缘：记住新边（collapsed 态拖条不写——旧语义：条的边以折叠时为准）。
        if (m !== "normal") updateStickerWindowState({ snap_edge: d });
        try {
          await doCollapse(d);
          setEdge(d);
          setMode("collapsed");
        } catch (err) {
          console.error("[snap] snap_collapse 失败：", err);
        }
      }
    } else if (m !== "normal") {
      // 离边松手 → 还原普通窗口
      const { w, h } = normalSize();
      try {
        await invoke("snap_restore", { width: w, height: h, pinned: pinnedRef.current });
        updateStickerWindowState({ snap_edge: null });
        setEdge(null);
        setMode("normal");
        persistPosition(); // 还原钳位后的落点即最新正常位置
      } catch (err) {
        console.error("[snap] snap_restore 失败：", err);
      }
    } else {
      // 正常态拖动落点：记住位置（插件不再管 main，见 lib.rs denylist）。
      persistPosition();
    }
  }, [doCollapse, persistPosition]);

  // 用户拖边框缩放窗口（后端 WM_SIZING/WM_EXITSIZEMOVE 检测）：
  // - 缩放开始(user-resized)：暂解除吸附变普通窗口，避免缩放中被吸附逻辑误折叠抖动；记住原吸附边。
  // - 缩放结束(user-resize-end)：若缩放前是吸附的，按新尺寸重新吸回原来那条边（保留新尺寸）。
  useEffect(() => {
    if (isMacPanel()) return;
    const unStart = listen("user-resized", () => {
      if (modeRef.current === "normal") return; // 本就普通态，无吸附可解
      preResizeEdgeRef.current = edgeRef.current; // 记住缩放前的吸附边
      invoke("unsnap", { pinned: pinnedRef.current }).catch(() => {});
      updateStickerWindowState({ snap_edge: null });
      setEdge(null);
      setMode("normal");
    });
    const unEnd = listen("user-resize-end", async () => {
      const e = preResizeEdgeRef.current;
      preResizeEdgeRef.current = null;
      if (!e) {
        // 缩放前非吸附态 → 保持普通窗口；把新尺寸/位置记为常用几何，供启动还原沿用，
        // 避免用户自定义的小窗口被误判为「吸附遗留细条」而被强行放大。
        try {
          const win = getCurrentWindow();
          const [sz, pos, sf] = await Promise.all([
            win.outerSize(),
            win.outerPosition(),
            win.scaleFactor(),
          ]);
          updateStickerWindowState({
            normal_width: sz.width / sf,
            normal_height: sz.height / sf,
            x: pos.x,
            y: pos.y,
          });
        } catch {
          /* ignore */
        }
        return;
      }
      // 把缩放后的尺寸记为新的常用尺寸，再按它重新吸回原边（snap_expand 会贴边定位）。
      // 后端 sanitize 会钳到最小尺寸：细条/异常尺寸进不了「正常尺寸」基准（W-17 起折叠态
      // 禁 resizable，细条已拖不出来，这里是纵深防御）。
      try {
        const sz = await getCurrentWindow().outerSize();
        const sf = await getCurrentWindow().scaleFactor();
        updateStickerWindowState({
          normal_width: sz.width / sf,
          normal_height: sz.height / sf,
          snap_edge: e,
        });
      } catch {
        updateStickerWindowState({ snap_edge: e });
      }
      const { w, h } = normalSize();
      setEdge(e);
      setMode("expanded");
      invoke("snap_expand", { edge: e, width: w, height: h }).catch((err) =>
        console.error("[snap] 缩放后重新吸附失败：", err)
      );
    });
    return () => {
      unStart.then((f) => f());
      unEnd.then((f) => f());
    };
  }, []);

  // 监听窗口移动：拖拽中靠近边缘则发光，并轮询鼠标左键——真正松手才吸附。
  // data-tauri-drag-region 的 OS 拖动循环里 webview 收不到 mouseup，故问后端键状态；
  // 用 setInterval(而非定时兜底)，即使停在边缘不动也会持续轮询、松手即触发，不会因停顿误吸。
  // macOS 面板模式无吸边，不注册此监听器。
  useEffect(() => {
    if (isMacPanel()) return;
    const un = listen<{ edge: Edge | null }>("snap-changed", (e) => {
      const d = e.payload.edge;
      lastEdgeRef.current = d;
      if (!draggingRef.current) {
        setGlow(null);
        return;
      }
      setGlow(d);
      if (settleRef.current) return; // 轮询已在跑，无需重复启动
      settleRef.current = window.setInterval(() => {
        if (!draggingRef.current) {
          if (settleRef.current) window.clearInterval(settleRef.current);
          settleRef.current = null;
          return;
        }
        invoke<boolean>("pointer_left_down")
          .then((down) => {
            if (down) return; // 仍按着，继续等
            if (settleRef.current) window.clearInterval(settleRef.current);
            settleRef.current = null;
            void handleDragRelease();
          })
          .catch(() => {});
      }, RELEASE_POLL_MS);
    });
    return () => {
      un.then((f) => f());
      // 拖拽中途卸载也要停掉松手轮询：否则 90ms 的 pointer_left_down IPC 轮询随组件泄漏、
      // 卸载后仍持续空转（与 handleDragRelease 里的清理保持一致）。
      if (settleRef.current) {
        window.clearInterval(settleRef.current);
        settleRef.current = null;
      }
    };
  }, [handleDragRelease]);

  // 拖拽开始/结束检测：mousedown 命中拖拽区 → 标记拖拽；mouseup → 处理松手。
  // macOS 面板模式：无拖拽/吸边，不注册此监听器。
  useEffect(() => {
    if (isMacPanel()) return;
    const onDown = (ev: MouseEvent) => {
      const t = ev.target as HTMLElement | null;
      if (t && t.closest("[data-tauri-drag-region]")) {
        // 双击拖拽区会触发 Tauri 默认的窗口最大化，贴纸不该被最大化 → 在 capture 阶段拦掉。
        if (ev.detail >= 2) {
          ev.preventDefault();
          ev.stopPropagation();
          return;
        }
        draggingRef.current = true;
        // 记录按下时的窗口位置：松手时按位移区分「真拖动」与「纯点击」（见 handleDragRelease）。
        // try 包住同步段：窗口桩缺 outerPosition 时会在事件处理器里直接抛（测试环境实测），
        // 拿不到起点就保持 null——handleDragRelease 对 null 按纯点击处理，行为不变。
        dragStartPosRef.current = null;
        try {
          getCurrentWindow().outerPosition()
            .then((p) => { dragStartPosRef.current = { x: p.x, y: p.y }; })
            .catch(() => {});
        } catch { /* 见上 */ }
        // 拖拽全程给 <html> 挂 class，驱动拖拽条放大——:active 在 OS 拖动接管后会丢失，不可靠。
        document.documentElement.classList.add("win-dragging");
      }
    };
    const onUp = () => {
      void handleDragRelease();
    };
    window.addEventListener("mousedown", onDown, true);
    window.addEventListener("mouseup", onUp, true);
    return () => {
      window.removeEventListener("mousedown", onDown, true);
      window.removeEventListener("mouseup", onUp, true);
    };
  }, [handleDragRelease]);

  // CollapsedStrip 测量到的真实内容尺寸 → 精确调整缩略条主轴长度（贴合、无滚动条）。
  // 最近一次实测 extent 存 ref：DPI/显示器变化时按它重跑 snap_collapse（见下方 onScaleChanged）。
  const lastStripExtentRef = useRef<number | null>(null);
  const onMeasure = useCallback((ext: number) => {
    lastStripExtentRef.current = ext;
    if (modeRef.current === "collapsed" && edgeRef.current) {
      invoke("snap_collapse", { edge: edgeRef.current, extent: ext }).catch((err) =>
        console.error("[snap] 调整缩略条尺寸失败：", err)
      );
    }
  }, []);

  // W-5：collapsed 态下 DPI/显示器变化（拖到另一块屏、系统改缩放）后，条的物理尺寸与贴边
  // 位置全部按旧 scale 算出、已失真。监听 onScaleChanged，用上次实测 extent 重跑 snap_collapse，
  // 后端按新显示器的 scale/工作区重算尺寸与位置。
  useEffect(() => {
    if (isMacPanel()) return;
    let un: (() => void) | undefined;
    try {
      getCurrentWindow()
        .onScaleChanged(() => {
          if (modeRef.current === "collapsed" && edgeRef.current && lastStripExtentRef.current != null) {
            invoke("snap_collapse", { edge: edgeRef.current, extent: lastStripExtentRef.current }).catch((err) =>
              console.error("[snap] 缩放变化后重算缩略条失败：", err)
            );
          }
        })
        .then((f) => { un = f; })
        .catch(() => {});
    } catch {
      /* 非 Tauri 环境（测试/浏览器）忽略 */
    }
    return () => un?.();
  }, []);

  // 启动：读 settings 的贴纸几何（W-17；内含 localStorage 旧键一次性迁移）。上次吸附态 →
  // 折叠回竖条；否则按记住的正常尺寸还原（位置由后端 setup 在 show 前从 settings 恢复，见
  // lib.rs restore_sticker_window_position）。bootReady 闸门保证几何落地前不渲染、不显示。
  // macOS 面板模式无吸边/尺寸还原，但 pin 偏好同在 settings：异步读入后套用，不挡首帧。
  useEffect(() => {
    if (isMacPanel()) {
      initStickerWindowState()
        .then((s) => applyPinned(s.pinned))
        .catch(() => {});
      return;
    }
    let cancelled = false;
    (async () => {
      try {
        const s = await initStickerWindowState();
        if (cancelled) return;
        applyPinned(s.pinned);
        if (s.snap_edge) {
          setEdge(s.snap_edge);
          setMode("collapsed");
          try {
            await doCollapse(s.snap_edge);
          } catch (err) {
            console.error("[snap] 启动沿用折叠失败：", err);
          }
        } else {
          const { w, h } = normalSize();
          try {
            await invoke("snap_restore", { width: w, height: h, pinned: s.pinned });
          } catch (err) {
            console.error("[snap] 启动还原正常尺寸失败：", err);
          }
        }
      } catch (err) {
        console.error("[window-state] 启动读取贴纸几何失败：", err);
      } finally {
        // 成功失败都放行显示：失败时窗口留在默认几何，藏着不显示只会更糟
        // （后端 show_after_grace 迟早也会把它亮出来）。
        if (!cancelled) setBootReady(true);
      }
    })();
    return () => {
      cancelled = true;
    };
    // 仅启动跑一次
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 偷看：悬停缩略条 → 就地放大成全尺寸（仍贴边）。先切到过渡占位（W-4）再放大，
  // 避免放大瞬间渲染缩略条、也避免看板被压进 28px 细条宽度。macOS 面板模式：无此操作。
  // 收回由下方光标轮询负责（不用 DOM mouseleave，缩放时会误报）。
  const onExpand = useCallback(() => {
    if (isMacPanel()) return;
    if (modeRef.current !== "collapsed" || !edgeRef.current) return;
    const { w, h } = normalSize();
    const e = edgeRef.current;
    setExpanding(true);
    setMode("expanded");
    invoke("snap_expand", { edge: e, width: w, height: h })
      .catch((err) => console.error("[snap] snap_expand 失败：", err))
      .finally(() => setExpanding(false));
  }, []);

  // 展开态收回：不用 DOM 的 mouseleave（窗口缩放时会误报一串假 leave/enter → 抖动死循环），
  // 改为轮询真实光标坐标，连续两次确认在窗口外才收回（给短暂掠出一点容差）。
  useEffect(() => {
    if (isMacPanel() || mode !== "expanded") return;
    let outCount = 0;
    const id = window.setInterval(() => {
      if (draggingRef.current) {
        outCount = 0;
        return;
      }
      // 交互保护：搜索框/编辑器有焦点、或菜单开着时不自动收回。键盘输入时手常不在鼠标上，
      // 光标停在窗外 ~360ms 就折叠 = Sticker 卸载，编辑中的草稿与搜索状态全部丢失。
      const active = document.activeElement;
      const interacting =
        (active instanceof HTMLElement
          && (active.tagName === "INPUT" || active.tagName === "TEXTAREA" || active.isContentEditable))
        || document.querySelector(".ctx-menu") != null;
      if (interacting) {
        outCount = 0;
        return;
      }
      invoke<boolean>("cursor_over_window")
        .then((over) => {
          if (over) {
            outCount = 0;
            return;
          }
          outCount += 1;
          if (outCount >= 2 && modeRef.current === "expanded" && edgeRef.current) {
            doCollapse(edgeRef.current)
              .then(() => setMode("collapsed"))
              .catch((err) => console.error("[snap] 收回竖条失败：", err));
          }
        })
        .catch(() => {});
    }, 180);
    return () => window.clearInterval(id);
  }, [mode, doCollapse]);

  // W-17：settings 几何读取 + 折叠/还原落地前不渲染任何内容（窗口此刻还是默认几何且隐藏，
  // 渲染了也看不见，但会白付一次错误形态的布局）。bootReady 翻转后首帧即正确形态。
  if (!isMacPanel() && !bootReady) {
    return null;
  }
  // W-4：展开过渡占位——snap_expand/snap_restore 落地前窗口仍是 28px 细条几何，
  // 此刻渲染全量看板会被压进细条宽度（布局抖动）；先铺一块与贴纸同底色的纯色块，
  // 尺寸落地（setExpanding(false)）后再挂载内容。
  if (!isMacPanel() && expanding) {
    return <div className="snap-expanding" aria-hidden="true" />;
  }
  if (!isMacPanel() && mode === "collapsed" && edge) {
    return <CollapsedStrip data={stripSessions} edge={edge} onExpand={onExpand} onMeasure={onMeasure} />;
  }
  // 有新版本：贴纸不再弹浮动条，改为齿轮按钮上的红点提示(安装入口在设置→关于)。
  const hasUpdate = upStatus === "available" || upStatus === "downloading" || upStatus === "ready";
  // W-2：吸附落点预览的幽灵条主轴长度——与真实折叠条同一份 stripExtent，钳进当前窗口
  // 内缘。候选边判定阈值 20px，此刻窗口内缘与屏幕工作区边缘基本重合，轮廓即落点形状。
  const ghostLen = glow
    ? Math.min(stripExtent(connectedCount), Math.max(48, (glow === "top" ? window.innerWidth : window.innerHeight) - 16))
    : 0;
  return (
    <div style={{ height: "100%" }}>
      {!isMacPanel() && glow && <div className={"snap-glow snap-glow-" + glow} />}
      {!isMacPanel() && glow && (
        <div
          aria-hidden="true"
          className={"snap-ghost snap-ghost-" + glow}
          style={glow === "top" ? { width: ghostLen } : { height: ghostLen }}
        />
      )}
      <Sticker
        filter={filter}
        onFilterChange={pickFilter}
        data={items}
        counts={counts}
        total={totalFor(filter, counts)}
        hasMore={!reachedEnd}
        loadMore={loadMore}
        loadingMore={loadingMore}
        hasUpdate={hasUpdate}
        search={search}
        onSearchChange={changeSearch}
        onArchiveOptimistic={onArchiveOptimistic}
        onArchiveFailed={onArchiveFailed}
        initialLoading={initialLoading}
        loadError={loadError}
        onRetry={retryLoad}
        switching={switching}
        snapped={mode !== "normal"}
        pinned={pinned}
        onTogglePin={togglePin}
      />
    </div>
  );
}

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
import { PIN_KEY, type Item, type Tab } from "./views/sticker/types";
import { CollapsedStrip } from "./views/CollapsedStrip";
import { useUpdate } from "./useUpdate";
import { useShowWhenReady } from "./useShowWhenReady";
import { isMacPanel } from "./platform";

type Edge = "left" | "right" | "top";
type Mode = "normal" | "collapsed" | "expanded";

const SNAP_KEY = "meowo-snap-edge";
const SIZE_KEY = "meowo-normal-size";
const TAB_KEY = "meowo-tab";
const RELEASE_POLL_MS = 90; // 拖拽中轮询鼠标左键的间隔（检测真正松手）
const PAGE_SIZE = 100; // 贴纸会话每页条数，与首屏一致
const REFRESH_THROTTLE_MS = 400; // board-changed 刷新的冷却窗口，见 refresh

// board-changed 常是「空转」：命令写库后 db-watcher 又为同一次写入报一次、liveness 轮询重发、
// 甚至 app 自身读库触碰 board.db-wal/-shm 的 mtime 也会被 watcher 当成变更而回声。这些刷新拉回的
// 数据与当前完全一致，若照旧整表替换成新对象引用，会让整个虚拟列表无谓重渲染（视觉上「一直在更新」）。
//
// 行级 JSON 缓存 + 结构共享：每行 stringify 一次、与上次的缓存比对，没变的行**复用旧对象引用**。
// 于是（1）空转刷新整表引用不变（sameRefs 命中，跳过 setState）；（2）只有一行变化时，其余行
// 引用稳定，配合卡片层的 memo 只重渲染那一张。全字段 JSON 比较是刻意的**排除法**——按字段挑
// 白名单的签名方案会在漏字段时让 UI 静默不更新（sameHistoryMeta 的注释记过三回同类事故）。
// 缓存只增不清：键是 session.id，量级为窗口生命周期内见过的会话数，几百条小对象，不值得管理。
type RowCache = Map<number, { json: string; item: Item }>;
const reconcileRows = (cache: RowCache, rows: Item[]): Item[] =>
  rows.map((row) => {
    const id = row.session.id;
    const json = JSON.stringify(row);
    const hit = cache.get(id);
    if (hit && hit.json === json) return hit.item;
    cache.set(id, { json, item: row });
    return row;
  });
// 行已结构共享，引用比较即语义比较。
const sameRefs = (a: Item[], b: Item[]): boolean =>
  a.length === b.length && a.every((x, i) => x === b[i]);

// 缩略条主轴逻辑长度：按 connected 点数贴合内容（点 10px + 间距 7px = 17，两端留白 26），最小 48。
// 仅作折叠初值，CollapsedStrip 挂载后会按真实 DOM 尺寸精确校正。
function stripExtent(count: number): number {
  return Math.max(48, count * 17 + 26);
}

// 正常窗口最小尺寸：与 tauri.conf.json 的 minWidth/minHeight、snap.rs 的 STICKER_MIN_* 三处对齐。
// SIZE_KEY 是「正常尺寸」基准，不应低于此（低于即被细条尺寸毒化）。
// 高度按「至少完整显示两张会话卡」定（约 132px 窗框 + 2×90px 卡片）。
const SIZE_MIN_W = 360;
const SIZE_MIN_H = 330;
const SIZE_MAX = 20000; // 上界：与后端 snap_* 命令的 clamp 上限一致（防 f64→i32 回绕/异常大值）
const SIZE_DEFAULT = { w: 360, h: 440 }; // 与 tauri.conf.json 默认 width/height 一致

const sizeOk = (v: unknown): v is number =>
  typeof v === "number" && Number.isFinite(v);

function loadSize(): { w: number; h: number } {
  try {
    const s = JSON.parse(localStorage.getItem(SIZE_KEY) || "");
    // 校验有限数 + 落在 [最小, 最大] 内才采用；否则回落默认。低于最小=被「吸附态拖角缩成细条」的尺寸
    // 毒化(实测 {80,240}/{136,20})；非有限数/超大值=localStorage 被改坏，直接 set_size 会设出极端窗口。
    if (sizeOk(s?.w) && sizeOk(s?.h)
      && s.w >= SIZE_MIN_W && s.h >= SIZE_MIN_H
      && s.w <= SIZE_MAX && s.h <= SIZE_MAX) {
      return s;
    }
  } catch {
    /* ignore */
  }
  return { ...SIZE_DEFAULT };
}

// 写入「正常尺寸」基准：钳到 [最小, 最大]、非有限数回落默认。吸附态下 min_size 被放开（snap_collapse），
// 拖角可把窗缩成细条；若把细条/异常尺寸当正常尺寸存入会毒化 loadSize 基准，令启动还原异常。
function saveSize(w: number, h: number) {
  const clamp = (v: number, min: number, def: number) =>
    Number.isFinite(v) ? Math.min(SIZE_MAX, Math.max(min, v)) : def;
  localStorage.setItem(
    SIZE_KEY,
    JSON.stringify({ w: clamp(w, SIZE_MIN_W, SIZE_DEFAULT.w), h: clamp(h, SIZE_MIN_H, SIZE_DEFAULT.h) })
  );
}

function isPinned(): boolean {
  return localStorage.getItem(PIN_KEY) === "1";
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
  // 贴纸窗口以 visible:false 创建（tauri.conf.json），首帧渲染后再显示，消除启动瞬间的白框闪烁。
  // 不抢焦点（窗口配置 focus:false，开机自启同理）；macOS 面板模式显隐归 menubar 管，这里不越权。
  useShowWhenReady({ focus: false, enabled: !isMacPanel() });
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
  const pickFilter = useCallback((f: Tab) => {
    setFilter(f);
    localStorage.setItem(TAB_KEY, f);
  }, []);
  const [edge, setEdge] = useState<Edge | null>(() => {
    // macOS 面板模式：无吸边，edge 固定为 null。
    if (isMacPanel()) return null;
    const s = localStorage.getItem(SNAP_KEY);
    return s === "left" || s === "right" || s === "top" ? s : null;
  });
  // mode 初值与 edge 同源：上次在吸附态关闭（window-state 会把窗口恢复成缩略条尺寸），
  // 首屏就渲染缩略条，避免在细条窗口里显示完整贴纸的脱节。
  const [mode, setMode] = useState<Mode>(() => {
    // macOS 面板模式：始终以普通态启动。
    if (isMacPanel()) return "normal";
    const s = localStorage.getItem(SNAP_KEY);
    return s === "left" || s === "right" || s === "top" ? "collapsed" : "normal";
  });
  const [glow, setGlow] = useState<Edge | null>(null); // 拖拽中靠近边缘的发光提示
  // 展开过渡中：缩放期间强制不渲染缩略条（三个圆点），落地后再恢复按 mode 判定。
  const [expanding, setExpanding] = useState(false);
  // 只读检查：仅驱动贴纸设置钮上的更新红点；下载/安装由更新窗口（views/Updater）全权负责。
  const { status: upStatus } = useUpdate({ automatic: true });

  // 折叠条恒显示全部「连接中」会话（running + waiting），与当前选中 tab 无关——
  // 故独立于分页 items 单独加载（按状态查，覆盖旧但仍连接的会话，不受 tab/分页窗口影响）。
  const [stripSessions, setStripSessions] = useState<Item[]>([]);
  const stripRowCacheRef = useRef<RowCache>(new Map());
  const loadStrip = useCallback(() => {
    Promise.all([
      getLiveSessionsPage("running", null, null, 200),
      getLiveSessionsPage("waiting", null, null, 200),
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
  const loadPage = useCallback(
    async (
      filter: Tab,
      search: string,
      cursor: PageCursor | null,
      limit: number = PAGE_SIZE
    ): Promise<{ page: Item[]; cursor: PageCursor | null; applied: boolean }> => {
      const seq = ++refreshSeqRef.current;
      // counts 只在首页/刷新（cursor === null）时才需要；loadMore 复用已有 counts，
      // 避免频繁 loadMore 时对 counts 做纯重复的后端查询（审查发现）。
      const needCounts = cursor === null;
      try {
        const [countsRes, res] = await Promise.all([
          needCounts ? getLiveSessionsCounts() : Promise.resolve(null),
          getLiveSessionsPage(filter, search, cursor, limit),
        ]);
        const page = res.items;
        const applied = seq === refreshSeqRef.current;
        if (!applied) return { page: page as Item[], cursor: res.next_cursor, applied };
        nextCursorRef.current = res.next_cursor;
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
            const next = reconcileRows(itemsRowCacheRef.current, page as Item[]);
            if (!search.trim()) unsearchedItemsRef.current[filter] = next;
            // 空转刷新（数据未变）保持原引用，跳过整表重渲染，消除视觉抖动（见 reconcileRows）。
            return sameRefs(prev, next) ? prev : next;
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
          const next = [...prev.map((l) => map.get(l.session.id)!), ...append];
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

  // 一次刷新：重查「已加载窗口」大小（max(PAGE_SIZE, 当前条数)），保住用户已滚动加载的会话、
  // 同时反映最新排序/状态，避免被打回第一页（P0）。
  const itemsLenRef = useRef(0);
  itemsLenRef.current = items.length;
  // 首页/搜索视图正在切换时，旧 items 仍会短暂留在 DOM。此时虚拟列表若触发 loadMore，
  // 会拿旧列表游标发起一个更新请求，取消首页请求并把结果按旧顺序合并（清空搜索排序错乱的根因）。
  const resettingPageRef = useRef(false);
  const pageResetSeqRef = useRef(0);
  const doRefresh = useCallback(() => {
    const resetSeq = ++pageResetSeqRef.current;
    resettingPageRef.current = true;
    setReachedEnd(false);
    const w = Math.max(PAGE_SIZE, itemsLenRef.current);
    loadPage(filter, search, null, w)
      .then(({ cursor, applied }) => {
        if (applied) {
          setInitialLoading(false);
          setLoadError(false);
          if (resetSeq === pageResetSeqRef.current) resettingPageRef.current = false;
          if (cursor === null) setReachedEnd(true);
        }
      })
      .catch(() => {
        if (resetSeq === pageResetSeqRef.current) {
          setInitialLoading(false);
          setLoadError(true);
          resettingPageRef.current = false;
        }
      });
    // 折叠条数据独立刷新（不随 tab）——但只在非 normal 态：正常态下缩略条不渲染，
    // 每次 board-changed 都跟着拉 2×200 条只为一个折叠时才用的计数，不值。挂载时和
    // 折叠动作发起时（doCollapse）各拉一次，保证真折叠的那一刻数据是新的。
    if (modeRef.current !== "normal") loadStrip();
  }, [filter, search, loadPage, loadStrip]);

  // trailing 刷新必须用「触发那一刻」的 filter/search，而非排队那一刻捕获的值：否则排队期间切了 tab，
  // 旧 filter 的刷新会晚于新 tab 的加载落地，把新 tab 的列表覆盖掉。
  const doRefreshRef = useRef(doRefresh);
  doRefreshRef.current = doRefresh;

  // board-changed 会连发（命令写库后端立即通知 + db-watcher 稍后为同一次写入回声 + liveness 轮询），
  // 故 leading + trailing 节流：首个事件立即刷新（用户操作零延迟），冷却窗口内的后续事件合并成窗口
  // 末尾的一次刷新。每次刷新是 counts + 一整页 + 折叠条两查询，页大小随滚动增长，值得省。
  const refreshTimerRef = useRef<number | undefined>(undefined);
  const refreshLastRunRef = useRef(0);
  const refresh = useCallback(() => {
    if (refreshTimerRef.current !== undefined) return; // trailing 已排队，本次并入
    const fire = () => {
      refreshTimerRef.current = undefined;
      refreshLastRunRef.current = Date.now();
      doRefreshRef.current();
    };
    const since = Date.now() - refreshLastRunRef.current;
    if (since >= REFRESH_THROTTLE_MS) fire();
    else refreshTimerRef.current = window.setTimeout(fire, REFRESH_THROTTLE_MS - since);
  }, []);

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
            if (resetSeq === pageResetSeqRef.current) resettingPageRef.current = false;
            if (cursor === null) setReachedEnd(true);
          }
        })
        .catch(() => {
          if (resetSeq === pageResetSeqRef.current) {
            setInitialLoading(false);
            setLoadError(true);
            resettingPageRef.current = false;
          }
        });
    }, search ? 300 : 0);
    return () => window.clearTimeout(t);
  }, [filter, search, loadPage, retryNonce]);

  const changeSearch = useCallback((next: string) => {
    if (search.trim() !== next.trim()) resettingPageRef.current = true;
    // 后端的无搜索请求回来前就恢复原数组，既不让搜索结果继续占位，也完整保留原顺序。
    // 同时使仍在途的搜索请求失效，防止它在清空后短暂覆盖恢复出的列表。
    if (search.trim() && !next.trim()) {
      refreshSeqRef.current += 1;
      const cached = unsearchedItemsRef.current[filter];
      if (cached) setItems(cached);
    }
    setSearch(next);
  }, [filter, search]);

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
  const onArchiveFailed = useCallback(() => refresh(), [refresh]);

  useEffect(() => {
    let cancelled = false;
    let un: (() => void) | undefined;
    // E2E 观测点（仅 VITE_E2E=1 构建启用）：累计收到的 board-changed 次数，供回归测试断言
    // 「空闲时看板不再被刷新」（见 app/e2e/specs/board-refresh.e2e.ts）。生产构建下 VITE_E2E
    // 未定义，三元恒取 refresh、这段被 vite 死代码消除，运行时零开销。
    if (import.meta.env.VITE_E2E === "1") {
      // 挂载即初始化为 0，让 E2E 测试挂载后立刻能读到计数（不必等第一次事件）。
      (window as typeof window & { __MEOWO_BOARD_CHANGED__?: number }).__MEOWO_BOARD_CHANGED__ ??= 0;
    }
    const onBoardChanged =
      import.meta.env.VITE_E2E === "1"
        ? () => {
            const w = window as typeof window & { __MEOWO_BOARD_CHANGED__?: number };
            w.__MEOWO_BOARD_CHANGED__ = (w.__MEOWO_BOARD_CHANGED__ ?? 0) + 1;
            refresh();
          }
        : refresh;
    // refresh 恒等（useCallback([])，最新的 filter/search 经 doRefreshRef 取），listener 只注册一次。
    listen("board-changed", onBoardChanged)
      .then((f) => {
        if (cancelled) f();
        else un = f;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      if (un) un();
      window.clearTimeout(refreshTimerRef.current);
      // 复位而不仅是 clearTimeout：refresh() 以「ref 非 undefined」判定 trailing 已排队。
      // HMR 重挂载会跑 cleanup 但保留 ref——不复位则残留一个永不触发的定时器 id，
      // 之后每次 refresh() 都以为 trailing 在排队而直接 return，看板刷新永久静默丢失。
      refreshTimerRef.current = undefined;
    };
  }, [refresh]);

  // 托盘「找回贴纸」：把贴纸拉回主屏中央并置顶。折叠/吸附态先展开还原成正常窗口，再居中置顶。
  // macOS 面板模式无吸边/托盘菜单项，跳过。
  useEffect(() => {
    if (isMacPanel()) return;
    const recall = async () => {
      // 置顶偏好（Sticker 的 pin 按钮也读此 key）；折叠态下 Sticker 未挂载，展开后据此初始化为置顶。
      localStorage.setItem(PIN_KEY, "1");
      if (modeRef.current !== "normal") {
        const { w, h } = loadSize();
        localStorage.removeItem(SNAP_KEY);
        setEdge(null);
        setMode("normal");
        try {
          await invoke("snap_restore", { width: w, height: h, pinned: true });
        } catch (err) {
          console.error("[recall] snap_restore 失败：", err);
        }
      }
      try {
        await invoke("recall_center");
      } catch (err) {
        console.error("[recall] recall_center 失败：", err);
      }
    };
    const un = listen("recall-sticker", () => void recall());
    return () => {
      un.then((f) => f());
    };
  }, []);

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
    const d = lastEdgeRef.current;
    const m = modeRef.current;
    if (d) {
      // 靠边松手 → 折叠
      if (m === "normal") {
        try {
          const sz = await getCurrentWindow().outerSize();
          const sf = await getCurrentWindow().scaleFactor();
          saveSize(sz.width / sf, sz.height / sf);
        } catch {
          /* ignore */
        }
      }
      if (m !== "collapsed") {
        try {
          await doCollapse(d);
          localStorage.setItem(SNAP_KEY, d);
          setEdge(d);
          setMode("collapsed");
        } catch (err) {
          console.error("[snap] snap_collapse 失败：", err);
        }
      }
    } else if (m !== "normal") {
      // 离边松手 → 还原普通窗口
      const { w, h } = loadSize();
      try {
        await invoke("snap_restore", { width: w, height: h, pinned: isPinned() });
        localStorage.removeItem(SNAP_KEY);
        setEdge(null);
        setMode("normal");
      } catch (err) {
        console.error("[snap] snap_restore 失败：", err);
      }
    }
  }, [doCollapse]);

  // 用户拖边框缩放窗口（后端 WM_SIZING/WM_EXITSIZEMOVE 检测）：
  // - 缩放开始(user-resized)：暂解除吸附变普通窗口，避免缩放中被吸附逻辑误折叠抖动；记住原吸附边。
  // - 缩放结束(user-resize-end)：若缩放前是吸附的，按新尺寸重新吸回原来那条边（保留新尺寸）。
  useEffect(() => {
    if (isMacPanel()) return;
    const unStart = listen("user-resized", () => {
      if (modeRef.current === "normal") return; // 本就普通态，无吸附可解
      preResizeEdgeRef.current = edgeRef.current; // 记住缩放前的吸附边
      invoke("unsnap", { pinned: isPinned() }).catch(() => {});
      localStorage.removeItem(SNAP_KEY);
      setEdge(null);
      setMode("normal");
    });
    const unEnd = listen("user-resize-end", async () => {
      const e = preResizeEdgeRef.current;
      preResizeEdgeRef.current = null;
      if (!e) {
        // 缩放前非吸附态 → 保持普通窗口；把新尺寸记为常用尺寸，供启动对账/还原沿用，
        // 避免用户自定义的小窗口被误判为「吸附遗留细条」而被强行放大。
        try {
          const sz = await getCurrentWindow().outerSize();
          const sf = await getCurrentWindow().scaleFactor();
          saveSize(sz.width / sf, sz.height / sf);
        } catch {
          /* ignore */
        }
        return;
      }
      // 把缩放后的尺寸记为新的常用尺寸，再按它重新吸回原边（snap_expand 会贴边定位）。
      // saveSize 会钳到最小尺寸：吸附态下 min 被放开，拖角可把窗缩成细条，不能让细条尺寸污染基准。
      try {
        const sz = await getCurrentWindow().outerSize();
        const sf = await getCurrentWindow().scaleFactor();
        saveSize(sz.width / sf, sz.height / sf);
      } catch {
        /* ignore */
      }
      const { w, h } = loadSize();
      localStorage.setItem(SNAP_KEY, e);
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
  const onMeasure = useCallback((ext: number) => {
    if (modeRef.current === "collapsed" && edgeRef.current) {
      invoke("snap_collapse", { edge: edgeRef.current, extent: ext }).catch((err) =>
        console.error("[snap] 调整缩略条尺寸失败：", err)
      );
    }
  }, []);

  // 重启沿用：上次吸附态→折叠回竖条；否则按 SIZE_KEY 还原正常尺寸。macOS 面板模式跳过。
  useEffect(() => {
    if (isMacPanel()) return;
    const e = edgeRef.current;
    if (e) {
      doCollapse(e)
        .then(() => setMode("collapsed"))
        .catch((err) => console.error("[snap] 启动沿用折叠失败：", err));
      return;
    }
    // 非吸附态启动：main 窗口尺寸由 localStorage(SIZE_KEY) 单一持有——window-state 已配置为不恢复尺寸
    // (见 lib.rs)，窗口此刻是 tauri.conf 默认尺寸、位置则由 window-state 恢复。无条件按 SIZE_KEY 还原
    // 成用户上次的正常尺寸(snap_restore 保留位置并拉回工作区内)。这样 window-state 永不持有「细条几何」，
    // 从根上消除「window-state 几何 ↔ SNAP_KEY 吸附态」不同步导致的「细条大小+完整内容+没真正吸附」。
    (async () => {
      try {
        const { w, h } = loadSize();
        await invoke("snap_restore", { width: w, height: h, pinned: isPinned() });
      } catch {
        /* 非 Tauri 环境（测试/浏览器）忽略 */
      }
    })();
    // 仅启动跑一次
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 偷看：悬停缩略条 → 就地放大成全尺寸（仍贴边）。先切看板视图再放大，避免放大瞬间渲染缩略条。
  // macOS 面板模式：无此操作。收回由下方光标轮询负责（不用 DOM mouseleave，缩放时会误报）。
  const onExpand = useCallback(() => {
    if (isMacPanel()) return;
    if (modeRef.current !== "collapsed" || !edgeRef.current) return;
    const { w, h } = loadSize();
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

  if (!isMacPanel() && mode === "collapsed" && edge && !expanding) {
    return <CollapsedStrip data={stripSessions} edge={edge} onExpand={onExpand} onMeasure={onMeasure} />;
  }
  // 有新版本：贴纸不再弹浮动条，改为齿轮按钮上的红点提示(安装入口在设置→关于)。
  const hasUpdate = upStatus === "available" || upStatus === "downloading" || upStatus === "ready";
  return (
    <div style={{ height: "100%" }}>
      {!isMacPanel() && glow && <div className={"snap-glow snap-glow-" + glow} />}
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
      />
    </div>
  );
}

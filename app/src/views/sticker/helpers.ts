// 贴纸看板的纯逻辑 helper：行内编辑键盘处理、相对时间、置顶持久化、tab 过滤。
import { useCallback, useEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import type { Dict } from "../../i18n/zh";
import { STAR_KEY, type Item, type Tab } from "./types";

export const editorKeyDown =
  (submit: () => void, cancel: () => void) =>
  (e: ReactKeyboardEvent<HTMLInputElement>) => {
    if (e.nativeEvent.isComposing || e.keyCode === 229) return;
    if (e.key === "Enter") submit();
    else if (e.key === "Escape") cancel();
  };

export function fmtAgo(ms: number, t: Dict): string {
  const m = Math.floor((Date.now() - ms) / 60000);
  if (m < 1) return t.time.now;
  if (m < 60) return t.time.minAgo(m);
  const h = Math.floor(m / 60);
  if (h < 24) return t.time.hourAgo(h);
  return t.time.dayAgo(Math.floor(h / 24));
}

/** waiting tab 的时长语义：「已等待 X」。它是全应用唯一倒排（等最久在前）的列表，
 *  普通的「X 前」读不出排序含义——最上面是三小时前的老会话时，用户第一反应是
 *  「列表坏了」。改成等待时长，排序自解释。 */
export function fmtWaited(ms: number, t: Dict): string {
  const m = Math.floor((Date.now() - ms) / 60000);
  if (m < 1) return t.time.waitedNow;
  if (m < 60) return t.time.waitedMin(m);
  const h = Math.floor(m / 60);
  if (h < 24) return t.time.waitedHour(h);
  return t.time.waitedDay(Math.floor(h / 24));
}

/** 读取已置顶会话集合（按 cc_session_id 持久化，跨重启/换库稳定）。
 *  存储键沿用 star 命名（meowo-starred）：界面改叫「置顶」后不动键名，改了会丢用户已有数据。 */
export function loadStarred(): Set<string> {
  try {
    const raw = JSON.parse(localStorage.getItem(STAR_KEY) ?? "[]");
    return new Set(Array.isArray(raw) ? raw.filter((x): x is string => typeof x === "string") : []);
  } catch {
    return new Set();
  }
}

/** 置顶集变更的同窗口广播事件名。storage 事件只发给**其他**窗口（浏览器语义），
 *  同窗口内的会话侧栏 ↔ 对话窗标题菜单要靠它互相通知。 */
export const STARRED_CHANGED_EVENT = "meowo-starred-changed";

/** 翻转一条会话的置顶态：写库 + 同窗口广播（跨窗口由原生 storage 事件覆盖）。
 *  返回新集合供调用方 setState。看板/侧栏/标题菜单共用这一份写入口——
 *  三处各写一份时漏了广播就是视图失同步（评审发现）。 */
export function toggleStarred(current: Set<string>, ccSessionId: string): Set<string> {
  const next = new Set(current);
  if (!next.delete(ccSessionId)) next.add(ccSessionId);
  localStorage.setItem(STAR_KEY, JSON.stringify([...next]));
  window.dispatchEvent(new Event(STARRED_CHANGED_EVENT));
  return next;
}

/** 置顶集的读端 + 翻转入口（看板/侧栏/标题菜单同款）。变更无论来自本窗口
 *  （自定义事件）还是其他窗口（storage 事件），都重读落库值。 */
export function useStarred(): { starred: Set<string>; toggleStar: (ccSessionId: string) => void } {
  const [starred, setStarred] = useState<Set<string>>(loadStarred);
  const starredRef = useRef(starred);
  starredRef.current = starred;
  useEffect(() => {
    const reload = () => setStarred(loadStarred());
    const onStorage = (event: StorageEvent) => { if (event.key === STAR_KEY) reload(); };
    window.addEventListener(STARRED_CHANGED_EVENT, reload);
    window.addEventListener("storage", onStorage);
    return () => {
      window.removeEventListener(STARRED_CHANGED_EVENT, reload);
      window.removeEventListener("storage", onStorage);
    };
  }, []);
  const toggleStar = useCallback((ccSessionId: string) => {
    setStarred(toggleStarred(starredRef.current, ccSessionId));
  }, []);
  return { starred, toggleStar };
}

/** 连接中会话的活动态阶梯——卡片状态环（cardTone）与 tab 归属（match）的**共同地基**，
 *  与后端 session_query.rs 的 tab_class 同序同源：
 *  审批/屏幕 blocked > 屏幕 working/idle > DB status。
 *  屏幕检测（screen_state，仅托管会话有值）优先于 DB status：DB 的 running 由 hook 事件
 *  驱动，agent 在屏幕上弹出审批/提问时它还停在上一个事件；屏幕状态 300ms 级实时。
 *  pending_review（broker 正压着的审批）仍最优先——那是事实源。
 *  环与 tab 曾各看各的（环吃屏幕、tab 只看 status）：一条 status=running 而屏幕已 idle
 *  的会话顶着黄环（等你）待在「运行中」tab 里，用户以为状态坏了（实拍反馈）——
 *  阶梯必须住在这一处，环和 tab 共用。 */
function activity(l: Item): "pending" | "running" | "waiting" | null {
  if (l.pending_review || l.screen_state === "blocked") return "pending";
  if (l.screen_state === "working") return "running";
  // 屏幕 idle / DB waiting 只说明**主回合**停了——后台子任务还在跑时是「运行中」,
  // 不该催人(与后端 tab_class 的 background_busy 同口径,原料同为 busy_subagents)。
  const busy = (l.busy_subagents ?? 0) > 0;
  if (l.screen_state === "idle") return busy ? "running" : "waiting";
  if (l.session.status === "running") return "running";
  if (l.session.status === "waiting") return busy ? "running" : "waiting";
  return null;
}

/** 卡片状态徽标的统一口径（看板卡片指示器与折叠缩略条共用的单点判定）。
 *  缩略条曾自写一套只看 status 的映射，待审批会话在条上被画成绿色运行点、
 *  「有人在等你」的信号被抹掉（评审发现）——判定必须住在这一处。 */
export type CardTone = "offline" | "error" | "pending" | "running" | "waiting" | "on";
export function cardTone(l: Item): CardTone {
  if (!l.connected) return "offline";
  if (l.errored) return "error";
  return activity(l) ?? "on";
}

export function match(tab: Tab, l: Item): boolean {
  if (l.archived) return false; // 已归档的不上看板（管理入口在设置 → 会话）
  if (tab === "all") return true;
  // running = AI 自主运行且无需用户介入；waiting = 等用户交互（含 pending：审批/屏幕阻塞）。
  // 判定走与卡片状态环相同的 activity 阶梯，也与后端 tab_class（列表分页 + 角标计数）
  // 同口径——三端喂同一份原料（校正后的 pending_review + 屏幕状态 + status）。
  //
  // 断开的会话两类都不进：进程都没了，催用户去交互毫无意义（点进去只是个历史会话），显示成
  // 在跑更是假的。它们只作为历史留在「全部」里。
  if (!l.connected) return false;
  const a = activity(l);
  if (tab === "waiting") return a === "waiting" || a === "pending";
  if (tab === "running") return a === "running";
  return true;
}

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

export function match(tab: Tab, l: Item): boolean {
  if (l.archived) return false; // 已归档的不上看板（管理入口在设置 → 会话）
  if (tab === "all") return true;
  // running = AI 自主运行且无需用户介入；waiting = 等用户交互（status=waiting 或 pending_review）。
  // 与后端 live_sessions / tab_class 语义保持一致。
  //
  // 断开的会话两类都不进：进程都没了，催用户去交互毫无意义（点进去只是个历史会话），显示成
  // 在跑更是假的。DB 里残留的 pending_review 会让它们漏进 waiting——后台收尾只改 status、
  // 不清 pending_review，而 waiting 的判定是 `status=waiting || pending_review != null`。
  // 它们只作为历史留在「全部」里。
  if (!l.connected) return false;
  if (tab === "waiting") return l.session.status === "waiting" || l.pending_review != null;
  if (tab === "running") return l.session.status === "running" && l.pending_review == null;
  return true;
}

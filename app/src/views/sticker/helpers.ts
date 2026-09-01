// 贴纸看板的纯逻辑 helper：行内编辑键盘处理、相对时间、置顶持久化、tab 过滤。
import { useCallback, useEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import type { Dict } from "../../i18n/zh";
import { activityTone } from "../../activity";
import { STAR_KEY, type Item, type Tab } from "./types";

export const editorKeyDown =
  (submit: () => void, cancel: () => void) =>
  (e: ReactKeyboardEvent<HTMLInputElement>) => {
    if (e.nativeEvent.isComposing || e.keyCode === 229) return;
    if (e.key === "Enter") submit();
    else if (e.key === "Escape") cancel();
  };

// 相对时间走 Intl.RelativeTimeFormat（G-13）：手拼「N 分钟前 / N min ago」只照顾了
// 中英两种语序，复数规则与语序随 locale 交给 Intl（英文还顺带从 "5 min ago" 的缩写
// 回到自然的 "5 minutes ago"，1 天前得 "yesterday"）。
// formatter 按 locale 缓存：fmtAgo 在列表渲染里逐卡片调用，每次 new 太贵。
const agoFormatters = new Map<string, Intl.RelativeTimeFormat>();
function agoFormatter(locale: string): Intl.RelativeTimeFormat {
  let rtf = agoFormatters.get(locale);
  if (!rtf) {
    rtf = new Intl.RelativeTimeFormat(locale, { numeric: "auto" });
    agoFormatters.set(locale, rtf);
  }
  return rtf;
}

export function fmtAgo(ms: number, t: Dict): string {
  const m = Math.floor((Date.now() - ms) / 60000);
  if (m < 1) return t.time.now;
  const rtf = agoFormatter(t.locale);
  if (m < 60) return rtf.format(-m, "minute");
  const h = Math.floor(m / 60);
  if (h < 24) return rtf.format(-h, "hour");
  return rtf.format(-Math.floor(h / 24), "day");
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

/** 卡片状态环（cardTone）与 tab 归属（match）读活动态的入口——阶梯本身住在
 *  `api.ts` 的 [`activityTone`]，与对话窗侧栏的 `sessionTone`、后端
 *  `session_query.rs` 的 tab_class 共用同一份判定（三处曾各有一套，见那里的注释）。
 *  这里只负责把看板的 `Item` 摊平成阶梯认识的字段。 */
function activity(l: Item): "pending" | "running" | "waiting" | null {
  return activityTone({
    status: l.session.status,
    pendingReview: l.pending_review,
    screenState: l.screen_state,
    busySubagents: l.busy_subagents,
  });
}

/** 卡片状态徽标的统一口径（看板卡片指示器与折叠缩略条共用的单点判定）。
 *  缩略条曾自写一套只看 status 的映射，待审批会话在条上被画成绿色运行点、
 *  「有人在等你」的信号被抹掉（评审发现）——判定必须住在这一处。
 *  注：tone 只回答「画成哪一档」，不回答「这个判定有多可信」——角标的两层不自信
 *  区分（无 screen_state 回落 DB status 的 assumed、fallback idle 降中性点，T-15）
 *  由下方的 toneConfidence 回答（tab 归属等判定不消费它，呈现层才消费）。
 *  assumed 的区分只走文案层，不降透明度（实拍反馈，见 toneConfidence 注释）。 */
export type CardTone = "offline" | "error" | "pending" | "running" | "waiting" | "on";
export function cardTone(l: Item): CardTone {
  if (!l.connected) return "offline";
  if (l.errored) return "error";
  return activity(l) ?? "on";
}

/** 徽标置信度（T-15 的两层「不自信」）：tone 只回答「画成哪一档」，本函数回答
 *  「这个判定有多可信」。卡片徽标（Sticker）与缩略条状态点（CollapsedStrip）共用
 *  这一处判定——条上曾只看 tone，弱化口径与卡片漂移。
 *  - "assumed"：无屏幕检测（screen_state==null）的会话（外部终端、外库卡），tone
 *    回落 DB status——hook 事件驱动的滞后快照。「按记录推断 ≠ 实时判定」的区分只走
 *    文案层（卡片包层/条点的「（推断）」tip），不降透明度——实拍反馈：外部打开的
 *    会话被降亮度读成了「不可用/已断开」。
 *  - "fallback"：屏幕检测走了兜底（什么规则都没命中、回退 idle 判 waiting）——是
 *   「认不出来」而不是「确认空闲」，降中性灰点。
 *  tone 由 cardTone 算出后传入，避免重复计算。 */
export function toneConfidence(l: Item, tone: CardTone): "assumed" | "fallback" | null {
  if (tone === "waiting" && l.screen_state === "idle" && l.screen_assumed === true) return "fallback";
  if (l.connected && l.screen_state == null && (tone === "running" || tone === "waiting")) return "assumed";
  return null;
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

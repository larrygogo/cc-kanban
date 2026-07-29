// 贴纸看板的纯逻辑 helper：行内编辑键盘处理、相对时间、置顶持久化、tab 过滤。
import type { KeyboardEvent as ReactKeyboardEvent } from "react";
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

export function match(tab: Tab, l: Item): boolean {
  if (tab === "archived") return l.archived;
  if (l.archived) return false; // 已归档的不在其它分类显示
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

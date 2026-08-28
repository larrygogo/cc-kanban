/**
 * 标题栏的两个菜单（从 ChatWindow.tsx 原样搬出，行为不变）：
 * - ChatTitleMenu：标题胶囊 + 会话动作菜单（置顶/打开目录/重命名/归档）；
 * - ChatTodoMenu：任务进度入口与浮出面板（任务清单 + 子任务分节）。
 */
import { useEffect, useLayoutEffect, useState } from "react";
import { openProjectDir } from "../../api";
import { remoteUi } from "../../remoteMode";
import { useT } from "../../i18n";
import { useMenuPopup } from "../menu";
import { ArchiveIcon, ChevronDownIcon, TopIcon } from "../sticker/icons";
import { durationText } from "./shared";
import { TodoBadge } from "./TodoBadge";

/** 标题胶囊 + 会话动作菜单（Kimi 式）：标题本身是触发钮（▾），置顶/打开目录/重命名/
 *  归档都收进菜单——标题栏只留「正在看哪条 + 状态 + 视图切换」。重命名走独立 modal
 *  （onRename 只负责打开它）。 */
export function ChatTitleMenu({ title, cwd, archived, archiving, starred, onToggleStar, onRename, onToggleArchived, t }: {
  title: string;
  cwd: string | null;
  archived: boolean;
  archiving: boolean;
  starred: boolean;
  onToggleStar: () => void;
  onRename: () => void;
  onToggleArchived: () => void;
  t: ReturnType<typeof useT>;
}) {
  const { open, setOpen, pos, ref, btnRef, menuRef, toggle, onKeyDown } = useMenuPopup({ align: "left" });
  // 先收菜单再执行动作：动作可能弹确认/切会话，菜单不该压在那些之上。
  const pick = (action: () => void) => { setOpen(false); btnRef.current?.focus(); action(); };
  return (
    <div className="dd chat-title-menu" ref={ref} onKeyDown={onKeyDown}>
      <button ref={btnRef} type="button" className="chat-title-btn" aria-haspopup="menu" aria-expanded={open} onClick={toggle}>
        <span className="chat-title">{title}</span>
        <ChevronDownIcon className="chat-title-chev" />
      </button>
      {open && (
        /* 不铺 pos.width：菜单宽随内容（.dd .dd-menu 的 min-width: max-content），
           只沿用左对齐与视口钳制。 */
        <div className="dd-menu chat-title-actions" role="menu" ref={menuRef} style={{ position: "fixed", top: pos.top, bottom: pos.bottom, left: pos.left }}>
          <button type="button" role="menuitem" className="dd-item" onClick={() => pick(onToggleStar)}>
            <span className="chat-title-action-ico"><TopIcon /></span>
            <span className="dd-label">{starred ? t.sticker.unstar : t.sticker.star}</span>
          </button>
          {/* open_project_dir 开的是宿主文件管理器,/rpc 拒绝项——远程隐藏(侧栏同款动作同款门控)。 */}
          {cwd && !remoteUi() && (
            // 菜单内刻意用原生 title：TooltipLayer 对 .dd-menu 区域静默（自绘提示会糊在菜单上）。
            <button type="button" role="menuitem" className="dd-item" title={cwd} onClick={() => pick(() => void openProjectDir(cwd).catch(() => {}))}>
              <span className="chat-title-action-ico">
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z" /></svg>
              </span>
              <span className="dd-label">{t.sticker.openProjectDir}</span>
            </button>
          )}
          <button type="button" role="menuitem" className="dd-item" onClick={() => pick(onRename)}>
            <span className="chat-title-action-ico">
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M17 3a2.85 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z" /></svg>
            </span>
            <span className="dd-label">{t.sticker.renameTitle}</span>
          </button>
          <button type="button" role="menuitem" className="dd-item" disabled={archiving} onClick={() => pick(onToggleArchived)}>
            <span className="chat-title-action-ico"><ArchiveIcon archived={archived} /></span>
            <span className="dd-label">{archived ? t.sticker.unarchive : t.sticker.archive}</span>
          </button>
        </div>
      )}
    </div>
  );
}

/** 标题栏任务进度入口:常驻清单图标钮,点开浮出「进度」面板(参考 Claude 桌面版形态,
 *  用户指定)。无任务时不藏图标——面板给骨架占位,让用户知道任务进度会出现在这里。
 *  清单行样式与底部 TodoPanel 共用同一组类(.chat-todos-list/.chat-todo-mark),两处不分叉。 */
/** 折叠态每小节最多露的行数;两节合计只多藏得下 1 条时不折(为省 1 行多点一次不值)。 */
const TODO_PANEL_COLLAPSED_MAX = 4;

/** 面板行的统一形态:任务清单与子任务都归一到「文字 + 状态」再渲染。
 *  时间戳仅子任务行携带(委派时刻/最终回执时刻),行尾据此显示执行时长。 */
export type TodoPanelRow = {
  content: string;
  status: string;
  startedAt?: string | null;
  finishedAt?: string | null;
  /** 子任务行携带委派的 tool_use id，中途结局的侧车探测按它索引。任务清单行没有。 */
  id?: string;
};

/** 行尾的执行时长:在跑 = 从委派时刻到现在(面板开着时逐秒刷新),结束 = 总用时。
 *  没有时间戳(任务清单行/部分 provider)不显示。 */
function rowDuration(row: TodoPanelRow, t: ReturnType<typeof useT>): string | null {
  const start = row.startedAt ? Date.parse(row.startedAt) : NaN;
  if (!Number.isFinite(start)) return null;
  const end = row.status === "in_progress" ? Date.now() : row.finishedAt ? Date.parse(row.finishedAt) : NaN;
  if (!Number.isFinite(end) || end < start) return null;
  return durationText(end - start, t);
}

/** 显示序与原始序脱钩:进行中最前、待办其次、失败(仅子任务有)再次、完成沉底(用户指定)
 *  ——折叠态露出的是「正在做/接下来做什么」,而不是一排划了线的旧账。sort 稳定,组内保持原顺序。 */
function sortPanelRows(rows: TodoPanelRow[]): TodoPanelRow[] {
  const rank: Record<string, number> = { in_progress: 0, pending: 1, failed: 2, completed: 3 };
  return [...rows].sort((a, b) => (rank[a.status] ?? 1) - (rank[b.status] ?? 1));
}

function TodoPanelList({ rows, t }: { rows: TodoPanelRow[]; t: ReturnType<typeof useT> }) {
  return (
    <ul className="chat-todo-panel-list">
      {rows.map((row, index) => {
        const duration = rowDuration(row, t);
        return (
          <li key={`${index}:${row.content}`} className={"is-" + row.status}>
            <TodoBadge status={row.status} />
            {/* 面板本身是 .dd-menu：TooltipLayer 对菜单区域静默，完整文案走原生 title。 */}
            <span className="chat-todo-panel-text" title={row.content}>{row.content}</span>
            {duration && <span className="chat-todo-panel-time">{duration}</span>}
          </li>
        );
      })}
    </ul>
  );
}

export function ChatTodoMenu({ todos, subagents, onOpenChange, t }: {
  todos: TodoPanelRow[];
  /** 从时间线聚合的子任务(Agent 委派),与任务清单分节显示——用户反馈「子任务没在这里」。 */
  subagents: TodoPanelRow[];
  /** 面板开合。并行委派的中途结局要读侧车才知道(主链回执要等整批跑完才写),那是按需
   *  I/O——只在用户真看着面板时探测,收起即停。 */
  onOpenChange?: (open: boolean) => void;
  t: ReturnType<typeof useT>;
}) {
  const { open, pos, ref, btnRef, menuRef, toggle, onKeyDown } = useMenuPopup();
  const done = todos.filter((todo) => todo.status === "completed").length;
  const subDone = subagents.filter((row) => row.status === "completed").length;
  // 里面有活儿在跑(任务或子任务进行中)→ 入口图标挂脉冲小点。面板收着时这是唯一的
  // 活动信号——用户反馈「入口上看不出里面有在执行的」。
  const live = todos.some((row) => row.status === "in_progress") || subagents.some((row) => row.status === "in_progress");
  // 面板开着且有在跑的子任务时逐秒重渲染,让行尾「已耗时」走起来;收起即停。
  const ticking = open && subagents.some((row) => row.status === "in_progress" && row.startedAt);
  const [, setTick] = useState(0);
  useEffect(() => {
    if (!ticking) return;
    const timer = window.setInterval(() => setTick((n) => n + 1), 1_000);
    return () => window.clearInterval(timer);
  }, [ticking]);
  useEffect(() => {
    onOpenChange?.(open);
  }, [open, onOpenChange]);
  // 长清单默认折叠只露前几条,底部「展开 N 个 / 收起」切换(参考形态,用户指定)。
  // 每次重开面板回到折叠态——上次翻开的状态对这次没意义。
  const [expanded, setExpanded] = useState(false);
  useEffect(() => {
    if (open) setExpanded(false);
  }, [open]);
  // 面板水平贴的是对话窗格(.chat-main)右缘,不是窗口右缘:改动侧栏是与 .chat-main
  // 并列的右列,写死贴窗口会让面板悬到侧栏上、远离按钮(实拍反馈)。只在打开瞬间量一次
  // 即可——开着期间窗口 resize、点分栏柄/改动钮都会先把菜单关掉(hook 的关闭语义)。
  // 窗格比面板窄时钳到视口内,不许被左缘裁掉(与 hook 的水平钳制同一取舍)。
  const [panelRight, setPanelRight] = useState(10);
  useLayoutEffect(() => {
    if (!open) return;
    const pane = ref.current?.closest(".chat-main")?.getBoundingClientRect();
    const menuW = menuRef.current?.offsetWidth ?? 0;
    const want = pane ? window.innerWidth - pane.right + 10 : 10;
    setPanelRight(Math.max(10, Math.min(want, window.innerWidth - menuW - 6)));
  }, [open, ref, menuRef]);
  const sortedTodos = sortPanelRows(todos);
  const sortedSubs = sortPanelRows(subagents);
  const hiddenCount =
    Math.max(0, sortedTodos.length - TODO_PANEL_COLLAPSED_MAX) +
    Math.max(0, sortedSubs.length - TODO_PANEL_COLLAPSED_MAX);
  const collapsible = hiddenCount > 1;
  const clip = (rows: TodoPanelRow[]) =>
    collapsible && !expanded ? rows.slice(0, TODO_PANEL_COLLAPSED_MAX) : rows;
  const empty = todos.length === 0 && subagents.length === 0;
  return (
    <div className="dd chat-todo-menu" ref={ref} onKeyDown={onKeyDown}>
      <button
        ref={btnRef}
        type="button"
        className="chat-todo-btn"
        aria-haspopup="dialog"
        aria-expanded={open}
        data-tip={t.chat.todoBadgeTip}
        onClick={toggle}
      >
        {/* list-checks:清单 + 勾,与「任务进度」语义对齐。 */}
        <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
          <path d="m3 17 2 2 4-4" /><path d="m3 7 2 2 4-4" />
          <path d="M13 6h8" /><path d="M13 12h8" /><path d="M13 18h8" />
        </svg>
        {/* 活动小点:有任务/子任务在跑时挂在图标右上角脉冲。 */}
        {live && <i className="chat-todo-live" aria-hidden="true" />}
      </button>
      {open && (
        /* 水平不跟按钮对齐:面板统一贴对话窗格右缘、留 10px 间距(用户指定,与参考形态一致)
           ——按钮左侧还有页签等控件,对齐按钮会让面板悬在标题栏中段。垂直仍由 hook 算
           (按钮下方、放不下自动上翻)。 */
        <div className="dd-menu chat-todo-panel" role="dialog" aria-label={t.chat.todoPanelTitle} ref={menuRef} style={{ position: "fixed", top: pos.top, bottom: pos.bottom, right: panelRight }}>
          <div className="chat-todo-panel-head">
            <span className="chat-todo-panel-title">{t.chat.todoPanelTitle}</span>
            {todos.length > 0 && <span className="chat-todos-count">{t.chat.todoProgress(done, todos.length)}</span>}
          </div>
          {empty ? (
            <div className="chat-todo-panel-empty">
              {/* 骨架占位:两行假清单 + 右上勾圈,示意「这里将来长什么样」。 */}
              <div className="chat-todo-skeleton" aria-hidden="true">
                <span className="sk-row"><i className="sk-dot" /><i className="sk-bar" /></span>
                <span className="sk-row"><i className="sk-dot" /><i className="sk-bar is-short" /></span>
                <svg className="sk-check" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round">
                  <circle cx="12" cy="12" r="9" /><path d="m8.5 12 2.5 2.5 4.5-5" />
                </svg>
              </div>
              <div className="chat-todo-panel-hint">{t.chat.todoEmptyHint}</div>
            </div>
          ) : (
            <>
              {/* 行样式独立于底部 TodoPanel 的紧凑清单(用户按参考形态指定):实心圆勾徽章、
                  宽松行距、单行省略。 */}
              {todos.length > 0 && <TodoPanelList rows={clip(sortedTodos)} t={t} />}
              {subagents.length > 0 && (
                <>
                  <div className="chat-todo-panel-head is-sub">
                    <span className="chat-todo-panel-title">{t.chat.todoSubagentSection}</span>
                    <span className="chat-todos-count">{t.chat.todoProgress(subDone, subagents.length)}</span>
                  </div>
                  <TodoPanelList rows={clip(sortedSubs)} t={t} />
                </>
              )}
              {collapsible && (
                <button type="button" className="chat-todo-panel-collapse" onClick={() => setExpanded((v) => !v)}>
                  {expanded ? (
                    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round"><path d="m6 15 6-6 6 6" /></svg>
                  ) : (
                    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round"><path d="m6 9 6 6 6-6" /></svg>
                  )}
                  {expanded ? t.chat.todoCollapse : t.chat.todoExpand(hiddenCount)}
                </button>
              )}
            </>
          )}
        </div>
      )}
    </div>
  );
}

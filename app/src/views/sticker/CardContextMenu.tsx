// 卡片右键/菜单按钮弹出的操作菜单：置顶/便签/重命名/归档/新建会话/打开目录。
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useT } from "../../i18n";
import { ArchiveIcon, FolderIcon, NoteIcon, PencilIcon, PlusIcon, StopIcon, TopIcon } from "./icons";

// 卡片右键菜单：置顶/便签/重命名/归档收拢于此（替代原 hover 图标行，卡片标题行更干净）。
// fixed 定位 + useLayoutEffect 钳位：贴纸窗口小，菜单贴边时向内收、不被窗口边缘裁掉。
// 关闭时机：点菜单外任意处 / Escape / 窗口失焦 / 任一菜单项执行后。
export function CardContextMenu({
  x,
  y,
  starred,
  hasNote,
  archived,
  onStar,
  onNote,
  onRename,
  onArchive,
  onNewSession,
  onOpenDir,
  onEndSession,
  onClose,
}: {
  x: number;
  y: number;
  starred: boolean;
  hasNote: boolean;
  archived: boolean;
  onStar: () => void;
  onNote: () => void;
  onRename: () => void;
  onArchive: () => void;
  /** 用当前会话的路径和模型新建会话。 */
  onNewSession: () => void;
  /** 打开项目目录；会话无 cwd（旧数据）时传 null 隐藏该项。 */
  onOpenDir: (() => void) | null;
  /** 结束会话（杀托管 PTY）；仅本 GUI 托管的会话可结束，其余传 null 隐藏该项。 */
  onEndSession: (() => void) | null;
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
    // 点菜单外关闭：用 click **捕获相**而非 mousedown——捕获相里 stopPropagation 把这次点击
    // 整个拦下，不再传到卡片的 onClick（否则点外部关个菜单会顺手触发卡片点击、把终端打开）。
    // 菜单项在 ref 内不受拦截；本监听在菜单挂载后才注册，打开菜单的那次点击不会误触发。
    const clickAway = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) {
        e.stopPropagation();
        onClose();
      }
    };
    // 右键他处：只关闭本菜单、不拦事件——落在卡片上时让其 onContextMenu 原地弹出新菜单。
    const ctxAway = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) onClose();
    };
    const key = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      // 这次 Esc 被菜单消费掉了,必须标记 preventDefault(同 menu.tsx 的下拉菜单):本菜单
      // 现在也挂在对话窗的侧栏上,而那里有窗口级的「Esc = 拒绝审批」监听(以 defaultPrevented
      // 让路)。document 冒泡在 window 之前——不标记的话,关个右键菜单的同一次按键会顺手
      // 把 agent 的审批请求拒了。
      e.preventDefault();
      onClose();
    };
    document.addEventListener("click", clickAway, true);
    document.addEventListener("contextmenu", ctxAway, true);
    document.addEventListener("keydown", key);
    window.addEventListener("blur", onClose);
    return () => {
      document.removeEventListener("click", clickAway, true);
      document.removeEventListener("contextmenu", ctxAway, true);
      document.removeEventListener("keydown", key);
      window.removeEventListener("blur", onClose);
    };
  }, [onClose]);
  const act = (fn: () => void) => () => {
    fn();
    onClose();
  };
  return (
    <div ref={ref} className="ctx-menu" role="menu" style={pos} onClick={(e) => e.stopPropagation()}>
      <button type="button" role="menuitem" className="ctx-item" onClick={act(onStar)}>
        <TopIcon />
        {starred ? t.sticker.unstar : t.sticker.star}
      </button>
      <button type="button" role="menuitem" className="ctx-item" onClick={act(onNote)}>
        <NoteIcon />
        {hasNote ? t.sticker.noteEdit : t.sticker.noteAdd}
      </button>
      <button type="button" role="menuitem" className="ctx-item" onClick={act(onRename)}>
        <PencilIcon />
        {t.sticker.renameTitle}
      </button>
      <button type="button" role="menuitem" className="ctx-item" onClick={act(onArchive)}>
        <ArchiveIcon archived={archived} />
        {archived ? t.sticker.unarchive : t.sticker.archive}
      </button>
      <div className="ctx-sep" role="separator" />
      <button type="button" role="menuitem" className="ctx-item" onClick={act(onNewSession)}>
        <PlusIcon />
        {t.sticker.newSession}
      </button>
      {onOpenDir && (
        <>
          <div className="ctx-sep" role="separator" />
          <button type="button" role="menuitem" className="ctx-item" onClick={act(onOpenDir)}>
            <FolderIcon />
            {t.sticker.openProjectDir}
          </button>
        </>
      )}
      {onEndSession && (
        <>
          <div className="ctx-sep" role="separator" />
          <button type="button" role="menuitem" className="ctx-item ctx-item-danger" onClick={act(onEndSession)}>
            <StopIcon />
            {t.chat.endSession}
          </button>
        </>
      )}
    </div>
  );
}

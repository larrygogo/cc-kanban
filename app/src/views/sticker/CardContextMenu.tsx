// 卡片右键/菜单按钮弹出的操作菜单：置顶/便签/重命名/归档/新建会话/打开目录。
import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { pushEscLayer } from "../../escLayers";
import { useT } from "../../i18n";
import { ArchiveIcon, FolderIcon, FolderPlusIcon, NoteIcon, PencilIcon, PlusIcon, StopIcon, TopIcon } from "./icons";

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
  onAddDir,
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
  /** 附加目录(一个会话跨多仓,--add-dir):会话 provider 未声明该能力时传 null 隐藏。 */
  onAddDir: (() => void) | null;
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
    // 菜单外滚动关闭：fixed 定位下列表滚走后菜单钉在原地、指向另一张卡（与 menu.tsx 的
    // 下拉同款策略；菜单自身无滚动内容，不用区分内外目标——ctx-menu 不限高）。
    const closeOnScroll = (e: Event) => {
      if (ref.current && e.target instanceof Node && ref.current.contains(e.target)) return;
      onClose();
    };
    // 注册 Esc 层（见 escLayers.ts）：菜单开着期间，窗口级「Esc=拒绝审批」一律让位。
    const popLayer = pushEscLayer();
    document.addEventListener("click", clickAway, true);
    document.addEventListener("contextmenu", ctxAway, true);
    document.addEventListener("keydown", key);
    window.addEventListener("blur", onClose);
    window.addEventListener("scroll", closeOnScroll, true);
    return () => {
      popLayer();
      document.removeEventListener("click", clickAway, true);
      document.removeEventListener("contextmenu", ctxAway, true);
      document.removeEventListener("keydown", key);
      window.removeEventListener("blur", onClose);
      window.removeEventListener("scroll", closeOnScroll, true);
    };
  }, [onClose]);
  // 键盘可达：挂载即把焦点搬进首项（菜单 DOM 挂在列表滚动区之外，Tab 永远走不进来——
  // 这曾让置顶/便签/重命名/归档对键盘用户完全不可达）；关闭时焦点仍在菜单内才归还给
  // 打开前的元素（鼠标用户点外部关闭时焦点已在别处，不抢；某项动作打开了编辑框时，
  // 编辑框的 autoFocus 在归还之后生效、优先级更高）。
  useEffect(() => {
    const prev = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    ref.current?.querySelector<HTMLElement>('[role="menuitem"]')?.focus();
    return () => {
      if (document.activeElement === document.body || ref.current?.contains(document.activeElement)) {
        prev?.focus();
      }
    };
  }, []);
  // roving focus：↑↓/Home/End 搬 DOM 焦点，Enter/Space 激活当前项（与 menu.tsx 同款）。
  const onMenuKeyDown = (e: ReactKeyboardEvent) => {
    const items = Array.from(ref.current?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? []);
    if (items.length === 0) return;
    const cur = items.indexOf(document.activeElement as HTMLElement);
    let next: number;
    if (e.key === "ArrowDown") next = cur >= 0 ? (cur + 1) % items.length : 0;
    else if (e.key === "ArrowUp") next = cur >= 0 ? (cur - 1 + items.length) % items.length : items.length - 1;
    else if (e.key === "Home") next = 0;
    else if (e.key === "End") next = items.length - 1;
    else if ((e.key === "Enter" || e.key === " ") && cur >= 0) {
      // 显式激活：jsdom 不合成按钮的 Enter/Space 点击；preventDefault 挡住原生那次，只触发一回。
      e.preventDefault();
      (document.activeElement as HTMLElement).click();
      return;
    } else return;
    e.preventDefault();
    items[next]?.focus();
  };
  const act = (fn: () => void) => () => {
    fn();
    onClose();
  };
  return (
    <div ref={ref} className="ctx-menu" role="menu" style={pos} onClick={(e) => e.stopPropagation()} onKeyDown={onMenuKeyDown}>
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
      {(onOpenDir || onAddDir) && <div className="ctx-sep" role="separator" />}
      {onOpenDir && (
        <button type="button" role="menuitem" className="ctx-item" onClick={act(onOpenDir)}>
          <FolderIcon />
          {t.sticker.openProjectDir}
        </button>
      )}
      {onAddDir && (
        <button type="button" role="menuitem" className="ctx-item" onClick={act(onAddDir)}>
          <FolderPlusIcon />
          {t.chat.addExtraDir}
        </button>
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

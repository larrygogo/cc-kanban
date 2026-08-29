// 页内 modal 的统一壳（G-3）：overlay + role=dialog + aria-modal 此前在对话窗的三个弹层
// （重命名/快速切换/快捷键表）各写一份，却都没有焦点陷阱与背景 inert——aria-modal 的
// 承诺落空，Tab 会跑出弹层落进背景，屏幕阅读器也读得到背景。这里一次给全：
// Esc 关、Tab 焦点循环、挂载初始焦点、卸载归还焦点、背景 inert。
import { useEffect, useRef, type KeyboardEvent as ReactKeyboardEvent, type ReactNode } from "react";
import { pushEscLayer } from "../escLayers";

const FOCUSABLE = 'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

export function AppModal({
  label,
  className,
  onClose,
  onKeyDown,
  children,
}: {
  /** 对话框的无障碍名（aria-label）。 */
  label: string;
  /** 追加在 .chat-modal 上的样式类（如 chat-switcher / chat-shortcuts）。 */
  className?: string;
  onClose: () => void;
  /** 弹层自己的按键处理（方向键导航、Enter 提交等）；Esc 与 Tab 陷阱由壳统一兜底。 */
  onKeyDown?: (e: ReactKeyboardEvent) => void;
  children: ReactNode;
}) {
  const overlayRef = useRef<HTMLDivElement>(null);
  const dlgRef = useRef<HTMLDivElement>(null);
  // 注册 Esc 层（escLayers.ts）：弹层存续期间窗口级「Esc=拒绝审批」让位。
  useEffect(() => pushEscLayer(), []);
  useEffect(() => {
    const dlg = dlgRef.current;
    const overlay = overlayRef.current;
    if (!dlg || !overlay) return;
    // 背景 inert：aria-modal=true 承诺弹层外不可交互，不 inert 就是空头支票
    // （React 18 不认 inert prop，直写 DOM attribute）。
    const siblings = overlay.parentElement
      ? Array.from(overlay.parentElement.children).filter((el) => el !== overlay)
      : [];
    for (const el of siblings) el.setAttribute("inert", "");
    // 初始焦点：子树的 autoFocus 已生效则不动；否则落到第一个可聚焦元素，没有可聚焦
    // 元素（如纯展示的快捷键表）则聚焦对话框本体，保证 Esc/Tab 有落点。
    const prev = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    if (!dlg.contains(document.activeElement)) {
      (dlg.querySelector<HTMLElement>(FOCUSABLE) ?? dlg).focus();
    }
    return () => {
      for (const el of siblings) el.removeAttribute("inert");
      // 焦点仍在弹层内（或被清到 body）才归还给打开前的元素；点遮罩关闭时焦点已在别处，不抢。
      if (document.activeElement === document.body || dlg.contains(document.activeElement)) {
        prev?.focus();
      }
    };
  }, []);

  const handleKeyDown = (e: ReactKeyboardEvent) => {
    onKeyDown?.(e);
    if (e.defaultPrevented) return;
    if (e.key === "Escape") {
      // Esc 关弹层并截停：preventDefault + stopPropagation 双标记——窗口级「Esc=拒绝审批」
      // 监听靠层栈/标记让路（RenameModal 曾漏掉，关弹层顺手把 agent 的审批请求拒了）。
      e.preventDefault();
      e.stopPropagation();
      onClose();
      return;
    }
    if (e.key === "Tab") {
      // 焦点陷阱：Tab 在弹层内循环，不许落进（已 inert 的）背景。
      const dlg = dlgRef.current;
      if (!dlg) return;
      const items = Array.from(dlg.querySelectorAll<HTMLElement>(FOCUSABLE));
      e.preventDefault();
      if (items.length === 0) return;
      const cur = items.indexOf(document.activeElement as HTMLElement);
      const next = e.shiftKey
        ? cur <= 0 ? items.length - 1 : cur - 1
        : cur < 0 || cur === items.length - 1 ? 0 : cur + 1;
      items[next]?.focus();
    }
  };

  return (
    <div className="chat-modal-overlay" role="presentation" ref={overlayRef} onClick={onClose}>
      <div
        className={"chat-modal" + (className ? " " + className : "")}
        role="dialog"
        aria-modal="true"
        aria-label={label}
        tabIndex={-1}
        ref={dlgRef}
        onClick={(e) => e.stopPropagation()}
        onKeyDown={handleKeyDown}
      >
        {children}
      </div>
    </div>
  );
}

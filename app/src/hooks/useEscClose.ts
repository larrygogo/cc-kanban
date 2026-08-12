import { useEffect, useRef } from "react";

/**
 * Esc 关闭当前窗口/面板——弹出式任务窗（设置/更新/新建会话/引导）的桌面基本预期，
 * 此前六个独立窗口只有 ConfirmWindow 实现了它。
 *
 * 让路规则：
 * - 已被消费的 Esc（defaultPrevented，如菜单/补全）不重复处理；
 * - 焦点在输入框/文本域里时不关窗——那里的 Esc 另有含义（清词/收 IME/取消编辑），
 *   顺手把整扇窗关了会丢用户正在填的内容。
 */
export function useEscClose(onClose: () => void): void {
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented) return;
      const active = document.activeElement;
      if (
        active instanceof HTMLElement
        && (active.tagName === "INPUT" || active.tagName === "TEXTAREA" || active.isContentEditable)
      ) {
        return;
      }
      e.preventDefault();
      onCloseRef.current();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
}

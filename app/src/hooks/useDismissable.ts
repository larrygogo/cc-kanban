import { useEffect, useRef, type RefObject } from "react";
import { pushEscLayer } from "../escLayers";

/**
 * 弹层「点外 / Esc / 滚动关闭」的统一实现（G-1）：卡片右键菜单与下拉菜单曾各维护一套
 * 监听器组合，策略一漂移（一个漏滚动关闭、一个漏 blur 关闭）就是「列表滚走了菜单钉在
 * 另一张卡上」这种只在特定窗口/滚动位置复现的怪 bug。弹层关闭语义全项目只维护这一份。
 *
 * 关闭语义（按选项组合）：
 * - 点容器外：outsideEvent 捕获段判定。pointerdown（默认）先于 mousedown 派发、捕获段
 *   先于任何元素级监听——窗口拖拽区（data-tauri-drag-region）的 mousedown 拦截拦不住它；
 *   interceptOutsideClick 时把这次点击 stopPropagation 整个拦下，不再穿透到下层元素
 *   （右键菜单：点外部关菜单不能顺手触发卡片点击、把终端打开）。
 * - Esc：preventDefault 标记 + 关闭——document 冒泡在 window 之前，不标记的话后面窗口级
 *   监听（如「Esc=拒绝审批」）会把同一次按键当成自己的指令。escFocusReturn 给定时，焦点
 *   确在容器内才归还（鼠标开着弹层、焦点在输入框时按 Esc 不能抢焦点）。一律注册 Esc 层
 *   （escLayers.ts），窗口级监听在栈非空时让位——双保险，互不依赖。
 * - closeOnScroll：容器**外**滚动关闭（fixed 定位弹层的坐标在打开时一次性测量，页面滚走
 *   后与锚点错位、指向另一个目标）；容器自身限高内滚不算「页面滚走了」，不关。
 * - closeOnResize / closeOnBlur / closeOnContextMenu：窗口尺寸/焦点变化、他处右键时关闭
 *   （右键不拦截事件——落在卡片上时让其 onContextMenu 原地弹出新菜单）。
 */
export function useDismissable(
  ref: RefObject<HTMLElement | null>,
  {
    open = true,
    onClose,
    outsideEvent = "pointerdown",
    interceptOutsideClick = false,
    closeOnContextMenu = false,
    closeOnBlur = false,
    closeOnResize = false,
    closeOnScroll = false,
    escFocusReturn,
  }: {
    /** false 时完全不挂监听（菜单关闭态）。 */
    open?: boolean;
    onClose: () => void;
    /** 点外判定用的事件：pointerdown（默认，拖拽区拦不住）或 click（要拦截这次点击时）。 */
    outsideEvent?: "pointerdown" | "click";
    /** 点外关闭时 stopPropagation 拦下这次点击，防止穿透到下层元素的 click 处理。 */
    interceptOutsideClick?: boolean;
    /** 他处右键也关闭（不拦截事件，让落点的 onContextMenu 原地弹出新菜单）。 */
    closeOnContextMenu?: boolean;
    /** 窗口失焦关闭（右键菜单：切窗口后菜单不该还钉着）。 */
    closeOnBlur?: boolean;
    /** 窗口 resize 关闭（fixed 坐标失效）。 */
    closeOnResize?: boolean;
    /** 容器外滚动关闭（capture 捕获内层滚动容器）。 */
    closeOnScroll?: boolean;
    /** Esc 关闭后的焦点归还目标（仅当焦点确实落在容器内时才归还）。 */
    escFocusReturn?: () => HTMLElement | null;
  },
): void {
  // onClose/escFocusReturn 可能是调用方每次渲染新造的闭包 → 经 ref 取最新，
  // 避免关闭语义 effect 反复重挂。
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;
  const focusReturnRef = useRef(escFocusReturn);
  focusReturnRef.current = escFocusReturn;
  useEffect(() => {
    if (!open) return;
    // 注册 Esc 层（见 escLayers.ts）：弹层开着期间，窗口级「Esc=拒绝审批」一律让位。
    const popLayer = pushEscLayer();
    const onPointer = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) {
        if (interceptOutsideClick) e.stopPropagation();
        onCloseRef.current();
      }
    };
    const onCtx = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) onCloseRef.current();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      // 这次 Esc 被弹层消费掉了，必须标记 preventDefault（见 hook  doc 的 Esc 条）。
      e.preventDefault();
      onCloseRef.current();
      if (ref.current?.contains(document.activeElement)) focusReturnRef.current?.()?.focus();
    };
    const close = () => onCloseRef.current();
    const closeOnOutsideScroll = (e: Event) => {
      if (ref.current && e.target instanceof Node && ref.current.contains(e.target)) return;
      onCloseRef.current();
    };
    document.addEventListener(outsideEvent, onPointer, true);
    if (closeOnContextMenu) document.addEventListener("contextmenu", onCtx, true);
    document.addEventListener("keydown", onKey);
    if (closeOnBlur) window.addEventListener("blur", close);
    if (closeOnResize) window.addEventListener("resize", close);
    if (closeOnScroll) window.addEventListener("scroll", closeOnOutsideScroll, true);
    return () => {
      popLayer();
      document.removeEventListener(outsideEvent, onPointer, true);
      if (closeOnContextMenu) document.removeEventListener("contextmenu", onCtx, true);
      document.removeEventListener("keydown", onKey);
      if (closeOnBlur) window.removeEventListener("blur", close);
      if (closeOnResize) window.removeEventListener("resize", close);
      if (closeOnScroll) window.removeEventListener("scroll", closeOnOutsideScroll, true);
    };
  }, [open, outsideEvent, interceptOutsideClick, closeOnContextMenu, closeOnBlur, closeOnResize, closeOnScroll, ref]);
}

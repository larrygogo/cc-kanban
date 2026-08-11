// 全局自定义提示框：替代原生 title 那个迟钝又难看的系统提示。
// 单例挂在文档根，position:fixed 穿透 .stk-scroll/.sticker 等 overflow 裁剪；事件委托读元素的
// data-tip 文案，悬停或键盘聚焦 ~320ms 后在元素附近淡入，默认下方、放不下翻上方，左右夹在窗口内。
// 两个窗口（贴纸 main、设置 about）在 main.tsx 各挂一个即可。
import { useEffect, useLayoutEffect, useRef, useState } from "react";

const SHOW_DELAY = 320; // 悬停多久才弹：原生 title 约 500ms 偏迟钝，这里稍快更跟手
const GAP = 7; // 提示框与锚点元素的垂直间距
const MARGIN = 6; // 距窗口边的最小余量（贴纸窗仅 360px 宽，需夹边防溢出）

type Tip = { text: string; rect: DOMRect } | null;

export function TooltipLayer() {
  const [tip, setTip] = useState<Tip>(null);
  const boxRef = useRef<HTMLDivElement>(null);
  const anchor = useRef<HTMLElement | null>(null);
  const timer = useRef<number | undefined>(undefined);

  useEffect(() => {
    const clear = () => {
      if (timer.current) {
        clearTimeout(timer.current);
        timer.current = undefined;
      }
    };
    const hide = () => {
      clear();
      anchor.current = null;
      setTip(null);
    };
    // 最近的带非空 data-tip 的祖先（嵌套 data-tip 时内层优先）。
    const tipEl = (t: EventTarget | null): HTMLElement | null => {
      if (!(t instanceof Element)) return null;
      // 下拉菜单展开期间整体静默。菜单（.dd-menu）在 DOM 里是触发容器的子孙：容器带
      // data-tip 时，悬停菜单项会命中容器的提示，而提示定位在容器下缘——正糊在菜单顶上
      //（真实翻车过：终端封面的权限下拉）。悬停菜单本身、或容器内还开着菜单，都不弹。
      if (t.closest(".dd-menu")) return null;
      const el = t.closest<HTMLElement>("[data-tip]");
      if (!el || !el.getAttribute("data-tip")) return null;
      // 触发器自己开着弹层（aria-expanded 是所有弹出触发器的通用语义，不止 Dropdown 的
      // .dd-btn.open——侧栏筛选钮就翻车过：菜单开着还弹「筛选与分组」盖在菜单上）。
      if (el.getAttribute("aria-expanded") === "true") return null;
      if (el.querySelector('.dd-btn.open, [aria-expanded="true"]')) return null;
      return el;
    };
    // 悬停（mouseover）与键盘聚焦（focusin）走同一套延时显示；focusin 会冒泡，document 上能收到。
    const show = (t: EventTarget | null) => {
      const el = tipEl(t);
      if (!el || el === anchor.current) return;
      anchor.current = el;
      clear();
      setTip(null); // 先撤掉上一个，切换元素时不串显旧文案
      timer.current = window.setTimeout(() => {
        if (anchor.current !== el) return;
        setTip({ text: el.getAttribute("data-tip") || "", rect: el.getBoundingClientRect() });
      }, SHOW_DELAY);
    };
    const onOver = (e: MouseEvent) => show(e.target);
    // 聚焦弹提示只服务键盘导航（input-modality 的 data-im="kbd"）。鼠标路径下的 focusin
    // 多是**程序化 focus**——菜单收起时把焦点还给触发钮（useMenuPopup/SidebarFilterMenu
    // 的 btnRef.focus()），若不区分，每次收起菜单都强制弹一次提示（用户实拍反馈）。
    const onFocusIn = (e: FocusEvent) => {
      if (document.documentElement.getAttribute("data-im") !== "kbd") return;
      show(e.target);
    };
    const maybeHide = (t: EventTarget | null, to: EventTarget | null) => {
      const el = tipEl(t);
      if (!el || el !== anchor.current) return;
      if (to instanceof Node && el.contains(to)) return; // 移入/聚焦到自身子节点不算离开
      hide();
    };
    const onOut = (e: MouseEvent) => maybeHide(e.target, e.relatedTarget);
    const onFocusOut = (e: FocusEvent) => maybeHide(e.target, e.relatedTarget);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") hide();
    };
    document.addEventListener("mouseover", onOver);
    document.addEventListener("mouseout", onOut);
    document.addEventListener("focusin", onFocusIn);
    document.addEventListener("focusout", onFocusOut);
    document.addEventListener("keydown", onKey);
    // 位置是一次性测量的：滚动/点击/窗口失焦后会与锚点错位，直接收起。
    window.addEventListener("scroll", hide, true);
    window.addEventListener("blur", hide);
    document.addEventListener("mousedown", hide);
    return () => {
      clear();
      document.removeEventListener("mouseover", onOver);
      document.removeEventListener("mouseout", onOut);
      document.removeEventListener("focusin", onFocusIn);
      document.removeEventListener("focusout", onFocusOut);
      document.removeEventListener("keydown", onKey);
      window.removeEventListener("scroll", hide, true);
      window.removeEventListener("blur", hide);
      document.removeEventListener("mousedown", hide);
    };
  }, []);

  // 量好提示框尺寸后定位（useLayoutEffect 在绘制前执行，首帧即落到正确位置，无左上角闪现）：
  // 默认锚点下方居中，下方放不下且上方够则翻到上方，最后左右上下都夹在窗口内。
  useLayoutEffect(() => {
    const box = boxRef.current;
    if (!tip || !box) return;
    const { rect } = tip;
    const bw = box.offsetWidth;
    const bh = box.offsetHeight;
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    let left = rect.left + rect.width / 2 - bw / 2;
    left = Math.max(MARGIN, Math.min(left, vw - bw - MARGIN));
    const below = rect.bottom + GAP;
    const above = rect.top - GAP - bh;
    let top = below;
    if (below + bh > vh - MARGIN && above >= MARGIN) top = above;
    top = Math.max(MARGIN, Math.min(top, vh - bh - MARGIN));
    box.style.left = `${Math.round(left)}px`;
    box.style.top = `${Math.round(top)}px`;
  }, [tip]);

  if (!tip) return null;
  return (
    <div ref={boxRef} className="tip" role="tooltip" style={{ left: 0, top: 0 }}>
      {tip.text}
    </div>
  );
}

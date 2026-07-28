// 弹层菜单的唯一实现：开关状态、定位、关闭语义、方向键导航。
// 此前这套行为一式三份（Dropdown.tsx / settings/widgets.tsx / ChatWindow.tsx），
// 改一处忘另一处必然漂移——翻转阈值、Esc 焦点归还任何一处对不上，就是
// 「有的菜单被窗口底边切掉」这种只在特定滚动位置复现的怪 bug。全项目只维护这一份。
//
// 三个消费者：
// - `Dropdown`（选值）、`ActionMenu`（执行动作）：本文件内的组件，fixed 定位；
// - 对话窗模型/模式菜单：CSS 绝对定位 + 互斥状态在父组件，受控复用 `useMenuPopup`；
// - RelayAccess 的 ModelPicker 是 combobox（输入过滤 + aria-activedescendant），模式不同，不并入。
import { useEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent, type ReactElement } from "react";

/** 菜单定位坐标（仅 fixed 模式；`cssPositioned` 时恒为空对象，定位交给 CSS）。 */
type MenuPos = { top?: number; bottom?: number; left?: number; right?: number; width?: number };

/**
 * 弹层菜单的共享行为。
 *
 * 关闭语义：点外部关；Esc 关（焦点在菜单里时归还触发钮）；fixed 模式下窗口 resize / 滚动也关
 * ——菜单坐标在打开时一次性测量，滚动后与按钮错位，故滚动即关（capture 捕获内层滚动）。
 *
 * 键盘模型：**roving focus**（与 SwatchPicker 的 roving tabindex 同族，全项目统一这一种）：
 * ↑/↓ 在菜单项间循环移动 DOM 焦点，Home/End 跳首尾，Enter/Space 激活焦点项；
 * 焦点还在触发钮上时 ↓ 落到当前选中项（无选中则首项）、↑ 落末项。
 * 菜单打开本身不抢焦点（点击打开后焦点留在触发钮），第一根方向键才进菜单。
 */
export function useMenuPopup({
  itemCount = 0,
  align = "right",
  cssPositioned = false,
  open: controlledOpen,
  setOpen: controlledSetOpen,
}: {
  /** 菜单项数：仅 fixed 定位时用来估菜单高、决定向下弹还是向上翻。 */
  itemCount?: number;
  /**
   * fixed 定位时的水平对齐：
   * - `"right"`（默认）：菜单右边对齐按钮右边（设置页行尾控件），宽度随内容（受 `.dd .dd-menu` 钳制）。
   * - `"left"`：菜单左边对齐按钮左边、宽度钉成按钮宽（新建会话的整宽表单控件）。
   */
  align?: "left" | "right";
  /**
   * true = 菜单定位交给 CSS（如对话窗 compose 区里 `position:absolute` 的上弹菜单）：
   * hook 不测坐标；菜单随内容滚动、与按钮不错位，故滚动/resize 也不关。
   */
  cssPositioned?: boolean;
  /** 受控用法：开关状态由外部持有时传入（如对话窗两个菜单的互斥写在父组件状态里）。 */
  open?: boolean;
  setOpen?: (open: boolean) => void;
} = {}) {
  const [innerOpen, setInnerOpen] = useState(false);
  const open = controlledOpen ?? innerOpen;
  const setOpen: (open: boolean) => void = controlledSetOpen ?? setInnerOpen;
  // 受控的 setOpen 可能是调用方每次渲染新造的闭包 → 经 ref 取，避免关闭语义 effect 反复重挂。
  const setOpenRef = useRef(setOpen);
  setOpenRef.current = setOpen;
  const [pos, setPos] = useState<MenuPos>({});
  const ref = useRef<HTMLDivElement>(null); // 容器：click-away 边界、菜单项查询范围
  const btnRef = useRef<HTMLButtonElement>(null); // 触发钮：定位锚点、焦点归还目标

  useEffect(() => {
    if (!open) return;
    const setOpen = setOpenRef.current;
    const onDoc = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    const close = () => setOpen(false);
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      // 这次 Esc 被菜单消费掉了,必须标记 preventDefault:document 冒泡在 window 之前,
      // 后面还有 ChatWindow 的窗口级「Esc=拒绝审批」监听(它以 defaultPrevented 让路)。
      // 不标记的话,关个下拉菜单的同一次按键会顺手把 agent 的审批请求拒了。
      e.preventDefault();
      setOpen(false);
      // Esc 后焦点归还触发钮，键盘用户不至于丢焦点；但只在焦点确实落在菜单容器内时归还——
      // 鼠标开着菜单、焦点在输入框时按 Esc 不能把焦点抢回按钮。
      if (ref.current?.contains(document.activeElement)) btnRef.current?.focus();
    };
    document.addEventListener("mousedown", onDoc);
    document.addEventListener("keydown", onKey);
    if (!cssPositioned) {
      window.addEventListener("resize", close);
      window.addEventListener("scroll", close, true);
    }
    return () => {
      document.removeEventListener("mousedown", onDoc);
      document.removeEventListener("keydown", onKey);
      if (!cssPositioned) {
        window.removeEventListener("resize", close);
        window.removeEventListener("scroll", close, true);
      }
    };
  }, [open, cssPositioned]);

  const toggle = () => {
    if (!open && !cssPositioned) {
      const r = btnRef.current?.getBoundingClientRect();
      if (r) {
        // 估算菜单高（项高约 30px + 容器内边距），下方放不下且上方空间更充裕时向上弹。
        // WebView 的内容无法溢出原生窗口，所以按钮靠近窗口底部时必须向上翻转。
        const estHeight = itemCount * 30 + 10;
        const fitsBelow = r.bottom + 6 + estHeight <= window.innerHeight;
        const vert = !fitsBelow && r.top > window.innerHeight - r.bottom
          ? { bottom: window.innerHeight - r.top + 6 }
          : { top: r.bottom + 6 };
        setPos(align === "left"
          ? { ...vert, left: r.left, width: r.width }
          : { ...vert, right: Math.max(0, window.innerWidth - r.right) });
      }
    }
    setOpen(!open);
  };

  // 方向键导航，挂在容器上（事件从触发钮或菜单项冒泡上来）。roving focus：直接搬 DOM 焦点
  // 而不是 aria-activedescendant——菜单里没有输入框，焦点落在哪项，Enter/Space 就激活哪项。
  const onKeyDown = (e: ReactKeyboardEvent) => {
    if (!open) return;
    const items = Array.from(ref.current?.querySelectorAll<HTMLElement>('[role="menuitem"], [role="option"]') ?? []);
    if (items.length === 0) return;
    const cur = items.indexOf(document.activeElement as HTMLElement);
    let next: number | null = null;
    if (e.key === "ArrowDown") {
      // 焦点还在菜单外（触发钮上）：落到当前选中项，无选中则首项。
      next = cur >= 0 ? (cur + 1) % items.length : Math.max(0, items.findIndex((el) => el.getAttribute("aria-selected") === "true"));
    } else if (e.key === "ArrowUp") {
      next = cur >= 0 ? (cur - 1 + items.length) % items.length : items.length - 1;
    } else if (e.key === "Home") {
      next = 0;
    } else if (e.key === "End") {
      next = items.length - 1;
    } else if ((e.key === "Enter" || e.key === " ") && cur >= 0) {
      // 显式激活：jsdom 不合成按钮的 Enter/Space 点击；preventDefault 挡住浏览器原生那次，保证只触发一回。
      e.preventDefault();
      (document.activeElement as HTMLElement).click();
      return;
    } else {
      return;
    }
    e.preventDefault();
    items[next]?.focus();
  };

  return { open, setOpen, pos, ref, btnRef, toggle, onKeyDown };
}

/**
 * 选值下拉（替代原生 select，使下拉列表也跟随主题、圆角一致）。
 * icon 可选：给「选择器」型下拉（如账号页的模型切换）在按钮与每个选项前挂一个徽标；
 * muted 可选：把该选项显示为「次要/未就绪」（如未安装的 agent）——置灰、沉底由调用方排序。
 * 都不传则退化成纯文字下拉。
 */
export function Dropdown<T extends string | number>({
  value,
  options,
  onChange,
  align = "right",
}: {
  value: T;
  options: { value: T; label: string; icon?: ReactElement; muted?: boolean }[];
  onChange: (v: T) => void;
  /** 水平对齐：默认右对齐（设置页行尾）；`"left"` 左对齐并钉成按钮宽（新建会话的整宽表单）。 */
  align?: "left" | "right";
}) {
  const { open, setOpen, pos, ref, btnRef, toggle, onKeyDown } = useMenuPopup({ itemCount: options.length, align });
  const cur = options.find((o) => o.value === value);
  return (
    <div className="dd" ref={ref} onKeyDown={onKeyDown}>
      <button
        ref={btnRef}
        type="button"
        className={"dd-btn" + (open ? " open" : "")}
        aria-haspopup="listbox"
        aria-expanded={open}
        onClick={toggle}
      >
        <span className="dd-val">
          {cur?.icon && <span className="dd-ico">{cur.icon}</span>}
          <span className="dd-label">{cur?.label ?? ""}</span>
        </span>
        <svg className="dd-chev" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round">
          <polyline points="6 9 12 15 18 9" />
        </svg>
      </button>
      {open && (
        <div className="dd-menu" role="listbox" style={{ position: "fixed", top: pos.top, bottom: pos.bottom, left: pos.left, right: pos.right, width: pos.width }}>
          {options.map((o) => (
            <button
              type="button"
              role="option"
              aria-selected={o.value === value}
              key={o.value}
              className={"dd-item" + (o.value === value ? " sel" : "") + (o.muted ? " muted" : "")}
              onClick={() => {
                onChange(o.value);
                setOpen(false);
                btnRef.current?.focus(); // 选中后焦点归还触发按钮
              }}
            >
              <span className="dd-val">
                {o.icon && <span className="dd-ico">{o.icon}</span>}
                <span className="dd-label">{o.label}</span>
              </span>
              {o.value === value && (
                <svg className="dd-check" width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round">
                  <polyline points="20 6 9 17 4 12" />
                </svg>
              )}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

/**
 * 动作菜单（`⋯`）：点一项就执行它，**没有「当前选中值」**——这是它与 `Dropdown` 的根本区别。
 *
 * 用于把一行里挤成一排的按钮收进去（账号行的 退出登录 / 重命名 / 删除）。
 */
export function ActionMenu({
  items,
  label,
  testId,
}: {
  items: { key: string; label: string; danger?: boolean; onSelect: () => void }[];
  /** 触发按钮的无障碍名（也用作 tooltip）。 */
  label: string;
  testId?: string;
}) {
  const { open, setOpen, pos, ref, btnRef, toggle, onKeyDown } = useMenuPopup({ itemCount: items.length });
  if (items.length === 0) return null;
  return (
    <div className="dd" ref={ref} onKeyDown={onKeyDown}>
      <button
        ref={btnRef}
        type="button"
        className="icon-btn"
        aria-label={label}
        aria-haspopup="menu"
        aria-expanded={open}
        data-tip={label}
        data-testid={testId}
        onClick={toggle}
      >
        <svg width="15" height="15" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
          <circle cx="5" cy="12" r="1.6" />
          <circle cx="12" cy="12" r="1.6" />
          <circle cx="19" cy="12" r="1.6" />
        </svg>
      </button>
      {open && (
        <div className="dd-menu" role="menu" style={{ position: "fixed", top: pos.top, bottom: pos.bottom, right: pos.right }}>
          {items.map((it) => (
            <button
              key={it.key}
              type="button"
              role="menuitem"
              className={"dd-item" + (it.danger ? " dd-item-danger" : "")}
              data-testid={testId ? `${testId}-${it.key}` : undefined}
              onClick={() => {
                setOpen(false);
                it.onSelect();
              }}
            >
              <span>{it.label}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

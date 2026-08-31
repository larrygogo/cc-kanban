// 弹层菜单的唯一实现：开关状态、定位、关闭语义、方向键导航。
// 此前这套行为一式三份（Dropdown.tsx / settings/widgets.tsx / ChatWindow.tsx），
// 改一处忘另一处必然漂移——翻转阈值、Esc 焦点归还任何一处对不上，就是
// 「有的菜单被窗口底边切掉」这种只在特定滚动位置复现的怪 bug。全项目只维护这一份。
//
// 三个消费者：
// - `Dropdown`（选值）、`ActionMenu`（执行动作）：本文件内的组件，fixed 定位；
// - 对话窗模型/模式菜单：CSS 绝对定位 + 互斥状态在父组件，受控复用 `useMenuPopup`；
// - RelayAccess 的 ModelPicker 是 combobox（输入过滤 + aria-activedescendant），模式不同，不并入。
import { useCallback, useLayoutEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent, type ReactElement } from "react";
import { useDismissable } from "../hooks/useDismissable";
import { CheckIcon, ChevronDownIcon } from "./sticker/icons";

/** 菜单定位坐标（仅 fixed 模式；`cssPositioned` 时恒为空对象，定位交给 CSS）。
    maxHeight 不在这里：它由 relayout 直接写 DOM（imperative 全权管理）——曾放进 pos，
    关闭重开时「先直写 DOM 清掉、再 setPos 出相同值」让 React 的样式 diff 视为无变化
    跳过写回，钳制静默丢失（评审确认的回归）。 */
type MenuPos = { top?: number; bottom?: number; left?: number; right?: number; width?: number };

/**
 * 弹层菜单的共享行为。
 *
 * 关闭语义：点外部关；Esc 关（焦点在菜单里时归还触发钮）；fixed 模式下窗口 resize / 滚动也关
 * ——菜单坐标在打开时一次性测量，滚动后与按钮错位，故滚动即关（capture 捕获内层滚动）。
 *
 * 键盘模型：**roving focus**（与 SwatchPicker 的 roving tabindex 同族，全项目统一这一种）：
 * ↑/↓ 在菜单项间循环移动 DOM 焦点，Home/End 跳首尾，Enter/Space 激活焦点项；
 * 可打印字符按前缀跳项（typeahead，300ms 内连续击键拼前缀），替代原生 select 的首字母跳转；
 * 焦点还在触发钮上时 ↓ 落到当前选中项（无选中则首项）、↑ 落末项。
 * 菜单打开本身不抢焦点（点击打开后焦点留在触发钮），第一根方向键才进菜单。
 */
export function useMenuPopup({
  align = "right",
  cssPositioned = false,
  open: controlledOpen,
  setOpen: controlledSetOpen,
}: {
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
  const menuRef = useRef<HTMLDivElement>(null); // 菜单本体：fixed 模式量真实高度用

  // fixed 定位：菜单挂进 DOM 后、首帧绘制前量**真实高度**再定坐标。
  // 曾在 toggle 里按 itemCount*30+10 估高决定翻转，估矮了就误判「下方放得下」→
  // 向下弹、被窗口底边切掉（新建会话面板的权限下拉，实拍反馈）。useLayoutEffect
  // 同步跑在绘制前，用户看不到未定位的那一帧。
  //
  // 抽成可调用的 relayout：菜单在打开状态里内容变高时（如筛选面板从根视图切到
  // 目录列表）调用方要能主动重测——首开时那次测量对新高度一无所知。
  const relayout = useCallback(() => {
    if (!open || cssPositioned) return;
    const r = btnRef.current?.getBoundingClientRect();
    const menu = menuRef.current;
    if (!r || !menu) return;
    // maxHeight 全程直写 DOM、不进 React style：量高前清掉旧钳制（否则量到被压扁的
    // 高度），放不下再写回。走 React 的话「清掉→setPos 同值」会被样式 diff 跳过写回，
    // 钳制在第二次打开时静默丢失（评审确认）。
    menu.style.maxHeight = "";
    const menuH = menu.offsetHeight;
    // 上下各留 6px 按钮间隙 + 6px 贴边余量。下方放得下就向下；否则弹向空间更大的一侧，
    // 连那侧也塞不下整个菜单时钳 maxHeight（.dd-menu overflow-y:auto 内滚兜底）。
    const below = window.innerHeight - r.bottom - 12;
    const above = r.top - 12;
    const openUp = menuH > below && above > below;
    const avail = openUp ? above : below;
    if (menuH > avail) menu.style.maxHeight = `${Math.max(avail, 60)}px`;
    const vert: MenuPos = openUp
      ? { bottom: window.innerHeight - r.top + 6 }
      : { top: r.bottom + 6 };
    // 水平钳制：菜单比锚点侧空间宽时（如 208px 侧栏里右对齐一个 224px 面板），
    // 不许越出视口——宁可离开与按钮的对齐线，也不能被窗口边缘裁掉（实拍反馈）。
    const menuW = menu.offsetWidth;
    setPos(align === "left"
      ? { ...vert, left: Math.max(6, Math.min(r.left, window.innerWidth - menuW - 6)), width: r.width }
      : { ...vert, right: Math.max(6, Math.min(window.innerWidth - r.right, window.innerWidth - menuW - 6)) });
  }, [open, cssPositioned, align]);
  useLayoutEffect(() => { relayout(); }, [relayout]);

  // 关闭语义统一走 useDismissable（G-1，与 CardContextMenu 同一份实现）：点外关、
  // Esc 关（焦点在菜单里时归还触发钮）；fixed 模式下 resize / 菜单外滚动也关——
  // 菜单坐标在打开时一次性测量，滚动后与按钮错位，故滚动即关（capture 捕获内层滚动）。
  useDismissable(ref, {
    open,
    onClose: () => setOpenRef.current(false),
    closeOnResize: !cssPositioned,
    closeOnScroll: !cssPositioned,
    escFocusReturn: () => btnRef.current,
  });

  const toggle = () => setOpen(!open);

  // typeahead 缓冲：300ms 内的连续击键拼成前缀，超时重开。
  const typeaheadRef = useRef({ buf: "", at: 0 });

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
    } else if (e.key.length === 1 && e.key !== " " && !e.ctrlKey && !e.metaKey && !e.altKey) {
      // 可打印字符 typeahead：菜单里没有输入框，击键只能用来跳项。从焦点下一项起循环找——
      // 重复敲同一字母就在同首字母的几项间轮转。空格除外：它在上面是激活键，
      // 且焦点还在触发钮上时要留给按钮的原生空格行为，不能吞。
      const now = Date.now();
      const buf = now - typeaheadRef.current.at <= 300 ? typeaheadRef.current.buf + e.key : e.key;
      typeaheadRef.current = { buf, at: now };
      const needle = buf.toLowerCase();
      const from = cur >= 0 ? cur + 1 : 0;
      for (let i = 0; i < items.length; i++) {
        const idx = (from + i) % items.length;
        if ((items[idx].textContent ?? "").trim().toLowerCase().startsWith(needle)) { next = idx; break; }
      }
      if (next === null) return;
    } else {
      return;
    }
    e.preventDefault();
    items[next]?.focus();
  };

  return { open, setOpen, pos, ref, btnRef, menuRef, toggle, onKeyDown, relayout };
}

/**
 * 选值下拉（替代原生 select，使下拉列表也跟随主题、圆角一致）。
 * icon 可选：给「选择器」型下拉（如账号页的模型切换）在按钮与每个选项前挂一个徽标；
 * muted 可选：把该选项显示为「次要/未就绪」（如未安装的 agent）——置灰、沉底由调用方排序。
 * risky 可选：高风险档（如跳过权限确认）——警示色渲染，sub 附一行风险说明（契约
 * LaunchChoice.risk 的展示侧）。
 * 都不传则退化成纯文字下拉。
 */
export function Dropdown<T extends string | number>({
  value,
  options,
  onChange,
  align = "right",
  disabled,
}: {
  value: T;
  options: { value: T; label: string; icon?: ReactElement; muted?: boolean; risky?: boolean; sub?: string }[];
  onChange: (v: T) => void;
  /** 水平对齐：默认右对齐（设置页行尾）；`"left"` 左对齐并钉成按钮宽（新建会话的整宽表单）。 */
  align?: "left" | "right";
  /** 被上游设置门控时置灰（行保留 + 原因写在 row-desc），而不是整行隐藏。 */
  disabled?: boolean;
}) {
  const { open, setOpen, pos, ref, btnRef, menuRef, toggle, onKeyDown } = useMenuPopup({ align });
  const cur = options.find((o) => o.value === value);
  return (
    <div className="dd" ref={ref} onKeyDown={onKeyDown}>
      <button
        ref={btnRef}
        type="button"
        className={"dd-btn" + (open ? " open" : "")}
        aria-haspopup="listbox"
        aria-expanded={open}
        disabled={disabled}
        onClick={toggle}
      >
        <span className="dd-val">
          {cur?.icon && <span className="dd-ico">{cur.icon}</span>}
          <span className="dd-label">{cur?.label ?? ""}</span>
        </span>
        <ChevronDownIcon className="dd-chev" />
      </button>
      {open && (
        <div className="dd-menu" role="listbox" ref={menuRef} style={{ position: "fixed", top: pos.top, bottom: pos.bottom, left: pos.left, right: pos.right, width: pos.width }}>
          {options.map((o) => (
            <button
              type="button"
              role="option"
              aria-selected={o.value === value}
              key={o.value}
              className={"dd-item" + (o.value === value ? " sel" : "") + (o.muted ? " muted" : "") + (o.risky ? " risk" : "")}
              onClick={() => {
                onChange(o.value);
                setOpen(false);
                btnRef.current?.focus(); // 选中后焦点归还触发按钮
              }}
            >
              <span className="dd-val">
                {o.icon && <span className="dd-ico">{o.icon}</span>}
                <span className="dd-label">
                  {o.label}
                  {o.sub && <span className="dd-sub">{o.sub}</span>}
                </span>
              </span>
              {o.value === value && <CheckIcon className="dd-check" />}
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
  const { open, setOpen, pos, ref, btnRef, menuRef, toggle, onKeyDown } = useMenuPopup({});
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
        <div className="dd-menu" role="menu" ref={menuRef} style={{ position: "fixed", top: pos.top, bottom: pos.bottom, right: pos.right }}>
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

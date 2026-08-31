import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { ActionMenu, Dropdown } from "./menu";

afterEach(cleanup);

const options = [
  { value: "a", label: "方案 A" },
  { value: "b", label: "方案 B" },
  { value: "c", label: "方案 C" },
];

function renderDropdown(
  onChange: (v: string) => void = () => {},
  opts = options,
  initial = "b",
) {
  function Host() {
    const [v, setV] = useState(initial);
    return <Dropdown value={v} options={opts} onChange={(x) => { setV(x); onChange(x); }} />;
  }
  render(<Host />);
}

describe("Dropdown（统一菜单 primitive）", () => {
  it("点击打开，aria-expanded 同步；点选项后关闭并把焦点还给触发钮", () => {
    const onChange = vi.fn();
    renderDropdown(onChange);
    const btn = screen.getByRole("button");
    fireEvent.click(btn);
    expect(btn.getAttribute("aria-expanded")).toBe("true");
    fireEvent.click(screen.getByRole("option", { name: "方案 A" }));
    expect(onChange).toHaveBeenCalledWith("a");
    expect(screen.queryByRole("option")).toBeNull();
    expect(btn.getAttribute("aria-expanded")).toBe("false");
    expect(document.activeElement).toBe(btn);
  });

  it("方向键在选项间循环移动焦点：↓ 从当前选中项起步，Home/End 跳首尾", () => {
    renderDropdown();
    const btn = screen.getByRole("button");
    fireEvent.click(btn);
    const container = btn.parentElement as HTMLElement;
    // 焦点还在触发钮：↓ 落到当前选中项（方案 B）
    fireEvent.keyDown(container, { key: "ArrowDown" });
    expect(document.activeElement).toBe(screen.getByRole("option", { name: "方案 B" }));
    // ↓ 循环：B → C → A
    fireEvent.keyDown(container, { key: "ArrowDown" });
    expect(document.activeElement).toBe(screen.getByRole("option", { name: "方案 C" }));
    fireEvent.keyDown(container, { key: "ArrowDown" });
    expect(document.activeElement).toBe(screen.getByRole("option", { name: "方案 A" }));
    // ↑ 反向循环
    fireEvent.keyDown(container, { key: "ArrowUp" });
    expect(document.activeElement).toBe(screen.getByRole("option", { name: "方案 C" }));
    // Home/End
    fireEvent.keyDown(container, { key: "Home" });
    expect(document.activeElement).toBe(screen.getByRole("option", { name: "方案 A" }));
    fireEvent.keyDown(container, { key: "End" });
    expect(document.activeElement).toBe(screen.getByRole("option", { name: "方案 C" }));
  });

  it("Enter 激活焦点项：选中、关闭、焦点归还触发钮", () => {
    const onChange = vi.fn();
    renderDropdown(onChange);
    const btn = screen.getByRole("button");
    fireEvent.click(btn);
    const container = btn.parentElement as HTMLElement;
    fireEvent.keyDown(container, { key: "ArrowDown" }); // → 方案 B
    fireEvent.keyDown(container, { key: "ArrowDown" }); // → 方案 C
    fireEvent.keyDown(container, { key: "Enter" });
    expect(onChange).toHaveBeenCalledWith("c");
    expect(screen.queryByRole("option")).toBeNull();
    expect(document.activeElement).toBe(btn);
  });

  it("Space 同样激活焦点项", () => {
    const onChange = vi.fn();
    renderDropdown(onChange);
    const btn = screen.getByRole("button");
    fireEvent.click(btn);
    const container = btn.parentElement as HTMLElement;
    fireEvent.keyDown(container, { key: "ArrowUp" }); // 焦点在触发钮上：↑ 落末项（方案 C）
    expect(document.activeElement).toBe(screen.getByRole("option", { name: "方案 C" }));
    fireEvent.keyDown(container, { key: " " });
    expect(onChange).toHaveBeenCalledWith("c");
    expect(screen.queryByRole("option")).toBeNull();
  });

  it("Esc 关闭并把焦点还给触发钮（焦点在菜单里时）；点外部关闭", () => {
    renderDropdown();
    const btn = screen.getByRole("button");
    fireEvent.click(btn);
    const container = btn.parentElement as HTMLElement;
    fireEvent.keyDown(container, { key: "ArrowDown" });
    expect(document.activeElement).not.toBe(btn);
    // fireEvent 返回 false = 事件被 preventDefault:菜单消费掉这次 Esc 必须做标记,
    // 否则同一次按键会继续落到 ChatWindow 的窗口级「Esc=拒绝审批」监听上,
    // 关个菜单顺手把 agent 的审批请求拒了。
    expect(fireEvent.keyDown(document, { key: "Escape" })).toBe(false);
    expect(screen.queryByRole("option")).toBeNull();
    expect(document.activeElement).toBe(btn);
    // 重新打开后点外部（pointerdown 落在容器之外）关闭。用 pointerdown 而非 mousedown：
    // 实现挂在捕获段的 pointerdown 上，拖拽区（data-tauri-drag-region）消费 mousedown
    // 也拦不住它（见 useMenuPopup 内注释）。
    fireEvent.click(btn);
    expect(screen.getByRole("option", { name: "方案 A" })).toBeTruthy();
    fireEvent.pointerDown(document.body);
    expect(screen.queryByRole("option")).toBeNull();
  });

  it("焦点不在菜单里时 Esc 只关菜单、不抢焦点", () => {
    render(
      <>
        <input data-testid="outside" />
        <Dropdown value="a" options={options} onChange={() => {}} />
      </>,
    );
    const btn = screen.getByRole("button");
    fireEvent.click(btn);
    (screen.getByTestId("outside") as HTMLElement).focus();
    fireEvent.keyDown(document, { key: "Escape" });
    expect(screen.queryByRole("option")).toBeNull();
    expect(document.activeElement).toBe(screen.getByTestId("outside"));
  });
});

describe("Dropdown typeahead（可打印字符跳项）", () => {
  const latin = [
    { value: "backup", label: "Backup" },
    { value: "batch", label: "Batch" },
    { value: "beta", label: "Beta" },
  ];

  afterEach(() => vi.useRealTimers());

  it("单字母跳转：按 label 首字母循环匹配，从焦点下一项起找", () => {
    renderDropdown(() => {}, latin, "backup");
    const btn = screen.getByRole("button");
    fireEvent.click(btn);
    const container = btn.parentElement as HTMLElement;
    // 焦点还在触发钮（不在菜单项上）：从头找，b → 首个 B 开头项
    fireEvent.keyDown(container, { key: "b" });
    expect(document.activeElement).toBe(screen.getByRole("option", { name: "Backup" }));
  });

  it("同首字母多选项轮转：缓冲超时（>300ms）后重敲同字母跳到下一个", () => {
    vi.useFakeTimers();
    renderDropdown(() => {}, latin, "backup");
    const btn = screen.getByRole("button");
    fireEvent.click(btn);
    const container = btn.parentElement as HTMLElement;
    fireEvent.keyDown(container, { key: "b" });
    expect(document.activeElement).toBe(screen.getByRole("option", { name: "Backup" }));
    // 300ms 窗口内重敲会拼成 "bb"（无匹配），轮转发生在缓冲超时重开之后
    vi.advanceTimersByTime(301);
    fireEvent.keyDown(container, { key: "b" });
    expect(document.activeElement).toBe(screen.getByRole("option", { name: "Batch" }));
    vi.advanceTimersByTime(301);
    fireEvent.keyDown(container, { key: "b" });
    expect(document.activeElement).toBe(screen.getByRole("option", { name: "Beta" }));
    // 到底后循环回首个匹配项
    vi.advanceTimersByTime(301);
    fireEvent.keyDown(container, { key: "b" });
    expect(document.activeElement).toBe(screen.getByRole("option", { name: "Backup" }));
  });

  it("300ms 内连续击键拼前缀：跨前缀收窄到唯一匹配项", () => {
    vi.useFakeTimers();
    renderDropdown(() => {}, latin, "backup");
    const btn = screen.getByRole("button");
    fireEvent.click(btn);
    const container = btn.parentElement as HTMLElement;
    fireEvent.keyDown(container, { key: "b" });
    expect(document.activeElement).toBe(screen.getByRole("option", { name: "Backup" }));
    // 窗口内再敲 e：缓冲拼成 "be"，跳过同样 B 开头的 Backup/Batch，命中 Beta
    vi.advanceTimersByTime(100);
    fireEvent.keyDown(container, { key: "e" });
    expect(document.activeElement).toBe(screen.getByRole("option", { name: "Beta" }));
  });

  it("无匹配时焦点不动", () => {
    renderDropdown(() => {}, latin, "backup");
    const btn = screen.getByRole("button");
    fireEvent.click(btn);
    const container = btn.parentElement as HTMLElement;
    fireEvent.keyDown(container, { key: "ArrowDown" }); // → 当前选中项 Backup
    const focused = document.activeElement;
    fireEvent.keyDown(container, { key: "z" });
    expect(document.activeElement).toBe(focused);
  });
});

describe("ActionMenu（统一菜单 primitive）", () => {
  it("方向键导航（无当前值，↓ 落首项）+ Enter 执行动作并关闭菜单", () => {
    const onRename = vi.fn();
    const onDelete = vi.fn();
    render(
      <ActionMenu
        label="更多"
        items={[
          { key: "rename", label: "重命名", onSelect: onRename },
          { key: "delete", label: "删除", danger: true, onSelect: onDelete },
        ]}
      />,
    );
    const btn = screen.getByRole("button", { name: "更多" });
    fireEvent.click(btn);
    expect(btn.getAttribute("aria-expanded")).toBe("true");
    const container = btn.parentElement as HTMLElement;
    fireEvent.keyDown(container, { key: "ArrowDown" });
    expect(document.activeElement).toBe(screen.getByRole("menuitem", { name: "重命名" }));
    // ↓ 到底再循环回首项
    fireEvent.keyDown(container, { key: "ArrowDown" });
    expect(document.activeElement).toBe(screen.getByRole("menuitem", { name: "删除" }));
    fireEvent.keyDown(container, { key: "ArrowDown" });
    expect(document.activeElement).toBe(screen.getByRole("menuitem", { name: "重命名" }));
    fireEvent.keyDown(container, { key: "Enter" });
    expect(onRename).toHaveBeenCalledOnce();
    expect(onDelete).not.toHaveBeenCalled();
    expect(screen.queryByRole("menuitem")).toBeNull();
  });
});

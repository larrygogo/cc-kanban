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

function renderDropdown(onChange: (v: string) => void = () => {}) {
  function Host() {
    const [v, setV] = useState("b");
    return <Dropdown value={v} options={options} onChange={(x) => { setV(x); onChange(x); }} />;
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
    // 重新打开后点外部（mousedown 落在容器之外）关闭
    fireEvent.click(btn);
    expect(screen.getByRole("option", { name: "方案 A" })).toBeTruthy();
    fireEvent.mouseDown(document.body);
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

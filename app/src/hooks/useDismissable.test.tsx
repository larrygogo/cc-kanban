import { useRef } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useDismissable } from "./useDismissable";

afterEach(cleanup);

type Options = Parameters<typeof useDismissable>[1];

function Host({ options, onClose }: { options?: Partial<Options>; onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  useDismissable(ref, { onClose, ...options });
  return (
    <div>
      <button type="button" data-testid="trigger">触发钮</button>
      <button type="button" data-testid="outside">外部</button>
      <div ref={ref} data-testid="box">
        <button type="button" data-testid="inside">内部</button>
      </div>
    </div>
  );
}

describe("useDismissable（弹层关闭语义统一实现，G-1）", () => {
  it("点容器外关闭（pointerdown 捕获段），点内部不关", () => {
    const onClose = vi.fn();
    render(<Host onClose={onClose} />);
    fireEvent.pointerDown(screen.getByTestId("inside"));
    expect(onClose).not.toHaveBeenCalled();
    fireEvent.pointerDown(screen.getByTestId("outside"));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("open=false 时不挂任何监听", () => {
    const onClose = vi.fn();
    render(<Host onClose={onClose} options={{ open: false }} />);
    fireEvent.pointerDown(screen.getByTestId("outside"));
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", cancelable: true }));
    expect(onClose).not.toHaveBeenCalled();
  });

  it("Esc 关闭并标记 preventDefault（窗口级全局动作据此让路）", () => {
    const onClose = vi.fn();
    render(<Host onClose={onClose} />);
    const e = new KeyboardEvent("keydown", { key: "Escape", cancelable: true });
    document.dispatchEvent(e);
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(e.defaultPrevented).toBe(true);
  });

  it("Esc 后焦点在容器内才归还 escFocusReturn 目标", () => {
    const onClose = vi.fn();
    render(<Host onClose={onClose} options={{ escFocusReturn: () => screen.getByTestId("trigger") }} />);
    // 焦点在容器外：不归还（鼠标开着弹层、焦点在输入框时按 Esc 不能抢焦点）
    screen.getByTestId("outside").focus();
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", cancelable: true }));
    expect(document.activeElement).not.toBe(screen.getByTestId("trigger"));
    // 焦点在容器内：归还触发钮
    screen.getByTestId("inside").focus();
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", cancelable: true }));
    expect(document.activeElement).toBe(screen.getByTestId("trigger"));
  });

  it("closeOnScroll：容器外滚动关闭，容器自身内滚不关", () => {
    const onClose = vi.fn();
    render(<Host onClose={onClose} options={{ closeOnScroll: true }} />);
    screen.getByTestId("inside").dispatchEvent(new Event("scroll"));
    expect(onClose).not.toHaveBeenCalled();
    screen.getByTestId("outside").dispatchEvent(new Event("scroll"));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("closeOnBlur / closeOnResize / closeOnContextMenu 按选项启用", () => {
    const onClose = vi.fn();
    render(<Host onClose={onClose} options={{ closeOnBlur: true, closeOnResize: true, closeOnContextMenu: true }} />);
    fireEvent(window, new Event("blur"));
    expect(onClose).toHaveBeenCalledTimes(1);
    fireEvent(window, new Event("resize"));
    expect(onClose).toHaveBeenCalledTimes(2);
    fireEvent.contextMenu(screen.getByTestId("outside"));
    expect(onClose).toHaveBeenCalledTimes(3);
    // 菜单内右键不关（落点是菜单项自己）
    fireEvent.contextMenu(screen.getByTestId("inside"));
    expect(onClose).toHaveBeenCalledTimes(3);
  });

  it("interceptOutsideClick：点外关闭时 stopPropagation，点击不穿透到 document 冒泡监听", () => {
    const onClose = vi.fn();
    const docClick = vi.fn();
    document.addEventListener("click", docClick);
    render(<Host onClose={onClose} options={{ outsideEvent: "click", interceptOutsideClick: true }} />);
    fireEvent.click(screen.getByTestId("outside"));
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(docClick).not.toHaveBeenCalled();
    document.removeEventListener("click", docClick);
  });
});

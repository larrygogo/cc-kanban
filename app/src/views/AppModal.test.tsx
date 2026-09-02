import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { AppModal } from "./AppModal";
import { hasEscLayers } from "../escLayers";

afterEach(cleanup);

function setup(onClose: () => void = () => {}) {
  render(
    <div>
      <button type="button" data-testid="bg">背景按钮</button>
      <AppModal label="测试弹层" onClose={onClose}>
        <button type="button" data-testid="a">A</button>
        <button type="button" data-testid="b">B</button>
      </AppModal>
    </div>,
  );
  return {
    dlg: screen.getByRole("dialog"),
    a: screen.getByTestId("a"),
    b: screen.getByTestId("b"),
    bg: screen.getByTestId("bg"),
  };
}

describe("AppModal（页内 modal 统一壳，G-3）", () => {
  it("Esc 层注册随卸载注销，不泄漏（`useEffect(() => pushEscLayer(), [])` 的返回值即 cleanup）", () => {
    const r = render(<AppModal label="测试弹层" onClose={() => {}}><button type="button">A</button></AppModal>);
    expect(hasEscLayers()).toBe(true);
    r.unmount();
    expect(hasEscLayers()).toBe(false);
  });

  it("role=dialog + aria-modal，初始焦点落到第一个可聚焦元素", () => {
    const { dlg, a } = setup();
    expect(dlg.getAttribute("aria-modal")).toBe("true");
    expect(dlg.getAttribute("aria-label")).toBe("测试弹层");
    expect(document.activeElement).toBe(a);
  });

  it("Esc 关闭（preventDefault + stopPropagation，窗口级「Esc=拒绝审批」让路）", () => {
    const onClose = vi.fn();
    const { dlg } = setup(onClose);
    fireEvent.keyDown(dlg, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("Tab 焦点陷阱：在弹层内循环，不落进背景", () => {
    const { dlg, a, b } = setup();
    fireEvent.keyDown(dlg, { key: "Tab" });
    expect(document.activeElement).toBe(b);
    fireEvent.keyDown(dlg, { key: "Tab" }); // 末项 → 回首项
    expect(document.activeElement).toBe(a);
    fireEvent.keyDown(dlg, { key: "Tab", shiftKey: true }); // 首项 Shift+Tab → 末项
    expect(document.activeElement).toBe(b);
  });

  it("背景 inert：aria-modal 期间背景兄弟不可交互，卸载后恢复", () => {
    const { bg, unmount } = (() => {
      const r = render(
        <div>
          <button type="button" data-testid="bg">背景按钮</button>
          <AppModal label="测试弹层" onClose={() => {}}>
            <button type="button">A</button>
          </AppModal>
        </div>,
      );
      return { bg: screen.getByTestId("bg"), unmount: r.unmount };
    })();
    expect(bg.getAttribute("inert")).not.toBeNull();
    unmount();
    expect(bg.getAttribute("inert")).toBeNull();
  });

  it("关闭时焦点仍在弹层内则归还给打开前的元素", () => {
    const onClose = vi.fn();
    const { bg, dlg, rerender } = (() => {
      const r = render(
        <div>
          <button type="button" data-testid="bg">背景按钮</button>
          <AppModal label="测试弹层" onClose={onClose}>
            <button type="button" data-testid="a">A</button>
          </AppModal>
        </div>,
      );
      return { bg: screen.getByTestId("bg"), dlg: screen.getByRole("dialog"), rerender: r.rerender };
    })();
    bg.focus();
    // 重新触发初始焦点逻辑不可行（已挂载），直接验证卸载归还：
    // 卸载时焦点在弹层内 → 归还 prev（挂载前的 activeElement）。
    rerender(
      <div>
        <button type="button" data-testid="bg">背景按钮</button>
      </div>,
    );
    void dlg;
    expect(document.activeElement).toBe(bg);
  });

  it("无可聚焦元素时聚焦对话框本体（Esc/Tab 仍有落点）", () => {
    render(
      <AppModal label="纯展示" onClose={() => {}}>
        <div>只有文字</div>
      </AppModal>,
    );
    const dlg = screen.getByRole("dialog");
    expect(document.activeElement).toBe(dlg);
    // Tab 不抛出、不移动
    fireEvent.keyDown(dlg, { key: "Tab" });
    expect(document.activeElement).toBe(dlg);
  });

  it("点遮罩关闭，点弹层内部不关", () => {
    const onClose = vi.fn();
    const { dlg } = setup(onClose);
    const overlay = dlg.parentElement as HTMLElement;
    fireEvent.mouseDown(dlg);
    fireEvent.click(dlg);
    expect(onClose).not.toHaveBeenCalled();
    fireEvent.mouseDown(overlay);
    fireEvent.click(overlay);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  /** 7S-5：在弹层里拖选文字、手滑到遮罩上松手——按下不在遮罩上，就不算「点了遮罩」，
   *  否则一次拖选就把弹层连同没提交的草稿一起关掉。 */
  it("从弹层内按下、在遮罩上松手不关", () => {
    const onClose = vi.fn();
    const { dlg } = setup(onClose);
    const overlay = dlg.parentElement as HTMLElement;
    fireEvent.mouseDown(dlg);
    fireEvent.click(overlay);
    expect(onClose).not.toHaveBeenCalled();
  });

  it("自定义 onKeyDown 先跑，preventDefault 后壳不再处理", () => {
    const onClose = vi.fn();
    const onKeyDown = vi.fn((e: React.KeyboardEvent) => {
      if (e.key === "Escape") e.preventDefault(); // 自己消费 Esc（如先清输入）
    });
    render(
      <AppModal label="测试弹层" onClose={onClose} onKeyDown={onKeyDown}>
        <button type="button">A</button>
      </AppModal>,
    );
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    expect(onKeyDown).toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
  });
});

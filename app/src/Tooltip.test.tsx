import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { TooltipLayer } from "./Tooltip";

afterEach(cleanup);

describe("TooltipLayer", () => {
  it("悬停带 data-tip 的元素，延迟后弹出提示，移出后消失", () => {
    vi.useFakeTimers();
    try {
      render(
        <>
          <button data-tip="跳转到终端">btn</button>
          <TooltipLayer />
        </>,
      );
      const btn = screen.getByText("btn");
      fireEvent.mouseOver(btn);
      // 延迟未到不弹
      act(() => void vi.advanceTimersByTime(100));
      expect(screen.queryByRole("tooltip")).toBeNull();
      // 过了 SHOW_DELAY 弹出，文案取自 data-tip
      act(() => void vi.advanceTimersByTime(300));
      expect(screen.getByRole("tooltip").textContent).toBe("跳转到终端");
      // 移出元素即消失
      fireEvent.mouseOut(btn, { relatedTarget: document.body });
      expect(screen.queryByRole("tooltip")).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it("键盘聚焦带 data-tip 的元素，延迟后弹出提示，失焦后消失", () => {
    vi.useFakeTimers();
    // 聚焦弹提示门控在键盘模态（data-im="kbd"，见 input-modality）。
    document.documentElement.setAttribute("data-im", "kbd");
    try {
      render(
        <>
          <button data-tip="跳转到终端">btn</button>
          <TooltipLayer />
        </>,
      );
      const btn = screen.getByText("btn");
      fireEvent.focusIn(btn);
      // 延迟未到不弹
      act(() => void vi.advanceTimersByTime(100));
      expect(screen.queryByRole("tooltip")).toBeNull();
      // 过了 SHOW_DELAY 弹出，与悬停同一套逻辑
      act(() => void vi.advanceTimersByTime(300));
      expect(screen.getByRole("tooltip").textContent).toBe("跳转到终端");
      // 失焦即消失
      fireEvent.focusOut(btn, { relatedTarget: document.body });
      expect(screen.queryByRole("tooltip")).toBeNull();
    } finally {
      document.documentElement.removeAttribute("data-im");
      vi.useRealTimers();
    }
  });

  it("指针模态下程序化聚焦不弹提示（菜单收起还焦点给触发钮的场景）", () => {
    // 回归：鼠标点选菜单项后 useMenuPopup/SidebarFilterMenu 把焦点还给触发钮，
    // focusin 曾走与键盘相同的路径——每次收起菜单都强制弹一次提示。
    vi.useFakeTimers();
    try {
      render(
        <>
          <button data-tip="筛选与分组">btn</button>
          <TooltipLayer />
        </>,
      );
      fireEvent.focusIn(screen.getByText("btn"));
      act(() => void vi.advanceTimersByTime(500));
      expect(screen.queryByRole("tooltip")).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it("触发钮自己开着弹层(aria-expanded)时不弹提示", () => {
    // 回归：侧栏筛选钮不是 Dropdown（无 .dd-btn.open），菜单开着时悬停按钮
    // 曾弹「筛选与分组」盖在菜单上——aria-expanded 是弹出触发器的通用语义。
    vi.useFakeTimers();
    try {
      render(
        <>
          <button data-tip="筛选与分组" aria-expanded="true">btn</button>
          <TooltipLayer />
        </>,
      );
      fireEvent.mouseOver(screen.getByText("btn"));
      act(() => void vi.advanceTimersByTime(500));
      expect(screen.queryByRole("tooltip")).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it("下拉菜单展开期间不弹提示（悬停菜单项或容器都静默）", () => {
    // 回归：菜单(.dd-menu)是 data-tip 容器的 DOM 子孙，悬停菜单项曾命中容器的提示，
    // 且提示定位在容器下缘——正糊在展开的菜单顶上（终端封面的权限下拉）。
    vi.useFakeTimers();
    try {
      render(
        <>
          <div data-tip="本次恢复使用的权限模式">
            <div className="dd">
              <button className="dd-btn open">trigger</button>
              <div className="dd-menu">
                <button className="dd-item">默认</button>
              </div>
            </div>
          </div>
          <TooltipLayer />
        </>,
      );
      // 悬停展开菜单里的选项：静默
      fireEvent.mouseOver(screen.getByText("默认"));
      act(() => void vi.advanceTimersByTime(500));
      expect(screen.queryByRole("tooltip")).toBeNull();
      // 悬停触发按钮（菜单仍开着）：同样静默——提示会盖住菜单顶部
      fireEvent.mouseOver(screen.getByText("trigger"));
      act(() => void vi.advanceTimersByTime(500));
      expect(screen.queryByRole("tooltip")).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it("无 data-tip 的元素不弹提示", () => {
    vi.useFakeTimers();
    try {
      render(
        <>
          <button>plain</button>
          <TooltipLayer />
        </>,
      );
      fireEvent.mouseOver(screen.getByText("plain"));
      act(() => void vi.advanceTimersByTime(500));
      expect(screen.queryByRole("tooltip")).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });
});

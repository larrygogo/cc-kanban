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

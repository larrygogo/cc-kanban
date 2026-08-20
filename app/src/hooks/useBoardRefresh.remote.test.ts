import { afterEach, describe, expect, it, vi } from "vitest";
import { renderHook } from "@testing-library/react";
import { useBoardRefresh, REMOTE_BOARD_POLL_MS } from "./useBoardRefresh";
import { markRemoteUi } from "../remoteMode";

afterEach(() => {
  (globalThis as Record<string, unknown>).__MEOWO_REMOTE__ = undefined;
  document.body.classList.remove("remote-ui");
  vi.useRealTimers();
});

describe("useBoardRefresh 远程轮询", () => {
  it("远程模式按周期驱动刷新(board-changed 事件收不到时的兜底)", () => {
    vi.useFakeTimers();
    markRemoteUi();
    const refresh = vi.fn();
    renderHook(() => useBoardRefresh(refresh));

    // 首拍轮询后经节流触发一次刷新。
    vi.advanceTimersByTime(REMOTE_BOARD_POLL_MS + 500);
    const afterFirst = refresh.mock.calls.length;
    expect(afterFirst).toBeGreaterThanOrEqual(1);

    vi.advanceTimersByTime(REMOTE_BOARD_POLL_MS + 500);
    expect(refresh.mock.calls.length).toBeGreaterThan(afterFirst);
  });

  it("桌面模式不装轮询(只靠 board-changed 事件)", () => {
    vi.useFakeTimers();
    const refresh = vi.fn();
    renderHook(() => useBoardRefresh(refresh));

    vi.advanceTimersByTime(REMOTE_BOARD_POLL_MS * 3);
    expect(refresh).not.toHaveBeenCalled();
  });
});

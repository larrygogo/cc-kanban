import { afterEach, describe, expect, it, vi } from "vitest";
import { renderHook } from "@testing-library/react";

// 按事件名路由：board-changed → emitBoardChanged；board-urgent → emitBoardUrgent（T-15 高优通道）。
let emitBoardChanged: (() => void) | undefined;
let emitBoardUrgent: (() => void) | undefined;
vi.mock("@tauri-apps/api/event", () => ({
  listen: (event: string, cb: () => void) => {
    if (event === "board-changed") emitBoardChanged = cb;
    if (event === "board-urgent") emitBoardUrgent = cb;
    return Promise.resolve(() => {});
  },
}));

import { useBoardRefresh, BOARD_REFRESH_THROTTLE_MS } from "./useBoardRefresh";

afterEach(() => {
  vi.useRealTimers();
});

describe("useBoardRefresh board-urgent 高优通道（T-15）", () => {
  it("节流冷却窗内的 board-urgent 绕过 400ms 节流立即刷新", () => {
    vi.useFakeTimers();
    const refresh = vi.fn();
    renderHook(() => useBoardRefresh(refresh));

    // leading：首个 board-changed 立即刷新，进入冷却窗。
    emitBoardChanged?.();
    expect(refresh).toHaveBeenCalledTimes(1);

    // 冷却窗内的 board-urgent：不等窗口末尾，立即再刷一次。
    emitBoardUrgent?.();
    expect(refresh).toHaveBeenCalledTimes(2);

    // 没有遗留定时器在窗口末尾补第三刀。
    vi.advanceTimersByTime(BOARD_REFRESH_THROTTLE_MS * 3);
    expect(refresh).toHaveBeenCalledTimes(2);
  });

  it("board-urgent 抵达时已排队的 trailing 被提前兑现", () => {
    vi.useFakeTimers();
    const refresh = vi.fn();
    renderHook(() => useBoardRefresh(refresh));

    emitBoardChanged?.(); // leading 立即刷新
    emitBoardChanged?.(); // 冷却窗内 → trailing 排队，不立即刷
    expect(refresh).toHaveBeenCalledTimes(1);

    emitBoardUrgent?.(); // 排队中的 trailing 被清掉并立即兑现
    expect(refresh).toHaveBeenCalledTimes(2);

    vi.advanceTimersByTime(BOARD_REFRESH_THROTTLE_MS * 3);
    expect(refresh).toHaveBeenCalledTimes(2);
  });

  it("普通 board-changed 的节流行为不受高优通道影响", () => {
    vi.useFakeTimers();
    const refresh = vi.fn();
    renderHook(() => useBoardRefresh(refresh));

    emitBoardChanged?.();
    emitBoardChanged?.();
    emitBoardChanged?.();
    expect(refresh).toHaveBeenCalledTimes(1); // 冷却窗内合并
    vi.advanceTimersByTime(BOARD_REFRESH_THROTTLE_MS + 10);
    expect(refresh).toHaveBeenCalledTimes(2); // trailing 补发一次即停
    vi.advanceTimersByTime(BOARD_REFRESH_THROTTLE_MS * 3);
    expect(refresh).toHaveBeenCalledTimes(2);
  });
});

import { renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";

const showMock = vi.fn(() => Promise.resolve());
const setFocusMock = vi.fn(() => Promise.resolve());

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(() => Promise.resolve()) }));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ show: showMock, setFocus: setFocusMock }),
}));

import { useShowWhenReady } from "./useShowWhenReady";

describe("useShowWhenReady", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    // 双 rAF 同步化：jsdom 的 rAF 依赖 pretendToBeVisual，自行打桩让时序可控。
    vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => {
      cb(0);
      return 0;
    });
    vi.stubGlobal("cancelAnimationFrame", () => {});
  });

  it("focus:false（贴纸主窗口）走 show_sticker 命令，不直接 show/setFocus（W-7）", () => {
    renderHook(() => useShowWhenReady({ focus: false }));
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("show_sticker");
    expect(showMock).not.toHaveBeenCalled();
    expect(setFocusMock).not.toHaveBeenCalled();
  });

  it("默认（focus:true）照常 show + setFocus，不走 show_sticker", async () => {
    renderHook(() => useShowWhenReady());
    expect(showMock).toHaveBeenCalled();
    await vi.waitFor(() => expect(setFocusMock).toHaveBeenCalled());
    expect(vi.mocked(invoke)).not.toHaveBeenCalledWith("show_sticker");
  });

  it("ready 翻 true 才显示：false 时不动", () => {
    renderHook(() => useShowWhenReady({ focus: false, ready: false }));
    expect(vi.mocked(invoke)).not.toHaveBeenCalled();
    expect(showMock).not.toHaveBeenCalled();
  });
});

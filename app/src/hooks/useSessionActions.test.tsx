import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({ setArchived: vi.fn() }));
vi.mock("../api", () => api);

import { pickNextAfterArchive, useSessionActions } from "./useSessionActions";

beforeEach(() => {
  api.setArchived.mockReset().mockResolvedValue(undefined);
});

describe("pickNextAfterArchive", () => {
  it("优先取当前条之后的第一条幸存项", () => {
    expect(pickNextAfterArchive([1, 2, 3, 4], [2], 2)).toBe(3);
  });

  it("当前条在末尾则往前找", () => {
    expect(pickNextAfterArchive([1, 2, 3], [3], 3)).toBe(2);
  });

  it("批量归档跳过批内其它成员", () => {
    expect(pickNextAfterArchive([1, 2, 3, 4], [2, 3], 2)).toBe(4);
  });

  it("当前条不在可见顺序里（镜像滞后）退到幸存首条", () => {
    expect(pickNextAfterArchive([1, 2], [9], 9)).toBe(1);
  });

  it("全部被归档返回 null（留在原地）", () => {
    expect(pickNextAfterArchive([1, 2], [1, 2], 1)).toBeNull();
    expect(pickNextAfterArchive([], [1], 1)).toBeNull();
  });
});

describe("useSessionActions", () => {
  it("归档当前会话：写后端、按可见顺序跳「下一条」、给出撤销条", async () => {
    const onNavigate = vi.fn();
    const onError = vi.fn();
    const hook = renderHook(() => useSessionActions({ onNavigate, onError }));
    await act(async () => {
      await hook.result.current.archive({ ids: [2], visibleOrder: [1, 2, 3], activeId: 2, errorMessage: "失败" });
    });
    expect(api.setArchived).toHaveBeenCalledWith(2, true);
    expect(onNavigate).toHaveBeenCalledWith(3);
    expect(onError).not.toHaveBeenCalled();
    expect(hook.result.current.archiveUndo).toEqual([2]);
  });

  it("归档别的会话不动当前对话", async () => {
    const onNavigate = vi.fn();
    const hook = renderHook(() => useSessionActions({ onNavigate, onError: vi.fn() }));
    await act(async () => {
      await hook.result.current.archive({ ids: [3], visibleOrder: [1, 2, 3], activeId: 1, errorMessage: "失败" });
    });
    expect(onNavigate).not.toHaveBeenCalled();
    expect(hook.result.current.archiveUndo).toEqual([3]);
  });

  it("失败：报错并走 onFailure 兜底，不出撤销条；errorMessage 函数拿得到原因", async () => {
    api.setArchived.mockRejectedValue(new Error("db busy"));
    const onError = vi.fn();
    const onFailure = vi.fn();
    const hook = renderHook(() => useSessionActions({ onNavigate: vi.fn(), onError }));
    await act(async () => {
      await hook.result.current.archive({
        ids: [2],
        visibleOrder: [1, 2, 3],
        activeId: 2,
        onFailure,
        errorMessage: (reason) => `失败:${(reason as Error).message}`,
      });
    });
    expect(onError).toHaveBeenCalledWith("失败:db busy");
    expect(onFailure).toHaveBeenCalled();
    expect(hook.result.current.archiveUndo).toBeNull();
  });

  it("可见顺序为空时退回 fallbackNext 现查", async () => {
    const onNavigate = vi.fn();
    const fallbackNext = vi.fn().mockResolvedValue(9);
    const hook = renderHook(() => useSessionActions({ onNavigate, onError: vi.fn() }));
    await act(async () => {
      await hook.result.current.archive({ ids: [2], visibleOrder: [], activeId: 2, fallbackNext, errorMessage: "失败" });
    });
    expect(fallbackNext).toHaveBeenCalled();
    expect(onNavigate).toHaveBeenCalledWith(9);
  });

  it("撤销：整批取消归档并 onUndone，撤销条清空", async () => {
    const onUndone = vi.fn();
    const hook = renderHook(() => useSessionActions({ onNavigate: vi.fn(), onError: vi.fn() }));
    await act(async () => {
      await hook.result.current.archive({ ids: [2], visibleOrder: [1, 2, 3], activeId: 1, errorMessage: "失败" });
    });
    expect(hook.result.current.archiveUndo).toEqual([2]);
    await act(async () => {
      hook.result.current.undoArchive({ onUndone, errorMessage: "失败" });
    });
    expect(api.setArchived).toHaveBeenCalledWith(2, false);
    expect(onUndone).toHaveBeenCalledWith([2]);
    expect(hook.result.current.archiveUndo).toBeNull();
  });
});

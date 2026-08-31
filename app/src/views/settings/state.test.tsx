// useSettingsState 的首读缓存：getSettings() 的 Promise 缓存在 loadRef 里供首帧 patch 等待。
// 首读失败时被拒的 Promise 若留在缓存里，之后第一次 patch 的 await reload() 必打到这个已拒
// 缓存——patch 丢失，还把真正的保存错误盖成误导性的首读错误。失败必须清缓存、下次重拉。
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { act, cleanup, renderHook } from "@testing-library/react";

const api = vi.hoisted(() => ({
  getSettings: vi.fn(),
  setSettings: vi.fn(),
}));
vi.mock("../../api", () => api);

import { useSettingsState, SETTINGS_DEFAULTS } from "./state";

beforeEach(() => {
  Object.values(api).forEach((m) => m.mockReset());
  api.setSettings.mockResolvedValue(undefined);
});
afterEach(() => cleanup());

describe("useSettingsState", () => {
  it("首读失败后缓存被清空：随后的 patch 重新拉取并正常保存，不报误导性错误", async () => {
    api.getSettings
      .mockRejectedValueOnce(new Error("ipc down"))
      .mockResolvedValue(SETTINGS_DEFAULTS);

    const hook = renderHook(() => useSettingsState());
    await act(async () => {}); // 挂载时的 reload 落定（失败；修复后缓存已清）
    expect(api.getSettings).toHaveBeenCalledTimes(1);

    let result: string | null | undefined;
    await act(async () => {
      result = await hook.result.current[1]({ theme: "light" });
    });

    // patch 不丢：重新拉取成功 → 基于真实设置合并 → 落盘成功返回 null。
    expect(result).toBeNull();
    expect(api.getSettings).toHaveBeenCalledTimes(2); // 重新拉取，而非复用已拒缓存
    expect(api.setSettings).toHaveBeenCalledWith({ ...SETTINGS_DEFAULTS, theme: "light" });
  });

  it("首读成功后 patch 基于缓存合并，不重复拉取", async () => {
    api.getSettings.mockResolvedValue(SETTINGS_DEFAULTS);

    const hook = renderHook(() => useSettingsState());
    await act(async () => {});

    let result: string | null | undefined;
    await act(async () => {
      result = await hook.result.current[1]({ opacity: 80 });
    });

    expect(result).toBeNull();
    expect(api.getSettings).toHaveBeenCalledTimes(1); // 成功缓存仍然复用
    expect(api.setSettings).toHaveBeenCalledWith({ ...SETTINGS_DEFAULTS, opacity: 80 });
  });

  // 写-写竞态回归（P1-3）：S-2 后各分区常驻挂载、各持一份实例；快照与保存队列必须是
  // 模块级共享，否则 A 分区改完广播未到时，B 分区基于过旧快照整对象写回会把 A 打回。
  it("多实例共享快照与串行队列：B 紧接 A 的在途写入 patch，不打回 A 的改动", async () => {
    api.getSettings.mockResolvedValue(SETTINGS_DEFAULTS);
    // 第一次 setSettings 悬置：A 的写在途时 B 发起 patch，B 必须排在它之后。
    let resolveFirst: (() => void) | undefined;
    api.setSettings
      .mockImplementationOnce(() => new Promise<void>((r) => { resolveFirst = r; }))
      .mockResolvedValue(undefined);

    const a = renderHook(() => useSettingsState());
    const b = renderHook(() => useSettingsState());
    await act(async () => {});

    let resultA: string | null | undefined;
    let resultB: string | null | undefined;
    await act(async () => {
      const pa = a.result.current[1]({ theme: "light" });
      const pb = b.result.current[1]({ opacity: 80 });
      // A 的任务经若干微任务才推进到悬置的 setSettings：等它就绪再放行，
      // 否则 resolveFirst 还是 undefined、第一次写永远不落定。
      for (let i = 0; i < 20 && !resolveFirst; i += 1) await Promise.resolve();
      resolveFirst?.();
      [resultA, resultB] = await Promise.all([pa, pb]);
    });

    expect(resultA).toBeNull();
    expect(resultB).toBeNull();
    expect(api.setSettings).toHaveBeenCalledTimes(2);
    // 第二次整对象写回必须带上 A 的 theme，不能用过旧快照把它静默打回。
    expect(api.setSettings).toHaveBeenNthCalledWith(1, { ...SETTINGS_DEFAULTS, theme: "light" });
    expect(api.setSettings).toHaveBeenNthCalledWith(2, { ...SETTINGS_DEFAULTS, theme: "light", opacity: 80 });
  });

  // P1-2：patch 失败文案在产出端统一过 formatBackendError——后端 sentinel 是中文，
  // 各分区/调用方拿到的都应是映射后的当前语言文案（默认 zh 上下文）。
  it("patch 失败返回经 formatBackendError 映射的错误文案", async () => {
    api.getSettings.mockResolvedValue(SETTINGS_DEFAULTS);
    api.setSettings.mockRejectedValue(new Error("已选「自定义代理」，但代理地址为空"));

    const hook = renderHook(() => useSettingsState());
    await act(async () => {});

    let result: string | null | undefined;
    await act(async () => {
      result = await hook.result.current[1]({ theme: "light" });
    });

    expect(result).toBe("已选自定义代理，请填写代理地址");
    expect(hook.result.current[2]).toBe("已选自定义代理，请填写代理地址");
  });
});

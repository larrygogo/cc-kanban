import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { render, screen, cleanup, fireEvent } from "@testing-library/react";

// W-12：macOS 面板形态下 pin 的语义是「保持打开（失焦不自动收起）」。hostOs 是
// platform 模块的私有态（测试环境恒 null → 非面板），这里按面板平台整体 mock。
vi.mock("../platform", () => ({
  detectHostOs: () => Promise.resolve(),
  setHostOsUnknown: () => {},
  isMac: () => true,
  isWindows: () => false,
  isMacPanel: () => true,
}));

const invokeMock = vi.hoisted(() =>
  vi.fn((cmd: string, _args?: unknown) => {
    if (cmd === "get_settings") {
      return Promise.resolve({
        notifications_enabled: true,
        theme: "dark",
        opacity: 94,
        ui_scale: 100,
        resume_terminal: "terminal",
        language: "auto",
        terminal_open_mode: "card",
        card_menu_mode: "button",
        preview_enabled: true,
        sticker_style: "flat",
        sticker_color: "neutral",
        sticker_quota_providers: ["claude"],
        default_agent: "claude",
      });
    }
    if (cmd === "list_agents") return Promise.resolve([]);
    return Promise.resolve();
  })
);
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: unknown) => invokeMock(cmd, args),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: () => Promise.resolve(() => {}),
}));

import { Sticker } from "./Sticker";
import { zh } from "../i18n/zh";

// 与 Sticker.test.tsx 同款的 jsdom 视口桩：虚拟列表量到非零尺寸才会挂载内容。
const defaultRect: DOMRect = {
  top: 0, left: 0, bottom: 0, right: 0, width: 0, height: 0, x: 0, y: 0,
  toJSON: () => ({ top: 0, left: 0, bottom: 0, right: 0, width: 0, height: 0 }),
};
vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement): DOMRect {
  if (this.classList.contains("stk-scroll")) {
    return { ...defaultRect, bottom: 600, right: 400, width: 400, height: 600 } as DOMRect;
  }
  return defaultRect;
});
class MockResizeObserver {
  constructor(private cb: ResizeObserverCallback) {}
  observe(target: Element) {
    const rect = target.classList.contains("stk-scroll")
      ? { ...defaultRect, bottom: 600, right: 400, width: 400, height: 600 }
      : defaultRect;
    this.cb([{ target, contentRect: rect } as unknown as ResizeObserverEntry], this as unknown as ResizeObserver);
  }
  unobserve() {}
  disconnect() {}
}
vi.stubGlobal("ResizeObserver", MockResizeObserver);

afterEach(() => {
  cleanup();
  localStorage.clear();
});
beforeEach(() => invokeMock.mockClear());

describe("macOS 面板的 pin（W-12：保持打开）", () => {
  it("面板模式渲染 pin 按钮，语义为「保持打开」并把偏好推给后端 resign 监听器", async () => {
    render(<Sticker filter="all" data={[]} />);
    // 初始未 pin：按钮文案是「保持打开」，且挂载即把 false 推给后端（面板恢复失焦自隐的默认）。
    const btn = await screen.findByLabelText(zh.sticker.keepOpenOff);
    expect(invokeMock).toHaveBeenCalledWith("set_panel_keep_open", { open: false });

    fireEvent.click(btn);
    await screen.findByLabelText(zh.sticker.keepOpenOn);
    expect(invokeMock).toHaveBeenCalledWith("set_panel_keep_open", { open: true });
    expect(localStorage.getItem("meowo-pinned")).toBe("1");
  });

  it("面板模式挂载时按 PIN_KEY 恢复「保持打开」", async () => {
    localStorage.setItem("meowo-pinned", "1");
    render(<Sticker filter="all" data={[]} />);
    await screen.findByLabelText(zh.sticker.keepOpenOn);
    expect(invokeMock).toHaveBeenCalledWith("set_panel_keep_open", { open: true });
  });
});

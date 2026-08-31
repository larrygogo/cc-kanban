import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { STICKER_COLORS, STICKER_COLOR_KEYS, bootAppearance, stickerBgRgb } from "./appearance";

describe("stickerBgRgb", () => {
  it("已知预设按主题取深/浅一套底色 RGB", () => {
    expect(stickerBgRgb("slate", "dark")).toBe(STICKER_COLORS.slate.dark);
    expect(stickerBgRgb("slate", "light")).toBe(STICKER_COLORS.slate.light);
    expect(stickerBgRgb("amber", "dark")).toBe(STICKER_COLORS.amber.dark);
  });

  it("未知 key 回退默认预设（无色）", () => {
    expect(stickerBgRgb("does-not-exist", "dark")).toBe(STICKER_COLORS.neutral.dark);
    expect(stickerBgRgb("", "light")).toBe(STICKER_COLORS.neutral.light);
  });

  it("经典预设的深色底与 styles.css 初值一致（升级零变化）", () => {
    expect(stickerBgRgb("classic", "dark")).toBe("38, 38, 36");
    expect(stickerBgRgb("classic", "light")).toBe("250, 249, 245");
  });

  it("每个预设都含 swatch / dark / light 三个非空字段", () => {
    for (const k of STICKER_COLOR_KEYS) {
      const p = STICKER_COLORS[k];
      expect(p.swatch).toMatch(/^#[0-9a-fA-F]{6}$/);
      expect(p.dark).toMatch(/^\d+, \d+, \d+$/);
      expect(p.light).toMatch(/^\d+, \d+, \d+$/);
    }
  });
});

// G-11 后续：--cc-ui 下发面从贴纸窗扩到对话窗（main.tsx 对 main/chat 传 scale:true）。
// 这里钉住「scale 门控 + settings-changed 热生效」这条通路，防回退成只读缓存不订阅。
const listenHandlers: Array<(e: { payload: unknown }) => void> = [];
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (_name: string, cb: (e: { payload: unknown }) => void) => {
    listenHandlers.push(cb);
    return () => {};
  }),
}));
vi.mock("./api", () => ({
  getSettings: async () => ({}),
}));

describe("bootAppearance 的界面缩放（--cc-ui）下发", () => {
  beforeEach(() => {
    localStorage.clear();
    listenHandlers.length = 0;
    document.documentElement.removeAttribute("style");
    // jsdom 没有 matchMedia；深色（matches:false）即可，主题不是这里的观察点。
    window.matchMedia = vi.fn().mockImplementation(() => ({
      matches: false,
      addEventListener: () => {},
    }));
  });
  afterEach(() => {
    document.documentElement.removeAttribute("style");
    localStorage.clear();
  });

  it("scale:true（贴纸/对话窗）按缓存值下发 --cc-ui", () => {
    localStorage.setItem("meowo-appearance", JSON.stringify({ ui_scale: 112 }));
    bootAppearance({ scale: true });
    expect(document.documentElement.style.getPropertyValue("--cc-ui")).toBe("1.12");
  });

  it("scale:false（设置等独立窗）不下发，--cc-ui 保持 CSS 默认 1", () => {
    localStorage.setItem("meowo-appearance", JSON.stringify({ ui_scale: 112 }));
    bootAppearance({ scale: false });
    expect(document.documentElement.style.getPropertyValue("--cc-ui")).toBe("");
  });

  it("settings-changed 广播到达后 --cc-ui 热更新（与贴纸窗同一通道）", () => {
    bootAppearance({ scale: true });
    expect(listenHandlers).toHaveLength(1);
    listenHandlers[0]({ payload: { ui_scale: 90 } });
    expect(document.documentElement.style.getPropertyValue("--cc-ui")).toBe("0.9");
    // 越界值按 settings.rs 的 50–200 钳位（手改 settings.json 的兜底）。
    listenHandlers[0]({ payload: { ui_scale: 500 } });
    expect(document.documentElement.style.getPropertyValue("--cc-ui")).toBe("2");
  });
});

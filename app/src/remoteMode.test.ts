import { afterEach, describe, expect, it } from "vitest";
import { markRemoteUi, remoteUi } from "./remoteMode";

afterEach(() => {
  // 门控标志挂在 globalThis 上,测试间必须复位,否则一处 markRemoteUi 会污染后续用例。
  (globalThis as Record<string, unknown>).__MEOWO_REMOTE__ = undefined;
  document.body.classList.remove("remote-ui");
});

describe("remoteMode", () => {
  it("默认(桌面)恒为 false", () => {
    expect(remoteUi()).toBe(false);
  });

  it("markRemoteUi 后置真并挂 body.remote-ui 作用域类", () => {
    markRemoteUi();
    expect(remoteUi()).toBe(true);
    expect(document.body.classList.contains("remote-ui")).toBe(true);
  });
});

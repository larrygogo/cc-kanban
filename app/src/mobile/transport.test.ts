import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { convertFileSrc } from "@tauri-apps/api/core";
import { clearMocks } from "@tauri-apps/api/mocks";
import {
  installRemoteTransport,
  getToken,
  setToken,
  clearToken,
  onAuthLost,
  NEW_SESSION_EVENT,
} from "./transport";

// fetch 桩:记录每次调用,按测试预置的响应回。
function mockFetch(response: { status: number; body: string }) {
  // 标注入参类型:否则 vi.fn 推断零参,mock.calls[0] 成空元组,取 [url, init] 时类型不匹配。
  const fn = vi.fn(async (_url: string, _init?: RequestInit) => ({
    ok: response.status >= 200 && response.status < 300,
    status: response.status,
    text: async () => response.body,
  }));
  vi.stubGlobal("fetch", fn);
  return fn;
}

beforeEach(() => {
  clearToken();
  installRemoteTransport();
});

afterEach(() => {
  clearMocks();
  clearToken();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("remote transport", () => {
  it("invoke 转 POST /rpc/<cmd>,带 X-Meowo-Token 与原样 camelCase 参数", async () => {
    setToken("tok-123");
    const fetchFn = mockFetch({ status: 200, body: JSON.stringify({ items: [] }) });

    const result = await invoke("get_live_sessions_page", { pageSize: 20, cursor: null });

    expect(result).toEqual({ items: [] });
    expect(fetchFn).toHaveBeenCalledTimes(1);
    const [url, init] = fetchFn.mock.calls[0] as [string, RequestInit];
    expect(url).toBe("/rpc/get_live_sessions_page");
    expect(init.method).toBe("POST");
    expect((init.headers as Record<string, string>)["X-Meowo-Token"]).toBe("tok-123");
    // 参数名逐字透传(不改大小写):后端 deny_unknown_fields + camelCase 契约靠这个成立。
    expect(JSON.parse(init.body as string)).toEqual({ pageSize: 20, cursor: null });
  });

  it("非 200 抛出剥引号后的原始错误串(对齐 invoke reject)", async () => {
    mockFetch({ status: 400, body: JSON.stringify("expected sessionId") });
    await expect(invoke("write_managed_terminal", {})).rejects.toThrow("expected sessionId");
  });

  it("401 清令牌并广播 auth-lost", async () => {
    setToken("stale");
    mockFetch({ status: 401, body: "" });
    const lost = vi.fn();
    const off = onAuthLost(lost);

    await expect(invoke("pending_interaction", { sessionId: 1 })).rejects.toThrow();
    expect(getToken()).toBeNull();
    expect(lost).toHaveBeenCalledTimes(1);
    off();
  });

  it("open_new_session_window 不落网,转页内导航事件", async () => {
    const fetchFn = mockFetch({ status: 200, body: "" });
    const onNew = vi.fn();
    window.addEventListener(NEW_SESSION_EVENT, onNew);

    await invoke("open_new_session_window", { cwd: "C:/proj", provider: "claude" });

    expect(onNew).toHaveBeenCalledTimes(1);
    expect(fetchFn).not.toHaveBeenCalled();
    window.removeEventListener(NEW_SESSION_EVENT, onNew);
  });

  it("plugin:event|listen 给假订阅 id,其余 plugin:* no-op,均不落网", async () => {
    const fetchFn = mockFetch({ status: 200, body: "" });
    const id = await invoke("plugin:event|listen", { event: "board-changed" });
    expect(typeof id).toBe("number");
    await invoke("plugin:event|unlisten", { eventId: id });
    await invoke("plugin:window|maximize", {});
    expect(fetchFn).not.toHaveBeenCalled();
  });

  it("open_settings / open_path_with 静默 no-op,不落网", async () => {
    const fetchFn = mockFetch({ status: 200, body: "" });
    await invoke("open_settings", {});
    await invoke("open_path_with", { cwd: "C:/x", rel: "a", opener: "code" });
    expect(fetchFn).not.toHaveBeenCalled();
  });

  it("convertFileSrc 指向 /file 端点并带 token", () => {
    setToken("tok-xyz");
    const src = convertFileSrc("C:/img/a.png");
    expect(src).toContain("/file?path=");
    expect(src).toContain(encodeURIComponent("C:/img/a.png"));
    expect(src).toContain("token=tok-xyz");
  });
});

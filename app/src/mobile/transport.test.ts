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
  primeFileToken,
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

describe("remote transport 的 open_link scheme 白名单", () => {
  // 后端 open_link 只放行 http/https(理由:任由 transcript 内容触发本地程序是注入通道)。
  // 远程桥把这条命令在前端就地兑现,那道守卫在手机路径上等于不存在——模型输出里的链接
  // 就能把自定义 scheme 直接交给手机 OS 的对应 app。
  const armWindowOpen = () => {
    const open = vi.fn();
    vi.stubGlobal("open", open);
    return open;
  };

  it("http/https 照常打开", async () => {
    const open = armWindowOpen();
    await invoke("open_link", { url: "https://example.com/docs" });
    expect(open).toHaveBeenCalledWith("https://example.com/docs", "_blank", "noopener");
  });

  it("非 http/https 一律拒绝,且不静默吞掉", async () => {
    const open = armWindowOpen();
    for (const url of [
      "javascript:alert(1)",
      "file:///C:/Windows/System32/calc.exe",
      "intent://scan/#Intent;scheme=zxing;end",
      "ms-settings:windowsupdate",
    ]) {
      await expect(invoke("open_link", { url })).rejects.toThrow("只支持 http/https 链接");
    }
    // open_url(应用自己的链接)同一条通道,同样门控。
    await expect(invoke("open_url", { url: "file:///etc/passwd" })).rejects.toThrow();
    expect(open).not.toHaveBeenCalled();
  });

  it("解析不出的链接报「无效链接」而不是放行", async () => {
    const open = armWindowOpen();
    await expect(invoke("open_link", { url: "ht!tp://%%%" })).rejects.toThrow();
    expect(open).not.toHaveBeenCalled();
  });
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

  it("空 token 的 401 不广播——那是「还没配对」,不是「令牌失效」", async () => {
    // 首帧竞态的钉子:bootAppearance 在 TokenGate 存 token 前就发 get_settings,
    // 若这发 401 触发清态,会把随后扫码存好的合法 token 一并清掉。
    // 先 setToken 复位失效闩再清掉:上一条用例广播过一次,闩不复位的话本用例
    // 的 not.toHaveBeenCalled 是空跑(把守卫整条删掉照样绿——自审 H2)。
    setToken("reset-latch");
    clearToken();
    mockFetch({ status: 401, body: "" });
    const lost = vi.fn();
    const off = onAuthLost(lost);

    await expect(invoke("get_settings", {})).rejects.toThrow();
    expect(lost).not.toHaveBeenCalled();
    // 请求在途中用户完成配对:后到的 401 同样不许动新 token。
    setToken("fresh");
    expect(getToken()).toBe("fresh");
    off();
  });

  it("失效后续发 401 不重复广播;重新配对(setToken)后再失效仍能广播", async () => {
    setToken("stale");
    mockFetch({ status: 401, body: "" });
    const lost = vi.fn();
    const off = onAuthLost(lost);

    // 四五条并发轮询同时 401 的缩影:首发广播并清 token,后续 401 不再触发
    // (反复清态会让闸门反复重挂)。
    await expect(invoke("get_settings", {})).rejects.toThrow();
    await expect(invoke("get_settings", {})).rejects.toThrow();
    await expect(invoke("get_settings", {})).rejects.toThrow();
    expect(lost).toHaveBeenCalledTimes(1);

    // 重新配对复位失效闩:换发的 token 再失效,仍要能把用户带回闸门。
    setToken("renewed");
    await expect(invoke("get_settings", {})).rejects.toThrow();
    expect(lost).toHaveBeenCalledTimes(2);
    off();
  });

  it("装桥即收取 hash 里的 token 并清 hash", () => {
    window.location.hash = "#token=tok-from-qr";
    installRemoteTransport();
    expect(getToken()).toBe("tok-from-qr");
    expect(window.location.hash).toBe("");
  });

  it("200 + 非 JSON(网关劫持页)按错误抛,不给调用方一坨字符串", async () => {
    setToken("tok");
    mockFetch({ status: 200, body: "<html>portal</html>" });
    await expect(invoke("get_settings", {})).rejects.toThrow(/非 JSON/);
  });

  it("非 2xx 的非 JSON 响应收敛为状态码,不把整页 HTML 抛上 UI", async () => {
    setToken("tok");
    mockFetch({ status: 500, body: "<html><body>Internal Error</body></html>" });
    await expect(invoke("get_settings", {})).rejects.toThrow("远程请求失败(500)");
  });

  it("数组/非对象参数在前端就拒,不发 400 去后端", async () => {
    setToken("tok");
    const fetchFn = mockFetch({ status: 200, body: "null" });
    await expect(invoke("get_chat_history", [1] as unknown as Record<string, unknown>)).rejects.toThrow(
      "只接受对象参数",
    );
    expect(fetchFn).not.toHaveBeenCalled();
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

  it("领到 /file 降级凭据后,图片 URL 不再携带主 token", async () => {
    setToken("main-tok");
    const fetchFn = mockFetch({ status: 200, body: JSON.stringify("file-tok") });
    primeFileToken();
    await vi.waitFor(() => expect(fetchFn).toHaveBeenCalledTimes(1));
    // rpc 的 then 链要跨多个微任务,等到 URL 真切换为止。
    await vi.waitFor(() => expect(convertFileSrc("C:/img/a.png")).toContain("token=file-tok"));
    expect(convertFileSrc("C:/img/a.png")).not.toContain("main-tok");
    // 主 token 换代(重配对)即弃旧降级凭据,回退主 token 直至重领。
    setToken("next-main");
    expect(convertFileSrc("C:/img/a.png")).toContain("token=next-main");
  });
});

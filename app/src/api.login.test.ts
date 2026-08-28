import { describe, it, expect, vi, beforeEach } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invokeMock(...a) }));

import { getRelaySecrets, loginAgent, logoutAgent, installAgent, cancelInstall, isLoggedIn, listRelayModels, type ProviderAccountPayload } from "./api";

beforeEach(() => invokeMock.mockReset());

describe("login api", () => {
  // 命令名拼错时组件测试仍会通过（它们 mock 掉了整个 api 模块），只有这里能拦住。
  it("loginAgent 调用 login_agent 并透传 provider/terminal", () => {
    invokeMock.mockResolvedValue(undefined);
    loginAgent("claude", "wt", undefined, "op-1");
    expect(invokeMock).toHaveBeenCalledWith("login_agent", {
      provider: "claude",
      terminal: "wt",
      profile: null, // 省略 = 当前活跃账号
      useActive: true,
      operationId: "op-1",
    });
  });

  /**
   * 多账号：登录**必须**能指定登进哪个账号——凭据会写进那个账号自己的目录。
   * 漏传的话，新账号的登录就把默认账号的凭据覆盖了：用户以为加了个账号，其实是把原来那个换掉了。
   */
  it("loginAgent 透传 profile（决定凭据写进哪个账号的目录）", () => {
    invokeMock.mockResolvedValue(undefined);
    loginAgent("claude", "wt", "work", "op-2");
    expect(invokeMock).toHaveBeenCalledWith("login_agent", {
      provider: "claude",
      terminal: "wt",
      profile: "work",
      useActive: false,
      operationId: "op-2",
    });
  });

  it("loginAgent 省略 terminal 时传 undefined（后端回退设置里的默认终端）", () => {
    invokeMock.mockResolvedValue(undefined);
    loginAgent("kimi", undefined, undefined, "op-3");
    expect(invokeMock).toHaveBeenCalledWith("login_agent", {
      provider: "kimi",
      terminal: undefined,
      profile: null,
      useActive: true,
      operationId: "op-3",
    });
  });

  it("loginAgent 显式 null 表示默认账号，不跟随当前活跃账号", () => {
    invokeMock.mockResolvedValue(undefined);
    loginAgent("claude", undefined, null, "op-4");
    expect(invokeMock).toHaveBeenCalledWith("login_agent", {
      provider: "claude",
      terminal: undefined,
      profile: null,
      useActive: false,
      operationId: "op-4",
    });
  });

  it("installAgent 调用 install_agent", () => {
    invokeMock.mockResolvedValue(undefined);
    installAgent("codex");
    expect(invokeMock).toHaveBeenCalledWith("install_agent", { provider: "codex" });
  });

  it("cancelInstall 调用 cancel_install（S-8 取消安装）", () => {
    invokeMock.mockResolvedValue(undefined);
    cancelInstall("codex");
    expect(invokeMock).toHaveBeenCalledWith("cancel_install", { provider: "codex" });
  });

  it("logoutAgent 调用 logout_agent（省略 profile = 当前活跃账号）", () => {
    invokeMock.mockResolvedValue(undefined);
    logoutAgent("codex");
    expect(invokeMock).toHaveBeenCalledWith("logout_agent", {
      provider: "codex",
      profile: null,
    });
  });

  /**
   * 多账号：登出**必须**能指定登出哪个账号。
   *
   * 后端曾写死默认账号，且跑 `claude auth logout` 时不注入 `CLAUDE_CONFIG_DIR`——于是切到别的账号
   * 后点退出登录，被清掉的是**默认账号**的凭据，而你想登出的那个原封不动。删凭据不可逆。
   */
  it("logoutAgent 透传 profile（决定清哪个账号的凭据）", () => {
    invokeMock.mockResolvedValue(undefined);
    logoutAgent("claude", "work");
    expect(invokeMock).toHaveBeenCalledWith("logout_agent", {
      provider: "claude",
      profile: "work",
    });
  });
});

describe("relay api", () => {
  it("getRelaySecrets 读取本机保存的密钥", () => {
    invokeMock.mockResolvedValue({ codex: "sk-local" });
    getRelaySecrets();
    expect(invokeMock).toHaveBeenCalledWith("get_relay_secrets");
  });

  it("listRelayModels 只传配置元数据，密钥由后端读取", () => {
    invokeMock.mockResolvedValue(["relay-model"]);
    listRelayModels("claude", "https://relay.example/v1", "", "api_key");
    expect(invokeMock).toHaveBeenCalledWith("list_relay_models", {
      agent: "claude",
      baseUrl: "https://relay.example/v1",
      protocol: "",
      auth: "api_key",
    });
  });
});

describe("isLoggedIn", () => {
  const payload = (account: ProviderAccountPayload["account"]): ProviderAccountPayload => ({
    provider: "claude",
    account,
    usage: null,
    usage_supported: true,
  });

  it("能解析出账号即已登录（三家判据各异，已在后端 account() 内收敛）", () => {
    expect(isLoggedIn(payload({ email: "a@b.c" } as ProviderAccountPayload["account"]))).toBe(true);
    // codex 的 API Key 登录没有 email，只有 login_label——同样算已登录。
    expect(isLoggedIn(payload({ login_label: "API Key" } as ProviderAccountPayload["account"]))).toBe(true);
  });

  it("account 为 null 或 payload 缺失 → 未登录", () => {
    expect(isLoggedIn(payload(null))).toBe(false);
    expect(isLoggedIn(undefined)).toBe(false);
  });
});

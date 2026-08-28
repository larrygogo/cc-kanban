// 设置页「网络」分区：代理的**优先级**与**覆盖面如实**。
//
// 两件事值得钉死：
//
//   1. **模型的设置压过默认代理。** 「默认直连 + 只给 Claude 配代理」是最常见的配法（国内模型直连、
//      境外模型走代理），未单独设置的模型才回落到默认。
//   2. **覆盖面必须如实标注。** 只有 claude 能把代理写进自己的配置文件（谁启动都生效）；codex / kimi
//      的配置文件无处设代理，只认进程环境变量，而进程环境变量只能注入给 Meowo **自己拉起**的会话——
//      你在别处开的终端我们够不着。UI 说成「全部会话」就是骗人：用户会对着自己终端里连不上的 codex
//      毫无线索地瞎试。
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup, waitFor, act } from "@testing-library/react";

// vi.mock 会被提升到文件顶部，工厂里引用的外部变量必须走 vi.hoisted（与 About.account.test.tsx 同坑）。
const api = vi.hoisted(() => ({
  getSettings: vi.fn(),
  setSettings: vi.fn(),
  listAgents: vi.fn(),
  getEffectiveProxy: vi.fn(),
  // 网络分区顶部现内嵌 RemoteAccessCard,它挂载即拉 remoteAccessInfo——桩掉,别让真 invoke 打进来。
  remoteAccessInfo: vi.fn(),
}));
vi.mock("../../api", async (o) => ({ ...(await o<typeof import("../../api")>()), ...api }));
// 收集 install-done 监听者，测试里手动广播（模拟后端 emit）。
const ev = vi.hoisted(() => ({ doneCbs: [] as Array<(e: unknown) => void> }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: (event: string, cb: (e: unknown) => void) => {
    if (event === "install-done") ev.doneCbs.push(cb);
    return Promise.resolve(() => {});
  },
}));
const fireInstallDone = (provider: string, ok: boolean) =>
  act(() => ev.doneCbs.forEach((cb) => cb({ payload: { provider, ok, code: ok ? 0 : 1, logPath: null } })));

import { NetworkSection } from "./NetworkSection";
import { SETTINGS_DEFAULTS } from "./state";
import { descriptors } from "../../test/agents";
import { zh } from "../../i18n/zh";
import type { ProxySettings } from "../../api";

const DIRECT: ProxySettings = { mode: "off", url: "", per_agent: {} };

/// 挂载网络分区。`effective` 是后端解析出的生效代理（键 "" = 全局规则）。
const mount = async (
  proxy: ProxySettings,
  effective: Record<string, string | null> = {},
) => {
  api.getSettings.mockResolvedValue({ ...SETTINGS_DEFAULTS, proxy });
  api.getEffectiveProxy.mockImplementation((agent?: string) => Promise.resolve(effective[agent ?? ""] ?? null));
  render(<NetworkSection />);
  await waitFor(() => expect(api.getEffectiveProxy).toHaveBeenCalled());
};

beforeEach(() => {
  Object.values(api).forEach((m) => m.mockReset());
  api.setSettings.mockResolvedValue(undefined);
  api.listAgents.mockResolvedValue(descriptors(["claude", "codex", "kimi"]));
  api.remoteAccessInfo.mockResolvedValue({ enabled: false, port: 18620, token: "", ips: [], lastError: null });
});
afterEach(() => cleanup());

describe("代理优先级", () => {
  it("网络页不再承载模型接入方式", async () => {
    await mount(DIRECT);
    expect(screen.queryByText(zh.relay.accessMode)).toBeNull();
  });

  it("代理地址和认证信息按原值显示", async () => {
    await mount(
      { mode: "system", url: "", per_agent: {} },
      { "": "http://user:secret@proxy.example:7890" },
    );
    expect(await screen.findByText(zh.proxy.systemHint("http://user:secret@proxy.example:7890"))).toBeTruthy();
  });
  /// 默认直连不该妨碍单独给某个模型配代理——这是最常见的配法。
  it("模型的代理压过默认代理，未单独设置的才回落到默认", async () => {
    await mount(
      { mode: "custom", url: "http://g:1", per_agent: { kimi: { mode: "off", url: "" } } },
      { "": "http://g:1", claude: "http://g:1", codex: "http://g:1", kimi: null },
    );

    // 只有已装的（claude/codex/kimi）显示代理行。kimi 单独设了直连 → 生效是直连；
    // claude/codex 跟随默认 → 生效是默认那个代理。
    await waitFor(() => {
      expect(screen.getAllByText(zh.proxy.effective("http://g:1"))).toHaveLength(2);
      expect(screen.getByText(zh.proxy.effectiveDirect)).toBeTruthy();
    });
  });

  /// 未安装的 agent 不给代理行——还没有可运行的 agent，代理配了也无处生效。
  it("未安装的 agent 不显示代理行", async () => {
    // gemini 支持代理但未装；claude 已装。
    api.listAgents.mockResolvedValue(descriptors(["claude"]));
    await mount(DIRECT);
    expect(await screen.findByText("Claude Code")).toBeTruthy();
    expect(screen.queryByText("Gemini CLI")).toBeNull();
    expect(screen.queryByText("OpenCode")).toBeNull();
  });

  /// 装完一个 agent，它的代理行必须**立刻**出现——这一分区只在挂载时取过一次名单，
  /// 不订阅 install-done 的话，用户装完还得关掉设置页重开才看得到。
  it("装完 agent 后代理行立刻出现，不必重开设置页", async () => {
    api.listAgents
      .mockResolvedValueOnce(descriptors(["claude"]))         // 初次：gemini 未装
      .mockResolvedValue(descriptors(["claude", "gemini"]));  // 装完重查：gemini 已装
    await mount(DIRECT);
    expect(await screen.findByText("Claude Code")).toBeTruthy();
    expect(screen.queryByText("Gemini CLI")).toBeNull();

    fireInstallDone("gemini", true);
    expect(await screen.findByText("Gemini CLI")).toBeTruthy();
  });

  it("选「自定义」但地址还空着时不落盘——后端会拒空地址", async () => {
    await mount(DIRECT);

    fireEvent.click(screen.getByRole("radio", { name: zh.proxy.custom }));
    expect(screen.getByPlaceholderText(zh.proxy.urlPlaceholder)).toHaveProperty("type", "text");
    expect(api.setSettings).not.toHaveBeenCalled();

    // 填完失焦才提交。
    fireEvent.change(screen.getByPlaceholderText(zh.proxy.urlPlaceholder), {
      target: { value: "http://127.0.0.1:7890" },
    });
    fireEvent.blur(screen.getByPlaceholderText(zh.proxy.urlPlaceholder));
    await waitFor(() =>
      expect(api.setSettings).toHaveBeenCalledWith(
        expect.objectContaining({
          proxy: expect.objectContaining({ mode: "custom", url: "http://127.0.0.1:7890" }),
        }),
      ),
    );
  });
});

describe("覆盖面如实标注", () => {
  /// claude 的代理写进它自己的 settings.json → 你在任何终端敲 claude 都走代理。
  /// codex / kimi 的配置文件无处设代理，只认进程环境变量 → 只覆盖 Meowo 自己拉起的会话。
  /// 把后者说成「全部会话」，用户就会对着自己终端里连不上的 codex 抓瞎。
  it("claude 标「全部会话」，codex / kimi 标「仅从 Meowo 打开的」", async () => {
    await mount(
      { mode: "custom", url: "http://p:1", per_agent: {} },
      { "": "http://p:1", claude: "http://p:1", codex: "http://p:1", kimi: "http://p:1" },
    );

    expect(screen.getByText(zh.proxy.coverageFull)).toBeTruthy();
    expect(screen.getAllByText(zh.proxy.coveragePartial).length).toBe(2); // codex + kimi
    // 覆盖面的「为什么」解释（.proxy-why）：claude 写进配置文件，codex/kimi 只注进程环境变量。
    expect(screen.getByText(zh.proxy.coverageFullWhy)).toBeTruthy();
    expect(screen.getAllByText(zh.proxy.coveragePartialWhy).length).toBe(2);
  });

  it("自定义模式不在输入框上方重复显示生效地址", async () => {
    await mount(
      { mode: "off", url: "", per_agent: { codex: { mode: "custom", url: "proxy.example:8080:u:p" } } },
      { codex: "http://u:p@proxy.example:8080" },
    );
    expect(screen.getByDisplayValue("proxy.example:8080:u:p")).toBeTruthy();
    expect(screen.queryByText(zh.proxy.effective("http://u:p@proxy.example:8080"))).toBeNull();
  });

  /// 直连时谈「生效范围」是噪音——没走代理，何来覆盖面。
  it("直连的模型不显示覆盖面", async () => {
    await mount(DIRECT);
    expect(screen.queryByText(zh.proxy.coverageFull)).toBeNull();
    expect(screen.queryByText(zh.proxy.coveragePartial)).toBeNull();
  });

  /// Claude Code 与 Codex 都不支持 SOCKS（官方明确 / 未编 reqwest 的 socks feature）。
  /// 静默放行的话，用户配完发现它们连不上，且毫无线索。
  it("SOCKS 代理落到不支持它的模型上时顶部告警", async () => {
    await mount(
      { mode: "custom", url: "socks5://127.0.0.1:1080", per_agent: {} },
      { "": "socks5://127.0.0.1:1080", claude: "socks5://127.0.0.1:1080", codex: "socks5://127.0.0.1:1080", kimi: "socks5://127.0.0.1:1080" },
    );
    expect(screen.getByText(zh.proxy.socksWarn)).toBeTruthy();
    expect(screen.getByText(zh.proxy.updaterSocksHint)).toBeTruthy();
  });
});

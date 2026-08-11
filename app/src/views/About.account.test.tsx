import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup, waitFor, act } from "@testing-library/react";

// vi.mock 会被提升到文件顶部，工厂函数里引用的外部变量必须走 vi.hoisted
// （否则 TDZ：ReferenceError: Cannot access 'api' before initialization，与 NewSessionPanel.test.tsx 同坑）。
const api = vi.hoisted(() => ({
  getAccounts: vi.fn(),
  listAgents: vi.fn(),
  installAgent: vi.fn(),
  loginAgent: vi.fn(),
  cancelLogin: vi.fn(),
  logoutAgent: vi.fn(),
  apiKeyLogin: vi.fn(),
  checkProviderHooks: vi.fn(),
  refreshUsage: vi.fn(),
  getSettings: vi.fn(),
  setSettings: vi.fn(),
  getRelaySecretStatus: vi.fn(),
  getRelaySecrets: vi.fn(),
  listRelayModels: vi.fn(),
  setRelaySecret: vi.fn(),
  agentPathGap: vi.fn(),
  addAgentToUserPath: vi.fn(),
  // 多账号
  listProfiles: vi.fn(),
  createProfile: vi.fn(),
  setActiveProfile: vi.fn(),
  renameProfile: vi.fn(),
  deleteProfile: vi.fn(),
  mergeProfileIntoDefault: vi.fn(),
}));
vi.mock("../api", async (o) => ({ ...(await o<typeof import("../api")>()), ...api }));
// 确认框统一走 appConfirm（应用主题原生小窗）；沿用 dialog.confirm 变量名以保住既有断言。
const dialog = vi.hoisted(() => ({ confirm: vi.fn() }));
vi.mock("../confirm", () => ({ appConfirm: dialog.confirm }));

// 收集所有 ProviderCard 注册的 install-done / login-done 回调，测试里手动广播
// （模拟 Tauri emit 到全部监听者）。进度不透传英文、前端不订阅 install-progress，故只收集 done。
const ev = vi.hoisted(() => ({
  doneCbs: [] as Array<(e: unknown) => void>,
  loginCbs: [] as Array<(e: unknown) => void>,
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: (event: string, cb: (e: unknown) => void) => {
    if (event === "install-done") ev.doneCbs.push(cb);
    if (event === "login-done") ev.loginCbs.push(cb);
    return Promise.resolve(() => {});
  },
}));
const fireDone = (provider: string, ok: boolean, logPath: string | null = null) =>
  act(() => ev.doneCbs.forEach((cb) => cb({ payload: { provider, ok, code: ok ? 0 : 1, logPath } })));
const fireLogin = (provider: string, outcome: "success" | "cancelled" | "timeout") => {
  const call = [...api.loginAgent.mock.calls].reverse().find(([p]) => p === provider);
  const operationId = call?.[3] ?? `unrelated-${provider}`;
  act(() => ev.loginCbs.forEach((cb) => cb({ payload: { provider, operationId, outcome } })));
};

/**
 * 顶部模型下拉切到某个 agent（按展示名）——列表现在一次只渲染选中的那一张卡（agent 一多，全部
 * 竖排要滚半天）。默认选中首个已安装的（beforeEach 里是 claude），要断言别家就先切过去。
 */
async function selectAgent(name: string) {
  await waitFor(() => expect(document.querySelector(".account-agent-switch .dd-btn")).toBeTruthy());
  fireEvent.click(document.querySelector(".account-agent-switch .dd-btn") as HTMLElement);
  fireEvent.click(await screen.findByRole("option", { name }));
}

import { AccountSection } from "./settings/AccountSection";
import { modelMenuPlacement } from "./settings/RelayAccess";
import { descriptors } from "../test/agents";
import { zh } from "../i18n/zh";

beforeEach(() => {
  // 顶部模型切换记进 localStorage：不清的话，上个用例选过的 agent 会成为下个用例的默认选中卡。
  localStorage.clear();
  Object.values(api).forEach((m) => m.mockReset());
  api.getAccounts.mockResolvedValue([{ provider: "claude", account: { email: "a@b.c" }, usage: null, usage_supported: true }]);
  api.listAgents.mockResolvedValue(descriptors(["claude", "codex"]));
  api.refreshUsage.mockResolvedValue({ lanes: [], note: null });
  api.getSettings.mockResolvedValue({ sticker_quota_providers: [] });
  api.setSettings.mockResolvedValue(undefined);
  api.getRelaySecretStatus.mockResolvedValue({ claude: false, codex: false, kimi: false });
  api.getRelaySecrets.mockResolvedValue({});
  api.listRelayModels.mockResolvedValue([]);
  api.setRelaySecret.mockResolvedValue(undefined);
  api.logoutAgent.mockResolvedValue(undefined);
  dialog.confirm.mockReset();
  dialog.confirm.mockResolvedValue(true);
  // 默认：bin 目录都在 PATH 上（无提示条），个别用例再覆盖。
  api.agentPathGap.mockResolvedValue(null);
  api.addAgentToUserPath.mockResolvedValue(undefined);
  // 默认 hooks 已接入（无「未接入」提示条），验接线的用例再覆盖。
  api.checkProviderHooks.mockResolvedValue("installed");
  // 默认：只有一个默认账号（没建过自定义账号）。
  api.listProfiles.mockResolvedValue([
    { id: null, name: "", active: true, account: { email: "a@b.c" } },
  ]);
  ev.doneCbs.length = 0;
  ev.loginCbs.length = 0;
});
afterEach(() => cleanup());

describe("AccountSection agent 卡", () => {
  it("插件未声明中转能力时不渲染中转入口", async () => {
    api.listAgents.mockResolvedValue(
      descriptors(["claude", "codex"]).map((agent) =>
        agent.id === "claude" ? { ...agent, relay: null } : agent,
      ),
    );
    render(<AccountSection />);
    // claude（默认选中）relay 置空 → 不给中转入口。
    await screen.findByTestId("agent-card-claude");
    expect(screen.queryByTestId("agent-access-claude")).toBeNull();
    // codex 声明了中转 → 切过去应有入口。
    await selectAgent("Codex");
    expect(await screen.findByTestId("agent-access-codex")).toBeTruthy();
  });

  /**
   * API Key 登录入口只给声明了能力的 agent（当前 gemini：OAuth 被官方停用，key 是唯一活路）。
   * 判据是后端下发的 `supports_api_key_login`——别家未登录时不得出现这个按钮，
   * 否则点保存只会得到后端一句「该 agent 不支持 API Key 登录」。
   */
  it("API Key 登录：gemini 有入口，保存成功即收起并重查账号；别家没有", async () => {
    api.listAgents.mockResolvedValue(descriptors(["claude", "gemini"]));
    api.getAccounts.mockResolvedValue([]); // 全员未登录
    api.apiKeyLogin.mockResolvedValue(undefined);
    render(<AccountSection />);
    // claude（默认选中）未登录：只有交互式登录，没有 API Key 入口。
    await screen.findByTestId("agent-card-claude");
    expect(screen.queryByTestId("agent-api-key-login-claude")).toBeNull();

    await selectAgent("Gemini CLI");
    fireEvent.click(await screen.findByTestId("agent-api-key-login-gemini"));
    const input = await screen.findByTestId("agent-api-key-input-gemini");
    fireEvent.change(input, { target: { value: "AIzaTest123" } });
    const accountQueries = api.getAccounts.mock.calls.length;
    fireEvent.click(screen.getByTestId("agent-api-key-save-gemini"));
    await waitFor(() => expect(api.apiKeyLogin).toHaveBeenCalledWith("gemini", "AIzaTest123"));
    // 成功 → 输入区收起 + 重查账号（同步落盘、当场生效，不走 login-done 轮询）。
    await waitFor(() => expect(screen.queryByTestId("agent-api-key-form-gemini")).toBeNull());
    await waitFor(() => expect(api.getAccounts.mock.calls.length).toBeGreaterThan(accountQueries));
  });

  it("API Key 保存失败：错误就地展示，输入区保留可重试", async () => {
    api.listAgents.mockResolvedValue(descriptors(["claude", "gemini"]));
    api.getAccounts.mockResolvedValue([]);
    api.apiKeyLogin.mockRejectedValue("API Key 含有空白或不可见字符");
    render(<AccountSection />);
    await selectAgent("Gemini CLI");
    fireEvent.click(await screen.findByTestId("agent-api-key-login-gemini"));
    fireEvent.change(await screen.findByTestId("agent-api-key-input-gemini"), {
      target: { value: "bad key" },
    });
    fireEvent.click(screen.getByTestId("agent-api-key-save-gemini"));
    const err = await screen.findByTestId("agent-api-key-error-gemini");
    expect(err.textContent).toContain("API Key 含有空白或不可见字符");
    expect(screen.getByTestId("agent-api-key-form-gemini")).toBeTruthy();
  });

  it("模型菜单在下方空间不足时自动向上展开", () => {
    expect(modelMenuPlacement({ top: 430, bottom: 461 }, 500, 240)).toEqual({
      opensUp: true,
      top: 185,
      maxHeight: 240,
    });
    expect(modelMenuPlacement({ top: 80, bottom: 111 }, 500, 240)).toEqual({
      opensUp: false,
      top: 116,
      maxHeight: 240,
    });
  });

  it("下拉列出全部 agent，一次只渲染选中的那张卡；未装的标未安装 + 安装按钮", async () => {
    render(<AccountSection />);
    // 默认渲染首个已安装的（claude），别家的卡此刻不在 DOM 里——这正是「不用滚」的由来。
    await screen.findByTestId("agent-card-claude");
    expect(screen.queryByTestId("agent-card-codex")).toBeNull();
    expect(screen.queryByTestId("agent-card-kimi")).toBeNull();

    // 下拉里五家都在（含未装的 kimi）。
    fireEvent.click(document.querySelector(".account-agent-switch .dd-btn") as HTMLElement);
    for (const name of ["Claude Code", "Codex", "Kimi Code", "Gemini CLI", "OpenCode"]) {
      expect(await screen.findByRole("option", { name })).toBeTruthy();
    }

    // 切到 kimi：它未装 → 卡片带安装按钮；此时 claude 卡已从 DOM 撤下。
    fireEvent.click(screen.getByRole("option", { name: "Kimi Code" }));
    expect(await screen.findByTestId("agent-install-kimi")).toBeTruthy();
    expect(screen.queryByTestId("agent-card-claude")).toBeNull();

    // 切回 claude：已装 → 无安装按钮。
    await selectAgent("Claude Code");
    expect(await screen.findByTestId("agent-card-claude")).toBeTruthy();
    expect(screen.queryByTestId("agent-install-claude")).toBeNull();
  });

  it("点安装调 installAgent", async () => {
    api.installAgent.mockResolvedValue(undefined);
    render(<AccountSection />);
    await selectAgent("Kimi Code");
    fireEvent.click(await screen.findByTestId("agent-install-kimi"));
    await waitFor(() => expect(api.installAgent).toHaveBeenCalledWith("kimi"));
  });

  it("点安装进入安装中：转圈 + 本地化「安装中…」（不透传脚本英文）", async () => {
    api.installAgent.mockResolvedValue(undefined);
    render(<AccountSection />);
    await selectAgent("Kimi Code");
    fireEvent.click(await screen.findByTestId("agent-install-kimi"));
    // 安装中指示出现，且有非空本地化文案（不硬编码 i18n 串，避免 locale 依赖）
    const installing = await screen.findByTestId("agent-installing-kimi");
    expect(installing.textContent?.trim().length).toBeGreaterThan(0);
    // 安装按钮已被安装中指示替换
    expect(screen.queryByTestId("agent-install-kimi")).toBeNull();
  });

  it("install-done 成功后重查检测、卡片转已装", async () => {
    api.installAgent.mockResolvedValue(undefined);
    // 初次未装 kimi；装完重查返回含 kimi
    api.listAgents.mockResolvedValueOnce(descriptors(["claude", "codex"])).mockResolvedValue(descriptors(["claude", "codex", "kimi"]));
    render(<AccountSection />);
    await selectAgent("Kimi Code");
    fireEvent.click(await screen.findByTestId("agent-install-kimi"));
    await waitFor(() => expect(screen.getByTestId("agent-installing-kimi")).toBeTruthy());
    fireDone("kimi", true);
    await waitFor(() => expect(screen.queryByTestId("agent-install-kimi")).toBeNull());
    expect(screen.queryByTestId("agent-installing-kimi")).toBeNull();
  });

  it("install-done 失败：退出安装中、显示重试按钮", async () => {
    api.installAgent.mockResolvedValue(undefined);
    render(<AccountSection />);
    await selectAgent("Kimi Code");
    fireEvent.click(await screen.findByTestId("agent-install-kimi"));
    await waitFor(() => expect(screen.getByTestId("agent-installing-kimi")).toBeTruthy());
    fireDone("kimi", false);
    await waitFor(() => expect(screen.queryByTestId("agent-installing-kimi")).toBeNull());
    // 仍未装 → 按钮回来（文案为重试），testid 不变
    expect(screen.getByTestId("agent-install-kimi")).toBeTruthy();
  });

  // 已登录的卡片只显示邮箱。此前拼 显示名 · 邮箱 · 组织，个人账号的组织名恰是
  // 「<邮箱>'s Organization」，于是同一个邮箱在一行里出现两次，又长又重复。
  it("已登录只显示邮箱，不带显示名与组织", async () => {
    api.getAccounts.mockResolvedValue([
      { provider: "claude", account: { email: "a@b.c", display_name: "Larry", organization: "a@b.c's Organization" }, usage: null, usage_supported: true },
    ]);
    render(<AccountSection />);
    const desc = await screen.findByTestId("agent-desc-claude");
    expect(desc.textContent).toBe("a@b.c");
  });

  // 没有邮箱的登录方式（如 codex 的 API key）不能让描述行变空白。
  it("无邮箱时回退到显示名/登录标签", async () => {
    api.getAccounts.mockResolvedValue([
      { provider: "claude", account: { login_label: "API key" }, usage: null, usage_supported: true },
    ]);
    render(<AccountSection />);
    const desc = await screen.findByTestId("agent-desc-claude");
    expect(desc.textContent).toBe("API key");
  });

  it("中转模式不展示残留的官方账号配额或登录按钮", async () => {
    api.getAccounts.mockResolvedValue([
      {
        provider: "claude",
        account: { email: "old-official@example.com" },
        usage: { lanes: [{ kind: "five_hour", used_pct: 50 }], note: null },
        usage_supported: false,
        relay_enabled: true,
      },
    ]);
    render(<AccountSection />);
    expect((await screen.findByTestId("agent-desc-claude")).textContent).toBe(zh.account.relayActive);
    expect(screen.queryByTestId("agent-login-claude")).toBeNull();
    expect(screen.queryByTestId("agent-logout-claude")).toBeNull();
    expect(screen.queryByText(zh.account.quota)).toBeNull();
    // 账号列表管的是官方登录身份，中转与之互斥——一并隐藏，免得误导。
    expect(screen.queryByTestId("profiles-claude")).toBeNull();
  });

  it("官方账号可确认退出，成功后重新读取账号状态", async () => {
    api.getAccounts
      .mockResolvedValueOnce([{ provider: "claude", account: { email: "a@b.c" }, usage: null, usage_supported: true }])
      .mockResolvedValue([{ provider: "claude", account: null, usage: null, usage_supported: false }]);
    render(<AccountSection />);

    fireEvent.click(await screen.findByTestId("agent-logout-claude"));
    await waitFor(() => expect(dialog.confirm).toHaveBeenCalled());
    await waitFor(() => expect(api.logoutAgent).toHaveBeenCalledWith("claude"));
    await waitFor(() => expect(screen.queryByTestId("agent-logout-claude")).toBeNull());
    expect(screen.getByTestId("agent-login-claude")).toBeTruthy();
  });

  it("取消退出确认时不清除官方凭据", async () => {
    dialog.confirm.mockResolvedValue(false);
    render(<AccountSection />);
    fireEvent.click(await screen.findByTestId("agent-logout-claude"));
    await waitFor(() => expect(dialog.confirm).toHaveBeenCalled());
    expect(api.logoutAgent).not.toHaveBeenCalled();
  });

  /**
   * 活跃账号持续可见：切到过自定义账号，provider 卡片标题行就一直挂着「账号：<名>」徽章——
   * 这是「切过一次就忘了，之后会话全写进隔离账号」的防线。默认账号（active_profile_name 缺席）
   * 什么都不显示：没建过账号的用户零感知。
   */
  it("非默认账号活跃时卡片显示账号徽章，默认账号不显示", async () => {
    render(<AccountSection />);
    await screen.findByTestId("agent-card-claude");
    // 默认 mock 没有 active_profile_name（默认账号）→ 无徽章。
    expect(screen.queryByTestId("agent-profile-claude")).toBeNull();

    api.getAccounts.mockResolvedValue([
      { provider: "claude", account: { email: "a@b.c" }, usage: null, usage_supported: true, active_profile_name: "工作" },
    ]);
    cleanup();
    render(<AccountSection />);
    const badge = await screen.findByTestId("agent-profile-claude");
    expect(badge.textContent).toBe(zh.account.activeProfileBadge("工作"));
  });

  it("模型卡内用官方账号 / API 中转二选一，预配置完整时可直接切换", async () => {
    api.getSettings.mockResolvedValue({
      sticker_quota_providers: [],
      relay: {
        per_agent: {
          claude: {
            enabled: false,
            base_url: "https://relay.example/v1",
            model: "claude-relay",
            protocol: "",
            auth: "bearer",
          },
        },
      },
    });
    api.getRelaySecretStatus.mockResolvedValue({ claude: true, codex: false, kimi: false });
    render(<AccountSection />);

    const card = await screen.findByTestId("agent-card-claude");
    const relayChoice = Array.from(card.querySelectorAll('[role="radio"]')).find(
      (el) => el.textContent === zh.relay.title,
    ) as HTMLElement;
    expect(relayChoice).toBeTruthy();
    fireEvent.click(relayChoice);

    await waitFor(() => expect(api.setSettings).toHaveBeenCalled());
    const saved = api.setSettings.mock.calls.at(-1)?.[0];
    expect(saved.relay.per_agent.claude.enabled).toBe(true);
    expect(card.querySelector('[role="radio"][aria-checked="true"]')?.textContent).toBe(zh.relay.title);
  });

  it("中转密钥仍走独立命令，不进入 Settings", async () => {
    render(<AccountSection />);
    const card = await screen.findByTestId("agent-card-claude");
    const relayChoice = Array.from(card.querySelectorAll('[role="radio"]')).find(
      (el) => el.textContent === zh.relay.title,
    ) as HTMLElement;
    fireEvent.click(relayChoice);
    const secret = await screen.findByPlaceholderText(zh.relay.secretPlaceholder);
    fireEvent.change(secret, { target: { value: "sk-never-in-settings" } });
    fireEvent.blur(secret);

    await waitFor(() =>
      expect(api.setRelaySecret).toHaveBeenCalledWith("claude", "sk-never-in-settings"),
    );
    expect(JSON.stringify(api.setSettings.mock.calls)).not.toContain("sk-never-in-settings");
  });

  it("已保存的中转密钥明文回填（便于核对），清空后删除", async () => {
    api.getSettings.mockResolvedValue({
      sticker_quota_providers: [],
      relay: { per_agent: { claude: {
        enabled: true,
        base_url: "https://relay.example/v1",
        model: "claude-relay",
        protocol: "",
        auth: "bearer",
      } } },
    });
    api.getRelaySecretStatus.mockResolvedValue({ claude: true, codex: false, kimi: false });
    api.getRelaySecrets.mockResolvedValue({ claude: "sk-visible-local" });
    render(<AccountSection />);
    const secret = await screen.findByDisplayValue("sk-visible-local") as HTMLInputElement;
    // 令牌明文展示：粘贴错一个字符就是 401，用户需要肉眼核对，且仅本机可见。
    expect(secret.type).toBe("text");
    fireEvent.change(secret, { target: { value: "" } });
    fireEvent.blur(secret);
    await waitFor(() => expect(api.setRelaySecret).toHaveBeenCalledWith("claude", ""));
    await waitFor(() =>
      expect(api.setSettings.mock.calls.at(-1)?.[0].relay.per_agent.claude.enabled).toBe(false),
    );
  });

  it("旧中转规则协议为空时保存插件默认协议", async () => {
    api.listAgents.mockResolvedValue(descriptors(["claude", "codex", "kimi"]));
    api.getSettings.mockResolvedValue({
      sticker_quota_providers: [],
      relay: { per_agent: { kimi: {
        enabled: false,
        base_url: "https://relay.example/v1",
        model: "kimi-for-coding",
        protocol: "",
        auth: "bearer",
      } } },
    });
    api.getRelaySecretStatus.mockResolvedValue({ claude: false, codex: false, kimi: true });
    render(<AccountSection />);
    await selectAgent("Kimi Code");
    const card = await screen.findByTestId("agent-card-kimi");
    const relayChoice = Array.from(card.querySelectorAll('[role="radio"]')).find(
      (el) => el.textContent === zh.relay.title,
    ) as HTMLElement;
    fireEvent.click(relayChoice);

    await waitFor(() => expect(api.setSettings).toHaveBeenCalled());
    const saved = api.setSettings.mock.calls.at(-1)?.[0].relay.per_agent.kimi;
    expect(saved.enabled).toBe(true);
    expect(saved.protocol).toBe("kimi");
  });

  it("模型选择器从中转获取选项，同时仍可输入自定义模型", async () => {
    api.getSettings.mockResolvedValue({
      sticker_quota_providers: [],
      relay: { per_agent: { claude: {
        enabled: true,
        base_url: "https://relay.example/v1",
        model: "old-model",
        protocol: "",
        auth: "bearer",
      } } },
    });
    api.getRelaySecretStatus.mockResolvedValue({ claude: true, codex: false, kimi: false });
    api.listRelayModels.mockResolvedValue(["relay-model-a", "relay-model-b"]);
    render(<AccountSection />);

    const input = await screen.findByDisplayValue("old-model");
    await waitFor(() => expect(api.getRelaySecretStatus).toHaveBeenCalled());
    fireEvent.focus(input);
    await waitFor(() => expect(api.listRelayModels).toHaveBeenCalledWith(
      "claude", "https://relay.example/v1", "", "bearer",
    ));
    fireEvent.change(input, { target: { value: "relay-model" } });
    fireEvent.click(await screen.findByText("relay-model-b"));
    await waitFor(() => expect(api.setSettings).toHaveBeenCalled());
    expect(api.setSettings.mock.calls.at(-1)?.[0].relay.per_agent.claude.model).toBe("relay-model-b");

    fireEvent.focus(input);
    fireEvent.change(input, { target: { value: "vendor-private-model" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(api.setSettings.mock.calls.at(-1)?.[0].relay.per_agent.claude.model).toBe("vendor-private-model"));
  });

  // 回归：此前安装输出被丢进 Stdio::null()，失败时用户拿不到任何可排查的东西。
  it("install-done 失败：给出安装日志路径（有 logPath 时）", async () => {
    api.installAgent.mockResolvedValue(undefined);
    render(<AccountSection />);
    await selectAgent("Kimi Code");
    fireEvent.click(await screen.findByTestId("agent-install-kimi"));
    fireDone("kimi", false, "C:\\Users\\x\\.meowo\\install-kimi.log");
    const log = await screen.findByTestId("agent-install-log-kimi");
    expect(log.textContent).toContain("install-kimi.log");
  });

  // 回归：claude 的安装器不写 PATH 也照样 exit 0——「装好了」不等于「终端里敲得出来」。
  // 提示条须在挂载时就查（多数受害者是早就装好、从没进过 PATH 的用户），不能只在装完时查。
  it("已装但 bin 目录不在 PATH：显示提示条，可一键写入", async () => {
    api.agentPathGap.mockResolvedValue("C:\\Users\\x\\.local\\bin");
    render(<AccountSection />);
    const gap = await screen.findByTestId("agent-path-gap-claude");
    // 提示条刻意低调：正文只留一句「为什么该点」，**完整路径挪进 tooltip**——
    // 这条对多数人是背景噪音（装完就在 PATH 上），横一条长提示会喧宾夺主。
    expect(gap.getAttribute("title")).toContain(".local\\bin");
    expect(gap.textContent).not.toContain(".local\\bin");

    fireEvent.click(screen.getByTestId("agent-add-path-claude"));
    await waitFor(() => expect(api.addAgentToUserPath).toHaveBeenCalledWith("claude"));
    // 写入成功 → 提示条消失，转为「请重开终端」提示
    await waitFor(() => expect(screen.queryByTestId("agent-path-gap-claude")).toBeNull());
    expect(screen.getByTestId("agent-path-msg-claude")).toBeTruthy();
  });

  it("写入 PATH 失败：保留提示条并给出失败说明", async () => {
    api.agentPathGap.mockResolvedValue("C:\\Users\\x\\.local\\bin");
    api.addAgentToUserPath.mockRejectedValue(new Error("denied"));
    render(<AccountSection />);
    fireEvent.click(await screen.findByTestId("agent-add-path-claude"));
    await waitFor(() => expect(screen.getByTestId("agent-path-msg-claude")).toBeTruthy());
    // 没写成功 → 提示条不该消失，否则用户以为已修好
    expect(screen.getByTestId("agent-path-gap-claude")).toBeTruthy();
  });

  /// 后端在**跑脚本之前**就失败（引导脚本被 Cloudflare 人机校验拦截）：错误串是我们自己的中文
  /// 诊断，必须原样显示。此时还没有日志文件——旧代码 `.catch(() => setInstallState("error"))`
  /// 把它整个丢掉，用户只看到一句通用的「安装失败」，一点线索都没有。
  it("installAgent 直接 reject：显示后端诊断，而不是通用文案", async () => {
    const diag = "https://claude.ai/install.ps1 返回了 Cloudflare 人机校验页，而不是安装脚本。";
    api.installAgent.mockRejectedValue(diag);
    render(<AccountSection />);
    await selectAgent("Kimi Code");
    fireEvent.click(await screen.findByTestId("agent-install-kimi"));
    const errLine = await screen.findByTestId("agent-install-error-kimi");
    expect(errLine.textContent).toContain("Cloudflare");
    // 没有日志文件（脚本压根没跑），故不显示日志路径行。
    expect(screen.queryByTestId("agent-install-log-kimi")).toBeNull();
  });

  /// 两种失败不能串台：脚本真的跑了再失败时，该给通用文案 + 日志路径，而不是上一次的诊断。
  it("先被 CF 拦、重试后脚本跑起来又失败：诊断被清掉，改显示日志路径", async () => {
    api.installAgent.mockRejectedValueOnce("被 Cloudflare 人机校验拦截").mockResolvedValue(undefined);
    render(<AccountSection />);
    await selectAgent("Kimi Code");
    fireEvent.click(await screen.findByTestId("agent-install-kimi"));
    await waitFor(() => expect(screen.getByTestId("agent-install-error-kimi").textContent).toContain("Cloudflare"));

    fireEvent.click(screen.getByTestId("agent-install-kimi")); // 重试
    fireDone("kimi", false, "C:\\Users\\me\\.meowo\\install-kimi.log");
    await waitFor(() => expect(screen.getByTestId("agent-install-log-kimi")).toBeTruthy());
    expect(screen.getByTestId("agent-install-error-kimi").textContent).not.toContain("Cloudflare");
  });

  it("install-done 失败：显示本地化失败说明 + 重试按钮", async () => {
    api.installAgent.mockResolvedValue(undefined);
    render(<AccountSection />);
    await selectAgent("Kimi Code");
    fireEvent.click(await screen.findByTestId("agent-install-kimi"));
    await waitFor(() => expect(screen.getByTestId("agent-installing-kimi")).toBeTruthy());
    // 失败——错误行始终显示本地化 installFailed 文案
    fireDone("kimi", false);
    await waitFor(() => expect(screen.queryByTestId("agent-installing-kimi")).toBeNull());
    // 失败说明行可见且非空（不硬编码 i18n 文案，避免 locale 依赖）
    const errLine = screen.getByTestId("agent-install-error-kimi");
    expect(errLine.textContent?.trim().length).toBeGreaterThan(0);
    // 重试按钮仍在（testid 与安装共用）
    expect(screen.getByTestId("agent-install-kimi")).toBeTruthy();
  });
});

describe("AccountSection 登录", () => {
  it("已装未登录才显示登录按钮；已登录/未装都不显示", async () => {
    render(<AccountSection />);
    // claude（默认选中）已装且有账号 → 已登录，无登录按钮。
    await screen.findByTestId("agent-card-claude");
    expect(screen.queryByTestId("agent-login-claude")).toBeNull();
    // codex 已装但 getAccounts 没返回它 → account 为 null → 未登录，给登录按钮。
    await selectAgent("Codex");
    expect(await screen.findByTestId("agent-login-codex")).toBeTruthy();
    // kimi 未装 → 该先安装，不给登录按钮。
    await selectAgent("Kimi Code");
    await screen.findByTestId("agent-install-kimi");
    expect(screen.queryByTestId("agent-login-kimi")).toBeNull();
  });

  /**
   * 没有账号概念的 agent：已装也不给登录按钮、不报「未登录」。
   *
   * 回归：`getAccounts()` 只返回**声明了账号能力**的 agent。旧逻辑靠 `payload == null` 判定未登录，
   * 于是给没有账号能力的 agent 也亮出登录按钮——而它的 `login_argv()` 是 None，点下去后端只能回
   * 一句「拉起登录失败」。给出走不通的入口，比不给入口更糟：用户会以为是自己的环境有问题，
   * 反复去点。（gemini / opencode 真的这么翻过车，直到它们的登录入口被补上。）
   *
   * 判据必须是后端下发的 `supports_account`，不能靠「账号查不出来」去猜——那与「真的没登录」
   * （codex 那种）长得一模一样。当前五家都有账号能力，故这里手造一个没有的。
   */
  it("无账号概念的 agent 已装也不给登录按钮，且不报未登录", async () => {
    api.listAgents.mockResolvedValue([
      { id: "claude", display_name: "Claude Code", installed: true, supports_proxy: true, supports_account: true, supports_profiles: true },
      { id: "codex", display_name: "Codex", installed: true, supports_proxy: true, supports_account: true, supports_profiles: true },
      { id: "noacct", display_name: "No Account", installed: true, supports_proxy: false, supports_account: false, supports_profiles: false },
    ]);
    render(<AccountSection />);

    // 对照：codex 有账号能力但没登录 → 照旧给按钮。
    await selectAgent("Codex");
    expect(await screen.findByTestId("agent-login-codex")).toBeTruthy();

    // 已装，但没有账号概念 → 卡片在，登录按钮不在。
    await selectAgent("No Account");
    expect(await screen.findByTestId("agent-card-noacct")).toBeTruthy();
    expect(screen.queryByTestId("agent-login-noacct")).toBeNull();
    // 也不该显示「未登录」——它没有「登录」这回事。
    expect(screen.queryByTestId("agent-desc-noacct")).toBeNull();
  });

  it("点登录调 loginAgent 并进入等待态", async () => {
    api.loginAgent.mockResolvedValue(undefined);
    render(<AccountSection />);
    await selectAgent("Codex");
    const btn = await screen.findByTestId("agent-login-codex");
    fireEvent.click(btn);
    await waitFor(() => expect(api.loginAgent).toHaveBeenCalledWith("codex", undefined, undefined, expect.any(String)));
    // spawn 成功后不落回 idle——等 login-done。按钮此时变成「取消等待」，而不是一个死掉的禁用按钮：
    // 终端可能已被关掉，而后端要 5 分钟才超时。
    await waitFor(() => expect(screen.getByTestId("agent-login-codex").textContent).toBe(zh.account.cancelLogin));
    expect((screen.getByTestId("agent-login-codex") as HTMLButtonElement).disabled).toBe(false);
  });

  it("切换 agent 后仍保留等待中的 operationId，并可用同一 id 取消", async () => {
    api.loginAgent.mockResolvedValue(undefined);
    api.cancelLogin.mockResolvedValue(undefined);
    render(<AccountSection />);
    await selectAgent("Codex");
    fireEvent.click(await screen.findByTestId("agent-login-codex"));
    await waitFor(() => expect(api.loginAgent).toHaveBeenCalled());
    const operationId = api.loginAgent.mock.calls[0][3];

    await selectAgent("Claude Code");
    await selectAgent("Codex");
    await waitFor(() => expect(screen.getByTestId("agent-login-codex").textContent).toBe(zh.account.cancelLogin));

    fireEvent.click(screen.getByTestId("agent-login-codex"));
    await waitFor(() => expect(api.cancelLogin).toHaveBeenCalledWith("codex", operationId));
  });

  it("login-done 成功 → 重查账号（卡片可转已登录）", async () => {
    api.loginAgent.mockResolvedValue(undefined);
    render(<AccountSection />);
    await selectAgent("Codex");
    fireEvent.click(await screen.findByTestId("agent-login-codex"));
    await waitFor(() => expect(api.loginAgent).toHaveBeenCalled());
    const before = api.getAccounts.mock.calls.length;
    // 登录成功后 codex 也有账号了
    api.getAccounts.mockResolvedValue([
      { provider: "claude", account: { email: "a@b.c" }, usage: null, usage_supported: true },
      { provider: "codex", account: { login_label: "API Key" }, usage: null, usage_supported: false },
    ]);
    fireLogin("codex", "success");
    await waitFor(() => expect(api.getAccounts.mock.calls.length).toBeGreaterThan(before));
    await waitFor(() => expect(screen.queryByTestId("agent-login-codex")).toBeNull());
  });

  it("login-done 超时 → 落回可点 + 显示本地化提示（超时不是登录失败）", async () => {
    api.loginAgent.mockResolvedValue(undefined);
    render(<AccountSection />);
    await selectAgent("Codex");
    fireEvent.click(await screen.findByTestId("agent-login-codex"));
    await waitFor(() => expect(screen.getByTestId("agent-login-codex").textContent).toBe(zh.account.cancelLogin));
    fireLogin("codex", "timeout");
    await waitFor(() => expect(screen.getByTestId("agent-login-codex").textContent).toBe(zh.account.login));
    expect(screen.getByTestId("agent-login-error-codex").textContent).toBe(zh.account.loginTimeout);
  });

  /// 起因：点完登录后如果终端被关掉（用户手动关、崩溃、agent 自己退出），后端只轮询账号文件，
  /// 要 5 分钟才超时。这五分钟里按钮一直是「等待登录…」且不可点——用户既不能重来也不知道发生了什么。
  it("等待中点按钮 → 调 cancel_login，落回可点并提示已取消（而非「未检测到登录完成」）", async () => {
    api.loginAgent.mockResolvedValue(undefined);
    api.cancelLogin.mockResolvedValue(undefined);
    render(<AccountSection />);
    await selectAgent("Codex");
    fireEvent.click(await screen.findByTestId("agent-login-codex"));
    await waitFor(() => expect(screen.getByTestId("agent-login-codex").textContent).toBe(zh.account.cancelLogin));

    fireEvent.click(screen.getByTestId("agent-login-codex"));
    await waitFor(() => expect(api.cancelLogin).toHaveBeenCalledWith("codex", expect.any(String)));

    // 收尾由后端 emit login-done（它会再查一次账号；这里模拟「确实没登上」）。
    fireLogin("codex", "cancelled");
    await waitFor(() => expect(screen.getByTestId("agent-login-codex").textContent).toBe(zh.account.login));
    // 取消 ≠ 超时：文案必须区分，否则用户以为是没检测到。
    expect(screen.getByTestId("agent-login-error-codex").textContent).toBe(zh.account.loginCancelled);
  });

  /// 取消时后端会再查一次账号——用户可能已经在终端里登完了，只是嫌等得慢。此时应转「已登录」。
  it("取消时若其实已登录成功 → 转已登录，不显示取消提示", async () => {
    api.loginAgent.mockResolvedValue(undefined);
    api.cancelLogin.mockResolvedValue(undefined);
    render(<AccountSection />);
    await selectAgent("Codex");
    fireEvent.click(await screen.findByTestId("agent-login-codex"));
    await waitFor(() => expect(screen.getByTestId("agent-login-codex").textContent).toBe(zh.account.cancelLogin));
    fireEvent.click(screen.getByTestId("agent-login-codex"));
    await waitFor(() => expect(api.cancelLogin).toHaveBeenCalled());

    api.getAccounts.mockResolvedValue([
      { provider: "claude", account: { email: "a@b.c" }, usage: null, usage_supported: true },
      { provider: "codex", account: { login_label: "API Key" }, usage: null, usage_supported: false },
    ]);
    fireLogin("codex", "success"); // 后端复查发现真登上了
    await waitFor(() => expect(screen.queryByTestId("agent-login-codex")).toBeNull());
    expect(screen.queryByTestId("agent-login-error-codex")).toBeNull();
  });

  /// 装完 / 登录后，后端会顺手接线（best-effort），前端据此重查一次 hooks 状态——接上了就让
  /// 「未接入」提示条自动消失，不必让用户再去点「修复连接」。
  it("登录成功后重查 hooks：接上了则「未接入」提示条消失", async () => {
    // codex 已装，但 hooks 初始未接入；登录后端接线 → codex 的第二次查询返回 installed。
    // 按 provider 计数（mockResolvedValueOnce 是全局队列，claude 的挂载查询会抢走它）。
    let codexChecks = 0;
    api.checkProviderHooks.mockImplementation((p: string) => {
      if (p !== "codex") return Promise.resolve("installed");
      codexChecks += 1;
      return Promise.resolve(codexChecks === 1 ? "missing" : "installed");
    });
    api.getAccounts.mockResolvedValue([
      { provider: "claude", account: { email: "a@b.c" }, usage: null, usage_supported: true },
      { provider: "codex", account: null, usage: null, usage_supported: false }, // 未登录
    ]);
    api.loginAgent.mockResolvedValue(undefined);
    render(<AccountSection />);
    await selectAgent("Codex");

    // 未接入 → 提示条 + 修复按钮在。
    expect(await screen.findByTestId("agent-repair-hooks-codex")).toBeTruthy();

    fireEvent.click(screen.getByTestId("agent-login-codex"));
    fireLogin("codex", "success");
    // 登录成功 → 重查 hooks 得 installed → 提示条消失。
    await waitFor(() => expect(screen.queryByTestId("agent-repair-hooks-codex")).toBeNull());
  });

  it("拉起登录失败 → 落回可点 + 显示提示", async () => {
    api.loginAgent.mockRejectedValue(new Error("启动终端失败"));
    render(<AccountSection />);
    await selectAgent("Codex");
    fireEvent.click(await screen.findByTestId("agent-login-codex"));
    await waitFor(() => expect(screen.getByTestId("agent-login-error-codex")).toBeTruthy());
    expect((screen.getByTestId("agent-login-codex") as HTMLButtonElement).disabled).toBe(false);
  });

  it("装完自动引导：install-done 成功后，该 agent 的登录按钮被标为主要动作", async () => {
    api.installAgent.mockResolvedValue(undefined);
    // 初次未装 kimi；装完重查返回含 kimi（此时 kimi 无账号 → 未登录）
    api.listAgents.mockResolvedValueOnce(descriptors(["claude", "codex"])).mockResolvedValue(descriptors(["claude", "codex", "kimi"]));
    render(<AccountSection />);
    await selectAgent("Kimi Code");
    fireEvent.click(await screen.findByTestId("agent-install-kimi"));
    fireDone("kimi", true);
    const loginBtn = await screen.findByTestId("agent-login-kimi");
    // 「装完 → 登录」串成一条链路：按钮升为 primary，而非埋在一堆次要按钮里。
    expect(loginBtn.className).toContain("provider-card-action-primary");
    // 未经安装流程的 codex 则是普通次要按钮（切过去看）。
    await selectAgent("Codex");
    expect((await screen.findByTestId("agent-login-codex")).className).not.toContain("provider-card-action-primary");
  });
});

describe("AccountSection 多账号", () => {
  /**
   * 登录**必须带上那个账号自己的 id**。
   *
   * 这是整个多账号功能的命门：登录会把凭据写进「注入的隔离变量所指的目录」，而那个变量由 profile id
   * 决定。漏传 id，新账号的登录就会把**默认账号**的凭据覆盖掉——用户以为自己加了个账号，
   * 其实是把原来那个换掉了，而且毫无提示。
   */
  it("给某个账号点登录时，把它的 id 传给 loginAgent", async () => {
    api.listProfiles.mockResolvedValue([
      { id: null, name: "", active: true, account: { email: "a@b.c" } },
      { id: "work", name: "工作", active: false, account: null }, // 未登录
    ]);
    api.loginAgent.mockResolvedValue(undefined);
    render(<AccountSection />);
    // claude 是默认选中卡，直接就在，不必切换。

    fireEvent.click(await screen.findByTestId("profile-login-claude-work"));
    await waitFor(() =>
      expect(api.loginAgent).toHaveBeenCalledWith("claude", undefined, "work", expect.any(String))
    );
  });

  it("点账号行即切换；已是活跃的那行不可点", async () => {
    api.listProfiles.mockResolvedValue([
      { id: null, name: "", active: true, account: { email: "a@b.c" } },
      { id: "work", name: "工作", active: false, account: { email: "w@b.c" } },
    ]);
    api.setActiveProfile.mockResolvedValue(undefined);
    render(<AccountSection />);

    const row = await screen.findByTestId("profile-claude-work");
    fireEvent.click(row.querySelector(".profile-row-main")!);
    await waitFor(() => expect(api.setActiveProfile).toHaveBeenCalledWith("claude", "work"));

    // 活跃那行（默认账号）的主按钮是禁用的——点它没有意义。
    const def = screen.getByTestId("profile-claude-__default__");
    expect((def.querySelector(".profile-row-main") as HTMLButtonElement).disabled).toBe(true);
  });

  /**
   * 不支持多账号的 agent（gemini：数据目录不可被环境变量覆盖）不显示账号列表。
   *
   * 判据必须是后端下发的 `supports_profiles`，不能靠「列表只有一条」推断——那与「只建了默认账号」
   * 长得一模一样。谎称支持的后果是两个「账号」静默共用同一份凭据。
   */
  it("不支持多账号的 agent 不显示账号列表", async () => {
    api.listAgents.mockResolvedValue([
      { id: "claude", display_name: "Claude Code", installed: true, supports_proxy: true, supports_account: true, supports_profiles: true },
      { id: "gemini", display_name: "Gemini CLI", installed: true, supports_proxy: false, supports_account: true, supports_profiles: false },
    ]);
    render(<AccountSection />);

    await waitFor(() => expect(screen.getByTestId("profiles-claude")).toBeTruthy());
    expect(screen.queryByTestId("profiles-gemini")).toBeNull();
  });

  /**
   * 上下文占用不支持的 agent（gemini/opencode）明写「不支持」，不留空白让用户以为是 bug。
   * 支持的（claude）不显示这行。
   */
  it("上下文不支持的 agent 显式标注，支持的不标", async () => {
    // gemini 需已装才谈得上「能力」——未装时卡片只有安装按钮。
    api.listAgents.mockResolvedValue(descriptors(["claude", "codex", "gemini"]));
    render(<AccountSection />);
    // claude（默认选中、支持上下文）→ 没有这行。
    await screen.findByTestId("agent-card-claude");
    expect(screen.queryByTestId("agent-context-unsupported-claude")).toBeNull();
    // 切到 gemini（已装、不支持上下文）→ 有这行。
    await selectAgent("Gemini CLI");
    expect(await screen.findByTestId("agent-context-unsupported-gemini")).toBeTruthy();
  });

  /**
   * 会员等级是**徽章**，不是描述行。
   *
   * 描述行说的是「这是哪个账号」（邮箱；kimi 给不出邮箱，退到 userId 短码）。把等级写进那一行，
   * 账号看起来就像叫「Allegretto」——而且两个同档账号会长得一模一样。
   *
   * 等级由用量接口捎回（kimi 本地读不到），所以只有活跃账号拿得到，非活跃行没有徽章。
   */
  it("kimi 的会员等级是徽章，描述行仍是账号标识", async () => {
    api.listAgents.mockResolvedValue([
      { id: "kimi", display_name: "Kimi Code", installed: true, supports_proxy: true, supports_account: true, supports_profiles: true },
    ]);
    api.listProfiles.mockResolvedValue([
      { id: null, name: "", active: true, account: { login_label: "cnta…5a4g", plan: "Allegretto" } },
      { id: "alt", name: "小号", active: false, account: { login_label: "d0ah…9x2k" } },
    ]);
    render(<AccountSection />);

    const active = await screen.findByTestId("profile-kimi-__default__");
    expect(active.querySelector(".profile-badge-plan")?.textContent).toBe("Allegretto");
    // 描述行不被等级顶掉：它得说清这是哪个账号。
    expect(active.querySelector(".profile-desc")?.textContent).toBe("cnta…5a4g");

    // 非活跃行拿不到等级（用量缓存不按 profile 分键，只讲活跃账号的事）→ 无徽章，但仍有标识。
    const alt = screen.getByTestId("profile-kimi-alt");
    expect(alt.querySelector(".profile-badge-plan")).toBeNull();
    expect(alt.querySelector(".profile-desc")?.textContent).toBe("d0ah…9x2k");
  });
});

describe("AccountSection 账号的增删改", () => {
  const twoProfiles = () =>
    api.listProfiles.mockResolvedValue([
      { id: null, name: "", active: false, account: { email: "a@b.c" } },
      { id: "work", name: "工作", active: true, account: { email: "w@b.c" } }, // 正在使用
    ]);

  /**
   * **正在使用的账号也必须删得掉**（后端会把活跃标记落回默认账号）。
   *
   * 确认框走 `appConfirm`，**不是 `window.confirm`**——后者在 Tauri 的
   * webview 里会被直接吞掉、恒返回 false：按钮看着能点，点了却什么都不发生。这正是它此前删不掉的原因。
   */
  it("正在使用的账号也能删除，且走 appConfirm", async () => {
    twoProfiles();
    api.deleteProfile.mockResolvedValue(undefined);
    render(<AccountSection />);

    fireEvent.click(await screen.findByTestId("profile-menu-claude-work"));
    fireEvent.click(screen.getByTestId("profile-menu-claude-work-delete"));
    await waitFor(() => expect(dialog.confirm).toHaveBeenCalled());
    await waitFor(() => expect(api.deleteProfile).toHaveBeenCalledWith("claude", "work"));
  });

  it("确认框点取消 → 不删", async () => {
    twoProfiles();
    dialog.confirm.mockResolvedValue(false);
    render(<AccountSection />);

    fireEvent.click(await screen.findByTestId("profile-menu-claude-work"));
    fireEvent.click(screen.getByTestId("profile-menu-claude-work-delete"));
    await waitFor(() => expect(dialog.confirm).toHaveBeenCalled());
    expect(api.deleteProfile).not.toHaveBeenCalled();
  });

  /** 改名只动展示名，不动 id——id 是目录名，改了等于换了个账号（凭据和历史都在那个目录里）。 */
  it("可以给已有账号改名", async () => {
    twoProfiles();
    api.renameProfile.mockResolvedValue(undefined);
    render(<AccountSection />);

    fireEvent.click(await screen.findByTestId("profile-menu-claude-work"));
    fireEvent.click(screen.getByTestId("profile-menu-claude-work-rename"));
    const input = await screen.findByTestId("profile-rename-input-claude-work");
    fireEvent.change(input, { target: { value: "个人" } });
    fireEvent.keyDown(input, { key: "Enter" });

    await waitFor(() => expect(api.renameProfile).toHaveBeenCalledWith("claude", "work", "个人"));
  });

  it("改名可以取消——不调后端", async () => {
    twoProfiles();
    render(<AccountSection />);

    fireEvent.click(await screen.findByTestId("profile-menu-claude-work"));
    fireEvent.click(screen.getByTestId("profile-menu-claude-work-rename"));
    const input = await screen.findByTestId("profile-rename-input-claude-work");
    fireEvent.change(input, { target: { value: "别的名字" } });
    // mouseDown 而非 click：要抢在 input 的 onBlur（会提交）之前。
    fireEvent.mouseDown(screen.getByTestId("profile-rename-cancel-claude-work"));

    await waitFor(() => expect(screen.queryByTestId("profile-rename-input-claude-work")).toBeNull());
    expect(api.renameProfile).not.toHaveBeenCalled();
  });

  /** 点了「添加账号」之后得能反悔——只给 Esc 是让人去猜。 */
  it("添加账号可以取消", async () => {
    render(<AccountSection />);

    fireEvent.click(await screen.findByTestId("profile-add-claude"));
    const input = screen.getByPlaceholderText(zh.account.newProfileName);
    fireEvent.change(input, { target: { value: "半途而废" } });

    fireEvent.click(screen.getByTestId("profile-add-cancel-claude"));

    await waitFor(() => expect(screen.getByTestId("profile-add-claude")).toBeTruthy());
    expect(api.createProfile).not.toHaveBeenCalled();
  });
});

describe("AccountSection 合并进默认账号", () => {
  const twoProfiles = () =>
    api.listProfiles.mockResolvedValue([
      { id: null, name: "", active: false, account: { email: "a@b.c" } },
      { id: "work", name: "工作", active: true, account: { email: "w@b.c" } },
    ]);

  /**
   * 合并是「数据已拆散」的自救出口：会话与历史并入默认账号，账号本身消失。不可逆，
   * 故与删除同规——必须弹确认（appConfirm，不是 window.confirm），确认框里写明不可逆。
   */
  it("确认后调 mergeProfileIntoDefault，确认框写明不可逆", async () => {
    twoProfiles();
    api.mergeProfileIntoDefault.mockResolvedValue(undefined);
    render(<AccountSection />);

    fireEvent.click(await screen.findByTestId("profile-menu-claude-work"));
    fireEvent.click(screen.getByTestId("profile-menu-claude-work-merge"));
    await waitFor(() => expect(dialog.confirm).toHaveBeenCalled());
    // 确认文案必须讲清后果：数据并入默认账号 + 不可逆。
    expect(dialog.confirm.mock.calls[0][0]).toContain("工作");
    expect(dialog.confirm.mock.calls[0][0]).toContain("不可逆");
    await waitFor(() =>
      expect(api.mergeProfileIntoDefault).toHaveBeenCalledWith("claude", "work")
    );
  });

  it("确认框点取消 → 不合并", async () => {
    twoProfiles();
    dialog.confirm.mockResolvedValue(false);
    render(<AccountSection />);

    fireEvent.click(await screen.findByTestId("profile-menu-claude-work"));
    fireEvent.click(screen.getByTestId("profile-menu-claude-work-merge"));
    await waitFor(() => expect(dialog.confirm).toHaveBeenCalled());
    expect(api.mergeProfileIntoDefault).not.toHaveBeenCalled();
  });

  /** 默认账号没有「合并进自己」——它是合并的终点，菜单里不该出现这一项。 */
  it("默认账号的操作菜单没有合并项", async () => {
    twoProfiles();
    render(<AccountSection />);

    fireEvent.click(await screen.findByTestId("profile-menu-claude-__default__"));
    expect(screen.queryByTestId("profile-menu-claude-__default__-merge")).toBeNull();
    // 删除项同样不给（既有纪律），合并项与它并列管理。
    expect(screen.queryByTestId("profile-menu-claude-__default__-delete")).toBeNull();
  });

  /** 后端拒绝（如有进行中的会话）时错误就地展示，不静默。 */
  it("合并失败（后端拒绝）时错误就地展示", async () => {
    twoProfiles();
    api.mergeProfileIntoDefault.mockRejectedValue("该账号还有 2 个进行中的会话，请先结束这些会话再合并");
    render(<AccountSection />);

    fireEvent.click(await screen.findByTestId("profile-menu-claude-work"));
    fireEvent.click(screen.getByTestId("profile-menu-claude-work-merge"));
    await waitFor(() => expect(api.mergeProfileIntoDefault).toHaveBeenCalled());
    await screen.findByText(/进行中的会话/);
  });
});

describe("AccountSection 退出登录 vs 删除账号", () => {
  const twoProfiles = () =>
    api.listProfiles.mockResolvedValue([
      { id: null, name: "", active: false, account: { email: "a@b.c" } }, // 默认账号，已登录
      { id: "work", name: "工作", active: true, account: { email: "w@b.c" } },
    ]);

  /**
   * **登出必须带上那一行自己的 id。**
   *
   * 回归：`logout_agent` 原本写死默认账号（`install_for`），且跑 `claude auth logout` 时不注入
   * `CLAUDE_CONFIG_DIR`——于是你切到另一个账号后点「退出登录」，被清掉的是**默认账号**的凭据，
   * 而你想登出的那个原封不动。删凭据不可逆，这种错尤其伤。
   */
  it("登出某个账号时把它的 id 传下去", async () => {
    twoProfiles();
    api.logoutAgent.mockResolvedValue(undefined);
    render(<AccountSection />);

    fireEvent.click(await screen.findByTestId("profile-menu-claude-work"));
    fireEvent.click(screen.getByTestId("profile-menu-claude-work-logout"));
    await waitFor(() => expect(dialog.confirm).toHaveBeenCalled());
    await waitFor(() => expect(api.logoutAgent).toHaveBeenCalledWith("claude", "work"));
  });

  /**
   * **默认账号只能登出，不能删除**——它是 agent 自己的目录（`~/.claude`），不归 meowo 管。
   * 这正是「有了删除账号就不再需要退出登录」不成立的地方。
   */
  /**
   * **默认账号能登出、能改名，但不能删除。**
   *
   * 删不得是因为它就是 agent 自己的目录（`~/.claude`）——删它等于抹掉用户的凭据、配置和**全部
   * 会话历史**，那不是 meowo 该替他做的决定（自定义账号删的是 `~/.meowo/profiles/…`，那才是
   * 我们自己造的目录）。
   *
   * 但**改名没有理由不给**：名字只是个显示串，不碰任何文件。
   */
  it("默认账号可登出、可改名，但不可删除", async () => {
    twoProfiles();
    render(<AccountSection />);

    fireEvent.click(await screen.findByTestId("profile-menu-claude-__default__"));
    expect(screen.getByTestId("profile-menu-claude-__default__-logout")).toBeTruthy();
    expect(screen.getByTestId("profile-menu-claude-__default__-rename")).toBeTruthy();
    expect(screen.queryByTestId("profile-menu-claude-__default__-delete")).toBeNull();
  });

  /** 默认账号的名字单独存（它不在 settings.profiles 里），故 id 传 null。 */
  it("给默认账号改名时 id 传 null", async () => {
    twoProfiles();
    api.renameProfile.mockResolvedValue(undefined);
    render(<AccountSection />);

    fireEvent.click(await screen.findByTestId("profile-menu-claude-__default__"));
    fireEvent.click(screen.getByTestId("profile-menu-claude-__default__-rename"));
    const input = await screen.findByTestId("profile-rename-input-claude-__default__");
    fireEvent.change(input, { target: { value: "个人号" } });
    fireEvent.keyDown(input, { key: "Enter" });

    await waitFor(() => expect(api.renameProfile).toHaveBeenCalledWith("claude", null, "个人号"));
  });
});

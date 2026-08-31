import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor, cleanup, act, within } from "@testing-library/react";

// vi.mock 会被提升到文件顶部，工厂函数里引用的外部变量必须走 vi.hoisted
// （否则 TDZ：ReferenceError: Cannot access 'api' before initialization）。
const api = vi.hoisted(() => ({
  newSession: vi.fn(),
  recentCwds: vi.fn(),
  checkProviderHooks: vi.fn(),
  availableTerminals: vi.fn(),
  getSettings: vi.fn(),
  listAgents: vi.fn(),
  getAccounts: vi.fn(),
  loginAgent: vi.fn(),
  cancelLogin: vi.fn(),
  listSubdirectories: vi.fn(),
}));
vi.mock("../api", async (orig) => ({ ...(await orig<typeof import("../api")>()), ...api }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
const closeMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ close: closeMock }) }));
// 收集 login-done 回调，测试里手动广播（模拟 Tauri emit）。其余事件（ns-prefill）照常返回 unlisten。
const ev = vi.hoisted(() => ({ loginCbs: [] as Array<(e: unknown) => void> }));
const emitMock = vi.hoisted(() => vi.fn((_event: string, _payload?: unknown) => Promise.resolve()));
vi.mock("@tauri-apps/api/event", () => ({
  listen: (event: string, cb: (e: unknown) => void) => {
    if (event === "login-done") ev.loginCbs.push(cb);
    return Promise.resolve(() => {});
  },
  emit: emitMock,
}));
const fireLogin = (provider: string, outcome: "success" | "cancelled" | "timeout") => {
  const call = [...api.loginAgent.mock.calls].reverse().find(([p]) => p === provider);
  const operationId = call?.[3] ?? `unrelated-${provider}`;
  act(() => ev.loginCbs.forEach((cb) => cb({ payload: { provider, operationId, outcome } })));
};

import { NewSessionPanel } from "./NewSessionPanel";
import { descriptors } from "../test/agents";
import { zh } from "../i18n/zh";

/**
 * 当前是否处于「等待登录」态。
 *
 * 判据是按钮文案而非 `disabled`：等待中按钮不再禁用，而是变成「取消等待」——终端可能已被
 * 关掉（手动关、崩溃、agent 自己退出），而后端只轮询账号文件，要 5 分钟才超时。
 */
const waiting = () => screen.getByTestId("ns-login").textContent === zh.newSession.cancelLogin;

beforeEach(() => {
  Object.values(api).forEach((m) => m.mockReset());
  closeMock.mockReset();
  emitMock.mockClear();
  ev.loginCbs.length = 0;
  api.recentCwds.mockResolvedValue([]);
  api.checkProviderHooks.mockResolvedValue("installed");
  api.availableTerminals.mockResolvedValue(["wt"]);
  api.getSettings.mockResolvedValue({ default_agent: "claude", resume_terminal: "wt" });
  api.listAgents.mockResolvedValue(descriptors(["claude", "codex", "kimi"]));
  // 默认三家都已登录 → 不显示登录提示（各测试按需覆盖）。
  api.getAccounts.mockResolvedValue([
    { provider: "claude", account: { email: "a@b.c" }, usage: null, usage_supported: true },
    { provider: "codex", account: { login_label: "API Key" }, usage: null, usage_supported: false },
    { provider: "kimi", account: { email: "k@b.c" }, usage: null, usage_supported: true },
  ]);
});
afterEach(() => cleanup());

describe("NewSessionPanel (独立窗口)", () => {
  it("目录为空时启动禁用", async () => {
    render(<NewSessionPanel />);
    const launch = await screen.findByTestId("ns-launch");
    expect((launch as HTMLButtonElement).disabled).toBe(true);
  });

  it("填目录后启动调 newSession → 关窗", async () => {
    api.newSession.mockResolvedValue(undefined);
    render(<NewSessionPanel />);
    fireEvent.change(await screen.findByTestId("ns-dir"), { target: { value: "C:/proj" } });
    fireEvent.click(screen.getByTestId("ns-launch"));
    await waitFor(() => expect(api.newSession).toHaveBeenCalledWith("C:/proj", "claude", {}, []));
    await waitFor(() => expect(closeMock).toHaveBeenCalled());
  });

  it("同一事件批次重复点击只启动一个终端", async () => {
    let finish!: () => void;
    api.newSession.mockImplementation(() => new Promise<void>((resolve) => { finish = resolve; }));
    render(<NewSessionPanel />);
    fireEvent.change(await screen.findByTestId("ns-dir"), { target: { value: "C:/proj" } });
    const launch = screen.getByTestId("ns-launch");

    act(() => {
      launch.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      launch.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(api.newSession).toHaveBeenCalledTimes(1);
    finish();
    await waitFor(() => expect(closeMock).toHaveBeenCalled());
  });

  it("hooks 未装显示警告", async () => {
    api.checkProviderHooks.mockResolvedValue("missing");
    render(<NewSessionPanel />);
    expect(await screen.findByTestId("ns-hooks-warn")).toBeTruthy();
  });

  it("非默认账号活跃时启动按钮旁提示将使用的账号；默认账号不提示", async () => {
    render(<NewSessionPanel />);
    await screen.findByTestId("ns-launch");
    // 默认 mock 无 active_profile_name（默认账号）→ 什么都不显示（零感知底线）。
    expect(screen.queryByTestId("ns-active-profile")).toBeNull();

    cleanup();
    api.getAccounts.mockResolvedValue([
      { provider: "claude", account: { email: "a@b.c" }, usage: null, usage_supported: true, active_profile_name: "工作" },
    ]);
    render(<NewSessionPanel />);
    const hint = await screen.findByTestId("ns-active-profile");
    expect(hint.textContent).toBe(zh.newSession.activeProfile("工作"));
  });

  it("启动失败显示错误，不关窗", async () => {
    api.newSession.mockRejectedValue("启动终端失败");
    render(<NewSessionPanel />);
    fireEvent.change(await screen.findByTestId("ns-dir"), { target: { value: "C:/proj" } });
    fireEvent.click(screen.getByTestId("ns-launch"));
    expect((await screen.findByTestId("ns-error")).textContent).toContain("启动终端失败");
    expect(closeMock).not.toHaveBeenCalled();
  });

  it("目录输入框 Enter 直接启动，但 IME 合成中的 Enter 不启动（拼音选词误触）", async () => {
    api.newSession.mockResolvedValue(undefined);
    render(<NewSessionPanel />);
    const input = await screen.findByTestId("ns-dir");
    fireEvent.change(input, { target: { value: "C:/proj" } });
    // 输入法按 Enter 提交候选时先触发 keydown，放行会用半截路径误启动。
    fireEvent.keyDown(input, { key: "Enter", isComposing: true });
    expect(api.newSession).not.toHaveBeenCalled();
    fireEvent.keyDown(input, { key: "Enter", isComposing: false });
    await waitFor(() => expect(api.newSession).toHaveBeenCalledWith("C:/proj", "claude", {}, []));
  });

  it("启动失败后编辑目录/换 agent 清掉旧错误：错误不滞留到下一次动作", async () => {
    api.newSession.mockRejectedValue("启动终端失败");
    render(<NewSessionPanel />);
    const input = await screen.findByTestId("ns-dir");
    fireEvent.change(input, { target: { value: "C:/proj" } });
    fireEvent.click(screen.getByTestId("ns-launch"));
    expect((await screen.findByTestId("ns-error")).textContent).toContain("启动终端失败");
    // 编辑目录：旧错误随输入失效。
    fireEvent.change(input, { target: { value: "C:/proj2" } });
    expect(screen.queryByTestId("ns-error")).toBeNull();
    // 再失败一次，换 agent 同样清（不同 agent 的错误对新选择不成立）。
    fireEvent.click(screen.getByTestId("ns-launch"));
    expect((await screen.findByTestId("ns-error")).textContent).toContain("启动终端失败");
    fireEvent.click(await screen.findByTestId("ns-agent-codex"));
    expect(screen.queryByTestId("ns-error")).toBeNull();
  });

  it("agent 选择只列已装的", async () => {
    api.listAgents.mockResolvedValue(descriptors(["claude", "codex"]));
    render(<NewSessionPanel />);
    await screen.findByTestId("ns-launch");
    expect(screen.queryByTestId("ns-agent-claude")).toBeTruthy();
    expect(screen.queryByTestId("ns-agent-codex")).toBeTruthy();
    expect(screen.queryByTestId("ns-agent-kimi")).toBeNull();
  });

  it("启动选项由插件声明：选择随 newSession 回传，换 agent 清空且未声明者无此栏", async () => {
    api.newSession.mockResolvedValue(undefined);
    render(<NewSessionPanel />);
    // claude 声明了模型 + 权限两栏（表来自 descriptor.launch_options，前端零硬编码）。
    // 自绘 Dropdown：点开再点选项（"Opus" 是产品词回退，"计划模式" 来自 i18n）。
    fireEvent.click(within(await screen.findByTestId("ns-option-model")).getByRole("button"));
    fireEvent.click(screen.getByRole("option", { name: "Opus" }));
    fireEvent.click(within(screen.getByTestId("ns-option-permission")).getByRole("button"));
    fireEvent.click(screen.getByRole("option", { name: "计划模式" }));
    fireEvent.change(screen.getByTestId("ns-dir"), { target: { value: "C:/proj" } });
    fireEvent.click(screen.getByTestId("ns-launch"));
    // 只回传 choice id；翻译成 CLI flag 是后端按同一张声明表做的事。
    await waitFor(() => expect(api.newSession).toHaveBeenCalledWith(
      "C:/proj", "claude", { model: "opus", permission: "plan" }, [],
    ));
    // kimi 未声明启动选项 → 整块不渲染，也不会把 claude 的残留选择带过去。
    fireEvent.click(screen.getByTestId("ns-agent-kimi"));
    await waitFor(() => expect(screen.queryByTestId("ns-options")).toBeNull());
  });

  it("高风险启动档（契约 risk=true）渲染警示色与风险副标题", async () => {
    render(<NewSessionPanel />);
    fireEvent.click(within(await screen.findByTestId("ns-option-permission")).getByRole("button"));
    const risky = screen.getByRole("option", { name: new RegExp(zh.newSession.launchChoice["permission.bypassPermissions"]) });
    expect(risky.className).toContain("risk");
    expect(within(risky).getByText(zh.newSession.riskyChoiceSub)).toBeTruthy();
    // 普通档不带警示样式与副标题。
    const plain = screen.getByRole("option", { name: zh.newSession.launchChoice["permission.plan"] });
    expect(plain.className).not.toContain("risk");
    expect(within(plain).queryByText(zh.newSession.riskyChoiceSub)).toBeNull();
  });

  it("relay 启用时 model 档置灰并注明「由中转固定为 X」，其余档不受影响", async () => {
    api.getAccounts.mockResolvedValue([
      { provider: "claude", account: { email: "a@b.c" }, usage: null, usage_supported: true, relay_enabled: true },
    ]);
    api.getSettings.mockResolvedValue({
      default_agent: "claude",
      resume_terminal: "wt",
      relay: { per_agent: { claude: { enabled: true, base_url: "https://r.example/v1", model: "claude-sonnet-5", protocol: "", auth: "bearer" } } },
    });
    render(<NewSessionPanel />);
    const modelOption = await screen.findByTestId("ns-option-model");
    const note = await within(modelOption).findByTestId("ns-option-pinned");
    expect(note.textContent).toBe(zh.newSession.relayPinnedModel("claude-sonnet-5"));
    expect((within(modelOption).getByRole("button") as HTMLButtonElement).disabled).toBe(true);
    // 权限档不被 relay 置灰。
    expect((within(screen.getByTestId("ns-option-permission")).getByRole("button") as HTMLButtonElement).disabled).toBe(false);
  });

  it("启动成功后面板自关；「正在启动」反馈由看板占位卡承担，不再 emit session-starting", async () => {
    api.newSession.mockResolvedValue(undefined);
    render(<NewSessionPanel />);
    fireEvent.change(await screen.findByTestId("ns-dir"), { target: { value: "C:/proj" } });
    fireEvent.click(screen.getByTestId("ns-launch"));
    await waitFor(() => expect(closeMock).toHaveBeenCalled());
    // 占位卡（后端 pending PTY 合成负 id 占位项）取代了 toast 回执——这条事件已没有听众。
    expect(emitMock).not.toHaveBeenCalledWith("session-starting", expect.anything());
  });

  /// 跨仓同一需求 = 一个会话 + 附加目录:Ctrl+点击最近项附加,启动把附加目录交给后端
  /// 拼 --add-dir;主目录本身不重复附加。
  it("Ctrl+点击最近项加为附加目录,启动带 extraDirs", async () => {
    api.newSession.mockResolvedValue(undefined);
    // 清掉前序用例经 saveStoredOpts 记住的启动选项,免得 opts 断言串场。
    localStorage.removeItem("meowo-launch-selections");
    // 盘符路径经 normalizePath 统一成反斜杠,断言按归一后的形态。
    api.recentCwds.mockResolvedValue(["C:/w/web", "C:/w/api", "C:/w/proto"]);
    render(<NewSessionPanel />);
    // claude 支持附加目录 → 入口可见。
    await screen.findByTestId("ns-extra-add");
    fireEvent.click(await screen.findByText("C:\\w\\api"), { ctrlKey: true });
    fireEvent.click(screen.getByText("C:\\w\\proto"), { ctrlKey: true });
    expect(screen.getByTestId("ns-extra-list").textContent).toContain("api");
    // 主目录默认预填首个最近项(C:\w\web),Ctrl+点它不该重复附加。
    fireEvent.click(screen.getByText("C:\\w\\web"), { ctrlKey: true });
    fireEvent.click(screen.getByTestId("ns-launch"));
    await waitFor(() => expect(api.newSession).toHaveBeenCalledWith(
      "C:\\w\\web", "claude", {}, ["C:\\w\\api", "C:\\w\\proto"],
    ));
  });

  it("不支持附加目录的 agent 不显示入口,切换过去时清空已选", async () => {
    api.recentCwds.mockResolvedValue(["C:/w/web", "C:/w/api"]);
    render(<NewSessionPanel />);
    await screen.findByTestId("ns-extra-add");
    fireEvent.click(await screen.findByText("C:\\w\\api"), { ctrlKey: true });
    expect(screen.getByTestId("ns-extra-list")).toBeTruthy();
    // kimi 未声明附加目录 flag → 入口消失、已选清空(静默带过去会被后端如实拒绝)。
    fireEvent.click(screen.getByTestId("ns-agent-kimi"));
    await waitFor(() => expect(screen.queryByTestId("ns-extra-add")).toBeNull());
    expect(screen.queryByTestId("ns-extra-list")).toBeNull();
  });

  it("一个都没装时提示 + 启动禁用", async () => {
    api.listAgents.mockResolvedValue(descriptors([]));
    render(<NewSessionPanel />);
    expect(await screen.findByTestId("ns-no-agents")).toBeTruthy();
    expect((screen.getByTestId("ns-launch") as HTMLButtonElement).disabled).toBe(true);
  });

  it("输入斜杠与最近项反斜杠方向不同时仍能高亮匹配", async () => {
    api.recentCwds.mockResolvedValue(["C:\\Users\\larry\\proj"]);
    const { container } = render(<NewSessionPanel />);
    fireEvent.change(await screen.findByTestId("ns-dir"), { target: { value: "C:/Users/larry/proj" } });
    await waitFor(() =>
      expect(container.querySelector(".ns-recent-item.is-on")?.textContent).toContain("proj")
    );
  });

  it("同一目录因斜杠方向不同重复时只保留一条", async () => {
    api.recentCwds.mockResolvedValue([
      "C:/Users/larry/proj",
      "C:\\Users\\larry\\proj",
      "C:\\Users\\larry\\other",
    ]);
    const { container } = render(<NewSessionPanel />);
    await waitFor(() =>
      expect(container.querySelectorAll(".ns-recent-item").length).toBe(2)
    );
  });
});

describe("NewSessionPanel 登录", () => {
  /** 让当前选中的 claude 处于未登录（account 为 null）。 */
  const claudeSignedOut = () =>
    api.getAccounts.mockResolvedValue([
      { provider: "claude", account: null, usage: null, usage_supported: true },
      { provider: "codex", account: { login_label: "API Key" }, usage: null, usage_supported: false },
      { provider: "kimi", account: { email: "k@b.c" }, usage: null, usage_supported: true },
    ]);

  it("未登录时提示并给出登录按钮", async () => {
    claudeSignedOut();
    render(<NewSessionPanel />);
    expect(await screen.findByTestId("ns-login-warn")).toBeTruthy();
    expect(screen.getByTestId("ns-login")).toBeTruthy();
  });

  it("已登录时不提示", async () => {
    render(<NewSessionPanel />); // beforeEach 里三家都已登录
    await screen.findByTestId("ns-agent-claude");
    expect(screen.queryByTestId("ns-login-warn")).toBeNull();
  });

  it("拿不到账号时不提示（宁可不打扰，也不误报未登录）", async () => {
    api.getAccounts.mockRejectedValue(new Error("boom"));
    render(<NewSessionPanel />);
    await screen.findByTestId("ns-agent-claude");
    expect(screen.queryByTestId("ns-login-warn")).toBeNull();
  });

  /**
   * 「无账号能力」≠「未登录」。
   *
   * 回归：`getAccounts()` 只返回**声明了账号能力**的 agent（后端遍历 all_with_account）。曾把
   * 「查不到行」当成「未登录」，于是给没有账号能力的 agent 也亮出登录按钮——点下去后端
   * `login_argv()` 是 None，只能报「拉起登录失败」。给出入口却走不通，比没有入口更糟。
   *
   * 两者必须分开：真有账号能力却没登录的（kimi），照旧给入口。当前五家都有账号能力，故这里
   * 手造一个没有的。
   */
  it("无账号能力的 agent 不给登录入口；真未登录的照给", async () => {
    api.listAgents.mockResolvedValue([
      { id: "claude", display_name: "Claude Code", installed: true, supports_proxy: true, supports_account: true, supports_profiles: true },
      { id: "kimi", display_name: "Kimi Code", installed: true, supports_proxy: true, supports_account: true, supports_profiles: true },
      { id: "noacct", display_name: "No Account", installed: true, supports_proxy: false, supports_account: false, supports_profiles: false },
    ]);
    api.getAccounts.mockResolvedValue([
      { provider: "claude", account: { email: "a@b.c" }, usage: null, usage_supported: true },
      // 有账号能力，但确实没登录 → 该给入口。
      { provider: "kimi", account: null, usage: null, usage_supported: true },
      // noacct 不在返回里——它没声明账号能力，后端根本不会列它。
    ]);
    render(<NewSessionPanel />);

    // kimi：真未登录 → 亮出入口。
    fireEvent.click(await screen.findByTestId("ns-agent-kimi"));
    await waitFor(() => expect(screen.queryByTestId("ns-login")).toBeTruthy());

    // noacct：无账号概念 → 不该亮（曾经会亮，且点了必失败）。
    fireEvent.click(screen.getByTestId("ns-agent-noacct"));
    await waitFor(() => expect(screen.queryByTestId("ns-login-warn")).toBeNull());
    expect(screen.queryByTestId("ns-login")).toBeNull();
  });

  it("点登录调 loginAgent 并进入等待态", async () => {
    claudeSignedOut();
    api.loginAgent.mockResolvedValue(undefined);
    render(<NewSessionPanel />);
    fireEvent.click(await screen.findByTestId("ns-login"));
    await waitFor(() => expect(api.loginAgent).toHaveBeenCalledWith("claude", undefined, undefined, expect.any(String)));
    // 等 login-done 才落回。按钮此时变成「取消等待」，而不是死掉的禁用按钮——终端可能已被关掉，
    // 而后端只轮询账号文件，要 5 分钟才超时。
    await waitFor(() => expect(waiting()).toBe(true));
  });

  it("login-done 成功 → 提示消失", async () => {
    claudeSignedOut();
    api.loginAgent.mockResolvedValue(undefined);
    render(<NewSessionPanel />);
    fireEvent.click(await screen.findByTestId("ns-login"));
    await waitFor(() => expect(api.loginAgent).toHaveBeenCalled());
    fireLogin("claude", "success");
    await waitFor(() => expect(screen.queryByTestId("ns-login-warn")).toBeNull());
  });

  /// 起因：点完登录后如果终端被关掉（用户手动关、崩溃、agent 自己退出），后端只轮询账号文件，
  /// 要 5 分钟才超时。这五分钟里按钮一直是「等待登录…」且不可点——用户既不能重来也不知道发生了什么。
  it("等待中点按钮 → 调 cancel_login，落回可点并提示已取消（而非「未检测到登录完成」）", async () => {
    claudeSignedOut();
    api.loginAgent.mockResolvedValue(undefined);
    api.cancelLogin.mockResolvedValue(undefined);
    render(<NewSessionPanel />);
    fireEvent.click(await screen.findByTestId("ns-login"));
    await waitFor(() => expect(waiting()).toBe(true));

    fireEvent.click(screen.getByTestId("ns-login"));
    await waitFor(() => expect(api.cancelLogin).toHaveBeenCalledWith("claude", expect.any(String)));

    // 收尾由后端 emit login-done（它会再查一次账号；这里模拟「确实没登上」）。
    fireLogin("claude", "cancelled");
    await waitFor(() => expect(waiting()).toBe(false));
    // 取消 ≠ 超时：文案必须区分，否则用户以为是没检测到。
    expect(screen.getByTestId("ns-error").textContent).toBe(zh.newSession.loginCancelled);
  });

  /// 取消时后端会再查一次账号——用户可能已经在终端里登完了，只是嫌等得慢。此时应转「已登录」。
  it("取消时若其实已登录成功 → 未登录提示消失，不显示取消提示", async () => {
    claudeSignedOut();
    api.loginAgent.mockResolvedValue(undefined);
    api.cancelLogin.mockResolvedValue(undefined);
    render(<NewSessionPanel />);
    fireEvent.click(await screen.findByTestId("ns-login"));
    await waitFor(() => expect(waiting()).toBe(true));
    fireEvent.click(screen.getByTestId("ns-login"));
    await waitFor(() => expect(api.cancelLogin).toHaveBeenCalled());

    fireLogin("claude", "success"); // 后端复查发现真登上了
    await waitFor(() => expect(screen.queryByTestId("ns-login-warn")).toBeNull());
    expect(screen.queryByTestId("ns-error")).toBeNull();
  });

  it("login-done 超时 → 落回可点 + 显示提示，提示仍在", async () => {
    claudeSignedOut();
    api.loginAgent.mockResolvedValue(undefined);
    render(<NewSessionPanel />);
    fireEvent.click(await screen.findByTestId("ns-login"));
    await waitFor(() => expect(waiting()).toBe(true));
    fireLogin("claude", "timeout");
    await waitFor(() => expect(waiting()).toBe(false));
    // 超时不等于登录失败，未登录提示仍在，错误行给出本地化说明。
    expect(screen.getByTestId("ns-login-warn")).toBeTruthy();
    expect(screen.getByTestId("ns-error").textContent?.trim().length).toBeGreaterThan(0);
  });

  it("别的 provider 的 login-done 不影响当前选中项", async () => {
    claudeSignedOut();
    api.loginAgent.mockResolvedValue(undefined);
    render(<NewSessionPanel />);
    fireEvent.click(await screen.findByTestId("ns-login"));
    await waitFor(() => expect(waiting()).toBe(true));
    fireLogin("kimi", "success"); // 与当前选中的 claude 无关
    expect(waiting()).toBe(true);
    expect(screen.getByTestId("ns-login-warn")).toBeTruthy();
  });

  /** 两家都未登录，用于「登录中切走」的回归。 */
  const bothSignedOut = () =>
    api.getAccounts.mockResolvedValue([
      { provider: "claude", account: null, usage: null, usage_supported: true },
      { provider: "codex", account: { login_label: "API Key" }, usage: null, usage_supported: false },
      { provider: "kimi", account: null, usage: null, usage_supported: true },
    ]);

  it("登录中切到别的 agent：新 agent 的登录按钮可点，且能真的发起登录", async () => {
    // 回归：等待态曾是全局 boolean，切走后 claude 的 login-done 被按当前选中项过滤掉、
    // 等待态永远落不回 → kimi 的按钮虽显示可点，doLogin 却被 `if (loginBusy) return` 静默挡住。
    bothSignedOut();
    api.loginAgent.mockResolvedValue(undefined);
    render(<NewSessionPanel />);
    fireEvent.click(await screen.findByTestId("ns-login")); // 发起 claude 登录
    await waitFor(() => expect(api.loginAgent).toHaveBeenCalledWith("claude", undefined, undefined, expect.any(String)));
    await waitFor(() => expect(waiting()).toBe(true));

    fireEvent.click(screen.getByTestId("ns-agent-kimi")); // claude 还在登录中就切走
    // kimi 未登录且不在等待态 → 按钮可点
    await waitFor(() => expect(waiting()).toBe(false));
    fireEvent.click(screen.getByTestId("ns-login"));
    await waitFor(() => expect(api.loginAgent).toHaveBeenCalledWith("kimi", undefined, undefined, expect.any(String))); // 并发登录不被挡
    await waitFor(() => expect(waiting()).toBe(true));
  });

  it("登录中切走后，原 agent 的 login-done 仍能清掉它的等待态", async () => {
    bothSignedOut();
    api.loginAgent.mockResolvedValue(undefined);
    render(<NewSessionPanel />);
    fireEvent.click(await screen.findByTestId("ns-login")); // claude 登录中
    await waitFor(() => expect(waiting()).toBe(true));

    fireEvent.click(screen.getByTestId("ns-agent-kimi")); // 切走
    fireLogin("claude", "timeout"); // claude 登录超时（此时选中的是 kimi）
    fireEvent.click(screen.getByTestId("ns-agent-claude")); // 切回来

    // 等待态已被清掉 → 可以重试登录（旧实现在此永久禁用）
    await waitFor(() => expect(waiting()).toBe(false));
  });

  it("切走期间到达的成功事件，切回后应显示为已登录", async () => {
    bothSignedOut();
    api.loginAgent.mockResolvedValue(undefined);
    render(<NewSessionPanel />);
    fireEvent.click(await screen.findByTestId("ns-login"));
    fireEvent.click(await screen.findByTestId("ns-agent-kimi")); // 切走
    fireLogin("claude", "success"); // 登录成功是客观事实，与当前选中谁无关
    fireEvent.click(screen.getByTestId("ns-agent-claude")); // 切回来
    await waitFor(() => expect(screen.queryByTestId("ns-login-warn")).toBeNull());
  });
});

/// 远程模式的页内目录浏览:系统目录对话框在浏览器里弹不起来,「浏览…」改为就地
/// 列目录逐级下钻,选定后回填输入框。
describe("远程目录浏览", () => {
  afterEach(() => {
    (globalThis as Record<string, unknown>).__MEOWO_REMOTE__ = undefined;
  });

  it("点「浏览…」出磁盘列表,下钻两级后选定,路径回填输入框", async () => {
    (globalThis as Record<string, unknown>).__MEOWO_REMOTE__ = true;
    api.listSubdirectories
      .mockResolvedValueOnce({ path: "", parent: null, dirs: [{ name: "C:\\", path: "C:\\" }] })
      .mockResolvedValueOnce({ path: "C:\\", parent: "", dirs: [{ name: "repo", path: "C:\\repo" }] })
      .mockResolvedValueOnce({ path: "C:\\repo", parent: "C:\\", dirs: [] });
    render(<NewSessionPanel />);

    fireEvent.click(await screen.findByText(zh.newSession.browse));
    const browser = await screen.findByTestId("ns-dirbrowse");
    fireEvent.click(within(browser).getByText("C:\\"));
    fireEvent.click(await within(browser).findByText("repo"));
    await within(browser).findByText(zh.newSession.noSubdirs);

    fireEvent.click(within(browser).getByText(zh.newSession.pickHere));
    expect(screen.queryByTestId("ns-dirbrowse")).toBeNull();
    expect((screen.getByTestId("ns-dir") as HTMLInputElement).value).toBe("C:\\repo");
  });

  it("两级列目录都失败时报错可见（此前纯静默，点了「浏览」像卡死）", async () => {
    (globalThis as Record<string, unknown>).__MEOWO_REMOTE__ = true;
    api.listSubdirectories.mockRejectedValue("权限拒绝");
    render(<NewSessionPanel />);
    fireEvent.click(await screen.findByText(zh.newSession.browse));
    // browse 面板没打开,browseError 的展示位不渲染——借主错误行说出来。
    expect((await screen.findByTestId("ns-error")).textContent).toContain("权限拒绝");
  });
});

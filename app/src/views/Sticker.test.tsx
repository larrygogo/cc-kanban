import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { render, screen, cleanup, fireEvent, waitFor, act } from "@testing-library/react";

const events = vi.hoisted(() => ({
  settingsChanged: null as null | ((event: { payload: any }) => void),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: (name: string, cb: (event: { payload: any }) => void) => {
    if (name === "settings-changed") events.settingsChanged = cb;
    return Promise.resolve(() => {});
  },
}));

const invokeMock = vi.hoisted(() =>
  vi.fn((cmd: string, _args?: unknown) => {
    if (cmd === "get_settings") {
      return Promise.resolve({
        notifications_enabled: true,
        theme: "dark",
        opacity: 94,
        ui_scale: 100,
        resume_terminal: "wt",
        language: "auto",
        terminal_open_mode: "card",
        card_menu_mode: "context",
        preview_enabled: true,
        sticker_style: "elevated",
        sticker_color: "classic",
        sticker_quota_providers: ["claude"],
        default_agent: "claude",
      });
    }
    // agent 名单与展示名由后端下发（前端不再自带一份）。
    if (cmd === "list_agents") {
      return Promise.resolve([
        { id: "claude", display_name: "Claude Code", installed: true, supports_proxy: true },
        { id: "kimi", display_name: "Kimi Code", installed: false, supports_proxy: true },
        { id: "codex", display_name: "Codex", installed: false, supports_proxy: true },
        { id: "gemini", display_name: "Gemini CLI", installed: false, supports_proxy: true },
        { id: "opencode", display_name: "OpenCode", installed: false, supports_proxy: true },
      ]);
    }
    return Promise.resolve();
  })
);
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: unknown) => invokeMock(cmd, args),
}));

import { relayEnabledSignature, Sticker } from "./Sticker";
import { match } from "./sticker/helpers";
import { EmptyState } from "./sticker/EmptyState";
import { UsageScreen } from "./sticker/UsageScreen";
import type { LiveSession, ProviderUsage } from "../api";
import { zh } from "../i18n/zh";

// jsdom 没有真实视口尺寸，@tanstack/react-virtual 会以为 .stk-scroll 高度为 0 而不渲染卡片。
// mock 一个足够大的滚动容器，让测试里的卡片进入可视区并被挂载。
const defaultRect: DOMRect = {
  top: 0, left: 0, bottom: 0, right: 0, width: 0, height: 0, x: 0, y: 0,
  toJSON: () => ({ top: 0, left: 0, bottom: 0, right: 0, width: 0, height: 0, x: 0, y: 0 }),
};
vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement): DOMRect {
  if (this.classList.contains("stk-scroll")) {
    return {
      ...defaultRect,
      bottom: 600, right: 400, width: 400, height: 600,
      toJSON: () => ({ ...defaultRect, bottom: 600, right: 400, width: 400, height: 600 }),
    };
  }
  if (this.classList.contains("stk-vitem")) {
    return {
      ...defaultRect,
      right: 400, width: 400, height: 82,
      toJSON: () => ({ ...defaultRect, right: 400, width: 400, height: 82 }),
    };
  }
  return defaultRect;
});
// 用同步触发 rect 的 ResizeObserver 替换原生实现，确保虚拟列表在测试查询前已完成尺寸计算。
const mockScrollRect = { top: 0, left: 0, bottom: 600, right: 400, width: 400, height: 600, x: 0, y: 0 };
const mockItemRect = { top: 0, left: 0, bottom: 82, right: 400, width: 400, height: 82, x: 0, y: 0 };
class MockResizeObserver {
  constructor(private cb: ResizeObserverCallback) {}
  observe(target: Element) {
    const isScroll = target.classList.contains("stk-scroll");
    const rect = isScroll ? mockScrollRect : mockItemRect;
    this.cb([{
      target,
      contentRect: rect as unknown as DOMRectReadOnly,
      borderBoxSize: [{ inlineSize: rect.width, blockSize: rect.height } as unknown as ResizeObserverSize],
      contentBoxSize: [{ inlineSize: rect.width, blockSize: rect.height } as unknown as ResizeObserverSize],
      devicePixelContentBoxSize: [],
    } as ResizeObserverEntry], this as unknown as ResizeObserver);
  }
  unobserve() {}
  disconnect() {}
}
vi.stubGlobal("ResizeObserver", MockResizeObserver);

type Item = LiveSession & { connected: boolean };

function mk(over: Partial<Item> = {}): Item {
  return {
    session: { id: 1, project_id: 1, cc_session_id: "s", status: "running", started_at: 0, last_event_at: Date.now(), ended_at: null },
    project_name: "proj",
    task_title: "做点事",
    current_activity: "正在做点事",
    column: "doing", todo_done: 0, todo_total: 0, todos: [],
    pid: 1234, connected: true, pty_managed: false, archived: false, cwd: null, errored: false, error_label: null, error_raw: null,
    provider: "claude",
    ...over,
  } as Item;
}

afterEach(() => {
  cleanup();
  localStorage.clear(); // 防 tab/star 等持久化状态跨用例泄漏
});
beforeEach(() => {
  invokeMock.mockClear();
  events.settingsChanged = null;
});

/** 首帧占位已与生产默认(button)对齐；测试注入的 card_menu_mode=context 要等 get_settings
 *  resolve 并被 useSettingsEffect 套用后右键才生效——右键前先冲刷一次异步队列。 */
const settingsApplied = () => act(async () => {});

describe("断开会话不再催人交互", () => {
  const pending = { pending_review: "approval" } as Partial<Item>;

  it("连着时挂出交互标签", () => {
    const { container } = render(<Sticker filter="all" data={[mk({ ...pending, connected: true })]} />);
    expect(container.querySelector(".pending-pill")).not.toBeNull();
  });

  it("断开时不挂交互标签——进程都没了，「待批准」只会催用户去点一个点不动的东西", () => {
    const { container } = render(<Sticker filter="all" data={[mk({ ...pending, connected: false })]} />);
    expect(container.querySelector(".pending-pill")).toBeNull();
    // 卡片本身仍在「全部」里作为历史留存，只是不再伪装成待办。
    expect(container.querySelector(".stk-title")?.textContent).toBe("做点事");
  });

  it("断开的会话不进「待交互」/「运行中」分组", () => {
    // DB 里残留的 pending_review 曾让断开的会话漏进 waiting：后台收尾只改 status、
    // 不清 pending_review，而 waiting 的判定是 `status=waiting || pending_review != null`。
    const dead = mk({ ...pending, connected: false });
    expect(match("waiting", dead)).toBe(false);
    expect(match("running", dead)).toBe(false);
    expect(match("all", dead)).toBe(true); // 仍作为历史留在「全部」

    const deadRunning = mk({ connected: false, session: { ...mk().session, status: "running" } });
    expect(match("running", deadRunning)).toBe(false);

    // 连着的照旧各归其位。
    expect(match("waiting", mk({ ...pending, connected: true }))).toBe(true);
    expect(match("running", mk({ connected: true }))).toBe(true);
  });
});

describe("会话卡片的所属账号徽章", () => {
  it("归属自定义账号的会话显示账号徽章", () => {
    render(<Sticker filter="all" data={[mk({ profile: "work", profile_name: "工作" })]} />);
    expect(screen.getByTestId("stk-profile-badge").textContent).toBe("工作");
  });

  it("默认账号（profile_name 为 null）不显示徽章——没建过账号的用户零感知", () => {
    render(<Sticker filter="all" data={[mk({ profile: null, profile_name: null })]} />);
    expect(screen.queryByTestId("stk-profile-badge")).toBeNull();
  });
});

/// Agent 自己派出的后台会话（Claude Code 的 FleetView）：用户没开过它，看板却多出一张卡。
/// 它由 agent 的守护进程托管，Meowo 既接管不了、也没有终端窗口可切——卡片必须标明来历，
/// 并且不给那些注定失败的入口。
describe("会话卡片的后台会话标注", () => {
  it("点开后台会话不去定位终端，只说明它是什么、去哪找它", () => {
    const { container } = render(<Sticker filter="all" data={[mk({ background: true, connected: true })]} />);
    fireEvent.click(container.querySelector(".stk-card")!);
    // focus_session 会在 daemon 起的 PTY 宿主上落空（它没有终端窗口），根本不该发出去。
    expect(invokeMock).not.toHaveBeenCalledWith("focus_session", expect.anything());
    expect(screen.getByText(zh.sticker.focusBackground)).toBeTruthy();
  });

  /// 后台会话的 PTY 不在我们手上（pty_managed 恒 false），但 Agent 给了官方的结束入口：
  /// 后端连上那条旁路 socket 发 kill 控制帧。所以菜单该有「结束会话」——直接杀 pid 才是
  /// 错的（supervisor 会按 respawnFlags 把它拉回来）。
  it("后台会话给「结束会话」，走与托管会话同一条命令", async () => {
    const { container } = render(
      <Sticker filter="all" data={[mk({ background: true, pty_managed: false })]} />,
    );
    await settingsApplied();
    fireEvent.contextMenu(container.querySelector(".stk-card")!);
    const item = screen.getByText(zh.chat.endSession);
    invokeMock.mockImplementationOnce(() => Promise.resolve(true) as Promise<unknown> as Promise<void>);
    fireEvent.click(item);
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("stop_managed_terminal", { sessionId: 1 }));
  });
});

describe("Sticker", () => {
  it("中转状态切换时立即重新读取账号配额状态", async () => {
    render(<Sticker filter="all" data={[]} />);
    await waitFor(() => expect(events.settingsChanged).not.toBeNull());
    await waitFor(() => expect(invokeMock.mock.calls.filter(([cmd]) => cmd === "get_accounts").length).toBe(1));

    const officialSettings = {
      notifications_enabled: true, auto_update_enabled: true,
      theme: "dark", opacity: 94, ui_scale: 100, resume_terminal: "wt", language: "auto",
      terminal_open_mode: "card", card_menu_mode: "context", preview_enabled: true,
      sticker_style: "elevated", sticker_color: "classic", sticker_quota_providers: ["claude"],
      default_agent: "claude", proxy: { mode: "system", url: "", per_agent: {} },
      relay: { per_agent: {} },
    } as any;
    act(() => events.settingsChanged?.({ payload: officialSettings }));
    const relaySettings = {
      ...officialSettings,
      relay: { per_agent: { claude: { enabled: true, base_url: "https://relay/v1", model: "x", protocol: "", auth: "bearer" } } },
    } as any;
    expect(relayEnabledSignature(relaySettings)).toBe("claude");
    act(() => events.settingsChanged?.({ payload: relaySettings }));
    await waitFor(() => expect(invokeMock.mock.calls.filter(([cmd]) => cmd === "get_accounts").length).toBe(2));
  });

  it("底栏提供对话窗口入口，点按交给后端打开最近会话的对话窗", async () => {
    render(<Sticker filter="all" data={[]} />);
    const btn = await screen.findByRole("button", { name: zh.sticker.openChatWindow });
    fireEvent.click(btn);
    expect(invokeMock.mock.calls.some(([cmd]) => cmd === "open_latest_chat")).toBe(true);
  });

  it("对话功能关闭（轻量模式）时贴纸上不渲染任何 chat 入口", async () => {
    const { container } = render(<Sticker filter="all" data={[mk({ connected: true })]} />);
    // 默认开启：底栏按钮与卡片按钮都在。
    await screen.findByRole("button", { name: zh.sticker.openChatWindow });
    expect(container.querySelector(".stk-chat-btn")).toBeTruthy();
    await waitFor(() => expect(events.settingsChanged).not.toBeNull());
    // 完整对象：useSettingsEffect 的 apply 会读 terminal_open_mode/relay 等字段，瘦 payload 会踩空。
    act(() => events.settingsChanged?.({
      payload: {
        notifications_enabled: true, auto_update_enabled: true,
        theme: "dark", opacity: 94, ui_scale: 100, resume_terminal: "wt", language: "auto",
        terminal_open_mode: "card", card_menu_mode: "context", preview_enabled: true,
        sticker_style: "elevated", sticker_color: "classic", sticker_quota_providers: ["claude"],
        default_agent: "claude", proxy: { mode: "system", url: "", per_agent: {} },
        relay: { per_agent: {} }, chat_enabled: false,
      } as any,
    }));
    // 关闭后两个入口一起消失（settings-changed 实时生效，无需重启）。
    await waitFor(() => expect(screen.queryByRole("button", { name: zh.sticker.openChatWindow })).toBeNull());
    expect(container.querySelector(".stk-chat-btn")).toBeNull();
  });

  it("用量选择在卸载重挂后保留（记住上次选择，找不到才退第一个）", () => {
    const usageMap: Record<string, ProviderUsage> = {
      claude: { lanes: [], note: null } as ProviderUsage,
      codex: { lanes: [], note: null } as ProviderUsage,
    };
    const props = { quotaProviders: ["claude", "codex"], usageMap };
    const { unmount, container } = render(<UsageScreen {...props} />);
    const tabs = container.querySelectorAll(".stk-utab");
    expect(tabs.length).toBe(2);
    expect(tabs[0].classList.contains("on")).toBe(true); // 默认第一个 claude
    fireEvent.click(tabs[1]); // 选 codex
    expect(tabs[1].classList.contains("on")).toBe(true);
    unmount(); // 折叠 → 卸载
    const { container: c2 } = render(<UsageScreen {...props} />); // 展开 → 重挂
    const tabs2 = c2.querySelectorAll(".stk-utab");
    expect(tabs2[1].classList.contains("on")).toBe(true); // 应记住 codex
    expect(tabs2[0].classList.contains("on")).toBe(false);
  });

  it("待交互/运行中角标数字上限 99+", () => {
    const counts = { total: 0, running: 150, waiting: 3, archived: 0 };
    const { container } = render(<Sticker filter="all" data={[]} counts={counts} />);
    const badges = Array.from(container.querySelectorAll(".stab-n")).map((e) => e.textContent);
    expect(badges).toContain("99+"); // running=150 → 99+
    expect(badges).toContain("3"); // waiting=3 → 原样
  });

  it("空数据显示 all 空态主文案", () => {
    const { container } = render(<Sticker filter="all" data={[]} />);
    expect(screen.getByText(zh.empty.allTitle)).toBeTruthy();
    expect(container.querySelector("[data-tauri-drag-region]")).toBeTruthy();
  });

  it("渲染会话行：文件夹名 + 最近 AI 正文", () => {
    render(<Sticker filter="all" data={[mk({ cwd: "C:\\dev\\my-project", preview: "最近这条 AI 正文" })]} />);
    expect(screen.getByText("my-project")).toBeTruthy();
    expect(screen.getByText("最近这条 AI 正文")).toBeTruthy();
  });

  it("活动行常显最近 AI 正文(preview)，data-tip 带完整文本", () => {
    const { container } = render(<Sticker filter="all" data={[mk({ preview: "需要你确认下一步" })]} />);
    const subEl = container.querySelector(".stk-sub") as HTMLElement;
    expect(subEl?.textContent).toBe("需要你确认下一步");
    expect(subEl?.getAttribute("data-tip")).toBe("需要你确认下一步");
  });

  it("无 preview 且无错误时不渲染活动行", () => {
    const { container } = render(<Sticker filter="all" data={[mk({ preview: null })]} />);
    expect(container.querySelector(".stk-sub")).toBeNull();
  });

  it("右键菜单置顶切换状态并持久化到 localStorage,操作后菜单关闭", async () => {
    localStorage.removeItem("meowo-starred");
    const { container } = render(<Sticker filter="all" data={[mk({ session: { id: 7, project_id: 1, cc_session_id: "star-me", status: "running", started_at: 0, last_event_at: Date.now(), ended_at: null } })]} />);
    await settingsApplied();
    fireEvent.contextMenu(container.querySelector(".stk-card")!);
    fireEvent.click(screen.getByText(zh.sticker.star));
    expect(container.querySelector(".stk-card.is-star")).toBeTruthy();
    expect(JSON.parse(localStorage.getItem("meowo-starred") ?? "[]")).toContain("star-me");
    expect(document.querySelector(".ctx-menu")).toBeNull(); // 任一菜单项执行后菜单关闭
    localStorage.removeItem("meowo-starred");
  });

  it("菜单键盘可达:挂载即聚焦首项,↑↓ 移动,Enter 激活当前项", async () => {
    localStorage.removeItem("meowo-starred");
    const { container } = render(<Sticker filter="all" data={[mk({ session: { id: 7, project_id: 1, cc_session_id: "kbd-star", status: "running", started_at: 0, last_event_at: Date.now(), ended_at: null } })]} />);
    await settingsApplied();
    fireEvent.contextMenu(container.querySelector(".stk-card")!);
    const menu = document.querySelector(".ctx-menu")! as HTMLElement;
    const items = Array.from(menu.querySelectorAll<HTMLElement>('[role="menuitem"]'));
    // 挂载即聚焦首项：菜单 DOM 在滚动区外，Tab 走不进来，这是键盘用户唯一的入口。
    expect(document.activeElement).toBe(items[0]);
    fireEvent.keyDown(menu, { key: "ArrowDown" });
    expect(document.activeElement).toBe(items[1]);
    fireEvent.keyDown(menu, { key: "ArrowUp" });
    expect(document.activeElement).toBe(items[0]);
    // Enter 激活当前项（首项=置顶）并关闭菜单
    fireEvent.keyDown(menu, { key: "Enter" });
    expect(JSON.parse(localStorage.getItem("meowo-starred") ?? "[]")).toContain("kbd-star");
    expect(document.querySelector(".ctx-menu")).toBeNull();
    localStorage.removeItem("meowo-starred");
  });

  it("右键打开菜单:含置顶/便签/重命名/归档四项,Escape 关闭", async () => {
    const { container } = render(<Sticker filter="all" data={[mk()]} />);
    await settingsApplied();
    fireEvent.contextMenu(container.querySelector(".stk-card")!);
    const menu = document.querySelector(".ctx-menu")!;
    expect(menu).toBeTruthy();
    const labels = Array.from(menu.querySelectorAll(".ctx-item")).map((el) => el.textContent);
    expect(labels).toEqual([zh.sticker.star, zh.sticker.noteAdd, zh.sticker.renameTitle, zh.sticker.archive, zh.sticker.newSession]);
    fireEvent.keyDown(document, { key: "Escape" });
    expect(document.querySelector(".ctx-menu")).toBeNull();
  });

  it("点击菜单外部关闭菜单,且该次点击不触发卡片点击", async () => {
    // 观察哨改为「未发起导航」:编辑器有失焦提交后,菜单开启抢焦点会合法地提交并关闭
    // 编辑器,不能再当哨兵用。连接中的卡片被点击会发 focus_session,没发就证明点击被
    // 菜单的捕获相拦下了。
    const { container } = render(<Sticker filter="all" data={[mk({ connected: true, pid: 1234 })]} />);
    await settingsApplied();
    fireEvent.contextMenu(container.querySelector(".stk-card")!);
    expect(document.querySelector(".ctx-menu")).toBeTruthy();
    fireEvent.click(container.querySelector(".stk-card")!);
    expect(document.querySelector(".ctx-menu")).toBeNull();
    expect(invokeMock.mock.calls.some(([cmd]) => cmd === "focus_session")).toBe(false);
  });

  it("首帧按真实默认(button)渲染菜单按钮;注入 context 设置后按钮让位右键", async () => {
    const { container } = render(<Sticker filter="all" data={[mk()]} />);
    // 首帧占位与后端默认(button)一致：按钮直接在场，不再「settings 回来才凭空弹入」。
    expect(container.querySelector(".stk-menu-btn")).toBeTruthy();
    await settingsApplied();
    // 本测试文件的 get_settings 注入 card_menu_mode=context → 套用后二选一，按钮让位。
    expect(container.querySelector(".stk-menu-btn")).toBeNull();
  });

  it("有 cwd 的会话菜单末尾多出「打开项目目录」,无 cwd 则隐藏", async () => {
    const { container } = render(<Sticker filter="all" data={[mk({ cwd: "C:\\proj" })]} />);
    await settingsApplied();
    fireEvent.contextMenu(container.querySelector(".stk-card")!);
    expect(screen.getByText(zh.sticker.openProjectDir)).toBeTruthy();
    expect(document.querySelector(".ctx-sep")).toBeTruthy(); // 与卡片管理操作以分隔线分组
    fireEvent.keyDown(document, { key: "Escape" });

    cleanup();
    const { container: c2 } = render(<Sticker filter="all" data={[mk({ cwd: null })]} />);
    await settingsApplied();
    fireEvent.contextMenu(c2.querySelector(".stk-card")!);
    expect(screen.queryByText(zh.sticker.openProjectDir)).toBeNull();
  });

  it("本 GUI 托管的会话菜单末尾多出「结束会话」,确认后发出 stop_managed_terminal", async () => {
    const { container } = render(<Sticker filter="all" data={[mk({ pty_managed: true })]} />);
    await settingsApplied();
    fireEvent.contextMenu(container.querySelector(".stk-card")!);
    const item = screen.getByText(zh.chat.endSession);
    // 确认框应答「确认」→ 停止命令发出。once 紧贴点击设置,免被挂载期调用吃掉。
    invokeMock.mockImplementationOnce(() => Promise.resolve(true) as Promise<unknown> as Promise<void>);
    fireEvent.click(item);
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith(
        "confirm_dialog",
        expect.objectContaining({ title: zh.chat.endSession, danger: true }),
      ),
    );
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("stop_managed_terminal", { sessionId: 1 }));
  });

  it("「结束会话」确认框取消时不发停止命令", async () => {
    const { container } = render(<Sticker filter="all" data={[mk({ pty_managed: true })]} />);
    await settingsApplied();
    fireEvent.contextMenu(container.querySelector(".stk-card")!);
    // 默认 invokeMock 对 confirm_dialog 返回 undefined → appConfirm 按取消收场。
    fireEvent.click(screen.getByText(zh.chat.endSession));
    await new Promise((r) => setTimeout(r, 10));
    expect(invokeMock).not.toHaveBeenCalledWith("stop_managed_terminal", expect.anything());
  });

  it("非本 GUI 托管的会话(外部终端)不显示「结束会话」", async () => {
    const { container } = render(<Sticker filter="all" data={[mk({ pty_managed: false, connected: true })]} />);
    await settingsApplied();
    fireEvent.contextMenu(container.querySelector(".stk-card")!);
    expect(document.querySelector(".ctx-menu")).toBeTruthy();
    expect(screen.queryByText(zh.chat.endSession)).toBeNull();
  });

  it("已置顶/有便签的会话,菜单项显示反向文案", async () => {
    // 已归档的会话不再上看板（管理入口在设置 → 会话），板上的菜单只会出现「归档」正向文案。
    localStorage.setItem("meowo-starred", JSON.stringify(["s"]));
    const { container } = render(<Sticker filter="all" data={[mk({ note: "有便签", connected: true })]} />);
    await settingsApplied();
    fireEvent.contextMenu(container.querySelector(".stk-card")!);
    const labels = Array.from(document.querySelectorAll(".ctx-item")).map((el) => el.textContent);
    expect(labels).toEqual([zh.sticker.unstar, zh.sticker.noteEdit, zh.sticker.renameTitle, zh.sticker.archive, zh.sticker.newSession]);
    localStorage.removeItem("meowo-starred");
  });

  it("菜单「新建会话」用当前会话的 cwd 和 provider 打开新建窗口", async () => {
    const { container } = render(<Sticker filter="all" data={[mk({ cwd: "C:\\\\proj", provider: "kimi" })]} />);
    await settingsApplied();
    fireEvent.contextMenu(container.querySelector(".stk-card")!);
    fireEvent.click(screen.getByText(zh.sticker.newSession));
    expect(invokeMock).toHaveBeenCalledWith("open_new_session_window", { cwd: "C:\\\\proj", provider: "kimi" });
  });

  it("点归档即同步回调 onArchiveOptimistic（不等 IPC 往返），并发出 set_archived", async () => {
    const onArchiveOptimistic = vi.fn();
    const onArchiveFailed = vi.fn();
    const { container } = render(
      <Sticker filter="all" data={[mk()]} onArchiveOptimistic={onArchiveOptimistic} onArchiveFailed={onArchiveFailed} />
    );
    await settingsApplied();
    fireEvent.contextMenu(container.querySelector(".stk-card")!);
    fireEvent.click(screen.getByText(zh.sticker.archive));
    expect(onArchiveOptimistic).toHaveBeenCalledWith(1); // 同步：await 之前就已调用
    await new Promise((r) => setTimeout(r, 10));
    expect(invokeMock).toHaveBeenCalledWith("set_archived", { sessionId: 1, archived: true });
    expect(onArchiveFailed).not.toHaveBeenCalled();
  });

  // 回归：set_archived 失败时若不回滚，卡片已被乐观摘掉却没真归档，用户以为成功了。
  it("归档 invoke 失败时调用 onArchiveFailed 以便回滚乐观更新", async () => {
    const onArchiveOptimistic = vi.fn();
    const onArchiveFailed = vi.fn();
    const { container } = render(
      <Sticker filter="all" data={[mk()]} onArchiveOptimistic={onArchiveOptimistic} onArchiveFailed={onArchiveFailed} />
    );
    await settingsApplied();
    fireEvent.contextMenu(container.querySelector(".stk-card")!);
    // once 必须紧贴点击设置：放在 render 前会被挂载时的 get_settings 调用吃掉。
    // as Promise<void>：invokeMock 的返回类型是各命令结果的联合，reject 分支需显式收窄。
    invokeMock.mockImplementationOnce(() => Promise.reject(new Error("boom")) as Promise<void>);
    fireEvent.click(screen.getByText(zh.sticker.archive));
    expect(onArchiveOptimistic).toHaveBeenCalledWith(1);
    await new Promise((r) => setTimeout(r, 10));
    expect(onArchiveFailed).toHaveBeenCalledTimes(1);
  });

  it("待交互 tab 保留后端顺序，不客户端重排", () => {
    const base = (id: number, cc: string, last: number) =>
      mk({ task_title: cc, current_activity: null, connected: true,
        session: { id, project_id: 1, cc_session_id: cc, status: "waiting", started_at: 0, last_event_at: last, ended_at: null } });
    const now = Date.now();
    // 故意传入「非等待最久优先」的顺序（更近的排前面、等待更久的排后面）——
    // 若组件仍客户端按 last_event_at ASC 重排（旧实现），会把顺序翻成 [旧, 新]，断言会失败；
    // 新实现应原样保留后端给的顺序（只做 starred 浮顶），断言才会通过。
    const { container } = render(<Sticker filter="waiting" data={[
      base(1, "新", now - 60_000),   // 1 分钟前(更近)
      base(2, "旧", now - 600_000),  // 10 分钟前(等待更久)
    ]} />);
    const cards = container.querySelectorAll(".stk-card");
    expect(cards[0].querySelector(".stk-title")?.textContent).toBe("新");
    expect(cards[1].querySelector(".stk-title")?.textContent).toBe("旧");
  });

  it("已置顶会话排到列表最前", () => {
    localStorage.setItem("meowo-starred", JSON.stringify(["b"]));
    const { container } = render(<Sticker filter="all" data={[
      mk({ task_title: "甲", current_activity: null, session: { id: 1, project_id: 1, cc_session_id: "a", status: "running", started_at: 0, last_event_at: Date.now(), ended_at: null } }),
      mk({ task_title: "乙", current_activity: null, session: { id: 2, project_id: 1, cc_session_id: "b", status: "running", started_at: 0, last_event_at: Date.now(), ended_at: null } }),
    ]} />);
    const cards = container.querySelectorAll(".stk-card");
    expect(cards[0].querySelector(".stk-title")?.textContent).toBe("乙");
    expect(cards[0].classList.contains("is-star")).toBe(true);
    localStorage.removeItem("meowo-starred");
  });

  it("有便签时渲染便签块", () => {
    const { container } = render(<Sticker filter="all" data={[mk({ note: "记得 review PR" })]} />);
    expect(screen.getByText("记得 review PR")).toBeTruthy();
    expect(container.querySelector(".stk-note")).toBeTruthy();
  });

  it("无便签时经右键菜单打开编辑框", async () => {
    const { container } = render(<Sticker filter="all" data={[mk({ note: null })]} />);
    expect(container.querySelector(".stk-note-edit")).toBeNull();
    await settingsApplied();
    fireEvent.contextMenu(container.querySelector(".stk-card")!);
    fireEvent.click(screen.getByText(zh.sticker.noteAdd));
    const input = container.querySelector(".stk-note-edit") as HTMLInputElement;
    expect(input).toBeTruthy();
    expect(input.placeholder).toBe(zh.sticker.notePlaceholder);
  });

  it("点击便签块进入编辑并预填原文", () => {
    const { container } = render(<Sticker filter="all" data={[mk({ note: "旧便签" })]} />);
    fireEvent.click(container.querySelector(".stk-note")!);
    const input = container.querySelector(".stk-note-edit") as HTMLInputElement;
    expect(input.value).toBe("旧便签");
  });

  it("便签编辑框有保存/取消按钮，点取消关闭且保留原文", async () => {
    const { container } = render(<Sticker filter="all" data={[mk({ note: "保留我" })]} />);
    await settingsApplied();
    fireEvent.contextMenu(container.querySelector(".stk-card")!);
    fireEvent.click(screen.getByText(zh.sticker.noteEdit));
    expect(screen.getByLabelText(zh.sticker.noteSave)).toBeTruthy();
    fireEvent.click(screen.getByLabelText(zh.sticker.noteCancel));
    expect(container.querySelector(".stk-note-edit")).toBeNull();
    expect(screen.getByText("保留我")).toBeTruthy(); // 便签块仍在
  });

  it("点便签保存按钮关闭编辑框", async () => {
    const { container } = render(<Sticker filter="all" data={[mk({ note: null })]} />);
    await settingsApplied();
    fireEvent.contextMenu(container.querySelector(".stk-card")!);
    fireEvent.click(screen.getByText(zh.sticker.noteAdd));
    fireEvent.change(container.querySelector(".stk-note-edit") as HTMLInputElement, { target: { value: "新便签" } });
    fireEvent.click(screen.getByLabelText(zh.sticker.noteSave));
    expect(container.querySelector(".stk-note-edit")).toBeNull();
  });

  it("重命名编辑器有保存/取消按钮，点取消关闭", async () => {
    const { container } = render(<Sticker filter="all" data={[mk()]} />);
    await settingsApplied();
    fireEvent.contextMenu(container.querySelector(".stk-card")!);
    fireEvent.click(screen.getByText(zh.sticker.renameTitle));
    expect(container.querySelector(".stk-editbox")).toBeTruthy();
    expect(screen.getByLabelText(zh.sticker.noteSave)).toBeTruthy();
    fireEvent.click(screen.getByLabelText(zh.sticker.noteCancel));
    expect(container.querySelector(".stk-edit")).toBeNull();
  });

  it("编辑态下点击卡片只关闭编辑器、不导航开终端", async () => {
    // 守卫成立的可观察证据：点击卡片后编辑器关闭（setEditingId(null) 只在早返回分支执行）；
    // 若无守卫，onClick 会走 focus_session 分支、editingId 不变、编辑器仍在。
    const { container } = render(<Sticker filter="all" data={[mk({ connected: true })]} />);
    await settingsApplied();
    fireEvent.contextMenu(container.querySelector(".stk-card")!);
    fireEvent.click(screen.getByText(zh.sticker.renameTitle));
    expect(container.querySelector(".stk-edit")).toBeTruthy();
    fireEvent.click(container.querySelector(".stk-card")!);
    expect(container.querySelector(".stk-edit")).toBeNull();
  });

  it("终端不受支持时提示用支持的终端重新打开，并在结束原进程前经 appConfirm 确认", async () => {
    const original = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation((cmd: string, args?: unknown) => {
      if (cmd === "focus_session") {
        return Promise.resolve("unsupported_terminal") as unknown as ReturnType<typeof original>;
      }
      if (cmd === "confirm_dialog") {
        return Promise.resolve(true) as unknown as ReturnType<typeof original>;
      }
      return original(cmd, args);
    });
    try {
      const { container } = render(<Sticker filter="all" data={[mk({ connected: true, pid: 1234 })]} />);
      fireEvent.click(container.querySelector(".stk-card")!);
      await waitFor(() => expect(screen.getByText(zh.sticker.focusUnsupported)).toBeTruthy());
      fireEvent.click(screen.getByText(zh.sticker.reopenSupported));
      // 确认走全应用统一的 appConfirm 原生小窗（不再是 toast 内二次点击）。
      await waitFor(() => expect(invokeMock).toHaveBeenCalledWith(
        "confirm_dialog",
        expect.objectContaining({ message: zh.sticker.reopenConfirm, danger: true }),
      ));
      await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("restart_session_supported", {
        pid: 1234,
        cwd: null,
        sessionId: "s",
        provider: "claude",
      }));
    } finally {
      invokeMock.mockImplementation(original);
    }
  });

  it("只定位到宿主窗口时保留提示并允许用户选择精准重开", async () => {
    const original = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation((cmd: string, args?: unknown) => {
      if (cmd === "focus_session") {
        return Promise.resolve("host_focused") as unknown as ReturnType<typeof original>;
      }
      return original(cmd, args);
    });
    try {
      const { container } = render(<Sticker filter="all" data={[mk({ connected: true, pid: 1234 })]} />);
      fireEvent.click(container.querySelector(".stk-card")!);
      await waitFor(() => expect(screen.getByText(zh.sticker.focusHostOnly)).toBeTruthy());
      expect(screen.getByText(zh.sticker.reopenSupported)).toBeTruthy();
    } finally {
      invokeMock.mockImplementation(original);
    }
  });

  it("乐观连接但 PID 尚未认领时给出连接中提示，不再静默无动作", () => {
    const { container } = render(<Sticker filter="all" data={[mk({ connected: true, pid: null })]} />);
    fireEvent.click(container.querySelector(".stk-card")!);
    expect(screen.getByText(zh.sticker.focusConnecting)).toBeTruthy();
    expect(invokeMock.mock.calls.some(([cmd]) => cmd === "focus_session")).toBe(false);
  });

  it("默认(点击卡片模式)不渲染独立打开按钮", () => {
    const { container } = render(<Sticker filter="all" data={[mk({ connected: true })]} />);
    expect(container.querySelector(".stk-open")).toBeNull();
  });

  it("unnamed 会话且无动作时显示等待首次输入", () => {
    render(<Sticker filter="all" data={[mk({ task_title: "(未命名会话)", current_activity: null })]} />);
    expect(screen.getByText(zh.sticker.waitingFirstInput)).toBeTruthy();
  });

  it("connected 时 agent 图标高亮（非灰）", () => {
    const { container } = render(<Sticker filter="all" data={[mk({ connected: true })]} />);
    const agent = container.querySelector(".stk-agent");
    expect(agent).toBeTruthy();
    expect(agent?.classList.contains("stk-agent-off")).toBe(false);
  });

  it("disconnected 时 agent 图标变灰（stk-agent-off）", () => {
    const { container } = render(<Sticker filter="all" data={[mk({ connected: false })]} />);
    expect(container.querySelector(".stk-agent.stk-agent-off")).toBeTruthy();
  });

  it("stale + disconnected 时 agent 图标变灰", () => {
    const { container } = render(<Sticker filter="all" data={[mk({ session: { id: 2, project_id: 1, cc_session_id: "x", status: "stale", started_at: 0, last_event_at: Date.now(), ended_at: null }, connected: false })]} />);
    expect(container.querySelector(".stk-agent.stk-agent-off")).toBeTruthy();
  });

  it.each([
    ["all", zh.empty.allTitle, zh.empty.allHint],
    ["waiting", zh.empty.waitingTitle, zh.empty.waitingHint],
    ["running", zh.empty.runningTitle, null],
  ] as const)("EmptyState[%s] 渲染主文案与提示", (tab, title, hint) => {
    render(<EmptyState tab={tab} />);
    expect(screen.getByText(title)).toBeTruthy();
    if (hint) {
      expect(screen.getByText(hint)).toBeTruthy();
    }
  });

  it("EmptyState[running] 不渲染提示文案", () => {
    const { container } = render(<EmptyState tab="running" />);
    expect(container.querySelector(".stk-empty-hint")).toBeNull();
  });

  it("errored running 会话归入运行中、显示红点与错误文案", () => {
    const item = mk({
      session: { id: 9, project_id: 1, cc_session_id: "s9", status: "running", started_at: 0, last_event_at: Date.now(), ended_at: null },
      errored: true, error_label: "工具调用解析失败", error_raw: "The model's tool call could not be parsed (retry also failed).",
    });
    const { container } = render(<Sticker filter="all" data={[item]} />);
    const waitingTab = screen.getByText(zh.tabs.waiting).closest(".stab")!;
    expect(waitingTab.querySelector(".stab-n")!.textContent).toBe("0");
    const runningTab = screen.getByText(zh.tabs.running).closest(".stab")!;
    expect(runningTab.querySelector(".stab-n")!.textContent).toBe("1");
    expect(container.querySelector(".needs-error")).toBeTruthy();
    expect(screen.getByText("工具调用解析失败")).toBeTruthy();
    expect(screen.getByText("工具调用解析失败").closest(".stk-sub-err")).toBeTruthy();
  });

  it("运行中卡片在徽标圆内显示 Content 已用百分比", () => {
    const { container } = render(<Sticker filter="all" data={[mk({ context_pct: 47 })]} />);
    expect(container.querySelector(".run-badge")).toBeTruthy();
    expect(screen.getByText("47%")).toBeTruthy();
  });

  it("无 context_pct 时只渲染绿圆、不渲染百分比文字", () => {
    const { container } = render(<Sticker filter="all" data={[mk({ context_pct: null })]} />);
    expect(container.querySelector(".run-badge")).toBeTruthy();
    expect(container.querySelector(".run-core")?.textContent).toBe("");
  });

  it("待交互卡片用黄色徽标 run-badge--waiting，且同样显示百分比", () => {
    const { container } = render(<Sticker filter="all" data={[mk({
      session: { id: 3, project_id: 1, cc_session_id: "w", status: "waiting", started_at: 0, last_event_at: Date.now(), ended_at: null },
      connected: true, context_pct: 30,
    })]} />);
    expect(container.querySelector(".run-badge--waiting")).toBeTruthy();
    expect(screen.getByText("30%")).toBeTruthy();
  });

  it("断开优先于 errored：只显示断开环", () => {
    const item = mk({ connected: false, errored: true, error_label: "认证失败" });
    const { container } = render(<Sticker filter="all" data={[item]} />);
    expect(container.querySelector(".ring-stop")).toBeTruthy();
    expect(container.querySelector(".needs-error")).toBeFalsy();
  });

  it("pending_review running 会话归入待交互并正常排序", () => {
    const sess = (id: number, cc: string, status: "running" | "waiting", last: number) =>
      ({ id, project_id: 1, cc_session_id: cc, status, started_at: 0, last_event_at: last, ended_at: null });
    const now = Date.now();
    const items = [
      mk({ task_title: "运行更久的", connected: true, session: sess(1, "r1", "running", now - 600_000) }),
      mk({ task_title: "待批准", connected: true, pending_review: "approval", session: sess(2, "p1", "running", now - 60_000) }),
    ];
    const { container } = render(<Sticker filter="waiting" data={items} />);
    // waiting tab 计数：status=waiting 与 pending_review 都计入
    const waitingTab = screen.getByText(zh.tabs.waiting).closest(".stab")!;
    expect(waitingTab.querySelector(".stab-n")!.textContent).toBe("1");
    // pending_review 会话显示在 waiting tab 下
    expect(container.querySelector(".stk-title")?.textContent).toBe("待批准");
    // running tab 计数：只含无需用户介入的纯 running
    const runningTab = screen.getByText(zh.tabs.running).closest(".stab")!;
    expect(runningTab.querySelector(".stab-n")!.textContent).toBe("1");
  });

  it("pending 会话显示琥珀 pill 与 pending 徽标", () => {
    const item = mk({
      task_title: "审批中",
      connected: true,
      pending_review: "approval",
      context_pct: 30,
      session: { id: 5, project_id: 1, cc_session_id: "pp", status: "running", started_at: 0, last_event_at: Date.now(), ended_at: null },
    });
    const { container } = render(<Sticker filter="all" data={[item]} />);
    expect(screen.getByText(zh.pending.approval)).toBeTruthy();     // pill 文字「待批准」
    expect(container.querySelector(".pending-pill")).toBeTruthy();  // pill 元素
    expect(container.querySelector(".run-badge--pending")).toBeTruthy(); // 琥珀徽标
  });

  it("卡片优先显示 last_ai_text,并显示用户消息行", () => {
    const item = mk({
      connected: true,
      preview: "transcript 兜底的旧预览",
      last_ai_text: "调研完成,结论更微妙",
      last_user_text: "切到这个任务",
      session: { id: 7, project_id: 1, cc_session_id: "uai", status: "waiting", started_at: 0, last_event_at: Date.now(), ended_at: null },
    });
    render(<Sticker filter="all" data={[item]} />);
    expect(screen.getByText("调研完成,结论更微妙")).toBeTruthy(); // AI 行用 last_ai_text 而非 preview
    expect(screen.queryByText("transcript 兜底的旧预览")).toBeNull();
    expect(screen.getByText("切到这个任务")).toBeTruthy();         // 用户消息行
    expect(screen.getByText(zh.sticker.youPrefix)).toBeTruthy();   // 「你」前缀
  });

  it("显示 AI 正文时有 aiPrefix 标签，与用户行对称", () => {
    const item = mk({
      connected: true,
      last_ai_text: "完成了代码审查",
      last_user_text: "帮我看这个 PR",
    });
    const { container } = render(<Sticker filter="all" data={[item]} />);
    // 「AI」前缀标签存在
    const tags = container.querySelectorAll(".stk-msg-tag");
    const tagTexts = Array.from(tags).map((el) => el.textContent);
    expect(tagTexts).toContain(zh.sticker.aiPrefix);  // AI 前缀
    expect(tagTexts).toContain(zh.sticker.youPrefix); // 你 前缀，两行对称
    // AI 标签带品牌色 is-ai 修饰类，用户标签不带（视觉区分主角/用户）
    const aiTag = Array.from(tags).find((el) => el.textContent === zh.sticker.aiPrefix)!;
    const youTag = Array.from(tags).find((el) => el.textContent === zh.sticker.youPrefix)!;
    expect(aiTag.classList.contains("is-ai")).toBe(true);
    expect(youTag.classList.contains("is-ai")).toBe(false);
  });

  it("errored 活动行不显示 aiPrefix 标签", () => {
    const item = mk({
      connected: true,
      errored: true,
      error_label: "工具调用解析失败",
      error_raw: "parse error",
    });
    const { container } = render(<Sticker filter="all" data={[item]} />);
    // 错误标签行存在（红色错误文案），但无 aiPrefix
    expect(container.querySelector(".stk-sub-err")).toBeTruthy();
    const tags = container.querySelectorAll(".stk-msg-tag");
    const tagTexts = Array.from(tags).map((el) => el.textContent);
    expect(tagTexts).not.toContain(zh.sticker.aiPrefix);
  });

  it("有 model 时渲染模型胶囊与 agent 图标", () => {
    const { container } = render(<Sticker filter="all" data={[mk({ model: "Opus" })]} />);
    expect(container.querySelector(".stk-model")?.textContent).toBe("Opus");
    expect(container.querySelector(".stk-agent")).toBeTruthy();
  });

  it("无 model 时只渲染 agent 图标、不渲染模型胶囊", () => {
    const { container } = render(<Sticker filter="all" data={[mk({ model: null })]} />);
    expect(container.querySelector(".stk-agent")).toBeTruthy();
    expect(container.querySelector(".stk-model")).toBeNull();
  });

  it("项目名使用 cwd 的文件夹名，data-tip 显示完整路径", () => {
    const { container } = render(<Sticker filter="all" data={[mk({ cwd: "C:\\Users\\larry\\projects\\autopilot", project_name: "larrygogo/autopilot" })]} />);
    const repo = container.querySelector(".stk-repo") as HTMLElement;
    expect(repo?.textContent).toBe("autopilot");
    expect(repo?.getAttribute("data-tip")).toBe("C:\\Users\\larry\\projects\\autopilot");
  });

  it("无 cwd 时不显示项目名", () => {
    const { container } = render(<Sticker filter="all" data={[mk({ cwd: null, project_name: "larrygogo/autopilot" })]} />);
    expect(container.querySelector(".stk-repo")).toBeNull();
  });

  // 展示名来自后端 list_agents()（见上方 mock），图标来自前端资产表。异步 resolve 后才有名字，
  // 故用 findBy* 语义的 waitFor 等一拍。
  it("agent 标签取后端下发的展示名，徽标取前端资产表", async () => {
    const { container } = render(<Sticker filter="all" data={[mk({ provider: "kimi", project_name: "kimi-proj" })]} />);
    const agent = container.querySelector(".stk-agent") as HTMLElement;
    await waitFor(() => expect(agent.getAttribute("data-tip")).toBe("Kimi Code"));
    expect(agent.getAttribute("aria-label")).toBe("Kimi Code");
    expect(agent.querySelector("img")).toBeTruthy(); // kimi 徽标内嵌官方 PNG（黑圆角方块已在图内）

    cleanup();
    const { container: c2 } = render(<Sticker filter="all" data={[mk({ provider: "claude" })]} />);
    const a2 = c2.querySelector(".stk-agent") as HTMLElement;
    await waitFor(() => expect(a2.getAttribute("data-tip")).toBe("Claude Code"));
    expect(a2.querySelector("svg rect")).toBeNull(); // Claude logomark 无方块
  });

  /// 本版本不认识的 agent（DB 里存着更新版写入的 id）：显示 id 本身 + 中性徽标，绝不冒名成 Claude。
  it("未知 agent 不冒名成 claude", async () => {
    // 反例得挑一个**永远**不会被注册的 id。这里原本写的是 "gemini"——它后来真成了一个 agent，
    // 有了自己的徽标，这条断言当场变红（幸而如此，否则它会在无人察觉时失去意义）。
    const { container } = render(<Sticker filter="all" data={[mk({ provider: "not-an-agent" })]} />);
    const agent = container.querySelector(".stk-agent") as HTMLElement;
    // 展示名回退成 id 本身——显示 "not-an-agent" 好过显示 "Claude Code"。
    await waitFor(() => expect(agent.getAttribute("data-tip")).toBe("not-an-agent"));
    // 中性兜底徽标是一个半透明圆角方块（rect），不是 Claude 的 sunburst path。
    expect(agent.querySelector("svg rect")).toBeTruthy();
    expect(agent.querySelector("svg path")).toBeNull();
  });

  it("已注册 agent 用自己的徽标，不落到中性兜底", async () => {
    const { container } = render(<Sticker filter="all" data={[mk({ provider: "gemini" })]} />);
    const agent = container.querySelector(".stk-agent") as HTMLElement;
    await waitFor(() => expect(agent.getAttribute("data-tip")).toBe("Gemini CLI"));
    // gemini 用官方 aurora sparkle 位图（<img>）——而中性兜底是一个裸 <svg><rect>。
    // 断言渲染了 img 即证明用的是自己的徽标，没落到兜底。
    expect(agent.querySelector("img")).toBeTruthy();
  });

  it("搜索走后端：输入调用 onSearchChange，且不客户端过滤已加载数据", () => {
    const onSearchChange = vi.fn();
    const { container } = render(
      <Sticker filter="all" data={[mk({ task_title: "任务甲" })]} search="" onSearchChange={onSearchChange} />
    );
    const before = container.querySelectorAll(".stk-vitem").length;
    expect(before).toBeGreaterThan(0);
    // 打开搜索框并输入一个不匹配已加载标题的词
    fireEvent.click(screen.getByLabelText(zh.sticker.search));
    const input = container.querySelector(".stk-search-in") as HTMLInputElement;
    fireEvent.change(input, { target: { value: "不匹配任何标题zzz" } });
    // 搜索词经回调交后端处理
    expect(onSearchChange).toHaveBeenCalledWith("不匹配任何标题zzz");
    // 前端不再按搜索词过滤已加载数据（过滤由后端负责）→ 卡片数不变
    expect(container.querySelectorAll(".stk-vitem").length).toBe(before);
  });

  it("主导航 tab 是 button[role=tab]，容器 role=tablist，选中态 aria-selected", () => {
    const onFilterChange = vi.fn();
    const { container } = render(<Sticker filter="all" data={[]} onFilterChange={onFilterChange} />);
    const tablist = container.querySelector("[role='tablist']")!;
    expect(tablist).toBeTruthy();
    const tabs = tablist.querySelectorAll("button[role='tab']");
    expect(tabs.length).toBe(3);
    expect(tabs[0].getAttribute("aria-selected")).toBe("true"); // 当前 tab=all
    expect(tabs[1].getAttribute("aria-selected")).toBe("false");
    fireEvent.click(tabs[1]);
    expect(onFilterChange).toHaveBeenCalledWith("waiting");
  });

  it("底栏操作钮均为 button（键盘可达）", () => {
    render(<Sticker filter="all" data={[]} />);
    for (const label of [zh.newSession.newButton, zh.sticker.search, zh.sticker.openSettings, zh.sticker.pinOff]) {
      expect(screen.getByLabelText(label).tagName).toBe("BUTTON");
    }
    // 关闭搜索钮在搜索激活后出现，同样是 button
    fireEvent.click(screen.getByLabelText(zh.sticker.search));
    expect(screen.getByLabelText(zh.sticker.searchClose).tagName).toBe("BUTTON");
  });

  it("卡片支持键盘：Enter/Space 触发与点击相同的打开终端行为", async () => {
    const { container } = render(<Sticker filter="all" data={[mk({ connected: true, pid: 1234 })]} />);
    const card = container.querySelector(".stk-card") as HTMLElement;
    expect(card.getAttribute("role")).toBe("button");
    expect(card.getAttribute("tabindex")).toBe("0");
    fireEvent.keyDown(card, { key: "Enter" });
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("focus_session", expect.objectContaining({ pid: 1234 }))
    );
    invokeMock.mockClear();
    fireEvent.keyDown(card, { key: " " });
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("focus_session", expect.objectContaining({ pid: 1234 }))
    );
  });

  it("卡片内按钮的键盘事件不冒泡触发卡片的打开终端行为", () => {
    const { container } = render(<Sticker filter="all" data={[mk({ connected: true, pid: 1234 })]} />);
    expect(container.querySelector(".stk-card")).toBeTruthy();
    fireEvent.keyDown(screen.getByLabelText(zh.sticker.openChat), { key: "Enter" });
    expect(invokeMock.mock.calls.some(([cmd]) => cmd === "focus_session")).toBe(false);
  });

  it("便签块支持键盘：Enter 进入编辑并预填原文，且不开终端", () => {
    const { container } = render(<Sticker filter="all" data={[mk({ note: "旧便签" })]} />);
    const note = container.querySelector(".stk-note") as HTMLElement;
    expect(note.getAttribute("role")).toBe("button");
    expect(note.getAttribute("tabindex")).toBe("0");
    fireEvent.keyDown(note, { key: "Enter" });
    expect((container.querySelector(".stk-note-edit") as HTMLInputElement).value).toBe("旧便签");
    expect(invokeMock.mock.calls.some(([cmd]) => cmd === "focus_session")).toBe(false);
  });

  it("搜索有词且 0 结果：显示独立「无匹配」空态，不带「新建会话」CTA", () => {
    const { container } = render(<Sticker filter="all" data={[]} search="zzz" onSearchChange={() => {}} />);
    fireEvent.click(screen.getByLabelText(zh.sticker.search)); // 激活搜索
    expect(screen.getByText(zh.empty.searchTitle)).toBeTruthy();
    expect(screen.getByText(zh.empty.searchHint)).toBeTruthy();
    expect(container.querySelector("[data-testid='empty-new-cta']")).toBeNull();
    expect(screen.queryByText(zh.empty.allTitle)).toBeNull();
  });

  it("initialLoading 时显示加载占位而非空态文案", () => {
    render(<Sticker filter="all" data={[]} initialLoading />);
    expect(screen.getByText(zh.sticker.loading)).toBeTruthy();
    expect(screen.queryByText(zh.empty.allTitle)).toBeNull();
  });

  it("loadError 时显示加载失败与重试按钮，点击触发 onRetry", () => {
    const onRetry = vi.fn();
    const { container } = render(<Sticker filter="all" data={[]} loadError onRetry={onRetry} />);
    expect(screen.getByText(zh.sticker.loadFailed)).toBeTruthy();
    expect(screen.queryByText(zh.empty.allTitle)).toBeNull();
    fireEvent.click(container.querySelector("[data-testid='empty-retry-cta']")!);
    expect(onRetry).toHaveBeenCalledTimes(1);
  });

  it("重命名保存失败时经 focusNotice 提示，不静默吞错", async () => {
    const { container } = render(<Sticker filter="all" data={[mk()]} />);
    await settingsApplied();
    fireEvent.contextMenu(container.querySelector(".stk-card")!);
    fireEvent.click(screen.getByText(zh.sticker.renameTitle));
    fireEvent.change(container.querySelector(".stk-edit") as HTMLInputElement, { target: { value: "新名字" } });
    // once 必须紧贴点击设置：只让 rename_session 这一次调用失败
    invokeMock.mockImplementationOnce(() => Promise.reject(new Error("boom")) as Promise<void>);
    fireEvent.click(screen.getByLabelText(zh.sticker.noteSave));
    await waitFor(() => expect(screen.getByText(zh.sticker.renameFailed)).toBeTruthy());
  });

  it("便签保存失败时经 focusNotice 提示，不静默吞错", async () => {
    const { container } = render(<Sticker filter="all" data={[mk({ note: "旧" })]} />);
    fireEvent.click(container.querySelector(".stk-note")!);
    fireEvent.change(container.querySelector(".stk-note-edit") as HTMLInputElement, { target: { value: "新便签" } });
    invokeMock.mockImplementationOnce(() => Promise.reject(new Error("boom")) as Promise<void>);
    fireEvent.click(screen.getByLabelText(zh.sticker.noteSave));
    await waitFor(() => expect(screen.getByText(zh.sticker.noteFailed)).toBeTruthy());
  });
});

describe("自绘滚动条 thumb 拖拽", () => {
  // jsdom 没有布局，scrollHeight/clientHeight 恒为 0，syncSb 会判定无需滚动条、thumb 不渲染。
  // 给滚动容器补上假尺寸再触发一次 syncSb（只管 thumb 拖拽这条链路，不管布局精度）。
  const renderThumb = async () => {
    const { container } = render(<Sticker filter="all" data={[mk()]} />);
    const scrollEl = container.querySelector(".stk-scroll") as HTMLElement;
    Object.defineProperties(scrollEl, {
      scrollHeight: { configurable: true, value: 2000 },
      clientHeight: { configurable: true, value: 600 },
    });
    fireEvent.scroll(scrollEl);
    await waitFor(() => expect(container.querySelector(".stk-sb")).not.toBeNull());
    return { scrollEl, thumb: container.querySelector(".stk-sb") as HTMLElement };
  };

  it("拖拽中移出窗口松手（buttons=0 的 mousemove）→ 结束拖拽并注销监听", async () => {
    const { scrollEl, thumb } = await renderThumb();
    fireEvent.mouseDown(thumb, { clientY: 100, buttons: 1 });
    expect(thumb.className).toContain("is-drag");
    // 正常拖动：按住键移动会改写 scrollTop。
    fireEvent.mouseMove(window, { clientY: 220, buttons: 1 });
    const dragged = scrollEl.scrollTop;
    expect(dragged).toBeGreaterThan(0);
    // 窗外松手后移回窗内：这次无按键的移动必须被认作松手，而不是继续拖动。
    fireEvent.mouseMove(window, { clientY: 500, buttons: 0 });
    expect(thumb.className).not.toContain("is-drag");
    expect(scrollEl.scrollTop).toBe(dragged);
    // 监听已注销：之后按住键移动也不许再改写 scrollTop。
    fireEvent.mouseMove(window, { clientY: 900, buttons: 1 });
    expect(scrollEl.scrollTop).toBe(dragged);
  });

  it("拖拽中窗口失焦（blur）→ 同样结束拖拽", async () => {
    const { scrollEl, thumb } = await renderThumb();
    fireEvent.mouseDown(thumb, { clientY: 100, buttons: 1 });
    expect(thumb.className).toContain("is-drag");
    fireEvent(window, new Event("blur"));
    expect(thumb.className).not.toContain("is-drag");
    fireEvent.mouseMove(window, { clientY: 900, buttons: 1 });
    expect(scrollEl.scrollTop).toBe(0);
  });
});

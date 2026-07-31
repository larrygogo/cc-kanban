import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

const invoke = vi.hoisted(() => vi.fn());
const openDialog = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: openDialog }));
// 捕获事件回调：审批类用例需要手动投递 pending-approval，验证「别的会话要授权时不切窗」。
const eventListeners = vi.hoisted(() => new Map<string, (event: { payload: unknown }) => void>());
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn((name: string, cb: (event: { payload: unknown }) => void) => {
    eventListeners.set(name, cb);
    return Promise.resolve(() => {});
  }),
}));
const setTitleMock = vi.hoisted(() => vi.fn(() => Promise.resolve()));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ close: vi.fn(() => Promise.resolve()), setTitle: setTitleMock }),
}));
// 记录挂载次数：切 tab 不应重建终端（重建=dispose+new Terminal+全量 backlog 重放）。
const terminalMounts = vi.hoisted(() => ({ count: 0 }));
// 真实组件的屏幕识别在这里跑不动（没有 xterm）。把它的回调与入参暴露出来，测试就能
// 用**真实解析器**产出的 attention 驱动 ChatWindow，验证渲染与按键下发这一段。
const terminalProps = vi.hoisted(() => ({ current: null as null | Record<string, unknown> }));
vi.mock("./ManagedTerminal", async () => {
  const { useEffect } = await import("react");
  return {
    ManagedTerminal: (props: { sessionId: number }) => {
      terminalProps.current = props as unknown as Record<string, unknown>;
      useEffect(() => { terminalMounts.count += 1; }, []);
      return <div>PTY {props.sessionId}</div>;
    },
  };
});

import { ChatWindow } from "./ChatWindow";
import { zh } from "../i18n/zh";
import { chatUi } from "../test/agents";
import { terminalAttention } from "../terminalAttention";

function respondWithHistory(history: unknown, approval: unknown = null) {
  invoke.mockImplementation((command: string, args?: Record<string, unknown>) => {
    // 增量语义必须模拟：组件把每次轮询返回的 items **追加**进 transcript，真实后端
    // 只回 offset 之后的新事件。无视 offset 每轮都回整批，慢机上测试跨过 650ms
    // 轮询间隔时同一批会被追加两遍——Windows CI 上「找到多个元素」的抖动即源于此。
    if (command === "get_chat_history") {
      // connected 是 DTO 必填字段(存活校正的数据源):老夹具没写它时补 true,
      // 避免 mock 形状与真实后端分叉、被 `?? false` 静默降级成离线语义。
      const h = { connected: true, ...(history as { offset?: number; items?: unknown[]; connected?: boolean }) };
      const cursor = (args?.offset as number) ?? 0;
      // cursor=0 是首读/全量重读，恒回整批；追平后的轮询才回空。
      if (cursor > 0 && cursor >= (h.offset ?? 0)) {
        return Promise.resolve({ ...h, items: [], hasMore: false });
      }
      return Promise.resolve(h);
    }
    if (command === "get_pending_approval") return Promise.resolve(approval);
    if (command === "managed_terminal_binding") return Promise.resolve(null);
    if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 1, active: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
    return Promise.resolve();
  });
}

afterEach(() => {
  cleanup();
  invoke.mockReset();
  openDialog.mockReset();
  eventListeners.clear();
  window.history.replaceState({}, "", "/");
  // 侧栏折叠状态持久化在 localStorage，不清会串到下一个用例。
  localStorage.clear();
});

describe("ChatWindow", () => {
  it("renders structured transcript entries", async () => {
    window.history.replaceState({}, "", "/?sessionId=7");
    respondWithHistory({
      sessionId: 7,
      title: "实现同步对话",
      status: "running",
      provider: "claude",
      cwd: "C:/repo",
      supported: true,
      offset: 120,
      reset: false,
      pendingReview: null,
      items: [
        { type: "user_text", id: "u1", timestamp: null, text: "开始" },
        { type: "assistant_text", id: "a1", timestamp: null, text: "我来实现" },
        { type: "reasoning", id: "r1", timestamp: null, text: "先检查现有协议" },
        { type: "tool_use", id: "t1", timestamp: null, name: "Bash", summary: "cargo test" },
        { type: "tool_result", id: "tr1", timestamp: null, tool_use_id: "t1", text: "ok", is_error: false },
      ],
    });
    render(<ChatWindow />);
    await waitFor(() => expect(screen.getByText("实现同步对话")).toBeTruthy());
    expect(screen.getByText("实现同步对话").hasAttribute("data-tauri-drag-region")).toBe(true);
    // cwd 是「打开项目目录」按钮：可点击、不做拖拽区（拖拽与点击手势会互相吞）。
    const cwd = screen.getByText("C:/repo");
    expect(cwd.tagName).toBe("BUTTON");
    fireEvent.click(cwd);
    expect(invoke).toHaveBeenCalledWith("open_project_dir", { cwd: "C:/repo" });
    expect(screen.getByText("开始")).toBeTruthy();
    expect(screen.getByText("我来实现")).toBeTruthy();
    // 短思考直接摊开，不为几行内容加一次点击（长的才收成预览态，见下一个用例）。
    const reasoning = screen.getByText("先检查现有协议").closest("details");
    expect(reasoning?.hasAttribute("open")).toBe(true);
    expect(reasoning?.className).not.toContain("is-long");
    const activity = screen.getByText("执行了 1 次工具调用").closest("details");
    expect(activity?.hasAttribute("open")).toBe(false);
    expect(screen.getAllByText("运行终端").length).toBeGreaterThan(0);
    expect(screen.queryByText("工具结果")).toBeNull();
    expect(invoke).toHaveBeenCalledWith("get_chat_history", { sessionId: 7, offset: 0 });
    fireEvent.change(screen.getByRole("textbox", { name: "发送消息给 Agent" }), { target: { value: "继续实现" } });
    fireEvent.keyDown(screen.getByRole("textbox", { name: "发送消息给 Agent" }), { key: "Enter" });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 7, data: "继续实现" }));
    // Enter 在正文之后隔 SUBMIT_GAP_MS 才发（等 TUI 的斜杠补全渲染完，否则 Enter 会被
    // 补全菜单吃掉、只换行不提交），故这里同样要 waitFor。
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 7, data: "\r" }));
    fireEvent.click(screen.getByRole("button", { name: "终端" }));
    expect(screen.getByText("PTY 7")).toBeTruthy();
  });

  it("展开子任务时才拉取它的时间线，并嵌套渲染", async () => {
    window.history.replaceState({}, "", "/?sessionId=9");
    const nested = [{
      label: "agent-0", status: "completed",
      items: [
        { type: "assistant_text", id: "s1", timestamp: null, text: "子任务的结论" },
        { type: "tool_use", id: "st1", timestamp: null, name: "Bash", summary: "rg foo" },
      ],
    }];
    respondWithHistory({
      sessionId: 9, title: "派活", status: "running", provider: "claude", cwd: "C:/repo",
      supported: true, offset: 10, reset: false, pendingReview: null,
      items: [
        { type: "user_text", id: "u1", timestamp: null, text: "查一下" },
        {
          type: "tool_use", id: "toolu_1", timestamp: null, name: "Agent", summary: "验证审批双轨",
          subagent: { description: "验证审批双轨", agent_type: "general-purpose", count: 1 },
        },
      ],
    });
    invoke.mockImplementation((command: string, args: Record<string, unknown>) => {
      // 同 respondWithHistory：只有 offset 落后时才回 items，重复整批会在慢机上被追加两遍。
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 9, title: "派活", status: "running", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 10, reset: false, pendingReview: null,
        items: (args.offset as number) >= 10 ? [] : [
          { type: "user_text", id: "u1", timestamp: null, text: "查一下" },
          {
            type: "tool_use", id: "toolu_1", timestamp: null, name: "Agent", summary: "验证审批双轨",
            subagent: { description: "验证审批双轨", agent_type: "general-purpose", count: 1 },
          },
        ],
      });
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 9, active: false, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "get_subagent_transcript") {
        expect(args).toEqual({ sessionId: 9, toolUseId: "toolu_1" });
        return Promise.resolve(nested);
      }
      return Promise.resolve();
    });
    render(<ChatWindow />);

    // 子任务自成一行，不被并进「N 个操作」那一坨；摘要显示的是描述而不是整包参数。
    const summary = await screen.findByText("验证审批双轨");
    expect(screen.getByText("子任务")).toBeTruthy();
    expect(screen.getByText("general-purpose")).toBeTruthy();
    expect(screen.queryByText("执行了 1 次工具调用")).toBeNull();
    // 未展开前绝不请求：一个会话可能有几十个子任务。
    expect(invoke).not.toHaveBeenCalledWith("get_subagent_transcript", expect.anything());

    const details = summary.closest("details")!;
    const toggle = (open: boolean) => {
      details.open = open;
      fireEvent(details, new Event("toggle"));
    };
    toggle(true);
    expect(await screen.findByText("子任务的结论")).toBeTruthy();
    // 嵌套时间线沿用同一套渲染：里面的工具调用照样分组。
    expect(screen.getByText("执行了 1 次工具调用")).toBeTruthy();

    // 折叠再展开不该重复请求（结果缓存在组件里）。
    const calls = invoke.mock.calls.filter(([command]) => command === "get_subagent_transcript").length;
    toggle(false);
    toggle(true);
    expect(invoke.mock.calls.filter(([command]) => command === "get_subagent_transcript").length).toBe(calls);
  });

  it("不展开也显示子任务状态：无回执=在跑，有回执=按结局统计", async () => {
    window.history.replaceState({}, "", "/?sessionId=17");
    const swarm = {
      type: "tool_use", id: "tool_s", timestamp: null, name: "AgentSwarm", summary: "分组审查",
      subagent: { description: "分组审查", agent_type: "explore", count: 3 },
    };
    let done = false;
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 17, title: "批量", status: "running", provider: "kimi", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null,
        items: done
          ? [swarm, {
              type: "tool_result", id: "r1", timestamp: null, tool_use_id: "tool_s",
              text: "done", is_error: false,
              subagent: { running: 0, completed: 2, failed: 1 },
            }]
          // 一批 fan-out 的结局要等整批跑完才写进主链——跑着的时候主链上没有回执。
          : [swarm],
      });
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 17, active: false, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("kimi"));
      return Promise.resolve();
    });
    render(<ChatWindow />);

    // 没有回执 → 三个都在跑，且**不必展开**（不该为一个徽标去读侧车流）。
    expect(await screen.findByText("3 进行中")).toBeTruthy();
    expect(invoke).not.toHaveBeenCalledWith("get_subagent_transcript", expect.anything());

    // 回执到达后按真实结局显示（靠历史轮询自然刷新，不手动重渲染）。
    done = true;
    await waitFor(() => expect(screen.getAllByText("2 完成 · 1 失败").length).toBeGreaterThan(0), { timeout: 3_000 });
    expect(screen.queryByText("3 进行中")).toBeNull();
    expect(invoke).not.toHaveBeenCalledWith("get_subagent_transcript", expect.anything());
  });

  it("交互式菜单型 CLI：切换模型发出 /model，再把弹出的菜单转成按钮", async () => {
    window.history.replaceState({}, "", "/?sessionId=18");
    // 真机抓屏形态（见 app/src-tauri/tests/capture_model_menu.rs）：无编号，靠 ❯ 标当前项。
    const menu = [
      "\x1b[2J Select a model  (type to search)",
      "  Tab toggle provider · ↑↓ navigate · Enter select · Esc cancel",
      "     K2.7 Coding            Kimi Code",
      "   ❯ K3                     Kimi Code ← current",
    ].join("\r\n");
    let sentMenuCommand = false;
    invoke.mockImplementation((command: string, args: Record<string, unknown>) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 18, title: "换模型", status: "running", provider: "kimi", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [], model: "K3",
      });
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("kimi"));
      if (command === "write_managed_terminal" && args.data === "/model") sentMenuCommand = true;
      if (command === "managed_terminal_snapshot") {
        // 命令发出后，CLI 把菜单画到屏幕上。
        return Promise.resolve({
          sessionId: 18, active: true,
          data: sentMenuCommand ? btoa(menu) : btoa("ready"),
          startOffset: 0, endOffset: sentMenuCommand ? 400 : 5, exited: false, exitCode: null,
        });
      }
      return Promise.resolve();
    });
    render(<ChatWindow />);

    // kimi 没有静态预设，按钮改为「发命令打开 CLI 自己的菜单」。
    const button = await screen.findByRole("button", { name: "切换模型" });
    fireEvent.click(button);
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 18, data: "/model" }));

    // 识别窗口已打开，交给终端组件去认（真实组件读 xterm 画面，这里直接喂真实解析结果）。
    await waitFor(() => expect(terminalProps.current?.expectMenu).toBe(true));
    const attention = terminalAttention(menu, [], false, true);
    expect(attention?.id).toBe("interactive:cursor-menu");
    act(() => { (terminalProps.current?.onAttention as (a: unknown) => void)(attention); });

    // 菜单被渲染成 GUI 选项；选项文字来自 CLI 现给的清单，宿主没有维护一份。
    const choice = await screen.findByRole("button", { name: /K2\.7 Coding/ }, { timeout: 3_000 });
    fireEvent.click(choice);
    // 菜单首尾循环：从 ❯（K3）上移一格到 K2.7 再回车。
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 18, data: "\x1b[A\r" }));
    // 切换模型不产生 Stop hook，模型不会自己落库；必须主动刷一次，
    // 否则对话页与贴纸会一直挂着旧模型直到下一条消息跑完。
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("refresh_session_model", { sessionId: 18 }), { timeout: 3_000 });
  });

  it("粘贴图片/文件：经宿主落盘拿路径，进入附件条（纯文本粘贴不受影响）", async () => {
    window.history.replaceState({}, "", "/?sessionId=53");
    invoke.mockImplementation((command: string, args?: Record<string, unknown>) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 53, title: "粘贴", status: "running", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [], model: null,
      });
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 53, active: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      if (command === "save_pasted_attachment") {
        return Promise.resolve(`C:/tmp/meowo-paste/1-0/${(args as { fileName: string }).fileName}`);
      }
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const input = await screen.findByRole("textbox");
    // jsdom 的 File/Blob 实现参差，只按组件用到的形状（name + arrayBuffer）伪造。
    const file = { name: "shot.png", arrayBuffer: () => Promise.resolve(new Uint8Array([1, 2, 3]).buffer) } as unknown as File;
    fireEvent.paste(input, { clipboardData: { files: [file] } });
    // 内容经 base64 交给宿主落盘（[1,2,3] → "AQID"），路径回来后按文件名显示为附件。
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("save_pasted_attachment", { fileName: "shot.png", dataBase64: "AQID" }));
    expect(await screen.findByText("shot.png")).toBeTruthy();
  });

  it("手敲交互式内置命令（/config）走菜单识别通道：清空输入框并打开识别窗口", async () => {
    window.history.replaceState({}, "", "/?sessionId=52");
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 52, title: "交互命令", status: "running", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [], model: "Opus",
      });
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve({ sessionId: 52, active: true, data: btoa("ready"), startOffset: 0, endOffset: 5, exited: false, exitCode: null });
      }
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const input = await screen.findByRole("textbox");
    // 等 chatUi（menu_slash_commands 的来源）就位再回车，否则命中判断还没有数据。
    await screen.findByRole("button", { name: "切换模式: 权限模式" });
    fireEvent.change(input, { target: { value: "/config" } });
    fireEvent.keyDown(input, { key: "Enter" });
    // 命令原样送达 CLI，同时打开屏幕识别窗口（expectMenu 传给终端组件）。
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 52, data: "/config" }));
    await waitFor(() => expect(terminalProps.current?.expectMenu).toBe(true));
    // 输入框已清空（提交序列含 SUBMIT_GAP_MS 的回车间隔，清空在其后）；识别期间给过渡
    // 横幅（识别不出的面板也有「切到终端/收起」的出口）。
    await waitFor(() => expect((input as HTMLTextAreaElement).value).toBe(""));
    expect(await screen.findByText("命令界面已在终端打开，正在识别可选项…")).toBeTruthy();
    // 普通消息不受影响：带参数的命令不是「裸命令」，不走菜单通道。
  });

  it("菜单已打开时再点不重发命令（否则会打进搜索框把候选全过滤掉）", async () => {
    window.history.replaceState({}, "", "/?sessionId=19");
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 19, title: "换模型", status: "running", provider: "kimi", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [], model: "K3",
      });
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("kimi"));
      if (command === "managed_terminal_snapshot") return Promise.resolve({
        sessionId: 19, active: true, data: btoa("ready"), startOffset: 0, endOffset: 5, exited: false, exitCode: null,
      });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const button = await screen.findByRole("button", { name: "切换模型" });
    const sentModel = () => invoke.mock.calls.filter(([command, args]) =>
      command === "write_managed_terminal" && (args as { data?: string }).data === "/model").length;

    fireEvent.click(button);
    await waitFor(() => expect(sentModel()).toBe(1));
    // 识别窗口开着时再点：只收起（发 Esc），绝不重发——重发会变成 `Search: /model/model`。
    await waitFor(() => expect(terminalProps.current?.expectMenu).toBe(true));
    fireEvent.click(button);
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 19, data: "\x1b" }));
    expect(sentModel()).toBe(1);
  });

  it("长思考收成预览态：内容仍在 DOM 里，标题给出行数", async () => {
    window.history.replaceState({}, "", "/?sessionId=22");
    const long = Array.from({ length: 30 }, (_, i) => `推理第 ${i + 1} 步`).join("\n");
    respondWithHistory({
      sessionId: 22, title: "长推理", status: "running", provider: "claude", cwd: "C:/repo",
      supported: true, offset: 0, reset: false, pendingReview: null,
      items: [{ type: "reasoning", id: "r1", timestamp: null, text: long }],
    });
    render(<ChatWindow />);

    // 预览态：details 不展开，但内容**不是**被藏起来——CSS 会显示开头几行并渐隐，
    // 所以节点必须仍在 DOM 里（也让浏览器内搜索、屏幕阅读器仍能命中）。
    const first = await screen.findByText(/推理第 1 步/);
    const details = first.closest("details");
    expect(details?.hasAttribute("open")).toBe(false);
    expect(details?.className).toContain("is-long");
    expect(screen.getByText("30 行")).toBeTruthy();
    // 末尾那步也在 DOM 里，只是被 max-height 裁掉了视觉。
    expect(screen.getByText(/推理第 30 步/)).toBeTruthy();
  });

  it("shows the provider capability fallback", async () => {
    window.history.replaceState({}, "", "/?sessionId=8");
    respondWithHistory({
      sessionId: 8, title: "Codex", status: "ended", provider: "codex", cwd: null,
      supported: false, offset: 0, reset: false, pendingReview: null, items: [],
    });
    render(<ChatWindow />);
    await waitFor(() => expect(screen.getByText("这个 Agent 暂未提供结构化对话记录")).toBeTruthy());
  });

  it("running session with no entries says the agent is working, not that there is nothing", async () => {
    // 刚启动的会话：hook 已入库（running）但 transcript 还没落第一条。此时「还没有可显示的
    // 对话记录」与下方的运行指示自相矛盾——空列表 ≠ 没在干活。
    window.history.replaceState({}, "", "/?sessionId=41");
    respondWithHistory({
      sessionId: 41, title: "刚启动", status: "running", provider: "claude", cwd: "C:/repo",
      supported: true, offset: 0, reset: false, pendingReview: null,
      currentActivity: null, items: [],
    });
    render(<ChatWindow />);
    expect(await screen.findByText("Agent 已开始工作，对话内容马上出现")).toBeTruthy();
    expect(screen.queryByText("还没有可显示的对话记录")).toBeNull();
    // 会话结束且确实没有记录时，仍然如实说「没有」。
  });

  it("renders the hook-recorded exchange while the transcript has not landed yet", async () => {
    // transcript 未落盘/未定位到 ≠ 什么都不知道：UserPromptSubmit / Stop 已把最近一问一答
    // 落进 DB（lastUserText / lastAiText），空窗期先渲染它们，而不是一句占位文案。
    window.history.replaceState({}, "", "/?sessionId=42");
    respondWithHistory({
      sessionId: 42, title: "空窗期", status: "running", provider: "claude", cwd: "C:/repo",
      supported: true, offset: 0, reset: false, pendingReview: null,
      lastUserText: "帮我修这个 bug", lastAiText: "我先复现一下", items: [],
    });
    render(<ChatWindow />);
    expect(await screen.findByText("帮我修这个 bug")).toBeTruthy();
    expect(screen.getByText("我先复现一下")).toBeTruthy();
    expect(screen.queryByText("Agent 已开始工作，对话内容马上出现")).toBeNull();
  });

  it("shows the hook-recorded exchange for agents without structured transcripts", async () => {
    // 不提供结构化 transcript 的 agent：hook 数据仍是真实内容，「暂未提供」降为注脚。
    window.history.replaceState({}, "", "/?sessionId=43");
    respondWithHistory({
      sessionId: 43, title: "无结构化记录", status: "running", provider: "gemini", cwd: null,
      supported: false, offset: 0, reset: false, pendingReview: null,
      lastUserText: "整理下这份文档", items: [],
    });
    render(<ChatWindow />);
    expect(await screen.findByText("整理下这份文档")).toBeTruthy();
    expect(screen.getByText("这个 Agent 暂未提供结构化对话记录")).toBeTruthy();
  });

  it("deduplicates adjacent equivalent Kimi user records", async () => {
    window.history.replaceState({}, "", "/?sessionId=18");
    respondWithHistory({
      sessionId: 18, title: "Kimi", status: "ended", provider: "kimi", cwd: null,
      supported: true, offset: 100, reset: false, pendingReview: null,
      items: [
        { type: "user_text", id: "turn", timestamp: null, text: "同一条输入" },
        { type: "user_text", id: "append", timestamp: null, text: "同一条输入" },
      ],
    });
    render(<ChatWindow />);
    await waitFor(() => expect(screen.getAllByText("同一条输入")).toHaveLength(1));
  });

  it("opens a pending managed launch directly in the terminal", async () => {
    window.history.replaceState({}, "", "/?sessionId=-3");
    invoke.mockResolvedValue(null);
    render(<ChatWindow />);
    expect(await screen.findByText("PTY -3")).toBeTruthy();
    expect(invoke).toHaveBeenCalledWith("managed_terminal_binding", { sessionId: -3 });
    expect(invoke).not.toHaveBeenCalledWith("get_chat_history", expect.anything());
  });

  it("merges streaming assistant deltas into one message", async () => {
    window.history.replaceState({}, "", "/?sessionId=9");
    respondWithHistory({
      sessionId: 9, title: "Kimi", status: "running", provider: "kimi", cwd: null,
      supported: true, offset: 2, reset: false, pendingReview: null, items: [
        { type: "user_text", id: "u", timestamp: null, text: "继续" },
        { type: "assistant_delta", id: "d1", timestamp: null, text: "正在" },
        { type: "assistant_delta", id: "d2", timestamp: null, text: "处理" },
      ],
    });
    render(<ChatWindow />);
    expect(await screen.findByText("正在处理")).toBeTruthy();
    expect(screen.queryByText("正在")).toBeNull();
  });

  it("sends selected images and files through the managed PTY", async () => {
    window.history.replaceState({}, "", "/?sessionId=11");
    respondWithHistory({
      sessionId: 11, title: "附件", status: "running", provider: "codex", cwd: "C:/repo",
      supported: true, offset: 0, reset: false, pendingReview: null, items: [],
    });
    openDialog.mockResolvedValue(["C:/tmp/design.png", "C:/tmp/spec.pdf"]);
    render(<ChatWindow />);
    fireEvent.click(await screen.findByRole("button", { name: "添加图片或文件" }));
    expect(await screen.findByText("design.png")).toBeTruthy();
    expect(screen.getByText("spec.pdf")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "发送" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", {
      sessionId: 11,
      data: expect.stringContaining("C:/tmp/design.png"),
    }));
  });

  it("automatically resumes an inactive managed terminal before sending", async () => {
    window.history.replaceState({}, "", "/?sessionId=13");
    const history = {
      sessionId: 13, title: "恢复", status: "ended", provider: "claude", cwd: "C:/repo",
      supported: true, offset: 0, reset: false, pendingReview: null, items: [],
    };
    respondWithHistory(history);
    let started = false;
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve(history);
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "start_managed_terminal") { started = true; return Promise.resolve(); }
      if (command === "managed_terminal_snapshot") {
        // endOffset 是「已产生多少输出」的判据（data 现在是 base64 增量，可能为空）；
        // 就绪判定还要求 data 里有可见文本（纯控制序列不算）。
        return Promise.resolve(started
          ? { sessionId: 13, active: true, data: btoa("ready"), startOffset: 0, endOffset: 5, exited: false, exitCode: null }
          : { sessionId: 13, active: false, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      }
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const input = await screen.findByRole("textbox", { name: "发送消息给 Agent" });
    fireEvent.change(input, { target: { value: "继续" } });
    fireEvent.click(screen.getByRole("button", { name: "发送" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("start_managed_terminal", {
      sessionId: 13, cols: 100, rows: 30,
    }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 13, data: "继续" }), { timeout: 2_000 });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 13, data: "\r" }));
  });

  it("opens a non-Claude startup trust prompt in the terminal without typing the chat message into it", async () => {
    window.history.replaceState({}, "", "/?sessionId=14");
    const history = {
      sessionId: 14, title: "待信任目录", status: "ended", provider: "codex", cwd: "C:/new-repo",
      supported: true, offset: 0, reset: false, pendingReview: null, items: [],
    };
    let started = false;
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve(history);
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "managed_terminal_binding") return Promise.resolve(null);
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("codex"));
      if (command === "start_managed_terminal") { started = true; return Promise.resolve(); }
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve(started
          ? {
              sessionId: 14, active: true,
              data: btoa("\x1b[2JDo you trust the contents of this directory?\r\n> 1. Yes, continue\r\n  2. No, quit"),
              startOffset: 0, endOffset: 76, exited: false, exitCode: null,
            }
          : { sessionId: 14, active: false, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      }
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const input = await screen.findByRole("textbox", { name: "发送消息给 Agent" });
    fireEvent.change(input, { target: { value: "继续修复" } });
    fireEvent.click(screen.getByRole("button", { name: "发送" }));

    // composer 常驻后 footer 里的 sendError 横幅(也是 alert)可能与卡片同屏——按类名取卡片本体。
    const trustCard = (await screen.findAllByRole("alert")).find((el) => el.className.includes("chat-approval"));
    expect(trustCard?.textContent).toContain("是否信任此文件夹？");
    // 卡片在场时 composer 锁定但**不卸载**:textarea 还是同一个节点(草稿不丢),只是禁用。
    expect((screen.getByRole("textbox", { name: "发送消息给 Agent" }) as HTMLTextAreaElement).disabled).toBe(true);
    expect(screen.getByText("PTY 14")).toBeTruthy();
    expect(screen.getByRole("button", { name: "对话" }).className).toContain("is-active");
    expect(screen.getByText("PTY 14").closest(".chat-terminal-pane")?.className).toContain("is-background");
    expect(screen.getByText("PTY 14").closest(".chat-terminal-pane")?.getAttribute("aria-hidden")).toBe("true");
    expect(invoke).not.toHaveBeenCalledWith("write_managed_terminal", { sessionId: 14, data: "继续修复" });
    expect((input as HTMLTextAreaElement).value).toBe("继续修复");
    // 原始终端页已经显示 TUI，不再叠加 GUI 卡片；切回对话后仍可直接点击结构化选项。
    fireEvent.click(screen.getByRole("button", { name: "终端" }));
    expect(screen.queryByRole("alert")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "对话" }));
    fireEvent.click(await screen.findByRole("button", { name: "Yes, continue" }));
    // 光标已停在第一项：相对移动为 0，直接回车确认，不再盲按上键绕圈。
    expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 14, data: "\r" });
  });

  it("shows a managed PTY startup choice when the conversation opens without visiting Terminal", async () => {
    window.history.replaceState({}, "", "/?sessionId=44");
    const history = {
      // Agent 等用户处理启动选择时，reporter 可能已把会话从 running 标成 waiting。
      sessionId: 44, title: "后台信任提示", status: "waiting", provider: "claude", cwd: "C:/new-repo",
      supported: true, offset: 0, reset: false, pendingReview: null, items: [],
    };
    const prompt = "\x1b[2JDo you trust the files in this folder?\r\n> 1. Yes, I trust this folder\r\n  2. No, exit";
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve(history);
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve({ sessionId: 44, active: true, data: btoa(prompt), startOffset: 0, endOffset: prompt.length, exited: false, exitCode: null });
      }
      return Promise.resolve(null);
    });
    render(<ChatWindow />);

    expect(await screen.findByRole("button", { name: "Yes, I trust this folder" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "对话" }).className).toContain("is-active");
    expect(invoke).not.toHaveBeenCalledWith("start_managed_terminal", expect.anything());
  });

  it("renders assistant markdown but keeps user text verbatim", async () => {
    window.history.replaceState({}, "", "/?sessionId=21");
    respondWithHistory({
      sessionId: 21, title: "MD", status: "running", provider: "claude", cwd: null,
      supported: true, offset: 0, reset: false, pendingReview: null,
      items: [
        { type: "user_text", id: "u1", timestamp: null, text: "# 不是标题" },
        { type: "assistant_text", id: "a1", timestamp: null, text: "看 **重点** 和 `code`，详见 [官网](https://example.com/docs)" },
        { type: "assistant_text", id: "a2", timestamp: null, text: "```\n┌─────┐\n│ 会话A │\n└─────┘\n```" },
      ],
    });
    render(<ChatWindow />);
    const strong = await screen.findByText("重点");
    expect(strong.tagName).toBe("STRONG");
    expect(screen.getByText("code").tagName).toBe("CODE");
    // 用户消息按原文展示：行首 # 不得升格成标题。
    const user = screen.getByText("# 不是标题");
    expect(user.tagName).not.toMatch(/^H[1-6]$/);
    // 链接不许让 webview 导航（这个窗口跳走就回不来了），必须交给后端开默认浏览器。
    const link = screen.getByRole("link", { name: "官网" });
    fireEvent.click(link);
    expect(invoke).toHaveBeenCalledWith("open_link", { url: "https://example.com/docs" });
    expect(window.location.href).not.toContain("example.com");
    // 含框线字符的代码块被钉到字符网格：中文锁 2ch 盒子（renderGrid 拆成单字符 span），
    // 整块标记 chat-md-diagram；普通行内代码不受牵连、不被拆分。
    const wide = screen.getByText("话");
    expect(wide.className).toBe("chat-md-cell2");
    expect(wide.closest("code")?.className).toContain("chat-md-diagram");
    expect(screen.getByText("code").className).not.toContain("chat-md-diagram");
  });

  it("shows agent badge, running pulse, slash completions and model switcher", async () => {
    window.history.replaceState({}, "", "/?sessionId=31");
    const history = {
      sessionId: 31, title: "运行观察", status: "running", provider: "claude", cwd: "C:/repo",
      supported: true, offset: 0, reset: false, pendingReview: null, connected: true,
      model: "Opus", contextPct: 63, contextWindow: 200000, currentActivity: "Bash: cargo test",
      items: [{ type: "user_text", id: "u1", timestamp: null, text: "跑" }],
    };
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve(history);
      if (command === "get_pending_approval") return Promise.resolve(null);
      // 斜杠补全与模型预设不是前端硬编码表：按会话查 agent_chat_ui（内置表 ∪ 自定义命令）。
      if (command === "agent_chat_ui") {
        return Promise.resolve(chatUi("claude", [
          { name: "/deploy", description: "部署到测试环境", source: "project" },
        ]));
      }
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve({ sessionId: 31, active: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      }
      return Promise.resolve();
    });
    render(<ChatWindow />);
    await screen.findByText("运行观察");
    // agent logo（标题栏最前，aria-label=provider）+ 运行指示（有活动时显示活动文本）。
    expect(screen.getByLabelText("claude")).toBeTruthy();
    expect(screen.getByText("Bash: cargo test")).toBeTruthy();
    // 上下文用量环：环内百分比 + 环右已用/总量（63% × 200K ≈ 126K）。
    expect(screen.getByText("63")).toBeTruthy();
    expect(screen.getByText("126K/200K")).toBeTruthy();
    // "/" 前缀弹补全；选中后填入输入框并留出参数位，不自动发送。
    const input = screen.getByRole("textbox", { name: "发送消息给 Agent" }) as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: "/mo" } });
    fireEvent.click(screen.getByRole("option", { name: /^\/model/ }));
    expect(input.value).toBe("/model ");
    // 自定义命令来自安装实况（agent_chat_ui 从项目目录发现的），描述取自命令文件头。
    fireEvent.change(input, { target: { value: "/de" } });
    // accessible-name 会按 DOM 实现把相邻 code/span 拼成有空格或无空格，两种都等价。
    fireEvent.click(screen.getByRole("option", { name: /^\/deploy\s*部署到测试环境/ }));
    expect(input.value).toBe("/deploy ");
    // 模型菜单：选择预设即向 PTY 发送 /model <id>。
    fireEvent.click(screen.getByRole("button", { name: "切换模型" }));
    fireEvent.click(screen.getByRole("menuitem", { name: /^Sonnet/ }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 31, data: "/model sonnet" }));
  });

  it("keeps probing a pending runtime skill listing and exposes code-review when it arrives", async () => {
    window.history.replaceState({}, "", "/?sessionId=32");
    let offset = 1;
    let uiCalls = 0;
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 32, title: "技能发现", status: "running", provider: "claude", cwd: "C:/repo",
        supported: true, offset, reset: false, pendingReview: null, items: [],
      });
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve({ sessionId: 32, active: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      }
      if (command === "agent_chat_ui") {
        uiCalls += 1;
        const base = chatUi("claude")!;
        return Promise.resolve(offset === 1
          ? { ...base, runtime_commands_pending: true }
          : {
              ...base,
              runtime_commands_pending: false,
              slash_commands: [...base.slash_commands, {
                name: "/code-review", description: "Review the current diff", source: "builtin" as const,
              }],
            });
      }
      return Promise.resolve();
    });
    render(<ChatWindow />);
    await waitFor(() => expect(uiCalls).toBeGreaterThan(0));
    offset = 2;

    const input = await screen.findByRole("textbox", { name: "发送消息给 Agent" });
    fireEvent.change(input, { target: { value: "/code" } });
    // 探测有 2s 限频（避免随 650ms 轮询打满后端），等待窗口相应放宽。
    expect(await screen.findByRole("option", { name: /\/code-review/ }, { timeout: 4_000 })).toBeTruthy();
    expect(uiCalls).toBeGreaterThan(1);
  });

  it("still reflects metadata changes despite the re-render short-circuit", async () => {
    // sameHistoryMeta 保留旧引用来跳过稳态重渲染；漏掉某个字段就会「数据变了界面不动」。
    // 这里逐个字段改动并断言 UI 跟上，锁住那份比较清单。
    window.history.replaceState({}, "", "/?sessionId=21");
    const base = {
      sessionId: 21, title: "初始标题", status: "running", provider: "claude", cwd: "C:/repo",
      supported: true, offset: 0, reset: false, pendingReview: null, connected: true,
      model: "Opus", agentModes: [{ dimension: "permission", value: "default" }], contextPct: 10, contextWindow: 200000,
      currentActivity: "Bash: 第一步", items: [],
    };
    let current: Record<string, unknown> = { ...base };
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve(current);
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve({ sessionId: 21, active: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      }
      return Promise.resolve();
    });
    render(<ChatWindow />);
    await screen.findByText("初始标题");
    expect(screen.getByText("Bash: 第一步")).toBeTruthy();
    expect(screen.getByText("10")).toBeTruthy();
    expect(screen.getByText("权限模式: 默认")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "切换模式: 权限模式" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 21, data: "\u001b[Z" }));

    // 逐字段单独改：若合并成一次改动，任一字段触发的重渲染都会把其它字段的漏判一并掩盖，
    // 测试就变成假绿（验证过：合并写法下从比较清单里删掉 currentActivity 仍然通过）。
    current = { ...base, currentActivity: "Bash: 第二步" };
    expect(await screen.findByText("Bash: 第二步")).toBeTruthy();

    current = { ...base, currentActivity: "Bash: 第二步", contextPct: 42 };
    expect(await screen.findByText("42")).toBeTruthy();

    current = { ...base, currentActivity: "Bash: 第二步", contextPct: 42, title: "改后标题" };
    expect(await screen.findByText("改后标题")).toBeTruthy();

    current = { ...base, currentActivity: "Bash: 第二步", contextPct: 42, title: "改后标题", agentModes: [{ dimension: "permission", value: "plan" }] };
    expect(await screen.findByText("权限模式: 计划")).toBeTruthy();

    // 兜底时间线读 lastUserText/lastAiText（transcript 空窗期渲染 hook 落库的最近往来），
    // 它们也在比较清单里——漏掉的话空窗期内容永远停在第一轮。
    current = { ...base, currentActivity: "Bash: 第二步", contextPct: 42, title: "改后标题", agentModes: [{ dimension: "permission", value: "plan" }], lastUserText: "hook 落库的提问" };
    expect(await screen.findByText("hook 落库的提问")).toBeTruthy();

    // ptyManaged 也在比较清单里——会话已 connected 时中途拉起托管 PTY,那一轮往往只有
    // 这个字段变;漏掉的话「结束会话」按钮永远不出现(真实翻车过:发消息拉起 PTY 后按钮不见)。
    expect(screen.queryByText("结束会话")).toBeNull();
    current = { ...base, currentActivity: "Bash: 第二步", contextPct: 42, title: "改后标题", agentModes: [{ dimension: "permission", value: "plan" }], lastUserText: "hook 落库的提问", ptyManaged: true };
    expect(await screen.findByText("结束会话")).toBeTruthy();

    // errored 同理——agent 报错通常不伴随其他元数据变化,漏掉的话错误徽标永远不亮。
    current = { ...base, currentActivity: "Bash: 第二步", contextPct: 42, title: "改后标题", agentModes: [{ dimension: "permission", value: "plan" }], lastUserText: "hook 落库的提问", ptyManaged: true, errored: true };
    expect(await screen.findByText("出错了")).toBeTruthy();

    // connected 也在比较清单里——漏掉的话进程死亡(connected 翻 false、status 仍 running)
    // 时标题栏徽标滞留「运行中」,假运行中复活。只翻 connected,断言徽标跟上。
    current = { ...base, currentActivity: "Bash: 第二步", contextPct: 42, title: "改后标题", agentModes: [{ dimension: "permission", value: "plan" }], lastUserText: "hook 落库的提问", ptyManaged: true, errored: true, connected: false };
    await waitFor(() => expect(screen.getByText("未连接")).toBeTruthy());
    // 每步都要等一轮 650ms 轮询,8 个步骤加起来超出 5s 默认时限。
  }, 15_000);

  it("renders Codex mode dimensions and sends direct Kimi mode actions", async () => {
    window.history.replaceState({}, "", "/?sessionId=41");
    const history = {
      sessionId: 41, title: "Kimi 模式", status: "running", provider: "kimi", cwd: "C:/repo",
      supported: true, offset: 0, reset: false, pendingReview: null, model: null,
      agentModes: [
        { dimension: "work", value: "default" },
        { dimension: "permission", value: "manual" },
      ],
      contextPct: null, contextWindow: null, currentActivity: null, hasMore: false, items: [],
    };
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve(history);
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve({ sessionId: 41, active: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      }
      if (command === "agent_chat_ui") return Promise.resolve({
        // 与 ChatUi 真实形状对齐(必填字段全给):此前缺 menu_slash_commands 等新字段,
        // 全靠生产码的可选链兜底侥幸通过,是唯一漏网的 mock。取值保持中性(空表/null),
        // 不引入本用例无关的菜单行为。
        slash_commands: [], model_presets: [], version: "0.26.0",
        model_menu_command: null, menu_slash_commands: [],
        startup_attention_markers: [], selector_anchors: [], interrupt_input: null, runtime_commands_pending: false,
        attachment_mention: false, clipboard_image_paste: null,
        mode_controls: [
          {
            dimension: "work", cycle_input: "\u001b[Z", options: [
              { value: "default", inputs: [{ data: "/plan off", submit: true }] },
              { value: "plan", inputs: [{ data: "/plan on", submit: true }] },
            ], screen_markers: [],
          },
          {
            dimension: "permission", cycle_input: null, options: [
              { value: "manual", inputs: [{ data: "/yolo off", submit: true }, { data: "/auto off", submit: true }] },
              { value: "yolo", inputs: [{ data: "/yolo on", submit: true }] },
              { value: "auto", inputs: [{ data: "/auto on", submit: true }] },
            ], screen_markers: [],
          },
        ],
      });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    await screen.findByText("工作模式: 默认");
    expect(screen.getByText("权限模式: 手动确认")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "切换模式: 权限模式" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "YOLO" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 41, data: "/yolo on" }));
    // Enter 隔 SUBMIT_GAP_MS 才发（见 submitToTerminal），同样要 waitFor。
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 41, data: "\r" }));
    expect(await screen.findByText("权限模式: YOLO")).toBeTruthy();
  });

  it("cycle 盲切后从终端屏幕回显真实落点（claude 空闲期 transcript 不写模式记录）", async () => {
    window.history.replaceState({}, "", "/?sessionId=51");
    const base = {
      sessionId: 51, title: "屏幕回显", status: "running", provider: "claude", cwd: "C:/repo",
      supported: true, offset: 0, reset: false, pendingReview: null, model: null,
      agentModes: [{ dimension: "permission", value: "default" }],
      contextPct: null, contextWindow: null, currentActivity: null, hasMore: false, items: [],
    };
    let current: Record<string, unknown> = { ...base };
    const screenText = "plan mode on (shift+tab to cycle)";
    // 时间线式快照 mock:out 是 PTY 全量输出,snapshot(since) 回 since 之后的增量。
    // 回显必须出现在**按键之后的新输出**里——echo 循环的首帧只取偏移基线、不扫 backlog
    // (切换前的旧指示不作数),backlog 里预置的旧指示正好验证这一点。
    let out = "accept edits on (shift+tab to cycle)\r\n";
    invoke.mockImplementation((command: string, args?: Record<string, unknown>) => {
      if (command === "get_chat_history") return Promise.resolve(current);
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "write_managed_terminal" && (args as { data?: string } | undefined)?.data === "[Z") {
        // CLI 收到 Shift+Tab 后重绘出新指示。
        out += screenText;
      }
      if (command === "managed_terminal_snapshot") {
        const since = Math.min(Number((args as { since?: number } | undefined)?.since ?? 0), out.length);
        return Promise.resolve({ sessionId: 51, active: true, data: btoa(out.slice(since)), startOffset: since, endOffset: out.length, exited: false, exitCode: null });
      }
      return Promise.resolve();
    });
    render(<ChatWindow />);
    await screen.findByText("权限模式: 默认");
    // 空闲期：后续轮询不再带模式增量（真实后端普通增量的 agent_modes 就是空）。
    current = { ...base, agentModes: [] };
    fireEvent.click(screen.getByRole("button", { name: "切换模式: 权限模式" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 51, data: "\u001b[Z" }));
    // transcript 静默，但屏幕是 CLI 自己画的权威状态——标签应据它回显为「计划」。
    expect(await screen.findByText("权限模式: 计划")).toBeTruthy();
  });

  it("offers to load earlier messages when the first read was truncated", async () => {
    window.history.replaceState({}, "", "/?sessionId=33");
    const truncated = {
      sessionId: 33, title: "长会话", status: "running", provider: "claude", cwd: null,
      supported: true, offset: 500, reset: false, pendingReview: null,
      model: null, contextPct: null, contextWindow: null, currentActivity: null,
      hasMore: true,
      items: [{ type: "user_text", id: "recent", timestamp: null, text: "最近的消息" }],
    };
    // 增量轮询恒为 hasMore:false——提示不能因此闪掉。
    const incremental = { ...truncated, items: [], hasMore: false };
    let firstRead = true;
    invoke.mockImplementation((command: string, args: { full?: boolean }) => {
      if (command === "get_chat_history") {
        if (args?.full) {
          return Promise.resolve({
            ...truncated, hasMore: false,
            items: [
              { type: "user_text", id: "old", timestamp: null, text: "很早以前的消息" },
              { type: "user_text", id: "recent", timestamp: null, text: "最近的消息" },
            ],
          });
        }
        if (firstRead) { firstRead = false; return Promise.resolve(truncated); }
        return Promise.resolve(incremental);
      }
      if (command === "get_pending_approval") return Promise.resolve(null);
      return Promise.resolve();
    });
    render(<ChatWindow />);
    await screen.findByText("最近的消息");
    const button = await screen.findByRole("button", { name: "加载更早的对话" });
    // 被裁掉的消息此刻不在 DOM 里——这正是首屏省下的成本。
    expect(screen.queryByText("很早以前的消息")).toBeNull();

    fireEvent.click(button);
    expect(await screen.findByText("很早以前的消息")).toBeTruthy();
    expect(invoke).toHaveBeenCalledWith("get_chat_history", { sessionId: 33, offset: 0, full: true });
    // 取完整历史后提示消失，且不重复插入已有消息。
    await waitFor(() => expect(screen.queryByRole("button", { name: "加载更早的对话" })).toBeNull());
    expect(screen.getAllByText("最近的消息")).toHaveLength(1);
  });

  it("keeps the terminal mounted across tab switches", async () => {
    window.history.replaceState({}, "", "/?sessionId=7");
    respondWithHistory({
      sessionId: 7, title: "保活", status: "running", provider: "claude", cwd: null,
      supported: true, offset: 0, reset: false, pendingReview: null, items: [],
    });
    terminalMounts.count = 0;
    render(<ChatWindow />);
    await screen.findByText("保活");
    // broker 报告活跃 PTY 后即在屏幕外挂载一次，以便无需切 tab 也能还原 ANSI 选择器。
    await waitFor(() => expect(terminalMounts.count).toBe(1));

    fireEvent.click(screen.getByRole("button", { name: "终端" }));
    expect(screen.getByText("PTY 7")).toBeTruthy();
    expect(terminalMounts.count).toBe(1);

    // 切回对话再切回终端：终端留在树上（隐藏），不得重建。
    fireEvent.click(screen.getByRole("button", { name: "对话" }));
    fireEvent.click(screen.getByRole("button", { name: "终端" }));
    expect(terminalMounts.count).toBe(1);
  });

  it("keeps terminal view when switching sessions from the sidebar", async () => {
    window.history.replaceState({}, "", "/?sessionId=7");
    invoke.mockImplementation((command: string, args: { sessionId?: number }) => {
      if (command === "get_chat_history") {
        return Promise.resolve({
          sessionId: args?.sessionId ?? 7, title: `会话 ${args?.sessionId}`, status: "running",
          provider: "claude", cwd: null, supported: true, offset: 0, reset: false,
          pendingReview: null, items: [],
        });
      }
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "managed_terminal_binding") return Promise.resolve(null);
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 7, active: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      if (command === "get_live_sessions_page") {
        return Promise.resolve([
          { session: { id: 7, cc_session_id: "a", status: "running" }, project_name: "p", task_title: "会话 7", connected: true, pending_review: null, provider: "claude", cwd: "C:/a" },
          { session: { id: 42, cc_session_id: "b", status: "running" }, project_name: "p", task_title: "另一个会话", connected: true, pending_review: null, provider: "claude", cwd: "C:/b" },
        ]);
      }
      return Promise.resolve();
    });
    render(<ChatWindow />);
    // 切到终端视图。
    fireEvent.click(await screen.findByRole("button", { name: "终端" }));
    expect(screen.getByText("PTY 7")).toBeTruthy();
    // 从侧栏切到另一个会话——视图必须仍是终端，而不是弹回对话。
    fireEvent.click(await screen.findByRole("button", { name: /另一个会话/ }));
    expect(await screen.findByText("PTY 42")).toBeTruthy();
    expect(screen.queryByRole("textbox", { name: "发送消息给 Agent" })).toBeNull();
  });

  /// 归档此前只有看板的卡片菜单能做：读完一个会话想收起它，得先切回看板、在一堆卡片里
  /// 找回同一条。入口补进标题栏后，按钮本身也是「这条已归档」的唯一提示，故点完必须当场
  /// 翻面（乐观），失败再翻回来并报错——静默失败等于骗用户「已归档」。
  it("标题栏归档当前会话：按钮当场翻面，失败回滚并报错", async () => {
    window.history.replaceState({}, "", "/?sessionId=7");
    const base = {
      sessionId: 7, title: "要收起的会话", status: "ended", provider: "claude", cwd: null,
      supported: true, offset: 0, reset: false, pendingReview: null, items: [], archived: false,
    };
    let archiveFails = false;
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve(base);
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "set_archived") return archiveFails ? Promise.reject(new Error("db busy")) : Promise.resolve();
      // 归档后要切走：这里只有它自己一条，没有可切的下一条，窗口留在原地。
      if (command === "get_live_sessions_page") return Promise.resolve([]);
      return Promise.resolve();
    });
    render(<ChatWindow />);
    fireEvent.click(await screen.findByRole("button", { name: "归档" }));
    expect(invoke).toHaveBeenCalledWith("set_archived", { sessionId: 7, archived: true });
    // 翻面后按钮变成「取消归档」——它是这条会话已归档的唯一提示。
    expect(await screen.findByRole("button", { name: "取消归档" })).toBeTruthy();

    archiveFails = true;
    fireEvent.click(screen.getByRole("button", { name: "取消归档" }));
    // 失败回滚：按钮退回归档态，并且错误可见。
    expect(await screen.findByRole("button", { name: "取消归档" })).toBeTruthy();
    expect(await screen.findByText(/db busy/)).toBeTruthy();
  });

  /// 归档 = 收纳：侧栏里当场消失，右边却还停在它的对话上等于「收起来了还摊在桌上」。
  /// 归档后自动切到列表里的下一条会话。
  it("归档当前会话后切到下一条", async () => {
    window.history.replaceState({}, "", "/?sessionId=7");
    const histories: Record<number, unknown> = {
      7: { sessionId: 7, title: "要收起的", status: "ended", provider: "claude", cwd: null, supported: true, offset: 0, reset: false, pendingReview: null, items: [], archived: false },
      9: { sessionId: 9, title: "下一条", status: "ended", provider: "claude", cwd: null, supported: true, offset: 0, reset: false, pendingReview: null, items: [], archived: false },
    };
    invoke.mockImplementation((command: string, args?: Record<string, unknown>) => {
      if (command === "get_chat_history") return Promise.resolve(histories[args?.sessionId as number]);
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "get_live_sessions_page") {
        return Promise.resolve([{ session: { id: 9, cc_session_id: "cc-9", status: "ended" }, task_title: "下一条", connected: false, provider: "claude", cwd: null }]);
      }
      return Promise.resolve();
    });
    render(<ChatWindow />);
    fireEvent.click(await screen.findByRole("button", { name: "归档" }));
    expect(await screen.findByText("下一条")).toBeTruthy();
  });

  it("collapses the sidebar into a title-bar toggle and restores it", async () => {
    window.history.replaceState({}, "", "/?sessionId=7");
    respondWithHistory({
      sessionId: 7, title: "折叠", status: "ended", provider: "claude", cwd: null,
      supported: true, offset: 0, reset: false, pendingReview: null, items: [],
    });
    render(<ChatWindow />);
    // 展开态：收起按钮在侧栏里，标题栏没有展开按钮。
    const collapse = await screen.findByRole("button", { name: "收起会话列表" });
    expect(screen.queryByRole("button", { name: "展开会话列表" })).toBeNull();
    fireEvent.click(collapse);
    // 收起态：侧栏整个消失，展开入口出现在标题栏，偏好落盘。
    expect(screen.queryByRole("button", { name: "收起会话列表" })).toBeNull();
    expect(localStorage.getItem("meowo-chat-sidebar-collapsed")).toBe("1");
    fireEvent.click(screen.getByRole("button", { name: "展开会话列表" }));
    expect(await screen.findByRole("button", { name: "收起会话列表" })).toBeTruthy();
    expect(localStorage.getItem("meowo-chat-sidebar-collapsed")).toBe("0");
  });

  it("外部占用时就地给出接管入口，确认后重放刚才那次发送", async () => {
    window.history.replaceState({}, "", "/?sessionId=15");
    const history = {
      sessionId: 15, title: "外部运行中", status: "running", provider: "claude", cwd: "C:/repo",
      supported: true, offset: 0, reset: false, pendingReview: null, items: [],
    };
    let takenOver = false;
    // 确认走应用内原生小窗(invoke confirm_dialog):按队列依次给答案,不再 mock 系统 confirm。
    const confirmAnswers: boolean[] = [false, true];
    invoke.mockImplementation((command: string) => {
      if (command === "confirm_dialog") return Promise.resolve(confirmAnswers.shift() ?? false);
      if (command === "get_chat_history") return Promise.resolve(history);
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "managed_terminal_snapshot") {
        // 接管前没有托管 PTY；接管后有，且已画出可见内容。
        return Promise.resolve(takenOver
          ? { sessionId: 15, active: true, data: btoa("ready"), startOffset: 0, endOffset: 5, exited: false, exitCode: null }
          : { sessionId: 15, active: false, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      }
      // 进程是否真活着由后端按 pid 判定——前端不再靠 status 猜，而是让这次 start 被拒。
      if (command === "start_managed_terminal") return Promise.reject("会话仍在外部终端运行，不能重复接管");
      if (command === "takeover_managed_terminal") { takenOver = true; return Promise.resolve(); }
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const input = await screen.findByRole("textbox", { name: "发送消息给 Agent" }) as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: "别起第二个" } });
    fireEvent.click(screen.getByRole("button", { name: "发送" }));

    expect(await screen.findByText(/会话仍在外部终端运行/)).toBeTruthy();
    // 输入不能丢：接管后要原样重发这条。
    expect(input.value).toBe("别起第二个");
    expect(invoke).not.toHaveBeenCalledWith("write_managed_terminal", expect.anything());

    // 接管是破坏性的（杀掉外部进程），必须显式确认；取消（队列首个 false）则什么都不做。
    fireEvent.click(screen.getByRole("button", { name: "结束外部进程并接管" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("confirm_dialog", expect.anything()));
    expect(invoke).not.toHaveBeenCalledWith("takeover_managed_terminal", expect.anything());

    // 再点一次（队列次个 true）→ 确认接管。
    fireEvent.click(screen.getByRole("button", { name: "结束外部进程并接管" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("takeover_managed_terminal", { sessionId: 15, cols: 100, rows: 30 }));
    // 接管成功后自动重放刚才那次发送，用户不必重打一遍。
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 15, data: "别起第二个" }), { timeout: 3_000 });
  });

  /// Agent 自己派出的后台会话（Claude Code 的 FleetView）：它不消费 stdin，往它的 PTY 写
  /// 按键石沉大海。送话要经 Agent 守护进程的控制通道——发送必须走那条路，且**不能**碰
  /// 托管终端的任何命令（起终端、接管、写终端）。
  it("后台会话发消息走守护进程通道，不碰托管终端", async () => {
    window.history.replaceState({}, "", "/?sessionId=17");
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 17, title: "后台跑着", status: "waiting", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, background: true,
        // 有对话内容 → 停在对话页（transcript 空的后台会话会被自动切到终端页，另有用例覆盖）。
        items: [{ type: "user_text", id: "u0", timestamp: null, text: "之前的" }],
      });
      if (command === "get_pending_approval") return Promise.resolve(null);
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const input = await screen.findByRole("textbox", { name: "发送消息给 Agent" }) as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: "继续干活" } });
    fireEvent.click(screen.getByRole("button", { name: "发送" }));

    await waitFor(() => expect(invoke).toHaveBeenCalledWith("send_background_prompt", { sessionId: 17, text: "继续干活" }));
    // 守护进程的 ok 只代表收下了投递，手动模式的 worker 根本不消费——**绝不能清空输入框**。
    // 实测踩过：清掉之后用户刚打的字再也找不回来，而消息其实没被处理。
    expect(await screen.findByText(zh.chat.sendBackgroundQueued)).toBeTruthy();
    expect(input.value).toBe("继续干活");
    // 托管终端那套对它无效，一条都不该发出去。
    expect(invoke).not.toHaveBeenCalledWith("start_managed_terminal", expect.anything());
    expect(invoke).not.toHaveBeenCalledWith("write_managed_terminal", expect.anything());
    expect(invoke).not.toHaveBeenCalledWith("takeover_managed_terminal", expect.anything());
  });

  /// claude 的 fork/resume 后台 worker 有不落盘的老毛病：transcript 里只剩两行元数据，
  /// 正文只活在进程内存。对话页给不出任何东西，而终端旁路拿得到完整画面——这类会话
  /// 打开就该落在终端页，而不是让用户对着「还没有对话记录」发呆。
  it("后台会话的 transcript 为空时直接落在终端页", async () => {
    window.history.replaceState({}, "", "/?sessionId=18");
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 18, title: "没落盘", status: "running", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [], background: true,
      });
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "managed_terminal_snapshot") return Promise.resolve({
        sessionId: 18, active: true, data: btoa("screen"), startOffset: 0, endOffset: 6,
        exited: false, exitCode: null,
      });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    // 终端页是「按下」态,且画面已接上。
    await waitFor(() => expect(screen.getByRole("button", { name: "终端" }).getAttribute("aria-pressed")).toBe("true"));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("attach_background_session", { sessionId: 18 }));
    // 注：「在终端页打字会被带回对话页」那条走 xterm 的 onData，而 xterm 在 jsdom 里不渲染，
    // 这里模拟不了按键。该行为只在真机验证过，没有单测护着。
  });

  /// items 这一帧还没读到、但 hook 记着最近往来 → 内容是有的，不能甩去终端页。
  it("后台会话 items 为空但 hook 有往来时仍停在对话页", async () => {
    window.history.replaceState({}, "", "/?sessionId=20");
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 20, title: "有往来", status: "ended", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [], background: true,
        lastUserText: "hi", lastAiText: "你好",
      });
      if (command === "get_pending_approval") return Promise.resolve(null);
      return Promise.resolve();
    });
    render(<ChatWindow />);
    await screen.findByText("你好");
    expect(screen.getByRole("button", { name: "终端" }).getAttribute("aria-pressed")).toBe("false");
  });

  /// 有对话内容的后台会话照旧停在对话页——自动切终端只是「没东西可显示」时的兜底。
  it("后台会话有对话内容时仍停在对话页", async () => {
    window.history.replaceState({}, "", "/?sessionId=19");
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 19, title: "有内容", status: "running", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, background: true,
        items: [{ type: "user_text", id: "u1", timestamp: null, text: "在吗" }],
      });
      if (command === "get_pending_approval") return Promise.resolve(null);
      return Promise.resolve();
    });
    render(<ChatWindow />);
    await screen.findByText("在吗");
    expect(screen.getByRole("button", { name: "终端" }).getAttribute("aria-pressed")).toBe("false");
  });

  it("外部会话其实已经死了（status 陈旧）时，直接起托管终端而不是一律拒绝", async () => {
    window.history.replaceState({}, "", "/?sessionId=16");
    // status 仍是 running/stale，但进程早没了——后端 pid 判定会放行这次 start。
    const history = {
      sessionId: 16, title: "陈旧状态", status: "stale", provider: "claude", cwd: "C:/repo",
      supported: true, offset: 0, reset: false, pendingReview: null, items: [],
    };
    let started = false;
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve(history);
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "start_managed_terminal") { started = true; return Promise.resolve(); }
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve(started
          ? { sessionId: 16, active: true, data: btoa("ready"), startOffset: 0, endOffset: 5, exited: false, exitCode: null }
          : { sessionId: 16, active: false, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      }
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const input = await screen.findByRole("textbox", { name: "发送消息给 Agent" });
    fireEvent.change(input, { target: { value: "继续" } });
    fireEvent.click(screen.getByRole("button", { name: "发送" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("start_managed_terminal", { sessionId: 16, cols: 100, rows: 30 }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 16, data: "继续" }), { timeout: 3_000 });
    // 等回车也落地再结束本用例:正文与回车之间隔着 SUBMIT_GAP_MS,提前收工的话这次写入会在
    // 后面某个用例执行到一半时才落进共享的 invoke.mock.calls,把那边的「零副作用」断言打翻
    // (真实现场:软拦用例偶发失败在这条 sessionId 16 的 "\r" 上)。
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 16, data: "\r" }), { timeout: 3_000 });
  });

  it("keeps the prompt and reports a managed terminal that exits during startup", async () => {
    window.history.replaceState({}, "", "/?sessionId=14");
    const history = {
      sessionId: 14, title: "恢复失败", status: "ended", provider: "claude", cwd: "C:/repo",
      supported: true, offset: 0, reset: false, pendingReview: null, items: [],
    };
    let snapshotCalls = 0;
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve(history);
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "managed_terminal_snapshot") {
        snapshotCalls += 1;
        return Promise.resolve(snapshotCalls === 1
          ? { sessionId: 14, active: false, data: "", exited: false, exitCode: null }
          : { sessionId: 14, active: false, data: "launch error", exited: true, exitCode: 1 });
      }
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const input = await screen.findByRole("textbox", { name: "发送消息给 Agent" }) as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: "不要丢失" } });
    fireEvent.click(screen.getByRole("button", { name: "发送" }));
    expect(await screen.findByText(/Agent 启动后立即退出（退出码 1）/)).toBeTruthy();
    expect(input.value).toBe("不要丢失");
    expect(invoke).not.toHaveBeenCalledWith("write_managed_terminal", expect.objectContaining({ sessionId: 14 }));
  });

  it("shows Claude's native command approval for an already-managed PTY", async () => {
    window.history.replaceState({}, "", "/?sessionId=45");
    const prompt = [
      "\x1b[2JBash command",
      "cargo build -p meowo-agent -p meowo-store 2>&1 | tail -20",
      "Build rust crates",
      "This command requires approval",
      "Do you want to proceed?",
      "> 1. Yes",
      "  2. Yes, and don't ask again for: cargo build *",
      "  3. No",
    ].join("\r\n");
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 45, title: "托管命令审批", status: "waiting", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: "approval", items: [],
      });
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "managed_terminal_snapshot") return Promise.resolve({
        sessionId: 45, active: true, data: btoa(prompt), startOffset: 0, endOffset: prompt.length,
        exited: false, exitCode: null,
      });
      return Promise.resolve();
    });
    render(<ChatWindow />);

    expect(await screen.findByRole("button", { name: "允许一次" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "允许并记住 · cargo build *" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "拒绝" })).toBeTruthy();
    expect(screen.getByText(/cargo build -p meowo-agent/)).toBeTruthy();
    expect(screen.getByText("Do you want to proceed?")).toBeTruthy();
    expect(screen.queryByText("该请求来自非托管会话，请在原终端中处理")).toBeNull();
  });

  /**
   * claude 的拒绝项存在长文案形态(「No, and tell Claude what to do differently (esc)」)。
   * 回归:全等匹配 /^(?:no|reject)$/ 认不出它——拒绝按钮和 Esc 快捷键无声消失,
   * 且该项被「既非拒绝也非允许的第一项」规则吸收成「允许并记住」,点持久放行实际发拒绝。
   */
  it("claude 长文案拒绝项仍归类为拒绝,不冒充「允许并记住」", async () => {
    window.history.replaceState({}, "", "/?sessionId=49");
    const prompt = [
      "\x1b[2JBash command",
      "rm -rf build",
      "Clean build output",
      "This command requires approval",
      "Do you want to proceed?",
      "> 1. Yes",
      "  2. No, and tell Claude what to do differently (esc)",
    ].join("\r\n");
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 49, title: "长文案拒绝", status: "waiting", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: "approval", items: [],
      });
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "managed_terminal_snapshot") return Promise.resolve({
        sessionId: 49, active: true, data: btoa(prompt), startOffset: 0, endOffset: prompt.length,
        exited: false, exitCode: null,
      });
      return Promise.resolve();
    });
    render(<ChatWindow />);

    expect(await screen.findByRole("button", { name: "允许一次" })).toBeTruthy();
    // 拒绝按钮在,且没有任何选项被误认成「允许并记住」。
    expect(screen.getByRole("button", { name: "拒绝" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /允许并记住/ })).toBeNull();
  });

  /**
   * TUI 重绘残留:审批框弹出前 claude 已经刷过一屏工具输出,上一次的审批框也还留在
   * 缓冲里。此前识别窗口从**第一处**签名起截,于是「Do you want to proceed?」在卡上
   * 显示两遍(description 一遍、question 一遍),命令区里还混着上一轮的搜索结果——
   * 真机截图为证。窗口改取最后一处签名,解析再把重复问句和铺行的框线横杠清掉。
   */
  it("重绘残留:审批问句只显示一遍,命令区不夹带上一屏输出", async () => {
    window.history.replaceState({}, "", "/?sessionId=46");
    const prompt = [
      "\x1b[2JSearched for 3 patterns, ran 5 shell commands",
      "● Web Search(\"codex tui approval prompt not displayed\")",
      "  └ Did 1 search in 8s",
      "Do you want to proceed? ──────────────────────────────",
      "> 1. Yes",
      "  2. No",
      "Tool use",
      "Bash command",
      "lark-cli attendance query --employee-type employee_id",
      "查询打卡记录",
      "This command requires approval",
      "Do you want to proceed? ──────────────────────────────",
      "> 1. Yes",
      "  2. Yes, and don't ask again for: lark-cli *",
      "  3. No",
    ].join("\r\n");
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 46, title: "重绘残留", status: "waiting", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: "approval", items: [],
      });
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "managed_terminal_snapshot") return Promise.resolve({
        sessionId: 46, active: true, data: btoa(unescape(encodeURIComponent(prompt))), startOffset: 0, endOffset: prompt.length,
        exited: false, exitCode: null,
      });
      return Promise.resolve();
    });
    render(<ChatWindow />);

    expect(await screen.findByRole("button", { name: "允许一次" })).toBeTruthy();
    // 问句只出现一次,且尾部铺行的框线横杠已剥掉。
    expect(screen.getAllByText("Do you want to proceed?")).toHaveLength(1);
    expect(screen.queryByText(/Do you want to proceed\? ─/)).toBeNull();
    // 命令区认得出这次的命令,不夹带上一屏的搜索输出。
    expect(screen.getByText(/lark-cli attendance query/)).toBeTruthy();
    expect(screen.queryByText(/Web Search/)).toBeNull();
    expect(screen.queryByText(/Searched for 3 patterns/)).toBeNull();
  });

  it("shows Kimi's approval panel as actionable GUI choices (digit keys)", async () => {
    window.history.replaceState({}, "", "/?sessionId=47");
    const prompt = [
      "\x1b[2J  ▶ Run this command?",
      "  $ echo meowo-approval-probe-47",
      "  Run the probe command",
      "  ▶ 1. Approve once",
      "    2. Approve for this session",
      "    3. Reject",
      "    4. Reject with feedback",
      "  ↑/↓ select · 1/2/3/4 choose · ↵ confirm",
    ].join("\r\n");
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 47, title: "kimi 审批", status: "waiting", provider: "kimi", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: "approval", items: [],
      });
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("kimi"));
      if (command === "managed_terminal_snapshot") return Promise.resolve({
        sessionId: 47, active: true,
        // 面板含 ▶/↑/↓ 等非 Latin1 字符,btoa 不能直接吃 Unicode 字符串,先走 UTF-8 字节。
        data: btoa(String.fromCharCode(...new TextEncoder().encode(prompt))),
        startOffset: 0, endOffset: prompt.length, exited: false, exitCode: null,
      });
      return Promise.resolve();
    });
    render(<ChatWindow />);

    // 识别成可操作审批卡：三个能直接完成的选项（feedback 项不收），命令原文在详情里。
    expect(await screen.findByRole("button", { name: "允许一次" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "本会话内允许" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "拒绝" })).toBeTruthy();
    expect(screen.getByText("Run this command?")).toBeTruthy();
    expect(screen.getByText(/echo meowo-approval-probe-47/)).toBeTruthy();
    // 不再是「只能去终端处理」的降级卡。
    expect(screen.queryByText("Meowo 正在从托管终端读取 Agent 的选项…")).toBeNull();
    // 按钮 = 直接向 PTY 打数字键（kimi 面板数字直选，官方源码 selectAndSubmit）。
    fireEvent.click(screen.getByRole("button", { name: "本会话内允许" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 47, data: "2" }));
  });

  it("shows a managed multi-select question without requiring a Terminal tab visit", async () => {
    window.history.replaceState({}, "", "/?sessionId=46");
    const prompt = [
      "\x1b[2JWhich items should I continue with?",
      "> 1. [ ] First-screen tail reading",
      "  2. [ ] Connection pooling",
      "  3. [ ] Keep the current state",
      "  4. [ ] Type something",
      "Submit",
      "Enter to select · up/down to navigate · Esc to cancel",
    ].join("\r\n");
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 46, title: "托管问答", status: "waiting", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: "question", items: [],
      });
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "managed_terminal_snapshot") return Promise.resolve({
        sessionId: 46, active: true, data: btoa(prompt), startOffset: 0, endOffset: prompt.length,
        exited: false, exitCode: null,
      });
      return Promise.resolve();
    });
    render(<ChatWindow />);

    expect(await screen.findByText("Agent 正在等待你的回答")).toBeTruthy();
    expect(screen.getByText(/Which items should I continue with/)).toBeTruthy();
    const firstChoice = screen.getByRole("button", { name: "First-screen tail reading" });
    expect(firstChoice).toBeTruthy();
    expect(screen.getByPlaceholderText("输入其他回答")).toBeTruthy();
    expect(screen.getByRole("button", { name: "提交选择" })).toBeTruthy();
    fireEvent.click(firstChoice);
    expect(firstChoice.className).toContain("is-selected");
    expect(invoke).toHaveBeenCalledWith("write_managed_terminal", {
      sessionId: 46, data: "\r",
    });
    expect(screen.queryByRole("button", { name: "上一项 ↑" })).toBeNull();
    expect(screen.queryByText("Meowo 正在从托管终端读取 Agent 的选项…")).toBeNull();
  });

  /**
   * 「仅收起」:识别是启发式的,误报/过期的卡必须有不写 PTY 的零副作用出口——
   * 此前唯一的关闭方式都要发按键(\r/Esc),Esc 还会打断正在跑的回合。
   */
  it("交互卡可仅收起:不向终端写任何字节,输入框恢复", async () => {
    window.history.replaceState({}, "", "/?sessionId=47");
    const prompt = [
      "\x1b[2JWhich items should I continue with?",
      "> 1. [ ] First-screen tail reading",
      "  2. [ ] Connection pooling",
      "Submit",
      "Enter to select · up/down to navigate · Esc to cancel",
    ].join("\r\n");
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 47, title: "误报收起", status: "waiting", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: "question", items: [],
      });
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "managed_terminal_snapshot") return Promise.resolve({
        sessionId: 47, active: true, data: btoa(prompt), startOffset: 0, endOffset: prompt.length,
        exited: false, exitCode: null,
      });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    expect(await screen.findByText("Agent 正在等待你的回答")).toBeTruthy();
    // 只数**本会话**的写入:invoke 的调用记录是全测试文件共享的,别的用例卸载后仍在飞的
    // 异步写入会迟到落进来(见 sessionId 16 那个用例的收尾注释)。
    const writes = () => invoke.mock.calls.filter(
      ([command, args]) => command === "write_managed_terminal" && (args as { sessionId: number }).sessionId === 47,
    ).length;
    const writesBefore = writes();
    fireEvent.click(screen.getByRole("button", { name: "仅收起" }));
    // 零副作用断言必须**同步**做(点击处理器只 setTerminalAttention(null),纯同步):
    // 放到下面的 await/waitFor 之后,慢机(macOS CI)上后台 timer 会在那段窗口里插入一次
    // 无关写入,把「零副作用」误判成失败。点击后当场查,timer 还没机会触发。
    expect(writes()).toBe(writesBefore);
    await waitFor(() => expect(screen.queryByText(/Which items should I continue with/)).toBeNull());
  });

  /**
   * 软拦:hook 说 agent 在等交互(pendingReview)但屏幕识别没认出卡片(未覆盖的提示
   * 形态)——直接发送会把正文+回车打进看不见的选择器。此时要一次明确知情的确认,
   * 拒绝则不写终端;确认后照常发送。
   */
  it("pendingReview 未识别成卡片时,发送先弹确认", async () => {
    window.history.replaceState({}, "", "/?sessionId=48");
    const confirmAnswers: boolean[] = [false, true];
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 48, title: "未识别提示", status: "waiting", provider: "codex", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: "question", items: [],
      });
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("codex"));
      if (command === "confirm_dialog") return Promise.resolve(confirmAnswers.shift() ?? false);
      if (command === "managed_terminal_snapshot") return Promise.resolve({
        // 屏幕上是识别不出的提示形态(比如 codex 自家的选择器),没有任何卡片。
        sessionId: 48, active: true, data: btoa("\x1b[2Jsome unrecognized picker"), startOffset: 0, endOffset: 28,
        exited: false, exitCode: null,
      });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const input = await screen.findByRole("textbox", { name: "发送消息给 Agent" });
    fireEvent.change(input, { target: { value: "继续" } });

    // 拒绝(队列首个 false):软拦确认返回 false → 不向终端写任何内容。
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("confirm_dialog", expect.anything()));
    // 只看**本会话**的写入:invoke 的调用记录是全测试文件共享的,别的用例卸载后仍在飞的
    // 异步写入(正文与回车之间隔着 SUBMIT_GAP_MS)会迟到落进来,不限定 sessionId 就会把
    // 那些无关写入算到这里头上。
    expect(invoke.mock.calls.some(
      ([command, args]) => command === "write_managed_terminal" && (args as { sessionId: number }).sessionId === 48,
    )).toBe(false);
    // 等第一次发送的异步守卫彻底收尾(sending→false,按钮从「发送中…」回到「发送」),
    // 否则慢机上(macOS CI)第二次 Enter 会撞进 sending 守卫被吞掉,等不到下面的 write。
    await waitFor(() => expect(screen.getByRole("button", { name: "发送" })).toBeTruthy());

    // 确认(队列次个 true):照常发送正文。
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 48, data: "继续" }), { timeout: 3_000 });
  });

  it("renders and resolves a managed permission request", async () => {
    window.history.replaceState({}, "", "/?sessionId=12");
    const history = {
      sessionId: 12, title: "审批", status: "running", provider: "codex", cwd: "C:/repo",
      supported: true, offset: 0, reset: false, pendingReview: "approval", items: [],
    };
    let pending: unknown = {
      sessionId: 12, requestId: "request-1", provider: "codex", toolName: "Bash",
      description: "运行测试", input: "{\"command\":\"cargo test\"}",
      permissionSuggestions: [{
        type: "addRules", behavior: "allow", destination: "localSettings",
        rules: [{ toolName: "Bash", ruleContent: "cargo test" }],
      }],
    };
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve(history);
      if (command === "get_pending_approval") return Promise.resolve(pending);
      if (command === "resolve_pending_approval") { pending = null; return Promise.resolve(); }
      return Promise.resolve();
    });
    render(<ChatWindow />);
    expect(await screen.findByText("运行测试")).toBeTruthy();
    expect(screen.getByText("{\"command\":\"cargo test\"}")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /^允许并记住（此项目、本机）/ }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("resolve_pending_approval", {
      sessionId: 12, requestId: "request-1", choice: "suggestion:0",
    }));
    await waitFor(() => expect(screen.queryByRole("button", { name: "允许一次" })).toBeNull());
    expect(screen.queryByText("该请求来自非托管会话，请在原终端中处理")).toBeNull();
  });

  /**
   * 审批卡上的 `Esc` 徽章必须说话算话——键真绑上,落点是「拒绝」。
   *
   * 只绑这一个方向:Enter→允许**故意没绑**。输入框外随手一个回车就把执行权交出去,
   * 换不来那点快捷。焦点在输入框里时整条快捷键让开,那里的 Esc 另有主人(收补全菜单)。
   */
  it("审批卡:Esc 落到拒绝,且不抢输入框里的 Esc", async () => {
    window.history.replaceState({}, "", "/?sessionId=13");
    let pending: unknown = {
      sessionId: 13, requestId: "request-esc", provider: "claude", toolName: "Bash",
      description: "查打卡记录", input: "lark-cli attendance query", permissionSuggestions: [],
    };
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 13, title: "Esc 拒绝", status: "running", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: "approval", items: [], connected: true,
      });
      if (command === "get_pending_approval") return Promise.resolve(pending);
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "resolve_pending_approval") { pending = null; return Promise.resolve(); }
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const deny = await screen.findByRole("button", { name: "拒绝" });
    // kbd 徽章是给眼睛的,不进可访问名——上面按 "拒绝" 精确找得到就是证据。
    expect(deny.textContent).toContain("Esc");

    // 焦点在输入框里:这一下归补全菜单/输入框,审批卡不许截走。
    const input = await screen.findByRole("textbox", { name: "发送消息给 Agent" });
    input.focus();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(invoke.mock.calls.some(([command]) => command === "resolve_pending_approval")).toBe(false);

    input.blur();
    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("resolve_pending_approval", {
      sessionId: 13, requestId: "request-esc", choice: "deny",
    }));
  });

  /**
   * 回归：负载缺 `permissionSuggestions` 时审批条照常渲染，不许崩整窗。
   *
   * 类型上该字段恒在（DTO 保证），但真实世界里出现过缺席：后端曾直接 emit 原始
   * `ApprovalRequest`（空列表被 `skip_serializing_if` 略去），codex 的审批一弹，
   * ChatWindow 就死在 `.map` 上（TypeError: Cannot read properties of undefined）。
   * 后端已改走 DTO；这里钉住前端的 `?? []` 防御，堵旧后端/新前端错配的同一条死路。
   */
  it("survives an approval payload that lacks permissionSuggestions", async () => {
    window.history.replaceState({}, "", "/?sessionId=12");
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 12, title: "审批", status: "running", provider: "codex", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: "approval", items: [],
      });
      if (command === "get_pending_approval") return Promise.resolve({
        sessionId: 12, requestId: "request-lean", provider: "codex", toolName: "Bash",
        description: "运行测试", input: "{\"command\":\"cargo test\"}",
        // 刻意没有 permissionSuggestions —— 模拟被 skip 掉字段的瘦负载。
      });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    // 审批条正常出现：允许/拒绝都在，只是没有「记住」类按钮。
    expect(await screen.findByRole("button", { name: "允许一次" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "拒绝" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /允许并记住/ })).toBeNull();
  });

  /**
   * 回归：用户在会话 A 输入时，会话 B 的授权请求**不许**把窗口切到 B（旧行为：后端
   * emit chat-session-changed + set_focus，草稿被收走、焦点被抢）。现在后端只闪任务栏，
   * 前端的职责是：A 的画面纹丝不动，B 在侧栏亮琥珀徽标，cleared 后徽标回落状态点。
   */
  it("别的会话请求授权:不切会话不弹卡,只在侧栏亮徽标,清除后回落", async () => {
    window.history.replaceState({}, "", "/?sessionId=12");
    const liveSession = (id: number, title: string) => ({
      session: { id, cc_session_id: `cc-${id}`, status: "running" },
      project_name: "meowo", task_title: title, connected: true,
      pending_review: null, cwd: "C:/repo", provider: "claude",
    });
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 12, title: "当前会话", status: "running", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [], connected: true,
      });
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "managed_terminal_binding") return Promise.resolve(null);
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 12, active: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      if (command === "get_live_sessions_page") return Promise.resolve({
        items: [liveSession(12, "当前会话"), liveSession(13, "另一条会话")],
        next_cursor: null,
      });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const otherDot = () => screen.getByRole("button", { name: /另一条会话/ }).querySelector(".chat-sidebar-dot");
    await waitFor(() => expect(otherDot()?.className).toContain("is-running"));

    const payload = {
      sessionId: 13, requestId: "request-bg", provider: "claude", toolName: "Bash",
      description: "跑构建", input: "{}", permissionSuggestions: [],
    };
    act(() => { eventListeners.get("pending-approval")?.({ payload }); });
    // 徽标亮起、压过 running 点；当前会话的画面不受任何打扰。
    await waitFor(() => expect(otherDot()?.className).toContain("is-approval"));
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.queryByText("跑构建")).toBeNull();

    act(() => { eventListeners.get("pending-approval-cleared")?.({ payload }); });
    await waitFor(() => expect(otherDot()?.className).toContain("is-running"));
  });

  /**
   * AskUserQuestion 自动放行后的题面直达：interactive-question 事件携带结构化题面，
   * 选择列表卡与终端表单同步渲染（问题/选项/描述来自 hook 参数 JSON，不等屏幕识别
   * 反推），且**不再**出现允许/拒绝的审批按钮——提问不是权限，broker 已经放行了。
   */
  it("interactive-question 题面卡从结构化参数同步渲染,不再是审批卡", async () => {
    window.history.replaceState({}, "", "/?sessionId=31");
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 31, title: "提问会话", status: "running", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [], connected: true,
        ptyManaged: true, // 托管会话才开放点选排队
      });
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "managed_terminal_binding") return Promise.resolve(null);
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 31, active: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    await waitFor(() => expect(screen.getByText("提问会话")).toBeTruthy());

    const payload = {
      sessionId: 31, requestId: "request-question", provider: "claude", toolName: "AskUserQuestion",
      description: null,
      input: JSON.stringify({
        questions: [{
          header: "仓库名", question: "新仓库叫什么名字？", multiSelect: false,
          options: [{ label: "autopilot-v2", description: "沿用产品名" }, { label: "autopilot-core" }],
        }],
      }),
      permissionSuggestions: [],
    };
    act(() => { eventListeners.get("interactive-question")?.({ payload }); });
    // 题面立即可见：问题、选项与描述。
    expect(await screen.findByText(/新仓库叫什么名字/)).toBeTruthy();
    expect(screen.getByText("autopilot-v2")).toBeTruthy();
    expect(screen.getByText("沿用产品名")).toBeTruthy();
    expect(screen.getByRole("button", { name: "去终端作答" })).toBeTruthy();
    // 不是审批卡：没有允许/拒绝，也没有原始 JSON 参数。
    expect(screen.queryByRole("button", { name: "允许一次" })).toBeNull();
    expect(screen.queryByRole("button", { name: "拒绝" })).toBeNull();
    expect(screen.queryByText(/"questions"/)).toBeNull();
    // 点选即排队：提示切换为「已选…」，等屏幕识别确认表单在屏后才自动落键，
    // 不在此刻向 PTY 写任何字节。再点一次取消排队。
    fireEvent.click(screen.getByRole("button", { name: /autopilot-v2/ }));
    expect(screen.getByText("已选「autopilot-v2」，表单就绪后自动作答")).toBeTruthy();
    expect(invoke).not.toHaveBeenCalledWith("write_managed_terminal", expect.anything());
    fireEvent.click(screen.getByRole("button", { name: /autopilot-v2/ }));
    expect(screen.queryByText(/已选「/)).toBeNull();
  });

  /**
   * 外部终端会话（GUI 不持有 PTY）：题面纯展示——选项不是按钮（点了也写不进按键，
   * 不承诺做不到的事），提示改为「回终端作答」，「去终端作答」按钮也不给（终端页
   * 对外部会话是空的）。
   */
  it("外部会话的题面卡纯展示:选项不可点,提示回终端作答", async () => {
    window.history.replaceState({}, "", "/?sessionId=32");
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 32, title: "外部提问", status: "running", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [], connected: true,
        ptyManaged: false,
      });
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "managed_terminal_binding") return Promise.resolve(null);
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 32, active: false, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    await waitFor(() => expect(screen.getByText("外部提问")).toBeTruthy());
    const payload = {
      sessionId: 32, requestId: "request-question-ext", provider: "claude", toolName: "AskUserQuestion",
      description: null,
      input: JSON.stringify({ questions: [{ question: "晚饭吃什么？", multiSelect: false, options: [{ label: "火锅" }, { label: "寿司" }] }] }),
      permissionSuggestions: [],
    };
    act(() => { eventListeners.get("interactive-question")?.({ payload }); });
    expect(await screen.findByText(/晚饭吃什么/)).toBeTruthy();
    expect(screen.getByText("火锅")).toBeTruthy();
    expect(screen.queryByRole("button", { name: /火锅/ })).toBeNull();
    expect(screen.queryByRole("button", { name: "去终端作答" })).toBeNull();
    expect(screen.getByText(/请回到那个终端作答/)).toBeTruthy();
  });

  /**
   * 附件注入按 agent 能力分流:声明了 attachment_mention(claude/gemini,实测 @绝对路径
   * 在提交时被原生附加)就用 `@路径` 提及;图片或含空白的路径退回指令文本——前者经
   * @提及不产生图像块,后者的提及会在空白处截断。
   */
  it("claude 附件走原生 @提及,图片退回指令文本兜底", async () => {
    window.history.replaceState({}, "", "/?sessionId=21");
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 21, title: "附件注入", status: "waiting", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [], connected: true,
      });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "managed_terminal_binding") return Promise.resolve(null);
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 21, active: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    await waitFor(() => expect(screen.getByText("附件注入")).toBeTruthy());
    const box = () => screen.getByRole("textbox", { name: "发送消息给 Agent" });

    // 纯文本文件 + 无空白路径 → 原生 @提及,不再有指令文本。
    openDialog.mockResolvedValueOnce(["C:\\repo\\notes.txt"]);
    fireEvent.click(screen.getByRole("button", { name: "添加图片或文件" }));
    await waitFor(() => expect(screen.getByText("notes.txt")).toBeTruthy());
    fireEvent.change(box(), { target: { value: "看看这个" } });
    fireEvent.keyDown(box(), { key: "Enter" });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 21, data: "@C:\\repo\\notes.txt 看看这个" }));
    // 等提交回车落地(SUBMIT_GAP_MS 之后),sending 才复位,下一次发送才不会被守卫吞掉。
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 21, data: "\r" }));

    // 图片 → 退回指令文本(经 @提及不会作为图像块附加)。
    openDialog.mockResolvedValueOnce(["C:\\repo\\shot.png"]);
    fireEvent.click(screen.getByRole("button", { name: "添加图片或文件" }));
    await waitFor(() => expect(screen.getByText("shot.png")).toBeTruthy());
    fireEvent.change(box(), { target: { value: "看图" } });
    fireEvent.keyDown(box(), { key: "Enter" });
    // 多行文本经括号粘贴包裹送入 PTY(\x1b[200~…\x1b[201~),指令文本恒多行,断言带包裹。
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", {
      sessionId: 21,
      data: `[200~看图\n\n请读取并结合以下本地附件完成任务（图片请使用图像读取能力）：\n- C:\\repo\\shot.png[201~`,
    }));
  });

  /**
   * 剪贴板原生图片附加:粘贴图片且发送时剪贴板指纹未变 → 向 PTY 发 Ctrl-V 让 TUI 自己
   * 读剪贴板(claude 原生 [Image #N]),屏幕上确认占位符后写正文提交,全程不出现指令文本。
   */
  it("粘贴图片且剪贴板未变:Ctrl-V 原生附加,占位符确认后写正文", async () => {
    window.history.replaceState({}, "", "/?sessionId=22");
    let pasted = false;
    invoke.mockImplementation((command: string, args?: Record<string, unknown>) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 22, title: "原生图", status: "waiting", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [], connected: true,
      });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "managed_terminal_binding") return Promise.resolve(null);
      if (command === "save_pasted_attachment") return Promise.resolve("C:\\Temp\\meowo-paste\\1-0\\image.png");
      if (command === "clipboard_image_fingerprint") return Promise.resolve("fp-1");
      if (command === "write_managed_terminal" && args?.data === "\x16") {
        pasted = true;
        return Promise.resolve();
      }
      if (command === "managed_terminal_snapshot") {
        // ^V 之后屏幕出现 claude 的原生占位符;mock 无状态,重复返回同段增量无妨。
        const data = pasted ? btoa("> [Image #1]") : "";
        return Promise.resolve({ sessionId: 22, active: true, data, startOffset: 0, endOffset: pasted ? 12 : 0, exited: false, exitCode: null });
      }
      return Promise.resolve();
    });
    render(<ChatWindow />);
    await waitFor(() => expect(screen.getByText("原生图")).toBeTruthy());
    const box = () => screen.getByRole("textbox", { name: "发送消息给 Agent" });
    fireEvent.paste(box(), { clipboardData: { files: [new File([new Uint8Array([137, 80])], "image.png", { type: "image/png" })] } });
    await waitFor(() => expect(screen.getByText("image.png")).toBeTruthy());
    fireEvent.change(box(), { target: { value: "看这张图" } });
    fireEvent.keyDown(box(), { key: "Enter" });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 22, data: "\x16" }));
    // 占位符确认(首个 250ms 轮询)+ SUBMIT_GAP 后正文与回车相继写入。
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 22, data: "看这张图" }), { timeout: 3000 });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 22, data: "\r" }), { timeout: 3000 });
    // 全程没有指令文本注入。
    const writes = invoke.mock.calls.filter((call) => call[0] === "write_managed_terminal");
    expect(writes.every((call) => !String((call[1] as { data: string }).data).includes("请读取并结合"))).toBe(true);
  });

  /** 发送时剪贴板已被复制成别的内容(指纹不匹配):不发 Ctrl-V,退回指令文本。 */
  it("剪贴板指纹不匹配时退回指令文本,不发 Ctrl-V", async () => {
    window.history.replaceState({}, "", "/?sessionId=23");
    let fingerprint = "fp-1";
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 23, title: "指纹变了", status: "waiting", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [], connected: true,
      });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "managed_terminal_binding") return Promise.resolve(null);
      if (command === "save_pasted_attachment") return Promise.resolve("C:\\Temp\\meowo-paste\\2-0\\image.png");
      if (command === "clipboard_image_fingerprint") return Promise.resolve(fingerprint);
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 23, active: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    await waitFor(() => expect(screen.getByText("指纹变了")).toBeTruthy());
    const box = () => screen.getByRole("textbox", { name: "发送消息给 Agent" });
    fireEvent.paste(box(), { clipboardData: { files: [new File([new Uint8Array([1])], "image.png", { type: "image/png" })] } });
    await waitFor(() => expect(screen.getByText("image.png")).toBeTruthy());
    fingerprint = "fp-2"; // 用户中途复制了别的东西
    fireEvent.change(box(), { target: { value: "看图" } });
    fireEvent.keyDown(box(), { key: "Enter" });
    // 发送链上多了一次指纹比对的异步往返,写入落地晚于 waitFor 默认 1s,放宽超时。
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", {
      sessionId: 23,
      data: `[200~看图\n\n请读取并结合以下本地附件完成任务（图片请使用图像读取能力）：\n- C:\\Temp\\meowo-paste\\2-0\\image.png[201~`,
    }), { timeout: 3000 });
    expect(invoke.mock.calls.some((call) => call[0] === "write_managed_terminal" && (call[1] as { data: string }).data === "\x16")).toBe(false);
  });

  /**
   * 假运行中校正：DB 的 running 在进程死后、reaper 收尾前是滞留值。connected:false 且
   * 托管 PTY 也不活时,运行指示一律不显示——标题栏徽标退到「未连接」,运行条不渲染。
   */
  it("进程已死的 running 会话不显示运行指示", async () => {
    window.history.replaceState({}, "", "/?sessionId=90");
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 90, title: "假运行", status: "running", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [], connected: false,
      });
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "managed_terminal_binding") return Promise.resolve(null);
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 90, active: false, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    await waitFor(() => expect(screen.getByText("假运行")).toBeTruthy());
    expect(screen.getByText("未连接")).toBeTruthy();
    expect(screen.queryByText("运行中")).toBeNull();
    expect(document.querySelector(".chat-running")).toBeNull();
    // 窗口标题不带运行记号。
    await waitFor(() => expect(setTitleMock).toHaveBeenCalledWith("假运行 · Meowo"));
  });

  /**
   * errored 会话(transcript 分析口径,ChatHistoryDto.errored):标题栏徽标显示错误态,
   * 优先级压过 status=running——与侧栏/贴纸同口径,不再「贴纸报错、对话窗亮绿灯」。
   */
  it("errored 会话标题栏徽标显示出错", async () => {
    window.history.replaceState({}, "", "/?sessionId=91");
    respondWithHistory({
      sessionId: 91, title: "翻车会话", status: "running", provider: "claude", cwd: "C:/repo",
      supported: true, offset: 0, reset: false, pendingReview: null, items: [], errored: true,
    });
    render(<ChatWindow />);
    await waitFor(() => expect(screen.getByText("翻车会话")).toBeTruthy());
    expect(screen.getByText("出错了")).toBeTruthy();
    expect(document.querySelector(".chat-live.is-error")).toBeTruthy();
    expect(screen.queryByText("运行中")).toBeNull();
    // 非本 GUI 托管的会话(ptyManaged 缺省/false)不显示「结束会话」入口。
    expect(screen.queryByText("结束会话")).toBeNull();
  });

  /**
   * 运行中插话:CLI 把回合中收到的消息排队到回合结束,期间 transcript 不显示——
   * GUI 必须给排队回执,否则消息像消失了。中断键有声明(claude)时提供「立即插话」,
   * 它只发中断键(队列由 CLI 自己接着处理);回合结束回执自动消解。
   */
  it("运行中插话显示排队回执,可立即插话,回合结束自动消解", async () => {
    window.history.replaceState({}, "", "/?sessionId=93");
    let current: Record<string, unknown> = {
      sessionId: 93, title: "排队回执", status: "running", provider: "claude", cwd: "C:/repo",
      supported: true, offset: 0, reset: false, pendingReview: null, items: [], connected: true,
    };
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve(current);
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 93, active: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const input = await screen.findByRole("textbox", { name: "发送消息给 Agent" });
    fireEvent.change(input, { target: { value: "插一句" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 93, data: "插一句" }), { timeout: 3_000 });
    expect(await screen.findByText("1 条插话已排队,当前回合结束后处理")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "立即插话" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 93, data: "\u001b" }));

    current = { ...current, status: "waiting" };
    await waitFor(() => expect(screen.queryByText(/插话已排队/)).toBeNull(), { timeout: 3_000 });
  });

  /**
   * 插话打断当前回合被 CLI 立即处理时,tone 不离开 running(打断后无缝进入新回合),
   * 回执只能靠「消息出现在 transcript」消解。落盘文本带 TUI 的 [Image #N] 占位前缀与
   * 附件指令,与用户原文不全等——匹配必须是归一化后的包含关系,否则回执永远挂着。
   */
  it("插话被立即处理:transcript 出现带前缀的落盘文本即消解回执", async () => {
    window.history.replaceState({}, "", "/?sessionId=95");
    let current: Record<string, unknown> = {
      sessionId: 95, title: "插话消解", status: "running", provider: "claude", cwd: "C:/repo",
      supported: true, offset: 0, reset: false, pendingReview: null, items: [], connected: true,
    };
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve(current);
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 95, active: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const input = await screen.findByRole("textbox", { name: "发送消息给 Agent" });
    fireEvent.change(input, { target: { value: "插一句" } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(await screen.findByText("1 条插话已排队,当前回合结束后处理")).toBeTruthy();

    current = {
      ...current,
      offset: 10,
      items: [{ type: "user_text", id: "u9", timestamp: null, text: "[Image #1] 插一句\n请读取并结合以下本地附件完成任务（图片请使用图像读取能力）：\n- C:\\tmp\\x.png" }],
    };
    await waitFor(() => expect(screen.queryByText(/插话已排队/)).toBeNull(), { timeout: 3_000 });
    // 会话仍在运行(新回合),消解不靠回合结束。
    expect(screen.queryByText("运行中")).toBeTruthy();
  });

  /**
   * 消解水位线:「包含」匹配对短插话(ok/继续)常态性子串命中**上一回合**的旧消息。
   * 回归:旧的无水位线消解在下一次轮询就收走回执,而消息还在 CLI 队列里——复现了
   * 回执本要防止的「我的消息不见了」。只有入队之后才出现的证据才可消解。
   */
  it("短插话不被上一回合旧消息子串命中,新证据到来才消解", async () => {
    window.history.replaceState({}, "", "/?sessionId=97");
    let current: Record<string, unknown> = {
      sessionId: 97, title: "水位线", status: "running", provider: "claude", cwd: "C:/repo",
      supported: true, offset: 0, reset: false, pendingReview: null, connected: true,
      // 上一回合的旧消息(lastUserText 与 transcript 各一份)都包含「ok」这个子串。
      lastUserText: "ok, run the tests",
      items: [{ type: "user_text", id: "u1", timestamp: null, text: "ok, run the tests" }],
    };
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve(current);
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 97, active: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const input = await screen.findByRole("textbox", { name: "发送消息给 Agent" });
    // 等旧消息上屏,确保入队时快照里已含旧证据。
    await screen.findByText("ok, run the tests");
    fireEvent.change(input, { target: { value: "ok" } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(await screen.findByText("1 条插话已排队,当前回合结束后处理")).toBeTruthy();
    // 多轮轮询过去,旧证据不变:回执必须还在。
    await new Promise((resolve) => setTimeout(resolve, 1_500));
    expect(screen.queryByText(/插话已排队/)).toBeTruthy();
    // 新证据落盘(lastUserText 变成插话本身):这才消解。
    current = { ...current, lastUserText: "ok" };
    await waitFor(() => expect(screen.queryByText(/插话已排队/)).toBeNull(), { timeout: 3_000 });
  });

  /**
   * 打断并发送已从 composer 上的常驻按钮降成 Ctrl+Enter：它和右边的圆钮都是「把话递
   * 出去」的入口，并排摆着只会让人先停下来分辨该按哪个。功能本身不能丢——顺序仍是
   * 先写中断键、再提交正文。
   */
  it("打断并发送(Ctrl+Enter):先写中断键(Esc)再提交正文", async () => {
    window.history.replaceState({}, "", "/?sessionId=94");
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 94, title: "强制插话", status: "running", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [], connected: true,
      });
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 94, active: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const input = await screen.findByRole("textbox", { name: "发送消息给 Agent" });
    fireEvent.change(input, { target: { value: "改用另一种方案" } });
    // composer 上不该再有第二个「发送」入口:一颗圆钮 + 一个快捷键,没有常驻文字按钮。
    expect(screen.queryByRole("button", { name: "打断并发送" })).toBeNull();
    fireEvent.keyDown(input, { key: "Enter", ctrlKey: true });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 94, data: "\u001b" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 94, data: "改用另一种方案" }), { timeout: 3_000 });
  });

  /**
   * 裸中断:发出去的消息撤不回,能叫停当前回合的只有中断键——它不该以「输入框里先写点
   * 什么」为前提。回合运行中且输入框为空时,composer 右下角那颗主圆钮**原地**变成停止键
   * (同一个 .chat-send-button,不是旁边多出来的第二个按钮),只写 Esc,不把任何正文打进
   * PTY;一开始打字就换回「发送」。
   */
  it("运行中无草稿:主圆钮变停止键,点击只发中断键", async () => {
    window.history.replaceState({}, "", "/?sessionId=96");
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 96, title: "裸中断", status: "running", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [], connected: true,
      });
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 96, active: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const button = await screen.findByRole("button", { name: "中断" });
    // 就是那颗发送圆钮本人:换了身份,没换位置,也没在旁边多长一个。
    expect(button.className).toContain("chat-send-button");
    expect(screen.queryByRole("button", { name: "发送" })).toBeNull();
    expect(screen.queryByRole("button", { name: "打断并发送" })).toBeNull();
    // 停止键不能是 disabled 的:输入框空着正是它唯一能用的时候。
    expect((button as HTMLButtonElement).disabled).toBe(false);
    fireEvent.click(button);
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 96, data: "\u001b" }));
    const writes = invoke.mock.calls.filter(([command]) => command === "write_managed_terminal");
    expect(writes).toHaveLength(1);

    const input = await screen.findByRole("textbox", { name: "发送消息给 Agent" });
    fireEvent.change(input, { target: { value: "换个方案" } });
    // 一开始打字,圆钮就让回「发送」;停回合改走 Ctrl+Enter,composer 上不长出新按钮。
    expect((await screen.findByRole("button", { name: "发送" })).className).toContain("chat-send-button");
    expect(screen.queryByRole("button", { name: "中断" })).toBeNull();
    expect(screen.queryByRole("button", { name: "打断并发送" })).toBeNull();
  });

  /**
   * 排队回执逐条可移除。**只清 GUI 记账**:消息早已写进 PTY,CLI 队列里的它照常执行——
   * 移除不得再向终端写任何按键(否则用户以为"撤回"了,实际还多打断了一次回合)。
   */
  it("排队回执可逐条移除,且不向终端写任何按键", async () => {
    window.history.replaceState({}, "", "/?sessionId=97");
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 97, title: "移除回执", status: "running", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [], connected: true,
      });
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 97, active: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const input = await screen.findByRole("textbox", { name: "发送消息给 Agent" });
    fireEvent.change(input, { target: { value: "第一句" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 97, data: "第一句" }), { timeout: 3_000 });
    // 发送成功后组件才清空输入框——不等它,第二句会被那次 setPrompt("") 抹掉。
    await waitFor(() => expect((input as HTMLTextAreaElement).value).toBe(""));
    fireEvent.change(input, { target: { value: "第二句" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 97, data: "第二句" }), { timeout: 3_000 });
    expect(await screen.findByText("2 条插话已排队,当前回合结束后处理")).toBeTruthy();

    const writesBefore = invoke.mock.calls.filter(([command]) => command === "write_managed_terminal").length;
    fireEvent.click(screen.getAllByRole("button", { name: "移除这条回执" })[0]);
    expect(await screen.findByText("1 条插话已排队,当前回合结束后处理")).toBeTruthy();
    // 被移除的是第一条,第二条还挂着(按 id 删,不受重复文本/自动消解并发影响)。
    expect(screen.queryByText("第一句")).toBeNull();
    expect(screen.queryByText("第二句")).toBeTruthy();
    expect(invoke.mock.calls.filter(([command]) => command === "write_managed_terminal").length).toBe(writesBefore);
  });

  it("未声明中断键(interrupt_input=null)的 agent:圆钮不变停止,Ctrl+Enter 不发中断键", async () => {
    window.history.replaceState({}, "", "/?sessionId=95");
    // 当前五家都声明了 Esc;这里显式造一个 null 的 chatUi(模拟未取证/未来新 agent),
    // 验证门控:没有中断键就没有能停下回合的手段,界面不许摆出停止的样子。
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 95, title: "无中断键", status: "running", provider: "kimi", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [], connected: true,
      });
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "agent_chat_ui") return Promise.resolve({ ...chatUi("kimi")!, interrupt_input: null });
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 95, active: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const input = await screen.findByRole("textbox", { name: "发送消息给 Agent" });
    fireEvent.change(input, { target: { value: "插话" } });
    await waitFor(() => expect((input as HTMLTextAreaElement).value).toBe("插话"));
    // Ctrl+Enter 退化成普通发送:降级要静默,不能凭空发一个这家 CLI 不认的按键。
    fireEvent.keyDown(input, { key: "Enter", ctrlKey: true });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 95, data: "插话" }), { timeout: 3_000 });
    expect(invoke.mock.calls.filter(([command, args]) =>
      command === "write_managed_terminal" && (args as { data: string }).data === "")).toHaveLength(0);
    // 圆钮同一道门:停不下来就不许摆出停止的样子,按钮不能骗人。
    fireEvent.change(input, { target: { value: "" } });
    await waitFor(() => expect((input as HTMLTextAreaElement).value).toBe(""));
    expect(screen.queryByRole("button", { name: "中断" })).toBeNull();
    // 圆钮本身还在,只是身份仍是发送(刚发完那条时它可能正处于「发送中…」)。
    expect(screen.getByRole("button", { name: /^发送(中…)?$/ })).toBeTruthy();
  });

  /**
   * GUI 用户的「结束会话」:此前结束入口只藏在终端页操作条里,不开终端页的人无从结束,
   * 会话在后台一直占着进程。标题栏入口仅在本 GUI 托管该 PTY(ptyManaged)时可见,
   * confirm 通过才调 stop_managed_terminal,取消则不动。
   */
  it("ptyManaged 会话可从标题栏结束,confirm 取消则不调用", async () => {
    window.history.replaceState({}, "", "/?sessionId=92");
    // 确认走应用内原生小窗(invoke confirm_dialog):按队列给答案。
    const confirmAnswers: boolean[] = [false, true];
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 92, title: "托管会话", status: "running", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [],
        connected: true, ptyManaged: true,
      });
      if (command === "get_pending_approval") return Promise.resolve(null);
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "confirm_dialog") return Promise.resolve(confirmAnswers.shift() ?? false);
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 92, active: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const button = await screen.findByRole("button", { name: "结束会话" });
    // 取消(队列首个 false)→ 不杀进程。
    fireEvent.click(button);
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("confirm_dialog", expect.anything()));
    expect(invoke.mock.calls.some(([command]) => command === "stop_managed_terminal")).toBe(false);

    // 确定(队列次个 true)→ 杀进程。
    fireEvent.click(button);
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("stop_managed_terminal", { sessionId: 92 }));
  });

  /**
   * 存活的 running 会话:标题栏徽标显示「运行中」,常驻运行条在滚动流**之外**
   * (原实现在 .chat-scroll 里,上翻历史即滚出视口),窗口标题带 ▶ 前缀让任务栏可感知。
   */
  it("存活 running 会话:标题栏徽标+滚动流外的常驻运行条+窗口标题记号", async () => {
    window.history.replaceState({}, "", "/?sessionId=91");
    respondWithHistory({
      sessionId: 91, title: "跑着的会话", status: "running", provider: "claude", cwd: "C:/repo",
      supported: true, offset: 1, reset: false, pendingReview: null, connected: true,
      currentActivity: "› cargo test",
      items: [{ type: "user_text", id: "u1", timestamp: null, text: "开始" }],
    });
    render(<ChatWindow />);
    await waitFor(() => expect(screen.getByText("跑着的会话")).toBeTruthy());
    expect(screen.getByText("运行中")).toBeTruthy();
    const strip = document.querySelector(".chat-running");
    expect(strip).toBeTruthy();
    expect(strip!.closest(".chat-scroll")).toBeNull();
    expect(strip!.textContent).toContain("› cargo test");
    await waitFor(() => expect(setTitleMock).toHaveBeenCalledWith("▶ 跑着的会话 · Meowo"));
  });
});

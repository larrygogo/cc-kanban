import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

const invoke = vi.hoisted(() => vi.fn());
const openDialog = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({
  invoke,
  convertFileSrc: (path: string) => `asset://localhost/${encodeURIComponent(path)}`,
}));
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

import { ChatWindow, submitGapMs } from "./ChatWindow";
import { zh } from "../i18n/zh";
import { chatUi, descriptors } from "../test/agents";
import { terminalAttention } from "../terminalAttention";

/// UTF-8 → base64。btoa 只吃 latin1,含中文的终端回显必须先转字节,否则直接抛
/// InvalidCharacterError(生产端 decodeBase64 按 UTF-8 字节解码,两头对齐)。
function b64utf8(text: string): string {
  return btoa(String.fromCharCode(...new TextEncoder().encode(text)));
}

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
    if (command === "pending_interaction") return Promise.resolve({ approval, question: null });
    if (command === "managed_terminal_binding") return Promise.resolve(null);
    if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 1, active: true, managed: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
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
    // 标题是动作菜单触发钮（Kimi 式）：主仓路径入口在菜单的「打开项目目录」;
    // 多仓的目录清单与附加管理在 diff 面板头行的仓菜单(用户指定)。
    fireEvent.click(screen.getByRole("button", { name: /实现同步对话/ }));
    fireEvent.click(screen.getByRole("menuitem", { name: "打开项目目录" }));
    expect(invoke).toHaveBeenCalledWith("open_project_dir", { cwd: "C:/repo" });
    expect(screen.getByText("开始")).toBeTruthy();
    expect(screen.getByText("我来实现")).toBeTruthy();
    // 短思考直接摊开，不为几行内容加一次点击（长的才收成预览态，见下一个用例）。
    const reasoning = screen.getByText("先检查现有协议").closest("details");
    expect(reasoning?.hasAttribute("open")).toBe(true);
    expect(reasoning?.className).not.toContain("is-long");
    // 摘要直接写种类（「运行终端」），不再有「执行了 N 次工具调用」的计数短语。
    const activity = screen.getAllByText("运行终端")[0].closest("details");
    expect(activity?.className).toContain("chat-activity-group");
    expect(activity?.hasAttribute("open")).toBe(false);
    expect(screen.queryByText("工具结果")).toBeNull();
    expect(invoke).toHaveBeenCalledWith("get_chat_history", { sessionId: 7, offset: 0 });
    fireEvent.change(screen.getByRole("combobox", { name: "发送消息给 Agent" }), { target: { value: "继续实现" } });
    fireEvent.keyDown(screen.getByRole("combobox", { name: "发送消息给 Agent" }), { key: "Enter" });
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
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 9, active: false, managed: false, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
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
    expect(document.querySelector(".chat-activity-group")).toBeNull();
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
    expect(document.querySelector(".chat-subagent .chat-activity-group")).toBeTruthy();

    // 折叠再展开不该重复请求（结果缓存在组件里）。
    const calls = invoke.mock.calls.filter(([command]) => command === "get_subagent_transcript").length;
    toggle(false);
    toggle(true);
    expect(invoke.mock.calls.filter(([command]) => command === "get_subagent_transcript").length).toBe(calls);
  });

  it("不展开也显示子任务状态：无回执=在跑，有回执=按结局统计", async () => {
    window.history.replaceState({}, "", "/?sessionId=17");
    const swarm = {
      type: "tool_use", id: "tool_s", timestamp: "2026-08-18T04:00:00.000Z", name: "AgentSwarm", summary: "分组审查",
      subagent: { description: "分组审查", agent_type: "explore", count: 3 },
    };
    let done = false;
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 17, title: "批量", status: "running", provider: "kimi", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null,
        items: done
          ? [swarm, {
              type: "tool_result", id: "r1", timestamp: "2026-08-18T04:06:40.000Z", tool_use_id: "tool_s",
              text: "done", is_error: false,
              subagent: { running: 0, completed: 2, failed: 1 },
            }]
          // 一批 fan-out 的结局要等整批跑完才写进主链——跑着的时候主链上没有回执。
          : [swarm],
      });
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 17, active: false, managed: false, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("kimi"));
      return Promise.resolve();
    });
    render(<ChatWindow />);

    // 没有回执 → 三个都在跑，且**不必展开**（不该为一个徽标去读侧车流）。
    expect(await screen.findByText("3 进行中")).toBeTruthy();
    expect(invoke).not.toHaveBeenCalledWith("get_subagent_transcript", expect.anything());
    // 在跑时图标脉冲——折叠状态下不必读徽标小字也能看出里面有活儿。
    expect(document.querySelector(".chat-subagent-icon.is-running")).toBeTruthy();

    // 回执到达后按真实结局显示（靠历史轮询自然刷新，不手动重渲染）。
    done = true;
    await waitFor(() => expect(screen.getAllByText("2 完成 · 1 失败").length).toBeGreaterThan(0), { timeout: 3_000 });
    expect(screen.queryByText("3 进行中")).toBeNull();
    // 结束后:图标停脉冲,显示「委派 → 最终回执」的执行时长(04:00:00 → 04:06:40)。
    expect(document.querySelector(".chat-subagent-icon.is-running")).toBeNull();
    expect(screen.getAllByText("6 分 40 秒").length).toBeGreaterThan(0);
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
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("kimi"));
      if (command === "write_managed_terminal" && args.data === "/model") sentMenuCommand = true;
      if (command === "managed_terminal_snapshot") {
        // 命令发出后，CLI 把菜单画到屏幕上。
        return Promise.resolve({
          sessionId: 18, active: true, managed: true,
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
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 53, active: true, managed: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      if (command === "save_pasted_attachment") {
        return Promise.resolve(`C:/tmp/meowo-paste/1-0/${(args as { fileName: string }).fileName}`);
      }
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const input = await screen.findByRole("combobox", { name: "发送消息给 Agent" });
    // jsdom 的 File/Blob 实现参差，只按组件用到的形状（name + arrayBuffer）伪造。
    const file = { name: "shot.png", arrayBuffer: () => Promise.resolve(new Uint8Array([1, 2, 3]).buffer) } as unknown as File;
    fireEvent.paste(input, { clipboardData: { files: [file] } });
    // 内容经 base64 交给宿主落盘（[1,2,3] → "AQID"），路径回来后按文件名显示为附件。
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("save_pasted_attachment", { fileName: "shot.png", dataBase64: "AQID" }));
    // 图片附件渲染为缩略图（ImageRef），文件名在 img 的 alt 上而不再是文本 chip。
    expect(await screen.findByAltText("shot.png")).toBeTruthy();
  });

  it("手敲交互式内置命令（/config）走菜单识别通道：清空输入框并打开识别窗口", async () => {
    window.history.replaceState({}, "", "/?sessionId=52");
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 52, title: "交互命令", status: "running", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [], model: "Opus",
      });
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve({ sessionId: 52, active: true, managed: true, data: btoa("ready"), startOffset: 0, endOffset: 5, exited: false, exitCode: null });
      }
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const input = await screen.findByRole("combobox", { name: "发送消息给 Agent" });
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

  it("窗口关掉后,静默探测的收尾 Esc 不得再写进那个会话（幽灵按键会打断 agent 的回合）", async () => {
    // probeModelMenu 里 `await sendText(...)` 之后才调 endSilentProbe,而 sendText 内含
    // 写后回显校验与拉终端,期间用户完全可能关窗。此前这条路没有活跃会话复核,晚到的
    // Esc 会写进早已离开的会话——CI 实拍中它跨到了后面的用例里(sessionId 对不上)。
    window.history.replaceState({}, "", "/?sessionId=77");
    let failSend: (() => void) | null = null;
    invoke.mockImplementation((command: string, args?: Record<string, unknown>) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 77, title: "换模型", status: "running", provider: "kimi", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [], model: "K3",
      });
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("kimi"));
      if (command === "managed_terminal_snapshot") return Promise.resolve({
        sessionId: 77, active: true, managed: true, data: btoa("ready"), startOffset: 0, endOffset: 5, exited: false, exitCode: null,
      });
      // 把 /model 的下发**挂住**,模拟「还在途中用户就关窗」;放行时让它失败,于是
      // sendText 返回 false,probeModelMenu 走 `if (!sent) endSilentProbe()` 那条收尾路。
      if (command === "write_managed_terminal" && (args as { data?: string })?.data === "/model") {
        return new Promise<void>((_, reject) => { failSend = () => reject(new Error("发送失败")); });
      }
      return Promise.resolve();
    });
    render(<ChatWindow />);
    fireEvent.click(await screen.findByRole("button", { name: "切换模型" }));
    await waitFor(() => expect(failSend).toBeTruthy());
    // 关窗,再让在途的那一发失败——收尾的 Esc 就是在这之后发出的。
    cleanup();
    await act(async () => {
      failSend!();
      for (let i = 0; i < 8; i += 1) await Promise.resolve();
    });
    expect(invoke).not.toHaveBeenCalledWith("write_managed_terminal", { sessionId: 77, data: "\x1b" });
  });

  it("菜单已打开时再点不重发命令（否则会打进搜索框把候选全过滤掉）", async () => {
    window.history.replaceState({}, "", "/?sessionId=19");
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 19, title: "换模型", status: "running", provider: "kimi", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [], model: "K3",
      });
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("kimi"));
      if (command === "managed_terminal_snapshot") return Promise.resolve({
        sessionId: 19, active: true, managed: true, data: btoa("ready"), startOffset: 0, endOffset: 5, exited: false, exitCode: null,
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

  // 新建会话(负数临时 id)落在用户偏好的视图上:在对话就显示对话、在终端就显示终端
  // (实拍反馈:此前强制进终端,对话党每次新建都被甩去终端页)。
  it("新建会话:默认(偏好对话)进对话页,渲染启动占位而非谎报「没有记录」", async () => {
    window.history.replaceState({}, "", "/?sessionId=-3");
    invoke.mockResolvedValue(null);
    render(<ChatWindow />);
    expect(await screen.findByText("会话正在启动…")).toBeTruthy();
    expect(invoke).not.toHaveBeenCalledWith("get_chat_history", expect.anything());
  });

  it("新建会话:视图偏好是终端时直接进终端", async () => {
    window.history.replaceState({}, "", "/?sessionId=-3");
    localStorage.setItem("meowo-chat-view-pref", "terminal");
    try {
      invoke.mockResolvedValue(null);
      render(<ChatWindow />);
      expect(await screen.findByText("PTY -3")).toBeTruthy();
      expect(invoke).toHaveBeenCalledWith("managed_terminal_binding", { sessionId: -3 });
      expect(invoke).not.toHaveBeenCalledWith("get_chat_history", expect.anything());
    } finally {
      localStorage.removeItem("meowo-chat-view-pref");
    }
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
    // 图片附件是缩略图（alt=文件名），非图片仍是文本 chip。
    expect(await screen.findByAltText("design.png")).toBeTruthy();
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
    let wrote = false;
    invoke.mockImplementation((command: string, args?: Record<string, unknown>) => {
      if (command === "get_chat_history") return Promise.resolve(history);
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "start_managed_terminal") { started = true; return Promise.resolve(); }
      // 刚恢复的发送走写后回显验证(submitToTerminal 的 verify):正文写入后快照要
      // 能看到它的回显,否则发送被撤销——mock 据此在 write 之后把回显补进画面。
      if (command === "write_managed_terminal") {
        if (String((args as { data?: string } | undefined)?.data ?? "").includes("继续")) wrote = true;
        return Promise.resolve();
      }
      if (command === "managed_terminal_snapshot") {
        // endOffset 是「已产生多少输出」的判据（data 现在是 base64 增量，可能为空）；
        // 就绪判定还要求 data 里有可见文本（纯控制序列不算）。
        return Promise.resolve(started
          ? { sessionId: 13, active: true, managed: true, data: b64utf8(wrote ? "ready \u276f 继续" : "ready"), startOffset: 0, endOffset: wrote ? 20 : 5, exited: false, exitCode: null }
          : { sessionId: 13, active: false, managed: false, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      }
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const input = await screen.findByRole("combobox", { name: "发送消息给 Agent" });
    fireEvent.change(input, { target: { value: "继续" } });
    fireEvent.click(screen.getByRole("button", { name: "发送" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("start_managed_terminal", {
      sessionId: 13, cols: 100, rows: 30,
    }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 13, data: "继续" }), { timeout: 2_000 });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 13, data: "\r" }), { timeout: 2_000 });
  });

  it("恢复后发送:正文没有回显时撤销发送、绝不发回车", async () => {
    // 实拍回归:接管长会话后 claude 弹 resume 确认选择器,而「可见+安静」的就绪判定
    // 认不出它——正文被选择器吞掉,旧逻辑的无条件回车还会替用户按下默认项:消息蒸发、
    // 恢复继续、没有任何报错(transcript 里根本没有那条 user 消息)。
    window.history.replaceState({}, "", "/?sessionId=71");
    const history = {
      sessionId: 71, title: "恢复吞字", status: "ended", provider: "claude", cwd: "C:/repo",
      supported: true, offset: 0, reset: false, pendingReview: null, items: [],
    };
    respondWithHistory(history);
    let started = false;
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve(history);
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "start_managed_terminal") { started = true; return Promise.resolve(); }
      if (command === "managed_terminal_snapshot") {
        // 拉起后有可见画面但正文**永不回显**——未被识别的恢复确认选择器吞输入的形态。
        return Promise.resolve(started
          ? { sessionId: 71, active: true, managed: true, data: btoa("resume this session?"), startOffset: 0, endOffset: 20, exited: false, exitCode: null }
          : { sessionId: 71, active: false, managed: false, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      }
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const input = await screen.findByRole("combobox", { name: "发送消息给 Agent" }) as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: "go on" } });
    fireEvent.click(screen.getByRole("button", { name: "发送" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 71, data: "go on" }), { timeout: 2_000 });
    // 回显等满超时后:Ctrl-U 撤销、报错可见、输入框保留原文——唯独不能出现的是回车。
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 71, data: "\x15" }), { timeout: 5_000 });
    expect(invoke).not.toHaveBeenCalledWith("write_managed_terminal", { sessionId: 71, data: "\r" });
    expect(await screen.findByText(/消息未进入终端/)).toBeTruthy();
    expect(input.value).toBe("go on");
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
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "managed_terminal_binding") return Promise.resolve(null);
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("codex"));
      if (command === "start_managed_terminal") { started = true; return Promise.resolve(); }
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve(started
          ? {
              sessionId: 14, active: true, managed: true,
              data: btoa("\x1b[2JDo you trust the contents of this directory?\r\n> 1. Yes, continue\r\n  2. No, quit"),
              startOffset: 0, endOffset: 76, exited: false, exitCode: null,
            }
          : { sessionId: 14, active: false, managed: false, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      }
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const input = await screen.findByRole("combobox", { name: "发送消息给 Agent" });
    fireEvent.change(input, { target: { value: "继续修复" } });
    fireEvent.click(screen.getByRole("button", { name: "发送" }));

    // composer 常驻后 footer 里的 sendError 横幅(role=alert)可能与卡片同屏——卡片本体
    // G-16 后是 alertdialog，按角色 + 类名取。
    const trustCard = (await screen.findAllByRole("alertdialog")).find((el) => el.className.includes("chat-approval"));
    expect(trustCard?.textContent).toContain("是否信任此文件夹？");
    // C-9：卡片渲染在 overlay 层(零高度容器,不占文档流、不下推 composer),
    // 有卡在场时容器挂 is-active 启用消息列表底部的淡出过渡。
    expect(trustCard?.closest(".chat-approval-overlay")?.className).toContain("is-active");
    // 卡片在场时 composer 锁定但**不卸载**:textarea 还是同一个节点(草稿不丢),只是禁用。
    expect((screen.getByRole("combobox", { name: "发送消息给 Agent" }) as HTMLTextAreaElement).disabled).toBe(true);
    expect(screen.getByText("PTY 14")).toBeTruthy();
    expect(screen.getByRole("button", { name: "对话" }).className).toContain("is-active");
    expect(screen.getByText("PTY 14").closest(".chat-terminal-pane")?.className).toContain("is-background");
    expect(screen.getByText("PTY 14").closest(".chat-terminal-pane")?.getAttribute("aria-hidden")).toBe("true");
    expect(invoke).not.toHaveBeenCalledWith("write_managed_terminal", { sessionId: 14, data: "继续修复" });
    expect((input as HTMLTextAreaElement).value).toBe("继续修复");
    // 原始终端页已经显示 TUI，不再叠加 GUI 卡片；切回对话后仍可直接点击结构化选项。
    fireEvent.click(screen.getByRole("button", { name: "终端" }));
    expect(screen.queryByRole("alertdialog")).toBeNull();
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
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve({ sessionId: 44, active: true, managed: true, data: btoa(prompt), startOffset: 0, endOffset: prompt.length, exited: false, exitCode: null });
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
        { type: "assistant_text", id: "a1", timestamp: null, text: "看 **重点** 和 `code`，详见 [官网](https://example.com/docs)，来信 [邮箱](mailto:a@b.com)" },
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
    // mailto: 这类非 http(s) 链接后端 open_link 必然拒绝，渲染层直接降级为纯文本——
    // 「可点却必失败」比不可点更糟。
    expect(screen.queryByRole("link", { name: "邮箱" })).toBeNull();
    expect(screen.getByText(/来信/).textContent).toContain("邮箱");
    expect(invoke).not.toHaveBeenCalledWith("open_link", { url: "mailto:a@b.com" });
    // 含框线字符的代码块被钉到字符网格：中文锁 2ch 盒子（renderGrid 拆成单字符 span），
    // 整块标记 chat-md-diagram；普通行内代码不受牵连、不被拆分。
    const wide = screen.getByText("话");
    expect(wide.className).toBe("chat-md-cell2");
    expect(wide.closest("code")?.className).toContain("chat-md-diagram");
    expect(screen.getByText("code").className).not.toContain("chat-md-diagram");
  });

  it("persists drafts under a per-session key and migrates the legacy whole-map format", async () => {
    // 旧版整表格式：首次加载拆成按会话 key（多窗整表读-改-写会互相覆盖草稿）并恢复进输入框。
    localStorage.setItem("meowo-chat-drafts", JSON.stringify({
      "21": { prompt: "旧草稿", attachments: [], at: 1 },
    }));
    window.history.replaceState({}, "", "/?sessionId=21");
    respondWithHistory({
      sessionId: 21, title: "草稿", status: "running", provider: "claude", cwd: null,
      supported: true, offset: 0, reset: false, pendingReview: null, items: [],
    });
    render(<ChatWindow />);
    const input = await screen.findByRole("combobox", { name: "发送消息给 Agent" });
    await waitFor(() => expect((input as HTMLTextAreaElement).value).toBe("旧草稿"));
    // 迁移完成：旧整表已删，按会话 key 在场。
    expect(localStorage.getItem("meowo-chat-drafts")).toBeNull();
    expect(localStorage.getItem("meowo-chat-draft:21")).toContain("旧草稿");
    // 继续编辑只写自己这条 key（400ms 防抖）。
    fireEvent.change(input, { target: { value: "新草稿" } });
    await waitFor(() => expect(localStorage.getItem("meowo-chat-draft:21")).toContain("新草稿"), { timeout: 2000 });
    // 清空即删条目。
    fireEvent.change(input, { target: { value: "" } });
    await waitFor(() => expect(localStorage.getItem("meowo-chat-draft:21")).toBeNull(), { timeout: 2000 });
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
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      // 斜杠补全与模型预设不是前端硬编码表：按会话查 agent_chat_ui（内置表 ∪ 自定义命令）。
      if (command === "agent_chat_ui") {
        return Promise.resolve(chatUi("claude", [
          { name: "/deploy", description: "部署到测试环境", source: "project" },
        ]));
      }
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve({ sessionId: 31, active: true, managed: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      }
      return Promise.resolve();
    });
    render(<ChatWindow />);
    await screen.findByText("运行观察");
    // agent logo（标题栏最前，aria-label=provider）+ 运行指示（有活动时显示活动文本）。
    // agent 徽标已从标题栏移除（实拍反馈），不再断言它。
    expect(screen.getByText("Bash: cargo test")).toBeTruthy();
    // 上下文用量环：环内百分比 + 环右已用/总量（63% × 200K ≈ 126K）。
    expect(screen.getByText("63")).toBeTruthy();
    expect(screen.getByText("126K/200K")).toBeTruthy();
    // "/" 前缀弹补全；选中后填入输入框并留出参数位，不自动发送。
    const input = screen.getByRole("combobox", { name: "发送消息给 Agent" }) as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: "/mo" } });
    // combobox/listbox/option 的 ARIA 连线:菜单开着时 expanded/controls 指向 listbox,
    // activedescendant 指向当前高亮 option(默认第一项)。
    expect(input.getAttribute("aria-expanded")).toBe("true");
    expect(input.getAttribute("aria-controls")).toBe("chat-slash-listbox");
    expect(input.getAttribute("aria-activedescendant")).toBe("chat-slash-option-0");
    fireEvent.click(screen.getByRole("option", { name: /^\/model/ }));
    expect(input.value).toBe("/model ");
    // 菜单收起后 ARIA 连线一并撤掉。
    expect(input.getAttribute("aria-expanded")).toBe("false");
    expect(input.getAttribute("aria-controls")).toBeNull();
    // 子串匹配:词中片段也给候选(此前只前缀匹配,/epl 时菜单直接消失)。
    fireEvent.change(input, { target: { value: "/epl" } });
    expect(screen.getByRole("option", { name: /^\/deploy\s*部署到测试环境/ })).toBeTruthy();
    // 参数提示行:命令精确命中且开始填参数后,候选菜单收起、换成该命令的说明行。
    fireEvent.change(input, { target: { value: "/deploy prod" } });
    expect(screen.queryByRole("option")).toBeNull();
    expect(screen.getByRole("note").textContent).toContain("/deploy");
    expect(screen.getByRole("note").textContent).toContain("部署到测试环境");
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
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve({ sessionId: 32, active: true, managed: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
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

    const input = await screen.findByRole("combobox", { name: "发送消息给 Agent" });
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
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve({ sessionId: 21, active: true, managed: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      }
      return Promise.resolve();
    });
    render(<ChatWindow />);
    await screen.findByText("初始标题");
    expect(screen.getByText("Bash: 第一步")).toBeTruthy();
    expect(screen.getByText("10")).toBeTruthy();
    // 模式按钮只显示当前值，维度名在 aria-label/tooltip 里。
    expect(screen.getByText("默认")).toBeTruthy();

    // 权限模式现在是下拉（选项由屏幕回显标记派生）：点按钮开菜单、点某项才发 cycle 键。
    fireEvent.click(screen.getByRole("button", { name: "切换模式: 权限模式" }));
    fireEvent.click(screen.getByRole("menuitem", { name: /计划/ }));
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
    expect(await screen.findByText("计划模式")).toBeTruthy();

    // 兜底时间线读 lastUserText/lastAiText（transcript 空窗期渲染 hook 落库的最近往来），
    // 它们也在比较清单里——漏掉的话空窗期内容永远停在第一轮。
    current = { ...base, currentActivity: "Bash: 第二步", contextPct: 42, title: "改后标题", agentModes: [{ dimension: "permission", value: "plan" }], lastUserText: "hook 落库的提问" };
    expect(await screen.findByText("hook 落库的提问")).toBeTruthy();

    // ptyManaged/endable 也在比较清单里——会话已 connected 时中途拉起托管 PTY,那一轮往往只有
    // 这两个字段变;漏掉的话「结束会话」按钮永远不出现(真实翻车过:发消息拉起 PTY 后按钮不见)。
    // 按钮门控看 endable(托管或进程仍活的孤儿),真实后端两者同轮翻真。
    expect(screen.queryByText("结束会话")).toBeNull();
    current = { ...base, currentActivity: "Bash: 第二步", contextPct: 42, title: "改后标题", agentModes: [{ dimension: "permission", value: "plan" }], lastUserText: "hook 落库的提问", ptyManaged: true, endable: true };
    expect(await screen.findByText("结束会话")).toBeTruthy();

    // 标题栏状态徽标已整体移除(errored 的窗口内表面只剩侧栏状态点,不在本测试射程)。
    // connected 仍在比较清单里——漏掉的话进程死亡(connected 翻 false、status 仍 running)
    // 时窗口标题滞留「▶」运行记号,假运行中复活。只翻 connected,断言标题记号退场。
    current = { ...base, currentActivity: "Bash: 第二步", contextPct: 42, title: "改后标题", agentModes: [{ dimension: "permission", value: "plan" }], lastUserText: "hook 落库的提问", ptyManaged: true, endable: true, connected: false };
    await waitFor(() => expect(setTitleMock).toHaveBeenCalledWith("改后标题 · Meowo"));
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
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve({ sessionId: 41, active: true, managed: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      }
      if (command === "agent_chat_ui") return Promise.resolve({
        // 与 ChatUi 真实形状对齐(必填字段全给):此前缺 menu_slash_commands 等新字段,
        // 全靠生产码的可选链兜底侥幸通过,是唯一漏网的 mock。取值保持中性(空表/null),
        // 不引入本用例无关的菜单行为。
        slash_commands: [], model_presets: [], version: "0.26.0",
        model_menu_command: null, menu_slash_commands: [],
        startup_attention_markers: [], selector_anchors: [], interrupt_input: null, runtime_commands_pending: false,
        attachment_mention: false, clipboard_image_paste: null, clipboard_paste_input: null,
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
    // 模式按钮只显示当前值，维度名在 aria-label/tooltip 里。
    await screen.findByRole("button", { name: "切换模式: 工作模式" });
    expect(screen.getByText("默认")).toBeTruthy();
    expect(screen.getByText("手动确认")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "切换模式: 权限模式" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "YOLO" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 41, data: "/yolo on" }));
    // Enter 隔 SUBMIT_GAP_MS 才发（见 submitToTerminal），同样要 waitFor。
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 41, data: "\r" }));
    expect(await screen.findByText("YOLO")).toBeTruthy();
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
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "write_managed_terminal" && (args as { data?: string } | undefined)?.data === "[Z") {
        // CLI 收到 Shift+Tab 后重绘出新指示。
        out += screenText;
      }
      if (command === "managed_terminal_snapshot") {
        const since = Math.min(Number((args as { since?: number } | undefined)?.since ?? 0), out.length);
        return Promise.resolve({ sessionId: 51, active: true, managed: true, data: btoa(out.slice(since)), startOffset: since, endOffset: out.length, exited: false, exitCode: null });
      }
      return Promise.resolve();
    });
    render(<ChatWindow />);
    await screen.findByText("默认");
    // 空闲期：后续轮询不再带模式增量（真实后端普通增量的 agent_modes 就是空）。
    current = { ...base, agentModes: [] };
    // 只有 cycle 键的维度也给下拉：选项由屏幕回显标记派生，选中后循环按到位（cycleToMode）。
    fireEvent.click(screen.getByRole("button", { name: "切换模式: 权限模式" }));
    fireEvent.click(screen.getByRole("menuitem", { name: /计划/ }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 51, data: "\u001b[Z" }));
    // transcript 静默，但屏幕是 CLI 自己画的权威状态——标签应据它回显为「计划」。
    expect(await screen.findByText("计划模式")).toBeTruthy();
  });

  it("休眠会话的权限胶囊改卖「下一次恢复的权限」：选择只落启动参数，不碰终端", async () => {
    // connected=false 时切「活进程的模式」是空谈——进程都不在了，循环按键还得先拉起会话，
    // 且循环够不着 bypassPermissions 这类启动期档位。此时胶囊菜单换成启动档位（launch_options），
    // 选择记入 resumePermission，随下一次发送作为 start_managed_terminal 的 options 下发。
    window.history.replaceState({}, "", "/?sessionId=61");
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 61, title: "休眠会话", status: "ended", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, model: null, connected: false,
        agentModes: [{ dimension: "permission", value: "default" }],
        contextPct: null, contextWindow: null, currentActivity: null, hasMore: false, items: [],
      });
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "list_agents") return Promise.resolve(descriptors(["claude"]));
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 61, active: false, managed: false, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      // 拉起在断言点之后立刻失败收场，免得 waitForTerminalReady 的就绪轮询在测试结束后游荡。
      if (command === "start_managed_terminal") return Promise.reject("测试桩：不真正拉起");
      return Promise.resolve();
    });
    render(<ChatWindow />);
    fireEvent.click(await screen.findByRole("button", { name: "切换模式: 权限模式" }));
    // 菜单是启动档位（含「沿用原设置」与循环够不着的 bypassPermissions）；等 list_agents
    // 就位后条目才换装——findByRole 的重试恰好吸收这次异步。
    await screen.findByRole("menuitem", { name: "权限：沿用原设置" });
    fireEvent.click(screen.getByRole("menuitem", { name: "跳过权限确认" }));
    // 选择只落状态:不发 cycle 键、不发模式命令。不断言「零写入」——前序用例
    // submitToTerminal 的延迟回车/撤销键/探测写在慢机上会跨用例落进共享的 invoke 记录
    // (sessionId 16 用例注释记载过同一竞态;泄漏字节形形色色,曾按 \r/\x15 枚举仍抖出
    // 第三种)。改按 sessionId 隔离:只统计发给**本用例会话 61** 的写入,来源即隔断。
    const modeWrites = invoke.mock.calls.filter(([command, args]) =>
      command === "write_managed_terminal"
      && (args as { sessionId?: number } | undefined)?.sessionId === 61);
    expect(modeWrites).toEqual([]);
    // 胶囊显示所选档（启动档位词「跳过权限确认」，非运行时模式词「跳过权限检查」）。
    expect(await screen.findByText("跳过权限确认")).toBeTruthy();
    // 下一次发送把它作为启动选项带给 start_managed_terminal。
    const input = screen.getByRole("combobox", { name: "发送消息给 Agent" });
    fireEvent.change(input, { target: { value: "继续" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("start_managed_terminal", { sessionId: 61, cols: 100, rows: 30, options: { permission: "bypassPermissions" } }));
  });

  it("会话存过启动档位时，「沿用原设置」直接亮成那一档", async () => {
    // 回归：占位文案「权限：沿用原设置」是句黑盒——后端 sessions.launch_args 存着上次
    // 选的档，能对上插件 choices 就直接显示并预选它；「沿用」= 选中它本身，不写覆盖。
    window.history.replaceState({}, "", "/?sessionId=62");
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 62, title: "存过档位", status: "ended", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, model: null, connected: false,
        agentModes: [{ dimension: "permission", value: "default" }],
        contextPct: null, contextWindow: null, currentActivity: null, hasMore: false, items: [],
      });
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "list_agents") return Promise.resolve(descriptors(["claude"]));
      if (command === "session_launch_selections") return Promise.resolve({ permission: "bypassPermissions" });
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 62, active: false, managed: false, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    // 胶囊直接显示存的档（启动档位词），不再是 transcript 回读的旧模式词。
    expect(await screen.findByText("跳过权限确认")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "切换模式: 权限模式" }));
    // 菜单里没有黑盒占位项；存的那档带高亮。
    const item = await screen.findByRole("menuitem", { name: "跳过权限确认" });
    expect(screen.queryByRole("menuitem", { name: "权限：沿用原设置" })).toBeNull();
    expect(item.className).toContain("is-active");
  });

  it("offers to load earlier messages when the first read was truncated", async () => {
    window.history.replaceState({}, "", "/?sessionId=33");
    const truncated = {
      sessionId: 33, title: "长会话", status: "running", provider: "claude", cwd: null,
      supported: true, offset: 500, reset: false, pendingReview: null,
      model: null, contextPct: null, contextWindow: null, currentActivity: null,
      hasMore: true, earliest: 120,
      items: [{ type: "user_text", id: "recent", timestamp: null, text: "最近的消息" }],
    };
    // 增量轮询恒为 hasMore:false——提示不能因此闪掉。
    const incremental = { ...truncated, items: [], hasMore: false, earliest: 0 };
    let firstRead = true;
    invoke.mockImplementation((command: string, args: { before?: number }) => {
      if (command === "get_chat_history") {
        // 向上翻页：按 before（= 上一屏的 earliest）只回更早的一屏，可一直点到文件头。
        if (args?.before === 120) {
          return Promise.resolve({
            ...truncated, hasMore: true, earliest: 40,
            items: [{ type: "user_text", id: "mid", timestamp: null, text: "中间的消息" }],
          });
        }
        if (args?.before === 40) {
          return Promise.resolve({
            ...truncated, hasMore: false, earliest: 0,
            items: [{ type: "user_text", id: "old", timestamp: null, text: "很早以前的消息" }],
          });
        }
        if (firstRead) { firstRead = false; return Promise.resolve(truncated); }
        return Promise.resolve(incremental);
      }
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    await screen.findByText("最近的消息");
    const button = await screen.findByRole("button", { name: "加载更早的对话" });
    // 被裁掉的消息此刻不在 DOM 里——这正是首屏省下的成本。
    expect(screen.queryByText("很早以前的消息")).toBeNull();

    // 第一屏：增量前插，不整段重读（无 full 参数）。
    fireEvent.click(button);
    expect(await screen.findByText("中间的消息")).toBeTruthy();
    expect(invoke).toHaveBeenCalledWith("get_chat_history", expect.objectContaining({ sessionId: 33, before: 120 }));
    expect(invoke).not.toHaveBeenCalledWith("get_chat_history", expect.objectContaining({ full: true }));
    // 还没到文件头：入口保留，可以再点。
    const again = await screen.findByRole("button", { name: "加载更早的对话" });

    // 第二屏翻到文件头（hasMore=false）：提示消失，已有消息不重复插入。
    fireEvent.click(again);
    expect(await screen.findByText("很早以前的消息")).toBeTruthy();
    await waitFor(() => expect(screen.queryByRole("button", { name: "加载更早的对话" })).toBeNull());
    expect(screen.getAllByText("最近的消息")).toHaveLength(1);
    expect(screen.getAllByText("中间的消息")).toHaveLength(1);
  });

  it("delays the loading skeleton until the first read takes >150ms", async () => {
    window.history.replaceState({}, "", "/?sessionId=44");
    let resolveHistory!: (value: unknown) => void;
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") {
        return new Promise((resolve) => { resolveHistory = resolve; });
      }
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      return Promise.resolve();
    });
    const { container } = render(<ChatWindow />);
    // 首帧不得有占位：快读（<150ms）不该闪骨架/文案造成三段跳。
    expect(container.querySelector(".chat-skeleton")).toBeNull();
    await new Promise((resolve) => setTimeout(resolve, 200));
    await waitFor(() => expect(container.querySelector(".chat-skeleton")).not.toBeNull());
    // 数据到达后骨架消失、内容落地。
    resolveHistory({
      sessionId: 44, title: "慢会话", status: "running", provider: "claude", cwd: null,
      supported: true, offset: 0, reset: false, pendingReview: null,
      connected: true, hasMore: false, earliest: 0,
      items: [{ type: "user_text", id: "u1", timestamp: null, text: "慢会话内容" }],
    });
    await screen.findByText("慢会话内容");
    expect(container.querySelector(".chat-skeleton")).toBeNull();
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
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "managed_terminal_binding") return Promise.resolve(null);
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 7, active: true, managed: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
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
    expect(screen.queryByRole("combobox", { name: "发送消息给 Agent" })).toBeNull();
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
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "set_archived") return archiveFails ? Promise.reject(new Error("db busy")) : Promise.resolve();
      // 归档后要切走：这里只有它自己一条，没有可切的下一条，窗口留在原地。
      if (command === "get_live_sessions_page") return Promise.resolve([]);
      return Promise.resolve();
    });
    render(<ChatWindow />);
    // 归档入口收进标题菜单（Kimi 式）：点标题开菜单再点「归档」。
    fireEvent.click(await screen.findByRole("button", { name: /要收起的会话/ }));
    fireEvent.click(screen.getByRole("menuitem", { name: "归档" }));
    expect(invoke).toHaveBeenCalledWith("set_archived", { sessionId: 7, archived: true });
    // 翻面后菜单项变成「取消归档」——它是这条会话已归档的唯一提示（菜单点完即收，重开验证）。
    fireEvent.click(screen.getByRole("button", { name: /要收起的会话/ }));
    expect(await screen.findByRole("menuitem", { name: "取消归档" })).toBeTruthy();

    archiveFails = true;
    fireEvent.click(screen.getByRole("menuitem", { name: "取消归档" }));
    // 失败回滚：错误可见，重开菜单仍是归档态（取消归档）。
    expect(await screen.findByText(/db busy/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /要收起的会话/ }));
    expect(await screen.findByRole("menuitem", { name: "取消归档" })).toBeTruthy();
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
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "get_live_sessions_page") {
        return Promise.resolve([{ session: { id: 9, cc_session_id: "cc-9", status: "ended" }, task_title: "下一条", connected: false, provider: "claude", cwd: null }]);
      }
      return Promise.resolve();
    });
    render(<ChatWindow />);
    fireEvent.click(await screen.findByRole("button", { name: /要收起的/ }));
    fireEvent.click(screen.getByRole("menuitem", { name: "归档" }));
    expect(await screen.findByText("下一条")).toBeTruthy();
  });

  /// Ctrl/Cmd+K 命令面板：会话上百条时侧栏翻页太慢，这是键盘用户的主通道；
  /// U1-14 后同层还能搜并执行窗口命令。
  it("Ctrl+K 打开命令面板，↑↓ 选择、Enter 切到目标会话", async () => {
    window.history.replaceState({}, "", "/?sessionId=7");
    const histories: Record<number, unknown> = {
      7: { sessionId: 7, title: "当前会话", status: "ended", provider: "claude", cwd: null, supported: true, offset: 0, reset: false, pendingReview: null, items: [], archived: false },
      9: { sessionId: 9, title: "目标会话", status: "ended", provider: "claude", cwd: null, supported: true, offset: 0, reset: false, pendingReview: null, items: [], archived: false },
    };
    invoke.mockImplementation((command: string, args?: Record<string, unknown>) => {
      if (command === "get_chat_history") return Promise.resolve(histories[args?.sessionId as number]);
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "get_live_sessions_page") {
        return Promise.resolve([
          { session: { id: 7, cc_session_id: "cc-7", status: "ended" }, task_title: "当前会话", project_name: "repo", connected: false, provider: "claude", cwd: null },
          { session: { id: 9, cc_session_id: "cc-9", status: "ended" }, task_title: "目标会话", project_name: "other", connected: false, provider: "claude", cwd: null },
        ]);
      }
      return Promise.resolve();
    });
    render(<ChatWindow />);
    // 标题与侧栏条目都叫「当前会话」：等标题栏那颗菜单钮（chat-title-menu 内）出现即可。
    await waitFor(() => expect(screen.getAllByRole("button", { name: /当前会话/ }).length).toBeGreaterThan(0));
    fireEvent.keyDown(window, { key: "k", code: "KeyK", ctrlKey: true });
    const input = await screen.findByPlaceholderText(zh.chat.switcherPlaceholder);
    // 首屏（空查询）即列出最近会话 + 命令组全列（2 会话 + 8 命令）
    const options = await screen.findAllByRole("option");
    expect(options).toHaveLength(10);
    expect(screen.getByText(zh.chat.switcherGroupCommands)).toBeTruthy();
    // ↓ 移到第二条（目标会话），Enter 切换
    fireEvent.keyDown(input, { key: "ArrowDown" });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("get_chat_history", { sessionId: 9, offset: 0 }));
    // 切换器已关闭
    expect(screen.queryByPlaceholderText(zh.chat.switcherPlaceholder)).toBeNull();
  });

  /// 命令面板的另一半（U1-14）：命令条目执行窗口已有动作——这里点「收起 / 展开侧栏」，
  /// 验证 ChatWindow 的派发接线（弹层关闭 + 侧栏真收起）。
  it("命令面板执行「收起侧栏」命令：关弹层并收起侧栏", async () => {
    window.history.replaceState({}, "", "/?sessionId=7");
    invoke.mockImplementation((command: string, args?: Record<string, unknown>) => {
      if (command === "get_chat_history") {
        return Promise.resolve({ sessionId: 7, title: "当前会话", status: "ended", provider: "claude", cwd: null, supported: true, offset: 0, reset: false, pendingReview: null, items: [], archived: false });
      }
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "get_live_sessions_page") return Promise.resolve([]);
      return Promise.resolve();
    });
    render(<ChatWindow />);
    await screen.findByRole("button", { name: "收起会话列表" });
    fireEvent.keyDown(window, { key: "k", code: "KeyK", ctrlKey: true });
    const input = await screen.findByPlaceholderText(zh.chat.switcherPlaceholder);
    // 空查询时无会话：active=0 即第一条命令（新建会话）；End 跳到最后一项再往回到侧栏命令，
    // 直接点击更直给——点击路径与 Enter 共用同一条 run()。
    fireEvent.click(screen.getByRole("option", { name: new RegExp(zh.chat.shortcutSidebar) }));
    expect(screen.queryByPlaceholderText(zh.chat.switcherPlaceholder)).toBeNull();
    await waitFor(() => expect(screen.queryByRole("button", { name: "收起会话列表" })).toBeNull());
    expect(localStorage.getItem("meowo-chat-sidebar-collapsed")).toBe("1");
    expect(input).toBeTruthy();
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
    let wrote = false;
    // 确认走应用内原生小窗(invoke confirm_dialog):按队列依次给答案,不再 mock 系统 confirm。
    const confirmAnswers: boolean[] = [false, true];
    invoke.mockImplementation((command: string, args?: Record<string, unknown>) => {
      if (command === "confirm_dialog") return Promise.resolve(confirmAnswers.shift() ?? false);
      if (command === "get_chat_history") return Promise.resolve(history);
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      // 接管后的重发走写后回显验证(submitToTerminal 的 verify),write 之后补回显。
      if (command === "write_managed_terminal") {
        if (String((args as { data?: string } | undefined)?.data ?? "").includes("别起第二个")) wrote = true;
        return Promise.resolve();
      }
      if (command === "managed_terminal_snapshot") {
        // 接管前没有托管 PTY；接管后有，且已画出可见内容。
        return Promise.resolve(takenOver
          ? { sessionId: 15, active: true, managed: true, data: b64utf8(wrote ? "ready ❯ 别起第二个" : "ready"), startOffset: 0, endOffset: wrote ? 30 : 5, exited: false, exitCode: null }
          : { sessionId: 15, active: false, managed: false, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      }
      // 进程是否真活着由后端按 pid 判定——前端不再靠 status 猜，而是让这次 start 被拒。
      if (command === "start_managed_terminal") return Promise.reject("会话仍在外部终端运行，不能重复接管");
      if (command === "takeover_managed_terminal") { takenOver = true; return Promise.resolve(); }
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const input = await screen.findByRole("combobox", { name: "发送消息给 Agent" }) as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: "别起第二个" } });
    fireEvent.click(screen.getByRole("button", { name: "发送" }));

    expect(await screen.findByText(/在外部终端运行/)).toBeTruthy();
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

  /** C-8：接管条的显隐挂 needsTakeover 而不是 sendError。打字会清掉 sendError（输入框
   *  onChange 的 setSendError("")），但外部进程还活着，接管入口不能跟着消失——此前
   *  一敲键盘「结束外部进程并接管」按钮就没了，needsTakeover 还悬着。 */
  it("打字清掉发送错误后,接管入口仍在(C-8)", async () => {
    window.history.replaceState({}, "", "/?sessionId=21");
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 21, title: "外部运行中", status: "running", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [],
      });
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "managed_terminal_snapshot") return Promise.resolve({
        sessionId: 21, active: false, managed: false, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null,
      });
      // 后端按 pid 判定进程仍活着 → 拒绝起托管终端,前端给出就地接管入口。
      if (command === "start_managed_terminal") return Promise.reject("会话仍在外部终端运行，不能重复接管");
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const input = await screen.findByRole("combobox", { name: "发送消息给 Agent" });
    fireEvent.change(input, { target: { value: "在吗" } });
    fireEvent.click(screen.getByRole("button", { name: "发送" }));
    // 发送被拒 → 接管入口出现。
    expect(await screen.findByRole("button", { name: "结束外部进程并接管" })).toBeTruthy();
    // 继续打字：错误文本被清掉,但接管入口必须留在原地。
    fireEvent.change(input, { target: { value: "在吗?" } });
    expect(screen.getByRole("button", { name: "结束外部进程并接管" })).toBeTruthy();
  });

  /** 会话被外部终端占用（后端明确 ptyManaged=false）时门卡横幅给出接管动线，composer
   *  禁用而非卸载——输入框与草稿留在原地（C-15）；接管成功（ptyManaged 翻真）后解禁。
   *  （上一条用例的 mock 不带 ptyManaged 字段，覆盖的是信号缺失时的兜底旧路径。） */
  it("外部占用的会话直接显示接管门卡,接管成功后 composer 回归", async () => {
    window.history.replaceState({}, "", "/?sessionId=19");
    let takenOver = false;
    invoke.mockImplementation((command: string) => {
      if (command === "confirm_dialog") return Promise.resolve(true);
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 19, title: "外部占用", status: "running", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [],
        ptyManaged: takenOver,
      });
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve(takenOver
          ? { sessionId: 19, active: true, managed: true, data: btoa("ready"), startOffset: 0, endOffset: 5, exited: false, exitCode: null }
          : { sessionId: 19, active: false, managed: false, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      }
      if (command === "takeover_managed_terminal") { takenOver = true; return Promise.resolve(); }
      return Promise.resolve();
    });
    render(<ChatWindow />);
    expect(await screen.findByText(/在外部终端运行/)).toBeTruthy();
    // 门卡态：输入框在但禁用——草稿不藏，只是发不出去。
    const gatedBox = screen.getByRole("combobox", { name: "发送消息给 Agent" });
    expect((gatedBox as HTMLTextAreaElement).disabled).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "结束外部进程并接管" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("takeover_managed_terminal", { sessionId: 19, cols: 100, rows: 30 }));
    // ptyManaged 随下一轮轮询翻真 → 门卡撤下，composer 解禁。
    const box = await screen.findByRole("combobox", { name: "发送消息给 Agent" }, { timeout: 3_000 });
    await waitFor(() => expect((box as HTMLTextAreaElement).disabled).toBe(false));
  });

  /** 已结束的会话同理：composer 禁用而非卸载（草稿不藏），门卡横幅的「恢复会话」是
   *  唯一动线——只拉起托管终端，不发送任何内容，恢复完成后 composer 解禁。 */
  it("已结束的会话显示恢复门卡,点恢复只拉起终端不发内容", async () => {
    window.history.replaceState({}, "", "/?sessionId=20");
    let resumed = false;
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 20, title: "已结束", status: "ended", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [], connected: false,
        ptyManaged: resumed,
      });
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve(resumed
          ? { sessionId: 20, active: true, managed: true, data: btoa("ready"), startOffset: 0, endOffset: 5, exited: false, exitCode: null }
          : { sessionId: 20, active: false, managed: false, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      }
      if (command === "start_managed_terminal") { resumed = true; return Promise.resolve(); }
      return Promise.resolve();
    });
    render(<ChatWindow />);
    expect(await screen.findByText(zh.chat.composerGateEnded)).toBeTruthy();
    // 门卡态：输入框在但禁用——草稿不藏，只是发不出去。
    const endedBox = screen.getByRole("combobox", { name: "发送消息给 Agent" });
    expect((endedBox as HTMLTextAreaElement).disabled).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "恢复会话" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("start_managed_terminal", expect.objectContaining({ sessionId: 20 })));
    const resumedBox = await screen.findByRole("combobox", { name: "发送消息给 Agent" }, { timeout: 3_000 });
    await waitFor(() => expect((resumedBox as HTMLTextAreaElement).disabled).toBe(false));
    // 恢复只是拉起终端，不发送任何内容。只查本会话：前序用例的异步尾巴可能还在
    // 共享 mock 上落别的会话的写入，那不是本用例要防的。
    expect(invoke.mock.calls.some(
      (call) => call[0] === "write_managed_terminal" && (call[1] as { sessionId?: number } | undefined)?.sessionId === 20,
    )).toBe(false);
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
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const input = await screen.findByRole("combobox", { name: "发送消息给 Agent" }) as HTMLTextAreaElement;
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
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "managed_terminal_snapshot") return Promise.resolve({
        sessionId: 18, active: true, managed: true, data: btoa("screen"), startOffset: 0, endOffset: 6,
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
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
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
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
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
    let wrote = false;
    invoke.mockImplementation((command: string, args?: Record<string, unknown>) => {
      if (command === "get_chat_history") return Promise.resolve(history);
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "start_managed_terminal") { started = true; return Promise.resolve(); }
      // 刚拉起的发送走写后回显验证(submitToTerminal 的 verify),write 之后补回显。
      if (command === "write_managed_terminal") {
        if (String((args as { data?: string } | undefined)?.data ?? "").includes("继续")) wrote = true;
        return Promise.resolve();
      }
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve(started
          ? { sessionId: 16, active: true, managed: true, data: b64utf8(wrote ? "ready ❯ 继续" : "ready"), startOffset: 0, endOffset: wrote ? 20 : 5, exited: false, exitCode: null }
          : { sessionId: 16, active: false, managed: false, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      }
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const input = await screen.findByRole("combobox", { name: "发送消息给 Agent" });
    fireEvent.change(input, { target: { value: "继续" } });
    fireEvent.click(screen.getByRole("button", { name: "发送" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("start_managed_terminal", { sessionId: 16, cols: 100, rows: 30 }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 16, data: "继续" }), { timeout: 3_000 });
    // 等回车也落地再结束本用例:正文与回车之间隔着 SUBMIT_GAP_MS,提前收工的话这次写入会在
    // 后面某个用例执行到一半时才落进共享的 invoke.mock.calls,把那边的「零副作用」断言打翻
    // (真实现场:软拦用例偶发失败在这条 sessionId 16 的 "\r" 上)。
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 16, data: "\r" }), { timeout: 3_000 });
  });

  it("shows elapsed seconds and a terminal shortcut while waiting for the terminal to be ready", async () => {
    // T-8:就绪等待(最长 45s)此前只有转圈,用户分不清「还在启动」还是「卡死」。
    // 头 2 秒不报(快启动是常态,等待条不该一闪而过);之后显示已等秒数 + 「去终端页看」。
    window.history.replaceState({}, "", "/?sessionId=17");
    const history = {
      sessionId: 17, title: "慢启动", status: "ended", provider: "claude", cwd: "C:/repo",
      supported: true, offset: 0, reset: false, pendingReview: null, items: [],
    };
    let started = false;
    let ready = false;
    let wrote = false;
    invoke.mockImplementation((command: string, args?: Record<string, unknown>) => {
      if (command === "get_chat_history") return Promise.resolve(history);
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "start_managed_terminal") { started = true; return Promise.resolve(); }
      if (command === "write_managed_terminal") {
        if (String((args as { data?: string } | undefined)?.data ?? "").includes("继续")) wrote = true;
        return Promise.resolve();
      }
      if (command === "managed_terminal_snapshot") {
        if (!started) return Promise.resolve({ sessionId: 17, active: false, managed: false, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
        // ready 前:活着但一直画不出可见内容,waitForTerminalReady 持续等待;
        // ready 后:可见画面 + 输出静止 → 判就绪(回显验证需要的「继续」随 wrote 补上)。
        return Promise.resolve(ready
          ? { sessionId: 17, active: true, managed: true, data: b64utf8(wrote ? "ready ❯ 继续" : "ready"), startOffset: 0, endOffset: wrote ? 20 : 5, exited: false, exitCode: null }
          : { sessionId: 17, active: true, managed: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      }
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const input = await screen.findByRole("combobox", { name: "发送消息给 Agent" });
    fireEvent.change(input, { target: { value: "继续" } });
    fireEvent.click(screen.getByRole("button", { name: "发送" }));
    expect(await screen.findByText(/正在等待 Agent 终端就绪，已等 \d+ 秒/, {}, { timeout: 6_000 })).toBeTruthy();
    // 等待条里的那个钮(chat-send-takeover)。7G-9 后标签统一成「去终端页」,
    // 与顶部视图切换的「终端」页签不再同名。
    expect(screen.getAllByRole("button", { name: "去终端页" }).some((b) => b.className.includes("chat-send-takeover"))).toBe(true);
    // 放行就绪:等待条收起、消息送达。把在途链路走完再收工,别留轮询污染后面的用例
    // (与上一个用例的「等回车落地」同一纪律)。
    ready = true;
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 17, data: "继续" }), { timeout: 3_000 });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 17, data: "\r" }), { timeout: 3_000 });
    await waitFor(() => expect(screen.queryByText(/正在等待 Agent 终端就绪/)).toBeNull());
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
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "managed_terminal_snapshot") {
        snapshotCalls += 1;
        return Promise.resolve(snapshotCalls === 1
          ? { sessionId: 14, active: false, managed: false, data: "", exited: false, exitCode: null }
          // 失败带 tail(T-8):CLI 拒绝启动的原因随快照的 exitTail 送达,不只一句退出码。
          : { sessionId: 14, active: false, managed: false, data: "launch error", exited: true, exitCode: 1, exitTail: "launch error" });
      }
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const input = await screen.findByRole("combobox", { name: "发送消息给 Agent" }) as HTMLTextAreaElement;
    fireEvent.change(input, { target: { value: "不要丢失" } });
    fireEvent.click(screen.getByRole("button", { name: "发送" }));
    expect(await screen.findByText(/Agent 启动后退出（退出码 1）：launch error/)).toBeTruthy();
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
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "managed_terminal_snapshot") return Promise.resolve({
        sessionId: 45, active: true, managed: true, data: btoa(prompt), startOffset: 0, endOffset: prompt.length,
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
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "managed_terminal_snapshot") return Promise.resolve({
        sessionId: 49, active: true, managed: true, data: btoa(prompt), startOffset: 0, endOffset: prompt.length,
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
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "managed_terminal_snapshot") return Promise.resolve({
        sessionId: 46, active: true, managed: true, data: btoa(unescape(encodeURIComponent(prompt))), startOffset: 0, endOffset: prompt.length,
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
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("kimi"));
      if (command === "managed_terminal_snapshot") return Promise.resolve({
        sessionId: 47, active: true, managed: true,
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
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "managed_terminal_snapshot") return Promise.resolve({
        sessionId: 46, active: true, managed: true, data: btoa(prompt), startOffset: 0, endOffset: prompt.length,
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
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "managed_terminal_snapshot") return Promise.resolve({
        sessionId: 47, active: true, managed: true, data: btoa(prompt), startOffset: 0, endOffset: prompt.length,
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
  it("pendingReview 未识别成卡片时,发送不拦截,亮非阻断提示", async () => {
    window.history.replaceState({}, "", "/?sessionId=48");
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 48, title: "未识别提示", status: "waiting", provider: "codex", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: "question", items: [],
      });
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("codex"));
      if (command === "managed_terminal_snapshot") return Promise.resolve({
        // 屏幕上是识别不出的提示形态(比如 codex 自家的选择器),没有任何卡片。
        sessionId: 48, active: true, managed: true, data: btoa("\x1b[2Jsome unrecognized picker"), startOffset: 0, endOffset: 28,
        exited: false, exitCode: null,
      });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const input = await screen.findByRole("combobox", { name: "发送消息给 Agent" });
    fireEvent.change(input, { target: { value: "继续" } });

    // 软拦已改非阻断:照常发送正文,不弹确认小窗,同时亮一条可跳终端的提示横幅。
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 48, data: "继续" }), { timeout: 3_000 });
    expect(invoke.mock.calls.some(([command]) => command === "confirm_dialog")).toBe(false);
    expect(await screen.findByText(/消息已发出/)).toBeTruthy();
  });

  /**
   * 外部终端跑的会话:提问/审批一律留在终端处理(后端 broker 同口径整段短路)。
   * pendingReview 的降级卡对它不再渲染——弹一张只能「打开终端」的卡只是打扰,
   * 用户就坐在那个终端前(明确反馈)。
   */
  it("外部会话的 pendingReview 不弹「去终端处理」降级卡", async () => {
    window.history.replaceState({}, "", "/?sessionId=51");
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 51, title: "外部提问", status: "waiting", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: "question", items: [], ptyManaged: false,
      });
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      return Promise.resolve();
    });
    render(<ChatWindow />);
    // 正例锚:外部会话 composer 让位给接管门卡——门卡在场说明首轮渲染已完成。
    expect(await screen.findByText(/会话在外部终端运行/)).toBeTruthy();
    expect(screen.queryByText("Agent 正在等待你的回答")).toBeNull();
    expect(screen.queryByRole("button", { name: "去终端页" })).toBeNull();
  });

  /** 对照:托管会话在屏幕未识别出卡片时,降级卡仍是去终端页处理的入口,不得连带消失。 */
  it("托管会话的 pendingReview 降级卡保留", async () => {
    window.history.replaceState({}, "", "/?sessionId=52");
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 52, title: "托管降级", status: "waiting", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: "question", items: [], ptyManaged: true,
      });
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "managed_terminal_snapshot") return Promise.resolve({
        // 屏幕上是识别不出的形态:没有屏幕卡,降级卡是唯一入口。
        sessionId: 52, active: true, managed: true, data: btoa("\x1b[2Junrecognized"), startOffset: 0, endOffset: 14,
        exited: false, exitCode: null,
      });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    expect(await screen.findByText("Agent 正在等待你的回答")).toBeTruthy();
    expect(screen.getByRole("button", { name: "去终端页" })).toBeTruthy();
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
      if (command === "pending_interaction") return Promise.resolve({ approval: pending, question: null });
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
      if (command === "pending_interaction") return Promise.resolve({ approval: pending, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "resolve_pending_approval") { pending = null; return Promise.resolve(); }
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const deny = await screen.findByRole("button", { name: "拒绝" });
    // kbd 徽章是给眼睛的,不进可访问名——上面按 "拒绝" 精确找得到就是证据。
    expect(deny.textContent).toContain("Esc");

    // 焦点在输入框里:这一下归补全菜单/输入框,审批卡不许截走。
    const input = await screen.findByRole("combobox", { name: "发送消息给 Agent" });
    input.focus();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(invoke.mock.calls.some(([command]) => command === "resolve_pending_approval")).toBe(false);

    input.blur();
    // 两段式确认:第一下只把拒绝按钮点亮为「再按 Esc 确认拒绝」,不发拒绝——
    // 空手一个 Esc 就替 agent 递出不可撤销的正式拒绝,误触面太大。
    fireEvent.keyDown(window, { key: "Escape" });
    expect(invoke.mock.calls.some(([command]) => command === "resolve_pending_approval")).toBe(false);
    expect(await screen.findByRole("button", { name: "再按 Esc 确认拒绝" })).toBeTruthy();
    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("resolve_pending_approval", {
      sessionId: 13, requestId: "request-esc", choice: "deny",
    }));
  });

  /** C-10：审批提交失败不再是空 catch 完全静默——原因写进卡内错误行,卡片留在原地
   *  (请求未结算,下一次轮询仍可能恢复它)。顺带钉住 C-9:危险命令(rm -rf)的
   *  「允许一次」染红(is-danger),与安全命令的深色主按钮拉开视觉权重。 */
  it("审批提交失败写卡内错误行,危险命令主按钮染红(C-9/C-10)", async () => {
    window.history.replaceState({}, "", "/?sessionId=22");
    const pending = {
      sessionId: 22, requestId: "request-fail", provider: "claude", toolName: "Bash",
      description: "清理构建产物", input: "rm -rf build", permissionSuggestions: [],
    };
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 22, title: "审批失败", status: "running", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: "approval", items: [], connected: true,
      });
      if (command === "pending_interaction") return Promise.resolve({ approval: pending, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "resolve_pending_approval") return Promise.reject("broker 已结算");
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const allow = await screen.findByRole("button", { name: "允许一次" });
    // rm -rf 与 ls 不同权重:危险命令的允许钮是红色警示主按钮。
    expect(allow.className).toContain("is-danger");
    fireEvent.click(allow);
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("resolve_pending_approval", {
      sessionId: 22, requestId: "request-fail", choice: "allow_once",
    }));
    // 失败原因就地可见,卡片与按钮不消失(此前点了像没点一样)。
    expect(await screen.findByText("broker 已结算")).toBeTruthy();
    expect(screen.getByRole("button", { name: "允许一次" })).toBeTruthy();
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
      if (command === "pending_interaction") return Promise.resolve({
        question: null,
        approval: {
          sessionId: 12, requestId: "request-lean", provider: "codex", toolName: "Bash",
          description: "运行测试", input: "{\"command\":\"cargo test\"}",
          // 刻意没有 permissionSuggestions —— 模拟被 skip 掉字段的瘦负载。
        },
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
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "managed_terminal_binding") return Promise.resolve(null);
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 12, active: true, managed: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
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
   * 提问与审批同策略：会话 B 的 interactive-question 不切窗不弹卡，只亮侧栏徽标。
   * 题面没有 cleared 事件，徽标在用户切到 B 时退场（题面靠轮询取回）。
   */
  it("别的会话提问:不切会话不弹卡,侧栏亮徽标", async () => {
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
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "managed_terminal_binding") return Promise.resolve(null);
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 12, active: true, managed: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
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
      sessionId: 13, requestId: "request-q", provider: "claude", toolName: "AskUserQuestion",
      description: null, input: '{"questions":[{"question":"选哪个?","options":[{"label":"甲"}]}]}',
      permissionSuggestions: [],
    };
    act(() => { eventListeners.get("interactive-question")?.({ payload }); });
    await waitFor(() => expect(otherDot()?.className).toContain("is-approval"));
    // 当前会话不弹题面卡。
    expect(screen.queryByText("选哪个?")).toBeNull();
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
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "managed_terminal_binding") return Promise.resolve(null);
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 31, active: true, managed: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
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
    expect(screen.getByRole("button", { name: "去终端页" })).toBeTruthy();
    // 不是审批卡：没有允许/拒绝，也没有原始 JSON 参数。
    expect(screen.queryByRole("button", { name: "允许一次" })).toBeNull();
    expect(screen.queryByRole("button", { name: "拒绝" })).toBeNull();
    expect(screen.queryByText(/"questions"/)).toBeNull();
    // 点选即排队：提示切换为「已选…」，等屏幕识别确认表单在屏后才自动落键，
    // 不在此刻向 PTY 写任何字节。再点一次取消排队。
    //
    // 断言「**这次点击**没产生写入」而不是「全程零写入」：本用例的 snapshot 返回
    // active:true，会挂载终端组件，它自身的初始化时序在不同平台上可能产生 IPC——
    // 全局零调用的写法因此在 macOS runner 上间歇失败（CI 实测），而那与本用例要
    // 验证的「点选只排队」毫无关系。
    const writes = () =>
      invoke.mock.calls.filter((call) => call[0] === "write_managed_terminal").length;
    const before = writes();
    fireEvent.click(screen.getByRole("button", { name: /autopilot-v2/ }));
    expect(screen.getByText("已选「autopilot-v2」，表单就绪后自动作答")).toBeTruthy();
    expect(writes()).toBe(before);
    fireEvent.click(screen.getByRole("button", { name: /autopilot-v2/ }));
    expect(screen.queryByText(/已选「/)).toBeNull();
  });

  /** 题面卡的「仅收起」留一条可再展开的折叠条（7C-2 尾）；但挂起在别处了结后，折叠条
   *  必须自己撤掉——否则它永远挂着，点开是一张已死的卡，去 resolve 一个不存在的请求
   *  （复核实测）。存活以轮询 pending_interaction 回的 question 为准。 */
  it("题面卡收起后留折叠条；挂起在别处了结时折叠条一并撤掉", async () => {
    window.history.replaceState({}, "", "/?sessionId=33");
    // 先让轮询确认这条挂起还在，收起后折叠条才该留着。
    let questionAlive = true;
    const question = {
      sessionId: 33, requestId: "request-gone", provider: "claude", toolName: "AskUserQuestion",
      description: null, answerable: false,
      input: JSON.stringify({
        questions: [{ header: "去向", question: "接下来做哪一步？", multiSelect: false, options: [{ label: "继续" }] }],
      }),
      permissionSuggestions: [],
    };
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 33, title: "收起会话", status: "running", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [], connected: true,
        ptyManaged: true,
      });
      if (command === "pending_interaction") {
        return Promise.resolve({ approval: null, question: questionAlive ? question : null });
      }
      // 真实链路的耦合：清后端待处理表之后，轮询就再也读不到这条挂起了。上一版 mock
      // 让它返回空 Promise、questionAlive 不跟着翻，于是「收起即被自己秒清」这条回归
      // 溜了过去（复核指出）。这里如实建模：dismiss 一调，挂起即消失。
      if (command === "dismiss_interactive_question") {
        questionAlive = false;
        return Promise.resolve();
      }
      if (command === "managed_terminal_binding") return Promise.resolve(null);
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 33, active: false, managed: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    await waitFor(() => expect(screen.getByText("收起会话")).toBeTruthy());
    expect(await screen.findByText(/接下来做哪一步/, {}, { timeout: 5_000 })).toBeTruthy();

    // 「仅收起」：卡走了，折叠条留下（题面用自己的文案，与屏幕识别那条分得开）。
    const dismissCalls = () => invoke.mock.calls.filter((call) => call[0] === "dismiss_interactive_question").length;
    fireEvent.click(screen.getByRole("button", { name: "仅收起" }));
    expect(screen.queryByText(/接下来做哪一步/)).toBeNull();
    expect(await screen.findByRole("button", { name: "已收起提问，点击展开" })).toBeTruthy();
    // 收起 ≠ 了结：后端待处理表必须留着。清掉的话轮询立刻读不到这条挂起，折叠条会被
    // 判活逻辑自己秒清（复核实测 2.5s 内），恢复入口没了——只断言「折叠条出现过」抓不住
    // 那条回归（最终态同样是消失），得直接钉住「没去清后端表」。
    expect(dismissCalls()).toBe(0);

    // 在终端答掉/超时：下一轮轮询回 question:null，折叠条必须自己消失。
    questionAlive = false;
    await waitFor(
      () => expect(screen.queryByRole("button", { name: "已收起提问，点击展开" })).toBeNull(),
      { timeout: 5_000 },
    );
  });

  /** broker 挂起代答（answerable）：卡片是真正的作答面——多问题跨 tab、多选勾选、
   *  自定义输入，提交把 `answer:<正文>` 发给 resolve 通道，不写一个 PTY 字节。
   *  出口只有提交与「去终端作答」，刻意没有「仅收起」（收起=让 hook 干等 300s）。 */
  it("answerable 题面卡内作答:多问题多选提交 answer 正文", async () => {
    window.history.replaceState({}, "", "/?sessionId=32");
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 32, title: "代答会话", status: "running", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [], connected: true,
        ptyManaged: true,
      });
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "managed_terminal_binding") return Promise.resolve(null);
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 32, active: true, managed: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    await waitFor(() => expect(screen.getByText("代答会话")).toBeTruthy());

    const payload = {
      sessionId: 32, requestId: "request-answerable", provider: "claude", toolName: "AskUserQuestion",
      description: null, answerable: true,
      input: JSON.stringify({
        questions: [
          { header: "晚饭", question: "晚饭吃什么？", multiSelect: false, options: [{ label: "火锅" }, { label: "烧烤" }] },
          { header: "配菜", question: "配菜选哪些？", multiSelect: true, options: [{ label: "毛肚" }, { label: "虾滑" }, { label: "青菜" }] },
        ],
      }),
      permissionSuggestions: [],
    };
    act(() => { eventListeners.get("interactive-question")?.({ payload }); });
    expect(await screen.findByText(/晚饭吃什么/)).toBeTruthy();
    // 作答卡没有「仅收起」，提交键在但没答完先禁用。
    expect(screen.queryByRole("button", { name: "仅收起" })).toBeNull();
    const submit = screen.getByRole("button", { name: "提交选择" });
    expect((submit as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByText("还有 2 题未作答")).toBeTruthy();

    // 第 1 题单选：点选后完成度推进；换选直接替换。
    fireEvent.click(screen.getByRole("button", { name: /火锅/ }));
    expect(screen.getByText("还有 1 题未作答")).toBeTruthy();
    // 第 2 题多选：切 tab 勾两项，再补一句自定义。
    fireEvent.click(screen.getByRole("tab", { name: /配菜/ }));
    fireEvent.click(screen.getByRole("button", { name: /毛肚/ }));
    fireEvent.click(screen.getByRole("button", { name: /虾滑/ }));
    fireEvent.change(screen.getByPlaceholderText("输入其他回答"), { target: { value: "少放辣" } });
    expect(screen.getByText("提交后 Agent 立即继续")).toBeTruthy();
    // tab 上出现已答 ✓（两题都有内容）。
    expect(screen.getAllByLabelText("已作答").length).toBe(2);

    const writes = () =>
      invoke.mock.calls.filter((call) => call[0] === "write_managed_terminal").length;
    const before = writes();
    fireEvent.click(screen.getByRole("button", { name: "提交选择" }));
    await waitFor(() => {
      const resolved = invoke.mock.calls.find((call) => call[0] === "resolve_pending_approval");
      expect(resolved).toBeTruthy();
      const args = resolved![1] as { sessionId: number; requestId: string; choice: string };
      expect(args.sessionId).toBe(32);
      expect(args.requestId).toBe("request-answerable");
      expect(args.choice).toBe("answer:晚饭 · 晚饭吃什么？ → 火锅\n配菜 · 配菜选哪些？ → 毛肚、虾滑、少放辣");
    });
    // 提交即收卡，且全程不写 PTY（代答走 resolve 通道，不是按键回放）。
    await waitFor(() => expect(screen.queryByText(/晚饭吃什么/)).toBeNull());
    expect(writes()).toBe(before);
  });

  /** 作答卡的「去终端作答」不再只是切视图：先 resolve `pass` 把挂起交还终端
   *  （表单随权限流程出现在那里），再切终端页。 */
  it("answerable 题面卡去终端作答:先 resolve pass 再切终端", async () => {
    window.history.replaceState({}, "", "/?sessionId=33");
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 33, title: "交还会话", status: "running", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [], connected: true,
        ptyManaged: true,
      });
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "managed_terminal_binding") return Promise.resolve(null);
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 33, active: true, managed: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    await waitFor(() => expect(screen.getByText("交还会话")).toBeTruthy());
    act(() => {
      eventListeners.get("interactive-question")?.({ payload: {
        sessionId: 33, requestId: "request-to-terminal", provider: "claude", toolName: "AskUserQuestion",
        description: null, answerable: true,
        input: JSON.stringify({ questions: [{ question: "继续吗？", multiSelect: false, options: [{ label: "是" }] }] }),
        permissionSuggestions: [],
      } });
    });
    expect(await screen.findByText(/继续吗/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "去终端页" }));
    await waitFor(() => {
      const resolved = invoke.mock.calls.find((call) => call[0] === "resolve_pending_approval");
      expect(resolved).toBeTruthy();
      expect((resolved![1] as { choice: string }).choice).toBe("pass");
    });
  });

  /** 挂起结算/超时后轮询把 answerable 翻 false：作答卡当场降级为展示卡
   *  （恢复「仅收起」、不再有卡内提交），不需要任何 cleared 事件。
   *  题面用多问题：降级后选项仍可点选排队（按问题 keyed，C-11 尾）——降级只收回
   *  「提交选择」这条直达通道，排队落键路径不受影响。 */
  it("answerable 翻 false 时作答卡降级为展示卡", async () => {
    window.history.replaceState({}, "", "/?sessionId=34");
    const question = {
      sessionId: 34, requestId: "request-degrade", provider: "claude", toolName: "AskUserQuestion",
      description: null, answerable: true,
      input: JSON.stringify({ questions: [
        { header: "配菜", question: "配菜选哪些？", multiSelect: true, options: [{ label: "毛肚" }, { label: "青菜" }] },
        { header: "主食", question: "主食吃什么？", multiSelect: false, options: [{ label: "米饭" }] },
      ] }),
      permissionSuggestions: [],
    };
    let polledQuestion: typeof question | null = null;
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 34, title: "降级会话", status: "running", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [], connected: true,
        ptyManaged: true,
      });
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: polledQuestion });
      if (command === "managed_terminal_binding") return Promise.resolve(null);
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 34, active: true, managed: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    await waitFor(() => expect(screen.getByText("降级会话")).toBeTruthy());
    act(() => { eventListeners.get("interactive-question")?.({ payload: question }); });
    expect(await screen.findByRole("button", { name: "提交选择" })).toBeTruthy();
    // 挂起在后端结算：同 requestId 但 answerable=false 从轮询回来。
    polledQuestion = { ...question, answerable: false };
    await waitFor(() => expect(screen.queryByRole("button", { name: "提交选择" })).toBeNull());
    // 降级为展示形态：选项变成点选排队按钮（多问题按问题 keyed 排队），
    // 点「毛肚」进排队并提示「已选…」。
    fireEvent.click(await screen.findByRole("button", { name: /毛肚/ }));
    expect(screen.getByText(/已选「毛肚」/)).toBeTruthy();
  });

  /** C-11 尾：多问题展示卡按问题 keyed 排队 + 聚焦题识别落键。两题都点上答案后，
   *  屏幕停在第 2 题（题面原文在屏）→ 只落第 2 题的答案，第 1 题的排队保留等轮到它。 */
  it("多问题展示卡按问题排队：识别出第 2 题在屏时只落第 2 题的答案", async () => {
    window.history.replaceState({}, "", "/?sessionId=36");
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 36, title: "多题会话", status: "running", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [], connected: true,
        ptyManaged: true,
      });
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "managed_terminal_binding") return Promise.resolve(null);
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 36, active: true, managed: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    await waitFor(() => expect(screen.getByText("多题会话")).toBeTruthy());
    act(() => {
      eventListeners.get("interactive-question")?.({ payload: {
        sessionId: 36, requestId: "request-keyed", provider: "claude", toolName: "AskUserQuestion",
        description: null, answerable: false,
        input: JSON.stringify({ questions: [
          { header: "配菜", question: "配菜选哪些？", multiSelect: true, options: [{ label: "毛肚" }, { label: "虾滑" }] },
          { header: "主食", question: "主食吃什么？", multiSelect: false, options: [{ label: "米饭" }, { label: "面条" }] },
        ] }),
        permissionSuggestions: [],
      } });
    });
    // 第 1 题（默认激活 tab）点「毛肚」；切到第 2 题 tab 点「米饭」。两题排队都进提示。
    fireEvent.click(await screen.findByRole("button", { name: /毛肚/ }));
    fireEvent.click(screen.getByRole("tab", { name: /主食/ }));
    fireEvent.click(screen.getByRole("button", { name: /米饭/ }));
    expect(screen.getByText(/已选「毛肚、米饭」/)).toBeTruthy();
    // 屏幕识别接管：表单停在第 2 题（实拍形态：tab 条只有 header，题面原文在选项上方）。
    const form = [
      "\x1b[2J← 配菜 主食 ✓ Submit →",
      "主食吃什么？",
      "❯ 1. 米饭",
      "  2. 面条",
      "  3. Type something.",
      "Enter to select · Tab/Arrow keys to navigate · Esc to cancel",
    ].join("\r\n");
    const attention = terminalAttention(form, [], true);
    expect(attention?.id).toBe("interactive:numbered-selector");
    act(() => { (terminalProps.current?.onAttention as (a: unknown) => void)(attention); });
    // 只落第 2 题的答案：❯ 已停在「米饭」，直接回车；第 1 题的「毛肚」绝不动。
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 36, data: "\r" }));
    expect(invoke).not.toHaveBeenCalledWith("write_managed_terminal", { sessionId: 36, data: "\x1b[B\r" });
  });

  /** C-11：单问题多选的展示卡（非 answerable 路径）可点选排队——勾选累加进
   *  queuedAnswers 并提示「已选…」，再点取消；多问题的跨题落键见上个用例。 */
  it("单问题多选展示卡可点选排队：勾选累加、再点取消", async () => {
    window.history.replaceState({}, "", "/?sessionId=35");
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 35, title: "排队会话", status: "running", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [], connected: true,
        ptyManaged: true,
      });
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "managed_terminal_binding") return Promise.resolve(null);
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 35, active: true, managed: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    await waitFor(() => expect(screen.getByText("排队会话")).toBeTruthy());
    act(() => {
      eventListeners.get("interactive-question")?.({ payload: {
        sessionId: 35, requestId: "request-queue-multi", provider: "claude", toolName: "AskUserQuestion",
        description: null, answerable: false,
        input: JSON.stringify({ questions: [
          { header: "配菜", question: "配菜选哪些？", multiSelect: true, options: [{ label: "毛肚" }, { label: "虾滑" }] },
        ] }),
        permissionSuggestions: [],
      } });
    });
    const tripe = await screen.findByRole("button", { name: /毛肚/ });
    const shrimp = screen.getByRole("button", { name: /虾滑/ });
    // 多选可攒多个：两个都勾上，提示带排队中的答案。
    fireEvent.click(tripe);
    fireEvent.click(shrimp);
    expect(screen.getByText(/已选「毛肚、虾滑」/)).toBeTruthy();
    // 再点取消该项，提示回落到「点选答案…」前的单选态。
    fireEvent.click(screen.getByRole("button", { name: /虾滑/ }));
    expect(screen.getByText(/已选「毛肚」/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /毛肚/ }));
    expect(screen.getByText("点选答案，终端表单就绪后自动作答")).toBeTruthy();
  });

  /**
   * 用户消息里的图片引用（Claude Code 记成「[Image: source: 本地路径]」）渲染为缩略图，
   * 原始路径绝不上屏——它是对话里最脏的元素（asset 加载失败时降级为文件名徽章）。
   */
  /** `@` 一打出就要有清单（目录浏览模式），选文件插入 @相对路径——「打了 @ 没反应」
   *  是最直接的功能不可见（实拍反馈：曾要求 ≥2 字符才查询，@ 后一片死寂）。 */
  it("@ 文件补全:打出 @ 即列目录,选中插入路径", async () => {
    window.history.replaceState({}, "", "/?sessionId=61");
    respondWithHistory({
      sessionId: 61, title: "补全", status: "waiting", provider: "claude", cwd: "C:/repo",
      supported: true, offset: 0, reset: false, pendingReview: null, items: [],
    });
    const base = invoke.getMockImplementation()!;
    invoke.mockImplementation((command: string, args?: Record<string, unknown>) => {
      if (command === "list_dir_entries") return Promise.resolve([
        { name: "src", relPath: "src", isDir: true },
        { name: "README.md", relPath: "README.md", isDir: false },
      ]);
      return base(command, args);
    });
    render(<ChatWindow />);
    const input = await screen.findByRole("combobox", { name: "发送消息给 Agent" });
    fireEvent.change(input, { target: { value: "@" } });
    // 目录带 / 后缀可下钻；文件点击即插入完整 @路径 + 空格，菜单收起。
    expect(await screen.findByRole("option", { name: "@src/" })).toBeTruthy();
    fireEvent.click(await screen.findByRole("option", { name: "@README.md" }));
    await waitFor(() => expect((input as HTMLTextAreaElement).value).toBe("@README.md "));
    expect(screen.queryByRole("option", { name: "@README.md" })).toBeNull();
  });

  it("图片引用渲染为缩略图,原始路径不上屏", async () => {
    window.history.replaceState({}, "", "/?sessionId=41");
    respondWithHistory({
      sessionId: 41, title: "贴图", status: "running", provider: "claude", cwd: "C:/repo",
      supported: true, offset: 1, reset: false, pendingReview: null,
      items: [
        { type: "user_text", id: "u1", timestamp: null, text: "看这张图 [Image: source: C:\\Users\\me\\.claude\\image-cache\\abc\\1.png]" },
      ],
    });
    render(<ChatWindow />);
    const img = await screen.findByAltText("1.png");
    expect((img as HTMLImageElement).src).toContain("1.png");
    expect(screen.queryByText(/image-cache/)).toBeNull();
    expect(screen.getByText(/看这张图/)).toBeTruthy();
    // 点击缩略图开灯箱看大图，Esc 关闭。
    fireEvent.click(img.closest("button")!);
    const lightbox = screen.getByRole("dialog", { name: "1.png" });
    expect(lightbox.querySelector("img")).toBeTruthy();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("dialog", { name: "1.png" })).toBeNull();
    // 再开一次，走明显的关闭按钮；缩放按钮存在且百分比随点击变化。
    fireEvent.click(img.closest("button")!);
    fireEvent.click(screen.getByRole("button", { name: "放大" }));
    expect(screen.getByText("125%")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "关闭大图" }));
    expect(screen.queryByRole("dialog", { name: "1.png" })).toBeNull();
  });

  /** Claude Code 把一次多图粘贴记成连续多条独立的纯图片消息——必须合并成一行缩略图，
   *  否则竖着摞一列大图（用户实拍反馈过）。 */
  it("连续多条纯图片消息合并成一行缩略图", async () => {
    window.history.replaceState({}, "", "/?sessionId=42");
    respondWithHistory({
      sessionId: 42, title: "多图", status: "running", provider: "claude", cwd: "C:/repo",
      supported: true, offset: 2, reset: false, pendingReview: null,
      items: [
        { type: "user_text", id: "u1", timestamp: null, text: "[Image: source: C:\\cache\\a.png]" },
        { type: "user_text", id: "u2", timestamp: null, text: "[Image: source: C:\\cache\\b.png]" },
      ],
    });
    render(<ChatWindow />);
    await screen.findByAltText("a.png");
    const rows = document.querySelectorAll(".chat-image-row");
    expect(rows.length).toBe(1);
    expect(rows[0].querySelectorAll("img").length).toBe(2);
  });

  /** CLI 会把附件行「- <路径>」里的图片路径吃掉、转成独立 image 块：transcript 文本
   *  只剩指令头 + 光杆「-」，没有任何 [Image: source:] 引用。这形状同样要剥——
   *  曾因 splitUserText 在无图片引用时早退，把指令头和「-」原样上屏（实拍回归）。 */
  it("附件路径被 CLI 转成 image 块后,指令头与光杆弹头行不上屏", async () => {
    window.history.replaceState({}, "", "/?sessionId=44");
    respondWithHistory({
      sessionId: 44, title: "贴图残行", status: "running", provider: "claude", cwd: "C:/repo",
      supported: true, offset: 1, reset: false, pendingReview: null,
      items: [
        { type: "user_text", id: "u1", timestamp: null, text: "[Image #2]这个高度能不能低一点\n请读取并结合以下本地附件完成任务（图片请使用图像读取能力）：\n-" },
      ],
    });
    render(<ChatWindow />);
    await screen.findByText(/这个高度能不能低一点/);
    expect(screen.queryByText(/本地附件/)).toBeNull();
    expect(screen.queryByText(/^-$/)).toBeNull();
  });

  /** CC 还会把光杆指代「[Image #N]」挪到提交文本最前,与指令头粘成一行
   *  (「[Image #1]请读取并结合…」)——精确比对指令头会失配,整段样板漏进气泡
   *  (远程发图实拍)。判头前须先剥光杆指代。 */
  it("光杆 [Image #N] 粘在指令头行首时,样板照样剥净", async () => {
    window.history.replaceState({}, "", "/?sessionId=45");
    respondWithHistory({
      sessionId: 45, title: "贴图粘头", status: "running", provider: "claude", cwd: "C:/repo",
      supported: true, offset: 1, reset: false, pendingReview: null,
      items: [
        { type: "user_text", id: "u1", timestamp: null, text: "[Image #1]请读取并结合以下本地附件完成任务（图片请使用图像读取能力）：\n-" },
      ],
    });
    render(<ChatWindow />);
    await waitFor(() => expect(screen.getByText("贴图粘头")).toBeTruthy());
    expect(screen.queryByText(/本地附件/)).toBeNull();
    expect(screen.queryByText(/\[Image #1\]/)).toBeNull();
  });

  /** 只剥「文本顶端机器前缀」里的独行光杆:用户在正文中手打的 [Image #N] 是内容,
   *  不能一起吃掉——否则气泡与真正发出的 prompt 不一致(审查回归,非远程限定)。 */
  it("用户正文里手打的独行 [Image #N] 不被附件剥离误删", async () => {
    window.history.replaceState({}, "", "/?sessionId=46");
    respondWithHistory({
      sessionId: 46, title: "用户指代", status: "running", provider: "claude", cwd: "C:/repo",
      supported: true, offset: 1, reset: false, pendingReview: null,
      items: [
        { type: "user_text", id: "u1", timestamp: null, text: "[Image #1]重点看这张\n[Image #2]\n其余忽略\n\n请读取并结合以下本地附件完成任务（图片请使用图像读取能力）：\n-" },
      ],
    });
    render(<ChatWindow />);
    await screen.findByText(/其余忽略/);
    // 顶端机器前缀被剥,样板不上屏。
    expect(screen.queryByText(/本地附件/)).toBeNull();
    // 用户在正文里手打的 [Image #2] 保留(它是内容,不是机器残留)。
    expect(screen.getByText(/\[Image #2\]/)).toBeTruthy();
  });

  /** kimi 文本回退时不吃指令里的路径行:「指令头 + '- <图片绝对路径>'」整段落进文本
   *  (claude 会转成 [Image: source: …] 引用行)。有指令头时图片路径行抽成 chip、
   *  头与路径行都不上屏;单图/多图同口径。 */
  it("kimi 式指令头 + 图片路径行:抽成缩略图 chip,头与路径不上屏", async () => {
    window.history.replaceState({}, "", "/?sessionId=47");
    respondWithHistory({
      sessionId: 47, title: "kimi 路径行", status: "running", provider: "kimi", cwd: "C:/repo",
      supported: true, offset: 1, reset: false, pendingReview: null,
      items: [
        { type: "user_text", id: "u1", timestamp: null, text: "请读取并结合以下本地附件完成任务（图片请使用图像读取能力）：\n- C:\\Users\\me\\meowo-paste\\a.png\n- D:\\img\\b 2.jpeg" },
      ],
    });
    render(<ChatWindow />);
    await screen.findByAltText("a.png");
    expect(screen.getByAltText("b 2.jpeg")).toBeTruthy();
    expect(screen.queryByText(/本地附件/)).toBeNull();
    expect(screen.queryByText(/meowo-paste/)).toBeNull();
  });

  /** 混排非图附件行维持既有保留语义:图片路径行抽成 chip,指令头与「- <非图路径>」
   *  原文保留(头要留着解释那行附件是什么)。 */
  it("kimi 式混排附件:图片行抽 chip,非图附件行与指令头保留", async () => {
    window.history.replaceState({}, "", "/?sessionId=48");
    respondWithHistory({
      sessionId: 48, title: "kimi 混排", status: "running", provider: "kimi", cwd: "C:/repo",
      supported: true, offset: 1, reset: false, pendingReview: null,
      items: [
        { type: "user_text", id: "u1", timestamp: null, text: "请读取并结合以下本地附件完成任务（图片请使用图像读取能力）：\n- C:\\img\\a.png\n- C:\\docs\\notes.txt" },
      ],
    });
    render(<ChatWindow />);
    await screen.findByAltText("a.png");
    expect(screen.getByText(/本地附件/)).toBeTruthy();
    expect(screen.getByText(/notes\.txt/)).toBeTruthy();
    expect(screen.queryByText(/C:\\img/)).toBeNull();
  });

  /** 无指令头时不动:用户手打的「- <路径>」列表是正文内容,不能当附件吃掉。 */
  it("无指令头的「- 图片路径」行是用户正文,不抽 chip", async () => {
    window.history.replaceState({}, "", "/?sessionId=49");
    respondWithHistory({
      sessionId: 49, title: "手打列表", status: "running", provider: "kimi", cwd: "C:/repo",
      supported: true, offset: 1, reset: false, pendingReview: null,
      items: [
        { type: "user_text", id: "u1", timestamp: null, text: "这几个文件看下：\n- C:\\img\\a.png" },
      ],
    });
    render(<ChatWindow />);
    await screen.findByText(/C:\\img\\a\.png/);
    expect(screen.queryByAltText("a.png")).toBeNull();
  });

  /** 多问题题面用 tab 切换：全部竖排会把卡片堆得比对话区还高、把输入框挤出可视区。 */
  it("多问题题面渲染成 tab,一次只显示一题", async () => {
    window.history.replaceState({}, "", "/?sessionId=43");
    respondWithHistory({
      sessionId: 43, title: "多题", status: "running", provider: "claude", cwd: "C:/repo",
      supported: true, offset: 0, reset: false, pendingReview: null, items: [], ptyManaged: true,
    });
    render(<ChatWindow />);
    await waitFor(() => expect(screen.getByText("多题")).toBeTruthy());
    const payload = {
      sessionId: 43, requestId: "request-multi", provider: "claude", toolName: "AskUserQuestion",
      description: null,
      input: JSON.stringify({
        questions: [
          { header: "日志方案", question: "日志怎么处理？", multiSelect: false, options: [{ label: "自建本地日志" }] },
          { header: "auth 迁移", question: "现在做吗？", multiSelect: false, options: [{ label: "现在做" }] },
        ],
      }),
      permissionSuggestions: [],
    };
    act(() => { eventListeners.get("interactive-question")?.({ payload }); });
    // tab 条出现，默认停在第一题：第二题的内容不可见。
    expect(await screen.findByText(/日志怎么处理/)).toBeTruthy();
    expect(screen.getByRole("tab", { name: "日志方案" })).toBeTruthy();
    expect(screen.queryByText(/现在做吗/)).toBeNull();
    // 切到第二题：内容互换。
    fireEvent.click(screen.getByRole("tab", { name: "auth 迁移" }));
    expect(screen.getByText(/现在做吗/)).toBeTruthy();
    expect(screen.queryByText(/日志怎么处理/)).toBeNull();
  });

  /**
   * 冷启动路径：`interactive-question` 事件在 WebView2 起来之前发出（emit 不排队、
   * 不重放），窗口起来时事件早已消失——题面卡必须靠轮询补回来。这里刻意**不**投递
   * 事件，只让 pending_interactive_question 返回题面。
   */
  it("事件错过时,轮询把题面卡补回来", async () => {
    window.history.replaceState({}, "", "/?sessionId=33");
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 33, title: "冷启动提问", status: "running", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [], connected: true,
        ptyManaged: true,
      });
      if (command === "managed_terminal_binding") return Promise.resolve(null);
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 33, active: true, managed: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      if (command === "pending_interaction") return Promise.resolve({
        approval: null,
        question: {
          sessionId: 33, requestId: "request-cold", provider: "claude", toolName: "AskUserQuestion",
          description: null,
          input: JSON.stringify({ questions: [{ question: "冷启动也要能看到题面？", options: [{ label: "必须能" }] }] }),
          permissionSuggestions: [],
        },
      });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    // 没有任何 interactive-question 事件，卡仍然出现。
    expect(await screen.findByText(/冷启动也要能看到题面/)).toBeTruthy();
    expect(screen.getByText("必须能")).toBeTruthy();
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
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "managed_terminal_binding") return Promise.resolve(null);
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 32, active: false, managed: false, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
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
    expect(screen.queryByRole("button", { name: "去终端页" })).toBeNull();
    expect(screen.getByText(/请回到运行它的终端作答/)).toBeTruthy();
  });

  /**
   * 附件注入按 agent 能力分流:声明了 attachment_mention(claude/gemini,实测 @绝对路径
   * 在提交时被原生附加)就用 `@路径` 提及;图片优先走剪贴板原生附加(Ctrl-V),写不进
   * 剪贴板时退回指令文本;含空白的路径同样退回(提及会在空白处截断)。
   */
  it("claude 附件走原生 @提及,剪贴板写失败的图片退回指令文本兜底", async () => {
    window.history.replaceState({}, "", "/?sessionId=21");
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 21, title: "附件注入", status: "waiting", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [], connected: true,
      });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "managed_terminal_binding") return Promise.resolve(null);
      // 剪贴板被占用/附件文件已删:写不进去 → 退回指令文本(绝不能 Ctrl-V 贴别人的内容)。
      if (command === "clipboard_set_image") return Promise.reject(new Error("clipboard occupied"));
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 21, active: true, managed: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    await waitFor(() => expect(screen.getByText("附件注入")).toBeTruthy());
    const box = () => screen.getByRole("combobox", { name: "发送消息给 Agent" });

    // 纯文本文件 + 无空白路径 → 原生 @提及,不再有指令文本。
    openDialog.mockResolvedValueOnce(["C:\\repo\\notes.txt"]);
    fireEvent.click(screen.getByRole("button", { name: "添加图片或文件" }));
    await waitFor(() => expect(screen.getByText("notes.txt")).toBeTruthy());
    fireEvent.change(box(), { target: { value: "看看这个" } });
    fireEvent.keyDown(box(), { key: "Enter" });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 21, data: "@C:\\repo\\notes.txt 看看这个" }));
    // 等提交回车落地(SUBMIT_GAP_MS 之后),sending 才复位,下一次发送才不会被守卫吞掉。
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 21, data: "\r" }));

    // 图片 → 剪贴板写失败,退回指令文本(原生附加见下两个用例)。
    openDialog.mockResolvedValueOnce(["C:\\repo\\shot.png"]);
    fireEvent.click(screen.getByRole("button", { name: "添加图片或文件" }));
    // 图片附件是缩略图（alt=文件名）。
    await waitFor(() => expect(screen.getByAltText("shot.png")).toBeTruthy());
    fireEvent.change(box(), { target: { value: "看图" } });
    fireEvent.keyDown(box(), { key: "Enter" });
    // 多行文本经括号粘贴包裹送入 PTY(\x1b[200~…\x1b[201~),指令文本恒多行,断言带包裹。
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", {
      sessionId: 21,
      data: `[200~看图\n\n请读取并结合以下本地附件完成任务（图片请使用图像读取能力）：\n- C:\\repo\\shot.png[201~`,
    }));
  });

  /**
   * 剪贴板原生图片附加:附件全是图片 → 逐张写剪贴板 + Ctrl-V 让 TUI 自己读(claude 原生
   * [Image #N]),占位符确认后写正文提交,全程不出现指令文本;结束还原剪贴板快照。
   */
  it("纯图片附件:写剪贴板 + Ctrl-V 原生附加,占位符确认后写正文", async () => {
    window.history.replaceState({}, "", "/?sessionId=22");
    let pasted = false;
    invoke.mockImplementation((command: string, args?: Record<string, unknown>) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 22, title: "原生图", status: "waiting", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [], connected: true,
      });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "managed_terminal_binding") return Promise.resolve(null);
      if (command === "save_pasted_attachment") return Promise.resolve("C:\\Temp\\meowo-paste\\1-0\\image.png");
      if (command === "clipboard_set_image") return Promise.resolve();
      if (command === "write_managed_terminal" && args?.data === "\x16") {
        pasted = true;
        return Promise.resolve();
      }
      if (command === "managed_terminal_snapshot") {
        // ^V 之后屏幕出现 claude 的原生占位符;mock 无状态,重复返回同段增量无妨。
        const data = pasted ? btoa("> [Image #1]") : "";
        return Promise.resolve({ sessionId: 22, active: true, managed: true, data, startOffset: 0, endOffset: pasted ? 12 : 0, exited: false, exitCode: null });
      }
      return Promise.resolve();
    });
    render(<ChatWindow />);
    await waitFor(() => expect(screen.getByText("原生图")).toBeTruthy());
    const box = () => screen.getByRole("combobox", { name: "发送消息给 Agent" });
    fireEvent.paste(box(), { clipboardData: { files: [new File([new Uint8Array([137, 80])], "image.png", { type: "image/png" })] } });
    await waitFor(() => expect(screen.getByAltText("image.png")).toBeTruthy());
    fireEvent.change(box(), { target: { value: "看这张图" } });
    fireEvent.keyDown(box(), { key: "Enter" });
    // 先把附件写进剪贴板(以落盘路径为凭),再发 Ctrl-V。
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("clipboard_set_image", { path: "C:\\Temp\\meowo-paste\\1-0\\image.png" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 22, data: "\x16" }));
    // 占位符确认(首个 250ms 轮询)+ SUBMIT_GAP 后正文与回车相继写入。
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 22, data: "看这张图" }), { timeout: 3000 });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 22, data: "\r" }), { timeout: 3000 });
    // 全程没有指令文本注入;结束后还原剪贴板快照。
    const writes = invoke.mock.calls.filter((call) => call[0] === "write_managed_terminal");
    expect(writes.every((call) => !String((call[1] as { data: string }).data).includes("请读取并结合"))).toBe(true);
    expect(invoke).toHaveBeenCalledWith("clipboard_restore");
  });

  /**
   * 多张图片:逐张写剪贴板、逐张等占位符计数达标再贴下一张——两家 TUI 的 composer 都
   * 支持多图连续粘贴,原生化不应只覆盖单图(用户实拍:多图退回指令文本,路径裸奔)。
   */
  it("多张图片:逐张写剪贴板连续 Ctrl-V,占位符计数逐个达标", async () => {
    window.history.replaceState({}, "", "/?sessionId=23");
    let pasted = 0;
    let saved = 0;
    invoke.mockImplementation((command: string, args?: Record<string, unknown>) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 23, title: "多图", status: "waiting", provider: "kimi", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [], connected: true,
      });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("kimi"));
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "managed_terminal_binding") return Promise.resolve(null);
      if (command === "save_pasted_attachment") {
        saved += 1;
        return Promise.resolve(`C:\\Temp\\meowo-paste\\3-0\\image${saved}.png`);
      }
      if (command === "clipboard_set_image") return Promise.resolve();
      if (command === "write_managed_terminal" && args?.data === "\x1bv") {
        pasted += 1;
        return Promise.resolve();
      }
      if (command === "managed_terminal_snapshot") {
        // kimi 的原生占位符带尺寸;第 N 次 Alt+V 后屏幕上累计 N 个。
        const line = pasted >= 2 ? "> [image #1 (10×10)] [image #2 (20×20)]" : pasted === 1 ? "> [image #1 (10×10)]" : "";
        return Promise.resolve({ sessionId: 23, active: true, managed: true, data: line ? btoa(line) : "", startOffset: 0, endOffset: line.length, exited: false, exitCode: null });
      }
      return Promise.resolve();
    });
    render(<ChatWindow />);
    await waitFor(() => expect(screen.getByText("多图")).toBeTruthy());
    const box = () => screen.getByRole("combobox", { name: "发送消息给 Agent" });
    fireEvent.paste(box(), { clipboardData: { files: [
      new File([new Uint8Array([137, 80])], "image1.png", { type: "image/png" }),
      new File([new Uint8Array([137, 80])], "image2.png", { type: "image/png" }),
    ] } });
    await waitFor(() => expect(screen.getByAltText("image2.png")).toBeTruthy());
    fireEvent.change(box(), { target: { value: "做成类似这样的" } });
    fireEvent.keyDown(box(), { key: "Enter" });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 23, data: "做成类似这样的" }), { timeout: 3000 });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 23, data: "\r" }), { timeout: 3000 });
    // 两张各写一次剪贴板、顺序与附件一致;各发一次粘贴键——kimi 在 Windows 上是 Alt+V
    // (\x1bv),不是 Ctrl-V(发 \x16 会被 composer 无视,用户实拍故障)。
    const sets = invoke.mock.calls.filter((call) => call[0] === "clipboard_set_image");
    expect(sets.map((call) => (call[1] as { path: string }).path)).toEqual([
      "C:\\Temp\\meowo-paste\\3-0\\image1.png",
      "C:\\Temp\\meowo-paste\\3-0\\image2.png",
    ]);
    expect(invoke.mock.calls.filter((call) => call[0] === "write_managed_terminal" && (call[1] as { data: string }).data === "\x1bv")).toHaveLength(2);
    // 全程没有指令文本注入;结束后还原剪贴板快照。
    const writes = invoke.mock.calls.filter((call) => call[0] === "write_managed_terminal");
    expect(writes.every((call) => !String((call[1] as { data: string }).data).includes("请读取并结合"))).toBe(true);
    expect(invoke).toHaveBeenCalledWith("clipboard_restore");
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
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "managed_terminal_binding") return Promise.resolve(null);
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 90, active: false, managed: false, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    await waitFor(() => expect(screen.getByText("假运行")).toBeTruthy());
    // 标题栏状态徽标已移除:断言它不复活,且不出现任何「运行中」指示。
    expect(document.querySelector(".chat-live")).toBeNull();
    expect(screen.queryByText("运行中")).toBeNull();
    expect(document.querySelector(".chat-running")).toBeNull();
    // 窗口标题不带运行记号。
    await waitFor(() => expect(setTitleMock).toHaveBeenCalledWith("假运行 · Meowo"));
  });

  /**
   * 标题栏状态徽标已整体移除(运行态有窗口标题「▶」记号/底部运行条/侧栏状态点三处
   * 冗余信号,错误态由侧栏状态点承载):errored 会话不再渲染徽标——防悄悄回归。
   */
  it("errored 会话标题栏不再渲染状态徽标", async () => {
    window.history.replaceState({}, "", "/?sessionId=91");
    respondWithHistory({
      sessionId: 91, title: "翻车会话", status: "running", provider: "claude", cwd: "C:/repo",
      supported: true, offset: 0, reset: false, pendingReview: null, items: [], errored: true,
    });
    render(<ChatWindow />);
    await waitFor(() => expect(screen.getByText("翻车会话")).toBeTruthy());
    expect(document.querySelector(".chat-live")).toBeNull();
    expect(screen.queryByText("出错了")).toBeNull();
    expect(screen.queryByText("运行中")).toBeNull();
    // 没有结束入口的会话(endable 缺省/false:非托管且进程不活)不显示「结束会话」。
    expect(screen.queryByText("结束会话")).toBeNull();
  });

  /**
   * 标题栏任务进度入口:常驻清单图标,点开浮出「进度」面板——done/total 计数与
   * 完整清单都在面板里,不必翻到滚动区最底部找 TodoPanel。
   */
  it("标题栏任务图标点开显示进度面板", async () => {
    window.history.replaceState({}, "", "/?sessionId=95");
    respondWithHistory({
      sessionId: 95, title: "带任务", status: "running", provider: "claude", cwd: "C:/repo",
      supported: true, offset: 0, reset: false, pendingReview: null, items: [], connected: true,
      todos: [
        { content: "已完成的一步", status: "completed" },
        { content: "正在做的一步", status: "in_progress" },
        { content: "还没做的一步", status: "pending" },
      ],
    });
    render(<ChatWindow />);
    await waitFor(() => expect(screen.getByText("带任务")).toBeTruthy());
    const btn = document.querySelector(".chat-todo-btn");
    expect(btn).toBeTruthy();
    fireEvent.click(btn!);
    const panel = document.querySelector(".chat-todo-panel");
    expect(panel).toBeTruthy();
    expect(panel!.textContent).toContain("进度");
    expect(panel!.textContent).toContain("1/3");
    expect(panel!.textContent).toContain("正在做的一步");
    expect(panel!.textContent).toContain("还没做的一步");
  });

  /**
   * 面板收着时入口必须能看出里面有活儿(实拍反馈「入口上看不出在执行」):
   * 有在跑的子任务 → 图标挂脉冲小点;面板里子任务行尾显示执行时长(结束的定格总用时)。
   */
  it("进度入口带活动小点,面板子任务行尾显示执行时长", async () => {
    window.history.replaceState({}, "", "/?sessionId=99");
    respondWithHistory({
      sessionId: 99, title: "带时长", status: "running", provider: "claude", cwd: "C:/repo",
      supported: true, offset: 0, reset: false, pendingReview: null, connected: true,
      items: [
        { type: "tool_use", id: "sa_run", timestamp: "2026-08-18T04:00:00.000Z", name: "Agent", summary: "在跑的委派", subagent: { description: "在跑的委派", agent_type: null, count: 1 } },
        { type: "tool_use", id: "sa_done", timestamp: "2026-08-18T04:00:00.000Z", name: "Agent", summary: "完成的委派", subagent: { description: "完成的委派", agent_type: null, count: 1 } },
        {
          type: "tool_result", id: "sr_done", timestamp: "2026-08-18T04:02:05.000Z", tool_use_id: "sa_done",
          text: "done", is_error: false, subagent: { running: 0, completed: 1, failed: 0 },
        },
      ],
    });
    render(<ChatWindow />);
    await waitFor(() => expect(screen.getByText("带时长")).toBeTruthy());
    // 有委派没回执 = 在跑 → 入口图标右上角有脉冲小点。
    expect(document.querySelector(".chat-todo-btn .chat-todo-live")).toBeTruthy();
    fireEvent.click(document.querySelector(".chat-todo-btn")!);
    const panel = document.querySelector(".chat-todo-panel")!;
    // 完成的委派定格总用时(04:00:00 → 04:02:05);在跑的行尾也有滴答的已耗时。
    expect(panel.textContent).toContain("2 分 5 秒");
    const times = panel.querySelectorAll(".chat-todo-panel-time");
    expect(times.length).toBe(2);
  });

  /**
   * 多发委派(kimi AgentSwarm)在面板里逐分支一行,与 kimi TUI 的 001/002… 对齐,
   * 不再合并成「×N」。分支级状态由侧车探测下发:probe.branches[i] 按序号对应行,
   * 有分支标签时替换掉初始的 `#N` 后缀;探测仍按 tool_use id 一次(剥 `#` 后缀去重)。
   */
  it("多发委派逐分支一行,probe 分支结果逐行更新状态与标签", async () => {
    window.history.replaceState({}, "", "/?sessionId=100");
    respondWithHistory({
      sessionId: 100, title: "并发委派", status: "running", provider: "kimi", cwd: "C:/repo",
      supported: true, offset: 0, reset: false, pendingReview: null, connected: true,
      items: [
        { type: "tool_use", id: "sw1", timestamp: "2026-08-28T08:00:00.000Z", name: "AgentSwarm", summary: "审阅改动", subagent: { description: "审阅改动", agent_type: null, count: 2 } },
      ],
    });
    const base = invoke.getMockImplementation()!;
    invoke.mockImplementation((command: string, args?: Record<string, unknown>) => {
      if (command === "probe_subagent_states") {
        return Promise.resolve({
          sw1: {
            status: "running",
            branches: [
              { label: "agent-1", status: "completed", finished_at: "2026-08-28T08:03:05.000Z" },
              { label: "agent-2", status: "running" },
            ],
          },
        });
      }
      return base(command, args);
    });
    render(<ChatWindow />);
    await waitFor(() => expect(screen.getByText("并发委派")).toBeTruthy());
    // 面板收着时不探测:probe 是按需 I/O,不并进历史轮询。
    expect(invoke).not.toHaveBeenCalledWith("probe_subagent_states", expect.anything());
    fireEvent.click(document.querySelector(".chat-todo-btn")!);
    const panel = document.querySelector(".chat-todo-panel")!;
    // 一行一分支,初始标题带 #N 序号;不再出现合并的 ×N。
    expect(panel.textContent).toContain("审阅改动 #1");
    expect(panel.textContent).toContain("审阅改动 #2");
    expect(panel.textContent).not.toContain("×2");
    // 探测按 tool_use id 一次下发,剥掉了分支行的 `#` 后缀。
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("probe_subagent_states", { sessionId: 100, toolUseIds: ["sw1"] }),
    );
    // 分支实测到达:已完成的分支换上侧车标签并定格,还在跑的保持进行中。
    await waitFor(() => {
      expect(panel.textContent).toContain("审阅改动 agent-1");
      expect(panel.textContent).toContain("审阅改动 agent-2");
    });
    const lis = [...panel.querySelectorAll(".chat-todo-panel-list li")];
    const byText = (text: string) => lis.find((li) => li.textContent?.includes(text))!;
    expect(byText("agent-1").className).toContain("is-completed");
    expect(byText("agent-2").className).toContain("is-in_progress");
    // 完成的分支定格真实执行时长(08:00:00 → 08:03:05),不再按「委派→现在」滴答。
    expect(byText("agent-1").textContent).toContain("3 分 5 秒");
  });

  /**
   * 后台 Bash(`run_in_background`)也是「主回合停了还在跑」的活儿,面板必须看得见
   * (2026-08-27 实拍:`gh run watch` 在后台跑,会话报「等你输入」、面板一片空白)。
   * 委派证据只有启动回执 `Command running in background with ID: …`——后端据此给出
   * running 结局统计,前端靠它反认(isBackgroundShell)。
   */
  it("面板收下在跑的后台 Bash,对话流里它仍是普通工具块", async () => {
    window.history.replaceState({}, "", "/?sessionId=96");
    respondWithHistory({
      sessionId: 96, title: "后台命令", status: "waiting", provider: "claude", cwd: "C:/repo",
      supported: true, offset: 0, reset: false, pendingReview: null, connected: true,
      items: [
        { type: "tool_use", id: "bg1", timestamp: "2026-08-27T02:00:00.000Z", name: "Bash", summary: "gh run watch 33033677918" },
        {
          type: "tool_result", id: "br1", timestamp: "2026-08-27T02:00:01.000Z", tool_use_id: "bg1",
          text: "Command running in background with ID: b78nfkj1v.", is_error: false,
          subagent: { running: 1, completed: 0, failed: 0, task_id: "b78nfkj1v" },
        },
      ],
    });
    render(<ChatWindow />);
    await waitFor(() => expect(screen.getByText("后台命令")).toBeTruthy());
    // 有活儿在跑 → 入口挂脉冲小点。
    expect(document.querySelector(".chat-todo-btn .chat-todo-live")).toBeTruthy();
    fireEvent.click(document.querySelector(".chat-todo-btn")!);
    const panel = document.querySelector(".chat-todo-panel")!;
    expect(panel.textContent).toContain("gh run watch 33033677918");
    expect(panel.querySelector(".chat-todo-panel-list li")!.className).toContain("is-in_progress");
    // 但对话流不给它子任务块:没侧车流可展开,命令与输出本来就在工具块里。
    expect(document.querySelector(".chat-subagent")).toBeNull();
  });

  /**
   * claude 新版任务列表(TaskCreate/TaskUpdate)不触发 hook,DB 恒空——面板必须能从
   * 时间线的工具调用/回执里累积重建:编号从回执文本抠,状态由 TaskUpdate 摘要 JSON 驱动。
   */
  it("DB 无任务时面板从时间线的 TaskCreate/TaskUpdate 重建任务列表", async () => {
    window.history.replaceState({}, "", "/?sessionId=97");
    respondWithHistory({
      sessionId: 97, title: "重建任务", status: "running", provider: "claude", cwd: "C:/repo",
      supported: true, offset: 4, reset: false, pendingReview: null, connected: true,
      items: [
        { type: "tool_use", id: "tc1", timestamp: null, name: "TaskCreate", summary: "第一步:探索结构" },
        { type: "tool_result", id: "r1", timestamp: null, tool_use_id: "tc1", text: "Task #1 created successfully: 第一步:探索结构", is_error: false },
        { type: "tool_use", id: "tc2", timestamp: null, name: "TaskCreate", summary: "第二步:实现功能" },
        { type: "tool_result", id: "r2", timestamp: null, tool_use_id: "tc2", text: "Task #2 created successfully: 第二步:实现功能", is_error: false },
        { type: "tool_use", id: "tu1", timestamp: null, name: "TaskUpdate", summary: '{"taskId":"1","status":"in_progress","subject":null}' },
      ],
    });
    render(<ChatWindow />);
    await waitFor(() => expect(screen.getByText("重建任务")).toBeTruthy());
    fireEvent.click(document.querySelector(".chat-todo-btn")!);
    const panel = document.querySelector(".chat-todo-panel");
    expect(panel).toBeTruthy();
    expect(panel!.textContent).toContain("0/2");
    expect(panel!.textContent).toContain("第一步:探索结构");
    expect(panel!.textContent).toContain("第二步:实现功能");
    // 进行中的排最前(排序规则),且行状态类正确。
    expect(panel!.querySelector(".chat-todo-panel-list li")!.className).toContain("is-in_progress");
  });

  /**
   * 不堆积:任务/子任务列表跨回合只增不减,长会话会把历史旧账全堆进面板。
   * 已完成项只显示**最后一条用户消息之后**完成的;未完成/在跑的恒显示。
   */
  it("旧回合完成的任务与子任务被收走,未完成的恒显示", async () => {
    window.history.replaceState({}, "", "/?sessionId=98");
    respondWithHistory({
      sessionId: 98, title: "不堆积", status: "running", provider: "claude", cwd: "C:/repo",
      supported: true, offset: 9, reset: false, pendingReview: null, connected: true,
      items: [
        // 第一轮:任务 1 建立并完成、一个子任务派出并回执 → 都属于旧账。
        { type: "user_text", id: "u1", timestamp: null, text: "做第一件事" },
        { type: "tool_use", id: "tc1", timestamp: null, name: "TaskCreate", summary: "旧任务" },
        { type: "tool_result", id: "r1", timestamp: null, tool_use_id: "tc1", text: "Task #1 created successfully: 旧任务", is_error: false },
        { type: "tool_use", id: "tu1", timestamp: null, name: "TaskUpdate", summary: '{"taskId":"1","status":"completed","subject":null}' },
        { type: "tool_use", id: "sa1", timestamp: null, name: "Agent", summary: "旧委派", subagent: { description: "旧委派", agent_type: null, count: 1 } },
        { type: "tool_result", id: "sr1", timestamp: null, tool_use_id: "sa1", text: "done", is_error: false },
        // 第二轮:新指令后建了任务 2(未完成)+ 任务 3(本轮完成)。
        { type: "user_text", id: "u2", timestamp: null, text: "继续第二件事" },
        { type: "tool_use", id: "tc2", timestamp: null, name: "TaskCreate", summary: "新任务" },
        { type: "tool_result", id: "r2", timestamp: null, tool_use_id: "tc2", text: "Task #2 created successfully: 新任务", is_error: false },
        { type: "tool_use", id: "tc3", timestamp: null, name: "TaskCreate", summary: "本轮已完成任务" },
        { type: "tool_result", id: "r3", timestamp: null, tool_use_id: "tc3", text: "Task #3 created successfully: 本轮已完成任务", is_error: false },
        { type: "tool_use", id: "tu3", timestamp: null, name: "TaskUpdate", summary: '{"taskId":"3","status":"completed","subject":null}' },
      ],
    });
    render(<ChatWindow />);
    await waitFor(() => expect(screen.getByText("不堆积")).toBeTruthy());
    fireEvent.click(document.querySelector(".chat-todo-btn")!);
    const panel = document.querySelector(".chat-todo-panel")!;
    // 旧回合完成的任务与已回执的旧委派收走;本轮完成的、未完成的都在。
    expect(panel.textContent).toContain("新任务");
    expect(panel.textContent).toContain("本轮已完成任务");
    expect(panel.textContent).not.toContain("旧任务");
    expect(panel.textContent).not.toContain("旧委派");
    expect(panel.textContent).toContain("1/2");
  });

  it("无任务时图标仍在,面板显示骨架空态", async () => {
    window.history.replaceState({}, "", "/?sessionId=96");
    respondWithHistory({
      sessionId: 96, title: "无任务", status: "running", provider: "claude", cwd: "C:/repo",
      supported: true, offset: 0, reset: false, pendingReview: null, items: [], connected: true,
    });
    render(<ChatWindow />);
    await waitFor(() => expect(screen.getByText("无任务")).toBeTruthy());
    const btn = document.querySelector(".chat-todo-btn");
    expect(btn).toBeTruthy();
    // 没有任何在跑的任务/子任务 → 不挂活动小点。
    expect(document.querySelector(".chat-todo-live")).toBeNull();
    fireEvent.click(btn!);
    expect(document.querySelector(".chat-todo-skeleton")).toBeTruthy();
    expect(screen.getByText("任务进度将显示在这里")).toBeTruthy();
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
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 93, active: true, managed: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const input = await screen.findByRole("combobox", { name: "发送消息给 Agent" });
    fireEvent.change(input, { target: { value: "插一句" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 93, data: "插一句" }), { timeout: 3_000 });
    expect(await screen.findByText("1 条插话已排队，本回合结束后处理")).toBeTruthy();

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
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 95, active: true, managed: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const input = await screen.findByRole("combobox", { name: "发送消息给 Agent" });
    fireEvent.change(input, { target: { value: "插一句" } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(await screen.findByText("1 条插话已排队，本回合结束后处理")).toBeTruthy();

    current = {
      ...current,
      offset: 10,
      items: [{ type: "user_text", id: "u9", timestamp: null, text: "[Image #1] 插一句\n请读取并结合以下本地附件完成任务（图片请使用图像读取能力）：\n- C:\\tmp\\x.png" }],
    };
    await waitFor(() => expect(screen.queryByText(/插话已排队/)).toBeNull(), { timeout: 3_000 });
    // 会话仍在运行(新回合),消解不靠回合结束——运行条(徽标已移除)仍在即为证。
    expect(document.querySelector(".chat-running")).toBeTruthy();
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
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 97, active: true, managed: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const input = await screen.findByRole("combobox", { name: "发送消息给 Agent" });
    // 等旧消息上屏,确保入队时快照里已含旧证据。
    await screen.findByText("ok, run the tests");
    fireEvent.change(input, { target: { value: "ok" } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(await screen.findByText("1 条插话已排队，本回合结束后处理")).toBeTruthy();
    // 多轮轮询过去,旧证据不变:回执必须还在。
    await new Promise((resolve) => setTimeout(resolve, 1_500));
    expect(screen.queryByText(/插话已排队/)).toBeTruthy();
    // 新证据落盘(lastUserText 变成插话本身):这才消解。
    current = { ...current, lastUserText: "ok" };
    await waitFor(() => expect(screen.queryByText(/插话已排队/)).toBeNull(), { timeout: 3_000 });
  });

  /**
   * 纯附件插话的回执:正文为空时回执文本是占位「（附件）」,它永不落盘——文本包含
   * 匹配对它恒失败,旧逻辑下回执永不消解(claude/kimi 同样命中)。消解改认「水位线
   * 之后出现任意新 user_text」为证据。
   */
  it("纯附件插话:占位回执在新 user_text 落盘时消解", async () => {
    window.history.replaceState({}, "", "/?sessionId=98");
    let current: Record<string, unknown> = {
      sessionId: 98, title: "纯附件回执", status: "running", provider: "claude", cwd: "C:/repo",
      supported: true, offset: 0, reset: false, pendingReview: null, items: [], connected: true,
    };
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve(current);
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      // 剪贴板写不进去 → 退回指令文本通道;记账不受影响,占位回执照样进排队清单。
      if (command === "clipboard_set_image") return Promise.reject(new Error("clipboard occupied"));
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 98, active: true, managed: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    await waitFor(() => expect(screen.getByText("纯附件回执")).toBeTruthy());
    const box = () => screen.getByRole("combobox", { name: "发送消息给 Agent" });
    // 只挂附件、不写正文:回执文本只能是占位「（附件）」。
    openDialog.mockResolvedValueOnce(["C:\\repo\\shot.png"]);
    fireEvent.click(screen.getByRole("button", { name: "添加图片或文件" }));
    await waitFor(() => expect(screen.getByAltText("shot.png")).toBeTruthy());
    fireEvent.keyDown(box(), { key: "Enter" });
    expect(await screen.findByText("1 条插话已排队，本回合结束后处理")).toBeTruthy();
    // transcript 落盘的是图片引用行,不含占位文本——旧的包含匹配在此永不消解。
    current = {
      ...current,
      offset: 10,
      items: [{ type: "user_text", id: "u9", timestamp: null, text: "[Image: source: C:\\cache\\shot.png]" }],
    };
    await waitFor(() => expect(screen.queryByText(/插话已排队/)).toBeNull(), { timeout: 3_000 });
    // 会话仍在运行(打断后无缝进入新回合):消解不靠回合结束。
    expect(document.querySelector(".chat-running")).toBeTruthy();
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
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 94, active: true, managed: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const input = await screen.findByRole("combobox", { name: "发送消息给 Agent" });
    fireEvent.change(input, { target: { value: "改用另一种方案" } });
    // composer 上不该再有第二个「发送」入口:一颗圆钮 + 一个快捷键,没有常驻文字按钮。
    expect(screen.queryByRole("button", { name: "打断并发送" })).toBeNull();
    // Ctrl+Enter=打断并发送的说明挂在发送圆钮的 data-tip 上(回合在跑且有草稿时)。
    const sendBtn = screen.getByRole("button", { name: "发送" });
    expect(sendBtn.getAttribute("data-tip")).toBe(zh.chat.interruptAndSendTip);
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
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 96, active: true, managed: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
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
    // 只按会话 96 算账：前面用例留下的在途异步链（模型菜单静默探测的幽灵 Esc 之类，
    // 见 ChatWindow 2527 行的实拍注释）晚到时会混进全量调用，把 1 写成 2（CI 实拍）。
    const writes = invoke.mock.calls.filter(([command, args]) => command === "write_managed_terminal"
      && (args as { sessionId: number }).sessionId === 96);
    expect(writes).toHaveLength(1);

    const input = await screen.findByRole("combobox", { name: "发送消息给 Agent" });
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
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 97, active: true, managed: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const input = await screen.findByRole("combobox", { name: "发送消息给 Agent" });
    fireEvent.change(input, { target: { value: "第一句" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 97, data: "第一句" }), { timeout: 3_000 });
    // 发送成功后组件才清空输入框——不等它,第二句会被那次 setPrompt("") 抹掉。
    await waitFor(() => expect((input as HTMLTextAreaElement).value).toBe(""));
    fireEvent.change(input, { target: { value: "第二句" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 97, data: "第二句" }), { timeout: 3_000 });
    expect(await screen.findByText("2 条插话已排队，本回合结束后处理")).toBeTruthy();

    const writesBefore = invoke.mock.calls.filter(([command]) => command === "write_managed_terminal").length;
    fireEvent.click(screen.getAllByRole("button", { name: "移除这条回执" })[0]);
    expect(await screen.findByText("1 条插话已排队，本回合结束后处理")).toBeTruthy();
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
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "agent_chat_ui") return Promise.resolve({ ...chatUi("kimi")!, interrupt_input: null });
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 95, active: true, managed: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const input = await screen.findByRole("combobox", { name: "发送消息给 Agent" });
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
   * 占位符只随 needsTakeover 翻（C-15）：普通发送失败（终端写入被拒等）有 sendError 条
   * 说真实原因，输入框占位必须保持原样——此前任何错误都把占位改成「尚未接管」，
   * 与真实原因无关，误导用户去找一个不存在的接管入口。
   */
  it("普通发送错误不改变输入框占位符", async () => {
    window.history.replaceState({}, "", "/?sessionId=99");
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 99, title: "发送失败", status: "running", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [], connected: true,
      });
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 99, active: true, managed: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      if (command === "write_managed_terminal") return Promise.reject(new Error("pty write broke"));
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const input = await screen.findByRole("combobox", { name: "发送消息给 Agent" });
    expect((input as HTMLTextAreaElement).placeholder).toBe(zh.chat.inputPlaceholder);
    fireEvent.change(input, { target: { value: "发不出去" } });
    fireEvent.keyDown(input, { key: "Enter" });
    // 错误条出现（真实原因），占位符纹丝不动，也不冒出接管入口。
    await screen.findByRole("alert");
    expect((input as HTMLTextAreaElement).placeholder).toBe(zh.chat.inputPlaceholder);
    expect(screen.queryByRole("button", { name: zh.chat.terminalTakeover })).toBeNull();
  });

  /**
   * GUI 用户的「结束会话」:此前结束入口只藏在终端页操作条里,不开终端页的人无从结束,
   * 会话在后台一直占着进程。标题栏入口按 endable 口径可见(托管 PTY 或进程仍活的孤儿
   * 会话),confirm 通过才调 stop_managed_terminal,取消则不动。
   */
  it("endable 会话可从标题栏结束,confirm 取消则不调用", async () => {
    window.history.replaceState({}, "", "/?sessionId=92");
    // 确认走应用内原生小窗(invoke confirm_dialog):按队列给答案。
    const confirmAnswers: boolean[] = [false, true];
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 92, title: "托管会话", status: "running", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [],
        connected: true, ptyManaged: true, endable: true,
      });
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("claude"));
      if (command === "confirm_dialog") return Promise.resolve(confirmAnswers.shift() ?? false);
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 92, active: true, managed: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
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
   * 孤儿会话(真机事故复盘):ConPTY kill 静默无效,broker 已收掉 PTY 记录(ptyManaged=false)
   * 但进程还活着(endable=true)——结束入口必须还在,否则用户只能手动 taskkill。
   */
  it("ptyManaged=false 但进程仍活(endable)的孤儿会话仍有结束入口", async () => {
    window.history.replaceState({}, "", "/?sessionId=93");
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 93, title: "孤儿会话", status: "running", provider: "kimi", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [],
        connected: true, ptyManaged: false, endable: true,
      });
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi("kimi"));
      if (command === "confirm_dialog") return Promise.resolve(true);
      return Promise.resolve();
    });
    render(<ChatWindow />);
    const button = await screen.findByRole("button", { name: "结束会话" });
    fireEvent.click(button);
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("stop_managed_terminal", { sessionId: 93 }));
  });

  /**
   * 存活的 running 会话:常驻运行条在滚动流**之外**(原实现在 .chat-scroll 里,
   * 上翻历史即滚出视口),窗口标题带 ▶ 前缀让任务栏可感知。标题栏状态徽标已移除,
   * 断言它不复活。
   */
  it("存活 running 会话:滚动流外的常驻运行条+窗口标题记号", async () => {
    window.history.replaceState({}, "", "/?sessionId=91");
    respondWithHistory({
      sessionId: 91, title: "跑着的会话", status: "running", provider: "claude", cwd: "C:/repo",
      supported: true, offset: 1, reset: false, pendingReview: null, connected: true,
      currentActivity: "› cargo test",
      items: [{ type: "user_text", id: "u1", timestamp: null, text: "开始" }],
    });
    render(<ChatWindow />);
    await waitFor(() => expect(screen.getByText("跑着的会话")).toBeTruthy());
    expect(document.querySelector(".chat-live")).toBeNull();
    const strip = document.querySelector(".chat-running");
    expect(strip).toBeTruthy();
    expect(strip!.closest(".chat-scroll")).toBeNull();
    expect(strip!.textContent).toContain("› cargo test");
    await waitFor(() => expect(setTitleMock).toHaveBeenCalledWith("▶ 跑着的会话 · Meowo"));
  });

  /** 压缩（/compact）进行期间后端把 current_activity 写成哨兵 __meowo_compacting__:
   *  运行条显示本地化「正在压缩上下文…」,哨兵原值不上屏、不进 tooltip(data-tip)。 */
  it("压缩哨兵:运行条映射为本地化文案,原值不进 tooltip", async () => {
    window.history.replaceState({}, "", "/?sessionId=96");
    respondWithHistory({
      sessionId: 96, title: "压缩中", status: "running", provider: "claude", cwd: "C:/repo",
      supported: true, offset: 1, reset: false, pendingReview: null, connected: true,
      currentActivity: "__meowo_compacting__",
      items: [{ type: "user_text", id: "u1", timestamp: null, text: "开始" }],
    });
    render(<ChatWindow />);
    const strip = await waitFor(() => {
      const el = document.querySelector(".chat-running");
      expect(el).toBeTruthy();
      return el!;
    });
    expect(strip.textContent).toContain("正在压缩上下文…");
    expect(strip.textContent).not.toContain("__meowo_compacting__");
    expect(strip.querySelector("[data-tip]")).toBeNull();
  });

  /** 运行指示是盲文贪吃蛇（用户指定形态）：帧字符随时间轮转，不再是原地呼吸的圆点。 */
  it("运行指示:盲文蛇帧随时间轮转", async () => {
    window.history.replaceState({}, "", "/?sessionId=95");
    respondWithHistory({
      sessionId: 95, title: "蛇", status: "running", provider: "claude", cwd: "C:/repo",
      supported: true, offset: 1, reset: false, pendingReview: null, connected: true,
      items: [{ type: "user_text", id: "u1", timestamp: null, text: "开始" }],
    });
    render(<ChatWindow />);
    await waitFor(() => expect(document.querySelector(".chat-running-snake")).toBeTruthy());
    const snake = document.querySelector(".chat-running-snake")!;
    const first = snake.textContent;
    await waitFor(() => expect(snake.textContent).not.toBe(first), { timeout: 1_000 });
  });

  /**
   * 跨 provider 切换（切换引擎）:模型下拉出现「切换引擎」分组,点目标 agent 展开二级,
   * 点档位先 appConfirm(破坏性:要杀当前进程)——取消不发命令;确认后调
   * switch_session_provider,窗口切到返回的临时负 id(保持当前视图:对话页给启动占位)。
   */
  it("切换引擎:下拉分组→确认→调 switch 命令并切到临时会话", async () => {
    window.history.replaceState({}, "", "/?sessionId=61");
    const confirmAnswers: boolean[] = [false, true];
    invoke.mockImplementation((command: string, args?: Record<string, unknown>) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 61, title: "换引擎", status: "running", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [], model: "Opus",
        connected: true, predecessorId: null, supersededBy: null,
      });
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi((args as { provider: string }).provider));
      if (command === "list_agents") return Promise.resolve(descriptors(["claude", "codex"]));
      if (command === "confirm_dialog") return Promise.resolve(confirmAnswers.shift() ?? false);
      if (command === "switch_session_provider") {
        return Promise.resolve({ tempId: -5, handoffPath: "C:/tmp/meowo-handoff/1-61/handoff.md" });
      }
      if (command === "managed_terminal_binding") return Promise.resolve(null);
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 61, active: true, managed: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      return Promise.resolve();
    });
    render(<ChatWindow />);

    // claude 有静态预设,模型按钮直接开 GUI 下拉;分组与已装的其他 agent 在列。
    fireEvent.click(await screen.findByRole("button", { name: "切换模型" }));
    expect(await screen.findByText("切换引擎")).toBeTruthy();
    // 未安装的 agent 不列(descriptors 里 kimi/gemini/opencode 均未装)。
    expect(screen.queryByRole("menuitem", { name: /Kimi/ })).toBeNull();

    // 展开 Codex 二级:无预设 → 只有「默认模型」。
    fireEvent.click(screen.getByRole("menuitem", { name: /Codex/ }));
    const defaultModel = await screen.findByRole("menuitem", { name: "默认模型" });

    // 第一次点:确认框弹出但用户取消 → 不发 switch 命令。
    fireEvent.click(defaultModel);
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("confirm_dialog", expect.anything()));
    expect(invoke.mock.calls.some(([command]) => command === "switch_session_provider")).toBe(false);

    // 再点一次并确认 → 调命令,窗口切到临时负 id;用户此刻在对话页,视图不动,
    // 对话页渲染启动占位。
    fireEvent.click(await screen.findByRole("button", { name: "切换模型" }));
    fireEvent.click(screen.getByRole("menuitem", { name: /Codex/ }));
    fireEvent.click(await screen.findByRole("menuitem", { name: "默认模型" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("switch_session_provider", {
      sessionId: 61, targetProvider: "codex", options: undefined,
    }));
    expect(await screen.findByText("会话正在启动…")).toBeTruthy();
  });

  it("切换引擎:来源不支持导出(supports_chat_export=false)时不显示分组", async () => {
    window.history.replaceState({}, "", "/?sessionId=62");
    invoke.mockImplementation((command: string, args?: Record<string, unknown>) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 62, title: "不可导出", status: "running", provider: "claude", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [], model: "Opus",
        connected: true, predecessorId: null, supersededBy: null,
      });
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi((args as { provider: string }).provider));
      // 能力位由后端下发;这里模拟一个历史不可导出的来源(其余字段与真实矩阵一致)。
      if (command === "list_agents") {
        return Promise.resolve(descriptors(["claude", "codex"]).map((d) =>
          d.id === "claude" ? { ...d, supports_chat_export: false } : d));
      }
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 62, active: true, managed: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    fireEvent.click(await screen.findByRole("button", { name: "切换模型" }));
    // 模型档照常;「切换引擎」分组整个不出现——没有可交接的历史,给入口就是给假承诺。
    expect((await screen.findAllByRole("menuitem", { name: /Sonnet/ })).length).toBeGreaterThan(0);
    expect(screen.queryByText("切换引擎")).toBeNull();
  });

  /**
   * 交接注入:切换产生的新会话(provider=目标、predecessorId 非空)在终端就绪后,
   * 自动把「请读交接文件」写进 PTY 并回车;注入语引用 switch 返回的文件路径。
   */
  it("切换引擎:新会话就绪后自动注入交接提示", async () => {
    window.history.replaceState({}, "", "/?sessionId=63");
    const handoffPath = "C:/tmp/meowo-handoff/1-63/handoff.md";
    const injectPrompt = zh.chat.handoffPrompt(handoffPath);
    let switched = false;
    invoke.mockImplementation((command: string, args?: Record<string, unknown>) => {
      if (command === "get_chat_history") {
        const id = (args as { sessionId: number }).sessionId;
        if (id === 63) return Promise.resolve({
          sessionId: 63, title: "旧会话", status: "running", provider: "claude", cwd: "C:/repo",
          supported: true, offset: 0, reset: false, pendingReview: null, items: [], model: "Opus",
          connected: true, predecessorId: null, supersededBy: null,
        });
        // 切换后的新会话:目标 provider + 接续链已落库。
        return Promise.resolve({
          sessionId: 64, title: "新会话", status: "running", provider: "codex", cwd: "C:/repo",
          supported: true, offset: 0, reset: false, pendingReview: null, items: [], model: null,
          connected: true, predecessorId: 63, supersededBy: null,
        });
      }
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi((args as { provider: string }).provider));
      if (command === "list_agents") return Promise.resolve(descriptors(["claude", "codex"]));
      if (command === "confirm_dialog") return Promise.resolve(true);
      if (command === "switch_session_provider") {
        switched = true;
        return Promise.resolve({ tempId: -7, handoffPath });
      }
      // 临时 id 认领成真 id 后,binding 轮询把窗口带到新会话。
      if (command === "managed_terminal_binding") return Promise.resolve(switched ? 64 : null);
      if (command === "managed_terminal_snapshot") {
        // 新终端有可见输出且随即安静(waitForTerminalReady 的就绪判据);注入后的回显
        // 验证从同一份画面里读到注入语——mock 无条件带上它即可。
        return Promise.resolve({
          sessionId: 64, active: true, managed: true,
          data: b64utf8(`codex ready\n${injectPrompt}`),
          startOffset: 0, endOffset: 64, exited: false, exitCode: null,
        });
      }
      return Promise.resolve();
    });
    render(<ChatWindow />);
    fireEvent.click(await screen.findByRole("button", { name: "切换模型" }));
    fireEvent.click(await screen.findByRole("menuitem", { name: /Codex/ }));
    fireEvent.click(await screen.findByRole("menuitem", { name: "默认模型" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("switch_session_provider", expect.anything()));

    // 注入语作为正文写进 PTY(与回车分两次写,先到正文即可断言)。
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", {
      sessionId: 64, data: injectPrompt,
    }), { timeout: 8_000 });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", {
      sessionId: 64, data: "\r",
    }), { timeout: 3_000 });
  }, 15_000);

  /// 已被接替的旧段只读:composer 禁用、上方挂「已切换引擎」横幅(向它续话会让接续链
  /// 分叉),唯一动作是「前往新会话」——直接切到链尾。
  it("已被接替的会话禁发,横幅引导前往新会话", async () => {
    window.history.replaceState({}, "", "/?sessionId=65");
    invoke.mockImplementation((command: string, args?: Record<string, unknown>) => {
      if (command === "get_chat_history") {
        const id = (args as { sessionId: number }).sessionId;
        if (id === 65) return Promise.resolve({
          sessionId: 65, title: "被接替的", status: "ended", provider: "claude", cwd: "C:/repo",
          supported: true, offset: 1, reset: false, pendingReview: null, model: null,
          connected: false, predecessorId: null, supersededBy: 66,
          items: [{ type: "user_text", id: "u1", timestamp: null, text: "旧内容" }],
        });
        return Promise.resolve({
          sessionId: 66, title: "接替者", status: "running", provider: "codex", cwd: "C:/repo",
          supported: true, offset: 0, reset: false, pendingReview: null, items: [], model: null,
          connected: true, predecessorId: 65, supersededBy: null,
        });
      }
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi((args as { provider: string }).provider));
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 65, active: false, managed: false, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    // 旧内容仍可回看;composer 禁用而非卸载(输入框在、发不出去),「恢复会话」也不该
    // 出现——恢复它会分叉接续链。
    expect(await screen.findByText("旧内容")).toBeTruthy();
    expect(await screen.findByText("本会话已切换引擎，在新会话中继续。此处仅供回看")).toBeTruthy();
    const roBox = screen.getByRole("combobox", { name: "发送消息给 Agent" });
    expect((roBox as HTMLTextAreaElement).disabled).toBe(true);
    expect(screen.queryByRole("button", { name: "恢复会话" })).toBeNull();
    // 「前往新会话」切到链尾。
    fireEvent.click(screen.getByRole("button", { name: "前往新会话" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("get_chat_history", expect.objectContaining({ sessionId: 66 })));
  });

  /// /clear 换代:正开着的会话被同一 PTY 的新段接替(supersededBy 由空变有)时自动跳到
  /// 新段——终端进程还是同一个,留在旧段只剩定格画面。冷打开旧段回看(首帧就带
  /// supersededBy)不跳,由上一用例覆盖。
  it("眼前发生的换代自动跟随到新段", async () => {
    window.history.replaceState({}, "", "/?sessionId=71");
    let superseded: number | null = null;
    invoke.mockImplementation((command: string, args?: Record<string, unknown>) => {
      if (command === "get_chat_history") {
        const id = (args as { sessionId: number }).sessionId;
        if (id === 71) return Promise.resolve({
          sessionId: 71, title: "旧段", status: superseded == null ? "running" : "ended", provider: "claude", cwd: "C:/repo",
          supported: true, offset: 0, reset: false, pendingReview: null, items: [], model: null,
          connected: superseded == null, predecessorId: null, supersededBy: superseded,
        });
        return Promise.resolve({
          sessionId: 72, title: "新段", status: "running", provider: "claude", cwd: "C:/repo",
          supported: true, offset: 0, reset: false, pendingReview: null, items: [], model: null,
          connected: true, predecessorId: 71, supersededBy: null,
        });
      }
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi((args as { provider: string }).provider));
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 71, active: true, managed: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    // 首帧 supersededBy 为空:只记录基线,不跳。
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("get_chat_history", expect.objectContaining({ sessionId: 71 })));
    superseded = 72;
    // 下一轮轮询看到「由空变有」→ 自动切到新段。
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("get_chat_history", expect.objectContaining({ sessionId: 72 })), { timeout: 3_000 });
  });

  /**
   * 切换引擎入口不与模型清单就绪耦合:codex/kimi 这类无内联预设的 CLI,模型清单要先
   * 静默探测 /model 菜单学到标签才拼得出;若「切换引擎」分组躲在同一道门后,这些来源
   * 就永远切不回去(点按钮只会触发探测)。有切换目标时下拉必须直开。
   */
  it("切换引擎:模型清单未学到时下拉仍直开,分组可达且不触发探测", async () => {
    window.history.replaceState({}, "", "/?sessionId=67");
    invoke.mockImplementation((command: string, args?: Record<string, unknown>) => {
      if (command === "get_chat_history") return Promise.resolve({
        sessionId: 67, title: "codex 来源", status: "running", provider: "codex", cwd: "C:/repo",
        supported: true, offset: 0, reset: false, pendingReview: null, items: [], model: null,
        connected: true, predecessorId: null, supersededBy: null,
      });
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi((args as { provider: string }).provider));
      if (command === "list_agents") return Promise.resolve(descriptors(["claude", "codex"]));
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 67, active: true, managed: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    fireEvent.click(await screen.findByRole("button", { name: "切换模型" }));
    // 下拉直开:同 agent 的切换入口(探测通道,菜单项形态)与「切换引擎」分组都在。
    expect(await screen.findByText("切换引擎")).toBeTruthy();
    expect(screen.getByRole("menuitem", { name: /Claude Code/ })).toBeTruthy();
    // 没学到清单不该触发 /model 探测(那会往终端里写命令、弹 TUI 菜单)。
    expect(invoke.mock.calls.some(([command, args]) =>
      command === "write_managed_terminal" && (args as { data: string }).data === "/model")).toBe(false);
  });

  /**
   * 接续会话的时间线不从空白开始:前序段(已 ended、内容静态)的完整消息内联在上方,
   * 段间以「切换至 X」分隔条衔接;注入的那条「请读交接文件」机器消息不再以用户气泡
   * 重复出现(分隔条已表达同一事实)。
   */
  it("接续会话内联展示前序段历史,交接注入语不重复上屏", async () => {
    window.history.replaceState({}, "", "/?sessionId=70");
    const handoffPath = "C:/tmp/meowo-handoff/1-70/handoff.md";
    invoke.mockImplementation((command: string, args?: Record<string, unknown>) => {
      if (command === "get_chat_history") {
        const id = (args as { sessionId: number }).sessionId;
        if (id === 69) return Promise.resolve({
          sessionId: 69, title: "前序段", status: "ended", provider: "claude", cwd: "C:/repo",
          supported: true, offset: 2, reset: false, pendingReview: null, model: "Opus",
          connected: false, predecessorId: null, supersededBy: 70,
          items: [
            { type: "user_text", id: "u1", timestamp: null, text: "旧问题" },
            { type: "assistant_text", id: "a1", timestamp: null, text: "旧回答" },
          ],
        });
        return Promise.resolve({
          sessionId: 70, title: "接续段", status: "running", provider: "kimi", cwd: "C:/repo",
          supported: true, offset: 2, reset: false, pendingReview: null, model: null,
          connected: true, predecessorId: 69, supersededBy: null,
          items: [
            { type: "user_text", id: "u2", timestamp: null, text: zh.chat.handoffPrompt(handoffPath) },
            { type: "assistant_text", id: "a2", timestamp: null, text: "新回答" },
          ],
        });
      }
      if (command === "get_session_lineage") return Promise.resolve([
        { id: 69, provider: "claude", startedAt: 1, endedAt: 2, model: "Opus" },
        { id: 70, provider: "kimi", startedAt: 3, endedAt: null, model: null },
      ]);
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi((args as { provider: string }).provider));
      if (command === "list_agents") return Promise.resolve(descriptors(["claude", "kimi"]));
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 70, active: true, managed: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    // 前序段消息与段间分隔条上屏(引擎展示名来自 list_agents)。
    expect(await screen.findByText("旧问题")).toBeTruthy();
    expect(await screen.findByText("旧回答")).toBeTruthy();
    expect(await screen.findByText("切换至 Kimi Code 继续，完整上下文已交接")).toBeTruthy();
    expect(await screen.findByText("新回答")).toBeTruthy();
    // 注入语被显示层滤掉:不以用户气泡重复出现。
    expect(screen.queryByText(/请先完整阅读文件/)).toBeNull();
  });

  /**
   * 未读徽章口径:快照与差值都只数真实内容项。脱离底部期间到达的 handoff 分隔条
   * (meta 行)与前序段旧历史都不算未读(快照同步抬高,同 loadEarlier 前插);只有
   * 真正的新消息才 +1。
   */
  it("脱离底部后:接续分隔条与旧历史不计未读,新消息才 +1", async () => {
    window.history.replaceState({}, "", "/?sessionId=80");
    // lineage 拉到一半挂起:先把「脱离底部」的快照落下来,再放行前序段到达。
    let resolveLineage!: (value: unknown) => void;
    const lineagePending = new Promise((resolve) => { resolveLineage = resolve; });
    let newMessageArrived = false;
    const currentItems = () => [
      { type: "user_text", id: "u1", timestamp: null, text: "当前段的问题" },
      { type: "assistant_text", id: "a1", timestamp: null, text: "当前段的回答" },
      ...(newMessageArrived
        ? [{ type: "assistant_text", id: "a2", timestamp: null, text: "刚落地的新消息" }]
        : []),
    ];
    invoke.mockImplementation((command: string, args?: Record<string, unknown>) => {
      if (command === "get_chat_history") {
        const id = (args as { sessionId: number }).sessionId;
        if (id === 79) return Promise.resolve({
          sessionId: 79, title: "前序段", status: "ended", provider: "claude", cwd: "C:/repo",
          supported: true, offset: 2, reset: false, pendingReview: null, model: null,
          connected: false, predecessorId: null, supersededBy: 80,
          items: [
            { type: "user_text", id: "p1", timestamp: null, text: "前序段旧问题" },
            { type: "assistant_text", id: "p2", timestamp: null, text: "前序段旧回答" },
          ],
        });
        const items = currentItems();
        const cursor = ((args as { offset?: number }).offset) ?? 0;
        const base = {
          sessionId: 80, title: "接续段", status: "running", provider: "kimi", cwd: "C:/repo",
          supported: true, offset: items.length, reset: false, pendingReview: null, model: null,
          connected: true, predecessorId: 79, supersededBy: null, hasMore: false,
        };
        // 增量语义:轮询只回 offset 之后的新条目(回整批会被组件再追加一遍);
        // 元信息字段必须全量保留——增量响应缺字段会把 history 里的 provider/status 冲掉。
        if (cursor > 0) {
          return Promise.resolve({ ...base, items: cursor >= items.length ? [] : items.slice(cursor) });
        }
        return Promise.resolve({ ...base, items });
      }
      if (command === "get_session_lineage") return lineagePending;
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      if (command === "agent_chat_ui") return Promise.resolve(chatUi((args as { provider: string }).provider));
      if (command === "list_agents") return Promise.resolve(descriptors(["claude", "kimi"]));
      if (command === "managed_terminal_binding") return Promise.resolve(null);
      if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 80, active: true, managed: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
      return Promise.resolve();
    });
    render(<ChatWindow />);
    expect(await screen.findByText("当前段的回答")).toBeTruthy();
    // 滚离底部:jsdom 的滚动度量恒 0,直接定义出「视口 500、内容 2000、停在顶部」。
    const scroller = document.querySelector(".chat-scroll") as HTMLElement;
    Object.defineProperty(scroller, "scrollHeight", { value: 2000, configurable: true });
    Object.defineProperty(scroller, "clientHeight", { value: 500, configurable: true });
    scroller.scrollTop = 0;
    fireEvent.scroll(scroller);
    const jumpButton = await screen.findByRole("button", { name: /回到最新/ });
    // 脱离瞬间没有未读。
    expect(jumpButton.querySelector(".chat-jump-latest-count")).toBeNull();

    // 前序段到达:2 条旧消息 + 分隔条(meta 行)前插进时间线——都不算未读。
    resolveLineage([
      { id: 79, provider: "claude", startedAt: 1, endedAt: 2, model: null },
      { id: 80, provider: "kimi", startedAt: 3, endedAt: null, model: null },
    ]);
    expect(await screen.findByText("切换至 Kimi Code 继续，完整上下文已交接")).toBeTruthy();
    expect(await screen.findByText("前序段旧问题")).toBeTruthy();
    expect(screen.getByRole("button", { name: /回到最新/ }).querySelector(".chat-jump-latest-count")).toBeNull();

    // 当前段真来一条新消息:徽章 +1。
    newMessageArrived = true;
    await waitFor(() => {
      const badge = screen.getByRole("button", { name: /回到最新/ }).querySelector(".chat-jump-latest-count");
      expect(badge?.textContent).toBe("1");
    }, { timeout: 3000 });
  });

  /// 断线语言（C-18）：一次瞬时 IPC 抖动只配通用「读取失败，正在重试」；连续失败（≥3 次，
  /// ≈2s）单列为「同步中断」（role=alert）——它只说窗口与后端的通道断了，与状态徽标讲的
  /// 「agent 进程没了」（已断开）是两种事实，措辞与样式都分开。
  it("轮询连续失败后升级为「同步中断」横幅", async () => {
    window.history.replaceState({}, "", "/?sessionId=7");
    respondWithHistory({
      sessionId: 7, title: "会断线的会话", status: "running", provider: "claude", cwd: null,
      supported: true, offset: 1, reset: false, pendingReview: null,
      items: [{ type: "user_text", id: "u1", timestamp: null, text: "缓存的对话" }],
    });
    render(<ChatWindow />);
    expect(await screen.findByText("缓存的对话")).toBeTruthy();
    // 通道断掉：之后每轮 650ms 轮询都失败。
    invoke.mockImplementation((command: string) => {
      if (command === "get_chat_history") return Promise.reject(new Error("ipc down"));
      if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
      return Promise.resolve();
    });
    // 第一次失败：通用重试文案（role=status），缓存内容继续渲染。
    await waitFor(() => expect(screen.getByText(zh.chat.loadError)).toBeTruthy());
    expect(screen.getByText("缓存的对话")).toBeTruthy();
    // 连续失败升级为「同步中断」（role=alert），措辞不再与 loadError 混用。
    await waitFor(() => expect(screen.getByRole("alert").textContent).toContain(zh.chat.syncInterrupted), { timeout: 5000 });
    expect(screen.queryByText(zh.chat.loadError)).toBeNull();
  });
});

describe("submitGapMs", () => {
  it("短输入保持 250ms 基础间隔", () => {
    expect(submitGapMs("")).toBe(250);
    expect(submitGapMs("帮我跑一下测试")).toBe(250);
    expect(submitGapMs("x".repeat(1023))).toBe(250);
  });

  it("超长粘贴按长度追加消化时间(固定 250ms 时 TUI 还没消化完正文)", () => {
    expect(submitGapMs("x".repeat(1024))).toBe(300);
    expect(submitGapMs("x".repeat(10 * 1024))).toBe(750);
  });

  it("封顶 2s:再长的粘贴也不干等", () => {
    expect(submitGapMs("x".repeat(1024 * 1024))).toBe(2000);
  });
});

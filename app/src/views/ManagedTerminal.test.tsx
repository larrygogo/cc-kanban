import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.hoisted(() => vi.fn());
const write = vi.hoisted(() => vi.fn());
const eventHandlers = vi.hoisted(() => new Map<string, (event: { payload: unknown }) => void>());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn((event: string, handler: (event: { payload: unknown }) => void) => {
    eventHandlers.set(event, handler);
    return Promise.resolve(() => eventHandlers.delete(event));
  }),
}));

// xterm 需要真实 canvas/DOM 度量，jsdom 跑不了；这里只关心遮罩状态机，故把终端替换成哑实现。
const keyHandler = vi.hoisted(() => ({ current: null as ((event: KeyboardEvent) => boolean) | null }));
const linkOpen = vi.hoisted(() => ({ current: null as ((event: MouseEvent, uri: string) => void) | null }));
const termOptions = vi.hoisted(() => ({ current: null as Record<string, unknown> | null }));
// onData 处理器与可控的 write 完成回调:回放拦截测试要在「write 已入队、回调未触发」的
// 窗口里注入 xterm 自动应答,manual 模式把回调攒进队列由测试择机触发。
const dataHandler = vi.hoisted(() => ({ current: null as ((data: string) => void) | null }));
const writeCallbacks = vi.hoisted(() => ({ manual: false, queue: [] as (() => void)[] }));
vi.mock("@xterm/addon-web-links", () => ({
  WebLinksAddon: class {
    constructor(handler: (event: MouseEvent, uri: string) => void) { linkOpen.current = handler; }
  },
}));
vi.mock("@xterm/xterm", () => ({
  Terminal: class {
    constructor(options: Record<string, unknown>) { termOptions.current = options; }
    cols = 80;
    rows = 24;
    options = { fontSize: 12 };
    write = (data: Uint8Array | string, callback?: () => void) => {
      write(data);
      if (writeCallbacks.manual && callback) writeCallbacks.queue.push(callback);
      else callback?.();
    };
    open = vi.fn();
    reset = vi.fn();
    focus = vi.fn();
    dispose = vi.fn();
    loadAddon = vi.fn();
    onData = (handler: (data: string) => void) => { dataHandler.current = handler; return { dispose: vi.fn() }; };
    attachCustomKeyEventHandler = (handler: (event: KeyboardEvent) => boolean) => { keyHandler.current = handler; };
  },
}));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit = vi.fn(); } }));
// 接管确认走应用内原生小窗(invoke confirm_dialog),不再用 plugin-dialog / window.confirm。
// 用 confirmAnswer 控制那次 invoke 的返回;plugin-dialog 仍被 mock 以防其它路径引用。
vi.mock("@tauri-apps/plugin-dialog", () => ({ confirm: vi.fn(), open: vi.fn() }));
const confirmAnswer = vi.hoisted(() => ({ ok: true }));

import { findFakeCaret, ManagedTerminal, stripTerminalReplies } from "./ManagedTerminal";

const noPty = { sessionId: 163, active: false, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null };

describe("ManagedTerminal", () => {
  afterEach(cleanup);
  beforeEach(() => {
    invoke.mockReset();
    write.mockReset();
    confirmAnswer.ok = true;
    eventHandlers.clear();
    dataHandler.current = null;
    writeCallbacks.manual = false;
    writeCallbacks.queue = [];
    global.ResizeObserver = class {
      observe = vi.fn();
      disconnect = vi.fn();
    } as unknown as typeof ResizeObserver;
  });

  it("放行粘贴组合键：Ctrl/Cmd+V 与 Shift+Insert 交给浏览器原生 paste，其余按键仍由 xterm 处理", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") return Promise.resolve(noPty);
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" />);
    await waitFor(() => expect(keyHandler.current).toBeTruthy());
    const key = (init: Partial<KeyboardEvent> & { type: string; code: string }) => init as KeyboardEvent;
    // false = xterm 不处理也不 preventDefault → WebView 对隐藏 textarea 执行原生粘贴。
    expect(keyHandler.current!(key({ type: "keydown", code: "KeyV", ctrlKey: true }))).toBe(false);
    expect(keyHandler.current!(key({ type: "keydown", code: "KeyV", metaKey: true }))).toBe(false);
    expect(keyHandler.current!(key({ type: "keydown", code: "KeyV", ctrlKey: true, shiftKey: true }))).toBe(false);
    expect(keyHandler.current!(key({ type: "keydown", code: "Insert", shiftKey: true }))).toBe(false);
    // 组合键的 keyup 与普通按键照常交给 xterm（否则 ^V 之外的键序全部失灵）。
    expect(keyHandler.current!(key({ type: "keyup", code: "KeyV", ctrlKey: true }))).toBe(true);
    expect(keyHandler.current!(key({ type: "keydown", code: "KeyV" }))).toBe(true);
    expect(keyHandler.current!(key({ type: "keydown", code: "KeyC", ctrlKey: true }))).toBe(true);
    // Ctrl+Alt+V（AltGr 组合可能产字符）不劫持。
    expect(keyHandler.current!(key({ type: "keydown", code: "KeyV", ctrlKey: true, altKey: true }))).toBe(true);
  });

  it("链接走终端惯例：Ctrl/Cmd+点击经 open_link 打开，普通点击不动", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") return Promise.resolve(noPty);
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" />);
    await waitFor(() => expect(linkOpen.current).toBeTruthy());
    const click = (init: Partial<MouseEvent>) => init as MouseEvent;
    // 普通点击留给 TUI 的鼠标交互与选区。
    linkOpen.current!(click({}), "https://example.com/a");
    expect(invoke).not.toHaveBeenCalledWith("open_link", expect.anything());
    linkOpen.current!(click({ ctrlKey: true }), "https://example.com/a");
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("open_link", { url: "https://example.com/a" }));
    // OSC 8 超链接（TUI 显式声明）与纯文本 URL 同一个门控与通道。
    const handler = termOptions.current?.linkHandler as { activate: (e: MouseEvent, uri: string) => void };
    expect(handler).toBeTruthy();
    handler.activate(click({ metaKey: true }), "https://example.com/b");
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("open_link", { url: "https://example.com/b" }));
  });

  it("findFakeCaret：孤立单格反显是假光标，连排反显与多义画面不误认", () => {
    // 'X' = 反显格，'.' = 普通格。kimi 的输入光标就是一个孤立的反显空格。
    const bufferOf = (rows: string[]) => ({
      viewportY: 0,
      getLine: (y: number) => {
        const row = rows[y];
        if (row == null) return undefined;
        return {
          length: row.length,
          getCell: (x: number) => (x < row.length ? { isInverse: () => (row[x] === "X" ? 1 : 0) } : undefined),
        };
      },
    });
    // 唯一孤立反显 → 命中（输入行 "> ab▮"）。
    expect(findFakeCaret(bufferOf(["......", "..X...", "......"]), 3)).toEqual({ x: 2, y: 1 });
    // 菜单选中行是连排反显：整段跳过，孤立的那格仍命中。
    expect(findFakeCaret(bufferOf(["XXXXX.", "....X.", "......"]), 3)).toEqual({ x: 4, y: 1 });
    // 两个孤立反显：多义，放弃（维持 xterm 默认锚点）。
    expect(findFakeCaret(bufferOf(["..X...", "....X."]), 2)).toBeNull();
    // 没有反显：无从锚定。
    expect(findFakeCaret(bufferOf(["......", "......"]), 2)).toBeNull();
    expect(findFakeCaret(undefined, 2)).toBeNull();
  });

  it("offers takeover when the session is still running in an external terminal", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") return Promise.resolve(noPty);
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" />);
    // 没有托管 PTY 的运行中会话必须给出接管入口，而不是把用户丢在一块黑屏上。
    expect(await screen.findByRole("button", { name: "结束外部进程并接管" })).toBeTruthy();
    expect(screen.getByText(/会话仍在外部终端运行/)).toBeTruthy();
  });

  it("offers a plain start for a disconnected session", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") return Promise.resolve(noPty);
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="ended" />);
    expect(await screen.findByRole("button", { name: "在 Meowo 中接管" })).toBeTruthy();
  });

  /// 后台会话「没画面」有两种成因：worker 真退了，或只是旁路没接上。此前一律说成
  /// 「已结束」，于是一个还在跑的 worker 被谎报成死的，而且不给任何出路。
  it("后台会话没接上时说没接上并给重接入口，而不是谎报已结束", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") return Promise.resolve(noPty);
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" background />);
    expect(await screen.findByText(/没能接上这个后台会话的画面/)).toBeTruthy();
    expect(screen.queryByText(/这个后台会话已结束/)).toBeNull();
    // 接管/启动对后台会话必然失败，不给；能做的只有再接一次。
    expect(screen.queryByRole("button", { name: /接管/ })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "重新接入" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("attach_background_session", { sessionId: 163 }));
  });

  /// 拿到退出码才是真的结束了：那时既要说结束，也不该再给重接按钮。
  it("后台 worker 真的退出后说已结束，且收起重接按钮", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve({ ...noPty, exited: true, exitCode: 0 });
      }
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="ended" background />);
    expect(await screen.findByText(/这个后台会话已结束/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "重新接入" })).toBeNull();
  });

  it("shows the initializing cover until the managed PTY produces its first output", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve({ ...noPty, active: true, data: "" });
      }
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" />);
    // active 但还没有首屏输出：必须停在初始化态，不能是无提示的黑屏。
    await waitFor(() => expect(screen.getByRole("status")).toBeTruthy());
    expect(screen.getByText("正在初始化 Agent…")).toBeTruthy();
  });

  it("stays in the initializing cover while a resuming TUI only emits control sequences", async () => {
    // claude --resume 的真实首帧：查光标位置、进 alt screen、清屏、清 40 行——
    // 一个字都还没画。此前只要有字节就撤遮罩，用户面对的就是一块无提示的纯黑屏。
    const loading = "\x1b[6n\x1b[?25l\x1b[?1049h\x1b[2J" + "\x1b[K\r\n".repeat(40) + "\x1b[H";
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve({ ...noPty, active: true, data: btoa(loading) });
      }
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" />);
    await waitFor(() => expect(write).toHaveBeenCalled());
    expect(screen.getByRole("status")).toBeTruthy();
    expect(screen.getByText("正在初始化 Agent…")).toBeTruthy();
  });

  it("leaves the initializing cover once the TUI actually paints something", async () => {
    // btoa 只吃 latin1，这里用纯 ASCII 表示 TUI 画出的第一段文字。
    const painted = "\x1b[?1049h\x1b[2J\x1b[H\x1b[32mWelcome to Claude Code\x1b[0m";
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve({ ...noPty, active: true, data: btoa(painted) });
      }
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" />);
    await waitFor(() => expect(write).toHaveBeenCalled());
    await waitFor(() => expect(screen.queryByRole("status")).toBeNull());
  });

  it("reports a trust prompt during direct GUI takeover and uncovers the TUI", async () => {
    const attention = vi.fn();
    const prompt = "\x1b[2JDo you trust the contents of this directory?\r\n  Yes\r\n  No";
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve({ ...noPty, active: true, data: btoa(prompt), endOffset: prompt.length });
      }
      return Promise.resolve();
    });
    render(
      <ManagedTerminal
        sessionId={163}
        status="running"
        attentionMarkers={["do you trust the contents of this directory"]}
        onAttention={attention}
      />,
    );
    await waitFor(() => expect(attention).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(screen.queryByRole("status")).toBeNull());
  });

  it("reports an already-painted trust prompt when provider markers arrive later", async () => {
    const attention = vi.fn();
    const prompt = "\x1b[2JDo you trust the files in this folder?\r\n  Yes\r\n  No";
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve({ ...noPty, active: true, data: btoa(prompt), endOffset: prompt.length });
      }
      return Promise.resolve();
    });
    const view = render(<ManagedTerminal sessionId={163} status="running" onAttention={attention} />);
    await waitFor(() => expect(write).toHaveBeenCalled());
    expect(attention).not.toHaveBeenCalled();

    view.rerender(
      <ManagedTerminal
        sessionId={163}
        status="running"
        attentionMarkers={["do you trust the files in this folder"]}
        onAttention={attention}
      />,
    );
    await waitFor(() => expect(attention).toHaveBeenCalledTimes(1));
  });

  it("reports the token/auth step that appears after the trust step", async () => {
    const attention = vi.fn();
    const trust = "\x1b[2JDo you trust the files in this folder?\r\n  Yes\r\n  No";
    const auth = "\x1b[2JOAuth token has been revoked\r\nRun /login to sign in\r\nPress Enter to continue";
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve({ ...noPty, active: true, data: btoa(trust), endOffset: trust.length });
      }
      return Promise.resolve();
    });
    render(
      <ManagedTerminal
        sessionId={163}
        status="running"
        attentionMarkers={["do you trust the files in this folder"]}
        onAttention={attention}
      />,
    );
    await waitFor(() => expect(attention).toHaveBeenCalledTimes(1));
    eventHandlers.get("pty-output")!({ payload: { sessionId: 163, offset: trust.length, data: btoa(auth) } });
    await waitFor(() => expect(attention).toHaveBeenCalledTimes(2));
    expect(attention.mock.calls[1][0].text).toContain("OAuth token has been revoked");
  });

  /**
   * 提示从屏幕上消失(在终端里答掉了/界面翻页了)后必须自动收卡:此前 attention 只置
   * 不清,误报或已处理的提示会永久钉住卡片、锁死对话页输入框。清卡带连击门槛
   * (连续多次扫描不匹配)骑过 TUI 分笔重绘的中间帧;之后同类新提示还能再弹。
   */
  it("提示消失后自动发布 null 收卡,新提示可再弹", async () => {
    const attention = vi.fn();
    const trust = "\x1b[2JDo you trust the files in this folder?\r\n  Yes\r\n  No";
    const cleared = "\x1b[2Jworking on it...";
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve({ ...noPty, active: true, data: btoa(trust), endOffset: trust.length });
      }
      return Promise.resolve();
    });
    render(
      <ManagedTerminal
        sessionId={163}
        status="running"
        attentionMarkers={["do you trust the files in this folder"]}
        onAttention={attention}
      />,
    );
    await waitFor(() => expect(attention).toHaveBeenCalledTimes(1));
    // 提示被答掉:清屏后只剩普通输出。miss 分支会自我续排扫描凑满连击,无需更多输出。
    eventHandlers.get("pty-output")!({ payload: { sessionId: 163, offset: trust.length, data: btoa(cleared) } });
    await waitFor(() => expect(attention).toHaveBeenCalledTimes(2), { timeout: 3_000 });
    expect(attention.mock.calls[1][0]).toBeNull();
    // 下一个同类提示(内容相同)仍要能弹:签名去重已随清卡重置。
    eventHandlers.get("pty-output")!({ payload: { sessionId: 163, offset: trust.length + cleared.length, data: btoa(trust) } });
    await waitFor(() => expect(attention).toHaveBeenCalledTimes(3), { timeout: 3_000 });
    expect(attention.mock.calls[2][0]?.text).toContain("Do you trust");
  });

  /**
   * 收卡不能以「本组件报告过提示」为前提。启动提示由 ChatWindow 的 waitForTerminalReady
   * 直接置进去,本组件的签名 ref 仍是空的;老代码把清卡分支门控在那个 ref 上,于是用户
   * 在终端里答掉信任提示后,卡片在对话页永久钉死、把输入框一起锁住,后续真正的提问卡
   * (渲染条件含 !terminalAttention)再没机会出现。真机上正是这样卡住的。
   */
  it("屏幕上本就没有提示时也发一次 null,替外部置入的卡收场", async () => {
    const attention = vi.fn();
    const plain = "\x1b[2Jworking on it...";
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve({ ...noPty, active: true, data: btoa(plain), endOffset: plain.length });
      }
      return Promise.resolve();
    });
    render(
      <ManagedTerminal
        sessionId={163}
        status="running"
        attentionMarkers={["do you trust the files in this folder"]}
        onAttention={attention}
      />,
    );
    // 一次都没匹配过,仍要收到 null——这是老代码进不去的分支。
    await waitFor(() => expect(attention).toHaveBeenCalledWith(null), { timeout: 3_000 });
    // 且只发一次:每帧刷 null 会把 ChatWindow 的状态更新拖成噪音。
    const nullCalls = () => attention.mock.calls.filter((call) => call[0] === null).length;
    expect(nullCalls()).toBe(1);
    eventHandlers.get("pty-output")!({ payload: { sessionId: 163, offset: plain.length, data: btoa("still working") } });
    await new Promise((resolve) => setTimeout(resolve, 800));
    expect(nullCalls()).toBe(1);
  });

  it("orders and deduplicates live output buffered while the initial snapshot is loading", async () => {
    let resolveSnapshot!: (value: typeof noPty) => void;
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") {
        return new Promise((resolve) => { resolveSnapshot = resolve; });
      }
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" />);
    await waitFor(() => expect(eventHandlers.has("pty-output")).toBe(true));
    await waitFor(() => expect(resolveSnapshot).toBeTypeOf("function"));
    // DEF 已包含在稍后返回的 ABCDEF 快照里；offset 让前端只写一次完整内容。
    eventHandlers.get("pty-output")!({ payload: { sessionId: 163, offset: 3, data: btoa("DEF") } });
    resolveSnapshot({ ...noPty, active: true, data: btoa("ABCDEF"), endOffset: 6 });
    await waitFor(() => expect(write).toHaveBeenCalled());
    expect(write).toHaveBeenCalledTimes(1);
    expect(new TextDecoder().decode(write.mock.calls[0][0])).toBe("ABCDEF");
  });

  it("confirm 通过后真的调用接管 invoke", async () => {
    // 回归：此前用 window.confirm——Tauri webview 会吞掉它、恒返回 false，接管按钮永远点不动；
    // 而旧测试只断言按钮渲染、从不点击，刚好放过了这个 bug。这里必须点下去走完全链路。
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") return Promise.resolve(noPty);
      if (command === "confirm_dialog") return Promise.resolve(confirmAnswer.ok);
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" />);
    const button = await screen.findByRole("button", { name: "结束外部进程并接管" });
    button.click();
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("confirm_dialog", expect.anything()));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("takeover_managed_terminal", { sessionId: 163, cols: 80, rows: 24 }),
    );
  });

  it("confirm 取消时不调用接管 invoke", async () => {
    confirmAnswer.ok = false;
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") return Promise.resolve(noPty);
      if (command === "confirm_dialog") return Promise.resolve(confirmAnswer.ok);
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" />);
    const button = await screen.findByRole("button", { name: "结束外部进程并接管" });
    button.click();
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("confirm_dialog", expect.anything()));
    expect(invoke.mock.calls.some(([command]) => command === "takeover_managed_terminal")).toBe(false);
  });

  it("结束终端需 confirm 确认后才调用 stop_managed_terminal", async () => {
    // 回归：结束终端是破坏性操作（直接杀 Agent 进程），此前一点就杀、没有任何确认。
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve({ ...noPty, active: true, data: btoa("ready"), endOffset: 5 });
      }
      if (command === "confirm_dialog") return Promise.resolve(confirmAnswer.ok);
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" />);
    const button = await screen.findByRole("button", { name: "结束终端" });
    button.click();
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("confirm_dialog", expect.anything()));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("stop_managed_terminal", { sessionId: 163 }));
  });

  it("结束终端的 confirm 被取消时不杀进程", async () => {
    confirmAnswer.ok = false;
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve({ ...noPty, active: true, data: btoa("ready"), endOffset: 5 });
      }
      if (command === "confirm_dialog") return Promise.resolve(confirmAnswer.ok);
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" />);
    const button = await screen.findByRole("button", { name: "结束终端" });
    button.click();
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("confirm_dialog", expect.anything()));
    expect(invoke.mock.calls.some(([command]) => command === "stop_managed_terminal")).toBe(false);
  });

  it("realigns the output offset when the PTY is restarted in place", async () => {
    // 结束终端 → 再接管：新 PTY 的 output_end 从 0 重新计数。若沿用上一个进程的
    // nextOffset（这里是 7），新输出会被判成「已写过」而整段丢弃，终端定格在旧内容上。
    let snapshots = 0;
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") {
        snapshots += 1;
        return snapshots === 1
          // 上一个进程留下 7 字节输出后退出。
          ? Promise.resolve({ ...noPty, data: btoa("OLDDATA"), endOffset: 7, exited: true, exitCode: 0 })
          // 接管后的新 PTY：偏移归零，还没有输出。
          : Promise.resolve({ ...noPty, active: true, data: "", endOffset: 0 });
      }
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="ended" />);
    const takeover = await screen.findByRole("button", { name: "在 Meowo 中接管" });
    write.mockReset();
    takeover.click();
    await waitFor(() => expect(snapshots).toBe(2));
    // 新进程的首段输出比旧的短（2 < 7），偏移没归零的话会被整段吞掉。
    eventHandlers.get("pty-output")!({ payload: { sessionId: 163, offset: 0, data: btoa("HI") } });
    await waitFor(() => expect(write).toHaveBeenCalled());
    expect(new TextDecoder().decode(write.mock.calls[0][0])).toBe("HI");
  });

  /**
   * 预绘期不再需要前端拦 CPR:启动探测由后端代答并**从流中摘除**(pty.rs
   * StartupProbeScanner),xterm 根本看不到已代答的查询。反过来,凡是 xterm 真的
   * 应答了的查询(DECXCPR `?6n` 变体、首帧后的实时查询),它的应答就是唯一一份,
   * 必须原样转发——预绘期一并拦掉会让 TUI 等不到应答。
   */
  it("回放窗口关闭后 CPR 应答原样转发(预绘期不拦)", async () => {
    // 后端摘除探测后的冷启动形态:藏光标、清屏,流里已没有 \x1b[6n。
    const loading = "\x1b[?25l\x1b[2J";
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve({ ...noPty, active: true, data: btoa(loading), endOffset: loading.length });
      }
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" />);
    await waitFor(() => expect(dataHandler.current).toBeTruthy());
    await waitFor(() => expect(write).toHaveBeenCalled());
    // 回放窗口已随 write 回调关闭:首帧未画,CPR 也照常转发(它对应的查询后端没答)。
    dataHandler.current!("\x1b[24;1R");
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 163, data: "\x1b[24;1R" }));
    // 用户真实按键同样不受影响。
    dataHandler.current!("a");
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 163, data: "a" }));
  });

  it("stripTerminalReplies 剔除各类自动应答、保留用户输入", () => {
    // CPR / DA1 / DA2 / DSR 状态 / DECRPM / OSC 颜色应答 / DCS 应答。
    expect(stripTerminalReplies("\x1b[24;1R")).toBe("");
    expect(stripTerminalReplies("\x1b[?1;2c")).toBe("");
    expect(stripTerminalReplies("\x1b[>0;276;0c")).toBe("");
    expect(stripTerminalReplies("\x1b[0n")).toBe("");
    expect(stripTerminalReplies("\x1b[?2026;2$y")).toBe("");
    // DECRPM 的 ANSI 形态不带 '?'(xterm 对 CSI Ps $ p 的应答),同样要拦。
    expect(stripTerminalReplies("\x1b[4;2$y")).toBe("");
    // CSI-t 窗口尺寸报告(windowOptions 开启时 xterm 会应答 CSI 18 t 等)。
    expect(stripTerminalReplies("\x1b[8;24;80t")).toBe("");
    expect(stripTerminalReplies("\x1b]11;rgb:1e1e/1e1e/1e1e\x07")).toBe("");
    expect(stripTerminalReplies("\x1bP>|xterm\x1b\\")).toBe("");
    // 混着来也只剔应答;普通字符、回车、方向键(无参数的 \x1b[C)原样保留。
    expect(stripTerminalReplies("a\x1b[?1;2cb\r")).toBe("ab\r");
    expect(stripTerminalReplies("\x1b[C")).toBe("\x1b[C");
    expect(stripTerminalReplies("你好")).toBe("你好");
  });

  /**
   * 重连回放不得把 xterm 的自动应答打进 PTY:快照会整段回放历史,里面 agent 当年的
   * 查询(\x1b[6n 等)会被 xterm 再答一遍,迟到的应答落进正跑着的 agent 输入框,
   * 变成孤立的尾字符(真实案例:每次重连 claude 的 composer 里多一个 C)。
   * 拦截仅限回放窗口:窗口内用户按键照常放行,窗口结束后应答恢复转发。
   */
  it("历史回放窗口内拦下自动应答,回放结束与用户输入不受影响", async () => {
    writeCallbacks.manual = true;
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve({ sessionId: 163, active: true, data: btoa("history \x1b[6n tail"), startOffset: 0, endOffset: 16, exited: false, exitCode: null });
      }
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" />);
    await waitFor(() => expect(dataHandler.current).toBeTruthy());
    await waitFor(() => expect(write).toHaveBeenCalled());

    // 回放已入队、完成回调未触发:此刻 xterm 对历史查询吐出 CPR 应答 → 必须拦下。
    dataHandler.current!("\x1b[24;1R");
    expect(invoke.mock.calls.some(([command]) => command === "write_managed_terminal")).toBe(false);
    // 同一窗口里的用户真实输入不受影响。
    dataHandler.current!("a");
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 163, data: "a" }));

    // 回放完成后,agent 实时查询的应答是它正在等的,恢复原样转发。
    writeCallbacks.queue.forEach((callback) => callback());
    dataHandler.current!("\x1b[24;1R");
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 163, data: "\x1b[24;1R" }));
  });
});

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.hoisted(() => vi.fn());
const write = vi.hoisted(() => vi.fn());
const resetSpy = vi.hoisted(() => vi.fn());
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
// 终端选区：复制快捷键测试用（有选区的 Ctrl+C=复制不发 ^C，无选区照旧发）。
const selection = vi.hoisted(() => ({ text: "" }));
// terminal.paste 的间谍（右键粘贴测试断言剪贴板文本进了 xterm 粘贴通路）。
const pasteSpy = vi.hoisted(() => vi.fn());
// terminal.scrollToBottom 的间谍（退出提示写入后必须滚底，否则上翻视口里提示在屏外）。
const scrollToBottomSpy = vi.hoisted(() => vi.fn());
// terminal.resize 的间谍（隐藏态网格对齐：快照带的 PTY 尺寸要直接钉到网格上）。
const resizeGridSpy = vi.hoisted(() => vi.fn());
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
    reset = resetSpy;
    focus = vi.fn();
    dispose = vi.fn();
    loadAddon = vi.fn();
    onData = (handler: (data: string) => void) => { dataHandler.current = handler; return { dispose: vi.fn() }; };
    attachCustomKeyEventHandler = (handler: (event: KeyboardEvent) => boolean) => { keyHandler.current = handler; };
    hasSelection = () => selection.text.length > 0;
    getSelection = () => selection.text;
    clearSelection = () => { selection.text = ""; };
    // UnicodeGraphemesAddon 激活时会读写 unicode.activeVersion;哑实现只要可赋值。
    unicode = { activeVersion: "6" };
    paste = (data: string) => pasteSpy(data);
    scrollToBottom = scrollToBottomSpy;
    resize = (cols: number, rows: number) => { resizeGridSpy(cols, rows); this.cols = cols; this.rows = rows; };
  },
}));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit = vi.fn(); } }));
// 终端内搜索:findNext/findPrevious 的返回值驱动「无匹配」提示,用可控 spy。
const searchFindNext = vi.hoisted(() => vi.fn(() => true));
const searchFindPrevious = vi.hoisted(() => vi.fn(() => true));
vi.mock("@xterm/addon-search", () => ({
  SearchAddon: class {
    findNext = searchFindNext;
    findPrevious = searchFindPrevious;
  },
}));
// jsdom 无 WebGL:构造直接抛,组件必须静默回退 canvas(这正是要测的路径)。
vi.mock("@xterm/addon-webgl", () => ({
  WebglAddon: class {
    constructor() { throw new Error("no webgl in jsdom"); }
  },
}));
vi.mock("@xterm/addon-unicode-graphemes", () => ({ UnicodeGraphemesAddon: class {} }));
// 接管确认走应用内原生小窗(invoke confirm_dialog),不再用 plugin-dialog / window.confirm。
// 用 confirmAnswer 控制那次 invoke 的返回;plugin-dialog 仍被 mock 以防其它路径引用。
vi.mock("@tauri-apps/plugin-dialog", () => ({ confirm: vi.fn(), open: vi.fn() }));
const confirmAnswer = vi.hoisted(() => ({ ok: true }));

import { findFakeCaret, ManagedTerminal, STREAM_STALL_MS, stripTerminalReplies, terminalStreamStalled } from "./ManagedTerminal";

const noPty = { sessionId: 163, active: false, managed: false, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null, cols: 0, rows: 0, modes: [] as number[] };

describe("ManagedTerminal", () => {
  afterEach(cleanup);
  beforeEach(() => {
    invoke.mockReset();
    write.mockReset();
    resetSpy.mockReset();
    confirmAnswer.ok = true;
    eventHandlers.clear();
    dataHandler.current = null;
    writeCallbacks.manual = false;
    writeCallbacks.queue = [];
    selection.text = "";
    scrollToBottomSpy.mockReset();
    resizeGridSpy.mockReset();
    searchFindNext.mockReset().mockReturnValue(true);
    searchFindPrevious.mockReset().mockReturnValue(true);
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
    // 无选区的 Ctrl+C 维持终端语义：交给 xterm 发 ^C 中断前台进程。
    expect(keyHandler.current!(key({ type: "keydown", code: "KeyC", ctrlKey: true }))).toBe(true);
    // Ctrl+Alt+V（AltGr 组合可能产字符）不劫持。
    expect(keyHandler.current!(key({ type: "keydown", code: "KeyV", ctrlKey: true, altKey: true }))).toBe(true);
  });

  it("复制快捷键：有选区的 Ctrl/Cmd+C 与 Ctrl+Shift+C 复制选区且不下发按键", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") return Promise.resolve(noPty);
      return Promise.resolve();
    });
    const writeText = vi.fn(() => Promise.resolve());
    Object.assign(navigator, { clipboard: { writeText } });
    render(<ManagedTerminal sessionId={163} status="running" />);
    await waitFor(() => expect(keyHandler.current).toBeTruthy());
    const key = (init: Partial<KeyboardEvent> & { type: string; code: string }) => init as KeyboardEvent;
    selection.text = "error: os error 2";
    // 有选区：Ctrl+C = 复制，false 表示不交给 xterm（绝不发 ^C 中断 agent）。
    expect(keyHandler.current!(key({ type: "keydown", code: "KeyC", ctrlKey: true }))).toBe(false);
    expect(writeText).toHaveBeenCalledWith("error: os error 2");
    // Ctrl+Shift+C（终端惯用复制键）同样复制。
    expect(keyHandler.current!(key({ type: "keydown", code: "KeyC", ctrlKey: true, shiftKey: true }))).toBe(false);
    // macOS 的 Cmd+C 同理。
    expect(keyHandler.current!(key({ type: "keydown", code: "KeyC", metaKey: true }))).toBe(false);
    expect(writeText).toHaveBeenCalledTimes(3);
  });

  it("Ctrl+F 打开终端搜索:输入即搜、Enter/Shift+Enter 翻命中、Esc 关闭", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") return Promise.resolve(noPty);
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" />);
    await waitFor(() => expect(keyHandler.current).toBeTruthy());
    const key = (init: Partial<KeyboardEvent> & { type: string; code: string }) => init as KeyboardEvent;
    // Ctrl+F 被拦下（false = 不交给 xterm，^F 不落 PTY），搜索条出现。
    expect(keyHandler.current!(key({ type: "keydown", code: "KeyF", ctrlKey: true }))).toBe(false);
    const input = await screen.findByPlaceholderText("搜索终端输出");
    // 逐字输入走 incremental（在当前选区上扩展匹配，不往后跳）。
    fireEvent.change(input, { target: { value: "error" } });
    expect(searchFindNext).toHaveBeenCalledWith("error", { incremental: true });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(searchFindNext).toHaveBeenLastCalledWith("error", { incremental: false });
    fireEvent.keyDown(input, { key: "Enter", shiftKey: true });
    expect(searchFindPrevious).toHaveBeenCalledWith("error", { incremental: false });
    // Esc 关闭搜索条（容器统一截停，不落到窗口级动作）。
    fireEvent.keyDown(input, { key: "Escape" });
    expect(screen.queryByPlaceholderText("搜索终端输出")).toBeNull();
  });

  it("终端搜索无匹配时显示提示;WebGL 构造失败静默回退不炸组件", async () => {
    searchFindNext.mockReturnValue(false);
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") return Promise.resolve(noPty);
      return Promise.resolve();
    });
    // WebglAddon 的 mock 构造恒抛（jsdom 无 WebGL）——render 不抛即证明回退路径成立。
    render(<ManagedTerminal sessionId={163} status="running" />);
    await waitFor(() => expect(keyHandler.current).toBeTruthy());
    const key = (init: Partial<KeyboardEvent> & { type: string; code: string }) => init as KeyboardEvent;
    keyHandler.current!(key({ type: "keydown", code: "KeyF", ctrlKey: true }));
    const input = await screen.findByPlaceholderText("搜索终端输出");
    fireEvent.change(input, { target: { value: "nowhere" } });
    expect(await screen.findByText("无匹配")).toBeTruthy();
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
    expect(screen.getByText(/会话在外部终端运行/)).toBeTruthy();
  });

  it("offers a plain start for a disconnected session", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") return Promise.resolve(noPty);
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="ended" />);
    expect(await screen.findByRole("button", { name: "恢复会话" })).toBeTruthy();
  });

  /// 后台会话「没画面」有两种成因：worker 真退了，或只是旁路没接上。此前一律说成
  /// 「已结束」，于是一个还在跑的 worker 被谎报成死的，而且不给任何出路。
  it("后台会话没接上时说没接上并给重接入口，而不是谎报已结束", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") return Promise.resolve(noPty);
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" background />);
    expect(await screen.findByText(/没接上后台会话的画面/)).toBeTruthy();
    expect(screen.queryByText(/后台会话已结束/)).toBeNull();
    // 接管/恢复对后台会话必然失败，不给；能做的只有再接一次。
    expect(screen.queryByRole("button", { name: /接管|恢复会话/ })).toBeNull();
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
    expect(await screen.findByText(/后台会话已结束/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "重新接入" })).toBeNull();
  });

  /// 会话结束后「接管的框」必须清晰地在（实拍反馈：曾降成底部窄横条，TUI 退出清屏后
  /// 黑屏 + 窄条被读成「框不见了」）。现约定：居中卡片 + 层背景穿透（输出仍可滚可选）。
  it("会话退出后显示居中接管卡片（层穿透保留输出可达），按钮可点", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve({ ...noPty, exited: true, exitCode: 1 });
      }
      return Promise.resolve();
    });
    const { container } = render(<ManagedTerminal sessionId={163} status="ended" />);
    // 居中卡片出现，带退出说明与接管按钮
    await waitFor(() => expect(container.querySelector(".managed-terminal-exit-card")).toBeTruthy());
    expect(screen.getByText(/Agent 进程已退出（退出码 1）/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "恢复会话" })).toBeTruthy();
    // 遮罩层本体穿透（输出可滚可选），卡片自身可交互
    expect(container.querySelector(".managed-terminal-cover.is-exited")).toBeTruthy();
  });

  /// 实拍反馈「终端没有回到底部」：进程退出时若视口停在上翻位置，退出提示行与
  /// 接管卡片的语境都在屏外。写完提示必须显式滚底（不能再依赖 resize 重绘副作用）。
  it("进程退出时写入提示并滚动到底部", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve({ ...noPty, active: true, data: btoa("running"), endOffset: 7 });
      }
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" />);
    await waitFor(() => expect(eventHandlers.get("pty-exit")).toBeTruthy());
    await waitFor(() => expect(write).toHaveBeenCalled());
    scrollToBottomSpy.mockReset();
    eventHandlers.get("pty-exit")!({ payload: { sessionId: 163, code: 0 } });
    await waitFor(() => expect(scrollToBottomSpy).toHaveBeenCalled());
    expect(write.mock.calls.some(([data]) => typeof data === "string" && data.includes("process exited"))).toBe(true);
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

  it("事件流出现偏移缺口时拉快照重对齐,画面无缺无重", async () => {
    // 后端在 UI 卡顿时会丢 emit 帧(保 agent 不保画面),合帧线程刻意不抹平偏移洞;
    // 前端看到洞必须拉快照补齐,否则缺口两侧直接续写,画面静默错乱。
    let snapshots = 0;
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") {
        snapshots += 1;
        if (snapshots === 1) {
          return Promise.resolve({ ...noPty, active: true, data: btoa("ABC"), endOffset: 3 });
        }
        // 重对齐请求(since=3):返回缺口段 DEF 及其后的 GHI。
        return Promise.resolve({ ...noPty, active: true, data: btoa("DEFGHI"), startOffset: 3, endOffset: 9 });
      }
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" />);
    await waitFor(() => expect(write).toHaveBeenCalled());
    // DEF(offset 3..6)在后端被丢帧,直接到达 offset=6 的 GHI → 缺口。
    eventHandlers.get("pty-output")!({ payload: { sessionId: 163, offset: 6, data: btoa("GHI") } });
    await waitFor(() => expect(snapshots).toBe(2));
    // 快照补齐 DEFGHI;缓冲的 GHI 事件回放时因区间已覆盖被裁剪,不重复。
    const painted = () =>
      write.mock.calls
        .map((call) => (typeof call[0] === "string" ? call[0] : new TextDecoder().decode(call[0])))
        .join("");
    await waitFor(() => expect(painted()).toBe("ABCDEFGHI"));
  });

  it("右键:有选区复制并清选区,无选区读剪贴板走 paste 通路(仅 Windows 惯例)", async () => {
    // WebView 默认右键菜单被 devtools-guard 封死,封死后右键此前没有任何替代行为
    // (实拍反馈"右键复制没生效")。约定与 Windows 终端一致:右键=复制或粘贴;
    // macOS 的右键是菜单语义,组件按 platform 门控——测试环境显式装成 Windows。
    Object.defineProperty(navigator, "platform", { value: "Win32", configurable: true });
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") return Promise.resolve({ ...noPty, active: true, data: btoa("hi"), endOffset: 2 });
      if (command === "clipboard_text") return Promise.resolve("from-clipboard");
      return Promise.resolve();
    });
    const writeText = vi.fn(() => Promise.resolve());
    Object.assign(navigator, { clipboard: { writeText } });
    pasteSpy.mockClear();
    render(<ManagedTerminal sessionId={163} status="running" />);
    const host = document.querySelector(".managed-terminal-host")!;
    // 有选区:复制选区、清选区,不碰剪贴板读取。
    selection.text = "picked";
    fireEvent.contextMenu(host);
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("picked"));
    expect(selection.text).toBe("");
    expect(invoke).not.toHaveBeenCalledWith("clipboard_text");
    // 无选区:读后端剪贴板,文本进 xterm 的 paste 通路(bracketed paste + onData 下发)。
    fireEvent.contextMenu(host);
    await waitFor(() => expect(pasteSpy).toHaveBeenCalledWith("from-clipboard"));
  });

  it("Ctrl+Enter / Shift+Enter 按插件声明的换行序列注入,不落成提交", async () => {
    // 终端传统编码里带修饰的 Enter 与裸 Enter 同码(\r):用户按「换行」,CC 收到「提交」。
    // WT 的 /terminal-setup 把 Shift+Enter 配成发 \x1b\r(meta+return,ink 认作换行);
    // 注入序列由插件声明经 chatUi 下发(newlineInput prop),未声明的 agent 不注入。
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") return Promise.resolve({ ...noPty, active: true, data: btoa("hi"), endOffset: 2 });
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" newlineInput={"\x1b\r"} />);
    await waitFor(() => expect(keyHandler.current).toBeTruthy());
    for (const init of [{ ctrlKey: true }, { shiftKey: true }] as KeyboardEventInit[]) {
      invoke.mockClear();
      const handled = keyHandler.current!(new KeyboardEvent("keydown", { key: "Enter", ...init }));
      expect(handled).toBe(false); // xterm 不再自己编码(那会发裸 \r = 提交)
      await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 163, data: "\x1b\r" }));
    }
    // 裸 Enter 照常放行给 xterm(正常提交语义不变)。
    expect(keyHandler.current!(new KeyboardEvent("keydown", { key: "Enter" }))).toBe(true);
  });

  it("输出流停滞判定:只在「托管 + running + 超时无字节」时报警,空闲/后台/外部不误报", () => {
    const base = { active: true, background: false, status: "running" as string | undefined, reviewPending: false, lastByteAt: 0, now: STREAM_STALL_MS + 1 };
    expect(terminalStreamStalled(base)).toBe(true);
    // 阈值之内不报——CC 干活时 TUI 百毫秒级重绘,30s 零字节才是僵死的指纹。
    expect(terminalStreamStalled({ ...base, now: STREAM_STALL_MS - 1 })).toBe(false);
    // waiting/idle 时终端安静是常态;后台会话没有实时帧通道;非托管本就无流。
    expect(terminalStreamStalled({ ...base, status: "waiting" })).toBe(false);
    expect(terminalStreamStalled({ ...base, status: undefined })).toBe(false);
    expect(terminalStreamStalled({ ...base, background: true })).toBe(false);
    expect(terminalStreamStalled({ ...base, active: false })).toBe(false);
    // 等审批/屏幕提示时回合没结束,status 仍是 running,但 TUI 画完表单后零输出是
    // 「在等人」的常态——不豁免就是实拍过的误报(审批框在屏却弹「疑似卡死」)。
    expect(terminalStreamStalled({ ...base, reviewPending: true })).toBe(false);
  });

  it("挂载即注册「正在看」,卸载时注销——后端 emitter 只喂被观看的会话", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve({ ...noPty, active: true, data: btoa("hi"), endOffset: 2 });
      }
      return Promise.resolve();
    });
    const view = render(<ManagedTerminal sessionId={163} status="running" />);
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("register_terminal_viewer", { sessionId: 163 }));
    view.unmount();
    expect(invoke).toHaveBeenCalledWith("unregister_terminal_viewer", { sessionId: 163 });
  });

  it("重对齐增量过大时跳尾:reset 后只回放尾部,不全量灌 xterm", async () => {
    // 落后 256KB+ 时全量写 xterm 是数秒的渲染卡死,期间事件继续堆积、可能永远追不上。
    // 终端是实时视图:落后就该直接看最新画面(尾部一段),截断的半截 ANSI 由 TUI 的
    // 下一次全屏重绘自愈。
    const big = "A".repeat(300_000) + "TAIL-MARKER";
    let snapshots = 0;
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") {
        snapshots += 1;
        if (snapshots === 1) return Promise.resolve({ ...noPty, active: true, data: btoa("ABC"), endOffset: 3 });
        // 重对齐(since=3):落后的增量一次性到齐,远超跳尾阈值。
        return Promise.resolve({ ...noPty, active: true, data: btoa(big), startOffset: 3, endOffset: 3 + big.length });
      }
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" />);
    await waitFor(() => expect(write).toHaveBeenCalled());
    // 偏移洞触发重对齐;洞后的实时帧与快照末尾正好衔接。
    eventHandlers.get("pty-output")!({ payload: { sessionId: 163, offset: 3 + big.length, data: btoa("!") } });
    await waitFor(() => expect(resetSpy).toHaveBeenCalled());
    await waitFor(() => {
      const texts = write.mock.calls
        .map((call) => (typeof call[0] === "string" ? call[0] : new TextDecoder().decode(call[0])));
      // 尾段上屏且带着结尾标记;任何一次写入都不得是全量(证明没把 300KB 灌进 xterm)。
      expect(texts.some((text) => text.endsWith("TAIL-MARKER"))).toBe(true);
      expect(Math.max(...texts.map((text) => text.length))).toBeLessThanOrEqual(64 * 1024);
      // 回放基线:截断点之前的 ?25l 已丢,跳尾必须先藏硬件光标(防「输入框两个光标」)。
      expect(texts.some((text) => text.includes("\x1b[?25l"))).toBe(true);
    });
    // 跳尾后偏移已对齐到快照末尾:洞后的实时帧原样续写,不重复不再触发重对齐。
    await waitFor(() => {
      const texts = write.mock.calls
        .map((call) => (typeof call[0] === "string" ? call[0] : new TextDecoder().decode(call[0])));
      expect(texts.at(-1)).toBe("!");
    });
  });

  it("快照与回放之间再次丢帧:缓冲里仍有洞时继续补拉,不跨洞直写", async () => {
    // 回归:重对齐快照落地后,缓冲的实时帧之间可能**又**出现洞(持续重输出时后端连续
    // 丢帧)。旧逻辑对缓冲一律直写——洞两侧被拼成「看似连续」的字节流,终端花屏
    // (多帧内容交错重叠,实拍)。现在遇洞停下再拉一次快照,画面无缺无重。
    let snapshots = 0;
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") {
        snapshots += 1;
        if (snapshots === 1) return Promise.resolve({ ...noPty, active: true, data: btoa("ABC"), endOffset: 3 });
        // 第一次重对齐(since=3):只补到 6——缓冲里 offset=12 的帧仍隔着 9..12 的洞。
        if (snapshots === 2) return Promise.resolve({ ...noPty, active: true, data: btoa("DEF"), startOffset: 3, endOffset: 6 });
        // 第二次重对齐(since=9):补齐剩余全部。
        return Promise.resolve({ ...noPty, active: true, data: btoa("JKLMNO"), startOffset: 9, endOffset: 15 });
      }
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" />);
    await waitFor(() => expect(write).toHaveBeenCalled());
    // 洞 1(3..6 被丢)触发重对齐;重对齐期间又到达 offset=12 的帧(9..12 也被丢,洞 2)。
    eventHandlers.get("pty-output")!({ payload: { sessionId: 163, offset: 6, data: btoa("GHI") } });
    eventHandlers.get("pty-output")!({ payload: { sessionId: 163, offset: 12, data: btoa("MNO") } });
    await waitFor(() => expect(snapshots).toBeGreaterThanOrEqual(3));
    const painted = () =>
      write.mock.calls
        .map((call) => (typeof call[0] === "string" ? call[0] : new TextDecoder().decode(call[0])))
        .join("");
    await waitFor(() => expect(painted()).toBe("ABCDEFGHIJKLMNO"));
  });

  it("缺口段已被 backlog 淘汰时 reset 后按现存起点重画,不无限重试", async () => {
    let snapshots = 0;
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") {
        snapshots += 1;
        if (snapshots === 1) {
          return Promise.resolve({ ...noPty, active: true, data: btoa("ABC"), endOffset: 3 });
        }
        // since=3 的缺口段已被 1MiB 窗口淘汰:返回现存 backlog,起点仍在缺口之后。
        return Promise.resolve({ ...noPty, active: true, data: btoa("YZ"), startOffset: 1000, endOffset: 1002 });
      }
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" />);
    await waitFor(() => expect(write).toHaveBeenCalled());
    eventHandlers.get("pty-output")!({ payload: { sessionId: 163, offset: 1002, data: btoa("!") } });
    await waitFor(() => expect(snapshots).toBe(2));
    await waitFor(() => expect(resetSpy).toHaveBeenCalled());
    // reset 后以 startOffset=1000 为新起点:先落回放基线(藏硬件光标,截断点之前的
    // ?25l 已随淘汰丢失,不藏会双光标),再画快照内容 YZ,缓冲的实时帧 "!" 紧随其后。
    const paintedAfterReset = () =>
      write.mock.calls
        .slice(1)
        .map((call) => (typeof call[0] === "string" ? call[0] : new TextDecoder().decode(call[0])))
        .join("");
    await waitFor(() => expect(paintedAfterReset()).toBe("\x1b[?25lYZ!"));
    // 不再有第三次快照:淘汰是终局,不能拿重试打转。
    await new Promise((resolve) => setTimeout(resolve, 300));
    expect(snapshots).toBe(2);
  });

  /**
   * claude 2.1.238 的全屏渲染器启动即 `?1049h` + `?1000-1006h`,这些开关只发一次,1MiB
   * backlog 很快把它们淘汰;重对齐 reset 后若只补 `?25l`,xterm 退回主屏、关掉鼠标上报,
   * 而 TUI 仍按全屏 + 鼠标模式画(实拍:两条滚动条、滚轮滚的不是 TUI 的内容)。后端
   * ModeTracker 把此刻开着的模式随快照带来,基线按序补写(1049 在前:清屏切缓冲)。
   */
  it("reset 回放基线按快照 modes 补写备用屏/鼠标模式,1049 在前", async () => {
    let snapshots = 0;
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") {
        snapshots += 1;
        if (snapshots === 1) {
          return Promise.resolve({ ...noPty, active: true, data: btoa("ABC"), endOffset: 3 });
        }
        return Promise.resolve({ ...noPty, active: true, data: btoa("YZ"), startOffset: 1000, endOffset: 1002, modes: [1049, 1000, 1006] });
      }
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" />);
    await waitFor(() => expect(write).toHaveBeenCalled());
    eventHandlers.get("pty-output")!({ payload: { sessionId: 163, offset: 1002, data: btoa("!") } });
    await waitFor(() => expect(resetSpy).toHaveBeenCalled());
    const paintedAfterReset = () =>
      write.mock.calls
        .slice(1)
        .map((call) => (typeof call[0] === "string" ? call[0] : new TextDecoder().decode(call[0])))
        .join("");
    await waitFor(() => expect(paintedAfterReset()).toBe("\x1b[?25l\x1b[?1049h\x1b[?1000h\x1b[?1006hYZ!"));
  });

  it("首挂载回放起点已被裁剪时同样补写 modes 基线", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve({ ...noPty, active: true, data: btoa("tail"), startOffset: 5000, endOffset: 5004, modes: [1049, 2004] });
      }
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" />);
    await waitFor(() => expect(write).toHaveBeenCalled());
    const painted = () =>
      write.mock.calls
        .map((call) => (typeof call[0] === "string" ? call[0] : new TextDecoder().decode(call[0])))
        .join("");
    await waitFor(() => expect(painted()).toBe("\x1b[?25l\x1b[?1049h\x1b[?2004htail"));
  });

  /**
   * 隐藏态挂载（后台屏幕识别 / 切会话时停在对话页）的花屏根因：宿主是屏外停靠盒
   * （固定 1000×700），按它 fit 出的网格与 PTY 尺寸脱节，隐藏期到达的帧按错误宽度
   * 换行、错行叠画；切回终端页时若 PTY 尺寸未变，resize 同值短路不触发 TUI 重画，
   * 花屏永不自愈（实拍）。现约定：隐藏态网格向快照带的 PTY 尺寸对齐，且不反向
   * 下发 PTY resize（对齐的是网格跟 PTY，不是 PTY 跟停靠盒）。
   */
  it("隐藏态挂载把网格钉到快照的 PTY 尺寸,不下发 PTY resize", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve({ ...noPty, active: true, data: btoa("hi"), endOffset: 2, cols: 213, rows: 44 });
      }
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" visible={false} />);
    await waitFor(() => expect(resizeGridSpy).toHaveBeenCalledWith(213, 44));
    expect(invoke.mock.calls.some(([command]) => command === "resize_managed_terminal")).toBe(false);
  });

  it("可见挂载不做快照网格对齐(fit 才是尺寸权威),尺寸未知(0)也不对齐", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") {
        // 可见:即便快照带尺寸也不动网格——可见网格由 fit 按真实宿主算。
        return Promise.resolve({ ...noPty, active: true, data: btoa("hi"), endOffset: 2, cols: 213, rows: 44 });
      }
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" />);
    await waitFor(() => expect(write).toHaveBeenCalled());
    expect(resizeGridSpy).not.toHaveBeenCalled();
    cleanup();
    resizeGridSpy.mockClear();
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") {
        // 隐藏但尺寸未知(如后台旁路,cols/rows=0):跳过,维持现状。
        return Promise.resolve({ ...noPty, active: true, data: btoa("hi"), endOffset: 2, cols: 0, rows: 0 });
      }
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" visible={false} />);
    await waitFor(() => expect(write).toHaveBeenCalled());
    expect(resizeGridSpy).not.toHaveBeenCalled();
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

  it("终端操作条不再有「结束终端」——结束入口统一为标题栏的「结束会话」", async () => {
    // 曾并存两个入口(同一条 confirmStopSession 流程),终端页这份是纯冗余,已撤下。
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve({ ...noPty, active: true, data: btoa("ready"), endOffset: 5 });
      }
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" />);
    // 等操作条渲染出来(以仍保留的「在外部终端同步打开」为锚)再断言。
    await screen.findByRole("button", { name: "在外部终端同步打开" });
    expect(screen.queryByRole("button", { name: "结束终端" })).toBeNull();
  });

  it("realigns the output offset when the PTY is restarted in place", async () => {
    // 结束会话 → 再接管：新 PTY 的 output_end 从 0 重新计数。若沿用上一个进程的
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
    const takeover = await screen.findByRole("button", { name: "恢复会话" });
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

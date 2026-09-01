import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
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
// Terminal 构造计数:会话 id 变更不应重建 xterm(T-13 平滑重绑的断言锚点)。
const constructSpy = vi.hoisted(() => vi.fn());
// onData 处理器与可控的 write 完成回调:回放拦截测试要在「write 已入队、回调未触发」的
// 窗口里注入 xterm 自动应答,manual 模式把回调攒进队列由测试择机触发。
const dataHandler = vi.hoisted(() => ({ current: null as ((data: string) => void) | null }));
const writeCallbacks = vi.hoisted(() => ({ manual: false, queue: [] as (() => void)[] }));
// 终端选区：复制快捷键测试用（有选区的 Ctrl+C=复制不发 ^C，无选区照旧发）。
const selection = vi.hoisted(() => ({ text: "" }));
// 窗口焦点镜像（T-14 resize 仲裁）：jsdom 的 document.hasFocus() 恒 false，
// 不 mock 的话「失焦不下发 resize」的门会把所有用例的 resize 全挡掉。
const focusState = vi.hoisted(() => ({ current: true }));
// terminal.selectAll 的间谍（右键菜单「全选」用例）。
const selectAllSpy = vi.hoisted(() => vi.fn());
const terminalModes = vi.hoisted(() => ({ mouseTrackingMode: "none" as "none" | "x10" | "vt200" | "drag" | "any" }));
// 当前 buffer 类型:备用屏(alternate)没有 scrollback,Shift+滚轮旁路必须放行给 TUI。
const bufferType = vi.hoisted(() => ({ current: "normal" as "normal" | "alternate" }));
// terminal.paste 的间谍（右键粘贴测试断言剪贴板文本进了 xterm 粘贴通路）。
const pasteSpy = vi.hoisted(() => vi.fn());
// terminal.scrollToBottom 的间谍（退出提示写入后必须滚底，否则上翻视口里提示在屏外）。
const scrollToBottomSpy = vi.hoisted(() => vi.fn());
// Shift+滚轮旁路：自定义 wheel 处理器与 scrollLines 的间谍。
const wheelHandler = vi.hoisted(() => ({ current: null as ((event: WheelEvent) => boolean) | null }));
const scrollLinesSpy = vi.hoisted(() => vi.fn());
// terminal.resize 的间谍（隐藏态网格对齐：快照带的 PTY 尺寸要直接钉到网格上）。
const resizeGridSpy = vi.hoisted(() => vi.fn());
// 文件路径 link provider(T-4):捕获注册进来的 provider,测试直接驱动 provideLinks。
const linkProvider = vi.hoisted(() => ({ current: null as { provideLinks(y: number, cb: (links: { text: string; activate(e: MouseEvent, t: string): void; hover?(): void; leave?(): void }[] | undefined) => void): void } | null }));
// provider 读行文本用的正常屏 buffer:仅当 lineText 非 null 时暴露(缺席时组件按
// translateToString 拿不到行,回调 undefined——与真实 xterm 空行同路)。
const lineText = vi.hoisted(() => ({ current: null as string | null }));
vi.mock("@xterm/addon-web-links", () => ({
  WebLinksAddon: class {
    constructor(handler: (event: MouseEvent, uri: string) => void) { linkOpen.current = handler; }
  },
}));
vi.mock("@xterm/xterm", () => ({
  Terminal: class {
    constructor(options: Record<string, unknown>) { constructSpy(); termOptions.current = options; }
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
    attachCustomWheelEventHandler = (handler: (event: WheelEvent) => boolean) => { wheelHandler.current = handler; };
    scrollLines = (amount: number) => scrollLinesSpy(amount);
    // 只在备用屏用例下暴露 buffer:缺席时组件走 visibleTerminalText 兜底(与真实 xterm
    // 尚未就绪时同路),屏幕识别的那批用例依赖这条路径,给个空壳 buffer 会把它们打断。
    // lineText 非 null 时额外暴露带 getLine 的正常屏 buffer(文件路径 link provider 用)。
    get buffer() {
      if (bufferType.current === "alternate") return { active: { type: "alternate" } };
      if (lineText.current !== null) {
        const text = lineText.current;
        return { active: { type: "normal", getLine: () => ({ translateToString: () => text }) } };
      }
      return undefined;
    }
    // 文件路径 link provider 的注册口(T-4)。
    registerLinkProvider = (provider: NonNullable<typeof linkProvider.current>) => { linkProvider.current = provider; };
    hasSelection = () => selection.text.length > 0;
    // TUI 的鼠标上报模式(?1000-1006h 经 xterm 解析后暴露);测试按需改成 "any"。
    modes = terminalModes;
    getSelection = () => selection.text;
    clearSelection = () => { selection.text = ""; };
    selectAll = () => selectAllSpy();
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

import { findFakeCaret, gridResyncTarget, isCtrlLeftClickReport, isMouseMotionReport, ManagedTerminal, scanLineForFilePaths, STREAM_STALL_MS, stripTerminalReplies, terminalStreamStalled } from "./ManagedTerminal";

const noPty = { sessionId: 163, active: false, managed: false, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null, cols: 0, rows: 0, modes: [] as number[] };

/**
 * 可见期的网格自愈判定。resize 平时只由容器尺寸变化驱动：某次没落地（撞上后端那把有界
 * 锁——ResizePseudoConsole 在 conhost 僵死时永不返回，后来者快速失败；或会话正在重启），
 * 就没有「下一次」把它纠回来——用户不再动窗口，错位固化。TUI 按窄网格重绘、xterm 按宽
 * 网格显示，行右侧露出上一屏残字、同一块区域重画成好几份，而 TUI 画在最底下的那行输入框
 * 被挤出可视区（实拍反馈：「有时候会看不到终端的输入框」）。
 */
describe("gridResyncTarget", () => {
  const local = { cols: 213, rows: 44 };

  it("PTY 尺寸与本地网格不符时给出目标(以本地为准——可见期 fit 才是尺寸权威)", () => {
    expect(gridResyncTarget(local, { cols: 80, rows: 24 }, null, 1_000)).toEqual({ cols: 213, rows: 44 });
  });

  it("两边一致时不发:不平白给 TUI 一发 SIGWINCH 整屏重排", () => {
    expect(gridResyncTarget(local, { cols: 213, rows: 44 }, null, 1_000)).toBeNull();
  });

  it("PTY 尺寸未知(0/1)时不发:「不知道」不等于「不一致」", () => {
    expect(gridResyncTarget(local, { cols: 0, rows: 0 }, null, 1_000)).toBeNull();
    expect(gridResyncTarget(local, { cols: 213, rows: 1 }, null, 1_000)).toBeNull();
  });

  it("本地网格还没量出来(≤1)时不发", () => {
    expect(gridResyncTarget({ cols: 1, rows: 44 }, { cols: 80, rows: 24 }, null, 1_000)).toBeNull();
  });

  it("目标按 PTY 网格上限收:不收的话超限那一侧永远比不相等,每轮都在重发", () => {
    // 后端 pty::size 把两维都 clamp 到 500，PTY 停在 500 就已经是它能给的最大值了。
    expect(gridResyncTarget({ cols: 900, rows: 44 }, { cols: 500, rows: 44 }, null, 1_000)).toBeNull();
    expect(gridResyncTarget({ cols: 900, rows: 44 }, { cols: 480, rows: 44 }, null, 1_000))
      .toEqual({ cols: 500, rows: 44 });
  });

  it("同一目标刚试过就先不重发,过了重试间隔再试(失败是暂时的,要重试)", () => {
    const last = { cols: 213, rows: 44, at: 1_000 };
    expect(gridResyncTarget(local, { cols: 80, rows: 24 }, last, 3_000)).toBeNull();
    expect(gridResyncTarget(local, { cols: 80, rows: 24 }, last, 9_000)).toEqual({ cols: 213, rows: 44 });
  });

  it("目标换了就立刻发,不受上一次的重试间隔挡着(窗口刚改过大小)", () => {
    const last = { cols: 100, rows: 30, at: 1_000 };
    expect(gridResyncTarget(local, { cols: 80, rows: 24 }, last, 1_100)).toEqual({ cols: 213, rows: 44 });
  });
});

/// 文件路径识别(T-4):终端里最高频的可点内容。口径保守——裸相对路径必须有一段含「.」,
/// URL 的部分不收(归 WebLinksAddon)。
describe("scanLineForFilePaths", () => {
  const texts = (line: string) => scanLineForFilePaths(line).map((h) => h.text);

  it("收带分隔符的相对路径与各类锚头路径", () => {
    expect(texts("read src/main.ts please")).toEqual(["src/main.ts"]);
    expect(texts("./a/b/c")).toEqual(["./a/b/c"]);
    expect(texts("../up/file")).toEqual(["../up/file"]); // 无点也收:锚头即意图
    expect(texts("~/docs/note.md")).toEqual(["~/docs/note.md"]);
    expect(texts("/var/log/app.log")).toEqual(["/var/log/app.log"]);
    expect(texts(String.raw`C:\Users\foo\bar.txt`)).toEqual([String.raw`C:\Users\foo\bar.txt`]);
  });

  it("连词与无点裸路径不收:误链普通单词的代价不是零", () => {
    expect(texts("and/or this/that")).toEqual([]);
    expect(texts("no slash here")).toEqual([]);
  });

  it("URL 的部分不收:那是 WebLinksAddon 的地盘", () => {
    expect(texts("see https://example.com/a/b for details")).toEqual([]);
  });

  it("句读剥尾:括号/句号/行号不进命中", () => {
    expect(texts("(src/main.ts).")).toEqual(["src/main.ts"]);
    expect(texts("open src/main.ts:12 now")).toEqual(["src/main.ts"]);
    expect(texts("a/b.ts, c/d.ts")).toEqual(["a/b.ts", "c/d.ts"]);
  });

  it("命中位置是开区间下标(供 link range 映射)", () => {
    const [hit] = scanLineForFilePaths("xx src/main.ts yy");
    expect(hit).toEqual({ text: "src/main.ts", start: 3, end: 14 });
  });
});

describe("ManagedTerminal", () => {
  afterEach(cleanup);
  beforeEach(() => {
    invoke.mockReset();
    write.mockReset();
    resetSpy.mockReset();
    constructSpy.mockReset();
    confirmAnswer.ok = true;
    eventHandlers.clear();
    dataHandler.current = null;
    writeCallbacks.manual = false;
    writeCallbacks.queue = [];
    selection.text = "";
    focusState.current = true;
    document.hasFocus = () => focusState.current;
    selectAllSpy.mockReset();
    scrollToBottomSpy.mockReset();
    resizeGridSpy.mockReset();
    wheelHandler.current = null;
    scrollLinesSpy.mockReset();
    bufferType.current = "normal";
    linkProvider.current = null;
    lineText.current = null;
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
    searchFindPrevious.mockReturnValue(false);
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
    // 两头都落空才是真正的无匹配:Enter(非 incremental)的回绕尝试也不能把它救活。
    fireEvent.keyDown(input, { key: "Enter" });
    expect(await screen.findByText("无匹配")).toBeTruthy();
  });

  it("终端搜索到底后按 Enter 自动回绕并提示「已回绕」,与无匹配可区分", async () => {
    // findNext 落空、findPrevious 命中 = 尾部之上确有匹配,只是光标已在最后一条之后。
    searchFindNext.mockReturnValue(false);
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") return Promise.resolve(noPty);
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" />);
    await waitFor(() => expect(keyHandler.current).toBeTruthy());
    const key = (init: Partial<KeyboardEvent> & { type: string; code: string }) => init as KeyboardEvent;
    keyHandler.current!(key({ type: "keydown", code: "KeyF", ctrlKey: true }));
    const input = await screen.findByPlaceholderText("搜索终端输出");
    // 逐字期(incremental)不回绕:输入中的落空大概率是「还没打完」,回绕跳动反而吓人。
    fireEvent.change(input, { target: { value: "error" } });
    expect(await screen.findByText("无匹配")).toBeTruthy();
    // Enter(非 incremental)落空 → 自动反向找一次,命中则提示「已回绕」而不是「无匹配」。
    fireEvent.keyDown(input, { key: "Enter" });
    expect(await screen.findByText("已到末尾，回绕到第一个匹配")).toBeTruthy();
    expect(screen.queryByText("无匹配")).toBeNull();
    expect(searchFindPrevious).toHaveBeenCalledWith("error");
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

  it("悬停链接时右下角出 Ctrl+点击 提示,离开即收(T-4:此前这条终端惯例零界面表达)", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") return Promise.resolve(noPty);
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" />);
    await waitFor(() => expect(linkOpen.current).toBeTruthy());
    const handler = termOptions.current?.linkHandler as { hover?: () => void; leave?: () => void };
    expect(handler.hover).toBeTruthy();
    act(() => handler.hover!());
    expect(await screen.findByText("Ctrl+点击打开链接")).toBeTruthy();
    act(() => handler.leave!());
    await waitFor(() => expect(screen.queryByText("Ctrl+点击打开链接")).toBeNull());
  });

  it("文件路径 link provider:Ctrl+点击走文件管理器定位,普通点击不动;悬停有专属提示", async () => {
    lineText.current = "read src/main.ts and https://example.com/a/b";
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") return Promise.resolve(noPty);
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" cwd="/repo" />);
    await waitFor(() => expect(linkProvider.current).toBeTruthy());
    let links: { text: string; activate(e: MouseEvent, t: string): void; hover?(): void; leave?(): void }[] | undefined;
    linkProvider.current!.provideLinks(1, (l) => { links = l; });
    // URL 的部分不出链(归 WebLinksAddon),行内只有文件路径一个命中。
    expect(links?.map((l) => l.text)).toEqual(["src/main.ts"]);
    const click = (init: Partial<MouseEvent>) => init as MouseEvent;
    // 普通点击留给 TUI 鼠标交互与选区(与 URL 同一纪律)。
    links![0].activate(click({}), "src/main.ts");
    expect(invoke).not.toHaveBeenCalledWith("reveal_path_in_file_manager", expect.anything());
    links![0].activate(click({ ctrlKey: true }), "src/main.ts");
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("reveal_path_in_file_manager", { cwd: "/repo", rel: "src/main.ts" }));
    // 悬停提示指向文件管理器,离开即收。
    act(() => links![0].hover!());
    expect(await screen.findByText("Ctrl+点击在文件管理器中显示")).toBeTruthy();
    act(() => links![0].leave!());
    await waitFor(() => expect(screen.queryByText("Ctrl+点击在文件管理器中显示")).toBeNull());
  });

  it("cwd 未知时文件路径 provider 不出链:点了也只会报「目录不存在」", async () => {
    lineText.current = "read src/main.ts";
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") return Promise.resolve(noPty);
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" />);
    await waitFor(() => expect(linkProvider.current).toBeTruthy());
    let links: unknown;
    linkProvider.current!.provideLinks(1, (l) => { links = l; });
    expect(links).toBeUndefined();
  });

  it("假死横幅给「结束并恢复」(T-5):确认后先 stop、等本会话 pty-exit 再 start,一步走完此前两步", async () => {
    // 停滞判定靠 30s 阈值 + 5s 节拍,fake timers(含 Date)把时间直接推过去。
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    try {
      invoke.mockImplementation((command: string) => {
        if (command === "managed_terminal_snapshot") return Promise.resolve({ ...noPty, active: true });
        if (command === "confirm_dialog") return Promise.resolve(confirmAnswer.ok);
        return Promise.resolve();
      });
      render(<ManagedTerminal sessionId={163} status="running" />);
      // 挂载 effect 与首轮快照落地(纯微任务,不等计时器):active 翻 true。
      await act(async () => {});
      // 推过 30s 停滞阈值,5s 节拍至少跑一轮 → 假死横幅出现。
      await act(async () => { vi.advanceTimersByTime(36_000); });
      const button = screen.queryByRole("button", { name: "结束并恢复" });
      expect(button).toBeTruthy();
      invoke.mockClear();
      fireEvent.click(button!);
      // confirm → stop 完成;但 start 必须等本会话的 pty-exit:broker.stop 只发 kill 就返回,
      // 真正收尾在 waiter 升级链,此刻立即 start 会撞在将死的旧 PTY 上(外部占用误报 /
      // begin_start 判重被吞,3s 后 pty-exit 到达再弹一次退出封面)。
      await act(async () => {});
      const calls = invoke.mock.calls.map((c) => c[0]);
      expect(calls).toContain("confirm_dialog");
      expect(calls).toContain("stop_managed_terminal");
      expect(calls).not.toContain("start_managed_terminal");
      // pty-exit 到达(旧进程真正退出) → 这才 start。
      await act(async () => {
        eventHandlers.get("pty-exit")!({ payload: { sessionId: 163, code: null } });
      });
      const after = invoke.mock.calls.map((c) => c[0]);
      expect(after).toContain("start_managed_terminal");
      expect(after.indexOf("stop_managed_terminal")).toBeLessThan(after.indexOf("start_managed_terminal"));
    } finally {
      vi.useRealTimers();
    }
  });

  it("「结束并恢复」先挂监听再发 stop:pty-exit 抢在 stop 返回前到达也照接,不白等 4s", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    try {
      invoke.mockImplementation((command: string) => {
        if (command === "managed_terminal_snapshot") return Promise.resolve({ ...noPty, active: true });
        if (command === "confirm_dialog") return Promise.resolve(confirmAnswer.ok);
        if (command === "stop_managed_terminal") {
          // kill 一发进程就死:pty-exit 抢在 stop 的 Promise resolve 之前到达。监听已在
          // 发起 stop 前注册完成(await listen 后才 confirmStopSession),这发事件必须被
          // 接住;旧顺序(先 stop 后挂监听)会漏掉它,只能白等 4s 超时兜底。
          eventHandlers.get("pty-exit")!({ payload: { sessionId: 163, code: null } });
          return Promise.resolve();
        }
        return Promise.resolve();
      });
      render(<ManagedTerminal sessionId={163} status="running" />);
      await act(async () => {});
      await act(async () => { vi.advanceTimersByTime(36_000); });
      const button = screen.queryByRole("button", { name: "结束并恢复" });
      expect(button).toBeTruthy();
      invoke.mockClear();
      fireEvent.click(button!);
      // 不推计时器:纯事件驱动,start 应当在微任务里直接发生——等 4s 兜底才算输。
      await act(async () => {});
      expect(invoke.mock.calls.map((c) => c[0])).toContain("start_managed_terminal");
    } finally {
      vi.useRealTimers();
    }
  });

  it("「结束并恢复」等 pty-exit 有超时兜底:事件真丢了也照 start,不把用户钉死在横幅上", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    try {
      invoke.mockImplementation((command: string) => {
        if (command === "managed_terminal_snapshot") return Promise.resolve({ ...noPty, active: true });
        if (command === "confirm_dialog") return Promise.resolve(confirmAnswer.ok);
        return Promise.resolve();
      });
      render(<ManagedTerminal sessionId={163} status="running" />);
      await act(async () => {});
      await act(async () => { vi.advanceTimersByTime(36_000); });
      const button = screen.queryByRole("button", { name: "结束并恢复" });
      expect(button).toBeTruthy();
      invoke.mockClear();
      fireEvent.click(button!);
      await act(async () => {});
      expect(invoke.mock.calls.map((c) => c[0])).toContain("stop_managed_terminal");
      expect(invoke.mock.calls.map((c) => c[0])).not.toContain("start_managed_terminal");
      // pty-exit 始终不到达:4s 兜底(升级链末档 3s + 1s 余量)后仍 start 一次。
      await act(async () => { vi.advanceTimersByTime(4_100); });
      expect(invoke.mock.calls.map((c) => c[0])).toContain("start_managed_terminal");
    } finally {
      vi.useRealTimers();
    }
  });

  it("Ctrl+左键的鼠标上报不下发 PTY：宿主已开链接，TUI 再收到点击会对同一链接开第二次", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") return Promise.resolve(noPty);
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" />);
    await waitFor(() => expect(dataHandler.current).toBeTruthy());
    // Ctrl+左键按下/抬起(Cb=16)整对吞掉。
    dataHandler.current!("\x1b[<16;5;6M");
    dataHandler.current!("\x1b[<16;5;6m");
    expect(invoke.mock.calls.some(([command]) => command === "write_managed_terminal")).toBe(false);
    // 普通左键(Cb=0)照常转发——TUI 自己的点击语义不受牵连。
    dataHandler.current!("\x1b[<0;5;6M");
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 163, data: "\x1b[<0;5;6M" }));
    // 判别式本体:滚轮(64+16=80)与右键(2+16=18)带 Ctrl 都不在此列。
    expect(isCtrlLeftClickReport("\x1b[<16;1;1M")).toBe(true);
    expect(isCtrlLeftClickReport("\x1b[<16;1;1m")).toBe(true);
    expect(isCtrlLeftClickReport("\x1b[<80;1;1M")).toBe(false);
    expect(isCtrlLeftClickReport("\x1b[<18;1;1M")).toBe(false);
    expect(isCtrlLeftClickReport("\x1b[<0;1;1M")).toBe(false);
  });

  it("Shift+滚轮走本地滚动旁路，普通滚轮交还 xterm", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") return Promise.resolve(noPty);
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" />);
    await waitFor(() => expect(wheelHandler.current).toBeTruthy());
    const wheel = (init: Partial<WheelEvent>) => ({ preventDefault: vi.fn(), ...init }) as unknown as WheelEvent;
    // 普通滚轮:返回 true 交还 xterm 默认路径(视口滚动或鼠标上报,按模式定)。
    expect(wheelHandler.current!(wheel({ deltaY: 100 }))).toBe(true);
    expect(scrollLinesSpy).not.toHaveBeenCalled();
    // Shift+滚轮:本地滚动,返回 false 让 xterm 完全不处理(不上报 TUI)。
    expect(wheelHandler.current!(wheel({ shiftKey: true, deltaY: 100 }))).toBe(false);
    expect(scrollLinesSpy).toHaveBeenCalledWith(3);
    expect(wheelHandler.current!(wheel({ shiftKey: true, deltaY: -100 }))).toBe(false);
    expect(scrollLinesSpy).toHaveBeenCalledWith(-3);
    // 行制 deltaMode 的小数值靠下限保底,至少滚一行。
    expect(wheelHandler.current!(wheel({ shiftKey: true, deltaY: 3 }))).toBe(false);
    expect(scrollLinesSpy).toHaveBeenCalledWith(1);
    // 有环境按住 Shift 时把纵向滚轮改派成 deltaX;两轴都取,免得旁路一次都不触发。
    scrollLinesSpy.mockReset();
    expect(wheelHandler.current!(wheel({ shiftKey: true, deltaY: 0, deltaX: 100 }))).toBe(false);
    expect(scrollLinesSpy).toHaveBeenCalledWith(3);
  });

  it("备用屏里 Shift+滚轮放行给 TUI:那里没有 scrollback,吞掉只会变成按了没反应", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") return Promise.resolve(noPty);
      return Promise.resolve();
    });
    // claude 全屏渲染器(?1049h)就是这种情形:xterm 的备用屏 buffer 以 hasScrollback=false
    // 构造,scrollLines 位移恒为 0——本地根本没有可翻的历史。
    bufferType.current = "alternate";
    render(<ManagedTerminal sessionId={163} status="running" />);
    await waitFor(() => expect(wheelHandler.current).toBeTruthy());
    const wheel = (init: Partial<WheelEvent>) => ({ preventDefault: vi.fn(), ...init }) as unknown as WheelEvent;
    expect(wheelHandler.current!(wheel({ shiftKey: true, deltaY: 100 }))).toBe(true);
    expect(scrollLinesSpy).not.toHaveBeenCalled();
  });

  it("退出即收回鼠标上报模式：崩溃时模式来不及关，滚轮/选区在尸体上永远失灵", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") return Promise.resolve({ ...noPty, active: true, data: btoa("hi"), endOffset: 2 });
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" />);
    await waitFor(() => expect(eventHandlers.get("pty-exit")).toBeTruthy());
    await waitFor(() => expect(write).toHaveBeenCalled());
    eventHandlers.get("pty-exit")!({ payload: { sessionId: 163, code: 1 } });
    await waitFor(() => expect(write.mock.calls.some(([data]) => typeof data === "string" && data.includes("\x1b[?1003l"))).toBe(true));
    // 重开窗口回放已退出会话:基线补写的鼠标模式同样要被收回(写序在回放数据之后)。
    cleanup();
    write.mockReset();
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve({ ...noPty, exited: true, exitCode: 1, data: btoa("tail"), startOffset: 5000, endOffset: 5004, modes: [1049, 1003, 1006] });
      }
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" />);
    await waitFor(() => expect(write.mock.calls.some(([data]) => typeof data === "string" && data.includes("\x1b[?1003l"))).toBe(true));
    const calls = write.mock.calls.map(([data]) => data).filter((data): data is string => typeof data === "string");
    const baselineAt = calls.findIndex((data) => data.includes("\x1b[?1003h"));
    const offAt = calls.findIndex((data) => data.includes("\x1b[?1003l"));
    expect(baselineAt).toBeGreaterThanOrEqual(0);
    expect(offAt).toBeGreaterThan(baselineAt);
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

  /// 后台会话接上旁路后 active=true，但它并未被 Meowo 托管——attach_in_external_terminal
  /// 的 ensure_attachable 必然报「该会话尚未由 Meowo 接管」。挂着必错的按钮是误导。
  it("后台会话接上旁路后不出「在外部终端同步打开」按钮", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") return Promise.resolve({ ...noPty, active: true, data: btoa("hi"), endOffset: 2 });
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" background />);
    await waitFor(() => expect(write).toHaveBeenCalled());
    expect(screen.queryByRole("button", { name: "在外部终端同步打开" })).toBeNull();
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

  /// 强制收尾（kill 静默无效后的末档）会把会话摘除但进程可能仍在：文案必须与正常
  /// 退出区分，不能一律说成「已退出」（zombie 残留时那是谎报）。
  it("forced 的 pty-exit 显示强制结束文案，与正常退出区分", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve({ ...noPty, active: true, data: btoa("running"), endOffset: 7 });
      }
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" />);
    await waitFor(() => expect(eventHandlers.get("pty-exit")).toBeTruthy());
    eventHandlers.get("pty-exit")!({ payload: { sessionId: 163, code: null, forced: true } });
    expect(await screen.findByText(/已强制结束/)).toBeTruthy();
    expect(screen.queryByText(/Agent 进程已退出/)).toBeNull();
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
    // 退出提示行走 i18n（zh 基准字典），保持宿主注解形态。
    expect(write.mock.calls.some(([data]) => typeof data === "string" && data.includes("[Meowo：进程已退出（0）]"))).toBe(true);
  });

  it("正常退出（码 0）卡片用中性描边（is-clean）且文案不标退出码，与异常退出区分", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve({ ...noPty, exited: true, exitCode: 0 });
      }
      return Promise.resolve();
    });
    const { container } = render(<ManagedTerminal sessionId={163} status="ended" />);
    await waitFor(() => expect(container.querySelector(".managed-terminal-exit-card.is-clean")).toBeTruthy());
    // 码 0 是常态不是异常：不再标「（退出码 0）」。
    expect(screen.getByText(/Agent 进程已退出，上方保留了终端输出/)).toBeTruthy();
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

  it("右键菜单(U0-11):有选区复制可用(复制并清选区),无选区粘贴可用(读剪贴板走 paste 通路)", async () => {
    // 生产构建的 WebView 默认右键菜单被 devtools-guard 封死,应用内补一份
    // (复制/粘贴/全选/搜索)。粘贴只在无选区时可用——有选区的右键是复制语义。
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
    const menuItems = () => Array.from(document.querySelectorAll<HTMLButtonElement>(".ctx-menu .ctx-item"));
    // 有选区:弹菜单,复制可用、粘贴置灰;点「复制」写剪贴板、清选区、菜单关闭,不碰剪贴板读取。
    selection.text = "picked";
    fireEvent.contextMenu(host);
    expect(menuItems().length).toBe(4);
    expect(menuItems()[0].disabled).toBe(false); // 复制
    expect(menuItems()[1].disabled).toBe(true); // 粘贴(有选区=复制语义)
    fireEvent.click(menuItems()[0]);
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("picked"));
    expect(selection.text).toBe("");
    expect(invoke).not.toHaveBeenCalledWith("clipboard_text");
    expect(document.querySelector(".ctx-menu")).toBeNull();
    // 无选区:复制置灰、粘贴可用;点「粘贴」读后端剪贴板,文本进 xterm 的 paste 通路
    // (bracketed paste + onData 下发)。
    fireEvent.contextMenu(host);
    expect(menuItems()[0].disabled).toBe(true);
    expect(menuItems()[1].disabled).toBe(false);
    fireEvent.click(menuItems()[1]);
    await waitFor(() => expect(pasteSpy).toHaveBeenCalledWith("from-clipboard"));
    expect(document.querySelector(".ctx-menu")).toBeNull();
  });

  it("右键粘贴:剪贴板无文本(如截图位图)时回退发 ^V,读剪贴板失败要可见(P2-4)", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") return Promise.resolve({ ...noPty, active: true, data: btoa("hi"), endOffset: 2 });
      if (command === "clipboard_text") return Promise.resolve(null);
      return Promise.resolve();
    });
    pasteSpy.mockClear();
    render(<ManagedTerminal sessionId={163} status="running" />);
    const host = document.querySelector(".managed-terminal-host")!;
    await screen.findByRole("button", { name: "在外部终端同步打开" });
    const menuItems = () => Array.from(document.querySelectorAll<HTMLButtonElement>(".ctx-menu .ctx-item"));
    // 无文本可粘:不发空 paste,回退 ^V 让 TUI 自己读系统剪贴板(claude 贴图通路,
    // 与键盘路径的 pasteImageFallback 同语义)。
    fireEvent.contextMenu(host);
    fireEvent.click(menuItems()[1]);
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("write_managed_terminal", { sessionId: 163, data: "\x16" }));
    expect(pasteSpy).not.toHaveBeenCalled();
  });

  it("右键菜单(U0-11):全选走 terminal.selectAll,搜索打开终端内搜索条", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") return Promise.resolve({ ...noPty, active: true, data: btoa("hi"), endOffset: 2 });
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" />);
    const host = document.querySelector(".managed-terminal-host")!;
    const menuItems = () => Array.from(document.querySelectorAll<HTMLButtonElement>(".ctx-menu .ctx-item"));
    // 全选:选区由 xterm 自己管,组件只转发 selectAll。
    fireEvent.contextMenu(host);
    fireEvent.click(menuItems()[2]);
    expect(selectAllSpy).toHaveBeenCalledTimes(1);
    expect(document.querySelector(".ctx-menu")).toBeNull();
    // 搜索:打开与 Ctrl+F 同一条搜索条。
    fireEvent.contextMenu(host);
    fireEvent.click(menuItems()[3]);
    await waitFor(() => expect(document.querySelector(".term-search-input")).toBeTruthy());
  });

  /**
   * claude 全屏渲染器开鼠标上报后,xterm 把右键转发给它、它自己粘贴剪贴板(实测)。
   * Meowo 的右键必须让位:无选区不叠菜单不粘贴;有选区仍弹复制菜单,但 mousedown 在
   * 捕获段拦在 xterm 之外——TUI 不知道有过右键就不会顺手粘贴(实拍「右键复制,同时
   * 就粘贴进输入框」)。
   */
  it("TUI 开鼠标上报时:无选区右键不弹菜单不粘贴,有选区弹复制菜单且按键不进 xterm", async () => {
    Object.defineProperty(navigator, "platform", { value: "Win32", configurable: true });
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") return Promise.resolve({ ...noPty, active: true, data: btoa("hi"), endOffset: 2 });
      if (command === "clipboard_text") return Promise.resolve("from-clipboard");
      return Promise.resolve();
    });
    const writeText = vi.fn(() => Promise.resolve());
    Object.assign(navigator, { clipboard: { writeText } });
    pasteSpy.mockClear();
    terminalModes.mouseTrackingMode = "any";
    try {
      render(<ManagedTerminal sessionId={163} status="running" />);
      const host = document.querySelector(".managed-terminal-host")!;
      // xterm 自己的监听挂在 host 的子元素上;用一个子元素代替,验证捕获段拦截。
      const xtermEl = document.createElement("div");
      host.appendChild(xtermEl);
      const reachedXterm = vi.fn();
      xtermEl.addEventListener("mousedown", reachedXterm);
      xtermEl.addEventListener("mouseup", reachedXterm);
      // 无选区:右键归 TUI,Meowo 不弹菜单、不读剪贴板、不 paste。
      selection.text = "";
      fireEvent.mouseDown(xtermEl, { button: 2 });
      fireEvent.mouseUp(xtermEl, { button: 2 });
      fireEvent.contextMenu(host);
      await new Promise((resolve) => setTimeout(resolve, 50));
      expect(document.querySelector(".ctx-menu")).toBeNull();
      expect(pasteSpy).not.toHaveBeenCalled();
      expect(invoke).not.toHaveBeenCalledWith("clipboard_text");
      expect(reachedXterm).toHaveBeenCalledTimes(2);
      reachedXterm.mockClear();
      // 有选区:弹菜单(粘贴置灰),且这次按下/抬起都不到 xterm(TUI 看不见右键,不会粘贴)。
      selection.text = "picked";
      fireEvent.mouseDown(xtermEl, { button: 2 });
      fireEvent.mouseUp(xtermEl, { button: 2 });
      fireEvent.contextMenu(host);
      const items = Array.from(document.querySelectorAll<HTMLButtonElement>(".ctx-menu .ctx-item"));
      expect(items.length).toBe(4);
      expect(items[1].disabled).toBe(true); // 有选区:粘贴置灰
      expect(reachedXterm).not.toHaveBeenCalled();
      // 点「复制」才写剪贴板并清选区。
      fireEvent.click(items[0]);
      await waitFor(() => expect(writeText).toHaveBeenCalledWith("picked"));
      expect(selection.text).toBe("");
      // 左键照常放行。
      fireEvent.mouseDown(xtermEl, { button: 0 });
      expect(reachedXterm).toHaveBeenCalledTimes(1);
    } finally {
      terminalModes.mouseTrackingMode = "none";
    }
  });

  /**
   * T-14 resize 仲裁：对话窗与外部同步终端 attach 同一 PTY 时，两边各按自己的容器
   * 尺寸下发 resize，TUI 被来回重排。约定聚焦视图为尺寸主控——失焦期本组件一律
   * 静默（设置热应用驱动的 refit 也不发）；重新聚焦时只有查到网格真的漂移
   * （失焦期间被外部终端改过尺寸）才补发一次夺回主控，无漂移不多吃 SIGWINCH。
   */
  it("T-14 resize 仲裁:失焦期不下发 resize,重新聚焦时网格漂移才补发", async () => {
    const snapshot = { ...noPty, active: true, cols: 80, rows: 24, data: btoa("hi"), endOffset: 2 };
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") return Promise.resolve(snapshot);
      if (command === "managed_terminal_grid") return Promise.resolve([80, 24]);
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" />);
    // 聚焦态:可见 effect 的 fit 后正常下发一次 resize(80×24)。
    await waitFor(() => expect(invoke.mock.calls.some(([c]) => c === "resize_managed_terminal")).toBe(true));
    invoke.mockClear();
    // 失焦:设置热应用驱动的 refit 也不再下发(此刻外部终端可能是尺寸主控)。
    focusState.current = false;
    fireEvent(window, new Event("blur"));
    eventHandlers.get("settings-changed")!({ payload: { terminal_font_size: 14 } });
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(invoke.mock.calls.some(([c]) => c === "resize_managed_terminal")).toBe(false);
    // 重新聚焦但网格无漂移(PTY 仍是本地尺寸):不补发。
    focusState.current = true;
    fireEvent(window, new Event("focus"));
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(invoke.mock.calls.some(([c]) => c === "resize_managed_terminal")).toBe(false);
    // 聚焦时查到漂移(失焦期间 PTY 被改成 100×30):补发一次本地尺寸,夺回主控。
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") return Promise.resolve(snapshot);
      if (command === "managed_terminal_grid") return Promise.resolve([100, 30]);
      return Promise.resolve();
    });
    fireEvent(window, new Event("blur"));
    fireEvent(window, new Event("focus"));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("resize_managed_terminal", { sessionId: 163, cols: 80, rows: 24 }));
  });

  /**
   * 外部视图离线即夺回网格（实拍回归：外部同步终端关掉后，PTY 停在它的小网格，
   * 对话窗终端只显示上半截；原设计只在「重新聚焦」时夺回，用户盯着窗口不点击就
   * 一直错着，拖到窗口触发激活才复原）。仲裁对象没了就不需要等聚焦。
   */
  it("外部视图离线立即夺回网格,不等重新聚焦", async () => {
    const snapshot = { ...noPty, active: true, cols: 100, rows: 30, externalViewers: true, data: btoa("hi"), endOffset: 2 };
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") return Promise.resolve(snapshot);
      if (command === "managed_terminal_grid") return Promise.resolve([100, 30]);
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" />);
    // 挂载时可见 effect 补发一次（本窗聚焦，无仲裁冲突）。
    await waitFor(() => expect(invoke.mock.calls.some(([c]) => c === "resize_managed_terminal")).toBe(true));
    invoke.mockClear();
    // 失焦 + 外部视图在线：仲裁期一切让路（外部终端是尺寸主控）。
    focusState.current = false;
    fireEvent(window, new Event("blur"));
    // 外部视图离线：立刻按本地网格（哑终端 80×24）夺回，不等重新聚焦。
    eventHandlers.get("pty-external-viewers")!({ payload: { sessionId: 163, online: false } });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("resize_managed_terminal", { sessionId: 163, cols: 80, rows: 24 }));
    // 别的会话的离线事件不触发本组件。
    invoke.mockClear();
    eventHandlers.get("pty-external-viewers")!({ payload: { sessionId: 999, online: false } });
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(invoke.mock.calls.some(([c]) => c === "resize_managed_terminal")).toBe(false);
  });

  /**
   * 切回终端页无条件补发 resize（64ab995 的语义，U1-16 的同值短路曾把它弱化）：
   * PTY 可能已被别的通路改小（外部握手/手机查看后异常退出/占位网格重启），缓存同值
   * 跳过就永远收不回；后端 last_size 同值短路兜底，常规路径不多吃 SIGWINCH。
   */
  it("切回终端页无条件补发 resize,同值缓存不挡自愈", async () => {
    const snapshot = { ...noPty, active: true, cols: 80, rows: 24, data: btoa("hi"), endOffset: 2 };
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") return Promise.resolve(snapshot);
      if (command === "managed_terminal_grid") return Promise.resolve([80, 24]);
      return Promise.resolve();
    });
    const { rerender } = render(<ManagedTerminal sessionId={163} status="running" visible />);
    await waitFor(() => expect(invoke.mock.calls.some(([c]) => c === "resize_managed_terminal")).toBe(true));
    invoke.mockClear();
    // 切走再切回：fit 出的尺寸与缓存同值——旧实现被 resizeIfChanged 短路掉，
    // 现实现必须照样补发。
    rerender(<ManagedTerminal sessionId={163} status="running" visible={false} />);
    rerender(<ManagedTerminal sessionId={163} status="running" visible />);
    await waitFor(() => expect(invoke.mock.calls.some(([c]) => c === "resize_managed_terminal")).toBe(true));
  });

  it("isMouseMotionReport 只认无按键的 SGR 移动事件", () => {
    expect(isMouseMotionReport("\x1b[<35;10;20M")).toBe(true);
    expect(isMouseMotionReport("\x1b[<39;10;20M")).toBe(true); // shift+移动
    expect(isMouseMotionReport("\x1b[<0;10;20M")).toBe(false); // 左键按下
    expect(isMouseMotionReport("\x1b[<32;10;20M")).toBe(false); // 左键拖动
    expect(isMouseMotionReport("\x1b[<64;10;20M")).toBe(false); // 滚轮
    expect(isMouseMotionReport("\x1b[<35;10;20m")).toBe(false); // 抬起
    expect(isMouseMotionReport("\x1b[<35;10;20Ma")).toBe(false); // 混着按键
    expect(isMouseMotionReport("a")).toBe(false);
  });

  /**
   * `?1003h` 下 xterm 每跨一格发一次移动上报,快速划过每秒上百次,每次一趟 IPC——
   * debug 构建下按键排在后面(实拍「终端变得很卡」)。移动是位置采样,只留最新一条;
   * 按键不等:先冲积压的移动(顺序),再即时下发。
   */
  it("鼠标移动上报按帧合并只发最新一条,按键即时下发且排在其后", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") return Promise.resolve({ ...noPty, active: true, data: btoa("hi"), endOffset: 2 });
      return Promise.resolve();
    });
    render(<ManagedTerminal sessionId={163} status="running" />);
    await waitFor(() => expect(dataHandler.current).toBeTruthy());
    invoke.mockClear();
    const writes = () => invoke.mock.calls.filter(([command]) => command === "write_managed_terminal").map(([, args]) => (args as { data: string }).data);
    // 一帧内 50 条移动:IPC 只有一趟,且是最后的坐标。
    for (let i = 1; i <= 50; i += 1) dataHandler.current!(`\x1b[<35;${i};7M`);
    expect(writes()).toEqual([]);
    await waitFor(() => expect(writes()).toEqual(["\x1b[<35;50;7M"]));
    // 积压着移动时来了按键:移动先冲出去、按键紧随,都不等定时器。
    dataHandler.current!("\x1b[<35;51;7M");
    dataHandler.current!("a");
    expect(writes()).toEqual(["\x1b[<35;50;7M", "\x1b[<35;51;7M", "a"]);
    // 后续无移动积压:定时器到点不再多发。
    await new Promise((resolve) => setTimeout(resolve, 40));
    expect(writes()).toEqual(["\x1b[<35;50;7M", "\x1b[<35;51;7M", "a"]);
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

  /**
   * T-13：临时 id → 真实 id 的 claim 重绑不再换 key 整只重挂（xterm 重建、首屏清空重画）。
   * 后端 try_claim_rebind 只换映射键，同一 PTY、偏移连续——组件用 managed_terminal_binding
   * 权威确认后只补一次增量快照：不 reset、不重建 Terminal，viewer 注册换成真实 id。
   */
  it("临时 id → 真实 id 重绑:终端不重建不 reset,viewer 注册换成真实 id", async () => {
    invoke.mockImplementation((command: string, args?: { sessionId?: number }) => {
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve({ ...noPty, sessionId: args?.sessionId ?? -3, active: true, managed: true });
      }
      if (command === "managed_terminal_binding") return Promise.resolve(163);
      return Promise.resolve();
    });
    const { rerender } = render(<ManagedTerminal sessionId={-3} status="running" />);
    await waitFor(() => expect(dataHandler.current).toBeTruthy());
    constructSpy.mockClear();
    resetSpy.mockClear();
    rerender(<ManagedTerminal sessionId={163} status="running" />);
    // viewer 注册跟着 id 走(emitter 只对 viewed_session 推实时帧)。
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("register_terminal_viewer", { sessionId: 163 }));
    // 同一 PTY 只换名:确认绑定后补一次增量快照,画面不动。
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("managed_terminal_snapshot", expect.objectContaining({ sessionId: 163 })));
    expect(resetSpy).not.toHaveBeenCalled();
    expect(constructSpy).not.toHaveBeenCalled();
  });

  it("负 id 期间切去别的会话(binding 对不上):复用终端实例但按换会话复位", async () => {
    invoke.mockImplementation((command: string, args?: { sessionId?: number }) => {
      if (command === "managed_terminal_snapshot") {
        return Promise.resolve({ ...noPty, sessionId: args?.sessionId ?? -3, active: true, managed: true });
      }
      // 用户跳转目标的 163 不是这个临时 id 认领出的会话(它认出了 999)——不是重绑。
      if (command === "managed_terminal_binding") return Promise.resolve(999);
      return Promise.resolve();
    });
    const { rerender } = render(<ManagedTerminal sessionId={-3} status="running" />);
    await waitFor(() => expect(dataHandler.current).toBeTruthy());
    constructSpy.mockClear();
    resetSpy.mockClear();
    rerender(<ManagedTerminal sessionId={163} status="running" />);
    // 异会话:rearm 归零偏移 + terminal.reset 清掉旧画面,但 xterm 实例复用。
    await waitFor(() => expect(resetSpy).toHaveBeenCalled());
    expect(constructSpy).not.toHaveBeenCalled();
  });

  it("gridRef 暴露当前 xterm 网格,供对话页恢复/接管作 PTY 初始尺寸(T-9)", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "managed_terminal_snapshot") return Promise.resolve(noPty);
      return Promise.resolve();
    });
    const gridRef = { current: null as (() => { cols: number; rows: number } | null) | null };
    render(<ManagedTerminal sessionId={163} status="running" gridRef={gridRef} />);
    await waitFor(() => expect(gridRef.current).toBeTruthy());
    // 哑终端的默认网格;生产上是 fit 后(或隐藏期快照对齐后)的真实 cols/rows。
    expect(gridRef.current!()).toEqual({ cols: 80, rows: 24 });
  });
});

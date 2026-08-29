import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, cleanup, fireEvent, waitFor, screen } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";

const getLiveSessionsCounts = vi.fn();
const getLiveSessionsPage = vi.fn();
let emitBoardChanged: () => void = () => {};
let emitSnapChanged: (e: { payload: { edge: "left" | "right" | "top" | null } }) => void = () => {};
let emitRecallSticker: () => void = () => {};
const unlisten = vi.fn();

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

vi.mock("./api", () => ({
  getLiveSessionsCounts: () => getLiveSessionsCounts(),
  // 与真实 api.ts 同款归一化：允许各测试继续用「裸数组」写 mock 数据，这里补上
  // { items, next_cursor } 包装（给满 limit → 用末项当游标；给不满 → 到底）。
  getLiveSessionsPage: (
    filter: "all" | "running" | "waiting" | "archived",
    search: string | null,
    cursor: { last_event_at: number; id: number } | null,
    limit: number
  ) =>
    Promise.resolve(getLiveSessionsPage(filter, search, cursor, limit)).then((res: unknown) => {
      if (!Array.isArray(res)) return res;
      const rows = res as { session: { last_event_at: number; id: number } }[];
      const last = rows[rows.length - 1];
      return {
        items: rows,
        next_cursor: rows.length >= limit && last
          ? { last_event_at: last.session.last_event_at, id: last.session.id }
          : null,
      };
    }),
  getSettings: () =>
    Promise.resolve({
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
    }),
  getAccounts: () => Promise.resolve([]),
  firstImportNotice: () => Promise.resolve(null),
  refreshUsage: (_provider: string) => Promise.reject(new Error("USAGE_UNSUPPORTED")),
  // 本 mock 不展开真实模块（工厂里显式列举导出），故 useAgents 用到的两个都得在此提供。
  listAgents: () => Promise.resolve([]),
  agentName: (agents: { id: string; display_name: string }[], id: string) =>
    agents.find((a) => a.id === id)?.display_name ?? id,
}));
// 按事件名分别路由：board-changed → emitBoardChanged；snap-changed → emitSnapChanged
// （真实吸边靠 Tauri 后端无法在 jsdom 测，但拖拽中卸载的轮询泄漏可以模拟事件验证）。
vi.mock("@tauri-apps/api/event", () => ({
  listen: (event: string, cb: (e: { payload: { edge: "left" | "right" | "top" | null } }) => void) => {
    if (event === "board-changed") emitBoardChanged = cb as () => void;
    if (event === "snap-changed") emitSnapChanged = cb;
    if (event === "recall-sticker") emitRecallSticker = cb as unknown as () => void;
    return Promise.resolve(unlisten);
  },
  emit: vi.fn(() => Promise.resolve()),
}));
// 吸边相关的 Tauri 命令/窗口 API：jsdom 无后端，给空实现避免报错。
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve()),
}));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    setAlwaysOnTop: vi.fn(() => Promise.resolve()),
    outerSize: vi.fn(() => Promise.resolve({ width: 340, height: 440 })),
    scaleFactor: vi.fn(() => Promise.resolve(1)),
  }),
}));
// 更新检查相关插件：jsdom 无后端，给空实现（check 返回无更新）。
vi.mock("@tauri-apps/plugin-updater", () => ({ check: vi.fn(() => Promise.resolve(null)) }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: vi.fn(() => Promise.resolve()) }));

import { App } from "./App";
import { zh } from "./i18n/zh";

beforeEach(() => {
  getLiveSessionsCounts.mockReset();
  getLiveSessionsCounts.mockResolvedValue({ total: 0, running: 0, waiting: 0, archived: 0 });
  getLiveSessionsPage.mockReset();
  getLiveSessionsPage.mockResolvedValue([]);
  unlisten.mockReset();
  // mockReset 连实现一起清（防上个用例的 mockImplementation 泄漏），再装回默认空实现。
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(() => Promise.resolve(undefined));
  localStorage.clear();
});
afterEach(() => cleanup());

/** W-17：贴纸几何（尺寸/吸附边/置顶/位置）在 settings.json——用 invoke mock 播种读取值。 */
const seedWindowState = (s: {
  normal_width: number | null;
  normal_height: number | null;
  snap_edge: "left" | "right" | "top" | null;
  pinned: boolean;
  x: number | null;
  y: number | null;
}) => {
  vi.mocked(invoke).mockImplementation((cmd: string) =>
    Promise.resolve(cmd === "get_sticker_window_state" ? s : undefined)
  );
};
const EMPTY_WS = { normal_width: null, normal_height: null, snap_edge: null, pinned: false, x: null, y: null };

describe("App", () => {
  it("挂载时拉取 counts 和第 0 页", async () => {
    render(<App />);
    await waitFor(() => expect(getLiveSessionsCounts).toHaveBeenCalledTimes(1));
    // search 初值为空字符串（App 自持有的真实搜索状态，非固定 null）。
    await waitFor(() => expect(getLiveSessionsPage).toHaveBeenCalledWith("all", "", null, 100));
  });

  // 回归：冷启动首页未落地前不能闪「还没有会话」假空态，必须显示加载占位。
  it("初始加载中显示加载占位，落地后再切到正常空态", async () => {
    let resolvePage: (v: unknown[]) => void = () => {};
    getLiveSessionsPage.mockImplementation((_f: string, search: string | null) => {
      if (search === null) return Promise.resolve([]); // 折叠条查询不挂起
      return new Promise((r) => { resolvePage = r as (v: unknown[]) => void; });
    });
    render(<App />);
    await waitFor(() => expect(screen.getByText(zh.sticker.loading)).toBeTruthy());
    expect(screen.queryByText(zh.empty.allTitle)).toBeNull();
    resolvePage([]);
    await waitFor(() => expect(screen.getByText(zh.empty.allTitle)).toBeTruthy());
    expect(screen.queryByText(zh.sticker.loading)).toBeNull();
  });

  // 回归：首页加载失败曾只 console.error，用户看到「还没有会话」。现在必须显示
  // 「加载失败 + 重试」，且点重试会重新发起首页加载，成功后回到正常空态。
  it("初始加载失败显示错误与重试，重试成功后恢复正常", async () => {
    let failFirst = true;
    getLiveSessionsPage.mockImplementation((_f: string, search: string | null) => {
      if (search === null) return Promise.resolve([]);
      if (failFirst) {
        failFirst = false;
        return Promise.reject(new Error("boom"));
      }
      return Promise.resolve([]);
    });
    render(<App />);
    await waitFor(() => expect(screen.getByText(zh.sticker.loadFailed)).toBeTruthy());
    expect(screen.queryByText(zh.empty.allTitle)).toBeNull();

    const callsBefore = getLiveSessionsPage.mock.calls.filter(([, s]) => s === "").length;
    fireEvent.click(screen.getByTestId("empty-retry-cta"));
    await waitFor(() =>
      expect(getLiveSessionsPage.mock.calls.filter(([, s]) => s === "").length).toBeGreaterThan(callsBefore)
    );
    await waitFor(() => expect(screen.getByText(zh.empty.allTitle)).toBeTruthy());
    expect(screen.queryByText(zh.sticker.loadFailed)).toBeNull();
  });

  it("收到 board-changed 后重新拉取 counts 和第 0 页", async () => {
    render(<App />);
    await waitFor(() => expect(getLiveSessionsCounts).toHaveBeenCalledTimes(1));
    emitBoardChanged();
    await waitFor(() => expect(getLiveSessionsCounts).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(getLiveSessionsPage).toHaveBeenCalledWith("all", "", null, 100));
  });

  // T-12：「正在启动」占位卡（后端合成的负 id 占位项）随首页进列表；同 provider+cwd
  // 且 started_at 不早于占位的真实会话出现时对账撤卡（增量刷新也不留残影）。
  it("pending 占位卡（负 id）在真实会话出现时对账撤下", async () => {
    const startedAt = Date.now();
    const pendingRow = {
      session: { id: -1, project_id: 0, cc_session_id: "", status: "running", started_at: startedAt, last_event_at: startedAt, ended_at: null },
      project_name: "proj", task_title: "", current_activity: null, column: "doing",
      todo_done: 0, todo_total: 0, todos: [], pid: null, archived: false, archived_at: null,
      cwd: "C:/work/proj", context_pct: null, context_window: null, model: null, note: null,
      pending_review: null, last_ai_text: null, last_user_text: null, provider: "codex",
      profile: null, predecessor_id: null, extra_dirs: [],
      connected: true, pty_managed: true, background: false, errored: false,
      error_label: null, error_raw: null, busy_subagents: 0, preview: null,
      profile_name: null, foreign: false,
    };
    const realRow = {
      ...pendingRow,
      session: { ...pendingRow.session, id: 42, cc_session_id: "real-42", started_at: startedAt + 500, last_event_at: startedAt + 500 },
      task_title: "真实会话",
    };
    // 折叠条查询 search=null → 空；看板查询 search="" → 先只给占位行。
    getLiveSessionsPage.mockImplementation((_f: string, search: string | null) =>
      Promise.resolve(search === null ? [] : [pendingRow]));
    render(<App />);
    await screen.findByTestId("stk-starting-card");
    // 认领完成：后端不再上报占位，真实行（同 provider+cwd、started_at 更晚）进首页。
    getLiveSessionsPage.mockImplementation((_f: string, search: string | null) =>
      Promise.resolve(search === null ? [] : [realRow]));
    emitBoardChanged();
    await waitFor(() => expect(screen.queryByTestId("stk-starting-card")).toBeNull());
    await screen.findByText("真实会话");
  });

  // 回归（W-6）：「找回贴纸」曾永久改写用户的置顶偏好。现在只做临时置顶（recall_center
  // 带上当前偏好，聚焦后由后端还原），持久化偏好（W-17 起在 settings.json）不动。
  it("找回贴纸不改写 pin 偏好：recall_center 带当前偏好，不写 localStorage", async () => {
    render(<App />);
    await waitFor(() => expect(getLiveSessionsCounts).toHaveBeenCalledTimes(1));
    emitRecallSticker();
    await waitFor(() =>
      expect(vi.mocked(invoke)).toHaveBeenCalledWith("recall_center", { pinned: false })
    );
    expect(localStorage.getItem("meowo-pinned")).toBeNull();
  });

  it("找回贴纸：用户本就置顶时 recall_center 带 pinned=true（保持置顶）", async () => {
    seedWindowState({ ...EMPTY_WS, pinned: true });
    render(<App />);
    await waitFor(() => expect(getLiveSessionsCounts).toHaveBeenCalledTimes(1));
    emitRecallSticker();
    await waitFor(() =>
      expect(vi.mocked(invoke)).toHaveBeenCalledWith("recall_center", { pinned: true })
    );
  });

  // 回归：拖拽中途卸载组件，90ms 的 pointer_left_down 松手轮询必须随 cleanup 停掉——
  // 否则 interval 泄漏，卸载后仍持续打 IPC 空转。
  it("拖拽中途卸载：松手轮询 pointer_left_down 随卸载停止，不泄漏", async () => {
    // 左键一直按着（resolve true）：轮询不会自己停，只能指望卸载时的 cleanup。
    vi.mocked(invoke).mockImplementation((cmd: string) =>
      Promise.resolve(cmd === "pointer_left_down" ? true : undefined)
    );
    const { container, unmount } = render(<App />);
    await waitFor(() => expect(getLiveSessionsCounts).toHaveBeenCalledTimes(1));

    // mousedown 命中拖拽区 → 进入拖拽态；随后 snap-changed 近边 → 启动松手轮询。
    fireEvent.mouseDown(container.querySelector(".drag")!);
    emitSnapChanged({ payload: { edge: "left" } });
    await waitFor(() =>
      expect(vi.mocked(invoke)).toHaveBeenCalledWith("pointer_left_down")
    );

    unmount();
    const pollCalls = () =>
      vi.mocked(invoke).mock.calls.filter(([cmd]) => cmd === "pointer_left_down").length;
    const atUnmount = pollCalls();
    // 轮询若还活着，300ms（>3 个周期）内必然再触发数次。
    await new Promise((r) => setTimeout(r, 300));
    expect(pollCalls()).toBe(atUnmount);
  });

  it("清空搜索时恢复搜索前的列表顺序", async () => {
    const mk = (id: number, title: string, lastEventAt: number) => ({
      session: {
        id,
        cc_session_id: `s-${id}`,
        status: "running" as const,
        last_event_at: lastEventAt,
        started_at: 0,
        ended_at: null,
        project_id: 1,
      },
      project_name: "p",
      task_title: title,
      current_activity: null,
      column: "todo" as const,
      todo_done: 0,
      todo_total: 0,
      todos: [],
      pid: null,
      connected: true,
      archived: false,
      archived_at: null,
      cwd: "/p",
      errored: false,
      error_label: null,
      error_raw: null,
      preview: null,
      note: null,
      context_pct: null,
      context_window: null,
      model: null,
      pending_review: null,
      last_ai_text: null,
      last_user_text: null,
      provider: "claude" as const,
    });
    const original = [mk(2, "B", 2000), mk(1, "A", 1000)];
    getLiveSessionsCounts.mockResolvedValue({ total: 2, running: 2, waiting: 0, archived: 0 });
    let emptySearchCalls = 0;
    getLiveSessionsPage.mockImplementation((_f: string, search: string | null) => {
      if (search === null) return Promise.resolve([]);
      if (search === "A") return Promise.resolve([original[1]]);
      emptySearchCalls += 1;
      if (emptySearchCalls === 1) return Promise.resolve(original);
      // 模拟清空后的刷新尚未返回：原列表应从缓存立即恢复，不能依赖这次响应。
      return new Promise(() => {});
    });

    const { container } = render(<App />);
    const titles = () =>
      Array.from(container.querySelectorAll(".stk-title")).map((el) => el.textContent);
    await waitFor(() => expect(titles()).toEqual(["B", "A"]));

    // 按 aria-label 定位搜索按钮，不吃底栏按钮增减的位置变化。
    fireEvent.click(container.querySelector('.stk-bar-actions [aria-label="搜索会话"]')!);
    fireEvent.change(container.querySelector(".stk-search-in")!, { target: { value: "A" } });
    await waitFor(() => expect(getLiveSessionsPage).toHaveBeenCalledWith("all", "A", null, 100));
    await waitFor(() => expect(titles()).toEqual(["A"]));

    fireEvent.click(container.querySelector(".stk-search-x")!);
    await waitFor(() => expect(titles()).toEqual(["B", "A"]));
  });

  // 单一真相源（W-17）：main 窗口尺寸/吸附边/置顶由 settings.json 的 sticker_window 持有
  // （window-state 插件对 main 已 denylist，位置由后端 setup 从 settings 恢复）。非吸附态启动
  // 无条件按记住的正常尺寸（默认 {360,440}）snap_restore 还原，且不走折叠分支。
  it("非吸附态启动按 settings 尺寸还原正常尺寸(snap_restore)，不折叠", async () => {
    render(<App />);
    await waitFor(() =>
      expect(vi.mocked(invoke)).toHaveBeenCalledWith(
        "snap_restore",
        expect.objectContaining({ width: 360, height: 440 })
      )
    );
    expect(vi.mocked(invoke)).not.toHaveBeenCalledWith("snap_collapse", expect.anything());
  });

  it("吸附态启动(settings.snap_edge 有边)走折叠分支，不触发尺寸还原", async () => {
    seedWindowState({ ...EMPTY_WS, snap_edge: "left" });
    render(<App />);
    await waitFor(() =>
      expect(vi.mocked(invoke)).toHaveBeenCalledWith("snap_collapse", expect.anything())
    );
    expect(vi.mocked(invoke)).not.toHaveBeenCalledWith("snap_restore", expect.anything());
  });

  // W-17 一次性迁移：旧版三套 localStorage 键（尺寸/吸附边/置顶）在 settings 字段缺席时
  // 并入 sticker_window 原子落盘，旧键随即删除；此后 settings.json 是唯一真相源。
  it("W-17：localStorage 旧键一次性迁移进 settings 并删除旧键", async () => {
    localStorage.setItem("meowo-normal-size", JSON.stringify({ w: 420, h: 500 }));
    localStorage.setItem("meowo-snap-edge", "top");
    localStorage.setItem("meowo-pinned", "1");
    render(<App />);
    await waitFor(() =>
      expect(vi.mocked(invoke)).toHaveBeenCalledWith("set_sticker_window_state", {
        state: { normal_width: 420, normal_height: 500, snap_edge: "top", pinned: true, x: null, y: null },
      })
    );
    expect(localStorage.getItem("meowo-normal-size")).toBeNull();
    expect(localStorage.getItem("meowo-snap-edge")).toBeNull();
    expect(localStorage.getItem("meowo-pinned")).toBeNull();
    // 迁移来的吸附边生效：走折叠分支。
    await waitFor(() =>
      expect(vi.mocked(invoke)).toHaveBeenCalledWith("snap_collapse", expect.anything())
    );
  });

  it("W-17：settings 已有几何值时，残留旧键不覆盖（仅清除），不触发迁移写", async () => {
    seedWindowState({ ...EMPTY_WS, normal_width: 400, normal_height: 460 });
    localStorage.setItem("meowo-normal-size", JSON.stringify({ w: 999, h: 999 }));
    render(<App />);
    await waitFor(() =>
      expect(vi.mocked(invoke)).toHaveBeenCalledWith(
        "snap_restore",
        expect.objectContaining({ width: 400, height: 460 })
      )
    );
    expect(localStorage.getItem("meowo-normal-size")).toBeNull();
    expect(vi.mocked(invoke)).not.toHaveBeenCalledWith("set_sticker_window_state", expect.anything());
  });

  it("W-17：切换置顶经 set_sticker_window_state 落盘，不再写 localStorage", async () => {
    render(<App />);
    await waitFor(() => expect(getLiveSessionsCounts).toHaveBeenCalledTimes(1));
    fireEvent.click(await screen.findByLabelText(zh.sticker.pinOff));
    await screen.findByLabelText(zh.sticker.pinOn);
    await waitFor(() =>
      expect(vi.mocked(invoke)).toHaveBeenCalledWith("set_sticker_window_state", {
        state: expect.objectContaining({ pinned: true }),
      })
    );
    expect(localStorage.getItem("meowo-pinned")).toBeNull();
  });

  // 回归（W-4）：展开过渡期（snap_restore 未落地）窗口仍是 28px 细条几何，直接渲染
  // 全量看板会被压进细条宽度造成布局抖动。过渡期必须渲染 .snap-expanding 纯色占位，
  // 落地后才挂载看板。
  it("W-4：找回展开过渡期渲染纯色占位，snap_restore 落地后才挂载看板", async () => {
    let resolveRestore: () => void = () => {};
    vi.mocked(invoke).mockImplementation((cmd: string) => {
      if (cmd === "get_sticker_window_state") {
        return Promise.resolve({ ...EMPTY_WS, snap_edge: "right" });
      }
      return cmd === "snap_restore"
        ? new Promise<void>((r) => { resolveRestore = r; })
        : Promise.resolve(undefined);
    });
    const { container } = render(<App />);
    await waitFor(() =>
      expect(vi.mocked(invoke)).toHaveBeenCalledWith("snap_collapse", expect.anything())
    );
    emitRecallSticker();
    await waitFor(() => expect(container.querySelector(".snap-expanding")).toBeTruthy());
    // 占位期间缩略条不得回闪（展开瞬间渲染缩略条正是 expanding 闸门要防的）。
    expect(container.querySelector(".cstrip")).toBeNull();
    resolveRestore();
    await waitFor(() => expect(container.querySelector(".snap-expanding")).toBeNull());
  });

  // 回归：尺寸基准曾被「吸附态拖角缩成细条」的尺寸毒化(实测 {80,240}/{136,20})。
  // 读入时的 sanitize 须把低于最小尺寸的值丢弃、回落默认 {360,440}，否则还原会用毒化的
  // 细条尺寸、把窗口缩成细条。（W-17 起折叠态禁 resizable，细条已拖不出来；这里是纵深防御。）
  it("settings 尺寸被细条尺寸毒化时，启动回落默认 {360,440} 再 snap_restore", async () => {
    seedWindowState({ ...EMPTY_WS, normal_width: 80, normal_height: 37 });
    render(<App />);
    await waitFor(() =>
      expect(vi.mocked(invoke)).toHaveBeenCalledWith(
        "snap_restore",
        expect.objectContaining({ width: 360, height: 440 })
      )
    );
  });

  // 回归：异常大值/非有限数(settings.json 被手改坏)不能直接喂给 set_size，否则设出极端窗口。
  // sanitize 须校验上界(<=20000)与有限数，超界则回落默认 {360,440}。
  it("settings 尺寸异常大值时，启动回落默认 {360,440}，不设出极端窗口", async () => {
    seedWindowState({ ...EMPTY_WS, normal_width: 999999, normal_height: 999999 });
    render(<App />);
    await waitFor(() =>
      expect(vi.mocked(invoke)).toHaveBeenCalledWith(
        "snap_restore",
        expect.objectContaining({ width: 360, height: 440 })
      )
    );
  });

  // 回归：running tab 下刷新时，若某会话已从 running 迁移到 waiting，后端 running 分页不再返回它；
  // 旧合并逻辑会保留 prev 中的该会话（状态仍是旧 running），导致它错误地继续显示在 running tab 下。
  it("running tab 刷新时，状态迁出 running 的会话应从列表移除", async () => {
    localStorage.setItem("meowo-tab", "running");
    const mk = (id: number, status: "running" | "waiting", title: string) => ({
      session: {
        id,
        cc_session_id: `s-${id}`,
        status,
        last_event_at: 1000 - id,
        started_at: 0,
        ended_at: null,
        project_id: 1,
      },
      project_name: "p",
      task_title: title,
      current_activity: null,
      column: "todo" as const,
      todo_done: 0,
      todo_total: 0,
      todos: [],
      pid: null,
      connected: true,
      archived: false,
      archived_at: null,
      cwd: "/p",
      errored: false,
      error_label: null,
      error_raw: null,
      preview: null,
      note: null,
      context_pct: null,
      context_window: null,
      model: null,
      pending_review: null,
      last_ai_text: null,
      last_user_text: null,
      provider: "claude" as const,
    });

    getLiveSessionsCounts.mockResolvedValue({ total: 2, running: 2, waiting: 0, archived: 0 });
    // tab 页按序返回；折叠条调用（search===null，独立于 tab）返回 []，不消耗序列。
    const tabPages = [
      [mk(1, "running", "A"), mk(2, "running", "B")],
      [mk(1, "running", "A")], // B 已迁出 running
    ];
    let ti = 0;
    getLiveSessionsPage.mockImplementation((_f: string, search: string | null) => {
      if (search === null) return Promise.resolve([]);
      return Promise.resolve(tabPages[Math.min(ti++, tabPages.length - 1)]);
    });

    render(<App />);
    await waitFor(() => expect(screen.getByText("A")).toBeTruthy());
    await waitFor(() => expect(screen.getByText("B")).toBeTruthy());

    emitBoardChanged();
    await waitFor(() => expect(screen.queryByText("B")).toBeFalsy());
    expect(screen.getByText("A")).toBeTruthy();
  });

  // 回归：已存在会话的 last_event_at 变化时（如恢复断开的旧会话），board-changed 刷新应按
  // 新顺序重排；旧合并逻辑按 prev 数组的原位置合并，只有全新会话才 prepend 到最前，已存在
  // 会话不会因排序键变化而移动——用户须手动切 tab 才能看到它跳到最前。
  it("board-changed 刷新时，已存在会话应按新 last_event_at 重新排序（如恢复的旧会话跳到最前）", async () => {
    localStorage.setItem("meowo-tab", "all");
    const mk = (id: number, lastEventAt: number, title: string) => ({
      session: {
        id,
        cc_session_id: `s-${id}`,
        status: "running" as const,
        last_event_at: lastEventAt,
        started_at: 0,
        ended_at: null,
        project_id: 1,
      },
      project_name: "p",
      task_title: title,
      current_activity: null,
      column: "todo" as const,
      todo_done: 0,
      todo_total: 0,
      todos: [],
      pid: null,
      connected: true,
      archived: false,
      archived_at: null,
      cwd: "/p",
      errored: false,
      error_label: null,
      error_raw: null,
      preview: null,
      note: null,
      context_pct: null,
      context_window: null,
      model: null,
      pending_review: null,
      last_ai_text: null,
      last_user_text: null,
      provider: "claude" as const,
    });

    getLiveSessionsCounts.mockResolvedValue({ total: 2, running: 2, waiting: 0, archived: 0 });
    // 首页：B（last_event_at 更晚）排前，A（旧）排后；board-changed 后 A 恢复排最前。
    // tab 页按序返回；折叠条调用（search===null）返回 []，不消耗序列。
    const tabPages = [
      [mk(2, 2000, "B"), mk(1, 1000, "A")],
      [mk(1, 3000, "A"), mk(2, 2000, "B")],
    ];
    let ti = 0;
    getLiveSessionsPage.mockImplementation((_f: string, search: string | null) => {
      if (search === null) return Promise.resolve([]);
      return Promise.resolve(tabPages[Math.min(ti++, tabPages.length - 1)]);
    });

    render(<App />);
    await waitFor(() => {
      const titles = Array.from(document.querySelectorAll(".stk-title")).map((el) => el.textContent);
      expect(titles).toEqual(["B", "A"]);
    });

    emitBoardChanged();
    await waitFor(() => {
      const titles = Array.from(document.querySelectorAll(".stk-title")).map((el) => el.textContent);
      expect(titles).toEqual(["A", "B"]);
    });
  });
});

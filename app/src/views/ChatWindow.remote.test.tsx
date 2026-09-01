import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";

// 复刻 ChatWindow.test.tsx 的夹具(mock 按文件隔离,不能跨文件共享)。这里只关心门控,
// 故 mock 尽量瘦:够渲染出标题栏与页签即可。
const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({
  invoke,
  convertFileSrc: (path: string) => `asset://localhost/${encodeURIComponent(path)}`,
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ close: vi.fn(() => Promise.resolve()), setTitle: vi.fn(() => Promise.resolve()) }),
}));
vi.mock("./ManagedTerminal", () => ({
  ManagedTerminal: (props: { sessionId: number }) => <div>PTY {props.sessionId}</div>,
}));

import { ChatWindow } from "./ChatWindow";
import { markRemoteUi, SELECT_SESSION_EVENT } from "../remoteMode";

// git 仓库 + 运行中会话:diff 入口(需 cwd)与「终端」页签都应在桌面态出现。
function runningSessionImpl(command: string, args?: Record<string, unknown>): Promise<unknown> {
  if (command === "get_chat_history") {
    const h = {
      connected: true, sessionId: 7, title: "门控用例", status: "running",
      provider: "claude", cwd: "C:/repo", supported: true, offset: 10, reset: false,
      pendingReview: null, ptyManaged: true,
      items: [{ type: "assistant_text", id: "a1", timestamp: null, text: "在跑" }],
    };
    const cursor = (args?.offset as number) ?? 0;
    if (cursor > 0 && cursor >= h.offset) return Promise.resolve({ ...h, items: [], hasMore: false });
    return Promise.resolve(h);
  }
  if (command === "pending_interaction") return Promise.resolve({ approval: null, question: null });
  if (command === "managed_terminal_binding") return Promise.resolve(null);
  if (command === "managed_terminal_snapshot") return Promise.resolve({ sessionId: 7, active: true, managed: true, data: "", startOffset: 0, endOffset: 0, exited: false, exitCode: null });
  return Promise.resolve();
}
function runningSessionWithRepo() {
  invoke.mockImplementation(runningSessionImpl);
}

afterEach(() => {
  cleanup();
  invoke.mockReset();
  // 门控标志挂在 globalThis,不复位会把后面的桌面用例污染成远程态(参照 remoteMode.test.ts)。
  (globalThis as Record<string, unknown>).__MEOWO_REMOTE__ = undefined;
  document.body.classList.remove("remote-ui");
  window.history.replaceState({}, "", "/");
  localStorage.clear();
});

describe("ChatWindow 远程门控", () => {
  it("桌面态:终端页签与改动入口都在场", async () => {
    window.history.replaceState({}, "", "/?sessionId=7");
    runningSessionWithRepo();
    render(<ChatWindow />);
    await waitFor(() => expect(screen.getByText("门控用例")).toBeTruthy());

    expect(screen.getByRole("button", { name: "终端" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "对话" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "查看改动" })).toBeTruthy();
  });

  it("远程态:终端页签与改动入口一并隐藏(输入框仍在,可发消息)", async () => {
    markRemoteUi();
    window.history.replaceState({}, "", "/?sessionId=7");
    runningSessionWithRepo();
    render(<ChatWindow />);
    await waitFor(() => expect(screen.getByText("门控用例")).toBeTruthy());

    // 页签整块与 diff 入口被 remoteUi() 门控掉。
    expect(screen.queryByRole("button", { name: "终端" })).toBeNull();
    expect(screen.queryByRole("button", { name: "对话" })).toBeNull();
    expect(screen.queryByRole("button", { name: "查看改动" })).toBeNull();
    // 但对话核心(发消息)不受影响。
    expect(screen.getByRole("combobox", { name: "发送消息给 Agent" })).toBeTruthy();
  });

  it("远程态:新建会话事件选中临时 id(启动中占位),binding 认领后落到真会话", async () => {
    markRemoteUi();
    // 无 ?sessionId、无存储恢复 → sessionId=0 空态(用户实拍反馈的落点)。
    invoke.mockImplementation((command: string, args?: Record<string, unknown>) => {
      // 临时 id -3 认领成真 id 7(T-13 binding 轮询通道,managed_terminal_binding 在远程白名单)。
      if (command === "managed_terminal_binding") {
        return Promise.resolve((args?.sessionId as number) === -3 ? 7 : null);
      }
      return runningSessionImpl(command, args);
    });
    render(<ChatWindow />);
    await waitFor(() => expect(screen.getByText("从侧栏选择一个会话")).toBeTruthy());

    // RemoteApp 在 NewSessionPanel 启动成功后派发:临时负 id 即导航句柄。
    window.dispatchEvent(new CustomEvent(SELECT_SESSION_EVENT, { detail: -3 }));
    await waitFor(() => expect(screen.getByText("会话正在启动…")).toBeTruthy());
    expect(screen.queryByText("从侧栏选择一个会话")).toBeNull();

    // 认领(250ms 轮询)后原地重绑:加载真会话历史,空态/占位都退场。
    await waitFor(() => expect(screen.getByText("门控用例")).toBeTruthy(), { timeout: 3000 });
    expect(screen.queryByText("会话正在启动…")).toBeNull();
  });
});

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
import { markRemoteUi } from "../remoteMode";

// git 仓库 + 运行中会话:diff 入口(需 cwd)与「终端」页签都应在桌面态出现。
function runningSessionWithRepo() {
  invoke.mockImplementation((command: string, args?: Record<string, unknown>) => {
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
  });
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
    expect(screen.getByRole("textbox", { name: "发送消息给 Agent" })).toBeTruthy();
  });
});

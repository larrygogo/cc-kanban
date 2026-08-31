import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

import { QuickSwitcher, type PaletteCommand } from "./WindowModals";
import type { LiveSession } from "../../api";
import { zh } from "../../i18n/zh";

function session(id: number, title: string, extra: Partial<LiveSession> = {}): LiveSession {
  return {
    session: { id, cc_session_id: `cc-${id}`, status: "ended" },
    project_name: "meowo",
    task_title: title,
    connected: false,
    pending_review: null,
    cwd: "C:/Users/me/workspace/meowo",
    provider: "claude",
    ...extra,
  } as unknown as LiveSession;
}

/// 与 ChatWindow 实际下发的命令清单同构（标签直接取字典，防止两张皮）。
const COMMANDS: PaletteCommand[] = [
  { id: "new-session", label: zh.chat.shortcutNewSession, keys: "Ctrl+N" },
  { id: "view-chat", label: zh.chat.cmdViewChat, keys: "Ctrl+1" },
  { id: "toggle-sidebar", label: zh.chat.shortcutSidebar, keys: "Ctrl+B" },
  { id: "focus-search", label: zh.chat.shortcutSearch, keys: "Ctrl+F" },
  { id: "open-settings", label: zh.sticker.openSettings },
];

function mockSessions(items: LiveSession[]) {
  invoke.mockImplementation((command: string) => {
    if (command === "get_live_sessions_page") return Promise.resolve({ items, next_cursor: null });
    return Promise.resolve();
  });
}

function renderPalette(overrides: Partial<Parameters<typeof QuickSwitcher>[0]> = {}) {
  const props = {
    activeId: 0,
    commands: COMMANDS,
    onPick: vi.fn(),
    onCommand: vi.fn(),
    onClose: vi.fn(),
    ...overrides,
  };
  render(<QuickSwitcher {...props} />);
  return props;
}

describe("QuickSwitcher 命令面板（U1-14）", () => {
  beforeEach(() => invoke.mockReset());
  afterEach(cleanup);

  it("空查询时会话组与命令组都全列（组头小标签 + 命令右侧快捷键提示）", async () => {
    mockSessions([session(1, "修 bug"), session(2, "写文档")]);
    renderPalette();
    expect(await screen.findByRole("option", { name: /修 bug/ })).toBeTruthy();
    // 两组组头都在，会话组在前、命令组在后。
    const listbox = screen.getByRole("listbox");
    expect(listbox.textContent).toContain(zh.chat.switcherGroupSessions);
    expect(listbox.textContent).toContain(zh.chat.switcherGroupCommands);
    expect(listbox.textContent!.indexOf(zh.chat.switcherGroupSessions))
      .toBeLessThan(listbox.textContent!.indexOf(zh.chat.switcherGroupCommands));
    // 命令全列 + 快捷键提示渲染。
    expect(screen.getByRole("option", { name: /新建会话/ }).textContent).toContain("Ctrl+N");
    expect(screen.getByRole("option", { name: /打开设置/ })).toBeTruthy();
  });

  it("↑↓ 跨两组导航：从最后一个会话 ArrowDown 落到第一条命令，Enter 执行并关闭", async () => {
    mockSessions([session(1, "修 bug")]);
    const props = renderPalette();
    const input = await screen.findByRole("textbox");
    // 1 条会话 + 5 条命令：两下 ↓ 到第二条命令（view-chat）。
    fireEvent.keyDown(input, { key: "ArrowDown" });
    fireEvent.keyDown(input, { key: "ArrowDown" });
    expect(screen.getByRole("option", { name: new RegExp(zh.chat.cmdViewChat) }).getAttribute("aria-selected")).toBe("true");
    fireEvent.keyDown(input, { key: "Enter" });
    expect(props.onClose).toHaveBeenCalled();
    expect(props.onCommand).toHaveBeenCalledWith("view-chat");
    expect(props.onPick).not.toHaveBeenCalled();
  });

  it("Home / End 跨两组跳首尾", async () => {
    mockSessions([session(1, "修 bug")]);
    renderPalette();
    const input = await screen.findByRole("textbox");
    fireEvent.keyDown(input, { key: "End" });
    expect(screen.getByRole("option", { name: new RegExp(zh.sticker.openSettings) }).getAttribute("aria-selected")).toBe("true");
    fireEvent.keyDown(input, { key: "Home" });
    expect(screen.getByRole("option", { name: /修 bug/ }).getAttribute("aria-selected")).toBe("true");
  });

  it("输入查询同时过滤会话（下沉后端 search）与命令（客户端子串）", async () => {
    mockSessions([session(1, "修 bug")]);
    renderPalette();
    const input = await screen.findByRole("textbox");
    // 命令过滤即时生效：只剩标签含「视图」的命令。
    fireEvent.change(input, { target: { value: "视图" } });
    expect(screen.queryByRole("option", { name: /新建会话/ })).toBeNull();
    expect(screen.getByRole("option", { name: new RegExp(zh.chat.cmdViewChat) })).toBeTruthy();
    // 会话查询带 200ms 防抖下沉后端，search 参数原样传下去。
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith(
        "get_live_sessions_page",
        expect.objectContaining({ search: "视图" }),
      );
    });
  });

  it("两组都落空时显示统一空态", async () => {
    mockSessions([]);
    renderPalette();
    const input = await screen.findByRole("textbox");
    fireEvent.change(input, { target: { value: "不存在的东西" } });
    await waitFor(() => {
      expect(screen.getByText(zh.chat.switcherEmpty)).toBeTruthy();
    });
    expect(screen.queryByRole("option")).toBeNull();
  });

  it("命令条目点击执行并关闭弹层", async () => {
    mockSessions([session(1, "修 bug")]);
    const props = renderPalette();
    const cmd = await screen.findByRole("option", { name: /打开设置/ });
    fireEvent.click(cmd);
    expect(props.onClose).toHaveBeenCalled();
    expect(props.onCommand).toHaveBeenCalledWith("open-settings");
  });

  it("Enter 落在会话上仍是跳转（is-current 会话也先关弹层）", async () => {
    mockSessions([session(7, "当前会话"), session(8, "别的会话")]);
    const props = renderPalette({ activeId: 7 });
    const input = await screen.findByRole("textbox");
    // 默认 active=0（当前会话）：Enter 关闭但不重复 onPick。
    fireEvent.keyDown(input, { key: "Enter" });
    expect(props.onClose).toHaveBeenCalled();
    expect(props.onPick).not.toHaveBeenCalled();
    // 重新打开场景下 ArrowDown 一次再 Enter 才跳别的会话——这里直接测点击。
    fireEvent.click(screen.getByRole("option", { name: /别的会话/ }));
    expect(props.onPick).toHaveBeenCalledWith(8);
  });
});

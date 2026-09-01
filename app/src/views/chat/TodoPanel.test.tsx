import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

// i18n 模块链会牵到 tauri api（settings 读取）,测试里没有运行时,打桩即可。
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

import { zh as t } from "../../i18n/zh";
import { TodoPanel } from "./TodoPanel";
import { ChatTodoMenu } from "./TitleMenus";

afterEach(cleanup);

const row = (content: string, status: string, stale?: boolean) => ({ content, status, stale });

describe("TodoPanel 的「上一任务」分组", () => {
  it("无残留时维持现状:标题进度照算,清单一览", () => {
    const { container } = render(
      <TodoPanel todos={[row("写代码", "completed"), row("跑测试", "in_progress")]} />,
    );
    expect(screen.getByText(t.chat.todos)).toBeTruthy();
    expect(screen.getByText("1/2")).toBeTruthy();
    expect(container.querySelector(".chat-todos-stale")).toBeNull();
    // 主卡默认展开。
    expect(container.querySelector("details.chat-todos")?.hasAttribute("open")).toBe(true);
  });

  it("混合时:进度只计当前任务,残留收进默认折叠的「上一任务」区", () => {
    const { container } = render(
      <TodoPanel
        todos={[
          row("新活一", "in_progress"),
          row("旧活一", "completed", true),
          row("旧活二", "completed", true),
        ]}
      />,
    );
    // 头部进度只计非 stale 的 0/1,而不是把旧账也算进去的 2/3。
    expect(screen.getByText("0/1")).toBeTruthy();
    const staleSection = container.querySelector(".chat-todos-stale");
    expect(staleSection).toBeTruthy();
    // 默认折叠(可展开回看)。
    expect(staleSection?.hasAttribute("open")).toBe(false);
    expect(screen.getByText(t.chat.todoPrevTask(2, 2))).toBeTruthy();
    // 残留条目仍在 DOM 里(展开即可见),不与当前清单混排进主列表。
    const staleItems = staleSection!.querySelectorAll("li");
    expect(staleItems.length).toBe(2);
    expect(screen.getByText("旧活一")).toBeTruthy();
  });

  it("全是残留时:整张卡弱化成一行摘要,默认折叠,不再有主进度", () => {
    const { container } = render(
      <TodoPanel todos={[row("旧活一", "completed", true), row("旧活二", "completed", true)]} />,
    );
    const card = container.querySelector("details.chat-todos.is-stale-only");
    expect(card).toBeTruthy();
    expect(card?.hasAttribute("open")).toBe(false);
    expect(screen.getByText(t.chat.todoPrevTask(2, 2))).toBeTruthy();
    // 不出现「待办 N/N」的主进度口径。
    expect(screen.queryByText(t.chat.todos)).toBeNull();
  });
});

describe("ChatTodoMenu 的「上一任务」分组", () => {
  it("入口计数只计当前任务,残留进独立折叠区", () => {
    const { container } = render(
      <ChatTodoMenu
        todos={[row("新活", "pending"), row("旧活", "completed", true)]}
        subagents={[]}
        t={t}
      />,
    );
    // 打开面板。
    fireEvent.click(container.querySelector<HTMLButtonElement>(".chat-todo-btn")!);
    // 头部进度只计非 stale:0/1。
    expect(screen.getByText("0/1")).toBeTruthy();
    const staleSection = container.querySelector(".chat-todo-panel-stale");
    expect(staleSection).toBeTruthy();
    expect(staleSection?.hasAttribute("open")).toBe(false);
    expect(screen.getByText(t.chat.todoPrevTask(1, 1))).toBeTruthy();
  });
});

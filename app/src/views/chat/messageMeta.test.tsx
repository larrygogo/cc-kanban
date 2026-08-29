import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { Message } from "./Message";
import { Transcript } from "./Transcript";
import { ChatMarkdown } from "../ChatMarkdown";
import type { ChatItem } from "../../api";

afterEach(cleanup);

const user = (id: string, timestamp: string | null, text = `消息 ${id}`): ChatItem =>
  ({ type: "user_text", id, timestamp, text });

/// 日期分隔条:跨天处插一条(含首个带时间的块,IM 惯例);timestamp 为 null 的条目
/// 不参与——既不长分隔条也不重置游标,这保证既有 null 数据的测试与真实乐观回显不受影响。
describe("Transcript 日期分隔条", () => {
  it("跨天插分隔条,同天不重复", () => {
    const items: ChatItem[] = [
      user("a", "2026-08-16T10:00:00Z"),
      user("b", "2026-08-16T12:00:00Z"),
      user("c", "2026-08-17T09:00:00Z"),
    ];
    const { container } = render(<Transcript sessionId={1} items={items} />);
    expect(container.querySelectorAll(".chat-day-sep")).toHaveLength(2);
  });

  it("timestamp 全为 null 时不渲染任何分隔条", () => {
    const items: ChatItem[] = [user("a", null), user("b", null)];
    const { container } = render(<Transcript sessionId={1} items={items} />);
    expect(container.querySelectorAll(".chat-day-sep")).toHaveLength(0);
  });

  it("夹在中间的 null 条目不催生重复分隔条", () => {
    const items: ChatItem[] = [
      user("a", "2026-08-16T10:00:00Z"),
      user("echo", null),
      user("b", "2026-08-16T11:00:00Z"),
    ];
    const { container } = render(<Transcript sessionId={1} items={items} />);
    expect(container.querySelectorAll(".chat-day-sep")).toHaveLength(1);
  });
});

describe("Message 悬停时间", () => {
  it("带 timestamp 的用户/AI 气泡挂 data-tip", () => {
    const { container } = render(
      <>
        <Message item={user("u", "2026-08-16T10:00:00Z", "你好")} />
        <Message item={{ type: "assistant_text", id: "a", timestamp: "2026-08-16T10:01:00Z", text: "在" }} />
      </>,
    );
    const tipped = container.querySelectorAll("article[data-tip]");
    expect(tipped).toHaveLength(2);
  });

  it("timestamp 为 null 时不挂 data-tip", () => {
    const { container } = render(<Message item={user("u", null, "你好")} />);
    expect(container.querySelector("article[data-tip]")).toBeNull();
  });
});

/// 对话代码块语法高亮:复用文件/diff 面板的 hljs 基建(highlight.ts),块级带语言
/// 标注才上色;框线图与超大块保持原渲染(对齐/性能优先于颜色)。
describe("ChatMarkdown 代码高亮", () => {
  it("带语言标注的代码块渲染出 hljs 着色 span", () => {
    const { container } = render(<ChatMarkdown text={"```ts\nconst x = 1;\n```"} />);
    expect(container.querySelector("code.language-ts .hljs-keyword")).toBeTruthy();
    // 复制按钮与语言角标不因高亮而丢。
    expect(screen.getByText("ts")).toBeTruthy();
    expect(screen.getByText("复制")).toBeTruthy();
  });

  it("无语言标注/内联代码保持纯文本", () => {
    const { container } = render(<ChatMarkdown text={"```\nplain\n```\n\n行内 `code` 不动"} />);
    expect(container.querySelector(".hljs-keyword")).toBeNull();
  });

  it("含框线字符的代码块仍走网格重排,不高亮", () => {
    const { container } = render(<ChatMarkdown text={"```ts\n┌──┐\n```"} />);
    expect(container.querySelector(".chat-md-diagram")).toBeTruthy();
    expect(container.querySelector(".hljs-keyword")).toBeNull();
  });

  it("超过行数上限的代码块不高亮", () => {
    const big = "```ts\n" + "const x = 1;\n".repeat(300) + "```";
    const { container } = render(<ChatMarkdown text={big} />);
    expect(container.querySelector(".hljs-keyword")).toBeNull();
  });
});

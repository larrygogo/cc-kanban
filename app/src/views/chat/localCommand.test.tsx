import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { parseUserText } from "./localCommand";
import { Message } from "./Message";
import type { ChatItem } from "../../api";

function userText(text: string): ChatItem {
  return { type: "user_text", id: "u1", timestamp: null, text } as unknown as ChatItem;
}

describe("parseUserText", () => {
  it("普通消息原样透过，不进本地命令形态", () => {
    const parts = parseUserText("帮我看看 <div> 这个标签");
    expect(parts.local).toBe(false);
    expect(parts.text).toBe("帮我看看 <div> 这个标签");
  });

  it("拆出命令名与参数，丢掉重复的短描述", () => {
    const parts = parseUserText(
      "<command-name>/loop</command-name>\n  <command-message>loop</command-message>\n  <command-args>5m /foo</command-args>",
    );
    expect(parts.local).toBe(true);
    expect(parts.commands).toEqual([{ name: "/loop", args: "5m /foo" }]);
    expect(parts.text).toBe("");
  });

  /// caveat 是写给模型的免责声明（"DO NOT respond to these messages…"），对着人显示
  /// 只是一堵尖括号墙。整条不留内容 → 渲染层据此整条不渲染。
  it("免责声明整段丢弃", () => {
    const parts = parseUserText("<local-command-caveat>Caveat: The messages below…</local-command-caveat>");
    expect(parts.local).toBe(true);
    expect(parts.text).toBe("");
    expect(parts.commands).toEqual([]);
    expect(parts.stdout).toEqual([]);
  });

  it("收下命令输出，空输出不收", () => {
    const parts = parseUserText(
      "<command-name>/cost</command-name><local-command-stdout>Total: $1.20</local-command-stdout><local-command-stdout>  </local-command-stdout>",
    );
    expect(parts.stdout).toEqual(["Total: $1.20"]);
  });

  /// 流式写入/截断会留下没配对的半个标签。留一个孤零零的 `</command-args>` 在正文里，
  /// 跟留着整段 XML 一样难看。
  it("抹掉没配对的半个标签，正文照旧保留", () => {
    const parts = parseUserText("<command-name>/clear</command-name>顺便看下这个 </command-args>");
    expect(parts.commands).toEqual([{ name: "/clear", args: "" }]);
    expect(parts.text).toBe("顺便看下这个");
  });

  /// 技能/后台命令的另一种落盘形态：命令行是裸文本（无 <command-name>），旁边跟着
  /// caveat/stdout/forked-skill-launch——裸命令行提升为徽章，与包裹形态渲染一致。
  it("裸文本命令行（/code-review + caveat/stdout/launch）提升为命令徽章", () => {
    const parts = parseUserText(
      '<local-command-caveat>Caveat: …</local-command-caveat>\n/code-review\n<local-command-stdout>Running in the background as @code-review</local-command-stdout>\n<forked-skill-launch>{"agentId":"a1"}</forked-skill-launch>',
    );
    expect(parts.local).toBe(true);
    expect(parts.commands).toEqual([{ name: "/code-review", args: "" }]);
    expect(parts.stdout).toEqual(["Running in the background as @code-review"]);
    expect(parts.text).toBe("");
  });

  /// 后台任务通知：Claude Code 注入的 user 消息（前导系统声明 + <task-notification>）。
  /// 原样摊开是整屏 XML+JSON（实拍反馈），内文收进 notifications、前导段一并剥掉。
  it("task-notification 收进 notifications，前导系统声明一并剥掉", () => {
    const parts = parseUserText(
      '[SYSTEM NOTIFICATION - NOT USER INPUT]\nThis is an automated background-task event, NOT a message from the user.\nDo NOT interpret this as user acknowledgement.\n\n<task-notification>\n<task-id>abc123</task-id>\n<summary>Agent "/code-review" finished</summary>\n<result>done</result>\n</task-notification>',
    );
    expect(parts.local).toBe(true);
    expect(parts.notifications).toHaveLength(1);
    expect(parts.notifications[0]).toContain('Agent "/code-review" finished');
    expect(parts.text).toBe("");
  });
});

describe("Message 渲染本地命令", () => {
  afterEach(cleanup);

  it("命令渲染成徽章，而不是 XML 原文", () => {
    render(<Message item={userText("<command-name>/clear</command-name><command-message>clear</command-message><command-args></command-args>")} />);
    expect(screen.getByText("/clear")).toBeTruthy();
    expect(screen.queryByText(/command-name/)).toBeNull();
    expect(document.querySelector(".chat-message.is-command")).toBeTruthy();
  });

  it("只剩免责声明的那条整条不渲染", () => {
    const { container } = render(<Message item={userText("<local-command-caveat>Caveat: …</local-command-caveat>")} />);
    expect(container.innerHTML).toBe("");
  });

  it("命令输出收进可展开块", () => {
    render(<Message item={userText("<command-name>/cost</command-name><local-command-stdout>Total: $1.20</local-command-stdout>")} />);
    const details = screen.getByText("命令输出").closest("details");
    expect(details?.hasAttribute("open")).toBe(false);
    expect(details?.querySelector("pre")?.textContent).toBe("Total: $1.20");
  });

  it("普通用户消息照旧走气泡", () => {
    render(<Message item={userText("继续实现")} />);
    expect(screen.getByText("继续实现")).toBeTruthy();
    expect(document.querySelector(".chat-message.is-command")).toBeNull();
  });

  it("任务通知收成一行摘要（通知自带的 <summary>），展开才见全文", () => {
    render(<Message item={userText('<task-notification><task-id>t1</task-id><summary>Agent "/code-review" finished</summary><result>ok</result></task-notification>')} />);
    const details = screen.getByText("后台任务通知").closest("details");
    expect(details?.hasAttribute("open")).toBe(false);
    expect(details?.querySelector(".chat-tool-summary")?.textContent).toBe('Agent "/code-review" finished');
    // 全文在展开区，不再摊在气泡里。
    expect(document.querySelector(".chat-message .chat-text")).toBeNull();
  });
});

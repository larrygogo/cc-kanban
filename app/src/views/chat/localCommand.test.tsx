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
});

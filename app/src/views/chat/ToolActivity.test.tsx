import { afterEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { ToolActivity, ActivityGroup } from "./ToolActivity";
import type { ToolResultItem, ToolUseItem } from "./shared";

afterEach(cleanup);

const write = (content: string, lines: number, truncated = false): ToolUseItem => ({
  type: "tool_use", id: "t1", timestamp: null, name: "Write", summary: "src/a.ts",
  detail: { kind: "write", path: "src/a.ts", content, lines, truncated },
});
const done: ToolResultItem = { type: "tool_result", id: "r1", timestamp: null, tool_use_id: "t1", text: "File created", is_error: false };

/** 对话页的工具行像终端那样带详情：标题行有规模小标（「写入 N 行」），展开是带行号的
 *  正文预览，只露前十几行，余下折成「还有 N 行」；后端截过的正文展开到底要提示。 */
describe("ToolActivity 详情", () => {
  it("Write：标题行写行数，展开是带行号预览，长正文折成「还有 N 行」", () => {
    const body = Array.from({ length: 30 }, (_, i) => `const v${i} = ${i};`).join("\n");
    render(<ToolActivity item={write(body, 30)} result={done} />);
    expect(screen.getByText("写入 30 行")).toBeTruthy();
    // 有回执的行挂载时不自动展开；内容仍在 DOM 里（details 只是收起）。
    expect(document.querySelector("details.chat-tool")?.hasAttribute("open")).toBe(false);
    expect(document.querySelectorAll(".chat-tool-line").length).toBe(12);
    expect(screen.getByText("v0", { exact: false })).toBeTruthy();
    const more = screen.getByRole("button", { name: "还有 18 行 · 展开全部" });
    fireEvent.click(more);
    expect(document.querySelectorAll(".chat-tool-line").length).toBe(30);
    expect(screen.getByRole("button", { name: "收起预览" })).toBeTruthy();
  });

  it("Write：后端截断的正文展开到底时提示只保留了开头", () => {
    const body = Array.from({ length: 20 }, (_, i) => `line ${i}`).join("\n");
    render(<ToolActivity item={write(body, 2000, true)} result={done} />);
    expect(screen.getByText("写入 2000 行")).toBeTruthy();
    expect(screen.queryByText("内容过长，只保留了开头")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: /展开全部/ }));
    expect(screen.getByText("内容过长，只保留了开头")).toBeTruthy();
  });

  it("Edit：标题行是 -旧 +新 的行数，正文分删/增两段", () => {
    const item: ToolUseItem = {
      type: "tool_use", id: "t2", timestamp: null, name: "Edit", summary: "x.rs",
      detail: { kind: "edit", path: "x.rs", old: "let a = 1;", new: "let a = 1;\nlet b = 2;\nlet c = 3;", replace_all: true, truncated: false },
    };
    render(<ToolActivity item={item} result={{ ...done, tool_use_id: "t2" }} />);
    expect(screen.getByText("-1 +3")).toBeTruthy();
    expect(document.querySelectorAll(".chat-tool-code.is-del .chat-tool-line").length).toBe(1);
    expect(document.querySelectorAll(".chat-tool-code.is-add .chat-tool-line").length).toBe(3);
    expect(screen.getByText("全部替换")).toBeTruthy();
  });

  it("Bash：正在跑的调用挂载即展开，命令带 $ 记号，描述进标题行", () => {
    const item: ToolUseItem = {
      type: "tool_use", id: "t3", timestamp: null, name: "Bash", summary: "cargo test",
      detail: { kind: "command", command: "cargo test", description: "跑测试" },
    };
    render(<ToolActivity item={item} />);
    expect(document.querySelector("details.chat-tool")?.hasAttribute("open")).toBe(true);
    expect(screen.getByText("跑测试")).toBeTruthy();
    expect(document.querySelector(".chat-tool-ln")?.textContent).toBe("$");
    expect(screen.getByText("正在执行，结果尚未返回…")).toBeTruthy();
  });

  it("没有详情的工具行保持原样：只有摘要与回执", () => {
    const item: ToolUseItem = { type: "tool_use", id: "t4", timestamp: null, name: "TaskCreate", summary: "x" };
    render(<ToolActivity item={item} result={{ ...done, tool_use_id: "t4" }} />);
    expect(document.querySelector(".chat-tool-meta")).toBeNull();
    expect(document.querySelector(".chat-tool-code")).toBeNull();
    expect(document.querySelector("details.chat-tool")?.hasAttribute("open")).toBe(false);
  });

  it("活动组：挂载时有未回执的调用就默认展开，否则收起", () => {
    const { unmount } = render(<ActivityGroup className="chat-activity-group" defaultOpen><summary>a</summary></ActivityGroup>);
    expect(document.querySelector("details")?.hasAttribute("open")).toBe(true);
    unmount();
    render(<ActivityGroup className="chat-activity-group" defaultOpen={false}><summary>b</summary></ActivityGroup>);
    expect(document.querySelector("details")?.hasAttribute("open")).toBe(false);
  });
});

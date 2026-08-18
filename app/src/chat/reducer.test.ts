import { describe, expect, it } from "vitest";
import type { ChatItem } from "../api";
import { reduceChatEvents } from "./reducer";

const user = (id: string, text: string): ChatItem => ({
  type: "user_text",
  id,
  timestamp: null,
  text,
});

describe("reduceChatEvents", () => {
  it("merges assistant deltas across polling boundaries", () => {
    const first = reduceChatEvents([], [{
      type: "assistant_delta",
      id: "a1",
      timestamp: null,
      text: "正在",
    }], false);
    const second = reduceChatEvents(first, [{
      type: "assistant_delta",
      id: "a2",
      timestamp: null,
      text: "处理",
    }], false);
    expect(second).toEqual([{
      type: "assistant_text",
      id: "a1",
      timestamp: null,
      text: "正在处理",
    }]);
  });

  it("deduplicates adjacent equivalent semantic events and preserves the old reference", () => {
    const previous = [user("prompt", "继续")];
    const next = reduceChatEvents(previous, [user("append-message", "继续")], false);
    expect(next).toBe(previous);
  });

  it("keeps a genuinely repeated user message when timestamps differ", () => {
    const previous = [{ ...user("u1", "继续"), timestamp: "2026-08-18T01:00:00Z" }];
    const next = reduceChatEvents(previous, [{ ...user("u2", "继续"), timestamp: "2026-08-18T01:00:05Z" }], false);
    expect(next).toHaveLength(2);
  });

  it("deduplicates the dual-write only when text and timestamp both match", () => {
    const previous = [{ ...user("prompt", "继续"), timestamp: "2026-08-18T01:00:00Z" }];
    const next = reduceChatEvents(previous, [{ ...user("append-message", "继续"), timestamp: "2026-08-18T01:00:00Z" }], false);
    expect(next).toBe(previous);
  });

  it("does not deduplicate equal user text across another event", () => {
    const previous: ChatItem[] = [
      user("u1", "继续"),
      { type: "meta", id: "compact", timestamp: null, kind: "compacted" },
    ];
    expect(reduceChatEvents(previous, [user("u2", "继续")], false)).toHaveLength(3);
  });

  it("reset discards prior messages before reducing the new batch", () => {
    expect(reduceChatEvents([user("old", "旧")], [user("new", "新")], true))
      .toEqual([user("new", "新")]);
  });
});

import { collectSubagentReceipts } from "./reducer";

const toolResult = (
  id: string,
  toolUseId: string,
  subagent?: { running: number; completed: number; failed: number; task_id?: string },
): ChatItem => ({
  type: "tool_result",
  id,
  timestamp: null,
  tool_use_id: toolUseId,
  text: "",
  is_error: false,
  subagent: subagent ?? null,
});

describe("collectSubagentReceipts", () => {
  it("routes a TaskOutput outcome back to the launching delegation by task_id", () => {
    // 后台委派:启动回执(running,带 agentId)挂在委派本体上;主 agent 之后用 TaskOutput
    // 拉结果,CLI 便不再发 task-notification——结局回执挂在 TaskOutput 自己的调用上,
    // 只有 task_id 能归回。此前只认挂在委派上的回执,这些子任务永远显示「运行中」(实拍)。
    const items: ChatItem[] = [
      toolResult("r1", "toolu_agent", { running: 1, completed: 0, failed: 0, task_id: "a9b7" }),
      toolResult("r2", "toolu_taskoutput", { running: 0, completed: 1, failed: 0, task_id: "a9b7" }),
    ];
    const { outcomes, settledAt } = collectSubagentReceipts(items);
    expect(outcomes.get("toolu_agent")).toMatchObject({ completed: 1, running: 0 });
    // 归属后委派的「最后回执位置」同步推进(进度面板按它判断完成于哪个回合)。
    expect(settledAt.get("toolu_agent")).toBe(1);
  });

  it("records the final receipt timestamp under the delegation for duration display", () => {
    const items: ChatItem[] = [
      { ...toolResult("r1", "toolu_agent", { running: 1, completed: 0, failed: 0, task_id: "t9" }), timestamp: "2026-08-18T04:00:01Z" },
      { ...toolResult("r2", "toolu_taskoutput", { running: 0, completed: 1, failed: 0, task_id: "t9" }), timestamp: "2026-08-18T04:05:00Z" },
    ];
    const { finishedTs } = collectSubagentReceipts(items);
    // 子任务块的执行时长 = 委派时刻 → 最终回执时刻;路由归属后时间戳也要跟着落在委派名下。
    expect(finishedTs.get("toolu_agent")).toBe("2026-08-18T04:05:00Z");
  });

  it("timeout polls keep the delegation running until a final completed pull lands", () => {
    const items: ChatItem[] = [
      toolResult("r1", "toolu_agent", { running: 1, completed: 0, failed: 0, task_id: "t1" }),
      toolResult("r2", "toolu_poll1", { running: 1, completed: 0, failed: 0, task_id: "t1" }),
      toolResult("r3", "toolu_poll2", { running: 0, completed: 1, failed: 0, task_id: "t1" }),
    ];
    const { outcomes } = collectSubagentReceipts(items);
    expect(outcomes.get("toolu_agent")).toMatchObject({ completed: 1 });
    // 轮询回执不该在 TaskOutput 自己的调用名下留结局(那不是委派)。
    expect(outcomes.get("toolu_poll1")).toBeUndefined();
  });

  it("task-notification receipts without task_id keep binding by tool_use_id", () => {
    const items: ChatItem[] = [
      toolResult("r1", "toolu_agent", { running: 1, completed: 0, failed: 0, task_id: "t2" }),
      // 后端把 task-notification 合成为挂在原委派上的回执,不带 task_id。
      toolResult("r2", "toolu_agent", { running: 0, completed: 1, failed: 0 }),
    ];
    const { outcomes } = collectSubagentReceipts(items);
    expect(outcomes.get("toolu_agent")).toMatchObject({ completed: 1 });
  });

  it("an orphan TaskOutput receipt (launch receipt outside the loaded page) stays harmless", () => {
    const items: ChatItem[] = [
      toolResult("r1", "toolu_taskoutput", { running: 0, completed: 1, failed: 0, task_id: "gone" }),
    ];
    const { outcomes } = collectSubagentReceipts(items);
    // 找不到委派本体时只落在自己名下(没有对应的委派块,等于无操作),不得抛错或错挂。
    expect(outcomes.get("toolu_taskoutput")).toMatchObject({ completed: 1 });
  });
});

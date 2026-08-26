import { describe, expect, it, vi } from "vitest";
import type { ChatItem } from "../api";
import { buildTranscriptTodos, parseTaskUpdate, type TaskUpdateCache } from "./transcriptTodos";

const update = (id: string, payload: Record<string, unknown>): ChatItem => ({
  type: "tool_use", id, timestamp: null, name: "TaskUpdate",
  summary: JSON.stringify(payload), subagent: null,
} as ChatItem);
const create = (id: string, subject: string): ChatItem => ({
  type: "tool_use", id, timestamp: null, name: "TaskCreate", summary: subject, subagent: null,
} as ChatItem);
const receipt = (id: string, toolUseId: string, text: string): ChatItem => ({
  type: "tool_result", id, timestamp: null, tool_use_id: toolUseId, text, is_error: false, subagent: null,
} as ChatItem);
const user = (id: string): ChatItem => ({ type: "user_text", id, timestamp: null, text: "hi" } as ChatItem);

describe("parseTaskUpdate 的缓存", () => {
  it("同一条只 parse 一次,解析失败的结果同样被缓存", () => {
    const cache: TaskUpdateCache = new Map();
    const spy = vi.spyOn(JSON, "parse");
    parseTaskUpdate("t1", JSON.stringify({ taskId: 7, subject: "写文档" }), cache);
    parseTaskUpdate("t1", JSON.stringify({ taskId: 7, subject: "写文档" }), cache);
    parseTaskUpdate("t1", JSON.stringify({ taskId: 7, subject: "写文档" }), cache);
    expect(spy).toHaveBeenCalledTimes(1);
    // null(解析失败)是有效结果:用真值判命中会让这些条目每批重新 parse,正是要省的那部分。
    parseTaskUpdate("bad", "{截断的 JSON", cache);
    parseTaskUpdate("bad", "{截断的 JSON", cache);
    expect(spy).toHaveBeenCalledTimes(2);
    expect(cache.get("bad")).toBeNull();
    spy.mockRestore();
  });

  it("没有 taskId 视为无效;非字符串的 status/subject 不采纳", () => {
    const cache: TaskUpdateCache = new Map();
    expect(parseTaskUpdate("a", JSON.stringify({ subject: "无 id" }), cache)).toBeNull();
    expect(parseTaskUpdate("b", JSON.stringify({ taskId: 3, status: 9, subject: [] }), cache))
      .toEqual({ id: "3", status: undefined, subject: undefined });
    // 数字 id 归一成字符串,与 TaskCreate 回执里的 `Task #N` 对得上。
    expect(parseTaskUpdate("c", JSON.stringify({ taskId: 12, subject: "x" }), cache)?.id).toBe("12");
  });
});

describe("buildTranscriptTodos", () => {
  const cache = () => new Map() as TaskUpdateCache;

  it("Create 的编号来自回执文本,随后的 Update 按编号改写状态与标题", () => {
    const items = [
      create("c1", "写文档"),
      receipt("r1", "c1", "Task #1 created successfully"),
      update("u1", { taskId: 1, status: "in_progress" }),
      update("u2", { taskId: 1, subject: "写完文档" }),
    ];
    expect(buildTranscriptTodos(items, -1, cache()))
      .toEqual([{ content: "写完文档", status: "in_progress" }]);
  });

  it("status=deleted 把该条移除", () => {
    const items = [
      create("c1", "写文档"),
      receipt("r1", "c1", "Task #1 created successfully"),
      update("u1", { taskId: 1, status: "deleted" }),
    ];
    expect(buildTranscriptTodos(items, -1, cache())).toEqual([]);
  });

  it("错过 Create 但 Update 带标题时补建自愈(transcript 截段)", () => {
    const items = [update("u1", { taskId: 9, subject: "补建的任务", status: "in_progress" })];
    expect(buildTranscriptTodos(items, -1, cache()))
      .toEqual([{ content: "补建的任务", status: "in_progress" }]);
    // 只有状态没有标题时不补建——没有标题的行没法显示。
    expect(buildTranscriptTodos([update("u2", { taskId: 9, status: "completed" })], -1, cache()))
      .toEqual([]);
  });

  it("已完成项只留当前回合内完成的,未完成的恒显示", () => {
    const items = [
      update("u1", { taskId: 1, subject: "老的已完成", status: "completed" }),
      update("u2", { taskId: 2, subject: "老的没做完", status: "in_progress" }),
      user("me"),
      update("u3", { taskId: 3, subject: "新完成的", status: "completed" }),
    ];
    const lastUserIdx = items.findIndex((item) => item.type === "user_text");
    expect(buildTranscriptTodos(items, lastUserIdx, cache())).toEqual([
      { content: "老的没做完", status: "in_progress" },
      { content: "新完成的", status: "completed" },
    ]);
  });

  it("复用缓存与新建缓存的结果一致(缓存不改变语义)", () => {
    // 缓存跨批复用是这次优化的全部内容,它绝不能让结果漂移。
    const items = [
      create("c1", "甲"),
      receipt("r1", "c1", "Task #1 created successfully"),
      update("u1", { taskId: 1, status: "in_progress" }),
      update("u2", { taskId: 2, subject: "乙", status: "completed" }),
      update("bad", "{坏的" as unknown as Record<string, unknown>),
    ];
    const shared = cache();
    const first = buildTranscriptTodos(items, -1, shared);
    // 再跑两批(模拟流式刷新),结果必须一字不差。
    expect(buildTranscriptTodos(items, -1, shared)).toEqual(first);
    expect(buildTranscriptTodos(items, -1, shared)).toEqual(first);
    expect(first).toEqual(buildTranscriptTodos(items, -1, cache()));
  });
});

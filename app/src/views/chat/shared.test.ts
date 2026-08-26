import { describe, expect, it } from "vitest";
import type { ChatItem } from "../../api";
import { isSubagentDelegation, type ToolUseItem } from "./shared";

const toolUse = (id: string, name: string, subagent?: ToolUseItem["subagent"]): ToolUseItem => ({
  type: "tool_use",
  id,
  timestamp: null,
  name,
  summary: "",
  subagent: subagent ?? null,
});

const outcome = { running: 1, completed: 0, failed: 0, task_id: null };

describe("isSubagentDelegation", () => {
  it("认解析时就识别出的委派(Agent),与有没有回执无关", () => {
    const item = toolUse("toolu_a", "Agent", { description: "审查 PR", agent_type: "general-purpose", count: 1 });
    expect(isSubagentDelegation(item, new Map())).toBe(true);
  });

  it("认 forked skill:委派本体是 Skill 调用、subagent 为空,靠回执的结局统计反认", () => {
    // /code-review 这类 skill 是 fork 出去跑的:主链上只有 Skill 调用,fork 与否在调用
    // 参数里看不出来,只有回执写着 (forked execution)。不认它 = 整场审查的子任务全不可见。
    const item = toolUse("toolu_skill", "Skill");
    expect(isSubagentDelegation(item, new Map())).toBe(false);
    expect(isSubagentDelegation(item, new Map([["toolu_skill", outcome]]))).toBe(true);
  });

  it("不认 TaskOutput:它的结局归不到原委派时会落在自己头上,那不是一次委派", () => {
    // 原委派滚出已加载窗口时,collectSubagentReceipts 会把结局记在 TaskOutput 自己的
    // id 上。若按「凡有结局统计者皆委派」放行,对话流里会凭空多出一个子任务块。
    const item = toolUse("toolu_taskoutput", "TaskOutput");
    expect(isSubagentDelegation(item, new Map([["toolu_taskoutput", outcome]]))).toBe(false);
  });

  it("普通 Skill 调用(未 fork,无回执结局)不算委派", () => {
    expect(isSubagentDelegation(toolUse("toolu_inline", "Skill"), new Map())).toBe(false);
  });
});

// 类型层面的守卫:helper 收 ToolUseItem,调用方负责收窄。这里只确认导出的类型能对上。
const _typecheck: ChatItem = toolUse("toolu_x", "Bash");
void _typecheck;

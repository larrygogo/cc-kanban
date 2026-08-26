import { type ChatItem } from "../../api";
import type { Dict } from "../../i18n/zh";

export type ToolUseItem = Extract<ChatItem, { type: "tool_use" }>;
export type ToolResultItem = Extract<ChatItem, { type: "tool_result" }>;

/// 这条工具调用是不是一次子任务委派。对话流的子任务块与标题栏进度面板共用同一条判据
/// ——两处一旦分叉，就会出现「面板里有、对话流里没有」这类自相矛盾的画面。
///
/// 两个来源：
/// 1. **解析时就认出的**(claude 的 Agent/Task、kimi 的 Agent/AgentSwarm)：`subagent` 现成；
/// 2. **forked skill**(claude 的 `/code-review` 等)：主链上只有一条 `Skill` 调用，fork 与否
///    在调用参数里根本看不出来，只有回执写着 `(forked execution)`。后端据此给**回执**挂了
///    结局统计(见 forked_skill_outcome)，这里凭「Skill + 有结局统计」反认委派本体——
///    它的 `subagent` 恒为空，标题退回 `item.summary`(即 `/code-review 1692 高强度`)。
///
/// 第 2 条刻意限定 `Skill` 而不是「凡有结局统计者」：`TaskOutput` 的结局按 task_id 归给
/// 原委派，但原委派若已滚出已加载窗口，归属会落到 TaskOutput 自己头上——那不是一次委派，
/// 放行会在对话流里凭空多出一个子任务块。
/// 刻意**不写成类型谓词**:谓词会把否定分支也一并收窄(`tool_use` 但不是委派的普通工具
/// 调用会被 TS 判成不可能),调用方随后对 `item.type` 的判断整片失效。类型收窄交给调用方
/// 自己的 `item.type === "tool_use"`。
export function isSubagentDelegation(
  item: ToolUseItem,
  outcomes: Map<string, unknown>,
): boolean {
  return !!item.subagent || (item.name === "Skill" && outcomes.has(item.id));
}

/// 子任务执行时长的短格式:秒级起报,超一小时丢掉秒(读的是量级,不是秒表)。
/// 对话流的子任务块与标题栏进度面板共用。
export function durationText(ms: number, t: Dict): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  if (total < 60) return t.chat.subagentDurationSec(total);
  const minutes = Math.floor(total / 60);
  if (minutes < 60) return t.chat.subagentDurationMin(minutes, total % 60);
  return t.chat.subagentDurationHour(Math.floor(minutes / 60), minutes % 60);
}

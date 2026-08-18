import { type ChatItem } from "../../api";
import type { Dict } from "../../i18n/zh";

export type ToolUseItem = Extract<ChatItem, { type: "tool_use" }>;
export type ToolResultItem = Extract<ChatItem, { type: "tool_result" }>;

/// 子任务执行时长的短格式:秒级起报,超一小时丢掉秒(读的是量级,不是秒表)。
/// 对话流的子任务块与标题栏进度面板共用。
export function durationText(ms: number, t: Dict): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  if (total < 60) return t.chat.subagentDurationSec(total);
  const minutes = Math.floor(total / 60);
  if (minutes < 60) return t.chat.subagentDurationMin(minutes, total % 60);
  return t.chat.subagentDurationHour(Math.floor(minutes / 60), minutes % 60);
}

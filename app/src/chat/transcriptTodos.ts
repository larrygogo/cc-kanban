// claude 新版任务列表（TaskCreate/TaskUpdate）从时间线累积重建。
//
// 为什么住在这里而不是留在 ChatWindow 里：它是这条链路上**唯一真正昂贵**的扫描。
// 实测（30000 条、其中三成是顶到 800 字截断上限的 TaskUpdate）：带 JSON.parse 的一遍
// 要 5.5~6.4 ms，而同一批数据上 displayItems/lastUserIdx/collectSubagentReceipts/
// userHistory 全部在 1 ms 以下。流式期间 200ms 一批，6 ms 一次就吃掉小半帧。
//
// 症结是 parse 而不是遍历：条目不可变（reducer 写时复制），同一 id 的 summary 永不变，
// 每条 TaskUpdate 却在每一批里被重新 parse 一次。按 id 缓存解析结果后同样的数据降到
// 0.7~1.6 ms（6~8 倍），与其余扫描同量级。
//
// 刻意**不**改成增量累积：那要动对话页最核心的数据流，而这块注释里记着反复出过
// 「静默不更新」的事故；再省下的约 2 ms/批换不来这个风险。
import type { ChatItem } from "../api";

/// TaskUpdate 摘要里我们真正用到的三个字段。null = 解析不出或没有 taskId。
export type ParsedTaskUpdate = { id: string; status?: string; subject?: string } | null;

/// 解析缓存：key = ChatItem.id（由 transcript 行 uuid 派生，内容确定），value = 解析结果。
export type TaskUpdateCache = Map<string, ParsedTaskUpdate>;

/// 解析一条 TaskUpdate 的摘要（Rust 侧精简出的 JSON），带缓存。
export function parseTaskUpdate(
  itemId: string,
  summary: string,
  cache: TaskUpdateCache,
): ParsedTaskUpdate {
  const hit = cache.get(itemId);
  // 注意用 undefined 判命中而不是真值：null 是**有效**的缓存结果（解析失败），
  // 用真值判会让这些条目每批都重新 parse——正是要省掉的那部分。
  if (hit !== undefined) return hit;
  let parsed: ParsedTaskUpdate = null;
  try {
    const input = JSON.parse(summary) as { taskId?: unknown; status?: unknown; subject?: unknown };
    const id = input.taskId == null ? "" : String(input.taskId);
    if (id) {
      parsed = {
        id,
        status: typeof input.status === "string" ? input.status : undefined,
        subject: typeof input.subject === "string" ? input.subject : undefined,
      };
    }
  } catch {
    // 摘要被截断等罕见形态：这条跳过，状态晚一拍好过整表消失。
    parsed = null;
  }
  cache.set(itemId, parsed);
  return parsed;
}

export type TranscriptTodo = { content: string; status: string };

/// 从时间线重建任务列表。
///
/// 编号在 TaskCreate **回执**文本 `Task #N created successfully` 里；TaskUpdate 的摘要是
/// Rust 侧精简出的 JSON。已完成项只显示 `lastUserIdx`（最后一条用户消息的位置）之后完成的
/// ——任务列表跨回合只增不减，否则长会话会把历史旧账全堆进面板；未完成的恒显示，无论多老。
export function buildTranscriptTodos(
  items: ChatItem[],
  lastUserIdx: number,
  cache: TaskUpdateCache,
): TranscriptTodo[] {
  const byId = new Map<string, { content: string; status: string; touch: number }>();
  const pendingCreates = new Map<string, string>();
  items.forEach((item, index) => {
    if (item.type === "tool_use" && item.name === "TaskCreate") {
      pendingCreates.set(item.id, item.summary);
      return;
    }
    if (item.type === "tool_use" && item.name === "TaskUpdate") {
      const parsed = parseTaskUpdate(item.id, item.summary, cache);
      if (!parsed) return;
      if (parsed.status === "deleted") {
        byId.delete(parsed.id);
        return;
      }
      const row = byId.get(parsed.id);
      if (row) {
        if (parsed.status !== undefined) row.status = parsed.status;
        if (parsed.subject !== undefined) row.content = parsed.subject;
        row.touch = index;
      } else if (parsed.subject !== undefined) {
        // 错过 Create（如 transcript 截段）但这次带了标题 → 补建自愈。
        byId.set(parsed.id, { content: parsed.subject, status: parsed.status ?? "pending", touch: index });
      }
      return;
    }
    if (item.type === "tool_result" && item.tool_use_id && pendingCreates.has(item.tool_use_id)) {
      const numbered = /Task #(\d+)/.exec(item.text ?? "");
      if (numbered) {
        byId.set(numbered[1], { content: pendingCreates.get(item.tool_use_id)!, status: "pending", touch: index });
      }
      pendingCreates.delete(item.tool_use_id);
    }
  });
  return [...byId.values()]
    .filter((row) => row.status !== "completed" || row.touch > lastUserIdx)
    .map(({ content, status }) => ({ content, status }));
}

import { memo } from "react";
import { type ChatItem } from "../../api";
import { useT } from "../../i18n";
import type { Dict } from "../../i18n/zh";

/** 日期分隔条文案：今天/昨天沿用侧栏日期分组的同一对键，更早按界面语言格式化。 */
function dayLabel(date: Date, t: Dict): string {
  const today = new Date();
  today.setHours(0, 0, 0, 0);
  const day = new Date(date);
  day.setHours(0, 0, 0, 0);
  const diffDays = Math.round((today.getTime() - day.getTime()) / 86_400_000);
  if (diffDays === 0) return t.chat.dateToday;
  if (diffDays === 1) return t.chat.dateYesterday;
  return date.toLocaleDateString(t.locale);
}
import { imageOnlyPaths, Message, UserImageGroup } from "./Message";
import { SubagentBlock } from "./SubagentBlock";
import { friendlyToolName, ToolActivity } from "./ToolActivity";
import { type ToolResultItem, type ToolUseItem } from "./shared";

/// items 引用不变就不重算：稳态下 650ms 一轮的 history 刷新会让父组件重渲染，
/// 但 items 往往原样不动（reduceChatEvents 无新消息时返回同一引用）。没有这层
/// memo 时，每一轮都要重跑下面的分组循环、重建全部 JSX——长会话上千条时很贵。
export const Transcript = memo(function Transcript({ sessionId, items }: { sessionId: number; items: ChatItem[] }) {
  const t = useT();
  // 委派的结局写在**主链的工具回执**上，而回执往往排在委派之后若干条。先建一张
  // tool_use_id → 结局 的索引，折叠态的徽标才有数据可用。
  const outcomes = new Map<string, NonNullable<ToolResultItem["subagent"]>>();
  // 还要记下「哪些委派已经有回执了」：一批 fan-out 子任务的结局要等整批结束才写进主链，
  // 而**跑着的时候**恰恰是最该显示进度的时刻。没有回执 = 派出去了还没回来 = 在跑。
  const settled = new Set<string>();
  for (const item of items) {
    if (item.type !== "tool_result" || !item.tool_use_id) continue;
    settled.add(item.tool_use_id);
    if (item.subagent) outcomes.set(item.tool_use_id, item.subagent);
  }
  const blocks: JSX.Element[] = [];
  // 日期分隔条：跨天处插一条（含首个带时间的块，IM 惯例——每天的开头都有标签）。
  // timestamp 为 null 的条目（乐观回显/部分 provider 不带时间）不参与也不重置游标，
  // 混在带时间的消息中间不会催生重复的分隔条。
  let lastDay: string | null = null;
  const pushDaySep = (item: ChatItem) => {
    if (!item.timestamp) return;
    const date = new Date(item.timestamp);
    if (Number.isNaN(date.getTime())) return;
    const key = date.toDateString();
    if (key === lastDay) return;
    lastDay = key;
    blocks.push(
      <div className="chat-day-sep" role="separator" key={`day-${item.id}`}>
        <span>{dayLabel(date, t)}</span>
      </div>,
    );
  };
  for (let index = 0; index < items.length;) {
    const item = items[index];
    pushDaySep(item);
    // 连续的纯图片用户消息合成一行：Claude Code 把一次多图粘贴记成连续多条独立消息，
    // 逐条渲染是竖着摞一列大图；用户视角那是「一次发的几张图」，该排在一起。
    if (item.type === "user_text") {
      const paths = imageOnlyPaths(item.text);
      if (paths) {
        const merged = [...paths];
        let next = index + 1;
        while (next < items.length) {
          const candidate = items[next];
          if (candidate.type !== "user_text") break;
          const more = imageOnlyPaths(candidate.text);
          if (!more) break;
          merged.push(...more);
          next += 1;
        }
        blocks.push(<UserImageGroup key={item.id} paths={merged} />);
        index = next;
        continue;
      }
    }
    // 子任务委派不并进「N 次工具操作」那一坨：它代表一整段独立工作，值得单独一行，
    // 且展开的是子任务时间线而不是一段参数文本。
    if (item.type === "tool_use" && item.subagent) {
      blocks.push(<SubagentBlock
        key={item.id}
        sessionId={sessionId}
        item={item}
        outcome={outcomes.get(item.id)}
        settled={settled.has(item.id)}
      />);
      index += 1;
      continue;
    }
    if (item.type === "tool_use" || item.type === "tool_result") {
      const tools: Array<ToolUseItem | ToolResultItem> = [];
      while (index < items.length) {
        const candidate = items[index];
        // 子任务在上面已单独成块；遇到它就断组，别把它吞进这坨里。
        if (candidate.type === "tool_use" && candidate.subagent) break;
        if (candidate.type !== "tool_use" && candidate.type !== "tool_result") break;
        tools.push(candidate);
        index += 1;
      }
      const results = new Map<string, ToolResultItem>();
      for (const tool of tools) {
        if (tool.type === "tool_result" && tool.tool_use_id) results.set(tool.tool_use_id, tool);
      }
      const consumed = new Set<string>();
      const callCount = tools.filter((tool) => tool.type === "tool_use").length;
      const failureCount = tools.filter((tool) => tool.type === "tool_result" && tool.is_error).length;
      // 无回执的调用 = 还在跑：收起态此前完全看不出这组是「跑完了」还是「卡住了」，
      // 只能展开逐条找。摘要上复用单条工具的同一枚跳动点（.chat-tool-pending）。
      const pendingCount = tools.filter((tool) => tool.type === "tool_use" && !results.has(tool.id)).length;
      // 摘要直接说做了什么（「运行终端 ×2 · 读取文件」，Claude Code 的
      // 「Ran 2 commands, read a file」同款），不再是「执行了 N 次工具调用」+
      // 重复的种类列表——同一行两段文字说的是同一件事。种类只列前 3 个。
      const kindCounts = new Map<string, number>();
      for (const tool of tools) {
        if (tool.type !== "tool_use") continue;
        const name = friendlyToolName(tool.name, t);
        kindCounts.set(name, (kindCounts.get(name) ?? 0) + 1);
      }
      const kinds = [...kindCounts.entries()];
      const label = kinds.slice(0, 3).map(([name, count]) => (count > 1 ? `${name} ×${count}` : name)).join(" · ")
        + (kinds.length > 3 ? " …" : "");
      blocks.push(<details className={"chat-activity-group" + (failureCount ? " is-error" : "")} key={`tools-${tools[0].id}`}>
        <summary className="chat-activity-summary">
          <span className="chat-tool-icon"><svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8"><path d="M4 17l6-6-6-6M12 19h8" /></svg></span>
          <span className="chat-activity-kinds">{label || t.chat.toolActivities(callCount || tools.length)}</span>
          {pendingCount > 0 && <span className="chat-tool-pending" aria-label={t.chat.toolRunning}><i /><i /><i /></span>}
          {failureCount > 0 && <span className="chat-activity-errors">{t.chat.toolFailures(failureCount)}</span>}
          <span className="chat-tool-chevron">›</span>
        </summary>
        <div className="chat-activity-items">{tools.map((tool) => {
          if (tool.type === "tool_use") {
            const result = results.get(tool.id);
            if (result) consumed.add(result.id);
            return <ToolActivity key={tool.id} item={tool} result={result} />;
          }
          return consumed.has(tool.id) ? null : <Message key={tool.id} item={tool} />;
        })}</div>
      </details>);
      continue;
    }
    blocks.push(<Message key={item.id} item={item} />);
    index += 1;
  }
  return <>{blocks}</>;
});

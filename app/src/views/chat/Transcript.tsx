import { memo, useRef } from "react";
import { type ChatItem } from "../../api";
import { collectSubagentReceipts } from "../../chat/reducer";
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
import { isSubagentDelegation, type ToolResultItem, type ToolUseItem } from "./shared";

type SubagentOutcomeInfo = NonNullable<ToolResultItem["subagent"]>;

/// 增量缓存的块记录。start = 该块首条 item 的下标;lastDayBefore = 进入该块时的日期
/// 分隔游标(复算从此块起步时要还原它);subagentId = 子任务块对应的 tool_use id——
/// 该块的徽标读的是整个 items 里的回执,复用前必须核对 outcomes/settled 没变;
/// nodes = 该块产出的元素(块本体,可能前置一条日期分隔条)。
type BlockRec = {
  start: number;
  lastDayBefore: string | null;
  subagentId: string | null;
  nodes: JSX.Element[];
};

type BlockCache = {
  sessionId: number;
  t: Dict;
  items: ChatItem[];
  blocks: BlockRec[];
  outcomes: Map<string, SubagentOutcomeInfo>;
  settledAt: Map<string, number>;
};

/// items 引用不变就不重算：稳态下历史轮询每轮都让父组件重渲染，
/// 但 items 往往原样不动（reduceChatEvents 无新消息时返回同一引用）。没有这层
/// memo 时，每一轮都要重跑下面的分组循环、重建全部 JSX——长会话上千条时很贵。
///
/// 流式期间 items 每批(200ms 的 pty-output 合流)都换新引用,memo 帮不上——此前每批
/// 都从头全量重建整棵块数组,长会话是 O(n²)。现在按块做增量缓存:reducer 的写时复制
/// 保证未变条目同引用,据此找到首个变化下标,之前的块原样复用,只重算尾部。
export const Transcript = memo(function Transcript({ sessionId, items }: { sessionId: number; items: ChatItem[] }) {
  const t = useT();
  // 委派的结局写在**主链的工具回执**上，而回执往往排在委派之后若干条（TaskOutput 拉取
  // 的结局甚至挂在别的调用上，靠 task_id 归回原委派）。关联规则收敛在
  // collectSubagentReceipts——与标题栏进度面板同一份。没有回执 = 派出去了还没回来 = 在跑。
  // 每轮全量重扫:O(n) 的纯 Map 构建很便宜,贵的是按块生成 JSX——那部分走下面的缓存。
  const { outcomes, settledAt, finishedTs } = collectSubagentReceipts(items);
  const cacheRef = useRef<BlockCache | null>(null);
  const cache = cacheRef.current;
  let reuse: BlockRec[] = [];
  if (cache && cache.sessionId === sessionId && cache.t === t && cache.blocks.length > 0) {
    const common = Math.min(cache.items.length, items.length);
    let firstChanged = 0;
    while (firstChanged < common && cache.items[firstChanged] === items[firstChanged]) firstChanged += 1;
    // 最后一个缓存块永不复用:纯追加时它可能被新条目延长(工具组/连续图片组会吸收
    // 紧随其后的同类条目),必须从它的起点重算。
    let keep = 0;
    while (keep < cache.blocks.length - 1) {
      const block = cache.blocks[keep];
      const end = cache.blocks[keep + 1].start;
      if (end > firstChanged) break;
      const id = block.subagentId;
      // 尾部新到的回执会改写早先子任务块的结局徽标;outcomes 的值来自未变条目时是
      // 同一个对象引用,直接比引用即可。
      if (id !== null && (outcomes.get(id) !== cache.outcomes.get(id) || settledAt.has(id) !== cache.settledAt.has(id))) break;
      keep += 1;
    }
    reuse = cache.blocks.slice(0, keep);
  }
  const blocks: BlockRec[] = [...reuse];
  const resume = reuse.length > 0 ? cache!.blocks[reuse.length] : null;
  // 日期分隔条：跨天处插一条（含首个带时间的块，IM 惯例——每天的开头都有标签）。
  // timestamp 为 null 的条目（乐观回显/部分 provider 不带时间）不参与也不重置游标，
  // 混在带时间的消息中间不会催生重复的分隔条。
  let lastDay: string | null = resume ? resume.lastDayBefore : null;
  const pushDaySep = (record: BlockRec, item: ChatItem) => {
    if (!item.timestamp) return;
    const date = new Date(item.timestamp);
    if (Number.isNaN(date.getTime())) return;
    const key = date.toDateString();
    if (key === lastDay) return;
    lastDay = key;
    record.nodes.push(
      <div className="chat-day-sep" role="separator" key={`day-${item.id}`}>
        <span>{dayLabel(date, t)}</span>
      </div>,
    );
  };
  for (let index = resume ? resume.start : 0; index < items.length;) {
    const item = items[index];
    const record: BlockRec = { start: index, lastDayBefore: lastDay, subagentId: null, nodes: [] };
    blocks.push(record);
    pushDaySep(record, item);
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
        record.nodes.push(<UserImageGroup key={item.id} paths={merged} />);
        index = next;
        continue;
      }
    }
    // 子任务委派不并进「N 次工具操作」那一坨：它代表一整段独立工作，值得单独一行，
    // 且展开的是子任务时间线而不是一段参数文本。
    if (item.type === "tool_use" && isSubagentDelegation(item, outcomes)) {
      record.subagentId = item.id;
      record.nodes.push(<SubagentBlock
        key={item.id}
        sessionId={sessionId}
        item={item}
        outcome={outcomes.get(item.id)}
        settled={settledAt.has(item.id)}
        finishedAt={finishedTs.get(item.id) ?? null}
      />);
      index += 1;
      continue;
    }
    if (item.type === "tool_use" || item.type === "tool_result") {
      const tools: Array<ToolUseItem | ToolResultItem> = [];
      while (index < items.length) {
        const candidate = items[index];
        // 子任务在上面已单独成块；遇到它就断组，别把它吞进这坨里。
        if (candidate.type === "tool_use" && isSubagentDelegation(candidate, outcomes)) break;
        if (candidate.type !== "tool_use" && candidate.type !== "tool_result") break;
        tools.push(candidate);
        index += 1;
      }
      const results = new Map<string, ToolResultItem>();
      for (const tool of tools) {
        if (tool.type === "tool_result" && tool.tool_use_id) results.set(tool.tool_use_id, tool);
      }
      // 会被某次调用认领的回执先一次性算好。曾在下面的 map 里边填边查:回执排在其
      // 调用之前(乱序落盘)时,先被当成独立消息渲染一遍,又被 ToolActivity 当结果
      // 渲染一遍——同一条回执上屏两次。
      const consumed = new Set<string>();
      for (const tool of tools) {
        if (tool.type !== "tool_use") continue;
        const result = results.get(tool.id);
        if (result) consumed.add(result.id);
      }
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
      record.nodes.push(<details className={"chat-activity-group" + (failureCount ? " is-error" : "")} key={`tools-${tools[0].id}`}>
        <summary className="chat-activity-summary">
          <span className="chat-tool-icon"><svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8"><path d="M4 17l6-6-6-6M12 19h8" /></svg></span>
          <span className="chat-activity-kinds">{label || t.chat.toolActivities(callCount || tools.length)}</span>
          {pendingCount > 0 && <span className="chat-tool-pending" aria-label={t.chat.toolRunning}><i /><i /><i /></span>}
          {failureCount > 0 && <span className="chat-activity-errors">{t.chat.toolFailures(failureCount)}</span>}
          <span className="chat-tool-chevron">›</span>
        </summary>
        <div className="chat-activity-items">{tools.map((tool) => {
          if (tool.type === "tool_use") {
            return <ToolActivity key={tool.id} item={tool} result={results.get(tool.id)} />;
          }
          return consumed.has(tool.id) ? null : <Message key={tool.id} item={tool} />;
        })}</div>
      </details>);
      continue;
    }
    record.nodes.push(<Message key={item.id} item={item} />);
    index += 1;
  }
  cacheRef.current = { sessionId, t, items, blocks, outcomes, settledAt };
  return <>{blocks.flatMap((block) => block.nodes)}</>;
});

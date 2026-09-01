import type { ChatItem } from "../api";
import type { SubagentOutcome } from "../generated/contracts/SubagentOutcome";

/**
 * 把 Provider 无关的 transcript 事件折叠成可直接渲染的消息序列。
 *
 * 这里是 delta 合并与边界去重的唯一位置。React 组件只渲染结果；各 Provider parser 只负责
 * 准确描述日志事实。没有实际变化时返回旧引用，让稳定轮询跳过整棵消息树重渲染。
 */
export function reduceChatEvents(
  previous: ChatItem[],
  incoming: ChatItem[],
  reset: boolean,
): ChatItem[] {
  let next = reset ? [] : previous;
  let changed = reset;
  const writable = () => {
    if (!changed) {
      next = [...previous];
      changed = true;
    }
    return next;
  };

  for (const item of incoming) {
    const last = next[next.length - 1];
    if (item.type !== "assistant_delta" && item.type !== "reasoning_delta") {
      // 同一语义事件可能被 Provider 同时写进两个兼容日志入口；只消除相邻且完全等价的记录，
      // 不跨工具活动或其它消息猜测重复，避免吞掉用户确实连续发送的相同文本。
      // 「等价」= 同文 + 同刻：kimi 的 turn.prompt 与 context.append_message 双写是同一
      // 消息的两次落盘，time 各自取落盘时刻——实测相邻两行的戳**最多差 1ms**（不是同戳，
      // 全等判定会被 1ms 差击穿，消息双显）；用户连发两条相同消息的间隔必然远大于此。
      // 故时间维度用 ≤2s 容差窗口：双写消除、真人连发不吞（5s 差用例钉在 reducer.test）。
      if (item.type === "user_text" && last?.type === "user_text" && last.text === item.text
        && (last.timestamp === item.timestamp
          || (last.timestamp != null && item.timestamp != null
            && Math.abs(Date.parse(item.timestamp) - Date.parse(last.timestamp)) <= 2_000))) continue;
      if (item.type === "reasoning" && last?.type === "reasoning" && last.text === item.text) continue;
      writable().push(item);
      continue;
    }

    const target = item.type === "assistant_delta" ? "assistant_text" : "reasoning";
    if (last?.type === target) {
      const items = writable();
      items[items.length - 1] = { ...last, text: last.text + item.text };
    } else {
      writable().push({ type: target, id: item.id, timestamp: item.timestamp, text: item.text });
    }
  }
  return next;
}

/**
 * 主链工具回执的索引：settledAt = 各 tool_use 最后一次回执的位置；outcomes = 子任务委派
 * id → 结局统计。Transcript 的折叠徽标与标题栏进度面板共用（同一份关联规则）。
 *
 * 后台委派的结局不一定挂在委派自己的回执上：除 task-notification（后端已合成带
 * tool-use-id 的回执）外，主 agent 还可能用 TaskOutput 主动拉取——CLI 拉过就不再发
 * 通知，那条回执挂在 TaskOutput 自己的调用上，只有 task_id（= 启动回执里的 agentId）
 * 能关联。规则：同 task_id 首见的回执就是启动回执（它挂在委派本体上），后到的一律
 * 归回它；归属时委派的 settledAt 同步推进（「完成于当前回合吗」要看最终那次回执）。
 */
export function collectSubagentReceipts(items: ChatItem[]): {
  outcomes: Map<string, SubagentOutcome>;
  settledAt: Map<string, number>;
  /** 各 tool_use 最后一次回执的时间戳（路由归属后同步刷新）。子任务块用
   *  「委派时刻 → 最终回执时刻」算执行时长；无时间戳的 provider 为 null。 */
  finishedTs: Map<string, string | null>;
} {
  const outcomes = new Map<string, SubagentOutcome>();
  const settledAt = new Map<string, number>();
  const finishedTs = new Map<string, string | null>();
  const taskOwner = new Map<string, string>();
  items.forEach((item, index) => {
    if (item.type !== "tool_result" || !item.tool_use_id) return;
    settledAt.set(item.tool_use_id, index);
    finishedTs.set(item.tool_use_id, item.timestamp ?? null);
    const sub = item.subagent;
    if (!sub) return;
    let owner = item.tool_use_id;
    if (sub.task_id) {
      const seen = taskOwner.get(sub.task_id);
      if (seen === undefined) taskOwner.set(sub.task_id, owner);
      else owner = seen;
    }
    outcomes.set(owner, sub);
    if (owner !== item.tool_use_id) {
      settledAt.set(owner, index);
      finishedTs.set(owner, item.timestamp ?? null);
    }
  });
  return { outcomes, settledAt, finishedTs };
}

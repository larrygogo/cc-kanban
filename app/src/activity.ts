// 会话「活动态」的唯一判定。刻意**不依赖任何模块**：它同时被 api.ts（对话窗侧栏的
// sessionTone）与 views/sticker/helpers.ts（看板卡片状态环与 tab 归属）消费，挂在 api.ts
// 上会让每个 mock 了 ./api 的测试都得手动补一个 export，也把一段纯逻辑绑上了 IPC 层。

/** 会话压缩（/compact）进行期间后端写进 sessions.current_activity 的哨兵值——
 *  状态记号不是活动文本，前端一律映射成本地化「正在压缩上下文…」（t.chat.compacting），
 *  原值不上屏、不进 tooltip。后端出处：`app/src-tauri/crates/meowo-reporter/src/dispatch.rs`
 *  的 PreCompact 臂。 */
export const COMPACTING_ACTIVITY = "__meowo_compacting__";

/** 连接中会话的**活动态阶梯**——侧栏状态点(sessionTone)、看板卡片状态环与 tab 归属
 *  (views/sticker/helpers.ts 的 cardTone/match)、后端 `session_query.rs` 的 tab_class,
 *  三处共用这一个地基。
 *
 *  曾经是**三套**:后端与看板对齐过一次,侧栏这份漏在外面——它完全不看 screen_state,
 *  于是一条 DB status=running、屏幕却回退 idle 的会话在侧栏算「运行中」、在贴纸不算,
 *  同一时刻左边写 3、右边写 2(实拍)。判定必须住在一处。
 *
 *  顺序与后端 tab_class **逐字对齐**:审批/屏幕 blocked > ended 的残留 idle 屏 >
 *  屏幕 working/idle > DB status。返回 null = 连着但阶梯说不出在干嘛,由调用方兜底。 */
export function activityTone(input: {
  status?: string;
  pendingReview?: unknown;
  screenState?: string | null;
  busySubagents?: number;
}): "pending" | "running" | "waiting" | null {
  const { status, pendingReview, screenState, busySubagents } = input;
  if (pendingReview || screenState === "blocked") return "pending";
  // 已 ended 的会话,屏幕上留着的最后一屏不能替它抢答「等你」:idle 是「一条规则都没
  // 命中」的回退值,不是证据。托管 PTY 没回收就一直算在线,于是一个 19 秒的空会话顶着
  // 黄环在等待区躺了 12 分钟(实拍)。恢复窗口期靠下面有可见证据的 working 仍然成立。
  if (status === "ended" && screenState === "idle") return null;
  if (screenState === "working") return "running";
  // 屏幕 idle / DB waiting 只说明**主回合**停了——后台子任务还在跑时是「运行中」,
  // 不该催人(与后端 tab_class 的 background_busy 同口径,原料同为 busy_subagents)。
  const busy = (busySubagents ?? 0) > 0;
  if (screenState === "idle") return busy ? "running" : "waiting";
  if (status === "running") return "running";
  if (status === "waiting") return busy ? "running" : "waiting";
  return null;
}

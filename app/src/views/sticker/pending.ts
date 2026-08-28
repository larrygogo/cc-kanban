// 「正在启动」占位卡（pending PTY，负 id）的前端对账。
//
// 后端把 pending 启动合成负 id 占位项合入看板首页查询，并在 claim 认领（真实会话行
// 已由 hook 落库）或启动失败/PTY 退出时摘除。理论上同一次刷新里「占位消失」与「真卡
// 出现」同时发生；但首页增量修补（B-6 mergeTail）会把上一轮的占位行留在尾部、刷新
// 也可能乱序——所以前端再按 provider+cwd 对一次账：真实会话出现时撤掉占位卡，
// 后端迟迟不再上报（超过 HOLD）的占位也撤，绝不留僵尸卡。
import type { Item } from "./types";

/** 占位卡对账用的目录归一 key：与后端 session_query 的 cwd_key 同口径
 * （反斜杠→斜杠、去尾斜杠、不分大小写）。 */
export function normalizeCwdKey(cwd: string | null | undefined): string {
  return (cwd ?? "").replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase();
}

/** 后端不再上报后占位卡的本地留存上限。只兜两类缝隙：claim 已消费 pending 而真实
 *  会话行还没进首页的竞态窗（亚秒级）、mergeTail 留在尾部的旧占位行。它不是第二
 *  生命周期——占位的真实存续归后端 pending 表管（codex 到首个 turn 才认领，这期间
 *  后端每次刷新都原样上报，seenAt 不断续期，HOLD 永远不触发）。 */
export const PENDING_HOLD_MS = 10_000;

/** 对账时容忍的 started_at 偏差：真实行的 started_at 应不早于占位启动时刻，
 *  留一点余量吸收两端取时刻的先后（hook 落库时刻 vs start_pending 时刻）。 */
const STARTED_SKEW_MS = 2_000;

/** 占位登记簿：临时负 id → 占位行 + 后端最近一次上报它的时刻。 */
export type PendingRegistry = Map<number, { item: Item; seenAt: number }>;

function matches(real: Item, pending: Item): boolean {
  return (
    real.session.id > 0 &&
    !real.archived &&
    real.provider === pending.provider &&
    normalizeCwdKey(real.cwd) === normalizeCwdKey(pending.cwd) &&
    // 同 provider+cwd 的旧会话不能冒领：只有启动时刻不早于占位的真实行才算认领完成。
    real.session.started_at >= pending.session.started_at - STARTED_SKEW_MS
  );
}

/** 首页刷新到达时对一次账（registry 原地更新），返回当前应展示的占位卡（最新启动在前）。
 *  fresh 是后端首页响应的**原始** items（含占位行与真实行）。 */
export function reconcilePendings(
  registry: PendingRegistry,
  fresh: Item[],
  now: number
): Item[] {
  for (const item of fresh) {
    if (item.session.id < 0) registry.set(item.session.id, { item, seenAt: now });
  }
  for (const [id, entry] of registry) {
    const claimed = fresh.some((l) => matches(l, entry.item));
    if (claimed || now - entry.seenAt > PENDING_HOLD_MS) registry.delete(id);
  }
  return [...registry.values()]
    .map((e) => e.item)
    .sort((a, b) => b.session.started_at - a.session.started_at);
}

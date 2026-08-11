import { useCallback, useEffect, useRef } from "react";
import { useTauriEvent } from "./useTauriEvent";

/** board-changed 刷新的冷却窗口（ms）。该事件会三连发：命令写库后端立即通知 +
 *  db-watcher 稍后为同一次写入回声 + liveness 轮询。 */
export const BOARD_REFRESH_THROTTLE_MS = 400;

/**
 * 订阅 board-changed 并做 **leading + trailing** 节流：首个事件立即刷新（用户操作
 * 零延迟），冷却窗口内的后续事件合并成窗口末尾的一次。返回稳定引用的节流触发器，
 * 供「归档失败回滚」这类手动刷新复用同一条节流通道。
 *
 * - `doRefresh` 经 ref 取**触发那一刻**的最新闭包：排队期间切了 tab，旧 filter 的
 *   trailing 刷新不得把新 tab 的列表覆盖掉（看板的历史教训）。
 * - 卸载时清 trailing 定时器并复位排队标记：HMR 重挂载会跑 cleanup 但保留 ref，
 *   不复位则每次 refresh() 都以为 trailing 在排队而直接 return，刷新永久静默丢失。
 * - E2E 观测点（VITE_E2E=1 构建）：累计事件次数供 board-refresh.e2e.ts 断言
 *   「空闲时看板不再被刷新」。生产构建下该分支被 vite 死代码消除。
 *
 * 此前看板（App）与侧栏（ChatSidebar）各手写一份同参实现。
 */
export function useBoardRefresh(doRefresh: () => void): () => void {
  const doRefreshRef = useRef(doRefresh);
  doRefreshRef.current = doRefresh;
  const timerRef = useRef<number | undefined>(undefined);
  const lastRunRef = useRef(0);

  const refresh = useCallback(() => {
    if (timerRef.current !== undefined) return; // trailing 已排队，本次并入
    const fire = () => {
      timerRef.current = undefined;
      lastRunRef.current = Date.now();
      doRefreshRef.current();
    };
    const since = Date.now() - lastRunRef.current;
    if (since >= BOARD_REFRESH_THROTTLE_MS) fire();
    else timerRef.current = window.setTimeout(fire, BOARD_REFRESH_THROTTLE_MS - since);
  }, []);

  useTauriEvent("board-changed", () => {
    if (import.meta.env.VITE_E2E === "1") {
      const w = window as typeof window & { __MEOWO_BOARD_CHANGED__?: number };
      w.__MEOWO_BOARD_CHANGED__ = (w.__MEOWO_BOARD_CHANGED__ ?? 0) + 1;
    }
    refresh();
  });

  useEffect(() => {
    if (import.meta.env.VITE_E2E === "1") {
      // 挂载即初始化为 0，让 E2E 测试挂载后立刻能读到计数（不必等第一次事件）。
      (window as typeof window & { __MEOWO_BOARD_CHANGED__?: number }).__MEOWO_BOARD_CHANGED__ ??= 0;
    }
    return () => {
      window.clearTimeout(timerRef.current);
      timerRef.current = undefined;
    };
  }, []);

  return refresh;
}

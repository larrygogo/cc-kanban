import { useRef, useState } from "react";
import { installAgent, type AgentId, type InstallDone } from "../api";
import { useTauriEvent } from "./useTauriEvent";

export type InstallOperationState =
  /** startedAt：安装动辄一两分钟且无进度事件，卡片显示已耗时，别让用户对着静止转圈猜死活。 */
  | { phase: "installing"; startedAt: number }
  /** 脚本跑完（install-done 到达）：失败原因在 logPath 指向的日志里。 */
  | { phase: "done"; ok: boolean; logPath: string | null }
  /** 后端在 spawn 之前就失败（取不到脚本 / 被 CF 拦）：message 是后端的本地化诊断，此时没有日志文件。 */
  | { phase: "error"; message: string };

/**
 * 后台安装操作的完整生命周期（与 useLoginOperations 同一套路）。
 *
 * 状态按 provider 键控、挂在 provider 卡片**之上**：安装动辄一两分钟，用户会切下拉去看别的
 * agent——状态若住在 keyed 卡片里，切换即卸载、install-done 监听随之注销，回来时卡片显示
 * 「安装」按钮诱导二次安装，失败原因也永久丢失（登录侧早为同一坑做了页面级提升，安装此前没做）。
 */
export function useInstallOperations(onDone?: (event: InstallDone) => void) {
  const [states, setStates] = useState<Map<AgentId, InstallOperationState>>(new Map());
  // state 供渲染 / ref 供同步判定：拒绝双击重入。
  const installingRef = useRef<Set<AgentId>>(new Set());

  useTauriEvent<InstallDone>("install-done", (event) => {
    const { provider, ok, logPath } = event.payload;
    installingRef.current.delete(provider);
    setStates((current) => new Map(current).set(provider, { phase: "done", ok, logPath }));
    onDone?.(event.payload);
  });

  const start = (provider: AgentId) => {
    if (installingRef.current.has(provider)) return;
    installingRef.current.add(provider);
    setStates((current) => new Map(current).set(provider, { phase: "installing", startedAt: Date.now() }));
    installAgent(provider).catch((error) => {
      installingRef.current.delete(provider);
      setStates((current) => new Map(current).set(provider, { phase: "error", message: String(error) }));
    });
  };

  return {
    states,
    start,
    isInstalling: (provider: AgentId) => installingRef.current.has(provider),
  };
}

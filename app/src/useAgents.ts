import { useEffect, useRef, useState } from "react";
import { listAgents, agentName, type AgentDescriptor, type AgentId } from "./api";
import { useTauriEvent } from "./hooks/useTauriEvent";

/**
 * 装完一个 agent，名单就变了（未装 → 已装）。**凡是列 agent 的地方都得跟着重取**，否则装完还要
 * 重开页面才看得见：网络分区的代理行、新建会话面板的可选项、贴纸的徽标，都只在挂载时取过一次。
 *
 * 只有 `ok` 才重取——失败什么都没改变。
 */
export function useAgentListRefresh(reload: () => void) {
  useTauriEvent<{ ok: boolean }>("install-done", (e) => {
    if (e.payload.ok) reload();
  });
}

/**
 * 后端下发的 agent 名单（展示名 + 安装态）。前端不再自己维护这份名单——加一个 agent 不必改前端。
 *
 * 返回 `agents === null` 表示尚未 resolve：调用方据此避免首帧误判「未安装」而闪一下安装按钮。
 * `name(id)` 对未知 id 回退成 id 本身（显示 `"gemini"` 好过显示 `"Claude Code"`）。
 */
export function useAgents(): {
  agents: AgentDescriptor[] | null;
  installed: AgentId[];
  name: (id: AgentId) => string;
  reload: () => void;
} {
  const [agents, setAgents] = useState<AgentDescriptor[] | null>(null);
  // 事件订阅里的 reload 可能晚于卸载才 resolve:卸载后不得再 setState。
  const mountedRef = useRef(true);
  useEffect(() => () => { mountedRef.current = false; }, []);
  const reload = () => {
    listAgents().then((list) => { if (mountedRef.current) setAgents(list); }).catch(() => {});
  };
  useEffect(reload, []);
  useAgentListRefresh(reload); // 装完新 agent 立刻反映，不必重开页面
  return {
    agents,
    installed: (agents ?? []).filter((a) => a.installed).map((a) => a.id),
    name: (id) => agentName(agents ?? [], id),
    reload,
  };
}

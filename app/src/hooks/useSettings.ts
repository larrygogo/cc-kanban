import { useEffect, useRef } from "react";
import { listen } from "@tauri-apps/api/event";
import { getSettings, type Settings } from "../api";

/**
 * subscribe-first 读取 settings 的统一实现：先订 `settings-changed` 再 fetch 一次，
 * 事件先到则丢弃迟到的旧快照（消除 fetch-vs-subscribe 竞态，参考实现 appearance.ts）。
 * `apply` 经 ref 取每次渲染的最新闭包；组件卸载先于 listen resolve 时立即注销防泄漏。
 *
 * 刻意**不**用本 hook 的消费者（各有结构性理由，勿强迁）：
 * - `appearance.ts`：模块级非 React 代码；
 * - `i18n/index.tsx`：首帧走 localStorage 语言缓存，apply 还要写回缓存；
 * - `useUpdate.ts`：apply 与同一 effect 里的定时器生命周期纠缠，且 fetch 失败要置 error 态；
 * - `ManagedTerminal.tsx`：apply 闭包持有 effect 内创建的 xterm 实例，hook 提不出去。
 */
export function useSettingsEffect(apply: (settings: Settings) => void): void {
  const applyRef = useRef(apply);
  applyRef.current = apply;

  useEffect(() => {
    let eventApplied = false;
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    try {
      listen<Settings>("settings-changed", (event) => {
        eventApplied = true;
        applyRef.current(event.payload);
      })
        .then((dispose) => {
          if (cancelled) dispose();
          else unlisten = dispose;
        })
        .catch(() => {});
    } catch {
      /* 非 Tauri 环境（测试/浏览器） */
    }
    getSettings()
      .then((settings) => {
        if (!eventApplied && !cancelled) applyRef.current(settings);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      try { unlisten?.(); } catch { /* noop */ }
    };
  }, []);
}

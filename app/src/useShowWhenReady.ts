import { useEffect } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";

/**
 * 供以 `visible: false` 创建的窗口在首帧渲染提交后自行显示。
 * 窗口创建即可见的话，WebView 画出首帧前是默认白底——打开瞬间会闪一下白框。
 * 双 rAF 确保浏览器真正 paint 过一帧再 show，窗口出现时已是成品画面。
 * 后端留有兜底（window.rs 的 show_after_grace）：前端脚本没起来时窗口到点自动显示，
 * 不会永久隐身。
 *
 * `focus: false` 给贴纸主窗口：它配置了 focus:false（开机自启不能抢焦点），显示时同样不能。
 * `enabled: false` 给 macOS 面板模式的贴纸：显隐归 menubar 管，前端不得越权 show。
 * `ready`（默认 true）是**响应式**的额外闸门：贴纸以折叠态启动时要等 snap_collapse
 *   落地再显示——窗口此刻还是 360×440 的默认尺寸，先 show 会闪一帧「大框里挂几个圆点、
 *   随后骤缩成条」。false → true 的那次翻转触发显示；后端 show_after_grace 兜底不变。
 */
export function useShowWhenReady(opts?: { focus?: boolean; enabled?: boolean; ready?: boolean }): void {
  const focus = opts?.focus ?? true;
  const enabled = opts?.enabled ?? true;
  const ready = opts?.ready ?? true;
  useEffect(() => {
    if (!enabled || !ready) return;
    let raf2 = 0;
    const raf1 = requestAnimationFrame(() => {
      raf2 = requestAnimationFrame(() => {
        try {
          const w = getCurrentWindow();
          // 可选调用兼容测试环境的窗口 mock（往往只 mock 了 close）。
          void Promise.resolve(w.show?.())
            .then(() => (focus ? w.setFocus?.() : undefined))
            .catch(() => {});
        } catch {
          /* 非 Tauri 环境（测试/浏览器预览）没有窗口可显示 */
        }
      });
    });
    return () => {
      cancelAnimationFrame(raf1);
      cancelAnimationFrame(raf2);
    };
    // focus/enabled 是挂载时刻的一次性配置；ready 是响应式闸门，翻 true 才 show。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ready]);
}

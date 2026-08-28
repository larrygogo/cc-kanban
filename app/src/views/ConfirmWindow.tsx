/// 确认对话框小窗(label `confirm-<id>`,由后端 confirm.rs 创建)。无边框,整卡可拖拽
/// (原生窗口拖动,可拖出主窗边界);内容经命令取回而不是 URL 参数(任意语言文本免转义)。
/// Esc = 取消;默认焦点在取消上,Enter 顺手一按不该通过破坏性动作。
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { LogicalSize, PhysicalPosition } from "@tauri-apps/api/dpi";
import { useT } from "../i18n";

type Payload = { title: string; message: string; danger: boolean; confirm_label: string | null };

/// 后端固定 208 高开窗(给两行消息留量);实际内容常常只有一行,正文与按钮之间会剩一大片
/// 空白。收拢的下限护住「标题+一行+按钮」的最小形态,上限之外交给正文滚动区。
const MIN_HEIGHT = 132;
const MAX_HEIGHT = 400;

export function ConfirmWindow() {
  const t = useT();
  const [payload, setPayload] = useState<Payload | null>(null);
  const messageRef = useRef<HTMLSpanElement>(null);
  // 非 Tauri 环境(测试/浏览器预览)getCurrentWindow 在渲染期就会抛:id 置 null,
  // 组件渲染空壳并跳过所有窗口调用(与下方 fit 的降级同一口径),不能整棵树崩掉。
  const id = (() => {
    try {
      return Number(getCurrentWindow().label.slice("confirm-".length));
    } catch {
      return null;
    }
  })();
  const decide = (ok: boolean) => {
    if (id === null) return;
    // 结果送回请求方;窗口由后端在收到结果后关闭。失败也不留悬窗:自关兜底。
    void invoke("confirm_dialog_result", { id, ok }).catch(() => {
      void getCurrentWindow().close().catch(() => {});
    });
  };
  useEffect(() => {
    if (id === null) return;
    invoke<Payload>("confirm_dialog_payload", { id })
      .then(setPayload)
      // 取不到内容(请求已被并发取消)就直接按取消收场,不渲染空壳。
      .catch(() => decide(false));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id]);
  // 首帧 paint 后按内容实高收拢窗口,再显示——不用 useShowWhenReady(它 mount 即 show,
  // 用户会看到窗口先大后小跳一下)。正文里的 span 是块级测量探针:p 被 flex:1 撑开时
  // clientHeight 是「分到的高度」,span.offsetHeight 才是文本自然高度,差值即窗口该增减的量。
  // Windows 上 resizable(false) 会把 min/max 锁死成当前尺寸,setSize 被钳住,必须临时
  // 放开再锁回(同 Updater);收拢后把窗口下移一半差值,保持视觉中心不动(开窗时按 208
  // 高对着请求窗口居中)。show 独立兜底:调整失败也必须照常显示,宁可尺寸不完美也不能
  // 一直不可见。后端另有 show_after_grace 的到点强制 show(confirm.rs)——那是前端整个
  // 没起来(加载失败/崩溃)时防「父窗已禁用+confirm 隐身=应用被劫持」的最后防线,两层
  // 兜底各管一段,都不能省。
  useEffect(() => {
    if (!payload) return;
    const fit = async () => {
      let w: ReturnType<typeof getCurrentWindow>;
      try {
        w = getCurrentWindow();
      } catch {
        return; // 非 Tauri 环境(测试/浏览器预览)
      }
      try {
        const span = messageRef.current;
        const p = span?.parentElement;
        if (span && p) {
          const delta = span.offsetHeight - p.clientHeight;
          const desired = Math.round(Math.min(Math.max(window.innerHeight + delta, MIN_HEIGHT), MAX_HEIGHT));
          if (Math.abs(desired - window.innerHeight) > 1) {
            const pos = await w.outerPosition();
            const before = await w.outerSize();
            await w.setResizable(true);
            await w.setSize(new LogicalSize(window.innerWidth, desired));
            await w.setResizable(false);
            const after = await w.outerSize();
            await w.setPosition(new PhysicalPosition(pos.x, pos.y + Math.round((before.height - after.height) / 2)));
          }
        }
      } catch {
        /* 调整失败维持后端默认尺寸,继续显示 */
      }
      try {
        // 可选调用兼容测试环境的窗口 mock(往往只 mock 了 label/close)。
        await w.show?.();
        await w.setFocus?.();
      } catch {
        /* 非 Tauri 环境 */
      }
    };
    // 双 rAF:等浏览器真正 paint 过一帧再量与显示(同 useShowWhenReady 的时序约定)。
    let raf2 = 0;
    const raf1 = requestAnimationFrame(() => {
      raf2 = requestAnimationFrame(() => void fit());
    });
    return () => {
      cancelAnimationFrame(raf1);
      cancelAnimationFrame(raf2);
    };
  }, [payload]);
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") decide(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id]);
  if (!payload) return null;
  return (
    <div className="app-confirm is-window" data-tauri-drag-region>
      <strong data-tauri-drag-region>{payload.title}</strong>
      <p data-tauri-drag-region>
        <span ref={messageRef} data-tauri-drag-region>{payload.message}</span>
      </p>
      <div className="app-confirm-actions">
        <button type="button" autoFocus onClick={() => decide(false)}>{t.dialog.cancel}</button>
        <button type="button" className={payload.danger ? "is-danger" : "is-primary"} onClick={() => decide(true)}>
          {/* 主按钮说清后果（「删除」「合并」），未指定才回退通用「确定」。 */}
          {payload.confirm_label ?? t.dialog.ok}
        </button>
      </div>
    </div>
  );
}

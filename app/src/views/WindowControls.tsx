// Windows/Linux：对话窗右上角的标准窗口控制组（最小化 / 最大化·还原 / 关闭），
// Kimi 式融入顶栏行——内容顶到窗口最上沿，不加原生标题栏条（那会在应用顶栏之上
// 再叠一条系统栏，双标题栏）。macOS 由原生红绿灯（TitleBarStyle::Overlay，见
// window.rs）承担同职，本组件整体不渲染。
//
// Windows 的原生化：三颗按钮上方各盖一个原生覆盖子窗（snap_layout.rs），返回
// HTMINBUTTON / HTMAXBUTTON / HTCLOSE——最大化钮由此获得 Win11 Snap Layouts
// 悬停浮窗，三颗钮的点击都走系统命令（与原生标题栏同一条代码路径）。代价是这些
// 矩形的鼠标事件进不了 WebView：hover 视觉靠后端 caption-hover 事件补类名
// （:hover 失效），onClick 收不到（保留只为 Linux 与键盘激活）。
import { type ReactElement, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { isMac, isWindows } from "../platform";
import { useT } from "../i18n";

export function WindowControls({ onClose }: { onClose: () => void }): ReactElement | null {
  const t = useT();
  const mac = isMac();
  // 最大化态驱动 □/❐ 图标切换。除按钮外还有别的路径改变它（标题栏双击、Win+↑、
  // 拖到屏幕顶边、Snap 分屏），故不能只在点击时翻转本地态——监听窗口 resize 后查实际状态。
  const [maximized, setMaximized] = useState(false);
  // 覆盖窗吞掉了真实鼠标事件，hover 态由后端 caption-hover 事件驱动（按 kind 分钮）。
  const [hover, setHover] = useState<{ min?: boolean; max?: boolean; close?: boolean }>({});
  const minBtnRef = useRef<HTMLButtonElement>(null);
  const maxBtnRef = useRef<HTMLButtonElement>(null);
  const closeBtnRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (mac) return;
    let unlisteners: (() => void)[] = [];
    let disposed = false;
    // 节流句柄：拖拽缩放时 onResized 每秒连发几十次，不能每 tick 一趟 IPC + 一次
    // 主线程 Win32 更新（src-tauri/CLAUDE.md 守护的正是那个消息泵，评审发现）。
    let syncRaf = 0;
    let rectTimer = 0;
    const on = (p: Promise<() => void>) =>
      p.then((u) => { if (disposed) u(); else unlisteners.push(u); }).catch(() => {});
    // 非 Tauri 环境（测试/浏览器）getCurrentWindow 会抛错，吞掉即可（同 ChatWindow.setTitle 的写法）。
    try {
      const w = getCurrentWindow();
      const sync = () => void w.isMaximized().then((m) => { if (!disposed) setMaximized(m); }).catch(() => {});
      // isMaximized 查询按帧合并：一帧内多次 onResized 只发一趟 IPC。
      const scheduleSync = () => {
        if (syncRaf) return;
        syncRaf = requestAnimationFrame(() => { syncRaf = 0; sync(); });
      };
      // 覆盖窗矩形：物理像素、客户区坐标（getBoundingClientRect 原点即客户区原点，
      // 乘 devicePixelRatio 转物理）。
      const measureRects = () => {
        if (disposed || !isWindows()) return;
        const s = window.devicePixelRatio || 1;
        const rectOf = (kind: string, el: HTMLButtonElement | null) => {
          const r = el?.getBoundingClientRect();
          return r ? {
            kind,
            x: Math.round(r.left * s),
            y: Math.round(r.top * s),
            w: Math.round(r.width * s),
            h: Math.round(r.height * s),
          } : null;
        };
        const rects = [
          rectOf("min", minBtnRef.current),
          rectOf("max", maxBtnRef.current),
          rectOf("close", closeBtnRef.current),
        ].filter(Boolean);
        void invoke("set_caption_overlay_rects", { rects }).catch(() => {});
      };
      // 覆盖窗更新去抖到 resize 稳定后：拖拽期间没人悬停标题钮，逐帧跟随是白付的
      // FindWindowExW + SetWindowPos；停止缩放 150ms 后一次到位。
      const scheduleRects = () => {
        window.clearTimeout(rectTimer);
        rectTimer = window.setTimeout(measureRects, 150);
      };
      sync();
      // 首帧布局落定后上报一次。
      requestAnimationFrame(measureRects);
      on(w.onResized(() => { scheduleSync(); scheduleRects(); }));
      on(w.onScaleChanged(scheduleRects));
      on(w.listen<{ kind: "min" | "max" | "close"; hover: boolean }>("caption-hover", (e) => {
        if (!disposed && e.payload?.kind) setHover((prev) => ({ ...prev, [e.payload.kind]: e.payload.hover }));
      }));
    } catch { /* noop */ }
    return () => {
      disposed = true;
      cancelAnimationFrame(syncRaf);
      window.clearTimeout(rectTimer);
      unlisteners.forEach((u) => u());
      if (isWindows()) {
        try { void invoke("set_caption_overlay_rects", { rects: null }).catch(() => {}); } catch { /* noop */ }
      }
    };
  }, [mac]);
  if (mac) return null;

  const minimize = () => { try { void getCurrentWindow().minimize().catch(() => {}); } catch { /* noop */ } };
  const toggleMaximize = () => { try { void getCurrentWindow().toggleMaximize().catch(() => {}); } catch { /* noop */ } };
  return (
    <div className="wincaption">
      <button
        ref={minBtnRef}
        type="button"
        className={hover.min ? "is-hover" : undefined}
        aria-label={t.chat.minimize}
        onClick={minimize}
      >
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true">
          <path d="M5 12h14" />
        </svg>
      </button>
      <button
        ref={maxBtnRef}
        type="button"
        className={hover.max ? "is-hover" : undefined}
        aria-label={maximized ? t.chat.restoreWindow : t.chat.maximize}
        onClick={toggleMaximize}
      >
        {maximized ? (
          // 还原：前后两个错位方框（Windows 惯例 ❐）。
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinejoin="round" aria-hidden="true">
            <rect x="5" y="8.5" width="10.5" height="10.5" rx="1.5" />
            <path d="M8.8 8.5V6.6A1.6 1.6 0 0 1 10.4 5h7A1.6 1.6 0 0 1 19 6.6v7a1.6 1.6 0 0 1-1.6 1.6h-1.9" />
          </svg>
        ) : (
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinejoin="round" aria-hidden="true">
            <rect x="5.5" y="5.5" width="13" height="13" rx="1.5" />
          </svg>
        )}
      </button>
      <button
        ref={closeBtnRef}
        type="button"
        className={"win-close" + (hover.close ? " is-hover" : "")}
        aria-label={t.chat.close}
        onClick={onClose}
      >
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true">
          <path d="M6 6l12 12M18 6L6 18" />
        </svg>
      </button>
    </div>
  );
}

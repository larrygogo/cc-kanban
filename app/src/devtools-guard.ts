// 正式构建下封死 WebView 的「调试入口」：屏蔽右键菜单 + DevTools 快捷键。
// dev 构建（bun run dev）原样放行，方便开发期调试。
export function lockdownInProduction() {
  if (!import.meta.env.PROD) return;

  // 屏蔽 WebView 默认右键菜单（重新加载/另存为/检查等）。
  window.addEventListener("contextmenu", (e) => e.preventDefault(), { capture: true });

  // 封死 DevTools 快捷键：F12、Ctrl+Shift+I/J，以及 macOS 的 Cmd+Opt+I/J/C。
  // 用 e.code 而非 e.key，避免 Shift/Opt 改变字符带来的判定遗漏。
  // Ctrl+Shift+C 刻意不拦：它是终端界的惯用复制键（托管终端靠它复制选区），而生产构建
  // 的 WebView 本就不带 DevTools，单靠这一个组合键开不出任何调试入口——F12/I/J 仍拦。
  window.addEventListener(
    "keydown",
    (e) => {
      const ij = e.code === "KeyI" || e.code === "KeyJ";
      const isDevtools =
        e.code === "F12" ||
        (e.ctrlKey && e.shiftKey && ij) ||
        (e.metaKey && e.altKey && (ij || e.code === "KeyC"));
      if (isDevtools) e.preventDefault();
    },
    { capture: true }
  );
}

// 正式构建下封死 WebView 的「调试入口」：屏蔽右键菜单 + DevTools 快捷键。
// dev 构建（bun run dev）原样放行，方便开发期调试。

// Ctrl/Cmd+F（及 F3 查找下一个）会呼出 WebView 自带的页内查找条，与应用内搜索
// （贴纸/对话侧栏、托管终端）撞车。捕获层统一按掉默认行为；不 stopPropagation，
// 应用内的 Ctrl+F 监听与 xterm 的自定义键处理照常收到事件。dev 构建同样生效——
// 查找条不是调试入口，是各构建一致的行为缺陷，故不并入 lockdownInProduction。
export function suppressNativeFind() {
  window.addEventListener(
    "keydown",
    (e) => {
      const findCombo = (e.ctrlKey || e.metaKey) && !e.altKey && e.code === "KeyF";
      // F3 在托管终端内是发往 PTY 的功能键（xterm 自行消费并 preventDefault），不代拦。
      const findNext = e.code === "F3" && !document.activeElement?.closest(".managed-terminal");
      if (findCombo || findNext) e.preventDefault();
    },
    { capture: true }
  );
}

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

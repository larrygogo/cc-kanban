// dev 构建标识：dev 与安装版共库后两边界面内容完全相同，点开的窗口属于哪个实例
// 只能靠标识分辨（历史上三次排查到最后才发现是双实例，见仓库排查记录）。
// 仅 vite dev 渲染；vitest（MODE==="test"）也不渲染——现有视图测试不该看到它。
export function DevBadge() {
  if (!import.meta.env.DEV || import.meta.env.MODE === "test") return null;
  // 不设 aria-hidden：这个徽标存在的意义正是区分双实例，屏幕阅读器用户同样需要它。
  return (
    <span className="dev-badge" role="status">
      DEV
    </span>
  );
}

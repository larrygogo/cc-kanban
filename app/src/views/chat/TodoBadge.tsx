/// 任务/子任务的状态徽章——标题栏进度面板、对话底部 TodoPanel、子任务条共用同一套
/// 视觉语言(用户指定,不再各处一套字符/圆点):完成 = 实心圆勾,进行中 = 旋转的绿色
/// 缺口圆环,待办 = 淡描边空圈,失败 = 红底叉。`small` 是行内文字旁(子任务状态徽章)
/// 的缩小规格。样式见 styles.css 的 `.chat-todo-badge.is-*`。
export function TodoBadge({ status, small }: { status: string; small?: boolean }) {
  const size = small ? 8 : 10;
  const icon =
    status === "completed" ? <path d="m5 13 4.2 4.2L19 7" />
    : status === "failed" ? <path d="M7 7l10 10M17 7 7 17" />
    : null;
  return (
    <span className={"chat-todo-badge is-" + status + (small ? " is-sm" : "")} aria-hidden="true">
      {icon && (
        <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3.2" strokeLinecap="round" strokeLinejoin="round">{icon}</svg>
      )}
    </span>
  );
}

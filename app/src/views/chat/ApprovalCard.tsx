import type { ReactNode } from "react";

/// 审批/交互卡的统一外壳。对话页上四类卡——broker 审批(claude hook 劫走的请求)、
/// 终端命令审批(claude/kimi 屏幕识别)、交互选择器(question/plan)、其他屏幕提示
/// (信任页/启动步骤/菜单)——此前各写一段 section、三套视觉(白卡/琥珀内联/琥珀浮层),
/// 现在全部收敛到 .chat-approval 一族:标题圆点 + 待授权徽章、正文、左右两个动作组。
///
/// 只统一长相,不统一决定回路:broker 卡走 resolvePendingApproval RPC,屏幕卡把按键
/// 写进 PTY——那些差异留在调用方的按钮 onClick 里。动作排布的约定也在这里定死:
/// 左组(sideActions)放「不作本次决定」的动作(零副作用的仅收起、范围更大的持久放行),
/// 右组(actions)只放本次的决定(拒绝/允许/确认),与 Esc=拒绝的落点对得上。
export function ApprovalCard({ title, badge, className, children, sideActions, actions }: {
  title: string;
  badge?: string;
  className?: string;
  /// 卡片正文(工具名/描述/命令原文/选项列表…),渲染进 .chat-approval-copy。
  children?: ReactNode;
  sideActions?: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <section className={"chat-approval" + (className ? ` ${className}` : "")} role="alert">
      <div className="chat-approval-copy">
        <div className="chat-approval-head">
          <strong>{title}</strong>
          {badge && <span className="chat-approval-badge">{badge}</span>}
        </div>
        {children}
      </div>
      {/* 左组为空时由 CSS 的 :empty 收掉,右组按钮天然靠右。 */}
      {(sideActions || actions) && <div className="chat-approval-actions">
        <div className="chat-approval-actions-side">{sideActions}</div>
        {actions}
      </div>}
    </section>
  );
}

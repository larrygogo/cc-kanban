import { useEffect, useLayoutEffect, useRef, useState, type ReactNode, type RefObject } from "react";

/// 焦点是否落在用户正在打字的控件里（见 7C-1 的移焦让位）。
function isTypingTarget(el: Element | null): boolean {
  return el instanceof HTMLElement
    && (el.tagName === "TEXTAREA" || el.tagName === "INPUT" || el.isContentEditable);
}

/// 审批/交互卡的统一外壳。对话页上四类卡——broker 审批(claude hook 劫走的请求)、
/// 终端命令审批(claude/kimi 屏幕识别)、交互选择器(question/plan)、其他屏幕提示
/// (信任页/启动步骤/菜单)——此前各写一段 section、三套视觉(白卡/琥珀内联/琥珀浮层),
/// 现在全部收敛到 .chat-approval 一族:标题圆点 + 待授权徽章、正文、左右两个动作组。
///
/// 只统一长相,不统一决定回路:broker 卡走 resolvePendingApproval RPC,屏幕卡把按键
/// 写进 PTY——那些差异留在调用方的按钮 onClick 里。动作排布的约定也在这里定死:
/// 左组(sideActions)放「不作本次决定」的动作(零副作用的仅收起、范围更大的持久放行),
/// 右组(actions)只放本次的决定(拒绝/允许/确认),与 Esc=拒绝的落点对得上。
export function ApprovalCard({ title, badge, className, children, sideActions, actions, returnFocusTo }: {
  title: string;
  badge?: string;
  className?: string;
  /// 卡片正文(工具名/描述/命令原文/选项列表…),渲染进 .chat-approval-copy。
  children?: ReactNode;
  sideActions?: ReactNode;
  actions?: ReactNode;
  /// 收卡后的焦点归还目标(composer 输入框)。C-9 overlay 后卡片不再是输入框的
  /// 文档流邻居,浏览器不会自己找回去——拒/答/收起导致卸载时显式归还。
  returnFocusTo?: RefObject<HTMLElement | null>;
}) {
  const ref = useRef<HTMLElement>(null);
  // G-16：role=alert 只打断朗读、焦点却进不来，用户听不到也摸不着按钮。改 alertdialog
  // 并在出现时把焦点交给第一个动作按钮（alertdialog 的惯例）；非模态， aria-modal=false。
  //
  // 7C-1：但用户正在 composer 里打插话时卡片弹出，抢焦点会把下一个空格/回车喂给
  // 「拒绝」钮——一次误拒。U1-21 定的「不锁 composer」与这里的挂载移焦叠加出的输入
  // 竞态，让位给正在进行的输入：焦点在可编辑控件里就不抢，读屏用户仍可用 Tab 进卡。
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    if (isTypingTarget(document.activeElement)) return;
    // 优先右组（本次决定）的第一个按钮；左组是「不作决定」的次要动作，不该先落焦。
    (el.querySelector<HTMLElement>(".chat-approval-actions > button")
      ?? el.querySelector<HTMLElement>(".chat-approval-actions button")
      ?? el.querySelector<HTMLElement>("button"))?.focus();
  }, []);
  // 焦点归还用 layout cleanup:被动 effect 的 cleanup 在 DOM 移除后才跑,那时
  // contains(activeElement) 已失效(焦点先掉回 body);layout cleanup 在移除前执行,
  // 还能认出「焦点在卡内」。归还本身推迟到宏任务:同一帧挂载的新卡会先接管焦点
  // (那时 activeElement 已不是 body,不抢),且 compose 的 inert 解锁 effect
  // (屏幕识别卡在场时锁定 composer)也已跑完。
  useLayoutEffect(() => {
    const el = ref.current;
    return () => {
      if (!el || !el.contains(document.activeElement)) return;
      const target = returnFocusTo?.current;
      window.setTimeout(() => {
        // 切到终端视图时 composer 同帧卸载(detached),focus 无意义也不生效——跳过。
        if ((document.activeElement === document.body || document.activeElement == null) && target?.isConnected) target.focus();
      }, 0);
    };
  }, []);
  return (
    <section ref={ref} className={"chat-approval" + (className ? ` ${className}` : "")} role="alertdialog" aria-modal="false" aria-label={title}>
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

/// 破坏性命令的浅启发式:只用来给「允许一次」换危险色(rm -rf 与 ls 不该同一个
/// 视觉权重),不改变任何行为。宁漏勿冤——漏判只是回到默认主按钮,误判会把一条
/// 安全命令染红。同时覆盖 bash 与 Windows cmd 的常见破坏形态。
const RISKY_COMMAND = /(?:\brm\s+-[a-z]*[rf][a-z]*\b|\bsudo\b|\bmkfs(?:\.[a-z0-9]+)?\b|\bdd\b[^|&;]*\bof=|\bshutdown\b|\breboot\b|\bgit\s+reset\s+--hard\b|\bgit\s+clean\s+-[a-z]*[fd]|\bgit\s+push\b[^|&;]*--force|\bchmod\s+-R\b|\bchown\s+-R\b|>\s*\/dev\/sd|\bformat\s+[a-z]:|\bdel\s+\/[fsq]\b|\brd\s+\/s\b|\brmdir\s+\/s\b)/i;

export function isRiskyCommand(text: string): boolean {
  return RISKY_COMMAND.test(text);
}

/// 长命令详情:pre 限高 160px、外层 copy 又限高滚动,两层嵌套滚动里长命令读不全
/// 也拿不走。给两个出口:「展开全部」摘掉 pre 的限高(回到外层单层滚动)、
/// 「复制命令」整段进剪贴板。label/copy 文案由调用方给(i18n 在 ChatWindow 侧)。
export function ApprovalCommandDetail({ label, command, expandLabel, collapseLabel, copyLabel, copiedLabel }: {
  label: string;
  command: string;
  expandLabel: string;
  collapseLabel: string;
  copyLabel: string;
  copiedLabel: string;
}) {
  const [expanded, setExpanded] = useState(false);
  const [copied, setCopied] = useState(false);
  const copy = () => {
    if (!navigator.clipboard?.writeText) return;
    void navigator.clipboard.writeText(command).then(() => {
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    }).catch(() => {});
  };
  return (
    <div className={"chat-approval-detail" + (expanded ? " is-expanded" : "")}>
      <span>{label}</span>
      <pre>{command}</pre>
      <div className="chat-approval-detail-tools">
        <button type="button" onClick={() => setExpanded((value) => !value)}>{expanded ? collapseLabel : expandLabel}</button>
        <button type="button" onClick={copy}>{copied ? copiedLabel : copyLabel}</button>
      </div>
    </div>
  );
}

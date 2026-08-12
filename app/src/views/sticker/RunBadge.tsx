// 卡片状态徽标：边框流动亮线 + 中心圆内的上下文已用百分比。
import { useT } from "../../i18n";

/** 状态徽标：圆角矩形边框上流动的亮线（conic 渐变 + transform 旋转，纯 GPU 合成，
 *  拖动窗口不占主线程）+ 中心实心圆，圆内显示 Content 已用百分比。
 *  tone 与侧栏状态点同一套红绿灯语言：running=绿+流动（在干活），waiting=黄+静止
 *  （等你打字），pending=黄+闪烁（要你按钮决策/审批，颜色同 waiting、急迫感靠闪）。 */
export function RunBadge({
  pct,
  tone = "running",
}: {
  pct: number | null;
  tone?: "running" | "waiting" | "pending";
}) {
  const t = useT();
  const what = tone === "waiting" ? t.badge.waiting : tone === "pending" ? t.badge.pending : t.badge.running;
  const label = pct != null ? t.badge.full(what, pct) : what;
  return (
    <span
      className={"run-badge" + (tone === "waiting" ? " run-badge--waiting" : tone === "pending" ? " run-badge--pending" : "")}
      role="img"
      aria-label={label}
      data-tip={label}
    >
      {/* 旋转的亮段（被 .run-badge 的圆角裁剪 → 光点沿边框跑） */}
      <span className="run-sweep" />
      {/* 遮住中心黑底，只露出外圈一圈边框 */}
      <span className="run-mask" />
      {/* 中心实心圆 + 百分比 */}
      <span className="run-core">{pct != null ? `${pct}%` : ""}</span>
    </span>
  );
}

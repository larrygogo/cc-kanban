import { useEffect, useRef } from "react";
import { LiveSession } from "../api";
import { useT } from "../i18n";
import { cardTone } from "./sticker/helpers";

type Item = LiveSession & { connected: boolean };
type Edge = "left" | "right" | "top";

// 缩略条主轴最小长度：保证空状态/只有一个点时仍是一条好找好点的条，而非细缝。
const STRIP_MIN = 48;

// 缩略条的空态占位：无活跃会话时居中显示一双灰色眼睛，呼应 Meowo logo。
function EyesMark() {
  return (
    <svg
      width={22}
      height={22}
      viewBox="0 0 24 24"
      fill="currentColor"
      className="cstrip-eyes"
      aria-hidden
    >
      <circle cx="6.5" cy="12" r="4.5" />
      <circle cx="17.5" cy="12" r="4.5" />
    </svg>
  );
}

// 竖条：纵向排列各 connected 会话的状态色点（断开/历史会话不显示）。
// 无活跃会话时显示灰色眼睛占位，保持缩略条合理可点的尺寸。
// 悬停即偷看展开（onExpand）；测量真实内容高度上报（onMeasure）让窗口贴合，避免滚动条。
export function CollapsedStrip({
  data,
  edge,
  onExpand,
  onMeasure,
}: {
  data: Item[];
  edge: Edge;
  onExpand: () => void;
  onMeasure?: (heightPx: number) => void;
}) {
  const t = useT();
  const items = data.filter((l) => !l.archived && l.connected);
  const dotsRef = useRef<HTMLDivElement>(null);
  const horizontal = edge === "top"; // 顶部为横条，沿宽度排列

  // hover-intent：进入 250ms 后才展开。缩略条贴在屏幕边缘——那是鼠标的高频经过区
  // （滚动条、关闭按钮、开始菜单），零延迟展开意味着划过一下就弹出整块看板遮住工作区。
  // 离开/按下（开始拖动）都取消；键盘聚焦与 Enter/Space 仍即时展开（无误触问题）。
  const hoverTimer = useRef<number | null>(null);
  const cancelExpand = () => {
    if (hoverTimer.current != null) {
      window.clearTimeout(hoverTimer.current);
      hoverTimer.current = null;
    }
  };
  const scheduleExpand = () => {
    if (hoverTimer.current != null) return;
    hoverTimer.current = window.setTimeout(() => {
      hoverTimer.current = null;
      onExpand();
    }, 250);
  };
  useEffect(() => cancelExpand, []);

  useEffect(() => {
    const el = dotsRef.current;
    if (el && onMeasure) {
      // 横条量内容宽度、竖条量内容高度；再加 .cstrip padding 余量；不低于 STRIP_MIN。
      const content = horizontal ? el.scrollWidth : el.scrollHeight;
      onMeasure(Math.max(Math.ceil(content) + 12, STRIP_MIN));
    }
  }, [items.length, onMeasure, horizontal]);

  return (
    // 键盘可达：可聚焦，聚焦/Enter/Space 即时展开（悬停走 hover-intent 延迟）。用 group
    // 而非 button——button 会把内部状态点的 img 语义压掉，状态点的可访问文本就丢了。
    // data-tauri-drag-region：README 承诺的「拖离边缘恢复」此前根本做不到（条上没有拖拽
    // 区，按住毫无反应）。挂上后 App 的 mousedown 捕获会进入拖拽流程，拖离边缘松手时
    // handleDragRelease 走 snap_restore 还原普通窗口；原地点一下（不位移）不触发任何切换。
    <div
      className={"cstrip cstrip-" + edge}
      role="group"
      tabIndex={0}
      aria-label={t.sticker.expandBoard}
      data-tauri-drag-region
      onMouseEnter={scheduleExpand}
      onMouseLeave={cancelExpand}
      onMouseDown={cancelExpand}
      onFocus={onExpand}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onExpand();
        }
      }}
    >
      {/* 拖拽属性要下沉到子元素：Tauri 只认事件 target **自身**的 data-tauri-drag-region，
          点在点阵/占位上时 target 不是根节点，缺属性就拖不动。 */}
      <div className="cstrip-dots" ref={dotsRef} data-tauri-drag-region>
        {items.length === 0 ? (
          <span className="cstrip-empty" data-tauri-drag-region>
            <EyesMark />
          </span>
        ) : (
          items.map((l) => {
            // 状态判定与看板卡片同源（cardTone）：这里曾自写一套只看 DB status 的映射，
            // 漏掉 pending_review/screen_state，待审批会话被画成绿色运行点。
            // items 已过滤 connected，不会出现 offline。
            const tone = cardTone(l);
            const cls = tone === "error"
              ? "cstrip-error"
              : tone === "pending"
              ? "cstrip-pending"
              : tone === "running"
              ? "cstrip-running"
              : tone === "waiting"
              ? "cstrip-waiting"
              : "cstrip-on";
            const status = tone === "error"
              ? t.sticker.sessionError
              : tone === "pending"
              ? (l.pending_review ? t.pending[l.pending_review] : t.chat.status.pending)
              : tone === "running"
              ? t.badge.running
              : tone === "waiting"
              ? t.badge.waiting
              : t.sticker.online;
            return (
              <span
                key={l.session.id}
                className={"cstrip-dot " + cls}
                role="img"
                aria-label={`${l.task_title || t.sticker.waitingFirstInput} · ${status}`}
                data-tauri-drag-region
              />
            );
          })
        )}
      </div>
    </div>
  );
}

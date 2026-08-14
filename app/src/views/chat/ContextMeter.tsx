import { type useT } from "../../i18n";

/** token 数缩写：128000 → "128K"，1000000 → "1M"。 */
function shortTokens(n: number): string {
  if (n >= 1_000_000) {
    const m = n / 1_000_000;
    return (m >= 10 || Number.isInteger(m) ? Math.round(m) : m.toFixed(1)) + "M";
  }
  return Math.round(n / 1000) + "K";
}

/** 上下文用量环形进度条：环内百分比，环右侧「已用/总量」。60%↑黄、85%↑红。 */
export function ContextMeter({ pct, window, t }: { pct: number; window: number | null; t: ReturnType<typeof useT> }) {
  const clamped = Math.min(100, Math.max(0, pct));
  const R = 8;
  const C = 2 * Math.PI * R;
  const tone = pct >= 85 ? "is-full" : pct >= 60 ? "is-warn" : "";
  const usage = window ? `${shortTokens(window * pct / 100)}/${shortTokens(window)}` : null;
  return (
    <span className={"chat-context " + tone} data-tip={window ? t.chat.contextTip(pct, Math.round(window / 1000)) : t.chat.contextShort(pct)}>
      <span className="chat-context-ring">
        <svg width="20" height="20" viewBox="0 0 20 20">
          <circle className="chat-context-ring-bg" cx="10" cy="10" r={R} fill="none" strokeWidth="2.5" />
          <circle
            className="chat-context-ring-fg" cx="10" cy="10" r={R} fill="none" strokeWidth="2.5"
            strokeLinecap="round" strokeDasharray={C}
            strokeDashoffset={C * (1 - clamped / 100)} transform="rotate(-90 10 10)"
          />
        </svg>
        {/* 「100」三位数在 20px 环内会溢出贴边(实拍反馈),缩号收距才装得下。 */}
        <span className={"chat-context-pct" + (clamped >= 100 ? " is-wide" : "")}>{clamped}</span>
      </span>
      {usage && <span className="chat-context-usage">{usage}</span>}
    </span>
  );
}

// 各 tab 的空状态：图标 + 文案（+ 可选「新建会话」CTA）。
import { useT } from "../../i18n";
import type { Dict } from "../../i18n/zh";
import type { Tab } from "./types";
import { EmptyIcon } from "./icons";

function emptyCopy(tab: Tab, t: Dict): { title: string; hint: string | null } {
  switch (tab) {
    case "all": return { title: t.empty.allTitle, hint: t.empty.allHint };
    case "waiting": return { title: t.empty.waitingTitle, hint: t.empty.waitingHint };
    case "running": return { title: t.empty.runningTitle, hint: null };
  }
}

export function EmptyState({ tab, onNew, search }: { tab: Tab; onNew?: () => void; search?: boolean }) {
  const t = useT();
  // search=true：搜索有词但 0 结果的独立文案；调用方此时不传 onNew（无「新建会话」CTA）。
  const { title, hint } = search
    ? { title: t.empty.searchTitle, hint: t.empty.searchHint }
    : emptyCopy(tab, t);
  return (
    <div className="stk-empty">
      <span className="stk-empty-icon"><EmptyIcon tab={tab} /></span>
      <div className="stk-empty-title">{title}</div>
      {hint && <div className="stk-empty-hint">{hint}</div>}
      {onNew && (
        <button type="button" className="stk-empty-cta" data-testid="empty-new-cta" onClick={onNew}>
          {t.newSession.emptyCta}
        </button>
      )}
    </div>
  );
}

import { memo } from "react";
import { useT } from "../../i18n";
import { type ToolResultItem, type ToolUseItem } from "./shared";

/// 工具名的展示词。命中不了的原样回落成工具 id——比编一个错译好，但**常见工具**
/// 必须都译到：此前只译了 Bash/Read/Write 三家，一组活动摘要读起来是
/// 「运行终端 ×2 · Grep · WebFetch」这样的中英混排（7C-9）。
/// 判定顺序有讲究：web_search 含 "search"，网络类必须排在搜索类之前。
export function friendlyToolName(name: string, t: ReturnType<typeof useT>): string {
  const normalized = name.toLowerCase();
  if (normalized === "bash" || normalized === "exec" || normalized.includes("shell") || normalized.includes("terminal")) return t.chat.runTerminal;
  if (normalized.includes("web") || normalized.includes("fetch") || normalized.includes("http")) return t.chat.fetchWeb;
  if (normalized.includes("grep") || normalized.includes("glob") || normalized.includes("search")) return t.chat.searchFiles;
  if (normalized === "read" || normalized.includes("view_image")) return t.chat.readFile;
  if (normalized === "write" || normalized === "edit" || normalized.includes("patch") || normalized.includes("notebook")) return t.chat.editFile;
  if (normalized.includes("todo")) return t.chat.updateTodos;
  if (normalized === "task" || normalized.includes("agent")) return t.chat.runAgent;
  if (normalized === "ls" || normalized.includes("list_dir") || normalized.includes("listdir")) return t.chat.listDir;
  return name;
}

/// 按工具类型分图标：搜索 / 读文件 / 改文件 / 终端 / 网络 / 通用。
/// 一组操作若全是同一个 `>_`，用户在展开列表里无法一眼区分做了什么。
function ToolIcon({ name }: { name: string }) {
  const normalized = name.toLowerCase();
  const path = normalized.includes("grep") || normalized.includes("glob") || normalized.includes("search")
    ? "M10.5 3a7.5 7.5 0 1 0 4.55 13.46L20 21.4 21.4 20l-4.94-4.95A7.5 7.5 0 0 0 10.5 3zm0 2a5.5 5.5 0 1 1 0 11 5.5 5.5 0 0 1 0-11z"
    : normalized === "read" || normalized.includes("view_image") || normalized.includes("notebook")
    ? "M6 3h8l4 4v14H6zM14 3v5h5"
    : normalized === "write" || normalized === "edit" || normalized.includes("patch")
    ? "M4 20h4L19.5 8.5a2.1 2.1 0 0 0-3-3L5 17zM13.5 6.5l3 3"
    : normalized.includes("fetch") || normalized.includes("web") || normalized.includes("http")
    ? "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zm-9 9h18M12 3c2.5 2.4 3.8 5.6 3.8 9s-1.3 6.6-3.8 9c-2.5-2.4-3.8-5.6-3.8-9s1.3-6.6 3.8-9z"
    : "M4 17l6-6-6-6M12 19h8";
  return (
    <span className="chat-tool-icon">
      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8"><path d={path} /></svg>
    </span>
  );
}

/// memo 理由同 Message：item/result 都来自 reducer，未变化时引用稳定。
export const ToolActivity = memo(function ToolActivity({ item, result }: { item: ToolUseItem; result?: ToolResultItem }) {
  const t = useT();
  return (
    <details className={"chat-tool" + (result?.is_error ? " is-error" : "")}>
      <summary>
        <ToolIcon name={item.name} />
        <span className="chat-tool-name">{friendlyToolName(item.name, t)}</span>
        <span className="chat-tool-summary">{item.summary}</span>
        {/* 结果未到 = 工具还在跑：给行尾一个跳动指示，否则组头明明说「运行中」，
            展开后却看不出是哪条没跑完。 */}
        {/* 纯装饰:组头已经用一处 aria-label 播报「还有几条在跑」,每行再挂一个
            role=status,并行工具一多读屏就被轮番打断,什么也听不清(7C-9)。 */}
        {!result && <span className="chat-tool-pending" aria-hidden="true"><i /><i /><i /></span>}
        <span className="chat-tool-chevron">›</span>
      </summary>
      {/* summary 已经在标题行展示过，pre 里不再重复念一遍；展开只看结果本身。 */}
      <pre>{result ? (result.text || t.chat.toolNoOutput) : t.chat.toolRunning}</pre>
    </details>
  );
});

import { memo, useState, type ReactNode } from "react";
import { useT } from "../../i18n";
import type { ToolDetail } from "../../generated/contracts/ToolDetail";
import { highlightLines, languageForPath } from "./highlight";
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
  // Write 与 Edit 分开叫：终端里就是两个词，「写入 225 行」挂在「编辑文件」下面读着别扭。
  if (normalized === "write") return t.chat.writeFile;
  if (normalized === "edit" || normalized.includes("patch") || normalized.includes("notebook")) return t.chat.editFile;
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

/// 预览露出的行数上限。CC 终端里 Write 也只露十来行再写「+262 lines」——整篇摊开会把
/// 对话流顶掉几屏，而用户要的是「看一眼在写什么」，不是在对话页读文件。
const PREVIEW_LINES = 12;

/// 拆行：末尾的换行不算一行（"a\nb\n" 是两行），与后端 `lines()` 的口径一致。
function splitLines(text: string): string[] {
  const lines = text.split("\n");
  if (lines.length > 1 && lines[lines.length - 1] === "") lines.pop();
  return lines;
}

/// 带行号（或 diff 的 +/- 记号）的代码行。高亮复用文件面板那套 hljs——highlightLines
/// 逐行对齐，失败整体退纯文本，不会出现错位。
function CodeLines({ lines, lang, start = 1, tone, gutter }: {
  lines: string[];
  lang: string | null;
  start?: number;
  tone?: "add" | "del";
  /// 自定义行首记号（终端命令用 `$`）；缺省是行号，diff 态是 +/-。
  gutter?: (index: number) => string;
}) {
  const html = highlightLines(lines, lang);
  const mark = (index: number) => gutter ? gutter(index) : tone ? (tone === "add" ? "+" : "-") : String(start + index);
  return (
    <div className={"chat-tool-code" + (tone ? ` is-${tone}` : "")}>
      {html.map((line, index) => (
        <div className="chat-tool-line" key={index}>
          <span className="chat-tool-ln" aria-hidden="true">{mark(index)}</span>
          <span className="chat-tool-src" dangerouslySetInnerHTML={{ __html: line || " " }} />
        </div>
      ))}
    </div>
  );
}

/// 长正文只露前 PREVIEW_LINES 行，余下折成「还有 N 行 · 展开全部」；展开后可收回。
/// 后端截断过的正文在展开到底时提示「只保留了开头」——不能让用户以为文件就这么长。
function Preview({ text, lang, tone, truncated }: { text: string; lang: string | null; tone?: "add" | "del"; truncated?: boolean }) {
  const t = useT();
  const [all, setAll] = useState(false);
  const lines = splitLines(text);
  const shown = all ? lines : lines.slice(0, PREVIEW_LINES);
  const rest = lines.length - shown.length;
  const foldable = lines.length > PREVIEW_LINES;
  return (
    <>
      <CodeLines lines={shown} lang={lang} tone={tone} />
      {foldable && (
        <button type="button" className="chat-tool-more" onClick={() => setAll((value) => !value)}>
          {all ? t.chat.toolShowLess : `${t.chat.toolMoreLines(rest)} · ${t.chat.toolShowAll}`}
        </button>
      )}
      {truncated && all && <div className="chat-tool-note">{t.chat.toolTruncated}</div>}
    </>
  );
}

/// 标题行右侧的一枚小标：不展开也能看出规模——「写入 225 行」「-3 +12」「于 src」。
function detailMeta(detail: ToolDetail, t: ReturnType<typeof useT>): string | null {
  switch (detail.kind) {
    case "write":
      return t.chat.toolWroteLines(detail.lines);
    case "edit":
      return t.chat.toolEditDelta(splitLines(detail.old).length, splitLines(detail.new).length);
    case "command":
      return detail.description;
    case "read": {
      const parts: string[] = [];
      if (detail.offset != null) parts.push(t.chat.toolReadFrom(detail.offset));
      if (detail.limit != null) parts.push(t.chat.toolReadLimit(detail.limit));
      return parts.length ? parts.join(" · ") : null;
    }
    case "search":
      return detail.path ? t.chat.toolSearchIn(detail.path) : null;
  }
}

/// 展开后的详情正文：Write 是带行号的内容预览，Edit 是删/增两段，Bash 是 `$ 命令`。
/// Read / Search 的信息都已在标题行（路径 + 范围/模式），没有正文。
function DetailBody({ detail }: { detail: ToolDetail }): ReactNode {
  const t = useT();
  switch (detail.kind) {
    case "write":
      return <Preview text={detail.content} lang={languageForPath(detail.path)} truncated={detail.truncated} />;
    case "edit": {
      const lang = languageForPath(detail.path);
      return (
        <div className="chat-tool-diff">
          {detail.replace_all && <div className="chat-tool-note">{t.chat.toolReplaceAll}</div>}
          <Preview text={detail.old} lang={lang} tone="del" />
          <Preview text={detail.new} lang={lang} tone="add" truncated={detail.truncated} />
        </div>
      );
    }
    case "command":
      return <CodeLines lines={splitLines(detail.command)} lang="bash" gutter={(index) => (index === 0 ? "$" : "")} />;
    case "read":
    case "search":
      return null;
  }
}

/// memo 理由同 Message：item/result 都来自 reducer，未变化时引用稳定。
export const ToolActivity = memo(function ToolActivity({ item, result }: { item: ToolUseItem; result?: ToolResultItem }) {
  const t = useT();
  const detail = item.detail ?? null;
  // 挂载时还没回执 = 正在跑的那条：默认展开，命令/正文直接可见（用户要的正是
  // 「当前在做什么」）。只在挂载时定一次，之后开合归用户——React 只在 prop 变化时
  // 才碰 open 属性，值恒定就不会把用户收起的又弹开。
  const [initialOpen] = useState(() => !result && detail !== null);
  const meta = detail ? detailMeta(detail, t) : null;
  return (
    <details className={"chat-tool" + (result?.is_error ? " is-error" : "")} open={initialOpen || undefined}>
      <summary>
        <ToolIcon name={item.name} />
        <span className="chat-tool-name">{friendlyToolName(item.name, t)}</span>
        <span className="chat-tool-summary">{item.summary}</span>
        {meta && <span className="chat-tool-meta">{meta}</span>}
        {/* 结果未到 = 工具还在跑：给行尾一个跳动指示，否则组头明明说「运行中」，
            展开后却看不出是哪条没跑完。 */}
        {/* 纯装饰:组头已经用一处 aria-label 播报「还有几条在跑」,每行再挂一个
            role=status,并行工具一多读屏就被轮番打断,什么也听不清(7C-9)。 */}
        {!result && <span className="chat-tool-pending" aria-hidden="true"><i /><i /><i /></span>}
        <span className="chat-tool-chevron">›</span>
      </summary>
      <div className="chat-tool-body">
        {detail && <DetailBody detail={detail} />}
        {/* summary 已经在标题行展示过，pre 里不再重复念一遍；这里只看回执本身。 */}
        <pre>{result ? (result.text || t.chat.toolNoOutput) : t.chat.toolRunning}</pre>
      </div>
    </details>
  );
});

/// 活动组容器：挂载时组里还有没回执的调用（= 正在跑）就默认展开，用户当场看到正在做
/// 什么；跑完后保持用户留下的开合状态。open 只在挂载时定一次（理由见 ToolActivity）。
export function ActivityGroup({ defaultOpen, className, children }: { defaultOpen: boolean; className: string; children: ReactNode }) {
  const [initialOpen] = useState(() => defaultOpen);
  return <details className={className} open={initialOpen || undefined}>{children}</details>;
}

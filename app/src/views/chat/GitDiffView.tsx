import {
  memo,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
} from "react";
import {
  getGitFileDiff,
  listDirEntries,
  listFileOpeners,
  openPathWith,
  readFileText,
  readImagePreview,
  revealPathInFileManager,
  searchProjectFiles,
  type DirEntryDto,
  type FileOpenerDto,
  type FileTextDto,
  type GitDiffSummaryDto,
  type GitFileDiffDto,
  type ImagePreviewDto,
  type SearchResultDto,
} from "../../api";
import { FileMarkdown } from "./FileMarkdown";
import { useMenuPopup } from "../menu";
import { FolderPlusIcon } from "../sticker/icons";
import { folderName } from "../../paths";
import { pushEscLayer } from "../../escLayers";
import { useT } from "../../i18n";
import { formatBackendError } from "../../i18n/errors";
import type { Dict } from "../../i18n/zh";
import { buildFileTree, diffLineClass, type FileTreeNode } from "./gitDiff";
import { highlightLines, languageForPath } from "./highlight";

/** 后端对二进制 diff 给的占位行（见 git.rs untracked_file_diff / GitFileDiffDto 注释）。 */
const BINARY_PLACEHOLDER = "(binary file)";

/** 估算等宽网格下最宽一行的列数（ASCII=1、宽字符按 2、tab 按 8）。行容器开了
 *  content-visibility 后，跳过渲染的行不参与 pre 的 max-content 测量——不用这个
 *  估算钉住 min-width 的话，滚动时横向宽度/滚动条会随视口内最宽行跳变。 */
function estimateMaxColumns(lines: string[]): number {
  let max = 0;
  for (const line of lines) {
    let cols = 0;
    for (let i = 0; i < line.length; i += 1) {
      const code = line.charCodeAt(i);
      cols += code === 9 ? 8 : code < 0x1100 ? 1 : 2;
    }
    if (cols > max) max = cols;
  }
  return max;
}

/** cwd + rel 拼绝对路径（复制路径用）：cwd 是本机形态，Windows 下 rel 的正斜杠翻成反斜杠。 */
function joinAbsPath(cwd: string, rel: string): string {
  const base = cwd.replace(/[\\/]+$/, "");
  if (!rel) return base || cwd;
  const windows = cwd.includes("\\") || /^[A-Za-z]:/.test(cwd);
  return windows ? `${base}\\${rel.replace(/\//g, "\\")}` : `${base}/${rel}`;
}

/** rel 路径的小写扩展名（无扩展名/点文件返回空串），打开方式清单按它缓存。 */
function extKeyOf(rel: string): string {
  const base = rel.split("/").pop() ?? "";
  const dot = base.lastIndexOf(".");
  return dot > 0 ? base.slice(dot + 1).toLowerCase() : "";
}

/** 「文件」页签可直接预览的图片扩展名（与后端 image_mime 同表）。 */
const IMAGE_EXTS = new Set(["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "svg", "avif"]);
const MD_EXTS = new Set(["md", "markdown"]);

/** 文件的预览形态：图片直显 / markdown 排版渲染 / null = 纯文本（走高亮源码视图）。 */
function previewKindOf(rel: string): "image" | "markdown" | null {
  const ext = extKeyOf(rel);
  if (IMAGE_EXTS.has(ext)) return "image";
  if (MD_EXTS.has(ext)) return "markdown";
  return null;
}

/** 以 md 文件所在目录为基准解析文内相对引用（`/` 开头按仓库根）；越出仓库或
 *  解析失败返回 null（后端 resolve_inside 仍是最终防线）。 */
function resolveMdRel(baseDir: string, src: string): string | null {
  let clean = src.split(/[?#]/)[0];
  try {
    clean = decodeURIComponent(clean);
  } catch {
    return null;
  }
  if (!clean) return null;
  // 分段必须同时按 `/` 与 `\` 切:Windows 上文内引用可能写成 `..\..\x`,只按 `/` 切会让
  // 整串成为单个「段」,`..` 上跳守卫（out.pop）永不触发,穿越判定被绕过（后端 resolve_inside
  // 仍兜底,但前端不该把越界路径当合法解析出来）。根前缀也一并认反斜杠。
  const rooted = clean.startsWith("/") || clean.startsWith("\\");
  const body = rooted ? clean.replace(/^[\\/]+/, "") : baseDir ? `${baseDir}/${clean}` : clean;
  const out: string[] = [];
  for (const part of body.split(/[\\/]+/)) {
    if (!part || part === ".") continue;
    if (part === "..") {
      if (out.length === 0) return null;
      out.pop();
      continue;
    }
    out.push(part);
  }
  return out.length ? out.join("/") : null;
}

/** 打开方式清单按扩展名跨组件缓存：同类型文件共用一份，右键/切文件不重复枚举。 */
const openerCache = new Map<string, FileOpenerDto[]>();

/** 能打开 rel 的应用清单（rel 为 null/目录/无扩展名时恒空；失败静默为空——
 *  菜单仍有复制/定位/系统默认三项可用）。 */
function useFileOpeners(cwd: string, rel: string | null): FileOpenerDto[] {
  const ext = rel ? extKeyOf(rel) : "";
  const [openers, setOpeners] = useState<FileOpenerDto[]>(() => (ext ? openerCache.get(ext) ?? [] : []));
  useEffect(() => {
    if (!rel || !ext) {
      setOpeners([]);
      return;
    }
    const cached = openerCache.get(ext);
    if (cached) {
      setOpeners(cached);
      return;
    }
    let alive = true;
    listFileOpeners(cwd, rel)
      .then((list) => {
        openerCache.set(ext, list);
        if (alive) setOpeners(list);
      })
      .catch(() => {
        if (alive) setOpeners([]);
      });
    return () => {
      alive = false;
    };
  }, [cwd, rel, ext]);
  return openers;
}

type PathMenuItem = { key: string; label: string; icon: ReactNode; onSelect: () => void };

/** 预览里高亮首个命中片段（大小写不敏感；找不到原样返回）。 */
function markMatch(text: string, query: string): ReactNode {
  const needle = query.trim().toLowerCase();
  if (!needle) return text;
  const index = text.toLowerCase().indexOf(needle);
  if (index < 0) return text;
  return (
    <>
      {text.slice(0, index)}
      <mark className="chat-diff-hit-mark">{text.slice(index, index + needle.length)}</mark>
      {text.slice(index + needle.length)}
    </>
  );
}

/** 应用自带图标（data URL）优先，取不到落回通用编辑器线稿图标。 */
function OpenerIcon({ opener }: { opener: FileOpenerDto }) {
  return opener.icon ? <img className="path-app-ico" src={opener.icon} alt="" /> : <EditorIcon />;
}

/** 「打开」菜单条目：复制路径 / 文件管理器定位 / 分隔 / 编辑器们 / 系统默认应用。
 *  动作全部 fire-and-forget（打开失败无处安放弹窗，静默即可——与 openProjectDir 同约定）。 */
function buildPathMenuItems(
  cwd: string,
  rel: string,
  openers: FileOpenerDto[],
  t: Dict,
): (PathMenuItem | "sep")[] {
  return [
    {
      key: "copy",
      label: t.chat.diffCopyPath,
      icon: <CopyIcon />,
      onSelect: () => void navigator.clipboard?.writeText(joinAbsPath(cwd, rel)).catch(() => {}),
    },
    {
      key: "reveal",
      label: t.chat.diffReveal,
      icon: <RevealIcon />,
      onSelect: () => void revealPathInFileManager(cwd, rel).catch(() => {}),
    },
    "sep",
    ...openers.map((opener) => ({
      key: opener.id,
      label: opener.name,
      icon: <OpenerIcon opener={opener} />,
      onSelect: () => void openPathWith(cwd, rel, opener.id).catch(() => {}),
    })),
    {
      key: "default",
      label: t.chat.diffOpenWithDefault,
      icon: <AppWindowIcon />,
      onSelect: () => void openPathWith(cwd, rel, "default").catch(() => {}),
    },
  ];
}

/** 路径操作菜单：fixed 定位 + 钳位，点外/Esc/失焦/滚动关闭，roving focus——
 *  行为对齐 CardContextMenu（那份注释里的踩坑理由这里全部适用，不再复述）。 */
function PathMenu({ x, y, items, onClose }: {
  x: number;
  y: number;
  items: (PathMenuItem | "sep")[];
  onClose: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ left: x, top: y });
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const pad = 4;
    setPos({
      left: Math.max(pad, Math.min(x, window.innerWidth - el.offsetWidth - pad)),
      top: Math.max(pad, Math.min(y, window.innerHeight - el.offsetHeight - pad)),
    });
  }, [x, y]);
  useEffect(() => {
    const clickAway = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) {
        e.stopPropagation();
        onClose();
      }
    };
    const ctxAway = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) onClose();
    };
    const key = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.preventDefault();
      onClose();
    };
    const closeOnScroll = (e: Event) => {
      if (ref.current && e.target instanceof Node && ref.current.contains(e.target)) return;
      onClose();
    };
    const popLayer = pushEscLayer();
    document.addEventListener("click", clickAway, true);
    document.addEventListener("contextmenu", ctxAway, true);
    document.addEventListener("keydown", key);
    window.addEventListener("blur", onClose);
    window.addEventListener("scroll", closeOnScroll, true);
    return () => {
      popLayer();
      document.removeEventListener("click", clickAway, true);
      document.removeEventListener("contextmenu", ctxAway, true);
      document.removeEventListener("keydown", key);
      window.removeEventListener("blur", onClose);
      window.removeEventListener("scroll", closeOnScroll, true);
    };
  }, [onClose]);
  useEffect(() => {
    const prev = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    ref.current?.querySelector<HTMLElement>('[role="menuitem"]')?.focus();
    return () => {
      if (document.activeElement === document.body || ref.current?.contains(document.activeElement)) {
        prev?.focus();
      }
    };
  }, []);
  const onMenuKeyDown = (e: ReactKeyboardEvent) => {
    const nodes = Array.from(ref.current?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? []);
    if (nodes.length === 0) return;
    const cur = nodes.indexOf(document.activeElement as HTMLElement);
    let next: number;
    if (e.key === "ArrowDown") next = cur >= 0 ? (cur + 1) % nodes.length : 0;
    else if (e.key === "ArrowUp") next = cur >= 0 ? (cur - 1 + nodes.length) % nodes.length : nodes.length - 1;
    else if (e.key === "Home") next = 0;
    else if (e.key === "End") next = nodes.length - 1;
    else if ((e.key === "Enter" || e.key === " ") && cur >= 0) {
      e.preventDefault();
      (document.activeElement as HTMLElement).click();
      return;
    } else return;
    e.preventDefault();
    nodes[next]?.focus();
  };
  return (
    <div ref={ref} className="ctx-menu" role="menu" style={pos} onClick={(e) => e.stopPropagation()} onKeyDown={onMenuKeyDown}>
      {items.map((item, index) =>
        item === "sep" ? (
          <div key={`sep-${index}`} className="ctx-sep" role="separator" />
        ) : (
          <button
            key={item.key}
            type="button"
            role="menuitem"
            className="ctx-item"
            onClick={() => {
              item.onSelect();
              onClose();
            }}
          >
            {item.icon}
            {item.label}
          </button>
        ),
      )}
    </div>
  );
}

/** 文件头「打开」分体按钮：主键一键用系统首选应用（清单为空则系统默认关联）打开，
 *  箭头展开完整菜单（复制路径/文件管理器定位/各打开方式）。 */
function OpenSplitButton({ cwd, rel }: { cwd: string; rel: string }) {
  const t = useT();
  const openers = useFileOpeners(cwd, rel);
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const wrapRef = useRef<HTMLDivElement>(null);
  const primary = openers[0] ?? null;
  return (
    <div className="chat-diff-open" ref={wrapRef}>
      <button
        type="button"
        className="chat-diff-open-main"
        data-tip={primary ? primary.name : t.chat.diffOpenWithDefault}
        onClick={() => void openPathWith(cwd, rel, primary?.id ?? "default").catch(() => {})}
      >
        {primary ? <OpenerIcon opener={primary} /> : <OpenIcon />}
        {t.chat.diffOpen}
      </button>
      <button
        type="button"
        className="chat-diff-open-caret"
        aria-label={t.chat.diffOpenWith}
        aria-haspopup="menu"
        aria-expanded={!!menu}
        onClick={() => {
          if (menu) {
            setMenu(null);
            return;
          }
          const rect = wrapRef.current?.getBoundingClientRect();
          if (rect) setMenu({ x: rect.left, y: rect.bottom + 4 });
        }}
      >
        <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
          <path d="M6 9l6 6 6-6" />
        </svg>
      </button>
      {menu && (
        <PathMenu x={menu.x} y={menu.y} items={buildPathMenuItems(cwd, rel, openers, t)} onClose={() => setMenu(null)} />
      )}
    </div>
  );
}

/** 树行/结果行右键菜单：按需拉该文件类型的打开方式清单（目录不拉——只有
 *  复制/定位/系统默认三项）；清单异步到位后条目就地补齐（有扩展名缓存，通常瞬时）。 */
function PathContextMenu({ cwd, rel, isDir, x, y, onClose }: {
  cwd: string;
  rel: string;
  isDir: boolean;
  x: number;
  y: number;
  onClose: () => void;
}) {
  const t = useT();
  const openers = useFileOpeners(cwd, isDir ? null : rel);
  return <PathMenu x={x} y={y} items={buildPathMenuItems(cwd, rel, openers, t)} onClose={onClose} />;
}

type FetchEntry<T> = { loading: boolean; error: string | null; data: T | null };

/**
 * 「文件 / 更改」停靠面板：挂在 .chat-body 行内的右列（对话区收缩让位，非覆盖层）。
 * 「更改」页签仅仓库会话可见（summary?.isRepo）；非仓库会话打开时只有「文件」页签。
 * memo：折叠只是 display:none 不卸载，不隔断的话 ChatWindow 每次击键都要对面板里
 * 可能上千行的文件 DOM 做全量 reconcile；父层回调已换稳定引用配合生效。
 */
/** 面板头行的仓切换 + 附加目录管理菜单(用户指定收拢于此):行点选切换激活仓(当前
 *  项打勾),附加目录行尾 ✕ 移除(落库侧,下次恢复不再带上),菜单尾「附加目录」添加。 */
function DirSwitcher({ cwd, dirs, onDirChange, onAddDir, onRemoveDir }: {
  cwd: string;
  dirs: string[];
  onDirChange: (dir: string) => void;
  onAddDir: (() => void) | null;
  onRemoveDir: (dir: string) => void;
}) {
  const t = useT();
  const { open, setOpen, pos, ref, btnRef, menuRef, toggle, onKeyDown } = useMenuPopup({ align: "left" });
  const pick = (action: () => void) => { setOpen(false); btnRef.current?.focus(); action(); };
  return (
    <div className="dd chat-diff-dirs" ref={ref} onKeyDown={onKeyDown}>
      <button ref={btnRef} type="button" className="chat-diff-dirs-btn" aria-haspopup="menu" aria-expanded={open} data-tip={`${t.chat.dirsSection} · ${cwd}`} onClick={toggle}>
        <FolderIcon />
        <span className="chat-diff-dirs-name">{folderName(cwd)}</span>
        <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><polyline points="6 9 12 15 18 9" /></svg>
      </button>
      {open && (
        <div className="dd-menu chat-dirs-menu" role="menu" ref={menuRef} style={{ position: "fixed", top: pos.top, bottom: pos.bottom, left: pos.left }}>
          {dirs.map((dir, i) => {
            // 首项是主仓(会话根,不可移除);其余为附加目录。
            const isMain = i === 0;
            const active = dir === cwd;
            return (
              // 行是 div 而非 button:× 要住在行**内部**(用户指定),button 套 button
              // 非法。键盘可达靠 role+tabIndex,Enter/Space 由 useMenuPopup 显式 click。
              // 选中态 = 整行 active 背景(is-active),不再用勾。
              // 菜单内刻意用原生 title：TooltipLayer 对 .dd-menu 区域静默（自绘提示会糊在菜单上）。
              <div
                key={dir}
                role="menuitem"
                tabIndex={-1}
                className={"dd-item chat-dirs-item" + (active ? " is-active" : "")}
                title={dir}
                onClick={() => pick(() => onDirChange(dir))}
              >
                <span className="dd-label">{folderName(dir)}</span>
                {!isMain && (
                  <button
                    type="button"
                    className="chat-dirs-x"
                    aria-label={t.newSession.extraDirRemove}
                    title={t.newSession.extraDirRemove}
                    onClick={(e) => { e.stopPropagation(); pick(() => onRemoveDir(dir)); }}
                  >
                    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" aria-hidden="true"><path d="M18 6 6 18M6 6l12 12" /></svg>
                  </button>
                )}
              </div>
            );
          })}
          {onAddDir && (
            <>
              <div className="sf-sep" role="separator" />
              <button type="button" role="menuitem" className="dd-item" onClick={() => pick(onAddDir)}>
                <span className="chat-title-action-ico"><FolderPlusIcon /></span>
                <span className="dd-label">{t.chat.addExtraDir}</span>
              </button>
            </>
          )}
        </div>
      )}
    </div>
  );
}

export const GitDiffView = memo(GitDiffViewImpl);
function GitDiffViewImpl({ cwd, dirs, onDirChange, onAddDir, onRemoveDir, summary, width, collapsed, maximized, onCollapse, onToggleMaximize }: {
  /** 面板的激活仓:主仓或某个附加目录(--add-dir)。文件树与更改都对着它。 */
  cwd: string;
  /** 会话的全部目录(主仓在首)。仓切换与附加目录管理都在头行的 DirSwitcher。 */
  dirs: string[];
  onDirChange: (dir: string) => void;
  /** 中途附加目录;会话 provider 不支持时传 null 隐藏该动作。 */
  onAddDir: (() => void) | null;
  /** 移除附加目录(落库侧,下次恢复不再带上)。 */
  onRemoveDir: (dir: string) => void;
  summary: GitDiffSummaryDto | null;
  /** 停靠宽度（px，ChatWindow 侧的分栏柄拖动改写）；最大化时忽略（面板占满整宽）。 */
  width: number;
  /** 折叠 = 整面板 display:none（不卸载，内部状态全保留）。 */
  collapsed: boolean;
  /** 最大化 = 隐藏对话卡（.chat-main），面板占满整宽。 */
  maximized: boolean;
  onCollapse: () => void;
  onToggleMaximize: () => void;
}) {
  const t = useT();
  const changesAvailable = !!summary?.isRepo;
  const [tab, setTab] = useState<"changes" | "files">(changesAvailable ? "changes" : "files");

  // Esc 折叠面板。挂捕获段并拦住传播（ChatWindow 有全局 Esc=拒绝审批的监听，
  // 且焦点可能在 composer 输入框里——那里的 Esc 处理器不查层栈，只有捕获段
  // 截停才拦得住），同时注册 Esc 层作为统一让位判据——与灯箱同一套双保险。
  // 折叠期间不挂：面板不可见时 Esc 归对话页（含 Esc=拒绝审批）原语义。
  useEffect(() => {
    if (collapsed) return;
    const popLayer = pushEscLayer();
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.stopPropagation();
      onCollapse();
    };
    window.addEventListener("keydown", onKey, true);
    return () => {
      popLayer();
      window.removeEventListener("keydown", onKey, true);
    };
  }, [collapsed, onCollapse]);

  return (
    <aside
      className={"chat-diff-panel" + (collapsed ? " is-collapsed" : "")}
      style={maximized ? undefined : { width }}
      aria-label={t.chat.diff}
    >
      {/* 最大化时对话卡（含承载窗口拖拽区的 .chat-bar）整体隐藏，
          面板头行因此也要是拖拽区；行内按钮不带该属性，点击不受影响（同 .chat-bar 约定）。 */}
      <header className="chat-diff-head" data-tauri-drag-region>
        {/* 仓切换 + 附加目录管理(用户指定收拢于此):选中的仓即 cwd,文件树与更改整体
            跟随;summary 由 ChatWindow 按激活仓重拉,非 git 仓自动只剩「文件」页签。
            多仓或支持附加时才渲染——单仓且不支持附加的 agent 界面零变化。 */}
        {(dirs.length > 1 || onAddDir) && (
          <DirSwitcher cwd={cwd} dirs={dirs} onDirChange={onDirChange} onAddDir={onAddDir} onRemoveDir={onRemoveDir} />
        )}
        <div className="chat-diff-tabs">
          <button type="button" className={tab === "files" || !changesAvailable ? "is-active" : ""} onClick={() => setTab("files")}>
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />
            </svg>
            {t.chat.filesTab}
          </button>
          {changesAvailable && (
            <button type="button" className={tab === "changes" ? "is-active" : ""} onClick={() => setTab("changes")}>
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                <circle cx="6" cy="6" r="2.5" /><circle cx="6" cy="18" r="2.5" /><circle cx="18" cy="9" r="2.5" />
                <path d="M6 8.5v7" /><path d="M18 11.5c0 4-4 4.5-9.2 4.6" />
              </svg>
              {t.chat.diffChangesTab}
            </button>
          )}
        </div>
        {/* 分支名刻意不显示：长分支名（feat/xxx-日期-作者）会把头行挤到换行。 */}
        <span className="chat-diff-spacer" />
        {/* 最大化/还原：隐藏对话卡让面板占满整宽。图标走对角箭头——展开时两角向外，
            最大化后两角向内（还原）。 */}
        <button
          type="button"
          className="chat-diff-action"
          data-tip={maximized ? t.chat.diffRestore : t.chat.diffMaximize}
          aria-label={maximized ? t.chat.diffRestore : t.chat.diffMaximize}
          onClick={onToggleMaximize}
        >
          {maximized ? (
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <path d="M4 14h6v6" /><path d="M20 10h-6V4" /><path d="M14 10l7-7" /><path d="M3 21l7-7" />
            </svg>
          ) : (
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <path d="M15 3h6v6" /><path d="M9 21H3v-6" /><path d="M21 3l-7 7" /><path d="M3 21l7-7" />
            </svg>
          )}
        </button>
        {/* 折叠：面板收成隐藏（不卸载，状态保留）。图标与侧栏开关同款「面板+分隔线」，
            镜像到右缘（面板在右侧，线靠右）。 */}
        <button type="button" className="chat-diff-action" data-tip={t.chat.diffCollapse} aria-label={t.chat.diffCollapse} onClick={onCollapse}>
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinejoin="round" aria-hidden="true">
            <rect x="3.5" y="4.5" width="17" height="15" rx="3" />
            <path d="M14.5 4.5v15" />
          </svg>
        </button>
      </header>
      {/* key=cwd:切仓即重挂,树展开态/选中文件不跨仓串场。 */}
      {tab === "changes" && changesAvailable && summary
        ? <ChangesView key={cwd} cwd={cwd} summary={summary} />
        : <FilesView key={cwd} cwd={cwd} />}
    </aside>
  );
}

/** 「更改」页签：可搜索的变更目录树 + 单文件 diff（返回行 + 着色行）。 */
function ChangesView({ cwd, summary }: { cwd: string; summary: GitDiffSummaryDto }) {
  const t = useT();
  // 当前查看的文件（null = 文件树形态）；diff 结果按路径缓存，往返切换不重复拉取。
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  const [entries, setEntries] = useState<Map<string, FetchEntry<GitFileDiffDto>>>(new Map());
  // 图片改动的预览缓存(与 FilesView 的图片通道同款):git 对二进制只会说 "(binary file)",
  // 但图片完全可以直接把工作区当前版本渲染出来——「二进制文件,无法显示 diff」对一张
  // 刚生成的截图毫无帮助(实拍反馈)。
  const [images, setImages] = useState<Map<string, FetchEntry<ImagePreviewDto>>>(new Map());
  useEffect(() => {
    if (!selectedPath || previewKindOf(selectedPath) !== "image" || images.has(selectedPath)) return;
    const rel = selectedPath;
    setImages((prev) => new Map(prev).set(rel, { loading: true, error: null, data: null }));
    readImagePreview(cwd, rel)
      .then((data) => setImages((prev) => new Map(prev).set(rel, { loading: false, error: null, data })))
      .catch((error) => setImages((prev) => new Map(prev).set(rel, { loading: false, error: formatBackendError(error, t.locale), data: null })));
  }, [selectedPath, images, cwd, t.locale]);
  const [search, setSearch] = useState("");
  // 目录树默认全部展开，这里只记录被手动折叠的目录路径。
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  // 树行右键的路径菜单（复制/定位/打开方式）。已删除文件（status D）盘上无实体，不出菜单。
  const [ctxMenu, setCtxMenu] = useState<{ x: number; y: number; rel: string; isDir: boolean } | null>(null);
  const openCtxMenu = (event: ReactMouseEvent, rel: string, isDir: boolean) => {
    event.preventDefault();
    setCtxMenu({ x: event.clientX, y: event.clientY, rel, isDir });
  };

  // summary 每次刷新（面板重新可见时会重拉）都作废缓存：agent 随时在提交/改写文件，
  // 旧 diff 相对旧清单、旧图片是改写前的字节，继续用会展示过期内容。images 与 entries
  // 必须一起清——只清 entries 会让「改过的图重开仍显旧图」（与文本 diff 同一类 stale）。
  useEffect(() => {
    setEntries(new Map());
    setImages(new Map());
  }, [summary]);

  const fileByPath = useMemo(() => new Map(summary.files.map((file) => [file.path, file])), [summary.files]);

  const tree = useMemo(() => {
    const query = search.trim().toLowerCase();
    const files = query
      ? summary.files.filter((file) => file.path.toLowerCase().includes(query))
      : summary.files;
    return buildFileTree(files);
  }, [summary.files, search]);

  // 按需拉单文件 diff：一次会话改动可以很大，不随摘要预取全部。
  // 已缓存（含加载中/失败）的路径跳过，失败结果回树后重新点开即重试。
  useEffect(() => {
    if (!selectedPath || entries.has(selectedPath)) return;
    const file = fileByPath.get(selectedPath);
    if (!file) return;
    const path = selectedPath;
    setEntries((prev) => new Map(prev).set(path, { loading: true, error: null, data: null }));
    getGitFileDiff(cwd, path, file.status === "U")
      .then((data) => setEntries((prev) => new Map(prev).set(path, { loading: false, error: null, data })))
      .catch((error) => setEntries((prev) => new Map(prev).set(path, { loading: false, error: formatBackendError(error, t.locale), data: null })));
  }, [selectedPath, entries, cwd, fileByPath]);

  const toggleDir = (path: string) => {
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  };

  const selected = selectedPath ? fileByPath.get(selectedPath) ?? null : null;
  const selectedEntry = selectedPath ? entries.get(selectedPath) ?? null : null;

  // diff 文本高亮（useMemo 按 diff 对象缓存，切换/重渲染不重算）：
  // meta（diff --git/index/---/+++）与 hunk（@@）行返回 null 占位——保持原有配色不高亮。
  // 正文行不逐行独立高亮，而是拆成新旧两篇整体送 hljs（跨行 token 状态保留）：
  // del 行只属旧文，add 行只属新文，上下文行两篇都进（作双方的语法上下文，
  // 渲染取新文侧结果）。hunk 之间仍是拼接断层——可能错色，但比逐行强得多。
  const activeDiff = selectedEntry?.data ?? null;
  const highlightedDiff = useMemo(() => {
    if (!activeDiff || activeDiff.diff.trim() === BINARY_PLACEHOLDER) return null;
    const lang = languageForPath(activeDiff.path);
    const rawLines = activeDiff.diff.split("\n");
    const oldIndices: number[] = [];
    const oldLines: string[] = [];
    const newIndices: number[] = [];
    const newLines: string[] = [];
    rawLines.forEach((line, index) => {
      const cls = diffLineClass(line);
      if (cls === "is-meta" || cls === "is-hunk" || line === "") return;
      const content = cls === "is-add" || cls === "is-del" || line.startsWith(" ") ? line.slice(1) : line;
      if (cls === "is-del") {
        oldIndices.push(index);
        oldLines.push(content);
      } else if (cls === "is-add") {
        newIndices.push(index);
        newLines.push(content);
      } else {
        // 上下文行：-1 标记旧文侧「只当上下文、不取结果」。
        oldIndices.push(-1);
        oldLines.push(content);
        newIndices.push(index);
        newLines.push(content);
      }
    });
    const result: (string | null)[] = rawLines.map(() => null);
    highlightLines(newLines, lang).forEach((html, i) => {
      result[newIndices[i]] = html;
    });
    highlightLines(oldLines, lang).forEach((html, i) => {
      const at = oldIndices[i];
      if (at >= 0) result[at] = html;
    });
    return result;
  }, [activeDiff]);
  const diffMaxCols = useMemo(
    () => (activeDiff && activeDiff.diff.trim() !== BINARY_PLACEHOLDER ? estimateMaxColumns(activeDiff.diff.split("\n")) : 0),
    [activeDiff],
  );

  const renderTree = (nodes: FileTreeNode[], depth: number): ReactNode =>
    nodes.map((node) =>
      node.children ? (
        <div key={node.path}>
          <button
            type="button"
            className="chat-diff-tree-dir"
            style={{ paddingLeft: `calc(var(--sp-8) + ${depth * 14}px)` }}
            onClick={() => toggleDir(node.path)}
            onContextMenu={(event) => openCtxMenu(event, node.path, true)}
          >
            <svg
              className={collapsed.has(node.path) ? "chat-diff-chev is-collapsed" : "chat-diff-chev"}
              width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"
            >
              <path d="M6 9l6 6 6-6" />
            </svg>
            <FolderIcon />
            <span className="chat-diff-path">{node.name}</span>
          </button>
          {!collapsed.has(node.path) && renderTree(node.children, depth + 1)}
        </div>
      ) : (
        <button
          key={node.path}
          type="button"
          className="chat-diff-tree-file"
          style={{ paddingLeft: `calc(var(--sp-8) + ${depth * 14}px)` }}
          onClick={() => node.file && setSelectedPath(node.file.path)}
          onContextMenu={node.file && node.file.status !== "D" ? (event) => openCtxMenu(event, node.file!.path, false) : undefined}
        >
          {node.file && (
            <span className={`chat-diff-status is-${node.file.status.toLowerCase()}`}>{node.file.status}</span>
          )}
          <span className="chat-diff-path">{node.name}</span>
        </button>
      ),
    );

  const fileView = (() => {
    if (!selectedEntry || selectedEntry.loading) return <div className="chat-diff-empty">{t.chat.diffLoading}</div>;
    if (selectedEntry.error) return <div className="chat-diff-empty is-error">{selectedEntry.error}</div>;
    const fileDiff = selectedEntry.data;
    if (!fileDiff || fileDiff.diff.trim() === BINARY_PLACEHOLDER) {
      // 图片不甩「二进制」冷话:渲染工作区当前版本(新增/修改后的样子)。旧版对比
      // 做不了(git 不给二进制 diff),看得到现状已经回答了「这张图是什么」。
      // 已删除的文件工作区没有内容,才落回二进制占位文案。
      const status = summary.files.find((file) => file.path === selectedPath)?.status;
      if (selectedPath && previewKindOf(selectedPath) === "image" && status !== "D") {
        const entry = images.get(selectedPath);
        if (!entry || entry.loading) return <div className="chat-diff-empty">{t.chat.diffLoading}</div>;
        if (entry.error) return <div className="chat-diff-empty is-error">{entry.error}</div>;
        if (!entry.data?.dataUrl) return <div className="chat-diff-empty">{t.chat.diffImageTooLarge}</div>;
        return (
          <div className="chat-diff-scroll chat-diff-imgwrap">
            <img className="chat-diff-img" src={entry.data.dataUrl} alt={selectedPath} />
          </div>
        );
      }
      return <div className="chat-diff-empty">{t.chat.diffBinary}</div>;
    }
    // 空 diff ≠ 出错:清单快照落后于 agent 的提交/撤销时就会出现——给明确解释,
    // 不再渲染成一片看似坏掉的空白(实拍反馈)。
    if (fileDiff.diff.trim() === "") {
      return <div className="chat-diff-empty">{t.chat.diffFileUpToDate}</div>;
    }
    return (
      <>
        <div className="chat-diff-scroll">
          {/* min-width 钉住横向宽度（+4ch 给符号列/内边距），理由见 estimateMaxColumns。 */}
          <pre className="chat-diff-text" style={{ minWidth: `max(100%, ${diffMaxCols + 4}ch)` }}>
            {fileDiff.diff.split("\n").map((line, index) => {
              const cls = diffLineClass(line);
              const html = highlightedDiff?.[index];
              // meta/hunk/空行：保持原有整行配色（meta 暗、@@ 主题色），不进符号列结构。
              if (html == null) return <div key={index} className={cls || undefined}>{line || " "}</div>;
              // add/del/上下文行：符号列（+/−/空，不可选中）+ 逐 token 着色的正文。
              const sign = cls === "is-add" ? "+" : cls === "is-del" ? "−" : "";
              return (
                <div key={index} className={cls || undefined}>
                  <span className="chat-diff-sign">{sign || " "}</span>
                  <span dangerouslySetInnerHTML={{ __html: html || " " }} />
                </div>
              );
            })}
          </pre>
        </div>
        {fileDiff.truncated && <div className="chat-diff-truncated">{t.chat.diffTruncated}</div>}
      </>
    );
  })();

  if (!summary.gitAvailable) return <div className="chat-diff-empty">{t.chat.diffNoGit}</div>;
  if (!summary.isRepo) return <div className="chat-diff-empty">{t.chat.diffNoRepo}</div>;
  if (summary.files.length === 0) return <div className="chat-diff-empty">{t.chat.diffNoChanges}</div>;
  if (selectedPath) {
    return (
      <>
        <div className="chat-diff-filehead">
          <BackButton label={t.chat.diffBack} onClick={() => setSelectedPath(null)} />
          {selected && <span className={`chat-diff-status is-${selected.status.toLowerCase()}`}>{selected.status}</span>}
          <span className="chat-diff-path" data-tip={selectedPath}>{selectedPath}</span>
          {selected && selected.status !== "D" && <OpenSplitButton cwd={cwd} rel={selectedPath} />}
        </div>
        {fileView}
      </>
    );
  }
  return (
    <>
      {/* 分支名放页签内部而非面板头行：这里整行宽度可用，长分支名截断不挤布局。 */}
      {summary.branch && (
        <div className="chat-diff-branchline" data-tip={summary.branch}>
          <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            <circle cx="6" cy="6" r="2.5" /><circle cx="6" cy="18" r="2.5" /><circle cx="18" cy="9" r="2.5" />
            <path d="M6 8.5v7" /><path d="M18 11.5c0 4-4 4.5-9.2 4.6" />
          </svg>
          <span>{summary.branch}</span>
        </div>
      )}
      <input
        type="text"
        className="chat-diff-search"
        placeholder={t.chat.diffSearchPlaceholder}
        value={search}
        // IME 合成守卫：与侧栏搜索框同款，拼音中间态不下发。
        onChange={(event) => {
          if ((event.nativeEvent as InputEvent).isComposing) return;
          setSearch(event.target.value);
        }}
        onCompositionEnd={(event) => setSearch(event.currentTarget.value)}
      />
      <div className="chat-diff-tree">{renderTree(tree, 0)}</div>
      {ctxMenu && (
        <PathContextMenu
          cwd={cwd}
          rel={ctxMenu.rel}
          isDir={ctxMenu.isDir}
          x={ctxMenu.x}
          y={ctxMenu.y}
          onClose={() => setCtxMenu(null)}
        />
      )}
    </>
  );
}

/// 文件/图片缓存的上限。面板折叠时**不卸载**（打开的文件、树展开、diff 缓存全部保留，
/// 见 ChatWindow 的注释），而 ChangesView 有 `[summary]` 重置、FilesView 一个都没有——
/// 一次会话里翻几十个文件加十几张截图，就是几十 MB 只增不减地挂到换会话为止（图片还是
/// 1.33 倍体积的 base64 data URL）。20 条足够覆盖「来回对照几个文件」的实际用法。
const FILE_CACHE_MAX = 20;

/// 写入并按 LRU 裁剪：命中/重写都挪到队尾，超限从队首淘汰。Map 保持插入序，
/// 先 delete 再 set 即可把它变成访问序。
function cappedCache<T>(prev: Map<string, T>, key: string, value: T): Map<string, T> {
  const next = new Map(prev);
  next.delete(key);
  next.set(key, value);
  while (next.size > FILE_CACHE_MAX) {
    const oldest = next.keys().next().value;
    if (oldest === undefined) break;
    next.delete(oldest);
  }
  return next;
}

/** 「文件」页签：可搜索（文件名 + 内容）的懒加载目录树 + 单文件文本查看（返回行 + pre 文本）。 */
function FilesView({ cwd }: { cwd: string }) {
  const t = useT();
  // 目录清单与文件文本都按 rel 路径缓存：折叠/回退再展开不重复拉取。
  const [dirs, setDirs] = useState<Map<string, FetchEntry<DirEntryDto[]>>>(new Map());
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  const [files, setFiles] = useState<Map<string, FetchEntry<FileTextDto>>>(new Map());
  // 图片预览按路径缓存（data URL）；与文本缓存分开——同一 svg 两种形态都可能要。
  const [images, setImages] = useState<Map<string, FetchEntry<ImagePreviewDto>>>(new Map());
  // 预览形态文件（md/svg）的「源码」切换；打开新文件回到预览态。
  const [sourceMode, setSourceMode] = useState(false);
  // 名称+内容搜索：防抖后发后端，seq 挡乱序回包；查询清空即回树形态（结果状态一并清）。
  const [search, setSearch] = useState("");
  const [searchState, setSearchState] = useState<FetchEntry<SearchResultDto>>({ loading: false, error: null, data: null });
  const searchSeq = useRef(0);
  // markdown 预览里相对路径图片 → data URL（logo/截图等仓库内引用；http(s) 由
  // FileMarkdown 直用）。按打开的 md 文件所在目录做基准。
  const resolveMdImage = useCallback(
    (src: string) => {
      const baseDir = selectedPath ? selectedPath.split("/").slice(0, -1).join("/") : "";
      const rel = resolveMdRel(baseDir, src);
      if (!rel) return Promise.resolve(null);
      return readImagePreview(cwd, rel)
        .then((preview) => preview.dataUrl ?? null)
        .catch(() => null);
    },
    [cwd, selectedPath],
  );
  // 内容命中组的折叠状态（按 rel 路径记被折叠的组；换关键词后旧路径残留无害）。
  const [collapsedHits, setCollapsedHits] = useState<Set<string>>(new Set());
  const toggleHitGroup = (rel: string) => {
    setCollapsedHits((prev) => {
      const next = new Set(prev);
      if (next.has(rel)) next.delete(rel);
      else next.add(rel);
      return next;
    });
  };
  // 内容命中跳行：打开文件后滚到目标行并常亮标出（再从树里正常打开文件时清除）。
  const [target, setTarget] = useState<{ rel: string; line: number } | null>(null);
  const preRef = useRef<HTMLPreElement>(null);
  // 树行/结果行右键的路径菜单（复制/定位/打开方式）。
  const [ctxMenu, setCtxMenu] = useState<{ x: number; y: number; rel: string; isDir: boolean } | null>(null);
  const openCtxMenu = (event: ReactMouseEvent, rel: string, isDir: boolean) => {
    event.preventDefault();
    setCtxMenu({ x: event.clientX, y: event.clientY, rel, isDir });
  };

  const query = search.trim();
  // 单个 ASCII 字符不触发（后端同样拒：内容命中噪声太大）；CJK 单字放行。
  const searchActive = query.length >= 2 || /[^\x00-\x7f]/.test(query);

  useEffect(() => {
    if (!searchActive) {
      searchSeq.current += 1;
      setSearchState({ loading: false, error: null, data: null });
      return;
    }
    const seq = ++searchSeq.current;
    setSearchState((prev) => ({ ...prev, loading: true }));
    const timer = window.setTimeout(() => {
      searchProjectFiles(cwd, query)
        .then((data) => {
          if (searchSeq.current === seq) setSearchState({ loading: false, error: null, data });
        })
        .catch((error) => {
          if (searchSeq.current === seq) setSearchState({ loading: false, error: formatBackendError(error, t.locale), data: null });
        });
    }, 250);
    return () => window.clearTimeout(timer);
  }, [cwd, query, searchActive]);

  // 打开文件的逐行高亮 HTML（useMemo 按文件对象缓存；二进制/未打开为 null）。
  const selectedFile = selectedPath ? files.get(selectedPath)?.data ?? null : null;
  const highlightedFile = useMemo(() => {
    if (!selectedFile || selectedFile.binary) return null;
    return highlightLines(selectedFile.text.split("\n"), languageForPath(selectedFile.relPath));
  }, [selectedFile]);
  const fileMaxCols = useMemo(
    () => (selectedFile && !selectedFile.binary ? estimateMaxColumns(selectedFile.text.split("\n")) : 0),
    [selectedFile],
  );

  // 跳行滚动：等文件加载 + 高亮渲染完成后再滚（children 与行号一一对应）。
  useEffect(() => {
    if (!target || selectedPath !== target.rel || !highlightedFile) return;
    preRef.current?.children[target.line - 1]?.scrollIntoView({ block: "center" });
  }, [target, selectedPath, highlightedFile]);

  const fetchDir = (rel: string) => {
    setDirs((prev) => new Map(prev).set(rel, { loading: true, error: null, data: null }));
    listDirEntries(cwd, rel)
      .then((data) => setDirs((prev) => new Map(prev).set(rel, { loading: false, error: null, data })))
      .catch((error) => setDirs((prev) => new Map(prev).set(rel, { loading: false, error: formatBackendError(error, t.locale), data: null })));
  };

  // 根目录进面板即拉。
  useEffect(() => {
    if (!dirs.has("")) fetchDir("");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cwd, dirs]);

  // 展开时按需拉子目录；上次失败的条目借「重新展开」重试。
  const toggleDir = (rel: string) => {
    const cached = dirs.get(rel);
    if (!expanded.has(rel) && (!cached || cached.error)) fetchDir(rel);
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(rel)) next.delete(rel);
      else next.add(rel);
      return next;
    });
  };

  const fetchText = (rel: string) => {
    if (files.has(rel)) return;
    setFiles((prev) => cappedCache(prev, rel, { loading: true, error: null, data: null }));
    readFileText(cwd, rel)
      .then((data) => setFiles((prev) => cappedCache(prev, rel, { loading: false, error: null, data })))
      .catch((error) => setFiles((prev) => cappedCache(prev, rel, { loading: false, error: formatBackendError(error, t.locale), data: null })));
  };

  const fetchImage = (rel: string) => {
    if (images.has(rel)) return;
    setImages((prev) => cappedCache(prev, rel, { loading: true, error: null, data: null }));
    readImagePreview(cwd, rel)
      .then((data) => setImages((prev) => cappedCache(prev, rel, { loading: false, error: null, data })))
      .catch((error) => setImages((prev) => cappedCache(prev, rel, { loading: false, error: formatBackendError(error, t.locale), data: null })));
  };

  const openFile = (rel: string, line: number | null = null) => {
    setTarget(line ? { rel, line } : null);
    setSourceMode(false);
    setSelectedPath(rel);
    // 图片走预览通道（读盘转 data URL），其余（含 markdown）走文本通道。
    if (previewKindOf(rel) === "image") fetchImage(rel);
    else fetchText(rel);
  };

  // 点中目录命中：回树形态并逐级展开到该目录（沿途未拉取/拉取失败的目录顺手补拉）。
  const revealDirFromSearch = (rel: string) => {
    setSearch("");
    const ancestors: string[] = [];
    let acc = "";
    for (const part of rel.split("/")) {
      acc = acc ? `${acc}/${part}` : part;
      ancestors.push(acc);
    }
    setExpanded((prev) => {
      const next = new Set(prev);
      for (const path of ancestors) next.add(path);
      return next;
    });
    for (const path of ancestors) {
      const cached = dirs.get(path);
      if (!cached || cached.error) fetchDir(path);
    }
  };

  const indent = (depth: number) => ({ paddingLeft: `calc(var(--sp-8) + ${depth * 14}px)` });

  const renderDir = (rel: string, depth: number): ReactNode => {
    const dir = dirs.get(rel);
    if (!dir || dir.loading) {
      return <div className="chat-diff-tree-note" style={indent(depth)}>{t.chat.diffLoading}</div>;
    }
    if (dir.error) {
      return <div className="chat-diff-tree-note is-error" style={indent(depth)}>{dir.error}</div>;
    }
    return (dir.data ?? []).map((entry) =>
      entry.isDir ? (
        <div key={entry.relPath}>
          <button
            type="button"
            className="chat-diff-tree-dir"
            style={indent(depth)}
            onClick={() => toggleDir(entry.relPath)}
            onContextMenu={(event) => openCtxMenu(event, entry.relPath, true)}
          >
            <svg
              className={expanded.has(entry.relPath) ? "chat-diff-chev" : "chat-diff-chev is-collapsed"}
              width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"
            >
              <path d="M6 9l6 6 6-6" />
            </svg>
            <FolderIcon />
            <span className="chat-diff-path">{entry.name}</span>
          </button>
          {expanded.has(entry.relPath) && renderDir(entry.relPath, depth + 1)}
        </div>
      ) : (
        <button
          key={entry.relPath}
          type="button"
          className="chat-diff-tree-file"
          style={indent(depth)}
          onClick={() => openFile(entry.relPath)}
          onContextMenu={(event) => openCtxMenu(event, entry.relPath, false)}
        >
          <FileIcon />
          <span className="chat-diff-path">{entry.name}</span>
        </button>
      ),
    );
  };

  // 搜索结果（VS Code 搜索面板式按文件分组）：
  //  - 名称命中的目录/文件在前：目录点开即展开树、无内容命中的文件点开即查看；
  //  - 带内容命中的组：头行点击折叠/展开（角标 = 该文件命中总行数，可大于展示的行数），
  //    行命中点开后跳到该行并高亮命中词。
  const searchView = (() => {
    if (searchState.error) return <div className="chat-diff-empty is-error">{searchState.error}</div>;
    const groups = searchState.data?.files ?? [];
    if (groups.length === 0) {
      return (
        <div className="chat-diff-empty">
          {searchState.loading ? t.chat.diffLoading : t.chat.diffSearchNoResults}
        </div>
      );
    }
    return (
      <>
        <div className="chat-diff-tree">
          {groups.map((group) => {
            const hasLines = group.lines.length > 0;
            const collapsed = collapsedHits.has(group.relPath);
            return (
              <div key={group.relPath}>
                <button
                  type="button"
                  className="chat-diff-tree-file"
                  onClick={() =>
                    group.isDir
                      ? revealDirFromSearch(group.relPath)
                      : hasLines
                        ? toggleHitGroup(group.relPath)
                        : openFile(group.relPath)
                  }
                  onContextMenu={(event) => openCtxMenu(event, group.relPath, group.isDir)}
                >
                  {hasLines && (
                    <svg
                      className={collapsed ? "chat-diff-chev is-collapsed" : "chat-diff-chev"}
                      width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"
                    >
                      <path d="M6 9l6 6 6-6" />
                    </svg>
                  )}
                  {group.isDir ? <FolderIcon /> : <FileIcon />}
                  <span className="chat-diff-path" data-tip={group.relPath}>{group.relPath}</span>
                  {group.totalLineMatches > 0 && (
                    <span className="chat-diff-hit-count">{group.totalLineMatches}</span>
                  )}
                </button>
                {hasLines && !collapsed &&
                  group.lines.map((lineHit) => (
                    <button
                      key={lineHit.line}
                      type="button"
                      className="chat-diff-hit-row"
                      onClick={() => openFile(group.relPath, lineHit.line)}
                      onContextMenu={(event) => openCtxMenu(event, group.relPath, false)}
                    >
                      <span className="chat-diff-hit-ln">{lineHit.line}</span>
                      <span className="chat-diff-hit-preview">{markMatch(lineHit.preview, query)}</span>
                    </button>
                  ))}
              </div>
            );
          })}
        </div>
        {searchState.data?.truncated && <div className="chat-diff-truncated">{t.chat.diffSearchTruncated}</div>}
      </>
    );
  })();

  if (selectedPath) {
    const kind = previewKindOf(selectedPath);
    // md 与 svg 提供「预览/源码」切换（svg 本质是文本）；位图没有可读源码，不出切换。
    const showModeToggle = kind === "markdown" || extKeyOf(selectedPath) === "svg";
    const setMode = (source: boolean) => {
      setSourceMode(source);
      if (source) fetchText(selectedPath);
      else if (kind === "image") fetchImage(selectedPath);
    };
    const content = (() => {
      // 图片预览通道：独立缓存与占位（过大/失败给文字，不落到「二进制」歧义提示）。
      if (kind === "image" && !sourceMode) {
        const entry = images.get(selectedPath);
        if (!entry || entry.loading) return <div className="chat-diff-empty">{t.chat.diffLoading}</div>;
        if (entry.error) return <div className="chat-diff-empty is-error">{entry.error}</div>;
        const image = entry.data;
        if (!image?.dataUrl) return <div className="chat-diff-empty">{t.chat.diffImageTooLarge}</div>;
        return (
          <div className="chat-diff-scroll chat-diff-imgwrap">
            <img className="chat-diff-img" src={image.dataUrl} alt={selectedPath} />
          </div>
        );
      }
      const entry = files.get(selectedPath);
      if (!entry || entry.loading) return <div className="chat-diff-empty">{t.chat.diffLoading}</div>;
      if (entry.error) return <div className="chat-diff-empty is-error">{entry.error}</div>;
      const file = entry.data;
      if (!file || file.binary) return <div className="chat-diff-empty">{t.chat.diffBinary}</div>;
      // markdown 预览：FileMarkdown（渲染内嵌 HTML + sanitize 白名单 + 仓库内图片
      // 转 data URL），排版样式复用对话区 .chat-md 全套。
      if (kind === "markdown" && !sourceMode) {
        return (
          <>
            <div className="chat-diff-scroll">
              <div className="chat-md chat-diff-mdview">
                <FileMarkdown text={file.text} resolveImage={resolveMdImage} />
              </div>
            </div>
            {file.truncated && <div className="chat-diff-truncated">{t.chat.diffTruncated}</div>}
          </>
        );
      }
      return (
        <>
          <div className="chat-diff-scroll">
            {/* 行号槽宽按最大行号位数走（--ln-w），右对齐 + 右描边，不可选中。
                min-width 钉住横向宽度（+8ch 给行号槽/间距），理由见 estimateMaxColumns。 */}
            <pre ref={preRef} className="chat-diff-text is-code" style={{ "--ln-w": `${String((highlightedFile ?? []).length).length}ch`, minWidth: `max(100%, ${fileMaxCols + 8}ch)` } as CSSProperties}>
              {(highlightedFile ?? []).map((html, index) => (
                <div key={index} className={target && selectedPath === target.rel && index + 1 === target.line ? "is-target" : undefined}>
                  <span className="chat-diff-ln">{index + 1}</span>
                  <span dangerouslySetInnerHTML={{ __html: html || " " }} />
                </div>
              ))}
            </pre>
          </div>
          {file.truncated && <div className="chat-diff-truncated">{t.chat.diffTruncated}</div>}
        </>
      );
    })();
    return (
      <>
        <div className="chat-diff-filehead">
          <BackButton label={t.chat.diffBack} onClick={() => setSelectedPath(null)} />
          <span className="chat-diff-path" data-tip={selectedPath}>{selectedPath}</span>
          {showModeToggle && (
            <div className="chat-diff-tabs">
              <button type="button" className={sourceMode ? "" : "is-active"} onClick={() => setMode(false)}>
                {t.chat.diffMdPreview}
              </button>
              <button type="button" className={sourceMode ? "is-active" : ""} onClick={() => setMode(true)}>
                {t.chat.diffMdSource}
              </button>
            </div>
          )}
          <OpenSplitButton cwd={cwd} rel={selectedPath} />
        </div>
        {content}
      </>
    );
  }
  return (
    <>
      <input
        type="text"
        className="chat-diff-search"
        placeholder={t.chat.diffSearchFilesPlaceholder}
        value={search}
        // IME 合成守卫：与侧栏搜索框同款，拼音中间态不下发。
        onChange={(event) => {
          if ((event.nativeEvent as InputEvent).isComposing) return;
          setSearch(event.target.value);
        }}
        onCompositionEnd={(event) => setSearch(event.currentTarget.value)}
      />
      {searchActive ? searchView : <div className="chat-diff-tree">{renderDir("", 0)}</div>}
      {ctxMenu && (
        <PathContextMenu
          cwd={cwd}
          rel={ctxMenu.rel}
          isDir={ctxMenu.isDir}
          x={ctxMenu.x}
          y={ctxMenu.y}
          onClose={() => setCtxMenu(null)}
        />
      )}
    </>
  );
}

function BackButton({ label, onClick }: { label: string; onClick: () => void }) {
  return (
    <button type="button" className="chat-diff-back" onClick={onClick}>
      <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M15 18l-6-6 6-6" /></svg>
      {label}
    </button>
  );
}

function FolderIcon() {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />
    </svg>
  );
}

function FileIcon() {
  return (
    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" /><path d="M14 2v6h6" />
    </svg>
  );
}

/* ——「打开」按钮与路径菜单的小图标（与全库同款 lucide 线稿风）—— */

function OpenIcon() {
  return (
    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" />
      <polyline points="15 3 21 3 21 9" />
      <line x1="10" y1="14" x2="21" y2="3" />
    </svg>
  );
}

function CopyIcon() {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <rect x="9" y="9" width="13" height="13" rx="2" />
      <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
    </svg>
  );
}

function RevealIcon() {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="m6 14 1.45-2.9A2 2 0 0 1 9.24 10H20a2 2 0 0 1 1.94 2.5l-1.55 6a2 2 0 0 1-1.94 1.5H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3.9a2 2 0 0 1 1.69.9l.81 1.2a2 2 0 0 0 1.67.9H18a2 2 0 0 1 2 2v2" />
    </svg>
  );
}

function EditorIcon() {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <polyline points="16 18 22 12 16 6" />
      <polyline points="8 6 2 12 8 18" />
    </svg>
  );
}

function AppWindowIcon() {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <rect x="2" y="4" width="20" height="16" rx="2" />
      <path d="M10 4v4" />
      <path d="M2 8h20" />
      <path d="M6 4v4" />
    </svg>
  );
}

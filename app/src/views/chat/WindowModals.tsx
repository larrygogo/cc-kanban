/**
 * 对话窗的三个弹层（从 ChatWindow.tsx 原样搬出，行为不变）：
 * - RenameModal：重命名会话；
 * - QuickSwitcher：Ctrl/Cmd+K 命令面板（搜会话 + 搜并执行窗口命令）；
 * - ShortcutSheet：? 唤出的快捷键速查表。
 * 三者共用 AppModal 壳（G-3）：Esc 关、Tab 焦点循环、初始焦点、关闭归还焦点、背景 inert。
 */
import { useEffect, useMemo, useRef, useState } from "react";
import { getLiveSessionsPage, sessionTone, type LiveSession } from "../../api";
import { useT } from "../../i18n";
import { formatBackendError } from "../../i18n/errors";
import { parentSegment } from "../../paths";
import { AppModal } from "../AppModal";

/** 重命名会话的独立 modal（用户指定，不做就地编辑）。Enter 保存、Esc/取消/点遮罩关闭；
 *  失败原因就地显示在弹层里，不静默吞。 */
export function RenameModal({ initial, onSubmit, onClose, t }: {
  initial: string;
  onSubmit: (title: string) => Promise<void>;
  onClose: () => void;
  t: ReturnType<typeof useT>;
}) {
  const [value, setValue] = useState(initial);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const submit = async () => {
    const title = value.trim();
    if (!title || saving) return;
    if (title === initial) { onClose(); return; }
    setSaving(true);
    setError(null);
    try {
      await onSubmit(title);
      onClose();
    } catch (e) {
      setError(formatBackendError(e, t.locale));
      setSaving(false);
    }
  };
  return (
    <AppModal label={t.sticker.renameTitle} onClose={onClose}>
      <div className="chat-modal-title">{t.sticker.renameTitle}</div>
      <input
        className="ns-input"
        autoFocus
        value={value}
        placeholder={t.sticker.renamePlaceholder}
        onChange={(event) => setValue(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter" && !event.nativeEvent.isComposing) void submit();
        }}
      />
      {error && <div className="chat-modal-error" role="alert">{error}</div>}
      <div className="chat-modal-actions">
        <button type="button" className="ns-btn" onClick={onClose}>{t.newSession.cancel}</button>
        <button type="button" className="ns-btn is-primary" disabled={!value.trim() || saving} onClick={() => void submit()}>{t.chat.renameSave}</button>
      </div>
    </AppModal>
  );
}

/** 命令面板的命令条目：扁平一个数组由 ChatWindow 传入（专项明确不加插件化注册、
 *  不加最近使用排序）。keys 是右侧快捷键提示；没有全局快捷键的命令留空不渲染。 */
export interface PaletteCommand {
  id: string;
  label: string;
  keys?: string;
}

/** Ctrl/Cmd+K 命令面板：搜索 + 键盘导航直达任意会话，或搜索并执行窗口内命令
 *  （U1-14 专项：由「快速切换器」扩展而来，命令全部映射窗口已有动作）。会话查询
 *  下沉后端 search（与看板/侧栏同一条 LIKE）；命令就个位数条，客户端子串过滤。 */
export function QuickSwitcher({ activeId, commands, onPick, onCommand, onClose }: {
  activeId: number;
  commands: PaletteCommand[];
  onPick: (id: number) => void;
  onCommand: (id: string) => void;
  onClose: () => void;
}) {
  const t = useT();
  const [query, setQuery] = useState("");
  const [rows, setRows] = useState<LiveSession[]>([]);
  const [active, setActive] = useState(0);
  const seqRef = useRef(0);
  const listRef = useRef<HTMLDivElement>(null);
  // 首屏立即取（空查询 = 最近活跃），输入防抖 200ms；旧响应按序号丢弃。
  useEffect(() => {
    const seq = ++seqRef.current;
    const timer = window.setTimeout(() => {
      getLiveSessionsPage("all", query.trim() || null, null, 30)
        .then((page) => {
          if (seqRef.current !== seq) return;
          setRows(page.items);
          setActive(0);
          if (listRef.current) listRef.current.scrollTop = 0;
        })
        .catch(() => {});
    }, query ? 200 : 0);
    return () => window.clearTimeout(timer);
  }, [query]);
  // 空查询时命令组全列而非隐藏：命令只有个位数条，全列让「这不只是会话切换器」
  // 一眼可见（信息 scent），藏起来用户永远不知道还能搜命令。
  const filteredCommands = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return commands;
    return commands.filter((cmd) => cmd.label.toLowerCase().includes(q));
  }, [commands, query]);
  const pick = (id: number) => {
    onClose();
    if (id !== activeId) onPick(id);
  };
  // 执行命令与会话跳转同一条收尾路径：先关弹层（AppModal 卸载时把焦点归还给
  // 打开前的元素，通常是 composer），再执行——命令触发的视图切换不抢这拍焦点。
  const run = (id: string) => {
    onClose();
    onCommand(id);
  };
  // 键盘导航的平面索引跨两组：会话组在前（Ctrl+K 的老本行），命令组在后。
  // 组头小标签只是渲染分隔，不进索引、不参与 ↑↓。
  const total = rows.length + filteredCommands.length;
  // 查询变化收窄结果时旧 active 可能越界，渲染与按键统一用收敛后的值。
  const eff = total > 0 ? Math.min(active, total - 1) : 0;
  const scrollTo = (index: number) => {
    // 只在键盘导航时跟滚：mouseEnter 的 setActive 不滚——悬停在半可见项上
    // 会「滚动↔悬停」互相触发抖起来。jsdom 无 scrollIntoView，可选调用兜底。
    (listRef.current?.querySelector(`[data-idx="${index}"]`) as HTMLElement | null)?.scrollIntoView?.({ block: "nearest" });
  };
  // 同名项目消歧（与侧栏目录下拉同款，共用 parentSegment）：多个「codebase」并排时
  // 带上上一级目录。
  const metaOf = useMemo(() => {
    const counts = new Map<string, number>();
    for (const row of rows) counts.set(row.project_name, (counts.get(row.project_name) ?? 0) + 1);
    return (row: LiveSession) => {
      const name = row.project_name;
      if ((counts.get(name) ?? 0) <= 1 || !row.cwd) return name;
      const parent = parentSegment(row.cwd);
      return parent ? `${parent}/${name}` : name;
    };
  }, [rows]);
  return (
    <AppModal
      label={t.chat.switcherTitle}
      className="chat-switcher"
      onClose={onClose}
      onKeyDown={(event) => {
        if (event.key === "ArrowDown" || event.key === "ArrowUp") {
          event.preventDefault();
          if (total === 0) return;
          const next = (eff + (event.key === "ArrowDown" ? 1 : total - 1)) % total;
          setActive(next);
          scrollTo(next);
          return;
        }
        if (event.key === "Home" || event.key === "End") {
          event.preventDefault();
          if (total === 0) return;
          const next = event.key === "Home" ? 0 : total - 1;
          setActive(next);
          scrollTo(next);
          return;
        }
        if (event.key === "Enter" && !event.nativeEvent.isComposing) {
          event.preventDefault();
          if (eff < rows.length) {
            const row = rows[eff];
            if (row) pick(row.session.id);
          } else {
            const cmd = filteredCommands[eff - rows.length];
            if (cmd) run(cmd.id);
          }
        }
      }}
    >
      <input
        className="ns-input"
        autoFocus
        value={query}
        placeholder={t.chat.switcherPlaceholder}
        aria-label={t.chat.switcherTitle}
        onChange={(event) => {
          if ((event.nativeEvent as InputEvent).isComposing) return;
          setQuery(event.target.value);
        }}
        onCompositionEnd={(event) => setQuery(event.currentTarget.value)}
      />
      <div className="chat-switcher-list" ref={listRef} role="listbox" aria-label={t.chat.switcherTitle}>
        {total === 0 && <div className="chat-switcher-empty">{t.chat.switcherEmpty}</div>}
        {rows.length > 0 && (
          <>
            <div className="chat-switcher-group" role="presentation">{t.chat.switcherGroupSessions}</div>
            {rows.map((row, index) => {
              const tone = sessionTone(row.connected, row.session.status, row.pending_review, row.errored, row.busy_subagents, row.screen_state);
              return (
                <button
                  type="button"
                  key={row.session.id}
                  role="option"
                  aria-selected={index === eff}
                  data-idx={index}
                  className={"chat-switcher-item" + (index === eff ? " is-active" : "") + (row.session.id === activeId ? " is-current" : "")}
                  onMouseEnter={() => setActive(index)}
                  onClick={() => pick(row.session.id)}
                >
                  <i className={`chat-sidebar-dot is-${tone}`} aria-hidden="true" />
                  <span className="chat-switcher-name">{row.task_title || t.sticker.waitingFirstInput}</span>
                  <span className="chat-switcher-meta">{metaOf(row)}</span>
                </button>
              );
            })}
          </>
        )}
        {filteredCommands.length > 0 && (
          <>
            <div className="chat-switcher-group" role="presentation">{t.chat.switcherGroupCommands}</div>
            {filteredCommands.map((cmd, i) => {
              const index = rows.length + i;
              return (
                <button
                  type="button"
                  key={cmd.id}
                  role="option"
                  aria-selected={index === eff}
                  data-idx={index}
                  className={"chat-switcher-item" + (index === eff ? " is-active" : "")}
                  onMouseEnter={() => setActive(index)}
                  onClick={() => run(cmd.id)}
                >
                  <span className="chat-switcher-name">{cmd.label}</span>
                  {cmd.keys && <kbd className="chat-switcher-keys">{cmd.keys}</kbd>}
                </button>
              );
            })}
          </>
        )}
      </div>
    </AppModal>
  );
}

/** `?` 唤出的快捷键速查表：快捷键体系逐步长出来了（Ctrl+K/B/1/2/N/F…），
 *  没有一张总表就等于只有装了肌肉记忆的人用得上。纯静态展示，Esc/点外关闭。 */
export function ShortcutSheet({ onClose }: { onClose: () => void }) {
  const t = useT();
  const rows: Array<[string, string]> = [
    ["Ctrl+K", t.chat.shortcutSwitcher],
    ["Ctrl+B", t.chat.shortcutSidebar],
    ["Ctrl+1 / Ctrl+2", t.chat.shortcutViews],
    ["Ctrl+N", t.chat.shortcutNewSession],
    ["Ctrl+F", t.chat.shortcutSearch],
    ["Enter", t.chat.shortcutSend],
    ["Shift+Enter", t.chat.shortcutNewline],
    ["Ctrl+Enter", t.chat.shortcutInterruptSend],
    ["Tab / ↑↓", t.chat.shortcutSlashComplete],
    ["↑", t.chat.shortcutHistory],
    ["Esc", t.chat.shortcutDeny],
    ["?", t.chat.shortcutSheet],
  ];
  return (
    <AppModal label={t.chat.shortcutsTitle} className="chat-shortcuts" onClose={onClose}>
      <div className="chat-modal-title">{t.chat.shortcutsTitle}</div>
      <dl className="chat-shortcuts-list">
        {rows.map(([keys, desc]) => (
          <div className="chat-shortcuts-row" key={keys}>
            <dt>{keys.split(" / ").map((combo, i) => (
              <span key={combo}>{i > 0 && " / "}<kbd>{combo}</kbd></span>
            ))}</dt>
            <dd>{desc}</dd>
          </div>
        ))}
      </dl>
      {/* 终端视图的让位说明:Ctrl+K/B/N/1/2 被应用导航占用、readline 编辑键失效,
          Ctrl+F 也不抢 xterm 焦点——这些"静默失效"必须有一处写明,否则无从排查。 */}
      <div className="chat-shortcuts-note">{t.chat.shortcutTerminalNote}</div>
    </AppModal>
  );
}

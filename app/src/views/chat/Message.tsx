import { memo, useEffect, useRef, useState, type MouseEvent as ReactMouseEvent } from "react";
import { createPortal } from "react-dom";
import { convertFileSrc } from "@tauri-apps/api/core";
import { useT } from "../../i18n";
import { type ChatItem } from "../../api";
import { ChatMarkdown } from "../ChatMarkdown";
import { parseUserText } from "./localCommand";

// Claude Code 把粘贴/引用的图片在 transcript 里记成一行「[Image: source: <本地路径>]」。
// 原样渲染就是在气泡里摊一整行 C:\Users\... 路径——对话里最脏的元素。渲染成缩略图
// （asset 协议已启用，scope 限定在 image-cache 与 meowo-paste 两个图片目录，见
// tauri.conf.json）；scope 外/文件已删时 onError 回退成文件名徽章，路径永不上屏。
const IMAGE_REF = /\[Image(?: #\d+)?: source: ([^\]]+?)\]/g;

/** 把用户文本拆成「正文（去掉图片引用行）+ 图片路径列表」。图片不混排在文字里：
 *  气泡是「说的话」，图片是附件，各归各的（正文里的 [Image #N] 指代照旧保留）。 */
function splitUserText(text: string): { body: string; images: { path: string; key: string }[] } {
  const matches = [...text.matchAll(IMAGE_REF)];
  if (matches.length === 0) return { body: text, images: [] };
  const images = matches.map((match, index) => ({ path: match[1].trim(), key: `${match.index}:${index}` }));
  return { body: text.replace(IMAGE_REF, "").trim(), images };
}

/** 大图查看层：滚轮缩放、拖拽平移、双击复位；工具栏有缩放按钮与明显的关闭键。
 *  点击空白处或 Esc 也能关（拖拽松手不算点击）。 */
function Lightbox({ src, name, onClose }: { src: string; name: string; onClose: () => void }) {
  const t = useT();
  const [scale, setScale] = useState(1);
  const [offset, setOffset] = useState({ x: 0, y: 0 });
  const stageRef = useRef<HTMLDivElement>(null);
  const dragRef = useRef<{ startX: number; startY: number; baseX: number; baseY: number; moved: boolean } | null>(null);
  const justDraggedRef = useRef(false);
  const clampScale = (value: number) => Math.min(8, Math.max(0.2, value));
  const reset = () => { setScale(1); setOffset({ x: 0, y: 0 }); };

  // Esc 关灯箱。挂捕获段并拦住传播：ChatWindow 有一个全局 Esc=拒绝审批的监听，
  // 用户按 Esc 只想关图，不能顺手把审批拒了。
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.stopPropagation();
      onClose();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose]);

  // 滚轮缩放。React 的 onWheel 在根容器上是 passive 监听，preventDefault 无效
  // （页面会跟着滚）——自己挂非 passive 的原生监听。
  useEffect(() => {
    const stage = stageRef.current;
    if (!stage) return;
    const onWheel = (event: WheelEvent) => {
      event.preventDefault();
      setScale((current) => clampScale(current * (event.deltaY < 0 ? 1.15 : 1 / 1.15)));
    };
    stage.addEventListener("wheel", onWheel, { passive: false });
    return () => stage.removeEventListener("wheel", onWheel);
  }, []);

  const onMouseDown = (event: ReactMouseEvent) => {
    event.preventDefault();
    dragRef.current = { startX: event.clientX, startY: event.clientY, baseX: offset.x, baseY: offset.y, moved: false };
  };
  const onMouseMove = (event: ReactMouseEvent) => {
    const drag = dragRef.current;
    if (!drag) return;
    const dx = event.clientX - drag.startX;
    const dy = event.clientY - drag.startY;
    if (Math.abs(dx) + Math.abs(dy) > 3) drag.moved = true;
    setOffset({ x: drag.baseX + dx, y: drag.baseY + dy });
  };
  const onMouseUp = () => {
    if (!dragRef.current) return;
    justDraggedRef.current = dragRef.current.moved;
    dragRef.current = null;
  };
  const onStageClick = (event: ReactMouseEvent) => {
    // 拖拽平移松手不是「点空白关闭」；只有真正点在空白处才关。
    if (justDraggedRef.current) { justDraggedRef.current = false; return; }
    if (event.target === event.currentTarget) onClose();
  };

  return createPortal(
    <div className="chat-lightbox" role="dialog" aria-label={name}>
      <div
        ref={stageRef}
        className="chat-lightbox-stage"
        onClick={onStageClick}
        onMouseDown={onMouseDown}
        onMouseMove={onMouseMove}
        onMouseUp={onMouseUp}
        onMouseLeave={onMouseUp}
        onDoubleClick={reset}
      >
        <img
          src={src}
          alt={name}
          draggable={false}
          style={{ transform: `translate(${offset.x}px, ${offset.y}px) scale(${scale})` }}
        />
      </div>
      <div className="chat-lightbox-bar">
        <button type="button" aria-label={t.chat.zoomOut} data-tip={t.chat.zoomOut} onClick={() => setScale((s) => clampScale(s / 1.25))}>−</button>
        <button type="button" className="chat-lightbox-zoom" aria-label={t.chat.zoomReset} data-tip={t.chat.zoomReset} onClick={reset}>{Math.round(scale * 100)}%</button>
        <button type="button" aria-label={t.chat.zoomIn} data-tip={t.chat.zoomIn} onClick={() => setScale((s) => clampScale(s * 1.25))}>+</button>
        <button type="button" aria-label={t.chat.lightboxClose} data-tip={t.chat.lightboxClose} onClick={onClose}>
          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><path d="M6 6l12 12M18 6L6 18" /></svg>
        </button>
      </div>
    </div>,
    document.body,
  );
}

function ImageRef({ path }: { path: string }) {
  const [failed, setFailed] = useState(false);
  const [expanded, setExpanded] = useState(false);
  const name = path.split(/[\\/]/).pop() || path;
  if (failed) {
    return (
      <span className="chat-image-chip" title={path}>
        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><rect x="3" y="5" width="18" height="14" rx="2" /><circle cx="9" cy="10" r="1.6" /><path d="m5 17 4.5-4.5L13 16l3-3 3 4" /></svg>
        <span>{name}</span>
      </span>
    );
  }
  const src = convertFileSrc(path);
  return (
    <>
      <button type="button" className="chat-image-thumb-btn" title={path} onClick={() => setExpanded(true)}>
        <img className="chat-image-thumb" src={src} alt={name} loading="lazy" onError={() => setFailed(true)} />
      </button>
      {/* 灯箱走 portal（在 Lightbox 内）：消息块开着 content-visibility（paint 包含），
          fixed 覆盖层留在消息里会被裁剪在消息框内。 */}
      {expanded && <Lightbox src={src} name={name} onClose={() => setExpanded(false)} />}
    </>
  );
}

function ImageRow({ images }: { images: { path: string; key: string }[] }) {
  return (
    <div className="chat-image-row">
      {images.map((image) => <ImageRef path={image.path} key={image.key} />)}
    </div>
  );
}

/** 该条用户消息是否**只有**图片（无正文）。是则返回图片路径，供 Transcript 把连续的
 *  纯图片消息合并成一行——Claude Code 把一次多图粘贴记成连续多条独立消息，逐条渲染
 *  就是竖着摞一列，而用户视角那是「一次发的几张图」。 */
export function imageOnlyPaths(text: string): string[] | null {
  const { body, images } = splitUserText(text);
  return !body && images.length > 0 ? images.map((image) => image.path) : null;
}

/** 连续纯图片消息合并后的展示块（Transcript 专用）。 */
export function UserImageGroup({ paths }: { paths: string[] }) {
  return (
    <article className="chat-message is-user has-images">
      <ImageRow images={paths.map((path, index) => ({ path, key: `${index}:${path}` }))} />
    </article>
  );
}

/// 超过这个行数的思考过程收成预览态。与 styles.css 里 `.chat-reasoning.is-long` 的
/// max-height 是同一个意思，改一个要顺带看另一个。
const REASONING_PREVIEW_LINES = 6;

/// memo：流式期间 items 引用每轮都变（Transcript 整棵重建），但未变化的条目经
/// reducer 的写时复制保持同一引用——memo 让重渲染只落在真正变化的那条上，
/// parseUserText / split 这类逐条解析不再全量重跑。
export const Message = memo(function Message({ item }: { item: ChatItem }) {
  const t = useT();
  if (item.type === "user_text") {
    // 斜杠命令在 transcript 里是一条 XML 包裹的用户消息。渲染成命令徽章 + 可展开输出，
    // 而不是把 <command-name> 这类标签摊在对话里让人自己读。
    const parts = parseUserText(item.text);
    const { body, images } = splitUserText(parts.text);
    if (parts.local) {
      // 只剩免责声明（写给模型的 caveat）的那条：对人零信息量，整条不渲染。
      if (!parts.commands.length && !parts.stdout.length && !parts.text) return null;
      return (
        <article className="chat-message is-user is-command">
          {parts.commands.map((command, index) => (
            <span className="chat-command" key={`${command.name}-${index}`}>
              <span className="chat-command-icon" aria-hidden="true">
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round"><path d="M4 17l6-6-6-6M12 19h8" /></svg>
              </span>
              <span className="chat-command-name">{command.name || t.chat.localCommand}</span>
              {command.args && <span className="chat-command-args">{command.args}</span>}
            </span>
          ))}
          {images.length > 0 && <ImageRow images={images} />}
          {body && <div className="chat-text">{body}</div>}
          {/* 命令输出默认收起：/clear 之类没什么可看，/cost、/status 却能刷一屏。 */}
          {parts.stdout.map((out, index) => (
            <details className="chat-tool chat-command-out" key={index}>
              <summary>
                <span className="chat-tool-name">{t.chat.commandOutput}</span>
                <span className="chat-tool-summary">{out}</span>
                <span className="chat-tool-chevron">›</span>
              </summary>
              <pre>{out}</pre>
            </details>
          ))}
        </article>
      );
    }
    // 用户消息保持原文（用户不是在写 markdown，行首 # 变大标题只会失真）。
    // 有图时缩略图独立成行排在**气泡上方**、气泡只包正文（Claude Code 同款布局，
    // has-images 把气泡皮从 article 挪到 .chat-text 上）；纯图消息没有气泡，只有图。
    return (
      <article className={"chat-message is-user" + (images.length ? " has-images" : "")}>
        {images.length > 0 && <ImageRow images={images} />}
        {body && <div className="chat-text">{body}</div>}
      </article>
    );
  }
  if (item.type === "assistant_text" || item.type === "assistant_delta") {
    return (
      <article className="chat-message is-assistant">
        <div className="chat-text chat-md"><ChatMarkdown text={item.text} /></div>
      </article>
    );
  }
  if (item.type === "reasoning" || item.type === "reasoning_delta") {
    // 长推理默认收成**预览**（显示开头几行并渐隐），而不是整段藏起来——既能一眼看到
    // agent 在想什么，又不会让上百行把结论和后续对话挤出屏幕。短的直接摊开，
    // 没必要为几行内容加一次点击。
    const lines = item.text.split("\n").filter((line) => line.trim()).length;
    const long = lines > REASONING_PREVIEW_LINES;
    return (
      <details className={"chat-reasoning" + (long ? " is-long" : "")} open={!long}>
        <summary>
          <span className="chat-timeline-dot" />
          {t.chat.reasoning}
          {long && <span className="chat-reasoning-size">{t.chat.reasoningLines(lines)}</span>}
        </summary>
        <div className="chat-md"><ChatMarkdown text={item.text} /></div>
      </details>
    );
  }
  if (item.type === "tool_use") {
    return (
      <details className="chat-tool">
        <summary>
          <span className="chat-tool-icon"><svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8"><path d="M4 17l6-6-6-6M12 19h8" /></svg></span>
          <span className="chat-tool-name">{item.name}</span><span className="chat-tool-summary">{item.summary}</span><span className="chat-tool-chevron">›</span>
        </summary>
        <pre>{item.summary}</pre>
      </details>
    );
  }
  if (item.type === "tool_result") {
    return (
      <details className={"chat-tool chat-result" + (item.is_error ? " is-error" : "")}>
        <summary>
          <span className="chat-tool-icon is-file"><svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8"><path d="M6 3h8l4 4v14H6zM14 3v5h5" /></svg></span>
          <span className="chat-tool-name">{t.chat.toolResult}</span><span className="chat-tool-summary">{item.text}</span><span className="chat-tool-chevron">›</span>
        </summary>
        <pre>{item.text}</pre>
      </details>
    );
  }
  return <div className="chat-meta"><span />{t.chat.compact}<span /></div>;
});

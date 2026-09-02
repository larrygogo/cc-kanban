import { memo, useEffect, useRef, useState, type MouseEvent as ReactMouseEvent } from "react";
import { createPortal } from "react-dom";
import { pushEscLayer } from "../../escLayers";
import { convertFileSrc } from "@tauri-apps/api/core";
import { useT } from "../../i18n";
import { zh } from "../../i18n/zh";
import { en } from "../../i18n/en";
import { type ChatItem } from "../../api";
import { ChatMarkdown } from "../ChatMarkdown";
import { parseUserText, type CrossMessage } from "./localCommand";

// Claude Code 把粘贴/引用的图片在 transcript 里记成一行「[Image: source: <本地路径>]」。
// 原样渲染就是在气泡里摊一整行 C:\Users\... 路径——对话里最脏的元素。渲染成缩略图
// （asset 协议已启用，scope 限定在 image-cache 与 meowo-paste 两个图片目录，见
// tauri.conf.json）；scope 外/文件已删时 onError 回退成文件名徽章。
// 路径**任何形式都不上屏**——包括 title：hover 提示与读屏都会念出它，而本地路径常含
// 用户名等身份信息。要看是哪张图，缩略图本身与文件名就够了。
const IMAGE_REF = /\[Image(?: #\d+)?: source: ([^\]]+?)\]/g;

// kimi 文本回退时不吃指令里的路径行,附件行「- <图片绝对路径>」原文落进 transcript
// (claude 会转成上面的 [Image: source: …] 引用行)。有指令头时把这类行也抽成 chip。
// 只认 Windows 绝对路径(盘符 + 反斜杠,与实测形态一致);路径可含空格,故不按空白切、
// 整行捕获到扩展名收尾。扩展名口径与 ChatWindow 的 IMAGE_EXTENSIONS 保持一致。
const IMAGE_PATH_LINE = /^- ([A-Za-z]:\\.+\.(?:png|jpe?g|gif|webp|bmp|svg))\s*$/i;

/** 附件指令头（composer 注入给 CLI 的那句，见 i18n 的 attachmentInstruction）。
 *  图片已渲染成缩略图时，这句对人是纯噪音——按下方规则剥掉。直接从两份语言资产派生
 *  （attachmentInstruction("") 去掉附件列表后 trim），不手抄原句：文案改动自动跟上，
 *  历史消息无论当时用哪种语言写下都认（评审：手抄字面量会在改文案时静默失效）。 */
const ATTACHMENT_INSTRUCTION_HEADERS = new Set(
  [zh, en].map((dict) => dict.chat.attachmentInstruction("").trim()),
);

/** 把用户文本拆成「正文（去掉图片引用行）+ 图片路径列表」。图片不混排在文字里：
 *  气泡是「说的话」，图片是附件，各归各的（正文里的 [Image #N] 指代照旧保留）。
 *  图片来源有两种形态：claude 式 [Image: source: …] 引用行，以及 kimi 文本回退时
 *  的「- <图片绝对路径>」附件行（仅在有附件指令头时抽取，见下方 IMAGE_PATH_LINE）。
 *  导出给 ChatWindow 的 ↑ 召回复用：召回只回填正文，口径必须与气泡渲染一致。 */
export function splitUserText(text: string): { body: string; images: { path: string; key: string }[] } {
  const matches = [...text.matchAll(IMAGE_REF)];
  const images = matches.map((match, index) => ({ path: match[1].trim(), key: `${match.index}:${index}` }));
  // 无图片引用也不能早退：CLI 会把附件行「- <路径>」里的图片路径吃掉、转成独立
  // image 块，文本只剩指令头 + 光杆「-」——这形状同样要剥（实拍回归）。
  // 逐行抽引用，只删「原行确实含图片引用、抽走后只剩空白或空弹头」的行——
  // 曾按全文过滤 trim()==="-" 的行，把用户自己写的独立「-」行也误删了（评审确认）。
  const kept: string[] = [];
  for (const line of text.split("\n")) {
    const stripped = line.replace(IMAGE_REF, "");
    const leftover = stripped.trim();
    if (stripped !== line && (leftover === "" || leftover === "-")) continue;
    kept.push(stripped);
  }
  // 指令头连同其后紧跟的光杆「-」行（附件残行）一起处理：残行总是剥；仍有实质
  // 附件行（非图片路径「- xxx」）时保留头，否则头也剥。用户自己写的孤立「-」
  // 不紧跟在指令头后，不受影响。
  // 消息是否经附件机制发出(有指令头):独行光杆只在此前提下、且仅剥「文本最顶端那段
  // 机器前缀」——CC 把光杆「[Image #N]」挪到提交文本最前,始终排在用户正文之前。一旦
  // 遇到第一行真实内容就停止,用户正文里手打的孤立「[Image #N]」照旧保留(此前全消息
  // 范围剥,把用户写的指代也吃了——desktop 回归)。
  const hasInstructionHeader = kept.some((line) =>
    ATTACHMENT_INSTRUCTION_HEADERS.has(line.replace(/\[Image #\d+\]\s*/g, "").trim()),
  );
  // kimi 式「指令头 + 图片路径行」:文本回退时 kimi 不吃路径行,「- C:\…\image.png」
  // 原样落进文本。有指令头时把这些行抽成 chip 并剥掉——剥在判头保留之前,这样
  // 「头 + 纯图片路径行」会走「无实质附件行 → 头也剥」的既有分支;混排的非图片
  // 「- xxx」附件行留下,头按既有语义保留。无指令头时不动:用户手打的「- 路径」
  // 列表是正文内容。
  if (hasInstructionHeader) {
    const withoutImagePaths: string[] = [];
    for (let i = 0; i < kept.length; i++) {
      const match = IMAGE_PATH_LINE.exec(kept[i].trim());
      if (match) {
        images.push({ path: match[1], key: `path:${i}` });
        continue;
      }
      withoutImagePaths.push(kept[i]);
    }
    kept.length = 0;
    kept.push(...withoutImagePaths);
  }
  let inLeadingRefs = hasInstructionHeader;
  const body: string[] = [];
  for (let i = 0; i < kept.length; i++) {
    const line = kept[i];
    // CC 把粘贴图片的光杆指代「[Image #N]」挪到提交文本最前:可能与指令头粘成一行
    // (「[Image #1]请读取并结合…」),也可能独占一行——判头前先剥掉它,否则精确
    // 比对失配,整段样板漏进气泡(远程发图实拍)。带 source 的完整引用行已在上一轮抽走。
    const bareStripped = line.replace(/\[Image #\d+\]\s*/g, "");
    if (!ATTACHMENT_INSTRUCTION_HEADERS.has(bareStripped.trim())) {
      // 顶端机器前缀里的独行光杆指代(图片已由 image 块渲染),剥。
      if (inLeadingRefs && bareStripped.trim() === "" && line.trim() !== "") continue;
      // 第一行真实内容(空行不计):机器前缀结束,此后不再剥独行指代。
      if (line.trim() !== "") inLeadingRefs = false;
      body.push(line);
      continue;
    }
    let j = i + 1;
    while (j < kept.length && kept[j].trim() === "-") j++;
    // 头要保留(还有非图片附件行)时也用剥过指代的版本:图片已是缩略图,光杆指代无信息。
    if (kept[j]?.trim().startsWith("- ")) body.push(bareStripped);
    i = j - 1;
  }
  return { body: body.join("\n").trim(), images };
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
  // 用户按 Esc 只想关图，不能顺手把审批拒了。同时注册 Esc 层（escLayers.ts）——
  // 层栈是全局监听的统一让位判据，捕获段截停是本层自己的双保险。
  useEffect(() => {
    const popLayer = pushEscLayer();
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.stopPropagation();
      onClose();
    };
    window.addEventListener("keydown", onKey, true);
    return () => {
      popLayer();
      window.removeEventListener("keydown", onKey, true);
    };
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
        {/* no-referrer:远程模式下 src 是带 token 的 /file URL,不许经 Referer 外泄。 */}
        <img
          src={src}
          alt={name}
          draggable={false}
          referrerPolicy="no-referrer"
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

/** 跨会话消息块：另一个 Claude 会话经 SendMessage 发来的一条消息。
 *  它是真的对话内容（不是 <task-notification> 那种机器记录），所以正文直接可读；
 *  但复核回执这类消息动辄几十行，收起态限高 220px，超出才给「展开全部」。
 *  超没超按实测判（scrollHeight vs clientHeight），不按字数猜——同样行数的中英文、
 *  带不带长 URL，渲染出来的高度差很多。 */
function CrossMessageBlock({ message }: { message: CrossMessage }) {
  const t = useT();
  const [expanded, setExpanded] = useState(false);
  const bodyRef = useRef<HTMLDivElement>(null);
  const [overflowing, setOverflowing] = useState(false);
  useEffect(() => {
    const el = bodyRef.current;
    if (!el) return;
    // +4 容差：亚像素行高会让恰好齐平的块虚报溢出，白挂一个点了没变化的按钮。
    setOverflowing(el.scrollHeight > el.clientHeight + 4);
  }, [message.text]);
  return (
    <div className="chat-crossmsg">
      <div className="chat-crossmsg-head">
        <span className="chat-crossmsg-icon" aria-hidden="true">
          <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round"><path d="M7 8h10l-3-3M17 16H7l3 3" /></svg>
        </span>
        <span className="chat-crossmsg-from">{message.fromName || t.chat.crossMessageUnknown}</span>
        {/* 权限模式只在**非默认**时显示：default 是常态，写出来只是噪声。 */}
        {message.mode && message.mode !== "default" && (
          <span className="chat-crossmsg-mode">{message.mode}</span>
        )}
        <span className="chat-crossmsg-label">{t.chat.crossMessage}</span>
      </div>
      <div ref={bodyRef} className={"chat-crossmsg-body" + (expanded ? " is-expanded" : "")}>
        <div className="chat-text">{message.text}</div>
      </div>
      {/* 收起态只在真的超高时给按钮；展开后保留「收起」，长消息才收得回去。 */}
      {(overflowing || expanded) && (
        <button type="button" className="chat-crossmsg-more" onClick={() => setExpanded((v) => !v)}>
          {expanded ? t.chat.approvalCollapse : t.chat.approvalExpand}
        </button>
      )}
    </div>
  );
}

/** 图片缩略图 + 点开灯箱。导出给 composer 的附件条复用：粘贴的图片与 transcript
 *  里的图片走同一套预览（按路径走 convertFileSrc，asset 读不到时回退文件名 chip）。 */
export function ImageRef({ path }: { path: string }) {
  const [failed, setFailed] = useState(false);
  const [expanded, setExpanded] = useState(false);
  const name = path.split(/[\\/]/).pop() || path;
  if (failed) {
    return (
      <span className="chat-image-chip" data-tip={name}>
        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><rect x="3" y="5" width="18" height="14" rx="2" /><circle cx="9" cy="10" r="1.6" /><path d="m5 17 4.5-4.5L13 16l3-3 3 4" /></svg>
        <span>{name}</span>
      </span>
    );
  }
  const src = convertFileSrc(path);
  return (
    <>
      <button type="button" className="chat-image-thumb-btn" data-tip={name} onClick={() => setExpanded(true)}>
        <img className="chat-image-thumb" src={src} alt={name} loading="lazy" referrerPolicy="no-referrer" onError={() => setFailed(true)} />
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

/** 思考过程块。开合只在**首次挂载**时按当时长度定一次：流式期间行数越过阈值不再把
 *  open 从 true 翻成 false——用户正读着的思考被 React 收起是实拍投诉。之后 open 的
 *  vdom 值保持不变，用户手动开合(原生 details 行为)不会被渲染覆盖。 */
function ReasoningBlock({ item }: { item: Extract<ChatItem, { type: "reasoning" | "reasoning_delta" }> }) {
  const t = useT();
  const lines = item.text.split("\n").filter((line) => line.trim()).length;
  const long = lines > REASONING_PREVIEW_LINES;
  const [initialOpen] = useState(!long);
  return (
    <details className={"chat-reasoning" + (long ? " is-long" : "")} open={initialOpen}>
      <summary>
        <span className="chat-timeline-dot" />
        {t.chat.reasoning}
        {long && <span className="chat-reasoning-size">{t.chat.reasoningLines(lines)}</span>}
      </summary>
      <div className="chat-md"><ChatMarkdown text={item.text} /></div>
    </details>
  );
}

/// memo：流式期间 items 引用每轮都变（Transcript 整棵重建），但未变化的条目经
/// reducer 的写时复制保持同一引用——memo 让重渲染只落在真正变化的那条上，
/// parseUserText / split 这类逐条解析不再全量重跑。
export const Message = memo(function Message({ item }: { item: ChatItem }) {
  const t = useT();
  // hover 显示精确时间：统一走 data-tip 自绘浮层（全仓不再混用原生 title，G-5）。
  // 上面「路径不进 title」的纪律针对身份信息，时间无此虑。timestamp 缺失就整个不挂。
  const stampDate = item.timestamp ? new Date(item.timestamp) : null;
  const timeTitle = stampDate && !Number.isNaN(stampDate.getTime())
    ? stampDate.toLocaleString(t.locale)
    : undefined;
  if (item.type === "user_text") {
    // 斜杠命令在 transcript 里是一条 XML 包裹的用户消息。渲染成命令徽章 + 可展开输出，
    // 而不是把 <command-name> 这类标签摊在对话里让人自己读。
    const parts = parseUserText(item.text);
    const { body, images } = splitUserText(parts.text);
    if (parts.local) {
      // 只剩免责声明（写给模型的 caveat）的那条：对人零信息量，整条不渲染。
      if (!parts.commands.length && !parts.stdout.length && !parts.notifications.length
        && !parts.crossMessages.length && !parts.text) return null;
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
          {/* 跨会话消息：另一个 Claude 会话发来的话。它是**真的对话内容**，不是机器记录，
              所以默认展开、正文按原样排（对方写的是自然语言，不是 markdown）；只把
              来源标出来，好和用户自己说的话区分开。 */}
          {parts.crossMessages.map((message, index) => (
            <CrossMessageBlock message={message} key={`cross-${index}`} />
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
          {/* 后台任务通知默认收起：整段是 XML+JSON 的机器记录，一行摘要（通知自带的
              <summary>，如「Agent "/code-review" finished」）足够定位，想看细节再展开。 */}
          {parts.notifications.map((note, index) => (
            <details className="chat-tool chat-command-out" key={`notify-${index}`}>
              <summary>
                <span className="chat-tool-name">{t.chat.taskNotification}</span>
                <span className="chat-tool-summary">{/<summary>([\s\S]*?)<\/summary>/.exec(note)?.[1]?.trim() || note}</span>
                <span className="chat-tool-chevron">›</span>
              </summary>
              <pre>{note}</pre>
            </details>
          ))}
        </article>
      );
    }
    // 用户消息保持原文（用户不是在写 markdown，行首 # 变大标题只会失真）。
    // 有图时缩略图独立成行排在**气泡上方**、气泡只包正文（Claude Code 同款布局，
    // has-images 把气泡皮从 article 挪到 .chat-text 上）；纯图消息没有气泡，只有图。
    return (
      <article className={"chat-message is-user" + (images.length ? " has-images" : "")} data-tip={timeTitle}>
        {images.length > 0 && <ImageRow images={images} />}
        {body && <div className="chat-text">{body}</div>}
      </article>
    );
  }
  if (item.type === "turn_error") {
    // 回合错误（API Error 等，CC 以 model=<synthetic> 写盘的系统插入文案）：这不是模型
    // 说的话，套错误皮而不是正文皮——红标签一眼可辨故障，原文保留供排查。原文是固定
    // 短句不是 markdown，直接平铺。
    return (
      <article className="chat-message is-turn-error" role="alert">
        <div className="chat-turn-error-label">
          <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true"><circle cx="12" cy="12" r="9" /><path d="M12 8v5M12 16.5v.5" /></svg>
          {item.label}
        </div>
        <div className="chat-text">{item.text}</div>
      </article>
    );
  }
  if (item.type === "assistant_text" || item.type === "assistant_delta") {
    return (
      <article className="chat-message is-assistant" data-tip={timeTitle}>
        <div className="chat-text chat-md"><ChatMarkdown text={item.text} /></div>
      </article>
    );
  }
  if (item.type === "reasoning" || item.type === "reasoning_delta") {
    // 长推理默认收成**预览**（显示开头几行并渐隐），而不是整段藏起来——既能一眼看到
    // agent 在想什么，又不会让上百行把结论和后续对话挤出屏幕。短的直接摊开，
    // 没必要为几行内容加一次点击。开合定夺在 ReasoningBlock(首挂载一次,流式不翻转)。
    return <ReasoningBlock item={item} />;
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
  if (item.type === "meta" && item.kind.startsWith("handoff:")) {
    // 跨 provider 接续的段间分隔条（kind = "handoff:<目标引擎展示名>"，见 ChatWindow 的
    // prefixItems 组装）：前序段历史内联在上方，分隔条标明从哪一段起换了引擎。
    return <div className="chat-meta"><span />{t.chat.handoffDivider(item.kind.slice("handoff:".length))}<span /></div>;
  }
  return <div className="chat-meta"><span />{t.chat.compact}<span /></div>;
});

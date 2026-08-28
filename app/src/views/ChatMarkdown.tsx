import { memo, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import rehypeRaw from "rehype-raw";
import rehypeSanitize, { defaultSchema } from "rehype-sanitize";
import { openLink } from "../api";
import { useT } from "../i18n";
// 语法高亮复用文件/diff 面板的同一套 hljs 基建（highlight.ts）：语言表、回退策略、
// --cc-syn-* 主题变量全部现成，不另引第二套高亮引擎——两处不分叉。
import { highlightCode, languageForFence } from "./chat/highlight";

const PLUGINS = [remarkGfm];

/// 超过这个规模的代码块不高亮：hljs 同步跑在渲染路径上，几百 KB 的日志粘贴会卡住
/// 整窗；纯文本已有滚动限高，大块不上色是可接受的降级。
const HIGHLIGHT_MAX_CHARS = 20_000;
const HIGHLIGHT_MAX_LINES = 200;

/// (lang, code) → hljs HTML 的模块级缓存：流式期间正文每个 delta 都让整条消息重新
/// 渲染，同一批历史代码块会反复走到 code 组件；memo 只挡「text 未变」的整条消息，
/// 挡不住变化那条里的旧块。上限防长会话膨胀，超限整表清空（重算成本可接受）。
const highlightCache = new Map<string, string>();
function cachedHighlight(code: string, lang: string): string {
  const key = lang + String.fromCharCode(0) + code;
  const hit = highlightCache.get(key);
  if (hit !== undefined) return hit;
  const html = highlightCode(code, lang);
  if (highlightCache.size > 500) highlightCache.clear();
  highlightCache.set(key, html);
  return html;
}

/** 制表/框线/方块字符（U+2500–259F）。模型爱用它们画结构图。 */
const BOX_DRAWING = /[─-▟]/;

/**
 * 该字符在终端网格里占几格。生成端（CLI/模型）按 wcwidth 惯例排版：CJK 与全角标点 2 格，
 * 框线 1 格，几何图形/箭头等歧义宽度按 1 格。返回 0 表示交给等宽字体自然渲染（ASCII 等）。
 */
function charCells(cp: number): 0 | 1 | 2 {
  if (
    (cp >= 0x1100 && cp <= 0x115f) || // Hangul Jamo
    (cp >= 0x2e80 && cp <= 0x303e) || // CJK 部首/符号
    (cp >= 0x3041 && cp <= 0x33ff) || // 假名/CJK 标点/全角附号
    (cp >= 0x3400 && cp <= 0x4dbf) ||
    (cp >= 0x4e00 && cp <= 0x9fff) ||
    (cp >= 0xa000 && cp <= 0xa4cf) ||
    (cp >= 0xac00 && cp <= 0xd7a3) || // Hangul 音节
    (cp >= 0xf900 && cp <= 0xfaff) ||
    (cp >= 0xfe30 && cp <= 0xfe4f) ||
    (cp >= 0xff00 && cp <= 0xff60) || // 全角 ASCII
    (cp >= 0xffe0 && cp <= 0xffe6) ||
    cp === 0x3000 || // 全角空格
    (cp >= 0x1f300 && cp <= 0x1faff) || // emoji 主区
    cp >= 0x20000
  ) {
    return 2;
  }
  // 歧义宽度的常客（几何图形 ●○■、箭头 ←→、杂项符号与 dingbats ✕✓⚠ 等）：
  // wcwidth 记 1 格，但雅黑会画得接近全角，不锁宽就会把行推歪。
  // 框线本身（U+2500–259F）不锁——Consolas 有字形且恰为 1 格。
  if (
    (cp >= 0x2190 && cp <= 0x21ff) || // 箭头
    (cp >= 0x25a0 && cp <= 0x27bf) || // 几何图形 + 杂项符号 + dingbats（框线段 2500–259F 在此区间之前）
    (cp >= 0x2b00 && cp <= 0x2bff) || // 补充箭头/星形
    cp === 0x2022
  ) {
    return 1;
  }
  return 0;
}

/**
 * 像终端一样把文本钉到字符网格上：占 2 格的字符包进 2ch 盒子、歧义符号锁 1ch，
 * 其余走等宽字体自然流。对齐从此不依赖用户装了哪款字体——这是 xterm 的策略在 DOM 里的复刻。
 * 连续的普通字符合并成整段文本节点，span 数只与宽字符数量同阶。
 */
function renderGrid(text: string): ReactNode[] {
  const out: ReactNode[] = [];
  let plain = "";
  let key = 0;
  for (const ch of text) {
    const cells = charCells(ch.codePointAt(0) ?? 0);
    if (cells === 0) {
      plain += ch;
      continue;
    }
    if (plain) {
      out.push(plain);
      plain = "";
    }
    out.push(<span key={key++} className={cells === 2 ? "chat-md-cell2" : "chat-md-cell1"}>{ch}</span>);
  }
  if (plain) out.push(plain);
  return out;
}

/** 带复制按钮与语言角标的代码块外壳。AI 输出的代码是最高频「要拿走」的内容，
 *  此前只能跨滚动区手动框选。文本从 DOM 取（innerText）——网格重排（chat-md-diagram）
 *  后 children 已不是纯文本，从 props 拼会丢字。 */
function CopyablePre({ children }: { children?: ReactNode }) {
  const t = useT();
  const preRef = useRef<HTMLPreElement>(null);
  const [copied, setCopied] = useState(false);
  const copy = () => {
    const text = preRef.current?.innerText ?? "";
    if (!text) return;
    const done = () => {
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    };
    if (navigator.clipboard?.writeText) {
      void navigator.clipboard.writeText(text).then(done).catch(() => {});
      return;
    }
    // 回退:远程页跑在明文 HTTP(局域网 IP 非 secure context),navigator.clipboard
    // 直接是 undefined——没有这条回退,手机上代码一个字都拿不走。
    // iOS Safari 三件套:readOnly(否则弹键盘且可能不建选区)、钉在 (0,0)(fixed 不给
    // 坐标会落在静态位,select 把页面滚走)、setSelectionRange(iOS 对 select() 不认账)。
    const stage = document.createElement("textarea");
    stage.value = text;
    stage.readOnly = true;
    stage.style.position = "fixed";
    stage.style.top = "0";
    stage.style.left = "0";
    stage.style.opacity = "0";
    document.body.appendChild(stage);
    stage.focus();
    stage.setSelectionRange(0, stage.value.length);
    try {
      if (document.execCommand("copy")) done();
    } catch {
      /* 连 execCommand 都没有就没辙了 */
    }
    stage.remove();
  };
  // 语言角标：react-markdown 给 code 子元素挂 language-xxx 类，从子节点 props 里读。
  let lang: string | null = null;
  if (children && typeof children === "object" && "props" in children) {
    const cls = (children as { props?: { className?: string } }).props?.className ?? "";
    lang = /language-(\w+)/.exec(cls)?.[1] ?? null;
  }
  return (
    <div className="chat-md-pre">
      <pre ref={preRef}>{children}</pre>
      <div className="chat-md-pre-tools">
        {lang && <span className="chat-md-lang">{lang}</span>}
        <button type="button" onClick={copy}>{copied ? t.chat.codeCopied : t.chat.codeCopy}</button>
      </div>
    </div>
  );
}

const components: Components = {
  pre: ({ children }) => <CopyablePre>{children}</CopyablePre>,
  // ASCII 框图的对齐前提是「中文恰为两倍拉丁宽」，但代码字体 Consolas 没有中文字形，
  // 中文会回退到比例失配的雅黑；Windows 自带字体里也不存在框线半角 + 中文两倍宽的组合。
  // 故检测到框线字符的代码块直接按网格重排（见 renderGrid）；普通代码块原样输出。
  code: ({ className, children }) => {
    const text = String(children);
    // 框图优先于高亮：对齐比颜色重要，网格重排后的 span 结构也没法再叠 hljs 输出。
    if (BOX_DRAWING.test(text)) {
      return <code className={(className ? className + " " : "") + "chat-md-diagram"}>{renderGrid(text)}</code>;
    }
    // 块级代码（带 language-xxx 类）走 hljs 高亮；内联 code 没有语言类，自然跳过。
    // 文件头「不要换成 dangerouslySetInnerHTML 方案」的禁令针对的是**渲染消息里的
    // 原始 HTML**（XSS 面）；这里的 HTML 由 hljs 从纯文本转义生成（文本全部经它
    // 转义、只插入无属性值注入面的 span），与那条禁令不冲突。
    const tag = /language-([\w+-]+)/.exec(className ?? "")?.[1];
    const lang = tag ? languageForFence(tag) : null;
    if (!lang || text.length > HIGHLIGHT_MAX_CHARS || text.split("\n").length > HIGHLIGHT_MAX_LINES) {
      return <code className={className}>{children}</code>;
    }
    // 围栏文本以 \n 收尾（remark 保留）,高亮前剥掉,免得末尾多渲染一个空行。
    const html = cachedHighlight(text.replace(/\n$/, ""), lang);
    return <code className={className} dangerouslySetInnerHTML={{ __html: html }} />;
  },
  // 链接绝不能让 webview 自己导航（这个窗口没有地址栏，跳走就回不来了）；
  // 交给后端在默认浏览器打开，scheme 校验也在后端。
  a: ({ href, children }) => {
    // 后端 open_link 只放行 http/https：mailto:/xmpp: 这类链接点了必然被拒，
    // 「可点却必失败」比不可点更糟——渲染层直接降级为纯文本（终端侧另有显示
    // 拒绝原因的通道，见 ManagedTerminal；消息气泡里没有错误展示位）。
    if (!href || !/^https?:\/\//i.test(href)) return <>{children}</>;
    return (
      <a
        href={href}
        data-tip={href}
        onClick={(event) => {
          event.preventDefault();
          void openLink(href).catch(() => {});
        }}
      >
        {children}
      </a>
    );
  },
};

/**
 * 模型输出（正文 / reasoning）的 markdown 渲染。react-markdown 不渲染内嵌原始 HTML，
 * transcript 里的 `<script>` 之类只会按文本展示——不要换成任何 dangerouslySetInnerHTML 方案。
 * memo 按 text 比较：流式期间只有 delta 那一条重新解析，历史消息不重复付解析成本。
 */
export const ChatMarkdown = memo(function ChatMarkdown({ text }: { text: string }) {
  return (
    <ReactMarkdown remarkPlugins={PLUGINS} components={components}>
      {text}
    </ReactMarkdown>
  );
});

/* ——以下是「文件」面板 markdown 预览专用形态,对话渲染绝不共用这套(见上方注释)。—— */

/** 文件预览的 sanitize 白名单：defaultSchema（GitHub 风）基础上补 README 生态
 *  常用的展示属性（img 尺寸/对齐、div/p 的 align）。脚本、事件属性、iframe
 *  仍被整体剥除——rehype-raw 先解析、sanitize 再过滤，顺序不可颠倒。 */
const FILE_SCHEMA = {
  ...defaultSchema,
  attributes: {
    ...defaultSchema.attributes,
    img: [...(defaultSchema.attributes?.img ?? []), "alt", "width", "height", "align"],
    div: [...(defaultSchema.attributes?.div ?? []), "align"],
    p: [...(defaultSchema.attributes?.p ?? []), "align"],
  },
};
// 数组常量提出来：内联字面量每次渲染都是新引用，react-markdown 会重建处理管线。
const FILE_REHYPE: import("react-markdown").Options["rehypePlugins"] = [
  rehypeRaw,
  [rehypeSanitize, FILE_SCHEMA],
];

/** 预览里的图片：http(s) 直用（CSP img-src 已放行 https:）；相对路径交给调用方
 *  解析成 data URL（仓库文件走后端读盘，webview 里没有文件系统 base 可依赖）。 */
function PreviewImage({ src, alt, width, height, resolve }: {
  src?: string;
  alt?: string;
  width?: string | number;
  height?: string | number;
  resolve: (src: string) => Promise<string | null>;
}) {
  const remote = !!src && /^https?:/i.test(src);
  const [url, setUrl] = useState<string | null>(remote ? src! : null);
  useEffect(() => {
    // src 变了先归位:useState 只在首挂时取初值,effect 又只在 resolve 成功时 setUrl,
    // 二者都不覆盖旧值。react-markdown 按位置复用节点时（切文件 A→B、同位置换图),
    // remote→remote 会一直显示第一张,relative 换到解析失败的图则残留上一张——都得先清。
    setUrl(remote ? src! : null);
    if (remote || !src) return;
    let alive = true;
    resolve(src)
      .then((resolved) => {
        if (alive && resolved) setUrl(resolved);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [src, remote, resolve]);
  if (!url) return alt ? <span className="chat-md-img-broken">{alt}</span> : null;
  return <img src={url} alt={alt ?? ""} width={width} height={height} loading="lazy" />;
}

/**
 * 「文件」面板的 markdown 预览：与对话版的两点差异——
 * 1) 渲染内嵌 HTML（README 生态大量用 <img>/<div align> 排版），但过 FILE_SCHEMA
 *    白名单，与 GitHub 渲染 README 同策略；
 * 2) 相对路径图片经 resolve 回调转 data URL（GitDiffView 里接 read_image_preview）。
 */
export const FileMarkdown = memo(function FileMarkdown({ text, resolveImage }: {
  text: string;
  resolveImage: (src: string) => Promise<string | null>;
}) {
  // 同 FILE_REHYPE 的道理：components 每次渲染换新引用会让 react-markdown 重建管线,
  // 外层 memo 形同虚设。依赖只有 resolveImage,按它缓存。
  const fileComponents: Components = useMemo(() => ({
    ...components,
    img: ({ src, alt, width, height }) => (
      <PreviewImage src={src} alt={alt} width={width} height={height} resolve={resolveImage} />
    ),
  }), [resolveImage]);
  return (
    <ReactMarkdown remarkPlugins={PLUGINS} rehypePlugins={FILE_REHYPE} components={fileComponents}>
      {text}
    </ReactMarkdown>
  );
});

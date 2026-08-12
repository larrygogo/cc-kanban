import { memo, useRef, useState, type ReactNode } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { openLink } from "../api";
import { useT } from "../i18n";

const PLUGINS = [remarkGfm];

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
    }
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
    if (!BOX_DRAWING.test(text)) return <code className={className}>{children}</code>;
    return <code className={(className ? className + " " : "") + "chat-md-diagram"}>{renderGrid(text)}</code>;
  },
  // 链接绝不能让 webview 自己导航（这个窗口没有地址栏，跳走就回不来了）；
  // 交给后端在默认浏览器打开，scheme 校验也在后端。
  a: ({ href, children }) => (
    <a
      href={href}
      title={href}
      onClick={(event) => {
        event.preventDefault();
        if (href) void openLink(href).catch(() => {});
      }}
    >
      {children}
    </a>
  ),
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

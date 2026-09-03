/**
 * 「文件」面板的 markdown 预览形态。**刻意与对话渲染（ChatMarkdown）分家在两个模块**：
 * 这里要渲染 README 里的内嵌 HTML，得引 rehype-raw（整套 HTML 解析器）+ rehype-sanitize，
 * 二者合计约 193KB minified / 43KB brotli——而对话渲染完全用不上它们。
 *
 * 同住一个模块时，只要 Message.tsx 引了 ChatMarkdown，这 43KB 就跟着进首屏 chunk；
 * 而改动面板（GitDiffView，本模块唯一消费者）在远程 UI 里是**明确砍掉**的
 * （见 ChatWindow.tsx 的 `!remoteUi()` 门控），手机端等于为一段永不渲染的代码付传输费。
 * 拆开 + GitDiffView 走 lazy 后，这段只在用户真的点开改动面板时才下载。
 *
 * 共用的 components / PLUGINS 仍从 ChatMarkdown 引入——渲染规则两处不分叉。
 */
import { memo, useEffect, useMemo, useState } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import rehypeRaw from "rehype-raw";
import rehypeSanitize, { defaultSchema } from "rehype-sanitize";
import { components, PLUGINS } from "../ChatMarkdown";


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

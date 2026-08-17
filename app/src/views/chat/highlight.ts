/**
 * 面板内代码高亮（文件预览 + diff 视图）：highlight.js 模块化引入，只打包注册的语言。
 * 刻意不做 highlightAuto 自动探测——短行/片段上误判率高、颜色跳变比不高亮更糟。
 *
 * 渲染按行、高亮按篇：全文先整体交给 hljs（多行字符串/块注释/模板串的跨行状态
 * 得以保留），再把结果 HTML 按换行拆回逐行——断行处补闭合、下一行重开同名
 * span，每行 HTML 各自平衡，可独立塞进行容器（diff 行/带行号的代码行）。
 */
import hljs from "highlight.js/lib/core";
import bash from "highlight.js/lib/languages/bash";
import c from "highlight.js/lib/languages/c";
import cpp from "highlight.js/lib/languages/cpp";
import csharp from "highlight.js/lib/languages/csharp";
import css from "highlight.js/lib/languages/css";
import diff from "highlight.js/lib/languages/diff";
import dockerfile from "highlight.js/lib/languages/dockerfile";
import go from "highlight.js/lib/languages/go";
import ini from "highlight.js/lib/languages/ini";
import java from "highlight.js/lib/languages/java";
import javascript from "highlight.js/lib/languages/javascript";
import json from "highlight.js/lib/languages/json";
import kotlin from "highlight.js/lib/languages/kotlin";
import markdown from "highlight.js/lib/languages/markdown";
import php from "highlight.js/lib/languages/php";
import plaintext from "highlight.js/lib/languages/plaintext";
import python from "highlight.js/lib/languages/python";
import ruby from "highlight.js/lib/languages/ruby";
import rust from "highlight.js/lib/languages/rust";
import sql from "highlight.js/lib/languages/sql";
import swift from "highlight.js/lib/languages/swift";
import typescript from "highlight.js/lib/languages/typescript";
import xml from "highlight.js/lib/languages/xml";
import yaml from "highlight.js/lib/languages/yaml";

const LANGUAGES = {
  bash, c, cpp, csharp, css, diff, dockerfile, go, ini, java, javascript, json,
  kotlin, markdown, php, plaintext, python, ruby, rust, sql, swift, typescript, xml, yaml,
} as const;
for (const [name, language] of Object.entries(LANGUAGES)) {
  hljs.registerLanguage(name, language);
}

/** 扩展名（小写、不含点）→ 已注册的 hljs 语言名。 */
const EXT_TO_LANG: Record<string, string> = {
  ts: "typescript", tsx: "typescript", mts: "typescript", cts: "typescript",
  js: "javascript", jsx: "javascript", mjs: "javascript", cjs: "javascript",
  json: "json",
  sh: "bash", bash: "bash", zsh: "bash",
  py: "python",
  rs: "rust",
  go: "go",
  java: "java",
  c: "c", h: "c",
  cpp: "cpp", hpp: "cpp", cc: "cpp", cxx: "cpp", hh: "cpp",
  cs: "csharp",
  css: "css", scss: "css", less: "css",
  html: "xml", htm: "xml", xml: "xml", vue: "xml", svg: "xml",
  yml: "yaml", yaml: "yaml",
  md: "markdown", markdown: "markdown",
  toml: "ini", ini: "ini", cfg: "ini",
  sql: "sql",
  swift: "swift",
  kt: "kotlin", kts: "kotlin",
  rb: "ruby",
  php: "php",
  diff: "diff", patch: "diff",
  txt: "plaintext",
};

/** markdown 代码围栏的语言标注 → 已注册的 hljs 语言名。围栏里用户/模型写的常是
 *  扩展名式别名（```ts、```py、```sh），先认注册名、再走扩展名别名表；都不认识
 *  返回 null（按纯文本渲染，不做自动探测——文件头注释里说过为什么）。 */
export function languageForFence(tag: string): string | null {
  const lower = tag.trim().toLowerCase();
  if (!lower) return null;
  // 先查别名表拿规范名（hljs 的语言自带 aliases，"ts" 直接问 getLanguage 也命中，
  // 但返回别名会让高亮缓存键分裂成 ts/typescript 两份）；表外的再问 hljs 兜底。
  return EXT_TO_LANG[lower] ?? (hljs.getLanguage(lower) ? lower : null);
}

/** 路径 → 高亮语言；不认识/无扩展名/点文件（.gitignore 等）返回 null（按纯文本渲染）。 */
export function languageForPath(path: string): string | null {
  const base = path.split("/").pop()?.toLowerCase() ?? "";
  if (base === "dockerfile" || base.startsWith("dockerfile.")) return "dockerfile";
  const dot = base.lastIndexOf(".");
  if (dot <= 0) return null;
  return EXT_TO_LANG[base.slice(dot + 1)] ?? null;
}

/** HTML 转义：纯文本兜底与 hljs 失败降级共用。 */
export function escapeHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

/** 高亮一段代码，返回 hljs HTML；语言为空/未注册/高亮抛错时回退为转义纯文本。 */
export function highlightCode(code: string, lang: string | null): string {
  if (!lang || !hljs.getLanguage(lang)) return escapeHtml(code);
  try {
    // ignoreIllegals：diff 侧会把不连续的 hunk 拼成一篇，语法不完整是常态，
    // 不能因一处解析失败整篇退成纯文本。
    return hljs.highlight(code, { language: lang, ignoreIllegals: true }).value;
  } catch {
    return escapeHtml(code);
  }
}

/**
 * 把 hljs 整篇输出按换行拆回逐行 HTML。hljs 的 value 只含三种片段：
 * `<span class="…">`、`</span>`、转义后的文本（class 值不含 '>'，按 '>' 截取安全）。
 * 维护未闭合 span 栈：遇换行先按栈深补闭合，下一行以同栈重开——跨行 token
 * （多行注释/字符串）的颜色因此延续，且每行自平衡。
 */
function splitHighlightedHtml(html: string): string[] {
  const lines: string[] = [];
  const open: string[] = [];
  let current = "";
  let index = 0;
  while (index < html.length) {
    const char = html[index];
    if (char === "\n") {
      lines.push(current + "</span>".repeat(open.length));
      current = open.join("");
      index += 1;
    } else if (char === "<") {
      const close = html.indexOf(">", index);
      if (close === -1) {
        // 残缺标签按理不会出现（hljs 输出总是成对完整）；防御性当文本吞掉。
        current += html.slice(index);
        break;
      }
      const tag = html.slice(index, close + 1);
      if (tag.startsWith("</")) open.pop();
      else open.push(tag);
      current += tag;
      index = close + 1;
    } else {
      let next = index;
      while (next < html.length && html[next] !== "<" && html[next] !== "\n") next += 1;
      current += html.slice(index, next);
      index = next;
    }
  }
  lines.push(current + "</span>".repeat(open.length));
  return lines;
}

/**
 * 逐行高亮：行数组拼成整篇送 hljs（跨行状态保留），拆回后行数与输入一一对应。
 * 拆分结果行数对不上（拆分假设被打破）时宁可整体退纯文本，也不冒错位渲染的险。
 */
export function highlightLines(lines: string[], lang: string | null): string[] {
  if (!lang || !hljs.getLanguage(lang)) return lines.map(escapeHtml);
  try {
    const html = hljs.highlight(lines.join("\n"), { language: lang, ignoreIllegals: true }).value;
    const split = splitHighlightedHtml(html);
    return split.length === lines.length ? split : lines.map(escapeHtml);
  } catch {
    return lines.map(escapeHtml);
  }
}

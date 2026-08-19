/**
 * composer 的两套输入补全（从 ChatWindow.tsx 原样抽出，行为不变）：
 * - useAtFileCompletion：`@` 文件补全（目录浏览 + 全局模糊两种模式）；
 * - useSlashCompletion：`/xx` 斜杠命令补全。
 * 键盘交互与菜单渲染仍在 ChatWindow（与 composer 的 onKeyDown 交织），这里只封
 * 状态、取数与插入逻辑。
 */
import {
  useEffect,
  useRef,
  useState,
  type Dispatch,
  type RefObject,
  type SetStateAction,
} from "react";
import { listDirEntries, searchProjectFiles } from "../../api";
import { pushEscLayer } from "../../escLayers";
import { remoteUi } from "../../remoteMode";

/** 光标前的 @token(无空白)。全角 ＠ 一并触发(中文输入法下 Shift+2 常打出全角)。 */
export function detectAtToken(value: string, caret: number): string | null {
  const before = value.slice(0, caret);
  const match = /(^|\s)[@＠]([^\s@＠]*)$/.exec(before);
  return match ? match[2] : null;
}

/** `@` 文件补全:光标前的 @token(无空白)触发,查 search_project_files 的名称命中,
 *  选中把 token 替换为 @相对路径。含空白的路径剔除——claude 的提及解析在空白处截断,
 *  后半截会变成正文(与 promptWithAttachments 的 mention 门槛同一条实测结论)。 */
export function useAtFileCompletion({ cwd, prompt, setPrompt, promptInputRef }: {
  cwd: string | null;
  prompt: string;
  setPrompt: Dispatch<SetStateAction<string>>;
  promptInputRef: RefObject<HTMLTextAreaElement>;
}) {
  const [atQuery, setAtQuery] = useState<string | null>(null);
  const [atFiles, setAtFiles] = useState<string[]>([]);
  const [atIndex, setAtIndex] = useState(0);
  const atSeqRef = useRef(0);
  const atActive = atFiles.length ? Math.min(atIndex, atFiles.length - 1) : -1;
  useEffect(() => {
    // 远程:list_dir_entries / search_project_files 是 /rpc 拒绝项(v1 砍文件浏览),
    // 补全永远给不出候选——干脆不出菜单,免得看着像坏了。
    if (atQuery === null || !cwd || remoteUi()) { setAtFiles([]); return; }
    const seq = ++atSeqRef.current;
    // 两种模式:
    // - 目录浏览:`@` 一打出立即列 cwd 顶层(Claude Code 惯例——不是打满 2 个字符才有
    //   反应);token 含 `/` 时列对应子目录,末段做前缀过滤。目录带 `/` 后缀,可下钻。
    // - 全局模糊:纯文件名片段(≥2 字符、不含 `/`)走 search_project_files 的名称命中,
    //   跨目录直达(后端对 <2 字节直接回空,单字符噪声太大)。
    const slash = atQuery.lastIndexOf("/");
    const dirPart = slash >= 0 ? atQuery.slice(0, slash) : "";
    const leaf = slash >= 0 ? atQuery.slice(slash + 1) : atQuery;
    const browse = slash >= 0 || leaf.length < 2;
    const timer = window.setTimeout(() => {
      const apply = (list: string[]) => {
        if (atSeqRef.current !== seq) return;
        setAtFiles(list.slice(0, 8));
        setAtIndex(0);
      };
      if (browse) {
        listDirEntries(cwd, dirPart)
          .then((entries) => apply(entries
            .filter((entry) => !/\s/.test(entry.relPath)
              && (leaf === "" || entry.name.toLowerCase().startsWith(leaf.toLowerCase())))
            .map((entry) => entry.relPath + (entry.isDir ? "/" : ""))))
          .catch(() => {});
      } else {
        searchProjectFiles(cwd, atQuery)
          .then((result) => apply(result.files
            .filter((file) => !file.isDir && file.nameMatch && !/\s/.test(file.relPath))
            .map((file) => file.relPath)))
          .catch(() => {});
      }
    }, browse ? 80 : 200);
    return () => window.clearTimeout(timer);
  }, [atQuery, cwd]);
  // 输入框清空(发送成功/切会话)时收起补全。
  useEffect(() => {
    if (!prompt) { setAtQuery(null); setAtFiles([]); }
  }, [prompt]);
  const pickAtFile = (rel: string) => {
    const el = promptInputRef.current;
    const caret = el?.selectionStart ?? prompt.length;
    const before = prompt.slice(0, caret);
    const match = /(^|\s)[@＠]([^\s@＠]*)$/.exec(before);
    if (!match) { setAtQuery(null); setAtFiles([]); return; }
    const start = caret - match[2].length - 1;
    // 目录(`/` 结尾):插入后**不关菜单**,token 更新为该目录继续下钻;文件:补空格收尾。
    const isDir = rel.endsWith("/");
    const next = prompt.slice(0, start) + "@" + rel + (isDir ? "" : " ") + prompt.slice(caret);
    setPrompt(next);
    setAtQuery(isDir ? rel : null);
    if (!isDir) setAtFiles([]);
    requestAnimationFrame(() => {
      const pos = start + 1 + rel.length + (isDir ? 0 : 1);
      promptInputRef.current?.setSelectionRange(pos, pos);
      promptInputRef.current?.focus();
    });
  };
  return { atQuery, setAtQuery, atFiles, setAtFiles, atActive, setAtIndex, pickAtFile };
}

/** "/xx" 且尚未输入空格时给补全候选；一旦带参数或是普通句子就收起。
 *  键盘交互：默认高亮第一项，↑↓ 移动，Enter/Tab 选中写入（不是发送），Esc 收起本次
 *  （dismissed 期间不再弹出，继续输入即恢复）。高亮随每次输入重置回第一项。 */
export function useSlashCompletion<C extends { name: string }>(prompt: string, commands: C[] | undefined) {
  const [slashIndex, setSlashIndex] = useState(0);
  const [slashDismissed, setSlashDismissed] = useState(false);
  const slashMenuRef = useRef<HTMLDivElement>(null);
  const slashMatches = prompt.startsWith("/") && !prompt.includes(" ") && !slashDismissed
    ? (commands ?? []).filter((c) => c.name.startsWith(prompt) && c.name !== prompt)
    : [];
  // 候选变少时高亮可能越界（如退格删字后），钳回最后一项；无候选时无高亮。
  const slashActive = slashMatches.length ? Math.min(slashIndex, slashMatches.length - 1) : -1;
  // 补全菜单开着 = 一个 Esc 消费层（textarea 的 onKeyDown 会用 Esc 收起它）。
  const slashOpen = slashMatches.length > 0;
  useEffect(() => {
    if (!slashOpen) return;
    return pushEscLayer();
  }, [slashOpen]);
  // 键盘把高亮移出可视区时滚回来（菜单限高、内部滚动）。jsdom 没有 scrollIntoView，判一下再调。
  useEffect(() => {
    const selected = slashMenuRef.current?.querySelector('[aria-selected="true"]');
    if (typeof selected?.scrollIntoView === "function") selected.scrollIntoView({ block: "nearest" });
  }, [slashActive]);
  return { slashIndex, setSlashIndex, slashDismissed, setSlashDismissed, slashMenuRef, slashMatches, slashActive };
}

import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type Dispatch, type SetStateAction, type PointerEvent as ReactPointerEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
// 文件拖入走 Tauri 的 drag-drop 事件(webview 原生 drop 被拦截,DOM 拿不到 File;
// Tauri 事件反而直接给**源路径**,附件协议正好只要路径)。
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { pathKey } from "../paths";
import { appConfirm } from "../confirm";
import { pickAndAddExtraDir, removeSessionExtraDir, agentChatUi, attachBackgroundSession, sendBackgroundPrompt, clipboardRestore, clipboardSetImage, confirmStopSession, dismissInteractiveQuestion, getChatHistory, getGitDiffSummary, getLiveSessionsPage, getSessionLineage, isExternallyHeld, managedTerminalBinding, managedTerminalSnapshot, openNewSessionWindow, refreshSessionModel, refreshSessionTodos, renameSession as renameSessionCmd, resolvePendingApproval, savePastedAttachment, sessionLaunchSelections, setArchived as setArchivedCmd, setSessionLaunchSelection, sessionTone, startManagedTerminal, switchSessionProvider, takeoverManagedTerminal, writeManagedTerminal, type AgentId, type ChatHistory, type ChatItem, type ChatUi, type GitDiffSummaryDto, type LiveSession, type ModelPreset, type ModeScreenMarker } from "../api";
import { hasEscLayers, pushEscLayer } from "../escLayers";
import { useTauriEvent } from "../hooks/useTauriEvent";
import { useT } from "../i18n";
import { formatBackendError } from "../i18n/errors";
import { useShowWhenReady } from "../useShowWhenReady";
import { collectSubagentReceipts, reduceChatEvents } from "../chat/reducer";
import { ApprovalCard } from "./chat/ApprovalCard";
import { composeAnswerBody, matchOptionByLabel, observeTranscriptForDismiss, parseAskUserQuestions, type QuestionAnswerDraft, type QuestionDismissTracker, type StructuredQuestion } from "./chat/askUserQuestion";
import { detectAtToken, useAtFileCompletion, useSlashCompletion } from "./chat/composerCompletion";
import { useApprovalChannel } from "./chat/useApprovalChannel";
import { useModelPresets } from "./chat/useModelPresets";
import { ContextMeter } from "./chat/ContextMeter";
import { GitDiffView } from "./chat/GitDiffView";
import { ImageRef } from "./chat/Message";
import { TodoPanel } from "./chat/TodoPanel";
import { QuestionPanels } from "./chat/QuestionPanels";
import { ChatTitleMenu, ChatTodoMenu, type TodoPanelRow } from "./chat/TitleMenus";
import { QuickSwitcher, RenameModal, ShortcutSheet } from "./chat/WindowModals";
import { Transcript } from "./chat/Transcript";
import { ChatSidebar } from "./ChatSidebar";
import { ChevronDownIcon } from "./sticker/icons";
import { useStarred } from "./sticker/helpers";
import { ManagedTerminal } from "./ManagedTerminal";
import { WindowControls } from "./WindowControls";
import { DevBadge } from "./DevBadge";
import { isMac } from "../platform";
import { remoteUi, NEW_SESSION_EVENT } from "../remoteMode";
import { appendTerminalText, modeFromScreen, terminalAttention as detectTerminalAttention, visibleTerminalText, type AttentionGrammar, type TerminalAttention, type TerminalAttentionOption } from "../terminalAttention";
import { Dropdown, useMenuPopup } from "./menu";
// 恢复时的权限改选需要 agent 的启动选项声明表（与新建会话面板同源）。
import { listAgents, type AgentDescriptor } from "../api";
// 「切换引擎」分组的目标 agent 图标（前端资产表，未知 id 走中性兜底）。
import { agentAssets, tintStyle } from "../providers";

/** 远程刷新后回到上次看的会话(桌面开窗恒带 ?sessionId,不落到这条)。 */
const REMOTE_LAST_SESSION_KEY = "meowo-remote-last-session";
/** initialSessionId 若用了存储恢复,记下该 id:恢复目标可能已被删/归档,首次加载
 *  失败要放弃恢复回空态,而不是卡在「读取失败」(自审 M4)。 */
let restoredSessionId = 0;

function initialSessionId(): number {
  const value = new URLSearchParams(window.location.search).get("sessionId");
  const id = Number(value);
  if (Number.isSafeInteger(id) && id !== 0) return id;
  if (remoteUi()) {
    const stored = Number(localStorage.getItem(REMOTE_LAST_SESSION_KEY));
    if (Number.isSafeInteger(stored) && stored > 0) {
      restoredSessionId = stored;
      return stored;
    }
  }
  // 0 = 尚未选中任何会话(仅远程首开会出现):渲染「去侧栏选会话」空态,不是加载态。
  return 0;
}

const SIDEBAR_COLLAPSED_KEY = "meowo-chat-sidebar-collapsed";
// 视图偏好：用户在终端就显示终端、在对话就显示对话（实拍反馈）。只记**明确的**视图
// 选择（顶栏「对话/终端」切换、Ctrl+1/2）；程序性跳转（审批引导、后台会话兜底、
// 探测失败落终端）不算——偏好表达「用户平时住在哪」，不是「上次被带去过哪」。
const VIEW_PREF_KEY = "meowo-chat-view-pref";
function storedViewPref(): "chat" | "terminal" {
  return localStorage.getItem(VIEW_PREF_KEY) === "terminal" ? "terminal" : "chat";
}
function rememberViewPref(view: "chat" | "terminal") {
  localStorage.setItem(VIEW_PREF_KEY, view);
}
/** 远程模式选文件:临时挂一个隐藏 <input type=file multiple>,点开系统选择器,resolve 选中的 File。
 *  取消(未选)时 change 不触发,靠 window focus 兜底 resolve 空数组,避免 Promise 永挂。 */
function pickFilesViaInput(): Promise<File[]> {
  return new Promise((resolve) => {
    const input = document.createElement("input");
    input.type = "file";
    input.multiple = true;
    input.style.display = "none";
    let settled = false;
    const done = (files: File[]) => {
      if (settled) return;
      settled = true;
      window.removeEventListener("focus", onFocus);
      input.remove();
      resolve(files);
    };
    input.addEventListener("change", () => done(input.files ? Array.from(input.files) : []));
    // 用户点「取消」:多数浏览器不发 change,只在窗口重新获焦后能判定放弃。延迟要给足:
    // iOS/低端安卓的 change 可比 refocus 晚数百毫秒,300ms 会把真选中的照片静默吞掉。
    const onFocus = () => setTimeout(() => done([]), 1000);
    window.addEventListener("focus", onFocus, { once: true });
    document.body.appendChild(input);
    input.click();
  });
}
/** 改动/文件停靠面板的宽度持久化键（localStorage 既有 meowo-* 命名惯例）。 */
const DIFF_WIDTH_KEY = "meowo-diff-panel-width";
/** 面板宽度上下限：下限保面板可用，上限在拖拽时给对话区留 MIN_CHAT 动态计算。 */
const DIFF_PANEL_MIN_WIDTH = 280;
const DIFF_PANEL_DEFAULT_WIDTH = 400;
/** 窄窗门限：低于它停靠侧栏放不下（208px 侧栏 + 卡片最小可用宽），自动收起、开关改开浮窗。 */
const SIDEBAR_NARROW_QUERY = "(max-width: 880px)";

function approvalSuggestionParts(suggestion: unknown, index: number, t: ReturnType<typeof useT>): { base: string; detail: string } {
  if (!suggestion || typeof suggestion !== "object" || Array.isArray(suggestion)) {
    return { base: t.chat.allowSuggested(index + 1), detail: "" };
  }
  const entry = suggestion as Record<string, unknown>;
  const destination = entry.destination;
  const base = (() => { switch (destination) {
    case "session": return t.chat.allowSession;
    case "localSettings": return t.chat.allowLocalProject;
    case "projectSettings": return t.chat.allowProject;
    case "userSettings": return t.chat.allowUser;
    default: return t.chat.allowSuggested(index + 1);
  } })();
  const firstRule = Array.isArray(entry.rules) ? entry.rules[0] : null;
  if (!firstRule || typeof firstRule !== "object" || Array.isArray(firstRule)) return { base, detail: "" };
  const rule = firstRule as Record<string, unknown>;
  const tool = typeof rule.toolName === "string" ? rule.toolName : "";
  const content = typeof rule.ruleContent === "string" ? rule.ruleContent : "";
  return { base, detail: content || tool };
}

function approvalSuggestionLabel(suggestion: unknown, index: number, t: ReturnType<typeof useT>): string {
  const { base, detail } = approvalSuggestionParts(suggestion, index, t);
  if (!detail) return base;
  const short = detail.length > 42 ? detail.slice(0, 41) + "…" : detail;
  return `${base} · ${short}`;
}

/// 悬浮提示与按钮文案同源，但不截断：按钮上 42 字截断的长命令在这里能看全。
/// 此前直接 JSON.stringify 整个建议——把协议内部结构甩给用户，谁也读不动。
function approvalSuggestionTip(suggestion: unknown, index: number, t: ReturnType<typeof useT>): string {
  const { base, detail } = approvalSuggestionParts(suggestion, index, t);
  return detail ? `${base} · ${detail}` : base;
}

const PROCEED_QUESTION = /^do you want to proceed\?$/i;

/// 「proceed_box」框式审批详情（claude 真机取证的屏幕文法）。用哪种解析器由插件随
/// AttentionPattern.details 声明，前端不再按 pattern id 猜。
function proceedBoxApprovalDetails(text: string) {
  // TUI 把标题行用横线铺满整屏宽（"Do you want to proceed? ──────"）。这些框线是
  // 排版，不是内容：行尾的要剥掉，整行都是框线的整行丢弃。
  const lines = text.split("\n")
    // U+2500–U+257F 是 Unicode 的制表符块（─ ━ │ ┌ ╌ … TUI 画框全用它）。
    .map((line) => line.replace(/[\s─-╿]+$/u, "").trim())
    .filter((line) => line && !/^[\s─-╿|+-]+$/u.test(line));
  // 问句是审批框的末行，它之后的东西不属于这次请求。同样取**最后一次**出现——重绘
  // 残留会让同一句问话在缓冲里出现好几遍，取第一处就会把两屏内容都收进命令区。
  const questionIndex = lines.reduce((found, line, index) => (PROCEED_QUESTION.test(line) ? index : found), -1);
  const question = questionIndex >= 0 ? lines[questionIndex] : "";
  let before = questionIndex >= 0 ? lines.slice(0, questionIndex) : lines;
  const marker = before.findIndex((line) => /this command requires approval/i.test(line));
  if (marker >= 0) before = before.slice(0, marker);
  // 审批框以工具头（"Bash command"）开头；只取最后一个工具头之后的内容，
  // 避免把上一屏残留的输出并进命令文本。
  const header = before.reduce((found, line, index) => (/^bash command$/i.test(line) ? index : found), -1);
  if (header >= 0) before = before.slice(header + 1);
  before = before.filter((line) => !PROCEED_QUESTION.test(line));
  // 末行是「用途说明」这个判断，只有认出了框的边界（工具头或 requires-approval 行）
  // 才成立。认不出时整段进命令区，不去猜哪一行是说明——猜错就是把半条命令当成说明
  // 单独拎出来显示，比不显示更误导。
  const framed = header >= 0 || marker >= 0;
  const description = framed && before.length >= 2 ? before[before.length - 1] : "";
  return {
    tool: "Bash",
    // 长命令会按终端宽度硬换行成多行；除末行（用途说明）外全部并入命令整段显示，
    // 不能按「倒数第二行是命令」取——那只会摘到换行后的最后一个片段。
    command: (description ? before.slice(0, -1) : before).join("\n"),
    description,
    question,
  };
}

/// 「arrow_panel」面板式审批详情（kimi 官方源码取证 apps/kimi-code/.../approval-panel.ts @ 0.29）：
/// 标题行 ▶ <按工具定制的问题>（Run this command? / Write this file? / Approve X?），
/// 之下是命令/diff 等 display 块。详情区剥掉框线与标题，正文整段进 pre；
/// 工具名从标题反推，反推不到就留空（工具行整体不显示，比显示一个猜的名字诚实）。
function arrowPanelApprovalDetails(text: string) {
  const lines = text.split("\n").map((line) => line.trim()).filter(Boolean)
    .filter((line) => !/^[─━═|-]+$/.test(line));
  const headerIndex = lines.findIndex((line) => /^▶\s*[^\n]+\?$/.test(line));
  const question = headerIndex >= 0 ? lines[headerIndex].replace(/^▶\s*/, "") : "";
  const command = (headerIndex >= 0 ? lines.slice(headerIndex + 1) : lines).join("\n");
  const tool = /run this command/i.test(question) ? "Bash"
    : /write this file/i.test(question) ? "Write"
    : /apply these edits/i.test(question) ? "Edit"
    : /stop this task/i.test(question) ? "TaskStop"
    : /^approve ([^\n?]+)\?$/i.exec(question)?.[1] ?? "";
  return { tool, command, description: "", question };
}

/// 两次 history 的**渲染相关**字段是否完全一致。稳态轮询里这些值一轮都不变，
/// 据此保留旧引用即可跳过整窗重渲染。
///
/// **排除法**而非白名单:除 items/offset/reset(由 setItems 单独短路)外,next 的
/// 所有字段默认参与比较——后端新增 DTO 字段自动被覆盖,无需记得回来改这里。
/// 此前的逐字段清单同一类漏加事故出了三回(todos、connected、ptyManaged/errored),
/// 每次都表现为「数据变了但界面不动」,不再给第四次机会。
const HISTORY_META_EXCLUDED: ReadonlySet<string> = new Set(["items", "offset", "reset"]);
function sameHistoryMeta(prev: ChatHistory | null, next: ChatHistory): boolean {
  if (prev === null) return false;
  for (const key of Object.keys(next) as (keyof ChatHistory)[]) {
    if (HISTORY_META_EXCLUDED.has(key)) continue;
    if (key === "agentModes") {
      if (prev.agentModes.length !== next.agentModes.length) return false;
      if (!prev.agentModes.every((mode, index) => mode.dimension === next.agentModes[index]?.dimension && mode.value === next.agentModes[index]?.value)) return false;
    } else if (key === "todos") {
      // 容错取值：旧后端没有这个字段，不能因为版本错配就整窗崩掉。
      if ((prev.todos?.length ?? 0) !== (next.todos?.length ?? 0)) return false;
      if (!(prev.todos ?? []).every((todo, index) =>
        todo.content === next.todos?.[index]?.content && todo.status === next.todos?.[index]?.status)) return false;
    } else if (prev[key] !== next[key]) {
      return false;
    }
  }
  return true;
}

type Attachment = { path: string; name: string; image: boolean;
  /** 字节数，仅粘贴来源有（File.size 现成）；文件选择器只给路径，拿大小要多一次 IPC，不值。 */
  size?: number };

/** 附件卡片的体积行。附件都是本地小文件，GB 档没有出场机会。 */
function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(bytes < 100 * 1024 ? 1 : 0)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
const IMAGE_EXTENSIONS = new Set(["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg"]);

function attachmentOf(path: string): Attachment {
  const name = path.split(/[\\/]/).pop() || path;
  const extension = name.includes(".") ? name.split(".").pop()!.toLowerCase() : "";
  return { path, name, image: IMAGE_EXTENSIONS.has(extension) };
}

/** 草稿持久化（localStorage）：关窗/重启不丢——此前只在内存 Map，窗口一关半屏提示词
 *  就没了（附件反而因落盘临时文件幸存）。只存正式会话（负数临时 id 认领后会换号）；
 *  LRU 保 20 条防膨胀。 */
const DRAFTS_KEY = "meowo-chat-drafts";
type StoredDraft = { prompt: string; attachments: Attachment[]; at?: number };
function loadStoredDrafts(): Map<number, { prompt: string; attachments: Attachment[] }> {
  const map = new Map<number, { prompt: string; attachments: Attachment[] }>();
  try {
    const raw = JSON.parse(localStorage.getItem(DRAFTS_KEY) || "{}") as Record<string, StoredDraft>;
    for (const [key, value] of Object.entries(raw)) {
      const id = Number(key);
      if (!Number.isSafeInteger(id) || typeof value?.prompt !== "string") continue;
      const attachments = Array.isArray(value.attachments)
        ? value.attachments.filter((file) => file && typeof file.path === "string")
        : [];
      map.set(id, { prompt: value.prompt, attachments });
    }
  } catch { /* 坏数据当无草稿 */ }
  return map;
}

// 交接注入语（两种界面语言的模板前缀，见 i18n handoffPrompt）+ 交接目录标记：识别
// transcript 里那条「请读交接文件」的机器消息。前序段历史内联进时间线后，它再以用户
// 气泡出现只是重复分隔条已说的内容——显示层滤掉；items 记账层保留原文，水位线等
// 逻辑不受影响。
const HANDOFF_PROMPT_PREFIXES = ["请先完整阅读文件 ", "Please read the file "];
function isHandoffInjectedPrompt(text: string): boolean {
  return text.includes("meowo-handoff") && HANDOFF_PROMPT_PREFIXES.some((prefix) => text.startsWith(prefix));
}

function promptWithAttachments(prompt: string, attachments: Attachment[], instruction: (files: string) => string, mention = false): string {
  if (!attachments.length) return prompt;
  // @提及是提交时的原生附加(claude/gemini 实测,由插件按版本声明 attachment_mention)。
  // 两个硬性例外整条退回指令文本,保持单一注入形态:
  // - 路径含空白:提及解析在空白处截断(claude 实测),后半截变成正文;
  // - 图片:@提及不会作为图像块附加(claude -p 实测,模型明说看不到),指令文本反而
  //   能引导 agent 用自己的读图工具打开。
  if (mention && attachments.every((file) => !file.image && !/\s/.test(file.path))) {
    const mentions = attachments.map((file) => `@${file.path}`).join(" ");
    return prompt.trim() ? `${mentions} ${prompt.trim()}` : mentions;
  }
  // 指令文案随界面语言（i18n），不能硬编码中文——英文界面会把中文指令发给 agent。
  const directive = instruction(attachments.map((file) => `- ${file.path}`).join("\n"));
  return prompt.trim() ? `${prompt.trim()}\n\n${directive}` : directive;
}

/// ArrayBuffer → base64。btoa 只吃字符串且实参展开有调用栈上限，分块拼接。
function base64OfBuffer(buffer: ArrayBuffer): string {
  const bytes = new Uint8Array(buffer);
  let binary = "";
  const CHUNK = 0x8000;
  for (let i = 0; i < bytes.length; i += CHUNK) {
    binary += String.fromCharCode(...bytes.subarray(i, i + CHUNK));
  }
  return btoa(binary);
}

function terminalInput(content: string): string {
  // 多行内容必须作为一次 bracketed paste 交给 TUI composer，否则附件列表中的换行可能被当成
  // 多次 Enter，导致第一行提前提交。单行保持原协议，兼容不启用 bracketed paste 的旧 CLI。
  return content.includes("\n") ? `\x1b[200~${content}\x1b[201~` : content;
}

/// 正文与 Enter 之间的间隔。实测（probe_enter.rs 对真实 kimi）：
/// 20ms / 60ms 失败——正文刚落，斜杠命令的**补全菜单还在异步渲染**，此时的 Enter 被菜单
/// 吃掉（当成选中候选）而不是提交 composer，正文就留在框里只换了行；
/// 150ms / 400ms 稳定成功。取 250ms：在 150ms 的成功线之上留足余量，人也感知不到。
const SUBMIT_GAP_MS = 250;
// 稳定的空数组：`?? []` 每次渲染都是新引用，作为 props/依赖会让下游 memo/effect 空转
// （chatUi 未就绪时每 650ms 轮询渲染一次，ManagedTerminal 的标记 key 就白算一次）。
const EMPTY_MARKERS: string[] = [];
// 运行条活动文本的展示清洗：剥掉「cd <目录> && 」前缀（保留可能的 ›/> 提示符）。
// 状态条截断成单行后，冗长的项目路径会把真正在跑的命令挤出可视区；完整原文在 title 里。
const trimActivityCdPrefix = (activity: string): string =>
  activity.replace(/^([›>]\s*)?cd\s+("[^"]*"|'[^']*'|[^\s&]+)\s+&&\s*/, "$1");


/// 把 content 写进 composer 并提交：正文与回车**必须分两次写**。
///
/// 实测依据（app/src-tauri/tests/probe_enter.rs 对真实 kimi 的 PTY 探针）：
/// - 分两次写 `/plan on` → `\r`：状态栏切到 plan、输入框清空 = **提交成功**
/// - 合并成一次写 `"/plan on\r"`：`/plan on` 留在输入框里 = **只换行不提交**
///
/// 原因是 TUI 把一次写入当成一个输入事件/粘贴块处理，块内的 `\r` 只是内容里的换行，
/// 而不是「按下 Enter」。两次写才符合真实键盘的语义。多行内容同理，且必须先包成
/// bracketed paste（见 terminalInput），否则中间的换行会被当成多次 Enter 提前提交。
///
/// `abortIf`:回车前的最后复核。发送守卫只在开头查一次终端占用,而屏幕识别有 150ms
/// 节流、正文与回车之间又隔 SUBMIT_GAP_MS——交互提示恰在这窗口里弹出时,这个 `\r`
/// 会替用户回答它(如命令审批的默认焦点项)。复核命中则不发回车,并 Ctrl-U 尽力清掉
/// 已写进对方界面的正文,抛错让调用方把「终端在等交互」呈现出来。
///
/// `verify`:写后回显验证(恢复/接管后的险区才传,见 RESUME_VERIFY_WINDOW_MS)。刚拉起
/// 的 CLI 可能还压着 resume 确认选择器、或 composer 尚未挂载——waitForTerminalReady 的
/// 「可见 + 安静 700ms」对这两种形态都会误判成就绪。此时正文被吞,而无条件回车会替用户
/// 按下选择器的默认焦点项:消息蒸发、恢复继续、没有任何报错(实拍:接管后对话页发送,
/// transcript 里根本没有这条 user 消息)。写完正文先确认它真的出现在屏幕上,再发回车。
async function submitToTerminal(sessionId: number, content: string, abortIf?: () => string | null, verify?: { message: string }): Promise<void> {
  const before = verify ? await managedTerminalSnapshot(sessionId) : null;
  await writeManagedTerminal(sessionId, terminalInput(content));
  if (before && verify) {
    // 匹配规则:正文与可见文本都压掉空白与框线字符后做前缀包含——回显会被 composer
    // 边框/自动折行打散;长粘贴认 claude 的占位形态(Pasted text)。只扫**写入之后的
    // 增量**输出:屏幕旧内容(上一回合恰好含同样字句)不作数。
    const squash = (text: string) => text.replace(/[\s│┃❯>|]/g, "").toLowerCase();
    const needle = squash(content).slice(0, 16);
    const decoder = new TextDecoder();
    let tail = "";
    let since = Number.isFinite(before.endOffset) ? before.endOffset : 0;
    let seen = needle.length === 0;
    for (let attempt = 0; attempt < 10 && !seen; attempt += 1) {
      await new Promise((resolve) => window.setTimeout(resolve, 200));
      const snapshot = await managedTerminalSnapshot(sessionId, since);
      if (!snapshot.active) break;
      if (Number.isFinite(snapshot.endOffset) && snapshot.endOffset < since) {
        // 偏移回绕(PTY 被重启复位):归零重对齐,别拿旧偏移空等(同 sendWithClipboardImages)。
        since = 0;
        tail = "";
        continue;
      }
      since = snapshot.endOffset;
      tail = appendTerminalText(tail, snapshot.data, decoder);
      const visible = visibleTerminalText(tail);
      if (squash(visible).includes(needle) || /pasted text/i.test(visible)) seen = true;
    }
    if (!seen) {
      // 回显没来 = 正文没进 composer(被恢复确认/启动提示拦下,或被初始化丢弃)。
      // **绝不能发回车**;Ctrl-U 尽力清掉可能已落进对方界面的残余,抛错让消息退回输入框。
      await writeManagedTerminal(sessionId, "\x15").catch(() => {});
      throw new Error(verify.message);
    }
  }
  await new Promise((resolve) => window.setTimeout(resolve, SUBMIT_GAP_MS));
  const abortReason = abortIf?.() ?? null;
  if (abortReason) {
    await writeManagedTerminal(sessionId, "\x15").catch(() => {});
    throw new Error(abortReason);
  }
  await writeManagedTerminal(sessionId, "\r");
}

/// 恢复/接管完成后的险区时间窗:这段时间内的发送启用 submitToTerminal 的写后回显验证。
/// 稳态终端不验证——占用检查、attention 识别与 abortIf 复核已经覆盖,逐条轮询屏幕只会
/// 拖慢每次发送。
const RESUME_VERIFY_WINDOW_MS = 15_000;

type TerminalStartupResult = "ready" | TerminalAttention;

type TerminalReadyMessages = {
  exited: (code: number | null) => string;
  failed: string;
  timeout: string;
};

async function waitForTerminalReady(sessionId: number, attentionMarkers: string[], messages: TerminalReadyMessages, grammar?: AttentionGrammar): Promise<TerminalStartupResult> {
  const startedAt = Date.now();
  const decoder = new TextDecoder();
  let outputTail = "";
  let lastOutputAt = 0;
  let hasVisible = false;
  // 带 since 只拉增量；保留一小段解码后的尾部，让跨 IPC 分片的提示仍可识别。
  let since = 0;
  while (Date.now() - startedAt < 45_000) {
    const snapshot = await managedTerminalSnapshot(sessionId, since);
    if (!snapshot.active) {
      if (snapshot.exited) {
        throw new Error(messages.exited(snapshot.exitCode ?? null));
      }
      throw new Error(messages.failed);
    }
    // 判「有新输出」看 endOffset 而不是 data：data 现在是增量，首帧之后的轮次
    // 常常为空，用它判断会把已经就绪的终端误判成还没输出。
    const grew = snapshot.endOffset > since;
    since = snapshot.endOffset;
    outputTail = appendTerminalText(outputTail, snapshot.data, decoder);
    const attention = detectTerminalAttention(outputTail, attentionMarkers, false, false, grammar);
    if (attention) return attention;
    if (grew) lastOutputAt = Date.now();
    if (!hasVisible && visibleTerminalText(outputTail)) hasVisible = true;
    // 就绪 = 已画出可见内容（--resume 启动阶段只有清屏/光标序列，不算）且输出安静了
    // 700ms。回放长 transcript 时输出持续、计时随之顺延，不会把消息写进还在初始化的
    // composer；固定「首字节后 700ms」正是之前吞消息的根因。
    if (hasVisible && lastOutputAt && Date.now() - lastOutputAt >= 700) return "ready";
    // 极端情况：TUI 常驻动画让输出永不安静。已有可见画面且没识别到阻塞提示时，
    // 20 秒后按就绪处理——比直接超时报错对用户更有用。
    if (hasVisible && Date.now() - startedAt >= 20_000) return "ready";
    await new Promise((resolve) => window.setTimeout(resolve, 80));
  }
  throw new Error(messages.timeout);
}

export function ChatWindow() {
  const t = useT();
  // 窗口以 visible:false 创建（window.rs），首帧渲染后再显示，消除打开瞬间的白框闪烁。
  useShowWhenReady();
  const [sessionId, setSessionId] = useState(initialSessionId);
  const [history, setHistory] = useState<ChatHistory | null>(null);
  const [items, setItems] = useState<ChatItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [failed, setFailed] = useState(false);
  // 是否贴在底部（followRef 的渲染镜像）：驱动「回到最新」悬浮钮显隐。
  const [atBottom, setAtBottom] = useState(true);
  // 首读裁剪掉的更早消息：hasMore 只在首读那一发为 true，轮询会把它带回 false，
  // 所以单独存一份状态，别直接读 history.hasMore（提示会闪一下就没）。
  const [hasEarlier, setHasEarlier] = useState(false);
  const [loadingEarlier, setLoadingEarlier] = useState(false);
  // 新建会话（负数临时 id）落在用户偏好的视图上：对话页此时渲染「启动中」占位，
  // 发送链路（writeManagedTerminal）对临时 id 本就可用。已有会话恒从对话页进。
  const [view, setViewState] = useState<"chat" | "terminal">(
    remoteUi() ? "chat" : sessionId < 0 ? storedViewPref() : "chat",
  );
  // 远程 UI 没有可用的 xterm 终端视图(手机上不渲染 250KB 终端,也没有 pty-output 实时流)。
  // setView 在此收口:远程一律钉在对话页,散落各处的 setView("terminal")(后台自动切、
  // 空态出口、去终端作答兜底)统统无害化——隐藏的 ManagedTerminal 仍按需挂载做屏幕识别,
  // 但 visible 恒 false。桌面无此改写,行为逐字不变。
  const setView = useCallback<Dispatch<SetStateAction<"chat" | "terminal">>>((next) => {
    setViewState((prev) => {
      const resolved = typeof next === "function" ? next(prev) : next;
      return remoteUi() && resolved === "terminal" ? "chat" : resolved;
    });
  }, []);
  // 认领迟迟不来（信任目录询问/登录把 CLI 启动卡住，SessionStart hook 不触发）时，
  // 启动占位下亮「打开终端」出口——那些询问只在终端画面上，对话页看不见。
  // 计时器而非屏幕识别：认领前 provider 未知（history 还是 null），拿不到识别标记，
  // 且超时出口对报错退出等一切启动阻塞都兜底。
  const [startingSlow, setStartingSlow] = useState(false);
  useEffect(() => {
    setStartingSlow(false);
    if (sessionId >= 0) return;
    const timer = window.setTimeout(() => setStartingSlow(true), 8_000);
    return () => window.clearTimeout(timer);
  }, [sessionId]);
  const [prompt, setPrompt] = useState("");
  const [sending, setSending] = useState(false);
  const [sendError, setSendError] = useState("");
  // 软拦非阻断提示:pendingReview 但屏幕没识别出卡片时,发送照常、亮这条横幅(见 withSendGuard)。
  const [softPromptNotice, setSoftPromptNotice] = useState(false);
  // 运行中发出的插话:CLI 把它们排队到回合结束,期间 transcript 不显示——这里记账给
  // 用户回执。消解见轮询侧 effect(回合结束整单清空;单条提前现身 transcript 也移除)。
  // 带 id 而非裸文本:用户可手动移除单条回执,重复文本按下标删会在自动消解并发时错位。
  // 水位线快照(priorUserText / priorItemIds):消解只认**入队之后**才出现的证据——
  // 「包含」匹配对短插话(ok/继续/yes)常态性命中旧消息,没有水位线的话回执在下一次
  // 轮询就消失,而消息还在 CLI 队列里,复现了回执本要防止的「我的消息不见了」。
  const [queuedInterjections, setQueuedInterjections] = useState<{
    id: number;
    text: string;
    priorUserText: string | null;
    priorItemIds: ReadonlySet<string>;
  }[]>([]);
  const queuedIdRef = useRef(0);
  // 空闲态发送的乐观回显：正文要等 CLI 落盘 + 650ms 轮询才上屏，0.65~2s 的空窗是最典型的
  // 「消息发丢了？」体感。本地先以 user_text 形态接到 transcript 末尾（displayItems），
  // transcript 出现同文本（水位线之后的新证据，匹配规则与排队回执一致）即移除；15s 兜底
  // 防匹配失败时回显永挂。运行中的插话仍走排队回执（那是 CLI 真实的排队语义，不混用）。
  const [pendingEchoes, setPendingEchoes] = useState<{
    id: number;
    text: string;
    at: number;
    priorUserText: string | null;
    priorItemIds: ReadonlySet<string>;
  }[]>([]);
  const echoIdRef = useRef(0);
  // 渲染序列 = 真实 transcript + 乐观回显（以 user_text 形态挂在末尾）。
  // 消解/计数等逻辑一律读真实 items，回显只进显示层。交接注入语同样在显示层滤掉。
  const displayItems = useMemo<ChatItem[]>(() => {
    const visible = items.filter((item) => item.type !== "user_text" || !isHandoffInjectedPrompt(item.text));
    if (pendingEchoes.length === 0) return visible;
    return [
      ...visible,
      ...pendingEchoes.map((echo) => ({
        type: "user_text" as const,
        id: `echo-${echo.id}`,
        timestamp: null,
        text: echo.text,
      })),
    ];
  }, [items, pendingEchoes]);
  // 会话确实还活在用户自己的终端里（后端按 pid 判定）：就地给接管入口，而不是把用户
  // 打发去终端页自己找按钮。retryRef 记住被拒的那个动作，接管成功后原样重放。
  const [needsTakeover, setNeedsTakeover] = useState(false);
  const retryRef = useRef<(() => void | Promise<void>) | null>(null);
  const [terminalAttention, setTerminalAttention] = useState<TerminalAttention | null>(null);
  const [questionCustomText, setQuestionCustomText] = useState("");
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  // 附件超上限被截断时给用户的可见提示（此前 .slice(0, 12) 静默丢弃）。
  const [attachmentNotice, setAttachmentNotice] = useState("");
  const promptRef = useRef(prompt);
  const attachmentsRef = useRef(attachments);
  const viewRef = useRef(view);
  const promptInputRef = useRef<HTMLTextAreaElement>(null);
  // 终端视图一旦显示过就常驻树上（隐藏而非卸载），避免来回切 tab 反复 dispose/new Terminal。
  const terminalEverShownRef = useRef(view === "terminal");
  // 活跃托管 PTY 需要一个隐藏的 xterm 来执行 ANSI 光标/清行序列并得到真实屏幕；这不改变
  // 当前 tab，只把 xterm 从“可见终端 UI”降为后台屏幕状态机。
  const [terminalMonitorNeeded, setTerminalMonitorNeeded] = useState(view === "terminal");
  // 已挂载的 ManagedTerminal 暴露的重启复位钩子：对话页重启 PTY（sendText/changeMode）后
  // 调它把输出偏移归零，否则新进程的输出全被旧偏移判成重复而丢弃。
  const terminalRearmRef = useRef<(() => void) | null>(null);
  const terminalReadyMessages: TerminalReadyMessages = {
    exited: t.chat.terminalStartExited,
    failed: t.chat.terminalStartFailed,
    timeout: t.chat.terminalReadyTimeout,
  };
  // 这次终端菜单是我们为「选模型」发起的吗——识别到选项时据此学习真实清单（见 modelLabels）。
  const modelMenuPendingRef = useRef(false);
  // 静默探测：CLI 菜单只用来「问清单」，识别到就立刻 Esc 收掉，全程不上屏——用户看到的
  // 始终是 GUI 下拉，不是终端里闪一下的 TUI 菜单加一条「正在识别」的等待条。
  const silentProbeRef = useRef(false);
  const learnModelLabelsRef = useRef<(options: TerminalAttentionOption[]) => void>(() => {});
  const finishSilentProbeRef = useRef<() => void>(() => {});
  const revealTerminalAttention = useCallback((attention: TerminalAttention | null) => {
    if (!attention) { setTerminalAttention(null); return; }
    terminalEverShownRef.current = true;
    // CLI 弹出的模型菜单被识别成选项了：把清单学下来，之后直接渲染 GUI 下拉。
    if (modelMenuPendingRef.current && attention.options && attention.options.length > 0) {
      modelMenuPendingRef.current = false;
      learnModelLabelsRef.current(attention.options);
      if (silentProbeRef.current) { finishSilentProbeRef.current(); return; }
    }
    setTerminalAttention((current) => current?.id === attention.id
      && current.text === attention.text
      && JSON.stringify(current.options) === JSON.stringify(attention.options)
      ? current : attention);
  }, []);
  // 惰性初始化:从 localStorage 灌入历史草稿(useRef 的参数每次渲染都会求值,不能直接放)。
  const [initialDrafts] = useState(loadStoredDrafts);
  const draftsRef = useRef(initialDrafts);
  // 挂载恢复当前会话草稿(resetTo 只覆盖切换路径,首开窗口这条要单独走)。
  useEffect(() => {
    const draft = draftsRef.current.get(sessionId);
    if (draft && (draft.prompt || draft.attachments.length > 0)) {
      setPrompt(draft.prompt);
      setAttachments(draft.attachments);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- 仅首帧恢复一次
  }, []);
  // 草稿落盘:400ms 防抖合并连续击键;空草稿删条目(发送成功清空输入框即自动清理)。
  useEffect(() => {
    if (sessionId <= 0) return;
    const timer = window.setTimeout(() => {
      try {
        const raw = JSON.parse(localStorage.getItem(DRAFTS_KEY) || "{}") as Record<string, StoredDraft>;
        const key = String(sessionId);
        if (!prompt.trim() && attachments.length === 0) delete raw[key];
        else raw[key] = { prompt, attachments, at: Date.now() };
        const entries = Object.entries(raw);
        if (entries.length > 20) {
          entries.sort((a, b) => (b[1]?.at ?? 0) - (a[1]?.at ?? 0));
          for (const [stale] of entries.slice(20)) delete raw[stale];
        }
        localStorage.setItem(DRAFTS_KEY, JSON.stringify(raw));
      } catch { /* 存储失败不影响输入 */ }
    }, 400);
    return () => window.clearTimeout(timer);
  }, [sessionId, prompt, attachments]);
  // submitToTerminal 回车前复核用的实时镜像:闭包里读 state 会拿到过期值。
  const terminalAttentionRef = useRef<TerminalAttention | null>(null);
  promptRef.current = prompt;
  attachmentsRef.current = attachments;
  viewRef.current = view;
  terminalAttentionRef.current = terminalAttention;
  if (view === "terminal") terminalEverShownRef.current = true;
  const terminalMounted = terminalEverShownRef.current || terminalMonitorNeeded;
  const activeSessionRef = useRef(sessionId);
  // broker 审批/题面的接收通道（租约注册、push 监听、轮询兜底、领养、倒计时）抽在
  // useApprovalChannel.ts;本组件保留「回应」侧——queuedAnswer 的排队作答（点选的答案
  // 先记下,等屏幕识别确认表单在屏后才落键,绝不向未确认就绪的表单盲写）与 decideApproval。
  const {
    approval, setApproval, approvalCountdown,
    structuredQuestion, setStructuredQuestion,
    approvalAwaitingIds, setApprovalAwaitingIds,
    brokerOwnsReview, setBrokerOwnsReview,
    lastInteractiveQuestionRef,
  } = useApprovalChannel({ sessionId, activeSessionRef, viewRef, setView, setSendError });
  const [queuedAnswer, setQueuedAnswer] = useState<string | null>(null);
  // 作答卡（broker 挂起代答）上的逐题草稿。按 requestId 整体重置：新一轮提问不继承
  // 上一轮的选择。
  const [questionAnswers, setQuestionAnswers] = useState<ReadonlyMap<number, QuestionAnswerDraft>>(new Map());
  useEffect(() => {
    setQuestionAnswers(new Map());
  }, [structuredQuestion?.requestId]);
  // t 的实时镜像:mount-once 监听(拖放附件上限提示)里的闭包 state 是过期值。
  const tRef = useRef(t);
  tRef.current = t;
  const externalRunning = isExternallyHeld(history?.status);
  // 展示层状态口径:DB status 必须经存活校正才能显示「在跑」,存活事实只信后端 connected
  // ——它已把 pid 判活、事件宽限与托管 PTY 活性(pty registry,随每次轮询新鲜)合并成
  // 单一真相。不要再引入前端自计的 PTY 活性标志(曾有 managedPtyActive:探测循环移交
  // ManagedTerminal 后停更,PTY 退出后悬死 true,把 tone 钉死「运行中」——已整根拆除)。
  // history 未加载时按离线处理,避免首帧闪一下运行态。
  const tone = sessionTone(history?.connected ?? false, history?.status, history?.pendingReview, history?.errored);
  // claude 新版任务列表(TaskCreate/TaskUpdate)从时间线累积重建:CC 对这类 harness
  // 内部工具**不触发 PostToolUse hook**(实测:reporter 手喂事件能落库,真调用不触发),
  // hook 快照/增量两条路都收不到,transcript 是唯一来源。编号在 TaskCreate 回执文本
  // `Task #N created successfully` 里;TaskUpdate 的摘要是 Rust 侧精简出的 JSON。
  // 「当前回合」分界:最后一条用户消息的位置。任务/子任务列表跨回合只增不减,长会话
  // 会把历史旧账全堆进面板(用户反馈)——已完成项只显示**这条分界之后**完成的;
  // 未完成的恒显示,无论多老,它们都是还没做完的活计划。
  const lastUserIdx = useMemo(
    () => items.reduce((acc, item, index) => (item.type === "user_text" ? index : acc), -1),
    [items],
  );
  const transcriptTodos = useMemo(() => {
    const byId = new Map<string, { content: string; status: string; touch: number }>();
    const pendingCreates = new Map<string, string>();
    items.forEach((item, index) => {
      if (item.type === "tool_use" && item.name === "TaskCreate") {
        pendingCreates.set(item.id, item.summary);
      } else if (item.type === "tool_use" && item.name === "TaskUpdate") {
        try {
          const input = JSON.parse(item.summary) as { taskId?: unknown; status?: unknown; subject?: unknown };
          const id = input.taskId == null ? "" : String(input.taskId);
          if (!id) return;
          if (input.status === "deleted") { byId.delete(id); return; }
          const row = byId.get(id);
          if (row) {
            if (typeof input.status === "string") row.status = input.status;
            if (typeof input.subject === "string") row.content = input.subject;
            row.touch = index;
          } else if (typeof input.subject === "string") {
            // 错过 Create(如 transcript 截段)但这次带了标题 → 补建自愈。
            byId.set(id, { content: input.subject, status: typeof input.status === "string" ? input.status : "pending", touch: index });
          }
        } catch { /* 摘要被截断等罕见形态:跳过这条,状态晚一拍好过整表消失 */ }
      } else if (item.type === "tool_result" && item.tool_use_id && pendingCreates.has(item.tool_use_id)) {
        const numbered = /Task #(\d+)/.exec(item.text ?? "");
        if (numbered) byId.set(numbered[1], { content: pendingCreates.get(item.tool_use_id)!, status: "pending", touch: index });
        pendingCreates.delete(item.tool_use_id);
      }
    });
    return [...byId.values()]
      .filter((row) => row.status !== "completed" || row.touch > lastUserIdx)
      .map(({ content, status }) => ({ content, status }));
  }, [items, lastUserIdx]);
  // 标题栏任务进度面板与底部 TodoPanel 共用这一份数据。DB 快照(旧版 TodoWrite/kimi
  // 的 TodoList,hook 路线仍通)优先;空则用 transcript 重建的(claude 新版唯一来源)。
  const todos = history?.todos?.length ? history.todos : transcriptTodos;
  // 面板的「子任务」小节:从时间线聚合 Agent 委派(与 Transcript 的关联逻辑同源)。
  // 状态口径:回执统计里还有在跑的算进行中、有失败算失败、否则完成;没回执 = 还没回来 = 在跑。
  const panelSubagents = useMemo(() => {
    // 回执关联(含 TaskOutput 拉取的结局按 task_id 归回原委派)收敛在
    // collectSubagentReceipts——与 Transcript 的折叠徽标同一份规则。
    const { outcomes, settledAt, finishedTs } = collectSubagentReceipts(items);
    const rows: TodoPanelRow[] = [];
    for (const item of items) {
      if (item.type !== "tool_use" || !item.subagent) continue;
      const outcome = outcomes.get(item.id);
      const status = outcome
        ? outcome.running > 0 ? "in_progress" : outcome.failed > 0 ? "failed" : "completed"
        : settledAt.has(item.id) ? "completed" : "in_progress";
      // 与任务列表同一条「不堆积」规则:已结束的委派只显示当前回合内结束的,
      // 历史旧账收走;还在跑的恒显示(跨回合的后台子任务正是最该盯的)。
      if (status !== "in_progress" && (settledAt.get(item.id) ?? -1) <= lastUserIdx) continue;
      const count = item.subagent.count ?? 1;
      const label = item.subagent.description || item.summary || "";
      rows.push({
        content: count > 1 ? `${label} ×${count}` : label,
        status,
        // 行尾执行时长的原料:在跑=委派时刻起算,结束=委派到最终回执。
        startedAt: item.timestamp ?? null,
        finishedAt: status === "in_progress" ? null : finishedTs.get(item.id) ?? null,
      });
    }
    return rows;
  }, [items, lastUserIdx]);
  // 后台会话的 transcript 可能**永远是空的**:claude 以 fork/resume 起的后台 worker 存在
  // 不落盘的老毛病(实测 2.1.220:新会话文件里只有 ai-title / agent-name 两行元数据,正文
  // 既不在自己名下、也不在 fork 源里,只活在进程内存)。对这类会话,对话页给不出任何东西,
  // 而终端旁路拿得到完整画面——那就直接落在终端页,别让用户对着一句「还没有对话记录」发呆。
  // 只在首次加载判一次:此后用户自己切到哪就是哪。
  const autoTerminalRef = useRef(false);
  useEffect(() => {
    if (autoTerminalRef.current || !history) return;
    autoTerminalRef.current = true;
    // 只在**确认没有任何内容**时才切：items 空只说明这一帧没读到，而 hook 落库的最近往来
    // 是独立证据——两者都空才是真的没东西可显示。曾经只看 items，把一个有对话的后台会话
    // 也甩去了终端页，用户以为对话丢了。
    const hasAnything =
      history.items.length > 0 || !!history.lastUserText || !!history.lastAiText;
    if (history.background && !hasAnything) setView("terminal");
  }, [history]);
  // 后台会话(Agent 自己托管的,见 LiveSession.background):画面不在托管 PTY 表里,要先接上
  // 那条旁路 socket,终端页才有东西可看。background 每次翻真都接(后端 attach 幂等,活着
  // 就直接返回)——不能用一次性门闩:resume 接管会 detach 摘掉旁路,若会话之后又回到后台
  // 形态,门闩会让画面永久断供。接失败(worker 已退出/花名册没它了)就让终端页维持空画面
  // ——它本来就不是我们能拉起的进程,没有可重试的动作。
  useEffect(() => {
    if (!history?.background) return;
    attachBackgroundSession(sessionId).catch(() => {});
  }, [history?.background, sessionId]);
  // 窗口标题随会话与状态更新:任务栏/Alt-Tab 上也能看出 agent 在跑还是在等。
  // 后端 apply_language 已不再写 chat 窗标题(双写互相覆盖);窗口无装饰,标题只出现在
  // 任务栏,宜短。▶=在跑,●=等待/待处理,离线不加记号。
  useEffect(() => {
    const name = history?.title || t.chat.title;
    const marker = tone === "running" ? "▶ " : tone === "pending" || tone === "waiting" ? "● " : "";
    // 非 Tauri 环境(测试/浏览器)下 getCurrentWindow 会抛错,吞掉即可(同 Sticker 的置顶写法)。
    try { void getCurrentWindow().setTitle(`${marker}${name} · Meowo`).catch(() => {}); } catch { /* noop */ }
  }, [tone, history?.title, t]);
  // 「结束会话」:结束正在跑的 Agent 此前只有终端页操作条里的入口,不开终端页的 GUI 用户
  // 无从结束,会话就一直在后台占着进程。可见性由后端 pty_managed 门控——只有本 GUI 托管
  // 的 PTY 才能这样结束;外部终端里跑的会话不亮(要先走接管)。确认+停止走与终端页共用的
  // confirmStopSession(api.ts),两个入口的协议与文案不再各自为政。
  const [endingSession, setEndingSession] = useState(false);
  const endSession = async () => {
    try {
      // 成功后不用做别的:kill → waiter 收尾 → 下一轮 650ms 轮询里 connected/pty_managed
      // 双双翻 false,徽标退「未连接」、本按钮随之卸载。失败必须可见,否则用户以为已停。
      await confirmStopSession(
        sessionId,
        { title: t.chat.endSession, message: t.chat.endSessionConfirm },
        () => setEndingSession(true),
      );
    } catch (e) {
      setSendError(formatBackendError(e, t.locale));
    } finally {
      setEndingSession(false);
    }
  };
  // 「归档」:此前只有贴纸看板的卡片菜单能归档。用户读完一个会话往往就停在对话窗里,
  // 要收纳它却得先切回看板、在一堆卡片里找回同一条——把入口补在这里,读完即可收。
  // 语义与看板逐字一致(同一条 set_archived,同一个 archived 列):只改看板可见性,
  // 不动进程、不影响本窗继续对话,故不做二次确认(误点再点一次即还原)。
  const [archiving, setArchiving] = useState(false);
  // 侧栏当前显示顺序的镜像（ChatSidebar 写入）：归档当前会话后跳「用户看到的下一条」,
  // 与侧栏入口的归档同一取序——此前这里按后端默认排序取第一条,无视目录筛选/排序/置顶,
  // 会跳到侧栏里不挨着的会话。侧栏收起时镜像为空,退回后端现查。
  const sidebarOrderRef = useRef<number[]>([]);
  // 归档撤销条:归档无确认、取回入口(筛选菜单→已归档)较隐蔽,给 8s 的一键撤销。
  const [archiveUndo, setArchiveUndo] = useState<number | null>(null);
  useEffect(() => {
    if (archiveUndo == null) return;
    const timer = window.setTimeout(() => setArchiveUndo(null), 8_000);
    return () => window.clearTimeout(timer);
  }, [archiveUndo]);
  const undoArchive = async (id: number) => {
    setArchiveUndo(null);
    try {
      await setArchivedCmd(id, false);
      resetTo(id);
    } catch (e) {
      setSendError(formatBackendError(e, t.locale));
    }
  };
  // 标题菜单的置顶/重命名（Kimi 式）。置顶走 useStarred（看板/侧栏同款：
  // 同一份写入口 + 同窗口自定义事件/跨窗口 storage 双通道同步）。
  const [renaming, setRenaming] = useState(false);
  const { starred: starredSet, toggleStar: toggleStarById } = useStarred();
  const toggleStar = () => {
    const id = history?.ccSessionId;
    if (id) toggleStarById(id);
  };
  const renameSession = async (title: string) => {
    if (!history) return;
    // 与侧栏/看板同一条命令、同一套参数（cc_session_id 为键）。成功后标题由 650ms
    // 轮询自然刷新，rename_session 也会广播 board-changed 让侧栏跟上。
    await renameSessionCmd(history.cwd, history.ccSessionId, title, history.provider);
  };
  const toggleArchived = async () => {
    if (!history || archiving) return;
    const next = !history.archived;
    setArchiving(true);
    // 乐观翻转:IPC 往返 + 下一轮轮询才回真值,期间按钮维持旧文案会让人以为没点上。
    // 失败回滚并报错——静默失败等于骗用户「已归档」。
    setHistory((current) => (current ? { ...current, archived: next } : current));
    try {
      await setArchivedCmd(sessionId, next);
      // 归档 = 收纳:侧栏里它当场消失,右边却还停在它的对话上,等于「收起来了却还摊在桌上」。
      // 顺手切到列表里的下一条会话。取不到(它是唯一一条,或查询失败)就留在原地——
      // 空窗比自作主张关窗好,用户仍可继续读这段对话。
      if (next) {
        setArchiveUndo(sessionId);
        // 优先按侧栏当前显示顺序取下一条(当前条之后的第一条,没有则往前找),与侧栏
        // 入口的归档跳转一致;侧栏收起(镜像为空)时才退回后端现查。
        const order = sidebarOrderRef.current;
        const at = order.indexOf(sessionId);
        const following = at >= 0
          ? order[at + 1] ?? order[at - 1] ?? null
          : order.find((id) => id !== sessionId) ?? null;
        if (following != null) {
          resetTo(following);
        } else {
          const page = await getLiveSessionsPage("all", null, null, 5).catch(() => null);
          const fallback = page?.items.find((item) => item.session.id !== sessionId);
          if (fallback) resetTo(fallback.session.id);
        }
      }
    } catch (e) {
      setHistory((current) => (current ? { ...current, archived: !next } : current));
      setSendError(formatBackendError(e, t.locale));
    } finally {
      setArchiving(false);
    }
  };
  const [resolvingApproval, setResolvingApproval] = useState(false);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(() => localStorage.getItem(SIDEBAR_COLLAPSED_KEY) === "1");
  // 窄窗（<880px）自动收起停靠侧栏：两列塞不下。此时开关切换的是**浮窗侧栏**（overlay，
  // 盖在主列上、不挤压布局），且不写收起偏好——窗口拉宽后回到用户原本的停靠选择。
  // jsdom 没有 matchMedia，恒按宽窗走（测试语义不变）。
  const [narrow, setNarrow] = useState(() => typeof window.matchMedia === "function" && window.matchMedia(SIDEBAR_NARROW_QUERY).matches);
  const [overlaySidebar, setOverlaySidebar] = useState(false);
  useEffect(() => {
    if (typeof window.matchMedia !== "function") return;
    const mq = window.matchMedia(SIDEBAR_NARROW_QUERY);
    const onChange = () => {
      setNarrow(mq.matches);
      // 拉宽后浮窗失去意义，收掉并回到停靠形态。
      if (!mq.matches) setOverlaySidebar(false);
    };
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, []);
  const sidebarVisible = narrow ? overlaySidebar : !sidebarCollapsed;
  const toggleSidebar = () => {
    if (narrow) {
      setOverlaySidebar((prev) => !prev);
      return;
    }
    setSidebarCollapsed((prev) => {
      const next = !prev;
      localStorage.setItem(SIDEBAR_COLLAPSED_KEY, next ? "1" : "0");
      return next;
    });
  };
  // 窗口级快捷键（均带 Ctrl/Cmd，不与输入冲突）：B=收展侧栏，1/2=对话/终端视图，
  // N=新建会话。Ctrl+F 聚焦侧栏搜索在 ChatSidebar 内注册；Esc 语义各归其主（审批/菜单/弹层）。
  // 此前整个对话窗除 Esc 外零快捷键，收侧栏、切视图、新建全要鼠标。
  const toggleSidebarRef = useRef(toggleSidebar);
  toggleSidebarRef.current = toggleSidebar;
  // 侧栏回调的稳定引用:ChatSidebar 已 memo,内联箭头函数每次击键都换新引用会让 memo
  // 失效——composer 每个字符都重建整张会话列表(上百条 item)就是这么来的。
  const collapseSidebar = useCallback(() => toggleSidebarRef.current(), []);
  const closeOverlaySidebar = useCallback(() => setOverlaySidebar(false), []);
  // 远程:侧栏点「新建会话」派发页内导航事件(RemoteApp 据此叠加渲染 sheet)。窄屏抽屉
  // 层级(z 61)在 sheet 之上,不收就盖住 sheet——收到事件即收抽屉。桌面此事件永不派发。
  useEffect(() => {
    const close = () => setOverlaySidebar(false);
    window.addEventListener(NEW_SESSION_EVENT, close);
    return () => window.removeEventListener(NEW_SESSION_EVENT, close);
  }, []);
  // 远程首开没有会话上下文:窄屏自动拉开抽屉让用户先选,别对着「去侧栏选会话」空态干瞪眼。
  // 一次性:横竖屏切换会翻转 narrow 重跑 effect,不加闩的话手动收起的抽屉会被强行
  // 重开(自审 L13)。
  const autoDrawerDoneRef = useRef(false);
  useEffect(() => {
    if (autoDrawerDoneRef.current) return;
    if (remoteUi() && sessionId === 0 && narrow) {
      autoDrawerDoneRef.current = true;
      setOverlaySidebar(true);
    }
  }, [sessionId, narrow]);
  // 远程记住最后看的会话:刷新/重开页面直接回到它(桌面开窗恒带 query,不用这条)。
  useEffect(() => {
    if (remoteUi() && sessionId > 0) {
      try {
        localStorage.setItem(REMOTE_LAST_SESSION_KEY, String(sessionId));
      } catch {
        /* 隐私模式禁写:退化为每次都要手选 */
      }
    }
  }, [sessionId]);
  // Ctrl/Cmd+K 快速切换器（QuickSwitcher）。
  const [switcherOpen, setSwitcherOpen] = useState(false);
  // ? 打开快捷键速查表（输入框内的 ? 是正文，不拦）。
  const [shortcutsOpen, setShortcutsOpen] = useState(false);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "?" && !e.ctrlKey && !e.metaKey && !e.altKey) {
        const el = document.activeElement;
        if (el instanceof HTMLElement && (el.tagName === "TEXTAREA" || el.tagName === "INPUT" || el.isContentEditable)) return;
        e.preventDefault();
        setShortcutsOpen(true);
        return;
      }
      if (!(e.ctrlKey || e.metaKey) || e.altKey || e.shiftKey) return;
      if (e.code === "KeyK") {
        e.preventDefault();
        // 同键开合（VS Code/Slack 惯例）：已开着时再按一次是「收起」，不是无操作。
        setSwitcherOpen((open) => !open);
      } else if (e.code === "KeyB") {
        e.preventDefault();
        toggleSidebarRef.current();
      } else if (e.code === "Digit1") {
        e.preventDefault();
        rememberViewPref("chat");
        setView("chat");
      } else if (e.code === "Digit2") {
        e.preventDefault();
        rememberViewPref("terminal");
        setView("terminal");
      } else if (e.code === "KeyN") {
        e.preventDefault();
        void openNewSessionWindow().catch(() => {});
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
  const [modelMenu, setModelMenu] = useState(false);
  /// 刚发出一条会弹菜单的命令：在这个时间点之前让屏幕识别去认光标菜单。
  /// 不常开是刻意的——菜单形态（导航提示 + ❯）虽然特征明确，但常开等于把 agent 平时
  /// 画的任何选择列表都变成弹卡片，噪声大于价值。
  const [menuWatchUntil, setMenuWatchUntil] = useState(0);
  const [modeMenu, setModeMenu] = useState<string | null>(null);
  // 模型/模式菜单建在 menu.tsx 的 useMenuPopup 上（click-away、Esc+焦点归还、方向键导航）；
  // 定位交给 CSS（.chat-model-menu 绝对定位上弹），互斥写在受控 setOpen 里：开一个即关另一个。
  const modelMenuUi = useMenuPopup({
    cssPositioned: true,
    open: modelMenu,
    setOpen: (v) => { setModelMenu(v); if (v) setModeMenu(null); },
  });
  const modeMenuUi = useMenuPopup({
    cssPositioned: true,
    open: modeMenu !== null,
    setOpen: (v) => { if (!v) setModeMenu(null); },
  });
  // 对话页能力由安装实况组装（基础命令 ∪ 用户/项目命令 ∪ 当前会话 runtime skill 清单），
  // 按 provider+cwd 查询——换会话、换项目都重取，装了新命令下次打开就见。
  // 未知 provider / 查询未回时为空：不补全、不给菜单，宁缺毋滥。
  const [chatUi, setChatUi] = useState<ChatUi | null>(null);
  const [capabilityOffset, setCapabilityOffset] = useState(0);
  const provider = history?.provider;
  const cwd = history?.cwd ?? null;
  // 「查看改动」入口：仅当 cwd 落在 git 仓库内时后端才回 isRepo=true，按钮据此显隐。
  // 换会话（cwd 变化）重取；取不到/非仓库时保持 null，入口不出现。
  const [gitSummary, setGitSummary] = useState<GitDiffSummaryDto | null>(null);
  const [diffOpen, setDiffOpen] = useState(false);
  // 折叠/最大化都只在面板已挂载（diffOpen）后才有意义；折叠不卸载——面板内的
  // 打开文件、树展开与 diff/目录缓存全部保留，再展开时原样回来。换会话照旧整体重置。
  const [diffCollapsed, setDiffCollapsed] = useState(false);
  const [diffMaximized, setDiffMaximized] = useState(false);
  // 面板宽度随会话持久化（localStorage）；折叠只藏不改宽，再展开原宽回来。
  const [diffWidth, setDiffWidth] = useState(() => {
    const stored = Number(localStorage.getItem(DIFF_WIDTH_KEY));
    return Number.isFinite(stored) && stored >= DIFF_PANEL_MIN_WIDTH ? stored : DIFF_PANEL_DEFAULT_WIDTH;
  });
  // 拖拽分栏柄：pointer capture 挂在柄上，移出窗口也不丢事件；宽度上限按
  // .chat-body 实测宽给对话区留 ≥360px（CSS 的 max-width:40vw 仍是窗口缩小的兜底）。
  const startDiffResize = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;
    const handle = event.currentTarget;
    const body = handle.parentElement;
    if (!body) return;
    event.preventDefault();
    handle.setPointerCapture(event.pointerId);
    const startX = event.clientX;
    const panel = body.querySelector<HTMLElement>(".chat-diff-panel");
    // 以实测宽为起点：state 宽可能被 40vw 上限压小，从 state 起算会跳变。
    const startWidth = panel ? panel.getBoundingClientRect().width : diffWidth;
    const maxWidth = Math.max(DIFF_PANEL_MIN_WIDTH, body.getBoundingClientRect().width - 360);
    let lastWidth = startWidth;
    body.classList.add("is-diff-resizing");
    // pointermove 每次都 setState 会以指针事件频率重渲整个面板子树,开着大文件时拖不动;
    // 按帧合流,一帧至多提交一次宽度。
    let raf = 0;
    const onMove = (move: PointerEvent) => {
      lastWidth = Math.min(Math.max(startWidth + (startX - move.clientX), DIFF_PANEL_MIN_WIDTH), maxWidth);
      if (!raf) {
        raf = requestAnimationFrame(() => {
          raf = 0;
          setDiffWidth(lastWidth);
        });
      }
    };
    const onUp = () => {
      if (raf) cancelAnimationFrame(raf);
      raf = 0;
      setDiffWidth(lastWidth);
      body.classList.remove("is-diff-resizing");
      handle.removeEventListener("pointermove", onMove);
      handle.removeEventListener("pointerup", onUp);
      handle.removeEventListener("pointercancel", onUp);
      localStorage.setItem(DIFF_WIDTH_KEY, String(Math.round(lastWidth)));
    };
    handle.addEventListener("pointermove", onMove);
    handle.addEventListener("pointerup", onUp);
    handle.addEventListener("pointercancel", onUp);
  };
  // 稳定引用给 memo 的 GitDiffView:内联箭头函数每次击键换新引用,面板即使折叠也要
  // 对几千行文件 DOM 做全量 reconcile,还连带 Esc 层监听每渲染拆装一次。
  const collapseDiffPanel = useCallback(() => {
    setDiffCollapsed(true);
    setDiffMaximized(false);
  }, []);
  const toggleDiffMaximize = useCallback(() => {
    setDiffMaximized((max) => !max);
    setDiffCollapsed(false);
  }, []);
  // 面板的激活仓(多仓会话:主仓 + 各附加目录之一)。null = 主仓;换会话重置。
  const [diffDir, setDiffDir] = useState<string | null>(null);
  const diffCwd = diffDir ?? cwd;
  // 会话的全部目录。extraDirs 数组每次 650ms 轮询都是新引用,用 join 键稳住
  // useMemo——否则 dirs 每帧换新引用,memo 的 GitDiffView 白 memo。
  const extraDirsKey = (history?.extraDirs ?? []).join("\0");
  const diffDirs = useMemo(
    () => (cwd ? [cwd, ...extraDirsKey.split("\0").filter(Boolean)] : []),
    [cwd, extraDirsKey]
  );
  useEffect(() => {
    setDiffOpen(false);
    setDiffCollapsed(false);
    setDiffMaximized(false);
    setDiffDir(null);
  }, [cwd]);
  useEffect(() => {
    // 远程无 GitDiff 入口且命令不在 /rpc 白名单:短路,别每次换会话白打一发 404。
    if (!diffCwd || remoteUi()) { setGitSummary(null); return; }
    let cancelled = false;
    getGitDiffSummary(diffCwd)
      .then((result) => { if (!cancelled) setGitSummary(result); })
      .catch(() => { if (!cancelled) setGitSummary(null); });
    return () => { cancelled = true; };
  }, [diffCwd]);
  // 面板每次转为可见都重拉摘要:agent 随时在提交/改文件,进窗时拉的那份清单会过期
  // ——点开「更改」看到的是已提交文件的空 diff(实拍反馈)。git status 毫秒级,不设节流。
  useEffect(() => {
    if (!diffCwd || !diffOpen || diffCollapsed) return;
    let cancelled = false;
    getGitDiffSummary(diffCwd)
      .then((result) => { if (!cancelled) setGitSummary(result); })
      .catch(() => {});
    return () => { cancelled = true; };
  }, [diffCwd, diffOpen, diffCollapsed]);
  // 恢复/接管时的权限改选："" = 沿用会话存的选择；选了具体档就随本次恢复下发，
  // 后端按选项维度合并写回，成为会话新的持久形态——覆盖「当年不是以跳过权限新建、
  // 恢复时想切过去」的场景（存量会话同样适用）。没声明权限类选项的 agent 不显示。
  const [resumePermission, setResumePermission] = useState("");
  // 换会话即归零：改选是对「这一条会话的下一次恢复」说的，带给别的会话就是暗改人家权限。
  useEffect(() => setResumePermission(""), [sessionId]);
  // 会话存的启动选项（新建/接管改选时后端落库）：有它就把「沿用原设置」亮成具体档位，
  // 一句黑盒不如直接告诉用户沿用的是什么（实拍反馈）。换会话重取。
  const [storedSelections, setStoredSelections] = useState<Record<string, string>>({});
  useEffect(() => {
    setStoredSelections({});
    // 快速 A→B 切换且 A 响应慢时,迟到的 A 结果会盖掉 B 的——存的权限档随后还会
    // 下发给 B 的 resume。同文件其它异步 effect 都有 cancelled 守卫,这条补齐。
    let cancelled = false;
    sessionLaunchSelections(sessionId).then((map) => {
      if (!cancelled) setStoredSelections(map);
    });
    return () => {
      cancelled = true;
    };
  }, [sessionId]);
  const [agentsForOptions, setAgentsForOptions] = useState<AgentDescriptor[] | null>(null);
  useEffect(() => {
    listAgents().then(setAgentsForOptions).catch(() => {});
  }, []);
  const permissionOption = (provider
    ? agentsForOptions
        ?.find((agent) => agent.id === provider)
        ?.launch_options?.find((option) => option.id === "permission" || option.id === "approval")
    : null) ?? null;
  const resumeOptions = resumePermission && permissionOption
    ? { [permissionOption.id]: resumePermission }
    : undefined;
  // 会话存的权限档（在插件声明的 choices 里能对上号才算数——存了未知值不能拿去显示）。
  const storedPermission = permissionOption
    ? permissionOption.choices.find((choice) => choice.id === storedSelections[permissionOption.id]) ?? null
    : null;
  // 恢复权限档的选项清单：「沿用原设置」占位 + 插件声明的档位。resumePermissionPicker
  // 与休眠态权限胶囊共用同一份构造——此前两处各写一遍,改一处漏一处。
  // 有存值时不放「沿用原设置」占位项：直接预选存的那一档（沿用=选中它本身，不写覆盖），
  // 用户一眼看到沿用的是什么；没存过原值只有 CLI 自己知道，才退回占位文案。
  const permissionChoiceOptions = useMemo(() => {
    if (!permissionOption) return [];
    return [
      ...(storedPermission ? [] : [{ value: "", label: t.chat.resumeKeepOptions }]),
      ...permissionOption.choices.map((choice) => ({
        value: choice.id,
        label: t.newSession.launchChoice[`${permissionOption.id}.${choice.id}`] ?? choice.label,
      })),
    ];
  }, [permissionOption, storedPermission, t]);
  // 容器不挂 data-tip：提示气泡会盖在展开的下拉菜单上挡住选项（用户实拍两回）。
  // 「选了会记住」的说明性文案价值低于被它糊住的菜单，整个去掉。
  const resumePermissionPicker = permissionOption ? (
    <div className="chat-resume-permission">
      <Dropdown
        align="left"
        value={storedPermission ? (resumePermission || storedPermission.id) : resumePermission}
        options={permissionChoiceOptions}
        onChange={(value) => setResumePermission(
          storedPermission && String(value) === storedPermission.id ? "" : String(value),
        )}
      />
    </div>
  ) : null;
  const transcriptCapabilitiesReady = capabilityOffset > 0;
  const runtimeCapabilityProbe = chatUi?.runtime_commands_pending
    ? capabilityOffset
    : transcriptCapabilitiesReady ? 1 : 0;
  const startupAttentionMarkerKey = (chatUi?.startup_attention_markers ?? []).join("\0");
  const terminalInteractivePrompt = history?.pendingReview === "question" || history?.pendingReview === "plan";
  // 识别文法:provider 门控(claude 专有整句规则只对 claude 生效)+ 插件声明的选择器
  // 锚点。chatUi 未就绪时锚点为空——ManagedTerminal 的 grammarKey 复扫会在其到达后补投。
  const attentionGrammar = useMemo<AttentionGrammar>(() => ({
    provider: history?.provider,
    selectorAnchors: chatUi?.selector_anchors ?? [],
    attentionPatterns: chatUi?.attention_patterns ?? [],
  }), [history?.provider, chatUi]);
  // runtime 清单未就绪时，探测键随每次 650ms 轮询的 offset 变化而变化——不能每变一次就
  // 打一发 agent_chat_ui（后端要重扫命令目录、探 transcript）。同一会话内限频到 2s 一查；
  // 换会话/换 provider/换 cwd 仍立即查。
  const chatUiProbeRef = useRef({ key: "", at: 0 });
  useEffect(() => {
    if (!provider) return;
    let stale = false;
    let timer = 0;
    const key = `${provider}\0${cwd ?? ""}\0${sessionId}`;
    const fetchUi = () => {
      chatUiProbeRef.current = { key, at: Date.now() };
      agentChatUi(provider, cwd, sessionId).then((ui) => { if (!stale) setChatUi(ui); }).catch(() => {});
    };
    const last = chatUiProbeRef.current;
    const wait = last.key === key ? 2_000 - (Date.now() - last.at) : 0;
    if (wait > 0) timer = window.setTimeout(fetchUi, wait);
    else fetchUi();
    return () => { stale = true; window.clearTimeout(timer); };
  }, [provider, cwd, sessionId, runtimeCapabilityProbe]);

  // 启动阻塞属于会话状态，不属于终端视图。即使用户从未打开终端 tab，也先用轻量增量
  // snapshot 探测 PTY；发现活跃 PTY 后在屏幕外挂载 xterm，还原 ANSI 当前屏并持续识别选择器。
  useEffect(() => {
    if (sessionId <= 0) return;
    let cancelled = false;
    let timer = 0;
    let since = 0;
    let outputTail = "";
    let reportedId: string | null = null;
    const decoder = new TextDecoder();
    const markers = chatUi?.startup_attention_markers ?? [];
    const poll = async () => {
      try {
        const snapshot = await managedTerminalSnapshot(sessionId, since);
        if (cancelled) return;
        if (snapshot.endOffset < since) {
          since = 0;
          outputTail = "";
          reportedId = null;
        }
        outputTail = appendTerminalText(outputTail, snapshot.data, decoder);
        since = snapshot.endOffset;
        if (snapshot.data) {
          const attention = detectTerminalAttention(outputTail, markers, terminalInteractivePrompt, false, attentionGrammar);
          if (attention) {
            if (attention.id !== reportedId) revealTerminalAttention(attention);
            reportedId = attention.id;
          } else {
            reportedId = null;
          }
        }
        if (snapshot.active) {
          // 后续输出和 ANSI 屏幕识别交给 ManagedTerminal 的事件监听，不再重复轮询 IPC。
          setTerminalMonitorNeeded(true);
          return;
        }
        timer = window.setTimeout(() => void poll(), 1_200);
      } catch {
        if (!cancelled) timer = window.setTimeout(() => void poll(), 1_200);
      }
    };
    void poll();
    return () => { cancelled = true; window.clearTimeout(timer); };
  }, [sessionId, startupAttentionMarkerKey, terminalInteractivePrompt, revealTerminalAttention, attentionGrammar]);
  // ↑ 取回历史消息(CLI 惯例):空输入框按 ↑ 进入浏览,↑/↓ 前后翻,翻回最新即回到空框;
  // 一旦手动编辑退出浏览态(当前文本保留成草稿)。只浏览自己发过的 user_text。
  const historyNavRef = useRef<{ index: number } | null>(null);
  const userHistory = useMemo(
    () => items
      .filter((item) => item.type === "user_text" && !isHandoffInjectedPrompt(item.text))
      .map((item) => (item as { text: string }).text),
    [items],
  );
  useEffect(() => { historyNavRef.current = null; }, [sessionId]);
  // `@` 文件补全与 `/` 斜杠补全:状态、取数与插入在 composerCompletion.ts;键盘交互
  // 仍在下方 composer 的 onKeyDown(与发送/历史浏览交织,拆不动也不该拆)。
  const { atQuery, setAtQuery, atFiles, setAtFiles, atActive, setAtIndex, pickAtFile } =
    useAtFileCompletion({ cwd, prompt, setPrompt, promptInputRef });
  const { setSlashIndex, setSlashDismissed, slashMenuRef, slashMatches, slashActive } =
    useSlashCompletion(prompt, chatUi?.slash_commands);
  // transcript 之外的兜底时间线：hook 落库的最近一问一答（UserPromptSubmit / Stop）。
  // transcript 尚未落盘/尚未定位到、或该 agent 不提供结构化 transcript 时用它渲染，
  // 让「会话已在工作」有真实内容可看。transcript 一旦就位（items 非空）即被完整记录取代。
  // useMemo：它作为 items 传给 Transcript（memo 按引用比较），每次渲染新建数组会让
  // 650ms 轮询期间整棵消息树白白重建一遍。
  const provisional = useMemo<ChatItem[]>(() => {
    const out: ChatItem[] = [];
    if (history?.lastUserText) out.push({ type: "user_text", id: "hook:last-user", timestamp: null, text: history.lastUserText });
    if (history?.lastAiText) out.push({ type: "assistant_text", id: "hook:last-ai", timestamp: null, text: history.lastAiText });
    return out;
  }, [history?.lastUserText, history?.lastAiText]);
  // 模型清单三来源(CLI 自举/学到的标签/插件别名)的合流在 useModelPresets.ts;
  // 静默探测与菜单编排(与 sendText、终端 attention 交织)仍在本组件。
  const { modelPresets, modelMenuCommand, modelDropdownReady, learnModelLabels } =
    useModelPresets(history?.provider ?? null, chatUi);
  // attention 处理(组件顶部的 handleAttention)在 render 前定义,经 ref 转接读它。
  learnModelLabelsRef.current = learnModelLabels;
  // 静默探测进行中：按钮显示为忙，不弹任何终端界面。
  const [modelProbing, setModelProbing] = useState(false);
  const modelProbeTimerRef = useRef<number | undefined>(undefined);
  // 换会话/卸载必须清掉探测兜底 timer：它的闭包捕获旧 sessionId，12 秒后照样触发
  // endSilentProbe，往早已离开的会话终端写 Esc（幽灵按键；测试里表现为跨用例的写入泄漏）。
  useEffect(() => () => window.clearTimeout(modelProbeTimerRef.current), [sessionId]);
  // 这个会话的模型菜单读不到（该 CLI 的菜单形态未取证/输入通道不通）：不再反复空转，
  // 按钮改为「去终端页切」的直达入口。换会话时复位——换个 agent 可能就能读到。
  const [modelProbeFailed, setModelProbeFailed] = useState(false);
  useEffect(() => setModelProbeFailed(false), [sessionId, history?.provider]);
  // ── 跨 provider 切换（切换引擎）──
  // 展开的目标 agent（模型下拉里二级子项的开合）与惰性取到的目标模型预设。
  const [switchTarget, setSwitchTarget] = useState<AgentId | null>(null);
  const [switchPresets, setSwitchPresets] = useState<Record<string, ModelPreset[]>>({});
  useEffect(() => { if (!modelMenu) setSwitchTarget(null); }, [modelMenu]);
  // 待注入的交接：switch 命令返回后记下文件路径与目标 provider，新会话认领成真 id、
  // 终端就绪后由注入 effect 消费。ref 而非 state：注入是流程记账，不驱动渲染。
  const handoffRef = useRef<{ path: string; expectProvider: AgentId; at: number } | null>(null);
  // 当前会话 provider 的 descriptor：切换入口的可见性由后端下发的能力位决定
  //（supports_chat_export），前端不判 id——守卫测试盯着。
  const currentAgentDescriptor = provider
    ? agentsForOptions?.find((agent) => agent.id === provider) ?? null
    : null;
  // switch_session_provider 会 spawn 宿主进程,是 /rpc 拒绝项——远程直接不给切换引擎分组,
  // 免得用户走完不可逆确认后收一句 404。
  const switchTargets = !remoteUi() && currentAgentDescriptor?.supports_chat_export
    ? (agentsForOptions ?? []).filter((agent) => agent.installed && agent.id !== provider)
    : [];
  // ── 接续会话的前序段历史（跨 provider 切换的连续性）──
  // 切换引擎后新会话的时间线不从空白开始：前序段（已结束、内容静态）的完整消息内联在
  // 上方，段间以「切换至 X」分隔条衔接（见 timelineItems）。内容不变，进会话只取一次。
  const [prefixSegments, setPrefixSegments] = useState<{ provider: string; items: ChatItem[] }[]>([]);
  useEffect(() => {
    if (sessionId <= 0 || history?.predecessorId == null || prefixSegments.length > 0) return;
    let cancelled = false;
    (async () => {
      try {
        const chain = await getSessionLineage(sessionId);
        const predecessors = chain.filter((entry) => entry.id !== sessionId);
        const segments: { provider: string; items: ChatItem[] }[] = [];
        for (const entry of predecessors) {
          const full = await getChatHistory(entry.id, 0, true).catch(() => null);
          if (cancelled) return;
          segments.push({ provider: entry.provider, items: full && full.supported !== false ? full.items : [] });
        }
        if (!cancelled) setPrefixSegments(segments);
      } catch {
        // 链读取失败：prefixSegments 保持空，时间线退回 handoffContinued 注脚。
      }
    })();
    return () => { cancelled = true; };
  }, [sessionId, history?.predecessorId, prefixSegments.length]);
  // 渲染序列 = 前序段（含段间与当前段的分隔条）+ 当前会话消息。分隔条的展示名在渲染期
  // 解析（agentsForOptions 可能晚于拉取到达），拿不到就退回 provider id。
  const timelineItems = useMemo<ChatItem[]>(() => {
    const filled = prefixSegments.filter((segment) => segment.items.length > 0);
    if (filled.length === 0) return displayItems;
    const nameOf = (id: string) => agentsForOptions?.find((agent) => agent.id === id)?.display_name ?? id;
    const out: ChatItem[] = [];
    filled.forEach((segment, index) => {
      if (index > 0) out.push({ type: "meta", id: `handoff-${index}`, timestamp: null, kind: `handoff:${nameOf(segment.provider)}` });
      out.push(...segment.items);
    });
    out.push({ type: "meta", id: "handoff-current", timestamp: null, kind: `handoff:${nameOf(provider ?? "")}` });
    return [...out, ...displayItems];
  }, [prefixSegments, displayItems, agentsForOptions, provider]);
  // 前序内容没取到（或本来就没有）时的来历注脚；取到后由分隔条接力表达同一事实。
  const showHandoffNote = history?.predecessorId != null && prefixSegments.every((segment) => segment.items.length === 0);
  // 识别窗口是个时间点，不是布尔——过期后要真的停下来，故用一个到点自灭的计时器驱动重渲染。
  const [menuWatching, setMenuWatching] = useState(false);
  useEffect(() => {
    const remaining = menuWatchUntil - Date.now();
    if (remaining <= 0) { setMenuWatching(false); return; }
    setMenuWatching(true);
    const timer = window.setTimeout(() => setMenuWatching(false), remaining);
    return () => window.clearTimeout(timer);
  }, [menuWatchUntil]);
  const modeControls = chatUi?.mode_controls ?? [];
  const offsetRef = useRef(0);
  const busyRef = useRef(false);
  const scrollRef = useRef<HTMLDivElement>(null);
  const followRef = useRef(true);
  const positionedRef = useRef(false);

  // compose 进入文档流后不再浮在滚动区上：textarea 变高、审批卡出现/消失都会改变
  // .chat-scroll 的视口高度。用户停在底部时 scrollTop 不变、视口变矮，尾条消息会
  // 视觉上移——这里在滚动容器尺寸变化时把已吸底的用户重新钉回底部（反向变矮时
  // 浏览器会自动钳住 scrollTop，无需处理）；用户已上翻则不打扰。
  // 新消息本身的吸底仍由下方 [items, view] 的 layout effect 负责，这里不管内容增长。
  useEffect(() => {
    const el = scrollRef.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => {
      // hidden（终端视图）时 display:none，scrollHeight 恒 0，不能拿它改写滚动位置。
      if (el.hidden || !followRef.current) return;
      el.scrollTop = el.scrollHeight;
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  const resetTo = useCallback((id: number) => {
    if (!Number.isSafeInteger(id) || id === 0) return;
    if (id === activeSessionRef.current) return;
    draftsRef.current.set(activeSessionRef.current, {
      prompt: promptRef.current,
      attachments: attachmentsRef.current,
    });
    const draft = draftsRef.current.get(id);
    // 常驻只为省下同一会话内来回切 tab 的 dispose/new Terminal。换会话时 ManagedTerminal
    // 本来就随 key 重挂，继续记着「显示过」只会让每次切换都在后台白挂一个终端
    // （xterm 创建 + 两个 listen + 一次全量 backlog 拉取）。终端模式下 view 仍是 terminal，
    // 常驻照旧生效，不影响「切会话保持终端模式」。
    terminalEverShownRef.current = viewRef.current === "terminal";
    // 「每个会话判一次」的一次性闸门，换会话必须重开：不重置的话落页判断只对本窗口
    // 加载的第一个会话生效过。（后台旁路的 attach 已无门闩——background 翻真即接,
    // 幂等由后端保证,无需在这里重置。）
    autoTerminalRef.current = false;
    offsetRef.current = 0;
    activeSessionRef.current = id;
    // 人已经切过来了，授权徽标的使命完成——接下来的审批卡由本会话轮询接管。
    setApprovalAwaitingIds((prev) => {
      if (!prev.has(id)) return prev;
      const next = new Set(prev);
      next.delete(id);
      return next;
    });
    setItems([]);
    setPrefixSegments([]);
    setHistory(null);
    setQuestionCustomText("");
    setLoading(true);
    setFailed(false);
    setPrompt(draft?.prompt ?? "");
    setSendError("");
    // 发送态与其派生卡片按会话归零:发送类长异步(ensureWritableTerminal→waitForTerminalReady
    // 最长 45s)在旧会话在途时,sending 卡在 true 会让切过去的新会话圆钮永转圈禁用;
    // needsTakeover/softPromptNotice 不清则旧会话的接管卡/软提示跟着漏到新会话。
    // (在途路径的**晚到** setState 由 withSendGuard/ensureWritableTerminal 的活跃会话复核挡住,
    //  这里只负责切换瞬间的即时归零。)
    setSending(false);
    setNeedsTakeover(false);
    setSoftPromptNotice(false);
    // 静默模型探测(probeModelMenu,~12s)也按会话作废:只清兜底 timer 不够,modelProbing
    // 残留会让新会话模型钮一直转圈,silentProbeRef/modelMenuPendingRef 残留会串到新会话的
    // 菜单识别里。timer 的 [sessionId] cleanup 会在下次渲染清,这里同步先清一遍。
    window.clearTimeout(modelProbeTimerRef.current);
    setModelProbing(false);
    silentProbeRef.current = false;
    modelMenuPendingRef.current = false;
    setTerminalAttention(null);
    setTerminalMonitorNeeded(false);
    setMenuWatchUntil(0);
    setQueuedInterjections([]);
    setPendingEchoes([]);
    setAttachments(draft?.attachments ?? []);
    setAttachmentNotice("");
    setApproval(null);
    setModelMenu(false);
    setModeMenu(null);
    setSlashIndex(0);
    setSlashDismissed(false);
    setAtQuery(null);
    setAtFiles([]);
    historyNavRef.current = null;
    setChatUi(null);
    setCapabilityOffset(0);
    setBrokerOwnsReview(false);
    setHasEarlier(false);
    setLoadingEarlier(false);
    positionedRef.current = false;
    followRef.current = true;
    // 切会话一律保持当前视图：用户在终端就显示终端，在对话就显示对话。负 id（尚未
    // 认领的新会话）也不再强制进终端——对话页有「启动中」占位，claim 成真 id 时
    // 同样走这里，视图原地不动。
    setView(viewRef.current);
    setSessionId(id);
  }, []);
  // 窄窗抽屉里的选中:先收抽屉再切(resetTo 自带同 id 守卫)。稳定引用给 memo 的
  // ChatSidebar 用,理由见 collapseSidebar。
  const selectFromOverlay = useCallback((id: number) => {
    setOverlaySidebar(false);
    resetTo(id);
  }, [resetTo]);

  const refresh = useCallback(async () => {
    if (sessionId <= 0 || busyRef.current) {
      // 0(远程未选会话)与负数(冷启动临时 id)都不该停留在加载态——0 漏掉的话
      // 「正在读取对话…」会永远挂着。
      if (sessionId <= 0) setLoading(false);
      return;
    }
    busyRef.current = true;
    try {
      const next = await getChatHistory(sessionId, offsetRef.current);
      if (activeSessionRef.current !== sessionId) return;
      // 空批次的 reset 不采信:transcript 暂不可解析(换 profile 的路径切换途中、compaction
      // 重写期间 resolve 不到文件)时,后端会回 reset=true + items=[] + offset 原值。照单清屏
      // 会把整段历史抹掉,而 offset 不回 0,之后只拿增量,历史永久丢失。忽略这一发(消息、
      // offset、agentModes 全都不动),真正的「文件重建」reset 随后必然带全量 items 再来,
      // 到时再清不迟;offset 保持原值也让「文件确实变小」的场景在下一轮触发带内容的 reset。
      const emptyReset = next.reset && next.items.length === 0;
      const reset = next.reset && !emptyReset;
      if (!emptyReset) {
        setCapabilityOffset(next.offset);
        // hasMore 只有首读那一发才可能为 true，后续增量恒为 false——单独记下来。
        if (next.hasMore) setHasEarlier(true);
        offsetRef.current = next.offset;
      }
      // 保留旧引用（而非无条件 setHistory）——稳态下这些字段一轮都不变，但 next 每次
      // 都是新对象，无脑 set 会让整个窗口每 650ms 重渲染一次。items 已在下面单独短路。
      setHistory((prev) => {
        // mode 只在 transcript 出现新模式记录时随增量返回。普通增量为 null 时保留上次观测；
        // 文件 reset 则必须采信全量重读结果，避免沿用旧 transcript 的状态。
        const updates = next.agentModes ?? [];
        const agentModes = reset || prev?.sessionId !== next.sessionId
          ? updates
          : [...(prev?.agentModes ?? [])];
        if (!reset && prev?.sessionId === next.sessionId) {
          for (const update of updates) {
            const index = agentModes.findIndex((mode) => mode.dimension === update.dimension);
            if (index >= 0) agentModes[index] = update;
            else agentModes.push(update);
          }
        }
        const merged = { ...next, agentModes };
        return sameHistoryMeta(prev, merged) ? prev : merged;
      });
      setItems((prev) => next.items.length || reset ? reduceChatEvents(prev, next.items, reset) : prev);
      setLoading(false);
      setFailed(false);
      // 存储恢复的会话加载成功:解除恢复标记,此后的瞬时失败按普通失败处理。
      if (restoredSessionId === sessionId) restoredSessionId = 0;
    } catch {
      // 远程从 localStorage 恢复的会话可能已被删/归档:恒失败会卡在「读取失败」,
      // 空态与自动拉抽屉都进不去。首败即放弃恢复回「选会话」空态(误伤瞬时网络
      // 抖动可接受——抽屉在场,点回去即可);用户手选的会话不受此影响。
      if (restoredSessionId === sessionId) {
        restoredSessionId = 0;
        try {
          localStorage.removeItem(REMOTE_LAST_SESSION_KEY);
        } catch {
          /* ignore */
        }
        setSessionId(0);
        setLoading(false);
        return;
      }
      setLoading(false);
      setFailed(true);
    } finally {
      busyRef.current = false;
    }
  }, [sessionId]);

  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => void refresh(), 650);
    return () => window.clearInterval(timer);
  }, [refresh]);

  // 事件驱动的提前刷新（C-14 务实版）：托管会话的 pty-output 一到就在 200ms 合流窗口后
  // 拉一次增量——agent 打印输出时 transcript 多半同步在写，把「流式蹦字」的感知节奏从
  // 固定 650ms 压到输出帧粒度。650ms 轮询保留：外部终端会话没有 pty-output，事件也可能
  // 在窗口重挂间隙丢失，轮询是兜底。真·后端 push（按 transcript 文件 watch）留作后续。
  const outputRefreshTimerRef = useRef(0);
  useTauriEvent<{ sessionId: number }>("pty-output", (event) => {
    if (event.payload.sessionId !== activeSessionRef.current) return;
    if (outputRefreshTimerRef.current) return; // 合流：高频输出下最多 200ms 一次
    outputRefreshTimerRef.current = window.setTimeout(() => {
      outputRefreshTimerRef.current = 0;
      void refreshRef.current?.();
    }, 200);
  });
  const refreshRef = useRef(refresh);
  refreshRef.current = refresh;
  useEffect(() => () => window.clearTimeout(outputRefreshTimerRef.current), []);

  useEffect(() => {
    if (sessionId >= 0) return;
    let cancelled = false;
    const resolve = () => managedTerminalBinding(sessionId).then((id) => {
      if (!cancelled && id) resetTo(id);
    }).catch(() => {});
    void resolve();
    const timer = window.setInterval(() => void resolve(), 250);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [resetTo, sessionId]);

  // 打开/切换会话时用会话日志重建一次待办。hook 只在 meowo 在场时才捕获得到待办，
  // 中途启动、hook 漏接、早先解析有误（状态别名不认识）都会让 DB 与真实清单脱节，
  // 而 agent 自己的日志一直是对的。一次有界读，不进 650ms 轮询。
  useEffect(() => {
    if (sessionId <= 0) return;
    void refreshSessionTodos(sessionId).catch(() => {});
  }, [sessionId]);

  useEffect(() => {
    let un: (() => void) | undefined;
    let cancelled = false;
    listen<number>("chat-session-changed", (event) => resetTo(event.payload)).then((fn) => {
      if (cancelled) fn(); else un = fn;
    }).catch(() => {});
    return () => { cancelled = true; un?.(); };
  }, [resetTo]);

  // 题面的轮询兜底在 useApprovalChannel 的 pendingInteraction 合并轮询里（同一次 IPC
  // 取审批 + 题面）。兜底过期：识别一直没就绪、用户已在终端答完的情况下，别让展示卡挂一辈子。
  // 题面卡消失（过期/收起/会话切换）时排队的答案一并作废——没有卡背书的按键不许落。
  //
  // dismiss 后端待处理表只允许「本窗口真的展示过这张卡、且卡在同一会话上消解」时发：
  // 会话切换/挂载也会把 structuredQuestion 置 null，但那不是「问题了结」——若此时按新
  // sessionId 无条件 dismiss，会把目标会话里挂着的题面（超出 3s 领养窗口、要靠轮询兜底
  // 才能取到的那种）从后端待处理表里抹掉，题面卡永远不出现，用户只能回终端作答。
  const shownQuestionSessionRef = useRef<number | null>(null);
  useEffect(() => {
    if (!structuredQuestion) {
      setQueuedAnswer(null);
      // 卡已消解：清掉领养 ref；仅当消解发生在展示卡的同一会话上（= 过期/收起/答完，
      // 而非切会话）才清后端待处理表，防止切走再切回时弹幽灵卡。
      lastInteractiveQuestionRef.current = null;
      const shownSid = shownQuestionSessionRef.current;
      shownQuestionSessionRef.current = null;
      if (shownSid != null && shownSid === sessionId && sessionId > 0) {
        void dismissInteractiveQuestion(shownSid).catch(() => {});
      }
      return;
    }
    shownQuestionSessionRef.current = structuredQuestion.sessionId;
    // 作答卡不吃 180s 过期：broker 挂着 300s，到点它自己降级（answerable 随轮询翻
    // false，本 effect 因对象更换重跑，展示卡的 180s 从那一刻起算）。
    if (structuredQuestion.answerable) return;
    const timer = window.setTimeout(() => {
      setStructuredQuestion(null);
      // 过期不能只是凭空消失（用户离开一会儿回来，分不清是自己漏点还是超时）——
      // 说清卡去哪了、该去哪继续。
      setSendError(t.chat.questionExpired);
    }, 180_000);
    return () => window.clearTimeout(timer);
  }, [structuredQuestion, sessionId, t]);
  // 作答卡的剩余时间徽章：broker 侧挂起 300s，与审批倒计时同一课——到点降级不能像
  // bug 一样凭空发生。从本窗口首见该 requestId 起本地计时（晚开窗只会显得更宽裕）。
  const QUESTION_TIMEOUT_MS = 300_000;
  const questionSeenRef = useRef<{ id: string; at: number } | null>(null);
  const [questionNow, setQuestionNow] = useState(() => Date.now());
  useEffect(() => {
    if (!structuredQuestion?.answerable) {
      questionSeenRef.current = null;
      return;
    }
    if (questionSeenRef.current?.id !== structuredQuestion.requestId) {
      questionSeenRef.current = { id: structuredQuestion.requestId, at: Date.now() };
      setQuestionNow(Date.now());
    }
    const timer = window.setInterval(() => setQuestionNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [structuredQuestion]);
  const questionCountdown = (() => {
    if (!structuredQuestion?.answerable || !questionSeenRef.current) return null;
    const left = Math.max(0, QUESTION_TIMEOUT_MS - (questionNow - questionSeenRef.current.at));
    const totalSeconds = Math.floor(left / 1000);
    return `${Math.floor(totalSeconds / 60)}:${String(totalSeconds % 60).padStart(2, "0")}`;
  })();
  // 「问题已了结」的收卡信号：判定逻辑见 observeTranscriptForDismiss。
  //
  // 计数**必须**读独立累积的 items state，不能读 history.offset/history.items——那两个
  // 字段在 HISTORY_META_EXCLUDED 里，meta 未变时 setHistory 保留旧对象，从它身上读到的
  // 是冻结值，且增量到达时本效果根本不会重跑（sameHistoryMeta 注释里「数据变了界面
  // 不动」事故的第四回，实测踩过：卡永远收不掉）。status 在 meta 内，其变化会换新
  // history 对象，回合结束信号照常送达。
  //
  // 静置期 1.5s：提问自身的 tool_use 先于题面事件落盘、至迟一个轮询周期（650ms）进
  // items，1.5s 足够吸收；真人 Esc 快不过它，快过了还有 180s 兜底过期。
  const questionTranscriptBaseline = useRef<QuestionDismissTracker | null>(null);
  useEffect(() => {
    questionTranscriptBaseline.current = structuredQuestion
      ? { armAt: Date.now() + 1_500, count: null }
      : null;
  }, [structuredQuestion]);
  useEffect(() => {
    const baseline = questionTranscriptBaseline.current;
    if (!structuredQuestion || !baseline) return;
    const observation = {
      count: items.length,
      // 提问只可能发生在回合中（UserPromptSubmit 已把状态写成 running，且先于题面事件
      // 落库）；确证离开 running = 回合已结束 = 问题必已了结（作答或取消）。
      // history 未加载时是 **null（未知）** 而非 false——见 observeTranscriptForDismiss。
      running: history ? history.status === "running" : null,
    };
    if (observeTranscriptForDismiss(baseline, observation, Date.now())) setStructuredQuestion(null);
  }, [structuredQuestion, history, items]);

  useEffect(() => {
    // transcript 的 pendingReview 比 broker 的实时状态慢一拍。只有历史状态确实清空后，
    // 才重新启用“去终端处理”的兼容提示，避免 GUI 审批完成时闪一下旧提示。
    if (!history?.pendingReview) setBrokerOwnsReview(false);
  }, [history?.pendingReview]);

  // 程序化吸底一律瞬移（临时关掉 smooth）：`.chat-scroll` 的平滑滚动是动画，动画途中
  // 浏览器持续派发 scroll 事件——一条长消息到达时，从旧位置滚向新底部的中途距离远大于
  // onScroll 的 80px 阈值，followRef 被误置 false，吸底从此断掉（此坑首帧定位早就堵过，
  // 但只堵了首帧；后续每一批 delta 的吸底都在裸奔）。smooth 留给用户主动的滚动。
  const stickToBottom = (el: HTMLElement) => {
    const behavior = el.style.scrollBehavior;
    el.style.scrollBehavior = "auto";
    el.scrollTop = el.scrollHeight;
    el.style.scrollBehavior = behavior;
  };
  useLayoutEffect(() => {
    if (view !== "chat") {
      // 终端页会卸载 chat-scroll；切回来得到的是全新的滚动容器，必须重新做一次首帧定位。
      positionedRef.current = false;
      followRef.current = true;
      setAtBottom(true);
      return;
    }
    if (!followRef.current) return;
    const el = scrollRef.current;
    if (!el || timelineItems.length === 0) return;
    stickToBottom(el);
    positionedRef.current = true;
  }, [timelineItems, view]);

  // 终端页会把焦点交给 xterm（且 footer 整个卸载，回来的 textarea 是全新节点）；
  // 用户切回对话时把焦点还给输入框。只在「终端 → 对话」的切换沿触发——chat 内的
  // 普通重渲染不动焦点，不打断正在输入的人。
  const prevViewRef = useRef(view);
  useEffect(() => {
    const prev = prevViewRef.current;
    prevViewRef.current = view;
    if (view === "chat" && prev === "terminal") promptInputRef.current?.focus();
  }, [view]);

  // 输入框随内容长高（上限与 CSS max-height:150px 对齐）：rows=1 + min-height 曾把高度
  // 钉死不动，max-height 是死代码——写 5 行提示词只能在 3 行可视高的框里内部滚动。
  // 依赖 prompt 而非 onChange：清空发送、恢复草稿等程序化赋值也要跟着缩/涨。
  useEffect(() => {
    const el = promptInputRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 150)}px`;
  }, [prompt, view]);

  /// 拉取被首读裁掉的更早消息，并保持用户当前看的那一行不动。
  /// 直接替换 items 会让滚动位置塌到顶部——记下加载前的 scrollHeight，补回增量即可。
  /// 与轮询共用 busyRef：两者都会写 offsetRef/items，交叉执行时先返回的一方会被后返回的
  /// 覆盖——轮询刚追加的新消息会被这里的全量替换抹掉（下一轮才补回来，表现为闪烁）。
  const loadEarlier = async () => {
    if (loadingEarlier || busyRef.current) return;
    busyRef.current = true;
    setLoadingEarlier(true);
    const el = scrollRef.current;
    const prevHeight = el?.scrollHeight ?? 0;
    const prevTop = el?.scrollTop ?? 0;
    try {
      const full = await getChatHistory(sessionId, 0, true);
      if (activeSessionRef.current !== sessionId) return;
      offsetRef.current = full.offset;
      // 全量重建：这批数据已包含现有消息，直接替换而不是 append。
      setItems(reduceChatEvents([], full.items, true));
      setHasEarlier(false);
      // 跳过一次自动吸底，否则用户会被弹回最新消息。
      followRef.current = false;
      requestAnimationFrame(() => {
        const node = scrollRef.current;
        if (!node) return;
        // 保位是程序化定位：必须瞬移，smooth 会把「保持原位」动画成一次上下滑动。
        const behavior = node.style.scrollBehavior;
        node.style.scrollBehavior = "auto";
        node.scrollTop = prevTop + (node.scrollHeight - prevHeight);
        node.style.scrollBehavior = behavior;
        // 按落位重算吸底状态。内容不足一屏时 scrollTop 仍是 0，不会有 scroll 事件来
        // 恢复 followRef，漏掉这一步会让之后的新消息再也不自动滚到底。
        const at = node.scrollHeight - node.scrollTop - node.clientHeight < 80;
        followRef.current = at;
        setAtBottom(at);
      });
    } catch {
      // 失败不清空已有消息：保留提示让用户可以再点一次。
    } finally {
      busyRef.current = false;
      setLoadingEarlier(false);
    }
  };

  const onScroll = () => {
    const el = scrollRef.current;
    if (!el) return;
    const at = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
    followRef.current = at;
    // 驱动「回到最新」悬浮钮的显隐；同值短路避免滚动过程反复重渲染。
    setAtBottom((prev) => (prev === at ? prev : at));
  };
  // 上翻后回到底部的唯一出口（此前只能手动滚回去，新消息静默追加在视口外）。
  const jumpToLatest = () => {
    const el = scrollRef.current;
    if (!el) return;
    followRef.current = true;
    setAtBottom(true);
    stickToBottom(el);
  };
  const close = () => getCurrentWindow().close().catch(() => {});
  /// 发送类动作的公共守卫:sending 置位/清错 → 终端占用检查 → 确保可写终端 → 执行 body,
  /// 统一 catch/收尾。sendText/sendWithClipboardImages/changeMode 三个入口共用——门控语义
  /// (新增不可写原因、needsTakeover 复位时机)只需改这一处,不会漏出某条不检查占用的路径。
  const withSendGuard = async (body: () => Promise<boolean>): Promise<boolean> => {
    setSending(true);
    setSendError("");
    setSoftPromptNotice(false);
    setNeedsTakeover(false);
    try {
      if (terminalAttention) {
        terminalEverShownRef.current = true;
        setSendError(t.chat.terminalNeedsAttention);
        return false;
      }
      // 软拦(非阻断):hook 说 agent 在等交互(pendingReview),但屏幕识别没认出卡片——识别
      // 是启发式的,误报率不低,弹窗拦截会把最高频的「发消息」路径卡住。改为照常发送,
      // 同时亮一条可跳终端的提示;真误触的兜底仍是终端页人工确认。
      if (!terminalAttention && history?.pendingReview && !approval && !brokerOwnsReview) {
        setSoftPromptNotice(true);
      }
      if (!await ensureWritableTerminal()) return false;
      return await body();
    } catch (error) {
      // 切走后再报错会把错误横幅挂到别的会话上;只对仍在看的发起会话落错。
      if (activeSessionRef.current === sessionId) setSendError(formatBackendError(error, t.locale));
      return false;
    } finally {
      // 同理:切走后别用旧路径的收尾去动新会话的 sending(新会话的发送态归它自己管,
      // 切换瞬间已由 resetTo 归零)。仍在发起会话上才清。
      if (activeSessionRef.current === sessionId) setSending(false);
    }
  };
  /// 回车前复核(见 submitToTerminal 的 abortIf):正文落下后、回车发出前,交互提示
  /// 恰好弹出的话,这个回车会替用户回答它——命中就中止提交。
  const attentionAbort = () => (terminalAttentionRef.current ? t.chat.terminalNeedsAttention : null);
  /// 恢复/接管的完成时刻:险区窗口内(RESUME_VERIFY_WINDOW_MS)的发送做写后回显验证。
  const resumedAtRef = useRef(0);
  /// 发送一段文本到会话（消息正文与斜杠命令共用）。返回是否真的送达。
  const sendText = (content: string): Promise<boolean> => withSendGuard(async () => {
    const verify = Date.now() - resumedAtRef.current < RESUME_VERIFY_WINDOW_MS
      ? { message: t.chat.sendEchoTimeout }
      : undefined;
    await submitToTerminal(sessionId, content, attentionAbort, verify);
    return true;
  });
  /// 剪贴板原生图片附加:向 PTY 发 Ctrl-V,让 TUI 自己读剪贴板走它的官方图片粘贴通道
  /// (claude 的 `[Image #N]`、kimi 的 `[image:…]`),从屏幕上确认占位符出现后再写正文提交。
  /// 调用方已确认「剪贴板指纹 == 待发附件」,全程只读剪贴板、不写。
  /// 返回 false = 未送达,调用方退回指令文本。
  /// 原生图片附加：逐张把附件写进系统剪贴板 → 发 Ctrl-V 让 TUI 自己读（claude 的
  /// `[Image #N]`、kimi 的 `[image #N (W×H)]`）→ 等第 N 个占位符出现再贴下一张，
  /// 全部落地后提交正文。剪贴板是发送通道的耗材：首次写入前后端自动快照，这里 finally
  /// 还原。任一张落地失败（剪贴板写不进、占位符超时）返回 false，调用方回退指令文本。
  const sendWithClipboardImages = (content: string, marker: string, images: Attachment[]): Promise<boolean> => withSendGuard(async () => {
    const before = await managedTerminalSnapshot(sessionId);
    const markerPattern = new RegExp(marker, "gi");
    const decoder = new TextDecoder();
    let tail = "";
    let since = before.endOffset;
    try {
      for (let index = 0; index < images.length; index += 1) {
        try {
          await clipboardSetImage(images[index].path);
        } catch {
          // 剪贴板写不进去（被占用/附件文件已删）：绝不能照常 Ctrl-V——那会把剪贴板里
          // 别人的内容附给 agent。回退指令文本由调用方接。
          return false;
        }
        await writeManagedTerminal(sessionId, "\x16");
        // TUI 读剪贴板+缓存图片是异步的:轮询增量输出等占位符计数达标,1.5s 没出现放弃原生化。
        let landed = false;
        for (let attempt = 0; attempt < 6 && !landed; attempt += 1) {
          await new Promise((resolve) => window.setTimeout(resolve, 250));
          const snapshot = await managedTerminalSnapshot(sessionId, since);
          // PTY 中途退出/重启:定格帧上等不来占位符,直接走回退(与其余三处屏幕轮询同守卫)。
          if (!snapshot.active) break;
          if (snapshot.endOffset < since) {
            // 偏移回绕(PTY 被重启复位):按快照契约把 since 归零重新对齐,别拿旧偏移空等。
            since = 0;
            tail = "";
            continue;
          }
          since = snapshot.endOffset;
          tail = appendTerminalText(tail, snapshot.data, decoder);
          landed = (visibleTerminalText(tail).match(markerPattern) ?? []).length >= index + 1;
        }
        if (!landed) {
          // 占位符没出现。Ctrl-V 的副作用不可撤销——慢机/大图上图片可能在超时之后才落进
          // composer,若直接回退指令文本,迟到的原生图会和它一起提交成重复附件。先 Ctrl-U
          // 清行做尽力撤销(两家 TUI 都支持 emacs 行编辑),把残余竞态压缩到「清行后才落地」
          // 的极小窗口。
          await writeManagedTerminal(sessionId, "\x15").catch(() => {});
          return false;
        }
      }
      await submitToTerminal(sessionId, content, attentionAbort);
      return true;
    } finally {
      await clipboardRestore().catch(() => {});
    }
  });
  /// 确保有一个可写的托管终端；没有就地拉起。返回 false 表示已把原因写进 sendError。
  ///
  /// 关键点：**不再靠前端的 status 猜**「是不是还在外部终端跑着」。status 为 stale 只说明
  /// 一段时间没上报，进程很可能早就没了——而旧逻辑会把这类会话一律拒掉，用户明明可以直接发。
  /// 后端的 `session_agent_alive` 是按 pid 的权威判定，让它来拒：拒了才说明进程真活着，
  /// 这时给出就地接管入口，而不是一句「请自己切到终端页」的死路。
  async function ensureWritableTerminal(): Promise<boolean> {
    const snapshot = await managedTerminalSnapshot(sessionId);
    // 必须同时 managed:后台会话旁路的快照 active 只代表旁观连接活着,不代表能输入。
    // 曾只看 active,恢复会话被旁路活性短路——托管 PTY 根本没起,之后终端打字全无回显。
    if (snapshot.active && snapshot.managed) return true;
    // capability 查询通常已随 history 完成；用户极快发送时就在这里补等一次，不能因为
    // React 状态尚未落下而漏掉 provider 声明的信任/登录提示。
    const ui = chatUi ?? (provider ? await agentChatUi(provider, cwd, sessionId).catch(() => null) : null);
    try {
      await startManagedTerminal(sessionId, 100, 30, resumeOptions);
    } catch (error) {
      // 拉终端可跑 45s,await 期间用户可能已切走:这条路的失败结果(接管卡/错误)只属于
      // 发起它的会话,落到当前正看的别的会话就是凭空冒出的「需要接管」。切走了就静默收场。
      if (activeSessionRef.current !== sessionId) return false;
      // Agent 自己的守护进程托管着它（FleetView 后台会话）：接管这条路走不通——杀掉进程
      // 只会被 supervisor 按 respawnFlags 拉回来。给一句说明而不是一个注定失败的按钮。
      if (history?.background) {
        setSendError(t.chat.sendBackgroundSession);
        return false;
      }
      // 后端确认进程仍活着 → 接管要杀掉外部进程，必须由用户显式确认，不能由一次发送代劳。
      if (externalRunning) {
        setNeedsTakeover(true);
        setSendError(t.chat.sendNeedsTakeover);
        return false;
      }
      throw error;
    }
    // 已挂载的后台终端还停在旧进程的输出偏移上，必须归零重拉，否则新 PTY 的输出
    // 会被它当成「已写过」整段丢弃，画面定格、屏幕识别全部失效。
    terminalRearmRef.current?.();
    // 刚拉起的终端进入回显验证险区(见 RESUME_VERIFY_WINDOW_MS)。
    resumedAtRef.current = Date.now();
    const startup = await waitForTerminalReady(sessionId, ui?.startup_attention_markers ?? [], terminalReadyMessages, { provider: history?.provider, selectorAnchors: ui?.selector_anchors ?? [], attentionPatterns: ui?.attention_patterns ?? [] });
    // 就绪等待(最长 45s)期间切走的话,注意力卡/错误不再落到当前会话。
    if (activeSessionRef.current !== sessionId) return false;
    if (startup !== "ready") {
      terminalEverShownRef.current = true;
      setTerminalAttention(startup);
      setSendError(t.chat.terminalNeedsAttention);
      return false;
    }
    return true;
  }

  /// 就地接管：结束外部进程、在 Meowo 的 PTY 里恢复同一会话，然后重试刚才那个动作。
  /// 接管是破坏性的（杀掉用户自己终端里的 agent），故必须显式确认。
  const takeoverAndRetry = async (retry: () => void | Promise<void>) => {
    // 确认框走应用内模态(appConfirm)。**不是 `window.confirm`**:后者在 Tauri 的
    // webview(尤其 macOS WKWebView)里会被直接吞掉、恒返回 false;系统原生 MessageBox
    // (plugin-dialog)则与应用样式脱节,已弃用。
    const yes = await appConfirm(t.chat.terminalTakeoverConfirm, {
      title: t.chat.terminalTakeover,
      danger: true,
    });
    if (!yes) return;
    setSending(true);
    setSendError("");
    try {
      await takeoverManagedTerminal(sessionId, 100, 30, resumeOptions);
      terminalRearmRef.current?.();
      // 接管刚完成同样进入回显验证险区(见 RESUME_VERIFY_WINDOW_MS)。
      resumedAtRef.current = Date.now();
      setNeedsTakeover(false);
      const startup = await waitForTerminalReady(sessionId, chatUi?.startup_attention_markers ?? [], terminalReadyMessages, attentionGrammar);
      if (startup !== "ready") {
        terminalEverShownRef.current = true;
        setTerminalAttention(startup);
        setSendError(t.chat.terminalNeedsAttention);
        return;
      }
    } catch (error) {
      setSendError(formatBackendError(error, t.locale));
      return;
    } finally {
      setSending(false);
    }
    await retry();
  };

  /// 跨 provider 切换：确认 → 后端导出交接文件并起新会话 → 本窗切到临时负 id
  ///（既有 binding 轮询会把它换成真 id）→ 注入 effect 在新终端就绪后发交接提示。
  /// 破坏性动作（结束当前进程）必须显式确认，与接管同款纪律。
  const startProviderSwitch = async (target: AgentDescriptor, model?: string) => {
    setModelMenu(false);
    const from = currentAgentDescriptor?.display_name ?? provider ?? "";
    const yes = await appConfirm(t.chat.switchProviderConfirm(from, target.display_name), {
      title: t.chat.switchProvider,
      danger: true,
    });
    if (!yes) return;
    setSending(true);
    setSendError("");
    try {
      const started = await switchSessionProvider(sessionId, target.id, model ? { model } : undefined);
      handoffRef.current = { path: started.handoffPath, expectProvider: target.id, at: Date.now() };
      resetTo(started.tempId);
    } catch (error) {
      setSendError(formatBackendError(error, t.locale));
    } finally {
      setSending(false);
    }
  };

  /// 交接注入：切换产生的新会话被认领成真 id、chatUi 就绪后，等终端可写再把
  /// 「请读交接文件」发进去。启动交互（目录信任等）拦路时亮出注意力卡、保留待注入
  /// 记账，用户处理完（attention 清除）本 effect 重跑续注。任何失败都把注入语落回
  /// 输入框当草稿——宁可让用户多按一次回车，不能让交接静默丢失。
  const handoffInjectingRef = useRef(false);
  useEffect(() => {
    const pending = handoffRef.current;
    if (!pending || sessionId <= 0 || handoffInjectingRef.current) return;
    // 过期防污染：拖过 10 分钟（比如用户一直没处理信任提示就去干别的）不再自动注入。
    if (Date.now() - pending.at > 600_000) { handoffRef.current = null; return; }
    if (history?.provider !== pending.expectProvider || !chatUi) return;
    // 只认切换产生的那条会话：接续链已落库（predecessorId 非空）才是我们等的新段。
    if (history.predecessorId == null) return;
    if (terminalAttention) return; // 等用户处理完启动交互，attention 清除后重跑
    let cancelled = false;
    const injectPrompt = t.chat.handoffPrompt(pending.path);
    handoffInjectingRef.current = true;
    (async () => {
      try {
        const startup = await waitForTerminalReady(
          sessionId,
          chatUi.startup_attention_markers ?? [],
          terminalReadyMessages,
          attentionGrammar,
        );
        if (cancelled) return;
        if (startup !== "ready") {
          terminalEverShownRef.current = true;
          setTerminalAttention(startup);
          return;
        }
        await submitToTerminal(sessionId, injectPrompt, undefined, { message: t.chat.handoffInjectFailed });
        if (cancelled) return;
        handoffRef.current = null;
      } catch (error) {
        if (cancelled) return;
        handoffRef.current = null;
        setPrompt(injectPrompt);
        setSendError(formatBackendError(error, t.locale));
      } finally {
        handoffInjectingRef.current = false;
      }
    })();
    return () => { cancelled = true; handoffInjectingRef.current = false; };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- t/attentionGrammar 随渲染变化，注入只由会话代际与就绪信号驱动
  }, [sessionId, history?.provider, history?.predecessorId, chatUi, terminalAttention]);

  const sendPrompt = async () => {
    if ((!prompt.trim() && attachments.length === 0) || sending) return;
    // 交互式内置命令（/config、/resume、无参 /model…）裸发出去会在终端里弹选择器：
    // 对话页什么都看不见，后续输入还会打进那个看不见的菜单。命中插件声明的清单时
    // 改走菜单识别通道——与「切换模型」按钮同一条路。只认精确命中（带参数的
    // `/model sonnet` 是内联执行，不弹菜单，照常发送）。
    const bare = prompt.trim();
    if (attachments.length === 0 && (chatUi?.menu_slash_commands ?? []).includes(bare)) {
      if (await openTerminalMenu(bare)) setPrompt("");
      return;
    }
    // 后台会话不消费 stdin,往它的 PTY 写按键石沉大海。送话走 Agent 守护进程的控制通道:
    // 发出后 agent 就开始干活,回复照常落进 transcript,本页的轮询照常显示。
    if (history?.background) {
      if (attachments.length > 0) { setSendError(t.chat.sendBackgroundNoAttachments); return; }
      setSending(true);
      setSendError("");
      try {
        await sendBackgroundPrompt(sessionId, bare);
        // 守护进程回的 ok 只代表**收下了这条投递**,不代表 agent 会处理它:处于手动模式
        // 或正在收尾的 worker 不消费投递,消息就此蒸发。而这类会话的 transcript 往往不
        // 落盘(见上面的说明),对话页也验证不了它到底有没有被消化。
        // 所以**不清空输入框**——宁可让用户自己按需清掉,也不要把一条没人处理的消息连同
        // 用户刚打的字一起吞掉(实测踩过:输入框一空,内容再也找不回来)。
        setSendError(t.chat.sendBackgroundQueued);
      } catch (error) {
        setSendError(formatBackendError(error, t.locale));
      } finally {
        setSending(false);
      }
      return;
    }
    retryRef.current = () => sendPrompt();
    // 运行中发出的消息会被 CLI **排队**到回合结束才处理,期间 transcript 里看不到它
    // ——不记下来的话,消息在 GUI 上像消失了一样。发送成功后进排队清单,由轮询侧
    // 在它出现于 transcript / 回合结束时清除。
    const interjecting = tone === "running";
    const queuedText = bare || t.chat.queuedAttachmentOnly;
    const markQueued = () => {
      // 水位线快照:此刻已有的最近用户消息与 transcript 尾部条目都算旧证据,消解
      // 一律不认(见 queuedInterjections 声明处)。
      const priorItemIds: ReadonlySet<string> = new Set(
        items.slice(-12).filter((item) => item.type === "user_text").map((item) => item.id),
      );
      if (interjecting) {
        queuedIdRef.current += 1;
        const id = queuedIdRef.current;
        setQueuedInterjections((current) => [...current, {
          id,
          text: queuedText,
          priorUserText: history?.lastUserText ?? null,
          priorItemIds,
        }]);
      } else {
        // 空闲态：乐观回显（见 pendingEchoes 声明处）。
        echoIdRef.current += 1;
        setPendingEchoes((current) => [...current, {
          id: echoIdRef.current,
          text: queuedText,
          at: Date.now(),
          priorUserText: history?.lastUserText ?? null,
          priorItemIds,
        }]);
      }
    };
    // 原生图片附加:仅当「附件全部是图片 + 插件声明 TUI 支持 Ctrl-V 图片粘贴」。逐张写
    // 剪贴板让 TUI 自己读(kimi/claude 的 composer 都支持多张图连续粘贴)。含非图片附件、
    // 或任一张落地失败(剪贴板写不进、占位符超时)都静默退回指令文本——那条路径对所有
    // agent 恒可用。
    // 远程跳过:clipboard_set_image 是 /rpc 拒绝项,必 404 再回退——省掉每张图两发白请求。
    const clipMarker = !remoteUi() && chatUi?.clipboard_image_paste;
    if (clipMarker && attachments.length > 0 && attachments.every((file) => file.image)) {
      if (await sendWithClipboardImages(prompt.trim(), clipMarker, attachments)) {
        setPrompt("");
        setAttachments([]);
        setAttachmentNotice("");
        markQueued();
        return;
      }
    }
    if (await sendText(promptWithAttachments(prompt, attachments, t.chat.attachmentInstruction, chatUi?.attachment_mention ?? false))) {
      setPrompt("");
      setAttachments([]);
      setAttachmentNotice("");
      markQueued();
    }
  };
  /// 强制插话:先写插件声明的中断键停掉当前回合,稍候再正常发送。中断后 CLI 会先
  /// 处理它队列里已有的消息,这条随后跟上;当前回合已完成的工作保留在 transcript 里。
  const interruptAndSend = async () => {
    const interrupt = chatUi?.interrupt_input;
    if (!interrupt || sending) return;
    try {
      await writeManagedTerminal(sessionId, interrupt);
    } catch (error) {
      setSendError(formatBackendError(error, t.locale));
      return;
    }
    // 给 TUI 一拍完成中断收尾(打印 Interrupted、回到 composer),再走正常发送。
    await new Promise((resolve) => window.setTimeout(resolve, 500));
    await sendPrompt();
  };
  /// 只发中断键,停掉当前回合、不发送任何内容。composer 上的裸「中断」与排队条的
  /// 「立即插话」是同一个动作,只是语境不同:后者停下当前回合后,CLI 自己会接着处理队列。
  ///
  /// interrupting:写完中断键后 tone 要等 650ms+ 轮询才变,期间按钮若维持可点形态,
  /// 用户以为没点上会连点——多个中断键连打进 PTY(claude 的双 Esc 另有历史回退语义)。
  /// 置忙态直到 tone 离开 running 或超时兜底,期间按钮禁用并转圈。
  const [interrupting, setInterrupting] = useState(false);
  const sendInterrupt = () => {
    const interrupt = chatUi?.interrupt_input;
    if (!interrupt || interrupting) return;
    setInterrupting(true);
    void writeManagedTerminal(sessionId, interrupt)
      .catch((error) => {
        setSendError(formatBackendError(error, t.locale));
        setInterrupting(false);
      });
  };
  useEffect(() => {
    if (!interrupting) return;
    if (tone !== "running") { setInterrupting(false); return; }
    // 兜底:中断被 TUI 吞掉(如恰逢重绘)时 4s 后解锁,允许再按。
    const timer = window.setTimeout(() => setInterrupting(false), 4_000);
    return () => window.clearTimeout(timer);
  }, [interrupting, tone]);
  // 换会话清忙态:旧会话的中断在途与新会话无关。
  useEffect(() => { setInterrupting(false); }, [sessionId]);
  /// 手动移除一条排队回执。**只动 GUI 记账**:消息已经写进 PTY,CLI 队列里的它照常
  /// 在回合结束后执行——按钮文案与 tip 必须说清这点,否则用户会以为消息被撤回了。
  const dismissQueued = (id: number) =>
    setQueuedInterjections((current) => current.filter((item) => item.id !== id));
  // 排队插话的消解:回合结束(tone 离开 running)= CLI 开始处理队列,整单清空;
  // 某条提前出现在 transcript/兜底时间线里也单独移除(插话打断当前回合时 CLI 立即
  // 处理队列,tone 不离开 running,只有这条路径能收走回执)。匹配用「包含」而非全等:
  // 落盘文本会带附件指令和 TUI 的 [Image #N] 占位前缀,hook 的 lastUserText 又把换行
  // 归一成空格,全等永远失配,回执会一直挂着误导用户。「包含」的代价由水位线兜住:
  // 只认入队之后才出现的证据——lastUserText 要与入队时快照不同,transcript 条目要
  // 不在入队时的尾部快照里;否则「ok」「继续」这类短插话立刻子串命中上一回合的旧
  // 消息,回执在消息还排着队时就消失了。updater 里同引用短路,防 setState 新引用
  // 触发自循环。
  useEffect(() => {
    setQueuedInterjections((current) => {
      if (current.length === 0) return current;
      if (tone !== "running") return [];
      const collapse = (text: string) => text.replace(/\s+/g, " ").trim();
      const recent = items.slice(-12);
      const next = current.filter(({ text, priorUserText, priorItemIds }) => {
        const needle = collapse(text);
        const seen = (candidate: string | null | undefined) =>
          !!candidate && collapse(candidate).includes(needle);
        const lastUserChanged = history?.lastUserText != null
          && collapse(history.lastUserText) !== collapse(priorUserText ?? "");
        return !(lastUserChanged && seen(history?.lastUserText))
          && !recent.some((item) => item.type === "user_text"
            && !priorItemIds.has(item.id)
            && seen((item as { text?: string }).text));
      });
      return next.length === current.length ? current : next;
    });
  }, [tone, items, history?.lastUserText]);
  // 乐观回显的消解：与排队回执同一套「包含 + 水位线」证据规则，但**不随 tone 清空**——
  // 空闲态发送后 tone 会变 running，回显此刻正要发挥作用。15s 兜底防证据永不出现。
  useEffect(() => {
    setPendingEchoes((current) => {
      if (current.length === 0) return current;
      const collapse = (text: string) => text.replace(/\s+/g, " ").trim();
      const recent = items.slice(-12);
      const now = Date.now();
      const next = current.filter(({ text, at, priorUserText, priorItemIds }) => {
        if (now - at > 15_000) return false;
        const needle = collapse(text);
        const seen = (candidate: string | null | undefined) =>
          !!candidate && collapse(candidate).includes(needle);
        const lastUserChanged = history?.lastUserText != null
          && collapse(history.lastUserText) !== collapse(priorUserText ?? "");
        return !(lastUserChanged && seen(history?.lastUserText))
          && !recent.some((item) => item.type === "user_text"
            && !priorItemIds.has(item.id)
            && seen((item as { text?: string }).text));
      });
      return next.length === current.length ? current : next;
    });
  }, [items, history?.lastUserText]);
  /// 斜杠命令直通 PTY——CLI 的 composer 收到 "/xxx" + 回车会当命令执行，无需特殊协议。
  const sendSlash = (command: string) => {
    if (sending) return;
    retryRef.current = () => sendSlash(command);
    void sendText(command);
  };
  /// 发一条会弹出交互菜单的命令（如 `/model`），并在随后一小段时间里让屏幕识别去认那个菜单。
  ///
  /// 为什么不直接下发 `/model <id>`：除 claude 外几家的 `/model` 都是交互式菜单，内联参数
  /// 无效（实测 kimi 的命令描述就是 `/model: switch model`）。发命令再把 CLI 弹出的菜单
  /// 转成 GUI 按钮，模型清单由 CLI 现给——宿主不必维护一份会随用户配置过时的清单。
  const openTerminalMenu = async (command: string, forModelMenu = false) => {
    modelMenuPendingRef.current = forModelMenu;
    // `sending` 在写完就落回 false，而 TUI 的菜单要过一会儿才画出来。只看它的话，用户
    // 觉得「没反应」再点一次，第二遍命令就直接打进已经打开的菜单搜索框里——实测会变成
    // `Search: /model/model`、`No matches`，三个模型全被过滤掉，反而彻底选不了。
    // 故识别窗口开着期间一律不再重发。
    if (sending || menuWatching) return false;
    retryRef.current = () => { void openTerminalMenu(command); };
    // 菜单要靠屏幕识别，而识别跑在 ManagedTerminal 里——它可能还没挂载（用户从没开过终端页）。
    setTerminalMonitorNeeded(true);
    setMenuWatchUntil(Date.now() + 20_000);
    const sent = await sendText(command);
    if (!sent) setMenuWatchUntil(0);
    return sent;
  };
  /// 静默探测模型清单：让 CLI 弹一次 `/model` 菜单，识别到清单就立刻 Esc 收掉并打开 GUI 下拉。
  /// 整个过程不上屏——终端页不弹卡、compose 上方不出「正在识别」等待条。
  const probeModelMenu = async () => {
    if (!modelMenuCommand || modelProbing || sending) return;
    silentProbeRef.current = true;
    setModelProbing(true);
    setTerminalMonitorNeeded(true); // 识别跑在 ManagedTerminal 里，它可能还没挂载
    setMenuWatchUntil(Date.now() + 12_000); // 驱动识别扫描；提示条已按静默标记隐藏
    const sent = await sendText(modelMenuCommand);
    if (!sent) { endSilentProbe(); return; }
    // 超时兜底：识别不到就收掉菜单并**说清楚**——有些 CLI 的菜单形态尚未取证（见
    // docs/research/tui-menu-captures-2026-07.md 的结论表），读不到时不能只是悄悄
    // 转完 12 秒了事，那用户只会以为按钮坏了。给出真正能用的出路：去终端页自己切。
    window.clearTimeout(modelProbeTimerRef.current);
    modelProbeTimerRef.current = window.setTimeout(() => {
      endSilentProbe();
      setModelProbeFailed(true);
      setSendError(remoteUi() ? t.chat.modelListUnavailableRemote(modelMenuCommand) : t.chat.modelListUnavailable(modelMenuCommand));
    }, 12_000);
  };
  /// 结束静默探测：收掉 CLI 菜单、关识别窗口、清忙态。
  const endSilentProbe = () => {
    window.clearTimeout(modelProbeTimerRef.current);
    silentProbeRef.current = false;
    modelMenuPendingRef.current = false;
    setModelProbing(false);
    setMenuWatchUntil(0);
    void writeManagedTerminal(sessionId, "\x1b").catch(() => {});
  };
  finishSilentProbeRef.current = () => {
    endSilentProbe();
    setModeMenu(null);
    setModelMenu(true); // 清单到手，直接把 GUI 下拉打开——用户点的那一下就是为了看它
  };
  /// 放弃这次菜单交互：给 TUI 一个 Esc 收起菜单，并关掉识别窗口。
  const cancelTerminalMenu = () => {
    setMenuWatchUntil(0);
    setTerminalAttention(null);
    void writeManagedTerminal(sessionId, "\x1b").catch(() => {});
  };
  /// 把某维度的显示值写进 history（乐观更新与屏幕回显共用）。transcript 增量随后到达时
  /// 会覆盖为同一个值（或纠正我们），合并逻辑见轮询处。
  const applyModeValue = (dimension: string, value: string) => {
    setHistory((current) => {
      if (!current) return current;
      const agentModes = [...current.agentModes];
      const index = agentModes.findIndex((mode) => mode.dimension === dimension);
      const update = { dimension, value };
      if (index >= 0) agentModes[index] = update;
      else agentModes.push(update);
      return { ...current, agentModes };
    });
  };
  /// 屏幕回显的代际计数：新一次模式操作作废还在跑的旧轮询，避免旧屏幕的滞后帧倒灌。
  const modeEchoSeqRef = useRef(0);
  // 换会话/卸载同样作废：旧会话的回显不能写进新会话的 history。
  useEffect(() => () => { modeEchoSeqRef.current += 1; }, [sessionId]);
  /// cycle 是盲切，而 claude 只在活跃回合往 transcript 写模式记录——空闲时切换（恰是用户
  /// 切权限模式的典型时机）没有任何增量可等，标签会一直冻在旧值上，看起来就是「点了没效果」。
  /// 补救：写入后短暂轮询 PTY 屏幕，用插件声明的指示文案（provider 文档承诺的稳定文本）
  /// 识别落点。屏幕就是 CLI 自己画的权威状态，识别到什么就显示什么；识别不到则保持现状，
  /// 等 transcript 下一条记录兜底。
  /// 读一次屏幕回显：在 `budgetMs` 内轮询 PTY，识别到模式值就立刻返回（识别不到返回 null）。
  /// 供 echoModeFromScreen（观察）与 cycleToMode（逐步逼近目标）共用同一套识别口径。
  const readModeEcho = async (markers: ModeScreenMarker[], baseline: number, budgetMs: number): Promise<string | null> => {
    if (markers.length === 0) return null;
    const decoder = new TextDecoder();
    let tail = "";
    let since = baseline;
    const startedAt = Date.now();
    while (Date.now() - startedAt < budgetMs) {
      let snapshot;
      try { snapshot = await managedTerminalSnapshot(sessionId, since); } catch { return null; }
      if (!snapshot?.active) return null;
      if (snapshot.endOffset < since) { since = 0; tail = ""; continue; }
      since = snapshot.endOffset;
      tail = appendTerminalText(tail, snapshot.data, decoder);
      const value = modeFromScreen(visibleTerminalText(tail), markers);
      if (value) return value;
      await new Promise((resolve) => window.setTimeout(resolve, 120));
    }
    return null;
  };
  /// 循环按 cycle 键直到落到目标模式：claude 的权限模式只有「切下一个」的快捷键，没有
  /// 直达某个值的办法，但用户要的是「选那个模式」而不是「按 N 次」。每按一次读一次屏幕
  /// 回显确认落点，最多绕一圈（模式数）——转满一圈还没到说明该模式在当前账号/启动参数下
  /// 不可用（插件注释已声明模式集合会变），此时停手并说明，不再空转。
  const cycleToMode = async (dimension: string, target: string, cycleInput: string, markers: ModeScreenMarker[]) => {
    if (sending) return;
    await withSendGuard(async () => {
      const seq = ++modeEchoSeqRef.current;
      for (let step = 0; step < Math.max(1, markers.length); step += 1) {
        if (modeEchoSeqRef.current !== seq) return true; // 换会话/新操作作废本轮
        let base = 0;
        try { base = (await managedTerminalSnapshot(sessionId, 0))?.endOffset ?? 0; } catch { /* 从 0 起扫 */ }
        await writeManagedTerminal(sessionId, cycleInput);
        const value = await readModeEcho(markers, base, 1_500);
        if (value) applyModeValue(dimension, value);
        if (value === target) return true;
      }
      setSendError(t.chat.modeUnreachable);
      return true;
    });
  };
  const echoModeFromScreen = async (dimension: string, markers: ModeScreenMarker[], baseline: number) => {
    if (markers.length === 0) return;
    const seq = ++modeEchoSeqRef.current;
    const decoder = new TextDecoder();
    let tail = "";
    // 从**按键之前**的偏移起扫(baseline 由 changeMode 在写入前采):回显必然落在其后,
    // 旧指示天然出局——扫 backlog 会把切换前的旧指示当回显,把乐观值/transcript 刚落的
    // 正确模式覆写回去;而「循环启动时」当基线又会吞掉写入与启动之间已到达的重绘。
    let since = baseline;
    const startedAt = Date.now();
    while (Date.now() - startedAt < 3_000) {
      let snapshot;
      try { snapshot = await managedTerminalSnapshot(sessionId, since); } catch { return; }
      if (modeEchoSeqRef.current !== seq) return;
      if (!snapshot?.active) return;
      // 偏移回绕(PTY 被就地重启复位):与发送/探测循环同款守卫,从头扫——新进程的
      // 首屏重绘就带着当前模式指示,不会误认旧进程残影。
      if (snapshot.endOffset < since) {
        since = 0;
        tail = "";
        continue;
      }
      since = snapshot.endOffset;
      tail = appendTerminalText(tail, snapshot.data, decoder);
      const value = modeFromScreen(visibleTerminalText(tail), markers);
      if (value) applyModeValue(dimension, value);
      await new Promise((resolve) => window.setTimeout(resolve, 250));
      if (modeEchoSeqRef.current !== seq) return;
    }
  };
  /// 模式动作完全由插件描述：快捷键原样发送，命令则用和人工输入一致的 paste + Enter 序列。
  const changeMode = async (dimension: string, inputs: { data: string; submit: boolean }[], optimisticValue?: string, screenMarkers: ModeScreenMarker[] = []) => {
    if (inputs.length === 0 || sending) return;
    // 若因外部占用被拒，接管后重放的是这同一个动作。
    retryRef.current = () => changeMode(dimension, inputs, optimisticValue, screenMarkers);
    // 守卫与发送同一套(withSendGuard):交后端权威判定,被拒才给接管入口。
    await withSendGuard(async () => {
      // 回显基线:按键**之前**的输出末尾偏移。在写入前采,回显循环从它之后扫——
      // 见 echoModeFromScreen 的注释。拿不到就从 0 起扫,宁可多扫不可漏扫。
      let echoBase = 0;
      try { echoBase = (await managedTerminalSnapshot(sessionId, 0))?.endOffset ?? 0; } catch { /* noop */ }
      for (const input of inputs) {
        // submit 的动作（如 `/plan on`）走同一套提交逻辑，消除「只换行不提交」的竞态；
        // 非 submit 的是原始按键序列（如循环快捷键），原样写、不追回车。
        if (input.submit) await submitToTerminal(sessionId, input.data, attentionAbort);
        else await writeManagedTerminal(sessionId, input.data);
      }
      if (optimisticValue) applyModeValue(dimension, optimisticValue);
      // 不 await：回显是后台观察，不该把发送按钮按住 3 秒。
      void echoModeFromScreen(dimension, screenMarkers, echoBase);
      return true;
    });
  };
  const chooseAttachments = async () => {
    if (remoteUi()) {
      // 手机没有系统文件对话框(plugin-dialog 也拿不到源路径):用原生 <input type=file> 取 File,
      // 再走与粘贴完全相同的 base64→savePastedAttachment 落盘通道(该 command 已在远程白名单)。
      const files = await pickFilesViaInput();
      if (files.length) await pasteAttachments(files);
      return;
    }
    const selected = await open({ multiple: true, directory: false, title: t.chat.chooseAttachments });
    const paths = selected == null ? [] : Array.isArray(selected) ? selected : [selected];
    const fresh = (current: Attachment[]) => {
      const known = new Set(current.map((file) => file.path));
      return paths.filter((path) => !known.has(path)).map(attachmentOf);
    };
    // 超上限必须让用户看见——此前 .slice(0, 12) 之外的部分无声消失，用户以为全发出去了。
    // 提示按 ref 里的当前值预判；updater 内不做副作用。
    setAttachmentNotice(attachmentsRef.current.length + fresh(attachmentsRef.current).length > 12 ? t.chat.attachmentLimit(12) : "");
    setAttachments((current) => [...current, ...fresh(current)].slice(0, 12));
  };
  /// 粘贴文件/图片：webview 的剪贴板只给 File 内容、拿不到源路径，而附件协议是「路径列表
  /// 交给 CLI 自己读」——先经宿主落成临时文件拿到路径，再走与文件选择器完全相同的流程。
  /// 截图（剪贴板位图）与资源管理器里复制的文件都会出现在 clipboardData.files 里。
  const pasteAttachments = async (files: File[]) => {
    const results = await Promise.allSettled(files.map(async (file) => {
      const data = base64OfBuffer(await file.arrayBuffer());
      // 剪贴板截图统一叫 image.png；名字进的是带时间戳的独立子目录，不会互踩。
      const path = await savePastedAttachment(file.name || "pasted.png", data);
      // 缩略图不留 data: URL 副本：粘贴目录在 asset 协议作用域内（tauri.conf.json），
      // ImageRef 按路径回读即可——base64 常驻 state 是每图 1.33 倍体积的白付内存（评审）。
      return { ...attachmentOf(path), size: file.size };
    }));
    const fresh = results
      .filter((entry): entry is PromiseFulfilledResult<Attachment & { size: number }> => entry.status === "fulfilled")
      .map((entry) => entry.value);
    const failed = results.find((entry) => entry.status === "rejected") as PromiseRejectedResult | undefined;
    // 失败原因（超限/落盘失败）必须可见；没失败再按数量上限给提示，与文件选择器同一套。
    // formatBackendError 与其余错误路径同一口径——String(reason) 会把英文技术串原样上屏。
    setAttachmentNotice(failed
      ? formatBackendError(failed.reason, t.locale)
      : attachmentsRef.current.length + fresh.length > 12 ? t.chat.attachmentLimit(12) : "");
    setAttachments((current) => [...current, ...fresh].slice(0, 12));
  };
  // Esc=拒绝的两段式确认:焦点几乎总在输入框,空框时随手一个 Esc(常见意图是取消聚焦/
  // 收心流)不该直接替 agent 递出一个不可撤销的正式拒绝。第一下只把拒绝按钮点亮成
  // 「再按 Esc 确认」,3s 内第二下才真拒;点按钮拒绝不受影响(鼠标点击是明确意图)。
  const [escArmed, setEscArmed] = useState(false);
  useEffect(() => {
    if (!escArmed) return;
    const timer = window.setTimeout(() => setEscArmed(false), 3_000);
    return () => window.clearTimeout(timer);
  }, [escArmed]);
  // 审批对象换了就撤下确认态:上一张卡上武装的 Esc 不能拒掉下一张。
  useEffect(() => { setEscArmed(false); }, [approval?.requestId, terminalAttention?.id, sessionId]);
  const denyViaEsc = (deny: () => void) => {
    if (!escArmed) { setEscArmed(true); return; }
    setEscArmed(false);
    deny();
  };
  // 文件拖入 = 添加附件(与粘贴/文件选择器同一条流程、同一个 12 上限)。Tauri 拦截了
  // webview 的原生 drop(DOM 拿不到 File),它的 drag-drop 事件反而直接带**源路径**,
  // 无需像粘贴那样先落临时文件。订阅一次(文案经 tRef 取新鲜值)。
  const [dragHover, setDragHover] = useState(false);
  // 终端页的落点动作经 ref 取新鲜闭包(订阅只挂一次);赋值在 composerGated 等门卡算出之后。
  const dropPathsToTerminalRef = useRef<(paths: string[]) => void>(() => {});
  useEffect(() => {
    let un: (() => void) | undefined;
    let cancelled = false;
    // 非 Tauri 环境(测试/浏览器)下 getCurrentWebview 会抛错,吞掉即可(同 setTitle 的写法)。
    let webview: ReturnType<typeof getCurrentWebview>;
    try { webview = getCurrentWebview(); } catch { return; }
    webview.onDragDropEvent((event) => {
      if (event.payload.type === "over") return;
      // 终端页没有附件概念:拖入文件按终端惯例把**路径**打进输入行(含空白的加引号、
      // 多个以空格分隔),不把文件暗挂到对话页的 composer 上。遮罩文案随页面切换。
      if (viewRef.current === "terminal") {
        if (event.payload.type === "drop") {
          setDragHover(false);
          const paths = event.payload.paths ?? [];
          if (paths.length > 0) dropPathsToTerminalRef.current(paths);
          return;
        }
        setDragHover(event.payload.type === "enter");
        return;
      }
      if (event.payload.type === "drop") {
        setDragHover(false);
        const paths = event.payload.paths ?? [];
        if (paths.length === 0) return;
        const fresh = (current: Attachment[]) => {
          const known = new Set(current.map((file) => file.path));
          return paths.filter((path) => !known.has(path)).map(attachmentOf);
        };
        setAttachmentNotice(
          attachmentsRef.current.length + fresh(attachmentsRef.current).length > 12
            ? tRef.current.chat.attachmentLimit(12)
            : "",
        );
        setAttachments((current) => [...current, ...fresh(current)].slice(0, 12));
        return;
      }
      setDragHover(event.payload.type === "enter");
    }).then((fn) => { if (cancelled) fn(); else un = fn; }).catch(() => {});
    return () => { cancelled = true; un?.(); };
  }, []);
  const decideApproval = async (choice: string) => {
    if (!approval || resolvingApproval) return;
    setResolvingApproval(true);
    setBrokerOwnsReview(true);
    try {
      await resolvePendingApproval(sessionId, approval.requestId, choice);
      setApproval(null);
    } catch {
      // 下一次轮询会恢复仍有效的请求；若 hook 已结束则保持消失。
    } finally {
      setResolvingApproval(false);
    }
  };


  // 输入框有没有东西,决定了 composer 右下角那颗圆钮的身份:空 + 回合运行中 = 停止键,
  // 否则 = 发送键。两个判定要用同一份口径,别在 JSX 里各算各的。
  const hasDraft = prompt.trim().length > 0 || attachments.length > 0;
  const canInterrupt = tone === "running" && !!chatUi?.interrupt_input;
  const stopMode = canInterrupt && !hasDraft;

  // 屏幕上有活的选择器/提示时锁住 composer——**禁用而非卸载**:卸载会让 textarea 变成
  // 全新节点(草稿的光标/滚动、聚焦全丢)、整个底栏跳没,而锁的目的只是「别把正文打进
  // 选择器」(文字会过滤选项、回车会提交焦点项,见 sendPrompt 的软拦)。inert 属性经
  // ref 设置:React 18 的属性表还不认识 inert,JSX 直写过不了类型检查。
  const composerLocked = !!terminalAttention;
  // 会话不可直接对话(外部终端占用/已结束)时,composer 让位给「先接管/先恢复」门卡:
  // 输入框摆着也发不出去,placeholder 引导远不如把唯一可行的动作直接放在手边(用户反馈)。
  // ptyManaged 用**严格 === false**:它是 DTO 必填字段,真实后端总有值;宁可在信号缺失时
  // 退回旧路径(发送失败再给接管条),也不能把一个其实能对话的会话锁在门外。
  // background 例外:它没有终端可接管,composer 是唯一出口(发送时后端另有拒绝话术)。
  const composerGated = !!history && history.supported && !history.background
    && history.ptyManaged === false && (externalRunning || history.status === "ended");
  // 已被跨 provider 切换接替的会话：只读回看。禁发优先于其它门卡——「恢复会话」对它
  // 是错误动作（会让接续链分叉，store 层也会拒绝再次接替），唯一正确的出口是去链尾。
  const supersededTo = history?.supersededBy ?? null;
  // 终端页拖入文件 → 路径写进托管 PTY。后台/外部占用/已结束/已被接替的会话没有可写的
  // 终端,静默忽略(composer 同样被门卡锁着,此处不另起提示)。写失败走发送错误条,
  // 不许「拖了没反应」。
  dropPathsToTerminalRef.current = (paths) => {
    if (!history || history.background || composerGated || supersededTo) return;
    const text = paths.map((path) => (/\s/.test(path) ? `"${path}"` : path)).join(" ");
    writeManagedTerminal(sessionId, text).catch((e) => setSendError(formatBackendError(e, t.locale)));
  };
  // /clear 换代自动跟随：正开着的会话在眼前被同一 PTY 的新段接替（supersededBy 由空变有）
  // 时直接跳到新段——终端进程还是同一个，留在旧段只剩定格画面。冷打开旧段回看时首帧就带
  // supersededBy，不构成「由空变有」，不跳；门卡的「去链尾」仍是回看的出口。
  const supersededWatchRef = useRef({ sid: 0, had: false });
  useEffect(() => {
    if (!history || sessionId <= 0) return;
    const watch = supersededWatchRef.current;
    const had = history.supersededBy != null;
    if (watch.sid !== sessionId) {
      supersededWatchRef.current = { sid: sessionId, had };
      return;
    }
    if (!watch.had && history.supersededBy != null) resetTo(history.supersededBy);
    watch.had = had;
  }, [sessionId, history, resetTo]);
  // 门卡上的「恢复会话」:只拉起托管终端(withSendGuard 里的 ensureWritableTerminal),
  // 不发任何内容;成功后轮询把 ptyManaged 翻真,门卡自动让位给 composer。
  const resumeForChat = () => void withSendGuard(async () => true);
  const composeRef = useRef<HTMLElement>(null);
  useEffect(() => {
    const el = composeRef.current;
    if (!el) return;
    if (composerLocked) el.setAttribute("inert", "");
    else el.removeAttribute("inert");
  }, [composerLocked, view]);

  // 审批卡门控：插件声明了详情文法风格（details）的提示才走命令审批卡——不再枚举 pattern id。
  const commandAttention = terminalAttention && (terminalAttention.details === "proceed_box" || terminalAttention.details === "arrow_panel") ? terminalAttention : null;
  const interactiveAttention = terminalAttention?.id === "interactive:numbered-selector" ? terminalAttention : null;
  // 屏幕识别就绪 → 可作答的交互选择器接管，结构化题面的展示卡退场（同一份题面不双卡）。
  useEffect(() => {
    if (interactiveAttention) setStructuredQuestion(null);
  }, [interactiveAttention]);
  const structuredQuestions = useMemo(
    () => (structuredQuestion ? parseAskUserQuestions(structuredQuestion.input) : []),
    [structuredQuestion],
  );
  // 卡内点选作答的适用形态：单问题 + 单选。理由见卡片渲染处的注释（单值 queuedAnswer
  // 装不下多选，且跨问题的同名选项会把答案落到错的题上）。
  const answerableInCard =
    structuredQuestions.length === 1 && !structuredQuestions[0].multiSelect;
  // 作答卡（broker 挂起代答）：题面来自挂起路径且会话托管时，卡片就是作答面——
  // 多选/多问题/自定义输入全部可答，不再依赖屏幕识别与排队落键。
  const questionAnswerable = !!structuredQuestion?.answerable && history?.ptyManaged !== false;
  // 作答卡的逐题草稿操作：单选点即换（再点取消），多选勾选切换。
  const selectQuestionOption = (questionIndex: number, label: string) => {
    const multi = structuredQuestions[questionIndex]?.multiSelect === true;
    setQuestionAnswers((current) => {
      const next = new Map(current);
      const draft = next.get(questionIndex) ?? { selected: [], custom: "" };
      const selected = draft.selected.includes(label)
        ? draft.selected.filter((entry) => entry !== label)
        : multi ? [...draft.selected, label] : [label];
      next.set(questionIndex, { ...draft, selected });
      return next;
    });
  };
  const setQuestionCustom = (questionIndex: number, text: string) => {
    setQuestionAnswers((current) => {
      const next = new Map(current);
      const draft = next.get(questionIndex) ?? { selected: [], custom: "" };
      next.set(questionIndex, { ...draft, custom: text });
      return next;
    });
  };
  // 每题都有内容（点选或自定义）才可提交；正文即 answer: 的 payload。
  const answerBody = composeAnswerBody(structuredQuestions, questionAnswers);
  const unansweredCount = structuredQuestions.filter((_, index) => {
    const draft = questionAnswers.get(index);
    return !(draft && (draft.selected.length > 0 || draft.custom.trim()));
  }).length;
  // 提交：正文由这里拼，引导语由 broker 统一包上 → DenyWith 回 hook，agent 直接继续。
  // 复用 resolvingApproval 在途锁（与审批同类的终止动作）。
  const submitQuestionAnswers = async () => {
    if (!questionAnswerable || !structuredQuestion || resolvingApproval || !answerBody) return;
    setResolvingApproval(true);
    try {
      await resolvePendingApproval(sessionId, structuredQuestion.requestId, `answer:${answerBody}`);
      setStructuredQuestion(null);
    } catch (error) {
      // 挂起可能已在别处结算（超时/回合终止）：说明原因，卡随下一拍轮询降级或消失。
      setSendError(formatBackendError(error, t.locale));
    } finally {
      setResolvingApproval(false);
    }
  };
  // 作答卡的「去终端作答」：把挂起交还终端（broker 回 Pass → 表单随权限流程出现在
  // 终端），切到终端页等它。
  const sendQuestionToTerminal = async () => {
    if (!questionAnswerable || !structuredQuestion || resolvingApproval) return;
    setResolvingApproval(true);
    try {
      await resolvePendingApproval(sessionId, structuredQuestion.requestId, "pass");
    } catch {
      // 已在别处结算：表单要么已在终端，要么请求已了结，切过去看即可。
    } finally {
      setResolvingApproval(false);
    }
    setView("terminal");
  };
  const commandApproval = commandAttention
    ? commandAttention.details === "arrow_panel"
      ? arrowPanelApprovalDetails(commandAttention.text)
      : proceedBoxApprovalDetails(commandAttention.text)
    : null;
  const commandOptions = commandAttention?.options ?? [];
  // claude 的选项文案是 Yes/No，kimi 是 Approve once/Reject——两家的按钮语义同一套。
  // 拒绝项两轮匹配:先找裸文案(kimi 的 "Reject" 不能被 "Reject with feedback" 抢注),
  // 找不到再按词首前缀兜底——claude 的长文案拒绝项("No, and tell Claude what to do
  // differently (esc)")只有前缀能认;全等匹配会让它漏网,被下面的 commandRemember
  // (「既非拒绝也非允许的第一项」)吸收,点「允许并记住」实际发出的是拒绝按键。
  const commandDeny = commandOptions.find((option) => /^(?:no|reject)$/i.test(option.label.trim()))
    ?? commandOptions.find((option) => /^(?:no|reject)\b/i.test(option.label.trim()));
  const commandAllowOnce = commandOptions.find((option) => /^(?:yes|approve once|approve)$/i.test(option.label.trim())) ?? commandOptions[0];
  const commandRemember = commandOptions.find((option) => option !== commandDeny && option !== commandAllowOnce);
  // 屏幕菜单/审批「一次性作答」的在途去重(对应 broker 审批的 resolvingApproval):写 PTY
  // 到卡片随 .then 收起之间有窗口,期间极快双击会把 option.input 落键两次——命令审批场景
  // 等于替用户答两次。只锁**会收卡的终止动作**,多选勾选(卡片留驻)的连点不受影响。
  const optionResolvingRef = useRef(false);
  const chooseTerminalOption = (option: { input: string } | undefined) => {
    if (!option || optionResolvingRef.current) return;
    // 选完这次菜单就结束了：关掉识别窗口，否则按钮会一直停在「收起」态。
    // 判据取 attention 本身的类型而不是识别窗口是否还开着——窗口会到点自灭，
    // 用户慢慢选的话就漏掉刷新了。
    const wasModelMenu = terminalAttention?.id === "interactive:cursor-menu";
    setMenuWatchUntil(0);
    optionResolvingRef.current = true;
    void writeManagedTerminal(sessionId, option.input)
      .then(() => {
        setTerminalAttention(null);
        // 模型平时由 Stop hook 落库，而 `/model` 切换不产生 Stop——不主动刷一次的话，
        // 对话页和贴纸会一直挂着旧模型直到下一条消息跑完。CLI 要一会儿才把新模型写进
        // 会话日志，故稍等再读。
        if (wasModelMenu) window.setTimeout(() => void refreshSessionModel(sessionId).catch(() => {}), 600);
      })
      .catch((error) => setSendError(formatBackendError(error, t.locale)))
      .finally(() => { optionResolvingRef.current = false; });
  };
  // Esc = 拒绝本次请求。审批卡上的 kbd 徽章得说话算话,键在这里真绑上。
  // 只绑这一个安全方向:Enter→允许没绑,也不打算绑——输入框外随手一个回车就放行一条
  // 命令,换不来那点快捷。焦点在输入框/终端里时让开,那里的 Esc 另有主人（收补全菜单、
  // 写进 PTY）。无依赖数组是有意的:每次渲染重绑一个 keydown,换闭包永远新鲜。
  useEffect(() => {
    const terminalDeny = view === "chat" && commandAttention && commandApproval ? commandDeny : undefined;
    const brokerDeny = view === "chat" && !terminalAttention && approval ? approval : undefined;
    if (!terminalDeny && !brokerDeny) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      // 层栈让位（escLayers.ts）：任何浮层（菜单/弹层/灯箱/切换器/补全）开着时，
      // 这次 Esc 属于它们——统一判据，不再依赖各浮层「记得」用哪种截停约定。
      if (hasEscLayers()) return;
      const active = document.activeElement;
      if (active instanceof HTMLElement && (active.tagName === "TEXTAREA" || active.tagName === "INPUT" || active.isContentEditable)) return;
      event.preventDefault();
      // 两段式:第一下点亮确认,第二下才真拒(见 denyViaEsc)。
      if (terminalDeny) denyViaEsc(() => chooseTerminalOption(terminalDeny));
      else denyViaEsc(() => void decideApproval("deny"));
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const chooseInteractiveOption = (option: TerminalAttentionOption) => {
    if (!option.input) return;
    // 终止动作在途时不再接受点选(见 optionResolvingRef):多选勾选不置位,故连勾不受影响。
    if (optionResolvingRef.current) return;
    if (option.kind === "choice") {
      setTerminalAttention((current) => current?.id !== "interactive:numbered-selector" ? current : {
        ...current,
        options: current.options?.map((entry) => {
          const position = entry.position ?? 0;
          const target = option.position ?? 0;
          const delta = position - target;
          return {
            ...entry,
            selected: entry.position === option.position ? !entry.selected : entry.selected,
            focused: entry.position === option.position,
            input: delta < 0 ? "\x1b[A".repeat(-delta) + "\r" : "\x1b[B".repeat(delta) + "\r",
          };
        }),
      });
    }
    // 单选形态(无独立 submit 项,回车即提交)下 choice 落键就是作答完成:立即收卡。
    // 原来卡片停留到下一轮屏幕识别才消失,期间再点一下,第二串「方向键+回车」会打进
    // 已经翻页的终端画面,落点不可控。多选(有 submit 项)保持可交互,提交仍归用户。
    const soleChoice = option.kind === "choice"
      && !terminalAttentionRef.current?.options?.some((entry) => entry.kind === "submit");
    // 收卡的终止动作(提交/直答/单选即答)才上在途锁;多选勾选留驻,连勾自由。
    const terminates = option.kind === "submit" || option.kind === "chat" || soleChoice;
    if (terminates) optionResolvingRef.current = true;
    void writeManagedTerminal(sessionId, option.input)
      .then(() => { if (terminates) setTerminalAttention(null); })
      .catch((error) => setSendError(formatBackendError(error, t.locale)))
      .finally(() => { if (terminates) optionResolvingRef.current = false; });
  };
  // 排队作答的落键时刻：屏幕识别接管（表单确认在屏）后，把题面卡上点选的答案自动写进
  // 表单。choice 的 input 是「方向键定位 + 回车」——单选即完成作答，多选是勾选该项、
  // 提交仍归用户。匹配不上（截断歧义/多问题表单）就保持交互卡让用户手点，绝不猜。
  useEffect(() => {
    if (!interactiveAttention || !queuedAnswer) return;
    const choices = (interactiveAttention.options ?? []).filter((option) => option.kind === "choice");
    const match = matchOptionByLabel(choices, queuedAnswer);
    // 匹配不上就**保留**排队状态：清掉的话用户的点击被静默吞掉，提示还从「已选…」
    // 变回「点选答案…」，什么解释都没有。留着等下一帧识别（表单可能还在渲染中），
    // 收卡或手动取消时自然清空。
    if (!match) return;
    setQueuedAnswer(null);
    chooseInteractiveOption(match);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- chooseInteractiveOption 每次渲染新建，按值依赖会空转
  }, [interactiveAttention, queuedAnswer]);

  const submitCustomAnswer = (option: TerminalAttentionOption) => {
    const value = questionCustomText.trim();
    if (!value || !option.input) return;
    void writeManagedTerminal(sessionId, option.input + value + "\r")
      .then(() => setQuestionCustomText(""))
      .catch((error) => setSendError(formatBackendError(error, t.locale)));
  };

  // 中途附加目录(diff 面板的仓菜单):命令统一负责落库(sessions.extra_dirs,resume
  // 回放依据)与即时生效(有托管 PTY 时后端直写 /add-dir)。断开会话只落库,恢复回放
  // 兜底。useCallback:props 进 memo 的 GitDiffView,引用不稳会让 memo 白 memo。
  const addExtraDir = useCallback(async () => {
    try {
      await pickAndAddExtraDir(sessionId);
    } catch (error) {
      setSendError(formatBackendError(error, t.locale));
    }
  }, [sessionId, t.locale]);
  // 移除附加目录(落库侧,下次恢复不再带上;运行中进程已持有的权限收不回)。
  const removeExtraDir = useCallback((dir: string) => {
    removeSessionExtraDir(sessionId, dir).catch((error) => setSendError(formatBackendError(error, t.locale)));
  }, [sessionId, t.locale]);
  // 附加动作的能力位包装:memo 稳定引用(内联三元每渲染换新)。
  const onDiffAddDir = useMemo(
    () => (currentAgentDescriptor?.supports_extra_dirs ? () => void addExtraDir() : null),
    [currentAgentDescriptor?.supports_extra_dirs, addExtraDir]
  );
  // 激活仓被移除后 diffDir 悬空(指着已不在清单里的目录):回落主仓。
  useEffect(() => {
    if (diffDir !== null && !diffDirs.includes(diffDir)) setDiffDir(null);
  }, [diffDir, diffDirs]);

  // 侧栏折叠/展开的统一开关（Kimi 式面板图标，两态共用一个钮）。Windows/Linux 常驻
  // 顶栏左上角；macOS 没有独立顶栏，折叠钮在侧栏头部（ChatSidebar 内，仅 mac 渲染）、
  // 折叠后的展开钮进标题栏行。
  const sidebarToggleButton = (
    <button
      type="button"
      className="chat-sidebar-open"
      aria-label={sidebarVisible ? t.chat.sidebarCollapse : t.chat.sidebarExpand}
      data-tip={sidebarVisible ? t.chat.sidebarCollapse : t.chat.sidebarExpand}
      aria-expanded={sidebarVisible}
      onClick={toggleSidebar}
    >
      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinejoin="round">
        <rect x="3.5" y="4.5" width="17" height="15" rx="3" />
        <path d="M9.5 4.5v15" />
      </svg>
    </button>
  );

  return (
    <div className={"chat-window" + (view === "terminal" ? " is-terminal" : "")}>
      <DevBadge />
      {/* 拖拽落点遮罩:文件悬在窗口上时给「松开以添加附件」的明确反馈。 */}
      {dragHover && (
        <div className="chat-drop-overlay" aria-hidden="true">
          <span>{view === "terminal" ? t.chat.dropToTerminal : t.chat.dropToAttach}</span>
        </div>
      )}
      {/* Windows/Linux：独立顶栏（拖拽区 + 标准 − □ × 控制组），会话标题分离到下方
          .chat-bar 标题行。macOS 不渲染此行——红绿灯嵌在 52px 标题栏行内
          （unify_titlebar_toolbar 垂直居中），布局保持原样不动。 */}
      {!isMac() && (
        <header className="chat-topbar" data-tauri-drag-region>
          {sidebarToggleButton}
          {/* 最小化/最大化/关闭是桌面窗口能力,手机浏览器没有可控窗口——远程隐藏。 */}
          {!remoteUi() && <WindowControls onClose={close} />}
        </header>
      )}
      {/* 窄窗浮窗形态（Kimi 式全高抽屉）：挂在窗口层级、从最顶盖到最底——锚在 chat-body
          会被顶栏截一刀（实拍反馈「被分割」）。scrim 点外关；抽屉自带头部行放开关钮，
          与顶栏同高同位，视觉上按钮原地不动、抽屉从它底下展开；选中会话即收（抽屉惯例）。 */}
      {narrow && overlaySidebar && (
        <>
          <div className="chat-sidebar-scrim" onClick={() => setOverlaySidebar(false)} />
          <div className="chat-sidebar-overlay">
            <div className="chat-sidebar-overlay-head">{sidebarToggleButton}</div>
            <ChatSidebar
              activeId={sessionId}
              approvalAwaitingIds={approvalAwaitingIds}
              visibleOrderRef={sidebarOrderRef}
              onSelect={selectFromOverlay}
              onCollapse={closeOverlaySidebar}
            />
          </div>
        </>
      )}
      <div className={"chat-body" + (diffOpen && diffMaximized && !diffCollapsed ? " is-diff-max" : "")}>
      {!narrow && !sidebarCollapsed && <ChatSidebar
        activeId={sessionId}
        approvalAwaitingIds={approvalAwaitingIds}
        visibleOrderRef={sidebarOrderRef}
        onSelect={resetTo}
        onCollapse={collapseSidebar}
      />}
      <div className="chat-main">
      <header className="chat-bar" data-tauri-drag-region>
        {isMac() && !sidebarVisible && sidebarToggleButton}
        {history ? (
          <ChatTitleMenu
            title={history.title || t.chat.title}
            cwd={history.cwd}
            archived={history.archived}
            archiving={archiving}
            starred={!!history.ccSessionId && starredSet.has(history.ccSessionId)}
            onToggleStar={toggleStar}
            onRename={() => setRenaming(true)}
            onToggleArchived={() => void toggleArchived()}
            t={t}
          />
        ) : (
          /* 历史未回时没有可操作的会话，标题降级为纯文本占位（仍是拖拽区）。 */
          <div className="chat-title-menu" data-tauri-drag-region>
            <span className="chat-title" data-tauri-drag-region>{t.chat.title}</span>
          </div>
        )}
        {/* 标题栏刻意不放运行状态徽标（实拍反馈嫌吵）：运行态已有多处冗余信号——
            窗口标题的 ▶ 标记、对话区底部的脉冲指示条、侧栏状态点。 */}
        {/* 任务进度入口:常驻图标,点开浮出「进度」面板(无任务时是骨架占位空态)。 */}
        <ChatTodoMenu todos={todos} subagents={panelSubagents} t={t} />
        {/* 文件/改动面板入口:有 cwd 即显示(非仓库会话只有「文件」页签),徽标数 = 变更文件数。
            远程 v1 砍掉改动面板(GitDiffView),入口一并隐藏。 */}
        {!remoteUi() && cwd && (
          <button
            type="button"
            // 面板可见时亮 is-active:点击语义是「首开/折叠↔展开」,没有状态指示的话
            // 用户无法预判这一下是开还是收(折叠态与「没开过」在视觉上无差)。
            className={"chat-todo-btn chat-diff-btn" + (diffOpen && !diffCollapsed ? " is-active" : "")}
            data-tip={t.chat.diff}
            aria-label={t.chat.diff}
            aria-pressed={diffOpen && !diffCollapsed}
            onClick={() => {
              // 首次点击挂载并展开；之后折叠↔展开切换，不再卸载（保留面板内状态）。
              if (!diffOpen) setDiffOpen(true);
              else setDiffCollapsed((collapsed) => !collapsed);
            }}
          >
            {/* git-diff 语义:左右分叉的对比图标。 */}
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <circle cx="6" cy="6" r="2.5" /><circle cx="6" cy="18" r="2.5" /><circle cx="18" cy="9" r="2.5" />
              <path d="M6 8.5v7" /><path d="M18 11.5c0 4-4 4.5-9.2 4.6" />
            </svg>
            {gitSummary && gitSummary.isRepo && gitSummary.files.length > 0 && <span className="chat-diff-btn-count">{gitSummary.files.length}</span>}
          </button>
        )}
        {/* stop_managed_terminal 是 /rpc 明确拒绝项(宿主进程生杀),远程按钮点了必 404——隐藏。 */}
        {!remoteUi() && history?.ptyManaged && (
          <button type="button" className="chat-end" disabled={endingSession} onClick={() => void endSession()}>
            {endingSession ? t.chat.terminalStopping : t.chat.endSession}
          </button>
        )}
        {/* 终端页签在远程无意义(见 setView 说明):只留对话页,不给死按钮。 */}
        {!remoteUi() && (
          <div className="chat-view-tabs">
            <button type="button" className={view === "chat" ? "is-active" : ""} aria-pressed={view === "chat"} onClick={() => { rememberViewPref("chat"); setView("chat"); }}>{t.chat.conversation}</button>
            <button type="button" className={view === "terminal" ? "is-active" : ""} aria-pressed={view === "terminal"} onClick={() => { rememberViewPref("terminal"); setView("terminal"); }}>{t.chat.terminal}</button>
          </div>
        )}
      </header>
      {/* 两个视图都留在树上、用 CSS 切换可见性：此前三元表达式会在每次切 tab 时
          dispose + new Terminal()，还要把整个 backlog 重传并让 xterm 重放一遍 ANSI。
          终端侧懒挂载（terminalMounted），纯对话的用户不用白付 xterm 的创建成本。 */}
      <main className="chat-scroll" ref={scrollRef} onScroll={onScroll} hidden={view !== "chat"}>
        {/* 同步失败降级：已有内容时只挂顶部细横幅继续渲染缓存的对话——650ms 一发的轮询，
            一次瞬时 IPC 抖动就整屏换错误行，比不提示更吓人。整屏错误只留给「一条都没有」。 */}
        {failed && items.length > 0 && (
          <div className="chat-sync-warn" role="status">{t.chat.loadError}</div>
        )}
        {sessionId === 0 ? <div className="chat-empty">{t.chat.pickSession}</div>
          : loading ? <div className="chat-empty">{t.chat.loading}</div>
          : failed && items.length === 0 ? <div className="chat-empty is-error">{t.chat.loadError}</div>
          : history && !history.supported ? (
            /* 不提供结构化 transcript 的 agent：hook 落库的最近往来仍然是真实数据，先渲染它，
               「暂未提供结构化对话记录」降为其下的注脚——有什么就展示什么，而不是只报没有。 */
            provisional.length > 0
              ? <><Transcript sessionId={sessionId} items={provisional} /><div className="chat-empty is-note">{t.chat.unsupported}</div></>
              : <div className="chat-empty">{t.chat.unsupported}</div>
          )
          /* 空列表分两种事实：会话已在跑（transcript 尚未落第一条/尚未定位到）≠ 真的没有记录。
             hook 侧已知的最近往来（lastUserText / lastAiText）先顶上；连它也没有时，
             running 态也不能说「没有内容」——那与下面的运行指示互相打架。
             接续会话只看 timelineItems：当前段还没落字时，前序段历史已经撑起时间线。 */
          : timelineItems.length === 0 ? (
            provisional.length > 0
              ? <Transcript sessionId={sessionId} items={provisional} />
              /* 负数临时 id = 新会话认领前，CLI 正在冷启动：此时说「没有记录」是谎报。 */
              : <div className="chat-empty">{sessionId < 0
                  ? <div>
                      <div>{t.chat.emptyStarting}</div>
                      {/* 远程没有终端视图(setView 被无害化),不给死按钮。 */}
                      {startingSlow && !remoteUi() && (
                        /* 目的性跳转，不写视图偏好。 */
                        <button type="button" className="chat-empty-cta" onClick={() => setView("terminal")}>{t.chat.openTerminal}</button>
                      )}
                    </div>
                  : tone === "running" ? t.chat.emptyWorking : t.chat.empty}</div>
          )
          : <>
            {hasEarlier && (
              <div className="chat-load-earlier">
                <button type="button" onClick={() => void loadEarlier()} disabled={loadingEarlier}>
                  {loadingEarlier ? t.chat.loadingEarlier : t.chat.loadEarlier}
                </button>
              </div>
            )}
            {/* 跨 provider 接续的新会话：前序历史没取到时的来历注脚（取到后由段间
                分隔条表达同一事实，见 timelineItems）。 */}
            {showHandoffNote && !hasEarlier && (
              <div className="chat-empty is-note">{t.chat.handoffContinued}</div>
            )}
            <Transcript sessionId={sessionId} items={timelineItems} />
            {/* 上翻后回到底部的悬浮出口：sticky 钉在滚动视口底缘，贴底时不渲染。 */}
            {!atBottom && (
              <button type="button" className="chat-jump-latest" onClick={jumpToLatest}>
                <ChevronDownIcon />{t.chat.jumpLatest}
              </button>
            )}
          </>}
        {/* Agent 自己维护的待办清单。它不属于时间线（会被反复整份改写），故固定在底部
            而不是插进消息流里——否则每改一次待办就多一条历史。 */}
        {!loading && todos.length > 0 && <TodoPanel todos={todos} />}
      </main>
      {/* Agent 正在跑但 transcript 半天不落新行时，页面此前毫无动静，像卡死。
          运行条原在滚动流内、Transcript 之后——上翻历史就随内容滚出视口。移到 main 与
          compose 之间常驻:滚到哪里都看得见,有具体活动（最近命令）就显示出来。 */}
      {view === "chat" && !loading && tone === "running" && (
        <div className="chat-running" role="status">
          {/* 长命令被 CSS 截断成单行，全文放原生 title——data-tip 的浮层对一个高频变化的
              状态条太吵，原生悬停提示刚好。展示文本剥掉 cd 前缀（见 trimActivityCdPrefix）。 */}
          <i /><span data-tip={history?.currentActivity || undefined}>
            {history?.currentActivity ? trimActivityCdPrefix(history.currentActivity) : t.chat.running}
          </span>
        </div>
      )}
      {/* 排队回执:运行中发出的插话被 CLI 排队到回合结束,期间 transcript 不显示——
          没有这条回执,消息在 GUI 上像消失了。中断键有声明时给「立即插话」出口;
          逐条列出并可单独移除(仅清本 GUI 的记账,消息撤不回,见 dismissQueued)。 */}
      {view === "chat" && !loading && tone === "running" && queuedInterjections.length > 0 && (
        <div className="chat-queued" role="status">
          <div className="chat-queued-head">
            <span>{t.chat.queuedInterjections(queuedInterjections.length)}</span>
            {chatUi?.interrupt_input && (
              <button type="button" data-tip={t.chat.interjectNowTip} disabled={interrupting} onClick={sendInterrupt}>
                {interrupting ? t.chat.interrupting : t.chat.interjectNow}
              </button>
            )}
          </div>
          <ul className="chat-queued-list">
            {queuedInterjections.map((item) => (
              <li key={item.id}>
                <span>{item.text}</span>
                <button
                  type="button"
                  className="chat-queued-dismiss"
                  aria-label={t.chat.queuedDismiss}
                  data-tip={t.chat.queuedDismissTip}
                  onClick={() => dismissQueued(item.id)}
                >×</button>
              </li>
            ))}
          </ul>
        </div>
      )}
      {/* 归档撤销条:归档动作本身无确认,跳转又可能把人带去别的会话——8s 内一键撤销
          并跳回,不必去筛选菜单里翻「已归档」。 */}
      {archiveUndo != null && (
        <div className="chat-send-error chat-archive-undo" role="status">
          <span>{t.sticker.archivedNotice}</span>
          <button type="button" className="chat-send-takeover" onClick={() => void undoArchive(archiveUndo)}>{t.sticker.archiveUndo}</button>
          <button type="button" className="chat-send-takeover" onClick={() => setArchiveUndo(null)}>{t.chat.slashMenuDismiss}</button>
        </div>
      )}
      {/* ── 审批/交互卡:统一走 ApprovalCard 外壳(标题/徽章/正文/左右动作组一套视觉),
          同屏最多一张——交互选择器、命令审批、其他屏幕提示、broker 审批四种来源互斥。
          「仅收起」不向 PTY 写任何字节:识别是启发式的,误报/过期的卡必须有不产生
          副作用的出口(同屏有签名去重不会复弹;真提示仍在终端页等)。 ── */}
      {view === "chat" && interactiveAttention && <ApprovalCard
        className="chat-screen-approval"
        title={history?.pendingReview === "plan" ? t.chat.planTitle : t.chat.questionTitle}
        badge={history?.pendingReview === "plan" ? t.chat.approvalPending : t.chat.questionPending}
        sideActions={<button type="button" className="chat-attention-dismiss is-inline" data-tip={t.chat.attentionDismissTip} onClick={() => setTerminalAttention(null)}>{t.chat.attentionDismiss}</button>}
        actions={<>
          {interactiveAttention.options?.filter((option) => option.kind === "chat").map((option) => (
            <button type="button" disabled={!option.input} key={`${option.position}:${option.label}`} onClick={() => chooseInteractiveOption(option)}>{t.chat.chatAboutThis}</button>
          ))}
          {interactiveAttention.options?.filter((option) => option.kind === "submit").map((option) => (
            <button type="button" disabled={!option.input} className="is-allow" key={`${option.position}:${option.label}`} onClick={() => chooseInteractiveOption(option)}>{t.chat.submitAnswer}</button>
          ))}
        </>}
      >
        {interactiveAttention.text && <span className="chat-approval-prewrap">{interactiveAttention.text}</span>}
        <div className="chat-approval-options">
          {interactiveAttention.options?.filter((option) => option.kind === "choice").map((option) => (
            <button type="button" disabled={!option.input} className={option.selected ? "is-selected" : ""} key={`${option.position}:${option.label}`} onClick={() => chooseInteractiveOption(option)}>
              <i aria-hidden="true">{option.selected ? "✓" : ""}</i>
              <span><b>{option.label}</b>{option.description && <small>{option.description}</small>}</span>
            </button>
          ))}
        </div>
        {interactiveAttention.options?.filter((option) => option.kind === "input").map((option) => (
          <div className="chat-approval-custom" key={option.input}>
            <input value={questionCustomText} onChange={(event) => setQuestionCustomText(event.target.value)} placeholder={t.chat.customAnswerPlaceholder}
              onKeyDown={(event) => { if (event.key === "Enter") submitCustomAnswer(option); }} />
            <button type="button" disabled={!questionCustomText.trim() || !option.input} onClick={() => submitCustomAnswer(option)}>{t.chat.addCustomAnswer}</button>
          </div>
        ))}
      </ApprovalCard>}
      {view === "chat" && commandAttention && commandApproval && <ApprovalCard
        className="chat-screen-approval"
        title={t.chat.approvalTitle}
        badge={t.chat.approvalPending}
        sideActions={<>
          <button type="button" className="chat-attention-dismiss is-inline" data-tip={t.chat.attentionDismissTip} onClick={() => setTerminalAttention(null)}>{t.chat.attentionDismiss}</button>
          {commandRemember && <button type="button" className="is-persistent" onClick={() => chooseTerminalOption(commandRemember)}>
            {/* arrow_panel 的「Approve for this session」只记本会话，proceed_box 的记住是持久规则——文案不能混。 */}
            {commandAttention.details === "arrow_panel"
              ? t.chat.allowSession
              : `${t.chat.allowRemember}${commandRemember.label.match(/for:\s*(.+)$/i)?.[1] ? ` · ${commandRemember.label.match(/for:\s*(.+)$/i)?.[1]}` : ""}`}
          </button>}
        </>}
        actions={<>
          {commandDeny && <button type="button" className={"is-deny" + (escArmed ? " is-esc-armed" : "")} onClick={() => chooseTerminalOption(commandDeny)}>
            {escArmed ? t.chat.denyConfirmEsc : t.chat.deny}<kbd aria-hidden="true">Esc</kbd>
          </button>}
          {commandAllowOnce && <button type="button" className="is-allow" onClick={() => chooseTerminalOption(commandAllowOnce)}>{t.chat.allowOnce}</button>}
        </>}
      >
        {commandApproval.tool && <div className="chat-approval-tool"><span>{t.chat.approvalTool}</span><code>{commandApproval.tool}</code></div>}
        {commandApproval.description && <span>{commandApproval.description}</span>}
        {commandApproval.question && <span>{commandApproval.question}</span>}
        {commandApproval.command && <div className="chat-approval-detail">
          <span>{t.chat.approvalInput}</span>
          <pre>{commandApproval.command}</pre>
        </div>}
      </ApprovalCard>}
      {view === "chat" && terminalAttention && !commandAttention && !interactiveAttention && <ApprovalCard
        className="chat-screen-approval"
        title={terminalAttention.id === "claude:long-session-resume"
          ? t.chat.longSessionPromptTitle
          : terminalAttention.id === "claude:command-approval"
            ? t.chat.approvalTitle
          : terminalAttention.id === "claude:plan-approval"
            ? t.chat.planTitle
          : terminalAttention.options?.length && terminalAttention.id.startsWith("provider:")
            ? t.chat.trustPromptTitle
            : t.chat.terminalPromptTitle}
        badge={t.chat.approvalPending}
        sideActions={<button type="button" className="chat-attention-dismiss is-inline" data-tip={t.chat.attentionDismissTip} onClick={() => setTerminalAttention(null)}>{t.chat.attentionDismiss}</button>}
        actions={!terminalAttention.options?.length && <>
          {/* 取消发 Esc——在 Claude 里会打断正在跑的回合,与零副作用的「仅收起」是两回事。
              这个后果必须在 tip 里对用户说出来,不能只写在注释里给维护者看。 */}
          <button type="button" data-tip={t.chat.terminalPromptCancelTip} onClick={() => {
            void writeManagedTerminal(sessionId, "\x1b")
              .then(() => setTerminalAttention(null))
              .catch((error) => setSendError(formatBackendError(error, t.locale)));
          }}>{t.chat.terminalPromptCancel}</button>
          <button type="button" className="is-allow" onClick={() => {
            void writeManagedTerminal(sessionId, "\r")
              .then(() => setTerminalAttention(null))
              .catch((error) => setSendError(formatBackendError(error, t.locale)));
          }}>{t.chat.terminalPromptConfirm}</button>
        </>}
      >
        {terminalAttention.id === "claude:long-session-resume"
          ? <span>{t.chat.longSessionPromptHelp}</span>
          : terminalAttention.id === "claude:command-approval"
            ? <pre>{terminalAttention.text}</pre>
          : !terminalAttention.options?.length && <>
            <span>{t.chat.terminalPromptHelp}</span>
            <pre>{terminalAttention.text}</pre>
          </>}
        {(terminalAttention.options?.length ?? 0) > 0 && <div className="chat-approval-options">
          {terminalAttention.options!.map((option, index) => (
            // 走同一个 chooseTerminalOption：它还负责关掉菜单识别窗口、并在模型菜单
            // 选完后主动刷新模型（`/model` 切换不产生 Stop hook，不刷就一直显示旧值）。
            <button type="button" className={index === 0 ? "is-primary" : ""} key={`${index}:${option.label}`} onClick={() => chooseTerminalOption(option)}>
              <span><b>{option.label}</b></span>
            </button>
          ))}
        </div>}
      </ApprovalCard>}
      {terminalMounted && (
        <div className={`chat-terminal-pane${view !== "terminal" ? " is-background" : ""}`} aria-hidden={view !== "terminal"}>
          {/* broker 审批横幅——只给「hook 阻塞期间 TUI 不显示权限框」的 provider 挂
              （permission_prompt_races_hook=false，如 codex/未取证者）：那种终端视图下
              用户什么都看不到，agent 干等到 300s 超时，须领回对话页。claude 相反：官方
              hooks 文档明载 TUI 权限框与 hook 并行竞速（实测同现），授权框就在眼前，
              横幅只会误导，不挂。屏幕识别类提示（TUI 上真有表单的）同理不挂。 */}
          {view === "terminal" && approval && !chatUi?.permission_prompt_races_hook && (
            <div className="chat-terminal-approval" role="status">
              <span>{t.chat.terminalApprovalBanner}{approvalCountdown ? ` · ${approvalCountdown}` : ""}</span>
              <button type="button" onClick={() => setView("chat")}>{t.chat.terminalApprovalGo}</button>
            </div>
          )}
          <ManagedTerminal
            key={sessionId}
            sessionId={sessionId}
            status={history?.status}
            // 停滞检测豁免:broker 审批/hook 落库的待批/屏幕识别的表单/结构化题面,任一
            // 在场都说明 TUI 静止是「在等人」——回合没结束 status 仍 running,不能报僵死。
            reviewPending={Boolean(approval || history?.pendingReview || terminalAttention || structuredQuestion)}
            visible={view === "terminal"}
            background={history?.background ?? false}
            resumeOptions={resumeOptions}
            takeoverExtra={resumePermissionPicker}
            // 后台会话的按键注定无效。第一下就把用户领到对话页——那里的输入框走 daemon 的
            // 送话通道，是真能用的。原先的做法是让他打完一整句再弹错误，等于白打。
            onBackgroundInput={() => {
              setView("chat");
              setSendError(t.chat.sendBackgroundKeysMovedYou);
            }}
            attentionMarkers={chatUi?.startup_attention_markers ?? EMPTY_MARKERS}
            interactivePrompt={terminalInteractivePrompt}
            expectMenu={menuWatching}
            grammar={attentionGrammar}
            newlineInput={chatUi?.newline_input ?? null}
            onAttention={revealTerminalAttention}
            rearmRef={terminalRearmRef}
          />
        </div>
      )}
      {/* AskUserQuestion 的作答卡（broker 挂起代答）：表单尚未渲染，卡片就是作答面——
          多选/多问题/自定义输入全部可答，提交后答案经 hook 直达模型。刻意没有「仅收起」：
          收起等于让 hook 干等到 300s 超时（与 broker 审批卡同理），出口只有提交与
          「去终端作答」（交还终端表单）。 */}
      {view === "chat" && structuredQuestions.length > 0 && !terminalAttention && !approval && questionAnswerable && <ApprovalCard
        className="chat-screen-approval"
        title={t.chat.questionTitle}
        badge={questionCountdown ? `${t.chat.questionPending} · ${questionCountdown}` : t.chat.questionPending}
        actions={<>
          <button type="button" disabled={resolvingApproval} onClick={() => void sendQuestionToTerminal()}>{t.chat.answerInTerminal}</button>
          <button type="button" className="is-allow" disabled={resolvingApproval || !answerBody} onClick={() => void submitQuestionAnswers()}>{t.chat.submitAnswer}</button>
        </>}
      >
        <QuestionPanels
          mode="answer"
          items={structuredQuestions}
          answers={questionAnswers}
          onSelect={selectQuestionOption}
          onCustom={setQuestionCustom}
        />
        <span>{answerBody ? t.chat.questionAnswerReady : t.chat.questionAnswerIncomplete(unansweredCount)}</span>
      </ApprovalCard>}
      {/* AskUserQuestion 的同步题面卡（展示形态）：broker 自动放行后从结构化参数渲染，与
          终端表单同步出现（先于屏幕识别）。仅展示——作答按键要等识别确认表单在屏
          （interactiveAttention 接管后此卡退场）；识别不认的形态（多问题 tab 表单）走
          「去终端作答」。旧 reporter/降级（挂起超时）场景都落在这里。 */}
      {view === "chat" && structuredQuestions.length > 0 && !terminalAttention && !approval && !questionAnswerable && <ApprovalCard
        className="chat-screen-approval"
        title={t.chat.questionTitle}
        badge={t.chat.questionPending}
        sideActions={<button type="button" className="chat-attention-dismiss is-inline" data-tip={t.chat.attentionDismissTip} onClick={() => setStructuredQuestion(null)}>{t.chat.attentionDismiss}</button>}
        actions={!remoteUi() && history?.ptyManaged && <button type="button" className="is-allow" onClick={() => setView("terminal")}>{t.chat.answerInTerminal}</button>}
      >
        {/* 可点选排队的条件很严：托管会话（GUI 持有 PTY 才写得进按键）+ 单问题
            + 单选。多问题表单绝不可点——queuedAnswer 是单值、且匹配只按 label 不带
            问题下标，用户为问题 2 点的答案会落到停在前台的问题 1 上（识别出的
            numbered-selector 认不出这是第几题）；多选题同理装不下第二个选择。
            这些形态一律纯展示 + 「去终端作答」，不承诺做不到的事。 */}
        <QuestionPanels
          items={structuredQuestions}
          interactive={!!history?.ptyManaged && answerableInCard}
          queuedAnswer={queuedAnswer}
          onToggle={(label) => setQueuedAnswer((current) => current === label ? null : label)}
        />
        <span>{!history?.ptyManaged
          ? (remoteUi() ? t.chat.answerOnDesktop : t.chat.questionExternalHint)
          : !answerableInCard
          // 远程没有终端视图,「去终端作答」的指路改成「回桌面端」。
          ? (remoteUi() ? t.chat.answerOnDesktop : t.chat.questionMultiHint)
          : queuedAnswer
          ? t.chat.queuedAnswerHint(queuedAnswer)
          : t.chat.questionFormLoading}</span>
      </ApprovalCard>}
      {/* broker 审批卡(claude hook 劫走的请求)与「有 pendingReview 但 GUI 接不了」的降级态。
          注意它**没有**「仅收起」:收起等于让 hook 干等到 300s 超时,只能 allow/deny/持久放行。
          降级态只对 GUI 托管的会话渲染:外部终端会话的提问/审批一律留在终端处理
          (broker 同口径整段短路,见 pty.rs handle_approval)——对着它弹一张只能
          「打开终端」的卡,和不弹相比只多一次打扰。ptyManaged 用严格 === false
          (与 composerGated 同哲学):DTO 必填,信号缺失只可能是错配,退回旧路径。 */}
      {view === "chat" && !terminalAttention && (approval || (!brokerOwnsReview && history?.pendingReview && history?.ptyManaged !== false)) && <ApprovalCard
        title={approval ? t.chat.approvalTitle : history?.pendingReview === "question" ? t.chat.questionTitle : history?.pendingReview === "plan" ? t.chat.planTitle : t.chat.approvalTitle}
        // broker 审批 300s 超时回落终端处理——徽章带剩余时间，让「卡片会过期」这件事可见。
        badge={approval && approvalCountdown ? `${t.chat.approvalPending} · ${approvalCountdown}` : t.chat.approvalPending}
        sideActions={approval
          /* `?? []`：类型上字段恒在（DTO 保证），但旧后端/新前端错配时负载可能缺它——
             一个可选按钮组不值得让整个 ChatWindow 白屏。
             持久放行归左侧组：它的作用域比「这一次」大得多，不该和本次决定并排争主位。 */
          ? (approval.permissionSuggestions ?? []).map((suggestion, index) => (
            <button
              type="button"
              className="is-persistent"
              key={index}
              data-tip={approvalSuggestionTip(suggestion, index, t)}
              disabled={resolvingApproval}
              onClick={() => void decideApproval(`suggestion:${index}`)}
            >{approvalSuggestionLabel(suggestion, index, t)}</button>
          ))
          : undefined}
        actions={approval ? <>
          {/* 右端两颗是「就这一次」的决定：拒绝（中性，也是 Esc 的落点）、允许一次（主按钮）。 */}
          <button type="button" className={"is-deny" + (escArmed ? " is-esc-armed" : "")} disabled={resolvingApproval} onClick={() => void decideApproval("deny")}>
            {escArmed ? t.chat.denyConfirmEsc : t.chat.deny}<kbd aria-hidden="true">Esc</kbd>
          </button>
          <button type="button" className="is-allow" disabled={resolvingApproval} onClick={() => void decideApproval("allow_once")}>{t.chat.allowOnce}</button>
        </> : remoteUi()
          ? <span className="chat-remote-hint">{t.chat.answerOnDesktop}</span>
          : <button type="button" onClick={() => setView("terminal")}>{t.chat.openTerminal}</button>}
      >
        {approval ? <>
          <div className="chat-approval-tool"><span>{t.chat.approvalTool}</span><code>{approval.toolName}</code></div>
          {approval.description && <span>{approval.description}</span>}
          {approval.input && <div className="chat-approval-detail">
            <span>{t.chat.approvalInput}</span>
            <pre>{approval.input}</pre>
          </div>}
        {/* 降级分支仅托管会话可达(渲染条件已含 ptyManaged),文案恒为「正在读取终端」。 */}
        </> : <span>{t.chat.approvalReadingTerminal}</span>}
      </ApprovalCard>}
      {/* sessionId=0(远程未选会话)没有可发送的对象,composer 整体不渲染。 */}
      {view === "chat" && sessionId !== 0 && <footer ref={composeRef} className={"chat-compose" + (composerLocked ? " is-locked" : "") + (composerGated || supersededTo != null ? " is-gated" : "")}>
        {supersededTo != null ? <div className="chat-compose-gate">
          <span>{t.chat.supersededBanner}</span>
          <div className="chat-compose-gate-actions">
            <button
              type="button"
              className="chat-send-takeover"
              onClick={() => resetTo(supersededTo)}
            >{t.chat.supersededGo}</button>
          </div>
        </div> : composerGated ? <div className="chat-compose-gate">
          <span>{externalRunning ? t.chat.sendNeedsTakeover : t.chat.composerGateEnded}</span>
          {/* 恢复失败的原因就地可见（接管态的主文案已是同一句，不重复）。 */}
          {sendError && sendError !== t.chat.sendNeedsTakeover && (
            <span className="chat-compose-gate-error" role="alert">{sendError}</span>
          )}
          <div className="chat-compose-gate-actions">
            {resumePermissionPicker}
            <button
              type="button"
              className="chat-send-takeover"
              disabled={sending}
              onClick={() => { if (externalRunning) void takeoverAndRetry(() => {}); else resumeForChat(); }}
            >{externalRunning ? t.chat.terminalTakeover : t.chat.resumeSession}</button>
          </div>
        </div> : <>
        {slashMatches.length > 0 && <div className="dd-menu chat-slash-menu" role="listbox" ref={slashMenuRef}>
          {slashMatches.map((command, index) => (
            <button type="button" key={command.name} role="option" aria-selected={index === slashActive} className="chat-slash-item"
              // styles.css 不在本次改动范围：键盘高亮复用 hover 的同一条表面色变量。
              style={index === slashActive ? { background: "var(--cc-surface-hover)" } : undefined}
              onMouseEnter={() => setSlashIndex(index)}
              onClick={() => { setPrompt(command.name + " "); setSlashIndex(0); }}>
              <code>{command.name}</code>
              {/* 自定义命令的描述从命令文件头里读出（后端下发）；内置命令的描述是翻译资产，走 i18n。 */}
              <span>{command.description ?? t.chat.slashDesc[command.name] ?? ""}</span>
            </button>
          ))}
        </div>}
        {/* `@` 文件补全菜单:复用斜杠菜单的外观;候选是项目内名称命中的文件(相对路径)。 */}
        {atFiles.length > 0 && atQuery !== null && <div className="dd-menu chat-slash-menu" role="listbox">
          {atFiles.map((rel, index) => (
            <button type="button" key={rel} role="option" aria-selected={index === atActive} className="chat-slash-item"
              style={index === atActive ? { background: "var(--cc-surface-hover)" } : undefined}
              onMouseEnter={() => setAtIndex(index)}
              onClick={() => pickAtFile(rel)}>
              <code>@{rel}</code>
            </button>
          ))}
        </div>}
        {attachments.length > 0 && <div className="chat-attachments">
          {attachments.map((file) => file.image ? (
            /* 图片附件给真缩略图（点开进灯箱，与 transcript 图片同一套 ImageRef）；
               asset 协议读不到的路径（对话框选的任意目录）由 ImageRef 自行回退成文件名 chip。 */
            <div className="chat-attachment is-image" key={file.path} data-tip={file.path}>
              <ImageRef path={file.path} />
              <button type="button" aria-label={`${t.chat.removeAttachment} ${file.name}`} onClick={() => setAttachments((items) => items.filter((item) => item.path !== file.path))}>×</button>
            </div>
          ) : (
            <div className="chat-attachment" key={file.path} data-tip={file.path}>
              <span className="chat-file-icon" role="img" aria-label={t.chat.attachmentFile}>
                <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
                  <path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" />
                  <path d="M14 3v5h5" />
                </svg>
              </span>
              {/* 两行：文件名 + 「扩展名 · 大小」（大小仅粘贴来源有，见 Attachment.size）。 */}
              <span className="chat-file-info">
                <span className="chat-file-name">{file.name}</span>
                <span className="chat-file-meta">
                  {[
                    file.name.includes(".") ? file.name.split(".").pop()!.toUpperCase() : t.chat.attachmentFile,
                    file.size != null ? formatBytes(file.size) : null,
                  ].filter(Boolean).join(" · ")}
                </span>
              </span>
              <button type="button" aria-label={`${t.chat.removeAttachment} ${file.name}`} onClick={() => setAttachments((items) => items.filter((item) => item.path !== file.path))}>×</button>
            </div>
          ))}
        </div>}
        {attachmentNotice && <div className="chat-send-error" role="status"><span>{attachmentNotice}</span></div>}
        <textarea
          ref={promptInputRef}
          value={prompt}
          rows={1}
          aria-label={t.chat.inputLabel}
          disabled={composerLocked}
          // 后台会话不吃 inputUnavailable 那句：它说的是「尚未接管，请先切换到终端」，
          // 而后台会话恰恰相反——终端页是没用的那一页，这个输入框才是唯一能发出去的地方。
          placeholder={
            composerLocked
              ? t.chat.inputLocked
              : sendError && !history?.background
                ? t.chat.inputUnavailable
                // 手机上 Enter/Shift+Enter 是不存在的键盘语义,占位只留一句短的。
                : remoteUi() ? t.chat.inputPlaceholderRemote : t.chat.inputPlaceholder
          }
          onChange={(event) => {
            setPrompt(event.target.value);
            setSendError("");
            setSlashIndex(0);
            setSlashDismissed(false);
            // 手动编辑退出历史浏览态;@token 检测跟随光标(caret 在 onChange 时已是新值)。
            historyNavRef.current = null;
            setAtQuery(detectAtToken(event.target.value, event.target.selectionStart ?? event.target.value.length));
          }}
          onPaste={(event) => {
            const files = Array.from(event.clipboardData?.files ?? []);
            if (files.length === 0) return; // 纯文本粘贴照常走默认行为
            // 有文件就整次接管：默认行为会把文件名当正文粘进输入框。
            event.preventDefault();
            void pasteAttachments(files);
          }}
          onKeyDown={(event) => {
            // 补全菜单开着时按键先归菜单：↑↓ 移动高亮，Enter/Tab 把高亮项写进输入框
            // （不是发送——此前菜单展开时 Enter 直接把半截命令发出去了），Esc 收起。
            if (slashMatches.length > 0) {
              if (event.key === "ArrowDown" || event.key === "ArrowUp") {
                event.preventDefault();
                setSlashIndex((index) => (index + (event.key === "ArrowDown" ? 1 : slashMatches.length - 1)) % slashMatches.length);
                return;
              }
              if (event.key === "Escape") {
                event.preventDefault();
                setSlashDismissed(true);
                return;
              }
              if (event.key === "Tab" || (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing)) {
                event.preventDefault();
                const command = slashMatches[slashActive];
                if (command) setPrompt(command.name + " ");
                setSlashIndex(0);
                return;
              }
            }
            // `@` 文件补全菜单开着时按键先归它(与斜杠菜单同一套语义)。
            if (atFiles.length > 0 && atQuery !== null) {
              if (event.key === "ArrowDown" || event.key === "ArrowUp") {
                event.preventDefault();
                setAtIndex((index) => (index + (event.key === "ArrowDown" ? 1 : atFiles.length - 1)) % atFiles.length);
                return;
              }
              if (event.key === "Escape") {
                event.preventDefault();
                setAtQuery(null);
                setAtFiles([]);
                return;
              }
              if (event.key === "Tab" || (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing)) {
                event.preventDefault();
                const rel = atFiles[atActive];
                if (rel) pickAtFile(rel);
                return;
              }
            }
            // ↑ 取回历史消息:空框(或已在浏览态)时 ↑ 往前翻,↓ 往回,翻回最新回到空框。
            // 有草稿时 ↑ 保持原生光标语义,不抢多行编辑。
            if (event.key === "ArrowUp" && !event.nativeEvent.isComposing && userHistory.length > 0
              && (historyNavRef.current !== null || !prompt.trim())) {
              const at = historyNavRef.current?.index ?? userHistory.length;
              if (at > 0) {
                event.preventDefault();
                const next = at - 1;
                historyNavRef.current = { index: next };
                setPrompt(userHistory[next]);
                requestAnimationFrame(() => {
                  const el = promptInputRef.current;
                  el?.setSelectionRange(el.value.length, el.value.length);
                });
              }
              return;
            }
            if (event.key === "ArrowDown" && historyNavRef.current !== null && !event.nativeEvent.isComposing) {
              event.preventDefault();
              const next = historyNavRef.current.index + 1;
              if (next >= userHistory.length) {
                historyNavRef.current = null;
                setPrompt("");
              } else {
                historyNavRef.current = { index: next };
                setPrompt(userHistory[next]);
                requestAnimationFrame(() => {
                  const el = promptInputRef.current;
                  el?.setSelectionRange(el.value.length, el.value.length);
                });
              }
              return;
            }
            // 审批卡的 Esc 徽章要说话算话：窗口级「Esc=拒绝」监听按 activeElement 白名单
            // 让开了输入框，而默认焦点几乎总在这里——徽章几乎永远不生效。空输入框时在此
            // 放行同一语义；有草稿不动（Esc 另有清心流/收 IME 的含义，不能顺手拒了审批）。
            if (event.key === "Escape" && !prompt.trim() && !event.nativeEvent.isComposing && view === "chat") {
              const terminalDeny = commandAttention && commandApproval ? commandDeny : null;
              if (terminalDeny) {
                event.preventDefault();
                denyViaEsc(() => chooseTerminalOption(terminalDeny));
                return;
              }
              if (!terminalAttention && approval && !resolvingApproval) {
                event.preventDefault();
                denyViaEsc(() => void decideApproval("deny"));
                return;
              }
            }
            // Ctrl/⌘+Enter = 先中断当前回合再发送。原来这是 composer 上一颗常驻按钮，
            // 但它和右边的圆钮是两个「把话递出去」的入口，摆在一起只会让人先停下来
            // 分辨该按哪个。降成加速键：默认路径（Enter 排队、圆钮停止）各司其职，
            // 一步到位的组合动作留给知道自己要什么的人。
            if (event.key === "Enter" && (event.ctrlKey || event.metaKey) && !event.nativeEvent.isComposing) {
              event.preventDefault();
              if (canInterrupt && hasDraft) void interruptAndSend();
              else void sendPrompt();
              return;
            }
            if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
              event.preventDefault();
              void sendPrompt();
            }
          }}
        />
        <div className="chat-compose-actions">
          <button type="button" className="chat-attach-button" aria-label={t.chat.attach} data-tip={t.chat.attach} onClick={() => void chooseAttachments()}>
            <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.65"><path d="M12 5v14M5 12h14" /></svg>
          </button>
          {(modelPresets.length > 0 || modelMenuCommand || history?.model || switchTargets.length > 0) && <div className="chat-model" ref={modelMenuUi.ref} onKeyDown={modelMenuUi.onKeyDown}>
            <button
              ref={modelMenuUi.btnRef}
              type="button"
              className="chat-model-button"
              disabled={modelPresets.length === 0 && !modelMenuCommand && switchTargets.length === 0}
              aria-label={t.chat.switchModel}
              aria-expanded={modelMenu}
              data-tip={menuWatching && !modelProbing ? t.chat.modelMenuOpen : modelPresets.length > 0 || modelMenuCommand || switchTargets.length > 0 ? t.chat.switchModel : undefined}
              // 单一点击语义：恒为开/关菜单。探测中（转圈+取消）、读不到清单（去终端页）
              // 都改在菜单条目里表达——此前同一颗按钮隐形承担「开菜单/取消探测/跳终端/
              // 发起探测」四种动作，用户无从预判点下去会发生什么。
              // 清单未学到时开菜单顺带发起静默探测：CLI 自己弹一次菜单，屏幕识别转成 GUI
              // 选项并把真实标签学下来——清单始终是 CLI 现给的，宿主不维护会过时的那份。
              onClick={() => {
                // 与模式菜单互斥：同时只开一个。
                setModeMenu(null);
                const opening = !modelMenu;
                setModelMenu(opening);
                // 关菜单时若探测在途,一并取消(给 TUI 发 Esc 收掉菜单)——识别窗口开着时
                // 绝不能重发命令,否则会打进菜单搜索框把候选全过滤掉。
                if (!opening && modelProbing) { endSilentProbe(); return; }
                // 开菜单顺带发起静默探测学清单;有「切换引擎」分组时不自动探测(跨 provider
                // 切换不该被探测门槛拖住),菜单里保留显式的探测入口。
                if (opening && !modelDropdownReady && !modelProbing && !modelProbeFailed
                  && modelMenuCommand && switchTargets.length === 0) {
                  void probeModelMenu();
                }
              }}
            >
              {/* 标签套 span 才能省略号收尾：text-overflow 只作用于块容器，裸文本在
                  inline-flex 里挤压时会按 min-content（CJK=单字）竖排溢出（实拍）。 */}
              <span className="chat-model-label">{history?.model || t.chat.model}</span>
              {/* 静默探测时把箭头换成转圈：这一下点击确实在做事，但做的事不在屏幕上。 */}
              {modelProbing
                ? <span className="chat-model-spinner" aria-hidden="true" />
                : (modelPresets.length > 0 || modelMenuCommand) && <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4"><path d="M6 9l6 6 6-6" /></svg>}
            </button>
            {modelMenu && <div className="dd-menu chat-model-menu" role="menu">
              {/* 探测中：清单还在读，点击取消。探测成功后清单条目就地接管本菜单。 */}
              {modelProbing && (
                <button type="button" role="menuitem" className="chat-model-item" onClick={endSilentProbe}>
                  <span className="chat-model-item-text">
                    <span className="chat-model-item-name">{t.chat.modelProbing}</span>
                  </span>
                  <span className="chat-model-spinner" aria-hidden="true" />
                </button>
              )}
              {/* 清单未学到（无内联预设的 CLI）：重试探测；读不到清单的 CLI 给「去终端页」直达。 */}
              {!modelProbing && modelPresets.length === 0 && modelMenuCommand && (
                <button
                  type="button"
                  role="menuitem"
                  className="chat-model-item"
                  onClick={() => {
                    // 远程没有终端页可去,探测失败也只给「重试探测」(探测走 PTY,远程可用)。
                    if (modelProbeFailed && !remoteUi()) { setModelMenu(false); setView("terminal"); }
                    else void probeModelMenu();
                  }}
                >
                  <span className="chat-model-item-text">
                    <span className="chat-model-item-name">{modelProbeFailed && !remoteUi() ? t.chat.modelGoTerminal : t.chat.switchModel}</span>
                  </span>
                </button>
              )}
              {modelPresets.map((preset) => {
                const active = history?.model === preset.label;
                return (
                  <button
                    type="button"
                    key={preset.id}
                    role="menuitem"
                    className={"chat-model-item" + (active ? " is-active" : "")}
                    onClick={() => {
                      setModelMenu(false);
                      retryRef.current = () => sendSlash(`/model ${preset.id}`);
                      // 模型是启动参数：切换成功即落库，resume/接管回放 --model。
                      // 不落库的话每次重启都回默认档（1M 上下文档回落 200K，实拍反馈）。
                      void sendText(`/model ${preset.id}`).then((sent) => {
                        if (!sent) return;
                        // 写成功才更新本地显示；失败必须出声——静默吞掉的话用户切了档,
                        // 下次 resume 悄悄回默认,正是注释里那次实拍故障的成因。
                        setSessionLaunchSelection(sessionId, "model", preset.id)
                          .then(() => setStoredSelections((prev) => ({ ...prev, model: preset.id })))
                          .catch((error) => setSendError(formatBackendError(error, t.locale)));
                      });
                    }}
                  >
                    <span className="chat-model-item-text">
                      <span className="chat-model-item-name">{preset.label}</span>
                      <span className="chat-model-item-desc">{t.chat.modelDesc[preset.id] ?? ""}</span>
                    </span>
                    {active && <svg className="chat-model-check" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round"><path d="M4.5 12.5l5 5 10-11" /></svg>}
                  </button>
                );
              })}
              {/* 「切换引擎」分组：跨 provider 切换（携带交接历史）。仅当当前 agent 的历史
                  可导出（后端 supports_chat_export 能力位）且有其他已装 agent 时出现。
                  点 agent 行展开二级（默认模型 + 该 agent 的模型预设，惰性取），点具体
                  档位才真正发起切换——切换是破坏性动作，后面还有 appConfirm 把关。 */}
              {switchTargets.length > 0 && <>
                <div className="chat-model-group" role="presentation">{t.chat.switchProvider}</div>
                {switchTargets.map((agent) => {
                  const { Icon } = agentAssets(agent.id);
                  const expanded = switchTarget === agent.id;
                  const presets = switchPresets[agent.id];
                  return (
                    <div key={agent.id}>
                      <button
                        type="button"
                        role="menuitem"
                        className={"chat-model-item chat-switch-agent" + (expanded ? " is-active" : "")}
                        aria-expanded={expanded}
                        onClick={() => {
                          if (expanded) { setSwitchTarget(null); return; }
                          setSwitchTarget(agent.id);
                          // 目标模型预设惰性取一次（静态声明，不依赖会话），失败按无预设处理。
                          if (!switchPresets[agent.id]) {
                            void agentChatUi(agent.id, cwd, undefined)
                              .then((ui) => setSwitchPresets((prev) => ({ ...prev, [agent.id]: ui?.model_presets ?? [] })))
                              .catch(() => setSwitchPresets((prev) => ({ ...prev, [agent.id]: [] })));
                          }
                        }}
                      >
                        <span className="chat-switch-agent-ico" style={tintStyle(agent.id)}><Icon /></span>
                        <span className="chat-model-item-text">
                          <span className="chat-model-item-name">{agent.display_name}</span>
                        </span>
                        <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4" style={expanded ? { transform: "rotate(180deg)" } : undefined}><path d="M6 9l6 6 6-6" /></svg>
                      </button>
                      {expanded && <div className="chat-switch-models">
                        <button
                          type="button"
                          role="menuitem"
                          className="chat-model-item chat-switch-model"
                          onClick={() => void startProviderSwitch(agent)}
                        >
                          <span className="chat-model-item-text">
                            <span className="chat-model-item-name">{t.chat.switchProviderDefaultModel}</span>
                          </span>
                        </button>
                        {(presets ?? []).map((preset) => (
                          <button
                            type="button"
                            key={preset.id}
                            role="menuitem"
                            className="chat-model-item chat-switch-model"
                            onClick={() => void startProviderSwitch(agent, preset.id)}
                          >
                            <span className="chat-model-item-text">
                              <span className="chat-model-item-name">{preset.label}</span>
                              <span className="chat-model-item-desc">{t.chat.modelDesc[preset.id] ?? ""}</span>
                            </span>
                          </button>
                        ))}
                      </div>}
                    </div>
                  );
                })}
              </>}
            </div>}
          </div>}
          {(() => {
            const states = history?.agentModes ?? [];
            const controls = new Map(modeControls.map((control) => [control.dimension, control]));
            const dimensions = [...modeControls.map((control) => control.dimension)];
            for (const state of states) if (!dimensions.includes(state.dimension)) dimensions.push(state.dimension);
            return dimensions.map((dimension) => {
              const control = controls.get(dimension);
              const state = states.find((mode) => mode.dimension === dimension);
              // 休眠态（connected=false，进程不在）的权限维度换一套语义：此时切「活进程的
              // 模式」意味着先把会话拉起来再循环按键，而且循环够不着 bypassPermissions 这类
              // 只能在启动参数里给的档位。故这颗胶囊改卖「下一次恢复用什么权限」——选择写进
              // resumePermission，随下一次发送经 ensureWritableTerminal 作为启动参数下发并
              // 由后端持久化，与终端封面、接管横幅里的改选共用同一个状态。后台会话除外
              //（daemon 托管，我们恢复不了它）。
              // 选项清单与 resumePermissionPicker 共用同一份构造（permissionChoiceOptions）。
              const resumeChoices = permissionOption && dimension === permissionOption.id
                && history && !history.connected && !history.background
                ? permissionChoiceOptions.map((option) => ({ ...option, inputs: [] }))
                : null;
              const resumePick = resumeChoices !== null;
              // 只有 cycle 键的维度（claude 权限模式）用**屏幕回显标记**派生下拉：那些标记
              // 是 provider 文档承诺的稳定文本，每条对应一个可达模式值——用户要选的是模式，
              // 不是按几次快捷键。选中后由 cycleToMode 循环按到位（见它的注释）。
              const derived = (control?.screen_markers ?? []).map((marker) => ({
                value: marker.value,
                label: t.chat.modeNames[marker.value] ?? marker.value,
                inputs: [],
              }));
              // 三个来源（恢复档位/插件直达项/回显标记派生）统一带 label，渲染处不再各自查表。
              const options = resumeChoices ?? (control?.options?.length
                ? control.options.map((option) => ({ ...option, label: t.chat.modeNames[option.value] ?? option.value }))
                : control?.cycle_input ? derived : []);
              const canCycle = Boolean(control?.cycle_input);
              const interactive = options.length > 0 || canCycle;
              const label = t.chat.modeDimensions[dimension] ?? dimension;
              // 按钮只显示当前值（「跳过权限检查」），维度名（「权限模式」）不再做前缀
              // ——它在 aria-label 与 tooltip 里，占位不解释（实拍反馈「多此一举」）。
              // 状态未知时退回维度名：孤零零一个「—」没人知道这颗按钮是干嘛的。
              // 休眠态显示「下一次恢复将生效的档」：用户改选 > 会话存的启动档 > transcript
              // 回读的旧模式。存档比回读值更权威——恢复回放的就是它。
              const nextResume = resumePermission || storedPermission?.id || "";
              const value = resumePick && nextResume && permissionOption
                ? (t.newSession.launchChoice[`${permissionOption.id}.${nextResume}`] ?? nextResume)
                : state ? (t.chat.modeNames[state.value] ?? state.value) : label;
              return <div className="chat-model" key={dimension} ref={modeMenu === dimension ? modeMenuUi.ref : undefined} onKeyDown={modeMenu === dimension ? modeMenuUi.onKeyDown : undefined}>
                <button
                  ref={modeMenu === dimension ? modeMenuUi.btnRef : undefined}
                  type="button"
                  className="chat-model-button chat-mode-button"
                  disabled={!interactive || sending}
                  // aria-label 保持简短的动作名（屏幕阅读器要的是「做什么」，不是解释）；
                  // 轮换与下拉的差异放在下面的 tooltip 里说。
                  aria-label={interactive ? `${t.chat.switchMode}: ${label}` : label}
                  aria-expanded={options.length > 0 ? modeMenu === dimension : undefined}
                  // 两种交互要分开说：有 options 是「打开菜单挑一个」，只有 cycle_input 的
                  // （codex 的协作模式）是「按一次跳下一个」，没有直达某个值的办法。
                  // 休眠态的恢复改选另有一套说明（选了会记住，之后的恢复自动沿用）。
                  data-tip={resumePick ? t.chat.resumePermissionTip : interactive ? (options.length > 0 ? t.chat.switchMode : t.chat.cycleMode) : undefined}
                  onClick={() => {
                    // 与模型菜单互斥：同时只开一个。
                    if (options.length > 0) { setModelMenu(false); setModeMenu((open) => open === dimension ? null : dimension); }
                    // 无 options 也无回显标记（纯盲切）时才退回「按一次跳下一个」。
                    else if (control?.cycle_input) void changeMode(dimension, [{ data: control.cycle_input, submit: false }], undefined, control.screen_markers);
                  }}
                >
                  {/* 同模型按钮：标签套 span 防挤压竖排。 */}
                  <span className="chat-model-label">{value}</span>
                  {options.length > 0
                    ? <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4"><path d="M6 9l6 6 6-6" /></svg>
                    // 双向箭头而不是圆形箭头：后者在任何界面里都读作「刷新」，
                    // 而这里的语义是「在几个值之间轮换」。
                    : canCycle && <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="m16 3 4 4-4 4M20 7H5M8 21l-4-4 4-4M4 17h15" /></svg>}
                </button>
                {modeMenu === dimension && options.length > 0 && <div className="dd-menu chat-model-menu" role="menu">
                  {options.map((option) => {
                    // 休眠态未改选时高亮存的那档（「沿用」的具象化）；没存值高亮空占位项。
                    const active = resumePick
                      ? (resumePermission || storedPermission?.id || "") === option.value
                      : state?.value === option.value;
                    return <button
                      type="button"
                      key={option.value}
                      role="menuitem"
                      className={"chat-model-item" + (active ? " is-active" : "")}
                      onClick={() => {
                        setModeMenu(null);
                        // 恢复改选只落状态、不碰终端——生效点在下一次恢复的启动参数上。
                        // 选回存的那档 = 沿用（置空，不写覆盖）。
                        if (resumePick) {
                          setResumePermission(
                            storedPermission && String(option.value) === storedPermission.id ? "" : String(option.value),
                          );
                          return;
                        }
                        if (active) return; // 已经是这个模式，别白按一圈
                        // 有直达输入的走它；只有 cycle 键的（派生项 inputs 为空）循环按到位。
                        if (option.inputs.length > 0) {
                          void changeMode(dimension, option.inputs, option.value, control?.screen_markers ?? []);
                        } else if (control?.cycle_input) {
                          void cycleToMode(dimension, option.value, control.cycle_input, control.screen_markers ?? []);
                        }
                      }}
                    >
                      <span className="chat-model-item-name">{option.label}</span>
                      {active && <svg className="chat-model-check" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4"><path d="M4.5 12.5l5 5 10-11" /></svg>}
                    </button>;
                  })}
                </div>}
              </div>;
            });
          })()}
          {history?.contextPct != null && (
            <ContextMeter pct={history.contextPct} window={history.contextWindow} t={t} />
          )}
          {/* 「Enter ↵」提示已按用户要求取消(占位提示里写着 Enter 发送,按钮旁再标一遍
              是重复)。空元素本身不能省——它的 margin-left:auto 负责把圆钮顶到最右。
              Ctrl+Enter=打断并发送的说明只剩 textarea 的 onKeyDown 行为本身,不再有
              可见挂点(此前挂这里的 tooltip 随文字一起去掉:零宽元素 hover 不到)。 */}
          <span className="chat-compose-hint" />
          {/* 触屏没有 Ctrl+Enter:有草稿时主圆钮已是发送,回合还在跑的话停止另给一颗——
              否则手机上必须先清空输入框才能叫停。桌面有快捷键与空框停止,不渲染。 */}
          {remoteUi() && canInterrupt && hasDraft && (
            <button
              type="button"
              className="chat-send-button is-stop chat-stop-secondary"
              aria-label={interrupting ? t.chat.interrupting : t.chat.interruptNow}
              onClick={() => sendInterrupt()}
              disabled={interrupting}
            >
              {interrupting
                ? <span className="chat-model-spinner" aria-hidden="true" />
                : <svg width="15" height="15" viewBox="0 0 24 24" fill="currentColor"><rect x="4" y="4" width="16" height="16" rx="4.5" /></svg>}
            </button>
          )}
          {/* 主圆钮双身份:回合运行中且输入框是空的 → 它就是停止键(黑圆方块,一按停当前
              回合)。发出去的消息撤不回,能叫停的只有这个键,它不该藏在草稿或次级按钮后面。
              一旦开始打字,身份让回「发送」——那时用户要的是把话递进去,停回合走左边的
              「打断并发送」。sending 只可能发生在有草稿时,与 stopMode 天然互斥。 */}
          <button
            type="button"
            className={"chat-send-button" + (stopMode ? " is-stop" : "")}
            aria-label={stopMode ? (interrupting ? t.chat.interrupting : t.chat.interruptNow) : sending ? t.chat.sending : t.chat.send}
            data-tip={stopMode ? t.chat.interruptNowTip : undefined}
            onClick={() => { if (stopMode) sendInterrupt(); else void sendPrompt(); }}
            disabled={stopMode ? interrupting : (!prompt.trim() && attachments.length === 0) || sending}
          >
            {/* sending 不再借停止方块——一个「点不动的停止键」读不出是正在发送,转圈才是。
                中断在途同理:方块换转圈,直到回合真的停下来。 */}
            {stopMode
              ? interrupting
                ? <span className="chat-model-spinner" aria-hidden="true" />
                : <svg width="18" height="18" viewBox="0 0 24 24" fill="currentColor"><rect x="4" y="4" width="16" height="16" rx="4.5" /></svg>
              : sending
                ? <span className="chat-model-spinner" aria-hidden="true" />
                : <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><path d="M12 19V5M6.5 10.5 12 5l5.5 5.5" /></svg>}
          </button>
        </div>
        {sendError && <div className="chat-send-error" role="alert">
          <span>{sendError}</span>
          {/* 会话确实活在外部终端里：就地给接管入口。此前这里只有一句「请切到终端页接管」，
              用户得自己跨页找按钮，回来还要重打一遍刚才的消息。 */}
          {needsTakeover && <>
            {resumePermissionPicker}
            <button
              type="button"
              className="chat-send-takeover"
              disabled={sending}
              onClick={() => { const retry = retryRef.current; void takeoverAndRetry(() => retry?.()); }}
            >{t.chat.terminalTakeover}</button>
          </>}
        </div>}
        {/* 交互式命令的过渡横幅：识别成功后 terminalAttention 卡片会替掉它；识别不出的
            面板形态（识别器只认光标菜单/编号选择器）也至少给「切到终端 / 收起」的出口。 */}
        {/* 静默探测（问模型清单）期间不出这条等待条：那次交互对用户是不可见的，
            忙态只体现在模型按钮上。用户主动发的斜杠菜单命令照旧显示。 */}
        {menuWatching && !modelProbing && !terminalAttention && <div className="chat-send-error" role="status">
          <span>{t.chat.slashMenuOpened}</span>
          {!remoteUi() && <button type="button" className="chat-send-takeover" onClick={() => setView("terminal")}>{t.chat.terminal}</button>}
          <button type="button" className="chat-send-takeover" onClick={cancelTerminalMenu}>{t.chat.slashMenuDismiss}</button>
        </div>}
        {/* 软拦非阻断横幅:消息已发出,提示终端可能有未识别的交互等待,给跳终端/收起两个出口。
            terminalAttention 出现说明识别成功变成了卡片,此横幅即让位。 */}
        {softPromptNotice && !terminalAttention && <div className="chat-send-error" role="status">
          <span>{remoteUi() ? t.chat.unrecognizedPromptNoticeRemote : t.chat.unrecognizedPromptNotice}</span>
          {!remoteUi() && <button type="button" className="chat-send-takeover" onClick={() => setView("terminal")}>{t.chat.terminal}</button>}
          <button type="button" className="chat-send-takeover" onClick={() => setSoftPromptNotice(false)}>{t.chat.slashMenuDismiss}</button>
        </div>}
        </>}
      </footer>}
      </div>
      {/* 「查看改动」停靠面板：与 .chat-main 并列的右列，对话区收缩让位（非覆盖层）。
          折叠只加 is-collapsed（display:none）不卸载，面板状态原样保留。
          分栏柄落在两张卡之间的缝隙里，拖拽调宽（折叠/最大化时隐藏）。 */}
      {!remoteUi() && diffOpen && cwd && (
        <>
          <div
            className={"chat-diff-resizer" + (diffCollapsed ? " is-hidden" : "")}
            role="separator"
            aria-orientation="vertical"
            aria-label={t.chat.diffResize}
            onPointerDown={startDiffResize}
          />
          <GitDiffView
            cwd={diffDir ?? cwd}
            dirs={diffDirs}
            onDirChange={setDiffDir}
            onAddDir={onDiffAddDir}
            onRemoveDir={removeExtraDir}
            summary={gitSummary}
            width={diffWidth}
            collapsed={diffCollapsed}
            maximized={diffMaximized}
            onCollapse={collapseDiffPanel}
            onToggleMaximize={toggleDiffMaximize}
          />
        </>
      )}
      </div>
      {renaming && history && (
        <RenameModal
          initial={history.title || ""}
          onSubmit={renameSession}
          onClose={() => setRenaming(false)}
          t={t}
        />
      )}
      {switcherOpen && (
        <QuickSwitcher
          activeId={sessionId}
          onPick={(id) => resetTo(id)}
          onClose={() => setSwitcherOpen(false)}
        />
      )}
      {shortcutsOpen && <ShortcutSheet onClose={() => setShortcutsOpen(false)} />}
    </div>
  );
}

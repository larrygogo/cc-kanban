import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { appConfirm } from "../confirm";
import { agentChatUi, attachBackgroundSession, sendBackgroundPrompt, clipboardImageFingerprint, confirmStopSession, getChatHistory, getLiveSessionsPage, getPendingApproval, isExternallyHeld, managedTerminalBinding, managedTerminalSnapshot, refreshSessionModel, refreshSessionTodos, registerApprovalConsumer, resolvePendingApproval, savePastedAttachment, sessionTone, startManagedTerminal, takeoverManagedTerminal, unregisterApprovalConsumer, writeManagedTerminal, type ChatHistory, type ChatItem, type ChatUi, type ModeScreenMarker, type PendingApproval } from "../api";
import { useT } from "../i18n";
import { useShowWhenReady } from "../useShowWhenReady";
import { agentAssets, tintStyle } from "../providers";
import { reduceChatEvents } from "../chat/reducer";
import { ApprovalCard } from "./chat/ApprovalCard";
import { matchOptionByLabel, observeTranscriptForDismiss, parseAskUserQuestions, type QuestionDismissTracker } from "./chat/askUserQuestion";
import { ContextMeter } from "./chat/ContextMeter";
import { TodoPanel } from "./chat/TodoPanel";
import { Transcript } from "./chat/Transcript";
import { ChatSidebar } from "./ChatSidebar";
import { ArchiveIcon } from "./sticker/icons";
import { ManagedTerminal } from "./ManagedTerminal";
import { appendTerminalText, modeFromScreen, terminalAttention as detectTerminalAttention, visibleTerminalText, type AttentionGrammar, type TerminalAttention, type TerminalAttentionOption } from "../terminalAttention";
import { useMenuPopup } from "./menu";

function initialSessionId(): number {
  const value = new URLSearchParams(window.location.search).get("sessionId");
  const id = Number(value);
  return Number.isSafeInteger(id) && id !== 0 ? id : 0;
}

const SIDEBAR_COLLAPSED_KEY = "meowo-chat-sidebar-collapsed";

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

function claudeCommandApprovalDetails(text: string) {
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

/// kimi 审批面板的文本详情（官方源码取证 apps/kimi-code/.../approval-panel.ts @ 0.29）：
/// 标题行 ▶ <按工具定制的问题>（Run this command? / Write this file? / Approve X?），
/// 之下是命令/diff 等 display 块。详情区剥掉框线与标题，正文整段进 pre；
/// 工具名从标题反推，反推不到就留空（工具行整体不显示，比显示一个猜的名字诚实）。
function kimiCommandApprovalDetails(text: string) {
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

/// clipFingerprint:粘贴落盘那一刻的剪贴板图像指纹。仅粘贴来源的图片有——发送时若剪贴板
/// 指纹仍与它相等,才允许向 PTY 发 Ctrl-V 走 TUI 的原生图片附加(文件选择器来源没有指纹,
/// 天然不走该路径)。
type Attachment = { path: string; name: string; image: boolean; clipFingerprint?: string };
const IMAGE_EXTENSIONS = new Set(["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg"]);

function attachmentOf(path: string): Attachment {
  const name = path.split(/[\\/]/).pop() || path;
  const extension = name.includes(".") ? name.split(".").pop()!.toLowerCase() : "";
  return { path, name, image: IMAGE_EXTENSIONS.has(extension) };
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
async function submitToTerminal(sessionId: number, content: string, abortIf?: () => string | null): Promise<void> {
  await writeManagedTerminal(sessionId, terminalInput(content));
  await new Promise((resolve) => window.setTimeout(resolve, SUBMIT_GAP_MS));
  const abortReason = abortIf?.() ?? null;
  if (abortReason) {
    await writeManagedTerminal(sessionId, "\x15").catch(() => {});
    throw new Error(abortReason);
  }
  await writeManagedTerminal(sessionId, "\r");
}

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
  // 首读裁剪掉的更早消息：hasMore 只在首读那一发为 true，轮询会把它带回 false，
  // 所以单独存一份状态，别直接读 history.hasMore（提示会闪一下就没）。
  const [hasEarlier, setHasEarlier] = useState(false);
  const [loadingEarlier, setLoadingEarlier] = useState(false);
  const [view, setView] = useState<"chat" | "terminal">(sessionId < 0 ? "terminal" : "chat");
  const [prompt, setPrompt] = useState("");
  const [sending, setSending] = useState(false);
  const [sendError, setSendError] = useState("");
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
  const revealTerminalAttention = useCallback((attention: TerminalAttention | null) => {
    if (!attention) { setTerminalAttention(null); return; }
    terminalEverShownRef.current = true;
    setTerminalAttention((current) => current?.id === attention.id
      && current.text === attention.text
      && JSON.stringify(current.options) === JSON.stringify(attention.options)
      ? current : attention);
  }, []);
  const draftsRef = useRef(new Map<number, { prompt: string; attachments: Attachment[] }>());
  // submitToTerminal 回车前复核用的实时镜像:闭包里读 state 会拿到过期值。
  const terminalAttentionRef = useRef<TerminalAttention | null>(null);
  promptRef.current = prompt;
  attachmentsRef.current = attachments;
  viewRef.current = view;
  terminalAttentionRef.current = terminalAttention;
  if (view === "terminal") terminalEverShownRef.current = true;
  const terminalMounted = terminalEverShownRef.current || terminalMonitorNeeded;
  const [approval, setApproval] = useState<PendingApproval | null>(null);
  // AskUserQuestion 的结构化题面（broker 自动放行后经 interactive-question 事件直达）。
  // 负责「与终端表单同步出现」的展示与**排队作答**：点选的答案先记下（queuedAnswer），
  // 等屏幕识别确认表单在屏后自动落键——绝不向未确认就绪的表单盲写按键。
  const [structuredQuestion, setStructuredQuestion] = useState<PendingApproval | null>(null);
  const [queuedAnswer, setQueuedAnswer] = useState<string | null>(null);
  // 非当前会话的待授权请求：侧边栏亮徽标召唤用户自己过去。注意后端 **会** 为 broker 接管的
  // 审批把窗口切到目标会话（见 pty.rs 的 ensure_approval_window，不切的代价是请求 10s 后
  // 回落 TUI），所以这里是兜底：切换竞态期间、以及消费者已注册在别的会话时到达的请求。
  // 只进不出会越攒越多——clear 事件、切到该会话、请求落到当前会话三条路径都负责摘除。
  const [approvalAwaitingIds, setApprovalAwaitingIds] = useState<ReadonlySet<number>>(() => new Set());
  const [brokerOwnsReview, setBrokerOwnsReview] = useState(false);
  const externalRunning = isExternallyHeld(history?.status);
  // 展示层状态口径:DB status 必须经存活校正才能显示「在跑」,存活事实只信后端 connected
  // ——它已把 pid 判活、事件宽限与托管 PTY 活性(pty registry,随每次轮询新鲜)合并成
  // 单一真相。不要再引入前端自计的 PTY 活性标志(曾有 managedPtyActive:探测循环移交
  // ManagedTerminal 后停更,PTY 退出后悬死 true,把 tone 钉死「运行中」——已整根拆除)。
  // history 未加载时按离线处理,避免首帧闪一下运行态。
  const tone = sessionTone(history?.connected ?? false, history?.status, history?.pendingReview, history?.errored);
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
  // 那条旁路 socket,终端页才有东西可看。只接一次;接失败(worker 已退出/花名册没它了)就
  // 让终端页维持空画面——它本来就不是我们能拉起的进程,没有可重试的动作。
  const bgAttached = useRef(false);
  useEffect(() => {
    if (!history?.background || bgAttached.current) return;
    bgAttached.current = true;
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
      setSendError(String(e));
    } finally {
      setEndingSession(false);
    }
  };
  // 「归档」:此前只有贴纸看板的卡片菜单能归档。用户读完一个会话往往就停在对话窗里,
  // 要收纳它却得先切回看板、在一堆卡片里找回同一条——把入口补在这里,读完即可收。
  // 语义与看板逐字一致(同一条 set_archived,同一个 archived 列):只改看板可见性,
  // 不动进程、不影响本窗继续对话,故不做二次确认(误点再点一次即还原)。
  const [archiving, setArchiving] = useState(false);
  const toggleArchived = async () => {
    if (!history || archiving) return;
    const next = !history.archived;
    setArchiving(true);
    // 乐观翻转:IPC 往返 + 下一轮轮询才回真值,期间按钮维持旧文案会让人以为没点上。
    // 失败回滚并报错——静默失败等于骗用户「已归档」。
    setHistory((current) => (current ? { ...current, archived: next } : current));
    try {
      await invoke("set_archived", { sessionId, archived: next });
      // 归档 = 收纳:侧栏里它当场消失,右边却还停在它的对话上,等于「收起来了却还摊在桌上」。
      // 顺手切到列表里的下一条会话。取不到(它是唯一一条,或查询失败)就留在原地——
      // 空窗比自作主张关窗好,用户仍可继续读这段对话。
      if (next) {
        const page = await getLiveSessionsPage("all", null, null, 5).catch(() => null);
        const following = page?.items.find((item) => item.session.id !== sessionId);
        if (following) resetTo(following.session.id);
      }
    } catch (e) {
      setHistory((current) => (current ? { ...current, archived: !next } : current));
      setSendError(String(e));
    } finally {
      setArchiving(false);
    }
  };
  const [resolvingApproval, setResolvingApproval] = useState(false);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(() => localStorage.getItem(SIDEBAR_COLLAPSED_KEY) === "1");
  const toggleSidebar = () => setSidebarCollapsed((prev) => {
    const next = !prev;
    localStorage.setItem(SIDEBAR_COLLAPSED_KEY, next ? "1" : "0");
    return next;
  });
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
  // "/xx" 且尚未输入空格时给补全候选；一旦带参数或是普通句子就收起。
  // 键盘交互：默认高亮第一项，↑↓ 移动，Enter/Tab 选中写入（不是发送），Esc 收起本次
  // （dismissed 期间不再弹出，继续输入即恢复）。高亮随每次输入重置回第一项。
  const [slashIndex, setSlashIndex] = useState(0);
  const [slashDismissed, setSlashDismissed] = useState(false);
  const slashMenuRef = useRef<HTMLDivElement>(null);
  // transcript 之外的兜底时间线：hook 落库的最近一问一答（UserPromptSubmit / Stop）。
  // transcript 尚未落盘/尚未定位到、或该 agent 不提供结构化 transcript 时用它渲染，
  // 让「会话已在工作」有真实内容可看。transcript 一旦就位（items 非空）即被完整记录取代。
  const provisional: ChatItem[] = [];
  if (history?.lastUserText) provisional.push({ type: "user_text", id: "hook:last-user", timestamp: null, text: history.lastUserText });
  if (history?.lastAiText) provisional.push({ type: "assistant_text", id: "hook:last-ai", timestamp: null, text: history.lastAiText });
  const slashMatches = prompt.startsWith("/") && !prompt.includes(" ") && !slashDismissed
    ? (chatUi?.slash_commands ?? []).filter((c) => c.name.startsWith(prompt) && c.name !== prompt)
    : [];
  // 候选变少时高亮可能越界（如退格删字后），钳回最后一项；无候选时无高亮。
  const slashActive = slashMatches.length ? Math.min(slashIndex, slashMatches.length - 1) : -1;
  // 键盘把高亮移出可视区时滚回来（菜单限高、内部滚动）。jsdom 没有 scrollIntoView，判一下再调。
  useEffect(() => {
    const selected = slashMenuRef.current?.querySelector('[aria-selected="true"]');
    if (typeof selected?.scrollIntoView === "function") selected.scrollIntoView({ block: "nearest" });
  }, [slashActive]);
  const modelPresets = chatUi?.model_presets ?? [];
  const modelMenuCommand = chatUi?.model_menu_command ?? null;
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
  const activeSessionRef = useRef(sessionId);
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
    // 这两个是「每个会话判一次」的一次性闸门，换会话必须重开：不重置的话，从一个后台
    // 会话切到另一个，第二个的画面永远接不上（attach 只跑过一次），落页判断也只对本窗口
    // 加载的第一个会话生效过。
    bgAttached.current = false;
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
    setHistory(null);
    setQuestionCustomText("");
    setLoading(true);
    setFailed(false);
    setPrompt(draft?.prompt ?? "");
    setSendError("");
    setTerminalAttention(null);
    setTerminalMonitorNeeded(false);
    setMenuWatchUntil(0);
    setQueuedInterjections([]);
    setAttachments(draft?.attachments ?? []);
    setAttachmentNotice("");
    setApproval(null);
    setModelMenu(false);
    setModeMenu(null);
    setSlashIndex(0);
    setSlashDismissed(false);
    setChatUi(null);
    setCapabilityOffset(0);
    setBrokerOwnsReview(false);
    setHasEarlier(false);
    setLoadingEarlier(false);
    positionedRef.current = false;
    followRef.current = true;
    // 切会话保持当前视图（终端模式下切会话仍在终端）。负 id 是尚未认领的临时会话，
    // 还没有 transcript 可看，只能进终端——它 claim 成真 id 时 activeSessionRef 已是负数，
    // 此时 view 已是 terminal，保持即可，故无需为 pending 单列分支。
    setView(id < 0 ? "terminal" : viewRef.current);
    setSessionId(id);
  }, []);

  const refresh = useCallback(async () => {
    if (sessionId <= 0 || busyRef.current) {
      if (sessionId < 0) setLoading(false);
      return;
    }
    busyRef.current = true;
    try {
      const next = await getChatHistory(sessionId, offsetRef.current);
      if (activeSessionRef.current !== sessionId) return;
      setCapabilityOffset(next.offset);
      // hasMore 只有首读那一发才可能为 true，后续增量恒为 false——单独记下来。
      if (next.hasMore) setHasEarlier(true);
      offsetRef.current = next.offset;
      // 保留旧引用（而非无条件 setHistory）——稳态下这些字段一轮都不变，但 next 每次
      // 都是新对象，无脑 set 会让整个窗口每 650ms 重渲染一次。items 已在下面单独短路。
      setHistory((prev) => {
        // mode 只在 transcript 出现新模式记录时随增量返回。普通增量为 null 时保留上次观测；
        // 文件 reset 则必须采信全量重读结果，避免沿用旧 transcript 的状态。
        const updates = next.agentModes ?? [];
        const agentModes = next.reset || prev?.sessionId !== next.sessionId
          ? updates
          : [...(prev?.agentModes ?? [])];
        if (!next.reset && prev?.sessionId === next.sessionId) {
          for (const update of updates) {
            const index = agentModes.findIndex((mode) => mode.dimension === update.dimension);
            if (index >= 0) agentModes[index] = update;
            else agentModes.push(update);
          }
        }
        const merged = { ...next, agentModes };
        return sameHistoryMeta(prev, merged) ? prev : merged;
      });
      setItems((prev) => next.items.length || next.reset ? reduceChatEvents(prev, next.items, next.reset) : prev);
      setLoading(false);
      setFailed(false);
    } catch {
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

  useEffect(() => {
    if (sessionId <= 0) return;
    const consumerId = `chat-${sessionId}-${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
    let disposed = false;
    let retryTimer = 0;
    // 注册失败不能就此放弃：没有租约，后端会把所有审批直接交还终端 TUI，
    // 而这扇窗看起来一切正常（轮询永远拿不到 pending）——用户以为在 GUI 等审批，
    // 实际审批卡在终端里没人看。effect 不会重跑，这里自己做有限退避重试。
    const register = (attempt: number) => {
      void registerApprovalConsumer(sessionId, consumerId).then(() => {
        // effect 可能在 IPC 返回前已经因切会话/关窗清理；再次注销闭合这个竞态窗口。
        if (disposed) void unregisterApprovalConsumer(consumerId).catch(() => {});
      }).catch((error) => {
        console.error("注册审批消费者失败", error);
        if (disposed || attempt >= 5) return;
        retryTimer = window.setTimeout(() => register(attempt + 1), 500 * 2 ** attempt);
      });
    };
    register(0);
    return () => {
      disposed = true;
      window.clearTimeout(retryTimer);
      void unregisterApprovalConsumer(consumerId).catch(() => {});
    };
  }, [sessionId]);

  // 打开/切换会话时用会话日志重建一次待办。hook 只在 meowo 在场时才捕获得到待办，
  // 中途启动、hook 漏接、早先解析有误（状态别名不认识）都会让 DB 与真实清单脱节，
  // 而 agent 自己的日志一直是对的。一次有界读，不进 650ms 轮询。
  useEffect(() => {
    if (sessionId <= 0) return;
    void refreshSessionTodos(sessionId).catch(() => {});
  }, [sessionId]);

  useEffect(() => {
    if (sessionId <= 0) return;
    let cancelled = false;
    const poll = () => getPendingApproval(sessionId).then((next) => {
      if (!cancelled) {
        if (next) setBrokerOwnsReview(true);
        setApproval(next);
      }
    }).catch(() => {});
    void poll();
    const timer = window.setInterval(() => void poll(), 400);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [sessionId]);

  useEffect(() => {
    let un: (() => void) | undefined;
    let cancelled = false;
    listen<number>("chat-session-changed", (event) => resetTo(event.payload)).then((fn) => {
      if (cancelled) fn(); else un = fn;
    }).catch(() => {});
    return () => { cancelled = true; un?.(); };
  }, [resetTo]);

  useEffect(() => {
    let unApproval: (() => void) | undefined;
    let unCleared: (() => void) | undefined;
    let cancelled = false;
    listen<PendingApproval>("pending-approval", (event) => {
      if (event.payload.sessionId === activeSessionRef.current) {
        setApprovalAwaitingIds((prev) => {
          if (!prev.has(event.payload.sessionId)) return prev;
          const next = new Set(prev);
          next.delete(event.payload.sessionId);
          return next;
        });
        setBrokerOwnsReview(true);
        setApproval(event.payload);
        // 用户正在终端视图里操作时别把界面切回对话（焦点在 xterm，切换会打断输入）；
        // 不在终端才拉回来——审批卡在对话页，没人看就等于卡住。
        if (viewRef.current !== "terminal") setView("chat");
      } else {
        // 别的会话要授权：不切窗不抢焦点（后端只闪任务栏），侧边栏亮徽标等用户自己过去。
        setApprovalAwaitingIds((prev) => {
          if (prev.has(event.payload.sessionId)) return prev;
          const next = new Set(prev);
          next.add(event.payload.sessionId);
          return next;
        });
      }
    }).then((fn) => { if (cancelled) fn(); else unApproval = fn; }).catch(() => {});
    listen<PendingApproval>("pending-approval-cleared", (event) => {
      setApprovalAwaitingIds((prev) => {
        if (!prev.has(event.payload.sessionId)) return prev;
        const next = new Set(prev);
        next.delete(event.payload.sessionId);
        return next;
      });
      if (event.payload.sessionId === activeSessionRef.current) {
        setApproval((current) => current?.requestId === event.payload.requestId ? null : current);
      }
    }).then((fn) => { if (cancelled) fn(); else unCleared = fn; }).catch(() => {});
    return () => { cancelled = true; unApproval?.(); unCleared?.(); };
  }, []);

  // AskUserQuestion 题面直达。事件与 chat-session-changed 存在竞态（后端先切窗再发题面，
  // 前端两个事件到达顺序不保证）：无论当下会话是否匹配都先暂存进 ref，会话切换完成的
  // effect 里再领养，两种到达顺序都不丢卡。
  const lastInteractiveQuestionRef = useRef<{ payload: PendingApproval; at: number } | null>(null);
  useEffect(() => {
    let un: (() => void) | undefined;
    let cancelled = false;
    listen<PendingApproval>("interactive-question", (event) => {
      lastInteractiveQuestionRef.current = { payload: event.payload, at: Date.now() };
      if (event.payload.sessionId === activeSessionRef.current) {
        // 题面卡就是处理面：抑制 pendingReview 的「去终端处理」降级卡（与 broker 审批同理）。
        setBrokerOwnsReview(true);
        setStructuredQuestion(event.payload);
        // 与审批一致：用户正在终端视图操作时不抢（表单本来就在那里），否则拉回对话页。
        if (viewRef.current !== "terminal") setView("chat");
      }
    }).then((fn) => { if (cancelled) fn(); else un = fn; }).catch(() => {});
    return () => { cancelled = true; un?.(); };
  }, []);
  // 会话切换：清掉旧会话的题面卡；竞态暂存的题面若属于新会话（事件先于切换抵达）则领养。
  useEffect(() => {
    const last = lastInteractiveQuestionRef.current;
    if (last && last.payload.sessionId === sessionId && Date.now() - last.at < 60_000) {
      setBrokerOwnsReview(true);
      setStructuredQuestion(last.payload);
    } else {
      setStructuredQuestion(null);
    }
  }, [sessionId]);
  // 兜底过期：识别一直没就绪、用户已在终端答完的情况下，别让展示卡挂一辈子。
  // 题面卡消失（过期/收起/会话切换）时排队的答案一并作废——没有卡背书的按键不许落。
  useEffect(() => {
    if (!structuredQuestion) {
      setQueuedAnswer(null);
      return;
    }
    const timer = window.setTimeout(() => setStructuredQuestion(null), 180_000);
    return () => window.clearTimeout(timer);
  }, [structuredQuestion]);
  // 「在终端作答完成」的收卡信号：判定逻辑见 observeTranscriptForDismiss（含基线为何
  // 必须在题面出现当刻记下的教训）。3 秒静置期内的增长（提问自身 tool_use 落盘）被
  // 吸收进基线；3 秒内答完的极端手速由 180s 兜底过期收尾。
  const questionTranscriptBaseline = useRef<QuestionDismissTracker | null>(null);
  useEffect(() => {
    questionTranscriptBaseline.current = structuredQuestion
      ? { armAt: Date.now() + 3_000, count: null }
      : null;
  }, [structuredQuestion]);
  useEffect(() => {
    const baseline = questionTranscriptBaseline.current;
    if (!structuredQuestion || !baseline) return;
    const count = (history?.offset ?? 0) + (history?.items.length ?? 0);
    if (observeTranscriptForDismiss(baseline, count, Date.now())) setStructuredQuestion(null);
  }, [structuredQuestion, history]);

  useEffect(() => {
    // transcript 的 pendingReview 比 broker 的实时状态慢一拍。只有历史状态确实清空后，
    // 才重新启用“去终端处理”的兼容提示，避免 GUI 审批完成时闪一下旧提示。
    if (!history?.pendingReview) setBrokerOwnsReview(false);
  }, [history?.pendingReview]);

  useLayoutEffect(() => {
    if (view !== "chat") {
      // 终端页会卸载 chat-scroll；切回来得到的是全新的滚动容器，必须重新做一次首帧定位。
      positionedRef.current = false;
      followRef.current = true;
      return;
    }
    if (!followRef.current) return;
    const el = scrollRef.current;
    if (!el || items.length === 0) return;
    if (!positionedRef.current) {
      // 历史消息首次出现时必须在绘制前瞬移到底部；否则 `.chat-scroll` 的 smooth 行为会让
      // 用户每次打开窗口都看到整段对话从顶部滚下来。
      const behavior = el.style.scrollBehavior;
      el.style.scrollBehavior = "auto";
      el.scrollTop = el.scrollHeight;
      el.style.scrollBehavior = behavior;
      positionedRef.current = true;
      return;
    }
    el.scrollTop = el.scrollHeight;
  }, [items, view]);

  // 终端页会把焦点交给 xterm（且 footer 整个卸载，回来的 textarea 是全新节点）；
  // 用户切回对话时把焦点还给输入框。只在「终端 → 对话」的切换沿触发——chat 内的
  // 普通重渲染不动焦点，不打断正在输入的人。
  const prevViewRef = useRef(view);
  useEffect(() => {
    const prev = prevViewRef.current;
    prevViewRef.current = view;
    if (view === "chat" && prev === "terminal") promptInputRef.current?.focus();
  }, [view]);

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
        node.scrollTop = prevTop + (node.scrollHeight - prevHeight);
        // 按落位重算吸底状态。内容不足一屏时 scrollTop 仍是 0，不会有 scroll 事件来
        // 恢复 followRef，漏掉这一步会让之后的新消息再也不自动滚到底。
        followRef.current = node.scrollHeight - node.scrollTop - node.clientHeight < 80;
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
    if (el) followRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
  };
  const close = () => getCurrentWindow().close().catch(() => {});
  /// 发送类动作的公共守卫:sending 置位/清错 → 终端占用检查 → 确保可写终端 → 执行 body,
  /// 统一 catch/收尾。sendText/sendWithClipboardImage/changeMode 三个入口共用——门控语义
  /// (新增不可写原因、needsTakeover 复位时机)只需改这一处,不会漏出某条不检查占用的路径。
  const withSendGuard = async (body: () => Promise<boolean>): Promise<boolean> => {
    setSending(true);
    setSendError("");
    setNeedsTakeover(false);
    try {
      if (terminalAttention) {
        terminalEverShownRef.current = true;
        setSendError(t.chat.terminalNeedsAttention);
        return false;
      }
      // 软拦:hook 说 agent 在等交互(pendingReview),但屏幕识别没认出卡片——多半是
      // 未覆盖的提示形态(别家 CLI/新版/本地化)。此时直接发送会把正文+回车打进那个
      // 看不见的选择器:文字过滤选项、回车提交当前焦点项。识别是启发式的,不能硬拦
      // (误报会把人锁死),给一次明确知情的确认。
      if (!terminalAttention && history?.pendingReview && !approval && !brokerOwnsReview) {
        const proceed = await appConfirm(t.chat.unrecognizedPromptConfirm, {
          title: t.chat.terminalPromptTitle,
        });
        if (!proceed) return false;
      }
      if (!await ensureWritableTerminal()) return false;
      return await body();
    } catch (error) {
      setSendError(error instanceof Error ? error.message : String(error));
      return false;
    } finally {
      setSending(false);
    }
  };
  /// 回车前复核(见 submitToTerminal 的 abortIf):正文落下后、回车发出前,交互提示
  /// 恰好弹出的话,这个回车会替用户回答它——命中就中止提交。
  const attentionAbort = () => (terminalAttentionRef.current ? t.chat.terminalNeedsAttention : null);
  /// 发送一段文本到会话（消息正文与斜杠命令共用）。返回是否真的送达。
  const sendText = (content: string): Promise<boolean> => withSendGuard(async () => {
    await submitToTerminal(sessionId, content, attentionAbort);
    return true;
  });
  /// 剪贴板原生图片附加:向 PTY 发 Ctrl-V,让 TUI 自己读剪贴板走它的官方图片粘贴通道
  /// (claude 的 `[Image #N]`、kimi 的 `[image:…]`),从屏幕上确认占位符出现后再写正文提交。
  /// 调用方已确认「剪贴板指纹 == 待发附件」,全程只读剪贴板、不写。
  /// 返回 false = 未送达,调用方退回指令文本。
  const sendWithClipboardImage = (content: string, marker: string): Promise<boolean> => withSendGuard(async () => {
    const before = await managedTerminalSnapshot(sessionId);
    await writeManagedTerminal(sessionId, "\x16");
    // TUI 读剪贴板+缓存图片是异步的:轮询增量输出等占位符,1.5s 没出现放弃原生化。
    const markerPattern = new RegExp(marker, "i");
    const decoder = new TextDecoder();
    let tail = "";
    let since = before.endOffset;
    for (let attempt = 0; attempt < 6; attempt += 1) {
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
      if (markerPattern.test(visibleTerminalText(tail))) {
        await submitToTerminal(sessionId, content, attentionAbort);
        return true;
      }
    }
    // 占位符没出现。Ctrl-V 的副作用不可撤销——慢机/大图上图片可能在超时之后才落进
    // composer,若直接回退指令文本,迟到的原生图会和它一起提交成重复附件。先 Ctrl-U
    // 清行做尽力撤销(两家 TUI 都支持 emacs 行编辑),把残余竞态压缩到「清行后才落地」
    // 的极小窗口。
    await writeManagedTerminal(sessionId, "\x15").catch(() => {});
    return false;
  });
  /// 确保有一个可写的托管终端；没有就地拉起。返回 false 表示已把原因写进 sendError。
  ///
  /// 关键点：**不再靠前端的 status 猜**「是不是还在外部终端跑着」。status 为 stale 只说明
  /// 一段时间没上报，进程很可能早就没了——而旧逻辑会把这类会话一律拒掉，用户明明可以直接发。
  /// 后端的 `session_agent_alive` 是按 pid 的权威判定，让它来拒：拒了才说明进程真活着，
  /// 这时给出就地接管入口，而不是一句「请自己切到终端页」的死路。
  async function ensureWritableTerminal(): Promise<boolean> {
    const snapshot = await managedTerminalSnapshot(sessionId);
    if (snapshot.active) return true;
    // capability 查询通常已随 history 完成；用户极快发送时就在这里补等一次，不能因为
    // React 状态尚未落下而漏掉 provider 声明的信任/登录提示。
    const ui = chatUi ?? (provider ? await agentChatUi(provider, cwd, sessionId).catch(() => null) : null);
    try {
      await startManagedTerminal(sessionId, 100, 30);
    } catch (error) {
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
    const startup = await waitForTerminalReady(sessionId, ui?.startup_attention_markers ?? [], terminalReadyMessages, { provider: history?.provider, selectorAnchors: ui?.selector_anchors ?? [] });
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
      await takeoverManagedTerminal(sessionId, 100, 30);
      terminalRearmRef.current?.();
      setNeedsTakeover(false);
      const startup = await waitForTerminalReady(sessionId, chatUi?.startup_attention_markers ?? [], terminalReadyMessages, attentionGrammar);
      if (startup !== "ready") {
        terminalEverShownRef.current = true;
        setTerminalAttention(startup);
        setSendError(t.chat.terminalNeedsAttention);
        return;
      }
    } catch (error) {
      setSendError(String(error));
      return;
    } finally {
      setSending(false);
    }
    await retry();
  };

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
        setSendError(String(error));
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
      if (interjecting) {
        queuedIdRef.current += 1;
        const id = queuedIdRef.current;
        // 水位线快照:此刻已有的最近用户消息与 transcript 尾部条目都算旧证据,消解
        // 一律不认(见 queuedInterjections 声明处)。
        const priorItemIds: ReadonlySet<string> = new Set(
          items.slice(-12).filter((item) => item.type === "user_text").map((item) => item.id),
        );
        setQueuedInterjections((current) => [...current, {
          id,
          text: queuedText,
          priorUserText: history?.lastUserText ?? null,
          priorItemIds,
        }]);
      }
    };
    // 原生图片附加:仅当「单一图片附件 + 粘贴来源 + 剪贴板此刻仍是这张图 + 插件声明
    // TUI 支持 Ctrl-V 图片粘贴」。任一不满足(含指纹比对失败、占位符超时)都静默退回
    // 指令文本——那条路径对所有 agent 恒可用。
    const clipMarker = chatUi?.clipboard_image_paste;
    const single = attachments.length === 1 ? attachments[0] : null;
    if (clipMarker && single?.image && single.clipFingerprint) {
      const current = await clipboardImageFingerprint().catch(() => null);
      if (current && current === single.clipFingerprint && await sendWithClipboardImage(prompt.trim(), clipMarker)) {
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
      setSendError(error instanceof Error ? error.message : String(error));
      return;
    }
    // 给 TUI 一拍完成中断收尾(打印 Interrupted、回到 composer),再走正常发送。
    await new Promise((resolve) => window.setTimeout(resolve, 500));
    await sendPrompt();
  };
  /// 只发中断键,停掉当前回合、不发送任何内容。composer 上的裸「中断」与排队条的
  /// 「立即插话」是同一个动作,只是语境不同:后者停下当前回合后,CLI 自己会接着处理队列。
  const sendInterrupt = () => {
    const interrupt = chatUi?.interrupt_input;
    if (!interrupt) return;
    void writeManagedTerminal(sessionId, interrupt)
      .catch((error) => setSendError(error instanceof Error ? error.message : String(error)));
  };
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
  const openTerminalMenu = async (command: string) => {
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
    // 粘贴此刻剪贴板就是这批内容:记下图像指纹,发送时比对「剪贴板里还是不是这张图」,
    // 匹配才走 TUI 的原生 Ctrl-V 图片附加。读失败(平台不支持/无图像)按 null 处理,
    // 只影响原生化,不影响附件本身。
    const clipFingerprint = files.some((file) => (file.type ?? "").startsWith("image/"))
      ? await clipboardImageFingerprint().catch(() => null)
      : null;
    const results = await Promise.allSettled(files.map(async (file) => {
      const data = base64OfBuffer(await file.arrayBuffer());
      // 剪贴板截图统一叫 image.png；名字进的是带时间戳的独立子目录，不会互踩。
      const path = await savePastedAttachment(file.name || "pasted.png", data);
      const attachment = attachmentOf(path);
      return attachment.image && clipFingerprint ? { ...attachment, clipFingerprint } : attachment;
    }));
    const fresh = results
      .filter((entry): entry is PromiseFulfilledResult<Attachment> => entry.status === "fulfilled")
      .map((entry) => entry.value);
    const failed = results.find((entry) => entry.status === "rejected") as PromiseRejectedResult | undefined;
    // 失败原因（超限/落盘失败）必须可见；没失败再按数量上限给提示，与文件选择器同一套。
    setAttachmentNotice(failed
      ? String(failed.reason)
      : attachmentsRef.current.length + fresh.length > 12 ? t.chat.attachmentLimit(12) : "");
    setAttachments((current) => [...current, ...fresh].slice(0, 12));
  };
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
  const composeRef = useRef<HTMLElement>(null);
  useEffect(() => {
    const el = composeRef.current;
    if (!el) return;
    if (composerLocked) el.setAttribute("inert", "");
    else el.removeAttribute("inert");
  }, [composerLocked, view]);

  const commandAttention = terminalAttention && (terminalAttention.id === "claude:command-approval" || terminalAttention.id === "kimi:command-approval") ? terminalAttention : null;
  const interactiveAttention = terminalAttention?.id === "interactive:numbered-selector" ? terminalAttention : null;
  // 屏幕识别就绪 → 可作答的交互选择器接管，结构化题面的展示卡退场（同一份题面不双卡）。
  useEffect(() => {
    if (interactiveAttention) setStructuredQuestion(null);
  }, [interactiveAttention]);
  const structuredQuestions = useMemo(
    () => (structuredQuestion ? parseAskUserQuestions(structuredQuestion.input) : []),
    [structuredQuestion],
  );
  const commandApproval = commandAttention
    ? commandAttention.id === "kimi:command-approval"
      ? kimiCommandApprovalDetails(commandAttention.text)
      : claudeCommandApprovalDetails(commandAttention.text)
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
  const chooseTerminalOption = (option: { input: string } | undefined) => {
    if (!option) return;
    // 选完这次菜单就结束了：关掉识别窗口，否则按钮会一直停在「收起」态。
    // 判据取 attention 本身的类型而不是识别窗口是否还开着——窗口会到点自灭，
    // 用户慢慢选的话就漏掉刷新了。
    const wasModelMenu = terminalAttention?.id === "interactive:cursor-menu";
    setMenuWatchUntil(0);
    void writeManagedTerminal(sessionId, option.input)
      .then(() => {
        setTerminalAttention(null);
        // 模型平时由 Stop hook 落库，而 `/model` 切换不产生 Stop——不主动刷一次的话，
        // 对话页和贴纸会一直挂着旧模型直到下一条消息跑完。CLI 要一会儿才把新模型写进
        // 会话日志，故稍等再读。
        if (wasModelMenu) window.setTimeout(() => void refreshSessionModel(sessionId).catch(() => {}), 600);
      })
      .catch((error) => setSendError(String(error)));
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
      const active = document.activeElement;
      if (active instanceof HTMLElement && (active.tagName === "TEXTAREA" || active.tagName === "INPUT" || active.isContentEditable)) return;
      event.preventDefault();
      if (terminalDeny) chooseTerminalOption(terminalDeny);
      else void decideApproval("deny");
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const chooseInteractiveOption = (option: TerminalAttentionOption) => {
    if (!option.input) return;
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
    void writeManagedTerminal(sessionId, option.input)
      .then(() => { if (option.kind === "submit" || option.kind === "chat") setTerminalAttention(null); })
      .catch((error) => setSendError(String(error)));
  };
  // 排队作答的落键时刻：屏幕识别接管（表单确认在屏）后，把题面卡上点选的答案自动写进
  // 表单。choice 的 input 是「方向键定位 + 回车」——单选即完成作答，多选是勾选该项、
  // 提交仍归用户。匹配不上（截断歧义/多问题表单）就保持交互卡让用户手点，绝不猜。
  useEffect(() => {
    if (!interactiveAttention || !queuedAnswer) return;
    setQueuedAnswer(null);
    const choices = (interactiveAttention.options ?? []).filter((option) => option.kind === "choice");
    const match = matchOptionByLabel(choices, queuedAnswer);
    if (match) chooseInteractiveOption(match);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- chooseInteractiveOption 每次渲染新建，按值依赖会空转
  }, [interactiveAttention, queuedAnswer]);

  const submitCustomAnswer = (option: TerminalAttentionOption) => {
    const value = questionCustomText.trim();
    if (!value || !option.input) return;
    void writeManagedTerminal(sessionId, option.input + value + "\r")
      .then(() => setQuestionCustomText(""))
      .catch((error) => setSendError(String(error)));
  };

  return (
    <div className={"chat-window" + (view === "terminal" ? " is-terminal" : "")}>
      {!sidebarCollapsed && <ChatSidebar
        activeId={sessionId}
        approvalAwaitingIds={approvalAwaitingIds}
        onSelect={(id) => { if (id !== sessionId) resetTo(id); }}
        onCollapse={toggleSidebar}
      />}
      <div className="chat-main">
      <header className="chat-bar" data-tauri-drag-region>
        {sidebarCollapsed && (
          <button
            type="button"
            className="chat-sidebar-open"
            aria-label={t.chat.sidebarExpand}
            data-tip={t.chat.sidebarExpand}
            onClick={toggleSidebar}
          >
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8">
              <rect x="3" y="4" width="18" height="16" rx="2.5" />
              <path d="M9.5 4v16" />
            </svg>
          </button>
        )}
        {history?.provider && (() => {
          const Icon = agentAssets(history.provider).Icon;
          return (
            <span className="chat-provider-logo" style={tintStyle(history.provider, true)} role="img" aria-label={history.provider} data-tauri-drag-region>
              <Icon />
            </span>
          );
        })()}
        <div className="chat-heading" data-tauri-drag-region>
          <span className="chat-title" data-tauri-drag-region>{history?.title || t.chat.title}</span>
          {history?.cwd && (
            <button
              type="button"
              className="chat-cwd"
              data-tip={t.sticker.openProjectDir}
              onClick={() => history.cwd && void invoke("open_project_dir", { cwd: history.cwd }).catch(() => {})}
            >{history.cwd}</button>
          )}
        </div>
        {/* 标题栏常驻状态徽标:原「Live sync」恒亮绿点与运行状态无关,信息量为零;
            改为状态驱动——滚动到哪里都能一眼看到 agent 是否在跑。 */}
        <span className={`chat-live is-${tone}`} data-tauri-drag-region role="status">
          <i data-tauri-drag-region />
          {t.chat.status[tone]}
        </span>
        {history && (
          <button
            type="button"
            className={"chat-archive" + (history.archived ? " is-on" : "")}
            disabled={archiving}
            aria-pressed={history.archived}
            aria-label={history.archived ? t.sticker.unarchive : t.sticker.archive}
            data-tip={history.archived ? t.chat.unarchiveTip : t.chat.archiveTip}
            onClick={() => void toggleArchived()}
          >
            <ArchiveIcon archived={history.archived} />
          </button>
        )}
        {history?.ptyManaged && (
          <button type="button" className="chat-end" disabled={endingSession} data-tip={t.chat.endSessionConfirm} onClick={() => void endSession()}>
            {endingSession ? t.chat.terminalStopping : t.chat.endSession}
          </button>
        )}
        <div className="chat-view-tabs">
          <button type="button" className={view === "chat" ? "is-active" : ""} aria-pressed={view === "chat"} onClick={() => setView("chat")}>{t.chat.conversation}</button>
          <button type="button" className={view === "terminal" ? "is-active" : ""} aria-pressed={view === "terminal"} onClick={() => setView("terminal")}>{t.chat.terminal}</button>
        </div>
        <button type="button" className="winclose" aria-label={t.chat.close} data-tip={t.chat.close} onClick={close}>
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><path d="M6 6l12 12M18 6L6 18" /></svg>
        </button>
      </header>
      {/* 两个视图都留在树上、用 CSS 切换可见性：此前三元表达式会在每次切 tab 时
          dispose + new Terminal()，还要把整个 backlog 重传并让 xterm 重放一遍 ANSI。
          终端侧懒挂载（terminalMounted），纯对话的用户不用白付 xterm 的创建成本。 */}
      <main className="chat-scroll" ref={scrollRef} onScroll={onScroll} hidden={view !== "chat"}>
        {loading ? <div className="chat-empty">{t.chat.loading}</div>
          : failed ? <div className="chat-empty is-error">{t.chat.loadError}</div>
          : history && !history.supported ? (
            /* 不提供结构化 transcript 的 agent：hook 落库的最近往来仍然是真实数据，先渲染它，
               「暂未提供结构化对话记录」降为其下的注脚——有什么就展示什么，而不是只报没有。 */
            provisional.length > 0
              ? <><Transcript sessionId={sessionId} items={provisional} /><div className="chat-empty is-note">{t.chat.unsupported}</div></>
              : <div className="chat-empty">{t.chat.unsupported}</div>
          )
          /* 空列表分两种事实：会话已在跑（transcript 尚未落第一条/尚未定位到）≠ 真的没有记录。
             hook 侧已知的最近往来（lastUserText / lastAiText）先顶上；连它也没有时，
             running 态也不能说「没有内容」——那与下面的运行指示互相打架。 */
          : items.length === 0 ? (
            provisional.length > 0
              ? <Transcript sessionId={sessionId} items={provisional} />
              : <div className="chat-empty">{tone === "running" ? t.chat.emptyWorking : t.chat.empty}</div>
          )
          : <>
            {hasEarlier && (
              <div className="chat-load-earlier">
                <button type="button" onClick={() => void loadEarlier()} disabled={loadingEarlier}>
                  {loadingEarlier ? t.chat.loadingEarlier : t.chat.loadEarlier}
                </button>
              </div>
            )}
            <Transcript sessionId={sessionId} items={items} />
          </>}
        {/* Agent 自己维护的待办清单。它不属于时间线（会被反复整份改写），故固定在底部
            而不是插进消息流里——否则每改一次待办就多一条历史。 */}
        {!loading && (history?.todos?.length ?? 0) > 0 && <TodoPanel todos={history!.todos} />}
      </main>
      {/* Agent 正在跑但 transcript 半天不落新行时，页面此前毫无动静，像卡死。
          运行条原在滚动流内、Transcript 之后——上翻历史就随内容滚出视口。移到 main 与
          compose 之间常驻:滚到哪里都看得见,有具体活动（最近命令）就显示出来。 */}
      {view === "chat" && !loading && tone === "running" && (
        <div className="chat-running" role="status">
          <i /><span>{history?.currentActivity || t.chat.running}</span>
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
              <button type="button" data-tip={t.chat.interjectNowTip} onClick={sendInterrupt}>{t.chat.interjectNow}</button>
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
            {/* kimi 的「Approve for this session」只记本会话，不是 claude 的持久规则——文案不能混。 */}
            {commandAttention.id === "kimi:command-approval"
              ? t.chat.allowSession
              : `${t.chat.allowRemember}${commandRemember.label.match(/for:\s*(.+)$/i)?.[1] ? ` · ${commandRemember.label.match(/for:\s*(.+)$/i)?.[1]}` : ""}`}
          </button>}
        </>}
        actions={<>
          {commandDeny && <button type="button" className="is-deny" onClick={() => chooseTerminalOption(commandDeny)}>
            {t.chat.deny}<kbd aria-hidden="true">Esc</kbd>
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
          : terminalAttention.options?.length && terminalAttention.id.startsWith("provider:")
            ? t.chat.trustPromptTitle
            : t.chat.terminalPromptTitle}
        badge={t.chat.approvalPending}
        sideActions={<button type="button" className="chat-attention-dismiss is-inline" data-tip={t.chat.attentionDismissTip} onClick={() => setTerminalAttention(null)}>{t.chat.attentionDismiss}</button>}
        actions={!terminalAttention.options?.length && <>
          {/* 取消发 Esc——在 Claude 里会打断正在跑的回合,与零副作用的「仅收起」是两回事。 */}
          <button type="button" onClick={() => {
            void writeManagedTerminal(sessionId, "\x1b")
              .then(() => setTerminalAttention(null))
              .catch((error) => setSendError(String(error)));
          }}>{t.chat.terminalPromptCancel}</button>
          <button type="button" className="is-allow" onClick={() => {
            void writeManagedTerminal(sessionId, "\r")
              .then(() => setTerminalAttention(null))
              .catch((error) => setSendError(String(error)));
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
          <ManagedTerminal
            key={sessionId}
            sessionId={sessionId}
            status={history?.status}
            visible={view === "terminal"}
            background={history?.background ?? false}
            // 后台会话的按键注定无效。第一下就把用户领到对话页——那里的输入框走 daemon 的
            // 送话通道，是真能用的。原先的做法是让他打完一整句再弹错误，等于白打。
            onBackgroundInput={() => {
              setView("chat");
              setSendError(t.chat.sendBackgroundKeysMovedYou);
            }}
            attentionMarkers={chatUi?.startup_attention_markers ?? []}
            interactivePrompt={terminalInteractivePrompt}
            expectMenu={menuWatching}
            grammar={attentionGrammar}
            onAttention={revealTerminalAttention}
            rearmRef={terminalRearmRef}
          />
        </div>
      )}
      {/* AskUserQuestion 的同步题面卡：broker 自动放行后从结构化参数渲染，与终端表单同步
          出现（先于屏幕识别）。仅展示——作答按键要等识别确认表单在屏（interactiveAttention
          接管后此卡退场）；识别不认的形态（多问题 tab 表单）走「去终端作答」。 */}
      {view === "chat" && structuredQuestions.length > 0 && !terminalAttention && !approval && <ApprovalCard
        className="chat-screen-approval"
        title={t.chat.questionTitle}
        badge={t.chat.questionPending}
        sideActions={<button type="button" className="chat-attention-dismiss is-inline" data-tip={t.chat.attentionDismissTip} onClick={() => setStructuredQuestion(null)}>{t.chat.attentionDismiss}</button>}
        actions={history?.ptyManaged && <button type="button" className="is-allow" onClick={() => setView("terminal")}>{t.chat.answerInTerminal}</button>}
      >
        {structuredQuestions.map((item, questionIndex) => (
          <div key={questionIndex}>
            {item.question && <span className="chat-approval-prewrap">{item.header ? `${item.header} · ${item.question}` : item.question}</span>}
            {/* 只有托管会话能点选排队——GUI 持有 PTY 才写得进作答按键。外部终端会话
                的题面纯展示（省一次切窗看题），作答回其终端。勾选格只在多选题出现：
                单选题的选中态用边框高亮表达，挂一排空勾选框既丑又误导。 */}
            {history?.ptyManaged ? (
              <div className="chat-approval-options">
                {item.options.map((option, optionIndex) => (
                  <button
                    type="button"
                    className={queuedAnswer === option.label ? "is-selected" : ""}
                    key={`${optionIndex}:${option.label}`}
                    onClick={() => setQueuedAnswer((current) => current === option.label ? null : option.label)}
                  >
                    {item.multiSelect && <i aria-hidden="true">{queuedAnswer === option.label ? "✓" : ""}</i>}
                    <span><b>{option.label}</b>{option.description && <small>{option.description}</small>}</span>
                  </button>
                ))}
              </div>
            ) : (
              <div className="chat-approval-options is-static">
                {item.options.map((option, optionIndex) => (
                  <div className="chat-approval-option-static" key={`${optionIndex}:${option.label}`}>
                    <span><b>{option.label}</b>{option.description && <small>{option.description}</small>}</span>
                  </div>
                ))}
              </div>
            )}
          </div>
        ))}
        <span>{history?.ptyManaged
          ? (queuedAnswer ? t.chat.queuedAnswerHint(queuedAnswer) : t.chat.questionFormLoading)
          : t.chat.questionExternalHint}</span>
      </ApprovalCard>}
      {/* broker 审批卡(claude hook 劫走的请求)与「有 pendingReview 但 GUI 接不了」的降级态。
          注意它**没有**「仅收起」:收起等于让 hook 干等到 300s 超时,只能 allow/deny/持久放行。 */}
      {view === "chat" && !terminalAttention && (approval || (!brokerOwnsReview && history?.pendingReview)) && <ApprovalCard
        title={approval ? t.chat.approvalTitle : history?.pendingReview === "question" ? t.chat.questionTitle : history?.pendingReview === "plan" ? t.chat.planTitle : t.chat.approvalTitle}
        badge={t.chat.approvalPending}
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
          <button type="button" className="is-deny" disabled={resolvingApproval} onClick={() => void decideApproval("deny")}>
            {t.chat.deny}<kbd aria-hidden="true">Esc</kbd>
          </button>
          <button type="button" className="is-allow" disabled={resolvingApproval} onClick={() => void decideApproval("allow_once")}>{t.chat.allowOnce}</button>
        </> : <button type="button" onClick={() => setView("terminal")}>{t.chat.openTerminal}</button>}
      >
        {approval ? <>
          <div className="chat-approval-tool"><span>{t.chat.approvalTool}</span><code>{approval.toolName}</code></div>
          {approval.description && <span>{approval.description}</span>}
          {approval.input && <div className="chat-approval-detail">
            <span>{t.chat.approvalInput}</span>
            <pre>{approval.input}</pre>
          </div>}
        {/* 「GUI 托管中」只信后端 ptyManaged(每轮轮询新鲜):此前的前端 managedPtyActive
            在探测循环移交后停更,PTY 退出后悬死 true,这里会一直显示「正在读取终端」。 */}
        </> : <span>{history?.ptyManaged ? t.chat.approvalReadingTerminal : t.chat.approvalInTerminal}</span>}
      </ApprovalCard>}
      {view === "chat" && <footer ref={composeRef} className={"chat-compose" + (composerLocked ? " is-locked" : "")}>
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
        {attachments.length > 0 && <div className="chat-attachments">
          {attachments.map((file) => <div className="chat-attachment" key={file.path} data-tip={file.path}>
            <span className="chat-file-icon">{file.image ? t.chat.attachmentImage : t.chat.attachmentFile}</span>
            <span>{file.name}</span>
            <button type="button" aria-label={`${t.chat.removeAttachment} ${file.name}`} onClick={() => setAttachments((items) => items.filter((item) => item.path !== file.path))}>×</button>
          </div>)}
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
                : t.chat.inputPlaceholder
          }
          onChange={(event) => { setPrompt(event.target.value); setSendError(""); setSlashIndex(0); setSlashDismissed(false); }}
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
          {(modelPresets.length > 0 || modelMenuCommand || history?.model) && <div className="chat-model" ref={modelMenuUi.ref} onKeyDown={modelMenuUi.onKeyDown}>
            <button
              ref={modelMenuUi.btnRef}
              type="button"
              className="chat-model-button"
              disabled={modelPresets.length === 0 && !modelMenuCommand}
              aria-label={t.chat.switchModel}
              aria-expanded={modelPresets.length > 0 ? modelMenu : undefined}
              data-tip={menuWatching ? t.chat.modelMenuOpen : modelPresets.length > 0 || modelMenuCommand ? t.chat.switchModel : undefined}
              // 有静态预设（只有 claude，其 `/model <id>` 接受内联参数）就直接下拉；
              // 其余 CLI 的 `/model` 是交互式菜单，改为把命令发过去，再由屏幕识别把
              // CLI 自己弹出的菜单转成 GUI 按钮——模型清单由 CLI 现给，不必我们维护。
              onClick={() => {
                // 与模式菜单互斥：同时只开一个。
                if (modelPresets.length > 0) { setModeMenu(null); setModelMenu((open) => !open); return; }
                // 菜单已在终端里开着：再点是「收起」而不是重发（重发会打进搜索框）。
                if (menuWatching) { cancelTerminalMenu(); return; }
                if (modelMenuCommand) void openTerminalMenu(modelMenuCommand);
              }}
            >
              {history?.model || t.chat.model}
              {(modelPresets.length > 0 || modelMenuCommand) && <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4"><path d="M6 9l6 6 6-6" /></svg>}
            </button>
            {modelMenu && modelPresets.length > 0 && <div className="dd-menu chat-model-menu" role="menu">
              {modelPresets.map((preset) => {
                const active = history?.model === preset.label;
                return (
                  <button
                    type="button"
                    key={preset.id}
                    role="menuitem"
                    className={"chat-model-item" + (active ? " is-active" : "")}
                    onClick={() => { setModelMenu(false); sendSlash(`/model ${preset.id}`); }}
                  >
                    <span className="chat-model-item-text">
                      <span className="chat-model-item-name">{preset.label}</span>
                      <span className="chat-model-item-desc">{t.chat.modelDesc[preset.id] ?? ""}</span>
                    </span>
                    {active && <svg className="chat-model-check" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round"><path d="M4.5 12.5l5 5 10-11" /></svg>}
                  </button>
                );
              })}
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
              const options = control?.options ?? [];
              const canCycle = Boolean(control?.cycle_input);
              const interactive = options.length > 0 || canCycle;
              const label = t.chat.modeDimensions[dimension] ?? dimension;
              const value = state ? (t.chat.modeNames[state.value] ?? state.value) : "—";
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
                  data-tip={interactive ? (options.length > 0 ? t.chat.switchMode : t.chat.cycleMode) : undefined}
                  onClick={() => {
                    // 与模型菜单互斥：同时只开一个。
                    if (options.length > 0) { setModelMenu(false); setModeMenu((open) => open === dimension ? null : dimension); }
                    else if (control?.cycle_input) void changeMode(dimension, [{ data: control.cycle_input, submit: false }], undefined, control.screen_markers);
                  }}
                >
                  {label}: {value}
                  {options.length > 0
                    ? <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4"><path d="M6 9l6 6 6-6" /></svg>
                    // 双向箭头而不是圆形箭头：后者在任何界面里都读作「刷新」，
                    // 而这里的语义是「在几个值之间轮换」。
                    : canCycle && <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="m16 3 4 4-4 4M20 7H5M8 21l-4-4 4-4M4 17h15" /></svg>}
                </button>
                {modeMenu === dimension && options.length > 0 && <div className="dd-menu chat-model-menu" role="menu">
                  {options.map((option) => {
                    const active = state?.value === option.value;
                    return <button
                      type="button"
                      key={option.value}
                      role="menuitem"
                      className={"chat-model-item" + (active ? " is-active" : "")}
                      onClick={() => {
                        setModeMenu(null);
                        void changeMode(dimension, option.inputs, option.value, control?.screen_markers ?? []);
                      }}
                    >
                      <span className="chat-model-item-name">{t.chat.modeNames[option.value] ?? option.value}</span>
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
          {/* 停止态留空:方块图标已经把「停」说清楚了,旁边再写两个字只是重复,而这一行
              还挤着模型、权限模式、用量。文字说明去 tooltip 和 aria-label(圆钮上都有)。
              元素本身不能省——它的 margin-left:auto 负责把圆钮顶到最右。
              有草稿时挂上 Ctrl+Enter=打断并发送的说明:那个动作已从常驻按钮降成加速键
              (见 textarea 的 onKeyDown),这里是它剩下的唯一去处。 */}
          <span
            className="chat-compose-hint"
            data-tip={canInterrupt && hasDraft ? t.chat.interruptAndSendTip : undefined}
          >{stopMode ? "" : "Enter ↵"}</span>
          {/* 主圆钮双身份:回合运行中且输入框是空的 → 它就是停止键(黑圆方块,一按停当前
              回合)。发出去的消息撤不回,能叫停的只有这个键,它不该藏在草稿或次级按钮后面。
              一旦开始打字,身份让回「发送」——那时用户要的是把话递进去,停回合走左边的
              「打断并发送」。sending 只可能发生在有草稿时,与 stopMode 天然互斥。 */}
          <button
            type="button"
            className={"chat-send-button" + (stopMode ? " is-stop" : "")}
            aria-label={stopMode ? t.chat.interruptNow : sending ? t.chat.sending : t.chat.send}
            data-tip={stopMode ? t.chat.interruptNowTip : undefined}
            onClick={() => { if (stopMode) sendInterrupt(); else void sendPrompt(); }}
            disabled={stopMode ? false : (!prompt.trim() && attachments.length === 0) || sending}
          >
            {stopMode || sending
              ? <svg width="15" height="15" viewBox="0 0 24 24" fill="currentColor"><rect x="6" y="6" width="12" height="12" rx="2" /></svg>
              : <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><path d="M12 19V5M6.5 10.5 12 5l5.5 5.5" /></svg>}
          </button>
        </div>
        {sendError && <div className="chat-send-error" role="alert">
          <span>{sendError}</span>
          {/* 会话确实活在外部终端里：就地给接管入口。此前这里只有一句「请切到终端页接管」，
              用户得自己跨页找按钮，回来还要重打一遍刚才的消息。 */}
          {needsTakeover && <button
            type="button"
            className="chat-send-takeover"
            disabled={sending}
            onClick={() => { const retry = retryRef.current; void takeoverAndRetry(() => retry?.()); }}
          >{t.chat.terminalTakeover}</button>}
        </div>}
        {/* 交互式命令的过渡横幅：识别成功后 terminalAttention 卡片会替掉它；识别不出的
            面板形态（识别器只认光标菜单/编号选择器）也至少给「切到终端 / 收起」的出口。 */}
        {menuWatching && !terminalAttention && <div className="chat-send-error" role="status">
          <span>{t.chat.slashMenuOpened}</span>
          <button type="button" className="chat-send-takeover" onClick={() => setView("terminal")}>{t.chat.terminal}</button>
          <button type="button" className="chat-send-takeover" onClick={cancelTerminalMenu}>{t.chat.slashMenuDismiss}</button>
        </div>}
      </footer>}
      </div>
    </div>
  );
}

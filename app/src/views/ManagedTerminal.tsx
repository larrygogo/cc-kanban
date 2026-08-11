import { useEffect, useRef, useState, type MutableRefObject, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import { appConfirm } from "../confirm";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
// xterm 的样式跟着唯一使用者走(此前在 main.tsx 全局引入,贴纸窗也要付这份 CSS)。
import "@xterm/xterm/css/xterm.css";
import {
  attachBackgroundSession,
  confirmStopSession,
  getSettings,
  isExternallyHeld,
  managedTerminalSnapshot,
  openAttachedTerminal,
  openLink,
  resizeManagedTerminal,
  startManagedTerminal,
  takeoverManagedTerminal,
  writeManagedTerminal,
  type Settings,
} from "../api";
import { useT } from "../i18n";
import type { PtyExitEvent as ExitEvent } from "../generated/contracts/PtyExitEvent";
import type { PtyOutputEvent as OutputEvent } from "../generated/contracts/PtyOutputEvent";
import { terminalAttention, visibleTerminalText, type AttentionGrammar, type TerminalAttention } from "../terminalAttention";

function decodeBase64(data: string): Uint8Array {
  const binary = atob(data);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

/// 这段字节里是否有**画得出来的东西**，而不只是控制序列。
///
/// 全屏 TUI（claude/codex）启动时先甩一大串清屏与光标控制序列，真正的界面要等
/// `--resume` 把 transcript 读完才画得出来——长会话可以是几十秒。若把「收到字节」
/// 当成「初始化完成」，遮罩会在这段空窗期就撤掉，用户面对的是一块没有任何提示的纯黑屏。
function hasVisibleOutput(bytes: Uint8Array): boolean {
  let i = 0;
  while (i < bytes.length) {
    const byte = bytes[i];
    if (byte === 0x1b) {
      i += 1;
      const kind = bytes[i];
      if (kind === 0x5b) {
        // CSI：参数字节之后以 @~ 区间的最终字节收尾。
        i += 1;
        while (i < bytes.length && (bytes[i] < 0x40 || bytes[i] > 0x7e)) i += 1;
      } else if (kind === 0x5d) {
        // OSC：到 BEL 或 ST 为止，串里的标题文本不算可见内容。
        i += 1;
        while (i < bytes.length && bytes[i] !== 0x07 && bytes[i] !== 0x1b) i += 1;
      }
      i += 1;
      continue;
    }
    // 空格与制表符构不成画面——清屏后的空行全是它们。
    if (byte > 0x20 && byte !== 0x7f) return true;
    i += 1;
  }
  return false;
}

/// TUI 迟迟不画东西时的保底：宁可把黑屏交给用户，也不要让 spinner 永远转下去。
const INITIALIZING_TIMEOUT_MS = 25_000;

/// 行高预设 → xterm lineHeight。normal 即历史硬编码的 1.22（改它会让老用户画面变样）。
const LINE_HEIGHTS: Record<string, number> = { compact: 1.1, normal: 1.22, relaxed: 1.45 };

/// 剔除「终端自动应答」形态的序列：CPR 光标位置（`\x1b[n;mR`，含 DECXCPR 的 `?` 变体）、
/// DSR 状态（`\x1b[0n`）、DA1/DA2 设备属性（`…c`）、DECRPM（`…$y`）、OSC 应答（颜色查询
/// 等）、DCS 应答。重连时快照会把整段历史回放进 xterm，历史里 agent 当年的查询（`\x1b[6n`
/// 等）会被 xterm **再答一遍**，迟到的应答经 onData 打进正跑着的 agent 输入框，控制序列
/// 被部分吞掉后剩下孤立尾字符（真实案例：每次重连 claude 的 composer 里多出一个 C）。
/// 只在历史回放窗口内套用；用户按键唯一可能撞形态的是带修饰键的 F3（`\x1b[1;2R`），
/// 在几毫秒的回放窗口里按到它的代价可以忽略。
/// 首帧前的启动探测不需要前端拦截:后端代答后已把查询从流中摘除（pty.rs 的
/// StartupProbeScanner），xterm 根本看不到,不存在要拦的自动应答。
export function stripTerminalReplies(data: string): string {
  // DECRPM($y)的 '?' 必须可选:xterm 对 ANSI 模式查询(CSI Ps $ p)的应答不带 '?'
  // (如 \x1b[4;2$y),只匹配 DEC 私有形态会漏放。CSI-t 是窗口尺寸报告(CSI 18 t 等,
  // windowOptions 开启时 xterm 会应答)——无用户按键以裸 t 收尾,纳入无误伤。
  return data.replace(
    // eslint-disable-next-line no-control-regex
    /\x1b\[\??\d+(?:;\d+)*R|\x1b\[0n|\x1b\[[?>][\d;]*c|\x1b\[\??\d+;\d+\$y|\x1b\[\d+(?:;\d+)*t|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1bP[^\x1b]*\x1b\\/g,
    "",
  );
}

type InverseScanCell = { isInverse(): number };
type InverseScanLine = { length: number; getCell(x: number): InverseScanCell | undefined };
export type InverseScanBuffer = { viewportY: number; getLine(y: number): InverseScanLine | undefined };

/// 在 viewport 里找「孤立的单格反显」——TUI 自绘假光标的形态（kimi 的输入光标就是
/// `\e[7m \e[27m` 一个反显空格，见 capture_ime_cursor 探针）。连排反显（选中行、菜单
/// 焦点项）整段跳过；命中超过一个说明画面里另有反显装饰，多义即放弃（返回 null）。
export function findFakeCaret(buffer: InverseScanBuffer | undefined, rows: number): { x: number; y: number } | null {
  if (!buffer) return null;
  let hit: { x: number; y: number } | null = null;
  for (let row = 0; row < rows; row += 1) {
    const line = buffer.getLine(buffer.viewportY + row);
    if (!line) continue;
    for (let col = 0; col < line.length; col += 1) {
      if (!line.getCell(col)?.isInverse()) continue;
      let end = col;
      while (end + 1 < line.length && line.getCell(end + 1)?.isInverse()) end += 1;
      if (end === col) {
        if (hit) return null;
        hit = { x: col, y: row };
      }
      col = end;
    }
  }
  return hit;
}

type ManagedTerminalProps = {
  /**
   * Agent 自己托管的后台会话。它**不能**被「接管」：正文常常没落盘（claude 的 fork/resume
   * worker 只写两行元数据）， 只会得到 No conversation found + 退出码 1。
   * 画面另有旁路可看（后端的 attach_background_session），所以这里只需收起那个按钮。
   */
  background?: boolean;
  /**
   * 用户在**后台会话**的终端里按了键。这些按键注定无效（worker 不消费 stdin），所以不下发，
   * 转由宿主把用户领到真能发消息的地方（对话页）。
   */
  onBackgroundInput?: () => void;
  sessionId: number;
  status?: string;
  visible?: boolean;
  onUserSubmit?: () => void;
  attentionMarkers?: string[];
  interactivePrompt?: boolean;
  /// 刚发出会弹菜单的命令（如 `/model`）：这段窗口里额外识别光标菜单。
  expectMenu?: boolean;
  /// 识别文法(provider 门控 + 插件声明的选择器锚点)。缺省按 Claude 处理(兼容旧调用),
  /// 生产路径由 ChatWindow 从 chatUi 显式组装传入。
  grammar?: AttentionGrammar;
  onAttention?: (attention: TerminalAttention | null) => void;
  /// 供父组件在自己重启 PTY 后触发偏移复位（对话页发送/切模式也会重启 PTY，
  /// 不止组件内部的 start/takeover 按钮）。
  rearmRef?: MutableRefObject<(() => void) | null>;
  /// 恢复/接管时对启动选项的改选（option id → choice id），随 start/takeover 下发；
  /// 省略 = 沿用会话存的选择。状态归 ChatWindow（与对话页的接管入口共用同一份）。
  resumeOptions?: Record<string, string>;
  /// 渲染在 start/takeover 按钮旁的附加控件（权限改选下拉，由 ChatWindow 构造）。
  takeoverExtra?: ReactNode;
};

export function ManagedTerminal({ sessionId, status, background = false, onBackgroundInput, visible = true, onUserSubmit, attentionMarkers = [], interactivePrompt = false, expectMenu = false, grammar, onAttention, rearmRef: externalRearmRef, resumeOptions, takeoverExtra }: ManagedTerminalProps) {
  const t = useT();
  const hostRef = useRef<HTMLDivElement>(null);
  const terminalRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const [active, setActive] = useState(false);
  const [snapshotReady, setSnapshotReady] = useState(false);
  const [initialized, setInitialized] = useState(false);
  const [starting, setStarting] = useState(false);
  // 结束终端是异步的（杀进程 + 等 pty-exit 回传，Windows 上有几百毫秒延迟）。
  // 没有这个状态，按钮点完毫无变化，用户以为没反应。
  const [stopping, setStopping] = useState(false);
  const [error, setError] = useState("");
  const [exitCode, setExitCode] = useState<number | null | undefined>(undefined);
  const externalRunning = isExternallyHeld(status);
  /// 就地重启（结束终端 → 再接管）后重新对齐输出偏移。新 PTY 的 output_end 从 0 重新计数，
  /// 而 nextOffset 还停在上一个进程的高位，writeOutput 会把新输出全部当成「已写过」丢掉，
  /// 终端就永远定格在旧内容上。effect 内部把重置逻辑挂到这里，供 start/takeover 调用。
  const rearmRef = useRef<(() => void) | null>(null);
  const onUserSubmitRef = useRef(onUserSubmit);
  const onBackgroundInputRef = useRef(onBackgroundInput);
  const backgroundRef = useRef(background);
  const attentionMarkersRef = useRef(attentionMarkers);
  const interactivePromptRef = useRef(interactivePrompt);
  const expectMenuRef = useRef(expectMenu);
  const grammarRef = useRef(grammar);
  const onAttentionRef = useRef(onAttention);
  const visibleRef = useRef(visible);
  const attentionTailRef = useRef("");
  const attentionReportedRef = useRef<string | null>(null);
  const lastScreenRef = useRef("");
  onUserSubmitRef.current = onUserSubmit;
  onBackgroundInputRef.current = onBackgroundInput;
  backgroundRef.current = background;
  attentionMarkersRef.current = attentionMarkers;
  interactivePromptRef.current = interactivePrompt;
  expectMenuRef.current = expectMenu;
  grammarRef.current = grammar;
  onAttentionRef.current = onAttention;
  visibleRef.current = visible;

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    // 终端里的链接遵循终端惯例：Ctrl/Cmd+点击才打开（普通点击留给 TUI 的鼠标交互与选区）。
    // 打开走后端 open_link（限 http/https，与对话 Markdown 链接同一条通道）；被拒时把原因
    // 显示出来，不许无声吞掉——「点了没反应」正是这类问题最难排查的形态。
    const openTerminalLink = (event: MouseEvent, uri: string) => {
      if (!event.ctrlKey && !event.metaKey) return;
      void openLink(uri).catch((e) => setError(String(e)));
    };
    const terminal = new Terminal({
      // OSC 8 超链接（TUI 显式声明的链接）由这里接住；纯文本 URL 的识别在 WebLinksAddon。
      linkHandler: { activate: openTerminalLink },
      cursorBlink: true,
      convertEol: false,
      // "JetBrains Mono" 由 styles.css 的 @font-face 打包提供（不依赖本机安装），管拉丁+符号；
      // 它不含 CJK，中文逐字回退到各平台**真实存在**的好看字体：微软雅黑 / 苹方 / Noto。
      // （曾误写 "Microsoft YaHei Mono"——该字体名不存在，导致中文掉到 Courier New 的宋体兜底，
      //  正是「中文看着奇怪」的来源。）xterm 用等宽网格定位，中文按双宽对齐，非等宽也整齐。
      fontFamily: '"JetBrains Mono", ui-monospace, SFMono-Regular, Consolas, "PingFang SC", "Microsoft YaHei", "Noto Sans CJK SC", sans-serif',
      fontSize: 12,
      lineHeight: 1.22,
      scrollback: 5000,
      // 绿色只留给光标这一格宽的点缀；选区是成片色块，用低饱和灰绿保持清爽。
      theme: { background: "#151617", foreground: "#e7e9e8", cursor: "#55d6ae", selectionBackground: "#31403a" },
    });
    const fit = new FitAddon();
    terminal.loadAddon(fit);
    // 纯文本 URL 的链接化。不装它 xterm 根本不识别正文里的 URL——「Ctrl+点击打不开链接」
    // 的第一层原因就是链接从未存在过。
    terminal.loadAddon(new WebLinksAddon(openTerminalLink));
    terminal.open(host);
    // 按键粘贴：xterm 把 Ctrl+V 当普通组合键吞掉（preventDefault 后向 PTY 发 ^V），
    // 浏览器的原生 paste 事件因此永远不触发——Windows/Linux 上按键粘贴整个失效。
    // 返回 false 让 xterm 完全放行这个 keydown：WebView 对聚焦的隐藏 textarea 执行原生
    // 粘贴，xterm 自带的 paste 监听接住文本（含 bracketed paste 包装）走 onData 下发。
    // 刻意不自己读剪贴板：navigator.clipboard 在 webview 里要额外权限，原生事件路径零依赖。
    // Shift+Insert 是 Windows 终端的习惯粘贴键，一并放行。
    terminal.attachCustomKeyEventHandler((event) => {
      if (event.type !== "keydown") return true;
      const paste = ((event.ctrlKey || event.metaKey) && !event.altKey && event.code === "KeyV")
        || (event.shiftKey && !event.ctrlKey && !event.metaKey && event.code === "Insert");
      return !paste;
    });
    // 上面的放行有个盲区：剪贴板是**图片**时 paste 事件没有文本数据，xterm 的 paste 监听
    // 不产生任何输入，^V 也早被拦下——claude 的原生贴图（^V 让 TUI 自己读系统剪贴板出
    // [Image #N]）在终端页整条断掉。兜底：无文本而有文件（位图）的 paste，补发 ^V 给 CLI。
    // 文本存在时不动——bracketed paste 的既有通路优先。
    const pasteImageFallback = (event: ClipboardEvent) => {
      const data = event.clipboardData;
      if (!data || data.getData("text") || data.files.length === 0) return;
      event.preventDefault();
      void writeManagedTerminal(sessionId, "\x16").catch((e) => setError(String(e)));
    };
    host.addEventListener("paste", pasteImageFallback);
    // ── IME 锚点校正 ──
    // 实测（capture_ime_cursor 探针）：kimi 启动即 `?25l` 隐藏硬件光标、从不恢复，输入框里的
    // 光标是自绘的反显空格；帧尾硬件光标停在最后绘制行的行尾。而 xterm 的组合输入锚点就是
    // 硬件光标（CompositionHelper 按 buffer.x/y 定位），输入法候选栏于是钉在行尾——按硬件
    // 光标锚定 IME 的终端全都如此，属 TUI 侧缺陷，但宿主能救：组合期间找到唯一的假光标格
    // 就把组合视图与隐藏 textarea 改锚过去。xterm 在 compositionstart/update（含其内部
    // setTimeout 重定位）会反复写回硬件光标坐标，用 MutationObserver 盯住 style 每次覆盖；
    // 同值不写，观察器不会自我打环。找不到/多义时不动，维持 xterm 默认行为。
    const helperTextarea = host.querySelector<HTMLTextAreaElement>(".xterm-helper-textarea");
    const compositionView = host.querySelector<HTMLElement>(".composition-view");
    let composing = false;
    const alignIme = () => {
      if (!composing || !helperTextarea) return;
      const caret = findFakeCaret(terminal.buffer?.active, terminal.rows);
      if (!caret) return;
      const screen = host.querySelector<HTMLElement>(".xterm-screen");
      if (!screen || terminal.cols < 1 || terminal.rows < 1) return;
      const left = `${Math.round((caret.x * screen.clientWidth) / terminal.cols)}px`;
      const top = `${Math.round((caret.y * screen.clientHeight) / terminal.rows)}px`;
      for (const el of [helperTextarea, compositionView]) {
        if (!el) continue;
        if (el.style.left !== left) el.style.left = left;
        if (el.style.top !== top) el.style.top = top;
      }
    };
    const imeObserver = new MutationObserver(alignIme);
    const startComposition = () => { composing = true; alignIme(); };
    const endComposition = () => { composing = false; };
    if (helperTextarea) {
      helperTextarea.addEventListener("compositionstart", startComposition);
      helperTextarea.addEventListener("compositionend", endComposition);
      imeObserver.observe(helperTextarea, { attributes: true, attributeFilter: ["style"] });
      if (compositionView) imeObserver.observe(compositionView, { attributes: true, attributeFilter: ["style"] });
    }
    terminalRef.current = terminal;
    fitRef.current = fit;
    requestAnimationFrame(() => fit.fit());

    const input = terminal.onData((data) => {
      // 历史回放窗口内，xterm 对回放查询的自动应答不得下发 PTY（见 stripTerminalReplies）。
      // 预绘期不需要额外拦 CPR：启动探测由后端代答并**从流中摘除**（pty.rs 的
      // StartupProbeScanner），xterm 根本看不到就不会自动应答。实时查询的应答原样
      // 转发——那是 TUI 正在等的；用户真实按键不匹配回放过滤的形态，照常放行。
      const payload = replayingHistory ? stripTerminalReplies(data) : data;
      if (!payload) return;
      // 后台会话的 worker 不消费 stdin（取证见后端 bgpty.rs 的模块文档）：这些按键写下去
      // 服务端会收、PTY 会写，claude 那头就是不理。与其让用户打完一整句再弹「不接受终端
      // 按键」，不如第一下就把他送到真能发消息的地方去。
      if (backgroundRef.current) {
        onBackgroundInputRef.current?.();
        return;
      }
      if (payload.includes("\r")) {
        onUserSubmitRef.current?.();
      }
      // 写失败必须可见：典型场景是整段粘贴超过后端单次输入上限被拒——
      // 静默吞掉的话，粘贴无声消失，终端画面纹丝不动。
      void writeManagedTerminal(sessionId, payload).catch((e) => setError(String(e)));
    });
    let unOutput: (() => void) | undefined;
    let unExit: (() => void) | undefined;
    let cancelled = false;
    let hasWrittenOutput = false;
    let painted = false;
    let snapshotApplied = false;
    // 快照全量回放进行中(true 期间 onData 过滤终端自动应答);该次 write 的完成回调清位。
    let replayingHistory = false;
    let nextOffset = 0;
    const bufferedOutput: OutputEvent[] = [];
    let bufferedExit: ExitEvent | null = null;
    let snapshotTimer = 0;
    const attentionDecoder = new TextDecoder();
    // IME 候选栏对齐依赖 xterm 的字形测量：打包的 JetBrains Mono 由 @font-face 异步加载，
    // 终端常在字体就绪前完成测量——单元格宽高按回退字体计算，光标的像素坐标随行列越偏
    // 越远，组合输入期跟随光标的隐藏 textarea（输入法候选栏的锚点）就落不到输入框上。
    // 字体就绪后强制重测：同值赋值会被 options 服务去重，先动一格字号再改回才触发。
    // jsdom 没有 document.fonts，可选链让测试环境静默跳过。
    void document.fonts?.load('12px "JetBrains Mono"').then(() => {
      if (cancelled) return;
      const size = terminal.options.fontSize ?? 12;
      terminal.options.fontSize = size + 1;
      terminal.options.fontSize = size;
      fit.fit();
      // 重测可能改变行列数，PTY 侧要跟着调，否则 TUI 按旧尺寸画、连输入框的位置都是错的。
      if (visibleRef.current && terminal.cols > 1 && terminal.rows > 1) {
        void resizeManagedTerminal(sessionId, terminal.cols, terminal.rows).catch(() => {});
      }
    }).catch(() => {});
    // 终端样式（字号/行高）跟随设置：挂载时读一次，settings-changed 到达即热应用——
    // xterm 支持运行时改 options，改完重新 fit 并把新行列数下发 PTY（网格变了 TUI 必须重画，
    // 否则输入框位置按旧尺寸算全是错的）。字号钳到 [8,24]：防手改 settings.json 塞出 0 号字。
    const applyTermStyle = (s: Settings) => {
      const size = Math.min(24, Math.max(8, s.terminal_font_size ?? 12));
      const line = LINE_HEIGHTS[s.terminal_line_height] ?? LINE_HEIGHTS.normal;
      if (terminal.options.fontSize === size && terminal.options.lineHeight === line) return;
      terminal.options.fontSize = size;
      terminal.options.lineHeight = line;
      fit.fit();
      if (visibleRef.current && terminal.cols > 1 && terminal.rows > 1) {
        void resizeManagedTerminal(sessionId, terminal.cols, terminal.rows).catch(() => {});
      }
    };
    void getSettings().then((s) => { if (!cancelled) applyTermStyle(s); }).catch(() => {});
    let unSettings: (() => void) | undefined;
    listen<Settings>("settings-changed", ({ payload }) => applyTermStyle(payload)).then((un) => {
      if (cancelled) un();
      else unSettings = un;
    }).catch(() => {});
    // 保底：TUI 一直不画东西也不能永远停在 spinner 上。
    const giveUpTimer = window.setTimeout(() => {
      if (!cancelled) { painted = true; setInitialized(true); }
    }, INITIALIZING_TIMEOUT_MS);
    // 只有画得出东西的输出才算初始化完成；清屏/光标序列不算（见 hasVisibleOutput）。
    const markPainted = (bytes: Uint8Array) => {
      if (painted || !hasVisibleOutput(bytes)) return;
      painted = true;
      window.clearTimeout(giveUpTimer);
      setInitialized(true);
    };
    // 提示从屏幕上消失(在终端里答掉了/界面翻页了)后连续多少次扫描不再匹配,才发布 null
    // 自动收卡。>1 是为了骑过 TUI 的分笔重绘:整屏重画的中间帧可能短暂不匹配,立即清卡
    // 会闪烁——而清卡又重置了签名去重,重绘完成后同一屏会再弹一次,循环闪。
    const ATTENTION_CLEAR_STREAK = 3;
    let attentionMissStreak = 0;
    // 已经因「屏幕上没有提示」发布过一次 null,再 miss 也不重复发,避免每帧刷 null。
    // 匹配到新提示时复位。
    let clearPublished = false;
    const reportAttention = (text: string) => {
      if (text) lastScreenRef.current = text;
      const attention = terminalAttention(text, attentionMarkersRef.current, interactivePromptRef.current, expectMenuRef.current, grammarRef.current);
      if (!attention) {
        // 此前这里直接 return——attention 状态只置不清,误报或已在终端里处理过的提示会
        // 永久钉住卡片、锁死对话页输入框。现在:屏幕持续不匹配就发布 null 收卡,并重置
        // 签名去重,让真正的下一个提示(哪怕内容相同)还能再弹。
        //
        // 收卡**不能**以「本组件报告过」为前提:启动提示是 ChatWindow 的
        // waitForTerminalReady 直接置进去的,本组件的 attentionReportedRef 仍是 null。
        // 曾据此门控,于是用户在终端里答掉信任提示后,那张卡在对话页永久钉死、把输入框
        // 一起锁住,后续真正的提问卡(渲染条件含 !terminalAttention)再也没机会出现。
        if (clearPublished) return;
        // 首屏尚未画出来时屏幕是空的,此刻的「不匹配」不算证据——照发 null 会把
        // 启动提示在 attach 重放到达前抹掉。
        if (!painted) return;
        attentionMissStreak += 1;
        if (attentionMissStreak >= ATTENTION_CLEAR_STREAK) {
          attentionMissStreak = 0;
          attentionReportedRef.current = null;
          clearPublished = true;
          onAttentionRef.current?.(null);
        } else {
          // 扫描由输出事件驱动;最后一次重绘后终端可能归于安静,凑不满连击就永远
          // 清不掉。miss 期间自我续排,直到清卡或重新匹配。
          window.setTimeout(() => { if (!cancelled) scheduleAttentionScan(); }, 200);
        }
        return;
      }
      attentionMissStreak = 0;
      clearPublished = false;
      const signature = `${attention.id}\0${attention.text}\0${JSON.stringify(attention.options)}`;
      if (signature === attentionReportedRef.current) return;
      // 信任页本身就是当前需要展示的有效画面，不能继续被“正在初始化”遮罩盖住。
      if (!painted) {
        painted = true;
        window.clearTimeout(giveUpTimer);
        setInitialized(true);
      }
      attentionReportedRef.current = signature;
      onAttentionRef.current?.(attention);
    };
    const renderedScreen = () => {
      // 原始 PTY 流里的光标回退、逐行清除无法靠正则完整还原。xterm 已经替我们执行了
      // 这些控制序列，直接读它的当前 viewport 才是用户此刻真正看到的画面。
      const buffer = terminal.buffer?.active;
      if (!buffer) return visibleTerminalText(attentionTailRef.current);
      const first = Math.max(0, buffer.viewportY);
      const lines: string[] = [];
      for (let row = first; row < Math.min(buffer.length, first + terminal.rows); row += 1) {
        const line = buffer.getLine(row)?.translateToString(true).trimEnd();
        if (line) lines.push(line);
      }
      return lines.slice(-80).join("\n").trim();
    };
    const inspectAttention = (bytes: Uint8Array) => {
      attentionTailRef.current = (attentionTailRef.current + attentionDecoder.decode(bytes, { stream: true })).slice(-16_384);
    };
    // 整屏抓取 + 多条回溯正则不便宜，不能每个输出 chunk 都跑一遍——构建/日志刷屏时
    // 事件很密，逐帧扫描会拖垮主线程。合并成至多每 150ms 一次的尾随节流：持续输出时
    // 有界地扫，输出停下后最后一批也保证在 150ms 内被扫到（审批/信任页正是这种停帧画面）。
    let attentionScanTimer = 0;
    const scheduleAttentionScan = () => {
      if (attentionScanTimer) return;
      attentionScanTimer = window.setTimeout(() => {
        attentionScanTimer = 0;
        if (cancelled) return;
        const screen = renderedScreen();
        if (screen) reportAttention(screen);
      }, 150);
    };
    const writeOutput = (payload: OutputEvent) => {
      const bytes = decodeBase64(payload.data);
      const offset = Number.isFinite(payload.offset) ? payload.offset : nextOffset;
      const end = offset + bytes.length;
      if (end <= nextOffset) return;
      const visible = offset < nextOffset ? bytes.slice(nextOffset - offset) : bytes;
      if (visible.length === 0) return;
      hasWrittenOutput = true;
      nextOffset = end;
      inspectAttention(visible);
      markPainted(visible);
      terminal.write(visible, () => {
        // xterm 按入队顺序处理 chunk:回放那笔 write 解析完(它触发的自动应答也都发完)
        // 这里才回调,此后到达的都是实时数据,应答恢复放行。后续写清一个本就 false 的
        // 标志无副作用。
        replayingHistory = false;
        scheduleAttentionScan();
      });
    };
    const applyExit = (payload: ExitEvent) => {
      window.clearTimeout(snapshotTimer);
      window.clearTimeout(giveUpTimer);
      painted = true;
      setActive(false);
      setSnapshotReady(true);
      // 进程没了就没有下一帧可等：必须离开初始化态，把退出结果交给遮罩。
      setInitialized(true);
      setExitCode(payload.code);
      terminal.write(`\r\n\x1b[90m[Meowo: process exited${payload.code == null ? "" : ` (${payload.code})`}]\x1b[0m\r\n`);
    };
    // 首帧传 0 拿全量（要完整回放历史），补查轮询带 nextOffset 只取增量。
    // writeOutput 本来就按 startOffset 做区间裁剪，增量返回天然兼容。
    const inspectSnapshot = () => managedTerminalSnapshot(sessionId, hasWrittenOutput ? nextOffset : 0).then((snapshot) => {
      if (cancelled) return;
      setSnapshotReady(true);
      setActive(snapshot.active);
      setExitCode(snapshot.exited ? snapshot.exitCode : undefined);
      // 重对齐终局:请求的缺口段已被后端 1MiB backlog 淘汰(前端整体落后太多),
      // 字节永久丢失,重试无意义。reset 后按现存 backlog 全量重画(远超一屏,足以
      // 还原可见画面);回放的是历史,里面的查询都答过,拦 xterm 的重复应答。
      if (hasWrittenOutput && snapshot.data && Number.isFinite(snapshot.startOffset) && snapshot.startOffset > nextOffset) {
        terminal.reset();
        nextOffset = snapshot.startOffset;
        replayingHistory = true;
      }
      if (snapshot.data) {
        // 首次全量回放(重连/重开窗口)才拦应答:里面全是答过的旧查询。增量补查是准实时
        // 输出,agent 可能正等着这些应答,不拦。启动探测(ESC[6n)的代答不在这里做——
        // 曾在此按回放内容补答过,但临时 id→真实 id 的重挂会重扫同一段 backlog,同一个
        // 查询被答两次,多出的应答落进 composer 成杂字符;现在由后端 reader 单趟代答
        // (pty.rs StartupProbeScanner),回放路径只负责「不放行 xterm 的重复应答」。
        if (!hasWrittenOutput) replayingHistory = true;
        writeOutput({
          sessionId,
          offset: Number.isFinite(snapshot.startOffset) ? snapshot.startOffset : 0,
          data: snapshot.data,
        });
        // writeOutput 可能因区间裁剪空转(没写就没有清位回调):hasWrittenOutput 仍为
        // false 说明确实没写,标志必须当场收回,否则实时应答被永久拦截。
        replayingHistory = replayingHistory && hasWrittenOutput;
      }
      // data 是从 startOffset 起的增量，兜底算末尾要从 startOffset 加起，
      // 直接拿长度当绝对末尾会把偏移算小，之后的事件会被重复写一遍。
      const start = Number.isFinite(snapshot.startOffset) ? snapshot.startOffset : 0;
      nextOffset = Math.max(
        nextOffset,
        Number.isFinite(snapshot.endOffset)
          ? snapshot.endOffset
          : start + (snapshot.data ? decodeBase64(snapshot.data).length : 0),
      );
      snapshotApplied = true;
      bufferedOutput.sort((a, b) => a.offset - b.offset).forEach(writeOutput);
      bufferedOutput.length = 0;
      if (bufferedExit) { applyExit(bufferedExit); bufferedExit = null; }
      // PTY 可能先 active、后输出首屏；在此期间保持初始化遮罩并补查快照，避免监听器
      // 尚未注册完成时漏掉极早的一段输出，最终永远停在黑屏或加载态。
      // 补查只为等**第一批**字节：拿到就停，之后的帧走 pty-output 事件——每次快照都会
      // 把整个 backlog（可达 1MB）编码重传一遍，不能拿它轮询到界面画出来为止。
      if ((snapshot.active || sessionId < 0) && !hasWrittenOutput && !snapshot.exited) {
        snapshotTimer = window.setTimeout(() => void inspectSnapshot(), 120);
      }
    }).catch(() => {
      if (!cancelled) {
        snapshotApplied = true;
        bufferedOutput.sort((a, b) => a.offset - b.offset).forEach(writeOutput);
        bufferedOutput.length = 0;
        if (bufferedExit) { applyExit(bufferedExit); bufferedExit = null; }
        setSnapshotReady(true);
      }
    });

    // 缺口重对齐的排程。独立 timer,**不能**复用 snapshotTimer——每个 pty-output 事件
    // 都会 clearTimeout(snapshotTimer)(见监听器),输出持续刷新时重对齐会被永远取消。
    // 防抖 80ms 合并连发的缺口;期间 snapshotApplied 已为 false,后续事件只进缓冲、
    // 不再重复排程,天然单飞。
    let resyncTimer = 0;
    const scheduleResync = () => {
      window.clearTimeout(resyncTimer);
      resyncTimer = window.setTimeout(() => { if (!cancelled) void inspectSnapshot(); }, 80);
    };

    // 就地重启后把偏移归零并重新拉一次快照。新 PTY 从 0 重新计数，沿用旧的 nextOffset
    // 会让 writeOutput 把所有新输出判成「已写过」而丢弃（终端定格在旧内容）。
    rearmRef.current = () => {
      if (cancelled) return;
      window.clearTimeout(snapshotTimer);
      // 排程中的重对齐/扫描读的都是旧进程的画面，重启后不再有意义。
      window.clearTimeout(resyncTimer);
      window.clearTimeout(attentionScanTimer);
      attentionScanTimer = 0;
      nextOffset = 0;
      hasWrittenOutput = false;
      painted = false;
      attentionReportedRef.current = null;
      attentionTailRef.current = "";
      lastScreenRef.current = "";
      snapshotApplied = false;
      bufferedOutput.length = 0;
      bufferedExit = null;
      terminal.reset();
      setInitialized(false);
      setExitCode(undefined);
      void inspectSnapshot();
    };
    if (externalRearmRef) externalRearmRef.current = () => rearmRef.current?.();
    const outputListener = listen<OutputEvent>("pty-output", ({ payload }) => {
      if (payload.sessionId === sessionId) {
        window.clearTimeout(snapshotTimer);
        setActive(true);
        setSnapshotReady(true);
        setExitCode(undefined);
        if (!snapshotApplied) {
          bufferedOutput.push(payload);
        } else if (hasWrittenOutput && Number.isFinite(payload.offset) && payload.offset > nextOffset) {
          // 偏移出现缺口:后端在 UI 卡顿时保 agent 不保画面(emit 队列超时丢帧,
          // 合帧线程刻意不抹平洞)。缺口两侧直接续写会画面错乱——转入缓冲并拉快照
          // 补齐,快照回放后 buffer 按 offset 排序合并,复用首挂载的同一条路径。
          snapshotApplied = false;
          bufferedOutput.push(payload);
          scheduleResync();
        } else {
          writeOutput(payload);
        }
      }
    });
    const exitListener = listen<ExitEvent>("pty-exit", ({ payload }) => {
      if (payload.sessionId === sessionId) {
        if (snapshotApplied) applyExit(payload);
        else bufferedExit = payload;
      }
    });
    Promise.all([outputListener, exitListener]).then(([outputUnlisten, exitUnlisten]) => {
      if (cancelled) {
        outputUnlisten();
        exitUnlisten();
        return;
      }
      unOutput = outputUnlisten;
      unExit = exitUnlisten;
      // 监听器就绪后再取快照；期间到达的帧按 offset 在快照之后去重回放。
      void inspectSnapshot();
    }).catch(() => {
      if (!cancelled) void inspectSnapshot();
    });

    let resizeTimer = 0;
    const observer = new ResizeObserver(() => {
      window.clearTimeout(resizeTimer);
      resizeTimer = window.setTimeout(() => {
        fit.fit();
        if (visibleRef.current && terminal.cols > 1 && terminal.rows > 1) {
          void resizeManagedTerminal(sessionId, terminal.cols, terminal.rows).catch(() => {});
        }
      }, 80);
    });
    observer.observe(host);
    return () => {
      cancelled = true;
      window.clearTimeout(snapshotTimer);
      window.clearTimeout(resyncTimer);
      window.clearTimeout(giveUpTimer);
      window.clearTimeout(resizeTimer);
      window.clearTimeout(attentionScanTimer);
      observer.disconnect();
      imeObserver.disconnect();
      host.removeEventListener("paste", pasteImageFallback);
      helperTextarea?.removeEventListener("compositionstart", startComposition);
      helperTextarea?.removeEventListener("compositionend", endComposition);
      input.dispose();
      unOutput?.();
      unExit?.();
      unSettings?.();
      terminal.dispose();
      terminalRef.current = null;
      fitRef.current = null;
      rearmRef.current = null;
      if (externalRearmRef) externalRearmRef.current = null;
    };
  }, [sessionId, externalRearmRef]);

  // capability 查询可能比 PTY 首屏稍晚返回。提示文字先到、markers 后到时也要立刻补判，
  // 不能等一个可能永远不会来的后续输出 chunk。
  const attentionMarkerKey = attentionMarkers.join("\0");
  // 文法(锚点/provider)可能晚于首屏到达(chatUi 是异步查询):变化时用最后一屏复扫,
  // 与 markers 晚到的补投递同一套逻辑。
  const grammarKey = JSON.stringify(grammar ?? null);
  useEffect(() => {
    const attention = terminalAttention(lastScreenRef.current || attentionTailRef.current, attentionMarkers, interactivePrompt, expectMenu, grammar);
    if (!attention) return;
    const signature = `${attention.id}\0${attention.text}\0${JSON.stringify(attention.options)}`;
    if (signature === attentionReportedRef.current) return;
    attentionReportedRef.current = signature;
    setInitialized(true);
    onAttentionRef.current?.(attention);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [attentionMarkerKey, grammarKey, interactivePrompt, expectMenu]);

  // 隐藏期间容器尺寸为 0，xterm 会按 0 列算布局。切回来立刻 fit 一次并聚焦——
  // ResizeObserver 虽然也会触发，但带 80ms 防抖，中间会闪一帧错位的画面。
  useEffect(() => {
    if (!visible) return;
    const terminal = terminalRef.current;
    const fit = fitRef.current;
    if (!terminal || !fit) return;
    const raf = requestAnimationFrame(() => {
      fit.fit();
      if (terminal.cols > 1 && terminal.rows > 1) {
        void resizeManagedTerminal(sessionId, terminal.cols, terminal.rows).catch(() => {});
      }
      terminal.focus();
    });
    return () => cancelAnimationFrame(raf);
  }, [visible, sessionId]);

  const initializing = !snapshotReady || ((active || sessionId < 0) && !initialized);

  const start = async () => {
    const terminal = terminalRef.current;
    if (!terminal) return;
    setStarting(true);
    setStopping(false);
    setError("");
    terminal.focus();
    try {
      await startManagedTerminal(sessionId, terminal.cols || 80, terminal.rows || 24, resumeOptions);
      setActive(true);
      // 新 PTY 的偏移从 0 重新计数，必须归零重拉，否则新输出会被当成旧数据丢弃。
      rearmRef.current?.();
    } catch (e) {
      setError(String(e));
    } finally {
      setStarting(false);
    }
  };

  /// 重新接上后台会话的画面旁路。首次接入由 ChatWindow 在拿到 history.background 时发起，
  /// 但它可能失败（花名册里暂时查不到、socket 还没起来），而这类会话我们又拉不起来——
  /// 唯一能做的就是再试一次接。没有这个入口时，一次失败就永远停在空画面上。
  const reattach = async () => {
    setStarting(true);
    setError("");
    try {
      await attachBackgroundSession(sessionId);
      rearmRef.current?.(); // 偏移归零重拉：接上的是别人已经跑了一阵的 PTY。
    } catch (e) {
      setError(String(e));
    } finally {
      setStarting(false);
    }
  };

  const takeover = async () => {
    const terminal = terminalRef.current;
    if (!terminal) return;
    // 确认框走应用内模态(appConfirm)。**不是 `window.confirm`**:后者在 Tauri 的
    // webview(尤其 macOS WKWebView)里会被直接吞掉、恒返回 false;系统原生 MessageBox
    // 与应用样式脱节,已弃用。Host 挂在 ChatWindow 根上(本组件总在其内渲染)。
    const yes = await appConfirm(t.chat.terminalTakeoverConfirm, {
      title: t.chat.terminalTakeover,
      danger: true,
    });
    if (!yes) return;
    setStarting(true);
    setStopping(false);
    setError("");
    terminal.focus();
    try {
      await takeoverManagedTerminal(sessionId, terminal.cols || 80, terminal.rows || 24, resumeOptions);
      setActive(true);
      rearmRef.current?.();
    } catch (e) {
      setError(String(e));
    } finally {
      setStarting(false);
    }
  };

  const stop = async () => {
    // 结束是破坏性操作(直接杀掉正在跑的 Agent 进程):确认+停止走与对话页标题栏共用的
    // confirmStopSession(api.ts)。确认后立刻置 busy:按钮转「正在结束…」并禁用,直到
    // 进程退出(pty-exit 把 active 设 false,整个操作区随之卸载)。成功后不清 stopping。
    setError("");
    try {
      await confirmStopSession(
        sessionId,
        { title: t.chat.terminalStop, message: t.chat.endSessionConfirm },
        () => setStopping(true),
      );
    } catch (e) {
      // 失败要能重试：终端看着还活着，得把状态退回可点。
      setError(String(e));
      setStopping(false);
    }
  };

  return (
    <div className="managed-terminal">
      <div className="managed-terminal-host" ref={hostRef} />
      {initializing && (
        <div className="managed-terminal-cover is-initializing" role="status">
          <i className="managed-terminal-spinner" />
          <div>{t.chat.terminalInitializing}</div>
        </div>
      )}
      {!initializing && !active && (
        <div className="managed-terminal-cover">
          {/* 后台会话的「没画面」有两种截然不同的成因，此前一律说成「已结束」：worker 明明
              还在跑、只是旁路没接上时，那句话是错的，而且不给任何出路。只有拿到退出码
              （worker 真的退了）才说结束，否则说「没接上」并给一次重接。 */}
          <div className="managed-terminal-cover-msg">{error || (background
            ? (exitCode !== undefined ? t.chat.terminalBackgroundGone : t.chat.terminalBackgroundLost)
            : exitCode !== undefined ? t.chat.terminalExited(exitCode) : externalRunning ? t.chat.terminalExternal : t.chat.terminalReady)}</div>
          {/* 后台会话不给接管/启动按钮：那两条路对它必然失败（见 background 的说明）；
              还没拿到退出码时给「重新接入」——唯一对它有效的动作。 */}
          {background
            ? exitCode === undefined && (
              <button type="button" onClick={() => void reattach()} disabled={starting}>
                {starting ? t.chat.terminalStarting : t.chat.terminalBackgroundRetry}
              </button>
            )
            : (
              /* 权限改选（ChatWindow 构造）与动作按钮并排成一行：各自居中堆叠时两个
                 宽度互不相干的方块上下错落，读起来像散落的控件而不是一组操作。 */
              <div className="managed-terminal-cover-actions">
                {takeoverExtra}
                <button type="button" onClick={() => void (externalRunning ? takeover() : start())} disabled={starting}>
                  {starting ? t.chat.terminalStarting : externalRunning ? t.chat.terminalTakeover : t.chat.terminalStart}
                </button>
              </div>
            )}
        </div>
      )}
      {active && (
        <div className="managed-terminal-actions">
          {/* 后端刻意让 attach 失败可见（不静默回退 GUI），前端吞掉就前功尽弃；
              结束失败同理——终端看起来还活着，用户会以为已经停了。 */}
          <button type="button" onClick={() => { setError(""); void openAttachedTerminal(sessionId).catch((e) => setError(String(e))); }}>{t.chat.terminalAttach}</button>
          <button type="button" disabled={stopping} onClick={() => void stop()}>
            {stopping ? t.chat.terminalStopping : t.chat.terminalStop}
          </button>
        </div>
      )}
      {active && error && (
        // 容器只挂 role="alert"，关闭动作收进内嵌按钮——同一元素身兼 button 与 alert 两个角色会冲突。
        <div className="managed-terminal-error" role="alert">
          <span>{error}</span>
          <button type="button" aria-label={t.chat.close} onClick={() => setError("")}>×</button>
        </div>
      )}
    </div>
  );
}

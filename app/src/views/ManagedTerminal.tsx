import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent, type MutableRefObject, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import { appConfirm } from "../confirm";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { SearchAddon } from "@xterm/addon-search";
import { WebglAddon } from "@xterm/addon-webgl";
import { UnicodeGraphemesAddon } from "@xterm/addon-unicode-graphemes";
// xterm 的样式跟着唯一使用者走(此前在 main.tsx 全局引入,贴纸窗也要付这份 CSS)。
import "@xterm/xterm/css/xterm.css";
import {
  attachBackgroundSession,
  clipboardText,
  confirmStopSession,
  getSettings,
  isExternallyHeld,
  managedTerminalBinding,
  managedTerminalSnapshot,
  openAttachedTerminal,
  openLink,
  registerTerminalViewer,
  managedTerminalGrid,
  resizeManagedTerminal,
  revealPathInFileManager,
  startManagedTerminal,
  takeoverManagedTerminal,
  unregisterTerminalViewer,
  writeManagedTerminal,
  type Settings,
} from "../api";
import { useT } from "../i18n";
import { formatBackendError } from "../i18n/errors";
import { pushEscLayer } from "../escLayers";
import { useDismissable } from "../hooks/useDismissable";
import type { PtyExitEvent as ExitEvent } from "../generated/contracts/PtyExitEvent";
import type { PtyOutputEvent as OutputEvent } from "../generated/contracts/PtyOutputEvent";
import { terminalAttention, visibleTerminalText, type AttentionGrammar, type TerminalAttention } from "../terminalAttention";
import { remoteUi } from "../remoteMode";

/** 远程模式补查周期（ms）。手机端收不到 pty-output 事件,靠定时补查增量快照喂满屏幕识别
 *  （AskUserQuestion 表单检测跑在这条通道上）。带 nextOffset 只取增量,不重传 backlog。 */
const REMOTE_SNAPSHOT_POLL_MS = 700;

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

/// 通用启动提示(登录/凭据类)的识别窗口:挂载/就地重启起算。窗口内足够 attach 回放与
/// 慢启动 CLI 画出登录页;窗口外这些整会话通用的规则不再参与扫描,防止正跑着的会话
/// 因输出里引用登录话术被误锁(provider 声明的 markers/patterns 不受此窗限制)。
const GENERIC_STARTUP_WINDOW_MS = 20_000;

/// 重对齐增量超过它就跳尾（base64 字符数，≈256KB 原始字节）：落后这么多时把全量灌给
/// xterm 是数秒级的渲染卡死，而终端是实时视图——落后就该直接看最新画面。
const JUMP_TAIL_B64_CHARS = 350_000;
/// 跳尾后回放的尾部字节数：远超一屏，足以重建可见画面与提示识别所需的尾部文本。
const TAIL_KEEP_BYTES = 64 * 1024;

/// 输出流停滞阈值。CC 干活时 TUI 以百毫秒级持续重绘（spinner/计时都在动），
/// running 状态下 30 秒零字节几乎只有一种解释：ConPTY 管道内核僵死（conhost 死锁，
/// reader 永远读不到字节）。那是微软侧的内核对象故障，用户态无法自愈——能做的是
/// 明确告知并指路重启，而不是让用户对着静止画面猜。
export const STREAM_STALL_MS = 30_000;

/// 输出流停滞判定（纯函数，供组件定时评估与单测）。只对「本 GUI 托管、transcript
/// 说 agent 正在跑」的会话生效：后台会话没有实时帧通道、外部占用/已退出本就无流，
/// waiting/idle 时 TUI 安静是常态，都不参与判定——宁可漏报，不可把正常空闲谎报成卡死。
/// reviewPending（待审批/屏幕提示在场）同为豁免：回合没结束 status 仍是 running，但
/// TUI 画完权限框/表单后就一个字节都不再输出，静止正是「在等人」的常态而非僵死
/// （实拍误报：审批横幅与 TUI 授权框同屏时，30s 一到就弹「ConPTY 疑似卡死」）。
/// 注意本函数只看状态位；组件节拍上还有第二级判据——画面停在输入提示符（唯一自绘
/// 假光标）时同样豁免，因为 status 管线在 kimi 这类 TUI 上会滞后/漏翻成 waiting。
export function terminalStreamStalled(args: { active: boolean; background: boolean; status: string | undefined; reviewPending: boolean; lastByteAt: number; now: number }): boolean {
  return args.active && !args.background && args.status === "running" && !args.reviewPending && args.now - args.lastByteAt > STREAM_STALL_MS;
}

/// 在一行终端文本里找出**像文件路径**的片段（文件路径 link provider 用，纯函数便于单测）。
/// 文件路径是终端里最高频的可点内容，此前一律不可点（只有 URL 走 WebLinksAddon）。
/// 口径刻意保守——误链一个普通单词的代价只是 Ctrl+悬停多出一条下划线：
/// - 带显式锚头的（盘符 `C:\` / `~` / `./` `../` / 以 `/` 开头）直接收；
/// - 裸相对路径要求至少一段含「.」（`src/main.ts` 收，`and/or` 这类连词不收）；
/// - 前一个字符是路径/URL 字符的命中跳过——那是更长串（URL、更长路径）的中段，
///   URL 归 WebLinksAddon；
/// - 尾部的 `.,:;!?)]}"'` 视作句读剥掉；行号（`file.ts:12`）的冒号不在段字符集内，
///   本来就不进命中。
/// 返回的 end 是**开区间**字符串下标。
export function scanLineForFilePaths(line: string): { text: string; start: number; end: number }[] {
  // 段字符不含括号/逗号/冒号:`(src/main.ts)`、`file.ts:12`、`a/b, b/c` 的句读都不会进命中。
  const SEG = "[\\w.@+~-]";
  const re = new RegExp(`(?:[A-Za-z]:[\\\\/]|~[\\\\/]|\\.{1,2}[\\\\/]|\\/)${SEG}*(?:[\\\\/]${SEG}+)*|${SEG}+(?:[\\\\/]${SEG}+)+`, "g");
  const hits: { text: string; start: number; end: number }[] = [];
  let m: RegExpExecArray | null;
  while ((m = re.exec(line))) {
    if (m.index > 0 && /[\w.:/\\@-]/.test(line[m.index - 1])) continue;
    const text = m[0].replace(/[.,:;!?)\]}"']+$/, "");
    if (!text) continue;
    const anchored = /^([A-Za-z]:[\\/]|~[\\/]|\.{1,2}[\\/]|\/)/.test(text);
    if (!anchored && !text.includes(".")) continue;
    hits.push({ text, start: m.index, end: m.index + text.length });
  }
  return hits;
}

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
/// 一段 onData 是否**只是**无按键的鼠标移动上报(SGR `CSI < Cb ; Cx ; Cy M`,Cb = 32+3
/// 即 35,修饰键再加 4/8/16)。TUI 开了 `?1003h`(任意移动上报,claude 全屏渲染器即如此)
/// 后 xterm 每跨一个单元格就发一次,快速划过终端每秒上百次;每次一趟 IPC + 后端
/// spawn_blocking,debug 构建下足以把按键排在后面(实拍「终端变得很卡」)。移动事件是
/// 位置采样,只有最新一条有意义——可以合并,不像按键一个都不能丢。
/// 移动上报的合并窗口:一帧。再长鼠标悬停反馈会明显拖尾,再短合并不到几条。
const MOTION_FLUSH_MS = 16;

/// PTY 的网格上限，对应后端 `pty::size` 的 clamp(2, 500)。可见期的网格自愈拿本地行列数
/// 与快照里的 PTY 尺寸比对，不先按同一边界收一次的话，超限的那一侧永远比不相等，
/// 每帧都在重发 resize。
const PTY_GRID_MAX = 500;
/// 同一目标尺寸的网格自愈重试间隔。resize 会失败（撞上后端有界锁、会话正在重启），
/// 失败就该再试；但要是后端因为别的理由永远不接受这个尺寸，也不能每帧一发——
/// TUI 每次都吃一发 SIGWINCH 整屏重排。
const GRID_RESYNC_RETRY_MS = 5_000;
/// 网格自愈的比对间隔。错位是「一直错着」而不是一闪而过，秒级发现足够；而每次比对
/// 就是一趟只回两个数的 IPC，压不到什么。
const GRID_POLL_MS = 4_000;

/// 网格自愈的判定：本地网格 `local` 与 PTY 生效尺寸 `pty` 不一致时，返回该下发的目标
/// 尺寸；无需下发时返回 null。调度与 IPC 留在组件里，这里只有判据。
///
/// - `pty` 的任一维 ≤1 = 尺寸未知（会话不在、后台旁路、还没设过），跳过：不能拿「不知道」
///   当成「不一致」去发 resize。
/// - 目标尺寸按 [`PTY_GRID_MAX`] 收一次，与后端 `pty::size` 的 clamp 同一边界——不收的话
///   超限的那一侧永远比不相等，每一轮都在重发。
/// - `last` 是上次下发的目标与时刻：同一目标在 [`GRID_RESYNC_RETRY_MS`] 内不重发。失败要
///   重试（撞上后端有界锁、会话正在重启都是暂时的），但后端若始终不接受这个尺寸，每轮
///   一发只会让 TUI 一直吃 SIGWINCH 整屏重排。
export function gridResyncTarget(
  local: { cols: number; rows: number },
  pty: { cols: number; rows: number },
  last: { cols: number; rows: number; at: number } | null,
  now: number,
): { cols: number; rows: number } | null {
  if (pty.cols <= 1 || pty.rows <= 1) return null;
  const cols = Math.min(PTY_GRID_MAX, local.cols);
  const rows = Math.min(PTY_GRID_MAX, local.rows);
  if (cols <= 1 || rows <= 1) return null;
  if (cols === pty.cols && rows === pty.rows) return null;
  if (last && last.cols === cols && last.rows === rows && now - last.at < GRID_RESYNC_RETRY_MS) return null;
  return { cols, rows };
}

export function isMouseMotionReport(data: string): boolean {
  return /^\x1b\[<(?:35|39|43|47|51|55|59|63);\d+;\d+M$/.test(data);
}

/// 一段 onData 是否是 Ctrl+左键的点击上报(SGR 按下 `CSI < 16 ; x ; y M` / 抬起 `…m`,
/// Cb = 左键 0 + CTRL 修饰 16)。宿主把 Ctrl+点击定义为「打开链接」(openTerminalLink),
/// 但鼠标归 TUI 时 xterm 仍照常上报这次按击——claude 全屏渲染器识别到点击落在链接上
/// 会**自己再开一次**,一次 Ctrl+点击弹出两个浏览器窗口(实拍)。宿主语义优先:这对
/// 按下/抬起上报一律不下发,TUI 视角里这次点击从未发生。macOS 的 Cmd+点击 xterm 根本
/// 不编码 meta 位(上报形同普通点击),无从区分,不在此列。
export function isCtrlLeftClickReport(data: string): boolean {
  return /^\x1b\[<16;\d+;\d+[Mm]$/.test(data);
}

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
  /// 有待审批/屏幕提示在等用户（broker 审批、pendingReview、屏幕识别的表单）。
  /// 停滞检测的豁免位：这时 TUI 静止是「在等人」，不是 ConPTY 僵死。
  reviewPending?: boolean;
  visible?: boolean;
  onUserSubmit?: () => void;
  attentionMarkers?: string[];
  interactivePrompt?: boolean;
  /// 刚发出会弹菜单的命令（如 `/model`）：这段窗口里额外识别光标菜单。
  expectMenu?: boolean;
  /// 识别文法(provider 门控 + 插件声明的选择器锚点)。缺省按 Claude 处理(兼容旧调用),
  /// 生产路径由 ChatWindow 从 chatUi 显式组装传入。
  grammar?: AttentionGrammar;
  /// Ctrl/Shift+Enter「插入换行」的注入序列,插件声明经 chatUi 下发(claude 为 ESC+CR)。
  /// 缺省/null = 该 agent 未声明,带修饰的 Enter 保持终端原生语义(与裸 Enter 同码)。
  newlineInput?: string | null;
  onAttention?: (attention: TerminalAttention | null) => void;
  /// 供父组件在自己重启 PTY 后触发偏移复位（对话页发送/切模式也会重启 PTY，
  /// 不止组件内部的 start/takeover 按钮）。
  rearmRef?: MutableRefObject<(() => void) | null>;
  /// 供父组件读当前 xterm 网格（对话页恢复/接管以其为 PTY 初始尺寸：硬编码占位会让
  /// CLI 先按错误宽度排版，fit 落地后再 resize 整屏重排一遍）。
  gridRef?: MutableRefObject<(() => { cols: number; rows: number } | null) | null>;
  /// 恢复/接管时对启动选项的改选（option id → choice id），随 start/takeover 下发；
  /// 省略 = 沿用会话存的选择。状态归 ChatWindow（与对话页的接管入口共用同一份）。
  resumeOptions?: Record<string, string>;
  /// 渲染在 start/takeover 按钮旁的附加控件（权限改选下拉，由 ChatWindow 构造）。
  takeoverExtra?: ReactNode;
  /// 会话的工作目录：文件路径 link provider 点击时作 reveal 的根（见 scanLineForFilePaths）。
  /// null/缺席 = 根未知，路径链接整组不注册（点了也只会报「目录不存在」）。
  cwd?: string | null;
  /// 外部同步终端（attach 客户端）在线状态变化时上报（T-14）：初值来自快照的
  /// externalViewers；对话页据此提示「两边同时输入会交错」。
  onExternalViewers?: (online: boolean) => void;
};

export function ManagedTerminal({ sessionId, status, reviewPending = false, background = false, onBackgroundInput, visible = true, onUserSubmit, attentionMarkers = [], interactivePrompt = false, expectMenu = false, grammar, newlineInput = null, onAttention, rearmRef: externalRearmRef, gridRef: externalGridRef, resumeOptions, takeoverExtra, cwd = null, onExternalViewers }: ManagedTerminalProps) {
  const t = useT();
  const hostRef = useRef<HTMLDivElement>(null);
  const terminalRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const [active, setActive] = useState(false);
  const [snapshotReady, setSnapshotReady] = useState(false);
  const [initialized, setInitialized] = useState(false);
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState("");
  const [exitCode, setExitCode] = useState<number | null | undefined>(undefined);
  // 强制收尾（后端升级链末档）：UI 摘除了会话但进程可能仍在（zombie）——退出文案要区分。
  const [exitForced, setExitForced] = useState(false);
  // state 供渲染 / ref 供同步判定：onData 闭包里要同步判「进程是否已退出」。
  const exitedRef = useRef(false);
  exitedRef.current = exitCode !== undefined;
  // 输出流停滞检测(terminalStreamStalled):水位由 effect 闭包里的写入路径推进,
  // 判定由 5s 节拍读取——两头都要组件级 ref;status/active 同为镜像(节拍闭包不重建)。
  const [stalled, setStalled] = useState(false);
  // 停滞横幅可手动收起(误报时用户不该只能干等新字节);新一轮停滞重新弹出。
  const [stalledDismissed, setStalledDismissed] = useState(false);
  useEffect(() => { if (!stalled) setStalledDismissed(false); }, [stalled]);
  // 初始化 25s 超时:此前只撤遮罩留纯黑屏,零文字零出口。改为撤遮罩的同时挂提示横幅
  // (画面若真有内容不会被盖住),真画出东西(markPainted)即收。
  const [initTimedOut, setInitTimedOut] = useState(false);
  // 横幅上的「结束会话」在途态:确认弹窗+kill 有往返,防连点。
  const [stopping, setStopping] = useState(false);
  const stopFromBanner = () => {
    if (stopping) return;
    setStopping(true);
    void confirmStopSession(sessionId, { title: t.chat.endSession, message: t.chat.endSessionConfirm })
      .catch((e) => setError(formatBackendError(e, t.locale)))
      .finally(() => setStopping(false));
  };
  // 「结束并恢复」里 stop 与 start 之间必须等本会话的 pty-exit:broker.stop() 只发 kill
  // 就返回(Windows 上 portable-pty 的 kill 恒 Ok 不代表进程真死,见 pty.rs stop 注释),
  // 真正收尾靠 waiter 升级链(1s 杀树、3s 强制 finalize),pty-exit 才是「旧进程没了」的
  // 权威信号。不等它就 start 会撞在将死的旧 PTY 上:要么被「会话仍在外部终端运行」拦下
  // (语义还是错的),要么 begin_start 判重 Ok(false) 被吞——前端对着将死快照 setActive,
  // 3s 后 pty-exit 到达再弹一次退出封面(T-5 复查)。超时兜底仍 start 一次:事件真丢了
  // 不该把用户钉死在横幅上(升级链末档 3s,留 1s 余量)。
  const RESTART_EXIT_WAIT_MS = 4_000;
  const waitForExit = () => new Promise<void>((resolve) => {
    let unlisten: (() => void) | undefined;
    let done = false;
    const finish = () => {
      if (done) return;
      done = true;
      unlisten?.();
      resolve();
    };
    const timer = window.setTimeout(finish, RESTART_EXIT_WAIT_MS);
    void listen<ExitEvent>("pty-exit", ({ payload }) => {
      if (payload.sessionId !== sessionId) return;
      window.clearTimeout(timer);
      finish();
    }).then((un) => {
      // finish 先于注册完成(超时/极快退出)时立刻注销,不留野监听器。
      unlisten = un;
      if (done) un();
    });
  });
  // 假死/初始化超时横幅的「结束并恢复」:结束与重启两步合一(此前只有「结束会话」,重启
  // 入口在退出封面上,用户得自己找——而文案里写的就是「点结束会话再重启」)。确认取消
  // (false)即不动;重启失败由 start() 自己的错误通道呈现。start 在下方声明,点击时早已就绪。
  const restartFromBanner = () => {
    if (stopping) return;
    setStopping(true);
    void confirmStopSession(sessionId, { title: t.chat.endSession, message: t.chat.endSessionConfirm })
      .then((stopped) => (stopped ? waitForExit().then(() => start()) : undefined))
      .catch((e) => setError(formatBackendError(e, t.locale)))
      .finally(() => setStopping(false));
  };
  // 悬停在可点内容上(URL/文件路径)的操作提示:Ctrl+点击这条终端惯例此前没有任何
  // 界面表达,用户只能碰运气。非 null 时右下角挂一条固定提示,离开链接即收。
  const [linkHint, setLinkHint] = useState<string | null>(null);
  const lastByteAtRef = useRef(Date.now());
  // 终端内搜索(Ctrl+F,addon-search):搜索条状态归组件,addon 实例归挂载 effect。
  // findNext 会选中命中并滚动到位——不用 decorations(它在部分 xterm 版本走
  // proposed API,选区高亮已足够表达「当前命中在哪」)。
  const searchAddonRef = useRef<SearchAddon | null>(null);
  const [searchOpen, setSearchOpen] = useState(false);
  // 右键菜单（U0-11：复制/粘贴/全选/搜索）：生产构建的 WebView 默认菜单被
  // devtools-guard 整个封死，应用内必须补一份。canCopy/canPaste 在打开那一刻定格
  //（选区在菜单开着期间被键盘清掉也不追——动作落空是无害 no-op）。
  const [termMenu, setTermMenu] = useState<{ x: number; y: number; canCopy: boolean; canPaste: boolean } | null>(null);
  // 复制逻辑长在挂载 effect 里（拿得到 terminal 实例）；菜单在组件 JSX 层，经 ref 调用。
  const copySelectionRef = useRef<(() => void) | null>(null);
  const [searchQuery, setSearchQuery] = useState("");
  const [searchMiss, setSearchMiss] = useState(false);
  // 回绕提示:addon-search 的 findNext/findPrevious 不回绕,到底/到顶落空与「根本没匹配」
  // 此前都是同一句「无匹配」,用户不知道是该回头还是真没有。落空时自动朝反方向找一次,
  // 找到了就明说「已回绕」;两头都落空才是真正的无匹配。
  const [searchWrapped, setSearchWrapped] = useState<"next" | "prev" | null>(null);
  const runSearch = (query: string, direction: "next" | "prev", incremental = false) => {
    const addon = searchAddonRef.current;
    if (!addon || !query) { setSearchMiss(false); setSearchWrapped(null); return; }
    // incremental:逐字输入时在当前选区上扩展匹配,而不是每敲一个字往后跳一个命中。
    // 逐字期不回绕:输入中的落空大概率是「还没打完」,回绕跳动反而吓人。
    let found = direction === "next"
      ? addon.findNext(query, { incremental })
      : addon.findPrevious(query, { incremental });
    let wrapped: "next" | "prev" | null = null;
    if (!found && !incremental) {
      found = direction === "next" ? addon.findPrevious(query) : addon.findNext(query);
      if (found) wrapped = direction;
    }
    setSearchMiss(!found);
    setSearchWrapped(wrapped);
  };
  const closeSearch = () => {
    setSearchOpen(false);
    setSearchQuery("");
    setSearchMiss(false);
    setSearchWrapped(null);
    // 清掉命中选区,焦点还给终端——搜索完下一步几乎总是回去打字。
    terminalRef.current?.clearSelection();
    terminalRef.current?.focus();
  };
  // 搜索条开着期间注册 Esc 层:窗口级「Esc=拒绝审批」让位(与菜单/弹层同一纪律)。
  useEffect(() => {
    if (!searchOpen) return;
    return pushEscLayer();
  }, [searchOpen]);
  const statusRef = useRef(status);
  statusRef.current = status;
  const reviewPendingRef = useRef(reviewPending);
  reviewPendingRef.current = reviewPending;
  const activeRef = useRef(false);
  activeRef.current = active;
  // 最近一次下发给 PTY 的尺寸：同值跳过。切回终端 tab 时曾无条件重发 resize，后端照样
  // 调 master.resize → agent 收到 SIGWINCH 整屏重排，对话↔终端来回切一次闪一次。
  const lastSentSizeRef = useRef<{ cols: number; rows: number } | null>(null);
  // fit 至少跑过一次没有：没跑过时 terminal.cols/rows 还是 xterm 的默认 80×24，不是
  // 这个容器该有的网格，不能拿它去校正 PTY。
  const fittedRef = useRef(false);
  // 上一次网格自愈下发的目标尺寸与时刻（见 GRID_RESYNC_RETRY_MS）。
  const gridResyncRef = useRef<{ cols: number; rows: number; at: number } | null>(null);
  // 窗口焦点镜像（T-14 resize 仲裁）：对话窗与外部同步终端 attach 同一 PTY 时两边都会
  // 下发尺寸，尺寸不同 TUI 就被来回重排。约定**聚焦视图是尺寸主控**——本窗口失焦期间
  // 不下发任何 resize（含网格自愈），外部终端的尺寸得以稳定生效；重新聚焦时补一次
  // fit+resize 把主控权拿回来（监听器在挂载 effect 里）。
  const focusedRef = useRef(true);
  const resizeIfChanged = (sid: number, cols: number, rows: number) => {
    if (!focusedRef.current) return; // 非聚焦视图不下发 resize（T-14，见 focusedRef）
    const last = lastSentSizeRef.current;
    if (last && last.cols === cols && last.rows === rows) return;
    lastSentSizeRef.current = { cols, rows };
    void resizeManagedTerminal(sid, cols, rows).catch(() => {
      // 失败则清记录：下次同尺寸仍要重试（后端可能根本没收到）。
      lastSentSizeRef.current = null;
    });
  };
  /// 网格自愈：PTY 的生效尺寸（`ptyCols`/`ptyRows`，0 = 未知）与本地 fit 出的网格不等，
  /// 就补发一次把 PTY 拉齐。
  ///
  /// resize 平时只由容器尺寸变化驱动，一旦某次没落地——撞上后端那把有界锁
  /// （ResizePseudoConsole 在 conhost 僵死时永不返回，后来者快速失败）、会话正在重启
  /// ——就没有「下一次」把它纠回来：用户不再动窗口，错位就固化下来。TUI 按窄网格重绘、
  /// xterm 按宽网格显示，于是行右侧露出上一屏的残字、光标上移落在错行、同一块区域重画
  /// 成好几份，而 TUI 画在最底下的那行输入框被挤出可视区——实拍反馈：「有时候会看不到
  /// 终端的输入框」。
  ///
  /// 可见期才做，且方向与隐藏期相反：隐藏时 fit 的对象是屏外停靠盒，网格得反过来跟着
  /// PTY 走（见 inspectSnapshot）。
  const resyncGridIfDrifted = (sid: number, ptyCols: number, ptyRows: number) => {
    const terminal = terminalRef.current;
    // fit 没跑过时 terminal.cols/rows 还是 xterm 默认的 80×24，不是这个容器该有的网格。
    // 失焦期同样不纠（T-14）：此刻的尺寸差很可能就是外部同步终端生效的尺寸，拉回来
    // 正是「两个视图反复重排」的源头。
    if (!terminal || !visibleRef.current || !fittedRef.current || !focusedRef.current) return;
    const target = gridResyncTarget(terminal, { cols: ptyCols, rows: ptyRows }, gridResyncRef.current, Date.now());
    if (!target) return;
    gridResyncRef.current = { ...target, at: Date.now() };
    lastSentSizeRef.current = target;
    void resizeManagedTerminal(sid, target.cols, target.rows).catch(() => {
      // 同 resizeIfChanged：失败就清记录，别让同尺寸的下一次被短路挡掉。
      lastSentSizeRef.current = null;
    });
  };
  const externalRunning = isExternallyHeld(status);
  /// 就地重启（结束会话 → 再接管）后重新对齐输出偏移。新 PTY 的 output_end 从 0 重新计数，
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
  const newlineInputRef = useRef(newlineInput);
  const onAttentionRef = useRef(onAttention);
  const onExternalViewersRef = useRef(onExternalViewers);
  const visibleRef = useRef(visible);
  const cwdRef = useRef(cwd);
  // 会话 id 的活镜像：挂载 effect 不再以 sessionId 为键（换会话/重绑不重建 xterm），
  // 闭包里的「哪个会话」一律读它。刻意**不**在渲染期镜像——sessionId 变更 effect 要靠
  // 它拿旧值做变更检测与重绑判定，由那个 effect 推进。
  const sessionIdRef = useRef(sessionId);
  // 挂载后的会话 id 变更处理（临时 id→真实 id 的平滑重绑 / 换会话复位），挂载 effect
  // 内部赋值，组件级的 sessionId effect 调用。见挂载 effect 里的实现注释。
  const sessionChangedRef = useRef<((prevSessionId: number) => void) | null>(null);
  const attentionTailRef = useRef("");
  const attentionReportedRef = useRef<string | null>(null);
  const lastScreenRef = useRef("");
  // 通用启动提示(登录/凭据类,GENERIC_STARTUP_PROMPTS)只在启动窗口内参与识别:
  // 挂载(attach 会回放当前屏,真在等登录的会话立刻能被扫到)与就地重启各开一窗。
  // 窗口外正跑着的会话输出里引用登录话术(读 auth 文件等)不再弹卡锁输入。
  const startupPromptsUntilRef = useRef(Date.now() + GENERIC_STARTUP_WINDOW_MS);
  onUserSubmitRef.current = onUserSubmit;
  onBackgroundInputRef.current = onBackgroundInput;
  backgroundRef.current = background;
  attentionMarkersRef.current = attentionMarkers;
  interactivePromptRef.current = interactivePrompt;
  expectMenuRef.current = expectMenu;
  grammarRef.current = grammar;
  newlineInputRef.current = newlineInput;
  onAttentionRef.current = onAttention;
  onExternalViewersRef.current = onExternalViewers;
  visibleRef.current = visible;
  cwdRef.current = cwd;

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    // 终端里的链接遵循终端惯例：Ctrl/Cmd+点击才打开（普通点击留给 TUI 的鼠标交互与选区）。
    // 打开走后端 open_link（限 http/https，与对话 Markdown 链接同一条通道）；被拒时把原因
    // 显示出来，不许无声吞掉——「点了没反应」正是这类问题最难排查的形态。
    const openTerminalLink = (event: MouseEvent, uri: string) => {
      if (!event.ctrlKey && !event.metaKey) return;
      void openLink(uri).catch((e) => setError(formatBackendError(e, t.locale)));
    };
    const terminal = new Terminal({
      // OSC 8 超链接（TUI 显式声明的链接）由这里接住；纯文本 URL 的识别在 WebLinksAddon。
      // hover/leave 挂操作提示：Ctrl+点击这条终端惯例此前没有任何界面表达（T-4）。
      linkHandler: {
        activate: openTerminalLink,
        hover: () => setLinkHint(t.chat.terminalLinkHint),
        leave: () => setLinkHint(null),
      },
      cursorBlink: true,
      convertEol: false,
      // "JetBrains Mono" 由 styles.css 的 @font-face 打包提供（不依赖本机安装），管拉丁+符号；
      // 它不含 CJK，中文逐字回退到各平台**真实存在**的好看字体：微软雅黑 / 苹方 / Noto。
      // （曾误写 "Microsoft YaHei Mono"——该字体名不存在，导致中文掉到 Courier New 的宋体兜底，
      //  正是「中文看着奇怪」的来源。）xterm 用等宽网格定位，中文按双宽对齐，非等宽也整齐。
      fontFamily: '"JetBrains Mono", ui-monospace, SFMono-Regular, Consolas, "PingFang SC", "Microsoft YaHei", "Noto Sans CJK SC", sans-serif',
      fontSize: 12,
      lineHeight: 1.22,
      // 占位默认值,真实值由设置(terminal_scrollback)经 applyTermStyle 下发——挂载时读一次,
      // settings-changed 热应用。
      scrollback: 5000,
      // 绿色只留给光标这一格宽的点缀；选区是成片色块，用低饱和灰绿保持清爽。
      theme: { background: "#151617", foreground: "#e7e9e8", cursor: "#55d6ae", selectionBackground: "#31403a" },
    });
    const fit = new FitAddon();
    terminal.loadAddon(fit);
    // 纯文本 URL 的链接化。不装它 xterm 根本不识别正文里的 URL——「Ctrl+点击打不开链接」
    // 的第一层原因就是链接从未存在过。hover/leave 同 linkHandler,挂操作提示。
    terminal.loadAddon(new WebLinksAddon(openTerminalLink, {
      hover: () => setLinkHint(t.chat.terminalLinkHint),
      leave: () => setLinkHint(null),
    }));
    // 文件路径 link provider:终端里最高频的可点内容,此前一律不可点(T-4)。识别口径见
    // scanLineForFilePaths(保守:至少要一个路径分隔符)。Ctrl+点击在系统文件管理器中
    // 定位(reveal_path_in_file_manager,与「文件」页同一通道,根 = 会话 cwd;路径越出
    // cwd 或不存在时后端报错,经 setError 可见)。cwd 晚到/缺席时点击报「目录不存在」,
    // 故 cwd 未知干脆不注册。远程模式该命令不在桥白名单(宿主执行类),整条通道不装。
    if (!remoteUi()) {
      terminal.registerLinkProvider({
        provideLinks: (y, callback) => {
          const cwd = cwdRef.current;
          const lineText = terminal.buffer?.active?.getLine(y - 1)?.translateToString(true) ?? "";
          const hits = cwd ? scanLineForFilePaths(lineText) : [];
          if (!cwd || hits.length === 0) { callback(undefined); return; }
          callback(hits.map((hit) => ({
            // 字符串下标 1:1 当单元格用:前面有宽字符时范围会偏,xterm 官方 addon 也是
            // 先按字符映射再校正,这里取最简。end 为开区间,与 WebLinksAddon 口径一致。
            range: { start: { x: hit.start + 1, y }, end: { x: hit.end, y } },
            text: hit.text,
            activate: (event: MouseEvent, linkText: string) => {
              if (!event.ctrlKey && !event.metaKey) return;
              void revealPathInFileManager(cwd, linkText).catch((e) => setError(formatBackendError(e, t.locale)));
            },
            hover: () => setLinkHint(t.chat.terminalPathHint),
            leave: () => setLinkHint(null),
          })));
        },
      });
    }
    // Unicode grapheme 宽表:emoji 与新式符号按正确列宽排。用 graphemes 版而不是
    // Unicode11Addon——后者只更新裸码点宽表,不认 VS16 变体选择符,「⬆️」这类
    // 基础字符+VS16 的 emoji 仍算 1 列,字形按 2 列画、被 WebGL 按单元格裁掉一半
    // (实拍)。graphemes 版按字素簇处理 VS16/ZWJ 序列,与 CLI 侧 string-width 生态的
    // 宽度口径一致;装载时自注册并激活 '15-graphemes',无需手动设 activeVersion。
    try {
      terminal.loadAddon(new UnicodeGraphemesAddon());
    } catch { /* 环境不支持(测试哑实现)则维持默认宽表 */ }
    // 终端内搜索(Ctrl+F 呼出搜索条,addon 实例交给组件级 ref)。
    const searchAddon = new SearchAddon();
    terminal.loadAddon(searchAddon);
    searchAddonRef.current = searchAddon;
    terminal.open(host);
    // WebGL 渲染器:长 backlog 回放/整屏重绘走 GPU,大画面滚动不再掉帧。必须在
    // open 之后装(需要既有 renderer 可替换);构造或激活失败(无 GPU/驱动黑名单)
    // 静默回退 canvas——渲染路径的降级不值得打扰用户。上下文丢失(驱动重置)时
    // dispose 即回退,xterm 自动接管。
    let webgl: WebglAddon | null = null;
    try {
      webgl = new WebglAddon();
      webgl.onContextLoss(() => { webgl?.dispose(); webgl = null; });
      terminal.loadAddon(webgl);
    } catch {
      try { webgl?.dispose(); } catch { /* 已释放 */ }
      webgl = null;
    }
    // 按键粘贴：xterm 把 Ctrl+V 当普通组合键吞掉（preventDefault 后向 PTY 发 ^V），
    // 浏览器的原生 paste 事件因此永远不触发——Windows/Linux 上按键粘贴整个失效。
    // 返回 false 让 xterm 完全放行这个 keydown：WebView 对聚焦的隐藏 textarea 执行原生
    // 粘贴，xterm 自带的 paste 监听接住文本（含 bracketed paste 包装）走 onData 下发。
    // 刻意不自己读剪贴板：navigator.clipboard 在 webview 里要额外权限，原生事件路径零依赖。
    // Shift+Insert 是 Windows 终端的习惯粘贴键，一并放行。
    //
    // 复制：xterm 的选区是自绘的，不是 DOM selection——WebView 的原生复制拿不到它，
    // 右键菜单又在生产构建被封死（devtools-guard），此前终端内容只能肉眼抄。约定与
    // 现代终端一致：**有选区**的 Ctrl/Cmd+C 与 Ctrl+Shift+C = 复制选区且不下发按键；
    // 无选区的 Ctrl+C 维持终端语义（发 ^C 中断前台进程）。
    const copyTerminalSelection = () => {
      const text = terminal.getSelection();
      if (!text) return;
      const fallback = () => {
        // execCommand 通道零权限：临时 textarea 承接选区文本，复制完把焦点还给终端。
        const ta = document.createElement("textarea");
        ta.value = text;
        ta.style.position = "fixed";
        ta.style.opacity = "0";
        document.body.appendChild(ta);
        ta.select();
        document.execCommand("copy");
        ta.remove();
        terminal.focus();
      };
      if (navigator.clipboard?.writeText) {
        navigator.clipboard.writeText(text).catch(fallback);
      } else {
        fallback();
      }
    };
    copySelectionRef.current = copyTerminalSelection;
    terminal.attachCustomKeyEventHandler((event) => {
      if (event.type !== "keydown") return true;
      // 应用级导航键（Ctrl/Cmd+K 切换器、B 收侧栏、1/2 切视图、N 新建）放行给窗口监听：
      // xterm 吃掉 keydown 会 stopPropagation，事件到不了 window，这些快捷键在终端视图
      // 全部失灵。放行 = xterm 不处理也不下发 PTY，事件自然冒泡（与粘贴键同一机制）。
      // 代价：agent composer 的 readline 编辑键（如 Ctrl+K 删到行尾）在托管终端让位——
      // 应用内导航的跨视图一致性优先。
      const appNav = (event.ctrlKey || event.metaKey) && !event.altKey && !event.shiftKey
        && ["KeyK", "KeyB", "KeyN", "Digit1", "Digit2"].includes(event.code);
      if (appNav) return false;
      // Ctrl/Cmd+F = 终端内搜索:拦下打开搜索条,不让 ^F(光标前移)落进 PTY。
      // ChatSidebar 的窗口级 Ctrl+F 对 .managed-terminal 内的焦点已让位,互不抢。
      const findCombo = (event.ctrlKey || event.metaKey) && !event.altKey && !event.shiftKey && event.code === "KeyF";
      if (findCombo) {
        setSearchOpen(true);
        return false;
      }
      const paste = ((event.ctrlKey || event.metaKey) && !event.altKey && event.code === "KeyV")
        || (event.shiftKey && !event.ctrlKey && !event.metaKey && event.code === "Insert");
      if (paste) return false;
      const copyCombo = (event.ctrlKey || event.metaKey) && !event.altKey && event.code === "KeyC";
      if (copyCombo && (event.shiftKey || terminal.hasSelection())) {
        copyTerminalSelection();
        return false;
      }
      // composer 的换行:终端传统编码里 Ctrl+Enter / Shift+Enter 与裸 Enter 同码(都是
      // \r,修饰键信息根本不存在)——用户按「换行」,TUI 收到的却是「提交」。Alt+Enter
      // 能换行纯属 xterm 默认给 Alt 加 ESC 前缀;WT 的 /terminal-setup 正是把 Shift+Enter
      // 配置成发同一序列。注入序列由插件声明经 chatUi 下发(claude=ESC+CR;未声明的
      // agent 不注入——ESC 前缀在 crossterm 系可能被拆成裸 Esc = 中断回合,不能乱发)。
      const newlineSeq = newlineInputRef.current;
      const newlineCombo = event.key === "Enter" && (event.ctrlKey || event.shiftKey) && !event.altKey && !event.metaKey;
      if (newlineCombo && newlineSeq) {
        if (backgroundRef.current) {
          onBackgroundInputRef.current?.();
        } else if (!exitedRef.current) {
          void writeManagedTerminal(sessionIdRef.current, newlineSeq).catch((e) => setError(formatBackendError(e, t.locale)));
        }
        return false;
      }
      return true;
    });
    // Shift+滚轮 = 本地滚动旁路(xterm(1) 惯例:Shift 越过鼠标上报)。TUI 持有鼠标时普通
    // 滚轮整个交给它(既定方向:尊重 claude 配置),但 TUI 不理滚轮/正忙/假死时用户就
    // 彻底翻不了本地 scrollback。xterm 对 shift+wheel 本来就是 no-op(consumeWheelEvent
    // 见 shift 直接返回 0,事件被吞但什么都不做),征用零损失。
    //
    // **备用屏里这条旁路无效,必须放行**:xterm 的备用屏 buffer 以 hasScrollback=false
    // 构造(BufferSet.ts:44,注释原文 "The alt buffer should never have scrollback"),
    // maxLength 恒等于行数,scrollLines 位移永远是 0——本地根本没有可翻的历史。此时
    // 把事件还给 xterm(返回 true)让它按鼠标上报转发给 TUI,由 TUI 滚自己的视图,
    // 好过在这里吞掉变成「按了没反应」。claude 全屏渲染器(?1049h)正是这种情形。
    //
    // deltaY 常见像素制(Chromium 一格约 100),按 ~40px 折一行;行制(deltaMode=1)数值小,
    // 靠下限保底。按住 Shift 时部分环境会把纵向滚轮改派成 deltaX(未在 WebView2 实测),
    // 故两轴都取,免得旁路一次都不触发。
    terminal.attachCustomWheelEventHandler((event) => {
      if (!event.shiftKey) return true;
      if (terminal.buffer?.active?.type === "alternate") return true;
      const delta = event.deltaY || event.deltaX;
      if (!delta) return true;
      event.preventDefault();
      const lines = Math.max(1, Math.round(Math.abs(delta) / 40));
      terminal.scrollLines(delta > 0 ? lines : -lines);
      return false;
    });
    // 上面的放行有个盲区：剪贴板是**图片**时 paste 事件没有文本数据，xterm 的 paste 监听
    // 不产生任何输入，^V 也早被拦下——claude 的原生贴图（^V 让 TUI 自己读系统剪贴板出
    // [Image #N]）在终端页整条断掉。兜底：无文本而有文件（位图）的 paste，补发 ^V 给 CLI。
    // 文本存在时不动——bracketed paste 的既有通路优先。
    const pasteImageFallback = (event: ClipboardEvent) => {
      const data = event.clipboardData;
      if (!data || data.files.length === 0) return;
      // 截图工具常在写入位图的同时写入文本(路径/HTML):此时用户的意图是贴图,文本
      // 通路会把一串路径粘成正文、图片被丢。有位图就优先 ^V;无位图但有文本的
      // (资源管理器复制文件列表)仍走既有文本通路。
      const hasImage = Array.from(data.files).some((file) => file.type.startsWith("image/"));
      if (!hasImage && data.getData("text")) return;
      event.preventDefault();
      void writeManagedTerminal(sessionIdRef.current, "\x16").catch((e) => setError(formatBackendError(e, t.locale)));
    };
    host.addEventListener("paste", pasteImageFallback);
    // 右键 = 应用内菜单（U0-11：复制/粘贴/全选/搜索，复用上面的选区剪贴板逻辑与
    // addon-search）。生产构建的 WebView 默认菜单被 devtools-guard 封死（window 捕获段
    // preventDefault），不补一份右键就是死动作；此前的即时动作版（有选区直接复制、
    // 无选区直接粘贴）让全选/搜索对鼠标用户零入口。粘贴只在**无选区**时给（Windows
    // 终端惯例：有选区的右键是复制语义，此时粘贴一项置灰）。
    // TUI 开了鼠标上报(claude 2.1.238 全屏渲染器:?1000-1006h)时,xterm 把每次按键原样
    // 转发给它、自己不再处理——右键也在其中,而 claude 收到右键就**自己粘贴剪贴板**
    // (pywinpty 实测:发一个 SGR 右键事件,剪贴板内容直接进输入框)。此时 Meowo 再叠
    // 任何动作都是双份。约定:鼠标归 TUI 且无选区时,右键完全让给它(它有自己的右键
    // 语义);有选区的右键仍归 Meowo(弹复制菜单),但要在 mousedown 捕获段把这次按键
    // 拦在 xterm 之外,TUI 不知道有过右键,就不会顺手粘贴。对应的 mouseup 一并拦掉,
    // 否则 xterm 会发一个没有按下的抬起事件。
    const mouseOwnedByApp = () => (terminal.modes?.mouseTrackingMode ?? "none") !== "none";
    let swallowRightUp = false;
    const onMouseDown = (event: MouseEvent) => {
      if (event.button !== 2 || !mouseOwnedByApp() || !terminal.hasSelection()) return;
      event.stopPropagation();
      swallowRightUp = true;
    };
    const onMouseUp = (event: MouseEvent) => {
      if (event.button !== 2 || !swallowRightUp) return;
      event.stopPropagation();
      swallowRightUp = false;
    };
    host.addEventListener("mousedown", onMouseDown, { capture: true });
    host.addEventListener("mouseup", onMouseUp, { capture: true });
    const onContextMenu = (event: MouseEvent) => {
      // 鼠标归 TUI 且无选区:右键已由 xterm 转发给它,粘不粘由它决定(claude 会粘),
      // Meowo 不叠菜单。
      if (mouseOwnedByApp() && !terminal.hasSelection()) return;
      event.preventDefault();
      // 后台会话只读且无选区:粘贴无接收方,维持旧动线——交给宿主把用户领到对话页。
      if (backgroundRef.current && !terminal.hasSelection()) {
        onBackgroundInputRef.current?.();
        return;
      }
      setTermMenu({
        x: event.clientX,
        y: event.clientY,
        canCopy: terminal.hasSelection(),
        // 已退出(进程没了,粘贴无人接收)/有选区(右键=复制语义)时粘贴置灰。
        canPaste: !exitedRef.current && !backgroundRef.current && !terminal.hasSelection(),
      });
    };
    host.addEventListener("contextmenu", onContextMenu);
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
    // 当前网格的读取口（对话页恢复/接管以其为 PTY 初始尺寸，见 gridRef prop）。
    if (externalGridRef) {
      externalGridRef.current = () => {
        const term = terminalRef.current;
        if (!term || term.cols <= 1 || term.rows <= 1) return null;
        return { cols: term.cols, rows: term.rows };
      };
    }
    // 隐藏态挂载（后台屏幕识别 / 切会话时停在对话页）不 fit：宿主是屏外停靠盒
    // （.is-background 固定 1000×700），按它 fit 出的网格与 PTY 尺寸脱节，隐藏期
    // 到达的帧会按错误宽度换行、错行叠画——切回终端页看到的就是花屏，而且若
    // PTY 尺寸未变，resize 同值短路不触发 TUI 重画，花屏永不自愈（实拍反馈）。
    // 隐藏态的网格对齐交给快照的 cols/rows（见 inspectSnapshot）。
    requestAnimationFrame(() => { if (visibleRef.current) { fit.fit(); fittedRef.current = true; } });

    // 写失败必须可见：典型场景是整段粘贴超过后端单次输入上限被拒——
    // 静默吞掉的话，粘贴无声消失，终端画面纹丝不动。
    const sendInput = (data: string) => {
      void writeManagedTerminal(sessionIdRef.current, data).catch((e) => setError(formatBackendError(e, t.locale)));
    };
    let pendingMotion: string | null = null;
    let motionTimer: number | null = null;
    const flushMotion = () => {
      if (motionTimer !== null) {
        window.clearTimeout(motionTimer);
        motionTimer = null;
      }
      if (pendingMotion === null) return;
      const data = pendingMotion;
      pendingMotion = null;
      sendInput(data);
    };
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
      // 进程已退出后画面仍可交互（退出态只留底部横条，好让用户翻看/复制最后的输出）——
      // 此时按键没有接收方，写下去只会换来一条 IPC 报错盖掉退出说明，直接丢弃。
      if (exitedRef.current) return;
      if (payload.includes("\r")) {
        onUserSubmitRef.current?.();
      }
      // Ctrl+点击已由宿主消费(打开链接),上报不下发,否则 TUI 对同一链接再开一次
      // (见 isCtrlLeftClickReport)。
      if (isCtrlLeftClickReport(payload)) return;
      // 鼠标移动上报节流(见 isMouseMotionReport):积压里只留最新一条,每 16ms 至多
      // 一趟 IPC。其余输入(按键/粘贴/应答)不等——先把积压的移动冲出去保证顺序,再
      // 即时下发,按键延迟为零。
      if (isMouseMotionReport(payload)) {
        pendingMotion = payload;
        if (motionTimer === null) motionTimer = window.setTimeout(flushMotion, MOTION_FLUSH_MS);
        return;
      }
      flushMotion();
      sendInput(payload);
    });
    // 声明「正在看」:后端 emitter 只对已注册的会话推 pty-output 实时帧,其余托管会话
    // 不再白付 base64 与 IPC(N 会话齐跑时那正是压垮前端的部分)。失败静默——快照轮询
    // 兜底,最多退化为无实时帧;卸载时注销走后端 CAS,重挂竞态下不会误清新实例的注册。
    // 远程收不到 pty-output 推送(走快照轮询),且该命令不在 /rpc 白名单——短路省 404。
    if (!remoteUi()) void registerTerminalViewer(sessionIdRef.current).catch(() => {});
    // 输出流停滞检测节拍(判据见 terminalStreamStalled):挂载即重置水位——上一个
    // 会话/进程的旧水位不作数。
    lastByteAtRef.current = Date.now();
    // 换会话重挂即重开通用启动提示的识别窗口(attach 回放的当前屏若真停在登录页,
    // 首轮扫描就落在窗口内)。
    startupPromptsUntilRef.current = Date.now() + GENERIC_STARTUP_WINDOW_MS;
    // 识别态也随会话作废（换会话由 sessionChangedRef 走 rearm 归零,这里是首挂载的
    // 初始化）:这三个 ref 原先只在就地重启的 rearm 里清,换会话不清 → 上个会话的
    // 残屏会被晚到补扫读去,对新会话弹一张不属于它的 attention 卡；attentionReportedRef
    // 残留同签名还会反向抑制,让新会话首张同文案的卡片发不出来。与 rearm 同款一并归零。
    attentionReportedRef.current = null;
    attentionTailRef.current = "";
    lastScreenRef.current = "";
    const stallTimer = window.setInterval(() => {
      const candidate = terminalStreamStalled({
        active: activeRef.current,
        background: backgroundRef.current,
        status: statusRef.current,
        reviewPending: reviewPendingRef.current,
        lastByteAt: lastByteAtRef.current,
        now: Date.now(),
      });
      // 二级判据（实拍误报：回合早已结束、TUI 停在输入提示符上等人打字，但 status 管线
      // 仍挂 running，30s 一到就弹「ConPTY 疑似卡死」）。画面里有唯一的自绘输入光标
      // （findFakeCaret 多义/缺失时返回 null，自动放弃豁免）= 在等人，不是管道僵死。
      // 真在提示符上僵死的情形放给「打字无回显」自然暴露，不拿横幅吓人——与判据注释
      // 「宁可漏报，不可谎报」同一取向。
      if (candidate && findFakeCaret(terminal.buffer?.active, terminal.rows)) {
        setStalled(false);
        return;
      }
      setStalled(candidate);
    }, 5_000);
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
      // 隐藏时跳过 fit（理由见 ResizeObserver 处）：切回时的 visible effect 会补。
      if (visibleRef.current) {
        fit.fit();
        fittedRef.current = true;
        // 重测可能改变行列数，PTY 侧要跟着调，否则 TUI 按旧尺寸画、连输入框的位置都是错的。
        // resizeIfChanged 内含失焦门（T-14）：失焦期不下发，聚焦时统一补。
        if (terminal.cols > 1 && terminal.rows > 1) {
          resizeIfChanged(sessionIdRef.current, terminal.cols, terminal.rows);
        }
      }
    }).catch(() => {});
    // 终端样式（字号/行高）跟随设置：挂载时读一次，settings-changed 到达即热应用——
    // xterm 支持运行时改 options，改完重新 fit 并把新行列数下发 PTY（网格变了 TUI 必须重画，
    // 否则输入框位置按旧尺寸算全是错的）。字号钳到 [8,24]：防手改 settings.json 塞出 0 号字。
    const applyTermStyle = (s: Settings) => {
      const size = Math.min(24, Math.max(8, s.terminal_font_size ?? 12));
      const line = LINE_HEIGHTS[s.terminal_line_height] ?? LINE_HEIGHTS.normal;
      // 回滚缓冲进设置（此前硬编码 5000，见构造处）。改小会裁掉最老的行——用户主动
      // 调小换内存正是这个语义；xterm 运行时改 options.scrollback 即时生效。
      const back = Math.min(50_000, Math.max(500, s.terminal_scrollback ?? 5000));
      if (terminal.options.fontSize === size && terminal.options.lineHeight === line && terminal.options.scrollback === back) return;
      terminal.options.fontSize = size;
      terminal.options.lineHeight = line;
      terminal.options.scrollback = back;
      // 隐藏时跳过 fit（理由见 ResizeObserver 处）：切回时的 visible effect 会补。
      if (visibleRef.current) {
        fit.fit();
        fittedRef.current = true;
        if (terminal.cols > 1 && terminal.rows > 1) {
          resizeIfChanged(sessionIdRef.current, terminal.cols, terminal.rows);
        }
      }
    };
    void getSettings().then((s) => { if (!cancelled) applyTermStyle(s); }).catch(() => {});
    let unSettings: (() => void) | undefined;
    listen<Settings>("settings-changed", ({ payload }) => applyTermStyle(payload)).then((un) => {
      if (cancelled) un();
      else unSettings = un;
    }).catch(() => {});
    // 保底：TUI 一直不画东西也不能永远停在 spinner 上。撤遮罩的同时挂超时提示——
    // 纯黑屏零解释时用户不知道是启动失败、还在等、还是自己该做什么。
    let initTimerFired = false;
    const giveUpTimer = window.setTimeout(() => {
      if (!cancelled) { painted = true; initTimerFired = true; setInitialized(true); setInitTimedOut(true); }
    }, INITIALIZING_TIMEOUT_MS);
    // 只有画得出东西的输出才算初始化完成；清屏/光标序列不算（见 hasVisibleOutput）。
    const markPainted = (bytes: Uint8Array) => {
      if (painted) {
        // 超时横幅挂着期间真画面来了:自动收横幅(误报自愈,与停滞横幅同一取向)。
        if (initTimerFired && hasVisibleOutput(bytes)) {
          initTimerFired = false;
          setInitTimedOut(false);
        }
        return;
      }
      if (!hasVisibleOutput(bytes)) return;
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
      const attention = terminalAttention(
        text,
        attentionMarkersRef.current,
        interactivePromptRef.current,
        expectMenuRef.current,
        grammarRef.current,
        Date.now() < startupPromptsUntilRef.current,
      );
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
      // 停滞水位:有真实字节抵达就推进并立即收横幅(同值 setState 被 React 短路,无代价)。
      lastByteAtRef.current = Date.now();
      setStalled(false);
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
      setExitForced(payload.forced === true);
      // 退出画面保留可翻看,但鼠标上报模式若还开着(全屏 TUI 崩溃/被终止时来不及收),
      // xterm 会继续把滚轮/点击转发给早已不存在的进程——滚轮被吞、选区被禁,画面像
      // 「卡死」一样完全不可滚动(实拍「偶尔无法滚动」的主案)。本地收回全部鼠标模式
      // (只写进 xterm,不涉 PTY);备用屏 1049 不动——最后一帧正是用户要看的内容。
      terminal.write(MOUSE_MODES_OFF);
      // 写完滚底：视口若停在上翻位置，退出提示行（和叠在其上的接管卡片语境）都在屏外。
      // 提示行走 i18n（保持 ANSI 灰色素描样式：它是宿主注解，不是 agent 输出）。
      terminal.write(`\r\n\x1b[90m${t.chat.terminalExitedInline(payload.code)}\x1b[0m\r\n`, () => terminal.scrollToBottom());
    };
    // 回放基线:回放起点之前的内容已随 backlog 淘汰丢失,其中最要命的是 TUI 启动时
    // 只发一次的模式开关——硬件光标 `?25l`(claude/kimi 启动即发、此后自绘光标,不藏会
    // 「输入框两个光标」),以及后端 ModeTracker 跟踪到的、此刻仍开着的私有模式(备用屏
    // 1049 / 鼠标上报 1000-1006 / 括号粘贴 2004):不补的话 xterm 退回主屏、关掉鼠标
    // 上报,而 TUI 仍按全屏 + 鼠标模式画,实拍两条滚动条、滚轮滚的不是 TUI 的内容。
    // 顺序按后端给的(1049 在前:它清屏切缓冲,其余模式之后再开)。历史里若真有对应的
    // `?25h`/`?1049l`,回放会照常盖回来,不误伤。
    const replayBaseline = (modes: number[] | undefined) =>
      "\x1b[?25l" + (modes ?? []).map((mode) => `\x1b[?${mode}h`).join("");
    // 鼠标上报全家(1000-1015)的关闭序列,退出态收回滚轮/选区用(见 applyExit 与
    // inspectSnapshot 的 exited 分支)。幂等,重复写无副作用。
    const MOUSE_MODES_OFF = "\x1b[?1003l\x1b[?1002l\x1b[?1000l\x1b[?1006l\x1b[?1005l\x1b[?1015l";
    // 首帧传 0 拿全量（要完整回放历史），补查轮询带 nextOffset 只取增量。
    // writeOutput 本来就按 startOffset 做区间裁剪，增量返回天然兼容。
    const inspectSnapshot = () => managedTerminalSnapshot(sessionIdRef.current, hasWrittenOutput ? nextOffset : 0).then((snapshot) => {
      if (cancelled) return;
      setSnapshotReady(true);
      setActive(snapshot.active);
      setExitCode(snapshot.exited ? snapshot.exitCode : undefined);
      // 外部视图在线初值（T-14）：实时增删走 pty-external-viewers 事件，这里是
      // 重开窗口/重对齐时的对齐点。同值 setState 上游会 bail out，不去重。
      onExternalViewersRef.current?.(snapshot.externalViewers === true);
      // 快照不带 forced（重开窗口补齐历史画面）：按正常退出呈现。
      setExitForced(false);
      // 隐藏态网格对齐：网格必须与 PTY 尺寸一致，隐藏期的帧才排得对（fit 的对象是
      // 屏外停靠盒，不可用——见挂载处注释）。快照带的 cols/rows 是 PTY 当前生效尺寸
      // （0 = 未知，如后台旁路，跳过）。挂载与每次重对齐都会走到这里，隐藏期 PTY
      // 就地重启（对话页发送/切模式）后 rearm 重拉快照，同样被覆盖。
      if (!visibleRef.current && snapshot.cols > 1 && snapshot.rows > 1
        && (terminal.cols !== snapshot.cols || terminal.rows !== snapshot.rows)) {
        terminal.resize(snapshot.cols, snapshot.rows);
      }
      // 可见期反过来:以本地 fit 出的网格为准,把 PTY 拉齐(见 resyncGridIfDrifted)。
      // 快照顺路带着 PTY 的生效尺寸,走到这里就一并对一次;常态下的自愈靠那条心跳,
      // 快照不能拿来轮询——它要把整个 backlog 编码重传一遍。
      resyncGridIfDrifted(sessionIdRef.current, snapshot.cols, snapshot.rows);
      // 重对齐终局:请求的缺口段已被后端 1MiB backlog 淘汰(前端整体落后太多),
      // 字节永久丢失,重试无意义。reset 后按现存 backlog 全量重画(远超一屏,足以
      // 还原可见画面);回放的是历史,里面的查询都答过,拦 xterm 的重复应答。
      if (hasWrittenOutput && snapshot.data && Number.isFinite(snapshot.startOffset) && snapshot.startOffset > nextOffset) {
        terminal.reset();
        nextOffset = snapshot.startOffset;
        replayingHistory = true;
        // 裁剪起点之前的模式开关已随淘汰丢失,reset 又把一切归位——先落基线(见
        // replayBaseline)。
        terminal.write(replayBaseline(snapshot.modes));
      }
      if (snapshot.data) {
        // 首次全量回放(重连/重开窗口)才拦应答:里面全是答过的旧查询。增量补查是准实时
        // 输出,agent 可能正等着这些应答,不拦。启动探测(ESC[6n)的代答不在这里做——
        // 曾在此按回放内容补答过,但临时 id→真实 id 的重挂会重扫同一段 backlog,同一个
        // 查询被答两次,多出的应答落进 composer 成杂字符;现在由后端 reader 单趟代答
        // (pty.rs StartupProbeScanner),回放路径只负责「不放行 xterm 的重复应答」。
        if (!hasWrittenOutput) {
          replayingHistory = true;
          // 首挂载的回放起点若已被 1MiB backlog 裁剪(start > 0),截断点之前的模式
          // 开关同样丢了——与重对齐 reset 同款基线。完整历史(start=0)自带 TUI 的
          // 开关指令,写不写都会被覆盖,不多此一举。
          if (Number.isFinite(snapshot.startOffset) && snapshot.startOffset > 0) {
            terminal.write(replayBaseline(snapshot.modes));
          }
        }
        if (hasWrittenOutput && snapshot.data.length > JUMP_TAIL_B64_CHARS && Number.isFinite(snapshot.endOffset)) {
          // 跳尾降级:重对齐拿到的增量太大(UI 卡顿/长时间切走期间的持续重输出),全量
          // 写 xterm 是数秒的渲染卡死,期间事件继续堆积、可能永远追不上。reset 后只回放
          // 尾部一段——终端是实时视图,落后就看最新;截断处的半截 ANSI 序列由 TUI 的
          // 下一次全屏重绘自愈。回放属历史,照拦 xterm 的重复应答(replayingHistory)。
          // 首次全量回放(hasWrittenOutput=false)刻意不跳:那是重开窗口的历史重建,
          // 完整的 scrollback 正是它的价值。
          const tail = decodeBase64(snapshot.data).slice(-TAIL_KEEP_BYTES);
          terminal.reset();
          replayingHistory = true;
          nextOffset = snapshot.endOffset;
          // 回放基线同上(截断点之前的模式开关已丢)。
          terminal.write(replayBaseline(snapshot.modes));
          lastByteAtRef.current = Date.now();
          setStalled(false);
          inspectAttention(tail);
          markPainted(tail);
          terminal.write(tail, () => {
            replayingHistory = false;
            scheduleAttentionScan();
          });
        } else {
          writeOutput({
            sessionId: sessionIdRef.current,
            offset: Number.isFinite(snapshot.startOffset) ? snapshot.startOffset : 0,
            data: snapshot.data,
          });
          // writeOutput 可能因区间裁剪空转(没写就没有清位回调):hasWrittenOutput 仍为
          // false 说明确实没写,标志必须当场收回,否则实时应答被永久拦截。
          replayingHistory = replayingHistory && hasWrittenOutput;
        }
      }
      // 重开窗口看已退出的会话:回放基线把死进程遗留的鼠标模式原样补写回来(bitmap 记
      // 的是死亡瞬间的真实状态,崩溃/被终止时模式来不及收),不收回的话滚轮/选区在这具
      // 尸体上永远失灵。write 按入队顺序执行,排在回放数据之后即可盖过基线。
      if (snapshot.exited) terminal.write(MOUSE_MODES_OFF);
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
      replayBuffered(true);
      if (bufferedExit) { applyExit(bufferedExit); bufferedExit = null; }
      // PTY 可能先 active、后输出首屏；在此期间保持初始化遮罩并补查快照，避免监听器
      // 尚未注册完成时漏掉极早的一段输出，最终永远停在黑屏或加载态。
      // 补查只为等**第一批**字节：拿到就停，之后的帧走 pty-output 事件——每次快照都会
      // 把整个 backlog（可达 1MB）编码重传一遍，不能拿它轮询到界面画出来为止。
      if ((snapshot.active || sessionIdRef.current < 0) && !hasWrittenOutput && !snapshot.exited) {
        snapshotTimer = window.setTimeout(() => void inspectSnapshot(), 120);
      }
    }).catch(() => {
      if (!cancelled) {
        snapshotApplied = true;
        // 快照 IPC 都失败了,没有可补的字节来源:尽力直写(可能跨洞错乱)也不锁死画面。
        replayBuffered(false);
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

    /// 快照落地后回放缓冲的实时帧。快照请求与回放之间后端可能**又**丢过帧——缓冲里的
    /// 相邻事件之间仍可能有洞,而 writeOutput 只裁剪重叠、不识别缺口。strict(正常路径)
    /// 遇洞即停:已连续的先上屏,剩余事件继续压在缓冲里再拉一次快照补齐;直接跨洞续写
    /// 会把洞两侧拼成「看似连续」的字节流,正是终端花屏(多帧内容交错重叠)的来源。
    /// 非 strict 供快照失败的兜底路径:没有可补的来源,宁可错乱也不锁死画面。
    const replayBuffered = (strict: boolean) => {
      bufferedOutput.sort((a, b) => a.offset - b.offset);
      while (bufferedOutput.length > 0) {
        const next = bufferedOutput[0];
        if (strict && hasWrittenOutput && Number.isFinite(next.offset) && next.offset > nextOffset) {
          snapshotApplied = false;
          scheduleResync();
          return;
        }
        bufferedOutput.shift();
        writeOutput(next);
      }
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
      // 新 PTY 重新计停滞水位:旧进程的静默期不该记在新进程头上。
      lastByteAtRef.current = Date.now();
      setStalled(false);
      attentionReportedRef.current = null;
      attentionTailRef.current = "";
      lastScreenRef.current = "";
      // 就地重启 = 新进程重新走一遍启动流程,通用启动提示的识别窗口随之重开。
      startupPromptsUntilRef.current = Date.now() + GENERIC_STARTUP_WINDOW_MS;
      snapshotApplied = false;
      bufferedOutput.length = 0;
      bufferedExit = null;
      // 尺寸去重记录随旧 PTY 作废：新 PTY 以启动参数的占位尺寸（如 100×30）起，
      // 若沿用旧记录，observer 下发的真实尺寸会因「同值」被跳过，TUI 停在占位宽度。
      lastSentSizeRef.current = null;
      terminal.reset();
      setInitialized(false);
      setExitCode(undefined);
      setExitForced(false);
      void inspectSnapshot();
    };
    // 挂载后的会话 id 变更（此前靠 ChatWindow 的 key={sessionId} 整只重挂：xterm 重建、
    // 首屏清空重画）。分两种：
    //  - 临时 id → 真实 id 的 claim 重绑：后端 try_claim_rebind 只换映射键，同一个 PTY、
    //    输出偏移连续。用 managedTerminalBinding 权威确认（负→正也可能是用户在临时 id
    //    期间切去了别的会话，不能只看符号）后只补一次增量快照——画面不动、不闪遮罩；
    //  - 其余（切到无关会话、provider 切换换出的新临时 id）：按换会话整体复位，状态
    //    清理与 key 重挂等价（rearm 归零偏移/识别态 + 各 useState 归位），但复用 xterm。
    sessionChangedRef.current = (prevSessionId: number) => {
      if (cancelled) return;
      const resetForSessionSwitch = () => {
        setActive(false);
        setSnapshotReady(false);
        setError("");
        setInitTimedOut(false);
        rearmRef.current?.();
      };
      if (!(prevSessionId < 0 && sessionIdRef.current > 0)) {
        resetForSessionSwitch();
        return;
      }
      void managedTerminalBinding(prevSessionId).then((bound) => {
        if (cancelled) return;
        if (bound === sessionIdRef.current) {
          // 同一 PTY 只换了名字：换 id 窗口里漏推的帧由增量快照补齐（inspectSnapshot
          // 带当前 nextOffset），偏移连续性由后端保证，无需 reset。
          void inspectSnapshot();
        } else {
          resetForSessionSwitch();
        }
      }).catch(resetForSessionSwitch);
    };
    if (externalRearmRef) externalRearmRef.current = () => rearmRef.current?.();
    const outputListener = listen<OutputEvent>("pty-output", ({ payload }) => {
      if (payload.sessionId === sessionIdRef.current) {
        window.clearTimeout(snapshotTimer);
        setActive(true);
        setSnapshotReady(true);
        setExitCode(undefined);
        setExitForced(false);
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
      if (payload.sessionId === sessionIdRef.current) {
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
        // 隐藏期间不 fit：隐藏态宿主是屏外固定 1000×700 的停靠盒（.is-background），
        // 按它 fit 会把网格改成与 PTY 列数脱节的尺寸，这期间到达的 TUI 帧全按错误
        // 网格排版；切回终端时 fit 回原尺寸，行列数与上次下发一致则 resize 同值短路、
        // 不发 SIGWINCH，TUI 不重画，乱掉的画面就一直留在屏上（实拍反馈）。
        // 网格冻结在最后一次可见时的尺寸 = 与 PTY 一致，隐藏期的帧照常排对。
        if (!visibleRef.current) return;
        fit.fit();
        fittedRef.current = true;
        if (terminal.cols > 1 && terminal.rows > 1) {
          resizeIfChanged(sessionIdRef.current, terminal.cols, terminal.rows);
        }
      }, 80);
    });
    observer.observe(host);

    // T-14 resize 仲裁的聚焦半边：失焦期间本视图不是尺寸主控、一律不下发 resize
    //（门在 resizeIfChanged / resyncGridIfDrifted 里）；重新聚焦时 fit 一次再比对
    // PTY 生效尺寸——只有真的漂移（失焦期间被外部终端改过）才补发 resize 夺回主控；
    // 无外部终端的常态下 PTY 与本地一致，一轮 focus/blur 不多发任何 SIGWINCH。
    focusedRef.current = document.hasFocus();
    const onWindowFocus = () => {
      focusedRef.current = true;
      if (!visibleRef.current || !fittedRef.current) return;
      fit.fit();
      if (terminal.cols > 1 && terminal.rows > 1) {
        void managedTerminalGrid(sessionIdRef.current)
          .then(([cols, rows]) => resyncGridIfDrifted(sessionIdRef.current, cols, rows))
          .catch(() => {});
      }
    };
    const onWindowBlur = () => { focusedRef.current = false; };
    window.addEventListener("focus", onWindowFocus);
    window.addEventListener("blur", onWindowBlur);

    // 远程模式:pty-output 事件在手机端永远不触发(无 Tauri 事件桥),用定时补查驱动同一条
    // 增量快照通道。inspectSnapshot 带 nextOffset 只取增量,每拍仅拉自上次以来的新字节。
    // 这条通道同时喂饱隐藏的屏幕识别(AskUserQuestion 表单检测)。桌面走事件,不进此分支。
    let remotePollTimer = 0;
    if (remoteUi()) {
      remotePollTimer = window.setInterval(() => {
        if (!cancelled) void inspectSnapshot();
      }, REMOTE_SNAPSHOT_POLL_MS);
    }

    return () => {
      cancelled = true;
      if (!remoteUi()) void unregisterTerminalViewer(sessionIdRef.current).catch(() => {});
      window.clearInterval(remotePollTimer);
      window.clearInterval(stallTimer);
      window.clearTimeout(snapshotTimer);
      window.clearTimeout(resyncTimer);
      window.clearTimeout(giveUpTimer);
      window.clearTimeout(resizeTimer);
      window.clearTimeout(attentionScanTimer);
      observer.disconnect();
      imeObserver.disconnect();
      window.removeEventListener("focus", onWindowFocus);
      window.removeEventListener("blur", onWindowBlur);
      host.removeEventListener("paste", pasteImageFallback);
      host.removeEventListener("contextmenu", onContextMenu);
      host.removeEventListener("mousedown", onMouseDown, { capture: true });
      host.removeEventListener("mouseup", onMouseUp, { capture: true });
      helperTextarea?.removeEventListener("compositionstart", startComposition);
      helperTextarea?.removeEventListener("compositionend", endComposition);
      input.dispose();
      // 卸载时丢弃积压的移动采样:会话都换了,旧位置没有接收方。
      if (motionTimer !== null) window.clearTimeout(motionTimer);
      pendingMotion = null;
      unOutput?.();
      unExit?.();
      unSettings?.();
      // webgl 先于 terminal dispose(xterm 惯例:renderer addon 依赖 core 的 DOM 还在)。
      try { webgl?.dispose(); } catch { /* 上下文丢失时已自释放 */ }
      searchAddonRef.current = null;
      copySelectionRef.current = null;
      terminal.dispose();
      terminalRef.current = null;
      fitRef.current = null;
      rearmRef.current = null;
      sessionChangedRef.current = null;
      if (externalRearmRef) externalRearmRef.current = null;
      if (externalGridRef) externalGridRef.current = null;
    };
    // 会话 id 变更不再重挂本 effect（xterm 重建、首屏清空重画）：临时 id→真实 id 的
    // 重绑与换会话都走 sessionChangedRef（见其实现注释），effect 只随宿主 ref 变化重建。
  }, [externalRearmRef, externalGridRef]);

  // 会话 id 变更（挂载后）：不重挂终端，只换「认哪个 id」。viewer 注册跟着 id 走——
  // emitter 只对 viewed_session 推实时帧，重绑后不重新注册就只剩快照兜底（画面退回
  // 每 80ms 一跳）。旧的注册不必注销：set_viewer 是单槽覆盖写，卸载时 cleanup 按 CAS
  // 清当前 id。重绑平滑续接 / 换会话复位的分派在 sessionChangedRef。
  useEffect(() => {
    const prev = sessionIdRef.current;
    if (prev === sessionId) return;
    sessionIdRef.current = sessionId;
    if (!remoteUi()) void registerTerminalViewer(sessionId).catch(() => {});
    sessionChangedRef.current?.(prev);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- sessionIdRef/sessionChangedRef 是稳定 ref
  }, [sessionId]);

  // capability 查询可能比 PTY 首屏稍晚返回。提示文字先到、markers 后到时也要立刻补判，
  // 不能等一个可能永远不会来的后续输出 chunk。
  const attentionMarkerKey = attentionMarkers.join("\0");
  // 文法(锚点/provider)可能晚于首屏到达(chatUi 是异步查询):变化时用最后一屏复扫,
  // 与 markers 晚到的补投递同一套逻辑。
  const grammarKey = JSON.stringify(grammar ?? null);
  useEffect(() => {
    const attention = terminalAttention(
      lastScreenRef.current || attentionTailRef.current,
      attentionMarkers,
      interactivePrompt,
      expectMenu,
      grammar,
      Date.now() < startupPromptsUntilRef.current,
    );
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
      fittedRef.current = true;
      if (terminal.cols > 1 && terminal.rows > 1) {
        resizeIfChanged(sessionId, terminal.cols, terminal.rows);
      }
      // 显式滚底：终端是实时视图，切回就该看最新。以前靠「切回必发 resize→TUI 整屏
      // 重绘」的副作用顺带回底，resize 同值短路后副作用消失，上翻的视口会停在原地。
      terminal.scrollToBottom();
      terminal.focus();
    });
    return () => cancelAnimationFrame(raf);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [visible, sessionId]);

  // 网格自愈的心跳。resize 平时由容器尺寸变化驱动,某次没落地就没有下一次纠正它
  // ——错位固化,底部那行输入框被挤出可视区(见 resyncGridIfDrifted)。这里定期比一次
  // PTY 的生效尺寸,不必等用户去拖窗口。
  //
  // 查的是只回两个数的轻量命令,不是快照(快照要把整个 backlog 编码重传)。会话没在跑
  // /已退出/临时 id 时不查:那时没有 PTY 可对齐。
  useEffect(() => {
    if (!visible || !active || exitCode !== undefined || sessionId < 0) return;
    const timer = window.setInterval(() => {
      managedTerminalGrid(sessionId)
        .then(([cols, rows]) => resyncGridIfDrifted(sessionId, cols, rows))
        .catch(() => {});
    }, GRID_POLL_MS);
    return () => window.clearInterval(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [visible, active, exitCode, sessionId]);

  const initializing = !snapshotReady || ((active || sessionId < 0) && !initialized);

  const start = async () => {
    const terminal = terminalRef.current;
    if (!terminal) return;
    setStarting(true);
    setError("");
    terminal.focus();
    try {
      await startManagedTerminal(sessionId, terminal.cols || 80, terminal.rows || 24, resumeOptions);
      setActive(true);
      // 新 PTY 的偏移从 0 重新计数，必须归零重拉，否则新输出会被当成旧数据丢弃。
      rearmRef.current?.();
    } catch (e) {
      setError(formatBackendError(e, t.locale));
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
      setError(formatBackendError(e, t.locale));
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
      // 主按钮说后果（S-14），与 ChatWindow 的接管入口同款纪律。
      confirmLabel: t.chat.terminalTakeover,
    });
    if (!yes) return;
    setStarting(true);
    setError("");
    terminal.focus();
    try {
      await takeoverManagedTerminal(sessionId, terminal.cols || 80, terminal.rows || 24, resumeOptions);
      setActive(true);
      rearmRef.current?.();
    } catch (e) {
      setError(formatBackendError(e, t.locale));
    } finally {
      setStarting(false);
    }
  };

  return (
    <div className="managed-terminal">
      <div className="managed-terminal-host" ref={hostRef} />
      {/* 终端内搜索条(Ctrl+F):Enter=下一个、Shift+Enter=上一个、Esc 关闭还焦点。
          Esc 统一在容器上截停(焦点在按钮上时输入框的 onKeyDown 接不到,RenameModal 同款课)。 */}
      {searchOpen && (
        <div
          className="term-search"
          role="search"
          onKeyDown={(event) => {
            if (event.key === "Escape") {
              event.stopPropagation();
              closeSearch();
            }
          }}
        >
          <input
            className="term-search-input"
            autoFocus
            value={searchQuery}
            placeholder={t.chat.termSearchPlaceholder}
            aria-label={t.chat.termSearchPlaceholder}
            // IME 合成守卫:拼音中间态不触发搜索(与侧栏搜索框同款)。
            onChange={(event) => {
              if ((event.nativeEvent as InputEvent).isComposing) return;
              setSearchQuery(event.target.value);
              runSearch(event.target.value, "next", true);
            }}
            onCompositionEnd={(event) => {
              setSearchQuery(event.currentTarget.value);
              runSearch(event.currentTarget.value, "next", true);
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter" && !event.nativeEvent.isComposing) {
                event.preventDefault();
                runSearch(searchQuery, event.shiftKey ? "prev" : "next");
              }
            }}
          />
          {searchMiss && searchQuery.length > 0 && (
            <span className="term-search-miss" role="status">{t.chat.termSearchNoMatch}</span>
          )}
          {!searchMiss && searchWrapped && searchQuery.length > 0 && (
            <span className="term-search-miss" role="status">{t.chat.termSearchWrapped(searchWrapped)}</span>
          )}
          <button type="button" className="term-search-btn" aria-label={t.chat.termSearchPrev} data-tip={t.chat.termSearchPrev} onClick={() => runSearch(searchQuery, "prev")}>
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="m18 15-6-6-6 6" /></svg>
          </button>
          <button type="button" className="term-search-btn" aria-label={t.chat.termSearchNext} data-tip={t.chat.termSearchNext} onClick={() => runSearch(searchQuery, "next")}>
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="m6 9 6 6 6-6" /></svg>
          </button>
          <button type="button" className="term-search-btn is-close" aria-label={t.chat.close} onClick={closeSearch}>×</button>
        </div>
      )}
      {termMenu && (
        <TerminalContextMenu
          x={termMenu.x}
          y={termMenu.y}
          canCopy={termMenu.canCopy}
          canPaste={termMenu.canPaste}
          onCopy={() => {
            copySelectionRef.current?.();
            // 与旧「右键=复制」行为一致：复制完清选区、焦点还给终端。
            terminalRef.current?.clearSelection();
            terminalRef.current?.focus();
          }}
          onPaste={() => {
            // 读后端剪贴板（readText 在 WebView2 要权限弹窗，arboard 零打扰），
            // 经 terminal.paste 走 bracketed paste 与 onData 既有下发通路。
            const terminal = terminalRef.current;
            if (!terminal) return;
            void clipboardText().then((text) => {
              if (text) {
                terminal.paste(text);
              } else {
                // 剪贴板没有文本（典型：截图位图）：回退发 ^V 让 TUI 自己读系统剪贴板
                // （claude 的 [Image #N] 贴图），与键盘路径 pasteImageFallback 同语义。
                void writeManagedTerminal(sessionIdRef.current, "\x16").catch((e) => setError(formatBackendError(e, t.locale)));
              }
              terminal.focus();
            }).catch((e) => setError(formatBackendError(e, t.locale))); // 读剪贴板失败必须可见，不许静默吞掉
          }}
          onSelectAll={() => {
            terminalRef.current?.selectAll();
            terminalRef.current?.focus();
          }}
          onSearch={() => setSearchOpen(true)}
          onClose={() => setTermMenu(null)}
        />
      )}
      {initializing && (
        <div className="managed-terminal-cover is-initializing" role="status">
          <i className="managed-terminal-spinner" />
          <div>{t.chat.terminalInitializing}</div>
        </div>
      )}
      {!initializing && !active && (
        // 拿到退出码 = 屏幕上留着最后的输出（报错原因多半就在那里）——此时不整屏遮罩：
        // 内容收进居中的醒目卡片（用户心智里的「接管框」），卡片外指针穿透，输出照样
        // 可滚可选。曾试过降成底部窄横条：TUI 退出常清屏，黑屏 + 一条不起眼的窄条 =
        // 用户以为「接管的框不见了」（实拍反馈）。其余状态（未启动/外部占用/旁路没接上）
        // 没有画面可看，维持整屏遮罩居中。
        <div className={"managed-terminal-cover" + (exitCode !== undefined ? " is-exited" : "")}>
          <div className={exitCode !== undefined ? "managed-terminal-exit-card" + (exitCode === 0 ? " is-clean" : "") : "managed-terminal-cover-inner"}>
          {/* 后台会话的「没画面」有两种截然不同的成因，此前一律说成「已结束」：worker 明明
              还在跑、只是旁路没接上时，那句话是错的，而且不给任何出路。只有拿到退出码
              （worker 真的退了）才说结束，否则说「没接上」并给一次重接。 */}
          <div className="managed-terminal-cover-msg">{error || (background
            ? (exitCode !== undefined ? t.chat.terminalBackgroundGone : t.chat.terminalBackgroundLost)
            : exitCode !== undefined ? (exitForced ? t.chat.terminalExitedForced(exitCode) : t.chat.terminalExited(exitCode)) : externalRunning ? t.chat.terminalExternal : t.chat.terminalReady)}</div>
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
        </div>
      )}
      {active && !background && (
        <div className="managed-terminal-actions">
          {/* 结束会话的入口在标题栏(ChatWindow 的「结束会话」),这里不再放一份——
              曾并存过「结束终端」按钮,同一条 confirmStopSession 流程双入口徒增困惑。
              后端刻意让 attach 失败可见（不静默回退 GUI），前端吞掉就前功尽弃。
              后台会话不接这个按钮:它只接上了旁路(bgpty),并未被 Meowo 托管,
              ensure_attachable 必然报「该会话尚未由 Meowo 接管」——挂着必错的按钮是误导。 */}
          <button type="button" onClick={() => { setError(""); void openAttachedTerminal(sessionId).catch((e) => setError(formatBackendError(e, t.locale))); }}>{t.chat.terminalAttach}</button>
        </div>
      )}
      {active && error && (
        // 容器只挂 role="alert"，关闭动作收进内嵌按钮——同一元素身兼 button 与 alert 两个角色会冲突。
        <div className="managed-terminal-error" role="alert">
          <span>{error}</span>
          <button type="button" aria-label={t.chat.close} onClick={() => setError("")}>×</button>
        </div>
      )}
      {linkHint && (
        // 悬停可点内容(URL/文件路径)时的操作提示:Ctrl+点击这条终端惯例此前没有任何
        // 界面表达。固定在右下角、pointer-events:none,不挡画面不占交互。
        <div className="managed-terminal-link-hint">{linkHint}</div>
      )}
      {active && stalled && !stalledDismissed && !error && (
        // 输出流停滞(ConPTY 管道疑似内核僵死,判据见 terminalStreamStalled):用户态无法
        // 自愈。「结束并恢复」把文案里的两步合一(此前只有「结束会话」,重启入口在退出
        // 封面上,用户得自己找);误报也要能手动收掉(×),不是只能干等新字节。新字节
        // 抵达仍自动收起(误报自愈)。
        <div className="managed-terminal-error is-stalled" role="alert">
          <span>{t.chat.terminalStreamStalled}</span>
          <button type="button" disabled={stopping} onClick={restartFromBanner}>
            {stopping ? t.chat.terminalStopping : t.chat.endAndResume}
          </button>
          <button type="button" disabled={stopping} onClick={stopFromBanner}>
            {stopping ? t.chat.terminalStopping : t.chat.endSession}
          </button>
          <button type="button" aria-label={t.chat.close} onClick={() => setStalledDismissed(true)}>×</button>
        </div>
      )}
      {active && initTimedOut && !stalled && !error && (
        // 初始化 25s 仍无可见输出:撤遮罩后的黑屏必须有解释与出口,不能让用户对着
        // 一片黑猜。真画面到达自动收(markPainted),也可手动收。「结束并恢复」与假死
        // 横幅对齐(黑屏再等下去多半也是僵死,一步到位少一次往返);临时负 id 会话
        // (pending 占位)尚未有真 PTY,start 对它不可用,按钮隐藏。
        <div className="managed-terminal-error is-stalled" role="alert">
          <span>{t.chat.terminalInitTimeout}</span>
          {sessionId > 0 && (
            <button type="button" disabled={stopping} onClick={restartFromBanner}>
              {stopping ? t.chat.terminalStopping : t.chat.endAndResume}
            </button>
          )}
          <button type="button" disabled={stopping} onClick={stopFromBanner}>
            {stopping ? t.chat.terminalStopping : t.chat.endSession}
          </button>
          <button type="button" aria-label={t.chat.close} onClick={() => setInitTimedOut(false)}>×</button>
        </div>
      )}
    </div>
  );
}

/// 托管终端的右键菜单（U0-11）：复制/粘贴/全选/搜索。生产构建的 WebView 默认右键
/// 菜单被 devtools-guard 整个封死——不补应用内菜单，这四件事对鼠标用户就是黑洞。
/// 复用贴纸卡片菜单的 .ctx-menu 外观与关闭纪律（useDismissable，G-1：点外/Esc/
/// 失焦/滚动关闭，Esc 层已登记、窗口级「Esc=拒绝审批」让位）。
function TerminalContextMenu({
  x,
  y,
  canCopy,
  canPaste,
  onCopy,
  onPaste,
  onSelectAll,
  onSearch,
  onClose,
}: {
  x: number;
  y: number;
  /// 有选区 = 复制可用（无选区时复制必落空，置灰而不是藏——菜单位置稳定，肌肉记忆不被打乱）。
  canCopy: boolean;
  /// 无选区且会话可写 = 粘贴可用（Windows 终端惯例：有选区的右键是复制语义）。
  canPaste: boolean;
  onCopy: () => void;
  onPaste: () => void;
  onSelectAll: () => void;
  onSearch: () => void;
  onClose: () => void;
}) {
  const t = useT();
  const ref = useRef<HTMLDivElement>(null);
  // fixed 定位 + 钳位：贴边打开时向内收，不被窗口边缘裁掉（与 CardContextMenu 同款）。
  const [pos, setPos] = useState({ left: x, top: y });
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const pad = 4;
    setPos({
      left: Math.max(pad, Math.min(x, window.innerWidth - el.offsetWidth - pad)),
      top: Math.max(pad, Math.min(y, window.innerHeight - el.offsetHeight - pad)),
    });
  }, [x, y]);
  useDismissable(ref, {
    onClose,
    outsideEvent: "click",
    interceptOutsideClick: true,
    closeOnContextMenu: true,
    closeOnBlur: true,
    closeOnScroll: true,
  });
  // 键盘可达：挂载即把焦点搬进首个可用项；↑↓/Home/End 搬焦点，Enter/Space 激活
  //（与 CardContextMenu 同款 roving；禁用项不在停靠序列里）。
  useEffect(() => {
    ref.current?.querySelector<HTMLElement>('[role="menuitem"]:not(:disabled)')?.focus();
  }, []);
  const onMenuKeyDown = (e: ReactKeyboardEvent) => {
    const items = Array.from(ref.current?.querySelectorAll<HTMLElement>('[role="menuitem"]:not(:disabled)') ?? []);
    if (items.length === 0) return;
    const cur = items.indexOf(document.activeElement as HTMLElement);
    let next: number;
    if (e.key === "ArrowDown") next = cur >= 0 ? (cur + 1) % items.length : 0;
    else if (e.key === "ArrowUp") next = cur >= 0 ? (cur - 1 + items.length) % items.length : items.length - 1;
    else if (e.key === "Home") next = 0;
    else if (e.key === "End") next = items.length - 1;
    else if ((e.key === "Enter" || e.key === " ") && cur >= 0) {
      // 显式激活：jsdom 不合成按钮的 Enter/Space 点击；preventDefault 挡住原生那次，只触发一回。
      e.preventDefault();
      (document.activeElement as HTMLElement).click();
      return;
    } else return;
    e.preventDefault();
    items[next]?.focus();
  };
  const act = (fn: () => void) => () => {
    fn();
    onClose();
  };
  return (
    <div ref={ref} className="ctx-menu" role="menu" style={pos} onClick={(e) => e.stopPropagation()} onKeyDown={onMenuKeyDown}>
      <button type="button" role="menuitem" className="ctx-item" disabled={!canCopy} onClick={act(onCopy)}>
        {t.chat.terminalMenuCopy}
      </button>
      <button type="button" role="menuitem" className="ctx-item" disabled={!canPaste} onClick={act(onPaste)}>
        {t.chat.terminalMenuPaste}
      </button>
      <div className="ctx-sep" role="separator" />
      <button type="button" role="menuitem" className="ctx-item" onClick={act(onSelectAll)}>
        {t.chat.terminalMenuSelectAll}
      </button>
      <button type="button" role="menuitem" className="ctx-item" onClick={act(onSearch)}>
        {t.chat.terminalMenuSearch}
      </button>
    </div>
  );
}

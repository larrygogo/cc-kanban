/** 把 PTY snapshot 的 base64 增量追加为 UTF-8 文本；损坏的一帧不打断启动流程。 */
export function appendTerminalText(tail: string, data: string, decoder: TextDecoder): string {
  if (!data) return tail;
  try {
    const binary = atob(data);
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
    return (tail + decoder.decode(bytes, { stream: true })).slice(-16_384);
  } catch {
    return tail;
  }
}

export type TerminalAttentionOption = {
  label: string;
  input: string;
  description?: string;
  selected?: boolean;
  focused?: boolean;
  position?: number;
  kind?: "choice" | "input" | "submit" | "chat";
};
/// 审批详情区的屏幕文法风格（后端 `AttentionDetails` 的镜像，插件随 AttentionPattern 声明）：
/// proceed_box = 「Do you want to proceed?」框式（光标移动选择、记住=持久规则）；
/// arrow_panel = 「▶ 标题」面板式（数字键直选、记住=仅本会话）。缺省/none = 不做结构化拆解。
export type AttentionDetailsStyle = "none" | "proceed_box" | "arrow_panel";
export type TerminalAttention = { id: string; text: string; options?: TerminalAttentionOption[]; details?: AttentionDetailsStyle };

export function visibleTerminalText(text: string): string {
  // backlog 可能保留已经处理过的旧提示。只看最后一次整屏清除之后的内容，避免重新挂载
  // ManagedTerminal 时把历史信任页再次报成当前状态。
  const clearAt = Math.max(text.lastIndexOf("\x1b[2J"), text.lastIndexOf("\x1b[3J"));
  const currentScreen = clearAt >= 0 ? text.slice(clearAt) : text;
  return currentScreen
    .replace(/\x1b\][^\x07]*(?:\x07|\x1b\\)/g, "")
    .replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, "")
    .replace(/\r(?!\n)/g, "\n")
    .split("\n")
    .map((line) => line.trimEnd())
    .filter((line, index, lines) => line.trim() && line !== lines[index - 1])
    .slice(-80)
    .join("\n")
    .trim();
}

/// 从终端可见文本里识别当前模式：命中位置**最靠后**的标记胜出——模式切换的重绘按时间
/// 顺序追加进输出流，最后出现的指示文案就是当前状态（backlog 尾部可能还留着旧指示）。
/// 小写比较；U+2019 弯引号归一为直引号（"don't ask on" 两种写法都认）。
export function modeFromScreen(visible: string, markers: { marker: string; value: string }[]): string | null {
  const haystack = visible.toLowerCase().replace(/’/g, "'");
  let best: { at: number; value: string } | null = null;
  for (const { marker, value } of markers) {
    const at = haystack.lastIndexOf(marker.toLowerCase());
    if (at >= 0 && (!best || at > best.at)) best = { at, value };
  }
  return best?.value ?? null;
}

// 登录、凭据恢复等提示并不总是由 provider 暴露为稳定文案。这里仅收需要键盘确认的
// 高信号启动提示；命中后 GUI 会显示 CLI 的原文，而不是猜测并重写它的选项。
// 直接带 g 标志预编译：terminalAttention 跑在 150ms 节流扫描 / 80ms 启动轮询的热路径上，
// 每次调用重新 new RegExp 是纯浪费；matchAll 不会改写 lastIndex，共享实例是安全的。
// 组合式提示(关键词 + 按键提示)只认**同行或紧邻的下一行**:此前的 `[\s\S]{0,320}` 跨距
// 匹配在 16KB 缓冲上每 150ms 回溯一次是纯浪费,而且会把相隔几段的正文(agent 读 auth
// 文件打印的内容 + 无关的 press enter 提示)拼成一次误命中。
const GENERIC_STARTUP_PROMPTS = [
  /restore[^\n]{0,100}(?:token|credential|authentication)/gi,
  /(?:token|credential|authentication)[^\n]{0,100}restore/gi,
  /oauth token has been revoked/gi,
  /no oauth token/gi,
  /run \/login to sign in/gi,
  /waiting for sign-in to complete/gi,
  /trust gateway/gi,
  /(?:oauth|token|credential|authentication|sign-in|sign in|login|gateway)[^\n]{0,160}(?:\n[^\n]{0,160})?press (?:enter|esc|escape) to/gi,
  /press (?:enter|esc|escape) to[^\n]{0,160}(?:\n[^\n]{0,160})?(?:oauth|token|credential|authentication|sign-in|sign in|login|gateway)/gi,
];

/// 导航提示：菜单在等键盘选择的信号。要求同时出现「方向键/导航」与「回车确认」两类线索，
/// 单独一个 ❯ 太常见（提示符、列表装饰都可能有），会把正文误报成菜单。中文本地化 CLI
/// 常把 enter/confirm 写作「回车/确认」，一并收。
const MENU_HINT = /(?:↑↓|↑\/↓|up\/down|arrow keys|方向键|j\/k)[^\n]{0,80}(?:enter|select|confirm|move|回车|确认|选择)|enter\s+(?:to\s+)?select/i;

/// 数字选择器的锚点项:该 provider 的选择器里**固有出现**的选项文案(小写子串匹配)。
/// 纯编号 run 只有含锚点才算选择器本体——会话正文里的普通编号列表没有这些文案。
/// kind 决定该项在卡片上的形态:input = 自由输入项,chat = 转聊天项。
export type SelectorAnchor = { marker: string; kind: "input" | "chat" };

/// 识别文法:provider 门控 + 插件声明的锚点。识别规则正逐步从「Claude 截屏硬编码」
/// 下放为插件声明(循 startup_attention_markers 的先例),这是第一步。
/// 可操作提示的识别规格，由插件声明、经 chatUi 下发（后端 AttentionPattern）。
/// patterns 是 JS 正则源码，任一命中即算命中该卡片；last=true 取最后一次匹配
/// （审批框必须如此：TUI 重绘不清屏，上一屏的残影在缓冲上方）。
export type AttentionPattern = { id: string; patterns: string[]; last: boolean; details?: AttentionDetailsStyle };

export type AttentionGrammar = {
  /// 会话的 agent id。仅用于日志/调试与默认文法的自述——**识别规则不再按它分支**，
  /// 整句规则全部来自 attentionPatterns（插件声明）。
  provider?: string;
  selectorAnchors: SelectorAnchor[];
  /// 整句可操作提示的识别规格。空 = 该 agent 未取证，不做整句识别（宁可不弹卡，
  /// 误报会凭空弹出一张卡并锁住输入框）。
  attentionPatterns?: AttentionPattern[];
};

/// 兼容默认:未显式传文法时按 Claude 处理(存量调用与测试的行为不变)。
/// 生产路径(ChatWindow/ManagedTerminal)一律显式传插件声明的文法。
const CLAUDE_GRAMMAR: AttentionGrammar = {
  provider: "claude",
  selectorAnchors: [
    { marker: "type something", kind: "input" },
    { marker: "chat about this", kind: "chat" },
  ],
  // 与 claude 插件的 attention_patterns 声明保持一致（后端是权威，这里只是默认文法的
  // 兜底，供未显式传文法的存量调用与单测使用）。
  attentionPatterns: [
    {
      id: "claude:long-session-resume",
      patterns: [
        "this session is[^\\n]{0,120}\\bold and[^\\n]{0,80}\\btokens\\b",
        "resuming the full session will consume a substantial portion of your usage limits",
      ],
      last: false,
    },
    {
      id: "claude:command-approval",
      details: "proceed_box",
      patterns: ["this command requires approval", "do you want to proceed\\?"],
      last: true,
    },
    {
      // 计划模式批准提示。该提示不触发任何 hook（上游回归，取证见插件声明处注释），
      // pendingReview 门控的选择器识别接不到它，整句识别是唯一出口。
      id: "claude:plan-approval",
      patterns: ["written\\s+up\\s+a\\s+plan\\s+and\\s+is\\s+ready\\s+to\\s+execute"],
      last: true,
    },
  ],
};

/// 按一条规格在屏幕文本里定位命中位置。返回 -1 = 未命中。
/// 统一以 `im` 编译（`last` 时加 `g`）：`^`/`$` 按行匹配，大小写不敏感。
/// 非法正则不得让整个识别崩掉——声明来自后端，坏一条只跳过它。
function matchAttentionPattern(rule: AttentionPattern, visible: string): number {
  let best = -1;
  for (const source of rule.patterns) {
    let index = -1;
    try {
      if (rule.last) {
        const re = new RegExp(source, "gim");
        for (let m = re.exec(visible); m; m = re.exec(visible)) {
          index = m.index;
          if (m[0] === "") re.lastIndex += 1; // 零宽匹配防死循环
        }
      } else {
        index = new RegExp(source, "im").exec(visible)?.index ?? -1;
      }
    } catch {
      continue;
    }
    // 同一张卡的多个 pattern 取位置最靠后的那个（与审批框「最后一屏才是当前」同理）。
    if (index > best) best = index;
  }
  return best;
}

/// 光标菜单：一句导航提示 + 一个 ❯ 标记当前项。返回选项块的行区间。
///
/// 边界靠**缩进**而不是空行：`visibleTerminalText` 与 xterm 抓屏都会丢掉空行，但保留行首
/// 缩进。同一个菜单的选项缩进一致，而小节标题（如 kimi 的 `Thinking (←→ to switch)`）
/// 缩进更浅——取「含 ❯ 且同缩进的连续行」正好圈出选项，不会把相邻小节吞进来。
function detectCursorMenu(
  visible: string,
): { index: number; lines: string[]; focused: number; title: string } | null {
  const lines = visible.split("\n");
  const hintLine = lines.findIndex((line) => MENU_HINT.test(line));
  if (hintLine < 0) return null;
  const focusedLine = lines.findIndex((line) => /^\s*[❯›]\s+\S/.test(line));
  if (focusedLine < 0) return null;
  // 文本起始列：带光标的行要跨过标记本身，才能和其余项对齐比较。
  const textIndent = (line: string): number => {
    const match = /^(\s*)([❯›]\s+)?/.exec(line);
    return (match?.[1]?.length ?? 0) + (match?.[2]?.length ?? 0);
  };
  const target = textIndent(lines[focusedLine]);
  const sameBlock = (line: string) => line.trim().length > 0 && textIndent(line) === target;
  let start = focusedLine;
  while (start > 0 && sameBlock(lines[start - 1])) start -= 1;
  let end = focusedLine;
  while (end + 1 < lines.length && sameBlock(lines[end + 1])) end += 1;
  // 只有一项的「菜单」多半是误判（提示符、单条列表）。
  if (end - start < 1) return null;
  // 标题以**提示行**为锚取它上面一行，而不是「选项块之前最近的一行」——两者之间还夹着
  // provider 过滤行之类的东西，那种会把抬头显示成 "All Kimi Code"。
  const title = lines
    .slice(0, hintLine)
    .reverse()
    .find((line) => line.trim() && !/^[\s─━═|-]+$/.test(line));
  const index = lines.slice(0, start).join("\n").length;
  return {
    index,
    lines: lines.slice(start, end + 1),
    focused: focusedLine - start,
    title: title?.trim() ?? "",
  };
}

/// 框线数字菜单——gemini `/model` 对话框的形态(真机 PTY 取证,gemini-cli 0.51,2026-07):
/// 全框线包裹(内容行首尾都是 │)、编号项、`●` 标记焦点、无导航提示行、Esc 关闭;
/// ↑/↓ 移动焦点、Enter 确认经按键探针证实(tests/capture_model_menu.rs,
/// MEOWO_CAPTURE_PROBE_KEYS=1)。只认框内行,普通正文的编号列表不会进来;
/// 仅在 expectMenu 窗口(刚发出菜单命令)内启用,进一步压误报面。
function detectFramedNumberedMenu(
  visible: string,
): { title: string; options: TerminalAttentionOption[] } | null {
  const lines = visible
    .split("\n")
    .filter((line) => /^\s*│/.test(line))
    .map((line) => line.replace(/^\s*│ ?/, "").replace(/\s*│\s*$/, "").trimEnd())
    .filter((line) => line.trim().length > 0);
  if (lines.length < 3) return null;
  type Item = { label: string; focused: boolean; description?: string; indent: number };
  type Run = { start: number; items: Item[] };
  const runs: Run[] = [];
  let current: Run | null = null;
  lines.forEach((line, index) => {
    const numbered = line.match(/^(\s*)([●❯›>]?)\s*(\d+)[.)]\s+(.+?)$/);
    if (numbered) {
      const num = Number(numbered[3]);
      if (num === 1) {
        current = { start: index, items: [] };
        runs.push(current);
      }
      if (current && num === current.items.length + 1) {
        current.items.push({
          label: numbered[4].trim(),
          focused: numbered[2].length > 0,
          indent: numbered[1].length + (numbered[2] ? numbered[2].length + 1 : 0),
        });
        return;
      }
      current = null;
      return;
    }
    const last = current?.items.at(-1);
    if (last) {
      // 比选项更深的缩进行是它的描述;缩进回到项级以下(Remember…/(Press Esc…)等
      // 尾注行)则菜单收束——不能把对话框的说明行折进最后一个选项。
      const indent = (line.match(/^\s*/) as RegExpMatchArray)[0].length;
      if (indent > last.indent) {
        last.description = [last.description, line.trim()].filter(Boolean).join(" ");
      } else {
        current = null;
      }
    }
  });
  // 重绘会把同一对话框多次留在缓冲里,取最后一个完整的 run;没有焦点标记时不接线——
  // 相对移动的起点未知,按钮会选错项,宁可不出卡片。
  const run = [...runs].reverse().find((r) => r.items.length >= 2 && r.items.some((item) => item.focused));
  if (!run) return null;
  const focusedIndex = run.items.findIndex((item) => item.focused);
  const title = [...lines.slice(0, run.start)].reverse().find((line) => line.trim()) ?? "";
  return {
    title: title.trim(),
    options: run.items.map((item, position) => {
      const delta = position - focusedIndex;
      return {
        label: item.label.replace(/\s{2,}/g, "  ").slice(0, 80),
        description: item.description,
        input: delta === 0 ? "\r" : delta < 0
          ? "\x1b[A".repeat(-delta) + "\r"
          : "\x1b[B".repeat(delta) + "\r",
        focused: delta === 0,
        position,
        kind: "choice" as const,
      };
    }),
  };
}

/// 编号选项行（`> 1. Yes` / `❯ 2) No`）。取上下文的起点和裁掉选项区的终点必须用
/// **同一条**规则——两边判定不一致，就会出现「起点跨过了一组选项、终点又锚在它上面」
/// 的组合，把当前这一屏整个裁没。终点侧的耦合方式：下游只从 promptSnippet 返回的
/// anchor（匹配行）**之后**找第一个编号行,起点侧被排除/保留的编号行都在 anchor 之前,
/// 两边不可能锚到对方保留的行上。无 g 标志：带状态的正则不能这样复用。
const NUMBERED_OPTION = /^\s*(?:[❯›>▶]\s*)?\d+[.)]\s+/;
/// 带光标标记的编号行:残影选择器的判定特征。上一屏审批框的选项组必然有一行带焦点
/// 光标(claude 用 `>`、kimi 用 `▶`),而命令正文里的编号列表(多行 commit message 的
/// 「1. fix parser」、带编号的输出)不会有——仅凭「是编号行」就钳位,会把这类命令的
/// 开头从审批卡上截掉,用户批准一条自己看不全的命令。
const MARKED_NUMBERED_OPTION = /^\s*[❯›>▶]\s*\d+[.)]\s+/;

/// 取正则的**最后一次**匹配。TUI 重绘不清屏,上一份面板留在缓冲上方:首个匹配会把
/// 标题/命令锁在残影上,而按键打给的是屏幕最下方的活动面板。传入的正则必须带 g。
function lastMatch(pattern: RegExp, text: string): (RegExpMatchArray & { index: number }) | null {
  let last: (RegExpMatchArray & { index: number }) | null = null;
  for (const match of text.matchAll(pattern)) {
    // matchAll 的 index 类型上可缺席(实际总在);过滤掉让下游拿到确定的 number。
    if (match.index !== undefined) last = match as RegExpMatchArray & { index: number };
  }
  return last;
}

function promptSnippet(
  visible: string,
  index: number,
  contextBefore = 1,
): { text: string; anchor: number } {
  const lines = visible.split("\n");
  const matchedLine = visible.slice(0, index).split("\n").length - 1;
  // 命令审批需要多保留几行，才能显示命令和用途；普通启动选择只留一行上下文。
  let start = Math.max(0, matchedLine - contextBefore);
  // 往前取上下文时不能跨过上一屏残留的**选择器**：跨过去了,残留选项会混进卡片正文。
  // 判据是成组的:连续编号行构成一个 run,run 里含带光标标记的行才是残留选择器;
  // 纯编号 run 是命令正文里的列表(commit message、编号输出),照常保留、继续向上。
  for (let line = matchedLine - 1; line >= start; line -= 1) {
    if (!NUMBERED_OPTION.test(lines[line])) continue;
    let top = line;
    while (top - 1 >= 0 && NUMBERED_OPTION.test(lines[top - 1])) top -= 1;
    let bottom = line;
    while (bottom + 1 < matchedLine && NUMBERED_OPTION.test(lines[bottom + 1])) bottom += 1;
    if (lines.slice(top, bottom + 1).some((entry) => MARKED_NUMBERED_OPTION.test(entry))) {
      start = bottom + 1;
      break;
    }
    line = top; // 纯编号组整组跳过,继续向上取上下文(for 自减后落在组上方)。
  }
  // 掐头去尾的空行按行做(不能对拼好的串 .trim():行号一变,anchor 就对不上匹配行)。
  let firstKept = start;
  while (firstKept < matchedLine && !lines[firstKept].trim()) firstKept += 1;
  let lastKept = Math.min(lines.length - 1, matchedLine + 9);
  while (lastKept > matchedLine && !lines[lastKept].trim()) lastKept -= 1;
  return {
    text: lines.slice(firstKept, lastKept + 1).join("\n"),
    anchor: matchedLine - firstKept,
  };
}

/// 光标菜单选项块 → GUI 按钮。行首缩进只是菜单的对齐手段，不属于选项文字；
/// 菜单首尾循环，只能从 ❯ 光标做相对移动，不能靠「多按几次上键」归零。
function cursorMenuOptions(lines: string[], focused: number): TerminalAttentionOption[] {
  return lines.map((line, position) => {
    const label = line.replace(/^\s*(?:[❯›]\s+)?/, "").trimEnd();
    const delta = position - focused;
    return {
      // 多列对齐用的大段空格在按钮上没意义；压成单空格，超长再截断。
      label: label.replace(/\s{2,}/g, "  ").slice(0, 80),
      input: delta === 0 ? "\r" : delta < 0
        ? "\x1b[A".repeat(-delta) + "\r"
        : "\x1b[B".repeat(delta) + "\r",
      focused: delta === 0,
      position,
      kind: "choice" as const,
    };
  });
}

/// 锚定光标菜单：best 已确认是阻塞提示，不再要求导航提示行（中文本地化 CLI 的长会话恢复
/// 等菜单经常没有 "enter to select" 这类线索），直接从提示锚点之后找 ❯ 光标所在的同缩进块。
/// 扩展时跳过以句读结尾的行——那是题干的说明句（如「完整恢复会消耗较多额度。」），
/// 与选项同缩进时容易被误吞进选项块。
function detectAnchoredCursorMenu(
  visible: string,
  fromIndex: number,
): { lines: string[]; focused: number } | null {
  const lines = visible.split("\n");
  const fromLine = visible.slice(0, fromIndex).split("\n").length - 1;
  const focusedLine = lines.findIndex((line, index) => index > fromLine && /^\s*[❯›]\s+\S/.test(line));
  if (focusedLine < 0) return null;
  const textIndent = (line: string): number => {
    const match = /^(\s*)([❯›]\s+)?/.exec(line);
    return (match?.[1]?.length ?? 0) + (match?.[2]?.length ?? 0);
  };
  const target = textIndent(lines[focusedLine]);
  const optionLine = (line: string) =>
    line.trim().length > 0 && textIndent(line) === target && !/[。？！?!.：:]$/.test(line.trim());
  let start = focusedLine;
  while (start > 0 && optionLine(lines[start - 1])) start -= 1;
  let end = focusedLine;
  while (end + 1 < lines.length && optionLine(lines[end + 1])) end += 1;
  if (end - start < 1) return null;
  return { lines: lines.slice(start, end + 1), focused: focusedLine - start };
}

/** 返回当前启动阻塞提示的可见原文；null 表示没有需要 GUI 接管的交互。 */
/// `expectMenu`：刚发出一条会弹交互菜单的命令（如 `/model`）。只在这段窗口里认光标菜单——
/// 常开的话，agent 平时画的任何带 ❯ 的列表都会弹成卡片，噪声大于价值。
/// `startupPrompts`：是否启用 GENERIC_STARTUP_PROMPTS(登录/凭据类通用启动提示)。这些
/// 提示只在**会话启动阶段**有意义——正跑着的会话输出里引用登录话术(读 auth 文件、讨论
/// 登录流程)不该弹卡锁输入。启动路径(waitForTerminalReady/挂载后的启动窗口)传 true,
/// 稳态 150ms 扫描传 false;provider 声明的 markers 与 attentionPatterns 不受影响。
export function terminalAttention(
  text: string,
  markers: string[],
  interactivePrompt = false,
  expectMenu = false,
  grammar: AttentionGrammar = CLAUDE_GRAMMAR,
  startupPrompts = true,
): TerminalAttention | null {
  if (!text) return null;
  const visible = visibleTerminalText(text);
  const lower = visible.toLowerCase();
  let best: { index: number; id: string; details?: AttentionDetailsStyle } | null = null;
  // 整句可操作提示（审批框、长会话确认）由插件声明、经 chatUi 下发——识别层不再
  // 认识任何具体 agent。规则**天然是 per-agent 的**：这些是各家 TUI 的界面原文，
  // 别家 agent 的输出里引用同一句（讨论审批流程、cat 一个含该句的脚本）不该误弹卡片、
  // 锁住输入框；下发的规则只含该会话自己那家，门控因此从 `provider ===` 变成了
  // 「你只会拿到自己的规则」。
  for (const rule of grammar.attentionPatterns ?? []) {
    const index = matchAttentionPattern(rule, visible);
    if (index >= 0 && (!best || index > best.index)) best = { index, id: rule.id, details: rule.details };
  }
  for (const marker of markers) {
    const normalized = marker.toLowerCase();
    const index = lower.lastIndexOf(normalized);
    if (index >= 0 && (!best || index > best.index)) best = { index, id: `provider:${normalized}` };
  }
  for (const pattern of startupPrompts ? GENERIC_STARTUP_PROMPTS : []) {
    for (const match of lower.matchAll(pattern)) {
      const index = match.index ?? -1;
      if (index >= 0 && (!best || index > best.index)) {
        // id 按识别规则稳定，而不带整屏文字。TUI 重绘会改变原始流的重复片段，若把它们
        // 放进 id，同一提示就会被误报成几十个新提示，导致对话页闪烁。用户操作后的下一屏
        // 由一次性抓屏交付，因此连续两个同类选择器也不会丢。
        best = { index, id: `generic:${pattern.source}` };
      }
    }
  }
  if (!best && interactivePrompt) {
    const selectorHint = /enter to select[^\n]*/i.exec(visible);
    const numberedChoices = visible.match(/^\s*(?:[❯›>]\s*)?\d+[.)]\s+/gm) ?? [];
    if (selectorHint && numberedChoices.length >= 2) {
      best = { index: selectorHint.index, id: "interactive:numbered-selector" };
    }
  }
  // 无编号的光标菜单（kimi 的 `/model`、provider 切换等）。与上面的编号选择器是两种形态：
  // 那种每项带 `1.`，这种只有一个 ❯ 光标 + 一句导航提示。实测形如：
  //   Select a model  (type to search)
  //   Tab toggle provider · ↑↓ navigate · Enter select · Esc cancel
  //     K2.7 Coding            Kimi Code
  //   ❯ K3                     Kimi Code ← current
  const cursorMenu = !best && expectMenu ? detectCursorMenu(visible) : null;
  if (cursorMenu) {
    return {
      id: "interactive:cursor-menu",
      text: cursorMenu.title,
      options: cursorMenuOptions(cursorMenu.lines, cursorMenu.focused),
    };
  }
  // 框线数字菜单(gemini 的 /model 对话框):无导航提示行,光标菜单认不出;形态与
  // 交互语义均经真机取证,复用同一个卡片通道(选项按钮 = 相对移动 + 回车)。
  const framedMenu = !best && expectMenu ? detectFramedNumberedMenu(visible) : null;
  if (framedMenu) {
    return {
      id: "interactive:cursor-menu",
      text: framedMenu.title,
      options: framedMenu.options,
    };
  }
  if (!best) return null;
  // 详情区文法风格由插件随 AttentionPattern 声明（此前按 pattern id 猜，是变相身份分支）。
  const detailsStyle: AttentionDetailsStyle = best.details ?? "none";
  const approvalCard = detailsStyle !== "none";
  // 面板式的详情从标题行(▶ Run this command?)起取:best 可能命中面板底部的
  // 按键提示行,从那里截会把命令正文裁掉。同样取最后一个标题——首个会落在重绘残影上。
  const panelHeader = detailsStyle === "arrow_panel"
    ? lastMatch(/^\s*▶\s*[^\n]+\?\s*$/gm, visible)
    : null;
  const snippetInfo = best.id === "interactive:numbered-selector"
    ? { text: visible, anchor: 0 }
    : panelHeader
      ? { text: visible.slice(panelHeader.index), anchor: 0 }
      : promptSnippet(visible, best.index, approvalCard ? 12 : 1);
  const snippet = snippetInfo.text;
  // 命令审批的选项已经转换成 GUI 按钮，详情区保留命令、用途和审批问题，只从第一个
  // 编号选项起裁掉。「第一个」从 anchor(匹配行)之后数——审批的问句永远在选项之前,
  // anchor 之前的编号行是命令正文的一部分(见 NUMBERED_OPTION 的耦合注释),既不能当
  // 截断锚点,也不能被解析成选项按钮。只对命令审批收紧:generic 提示的锚点可能是
  // 选项**下方**的按键提示行(press enter/esc to …),对它们门控会把真选项排除掉。
  const optionsFrom = approvalCard ? snippetInfo.anchor + 1 : 0;
  const snippetLines = snippet.split("\n");
  const firstOptionLine = snippetLines.findIndex(
    (line, index) => index >= optionsFrom && NUMBERED_OPTION.test(line),
  );
  const displayText = approvalCard && firstOptionLine >= 0
    ? snippetLines.slice(0, firstOptionLine).join("\n").trim()
    : snippet;
  const labels = snippetLines.flatMap((line, index) => {
    if (index < optionsFrom) return [];
    const match = line.match(/^\s*([❯›>▶]?)\s*(\d+)[.)]\s*(.+?)\s*$/);
    return match ? [{ index: Number(match[2]) - 1, label: match[3], focused: Boolean(match[1]) }] : [];
  });
  if (best.id === "interactive:numbered-selector") {
    const lines = snippet.split("\n");
    // 编号项分两种形态：复选框（多选题，`1. [ ] xxx`）和纯编号（单选题，`1. xxx`——
    // AskUserQuestion 的单选/多问题标签页形态就长这样）。会话正文也常有普通的
    // `1. / 2. / 3.` 列表，不能照单全收：菜单编号必然从 1 连续递增，据此把编号项分成
    // 「连续编号 run」；纯编号 run 只有当其中含 Claude 选择器固有的锚点项（Type
    // something / Chat about this / Submit）时才算选择器本体，正文列表 run 整组丢弃。
    // 复选框菜单照旧只认复选框项。
    type NumberedOccurrence = TerminalAttentionOption & { plain?: boolean; group?: number };
    const occurrences: NumberedOccurrence[] = [];
    let current: NumberedOccurrence | null = null;
    let run = 0;
    let lastIndex = -1;
    for (const line of lines) {
      const numbered = line.match(/^\s*([❯›>]?)\s*(\d+)[.)]\s*(?:\[([ x*✓✔])\]\s*)?(.+?)\s*$/i);
      const checkbox = numbered?.[3];
      if (numbered) {
        const index = Number(numbered[2]);
        // 编号断裂（重新从 1 起、跳号、回退）＝另一份列表/菜单重绘。
        if (index !== lastIndex + 1) run += 1;
        lastIndex = index;
        const label = numbered[4].trim();
        const lowerLabel = label.toLowerCase();
        const special = grammar.selectorAnchors
          .find((anchor) => lowerLabel.includes(anchor.marker.toLowerCase()))?.kind ?? null;
        current = {
          label,
          input: "",
          selected: Boolean(checkbox && !/\s/.test(checkbox)),
          focused: Boolean(numbered[1]),
          kind: special ?? "choice",
          plain: checkbox === undefined && !special,
          group: run,
        };
        occurrences.push(current);
        continue;
      }
      const submit = line.match(/^\s*([❯›>]?)\s*submit\s*$/i);
      if (submit) {
        // Submit 夹在编号项之间（多选布局：choices → Submit → hint），归当前 run，
        // 不打断编号连续性判断。
        current = {
          label: line.trim(),
          input: "",
          kind: "submit",
          focused: Boolean(submit[1]),
          group: run,
        };
        occurrences.push(current);
        continue;
      }
      if (/^[─━═\s]+$/.test(line) || /enter to select|↑\/↓|up\/down|esc to cancel|[?？]/i.test(line)) {
        current = null;
        continue;
      }
      if (current && line.trim()) {
        current.description = [current.description, line.trim()].filter(Boolean).join(" ");
      }
    }
    // 复选框项存在 → 多选菜单，纯编号行是正文噪声，照旧丢弃；
    // 全无复选框 → 单选菜单，只保留与锚点项同组的纯编号项。
    // 锚点来自插件声明(grammar.selectorAnchors,循 startup_attention_markers 的先例;
    // claude 声明 Type something / Chat about this)。未声明锚点的 provider,纯编号 run
    // 整组按正文丢弃——宁可不出卡片(发送侧有未识别交互的软拦兜底),也不把正文列表
    // 变成会向 PTY 打方向键的按钮。单选屏没有独立 Submit 行,submit 不计入锚。
    const hasCheckbox = occurrences.some((occurrence) => occurrence.kind === "choice" && !occurrence.plain);
    const anchoredGroups = new Set(
      occurrences
        .filter((occurrence) => occurrence.kind === "input" || occurrence.kind === "chat")
        .map((occurrence) => occurrence.group),
    );
    const kept = occurrences.filter((occurrence) =>
      !occurrence.plain || (!hasCheckbox && anchoredGroups.has(occurrence.group)));
    // 全屏 TUI 重绘可能把同一块内容多次留在 scrollback。按动作+标签合并，选中状态取
    // 最后一次重绘，描述取最短的完整版本（长版本通常混进了下一轮提示文字）。
    const unique = new Map<string, TerminalAttentionOption>();
    for (const { plain: _plain, group: _group, ...occurrence } of kept) {
      const key = `${occurrence.kind}:${occurrence.label.toLowerCase()}`;
      const existing = unique.get(key);
      if (!existing) {
        unique.set(key, { ...occurrence });
        continue;
      }
      existing.selected = occurrence.selected;
      existing.focused = occurrence.focused;
      if (occurrence.description && (!existing.description || occurrence.description.length < existing.description.length)) {
        existing.description = occurrence.description;
      }
    }
    const ordered = [...unique.values()];
    // 某些重绘只在状态行里写出 Submit，独立选择行被裁掉；Claude 的多选顺序固定为
    // checkbox choices → Submit → Chat about this，在 Chat 前补回它。
    // 仅限多选：单选菜单没有独立 Submit 行（Enter 选中即提交/切到下一问），虚构一个
    // 带相对移动输入的提交按钮只会把光标挪到错的选项上。
    if (hasCheckbox && !ordered.some((choice) => choice.kind === "submit")) {
      const chatAt = ordered.findIndex((choice) => choice.kind === "chat");
      ordered.splice(chatAt >= 0 ? chatAt : ordered.length, 0, { label: "Submit", input: "", kind: "submit", focused: false });
    }
    const focusedPosition = ordered.findIndex((choice) => choice.focused);
    const choices = ordered.map((choice, position) => {
      const delta = focusedPosition < 0 ? null : position - focusedPosition;
      return {
        ...choice,
        position,
        // 菜单会首尾循环，不能靠“多按几次上键”归零；必须从当前 ❯ 光标做相对移动。
        input: delta == null ? "" : delta < 0
          ? "\x1b[A".repeat(-delta) + "\r"
          : "\x1b[B".repeat(delta) + "\r",
      };
    });
    // 一项可点的都没剩(全部 run 因缺锚被判为正文):不出空卡——空卡只会收走输入框
    // 又给不了任何动作。返回 null 让发送侧的「未识别交互」软拦接手。
    if (choices.length === 0) return null;
    const questionLine = [...lines].reverse().find((line) => /[?？]/.test(line) && !/enter to select/i.test(line));
    const question = questionLine?.split(/→|->/).at(-1)?.trim() ?? "";
    return {
      id: best.id,
      text: question,
      options: choices,
    };
  }
  // 面板式(arrow_panel)审批:数字键直接选中并提交(kimi 源码 selectAndSubmit),GUI 按钮
  // 只需打数字,无需光标相对移动;需要现场输入反馈的选项(Reject with feedback / Revise)
  // 在卡片上完成不了,不收成按钮——想写反馈仍可从终端处理。
  if (detailsStyle === "arrow_panel") {
    const byIndex = new Map<number, (typeof labels)[number]>();
    for (const entry of labels) byIndex.set(entry.index, entry);
    const options = [...byIndex.values()]
      .sort((a, b) => a.index - b.index)
      .filter(({ label }) => !/reject with feedback|^revise$/i.test(label.trim()))
      .map(({ index, label }) => ({ label, input: String(index + 1) }));
    return { id: best.id, text: displayText, details: detailsStyle, options: options.length >= 2 ? options : undefined };
  }
  // trust、长会话恢复以及其他编号选择器共用同一套结构化按钮，不再退化成上一项/下一项。
  if (labels.length >= 2) {
    // TUI 重绘可能把同一菜单多次留在缓冲里；按编号合并，焦点状态取最后一次重绘。
    const byIndex = new Map<number, (typeof labels)[number]>();
    for (const entry of labels) byIndex.set(entry.index, entry);
    const merged = [...byIndex.values()].sort((a, b) => a.index - b.index);
    const focused = merged.filter((entry) => entry.focused).at(-1);
    return {
      id: best.id,
      text: displayText,
      details: detailsStyle,
      options: merged.map(({ index, label }) => ({
        label,
        // 这些菜单和 numbered-selector 一样首尾循环，「先按 8 次上键归零」在 3 项菜单上
        // 会绕圈选错（点拒绝实际选中批准）。抓到 ❯ 光标就从它做相对移动；确实没有光标
        // 标记时才退回归零法——那类菜单没有更可靠的定位依据。
        input: focused
          ? (index < focused.index
            ? "\x1b[A".repeat(focused.index - index) + "\r"
            : "\x1b[B".repeat(index - focused.index) + "\r")
          : "\x1b[A".repeat(8) + "\x1b[B".repeat(Math.max(0, index)) + "\r",
      })),
    };
  }
  // 无编号的光标菜单（中文本地化 CLI 的长会话恢复等）：编号提取落空时从 ❯ 光标块提取选项，
  // 让 GUI 直接给出选项按钮，而不是把「上一项/下一项」甩给用户。
  const anchored = detectAnchoredCursorMenu(visible, best.index) ?? detectCursorMenu(visible);
  if (anchored) {
    return { id: best.id, text: displayText, details: detailsStyle, options: cursorMenuOptions(anchored.lines, anchored.focused) };
  }
  return { id: best.id, text: displayText, details: detailsStyle };
}

/** 兼容启动发送路径只关心是否阻塞的调用。 */
export function terminalNeedsAttention(text: string, markers: string[], interactivePrompt = false): boolean {
  return terminalAttention(text, markers, interactivePrompt) != null;
}

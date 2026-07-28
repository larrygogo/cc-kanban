import { describe, expect, it } from "vitest";
import { modeFromScreen, terminalAttention, terminalNeedsAttention } from "./terminalAttention";

describe("terminalAttention", () => {
  it("returns the provider-declared trust screen for the GUI", () => {
    const prompt = "\x1b[2JDo you trust the contents of this directory?\r\n❯ 1. Yes, I trust this folder\r\n  2. No, exit";
    const attention = terminalAttention(prompt, ["do you trust the contents of this directory"]);
    expect(attention?.id).toContain("provider:");
    expect(attention?.options?.map((option) => option.label)).toEqual(["Yes, I trust this folder", "No, exit"]);
    expect(terminalNeedsAttention(prompt, ["do you trust the contents of this directory"])).toBe(true);
  });

  it("detects a credential/token step even when the provider did not declare its wording", () => {
    const prompt = "\x1b[2JOAuth token has been revoked\r\nRun /login to sign in\r\nPress Enter to continue · Esc to cancel";
    const attention = terminalAttention(prompt, []);
    expect(attention?.id).toContain("generic:");
    expect(attention?.text).toContain("OAuth token has been revoked");
  });

  it("selects the newest step after trust instead of keeping the old prompt forever", () => {
    const output = [
      "\x1b[2JDo you trust the files in this folder?\r\n  Yes\r\n  No",
      "\x1b[2JRestore authentication token for this session?\r\n  Restore\r\n  Continue without it",
    ].join("");
    const attention = terminalAttention(output, ["do you trust the files in this folder"]);
    expect(attention?.id).toContain("generic:");
    expect(attention?.text).not.toContain("Do you trust");
    expect(attention?.text).toContain("Restore authentication token");
  });

  it("does not mistake an ordinary TUI keyboard hint for a startup prompt", () => {
    const prompt = "\x1b[2JMigrate this session state?\r\n  Keep current\r\n  Recover previous\r\nEnter to select";
    expect(terminalAttention(prompt, [])).toBeNull();
  });

  it("surfaces a numbered TUI selector only when the session is waiting for an answer", () => {
    const prompt = [
      "\x1b[2J剩下这几个点要不要继续做？",
      "> 1. [ ] #1 首屏尾读",
      "  2. [ ] #2 连接复用",
      "  3. [ ] 都先不做",
      "  4. [ ] Type something",
      "Submit",
      "Enter to select · ↑/↓ to navigate · Esc to cancel",
    ].join("\r\n");
    expect(terminalAttention(prompt, [])).toBeNull();
    const attention = terminalAttention(prompt, [], true);
    expect(attention?.id).toBe("interactive:numbered-selector");
    expect(attention?.text).toContain("剩下这几个点要不要继续做？");
    expect(attention?.options?.map((option) => option.label)).toEqual([
      "#1 首屏尾读", "#2 连接复用", "都先不做", "Type something", "Submit",
    ]);
    expect(attention?.options?.[0].selected).toBe(false);
    expect(attention?.options?.[0].focused).toBe(true);
    expect(attention?.options?.[0].input).toBe("\r");
    expect(attention?.options?.[3].kind).toBe("input");
    expect(attention?.options?.[4].kind).toBe("submit");
  });

  it("deduplicates TUI redraws and ignores numbered prose outside checkbox choices", () => {
    const prompt = [
      "\x1b[2J1. <Transcript> 无虚拟化",
      "2. 每 650ms 轮询都新开一个 SQLite 连接",
      "剩下这几个点要不要继续做？",
      "> 1. [ ] #1 首屏尾读",
      "  2. [ ] #2 连接复用",
      "  3. [ ] 都先不做",
      "  4. [ ] Type something",
      "Submit",
      "剩下这几个点要不要继续做？",
      "  1. [✓] #1 首屏尾读",
      "> 2. [✓] #2 连接复用",
      "  3. [ ] 都先不做",
      "  4. [ ] Type something",
      "Submit",
      "Enter to select · ↑/↓ to navigate · Esc to cancel",
    ].join("\r\n");
    const attention = terminalAttention(prompt, [], true);
    expect(attention?.options?.map((option) => option.label)).toEqual([
      "#1 首屏尾读", "#2 连接复用", "都先不做", "Type something", "Submit",
    ]);
    expect(attention?.options?.slice(0, 2).map((option) => option.selected)).toEqual([true, true]);
    expect(attention?.options?.[1].focused).toBe(true);
    expect(attention?.options?.[2].input).toBe("\x1b[B\r");
    expect(attention?.options?.some((option) => option.label.includes("Transcript"))).toBe(false);
  });

  /// AskUserQuestion 的单选/多问题标签页形态:选项**不带复选框**,只有纯编号。
  /// 过去只认 `[ ]` 项,这批选项整组被当正文丢掉,GUI 卡片上只剩两个空按钮。
  /// 识别锚:菜单编号从 1 连续递增,且 run 内含 Type something / Chat about this。
  it("单选式提问(无复选框、多问题标签页)也要给出编号选项", () => {
    const prompt = [
      "\x1b[2J1. 正文里的普通列表项",
      "2. 不该被当成选项",
      "← 范围 风格 ✓ Submit →",
      "你觉得不好看的是哪个?",
      "❯ 1. 应用图标 (推荐)",
      "     任务栏/窗口上的默认 Tauri 蓝圆图标, 是占位符, 需要重新设计",
      "  2. 标题栏的琥珀色方块",
      "     界面左上角 Facet 文字旁的切面标记",
      "  3. 两个都要重做",
      "     统一重新设计一套品牌标识, 图标和界面标记保持一致",
      "  4. Type something.",
      "  5. Chat about this",
      "Enter to select · Tab/Arrow keys to navigate · Esc to cancel",
    ].join("\r\n");
    const attention = terminalAttention(prompt, [], true);
    expect(attention?.id).toBe("interactive:numbered-selector");
    expect(attention?.text).toContain("你觉得不好看的是哪个");
    expect(attention?.options?.map((option) => option.label)).toEqual([
      "应用图标 (推荐)", "标题栏的琥珀色方块", "两个都要重做", "Type something.", "Chat about this",
    ]);
    // 描述行归属各自选项。
    expect(attention?.options?.[0].description).toContain("占位符");
    expect(attention?.options?.[1].description).toContain("切面标记");
    // 相对移动从 ❯ 光标(第 1 项)出发。
    expect(attention?.options?.[0].focused).toBe(true);
    expect(attention?.options?.[0].input).toBe("\r");
    expect(attention?.options?.[2].input).toBe("\x1b[B\x1b[B\r");
    expect(attention?.options?.[3].kind).toBe("input");
    expect(attention?.options?.[4].kind).toBe("chat");
    // 单选菜单没有独立 Submit 行,不得虚构一个会乱移光标的提交按钮。
    expect(attention?.options?.some((option) => option.kind === "submit")).toBe(false);
    // 正文列表没混进来。
    expect(attention?.options?.some((option) => option.label.includes("正文"))).toBe(false);
  });

  /// 单选选项的描述里出现问号不该把选项 run 拦腰截断——分组锚是编号连续性,不是行内容。
  it("选项描述含问号时 run 不断裂", () => {
    const prompt = [
      "\x1b[2J要怎么处理这个端口冲突?",
      "❯ 1. 换端口",
      "     换到 2680, 有其他进程占用怎么办?",
      "  2. 杀掉占用进程",
      "  3. Chat about this",
      "Enter to select · ↑/↓ to navigate · Esc to cancel",
    ].join("\r\n");
    const attention = terminalAttention(prompt, [], true);
    expect(attention?.options?.map((option) => option.label)).toEqual([
      "换端口", "杀掉占用进程", "Chat about this",
    ]);
  });

  /// 字形兼容:`1)` 编号风格 + `[*]` 选中标记 + `›`(U+203A)光标——别家 CLI 的常见形态,
  /// 此前只认 Claude 当前版本的 `1.` / `[x]` / `❯`,这些菜单整组识别不出。
  it("兼容 1) 编号、[*] 勾选与 › 光标的菜单形态", () => {
    const numbered = [
      "\x1b[2J选择要启用的项",
      "› 1) [*] 已选中的项",
      "  2) [ ] 未选中的项",
      "  3) Type something",
      "Enter to select · ↑/↓ to navigate",
    ].join("\r\n");
    const attention = terminalAttention(numbered, [], true);
    expect(attention?.id).toBe("interactive:numbered-selector");
    expect(attention?.options?.map((option) => option.label)).toContain("已选中的项");
    expect(attention?.options?.find((option) => option.label === "已选中的项")?.selected).toBe(true);
    expect(attention?.options?.find((option) => option.label === "未选中的项")?.selected).toBe(false);

    const cursorMenu = [
      "\x1b[2JSelect a model",
      "Use arrow keys to move · enter to select",
      "  gpt-5.5-codex",
      "› gpt-5.5",
      "  gpt-5.5-mini",
    ].join("\r\n");
    const menu = terminalAttention(cursorMenu, [], false, true);
    expect(menu?.id).toBe("interactive:cursor-menu");
    expect(menu?.options?.map((option) => option.label)).toEqual(["gpt-5.5-codex", "gpt-5.5", "gpt-5.5-mini"]);
    expect(menu?.options?.[1].focused).toBe(true);
  });

  /// claude:* 整句规则带 provider 门控:别家 agent 的输出**引用**同一句话(讨论审批流程、
  /// cat 含该句的脚本)不得误弹 Claude 审批卡片锁住输入框。
  it("claude 专有整句规则只对 claude 会话生效", () => {
    const text = "\x1b[2Jecho test\r\nDo you want to proceed?\r\n> 1. Yes\r\n  2. No";
    const codex = terminalAttention(text, [], false, false, { provider: "codex", selectorAnchors: [] });
    expect(codex).toBeNull();
    // 缺省文法(存量调用)与显式 claude 都照旧识别。
    expect(terminalAttention(text, [])?.id).toBe("claude:command-approval");
    expect(terminalAttention(text, [], false, false, { provider: "claude", selectorAnchors: [] })?.id).toBe("claude:command-approval");
  });

  /// 锚点由插件声明:声明了别家文案的 provider,其纯编号单选菜单同样能卡片化;
  /// 未声明锚点的 provider 不出空卡(返回 null,交由发送侧软拦兜底)。
  it("选择器锚点走插件声明的文法", () => {
    const prompt = [
      "\x1b[2J选择下一步?",
      "❯ 1. 继续",
      "  2. 停止",
      "  3. 输入其他内容",
      "Enter to select · ↑/↓ to navigate",
    ].join("\r\n");
    const declared = terminalAttention(prompt, [], true, false, {
      provider: "someagent",
      selectorAnchors: [{ marker: "输入其他内容", kind: "input" }],
    });
    expect(declared?.id).toBe("interactive:numbered-selector");
    expect(declared?.options?.map((option) => option.label)).toEqual(["继续", "停止", "输入其他内容"]);
    expect(declared?.options?.[2].kind).toBe("input");
    // 同一屏,未声明锚点 → 不出没有任何可点项的空卡。
    expect(terminalAttention(prompt, [], true, false, { provider: "someagent", selectorAnchors: [] })).toBeNull();
  });

  /// gemini `/model` 对话框(真机 PTY 取证,gemini-cli 0.51):框线包裹的编号项 + ● 焦点,
  /// 无导航提示行。↑/↓ 移动、Enter 确认经按键探针证实——按钮输入 = 相对移动 + 回车。
  it("把 gemini 的框线数字菜单转成 GUI 选项(仅 expectMenu 窗口)", () => {
    const menu = [
      "\x1b[2J > /model",
      "╭──────────────────────────────╮",
      "│                              │",
      "│ Select Model│",
      "│                              │",
      "│ ● 1. Auto│",
      "│      Let Gemini CLI decide the best model for the task: gemini-3.1-pro-preview│",
      "│   2. Manual│",
      "│      Manually select a model│",
      "│                              │",
      "│ Remember model for future sessions: false (Press Tab to toggle)│",
      "│ > To use a specific Gemini model on startup, use the --model flag.│",
      "│                              │",
      "│ (Press Esc to close)│",
      "╰──────────────────────────────╯",
    ].join("\r\n");
    const attention = terminalAttention(menu, [], false, true, { provider: "gemini", selectorAnchors: [] });
    expect(attention?.id).toBe("interactive:cursor-menu");
    expect(attention?.text).toBe("Select Model");
    expect(attention?.options?.map((option) => option.label)).toEqual(["Auto", "Manual"]);
    expect(attention?.options?.[0].focused).toBe(true);
    expect(attention?.options?.[0].input).toBe("\r");
    expect(attention?.options?.[1].input).toBe("\x1b[B\r");
    expect(attention?.options?.[0].description).toContain("decide the best model");
    // 对话框尾注(Remember…/Press Esc…)不得折进最后一个选项的描述。
    expect(attention?.options?.[1].description).toBe("Manually select a model");
    // 不在菜单窗口内不认——框线数字形态只在刚发出菜单命令时有意义。
    expect(terminalAttention(menu, [], false, false, { provider: "gemini", selectorAnchors: [] })).toBeNull();
  });

  it("turns Claude's long-session token warning into explicit GUI choices", () => {
    const prompt = [
      "\x1b[2JThis session is 1d 9h old and 161.3k tokens.",
      "Resuming the full session will consume a substantial portion of your usage limits.",
      "❯ 1. Resume from summary (recommended)",
      "  2. Resume full session as-is",
      "  3. Don't ask me again",
      "Enter to confirm · Esc to cancel",
    ].join("\r\n");
    const attention = terminalAttention(prompt, []);
    expect(attention?.id).toBe("claude:long-session-resume");
    expect(attention?.options?.map((option) => option.label)).toEqual([
      "Resume from summary (recommended)",
      "Resume full session as-is",
      "Don't ask me again",
    ]);
    // 菜单首尾循环，必须从 ❯ 光标做相对移动，而不是盲按上键归零。
    expect(attention?.options?.[0].input).toBe("\r");
    expect(attention?.options?.[1].input).toBe("\x1b[B\r");
    expect(attention?.options?.[2].input).toBe("\x1b[B\x1b[B\r");
  });

  /// 中文本地化 CLI 的长会话菜单：选项不带 `1.` 编号、没有英文导航提示，过去会退化成
  /// 「上一项/下一项」。现在从 ❯ 光标块提取直接选项；与选项同缩进的说明句（句号结尾）
  /// 不能被吞进选项块。
  it("无编号的中文光标菜单也给直接选项，说明句不算选项", () => {
    const prompt = [
      "\x1b[2JThis session is 2d 3h old and 98.2k tokens.",
      "? 如何恢复这个长会话？",
      "  完整恢复会消耗较多额度，建议从摘要恢复。",
      "❯ 从摘要恢复（推荐）",
      "  恢复完整会话",
      "  取消",
    ].join("\r\n");
    const attention = terminalAttention(prompt, []);
    expect(attention?.id).toBe("claude:long-session-resume");
    expect(attention?.options?.map((option) => option.label)).toEqual([
      "从摘要恢复（推荐）",
      "恢复完整会话",
      "取消",
    ]);
    expect(attention?.options?.[0].input).toBe("\r");
    expect(attention?.options?.[1].input).toBe("\x1b[B\r");
    expect(attention?.options?.[2].input).toBe("\x1b[B\x1b[B\r");
  });

  /// 中文导航提示（「回车确认」而非 enter/select）同样算菜单信号。
  it("光标菜单的导航提示支持中文", () => {
    const screen = [
      "\x1b[2JSelect a model",
      "↑↓ 移动 · 回车确认",
      "  K2.7",
      "❯ K3",
    ].join("\r\n");
    const attention = terminalAttention(screen, [], false, true);
    expect(attention?.id).toBe("interactive:cursor-menu");
    expect(attention?.options?.map((option) => option.label)).toEqual(["K2.7", "K3"]);
    expect(attention?.options?.[0].input).toBe("\x1b[A\r");
  });

  /// 画面取自真机抓屏（app/src-tauri/tests/capture_model_menu.rs 跑 kimi `/model` 的结果）。
  /// 与编号选择器是两种形态：这里没有 `1.`，只有一个 ❯ 光标 + 一句导航提示。
  it("把 kimi 的 /model 光标菜单转成 GUI 选项", () => {
    const screen = [
      "\x1b[2J ──────────────────────────────────────────────",
      " Select a model  (type to search)",
      "  Tab toggle provider · ↑↓ navigate · Enter select · Alt+S session-only · Esc cancel",
      "  All   Kimi Code",
      "     K2.7 Coding            Kimi Code",
      "     K2.7 Coding Highspeed  Kimi Code",
      "   ❯ K3                     Kimi Code ← current",
      "  Thinking  (←→ to switch)",
      "     Low      High    [ Max ]",
      " ──────────────────────────────────────────────",
    ].join("\r\n");
    // 第四个参数 expectMenu：只有刚发出会弹菜单的命令时才认，避免把正文误报成菜单。
    expect(terminalAttention(screen, [])).toBeNull();
    const attention = terminalAttention(screen, [], false, true);
    expect(attention?.id).toBe("interactive:cursor-menu");
    expect(attention?.text).toContain("Select a model");
    // 只圈出与 ❯ 同缩进的那段：provider 过滤行（缩进更浅）和 Thinking 小节都不算选项。
    expect(attention?.options?.map((option) => option.label)).toEqual([
      "K2.7 Coding  Kimi Code",
      "K2.7 Coding Highspeed  Kimi Code",
      "K3  Kimi Code ← current",
    ]);
    // 菜单首尾循环，必须从 ❯ 做相对移动。
    expect(attention?.options?.[0].input).toBe("\x1b[A\x1b[A\r");
    expect(attention?.options?.[1].input).toBe("\x1b[A\r");
    expect(attention?.options?.[2].input).toBe("\r");
    expect(attention?.options?.[2].focused).toBe(true);
  });

  /// 守卫必须在**开着识别窗口时**依然成立——否则「没开窗口所以返回 null」会让用例空转。
  it("光标菜单要同时有导航提示和多个同级项，避免把正文误报成菜单", () => {
    // 只有 ❯ 没有导航提示：提示符、列表装饰都长这样，不能算菜单。
    const prose = "\x1b[2J❯ 第一点\r\n❯ 第二点\r\n就这些";
    expect(terminalAttention(prose, [], false, true)).toBeNull();
    // 有提示但只有一项：多半是提示符本身。
    const single = "\x1b[2JPick one\r\n↑↓ navigate · Enter select\r\n  ❯ 唯一项";
    expect(terminalAttention(single, [], false, true)).toBeNull();
    // 有提示、有多项，但那几项缩进不一致（是正文段落而非菜单块）。
    const uneven = "\x1b[2JPick one\r\n↑↓ navigate · Enter select\r\n  ❯ 甲\r\n      乙\r\n  丙";
    expect(terminalAttention(uneven, [], false, true)?.options?.length ?? 0).toBeLessThan(3);
  });

  it("turns Claude's command approval into its three native choices", () => {
    const prompt = [
      "\x1b[2JBash command",
      "cargo build -p meowo-agent -p meowo-store 2>&1 | tail -20",
      "Build rust crates",
      "This command requires approval",
      "Do you want to proceed?",
      "❯ 1. Yes",
      "  2. Yes, and don't ask again for: cargo build *",
      "  3. No",
      "Esc to cancel · Tab to amend · ctrl+e to explain",
    ].join("\r\n");
    const attention = terminalAttention(prompt, []);
    expect(attention?.id).toBe("claude:command-approval");
    expect(attention?.text).toContain("cargo build -p meowo-agent");
    expect(attention?.text).toContain("Build rust crates");
    expect(attention?.text).toContain("Do you want to proceed?");
    expect(attention?.text).not.toContain("1. Yes");
    expect(attention?.options?.map((option) => option.label)).toEqual([
      "Yes",
      "Yes, and don't ask again for: cargo build *",
      "No",
    ]);
  });

  /// kimi 审批面板(ApprovalPanel):面板形态与按键语义为官方源码取证
  /// (apps/kimi-code/src/tui/components/dialogs/approval-panel.ts @ 0.29)。
  /// PermissionRequest hook 是 observation-only,屏幕识别是 GUI 审批的唯一通道。
  it("turns Kimi's approval panel into digit-keyed choices, dropping feedback options", () => {
    const prompt = [
      "\x1b[2J────────────────────────",
      "  ▶ Run this command?",
      "",
      "  $ echo meowo-approval-probe-1234",
      "  Run the probe command",
      "",
      "  ▶ 1. Approve once",
      "    2. Approve for this session",
      "    3. Reject",
      "    4. Reject with feedback",
      "",
      "  ↑/↓ select · 1/2/3/4 choose · ↵ confirm",
      "────────────────────────",
    ].join("\r\n");
    const attention = terminalAttention(prompt, [], false, false, { provider: "kimi", selectorAnchors: [] });
    expect(attention?.id).toBe("kimi:command-approval");
    // 详情保留标题与命令,不含选项、按键提示和框线。
    expect(attention?.text).toContain("Run this command?");
    expect(attention?.text).toContain("echo meowo-approval-probe-1234");
    expect(attention?.text).not.toContain("Approve once");
    expect(attention?.text).not.toContain("choose");
    // 数字键直接选中提交(源码 selectAndSubmit),按钮 input 就是数字本身;
    // 「Reject with feedback」要现场输入反馈,卡片完成不了,不收。
    expect(attention?.options?.map((option) => [option.label, option.input])).toEqual([
      ["Approve once", "1"],
      ["Approve for this session", "2"],
      ["Reject", "3"],
    ]);
  });

  /// kimi 重绘不清屏:上一条命令的面板残留在缓冲上方。标题/提示行若锚定**首个**匹配,
  /// 卡片标题与命令会锁在残影上,数字键却打给屏幕最下方的活动面板——看着 A 批准了 B。
  /// (与 claude 路径的 lastIndexOf 同一课,见 terminalAttention 内注释。)
  it("kimi 面板残影:标题与命令取最后一份面板,不锁在残影上", () => {
    const prompt = [
      "\x1b[2J  ▶ Run this command?",
      "  $ echo old-command-A",
      "  ▶ 1. Approve once",
      "    2. Reject",
      "  ↑/↓ select · 1/2 choose · ↵ confirm",
      "  ▶ Run this command?",
      "  $ rm -rf build-B",
      "  ▶ 1. Approve once",
      "    2. Reject",
      "  ↑/↓ select · 1/2 choose · ↵ confirm",
    ].join("\r\n");
    const attention = terminalAttention(prompt, [], false, false, { provider: "kimi", selectorAnchors: [] });
    expect(attention?.id).toBe("kimi:command-approval");
    expect(attention?.text).toContain("rm -rf build-B");
    expect(attention?.text).not.toContain("old-command-A");
  });

  /// 命令正文里的顶格编号行(多行 commit message、带编号的输出)不是残影选项:回扫
  /// 钳位曾把上下文起点截到它后面,审批卡上命令开头(含 Bash command 标题)整段消失,
  /// 用户批准一条自己看不全的命令。残影选择器的判定要求编号组里带焦点光标。
  it("claude 审批:命令内的编号列表不截断上下文,也不解析成选项", () => {
    const prompt = [
      "\x1b[2JBash command",
      'git commit -m "fix: parser',
      "1. fix parser",
      "2. add tests",
      '"',
      "Commit the fix",
      "This command requires approval",
      "Do you want to proceed?",
      "❯ 1. Yes",
      "  2. No",
    ].join("\r\n");
    const attention = terminalAttention(prompt, []);
    expect(attention?.id).toBe("claude:command-approval");
    expect(attention?.text).toContain("Bash command");
    expect(attention?.text).toContain("git commit");
    expect(attention?.text).toContain("1. fix parser");
    expect(attention?.text).toContain("2. add tests");
    expect(attention?.options?.map((option) => option.label)).toEqual(["Yes", "No"]);
  });

  it("kimi 审批面板规则严格按 provider 门控:别家会话引用同一画面不弹卡", () => {
    const prompt = "\x1b[2J  ▶ Run this command?\r\n  ▶ 1. Approve once\r\n    2. Reject\r\n  ↑/↓ select · 1/2 choose · ↵ confirm";
    expect(terminalAttention(prompt, [], false, false, { provider: "claude", selectorAnchors: [] })).toBeNull();
    expect(terminalAttention(prompt, [], false, false, { provider: "kimi", selectorAnchors: [] })?.id).toBe("kimi:command-approval");
  });
});

describe("modeFromScreen", () => {
  const MARKERS = [
    { marker: "bypass permissions on", value: "bypassPermissions" },
    { marker: "plan mode on", value: "plan" },
    { marker: "manual mode on", value: "default" },
    { marker: "don't ask on", value: "dontAsk" },
  ];

  it("命中位置最靠后的指示胜出——backlog 尾部还留着切换前的旧指示", () => {
    const screen = `some output
⏸ manual mode on (shift+tab to cycle)
more
⏸ plan mode on (shift+tab to cycle)`;
    expect(modeFromScreen(screen, MARKERS)).toBe("plan");
  });

  it("大小写不敏感，弯引号归一（don't/don’t 都认）", () => {
    expect(modeFromScreen("⏵⏵ Don’t Ask On", MARKERS)).toBe("dontAsk");
    expect(modeFromScreen("BYPASS PERMISSIONS ON", MARKERS)).toBe("bypassPermissions");
  });

  it("无命中或无标记时返回 null，显示保持现状", () => {
    expect(modeFromScreen("plain composer text", MARKERS)).toBeNull();
    expect(modeFromScreen("⏸ plan mode on", [])).toBeNull();
  });
});

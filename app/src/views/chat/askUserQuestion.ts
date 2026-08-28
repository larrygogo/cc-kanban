// AskUserQuestion 题面的结构化解析。broker 自动放行该工具时把 PermissionRequest 的
// 参数原样转发过来（interactive-question 事件），选择列表卡直接从这份 JSON 渲染，
// 与终端表单同步出现——不必等屏幕识别从终端文本反推题面。
//
// 防御式解析：hook 参数来自 agent 侧，形态不受我们控制；解析不出就返回空数组，
// 调用方降级为不渲染（既有的屏幕识别路径仍会长出可作答的卡）。
export type StructuredQuestionOption = { label: string; description: string | null };
export type StructuredQuestion = {
  question: string;
  header: string | null;
  multiSelect: boolean;
  options: StructuredQuestionOption[];
};

/// 排队作答的选项匹配：题面 label（结构化参数）↔ 屏幕识别出的选项文本。识别文本可能
/// 被终端宽度截断（尾部省略号），按「精确 → 前缀互含（≥4 字符）」两级匹配；
/// 命中多个即歧义 → null，宁可留给用户手点，也不自动答错题。
export function matchOptionByLabel<T extends { label: string }>(
  options: readonly T[],
  label: string,
): T | null {
  const strip = (s: string) => s.trim().replace(/…+$/, "");
  const wanted = strip(label);
  if (!wanted) return null;
  const exact = options.filter((option) => strip(option.label) === wanted);
  if (exact.length === 1) return exact[0];
  if (exact.length > 1) return null;
  const prefixed = options.filter((option) => {
    const got = strip(option.label);
    return (
      (got.length >= 4 && wanted.startsWith(got)) ||
      (wanted.length >= 4 && got.startsWith(wanted))
    );
  });
  return prefixed.length === 1 ? prefixed[0] : null;
}

/// 多问题表单「当前聚焦第几题」的识别。实拍形态（terminalAttention.test.ts 的 fixture）：
/// tab 条 `← 头1 头2 ✓ Submit →` 只带各题 header，选项上方那行才是**聚焦题**的完整
/// question——屏幕文本里唯一能把第几题区分开的信号。故用结构化题面反查，两级匹配：
/// 1. 题面全文是屏幕文本的子串（双方**剥掉全部空白**再比：窄终端折行——含英文按字符
///    折断的 "consid er"——缩进、换行全归一，折成几行都照中）；
/// 2. 兜底：某条 ≥8 字符的屏幕行（去尾部截断省略号）是题面子串——题面被截断时仍有锚。
/// 命中零条或多条 → null：跨题写错答案比不答糟得多，歧义宁可保留排队等下一帧。
/// 刻意只匹配 question、不匹配 header：tab 条把**所有**题的 header 摆在一行，按 header
/// 匹配会题题都中——那正是要排除的伪信号。
export function matchFocusedQuestion(
  questions: readonly StructuredQuestion[],
  screenText: string,
): number | null {
  const collapse = (s: string) => s.replace(/\s+/g, "");
  const screen = collapse(screenText);
  if (!screen) return null;
  const lines = screenText
    .split("\n")
    .map((line) => collapse(line).replace(/…+$/, ""))
    .filter((line) => line.length >= 8);
  const matches: number[] = [];
  for (const [index, question] of questions.entries()) {
    const text = collapse(question.question);
    if (!text) continue;
    if (screen.includes(text) || lines.some((line) => text.includes(line))) {
      matches.push(index);
    }
  }
  return matches.length === 1 ? matches[0] : null;
}

/// 排队多选的落键规划：把一组排队 label 翻译成「方向键定位 + 回车勾选」的按键串序列。
/// 多选表单里每次勾选都是相对当前焦点项的移动——序列按焦点逐项推进计算，命中的重复项
/// 去重（同一项勾两次等于没勾）。匹配不上的 label 跳过（单条歧义不拖垮整组）；全部落空
/// 返回空数组，调用方保留排队状态，留给用户手点，绝不猜。
/// 提交不归这里：多选表单有独立 submit 项，勾完由用户在交互卡上按「提交选择」。
export function planQueuedChoiceWrites<T extends { label: string; position?: number; focused?: boolean }>(
  choices: readonly T[],
  labels: readonly string[],
): { option: T; input: string }[] {
  const writes: { option: T; input: string }[] = [];
  let focus = choices.find((option) => option.focused)?.position ?? choices[0]?.position ?? 0;
  const done = new Set<number>();
  for (const label of labels) {
    const match = matchOptionByLabel(choices, label);
    if (!match) continue;
    const target = match.position ?? 0;
    if (done.has(target)) continue;
    const delta = target - focus;
    writes.push({
      option: match,
      input: (delta < 0 ? "\x1b[A".repeat(-delta) : "\x1b[B".repeat(delta)) + "\r",
    });
    done.add(target);
    focus = target;
  }
  return writes;
}

/// 「问题已了结」的收卡判定，两个信号（实测各自都有盲区，必须并用）：
///
/// 1. **回合结束**（status 离开 running）：Esc 取消后 agent 回合直接结束、不再产出——
///    只靠 transcript 增长会漏掉快速取消（增长发生在静置期内被吸收，其后再无增长，
///    实测踩过）。Stop hook 把状态翻成 waiting 是确定性的了结信号。
/// 2. **transcript 增长**：作答后 agent 继续跑，回合可能很长，等回合结束才收卡太慢；
///    增长即说明答案已送达。陷阱是提问自身的 tool_use 与题面几乎同时落盘，故 armAt
///    之前的静置期把增长**吸收进基线**（状态信号同样等过静置期：题面刚出现时前端的
///    history 可能还是上一轮的旧值，立即判会误收）。
///
/// 基线在首次调用（题面出现的渲染周期）就记下，绝不能等下一次 transcript 变化才记：
/// 阻塞等答的会话在作答前零增长，作答后往往只有**一次**增长事件，懒记录会把它消费掉。
export type QuestionDismissTracker = { armAt: number; count: number | null };

export function observeTranscriptForDismiss(
  tracker: QuestionDismissTracker,
  // running 为 null = **状态未知**（history 尚未加载／刚 resetTo 清空），不是「已结束」。
  // 二者混同会让卡在冷启动路径上闪现 1.5 秒就自己消失：切窗触发 resetTo → history 置
  // null → 静置期一过，任何一次重渲染都判成回合结束而收卡（实测踩过）。
  observation: { count: number; running: boolean | null },
  now: number,
): boolean {
  if (now < tracker.armAt) {
    tracker.count = Math.max(tracker.count ?? observation.count, observation.count);
    return false;
  }
  if (observation.running === false) {
    return true;
  }
  if (observation.running === null) {
    return false;
  }
  if (tracker.count == null) {
    tracker.count = observation.count;
    return false;
  }
  return observation.count > tracker.count;
}

/// 作答卡上单题的作答草稿：selected 是点选的选项 label（单选至多一个，多选任意个），
/// custom 是自定义补充文本。二者可并存（选了选项还想补一句）。
export type QuestionAnswerDraft = { selected: string[]; custom: string };

/// 拼代答正文（`answer:<正文>` 的 payload，broker 会在前面统一包引导语）。
/// 每题一行「题面 → 答案」，多选顿号拼接，自定义文本追加在选项之后。
/// 任何一题既没点选也没自定义文本 → null（不可提交）；正文是给模型看的，不进 i18n。
export function composeAnswerBody(
  questions: StructuredQuestion[],
  answers: ReadonlyMap<number, QuestionAnswerDraft>,
): string | null {
  if (questions.length === 0) return null;
  const lines: string[] = [];
  for (const [index, question] of questions.entries()) {
    const draft = answers.get(index);
    const selected = draft?.selected ?? [];
    const custom = draft?.custom.trim() ?? "";
    if (selected.length === 0 && !custom) return null;
    const parts = [...selected];
    if (custom) parts.push(custom);
    const title = (question.header ? `${question.header} · ${question.question}` : question.question)
      || `问题 ${index + 1}`;
    lines.push(`${title} → ${parts.join("、")}`);
  }
  return lines.join("\n");
}

export function parseAskUserQuestions(input: string): StructuredQuestion[] {
  let parsed: unknown;
  try {
    parsed = JSON.parse(input);
  } catch {
    return [];
  }
  const questions = (parsed as { questions?: unknown } | null)?.questions;
  if (!Array.isArray(questions)) return [];
  const out: StructuredQuestion[] = [];
  for (const raw of questions) {
    if (typeof raw !== "object" || raw === null) continue;
    const item = raw as Record<string, unknown>;
    const question = typeof item.question === "string" ? item.question : "";
    const options = Array.isArray(item.options)
      ? item.options.flatMap((entry) => {
          if (typeof entry !== "object" || entry === null) return [];
          const option = entry as Record<string, unknown>;
          if (typeof option.label !== "string" || !option.label) return [];
          return [{
            label: option.label,
            description: typeof option.description === "string" && option.description ? option.description : null,
          }];
        })
      : [];
    // 没题面也没选项的条目不可展示，跳过；只有其一仍可渲染（纯开放问题/纯选项）。
    if (!question && options.length === 0) continue;
    out.push({
      question,
      header: typeof item.header === "string" && item.header ? item.header : null,
      multiSelect: item.multiSelect === true,
      options,
    });
  }
  return out;
}

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
  options: T[],
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

/// 「在终端作答完成」的收卡判定。题面出现后 transcript 一旦增长（agent 拿到答案/拒绝
/// 才会继续产出）就该收卡，但有一个陷阱：提问自身的 tool_use 与题面几乎同时落盘。
/// 故分两段——armAt 之前的静置期把一切增长**吸收进基线**；静置期过后首次增长即收卡。
/// 基线在首次调用（题面出现的渲染周期）就记下，绝不能等下一次 transcript 变化才记：
/// 阻塞等答的会话在作答前零增长，作答后往往只有**一次**增长事件，懒记录会把它消费掉，
/// 卡就永远收不掉（实测踩过）。
export type QuestionDismissTracker = { armAt: number; count: number | null };

export function observeTranscriptForDismiss(
  tracker: QuestionDismissTracker,
  count: number,
  now: number,
): boolean {
  if (now < tracker.armAt) {
    tracker.count = Math.max(tracker.count ?? count, count);
    return false;
  }
  if (tracker.count == null) {
    tracker.count = count;
    return false;
  }
  return count > tracker.count;
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

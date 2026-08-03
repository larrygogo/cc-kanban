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

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

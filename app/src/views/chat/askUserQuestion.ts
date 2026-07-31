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

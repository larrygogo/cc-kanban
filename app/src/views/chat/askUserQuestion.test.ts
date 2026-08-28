import { describe, expect, it } from "vitest";
import {
  composeAnswerBody,
  matchFocusedQuestion,
  matchOptionByLabel,
  observeTranscriptForDismiss,
  parseAskUserQuestions,
  planQueuedChoiceWrites,
  type QuestionAnswerDraft,
  type QuestionDismissTracker,
  type StructuredQuestion,
} from "./askUserQuestion";

describe("composeAnswerBody", () => {
  const question = (over: Partial<StructuredQuestion>): StructuredQuestion => ({
    question: "问点什么？",
    header: null,
    multiSelect: false,
    options: [],
    ...over,
  });
  const draft = (selected: string[], custom = ""): QuestionAnswerDraft => ({ selected, custom });

  it("逐题一行,多选顿号拼接,自定义追加在选项后", () => {
    const questions = [
      question({ header: "晚饭", question: "晚饭吃什么？" }),
      question({ header: "配菜", question: "配菜选哪些？", multiSelect: true }),
    ];
    const answers = new Map([
      [0, draft(["火锅"])],
      [1, draft(["毛肚", "虾滑"], " 少放辣 ")],
    ]);
    expect(composeAnswerBody(questions, answers)).toBe(
      "晚饭 · 晚饭吃什么？ → 火锅\n配菜 · 配菜选哪些？ → 毛肚、虾滑、少放辣",
    );
  });

  it("任一题空着(含纯空白自定义)即不可提交", () => {
    const questions = [question({}), question({ multiSelect: true })];
    expect(composeAnswerBody(questions, new Map([[0, draft(["A"])]]))).toBeNull();
    expect(
      composeAnswerBody(questions, new Map([[0, draft(["A"])], [1, draft([], "   ")]])),
    ).toBeNull();
    expect(composeAnswerBody([], new Map())).toBeNull();
  });

  it("纯自定义文本可作答,无 header 的题面直接用问题原文", () => {
    const questions = [question({ question: "还有什么要补充？" })];
    expect(composeAnswerBody(questions, new Map([[0, draft([], "没有了")]]))).toBe(
      "还有什么要补充？ → 没有了",
    );
  });
});

describe("planQueuedChoiceWrites", () => {
  const choice = (label: string, position: number, focused = false) => ({ label, position, focused });

  it("按焦点逐项推进生成「方向键 + 回车」勾选序列", () => {
    const choices = [choice("A", 0, true), choice("B", 1), choice("C", 2), choice("D", 3)];
    // 勾 D 再勾 B：先到 D(↓×3)，再回 B(↑×2)。
    expect(planQueuedChoiceWrites(choices, ["D", "B"])).toEqual([
      { option: choices[3], input: "\x1b[B".repeat(3) + "\r" },
      { option: choices[1], input: "\x1b[A".repeat(2) + "\r" },
    ]);
  });

  it("无焦点标记时以首项为起点；当前项勾选不带方向键", () => {
    const choices = [choice("A", 0), choice("B", 1)];
    expect(planQueuedChoiceWrites(choices, ["A", "B"])).toEqual([
      { option: choices[0], input: "\r" },
      { option: choices[1], input: "\x1b[B\r" },
    ]);
  });

  it("重复命中去重；匹配不上的 label 跳过；全部落空返回空", () => {
    const choices = [choice("A", 0, true), choice("B", 1)];
    // "B" 与截断文本 "B" 同指一项（此处用相同 label 模拟同位去重）。
    expect(planQueuedChoiceWrites(choices, ["B", "B"])).toEqual([
      { option: choices[1], input: "\x1b[B\r" },
    ]);
    expect(planQueuedChoiceWrites(choices, ["不存在的选项"])).toEqual([]);
  });
});

describe("observeTranscriptForDismiss", () => {
  const tracker = (armAt: number): QuestionDismissTracker => ({ armAt, count: null });
  const running = (count: number) => ({ count, running: true as const });
  const settled = (count: number) => ({ count, running: false as const });
  const unknown = (count: number) => ({ count, running: null });

  /**
   * 回归（实测踩过）：阻塞等答的会话在作答前零增长，作答后往往只有一次增长事件。
   * 基线必须在题面出现当刻（首次观察）记下——懒等下一次变化才记，会把唯一的增长
   * 事件消费成基线，卡永远收不掉。
   */
  it("静置期内记基线,静置期后唯一一次增长即收卡", () => {
    const state = tracker(3_000);
    // 题面出现当刻（静置期内）：记基线，不收卡。
    expect(observeTranscriptForDismiss(state, running(10), 0)).toBe(false);
    // 提问自身的 tool_use 落盘（仍在静置期）：吸收进基线。
    expect(observeTranscriptForDismiss(state, running(11), 1_000)).toBe(false);
    // 静置期后长时间无变化：不收卡。
    expect(observeTranscriptForDismiss(state, running(11), 60_000)).toBe(false);
    // 用户作答后 agent 继续产出——唯一一次增长，必须收卡。
    expect(observeTranscriptForDismiss(state, running(12), 61_000)).toBe(true);
  });

  /**
   * 回归（实测踩过）：快速 Esc 取消——增长发生在静置期内被吸收进基线，其后回合直接
   * 结束、再无任何增长。回合结束（status 离开 running）必须是独立的收卡信号。
   */
  it("快速 Esc:增长被静置期吸收后,回合结束信号收卡", () => {
    const state = tracker(3_000);
    expect(observeTranscriptForDismiss(state, running(10), 0)).toBe(false);
    // Esc 在静置期内落盘：增长被吸收，此刻还不收（前端 history 可能还是旧值）。
    expect(observeTranscriptForDismiss(state, settled(12), 2_000)).toBe(false);
    // 静置期后：状态已是回合结束 → 无论有没有新增长都收卡。
    expect(observeTranscriptForDismiss(state, settled(12), 3_500)).toBe(true);
  });

  /**
   * 回归（实测踩过）：history 尚未加载时 running 是 **null（未知）**，不能当「已结束」。
   * 冷启动路径正是如此——broker 切窗触发 resetTo 把 history 置 null，若判成回合结束，
   * 卡会在静置期一过的任意一次重渲染里闪现 1.5 秒就自己消失。
   */
  it("状态未知不等于回合结束,不收卡", () => {
    const state = tracker(1_500);
    expect(observeTranscriptForDismiss(state, unknown(0), 0)).toBe(false);
    expect(observeTranscriptForDismiss(state, unknown(0), 2_000)).toBe(false);
    expect(observeTranscriptForDismiss(state, unknown(0), 60_000)).toBe(false);
    // history 加载出来、确证仍在回合中：照常按增长判定。
    expect(observeTranscriptForDismiss(state, running(0), 61_000)).toBe(false);
    expect(observeTranscriptForDismiss(state, running(1), 62_000)).toBe(true);
  });

  it("静置期内没有任何观察时,静置期后先补记基线再看增长", () => {
    const state = tracker(3_000);
    expect(observeTranscriptForDismiss(state, running(10), 5_000)).toBe(false); // 补记基线
    expect(observeTranscriptForDismiss(state, running(10), 6_000)).toBe(false);
    expect(observeTranscriptForDismiss(state, running(13), 7_000)).toBe(true);
  });
});

describe("matchOptionByLabel", () => {
  const options = [{ label: "autopilot-v2" }, { label: "autopilot-core" }, { label: "全新产品名" }];

  it("精确匹配优先", () => {
    expect(matchOptionByLabel(options, "autopilot-core")).toEqual({ label: "autopilot-core" });
  });

  it("识别文本被截断（尾部省略号）时按前缀互含匹配", () => {
    expect(matchOptionByLabel([{ label: "autopilot-v2 (recomm…" }], "autopilot-v2 (recommended)"))
      .toEqual({ label: "autopilot-v2 (recomm…" });
  });

  it("歧义与未命中都返回 null，不自动作答", () => {
    // "autopilot-" 同时是两个选项的前缀 → 歧义。
    expect(matchOptionByLabel(options, "autopilot-")).toBeNull();
    expect(matchOptionByLabel(options, "不存在的选项")).toBeNull();
    expect(matchOptionByLabel(options, "  ")).toBeNull();
  });
});

describe("matchFocusedQuestion", () => {
  const questions: StructuredQuestion[] = [
    { header: "范围", question: "重做的范围是哪些？", multiSelect: true, options: [{ label: "图标", description: null }] },
    { header: "风格", question: "你觉得不好看的是哪个?", multiSelect: false, options: [{ label: "应用图标 (推荐)", description: null }] },
    { header: "节奏", question: "什么时候交付？", multiSelect: false, options: [{ label: "本周", description: null }] },
  ];
  // 实拍形态（terminalAttention.test.ts 的多问题 fixture）：tab 条只带 header，
  // 选项上方那行是聚焦题的完整 question。
  const screenOf = (focused: string) => [
    "← 范围 风格 节奏 ✓ Submit →",
    focused,
    "❯ 1. 应用图标 (推荐)",
    "  2. 标题栏的琥珀色方块",
    "Enter to select · Tab/Arrow keys to navigate · Esc to cancel",
  ].join("\r\n");

  it("题面原文上屏即认出聚焦题", () => {
    expect(matchFocusedQuestion(questions, screenOf("你觉得不好看的是哪个?"))).toBe(1);
    expect(matchFocusedQuestion(questions, screenOf("重做的范围是哪些？"))).toBe(0);
    expect(matchFocusedQuestion(questions, screenOf("什么时候交付？"))).toBe(2);
  });

  it("题面被窄终端折行（空白归一后）仍命中", () => {
    const wrapped = screenOf("你觉得不好看\n的是哪个?");
    expect(matchFocusedQuestion(questions, wrapped)).toBe(1);
  });

  it("tab 条的 header 不产生匹配（只按 question 反查）", () => {
    // 三题的 header 全在 tab 条上，但没有任何一题的 question 上屏 → null。
    const tabOnly = ["← 范围 风格 节奏 ✓ Submit →", "❯ 1. 某选项"].join("\r\n");
    expect(matchFocusedQuestion(questions, tabOnly)).toBeNull();
  });

  it("歧义（两题同文）与未命中都返回 null，不跨题盲写", () => {
    const dup: StructuredQuestion[] = [
      { ...questions[0], question: "一样的问题？" },
      { ...questions[1], question: "一样的问题？" },
    ];
    expect(matchFocusedQuestion(dup, screenOf("一样的问题？"))).toBeNull();
    expect(matchFocusedQuestion(questions, screenOf("屏幕上是别的东西"))).toBeNull();
    expect(matchFocusedQuestion(questions, "")).toBeNull();
    // question 为空的题不可识别，但也不拖垮能认出的题。
    const withEmpty: StructuredQuestion[] = [{ ...questions[0], question: "" }, questions[1]];
    expect(matchFocusedQuestion(withEmpty, screenOf("你觉得不好看的是哪个?"))).toBe(1);
  });
});

describe("parseAskUserQuestions", () => {
  it("解析标准题面（问题/标签/单多选/选项描述）", () => {
    const input = JSON.stringify({
      questions: [
        {
          header: "仓库名",
          question: "新仓库叫什么名字？",
          multiSelect: false,
          options: [
            { label: "autopilot-v2", description: "沿用产品名" },
            { label: "autopilot-core" },
          ],
        },
      ],
    });
    expect(parseAskUserQuestions(input)).toEqual([
      {
        question: "新仓库叫什么名字？",
        header: "仓库名",
        multiSelect: false,
        options: [
          { label: "autopilot-v2", description: "沿用产品名" },
          { label: "autopilot-core", description: null },
        ],
      },
    ]);
  });

  it("hook 参数不受控：坏 JSON / 缺字段 / 空条目一律安静降级", () => {
    expect(parseAskUserQuestions("not json")).toEqual([]);
    expect(parseAskUserQuestions("{}")).toEqual([]);
    expect(parseAskUserQuestions(JSON.stringify({ questions: "nope" }))).toEqual([]);
    // 既无题面也无可用选项的条目被跳过；label 缺失的选项被丢弃。
    expect(
      parseAskUserQuestions(
        JSON.stringify({ questions: [{ multiSelect: true }, { question: "q", options: [{ description: "无 label" }] }] }),
      ),
    ).toEqual([{ question: "q", header: null, multiSelect: false, options: [] }]);
  });
});

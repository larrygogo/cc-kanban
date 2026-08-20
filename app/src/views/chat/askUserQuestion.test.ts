import { describe, expect, it } from "vitest";
import {
  composeAnswerBody,
  matchOptionByLabel,
  observeTranscriptForDismiss,
  parseAskUserQuestions,
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

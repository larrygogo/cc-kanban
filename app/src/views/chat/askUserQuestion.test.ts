import { describe, expect, it } from "vitest";
import {
  matchOptionByLabel,
  observeTranscriptForDismiss,
  parseAskUserQuestions,
  type QuestionDismissTracker,
} from "./askUserQuestion";

describe("observeTranscriptForDismiss", () => {
  const tracker = (armAt: number): QuestionDismissTracker => ({ armAt, count: null });

  /**
   * 回归（实测踩过）：阻塞等答的会话在作答前零增长，作答后往往只有一次增长事件。
   * 基线必须在题面出现当刻（首次观察）记下——懒等下一次变化才记，会把唯一的增长
   * 事件消费成基线，卡永远收不掉。
   */
  it("静置期内记基线,静置期后唯一一次增长即收卡", () => {
    const state = tracker(3_000);
    // 题面出现当刻（静置期内）：记基线，不收卡。
    expect(observeTranscriptForDismiss(state, 10, 0)).toBe(false);
    // 提问自身的 tool_use 落盘（仍在静置期）：吸收进基线。
    expect(observeTranscriptForDismiss(state, 11, 1_000)).toBe(false);
    // 静置期后长时间无变化：不收卡。
    expect(observeTranscriptForDismiss(state, 11, 60_000)).toBe(false);
    // 用户作答/取消后 agent 继续产出——唯一一次增长，必须收卡。
    expect(observeTranscriptForDismiss(state, 12, 61_000)).toBe(true);
  });

  it("静置期内没有任何观察时,静置期后先补记基线再看增长", () => {
    const state = tracker(3_000);
    expect(observeTranscriptForDismiss(state, 10, 5_000)).toBe(false); // 补记基线
    expect(observeTranscriptForDismiss(state, 10, 6_000)).toBe(false);
    expect(observeTranscriptForDismiss(state, 13, 7_000)).toBe(true);
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

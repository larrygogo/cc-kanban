import { describe, expect, it } from "vitest";
import { matchOptionByLabel, parseAskUserQuestions } from "./askUserQuestion";

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

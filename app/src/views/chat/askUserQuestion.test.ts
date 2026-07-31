import { describe, expect, it } from "vitest";
import { parseAskUserQuestions } from "./askUserQuestion";

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

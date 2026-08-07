import { describe, it, expect, beforeEach } from "vitest";
import { applyLearnedLabels, loadLearnedLabels, saveLearnedLabels } from "./modelLabels";

const presets = [
  { id: "fable", label: "Fable" },
  { id: "opus", label: "Opus" },
  { id: "sonnet", label: "Sonnet" },
  { id: "haiku", label: "Haiku" },
  { id: "opusplan", label: "Opus Plan" },
];

describe("模型标签学习", () => {
  beforeEach(() => localStorage.clear());

  /// 核心诉求：清单标签由 CLI 现给，宿主那份只是兜底——CLI 改版（Opus → Opus 5）不必改代码。
  it("用 CLI 菜单的真实标签替换内置标签", () => {
    const applied = applyLearnedLabels(presets, ["Fable 5", "Opus 5", "Sonnet 5", "Haiku 4.5"]);
    expect(applied.map((p) => p.label)).toEqual(["Fable 5", "Opus 5", "Sonnet 5", "Haiku 4.5", "Opus Plan"]);
    // 别名不动：切换仍走 `/model <别名>`，那是 CLI 文档化的稳定契约。
    expect(applied.map((p) => p.id)).toEqual(presets.map((p) => p.id));
  });

  /// "Opus Plan" 同时含 opus 与 opusplan（规范化后），必须落到更长的 opusplan 上，
  /// 否则 opus 会被写成 "Opus Plan"，用户点「Opus」实际切到了 opusplan。
  it("多别名命中时取最长匹配", () => {
    const applied = applyLearnedLabels(presets, ["Opus Plan", "Opus 5"]);
    expect(applied.find((p) => p.id === "opusplan")?.label).toBe("Opus Plan");
    expect(applied.find((p) => p.id === "opus")?.label).toBe("Opus 5");
  });

  it("学不到的预设保留内置标签，菜单里的陌生条目被忽略", () => {
    const applied = applyLearnedLabels(presets, ["Opus 5", "Some New Model"]);
    expect(applied.find((p) => p.id === "opus")?.label).toBe("Opus 5");
    expect(applied.find((p) => p.id === "sonnet")?.label).toBe("Sonnet");
    expect(applied).toHaveLength(presets.length);
  });

  it("没学过时原样返回", () => {
    expect(applyLearnedLabels(presets, null)).toEqual(presets);
    expect(applyLearnedLabels(presets, [])).toEqual(presets);
  });

  /// 缓存按 provider + CLI 版本：CLI 升级后旧标签自动失效、重新学一次。
  it("缓存按 CLI 版本隔离，升级后旧条目被清掉", () => {
    saveLearnedLabels("claude", "2.1.0", ["Opus 5"]);
    expect(loadLearnedLabels("claude", "2.1.0")).toEqual(["Opus 5"]);
    expect(loadLearnedLabels("claude", "2.2.0")).toBeNull();

    saveLearnedLabels("claude", "2.2.0", ["Opus 6"]);
    expect(loadLearnedLabels("claude", "2.2.0")).toEqual(["Opus 6"]);
    // 同 provider 的旧版本条目不再有意义，顺手清掉，别让存储无限长。
    expect(loadLearnedLabels("claude", "2.1.0")).toBeNull();
  });

  it("不同 provider 互不干扰", () => {
    saveLearnedLabels("claude", "2.1.0", ["Opus 5"]);
    saveLearnedLabels("kimi", "1.0.0", ["K2"]);
    expect(loadLearnedLabels("claude", "2.1.0")).toEqual(["Opus 5"]);
    expect(loadLearnedLabels("kimi", "1.0.0")).toEqual(["K2"]);
  });
});

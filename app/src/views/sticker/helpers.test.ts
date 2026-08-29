import { describe, expect, it } from "vitest";
import { zh } from "../../i18n/zh";
import { en } from "../../i18n/en";
import { fmtAgo } from "./helpers";

// fmtAgo 走 Intl.RelativeTimeFormat（G-13）：按界面语言生成相对时间，不再手拼。
describe("fmtAgo", () => {
  const now = Date.now();

  it("returns the 'now' label under one minute, in both locales", () => {
    expect(fmtAgo(now, zh)).toBe("刚刚");
    expect(fmtAgo(now, en)).toBe("now");
    expect(fmtAgo(now - 30_000, en)).toBe("now");
  });

  it("formats minutes/hours via Intl for zh and en", () => {
    expect(fmtAgo(now - 3 * 60_000, zh)).toBe("3分钟前");
    expect(fmtAgo(now - 5 * 3_600_000, zh)).toBe("5小时前");
    expect(fmtAgo(now - 3 * 60_000, en)).toBe("3 minutes ago");
    expect(fmtAgo(now - 5 * 3_600_000, en)).toBe("5 hours ago");
    // Intl 的自然语：手拼时代 en 是 "1 min ago" 缩写，Intl 不缩写且处理单复数。
    expect(fmtAgo(now - 60_000, en)).toBe("1 minute ago");
  });

  it("uses calendar words for day-scale gaps (numeric: auto)", () => {
    // 与独立构造的 Intl 对齐，而非硬编码字符串——运行环境 ICU 差异不致误报。
    const zhRtf = new Intl.RelativeTimeFormat("zh-CN", { numeric: "auto" });
    const enRtf = new Intl.RelativeTimeFormat("en-US", { numeric: "auto" });
    expect(fmtAgo(now - 26 * 3_600_000, zh)).toBe(zhRtf.format(-1, "day"));
    expect(fmtAgo(now - 3 * 86_400_000, en)).toBe(enRtf.format(-3, "day"));
  });

  it("falls back to days beyond 24 hours instead of large hour counts", () => {
    expect(fmtAgo(now - 30 * 3_600_000, en)).not.toContain("hour");
  });
});

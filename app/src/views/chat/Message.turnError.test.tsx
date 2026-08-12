import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { Message } from "./Message";
import type { ChatItem } from "../../api";

afterEach(cleanup);

/// 回合错误气泡(turn_error):CC 把 API Error 写成 model=<synthetic> 的系统插入文案,
/// 后端解析降级成独立的 turn_error 条目。此前它披着模型正文的皮平铺在对话里,
/// 用户读不出这是故障——本测试钉住「错误必须套错误皮」这条回归。
describe("Message turn_error", () => {
  const item: ChatItem = {
    type: "turn_error",
    id: "e1",
    timestamp: null,
    label: "API 请求出错",
    text: "API Error: Overloaded",
  };

  it("渲染成 role=alert 的错误气泡,标签与原文都可见", () => {
    render(<Message item={item} />);
    const alert = screen.getByRole("alert");
    expect(alert.className).toContain("is-turn-error");
    // 标签(与看板卡片 error_label 同源)和原文都要在——标签给一眼判断,原文给排查。
    expect(screen.getByText("API 请求出错")).toBeTruthy();
    expect(screen.getByText("API Error: Overloaded")).toBeTruthy();
  });

  it("不套模型正文的皮(is-assistant)", () => {
    render(<Message item={item} />);
    expect(screen.getByRole("alert").className).not.toContain("is-assistant");
  });
});

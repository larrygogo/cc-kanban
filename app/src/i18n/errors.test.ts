import { describe, it, expect } from "vitest";
import { formatBackendError } from "./errors";

describe("formatBackendError", () => {
  it("精确匹配:中英各取对应文案", () => {
    expect(formatBackendError("无效 session_id", "zh-CN")).toBe("会话已失效，请重新打开");
    expect(formatBackendError("无效 session_id", "en-US")).toBe("Session is no longer valid; reopen it");
  });

  it("前缀匹配:带参数的后端错误也能命中", () => {
    expect(formatBackendError("连接后台会话失败：boom", "en-US")).toContain("background session");
    expect(formatBackendError("附件过大（上限 32MB）", "en-US")).toBe("Attachment too large (max 32MB)");
  });

  it("Error 对象取 message 再匹配", () => {
    expect(formatBackendError(new Error("会话不存在"), "en-US")).toBe("Session not found");
  });

  it("未命中:中文界面原样显示", () => {
    expect(formatBackendError("某个没映射过的错误", "zh-CN")).toBe("某个没映射过的错误");
  });

  it("未命中且含汉字:英文界面降级为通用前缀,不让中文直漏", () => {
    expect(formatBackendError("某个没映射过的错误", "en-US")).toBe("Operation failed: 某个没映射过的错误");
  });

  it("未命中且不含汉字:英文界面原样显示", () => {
    expect(formatBackendError("connection refused", "en-US")).toBe("connection refused");
  });

  it("profile reason 码(S-9):按当前语言映射,detail 透传", () => {
    // 进行中会话数走 tail 透传
    expect(formatBackendError("profile/has-live-sessions: 2", "zh-CN")).toContain("进行中的会话");
    expect(formatBackendError("profile/has-live-sessions: 2", "en-US")).toContain("live sessions");
    // 长前缀优先:unrestored 不被 delete-dir-failed 抢先命中
    expect(formatBackendError("profile/delete-dir-failed-unrestored: io", "en-US")).toContain("restoring the entry also failed");
    expect(formatBackendError("profile/delete-dir-failed: io", "zh-CN")).toContain("已恢复");
    // 英文界面不再漏中文
    expect(formatBackendError("profile/copy-failed: /tmp/x: io", "en-US")).not.toMatch(/[一-鿿]/);
  });
});

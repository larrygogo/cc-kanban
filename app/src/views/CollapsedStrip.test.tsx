import { describe, it, expect, afterEach, vi } from "vitest";
import { render, cleanup } from "@testing-library/react";
import { CollapsedStrip } from "./CollapsedStrip";
import type { LiveSession } from "../api";

type Item = LiveSession & { connected: boolean };

function mk(over: Partial<Item> = {}): Item {
  return {
    session: { id: 1, project_id: 1, cc_session_id: "s", status: "running", started_at: 0, last_event_at: 0, ended_at: null },
    project_name: "proj",
    task_title: "t",
    current_activity: null,
    column: "doing", todo_done: 0, todo_total: 0, todos: [],
    pid: 1, connected: true, archived: false, provider: "claude",
    ...over,
  } as Item;
}

afterEach(() => cleanup());

describe("CollapsedStrip", () => {
  it("connected 会话各渲染一个圆点，按状态给类名", () => {
    const data: Item[] = [
      mk({ session: { id: 1, project_id: 1, cc_session_id: "a", status: "running", started_at: 0, last_event_at: 0, ended_at: null }, connected: true }),
      mk({ session: { id: 2, project_id: 1, cc_session_id: "b", status: "waiting", started_at: 0, last_event_at: 0, ended_at: null }, connected: true }),
    ];
    const { container } = render(<CollapsedStrip data={data} edge="left" onExpand={() => {}} />);
    expect(container.querySelectorAll(".cstrip-dot").length).toBe(2);
    expect(container.querySelectorAll(".cstrip-running").length).toBe(1);
    expect(container.querySelectorAll(".cstrip-waiting").length).toBe(1);
  });

  it("待审批/屏幕阻塞会话显示琥珀点（与看板卡片 cardTone 同口径），不被画成绿色运行点", () => {
    const data: Item[] = [
      // DB status 还停在 running，但 broker 正压着审批——必须显示 pending 而不是 running。
      mk({ session: { id: 1, project_id: 1, cc_session_id: "a", status: "running", started_at: 0, last_event_at: 0, ended_at: null }, pending_review: "approval" }),
      // 屏幕检测到 blocked（审批弹窗挂在 TUI 上）同理。
      mk({ session: { id: 2, project_id: 1, cc_session_id: "b", status: "running", started_at: 0, last_event_at: 0, ended_at: null }, screen_state: "blocked" }),
    ];
    const { container } = render(<CollapsedStrip data={data} edge="left" onExpand={() => {}} />);
    expect(container.querySelectorAll(".cstrip-pending").length).toBe(2);
    expect(container.querySelectorAll(".cstrip-running").length).toBe(0);
  });

  it("出错会话优先显示红点，即便 status 是 running", () => {
    const data: Item[] = [
      mk({ session: { id: 1, project_id: 1, cc_session_id: "a", status: "running", started_at: 0, last_event_at: 0, ended_at: null }, errored: true }),
    ];
    const { container } = render(<CollapsedStrip data={data} edge="left" onExpand={() => {}} />);
    expect(container.querySelectorAll(".cstrip-error").length).toBe(1);
    expect(container.querySelectorAll(".cstrip-running").length).toBe(0);
  });

  it("disconnected（断开/历史）会话不显示", () => {
    const data: Item[] = [
      mk({ session: { id: 1, project_id: 1, cc_session_id: "a", status: "running", started_at: 0, last_event_at: 0, ended_at: null }, connected: true }),
      mk({ session: { id: 2, project_id: 1, cc_session_id: "b", status: "ended", started_at: 0, last_event_at: 0, ended_at: null }, connected: false }),
    ];
    const { container } = render(<CollapsedStrip data={data} edge="left" onExpand={() => {}} />);
    expect(container.querySelectorAll(".cstrip-dot").length).toBe(1);
  });

  it("归档会话不计入竖条", () => {
    const data: Item[] = [
      mk({ archived: true }),
      mk({ session: { id: 2, project_id: 1, cc_session_id: "b", status: "running", started_at: 0, last_event_at: 0, ended_at: null } }),
    ];
    const { container } = render(<CollapsedStrip data={data} edge="right" onExpand={() => {}} />);
    expect(container.querySelectorAll(".cstrip-dot").length).toBe(1);
  });

  it("edge 决定容器修饰类", () => {
    const { container } = render(<CollapsedStrip data={[]} edge="right" onExpand={() => {}} />);
    expect(container.querySelector(".cstrip-right")).toBeTruthy();
  });

  it("无活跃会话时显示灰色眼睛占位，不显示圆点", () => {
    const { container } = render(<CollapsedStrip data={[]} edge="left" onExpand={() => {}} />);
    expect(container.querySelectorAll(".cstrip-dot").length).toBe(0);
    expect(container.querySelector(".cstrip-empty svg")).toBeTruthy();
    expect(container.querySelector(".cstrip-eyes")).toBeTruthy();
  });

  it("无活跃会话时 onMeasure 上报值不低于最小尺寸 48", () => {
    let measured = 0;
    render(
      <CollapsedStrip data={[]} edge="left" onExpand={() => {}} onMeasure={(h) => (measured = h)} />
    );
    expect(measured).toBeGreaterThanOrEqual(48);
  });

  it("超出主轴上限的会话折成末尾「+N」徽章，不再被无声裁掉（W-5）", () => {
    // 钉死屏幕可用主轴 300px：容纳上限 floor((300-60)/17)=14 点，其中一格让给徽章。
    vi.stubGlobal("screen", { availWidth: 300, availHeight: 300 });
    const data: Item[] = Array.from({ length: 30 }, (_, i) =>
      mk({ session: { id: i + 1, project_id: 1, cc_session_id: `s${i}`, status: "running", started_at: 0, last_event_at: 0, ended_at: null } })
    );
    const { container } = render(<CollapsedStrip data={data} edge="left" onExpand={() => {}} />);
    expect(container.querySelectorAll(".cstrip-dot").length).toBe(13);
    const badge = container.querySelector(".cstrip-more");
    expect(badge?.textContent).toBe("+17");
    vi.unstubAllGlobals();
  });

  it("会话数没超上限时不渲染「+N」徽章（W-5）", () => {
    const { container } = render(<CollapsedStrip data={[mk(), mk({ session: { id: 2, project_id: 1, cc_session_id: "b", status: "running", started_at: 0, last_event_at: 0, ended_at: null } })]} edge="left" onExpand={() => {}} />);
    expect(container.querySelector(".cstrip-more")).toBeNull();
    expect(container.querySelectorAll(".cstrip-dot").length).toBe(2);
  });
});

import { describe, it, expect } from "vitest";
import { todoProgress } from "./api";

describe("todoProgress", () => {
  it("counts completed over total", () => {
    expect(todoProgress([
      { id: 1, task_id: 1, content: "a", status: "completed", order_idx: 0, stale: false },
      { id: 2, task_id: 1, content: "b", status: "in_progress", order_idx: 1, stale: false },
    ])).toEqual({ done: 1, total: 2, percent: 50 });
  });
  it("zero todos -> 0% and total 0", () => {
    expect(todoProgress([])).toEqual({ done: 0, total: 0, percent: 0 });
  });
  it("all done -> 100%", () => {
    expect(todoProgress([
      { id: 1, task_id: 1, content: "a", status: "completed", order_idx: 0, stale: false },
    ])).toEqual({ done: 1, total: 1, percent: 100 });
  });
});

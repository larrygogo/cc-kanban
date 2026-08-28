// 占位卡（pending PTY，负 id）对账逻辑的单元测试。对账规则见 pending.ts 模块注释。
import { describe, it, expect } from "vitest";
import {
  reconcilePendings,
  normalizeCwdKey,
  PENDING_HOLD_MS,
  type PendingRegistry,
} from "./pending";
import type { Item } from "./types";

let seq = 0;
function mk(over: {
  id: number;
  provider?: string;
  cwd?: string | null;
  started_at?: number;
  archived?: boolean;
}): Item {
  seq += 1;
  return {
    session: {
      id: over.id,
      project_id: 1,
      cc_session_id: `cc-${seq}`,
      status: "running",
      started_at: over.started_at ?? 1_000,
      last_event_at: over.started_at ?? 1_000,
      ended_at: null,
    },
    provider: over.provider ?? "codex",
    cwd: over.cwd === undefined ? "C:\\work\\proj" : over.cwd,
    archived: over.archived ?? false,
    connected: true,
    task_title: over.id < 0 ? "" : `会话 ${over.id}`,
  } as unknown as Item;
}

describe("reconcilePendings", () => {
  it("后端上报的占位项被登记并前置返回（最新启动在前）", () => {
    const reg: PendingRegistry = new Map();
    const old = mk({ id: -1, started_at: 1_000 });
    const fresh = mk({ id: -2, started_at: 2_000 });
    const out = reconcilePendings(reg, [old, fresh], 10_000);
    expect(out.map((i) => i.session.id)).toEqual([-2, -1]);
    expect(reg.size).toBe(2);
  });

  it("同 provider+cwd 的真实会话出现时撤掉占位卡（认领完成）", () => {
    const reg: PendingRegistry = new Map();
    const pending = mk({ id: -1, started_at: 1_000 });
    reconcilePendings(reg, [pending], 10_000);
    // 认领后：占位从后端消失，真实行（同 provider+cwd、started_at 不早于占位）出现。
    const real = mk({ id: 42, started_at: 1_500 });
    const out = reconcilePendings(reg, [real], 11_000);
    expect(out).toEqual([]);
    expect(reg.size).toBe(0);
  });

  it("同 provider+cwd 的旧会话不能冒领：started_at 早于占位启动的不算认领", () => {
    const reg: PendingRegistry = new Map();
    const pending = mk({ id: -1, started_at: 10_000 });
    reconcilePendings(reg, [pending], 10_000);
    // 目录里早就在跑的旧会话还在列表里——占位必须继续展示（新一轮启动还没出真卡）。
    const older = mk({ id: 7, started_at: 1_000 });
    const out = reconcilePendings(reg, [older], 11_000);
    expect(out.map((i) => i.session.id)).toEqual([-1]);
    expect(reg.size).toBe(1);
  });

  it("provider 或 cwd 不符的真实会话不触发撤卡（cwd 按斜杠归一比较）", () => {
    const reg: PendingRegistry = new Map();
    const pending = mk({ id: -1, provider: "codex", cwd: "C:\\work\\proj", started_at: 1_000 });
    reconcilePendings(reg, [pending], 10_000);
    const wrongProvider = mk({ id: 42, provider: "claude", started_at: 1_500 });
    let out = reconcilePendings(reg, [wrongProvider], 11_000);
    expect(out.map((i) => i.session.id)).toEqual([-1]);
    // 另一种斜杠写法 + 尾斜杠 = 同一目录，应触发撤卡。
    const sameDir = mk({ id: 43, provider: "codex", cwd: "c:/work/proj/", started_at: 1_500 });
    out = reconcilePendings(reg, [sameDir], 12_000);
    expect(out).toEqual([]);
  });

  it("后端超过 HOLD 未再上报的占位被撤（启动失败/mergeTail 残留，不留僵尸卡）", () => {
    const reg: PendingRegistry = new Map();
    const pending = mk({ id: -1, started_at: 1_000 });
    const t0 = 100_000;
    reconcilePendings(reg, [pending], t0);
    // HOLD 内后端没再上报（启动失败被摘除、真卡又还没来）：仍短暂保留，兜住竞态窗。
    let out = reconcilePendings(reg, [], t0 + PENDING_HOLD_MS - 1);
    expect(out.map((i) => i.session.id)).toEqual([-1]);
    // 超过 HOLD：撤。
    out = reconcilePendings(reg, [], t0 + PENDING_HOLD_MS + 1);
    expect(out).toEqual([]);
    expect(reg.size).toBe(0);
  });

  it("后端持续上报时 seenAt 续期，HOLD 不触发（codex 首个 turn 前的长启动）", () => {
    const reg: PendingRegistry = new Map();
    const pending = mk({ id: -1, started_at: 1_000 });
    const t0 = 100_000;
    reconcilePendings(reg, [pending], t0);
    const out = reconcilePendings(reg, [pending], t0 + PENDING_HOLD_MS * 10);
    expect(out.map((i) => i.session.id)).toEqual([-1]);
    expect(reg.size).toBe(1);
  });
});

describe("normalizeCwdKey", () => {
  it("反斜杠归一、去尾斜杠、不分大小写；空值为空串", () => {
    expect(normalizeCwdKey("C:\\Work\\Proj\\")).toBe("c:/work/proj");
    expect(normalizeCwdKey("/home/x/")).toBe("/home/x");
    expect(normalizeCwdKey(null)).toBe("");
    expect(normalizeCwdKey(undefined)).toBe("");
  });
});

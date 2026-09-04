import { describe, it, expect } from "vitest";
import { effectiveState, aggregate } from "./state";
import type { RawSession } from "./state";

const now = 1_000_000;

function sess(overrides: Partial<RawSession>): RawSession {
  return { state: "thinking", ts: now, lastTurnLine: "", ...overrides };
}

describe("effectiveState", () => {
  it("工作态未超时保持原状态", () => {
    expect(effectiveState(sess({ state: "thinking" }), now)).toBe("thinking");
    expect(effectiveState(sess({ state: "tool" }), now)).toBe("tool");
  });

  it("工作态超 15 分钟归为 rest", () => {
    expect(effectiveState(sess({ state: "thinking", ts: now - 901 }), now)).toBe("rest");
    expect(effectiveState(sess({ state: "tool", ts: now - 15 * 60 - 1 }), now)).toBe("rest");
  });

  it("permission 未超时保持、超 2 小时归 rest", () => {
    expect(effectiveState(sess({ state: "permission" }), now)).toBe("permission");
    expect(effectiveState(sess({ state: "permission", ts: now - 7201 }), now)).toBe("rest");
  });

  it("Esc 中断标记使工作态归为 rest", () => {
    const s = sess({
      state: "thinking",
      lastTurnLine: '{"type":"user","message":{"content":"interrupted by user"}}',
    });
    expect(effectiveState(s, now)).toBe("rest");
  });

  it("done / idle / 未知状态一律 rest", () => {
    expect(effectiveState(sess({ state: "done" }), now)).toBe("rest");
    expect(effectiveState(sess({ state: "idle" }), now)).toBe("rest");
    expect(effectiveState(sess({ state: "weird" }), now)).toBe("rest");
  });
});

describe("aggregate", () => {
  it("空列表为 rest", () => {
    expect(aggregate([], now)).toBe("rest");
  });

  it("仅工作态为 walking", () => {
    expect(aggregate([sess({ state: "thinking" }), sess({ state: "done" })], now)).toBe("walking");
  });

  it("permission 优先级最高，掩盖 working", () => {
    expect(aggregate([sess({ state: "tool" }), sess({ state: "permission" })], now)).toBe("alert");
  });

  it("全部休息为 rest", () => {
    expect(aggregate([sess({ state: "done" }), sess({ state: "idle" })], now)).toBe("rest");
  });
});

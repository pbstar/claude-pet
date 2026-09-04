// 纯函数：状态解析、超时兜底、Esc 中断检测、多会话聚合。与 UI 无关，用 vitest 单测。
export type RawSession = {
  state: string;
  ts: number;
  lastTurnLine: string;
};

export type DisplayState = "walking" | "alert" | "rest";

// 超时兜底（秒）：hook 进程被强杀时，状态不会永久冻结
const WORKING_TIMEOUT = 15 * 60; // thinking/tool 超 15 分钟 → rest
const PERMISSION_TIMEOUT = 2 * 60 * 60; // permission 超 2 小时 → rest
const INTERRUPT_MARKER = "interrupted by user";

export function effectiveState(s: RawSession, now: number): string {
  if (s.state === "thinking" || s.state === "tool") {
    if (now - s.ts > WORKING_TIMEOUT) return "rest";
    if (s.lastTurnLine.includes(INTERRUPT_MARKER)) return "rest";
    return s.state;
  }
  if (s.state === "permission") {
    return now - s.ts > PERMISSION_TIMEOUT ? "rest" : "permission";
  }
  return "rest"; // done / idle / 其他未知值一律休息
}

// 聚合：任意 permission → alert；否则任意 working → walking；否则 rest
export function aggregate(sessions: RawSession[], now: number): DisplayState {
  let hasWorking = false;
  for (const s of sessions) {
    const eff = effectiveState(s, now);
    if (eff === "permission") return "alert";
    if (eff === "thinking" || eff === "tool") hasWorking = true;
  }
  return hasWorking ? "walking" : "rest";
}

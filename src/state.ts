// 纯函数：状态解析、超时兜底、Esc 中断检测、多会话聚合。与 UI 无关。
export type RawSession = {
  state: string;
  ts: number;
  lastTurnLine: string;
  // transcript jsonl 的 mtime（秒），0 = 读不到。CLI 在沙箱外实时写 transcript，
  // 它是 hook 信号失效（桌面端不触发工具 hook / settings.json 被外部重写清空 hooks）时的兜底依据
  transcriptMtime: number;
};

export type DisplayState = "walking" | "alert" | "rest";

// 超时兜底（秒）：hook 进程被强杀时，状态不会永久冻结
const WORKING_TIMEOUT = 15 * 60; // thinking/tool 超 15 分钟且 transcript 不再刷新 → rest
const PERMISSION_TIMEOUT = 2 * 60 * 60; // permission 超 2 小时 → rest
const TRANSCRIPT_LIVE = 120; // transcript 最后写入距今 ≤120s 视为会话仍在推进
const INTERRUPT_MARKER = "interrupted by user";
// 一轮答完的尾部标志：assistant 消息 stop_reason 为 end_turn/stop_sequence
const TURN_DONE_RE = /"stop_reason":"(?:end_turn|stop_sequence)"/;

function transcriptLive(s: RawSession, now: number): boolean {
  return s.transcriptMtime > 0 && now - s.transcriptMtime <= TRANSCRIPT_LIVE;
}

// transcript 尾部是否停在"本轮已答完"的 assistant 消息上
function turnDone(s: RawSession): boolean {
  return s.lastTurnLine.includes('"type":"assistant"') && TURN_DONE_RE.test(s.lastTurnLine);
}

export function effectiveState(s: RawSession, now: number): string {
  if (s.state === "thinking" || s.state === "tool") {
    if (s.lastTurnLine.includes(INTERRUPT_MARKER)) return "rest";
    // Stop hook 没落盘（hooks 失效）但 CLI 已答完本轮 → 休息
    if (turnDone(s)) return "rest";
    // hook 冻结但 transcript 仍在刷新 → 工作中（覆盖 15 分钟超时，长任务不误判休息）
    if (transcriptLive(s, now)) return s.state;
    if (now - s.ts > WORKING_TIMEOUT) return "rest";
    return s.state;
  }
  if (s.state === "permission") {
    return now - s.ts > PERMISSION_TIMEOUT ? "rest" : "permission";
  }
  // done/idle/未知：hooks 死后用户又发了新消息（transcript 比状态文件新、未答完、仍在刷新）
  // → 复活为工作中。hooks 在位时 Stop 必晚于 transcript 写入（ts >= mtime），不会误触发
  if (s.transcriptMtime > s.ts && !turnDone(s) && transcriptLive(s, now)) return "thinking";
  return "rest";
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

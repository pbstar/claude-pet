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

// transcript 尾部是否已停在终局：本轮答完，或被用户打断
function terminal(s: RawSession): boolean {
  return s.lastTurnLine.includes(INTERRUPT_MARKER) || turnDone(s);
}

export function effectiveState(s: RawSession, now: number): string {
  // hook 之后 transcript 是否有推进。严格大于：同秒无法判定先后，不能当成「绕过 hook 信号」的
  // 依据（发送窗口里尾部还是上一轮内容，会被误判成休息）
  const advanced = s.transcriptMtime > s.ts;
  // 唯一的例外：状态是 permission、且尾部已停在终局。用户按 Esc 打断一个待授权工具时，打断标记
  // 与权限写入经常落在同一秒（mtime == ts），严格大于的判据会把解冻路彻底挡死——黄灯一直举到 2h
  // 超时。此时尾部的中断/答完标记是「这次授权已终结」的确凿证据，足以解冻
  const evidence =
    advanced || (s.state === "permission" && s.ts > 0 && s.transcriptMtime === s.ts && terminal(s));

  // permission 解冻：批准后 transcript 必然继续写入，或尾部已见终局标记。桌面端 hook 信号可能
  // 迟到、transcript 也按批落盘，冻结期间不做解冻会把黄灯挂满整个 2h
  const st = s.state === "permission" && evidence ? "thinking" : s.state;

  if (st === "thinking" || st === "tool") {
    // transcript 兜底仅在 hook 信号已过期（transcript 已越过状态文件）时生效。
    // Sending/Waiting 阶段尾部仍是上一轮内容（end_turn/中断标记），而 hook 刚落盘的
    // thinking 是最新事实，不能被旧尾部推翻（否则螃蟹在发送窗口误判休息、定住不动）
    if (evidence) {
      if (terminal(s)) return "rest";
      // hook 冻结但 transcript 仍在刷新 → 工作中（覆盖 15 分钟超时，长任务不误判休息）
      if (transcriptLive(s, now)) return st;
    }
    if (now - s.ts > WORKING_TIMEOUT) return "rest";
    return st;
  }
  if (st === "permission") {
    return now - s.ts > PERMISSION_TIMEOUT ? "rest" : "permission";
  }
  // done/idle/未知：hooks 死后用户又发了新消息（transcript 比状态文件新、未到终局、仍在刷新）
  // → 复活为工作中。hooks 在位时 Stop 必晚于 transcript 写入（ts >= mtime），不会误触发
  if (advanced && !terminal(s) && transcriptLive(s, now)) return "thinking";
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

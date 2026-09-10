import "./style.css";
import { aggregate } from "./state";
import type { DisplayState } from "./state";
import { fetchSessions } from "./poller";
import { render } from "./renderer";
import { restorePosition, trackPosition } from "./window-pos";
import "./menu";

const POLL_INTERVAL_MS = 1000;
// 离开 alert 需连续两拍确认：hook 信号偶发抖动时，避免黄灯一闪而过又跳回走动
const ALERT_CLEAR_TICKS = 2;

let shown: DisplayState | null = null;
let clearTicks = 0;

// 报警迟滞：进入 alert 立即生效（宁可早报），离开 alert 需连续确认（避免闪烁）
function apply(next: DisplayState): void {
  if (next === shown) {
    clearTicks = 0;
    return;
  }
  if (shown === "alert") {
    clearTicks += 1;
    if (clearTicks < ALERT_CLEAR_TICKS) return;
  }
  clearTicks = 0;
  shown = next;
  render(next);
}

async function tick(): Promise<void> {
  try {
    const sessions = await fetchSessions();
    const now = Math.floor(Date.now() / 1000);
    apply(aggregate(sessions, now));
  } catch (e) {
    console.error("poll failed:", e);
    apply("rest");
  }
}

// 自调度而非 setInterval：上一轮没回来就不再叠加下一轮，避免慢 IPC 时并发堆积
async function loop(): Promise<void> {
  await tick();
  setTimeout(() => void loop(), POLL_INTERVAL_MS);
}

async function boot(): Promise<void> {
  await restorePosition();
  void trackPosition();
  void loop();
}

void boot();

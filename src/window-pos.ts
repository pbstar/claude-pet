// 窗口位置记忆：localStorage 存坐标，启动还原，移动时落盘
import { getCurrentWindow } from "@tauri-apps/api/window";
import { PhysicalPosition } from "@tauri-apps/api/dpi";

const POS_KEY = "claude-pet:window-pos";
const OFF_SCREEN_GUARD = -10000; // 明显出屏的坐标视为无效（显示器拔掉等场景）
const SAVE_DEBOUNCE_MS = 300; // 拖动时 moved 事件密集，去抖后再落盘

type Pos = { x: number; y: number };

export async function restorePosition(): Promise<void> {
  const raw = localStorage.getItem(POS_KEY);
  if (!raw) return;
  let pos: Pos;
  try {
    pos = JSON.parse(raw) as Pos;
  } catch {
    return;
  }
  if (!Number.isFinite(pos.x) || !Number.isFinite(pos.y)) return;
  if (pos.x < OFF_SCREEN_GUARD || pos.y < OFF_SCREEN_GUARD) return;
  try {
    await getCurrentWindow().setPosition(new PhysicalPosition(pos.x, pos.y));
  } catch (e) {
    console.error("restore position failed:", e);
  }
}

// 事件驱动落盘：原先是在轮询 tick 里每秒调一次 outerPosition()，窗口不动也白付一次 IPC；
// 改为监听 moved，只在真正移动时写，拖动过程中再去抖
export async function trackPosition(): Promise<void> {
  let timer: number | null = null;
  try {
    await getCurrentWindow().onMoved(({ payload }) => {
      if (timer !== null) clearTimeout(timer);
      timer = window.setTimeout(() => {
        timer = null;
        try {
          localStorage.setItem(POS_KEY, JSON.stringify({ x: payload.x, y: payload.y }));
        } catch (e) {
          console.error("remember position failed:", e);
        }
      }, SAVE_DEBOUNCE_MS);
    });
  } catch (e) {
    console.error("track position failed:", e);
  }
}

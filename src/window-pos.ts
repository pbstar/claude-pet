// 窗口位置记忆：localStorage 存坐标，启动还原，运行中跟随变化保存
import { getCurrentWindow } from "@tauri-apps/api/window";
import { PhysicalPosition } from "@tauri-apps/api/dpi";

const POS_KEY = "claude-pet:window-pos";
const OFF_SCREEN_GUARD = -10000; // 明显出屏的坐标视为无效（显示器拔掉等场景）

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

let last: Pos | null = null;

export async function rememberPosition(): Promise<void> {
  try {
    const p = await getCurrentWindow().outerPosition();
    const cur: Pos = { x: p.x, y: p.y };
    if (last && last.x === cur.x && last.y === cur.y) return;
    last = cur;
    localStorage.setItem(POS_KEY, JSON.stringify(cur));
  } catch (e) {
    console.error("remember position failed:", e);
  }
}

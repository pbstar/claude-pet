// 三态渲染：walking 播帧动画，alert 定住 + ❗徽标，rest 静止半透明
import type { DisplayState } from "./state";

const FRAME_COUNT = 20;
const FRAME_INTERVAL_MS = 80; // 12.5 fps，与源 GIF 一致

const crabEl = document.getElementById("crab") as HTMLDivElement;
const badgeEl = document.getElementById("badge") as HTMLDivElement;

let current: DisplayState | null = null;
let frame = 0;
let animTimer: number | null = null;

// 预加载全部帧：否则首次进入 walking 时逐帧懒加载，第一轮动画会卡顿、闪一下
for (let i = 0; i < FRAME_COUNT; i += 1) {
  const img = new Image();
  img.src = `/crab/${i}.png`;
}

function setFrame(i: number): void {
  crabEl.style.backgroundImage = `url(/crab/${i}.png)`;
}

function stopAnim(): void {
  if (animTimer !== null) {
    clearInterval(animTimer);
    animTimer = null;
  }
  frame = 0;
}

export function render(state: DisplayState): void {
  if (state === current) return;
  current = state;

  if (state === "walking") {
    badgeEl.style.display = "none";
    crabEl.style.opacity = "1";
    if (animTimer === null) {
      animTimer = window.setInterval(() => {
        frame = (frame + 1) % FRAME_COUNT;
        setFrame(frame);
      }, FRAME_INTERVAL_MS);
    }
  } else if (state === "alert") {
    badgeEl.style.display = "block";
    crabEl.style.opacity = "1";
    stopAnim();
    setFrame(0);
  } else {
    badgeEl.style.display = "none";
    crabEl.style.opacity = "0.7";
    stopAnim();
    setFrame(0);
  }
}

// 右键菜单：退出应用
import { invoke } from "@tauri-apps/api/core";

const menu = document.getElementById("menu") as HTMLDivElement;
const quitBtn = document.getElementById("menu-quit") as HTMLDivElement;

document.addEventListener("contextmenu", (e) => {
  e.preventDefault();
  menu.hidden = false;
  const rect = menu.getBoundingClientRect();
  // 防止菜单超出窗口右/下边界
  let x = e.clientX;
  let y = e.clientY;
  if (x + rect.width > window.innerWidth) x = window.innerWidth - rect.width - 4;
  if (y + rect.height > window.innerHeight) y = window.innerHeight - rect.height - 4;
  menu.style.left = `${x}px`;
  menu.style.top = `${y}px`;
});

document.addEventListener("click", (e) => {
  if (!menu.contains(e.target as Node)) menu.hidden = true;
});

quitBtn.addEventListener("click", () => {
  void invoke("quit");
});

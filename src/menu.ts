// 右键菜单：原生系统菜单（muda）。原生菜单由 OS 绘制，不受窗口 112×80 边界裁剪，
// 可从任意位置弹出并支持子菜单/勾选项，扩展菜单时直接往 items 数组加项即可
import { Menu, MenuItem } from "@tauri-apps/api/menu";
import { invoke } from "@tauri-apps/api/core";

async function buildMenu(): Promise<Menu> {
  const quit = await MenuItem.new({
    id: "quit",
    text: "退出 ClaudePet",
    action: () => {
      void invoke("quit");
    },
  });
  return Menu.new({ items: [quit] });
}

// 菜单内容静态，懒构建一次后复用，避免每次右键重复创建 OS 资源
let menuPromise: Promise<Menu> | null = null;

document.addEventListener("contextmenu", (e) => {
  e.preventDefault();
  menuPromise ??= buildMenu();
  // 不传位置：popup 默认弹出在当前鼠标处，即用户右键的位置
  void menuPromise.then((m) => m.popup());
});

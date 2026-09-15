// 唤起 Claude Desktop：双击桌宠、或走右键菜单触发。
// 真正干活的是 Rust 侧（WebView 里既没有 Node，也没有 shell 权限）
import { invoke } from "@tauri-apps/api/core";

export function openClaudeDesktop(): void {
  // 意图忽略失败：没装 Claude Desktop 时静默无动作，不该影响桌宠本身
  void invoke("focus_desktop_app").catch((e) => {
    console.error("focus desktop app failed:", e);
  });
}

// 绑在 document 而非 #crab：#badge（黄灯徽标）叠在窗口右上角，绑元素会让那块区域的双击失效。
// 双击与本窗口的拖动不冲突——Tauri 的 drag region 脚本在 macOS 上遇到双击只走
// internal_toggle_maximize，而该命令对 resizable: false 的窗口是 no-op
export function bindOpenClaude(): void {
  document.addEventListener("dblclick", openClaudeDesktop);
}

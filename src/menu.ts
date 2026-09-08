// 右键菜单：原生系统菜单（muda）。原生菜单由 OS 绘制，不受窗口 112×80 边界裁剪，
// 可从任意位置弹出并支持子菜单/勾选项，扩展菜单时直接往 items 数组加项即可
// 模型条目每次右键动态重建（七）：✓ 标记 active，点击即切换
import {
  Menu,
  MenuItem,
  CheckMenuItem,
  IconMenuItem,
  NativeIcon,
  PredefinedMenuItem,
} from "@tauri-apps/api/menu";
import { invoke } from "@tauri-apps/api/core";

type ModelDto = {
  id: string;
  name: string;
  active: boolean;
};

async function buildMenu(): Promise<Menu> {
  const items: (MenuItem | CheckMenuItem | IconMenuItem | PredefinedMenuItem)[] = [];

  // 状态项：IconMenuItem + 系统原生状态圆点（绿=运行中，灰=未启动），
  const [running, proxyAddr] = await Promise.all([
    invoke<boolean>("proxy_status").catch(() => false),
    invoke<string>("proxy_addr").catch(() => ""),
  ]);
  const port = proxyAddr.split(":").pop();
  items.push(
    await IconMenuItem.new({
      id: "proxy-status",
      text: running ? `代理运行中${port ? ` · ${port}` : ""}` : "代理未启动",
      icon: running ? NativeIcon.StatusAvailable : NativeIcon.StatusNone,
      enabled: false,
    })
  );
  if (!running) {
    items.push(
      await MenuItem.new({
        id: "proxy-retry",
        text: "重试启动代理",
        action: () => void invoke("retry_proxy"),
      })
    );
  }
  items.push(await PredefinedMenuItem.new({ item: "Separator" }));

  // 模型条目：CheckMenuItem 显示 ✓ active，点击即切换
  const models = await invoke<ModelDto[]>("list_models").catch(() => []);
  if (models.length === 0) {
    items.push(
      await MenuItem.new({
        id: "no-models",
        text: "（暂无模型条目，请先在模型管理中添加）",
        enabled: false,
      })
    );
  }
  for (const m of models) {
    items.push(
      await CheckMenuItem.new({
        id: `model-${m.id}`,
        text: m.name,
        checked: m.active,
        action: () => {
          // 只改 models.json 的 activeId（第五节）；下次右键重建菜单刷新 ✓
          void invoke("switch_model", { id: m.id });
        },
      })
    );
  }
  items.push(await PredefinedMenuItem.new({ item: "Separator" }));

  items.push(
    await MenuItem.new({
      id: "manage",
      text: "模型管理…",
      action: () => void invoke("open_manager"),
    })
  );

  items.push(
    await MenuItem.new({
      id: "quit",
      text: "退出 ClaudePet",
      action: () => {
        void invoke("quit");
      },
    })
  );

  return Menu.new({ items });
}

// 模型条目/active 随操作变化，每次右键重建（毫秒级，不再懒构建复用）
document.addEventListener("contextmenu", (e) => {
  e.preventDefault();
  // 不传位置：popup 默认弹出在当前鼠标处，即用户右键的位置
  void buildMenu().then((m) => m.popup());
});

import { defineConfig } from "vite";
import { resolve } from "node:path";

const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  clearScreen: false,
  // 多页入口：index.html（桌宠主窗）+ manager.html（模型管理小窗）
  build: {
    rollupOptions: {
      input: {
        main: resolve(__dirname, "index.html"),
        manager: resolve(__dirname, "manager.html"),
      },
    },
  },
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
  },
});

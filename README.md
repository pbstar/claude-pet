# Claude Pet 🦀

Claude Code 桌面指示灯（桌宠）：一只悬浮在桌面上的像素小螃蟹，Claude 工作时它横着走，等你授权时定住举 ❗，空闲时趴下变半透明。

灵感来自 [claude-status-bar](https://github.com/m1ckc3s/claude-status-bar)——菜单栏图标只有 18pt 不显眼，这个项目把它放大成桌面上的一块活物。

## 特性

- **悬浮固定**：无边框、透明、置顶、所有桌面空间可见，日常操作不打扰
- **三态指示**：
  - 🚶 走路 —— Claude 正在思考 / 调用工具（20 帧 @12.5fps）
  - ⚠️ 举 ❗ —— 等待你的权限确认
  - 😴 趴下 —— 空闲 / 完成（半透明）
- **轻量**：Tauri v2 + 系统 WKWebView，不捆绑 Chromium；产物 ~9.6MB
- **自带数据源**：极简 shell hooks（不依赖 claude-status-bar，可独立工作）
- **位置记忆**：拖动位置自动记住，重启还原
- **右键退出**：右键菜单退出应用

## 技术栈

| 层 | 选型 |
|---|---|
| 桌面壳 | Tauri v2（Rust 仅作编译壳） |
| 应用逻辑 | TypeScript（状态机、轮询、渲染） |
| 数据源 | 纯 shell hooks（`~/.claude/claude-pet/`） |

## 构建与安装

### 前置

- Node.js ≥ 18 + pnpm
- Rust（[rustup](https://rustup.rs/) 安装 stable 工具链）
- macOS 13+

### 开发

```bash
pnpm install
pnpm dev
```

### 构建 `.app`

```bash
pnpm build
```

产物在 `src-tauri/target/release/bundle/macos/ClaudePet.app`。hooks 脚本内嵌在二进制中（`src-tauri/hook.sh` 通过 `include_str!` 编译进去），无需任何额外步骤。

### 安装与启动

1. 把构建出的 `ClaudePet.app` 拖进「应用程序」（Applications）
2. 双击启动——首次启动会**自动安装 hooks**（写 `~/.claude/claude-pet/` + 合并 `settings.json`，并自动备份）
3. 启动后桌面上出现一只螃蟹，开启任意 Claude Code 会话即可看到状态变化

> 提示：本应用无 Dock 图标、无菜单栏图标，只在桌面上悬浮。退出方式：右键螃蟹 → 退出。

## 工作原理

```
Claude Code hooks（shell）                ClaudePet.app
UserPromptSubmit/PostToolUse → thinking
PreToolUse                  → tool       → 1s 轮询 read_sessions
Notification/PermissionRequest → permission     ├─ state.ts  纯函数 FSM
Stop                        → done       │  ├─ poller.ts
SessionEnd                  → 清理       │  └─ renderer.ts
        ↓ 原子写                            ↓
~/.claude/claude-pet/<session_id>.json   三态渲染（走/❗/趴）
```

- hooks 用「tmp + rename」原子写，读不到半截 JSON
- 超时兜底：working 超 15 分钟、permission 超 2 小时自动归为休息（hook 进程被强杀时不冻结）
- 聚合优先级：任意 `permission` > 任意 `working` > `rest`（等授权的会话永不被工作中掩盖）

## 目录结构

```
src/               # TS 业务逻辑
  state.ts         # 纯函数：解析/超时/聚合
  poller.ts        # 轮询调度
  renderer.ts      # 三态渲染
  main.ts          # 装配
public/crab/       # 20 帧螃蟹素材（自 claude-status-bar 导出）
src-tauri/         # Rust 壳 + tauri.conf.json + Info.plist + hook.sh（内嵌）
```

## 卸载

```bash
rm -rf ~/.claude/claude-pet   # 1. 删除状态目录与 hook 脚本
# 2. 删除 ~/.claude/settings.json 里含 ".claude/claude-pet" 的 hook 命令
# 3. 把 ClaudePet.app 拖进废纸篓
```

> 首次自动安装时已备份原 `settings.json` 到 `settings.json.bak-claude-pet`，可用于恢复。

## 版权说明

螃蟹素材来自 claude-status-bar 的 `CrabFrames.swift`（MIT 协议，代码部分）；螃蟹形象是 Anthropic 的 Clawd，仅个人本地使用，未获商标授权。

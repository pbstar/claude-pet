# Claude Pet 🦀

Claude Code 的桌面指示灯 —— 一只悬浮在桌面上的像素小螃蟹。

Claude 干活时它横着走，等你授权时定住举 ❗，闲下来就趴着变半透明。CLI 与 Desktop 的 **Code tab** 都支持。

## 三态

| 状态 | 表现 | 含义 |
|---|---|---|
| 🚶 `walking` | 20 帧循环横走 @12.5fps | 至少有一个会话在思考 / 调用工具 |
| ⚠️ `alert` | 定住不动 + 黄色 ❗ 徽标 | 至少有一个会话在等你授权 |
| 😴 `rest` | 第 1 帧静止，透明度 0.7 | 没有会话在工作 |

多个会话同时存在时按 **`permission` > `working` > `rest`** 聚合：只要有一个会话在等授权，螃蟹就举 ❗，不会被其它会话的工作态掩盖。

## 特性

- **悬浮固定**：无边框、透明、置顶、所有桌面空间可见，不抢焦点、不打扰
- **多会话聚合**：同时开多个 Claude Code 会话，用一个螃蟹反映全局状态
- **轻量**：Tauri v2 + 系统 WKWebView，不捆绑 Chromium，安装包与常驻内存都远小于 Electron
- **自带数据源**：极简 shell hooks，不依赖 Node
- **自愈**：hooks 被外部程序丢弃会自动重装；会话被强杀留下的状态文件会自动回收
- **位置记忆**：拖动位置自动记住，重启还原

## 技术栈

| 层 | 选型 |
|---|---|
| 桌面壳 | Tauri v2（Rust） |
| 应用逻辑 | TypeScript（纯函数 FSM、轮询、渲染） |
| 数据源 | 纯 shell hooks（`~/.claude/claude-pet/`） |

## 安装

### 前置

- macOS
- Node.js ≥ 18 + pnpm
- Rust（[rustup](https://rustup.rs/) 安装 stable 工具链）

### 构建 `.app`

```bash
pnpm install
pnpm build
```

产物在 `src-tauri/target/release/bundle/macos/ClaudePet.app`。

hook 脚本内嵌在二进制里（`src-tauri/hook.sh` 经 `include_str!` 编译进去），构建无需额外步骤。

### 首次启动

1. 把 `ClaudePet.app` 拖进「应用程序」
2. 双击启动 —— 会自动安装 hooks：写 `~/.claude/claude-pet/hook.sh`，并把 9 个事件合并进 `~/.claude/settings.json`（首次会备份原文件为 `settings.json.bak-claude-pet`）
3. 在 Claude Code 里开一个**新会话**，桌面上就会出现螃蟹（已在运行的会话不会热加载 hooks，新会话立即生效）

> 应用无 Dock 图标、无菜单栏图标，只悬浮在桌面上。**退出方式：右键螃蟹 → 退出 ClaudePet。**
> 若被 Gatekeeper 拦截，右键 `ClaudePet.app` → 打开。

## 工作原理

```
Claude Code hooks（shell）
  SessionStart                      → idle
  UserPromptSubmit / PostToolUse    → thinking
  PreToolUse                        → tool
  PreCompact                        → thinking
  Notification / PermissionRequest  → permission
  Stop                              → done
  SessionEnd                        → 清理
        │
        │ 每个会话原子写一个文件（tmp + rename，带上会话进程 pid）
        ▼
  ~/.claude/claude-pet/<session_id>.json
        │
        │ ClaudePet.app 每秒轮询 read_sessions
        ▼
  state.ts（纯函数 FSM）→ renderer.ts（三态渲染：走 / ❗ / 趴）
```

每个会话一个状态文件，`<uuid>.json` 即一个会话。hooks 用「tmp + rename」原子写，读端不会看到半截 JSON；Rust 侧每秒读一次目录，聚合与超时判定都在 TS 的纯函数 `state.ts` 里完成。

### 健壮性设计

| 场景 | 兜底机制 |
|---|---|
| 会话被强杀 / 关终端 / 崩溃（`SessionEnd` 不触发） | 状态文件记 `$PPID`（即该会话的 claude 进程），每秒 `kill(pid,0)` 判存活，进程消失即删文件；工作 / 等授权态需超 2h 才回收，避免误删正在显示的会话 |
| hook 进程被强杀 | working 超 15 分钟、permission 超 2 小时自动归为休息 |
| Code tab 不触发工具类 hook / hook 写盘被沙箱拦截 | transcript 活跃度兜底：jsonl 的 mtime ≤120s 视为会话仍在推进；尾部出现 `stop_reason:end_turn` 则归为休息 |
| 权限批准后状态冻结 | permission 态补读 transcript mtime，`mtime > ts` 即解冻 |
| `settings.json` 被外部程序整键丢弃 hooks | 每 5s 节流校验一次，9 个事件里任一缺失即自动重装 |
| `settings.json` 被外部程序非原子重写，读到半截 JSON | 解析失败即放弃本次写入（绝不按空对象写回，否则会抹掉用户的 `env` / `model` / `permissions`），留待 5s 后重试；自身写入走 tmp + rename |
| hook 的 stdin 不关闭 | 分片读 + 单次 1s 超时，最多等 1s 就放弃，绝不拖住会话 |
| 事件 payload 缺 `transcript_path` | 保留状态文件里已有的 transcript，不覆盖 |
| 工具入参里嵌套同名 `session_id` | 字段只取首个匹配，不会串号 |

## 开发

```bash
pnpm dev      # tauri dev：热重载前端 + Rust 壳
pnpm build    # 出 release .app
```

改 `src-tauri/hook.sh` 后需要重启应用才会把新脚本写进 `~/.claude/claude-pet/`（启动时刷新，幂等）。

```bash
cd src-tauri && cargo test    # 回收判据 / hooks 自愈判据的单元测试
```

## 排错

| 现象 | 排查 |
|---|---|
| 螃蟹一直趴着 | 确认 Claude Code 里有**新开**的会话（已运行的会话不热加载 hooks）；看 `ls ~/.claude/claude-pet/` 有没有 `<uuid>.json`；重启应用会重装 hooks |
| 一直举 ❗ 不消失 | 授权后需要 transcript 有写入才会解冻，最多 2h 自动归位。若 transcript 路径读不到（如被沙箱挡住），会退化成等超时 |
| 动画偶尔抖动 | 权限待批准期间会丢弃并发的工具态写入，这是刻意的抗抖动设计 |
| 想完全重置 | `rm -rf ~/.claude/claude-pet` 后重启应用 |

## 目录结构

```
src/                      # TS 业务逻辑
  state.ts                # 纯函数：解析 / 超时兜底 / Esc 中断检测 / 多会话聚合
  poller.ts               # 轮询调度（调用 Rust 侧 read_sessions）
  renderer.ts             # 三态渲染（帧动画 / ❗徽标 / 半透明）
  window-pos.ts           # 窗口位置记忆（localStorage，moved 事件驱动）
  menu.ts                 # 右键菜单（退出）
  main.ts                 # 装配
  style.css               # 主窗样式
public/crab/              # 20 帧螃蟹素材（自 claude-status-bar 导出）
src-tauri/
  hook.sh                 # 状态 hook（纯 shell，内嵌进二进制）
  src/main.rs             # read_sessions / quit 命令、hooks 安装与自愈、僵尸会话回收
  tauri.conf.json         # 窗口：112×80 无边框透明置顶
  Info.plist              # LSUIElement：无 Dock 图标
  capabilities/           # 窗口权限（拖动 / 定位 / 读坐标）
```

## 已知限制

- **仅 macOS**：`Info.plist` 与 `LSUIElement` 均为 macOS 形态，Windows / Linux 未适配
- 应用未做代码签名，自行构建本地运行没问题，对外分发会被 Gatekeeper 拦

## 卸载

```bash
rm -rf ~/.claude/claude-pet          # 状态目录与 hook 脚本
```

再编辑 `~/.claude/settings.json`，删掉 hooks 里 command 含 `.claude/claude-pet` 的条目。

> 首次自动安装时备份了原文件到 `settings.json.bak-claude-pet`，可用于恢复。

## 版权与致谢

- 灵感来自 [claude-status-bar](https://github.com/m1ckc3s/claude-status-bar) —— 菜单栏图标只有 18pt 不显眼，这个项目把它放大成桌面上的一块活物
- 螃蟹素材来自 claude-status-bar 的 `CrabFrames.swift`（MIT 协议，代码部分）；螃蟹形象是 Anthropic 的 Clawd，仅个人本地使用，未获商标授权

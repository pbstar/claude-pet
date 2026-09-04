# Claude Pet · Claude Code 桌面指示灯 — 方案设计（v3 · 已定稿）

> 状态：待评审 ｜ 日期：2026-09-04 ｜ 参考：[claude-status-bar](https://github.com/m1ckc3s/claude-status-bar)（源码已通读，有取舍地借鉴）

## 1. 背景与目标

目前在用 claude-status-bar（菜单栏指示灯），问题：**菜单栏图标只有 18pt，不显眼、不易观察**。

目标：做一个**简化版桌面指示灯（桌宠）**——

- 悬浮固定在桌面上，所有桌面空间可见，日常操作零打扰
- 动画用 crab walking（像素小螃蟹：工作时横着走，休息时趴着）
- 功能克制：只做「状态指示」一件事
- 技术原则：**轻量、先进、TS 优先**，零冗余依赖

**非目标**（明确不做）：菜单栏入口、会话列表 UI、多种动画风格、计时器文字、音效、更新检查、遥测。

**关键约束：claude-status-bar 将被卸载**——pet 必须自带数据源，不得依赖其 `state.d` 状态目录。

## 2. 借鉴与摒弃（对 claude-status-bar 的取舍）

原项目架构：hooks 把每个会话状态**原子写**到状态文件，app 只读轮询聚合。「hooks → 状态文件 → 只读消费者」这个骨架经过验证，值得保留；实现细节有扬弃。

**✅ 借鉴**

| 机制 | 为什么好 |
|---|---|
| hooks → 状态文件 → 只读消费者 | 解耦彻底：hook 挂了 pet 只是退化为休息态，pet 挂了不影响 Claude Code |
| 原子写（tmp + rename） | 永远读不到半截 JSON |
| 有效状态收敛为 3~4 种 + 优先级聚合 | permission > working > rest：多会话时等授权的永不被工作中的掩盖 |
| transcript 尾部「interrupted by user」检测 | 补上 Esc 中断不触发任何 hook 的盲区（原项目最巧妙的一手） |

**❌ 摒弃**

| 机制 | 为什么不要 |
|---|---|
| node 写 hook（每次工具调用 spawn 一次 node，~50ms） | hook 应该是 shell（~5ms），不给每次工具调用加 node 冷启动 |
| 自启动 / 自退出生命周期机器（quit-intent 标记、pgrep 拉活、宽限期去抖，~150 行） | 桌宠常驻即可：登录项启动 + 手动退出，简单直观 |
| 0.4s 高频轮询 + mtime 缓存层 | 桌宠对延迟不敏感，1s 轮询足够；文件只有几个，无需缓存 |
| pid 判活（kill(pid,0)） | pet 自装 hooks 可用 SessionEnd 清理状态，进程判活是原项目「hooks 不归我管」的补丁，不再是必需（保留超时兜底作保险，见 §4.2） |
| git 分支读取、多会话下拉列表、hover 高亮、更新检查、音效、图标缓存 | 无列表 UI，全部不需要 |
| 素材内嵌为源码字符串（20 帧 base64 塞进 .swift） | web 项目里素材就是文件，导出成 PNG 即可 |

## 3. 选型

### 3.1 数据源（因「将卸载 status-bar」改为主方案）

**自装极简 shell hooks，写自有目录 `~/.claude/claude-pet/`**，pet 只读消费。设计见附录 B，三个要点：

1. **纯 shell**（~10 行）：8 个 hook 事件只产生 4 个状态（thinking / tool / permission / done），每次工具调用 spawn 开销 ~5ms。
2. **原子写 + 双文件分工**：`state.json`（最新状态）+ `pulse`（触控时间戳），写入永不撕裂，新鲜度判断不依赖字段。
3. **SessionEnd 清理 + 超时兜底**：正常退出删状态文件；hook 被强杀等异常由 pet 的超时逻辑兜底。

与方案 A（只读 state.d）相比：多一次一次性安装（往 settings.json 合并 hooks，原项目 install.js 已验证可行），换来零软依赖与更小的运行时开销。**因卸载约束，此项已定。**

### 3.2 技术栈

| 候选 | 包体积 | 常驻内存 | 逻辑语言 | 关键问题 |
|---|---|---|---|---|
| **Tauri v2**（推荐） | ~3–6MB | ~40–70MB | **100% TS** | 构建需 Rust 工具链（一次性）；点击窗口会激活应用（见 §4.3 焦点行为） |
| Electron | ~60–90MB | ~100–150MB | 100% TS | 窗口 API 最全（免激活是文档化行为），但体积/内存与轻量原则直接冲突 |
| Neutralino | ~2MB | ~40–80MB | 100% TS | 零 Rust 工具链，但 mac 透明窗/置顶等成熟度存疑 |
| Swift + AppKit | <1MB | ~20–30MB | Swift | 最轻，但语言不符偏好 |

**推荐 Tauri v2**：应用逻辑 100% 跑在 TS 里，Rust 只是编译期壳（本项目 `src-tauri` 几乎零自定义代码，窗口行为全部走声明式配置 + 官方插件）；用系统 WKWebView 不捆绑 Chromium，符合「轻量先进」；Vite + TS 开发体验（HMR）远好于原生。代价如实说明：构建需装 Rust 工具链，内存下限 ~40MB（webview 多进程的现实，约为原生方案 2 倍）。

渲染层刻意不用框架：一只螃蟹不需要 React，vanilla TS + 原生 CSS 足够。

## 4. 总体设计

### 4.1 架构与模块

```
Claude Code hooks（自装，附录 B）
  UserPromptSubmit/PostToolUse → thinking    PreToolUse → tool
  Notification/PermissionRequest → permission    Stop → done    SessionEnd → 清理
        │  原子写 ~/.claude/claude-pet/（state.json + pulse）
        ▼
ClaudePet.app（Tauri v2，系统 WKWebView，常驻）
  ├─ state.ts    纯函数：解析/新鲜度/FSM（vitest 单测）
  ├─ poller.ts   1s 轮询（tauri-plugin-fs，scope 限定 ~/.claude/claude-pet/**）
  └─ renderer.ts 三态渲染（CSS class 切换）
```

### 4.2 状态判定（FSM）

| 渲染态 | 条件 | 表现 |
|---|---|---|
| 🚶 walking | 有效状态 = thinking / tool | 20 帧 @12.5fps 横走，全不透明 |
| ⚠️ alert | 有效状态 = permission | 定住 + 琥珀色 ❗ 徽标 |
| 😴 rest | 其余（done / idle / 无状态） | 静止第 0 帧，整体 ~70% 透明度 |

单会话修正顺序（都是对「hook 没有下文」的兜底）：

1. **超时兜底**：working 超 15 分钟无新事件 → rest；permission 超 2 小时 → rest（hook 进程被强杀，永远等不到 Stop）。
2. **新鲜度**：`pulse` mtime 超过 5s 视为过期（可选的更强兜底，M1 可不做）。
3. **Esc 中断检测**：读 transcript 尾部 8KB，最后一条 user/assistant 行含 "interrupted by user" → rest。需要 hooks 写入 transcript 路径（附录 B 已含），mtime 先行比对避免每 tick 读盘。

多会话：自装 hooks 为多会话各写一份状态（文件名含 session_id），聚合规则同原项目——**任意 permission → alert；否则任意 working → walking；否则 rest**。

### 4.3 窗口与渲染

| 项 | Tauri v2 实现 |
|---|---|
| 透明无边框 | 配置 `transparent: true` + `decorations: false` + `hasShadow: false` |
| 悬浮置顶 | 配置 `alwaysOnTop: true` |
| 全空间可见 | `setVisibleOnAllWorkspaces(true)`；全屏应用旁可见性 M1 实测 |
| 焦点行为 | ⚠️ 点击会激活应用（Tauri 无 NSPanel 免激活等价物）。M1 花 10 分钟实测 `focusable` 配置在 macOS 的实际效果；若无效则接受「点击才聚焦」（低频操作），仍不满意再考虑 ~10 行 objc 补丁 |
| 拖动 | `data-tauri-drag-region` 一行搞定 |
| 位置记忆 | localStorage，恢复时钳制到可见屏幕范围 |
| 渲染 | 素材 20 张 51×36 PNG，`<img>` 以 80ms 间隔换帧；`image-rendering: pixelated` 保持像素锐利；放大 3 倍 → 显示区约 153×108 |
| 动画开销 | 帧切换走 GPU 合成；仅 walking 态跑定时器，rest/alert 停掉；静止时 CPU 趋近 0 |
| 交互 | 左键拖动；右键菜单：退出 |

webview 的附带红利：透明度渐变、❗徽标样式、`drop-shadow` 都是几行 CSS，比原生绘图省事。

## 5. 工程结构

```
claude-pet/
├── PLAN.md
├── index.html
├── src/                  # 全部业务逻辑（TS）
│   ├── main.ts           # 装配与启动
│   ├── state.ts          # 纯函数：解析、新鲜度、FSM（vitest 单测）
│   ├── poller.ts         # 轮询调度
│   └── renderer.ts       # 三态渲染
├── public/crab/          # 20 帧素材（0.png … 19.png）
├── tools/export-sprite.mjs   # 从原仓库 CrabFrames.swift 导出 PNG（一次性脚本）
├── tools/hook.sh         # 极简 shell hook（附录 B），安装器一并写入 settings.json
├── src-tauri/            # 壳：tauri.conf.json + 极少量 Rust
├── package.json / vite.config.ts / tsconfig.json
└── README.md             # 使用说明（含 Clawd 素材出处声明）
```

每个 TS 文件 ≤300 行、函数 ≤30 行；`state.ts` 与 UI 无关，用 vitest 覆盖 FSM 边界（超时、中断、聚合优先级、坏 JSON）。

## 6. 实施计划

- **M1 · 能看**：Tauri 脚手架 + 素材导出 + shell hooks 与安装器 + 悬浮窗（透明/置顶/全空间）+ 走/趴两态 + 1s 轮询 + 超时兜底 + **焦点行为实测**。
- **M2 · 用得放心**：❗徽标 + Esc 中断检测 + 拖动与位置记忆 + 右键菜单（退出）+ state.ts 单测。
- **M3 · 可选打磨**：点击穿透开关（`setIgnoreCursorEvents`，顺带解决焦点问题）、尺寸/透明度设置、fs watch 事件驱动（权限弹出即刻响应）。

**验收标准**：发 prompt 后螃蟹开始走；Stop 后一个轮询周期内趴下；弹权限时定住带 ❗；Esc 中断即停；重启后位置不变；卸载 claude-status-bar 后功能不受影响。

## 7. 边界与风险

| 风险 | 对策 |
|---|---|
| hooks 写坏 / 没装上 | pet 读不到有效状态即显示 rest，不会误报工作；安装器幂等可重跑 |
| shell hook 的 JSON 注入面 | hook 不解析 stdin JSON，只写固定状态字符串，无注入面 |
| Tauri 点击窗口会激活应用，可能抢走终端焦点 | M1 实测 focusable 配置；无效则接受（点击频率极低）或加 objc 小补丁；M3 可提供点击穿透模式 |
| 全屏应用旁是否可见待确认 | M1 实测 `setVisibleOnAllWorkspaces` + `alwaysOnTop` 组合，不达标记 issue 调整 |
| 构建需 Rust 工具链 | 一次性安装；只影响构建机，产物是自包含 .app |
| webview 常驻内存 ~40–70MB | 轻量化现实下限（原生 ~20–30MB 的 2 倍）；换取 100% TS 逻辑与开发效率，属已明示的取舍 |
| 多会话 | hooks 按 session_id 各写一份，聚合取最高优先级；单文件「最新赢」不采纳（v3 已改为按会话分文件） |

## 8. 决策记录（已确认）

1. **数据源**：自装 shell hooks（附录 B），写自有目录 `~/.claude/claude-pet/` —— 已确认。
2. **技术栈**：Tauri v2（Rust 工具链正在安装中；应用逻辑 100% TS）—— 已确认。
3. **开机自启**：**不做**。pet 手动启动即可，M2 已移除自启项。
4. **命名**：ClaudePet / bundle id `com.local.claudepet` —— 已确认。

## 附录 · 方案 B（自装 hooks）设计

`~/.claude/claude-pet/hook.sh`（~10 行，纯 shell，不解析 stdin）：

```bash
#!/bin/bash
# usage: hook.sh <thinking|tool|permission|done|clean> <session_id> [transcript_path]
# 事件映射：prompt/post→thinking  pre→tool  notify/permreq→permission
#           stop→done  session-end→clean；hook JSON 从 stdin 传入但本脚本不读
DIR="$HOME/.claude/claude-pet"
mkdir -p "$DIR"
if [ "$1" = "clean" ]; then rm -f "$DIR/$2.json" "$DIR/$2.pulse"; exit 0; fi
TMP="$DIR/$2.json.tmp.$$"
printf '{"state":"%s","ts":%s,"transcript":%s}\n' \
  "$1" "$(date +%s)" "\"${3:-}\"" > "$TMP" && mv -f "$TMP" "$DIR/$2.json"
touch "$DIR/$2.pulse"
```

pet 端：读 `*.json`，以 `ts` 与文件 mtime 做新鲜度；`clean` 对应 SessionEnd 清理。settings.json 合并安装沿用原项目 install.js 的「先剥离再追加」幂等模式（识别标记：命令路径含 `claude-pet`），安装前自动备份。

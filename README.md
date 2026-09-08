# Claude Pet 🦀

Claude Desktop Code tab 的桌面指示灯（桌宠）+ 本地模型代理：一只悬浮在桌面上的像素小螃蟹，Claude 工作时它横着走，等你授权时定住举 ❗，空闲时趴下变半透明；同时内置本地代理（127.0.0.1:15721），右键即可在多条模型线路间一键切换。

## 特性

- **悬浮固定**：无边框、透明、置顶、所有桌面空间可见，日常操作不打扰
- **三态指示**：
  - 🚶 走路 —— Claude 正在思考 / 调用工具（20 帧 @12.5fps）
  - ⚠️ 举 ❗ —— 等待你的权限确认
  - 😴 趴下 —— 空闲 / 完成（半透明）
- **本地模型代理**：
  - 单端口 `127.0.0.1:15721`，仅服务 Code tab（网关前缀 `/claude-desktop/*`）
  - 右键菜单列出模型条目，点击即切换，**下一条消息即时生效**，无需重启任何应用
  - 双格式上游：条目逐个配置 `anthropic`（换头直通）或 `openai`（协议转换，含 SSE 流式翻译）
  - 「模型管理」小窗做条目增删改
- **轻量**：Tauri v2 + 系统 WKWebView，不捆绑 Chromium
- **自带数据源**：极简 shell hooks
- **位置记忆**：拖动位置自动记住，重启还原
- **右键退出**：右键菜单退出应用

## 技术栈

| 层 | 选型 |
|---|---|
| 桌面壳 | Tauri v2（Rust 壳 + axum 本地代理） |
| 应用逻辑 | TypeScript（状态机、轮询、渲染） |
| 数据源 | 纯 shell hooks（`~/.claude/claude-pet/`） |
| 模型配置 | `~/.claude/claude-pet/models.json`（600 权限，原子写） |

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
3. 启动后桌面上出现一只螃蟹，在 Claude Desktop 的 Code tab 里开启任意会话即可看到状态变化

> 提示：本应用无 Dock 图标、无菜单栏图标，只在桌面上悬浮。退出方式：右键螃蟹 → 退出。

## 工作原理

```
Code tab hooks（shell）                  ClaudePet.app
UserPromptSubmit/PostToolUse → thinking
PreToolUse                  → tool       → 1s 轮询 read_sessions
Notification/PermissionRequest → permission     ├─ state.ts  纯函数 FSM
Stop                        → done       │  ├─ poller.ts
SessionEnd                  → 清理       │  └─ renderer.ts
        ↓ 原子写                            ↓
~/.claude/claude-pet/<session_id>.json   三态渲染（走/❗/趴）

Claude Desktop（Code tab）                  上游线路
        │ host-creds 注入 ANTHROPIC_BASE_URL
        ▼
proxy.rs (axum, 127.0.0.1:15721)
  · /claude-desktop/*   ← Code tab（token 校验）
  · 按 active 条目 format 分流：
    anthropic → 换头 + 模型替换后直通
    openai    → Anthropic⇄OpenAI 双向协议翻译（SSE 逐块状态机）
        ↓
~/.claude/claude-pet/models.json 的 active 条目 → 上游端点
```

- hooks 用「tmp + rename」原子写，读不到半截 JSON
- 超时兜底：working 超 15 分钟、permission 超 2 小时自动归为休息（hook 进程被强杀时不冻结）
- transcript 活跃度兜底：CLI 实时写 transcript jsonl，pet 轮询时以它 mtime ≤120s 判会话仍在推进——覆盖 Code tab 不触发工具类 hook、hook 写盘被沙箱拦截等 hooks 信号失效场景；尾部出现 `stop_reason:end_turn` 则即使状态文件还停在 working 也归为休息
- hooks 自愈：外部程序（实测 Claude Desktop 改设置）会整键丢弃 settings.json 里的 hooks，pet 每 5s 节流校验一次，丢失即自动重装
- 聚合优先级：任意 `permission` > 任意 `working` > `rest`（等授权的会话永不被工作中掩盖）
- 代理每请求现读 models.json 取 active 条目：换线路对下一个请求即时生效，跑着的流不断
- 模型替换：claude-* 角色模型名（及 Desktop 选择器写死的 `claude-custom` 哨兵名）固定替换为条目目标模型；`[1m]` 是客户端本地能力标记，转发前一律剥离（上游普遍拒收），仅用于决定是否放行 `context-1m` beta 头（条目 `supports1m` 控制）
- openai 管道兼容处理：流式自动注入 `stream_options.include_usage`（否则上游不回 usage）；`prompt_tokens` 扣除缓存命中再算 `input_tokens`（三桶互斥）；`reasoning_content`/`reasoning` → `thinking` 块；o 系列模型改用 `max_completion_tokens`；thinking 预算 → `reasoning_effort`；system 首行的 `x-anthropic-billing-header` 剥离（其 `cch=` 每次变化会破坏上游前缀缓存）
- Desktop 模型选择器：profile 只注册单条 `claude-custom`（显示 "Claude Custom"），不可切换、无 1M 派生项；实际线路唯一由 pet 右键菜单的 active 条目决定

## 目录结构

```
src/               # TS 业务逻辑
  state.ts         # 纯函数：解析/超时/聚合
  poller.ts        # 轮询调度
  renderer.ts      # 三态渲染
  menu.ts          # 右键菜单（动态模型条目）
  manager.ts       # 模型管理窗逻辑
  main.ts          # 装配
manager.html       # 模型管理小窗（第二个 WebView）
public/crab/       # 20 帧螃蟹素材（自 claude-status-bar 导出）
src-tauri/         # Rust 壳 + tauri.conf.json + Info.plist + hook.sh（内嵌）
  src/proxy.rs     # axum 代理：路由、anthropic 直通管道
  src/convert.rs   # openai 管道：Anthropic⇄OpenAI 请求/响应转换
  src/stream.rs    # openai 管道：SSE 流式翻译状态机 + UTF-8 跨块拼接
  src/models.rs    # models.json 读写
  src/desktop_profile.rs  # Desktop 3p profile 写入与回滚
```

## 添加模型线路

1. 右键螃蟹 → 模型管理 → 填写表单保存。每条条目 = 一条线路（端点 + Token + 目标模型）：
   - **上游格式**：`openai`（Chat Completions 端点，填 OpenAI 根地址）或 `anthropic`（Anthropic Messages 端点，填 `ANTHROPIC_BASE_URL` 形态地址）
   - **模型名必填**：Code tab 发来的 claude-* 角色模型名统一替换为它
   - 保存前可点「测试」：用表单当前值向上游打一发最小请求，验证端点/Token/模型名连通性
2. 右键螃蟹 → 点选条目即切换，下一条消息即时生效，无需重启任何应用

## 卸载

```bash
rm -rf ~/.claude/claude-pet   # 1. 删除状态目录、hook 脚本与 models.json
# 2. 删除 ~/.claude/settings.json 里含 ".claude/claude-pet" 的 hook 命令
# 3. 还原 Claude Desktop：删除 ~/Library/Application Support/Claude-3p/configLibrary/
#    里名为 00000000-0000-4000-8000-000000157210.json 的 profile 与 _meta.json 中对应条目，
#    两份 claude_desktop_config.json 的 deploymentMode 改回 "1p"（如需切回官方登录）
# 4. 把 ClaudePet.app 拖进废纸篓
```

> 首次自动安装时已备份原 `settings.json` 到 `settings.json.bak-claude-pet`，可用于恢复。

## 版权与致谢

- 灵感来自 [claude-status-bar](https://github.com/m1ckc3s/claude-status-bar)——菜单栏图标只有 18pt 不显眼，这个项目把它放大成桌面上的一块活物
- 螃蟹素材来自 claude-status-bar 的 `CrabFrames.swift`（MIT 协议，代码部分）；螃蟹形象是 Anthropic 的 Clawd，仅个人本地使用，未获商标授权
- 本地模型代理的思路参考了 [cc-switch](https://github.com/farion1231/cc-switch)：它验证了本地代理服务 Claude 客户端的可行性，本项目把切换语义简化为一次 activeId 翻转

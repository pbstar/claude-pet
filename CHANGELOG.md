# 更新日志

本项目所有值得注意的变更都记录在此文件。

格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。
安装包版本单一来源为根 `package.json`（`tauri.conf.json` 引用它，安装包版本自动跟随）；`src-tauri/Cargo.toml` 的 crate 版本仅内部元数据，发版时同步。

## [0.1.2] - 2026-09-14

### 修复

- **授权处理完仍举黄灯，最长挂满 2 小时**：`permission` 只有 `SessionEnd` / transcript 推进 / 2h 超时三条出路，而桌面端三条都容易失效——切走会话而不关标签页时 `SessionEnd` 不触发，transcript 又按批落盘（实测可滞后分钟级）。现在：
  - 状态文件记下待授权工具的 `tool_use_id`，该工具自己的后续 `PostToolUse` 一到即解冻，不再依赖 transcript 落盘时机
  - 授权请求被 `Notification` 二次覆盖、payload 不带 id 时，沿用上一份状态里的 `tool_use_id`，避免 id 被抹掉后退化成干等转录
  - Esc 打断与权限写入落在同一秒（`mtime == ts`）时也读 transcript 尾部：中断 / 答完标记即为解冻证据，不再干等 2h
- **用户点「拒绝」没有任何信号**：新增 `PermissionDenied` 事件，拒绝后立即按工作中重新解析（事件数 9 → 10）
- Rust 侧读 transcript 尾部的闸门放宽到同秒（`>=`），支撑上面的同秒解冻；陈旧会话仍不白读 64KB

### 文档

- README 更正：桌面端 Code tab **会**触发工具类 hook（实测 `PreToolUse` / `PostToolUse` / `PermissionRequest` 均触发），transcript 兜底的真实定位是「hooks 被沙箱拦截 / 失效」

## [0.1.1] - 2026-09-10

### 修复

- **`settings.json` 可能被整份覆盖**：原逻辑解析失败时按空对象写回，会把用户的 `env` / `model` / `permissions` 全部抹掉。触发场景真实存在——Claude Desktop 改设置时会非原子重写该文件，此刻读到半截 JSON 即解析失败。现在解析失败直接放弃本次写入、留待 5s 后重试；自身写入改为 tmp + rename 原子替换
- **hook 可能阻塞会话**：`hook.sh` 用 `cat` 读 stdin，stdin 不关闭时会永久阻塞，把整个 Claude 会话拖死。改为分片读 + 单次 1s 超时
- **会话串号**：字段解析用贪婪 sed（`.*"key"...`）会命中最后一个匹配，工具入参里嵌套同名 `session_id` 时写错会话。改为只取首个匹配
- **transcript 被清空导致权限态冻结**：部分事件 payload 不含 `transcript_path`，原逻辑会写成空串，TS 端因此丢掉 mtime 兜底，权限批准后只能干等 2h 超时。现在保留文件里已有的 transcript
- **超长 transcript 行导致判定失效**：`last_turn_line` 只读尾部 8KB，单行超过 8KB 时行首的 `"type":"assistant"` 落在窗口外、`turnDone` 兜底失效。窗口放宽到 64KB
- 无参 / 未知参数调用 `hook.sh` 会写出空状态文件污染状态目录（加参数守卫）
- `session_id` 被直接当作文件名，理论上可路径穿越（改为字符白名单）
- `transcript` 含引号 / 反斜杠会写出非法 JSON 而被 Rust 端静默丢弃（改为弃用该字段）

### 新增

- **僵尸会话回收**：`SessionEnd` 是唯一会删状态文件的 hook，强杀 / 关终端 / 崩溃时不会触发，残留会永久堆积（实测已积 7 个，`read_sessions` 每秒全量扫描越来越慢）。现在 hooks 落盘会话进程 `$PPID`，Rust 侧每秒 `kill(pid, 0)` 判存活，进程消失即回收
- **`SessionStart` / `PreCompact` 事件**：新会话立刻登记（便于回收）并清掉 resume 残留的冻结状态；上下文压缩期间显示为工作中，不再误判休息
- hooks 自愈判据从「任意事件在位」改为「9 个事件全部在位」，否则升级新增的事件永远装不上
- 回收判据与自愈判据的单元测试（`cargo test`）

### 优化

- **位置落盘改事件驱动**：原每轮轮询都调一次 `outerPosition()`，窗口不动也白付一次 IPC；改为监听 `moved` 事件并去抖 300ms
- **预加载 20 帧**：消除首次进入 walking 时的逐帧懒加载卡顿
- **轮询改自调度**：`setInterval` → 自调度 `setTimeout`，慢 IPC 时不再并发叠加
- **`notify` 文案兜底**：`notification_type` 字段整体缺失时（[claude-code#11964](https://github.com/anthropics/claude-code/issues/11964)）退回文案匹配，避免漏报权限提示；字段存在时仍严格按结构化值判定
- **`clean` 纳入锁临界区**：不再与并发的 tool / thinking 写入交错
- 清理死配置：`vite.config.ts` 空 `build` 段、`tsconfig.json` 未使用的 `resolveJsonModule`
- README 重写：补三态说明表、排错表与开发说明

## [0.1.0] - 2026-09-10

### 移除

- **本地模型代理**：`127.0.0.1:15721` HTTP 代理（anthropic 直通 / openai 协议转换）整体移除，CLI 与 Desktop 不再被接管请求通路；`settings.json` 亦不再写入 `ANTHROPIC_BASE_URL` / `ANTHROPIC_AUTH_TOKEN` 等 env
- **模型管理**：右键切换模型、模型管理小窗（增删改、连通性测试）及 `models.json` 配置体系
- **Desktop 3p profile 写入**：不再改写 Claude Desktop 的 `deploymentMode` / `configLibrary`
- Rust 依赖瘦身：移除 axum / reqwest / tokio / futures-util / bytes / uuid

## [0.0.3] - 2026-09-09

### 修复

- **黄灯闪跳**：并行工具/子代理触发的 `PreToolUse`/`PostToolUse` 会覆盖尚未解除的 `permission` 状态，桌宠在 alert 与 walking 之间来回跳。hook 侧加锁串行化「读旧状态 → 判定 → 写新状态」，并在权限待批准期间丢弃工作态写入（判据与 TS 端解冻逻辑一致：transcript 未推进即权限仍未解除）
- `notify` 改按结构化 `notification_type` 判定权限通知；原先全文匹配 `permission/approve/allow`，会被通知文案或路径误命中
- **压缩摘要的内部标签泄漏到正文**：上下文压缩时 Claude Code 要求模型把思考写进 `<analysis>`、总结写进 `<summary>`，官方链路由客户端自己剥离这两个标签；走 openai 转换时原文被当普通 text 块透传，标签直接显示在输出里。新增 `TagSanitizer`，语义与客户端 `ihg()` 一致——丢弃 `analysis` 段、`summary` 只留正文，流式分片切断标签也能正确识别（流式与非流式两条路径均生效）

### 优化

- 报警迟滞：进入 alert 立即生效，离开 alert 需连续两拍确认，吸收残余信号抖动
- `read_sessions` 先取 transcript mtime 再决定是否读尾部，陈旧会话不再每轮白读 8KB

## [0.0.2] - 2026-09-09

### 修复

- openai 协议转换对齐 cc-switch 语义，修复一批流式故障：`[1m]` 后缀误发上游、工具块 start/stop 不配对、UTF-8 跨块截断断流、usage 缺失/重复、thinking 内容丢失等

### 重构

- 流式翻译状态机拆分到 `stream.rs`；清理无用状态包装与重复定义；收窄依赖 features

### 文档

- README 重写：补「已知限制」，分节说明工作原理；修正无依据的系统版本要求

## [0.0.1] - 2026-09-08

### 新增

- **桌宠状态指示**：常驻桌面的螃蟹小窗，由 Claude Code hooks 驱动，实时反映会话状态（空闲 / 思考中 / 执行中 / 等待授权等）
- **本地模型代理**：内置 `127.0.0.1:15721` HTTP 代理，Claude Code 与 Claude Desktop 共用；支持 Anthropic / OpenAI 两种上游协议（OpenAI 走协议转换）
- **模型管理**：右键菜单切换模型 + 独立管理小窗（增删改、连通性测试）
- **原生右键菜单**：系统级菜单，绿点显示代理运行状态，未启动时可重试
- **首启自动化**：自动安装 hooks、生成模型配置、注册 Claude Desktop profile

### 修复

- hooks 信号失效时以 transcript 活跃度兜底，桌宠动画不僵死
- 权限批准后徽标正确解除；Claude Desktop 模型选择器收敛为单条入口

[0.1.2]: https://github.com/pbstar/claude-pet/releases/tag/v0.1.2
[0.1.1]: https://github.com/pbstar/claude-pet/releases/tag/v0.1.1
[0.1.0]: https://github.com/pbstar/claude-pet/releases/tag/v0.1.0
[0.0.3]: https://github.com/pbstar/claude-pet/releases/tag/v0.0.3
[0.0.2]: https://github.com/pbstar/claude-pet/releases/tag/v0.0.2
[0.0.1]: https://github.com/pbstar/claude-pet/releases/tag/v0.0.1
